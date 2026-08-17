# 03 · Phase 0：基线与安全网（先做，不碰业务）

> 本阶段不移动任何代码，只建立“如果拆坏立刻能发现”的机制。

## 1. 代码与提交基线

1. 从 `main` 拉出专用分支：`refactor/split-maintainability`。
2. 在分支创建 `docs/plan/` 之外**不修改任何源码**。
3. 记录基线信息：

```bash
git rev-parse HEAD
rustc --version
cargo --version
cargo test -- --list | wc -l
cargo test 2>&1 | tee /tmp/miyu-refactor-baseline-test.log
```

4. 记录两个**当前环境事实**（2026-08-17 已实测）：
   - `cargo fmt --check` 当前失败（约 4330 行 diff）；必须先做一次独立格式化提交，否则后续“每步 fmt 通过”不可行。
   - `cargo test` 当前 1482 例中 1474 通过、1 个 PTY 环境测试失败（`web::tests::origin_tty_gates_and_writeback_against_real_pty`，依赖本机 PTY/子进程环境）；基线需明确是“修复环境后全绿”，还是记录已知跳过项并留 issue，后续 CI 不允许新增失败。
6. 以上两项以独立 PR 处理，禁止和结构拆分混在一起。

7. 保存一份基线产物（不要提交二进制）：

```bash
mkdir -p /tmp/miyu-refactor-baseline
cargo build --release
cp target/release/miyu /tmp/miyu-refactor-baseline/miyu.baseline
miyu_bin=/tmp/miyu-refactor-baseline/miyu.baseline
```

8. 后续每阶段用基线二进制与当前二进制做同一批黑盒冒烟，对比输出。

## 2. 建立黑盒冒烟清单（手工/脚本各跑一遍）

| 场景 | 命令 | 关注点 |
|---|---|---|
| 帮助/版本 | `miyu -h`、`miyu -V`、`miyu normal -h` | 输出完全一致 |
| 路径 | `miyu paths` | 路径一致 |
| 初始化 | `MIYU_HOME=/tmp/miyu-smoke-init miyu init` | 创建目录、无报错 |
| 一次性提问 | `MIYU_HOME=... miyu ask --stdout "1+1"` | 输出结构一致 |
| REPL 启动 | `MIYU_HOME=... timeout 10 miyu normal` | 进入、退出无 panic |
| daemon | `MIYU_HOME=... miyu daemon start/status/logs -n 1/stop` | 状态输出一致 |
| Web 启动 | `MIYU_HOME=... timeout 15 miyu web --bind 127.0.0.1` | URL/日志一致 |
| 导出 | `MIYU_HOME=... miyu export --dry-run` | 清单、体积、权限一致 |
| tool-call | `MIYU_HOME=... miyu tool-call --list` | 工具列表一致 |
| 知识库 | `MIYU_HOME=... miyu kb list/search/stats` | 输出一致 |
| 技能 | `miyu skills list/show/stats` | 输出一致 |
| 记忆 | `miyu memory stats/search` | 输出一致 |

> 所有冒烟都使用独立 `MIYU_HOME`，不污染真实用户数据。

## 3. 建立机械拆分检查脚本

新增 `scripts/refactor-check.sh`（可提交；它只读代码，不改变行为）：

```bash
#!/usr/bin/env bash
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
# Phase 0.5 之前：仅检查本次 PR 触及的 .rs 文件
# Phase 0.5 格式化提交之后：恢复全局 cargo fmt --check
if [ -f .rustfmt-global-enabled ]; then
  cargo fmt --check
else
  git diff --name-only HEAD -- '*.rs' | xargs -r rustfmt --check
fi
cargo check --all-targets
cargo test
# 目标规模检查：红色线文件（>3000 行）必须逐阶段减少
python3 scripts/refactor_size_report.py
```

新增 `scripts/refactor_size_report.py`，逻辑：

- 统计 `src/**/*.rs` 行数；
- 输出超过 800 / 1500 / 3000 行的文件列表；
- 与基线 JSON 对比，断言没有新的 >3000 行文件；
- 该脚本只报告，不修改代码。

## 3.5 死代码基线（Phase 8.5 使用）

1. 生成基线报告：

```bash
RUSTFLAGS='--force-warn=dead_code'   cargo check --all-targets --message-format=short   2>&1 | tee /tmp/miyu-dead-code-baseline.txt
```

2. 保存告警数量与清单，记录基线时点（例如：生产目标 103 条、测试目标 72 条）。
3. Phase 1–8 结束后对比清单：新出现的告警定位到具体文件移动；原有告警不处理，留给 [13-死代码清理计划](13-死代码清理计划.md)。
4. 不在本阶段删除任何未使用代码。

## 4. 架构依赖门禁（Phase 1 之后启用）

新增 `scripts/arch_dep_check.py`：

1. 解析每个 `src/*.rs` 和模块目录的 `use crate::...`；
2. 维护一个规则表：

| 规则 | 允许 |
|---|---|
| `tools` | 不允许 `use crate::platforms` / `use crate::web` / `use crate::cli` |
| `memory/state/llm/agent` | 不允许 `use crate::web` / `use crate::cli` |
| `platforms` | 不允许 `use crate::web` / `use crate::cli` / `use crate::config_tui` |
| `web` | 不允许 `use crate::cli` |
| `render` | 不允许 `use crate::tools` 的执行类型（类型下沉后） |
| `state` 与 `memory` | 只允许通过 `memory_types`/trait 互相引用，禁止直接 SQL 交叉 |

3. 违规即失败；确有例外时在规则表加白名单并写明原因。

> 门禁先按 warn 模式加入，Phase 1 完成后切 fail 模式。

## 5. 字节级缓存回归快照

缓存是 Miyu 最容易“看起来正常、实际退化”的部分。Phase 0 需要锁定当前行为：

1. 对同一 `MIYU_HOME` 跑一个固定会话脚本（用本地 mock 或真实 provider）。
2. 抓取两轮请求的 `cache-usage.<date>.jsonl` 与 `requests-<date>.jsonl`（如开启）。
3. 记录第二轮相比第一轮的 `cache_read` 绝对值。
4. 每个涉及 `agent/llm/tools/registry` 的 Phase 之后重跑，要求 `cache_read` 不下降超过基线（允许 provider 波动时记录原因）。

## 6. 测试数据隔离约定

- 所有拆分阶段禁止修改 `tests/`、`testkit/` 的测试语义；只允许在源码模块间搬运 `#[cfg(test)]` 测试。
- 测试中的路径假设（`MIYU_HOME`、XDG、端口）保持不变。
- 每次移动测试块后，用 `cargo test -- --list` 确认用例数量不减少。

## 7. Phase 0 完成标准

- [ ] 基线日志与二进制已保存。
- [ ] `refactor-check.sh`、`refactor_size_report.py`、`arch_dep_check.py` 已提交。
- [ ] 黑盒冒烟清单在基线通过。
- [ ] `cargo test` 全绿。
