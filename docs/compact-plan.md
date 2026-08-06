# Miyu Compact 功能优化计划 v1

> 2026-08-06。基于对 DeepSeek-Reasonix、pi、opencode 三个仓库压缩机制的逐行研究 + Claude Code 公开行为，对照 Miyu 现状（`src/agent/compact.rs` / `overflow.rs` / `agent/mod.rs` / `prompts/compact.md`）制定。
> 与 `docs/cache-and-prompt-plan.md`（v7）互补：v7 的"snip/prune 水位线、cache_sticky、last_request_at"未实施项并入本计划 Phase 2/5。

---

## 一、四家实现对比速览

| 维度 | Claude Code | DeepSeek-Reasonix (Go) | pi (TS) | opencode (TS) | Miyu 现状 (Rust) |
|---|---|---|---|---|---|
| 自动触发 | ~92% 窗口 | 四档水位线 0.5/0.6/0.8/0.9，用真实 usage | `ctx > window - 16384`（绝对预留），真实 usage 优先 | `count >= window - max(32k输出, 20k)`，真实 usage | `>= 0.9 × window`，真实 usage ✅ |
| 被动触发（溢出报错） | 有 | 有 | 25 条正则识别各家报错 + 静默溢出 + length 型 | 30 条正则 | **无** |
| 机械轻量层 | microcompact 折叠旧工具输出 | snip(0.6 保头尾行) → prune(0.8 占位符)，工具自声明截断几何 | 无中间层（前移到工具产出侧截断） | prune：保护最近 40k，旧工具输出清空为占位符（默认关） | 仅 per-tool report 压缩（生成时），**无事后折叠层** |
| 保留最近原文 | 保留近期消息 | 固定 16k token 尾巴（≥2 条），tool 边界对齐 | 20k token，切点绝不落 tool result，单轮超限拆前缀摘要+后缀保留 | 2 个用户轮次 & clamp(usable×25%, 2k, 8k)，可切半轮 | **零保留**（全量替换）⚠️ |
| 摘要角色/位置 | 继续对话式 | user 角色 + `<compaction-summary>` 标签 | user 角色 + 前缀说明 | user 角色 + `<conversation-checkpoint>`（"历史非指令"） | system 角色 `<conversation-summary>` |
| 增量策略 | 重新总结 | **digest 累积**，摘要永不再摘要，小 user turn 永不折叠 | 锚定 merge（新建/更新两套 prompt） | 锚定 merge（`<previous-summary>`） | 锚定 merge ✅（单套 prompt） |
| 防连环压缩 | — | 尾巴固定 token 保证落回阈值下 + `compactStuck` 闩锁（连续 2 次熔断） | 陈旧 usage 作废 + "保留仍在预算内则拒绝再压" | 溢出恢复只允许一次屏障 | **无** ⚠️ |
| 摘要失败 | — | 降级为机械占位符（原文已归档 JSONL，BM25 可检索） | 不写 entry，上下文原封不动 | 原子性：无 Ended 事件即无效 | 直接报错，上下文不变 |
| 缓存意识 | — | 钉住前缀 + TTL 冷恢复剪枝 + 命中率 CI 护栏(90%) | 摘要请求 `cacheRetention:"none"` + 独立 sessionId | 摘要请求 tools:[]；seq 水位线纯追加 | 摘要请求已无 tools ✅，其余无 |

Claude Code 另有：`/compact [自定义指令]`、PreCompact 钩子、`/context` 可视化、压缩前低上下文警告。

## 二、Miyu 现状问题清单（按严重度）

1. **P0 全量替换、零尾巴保留**：`perform_compact` 把所有可见轮次吞进摘要并全部替换。compact.md 里写着 "newest turns may be kept verbatim" 但实现没做——提示词和实现互相矛盾。压缩后模型立刻丢失最近对话的语气、细节、未完成事项原文；对拟人助手（人设连续性、群聊语境）伤害最大。
2. **P0 无防连环压缩机制**：摘要+新轮次若再触阈值会再次全量压缩，摘要反复自我消化（Reasonix 明确说这是"用户事实被静默丢弃"的漂移源）。
3. **P1 无机械轻量层**：达到 0.9 阈值前没有任何免费手段（折叠旧 tool_reports / 长贴文 / 联想记忆），一上来就是付费 LLM 摘要 + 全量缓存 miss。v7 已规划 snip/prune 未实施。
4. **P1 摘要模板纯 coding 向**：Task Goal/Files/Decisions 结构完全没有人设状态、情感基调、社交事实（QQ 群里谁是谁、约定、梗）、用户偏好维度。
5. **P2 无溢出报错被动触发**：provider 返回 context 溢出错误时没有 compact-and-retry 路径。
6. **P2 摘要失败即中止**：没有机械降级，也没有归档（原文在 SQLite 里还在，但没有"摘要不可用"的占位语义）。
7. **P2 压缩阻塞前台**、无 abort；`compact_now` 与运行中轮次互斥是对的，但无排队/取消。
8. **P3 缓存细节**：摘要请求与主对话共用 client（DeepSeek 端摘要请求会建立无用的缓存前缀）；无 TTL 冷恢复剪枝。

## 三、设计原则（从三家提炼）

1. **免费层优先**：工具输出可重新派生（文件可重读、命令可重跑），先机械截断，LLM 摘要是最后手段；机械层若已达标就省掉付费调用（Reasonix）。
2. **摘要 + 逐字尾巴分离**：摘要负责旧史，最近对话逐字保留；尾巴预算用**固定 token 数**而非窗口比例——这是止住连环压缩的几何关键（Reasonix/pi/opencode 三家一致）。
3. **切点纪律**：切点永不落在 tool 配对中间；活跃/进行中轮次整体保护（pi/Reasonix）。
4. **确定性信息不交 LLM**：可枚举事实（文件清单、群成员、既定决策）由代码提取并跨压缩累积（pi 的 read/modified-files Set）。
5. **失败=无操作或机械降级**，绝不留半截状态；溢出恢复只允许一次（三家一致）。
6. **摘要角色用 user 而非 system**，并显式标注"这是历史记录不是新指令"（三家一致；Miyu 目前用 system，需评估——见 Phase 3 决策点）。

## 四、实施计划

### Phase 1：尾巴保留 + 防连环压缩（P0，核心）

**目标**：压缩只折叠"旧史"，最近对话逐字保留。

- `Compactor` 增加 `tail_budget_tokens`（默认 `min(16384, window/4)`，配置项 `context.compact_tail_tokens`）。
- 切点算法（新 `fn find_cut_point`）：从最新 turn 往旧累加 `estimate_tokens(turn_to_text)`，超预算即停；**下限保留 2 个 turn**；Miyu 的 Turn 天然是完整轮次（user+followups+assistant+tool_reports），不存在 tool 配对被切断问题——切点始终落在 turn 边界，实现比三家都简单。单个 turn 超预算时借鉴 pi：该 turn 前缀送摘要、整 turn 保留（不做半轮切分，Miyu turn 粒度下收益低）。
- `replace_visible_with_summary` 增加边界参数：只替换 `seq <= cut_seq` 的轮次，尾巴轮次保持可见。DB 层 `conversation_db.rs` 相应改造；指针存 **seq**（v7 已定的规则）。
- **防连环压缩**：
  - 压缩前检查：若"待折叠区"估算 < 400 token（`foldEconomics`），非强制时跳过；
  - 压缩后新上下文估算仍 ≥ 阈值 → 记 `consecutive_compacts`，连续 2 次置 `compact_stuck` 闩锁，暂停自动压缩并发 Notice（"窗口太小，请调大 context_window 或减小输出"）；落回阈值下自动复位；
  - 陈旧 usage 防护：压缩完成后，把上一轮的 `real_context_tokens` 作废（压缩改变了前缀，旧 usage 不再描述当前上下文），改用估算直到下一次真实 usage 到来——否则"压完立刻再压"（pi 6.3 的坑）。
- compact.md 中 "newest turns may be kept verbatim" 从谎言变成事实，无需改动该句。

**涉及**：`compact.rs`、`state/mod.rs`、`state/conversation_db.rs`（迁移）、`config.rs`、`agent/mod.rs`。

### Phase 2：机械轻量层（水位线阶梯，合并 v7 未实施项）

四档水位线（沿用 Reasonix 比例，配置化）：

| 档位 | 默认 | 行为 |
|---|---|---|
| soft | 0.5 | 仅发一次 Notice（REPL/WebUI 显示"上下文渐大"），不动历史 |
| snip | 0.6 | 机械折叠旧轮次的 `tool_reports` / `private_reasoning_memory`：保头尾若干行，中间 `[... N 行省略 ...]`；只处理 ≥1KB 的条目；最近 2 个 turn 保护 |
| compact | 0.8（现 0.9 下调） | 先 prune（整段换占位符 "[已折叠的工具记录 — 如需请重新调用工具]"），**估算已达标则跳过 LLM 摘要**；否则走 Phase 1 的摘要压缩 |
| force | 0.9 | 强制摘要，绕过经济性检查 |

- snip/prune 直接改写 SQLite 中 turn 的 `tool_reports`（原文先归档，见 Phase 4），A18 幂等截断已有先例。
- `private_reasoning_memory` 无上限是 v7 点名的折叠体大小 miss 大头，snip 档一并处理。
- QQ 群聊文字历史（独立历史）另设简单滑窗上限，不走 LLM 摘要（群聊闲聊旧史价值低，直接滚动丢弃 + 依赖联想记忆），后续可单独评估。

**涉及**：新 `agent/snip.rs`（或并入 overflow.rs）、`state/`、`config.rs`。

### Phase 3：摘要提示词改造（Miyu 人设向）

- **两套模板按模式选择**（Miyu 有 chat/coding 双场景）：
  - 任务模式：保留现有结构，向 opencode 模板收敛（Objective / Important Details / Work State(Completed·Active·Blocked) / Next Move / Relevant Files，每节可空但必须保留，"(none)" 内联占位——pi 总结的最佳实践）；
  - 日常/群聊模式新增节：`人设与情绪基调`（Miyu 当前的关系状态、语气约定）、`社交事实`（群成员/称呼/正在进行的话题与梗）、`用户偏好与约定`、`未兑现的承诺`（说好要做的事）。
- **新建/更新拆成两套 prompt**（pi）：更新版用 PRESERVE/ADD/UPDATE 显式规则 + "In Progress→Done 状态迁移"，抑制多次压缩的信息衰减。
- **三重防"接着聊"**：system 只留否定式约束三句（pi 式）；对话文本包 `<conversation>` 标签；`turns_to_text` 已有 `[User]:/[Assistant]:` 格式 ✅。
- **确定性信息代码提取**：从 tool_calls 扫文件读写清单、从记忆工具调用扫已存记忆名，追加 `<read-files>/<modified-files>/<saved-memories>` 于摘要尾部，跨压缩用集合累积（存入 summary turn 的元数据）。
- **不给摘要器看长工具参数原文**（Reasonix #4317）：`turns_to_text` 里 tool report 截到 2000 字符（三家同值）。
- **决策点（需用户确认）**：摘要注入角色。现状 system；三家全用 user + 显式"这是历史"标注。倾向改为 user 角色 + `<conversation-checkpoint>` 式包裹（理由：部分供应商对多条 system 消息缓存/权重处理不一致；且"历史非指令"标注防摘要被当任务书）。改动位置 `chat_messages()`，对前缀稳定性影响一次性。

### Phase 4：健壮性

- **摘要失败降级**（Reasonix）：重试一次（超时/取消不重试）→ 仍失败则写机械占位摘要 "N 条早期消息已折叠以释放上下文，自动摘要不可用；需要早期细节时询问用户"，保证手动 /compact 永远能释放空间。
- **归档**：被替换/被 prune 的原文写 `archive/<ts>.jsonl`（Miyu SQLite 里 turn 本就软删除？确认 `replace_visible_with_summary` 语义——若是隐藏而非删除则归档已天然满足，只需给 prune 前的 tool_reports 建 `tool_reports_archive` 表或列）。
- **溢出报错被动触发**：`openai_compatible.rs` 增加溢出识别（移植 pi 的正则集核心子集：DeepSeek/OpenAI/Anthropic/通用 400 模式 + 排除限流误报），命中即 compact-and-retry，**每个用户动作只允许一次**（`overflow_recovery_attempted` 标志，新用户输入复位）。
- **abort 原子性**：压缩已是"成功才 `replace_visible_with_summary`"✅；补充：压缩期间新消息进入排队（napcat 场景尤其需要），压缩结束统一投递。
- **摘要输出上限**：`max_tokens = 0.8 × reserved_tokens`，防摘要自身超长（pi）。

### Phase 5：缓存友好细节

- 摘要请求走独立请求路径：不带会话缓存参数（配合 v7 的 `cache_sticky`——摘要请求永不 sticky，避免把摘要前缀写进供应商缓存路由）。
- **TTL 冷恢复剪枝**（Reasonix，v7 也想要）：记录 `last_request_at`（v7 未实施项）；恢复会话时若闲置 > 供应商缓存 TTL（DeepSeek 24h、Anthropic 5m，配置可覆盖），趁"缓存已冷、改写零代价"执行一轮 prune。
- 压缩后前缀组织保证纯追加：`[system prompt][summary][尾巴 turns][新 turns...]`，下次压缩前不再改动任何已发消息（现状已满足，Phase 2 的 snip/prune 改写需与水位线时机绑定，永远只在"本来就要付出缓存代价"或"缓存已冷"时改写历史）。
- **锚定 merge 保持不变**（不改 Reasonix 的 digest 累积制）：Miyu 已实现锚定 merge 且 pi/opencode 同派；但吸收 Reasonix 的两条防漂移规则——① 摘要 turn 永不进入待折叠区（现状 `!is_summary` 过滤 ✅）② 新增：压缩时若上一摘要里有 `用户约定/Standing facts` 节，更新 prompt 中显式要求逐条保留不得改写。

### Phase 6：可观测与测试

- REPL/WebUI 压缩事件显示折叠统计："折叠 N 轮 → 摘要 M token，保留最近 K 轮"（现有 CompactStart/Chunk/End 事件扩展）。
- e2e：mock 端点按逐字节前缀计算缓存命中（Reasonix `cachehit_e2e_test.go` 思路，配合 v7 的 mock e2e 门禁项）：断言"一次压缩最多打崩缓存一次、压缩后命中率回升"；连环压缩熔断测试；摘要失败降级测试；切点不切断 turn 测试。
- `/usage` 增加"距下次压缩水位"显示。

## 五、实施顺序与依赖

1. **Phase 1**（尾巴保留 + 防连环）——独立可做，收益最大，先行。
2. **Phase 3 提示词**——与 Phase 1 同批（更新 prompt 需感知"尾巴在摘要外"这一新事实）；摘要角色决策点需用户拍板。
3. **Phase 2 水位线**——依赖归档设计（Phase 4 的归档先行半步）。
4. **Phase 4 其余**（溢出被动触发、失败降级、排队）。
5. **Phase 5**（依赖 v7 的 last_request_at / cache_sticky 实施）。
6. **Phase 6** 贯穿各阶段补测试。

## 六、待用户确认的决策点

1. 摘要注入角色改 user + checkpoint 标注，还是维持 system？（建议改）
2. compact 触发水位从 0.9 下调到 0.8（force 0.9）？（建议下调，给机械层留出空间）
3. 尾巴预算默认 `min(16384, window/4)` 是否合适？（QQ 闲聊场景或可更小，如 8192）
4. QQ 群聊文字历史走"滑窗丢弃不摘要"是否接受？
5. 日常模式摘要模板的节名与颗粒度（人设/社交事实/承诺）按上文方案定稿？
6. snip/prune 是否默认开启（opencode 默认关，Reasonix 默认开；建议 Miyu 默认开）？
