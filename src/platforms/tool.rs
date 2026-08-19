use super::PlatformTurnContext;
use crate::platform_types::{ConversationKind, OutboundMessage, OutboundOrigin, OutboundSegment};
use crate::tools::{ToolRegistry, ToolSpec};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
const MAX_TOOL_IMAGES: usize = 4;
const MAX_TOOL_FILES: usize = 4;
const MAX_TOOL_MENTIONS: usize = 32;

pub(crate) fn register(registry: &mut ToolRegistry, context: Arc<PlatformTurnContext>) {
    if context.conversation.kind == ConversationKind::Group {
        register_mention(registry, context.clone());
    }
    register_usage_query(registry, context.clone());
    let host_tools_allowed = context.host_tools_allowed();
    let parameters = if host_tools_allowed {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "Optional text or Markdown." },
                "images": {
                    "type": "array",
                    "maxItems": MAX_TOOL_IMAGES,
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "alt": { "type": "string" }
                        },
                        "required": ["path"],
                        "additionalProperties": false
                    }
                },
                "files": {
                    "type": "array",
                    "maxItems": MAX_TOOL_FILES,
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "name": { "type": "string" }
                        },
                        "required": ["path"],
                        "additionalProperties": false
                    }
                }
            },
            "additionalProperties": false
        })
    } else {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "Text or Markdown to send." }
            },
            "required": ["text"],
            "additionalProperties": false
        })
    };
    registry.register(
        ToolSpec::new(
            "send_message_to_user",
            "Send text, local images, or local files to the current messaging-platform conversation. Use native attachments only for content that another tool has not already emitted in this turn; the host delivers tool-emitted images automatically. This tool cannot target another conversation.",
            parameters,
            move |arguments| {
                let context = context.clone();
                async move { send(arguments, context).await }
            },
        )
        .writes()
        .with_display_name("Send message"),
    );
}

fn register_mention(registry: &mut ToolRegistry, context: Arc<PlatformTurnContext>) {
    registry.register(
        ToolSpec::new(
            "qq_mention_users",
            "Make Miyu's next outgoing message in the current QQ group natively @ one or more members. Provide exact QQ IDs; use get_group_members_info first when only names are known. These explicit targets replace the automatic reply mention but preserve its message-quote behavior. This tool does not send a separate message.",
            json!({
                "type": "object",
                "properties": {
                    "user_ids": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": MAX_TOOL_MENTIONS,
                        "items": {
                            "type": "string",
                            "pattern": "^[1-9][0-9]{4,11}$"
                        },
                        "description": "Exact QQ IDs of the members to mention, in display order."
                    }
                },
                "required": ["user_ids"],
                "additionalProperties": false
            }),
            move |arguments| {
                let context = context.clone();
                async move { mention(arguments, &context).await }
            },
        )
        .writes()
        .with_display_name("Mention group members"),
    );
}

async fn mention(arguments: Value, context: &PlatformTurnContext) -> Result<String> {
    if context.conversation.kind != ConversationKind::Group {
        bail!("mentioning users is only available in a group conversation");
    }
    let object = arguments
        .as_object()
        .context("arguments must be an object containing only user_ids")?;
    if object.len() != 1 || !object.contains_key("user_ids") {
        bail!("only user_ids may be specified");
    }
    let raw_user_ids = array(&arguments, "user_ids")?;
    if raw_user_ids.is_empty() {
        bail!("user_ids must contain at least one QQ ID");
    }
    if raw_user_ids.len() > MAX_TOOL_MENTIONS {
        bail!("at most {MAX_TOOL_MENTIONS} users may be mentioned at once");
    }

    let mut seen = HashSet::new();
    let mut user_ids = Vec::with_capacity(raw_user_ids.len());
    for (index, value) in raw_user_ids.iter().enumerate() {
        let user_id = value
            .as_str()
            .filter(|user_id| !user_id.is_empty())
            .with_context(|| format!("user_ids[{index}] must be a QQ ID string"))?;
        if !(5..=12).contains(&user_id.len())
            || !user_id
                .as_bytes()
                .first()
                .is_some_and(|byte| matches!(byte, b'1'..=b'9'))
            || !user_id.bytes().all(|byte| byte.is_ascii_digit())
        {
            bail!("user_ids[{index}] must be a 5-12 digit QQ ID");
        }
        let user_id = user_id.to_string();
        if seen.insert(user_id.clone()) {
            user_ids.push(user_id);
        }
    }
    let member_ids = context
        .group_members()
        .await
        .context("looking up current group members before mentioning them")?
        .into_iter()
        .map(|member| member.user_id)
        .collect::<HashSet<_>>();
    let missing = user_ids
        .iter()
        .filter(|user_id| !member_ids.contains(*user_id))
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        bail!(
            "these QQ IDs are not members of the current group: {}",
            missing.join(", ")
        );
    }
    context.set_explicit_response_mentions(user_ids.clone());

    Ok(json!({
        "ok": true,
        "user_ids": user_ids,
        "conversation": context.conversation.scope_key(),
        "message": "The next outgoing message will mention these members. Write that message normally."
    })
    .to_string())
}

async fn send(arguments: Value, context: Arc<PlatformTurnContext>) -> Result<String> {
    let mut segments = Vec::new();
    if let Some(text) = arguments
        .get("text")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
    {
        segments.push(OutboundSegment::Markdown(text.to_string()));
    }
    let images = array(&arguments, "images")?;
    if images.len() > MAX_TOOL_IMAGES {
        bail!("at most {MAX_TOOL_IMAGES} images may be sent at once");
    }
    let files = array(&arguments, "files")?;
    if files.len() > MAX_TOOL_FILES {
        bail!("at most {MAX_TOOL_FILES} files may be sent at once");
    }
    if (!images.is_empty() || !files.is_empty()) && !context.host_tools_allowed() {
        bail!("local attachments require an authorized platform administrator");
    }
    for image in images {
        let path = required_path(image, "path")?;
        let alt = image
            .get("alt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        segments.push(OutboundSegment::ImagePath { path, alt });
    }
    for file in files {
        let path = required_path(file, "path")?;
        let name = file
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        segments.push(OutboundSegment::FilePath { path, name });
    }
    if segments.is_empty() {
        bail!("text, images, or files is required");
    }
    let receipt = context
        .send(OutboundMessage::segments(OutboundOrigin::Tool, segments))
        .await?;
    Ok(json!({
        "ok": true,
        "message_ids": receipt.message_ids,
        "conversation": context.conversation.scope_key(),
    })
    .to_string())
}

fn array<'a>(arguments: &'a Value, key: &str) -> Result<&'a [Value]> {
    match arguments.get(key) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(values)) => Ok(values),
        Some(_) => bail!("{key} must be an array"),
    }
}

fn required_path(value: &Value, key: &str) -> Result<PathBuf> {
    let raw = value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .with_context(|| format!("{key} is required"))?;
    let path = if let Some(home) = raw.strip_prefix("~/") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("~"))
            .join(home)
    } else {
        PathBuf::from(raw)
    };
    let path = if path.is_absolute() {
        path
    } else {
        crate::tools::workspace::effective_workdir().join(path)
    };
    let metadata = std::fs::metadata(&path)
        .with_context(|| format!("reading attachment metadata: {}", path.display()))?;
    if !metadata.is_file() {
        bail!("attachment is not a regular file: {}", path.display());
    }
    Ok(Path::new(&path).to_path_buf())
}

/// 平台侧的 token 消耗查询:读 usage-history 聚合出适合聊天气泡的摘要。
/// 口径与 WebUI 控制台一致——智能体与各平台的全部 LLM 请求都在账上。
fn register_usage_query(registry: &mut ToolRegistry, context: Arc<PlatformTurnContext>) {
    registry.register(
        ToolSpec::new(
            "query_token_usage",
            "Query Miyu's token usage statistics: totals, request count, cache hit rate, and the per-source (agent / messaging platforms) model breakdown. range: 1d (rolling 24h, default) / 7d / 30d / all.",
            json!({
                "type": "object",
                "properties": {
                    "range": {
                        "type": "string",
                        "enum": ["1d", "7d", "30d", "all"],
                        "description": "Time range, defaults to 1d (rolling 24h)."
                    }
                },
                "additionalProperties": false
            }),
            move |arguments| {
                let context = context.clone();
                async move { query_token_usage(arguments, context).await }
            },
        )
        .with_display_name("Token usage"),
    );
}

async fn query_token_usage(arguments: Value, context: Arc<PlatformTurnContext>) -> Result<String> {
    let range_key = arguments
        .get("range")
        .and_then(Value::as_str)
        .unwrap_or("1d")
        .to_string();
    let range = crate::state::UsageRange::parse(&range_key);
    let store = context.state_store.clone();
    let config = context.config.clone();
    let stats = tokio::task::spawn_blocking(move || store.usage_stats(range, Some(&config)))
        .await
        .context("usage stats task panicked")??;
    Ok(crate::tools::usage_query::format_usage_summary(&stats, &range_key))
}
