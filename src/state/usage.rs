use crate::llm::Usage;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

fn usage_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn write_state(path: &Path, state: &UsageState) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    writeln!(file, "{}", serde_json::to_string_pretty(state)?)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[derive(Default, Serialize, Deserialize)]
struct UsageState {
    requests: u64,
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
    #[serde(default)]
    conversation_tokens: u64,
    /// Cumulative provider-cache accounting (v7 Release 1). cache_read is the
    /// portion of prompt_tokens served from the provider's prompt cache.
    #[serde(default)]
    cache_read_tokens: u64,
    #[serde(default)]
    cache_write_tokens: u64,
    #[serde(default)]
    reasoning_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_conversation_usage: Option<Usage>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct UsageSnapshot {
    pub requests: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub conversation_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
    pub last_usage: Option<Usage>,
    pub last_conversation_usage: Option<Usage>,
}

impl From<UsageState> for UsageSnapshot {
    fn from(state: UsageState) -> Self {
        let last_conversation_usage = state
            .last_conversation_usage
            .clone()
            .or_else(|| state.last_usage.clone());
        Self {
            requests: state.requests,
            prompt_tokens: state.prompt_tokens,
            completion_tokens: state.completion_tokens,
            total_tokens: state.total_tokens,
            conversation_tokens: state.conversation_tokens,
            cache_read_tokens: state.cache_read_tokens,
            cache_write_tokens: state.cache_write_tokens,
            reasoning_tokens: state.reasoning_tokens,
            last_usage: state.last_usage,
            last_conversation_usage,
        }
    }
}

pub fn add_usage(path: &Path, usage: &Usage) -> Result<()> {
    add_usage_with_scope(path, usage, true)
}

pub fn add_auxiliary_usage(path: &Path, usage: &Usage) -> Result<()> {
    add_usage_with_scope(path, usage, false)
}

fn add_usage_with_scope(path: &Path, usage: &Usage, is_conversation: bool) -> Result<()> {
    let _guard = usage_lock().lock().unwrap();
    let mut state = if path.exists() {
        let raw = std::fs::read_to_string(path)?;
        serde_json::from_str(&raw).unwrap_or_default()
    } else {
        UsageState::default()
    };
    state.requests += 1;
    state.prompt_tokens += usage.prompt_tokens;
    state.completion_tokens += usage.completion_tokens;
    state.total_tokens += usage.effective_total_tokens();
    state.cache_read_tokens += usage.cache_read_tokens;
    state.cache_write_tokens += usage.cache_write_tokens;
    state.reasoning_tokens += usage.reasoning_tokens;
    if is_conversation {
        state.conversation_tokens += usage.effective_total_tokens();
    }
    state.last_usage = Some(usage.clone());
    if is_conversation {
        state.last_conversation_usage = Some(usage.clone());
    }
    write_state(path, &state)
}

pub fn snapshot(path: &Path) -> Result<UsageSnapshot> {
    let _guard = usage_lock().lock().unwrap();
    if !path.exists() {
        return Ok(UsageSnapshot::default());
    }
    let raw = std::fs::read_to_string(path)?;
    let state = serde_json::from_str::<UsageState>(&raw).unwrap_or_default();
    Ok(state.into())
}

pub fn clear_last_usage(path: &Path) -> Result<()> {
    let _guard = usage_lock().lock().unwrap();
    if !path.exists() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(path)?;
    let mut state = serde_json::from_str::<UsageState>(&raw).unwrap_or_default();
    state.last_usage = None;
    state.last_conversation_usage = None;
    write_state(path, &state)
}

pub fn reset_conversation(path: &Path) -> Result<()> {
    let _guard = usage_lock().lock().unwrap();
    if !path.exists() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(path)?;
    let mut state = serde_json::from_str::<UsageState>(&raw).unwrap_or_default();
    state.conversation_tokens = 0;
    state.last_usage = None;
    state.last_conversation_usage = None;
    write_state(path, &state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_clears_last_usage() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("usage.json");
        let usage = Usage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            ..Usage::default()
        };

        add_usage(&path, &usage).unwrap();
        let usage_snapshot = snapshot(&path).unwrap();
        assert_eq!(usage_snapshot.last_usage.unwrap().total_tokens, 15);
        assert_eq!(
            usage_snapshot
                .last_conversation_usage
                .unwrap()
                .prompt_tokens,
            10
        );

        clear_last_usage(&path).unwrap();
        let usage_snapshot = snapshot(&path).unwrap();
        assert_eq!(usage_snapshot.total_tokens, 15);
        assert!(usage_snapshot.last_usage.is_none());
        assert!(usage_snapshot.last_conversation_usage.is_none());
    }

    #[test]
    fn auxiliary_usage_does_not_replace_conversation_usage() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("usage.json");

        add_usage(
            &path,
            &Usage {
                prompt_tokens: 100,
                completion_tokens: 20,
                total_tokens: 120,
                ..Usage::default()
            },
        )
        .unwrap();
        add_auxiliary_usage(
            &path,
            &Usage {
                prompt_tokens: 5,
                completion_tokens: 2,
                total_tokens: 7,
                ..Usage::default()
            },
        )
        .unwrap();

        let snapshot = snapshot(&path).unwrap();
        assert_eq!(snapshot.total_tokens, 127);
        assert_eq!(snapshot.last_usage.unwrap().prompt_tokens, 5);
        assert_eq!(snapshot.last_conversation_usage.unwrap().prompt_tokens, 100);
    }

    #[test]
    fn total_tokens_falls_back_to_prompt_plus_completion() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("usage.json");

        add_usage(
            &path,
            &Usage {
                prompt_tokens: 7,
                completion_tokens: 3,
                total_tokens: 0,
                ..Usage::default()
            },
        )
        .unwrap();

        let snapshot = snapshot(&path).unwrap();
        assert_eq!(snapshot.total_tokens, 10);
        assert_eq!(snapshot.conversation_tokens, 10);
    }

    #[test]
    fn reset_conversation_preserves_global_total() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("usage.json");
        add_usage(
            &path,
            &Usage {
                prompt_tokens: 7,
                completion_tokens: 3,
                total_tokens: 10,
                ..Usage::default()
            },
        )
        .unwrap();

        reset_conversation(&path).unwrap();
        let snapshot = snapshot(&path).unwrap();
        assert_eq!(snapshot.total_tokens, 10);
        assert_eq!(snapshot.conversation_tokens, 0);
        assert!(snapshot.last_conversation_usage.is_none());
    }
}
