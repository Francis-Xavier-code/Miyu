# 01 · CPU 优化方案

## 1. 优化顺序

按“风险从低到高、收益从高到低”排：

1. P1-A 一行级/常数级修复
2. P1-B 重复计算与 clone 削减
3. P1-C 异步阻塞治理
4. P2-A 前端流式渲染
5. P2-B 轮询与后台任务治理
6. P2-C token 计量与上下文计算

每个条目单独提交，commit 信息带 `perf(cpu):`。

## 2. P1-A：一行级/常数级修复

以下来自既有代码审查结论，绝大多数是“改一个调用或加一个上限”：

| # | 位置/符号 | 问题 | 修法 |
|---|---|---|---|
| C1 | `src/platforms/file_reader.rs` `read_platform_text` | ~~UTF-8 逐字节修整~~ | ✅ 当前 `token-optimization` 分支已用 `valid_up_to()` 修复，仅保留回归测试 |
| C2 | `src/llm/openai_compatible.rs` DSML 隐藏前缀扫描 | 每个 stream delta 对整个累积文本重扫，O(n²) | 记忆上次扫描位置，只扫描新增尾部 |
| C3 | `src/cli.rs` daemon log `drain(..=newline)`、tail chunk 前置拼接 | 大日志 O(n²) 字节搬移 | 用游标/环形缓冲/`VecDeque` |
| C4 | `src/tools/default_tools.rs` `glob_files`/`grep_text` | ~~超时后 rg 子进程未 kill_on_drop~~ | ✅ 当前分支已加 `kill_on_drop(true)`，仅复核 |
| C5 | `src/tools/web.rs` 搜索跳转解析 | 串行 await 最坏 20×15s | 限并发（如 4）批量解析 |
| C6 | `src/tools/scripts.rs` stdin 写入 | ~~write_all 在 timeout 外~~ | ✅ 当前分支已移入 timeout，仅复核 |
| C7 | `src/tools/mcp.rs` 同步 `read_line` | ~~无超时阻塞 tokio worker~~ | ✅ 当前分支已用 `spawn_blocking` + 外层超时 + SIGKILL 兜底，仅复核 |
| C8 | `src/tools/caniplayonlinux_query.rs` | 全站分页无上限串行抓取 | 加页数/时长预算，超限返回部分结果 |
| C9 | `src/tools/deep_research.rs` | xhigh 无修订轮数与时长上限 | 增加硬上限（如 20 轮、30 分钟） |
| C10 | `src/tools/diagnostics.rs` pacman 探测 | ~~无超时同步进程~~ | ✅ 当前分支已加超时与 `kill_on_drop`，仅复核 |
| C11 | `src/tools/package_advisor.rs` | ~~同步进程调用~~ | ✅ 当前分支已加超时与 `kill_on_drop`，仅复核 |

**验收**：每项运行对应单测/黑盒场景；确认工具输出与修改前逐字一致。

## 3. P1-B：重复计算与 clone 削减

### C12 `AppConfig` 深拷贝

- 现状：`platforms/onebot.rs` 每条入站消息对完整 `AppConfig` 多次深拷贝；普通不触发的群消息也在付成本。
- 修法：
  1. 把 `AppConfig` 读取改为 `Arc<AppConfig>` 快照；
  2. 仅在 turn 真正启动时克隆；
  3. 纯读路径只借 `&AppConfig`。
- 影响文件：`platforms/onebot.rs`、`platforms/mod.rs`、`web.rs`。
- 风险：配置热更新并发。统一使用 `Arc` 原子替换 + 版本号，禁止局部可变 clone 回流。

### C13 token 重复计算

- 现状：
  - `agent/mod.rs` 每轮多次 `chat_messages()` 重建 + 全量 BPE 重算；
  - provider 不报 usage 时每个工具轮再多次全量分词；
  - trim 循环内重复序列化工具定义并分词。
- 修法：
  1. 上下文 token 估算按“轮次增量”缓存：只有新增轮才重算；
  2. 工具定义序列化按 registry generation 缓存；
  3. 无 usage 时才估算，并把结果放 `UsageAccumulator` 复用；
  4. `turn_context_tokens`/`turn_to_text` 补 `tool_flow`，避免因低估触发压缩空转（此条同时是语义正确性修复，但必须与性能分开提交）。

### C14 keepalive 快照 clone

- 现状：`agent` 保存整段消息快照，后台 ping 与每个工具轮多次全量 clone。
- 修法：只保存**已序列化字节**（`Arc<Vec<u8>>`），ping 时直接复用；工具轮比较改为引用。

### C15 请求路径消息深拷贝

- 现状：`openai_compatible.rs` 每次尝试/重试克隆完整消息数组。
- 修法：构造一次 canonical messages，以 `Arc<[ChatMessage]>` 传递；仅在需要按协议 lower 时才生成新结构。

### C16 每调用新建 reqwest Client

- 位置：`exchange_rate.rs`、`man.rs`、`archlinux.rs`、`avatar.rs`、`web_images.rs` 重定向。
- 修法：统一复用 `http_response::shared_client()` 或模块级 OnceLock；重定向路径复用同一 client。

### C17 知识库搜索

- 现状：每次搜索全库逐文件整读 + 全文件 lowercase 拷贝，async 内同步执行。
- 修法：
  1. 先按元数据筛选，再读文件；
  2. 使用 `to_lowercase` 仅在命中候选上执行；
  3. 搜索放 `spawn_blocking`；
  4. 文件大小在读取前检查。

### C18 命令输出重渲染

- 现状：`render/mod.rs` 每 40ms 全量 clone + grapheme wrap 所有保留行。
- 修法：维护已渲染行的物理行索引，只重算最后一块；限制 live 行总字节。

### C19 `strip_inline_markup`

- 现状：每行 `Vec<char>` + 对每个 `[` 向后线性扫。
- 修法：单次扫描状态机；只在确定输出时分配。

### C20 TUI provider 浏览

- 现状：每按一次 j/k spawn 不可取消的 `/v1/models` 线程。
- 修法：60–120ms 去抖；只对进入焦点且未缓存的 provider 发起；保留 `fetch_seq` 防过期。

### C21 前端大对象深拷贝

- `web/app.js` 多处全量 `JSON.stringify`、全量配置重建、每秒全量 turns 拉取。
- 修法：用事件驱动 + 增量 patch；`refreshViewSnapshot` 从每秒全量改为仅在 run 事件后节流刷新。

## 4. P1-C：异步阻塞治理

### 原则

actor 专用线程 = 全部 turn 的 current_thread runtime；任何同步阻塞都会冻结所有会话。清单：

| # | 位置/符号 | 当前阻塞 |
|---|---|---|
| C22 | `agent/mod.rs` `compact_now` → `models_cache::refresh_blocking` | ~~30s 网络请求冻结 actor~~ | ✅ 当前分支已改为 `tokio::task::spawn_blocking`，仅复核 |
| C23 | `web.rs` manager 锁内磁盘 IO/SQLite 写 | bootstrap、配置保存、附件搬运 |
| C24 | `tools/clipboard.rs`、`tools/write.rs`、`tools/edit_replace.rs`、`tools/todowrite.rs`、`tools/memory.rs`、`tools/alarm.rs`、`tools/memes.rs` | 文件 IO |
| C25 | `memory/mod.rs` SQLite 全表扫描/逐行 UPDATE | 高频记忆路径 |
| C26 | `state/conversation_db.rs` 同步 SQLite 读写在 async handler | 会话/回合查询 |
| C27 | `default_kb.rs` `git ls-remote` 无超时 | REPL 启动路径 |
| C28 | `ipc.rs` 无限期 flock | 启动互斥 |

修法统一：

1. 文件 IO/SQLite/子进程 → `tokio::task::spawn_blocking` 或独立线程；
2. 网络阻塞 → 换成异步客户端，或 `spawn_blocking` + 超时；
3. 锁内只做内存状态修改，IO 在锁外完成；
4. 所有等待型系统调用加超时。

**注意**：这些改动会让执行时序发生变化。必须跑 REPL/WebUI/QQ 冒烟，重点验证并发 turn 与配置热更新。

## 5. P2-A：前端流式渲染 CPU

当前前端每个 delta：

1. 对累积全文全量 `renderMarkdown` 重建 DOM；
2. KaTeX 每帧重算；
3. reasoning 每 delta 全量 textContent + 全段 collect；
4. `renderConversation` 全量 `replaceChildren`。

修法分两步：

- **第一步（低风险）**：
  - 流式正文只对“新增片段”渲染，插入 fragment；
  - reasoning 使用 append-only 节点；
  - KaTeX 只在公式闭合后再渲染；
  - `content-visibility: auto` 应用于较旧的会话块。
- **第二步（较大）**：
  - 虚拟滚动/窗口化 conversation DOM；
  - 工具输出大文本用折叠摘要，不把 200KB raw 常驻 DOM 字符串。

## 6. P2-B：轮询与后台任务

| # | 现状 | 修法 |
|---|---|---|
| C29 | WebUI 每秒全量 turns 轮询 | 改为事件驱动 + 30s 对账一次 |
| C30 | 前端 resync 循环无上限 | run 起点事件重放；连续 resync N 次停止 |
| C31 | REPL jobs 轮询固定 1s | 有活动任务时 500ms，无任务时 5s |
| C32 | 后台 task 每行日志重开文件 | 打开一次缓冲写 |
| C33 | renderer worker 空闲 1h 才回收 | 若内存压力大，缩短为 10–15 分钟（可配置） |

## 7. P2-C：token 计量与压缩预算

- 先修口径：`turn_context_tokens` 与 `turn_to_text` 必须计 `tool_flow`（原偏差可达 57 倍，导致压缩反复空转、每轮重建上下文）。
- `trim_visible_context` 循环内不得每轮全量重序列化工具定义与分词；缓存 per-turn token 值。
- 图片 token 估算从固定 765 改为按尺寸估算（不改变外部 API，只影响内部裁剪时机）。
- 压缩折叠文本只分词一次，复用结果。

## 8. 验证

- [ ] `cargo test` 全绿。
- [ ] 活跃回合 CPU 对比基线：相同输入下采样 30s 的 user+sys 平均值下降。
- [ ] daemon 并发 4 个 turn 时，无单个 turn 阻塞其他 turn 超过 500ms。
- [ ] WebUI 使用 Playwright 脚本：滚动、流式 1000 token、KaTeX 渲染，主线程 long task 数量与总 CPU 下降。
- [ ] 所有性能优化 PR 中不出现工具输出、SSE 事件、请求字节变化。
