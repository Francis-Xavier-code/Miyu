use super::types::{OutboundMessage, OutboundOrigin, OutboundSegment};
use super::PlatformTurnContext;
use crate::i18n::agent_text as t;
use crate::tools::{ToolRegistry, ToolSpec};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_TOOL_IMAGES: usize = 4;
const MAX_TOOL_FILES: usize = 4;

pub(crate) fn register(registry: &mut ToolRegistry, context: Arc<PlatformTurnContext>) {
    let host_tools_allowed = context.host_tools_allowed();
    let parameters = if host_tools_allowed {
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": t("Optional text or Markdown.", "可选文本或 Markdown。") },
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
                "text": { "type": "string", "description": t("Text or Markdown to send.", "要发送的文本或 Markdown。") }
            },
            "required": ["text"],
            "additionalProperties": false
        })
    };
    registry.register(
        ToolSpec::new(
            "send_message_to_user",
            t(
                "Send text, local images, or local files to the current messaging-platform conversation. Use native attachments only for content that another tool has not already emitted in this turn; the host delivers tool-emitted images automatically. This tool cannot target another conversation.",
                "向当前通讯平台会话发送文本、本地图片或本地文件。仅发送本轮中尚未由其他工具发布的原生附件；工具已发布的图片会由宿主自动投递。此工具不能指定其他会话。",
            ),
            parameters,
            move |arguments| {
                let context = context.clone();
                async move { send(arguments, context).await }
            },
        )
        .writes()
        .with_display_name(t("Send message", "发送消息")),
    );
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
