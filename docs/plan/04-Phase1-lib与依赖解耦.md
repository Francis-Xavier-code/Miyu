# 04 · Phase 1：引入 lib.rs 并打破跨层循环

本阶段是后续所有拆分的“地基”。仍以机械移动为主，不修改业务逻辑。

## 1. 步骤 1.1：新增 `src/lib.rs`，main.rs 变薄

**现状**：`src/main.rs` 声明全部模块并定义 `run()`。

**做法**：

1. 把 `main.rs` 中的：

```rust
mod agent;
mod alarm;
...
mod web;
```

以及 `use anyhow::Result;`、`pub async fn run() -> Result<()>` 整体搬到新建的 `src/lib.rs`。

2. `src/main.rs` 只保留：

```rust
use anyhow::Result;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = miyu::run().await {
        eprintln!("{}: {error:#}", miyu::i18n_text("error", "错误"));
        std::process::exit(1);
    }
}
```

需要在 `lib.rs` 暴露 `run` 和一个 `i18n_text` 包装（或直接保留 `main.rs` 的错误打印逻辑在 lib 中提供 `run_main`）。

3. 若同名 lib/bin target 命名有歧义，在 `Cargo.toml` 明确：

```toml
[lib]
name = "miyu"
path = "src/lib.rs"
```

4. 原 `#![allow(dead_code)]` 移入 `lib.rs`，并**在后续 Phase 逐步移除**；移除后产生的 dead_code 要么删，要么在具体项上加精确 `#[allow(dead_code)]`，并注释原因。

**验收**：`cargo build --all-targets`、`cargo test`、`miyu -h` 输出与基线一致。

## 2. 步骤 1.2：下沉 CLI 参数类型，消除 `web → cli`

**现状**：`web.rs` 使用 `crate::cli::{build_tool_registry, WebArgs}`；`daemon.rs` 也使用 `cli::WebArgs`。

**做法**：

1. 新建 `src/args.rs`（基础层），把以下类型从 `cli.rs` 原样搬入：

   - `WebArgs`（含 Debug 脱敏实现）
   - `DaemonArgs`、`DaemonCommand`、`DaemonLogsArgs`
   - `AlarmWorkerArgs`（若 daemon/worker 共用）
   - 相关 clap `Args` 属性与注释

2. `cli.rs` 改为 `use crate::args::{WebArgs, ...}` 并 re-export，保证 `cli::WebArgs` 暂时仍可用；随后把 `daemon.rs`、`web.rs`、`ipc.rs` 的引用改为 `crate::args`。

3. 把 `build_tool_registry` 从 `cli.rs` 移到 `tools/mod.rs` 的新函数 `tools::build_tool_registry(...)`（该函数本来就只依赖 tools/config/paths/state，与 CLI 无关）。

4. 删除 `web.rs` 中的 `use crate::cli::...`。

**验收**：`cargo check` 无 `web → cli` 依赖；`arch_dep_check.py` 规则可启用该条。

## 3. 步骤 1.3：抽取宿主共享类型，消除 `platforms → web`

**现状**：`platforms/mod.rs`、`onebot.rs`、`assets.rs` 依赖：

```
crate::web::{random_id, validate_content, ActorCommand, DaemonState, IpcRunGuard, RunInfo}
```

**做法**：

1. 新建 `src/runtime/` 模块，从 `web.rs` 原样搬迁以下类型与实现：

   - `DaemonState`、`ManagerState`、`ContextSnapshot`
   - `TurnEngineState`
   - `RunInfo`、`RunOperation`
   - `ActorCommand`、`AdminFailure`、`PlatformSessionResetError`、`PlatformPersonaResetError`
   - `IpcRunGuard`
   - `EventHub`、`EventSubscription`、`EventRecord`
   - `QuestionBroker`
   - `random_id`、`validate_content`、`safe_error_message`
   - 被这些类型直接依赖的请求/响应结构（如 `TurnUpdateRequest/Receipt`、`ThinkingVariantUpdate`，若平台层也需要）

2. `web.rs` 通过 `use crate::runtime::{...}` 继续使用；`platforms/` 改为引用 `crate::runtime`。

3. 若搬迁后 `runtime` 依赖 `web::ApiError`，把 `ApiError` 也移到 `runtime/error.rs` 或让它依赖 `runtime`，避免 `runtime → web`。

4. 暂时允许 `runtime → platforms`（宿主层需要平台能力），但禁止反向。

**验收**：

```bash
grep -R "use crate::web" src/platforms || echo OK
grep -R "use crate::cli" src/web src/daemon.rs || echo OK
cargo check --all-targets
cargo test
```

## 4. 步骤 1.4：下沉纯平台类型，消除 `tools/agent/memory → platforms 运行时`

**现状**：

```
tools/vision  -> PlatformContextImageRef, PlatformImageData, PlatformTurnContext
agent         -> PlatformContextFileRef/ImageRef, PlatformTurnContext
memory        -> PlatformPrincipal
```

**做法**：

1. 新建 `src/platform_types.rs`（基础层），只放**纯数据**：

   - `PlatformPrincipal`（含 `stable_key()` 可保持实现，blake3 属基础能力）
   - `PlatformContextImageRef`、`PlatformContextFileRef`
   - `PlatformImageData`
   - `PlatformConversation`、`ConversationKind`
   - `PlatformInboundEvent/Media/...` 等无平台执行逻辑的结构

2. `platforms/types.rs` 改为 `use crate::platform_types::*` 并 re-export，保证现有引用不破。

3. `tools/vision.rs` 对 `PlatformTurnContext` 的依赖用一个小 trait 替代：

```rust
pub trait PlatformImageFetch {
    fn fetch_context_image(&self, id: &str) -> BoxFuture<Result<PlatformImageData>>;
}
```

   `PlatformTurnContext` 实现该 trait；`tools` 只依赖 trait，不再 import `platforms` 运行时类型。

4. `agent`、`memory` 全部改为 `crate::platform_types`。

**验收**：

```bash
grep -R "use crate::platforms" src/tools src/memory src/agent | grep -v tests || echo OK
cargo test
```

## 5. 步骤 1.5：解开 `state ↔ memory` 循环

**现状**：`state` 用 `memory::EvictedTurn`，`memory/organizer` 用 `state::StateStore`。

**做法**（二选一，推荐 a）：

a. 把 `EvictedTurn` 及其纯类型移到新模块 `src/memory_types.rs`（或 `platform_types.rs` 同风格的基础类型层）；`state` 和 `memory` 都依赖它。
b. 更彻底但风险较高：在 `state` 定义 `EvictedTurnStore` trait，`memory` 实现它；本方案 Phase 1 不采用。

**验收**：`grep -R "use crate::memory" src/state` 与 `grep -R "use crate::state" src/memory` 只剩 trait/类型层白名单；`cargo test`。

## 6. 步骤 1.6：把 `CommandOutputStream` 从 tools 下放到 render

**现状**：`render/mod.rs` 依赖 `tools::CommandOutputStream`，只用于命令输出流的类型。该类型没有工具执行逻辑。

**做法**：把 `CommandOutputStream` 原样移动到 `render/command_output.rs`（或新建 `src/stream_types.rs`），`tools` re-export 以兼容外部引用。

**验收**：`render` 不再 import `crate::tools`；`cargo test`。

## 7. Phase 1 完成标准

- [ ] `src/lib.rs` 存在，main.rs 为薄入口。
- [ ] 以下 grep 全部通过：

```text
platforms 不 import web/cli
web       不 import cli
tools     不 import platforms 运行时
agent/memory 不 import platforms 运行时
render    不 import tools 执行类型
state ↔ memory 循环断开
```

- [ ] `cargo fmt --check`、`cargo check --all-targets`、`cargo test` 全绿。
- [ ] 黑盒冒烟与 Phase 0 基线一致。
- [ ] `arch_dep_check.py` 切为 fail 模式。
