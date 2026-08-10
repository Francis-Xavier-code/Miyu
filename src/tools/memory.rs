use super::{ToolRegistry, ToolSpec};
use crate::config::AppConfig;
use crate::i18n::agent_text as t;
use crate::memory::{MemoryAccess, MemoryStore};
use crate::paths::MiyuPaths;
use anyhow::{bail, Result};
use serde_json::{json, Value};

pub fn register(registry: &mut ToolRegistry, config: AppConfig, paths: MiyuPaths) {
    register_with_context(
        registry,
        config,
        paths,
        MemoryAccess::Privileged,
        None,
        String::new(),
    );
}

pub(crate) fn register_with_context(
    registry: &mut ToolRegistry,
    config: AppConfig,
    paths: MiyuPaths,
    access: MemoryAccess,
    writer_principal: Option<String>,
    writer_display_name: String,
) {
    if !config.memory_config().enabled {
        return;
    }
    register_readonly_with_context(
        registry,
        config.clone(),
        paths.clone(),
        access.clone(),
        writer_principal.clone(),
        writer_display_name.clone(),
    );
    registry.register(ToolSpec::new(
        "remember_fact",
        t("Save a durable memory fact or useful knowledge point for future association. Use only for reusable facts, preferences, methods, or stable discoveries.", "保存长期记忆事实或有用知识点，供之后联想使用。仅用于可复用事实、偏好、方法或稳定发现。"),
        json!({
            "type": "object",
            "properties": {
                "content": { "type": "string", "description": t("The concise fact or knowledge point to remember.", "要记住的简洁事实或知识点。") },
                "source": { "type": "string", "description": t("Optional source label.", "可选来源标签。") }
            },
            "required": ["content"],
            "additionalProperties": false
        }),
        {
            let config = config.clone();
            let paths = paths.clone();
            let access = access.clone();
            let writer_principal = writer_principal.clone();
            let writer_display_name = writer_display_name.clone();
            move |args| {
                let config = config.clone();
                let paths = paths.clone();
                let access = access.clone();
                let writer_principal = writer_principal.clone();
                let writer_display_name = writer_display_name.clone();
                async move {
                    remember_fact(
                        args,
                        config,
                        paths,
                        access,
                        writer_principal,
                        writer_display_name,
                    )
                    .await
                }
            }
        },
    ).writes());
}

pub fn register_readonly(registry: &mut ToolRegistry, config: AppConfig, paths: MiyuPaths) {
    register_readonly_with_context(
        registry,
        config,
        paths,
        MemoryAccess::Privileged,
        None,
        String::new(),
    );
}

pub(crate) fn register_readonly_with_context(
    registry: &mut ToolRegistry,
    config: AppConfig,
    paths: MiyuPaths,
    access: MemoryAccess,
    writer_principal: Option<String>,
    writer_display_name: String,
) {
    if !config.memory_config().enabled {
        return;
    }
    if config.memory_config().evicted_context_enabled && config.context.on_overflow != "compact" {
        registry.register(ToolSpec::new(
            "search_evicted_context",
            t("Search conversation turns that were moved out of the active context window. Use this when the current context appears to be missing earlier discussion.", "搜索已经移出当前上下文窗口的对话轮次。当当前上下文明显缺少早前讨论时使用。"),
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": t("Search keywords or question.", "搜索关键词或问题。") },
                    "max_results": { "type": "integer", "description": t("Optional result limit.", "可选结果数量限制。") }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            {
                let config = config.clone();
                let paths = paths.clone();
                let access = access.clone();
                let writer_principal = writer_principal.clone();
                let writer_display_name = writer_display_name.clone();
                move |args| {
                    let config = config.clone();
                    let paths = paths.clone();
                    let access = access.clone();
                    let writer_principal = writer_principal.clone();
                    let writer_display_name = writer_display_name.clone();
                    async move {
                        search_evicted_context(
                            args,
                            config,
                            paths,
                            access,
                            writer_principal,
                            writer_display_name,
                        )
                        .await
                    }
                }
            },
        ));
    }
    registry.register(ToolSpec::new(
        "recall_past_events",
        t("Search the assistant's diary-like memory of things that happened in previous conversations.", "搜索助手对过往对话事件的日记式记忆。"),
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": t("Search keywords or question.", "搜索关键词或问题。") },
                "max_results": { "type": "integer", "description": t("Optional result limit.", "可选结果数量限制。") }
            },
            "required": ["query"],
            "additionalProperties": false
        }),
        {
            let config = config.clone();
            let paths = paths.clone();
            let access = access.clone();
            let writer_principal = writer_principal.clone();
            let writer_display_name = writer_display_name.clone();
            move |args| {
                let config = config.clone();
                let paths = paths.clone();
                let access = access.clone();
                let writer_principal = writer_principal.clone();
                let writer_display_name = writer_display_name.clone();
                async move {
                    recall_past_events(
                        args,
                        config,
                        paths,
                        access,
                        writer_principal,
                        writer_display_name,
                    )
                    .await
                }
            }
        },
    ));
    registry.register(ToolSpec::new(
        "recall_memories",
        t("Search remembered facts and past events, including forgotten memories when requested. This read-only tool does not change memory state.", "搜索已记住的事实和过往事件；需要时也可包含已遗忘记忆。此只读工具不会改变记忆状态。"),
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": t("Search keywords or question.", "搜索关键词或问题。") },
                "max_results": { "type": "integer", "description": t("Optional result limit.", "可选结果数量限制。") },
                "include_forgotten": { "type": "boolean", "description": t("Whether to include forgotten memories.", "是否包含已遗忘记忆。") }
            },
            "required": ["query"],
            "additionalProperties": false
        }),
        {
            let config = config.clone();
            let paths = paths.clone();
            let access = access.clone();
            let writer_principal = writer_principal.clone();
            let writer_display_name = writer_display_name.clone();
            move |args| {
                let config = config.clone();
                let paths = paths.clone();
                let access = access.clone();
                let writer_principal = writer_principal.clone();
                let writer_display_name = writer_display_name.clone();
                async move {
                    recall_memories(
                        args,
                        config,
                        paths,
                        access,
                        writer_principal,
                        writer_display_name,
                    )
                    .await
                }
            }
        },
    ));
}

/// Records store RFC 3339 timestamps; the model may write a bare date or a
/// local wall-clock time. `end_of_day` makes a bare end date cover that day.
fn optional_time_bound(args: &Value, key: &str, end_of_day: bool) -> Result<Option<String>> {
    let Some(raw) = args.get(key).and_then(Value::as_str) else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Ok(Some(parsed.to_rfc3339()));
    }
    for format in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(value) = chrono::NaiveDateTime::parse_from_str(raw, format) {
            return Ok(Some(local_rfc3339(value)));
        }
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        let time = if end_of_day {
            date.and_hms_opt(23, 59, 59)
        } else {
            date.and_hms_opt(0, 0, 0)
        }
        .ok_or_else(|| anyhow::anyhow!("date is outside the supported range"))?;
        return Ok(Some(local_rfc3339(time)));
    }
    anyhow::bail!("invalid {key} {raw:?}; use RFC 3339, YYYY-MM-DD, or YYYY-MM-DD HH:MM[:SS]")
}

fn local_rfc3339(value: chrono::NaiveDateTime) -> String {
    use chrono::TimeZone;
    chrono::Local
        .from_local_datetime(&value)
        .earliest()
        .map(|value| value.to_rfc3339())
        .unwrap_or_else(|| chrono::Utc.from_utc_datetime(&value).to_rfc3339())
}

async fn search_evicted_context(
    args: Value,
    config: AppConfig,
    paths: MiyuPaths,
    access: MemoryAccess,
    writer_principal: Option<String>,
    writer_display_name: String,
) -> Result<String> {
    let query = required_str(&args, "query")?;
    let limit = optional_limit(&args);
    let start = optional_time_bound(&args, "start_time", false)?;
    let end = optional_time_bound(&args, "end_time", true)?;
    let store = MemoryStore::new(&config, &paths).with_request_context(
        access,
        writer_principal,
        writer_display_name,
    );
    Ok(store
        .search_evicted_context_readonly(query, limit, start.as_deref(), end.as_deref())?
        .to_string())
}

async fn recall_past_events(
    args: Value,
    config: AppConfig,
    paths: MiyuPaths,
    access: MemoryAccess,
    writer_principal: Option<String>,
    writer_display_name: String,
) -> Result<String> {
    let query = required_str(&args, "query")?;
    let limit = optional_limit(&args);
    let store = MemoryStore::new(&config, &paths).with_request_context(
        access,
        writer_principal,
        writer_display_name,
    );
    Ok(store.recall_past_events_readonly(query, limit)?.to_string())
}

async fn remember_fact(
    args: Value,
    config: AppConfig,
    paths: MiyuPaths,
    access: MemoryAccess,
    writer_principal: Option<String>,
    writer_display_name: String,
) -> Result<String> {
    let content = required_str(&args, "content")?;
    let source = args
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or("conversation");
    let store = MemoryStore::new(&config, &paths).with_request_context(
        access,
        writer_principal,
        writer_display_name,
    );
    let id = store.remember_fact(content, source)?;
    Ok(json!({
        "ok": true,
        "id": id,
        "source": source.trim(),
        "content": content.trim(),
        "message": t("Memory saved. The saved content is included here so the current conversation can refer to it accurately.", "记忆已保存。这里包含已保存内容，方便当前对话准确引用。")
    })
    .to_string())
}

async fn recall_memories(
    args: Value,
    config: AppConfig,
    paths: MiyuPaths,
    access: MemoryAccess,
    writer_principal: Option<String>,
    writer_display_name: String,
) -> Result<String> {
    let query = required_str(&args, "query")?;
    let limit = optional_limit(&args);
    let include_forgotten = args
        .get("include_forgotten")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let store = MemoryStore::new(&config, &paths).with_request_context(
        access,
        writer_principal,
        writer_display_name,
    );
    Ok(store
        .recall_memories_readonly(query, limit, include_forgotten)?
        .to_string())
}

fn required_str<'a>(args: &'a Value, name: &str) -> Result<&'a str> {
    let value = args
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if value.is_empty() {
        bail!("{}: {name}", t("required argument missing", "缺少必需参数"));
    }
    Ok(value)
}

fn optional_limit(args: &Value) -> usize {
    args.get("max_results")
        .and_then(Value::as_u64)
        .unwrap_or(5)
        .clamp(1, 50) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    fn test_paths() -> MiyuPaths {
        let root = PathBuf::from("/tmp/miyu-memory-tool-test");
        MiyuPaths {
            config_dir: root.join("config"),
            config_file: root.join("config/config.jsonc"),
            skills_dir: root.join("config/skills"),
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            state_dir: root.join("state"),
            pictures_dir: root.join("pictures"),
            fish_hook_file: root.join("fish-hook.fish"),
            bash_hook_file: root.join("bash-hook.sh"),
            zsh_hook_file: root.join("zsh-hook.zsh"),
            scripts_dir: root.join("config/scripts"),
            system_scripts_dir: PathBuf::new(),
        }
    }

    fn tool_names(registry: &ToolRegistry) -> BTreeSet<String> {
        registry
            .lazy_definitions(&BTreeSet::new())
            .into_iter()
            .map(|definition| definition.function.name)
            .collect()
    }

    #[test]
    fn search_evicted_context_is_hidden_for_compact_overflow() {
        let paths = test_paths();
        let compact_config = AppConfig::default();
        let mut compact_registry = ToolRegistry::new();
        register_readonly(&mut compact_registry, compact_config, paths.clone());
        assert!(!tool_names(&compact_registry).contains("search_evicted_context"));

        let mut pop_config = AppConfig::default();
        pop_config.context.on_overflow = "pop".to_string();
        let mut pop_registry = ToolRegistry::new();
        register_readonly(&mut pop_registry, pop_config, paths);
        assert!(tool_names(&pop_registry).contains("search_evicted_context"));
    }
}
