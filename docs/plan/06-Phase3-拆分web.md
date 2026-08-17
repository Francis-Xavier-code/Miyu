# 06 · Phase 3：拆分 `src/web.rs`

`web.rs` 同时承担“daemon 宿主、HTTP 服务、actor、前端 API”。Phase 1 已把宿主共享类型移入 `runtime/`，本阶段把剩余 HTTP/actor 逻辑拆成 `src/web/` 目录。

## 1. 拆分顺序

1. 先拆无状态 helper 和 auth/assets。
2. 再拆 API handler。
3. 最后拆 actor/turn_task/config 热更新。
4. 测试块跟随函数移动。

## 2. `src/web/auth.rs`

搬入：

| 原位置 | 函数/类型 |
|---|---|
| 323–330 | `WebAuth` |
| 372–410 | 登录限流逻辑 |
| 4284–4362 | `auth_login`、`resolve_web_password` |
| 10515–10588 | `require_auth`、`require_mutation`、`cookie_value`、`origin_is_allowed`、`constant_time_eq`、`random_token` |

保持 cookie 名、登录窗口 60s、失败上限 5、SHA-256 摘要比较等行为不变。

## 3. `src/web/assets.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 3835–3912 | ETag、`embedded_asset`、`index_asset`、`styles_asset`、`theme_css`、`app_asset`、`logo_asset` |
| 3914–4038 | `media_mime`、`parse_byte_range`、`media_stream`、KaTeX 资源 |
| 4038–4283 | persona avatar、上传/校验/存储 persona 资产、`text_asset`/`binary_asset`/`finish_asset_response` |

注意 `include_str!/include_bytes!` 宏可以继续放本文件顶部。

## 4. `src/web/router.rs`

搬入 `router` 函数（3736–3832）与静态资源路由装配。拆分后 `mod.rs` 只做：

```rust
pub async fn run(...) { ... }
```

并把 `router(state)` 作为唯一路由装配点。

## 5. API handler 拆分

### `src/web/api_bootstrap.rs`

| 原位置 | 函数 |
|---|---|
| 4363–4369 | `health` |
| 4370–4518 | `bootstrap` |

### `src/web/api_config.rs`

| 原位置 | 函数 |
|---|---|
| 4519–4623 | `get_config`、`update_config` |
| 4624–4690 | `cleanup_persona_assets` |
| 8929–9441 | `config_response`、secret 红值/恢复/校验 |
| 9422–10003 | `validate_config_candidate`、`validate_prompt_documents`、`reconcile_qq_persona_references`、`prompt_configuration_changed` 等 |

### `src/web/api_sessions.rs`

| 原位置 | 函数 |
|---|---|
| 3040–3366 | 会话解析/列表/创建/更新/读取/删除/切换 |
| 3367–3735 | dev 会话、session JSON、turn mode |
| 5638–5711 | jobs/usage handler（或另建 `api_jobs.rs`） |

### `src/web/api_turns.rs`

| 原位置 | 函数 |
|---|---|
| 3492–3735 | `handle_ipc_turn`、`follow_run` |
| 5179–5637 | attachment 准备、redo、create_turn、queue/remove queue、jobs/cancel/question |
| 7070–7193 | `finish_turn_task`、标题精炼 |

### `src/web/api_media.rs`

| 原位置 | 函数 |
|---|---|
| 4691–5027 | image/artifact/user attachment 读写删 |
| 4897–5003 | attachment id/文件名/内容校验 |

### `src/web/api_events.rs`

| 原位置 | 函数 |
|---|---|
| 5028–5178 | `events`、`record_to_sse` |
| 625–719、840–1262 | `EventHub` 与 `RunEventMapper` 若 Phase 1 未完全下沉，则在此文件中收敛 |

## 6. `src/web/actor.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 6054–6550 | `spawn_actor`、`actor_loop`、StartTurn/RedoTurn/SetModels/ApplyConfig/Shutdown 分支 |
| 1966–2022 | 本地会话确保/可用性判断（或放 `api_sessions.rs`） |
| 2023–2692 | `start_ipc_server`、`handle_ipc_connection`、`handle_session_command` |
| 6496–6550 | `trim_process_memory` 等平台差异 helper |

## 7. `src/web/turn_task.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 6516–7070 | `into_pasted_images`、`run_turn_task` |
| 8417–8570 | `drop_cancelled_queue`、`finish_cancelled_run`、`finish_failed_run`、`publish_completed` |

该文件最终约 800–1000 行，是 web 中最核心的执行单元。

## 8. `src/web/prompts.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 9012–9230 | persona/identity、头像/看板路径解析 |
| 9466–10107 | prompt 文档读取、校验、备份、恢复、迁移 |
| 9782–10003 | persona scope 迁移/回滚 |

## 9. `src/web/models.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 7194–7328 | thinking variant 相关 |
| 7278–7362 | `rebuild_for_models` |
| 5810–5941 | thinking-variants/session models/active models API |
| 10108–10478 | `safe_models`、`web_display_config`、模式解析、model 校验 |

## 10. `src/web/config_apply.rs`

搬入 `rebuild_for_config` 及配置热应用相关：

| 原位置 | 函数 |
|---|---|
| 7329–7561 | `session_for_persona`、`rebuild_for_config` |
| 7562–7760 | 自动命名、会话切换、reset/clear actor 操作 |
| 9942–10003 | `config_change_requires_interrupt` 等（若未放 prompts/api_config） |

## 11. 测试拆分

`web.rs` 的测试块从 10632 行到文件末尾，按主题搬到：

- `web/tests/auth.rs`
- `web/tests/api_sessions.rs`
- `web/tests/api_turns.rs`
- `web/tests/actor.rs`
- `web/tests/config_apply.rs`
- `web/tests/event_mapper.rs`

测试模块只 `#[cfg(test)]`，不参与生产构建。

## 12. 验收

- [ ] `src/web.rs` 删除，`src/web/` 编译通过。
- [ ] 每个新文件 < 1500 行；`mod.rs` < 500 行。
- [ ] `cargo test web::` 全绿。
- [ ] WebUI 冒烟：启动、登录、bootstrap、会话列表、发送/流式、附件、Artifact、设置保存、redo/cancel、usage 面板与基线一致。
- [ ] `grep -R "use crate::web" src/platforms` 为空；`grep -R "use crate::cli" src/web` 为空。
