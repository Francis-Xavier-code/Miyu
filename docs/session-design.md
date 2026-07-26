# Miyu 多 Session 支持 · 最终设计方案

> 状态：待审阅（2026-07-26）
> 前置调研：5 个子系统（持久化 / Agent 核心 / CLI 入口 / Web·IPC·daemon / 配置·模型）已完成代码级调研，关键结论均有行号依据。

## 0. 已确认的产品决策

| # | 决策点 | 结论 |
|---|--------|------|
| 1 | 架构路线 | **daemon（miyud）成为唯一 core**，CLI/REPL/shell-hook 全部作为 IPC 瘦客户端；`MIYU_DIRECT` 仅保留为调试逃生口（降级为只能使用默认 session） |
| 2 | 存储 | **单一 conversation.db + session_id 列**，rebuild turns 表引入 `(session_id, seq)` 复合唯一 |
| 3 | session 归属 | **session 按 persona 分命名空间**；`reset_if_prompt_changed` 机制整体移除 |
| 4 | workspace | session 默认**不绑定** workspace；解析优先级：本次调用显式指定 > session 绑定 > 客户端 cwd 兜底 |
| 5 | 子代理 session | **持久化保留可审计**（kind='subagent'，默认隐藏，随父级联删除，7 天后台清理） |
| 6 | 多端同步 | **各端独立浏览**：切换 session 只影响发起端并更新全局指针；其他在线端不强制跳转 |
| 7 | 模型档位 | **仅作用于子代理与辅助任务**（compact、标题生成）；主对话模型完全由用户手选 |
| 8 | 其他默认 | usage 迁入 DB 按 session 归属（全局累计保留）；IPC bump v2；worker pool 默认并发 4；`/reset` 清 turns 保留 session 壳；归档=可逆 flag；删除=真删级联；删除 persona 文件不级联删其 session |

---

## 1. 总体架构

```
┌────────────┐  IPC (unix socket, v2)  ┌─────────────────────────────┐
│ miyu CLI    │◄───────────────────────►│ miyud (daemon, 唯一 core)    │
│ (REPL/一次性│                         │ ┌─────────────────────────┐ │
│  /shell-hook)                         │ │ SessionManager          │ │
└────────────┘                         │ │  - session CRUD/指针     │ │
┌────────────┐  HTTP + SSE             │ │  - 运行守卫/事件广播      │ │
│ WebUI       │◄───────────────────────►│ ├─────────────────────────┤ │
└────────────┘                         │ │ Agent (actor 线程)       │ │
                                       │ │ SubagentPool (信号量=4)  │ │
                                       │ ├─────────────────────────┤ │
                                       │ │ StateStore → SQLite v2   │ │
                                       │ └─────────────────────────┘ │
                                       └─────────────────────────────┘
```

- 现有 actor 模型（`web.rs:2308 spawn_actor` / `ActorCommand` channel）保留，**SessionManager 作为新模块 `src/session.rs` 挂在 daemon 内**，HTTP handler 和 IPC handler 统一经它调度——消除 reset/pop/compact/undo/start-turn 在 CLI direct 与 daemon actor 的双实现。
- CLI 侧已写好但未接线的 `run_remote_repl`（`cli.rs:3390`）接为正式路径；`run_direct_repl`、`run_chat_with_options` 直连路径删除（`MIYU_DIRECT` 保留一条最小直连仅供调试，锁死默认 session、无 session 管理命令）。
- 并发模型：**用户 session 的 turn 在 daemon 内全局串行**（沿用单 actor 现状，避免多 Agent 竞争）；**子代理并行**走 worker pool。不同用户 session 并行 turn 的能力由 schema 与守卫结构预留，本期不开放。

## 2. 数据模型（schema v2）

### 2.1 Migration 框架（新建，Phase 0）

- `PRAGMA user_version` 版本化；`src/state/migrations.rs` 维护 `[(version, fn)]` 有序数组，启动时在**独占事务**内依次执行未应用的迁移。
- v1 = 现状基线（把现有 `CREATE TABLE IF NOT EXISTS` + `add_column_if_missing` 逻辑收编为基线，`user_version` 0→1 不改数据）。
- v2 = session 化（见下）。迁移仅由 daemon 执行（唯一 core 保证了单进程迁移，无多进程竞态）。

### 2.2 新表

```sql
CREATE TABLE sessions (
    session_id        TEXT PRIMARY KEY,          -- ulid/时间戳+随机
    persona           TEXT NOT NULL,             -- 复用 memory 的 persona scope sanitize 规则
    name              TEXT NOT NULL,
    kind              TEXT NOT NULL DEFAULT 'user',   -- 'user' | 'subagent'
    parent_session_id TEXT REFERENCES sessions(session_id) ON DELETE CASCADE,
    workspace         TEXT,                      -- 可选绑定，可修改
    archived          INTEGER NOT NULL DEFAULT 0,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL,
    -- 子代理审计（kind='subagent' 时填写）
    provider_id       TEXT,
    model             TEXT,
    context_window    INTEGER,
    prompt_tokens     INTEGER NOT NULL DEFAULT 0,
    completion_tokens INTEGER NOT NULL DEFAULT 0,
    total_tokens      INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_sessions_persona_kind ON sessions(persona, kind, archived, updated_at);
CREATE INDEX idx_sessions_parent ON sessions(parent_session_id);

CREATE TABLE app_state (                          -- 通用 kv：当前指针、全局 usage 等
    key   TEXT PRIMARY KEY,                       -- 'current_session:<persona>' → session_id
    value TEXT NOT NULL
);
```

### 2.3 既有表改造

- **turns**：rebuild（唯一约束无法 ALTER）。新增 `session_id TEXT NOT NULL REFERENCES sessions ON DELETE CASCADE`、`workspace TEXT`（记录该 turn 实际执行时的 workspace，产品要求）；`seq UNIQUE` 改为 `UNIQUE(session_id, seq)`；索引改为 `(session_id, hidden, seq)`、`(session_id, is_summary, hidden, seq)`、`(session_id, status)`。
- **queued_prompts**：`ADD COLUMN session_id`，索引 `(session_id, status, seq)`。现有 `queue_session_id`（进程级队列路由）**保留原语义、更名为 `queue_owner_id`** 以杜绝与对话 session 混淆。
- **session_loaded_items**：rebuild，PK 改 `(session_id, kind, name)`——loaded tools 按 session 隔离。
- **image_assets / question_exchanges**：不加列，经 `turn_id` FK 级联即可。
- **usage**：`usage.json` 弃用。全局累计迁入 `app_state`（key `usage_totals`，JSON）；per-session 的 conversation_tokens 由 `turns.token_total` 按 session 聚合 + sessions 表冗余列维护。消除现有无锁 read-modify-write 的丢更新问题（`usage.rs:58-76`）。

### 2.4 v2 数据迁移

1. 创建 sessions/app_state 表。
2. 创建默认 session：`name="默认会话"`，`persona=当前 active_persona 的 scope`。
3. rebuild turns/session_loaded_items，把现有全部数据归入默认 session；queued_prompts 补 session_id。
4. `usage.json` 读入 `app_state.usage_totals` 后重命名为 `.bak`。
5. 写 `current_session:<persona>` 指针；删除 `prompt.sha256`（`reset_if_prompt_changed` 机制移除）。

## 3. SessionManager API（daemon 内，`src/session.rs`)

```rust
pub struct SessionManager { state: StateStore, events: EventHub, runs: RunGuard }

impl SessionManager {
    // CRUD（全部按 persona 过滤，kind='user' 且未归档为默认视图）
    fn list(&self, persona: &str, include_archived: bool) -> Vec<SessionSummary>;
    fn create(&self, persona: &str, name: Option<&str>) -> Session;      // 自动命名 "会话 N"
    fn rename(&self, id: &SessionId, name: &str) -> Result<()>;
    fn archive(&self, id) / fn unarchive(&self, id) -> Result<()>;
    fn delete(&self, id) -> Result<()>;                                   // 真删，级联
    fn reset(&self, id) -> Result<()>;                                    // 清 turns/queue/loaded，保留壳
    // 指针（决策 6：全局指针=默认入口，不强制在线端跳转）
    fn current(&self, persona: &str) -> SessionId;                        // 不存在则懒创建默认会话
    fn set_current(&self, persona: &str, id: &SessionId) -> Result<()>;
    // 守卫：目标 session 有 running turn 时，delete/reset/archive/切换 persona 等返回 Busy
    fn guard_mutation(&self, id: &SessionId) -> Result<MutationPermit>;
    // workspace
    fn set_workspace(&self, id, path: Option<PathBuf>) -> Result<()>;
    fn resolve_workspace(&self, id, request_cwd: Option<&Path>, explicit: Option<&Path>) -> PathBuf;
    // 子代理
    fn create_subagent(&self, parent: &SessionId, tier: ModelTier) -> Session;
    fn record_subagent_usage(&self, id, provider, model, ctx_window, usage);
}
```

- `RunGuard`：`HashMap<SessionId, RunHandle>`，daemon 内存态。`ActorCommand::StartTurn` 注册、结束注销；所有危险操作先 `guard_mutation`。
- 所有变更经 `EventHub` 广播（`session.created/renamed/archived/deleted/reset/current_changed`），事件体带 `session_id`。

## 4. 客户端语义

### 4.1 “当前 session” 与多端（决策 6）

- 全局指针（per-persona，存 `app_state`）是**默认入口**：新 REPL 启动、shell-hook、一次性 `miyu "..."`、WebUI 新标签 bootstrap 都从它取。
- 每个在线连接持有 **connection-local active session**（初始 = 全局指针）。`/session` 切换：更新本端视图 + 更新全局指针；其他在线端收到 `session.current_changed` 事件仅作提示（TUI footer / WebUI 列表高亮变化），**不切换视图**。
- `--session <名称|ID>`：仅本次命令生效，不更新全局指针。
- 关闭 REPL 不改变指针；重开恢复全局指针指向的 session。

### 4.2 Slash 命令（TUI）

前置：把 direct/remote 两份 if 链（`cli.rs:3762-3959` / `3420-3668`）重构为**单一表驱动 dispatcher**（命令名 → handler + 帮助文本 + 补全），daemon-only 后只剩 remote 一份。

| 命令 | 语义 |
|------|------|
| `/new [名称]` | 创建并立即切换（更新全局指针） |
| `/session` | 列出当前 persona 的 user session（编号+名称+摘要+workspace，标注当前） |
| `/session <名称或编号>` | 切换 |
| `/rename <名称>` | 重命名当前 session |
| `/archive` | 归档当前 session 并切到默认（无则新建） |
| `/delete [名称或编号]` | 删除（默认当前，需确认；运行中拒绝） |
| `/reset` | 清空当前 session（保留壳与名字） |
| `/workspace [path\|clear]` | 查看/绑定/解绑当前 session 的 workspace |
| `/undo` `/pop` `/compact` | 语义不变，作用域限定当前 session |

切换 session 时 REPL 重置项（参照现有 `/reset` 分支 `cli.rs:3934-3958`）：input_history、editor 队列、cumulative_tokens、footer 上下文条、清屏 + 回放新 session 尾部若干 turn。

### 4.3 shell-hook / 一次性命令

- 均走 IPC，使用全局当前 session；每次请求携带 `cwd` 作为本次操作的 workspace 兜底（决策 4 优先级）。
- hook 脚本本身无需改动（`miyu --shell-intercept` 进程的 cwd 即 shell cwd，由 CLI 客户端打包进 IPC 请求）。

## 5. IPC 协议 v2（`src/ipc.rs`）

`PROTOCOL_VERSION = 2`，不兼容 v1（server 拒绝 v1 并提示升级；daemon 与 CLI 同包发布，实际不会跨版本）。

```rust
enum Command {
    // 既有命令统一加 scope
    StartTurn { session: SessionRef, content, mode, images, cwd: PathBuf },
    ResetSession { session: SessionRef }, Undo { session }, Pop { session, turn_ids },
    Compact { session }, Cancel { run_id },
    // 新增
    ListSessions { include_archived: bool },
    CreateSession { name: Option<String>, switch: bool },
    SwitchSession { target: SessionRef },          // 名称/编号/ID，更新全局指针
    RenameSession { session, name }, ArchiveSession { session },
    DeleteSession { session }, SetWorkspace { session, path: Option<PathBuf> },
    GetStatus,                                      // 返回 current session + workspace + running runs
    // Ping/Shutdown/ReloadConfig/AnswerQuestion 不变
}
enum SessionRef { Current, Id(String), Name(String) }
```

`Frame::Event` 增加 `session_id` 字段，客户端按自己的 active session 过滤渲染。

## 6. HTTP API / WebUI

- `GET/POST /api/sessions`、`PATCH/DELETE /api/sessions/{id}`、`POST /api/sessions/{id}/activate`（更新全局指针）、`POST /api/sessions/{id}/reset`。
- `GET /api/bootstrap` 增加 `sessions[]`、`current_session_id`；`turns` 按请求的 session 返回（`?session=` 参数，缺省=全局指针）。
- `POST /api/turns`、`/api/queue` 增加 `session_id`；SSE 事件带 `session_id`，前端按 viewing session 过滤；`capabilities.multi_conversation: true`。
- WebUI 侧边栏：单条“当前对话”展示（`app.js:91-94`）替换为会话列表（新建/切换/重命名/归档/删除/归档区折叠），顶栏显示当前 session 名 + workspace。TUI footer 同步显示 `session名 · workspace`。

## 7. WorkspaceContext 统一

```rust
pub struct WorkspaceContext { pub root: PathBuf, pub source: WorkspaceSource } // Explicit | SessionBound | ClientCwd
```

- 注入 `ToolContext`，**15 处 `std::env::current_dir()` 调用点**（write/edit_replace/apply_patch/patch_preview/glob/grep/trash/vision/image_generation/alarm/memes 等）全部改为 `ctx.workspace.root`。
- `runtime_context`（`agent/mod.rs:2408`）的 `cwd=` 改为 resolved workspace。
- turn 落库时写 `turns.workspace`。
- daemon 化后此项为硬需求（daemon 自身 cwd 无意义）。

## 8. 子代理（Phase 3）

- `SubagentRunner` 启动时经 `SessionManager::create_subagent` 建 `kind='subagent'` session（persona 继承父，`parent_session_id` 关联），其内部轮次以 turns 落库 → 可审计。
- **并发**：`tokio::Semaphore`（配置 `subagents.max_concurrent`，默认 4）；主循环对同一轮的多个 `task` tool call 改为 `join_all` 并行执行（其余工具保持串行）。
- **取消**：引入 `CancellationToken`，父 turn 被 Cancel 时级联取消全部子代理；保留现有双层 timeout。
- 完成后 `record_subagent_usage` 写 provider/model/context_window/token 聚合；结果汇总进父 turn 的 tool report（现状不变）。
- 清理：daemon 启动 + 每 24h 删除 7 天前的 subagent session（可配置）。

## 9. 模型档位（Phase 4）

```jsonc
"model_tiers": {
  "cheap":    { "provider_id": "...", "model": "..." },
  "balanced": { "provider_id": "...", "model": "..." },
  "strong":   { "provider_id": "...", "model": "..." }
  // 未配置的档位回退 active 主模型
}
```

- 实现复刻 vision 的“按角色建 client”范式（`config.rs:1584-1606`、`vision.rs:247-250`），无单例障碍。
- `task` 工具新增 `tier` 枚举参数，由主模型按任务复杂度选择（= “auto” 的含义）；缺省 explore→cheap、general→balanced。
- compact、会话标题自动生成（新增：session 首 turn 后用 cheap 模型生成默认名）走 cheap。
- 主对话模型不参与路由（决策 7）。pricing 数据缺口（models_cache 无价格)无需补——档位由用户显式配置。

## 10. 分期里程碑

| Phase | 内容 | 交付判定 |
|-------|------|---------|
| **0 前置** | migration 框架（user_version）；slash dispatcher 表驱动化；**daemon 收尾**：REPL/一次性/shell-hook 全部接 IPC，删 direct 双实现，MIYU_DIRECT 降级保留 | 全部入口经 daemon 跑通现有功能，行为无回归 |
| **1 session 核心** | schema v2 迁移；SessionManager；IPC v2；WorkspaceContext；当前指针语义 | 现有历史迁入默认 session；CLI 可用 `--session` 与全部 session 命令的后端 |
| **2 UI 一次性适配** | TUI slash 命令 + footer；WebUI 会话面板 + SSE 过滤 | 两端功能等价，共用 SessionManager API |
| **3 子代理** | subagent session + worker pool + 并行 task + 取消 + 审计/清理 | 多 task 并行、可取消、usage 落库 |
| **4 模型档位** | model_tiers 配置 + task tier 参数 + compact/标题走 cheap | 档位路由生效并有 usage 归属 |

每个 Phase 结束汇报（含把握程度与回归验证结果）后再进入下一个。

## 11. 风险与把握程度

| 项 | 风险 | 把握 |
|----|------|------|
| turns rebuild 迁移 | 用户数据一次性变换；需迁移前自动备份 `conversation.db.bak` + 迁移后校验行数 | 高（~90%），SQLite rebuild 是成熟模式 |
| daemon 收尾 | `run_remote_repl` 未经实战，REPL 体验（流式渲染/排队/追问）经 IPC 的等价性需逐项验证 | 中高（~75%），最大不确定点；Phase 0 单独交付并回归 |
| 多端事件过滤 | SSE/IPC 事件加 session_id 后前端过滤遗漏会串台 | 高（~85%） |
| 并行子代理 | 主循环 tool call 并行化改动 `chat_with_tools`（`agent/mod.rs:920-1385`），影响面大 | 中（~70%），限定只并行 task 类调用以控风险 |
| 档位路由 | 范式已有先例，风险低 | 高（~90%） |
