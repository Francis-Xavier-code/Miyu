//! Per-turn workspace context. Tools resolve their working directory from a
//! task-local set by the running turn, so concurrently running turns with
//! different workspaces never interfere (std::env::current_dir is process
//! global). Outside a turn scope (direct CLI mode, tests) it falls back to
//! the process working directory.

use std::future::Future;
use std::path::PathBuf;

tokio::task_local! {
    static TURN_WORKSPACE: PathBuf;
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
            loop {
                tokio::select! {
                    result = &mut work => break result,
                    _ = std::future::ready(()) , if false => unreachable!(),
                }
            }
        })
        .await;
        assert_eq!(seen, workspace);
    }
}
