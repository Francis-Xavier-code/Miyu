//! WebUI 的斜杠命令平面。
//!
//! 命令清单来自 `crate::slash_commands` 的同一张表（`GET /api/commands` 按
//! `web` 标记过滤），前端不维护第二份——两份清单迟早分叉。
//!
//! 这里只放**WebUI 里没有别的办法做到**的那几条对应的端点。会话增删改、换模
//! 型、改人格早就有 REST 与 GUI 入口，再给一条命令是两条路做同一件事。

use crate::runtime::*;
use crate::web::*;

/// 一条命令在前端补全菜单里需要的东西。
#[derive(serde::Serialize)]
pub(in crate::web) struct WebCommand {
    name: &'static str,
    arg_hint: &'static str,
    help: &'static str,
}

/// WebUI 输入框认得哪些命令。前端据此渲染 `/` 补全菜单。
///
/// 不在这张表里的 `/xxx` 前端一律当普通消息发给模型——与 REPL 同一语义
/// （见 `slash_commands::parse_repl_input`），两个界面不能分叉。
pub(in crate::web) async fn list_commands(
    State(state): State<DaemonState>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, ApiError> {
    require_auth(&headers, &state)?;
    let commands = crate::slash_commands::web_commands()
        .into_iter()
        .map(|spec| WebCommand {
            name: spec.name,
            arg_hint: spec.arg_hint,
            help: spec.help(),
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "ok": true, "commands": commands })))
}

/// `/compact`：立即压缩当前会话上下文。
///
/// 请求体与 `/api/conversation/reset` 同形（只有一个可选 session_id），直接
#[derive(Deserialize)]
pub(in crate::web) struct GoalCommandRequest {
    pub(in crate::web) session_id: String,
    #[serde(default)]
    pub(in crate::web) input: String,
}

/// `/goal ...` 的 WebUI 入口。
///
/// 和 REPL 走同一个 `execute_goal_command`——命令的语法、拒绝文案、以及
/// 「armed 只在 daemon 内存里」这条，两个界面必须是同一份实现。
pub(in crate::web) async fn goal_command_http(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Json(request): Json<GoalCommandRequest>,
) -> std::result::Result<Json<Value>, ApiError> {
    require_mutation(&headers, &state)?;
    require_local_web_session(&state, &request.session_id)?;
    let text =
        crate::tools::goal::execute_goal_command(&state.paths, &request.session_id, &request.input);
    crate::web::nudge_goal_driver(&state, &request.session_id);
    Ok(Json(json!({ "text": text })))
}

/// 复用 `ResetConversationRequest`，不再造一个字段一样的类型。
///
/// 与 `IpcCommand::Compact` 同一条 actor 路径，回合运行中一律拒绝——压缩会重
/// 写消息数组，正在跑的回合手里那份就成了悬空引用。
pub(in crate::web) async fn compact_conversation(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Json(request): Json<ResetConversationRequest>,
) -> std::result::Result<Json<Value>, ApiError> {
    require_mutation(&headers, &state)?;
    let session_id = request
        .session_id
        .unwrap_or_else(|| state.state_store.session_id().to_string());
    require_local_web_session(&state, &session_id)?;
    let session_id: std::sync::Arc<str> = session_id.into();
    reserve_admin_for_session(&state.manager, &session_id)?;
    let (reply, receiver) = oneshot::channel();
    if state
        .actor_tx
        .send(ActorCommand::Compact {
            session_id: session_id.clone(),
            reply,
        })
        .is_err()
    {
        release_admin(&state.manager);
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "agent worker is unavailable",
        ));
    }
    match receiver.await {
        Ok(Ok(data)) => Ok(Json(json!({ "ok": true, "result": data }))),
        Ok(Err(AdminFailure::Invalid(message))) => {
            Err(ApiError::new(StatusCode::CONFLICT, message))
        }
        Ok(Err(AdminFailure::Internal(message))) => Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            safe_error_message(&message),
        )),
        Err(_) => {
            release_admin(&state.manager);
            Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "agent worker stopped while compacting the conversation",
            ))
        }
    }
}

#[derive(serde::Deserialize)]
pub(in crate::web) struct ResetMemoryRequest {
    /// `"dev"` 走开发模式那份记忆；其余（含缺省）是普通模式的。
    #[serde(default)]
    mode: Option<String>,
}

/// `/reset-memory`：清空当前模式的长期记忆。
///
/// dev 的记忆挂在保留人格名下，钥匙必须与 Agent 构造时同一把 `dev_scoped()`，
/// 否则清的是另一份（与 `IpcCommand::ResetMemory` 同一段逻辑）。
pub(in crate::web) async fn reset_memory_http(
    State(state): State<DaemonState>,
    headers: HeaderMap,
    Json(request): Json<ResetMemoryRequest>,
) -> std::result::Result<Json<Value>, ApiError> {
    require_mutation(&headers, &state)?;
    let config = state.manager.lock().unwrap().config.clone();
    let config = if request.mode.as_deref() == Some("dev") {
        config.dev_scoped()
    } else {
        config
    };
    let memory = crate::memory::MemoryStore::new(&config, &state.paths);
    memory
        .reset_all(false)
        .map_err(|error| ApiError::internal(safe_error_message(&error)))?;
    Ok(Json(json!({ "ok": true })))
}
