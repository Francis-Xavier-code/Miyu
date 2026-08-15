//! Per-turn workspace context. Tools resolve their working directory from a
//! task-local set by the running turn, so concurrently running turns with
//! different workspaces never interfere (std::env::current_dir is process
//! global). Outside a turn scope (direct CLI mode, tests) it falls back to
//! the process working directory.

use std::future::Future;
use std::path::PathBuf;

tokio::task_local! {
    static TURN_WORKSPACE: PathBuf;
    static TURN_SESSION: std::sync::Arc<str>;
}

/// Runs `future` with the given session id as the ambient turn session.
/// Subagents spawned inside the turn read it to link their audit sessions to
/// the parent.
pub async fn with_session<F: Future>(session_id: std::sync::Arc<str>, future: F) -> F::Output {
    TURN_SESSION.scope(session_id, future).await
}

/// The ambient turn session, if inside a turn scope.
pub fn try_session() -> Option<std::sync::Arc<str>> {
    TURN_SESSION.try_with(|session| session.clone()).ok()
}

/// Runs `future` with the given workspace as the ambient turn workspace.
pub async fn with_workspace<F: Future>(workspace: PathBuf, future: F) -> F::Output {
    TURN_WORKSPACE.scope(workspace, future).await
}

/// The directory tools should operate in: the ambient turn workspace, or the
/// process working directory outside a turn scope.
pub fn effective_workdir() -> PathBuf {
    try_workspace()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// The ambient turn workspace, if inside a turn scope.
pub fn try_workspace() -> Option<PathBuf> {
    TURN_WORKSPACE.try_with(|workspace| workspace.clone()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn effective_workdir_returns_scoped_workspace() {
        let workspace = PathBuf::from("/tmp/miyu-turn-workspace");
        let seen = with_workspace(workspace.clone(), async { effective_workdir() }).await;
        assert_eq!(seen, workspace);
    }

    #[tokio::test]
    async fn effective_workdir_falls_back_to_process_cwd_outside_scope() {
        assert_eq!(try_workspace(), None);
        let cwd = std::env::current_dir().expect("process cwd");
        assert_eq!(effective_workdir(), cwd);
    }

    #[tokio::test]
    async fn workspace_visible_inside_select_nested_future() {
        let workspace = PathBuf::from("/tmp/miyu-select-workspace");
        let seen = with_workspace(workspace.clone(), async {
            let work = async { effective_workdir() };
            tokio::pin!(work);
            tokio::select! {
                result = &mut work => result,
                _ = std::future::ready(()) , if false => unreachable!(),
            }
        })
        .await;
        assert_eq!(seen, workspace);
    }
}

tokio::task_local! {
    /// 触发本回合的终端身份(shellhook/单次 CLI),供后台任务捕获,
    /// 完成后把跟进回复写回原终端。
    static ORIGIN_TTY: Option<crate::ipc::OriginTty>;
}

pub async fn with_origin_tty<F>(origin: Option<crate::ipc::OriginTty>, future: F) -> F::Output
where
    F: std::future::Future,
{
    ORIGIN_TTY.scope(origin, future).await
}

pub fn current_origin_tty() -> Option<crate::ipc::OriginTty> {
    ORIGIN_TTY.try_with(|origin| origin.clone()).ok().flatten()
}

tokio::task_local! {
    /// 触发本回合的平台侧真实发起者(如 QQ user_id)。后台任务 spawn 时捕获,
    /// 完成唤醒的合成回合凭它继承发起者的身份与权限;不继承的话合成事件只能
    /// 伪装成机器人自己,is_admin=false 会把工具表降级成受限集合(issue #29)。
    static PLATFORM_SENDER: Option<String>;
}

pub async fn with_platform_sender<F>(sender: Option<String>, future: F) -> F::Output
where
    F: std::future::Future,
{
    PLATFORM_SENDER.scope(sender, future).await
}

pub fn current_platform_sender() -> Option<String> {
    PLATFORM_SENDER.try_with(|sender| sender.clone()).ok().flatten()
}
