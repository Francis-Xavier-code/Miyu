use super::vision::ReferenceResolver;
use super::{vision, ToolRegistry, ToolSpec};
use crate::config::{AppConfig, ImageGenerationPluginConfig};
use crate::paths::MiyuPaths;
use anyhow::{bail, Context, Result};
use base64::Engine;
use chrono::Local;
use reqwest::Client;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const DESCRIPTION: &str = "Generate an image from a text prompt using the configured OpenAI or RightCode image API. Pass reference_images to work from existing pictures (image-to-image); each entry takes the same kind of reference vision_analyze accepts — a local image path, an http(s) image URL, or a context_image_N id from the chat history. Emits an image event and returns a local path. Messaging platforms deliver emitted images automatically, so do not pass the same image to send_message_to_user. Do not call print_image unless the user explicitly asks to display it.";

fn parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "prompt": { "type": "string", "description": "Image generation prompt." },
            "reference_images": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Optional reference images for image-to-image. Same references vision_analyze takes: local image paths, http(s) image URLs, or context_image_N ids."
            },
            "aspect_ratio": { "type": "string", "enum": ["自动", "1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9"], "description": "Optional aspect ratio override." },
            "resolution": { "type": "string", "enum": ["1K", "2K", "4K"], "description": "Optional resolution hint for RightCode." }
        },
        "required": ["prompt"],
        "additionalProperties": false
    })
}

pub fn register(registry: &mut ToolRegistry, config: AppConfig, paths: MiyuPaths) {
    let resolver = ReferenceResolver::unscoped(config.clone(), paths);
    register_with_resolver(registry, config, resolver);
}

/// 平台面的注册:参考图沿用 `vision_analyze` 的白名单作用域,由 vision 侧在
/// 每回合构造好同一份 state 后调进来。
pub(crate) fn register_scoped(
    registry: &mut ToolRegistry,
    config: AppConfig,
    resolver: ReferenceResolver,
) {
    register_with_resolver(registry, config, resolver);
}

fn register_with_resolver(
    registry: &mut ToolRegistry,
    config: AppConfig,
    resolver: ReferenceResolver,
) {
    let resolver = Arc::new(resolver);
    registry.register(ToolSpec::new_with_progress(
        "generate_image",
        DESCRIPTION,
        parameters(),
        move |args, progress| {
            let config = config.clone();
            let resolver = resolver.clone();
            async move { generate_image(args, config, resolver, progress).await }
        },
    ).writes());
}

async fn generate_image(
    args: Value,
    config: AppConfig,
    resolver: Arc<ReferenceResolver>,
    progress: crate::tools::ToolProgress,
) -> Result<String> {
    let plugin = &config.plugins.image_generation;
    if !plugin.enabled {
        bail!("image generation plugin is disabled")
    }
    let prompt = args
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if prompt.is_empty() {
        bail!("prompt is required")
    }
    let aspect_ratio = args
        .get("aspect_ratio")
        .and_then(Value::as_str)
        .unwrap_or(&plugin.default_aspect_ratio)
        .trim();
    let resolution = args
        .get("resolution")
        .and_then(Value::as_str)
        .unwrap_or(&plugin.default_resolution)
        .trim();
    let mut references = Vec::new();
    if let Some(items) = args.get("reference_images").and_then(Value::as_array) {
        for item in items.iter().filter_map(Value::as_str) {
            if item.trim().is_empty() {
                continue;
            }
            references.push(resolver.resolve(item).await?);
        }
    }
    let bytes = request_image(plugin, prompt, aspect_ratio, resolution, &references).await?;
    let path = save_image(plugin, prompt, &bytes)?;
    progress.report_image(path.clone(), prompt.to_string());
    let should_render_terminal = plugin.auto_print
        && config.plugins.print_image.enabled
        && progress.prepare_for_external_output().await;
    let print_error = if should_render_terminal {
        vision::print_image_file(
            &path,
            vision::configured_print_size(&config.plugins.print_image),
        )
        .await
        .err()
        .map(|err| err.to_string())
    } else {
        None
    };
    let printed = should_render_terminal && print_error.is_none();
    let path_text = path.display().to_string();
    // The reply used to be told to quote the absolute path back. On a chat
    // platform that put the host's home directory in front of strangers — and
    // bought nothing, because the image itself is delivered as its own
    // attachment, independent of the text.
    Ok(json!({
        "status": "ok",
        "path": path_text,
        "bytes": bytes.len(),
        "reference_images": references.len(),
        "printed": printed,
        "print_error": print_error,
        "assistant_instruction": if printed { PRINTED_INSTRUCTION } else { EMITTED_INSTRUCTION }
    })
    .to_string())
}

const PRINTED_INSTRUCTION: &str = "The generated image has already been emitted to the host and printed in the terminal. Do not call send_message_to_user or print_image for the same image unless the user explicitly asks to resend or redisplay it. Do not put the local file path in your reply.";

const EMITTED_INSTRUCTION: &str = "The generated image was saved to disk and emitted as an image event. Messaging platforms deliver emitted images automatically, so do not call send_message_to_user for the same image unless the user explicitly asks to resend it. Do not call print_image unless the user explicitly asks to display it. Do not put the local file path in your reply.";

/// 参考图的两条传法。两条都不支持时静默忽略参考图,退回纯文生图——多传了
/// 一个参数不该让整次调用失败(与 AstrBot 生图插件同款取舍)。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ReferenceMode {
    /// RightCode 兼容接口:/v1/images/generations 的 JSON 里带 image:[base64…]。
    GenerationsJson,
    /// OpenAI GPT Image:/v1/images/edits 走 multipart,字段名 image[]。
    EditsMultipart,
    Unsupported,
}

fn reference_mode(plugin: &ImageGenerationPluginConfig) -> ReferenceMode {
    if plugin.provider_type == "rightcode" {
        ReferenceMode::GenerationsJson
    } else if looks_gpt_image_model(&plugin.model) {
        ReferenceMode::EditsMultipart
    } else {
        ReferenceMode::Unsupported
    }
}

async fn request_image(
    plugin: &ImageGenerationPluginConfig,
    prompt: &str,
    aspect_ratio: &str,
    resolution: &str,
    references: &[(Vec<u8>, String)],
) -> Result<Vec<u8>> {
    let api_key = plugin
        .api_keys
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .find(|key| !key.is_empty())
        .context("plugins.image_generation.api_keys is empty")?;
    let base = plugin.base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        bail!("plugins.image_generation.base_url is empty")
    }
    let mode = if references.is_empty() {
        ReferenceMode::Unsupported
    } else {
        reference_mode(plugin)
    };
    if !references.is_empty() && mode == ReferenceMode::Unsupported {
        tracing::warn!(
            provider_type = %plugin.provider_type,
            model = %plugin.model,
            count = references.len(),
            "image generation backend does not accept reference images; falling back to text-to-image"
        );
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(plugin.timeout_seconds))
        .build()?;
    let request = match mode {
        ReferenceMode::EditsMultipart => {
            let mut form = reqwest::multipart::Form::new()
                .text("model", plugin.model.clone())
                .text("prompt", prompt.to_string())
                .text("n", "1");
            if let Some(size) = resolve_size(plugin, aspect_ratio, resolution) {
                form = form.text("size", size);
            }
            for (index, (data, mime)) in references.iter().enumerate() {
                let part = reqwest::multipart::Part::bytes(data.clone())
                    .file_name(format!("reference-{}", index + 1))
                    .mime_str(mime)
                    .with_context(|| format!("invalid reference image mime: {mime}"))?;
                form = form.part("image[]", part);
            }
            client
                .post(format!("{base}/v1/images/edits"))
                .bearer_auth(api_key)
                .multipart(form)
        }
        ReferenceMode::GenerationsJson | ReferenceMode::Unsupported => {
            let include = mode == ReferenceMode::GenerationsJson;
            client
                .post(format!("{base}/v1/images/generations"))
                .bearer_auth(api_key)
                .json(&payload(
                    plugin,
                    prompt,
                    aspect_ratio,
                    resolution,
                    include.then_some(references).unwrap_or(&[]),
                ))
        }
    };
    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        bail!("image API error ({status}): {}", preview(&text, 500));
    }
    let data: Value = response.json().await?;
    extract_image(&client, data).await
}

fn payload(
    plugin: &ImageGenerationPluginConfig,
    prompt: &str,
    aspect_ratio: &str,
    resolution: &str,
    references: &[(Vec<u8>, String)],
) -> Value {
    let rightcode = plugin.provider_type == "rightcode";
    let mut payload = json!({
        "model": plugin.model,
        "prompt": prompt,
        "n": 1,
    });
    if let Some(size) = resolve_size(plugin, aspect_ratio, resolution) {
        payload["size"] = Value::String(size);
    }
    if rightcode {
        if !aspect_ratio.is_empty() && aspect_ratio != "自动" {
            payload["aspect_ratio"] = Value::String(aspect_ratio.to_string());
        }
    }
    if !references.is_empty() {
        // RightCode 兼容接口收 base64 数组;带参考图时它回 url 而不是 b64。
        payload["image"] = Value::Array(
            references
                .iter()
                .map(|(data, _)| {
                    Value::String(base64::engine::general_purpose::STANDARD.encode(data))
                })
                .collect(),
        );
        payload["response_format"] = Value::String("url".to_string());
    } else if !rightcode && !looks_gpt_image_model(&plugin.model) {
        payload["response_format"] = Value::String("b64_json".to_string());
    }
    payload
}

async fn extract_image(client: &Client, response: Value) -> Result<Vec<u8>> {
    let data = response
        .get("data")
        .and_then(Value::as_array)
        .context("image response missing data array")?;
    let first = data.first().context("image response data is empty")?;
    if let Some(b64) = first.get("b64_json").and_then(Value::as_str) {
        return base64::engine::general_purpose::STANDARD
            .decode(b64)
            .context("failed to decode b64_json image");
    }
    if let Some(url) = first.get("url").and_then(Value::as_str) {
        let response = client.get(url).send().await?;
        let status = response.status();
        if !status.is_success() {
            bail!("failed to download generated image ({status})")
        }
        return Ok(response.bytes().await?.to_vec());
    }
    bail!("image response contains neither b64_json nor url")
}

fn save_image(plugin: &ImageGenerationPluginConfig, prompt: &str, bytes: &[u8]) -> Result<PathBuf> {
    let output_dir = expand_path(&plugin.output_dir);
    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create output dir {}", output_dir.display()))?;
    let filename = format!(
        "{}-{}.png",
        slug(prompt),
        Local::now().format("%Y%m%d-%H%M%S")
    );
    let path = output_dir.join(filename);
    std::fs::write(&path, bytes).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

fn resolve_size(
    plugin: &ImageGenerationPluginConfig,
    aspect_ratio: &str,
    resolution: &str,
) -> Option<String> {
    if plugin.provider_type == "rightcode" {
        return match (aspect_ratio, resolution) {
            ("1:1", "2K" | "4K") => Some("2048x2048".to_string()),
            ("1:1", _) => Some("1024x1024".to_string()),
            ("3:2", "2K" | "4K") => Some("2048x1365".to_string()),
            ("16:9", "2K" | "4K") => Some("2048x1152".to_string()),
            ("4:3", "2K" | "4K") => Some("2048x1536".to_string()),
            ("5:4", "2K" | "4K") => Some("2048x1638".to_string()),
            ("21:9", "2K" | "4K") => Some("2048x878".to_string()),
            ("2:3", "2K" | "4K") => Some("1365x2048".to_string()),
            ("3:4", "2K" | "4K") => Some("1536x2048".to_string()),
            ("9:16", "2K" | "4K") => Some("1152x2048".to_string()),
            ("4:5", "2K" | "4K") => Some("1638x2048".to_string()),
            ("3:2", _) => Some("1536x1024".to_string()),
            ("16:9", _) => Some("1536x864".to_string()),
            ("4:3", _) => Some("1365x1024".to_string()),
            ("5:4", _) => Some("1280x1024".to_string()),
            ("21:9", _) => Some("1536x658".to_string()),
            ("2:3", _) => Some("1024x1536".to_string()),
            ("3:4", _) => Some("1024x1365".to_string()),
            ("9:16", _) => Some("864x1536".to_string()),
            ("4:5", _) => Some("1024x1280".to_string()),
            _ => None,
        };
    }
    if looks_gpt_image_model(&plugin.model) {
        return Some(
            match aspect_ratio {
                "1:1" => "1024x1024",
                "2:3" | "3:4" | "9:16" | "4:5" => "1024x1536",
                "3:2" | "16:9" | "4:3" | "5:4" | "21:9" => "1536x1024",
                _ => "auto",
            }
            .to_string(),
        );
    }
    Some(
        match aspect_ratio {
            "2:3" | "3:4" | "9:16" | "4:5" => "1024x1792",
            "3:2" | "16:9" | "4:3" | "5:4" | "21:9" => "1792x1024",
            _ => "1024x1024",
        }
        .to_string(),
    )
}

fn looks_gpt_image_model(model: &str) -> bool {
    model.to_ascii_lowercase().contains("gpt-image")
}

fn expand_path(value: &str) -> PathBuf {
    if let Some(rest) = value.trim().strip_prefix("~/") {
        if let Some(home) = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()) {
            return home.join(rest);
        }
    }
    let path = Path::new(value.trim());
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        super::workspace::effective_workdir().join(path)
    }
}

fn slug(value: &str) -> String {
    let mut out = value
        .chars()
        .filter_map(|ch| {
            if ch.is_ascii_alphanumeric() {
                Some(ch.to_ascii_lowercase())
            } else if ch.is_ascii_whitespace() || matches!(ch, '-' | '_') {
                Some('-')
            } else {
                None
            }
        })
        .collect::<String>();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    let out = out.trim_matches('-');
    if out.is_empty() {
        "image".to_string()
    } else {
        out.chars().take(48).collect()
    }
}

fn preview(value: &str, limit: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= limit {
        normalized
    } else {
        format!("{}...", normalized.chars().take(limit).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(provider_type: &str, model: &str) -> ImageGenerationPluginConfig {
        let mut plugin = ImageGenerationPluginConfig::default();
        plugin.provider_type = provider_type.to_string();
        plugin.model = model.to_string();
        plugin
    }

    /// 参考图的两条线路各自认哪个后端;都不认时退回纯文生图而不是报错。
    #[test]
    fn reference_mode_follows_the_backend() {
        assert_eq!(
            reference_mode(&plugin("rightcode", "gpt-image-2")),
            ReferenceMode::GenerationsJson
        );
        assert_eq!(
            reference_mode(&plugin("openai", "gpt-image-1")),
            ReferenceMode::EditsMultipart
        );
        assert_eq!(
            reference_mode(&plugin("openai", "dall-e-3")),
            ReferenceMode::Unsupported
        );
    }

    /// 带参考图时 JSON 里要出现 base64 数组,并且改回 url 响应;
    /// 不带参考图时载荷与从前逐字节一致(不能因为加了这个功能就掰缓存)。
    #[test]
    fn generations_payload_carries_reference_images_only_when_given() {
        let plugin = plugin("rightcode", "gpt-image-2");
        let bare = payload(&plugin, "a cat", "1:1", "1K", &[]);
        assert!(bare.get("image").is_none());

        let with_reference = payload(
            &plugin,
            "a cat",
            "1:1",
            "1K",
            &[(vec![1, 2, 3], "image/png".to_string())],
        );
        let images = with_reference["image"].as_array().unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].as_str().unwrap(), "AQID");
        assert_eq!(with_reference["response_format"], "url");
    }

    #[test]
    fn the_reply_is_never_asked_to_quote_the_local_path() {
        // It used to be: the result carried `final_response_must_include_path`
        // and both instructions ordered the model to repeat it. On a chat
        // platform that published the host's home directory, and it bought
        // nothing — the image rides as its own attachment, not as text.
        for instruction in [PRINTED_INSTRUCTION, EMITTED_INSTRUCTION] {
            assert!(
                instruction.contains("Do not put the local file path in your reply"),
                "{instruction}"
            );
            assert!(
                !instruction.contains("include the exact local image path"),
                "{instruction}"
            );
            assert!(
                !instruction.contains("final_response_must_include_path"),
                "{instruction}"
            );
        }
    }
}
