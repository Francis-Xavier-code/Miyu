# 11 · Phase 8：收尾、架构门禁与可选 workspace 评估

## 一、剩余大文件清理

### 1. `src/render/mod.rs`

拆成：

| 新文件 | 内容 |
|---|---|
| `render/mod.rs` | 公共 API（`print_markdown`、`StreamRenderer` 构造） |
| `render/ansi.rs` | ANSI 状态机、UTF-8 解码、敏感信息脱敏、URL 清理 |
| `render/markdown.rs` | Markdown 行渲染、inline 语法、代码块、表格、diff/todo |
| `render/command.rs` | `CommandOutputPreview/Tail`、`CommandLiveDisplay`、命令结果块 |
| `render/token.rs` | `TokenMeter`、token 格式函数 |
| `render/stream.rs` | `StreamRenderer` 主体、tool subject、工具进度/摘要 |

`math.rs`、`wait_spinner.rs` 保持独立。验收：`cargo test render::` 全绿，终端显示冒烟一致。

### 2. `src/memory/mod.rs`

拆成：

| 新文件 | 内容 |
|---|---|
| `memory/mod.rs` | `MemoryStore` 门面、生命周期、请求上下文 |
| `memory/jieba.rs` | `CompactJieba` 与分词 |
| `memory/association.rs` | 联想检索、格式化、去重、reinforce |
| `memory/diary.rs` | 日记写入、process_after_turn、pending events |
| `memory/facts.rs` | `remember_fact`、知识更新校验 |
| `memory/evicted.rs` | 被逐出上下文搜索、混合语义检索 |
| `memory/decay.rs` | 衰减/遗忘 |
| `memory/schema.rs` | data/state 表创建、迁移 v1/v2、列常量 |

验收：`cargo test memory::` 全绿；记忆联想/整理/遗忘冒烟一致。

### 3. `src/tools/web_images.rs`

拆成：

- `tools/web_images/mod.rs`（工具入口、候选池、下载/发布）
- `tools/web_images/providers.rs`（SearXNG/DDG/Bing/百度/360 解析）
- `tools/web_images/safety.rs`（SSRF/DNS/图片校验/视觉审核）

验收：`cargo test tools::web_images` 全绿。

### 4. `src/tools/web.rs`

拆成：

- `tools/web/mod.rs`（`web_search`/`web_fetch` 入口）
- `tools/web/providers.rs`（Tavily/Firecrawl/AnySearch/Exa/SearXNG/DDG/Yahoo/360/搜狗）
- `tools/web/parse.rs`（结果解析、URL 安全、格式化）

### 5. `src/tools/diagnostics.rs`

拆成：

- `tools/diagnostics/mod.rs`（入口、报告结构、分派）
- `tools/diagnostics/linux.rs`
- `tools/diagnostics/input_method.rs`
- `tools/diagnostics/commands.rs`

### 6. `src/tools/scripts.rs`

拆成：

- `tools/scripts/mod.rs`（扫描、注册、index 读写）
- `tools/scripts/run.rs`（执行、超时、输出上限）
- `tools/scripts/register_tools.rs`（register/unregister 工具 handler）

### 7. 其他

- `src/paths.rs` 尾部约 800 行测试迁到 `src/paths_tests.rs` 或 `paths/tests.rs`。
- `src/ipc.rs` 保留单文件，若超 1500 行再把 daemon 启动/恢复拆到 `ipc/daemon.rs`。

## 二、死代码专项（Phase 8.5）

删除根级 `#![allow(dead_code)]` 与全部死代码清理，按 [13-死代码清理计划](13-死代码清理计划.md) 执行：

- DC-1…DC-8 按模块分批提交；
- D1 删除 / D2 测试化 / D3 标注保留 / D4 语义修复或废弃；
- 最终 `RUSTFLAGS='--force-warn=dead_code' cargo check --all-targets` 零告警（白名单除外）。

## 三、架构门禁固化为 CI

在 `.github/workflows/` 增加或修改 CI：

```yaml
- name: Architecture gate
  run: |
    python3 scripts/arch_dep_check.py --fail
- name: Size gate
  run: |
    python3 scripts/refactor_size_report.py --max 3000 --fail
- name: Behavior gate
  run: |
    ./scripts/refactor-check.sh
- name: Dead code gate
  run: |
    RUSTFLAGS='--force-warn=dead_code' cargo check --all-targets --message-format=short 2>&1 | tee /tmp/dead-code-report.txt
    test $(grep -c 'never used\|never read' /tmp/dead-code-report.txt || true) -eq 0
```

要求：

- 依赖方向违规 = 失败；
- 新增大文件（>3000 行）= 失败；
- Phase 0.5 格式化提交之后：`cargo fmt --check` = 失败；
- `cargo test` 出现任何基线之外的失败 = 失败。

## 四、文档更新

拆分完成后：

1. 更新 `docs/wiki/14-参与开发.md` 与 `docs/wiki/15-扩展指南.md` 中的路径/行号。
2. 在仓库根 README 加“代码结构”一节，指向本目录结构。
3. 每个目录补 `README.md`（或模块顶部注释）说明职责与依赖方向。

## 五、可选：评估 workspace 多 crate

当以下条件全部满足后再评估：

- [ ] 单 crate 内模块分层稳定三个月；
- [ ] `arch_dep_check.py` 连续通过；
- [ ] 没有模块通过 `pub(crate)` 绕过边界；
- [ ] 已有新平台/新协议的实际扩展需求。

可选目标：

```text
crates/
├── miyu-core        # 基础层 + 领域层（paths/config/state/llm/tools/memory/agent）
├── miyu-runtime     # 宿主层（runtime/daemon/ipc）
├── miyu-platforms   # 平台层（platforms）
├── miyu-tui         # 呈现层（cli/render/config_tui/shell）
└── miyu             # bin
```

**注意**：workspace 化会改变编译单元与 feature 传递，可能影响构建时间、panic 定位与 `include_str!` 资源；必须单独走 RFC/评估，不与本方案其他阶段捆绑。
