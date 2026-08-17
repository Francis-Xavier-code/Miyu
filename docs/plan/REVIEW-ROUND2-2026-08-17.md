# Plan 第二轮逐个 Review（2026-08-17）

> 基准：当前分支 `token-optimization`，HEAD `8dea494`。本轮对 `docs/plan/` 下 21 个 Markdown 文件逐个复核，并在复核后修正了文档。

## 0. 本轮重新执行的验证

| 验证 | 结果 |
|---|---|
| `cargo fmt --check` | **失败**，约 4330 行 diff |
| `cargo test` | 1482 例：1474 通过，1 失败，7 ignored |
| 失败测试 | `web::tests::origin_tty_gates_and_writeback_against_real_pty`（PTY 环境依赖） |
| dead code 强制 lint | bin 103 条；bin test 72 条（60 重复） |
| 链接与代码围栏 | 全部通过 |
| 依赖循环 | `platforms→web`、`web→cli`、`tools→platforms`、`state↔memory` 仍存在 |
| 巨型文件 | cli 16631、web 12836、onebot 10475、agent 9711 |

## 1. 逐文件结论

### 结构性拆分方案

| 文件 | 第二轮结论 | 处理 |
|---|---|---|
| `README.md` | 结构清晰；缺少“行号可能漂移”的显式说明 | 已加行号快照说明 |
| `01-现状诊断` | 数据已与当前分支一致；依赖循环描述准确 | 无 |
| `02-目标架构` | 分层、目录、规模阈值、死代码红线合理 | 无 |
| `03-Phase0` | 原方案假设 `cargo fmt --check` 和全量测试已通过；本轮实测不成立 | 已补：格式化独立提交 + 测试基线/已知失败处理；脚本在格式化前只检查 PR 触及文件 |
| `04-Phase1` | lib.rs、runtime 下沉、平台类型下沉、state↔memory 解耦步骤可执行 | 无 |
| `05-Phase2-cli` | 文件划分合理；行号与当前分支有少量漂移 | 以符号重定位为准，已有全局说明 |
| `06-Phase3-web` | 文件划分合理；先移 runtime 再拆 handler 的顺序正确 | 无 |
| `07-Phase4-agent/llm` | 拆分点覆盖主循环/协议/thinking；缓存铁律明确 | 无 |
| `08-Phase5-config` | schema/validate/migrate/TUI 页面分离清晰 | 无 |
| `09-Phase6-state` | 按聚合拆分，迁移与固定列序保护到位 | 无 |
| `10-Phase7-platforms` | onebot/real_context/renderer/message_history 拆分可执行 | 无 |
| `11-Phase8` | CI 门禁、死代码专项、workspace 评估边界清楚 | 已按格式化基线修订 CI 说明 |
| `12-风险验收` | 风险、回滚、DoD 完整 | 已按格式化/测试基线修订验收标准 |
| `13-死代码清理` | 告警清单与当前强制 lint 复现一致；D1–D4 处置合理 | 无 |

### 独立专项

| 文件 | 第二轮结论 | 处理 |
|---|---|---|
| `专项-LaTeX渲染问题调查` | 根因、实测、三方案准确；已确认 chafa stdin 可行 | 无 |
| `performance/README` | 独立性与优先级正确 | 无 |
| `performance/01-CPU` | 多数热点仍成立；但 7 个条目当前分支已修复 | 第一轮已标记 ✅；本轮复核通过 |
| `performance/02-内存` | 多数成立；3 个条目过时 | 已修正 M4-2、M4-3、M4-9 |
| `performance/03-显存/3D` | 资产尺寸与 CSS 动画证据准确；原方案建议覆盖 `pics/` 源图有风险 | 已改为“源图不动，新增 `web/assets/` 优化副本” |
| `performance/04-测量验收` | 测量脚本与门禁完整；与 Phase 0 的新格式化/测试基线一致 | 无 |
| `REVIEW-2026-08-17` | 第一轮 review 记录准确 | 保留 |

## 2. 本轮发现并修正的问题

1. **格式化基线不成立**：原方案把 `cargo fmt --check` 当成已有前提，但当前仓库 4330 行不格式化。
   → 在 `03`、`11`、`12` 增加 Phase 0.5 独立格式化步骤；格式化前只对 PR 触及文件做 `rustfmt --check`。
2. **测试基线不成立**：当前环境有 1 个 PTY 环境测试失败。
   → Phase 0 要求明确“修复环境后全绿”或“记录已知跳过 + issue”，后续不得新增失败。
3. **`03` 编号重复**：插入环境事实后出现两个步骤 4。
   → 已重排编号。
4. **性能方案 M4 过时**：
   - clipboard_texts 仍无上限，但 wake/run 集合已随轮询重建 → 修改范围；
   - weather 缓存已有写入时 sweep，只是没有容量上限 → 修改描述；
   - meme_collector 已直接借用 data 哈希 → 标记已修复。
5. **显存方案 G1 会破坏源资源**：原写“压缩 pics/ 图片”会同时影响 README、终端演示等外部引用。
   → 改为保留 `pics/` 源文件，新增 `web/assets/` 的 WebUI 专用压缩副本。
6. **README 缺少行号漂移声明**。
   → 已加“2026-08-17 token-optimization 分支快照，执行前用 rg 重定位”。

## 3. 仍需人工决策项

- [ ] Phase 0.5 是否接受一次全仓库 `cargo fmt` 提交。
- [ ] `origin_tty_gates_and_writeback_against_real_pty` 是修环境/修测试，还是记录已知跳过。
- [ ] WebUI logo/wallpaper 优化副本的目标尺寸（2x 或 3x）与格式（WebP/PNG）。
- [ ] 性能 P1-A 中“已修复”条目是否保留为复核清单，还是移到已完成附录。

## 4. 结论

两轮 review 后，全部 21 个 plan 文件内部一致、链接与 Markdown 结构有效，事实性声明已与当前分支重新对齐。剩余未决项均是需要维护者拍板的产品/环境决策，不影响方案可执行性。

未 commit，未 push，仅修改 `docs/plan/`。
