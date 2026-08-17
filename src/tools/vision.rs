use super::{ToolRegistry, ToolSpec};
use crate::clipboard::write_image_cache_file;
use crate::config::{AppConfig, PrintImagePluginConfig};
use crate::i18n::agent_text as t;
use crate::llm::{ChatMessage, OpenAiCompatibleClient};
use crate::paths::MiyuPaths;
use crate::platforms::{PlatformContextImageRef, PlatformImageData, PlatformTurnContext};
use anyhow::{bail, Context, Result};
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::future::Future;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::process::Command;

const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_SCOPED_CONTEXT_FETCHES: usize = 4;
const MAX_SCOPED_TOTAL_BYTES: usize = 20 * 1024 * 1024;
const MAX_SCOPED_VISION_CALLS: usize = 6;

#[derive(Clone, Debug)]
struct ResolvedContextImage {
    image: PlatformImageData,
    digest: String,
    cache_path: PathBuf,
}

struct ScopedVisionState {
    allowed_paths: Vec<PathBuf>,
    context_images: HashMap<String, PlatformContextImageRef>,
    platform_context: Option<Arc<PlatformTurnContext>>,
    allow_general_access: bool,
    resolve_lock: tokio::sync::Mutex<()>,
    resolved: Mutex<HashMap<String, ResolvedContextImage>>,
    content_images: Mutex<HashMap<String, ResolvedContextImage>>,
    analyses: Mutex<HashMap<(String, String), String>>,
    calls: AtomicUsize,
    fetches: AtomicUsize,
    total_bytes: AtomicUsize,
}

pub fn register(
    registry: &mut ToolRegistry,
    config: AppConfig,
    paths: MiyuPaths,
    register_analyze: bool,
) {
    if !register_analyze {
        return;
    }
    registry.register(ToolSpec::new(
        "vision_analyze",
        t("Analyze an image using the current multimodal model or a configured vision provider. Supports local image paths and http(s) image URLs.", "使用当前多模态模型或配置的视觉 provider 分析图片。支持本地图片路径和 http(s) 图片 URL。"),
        json!({
            "type": "object",
            "properties": {
                "image": { "type": "string", "description": t("Local image path or http(s) image URL.", "本地图片路径或 http(s) 图片 URL。") },
                "prompt": { "type": "string", "description": t("Question or instruction for image analysis. Defaults to a concise description.", "图片分析问题或指令。默认简洁描述图片。") }
            },
            "required": ["image"],
            "additionalProperties": false
        }),
        move |args| {
            let config = config.clone();
            let paths = paths.clone();
            async move { analyze_image(args, config, paths).await }
        },
    ));
}

pub fn register_scoped_local(
    registry: &mut ToolRegistry,
    config: AppConfig,
    paths: MiyuPaths,
    allowed_images: Vec<PathBuf>,
) {
    register_scoped(
        registry,
        config,
        paths,
        allowed_images,
        Vec::new(),
        None,
        false,
    );
}

pub fn register_scoped_platform(
    registry: &mut ToolRegistry,
    config: AppConfig,
    paths: MiyuPaths,
    allowed_images: Vec<PathBuf>,
    context_images: Vec<PlatformContextImageRef>,
    platform_context: Arc<PlatformTurnContext>,
) {
    let allow_general_access = platform_context.host_tools_allowed();
    register_scoped(
        registry,
        config,
        paths,
        allowed_images,
        context_images,
        Some(platform_context),
        allow_general_access,
    );
}

fn register_scoped(
    registry: &mut ToolRegistry,
    config: AppConfig,
    paths: MiyuPaths,
    allowed_images: Vec<PathBuf>,
    context_images: Vec<PlatformContextImageRef>,
    platform_context: Option<Arc<PlatformTurnContext>>,
    allow_general_access: bool,
) {
    let allowed_paths = allowed_images
        .into_iter()
        .filter_map(|path| path.canonicalize().ok())
        .collect::<Vec<_>>();
    let context_images = context_images
        .into_iter()
        .map(|image| (image.id.clone(), image))
        .collect::<HashMap<_, _>>();
    // Register even with an empty scope: keeping the tool pinned keeps the
    // provider-visible tools array byte-stable across turns (cache prefix).
    // Analysis calls against an empty scope fail with the existing clear
    // "not attached to the current platform turn" style errors.
    let state = Arc::new(ScopedVisionState {
        allowed_paths,
        context_images,
        platform_context,
        allow_general_access,
        resolve_lock: tokio::sync::Mutex::new(()),
        resolved: Mutex::new(HashMap::new()),
        content_images: Mutex::new(HashMap::new()),
        analyses: Mutex::new(HashMap::new()),
        calls: AtomicUsize::new(0),
        fetches: AtomicUsize::new(0),
        total_bytes: AtomicUsize::new(0),
    });
    // 生图的参考图与看图共用同一份作用域:两者都会把图片原样送到第三方,
    // 信任面必须一致(08-17)。只在插件启用时接管,否则保持工具不存在。
    if config.plugins.image_generation.enabled {
        super::image_generation::register_scoped(
            registry,
            config.clone(),
            ReferenceResolver {
                config: config.clone(),
                paths: paths.clone(),
                state: Some(state.clone()),
            },
        );
    }
    if !config.plugins.vision.enabled {
        // 只为生图的参考图建作用域:看图插件关着就不注册 vision_analyze。
        return;
    }
    registry.register(ToolSpec::new(
        "vision_analyze",
        "分析图片。image 可以是本轮提示中的图片路径或 context_image_N；历史上下文图片会按需获取。",
        json!({
            "type": "object",
            "properties": {
                "image": { "type": "string", "description": "本轮图片提示中列出的路径或历史图片 ID（如 context_image_1）。" },
                "prompt": { "type": "string", "description": "图片分析问题或指令。默认简洁描述图片。" }
            },
            "required": ["image"],
            "additionalProperties": false
        }),
        move |args| {
            let config = config.clone();
            let paths = paths.clone();
            let state = state.clone();
            async move { analyze_scoped_image(args, config, paths, state).await }
        },
    ));
    registry.amend_description(
        "vision_analyze",
        if allow_general_access {
            " 本轮历史图片 ID（context_image_N）会按需获取；也可继续使用普通本地路径或 URL。"
        } else {
            " 仅可分析当前消息、引用消息中的本轮路径，此前群聊记录里明确列出的 context_image_N，或群查询工具返回的 avatar_url 头像链接；不得使用其他路径或 URL。"
        },
    );
}

pub fn register_print(registry: &mut ToolRegistry, config: AppConfig) {
    if !config.plugins.print_image.enabled {
        return;
    }
    registry.register(ToolSpec::new_with_progress(
        "print_image",
        t("Print/render a local image directly in the current terminal output. Use this when the user asks to show, print, render, or preview an image, or when you need to inspect an image visually in the terminal before answering.", "在当前终端输出中直接打印/渲染本地图片。当用户要求显示、打印、渲染、预览图片，或回答前需要在终端中目视检查图片时使用。"),
        json!({
            "type": "object",
            "properties": {
                "image": { "type": "string", "description": t("Local image path.", "本地图片路径。") },
                "size": { "type": "string", "description": t("Optional chafa size, e.g. 80x40. Use this or width/height to avoid oversized output.", "可选 chafa 尺寸，例如 80x40。用它或 width/height 避免输出过大。") },
                "width": { "type": "integer", "description": t("Optional output width in terminal cells, e.g. 80.", "可选终端单元格输出宽度，例如 80。") },
                "height": { "type": "integer", "description": t("Optional output height in terminal cells, e.g. 40.", "可选终端单元格输出高度，例如 40。") }
            },
            "required": ["image"],
            "additionalProperties": false
        }),
        move |args, progress| {
            let print_config = config.plugins.print_image.clone();
            async move { print_image(args, &print_config, progress).await }
        },
    ));
}

async fn print_image(
    args: Value,
    print_config: &PrintImagePluginConfig,
    progress: crate::tools::ToolProgress,
) -> Result<String> {
    let image = args
        .get("image")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if image.is_empty() {
        bail!("{}", t("image is required", "缺少图片路径"))
    }
    let path = expand_path(image);
    let metadata = std::fs::metadata(&path).with_context(|| {
        format!(
            "{} {}",
            t("failed to stat image", "无法读取图片元数据"),
            path.display()
        )
    })?;
    if !metadata.is_file() {
        bail!(
            "{}: {}",
            t("image path is not a file", "图片路径不是文件"),
            path.display()
        )
    }
    // 模型显式要的尺寸要随事件带走:daemon 模式下真正画图的是终端那一侧,
    // 这里 print_image_file 的参数它看不见。
    progress.report_sized_image(
        path.clone(),
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("image"),
        requested_print_size(&args),
    );
    if progress.prepare_for_external_output().await {
        print_image_file(&path, print_size(&args, print_config)).await?;
    }
    Ok(format!(
        "{}: {}",
        t("printed image in terminal", "已在终端打印图片"),
        path.display()
    ))
}

pub async fn print_image_file(path: &Path, size: Option<String>) -> Result<()> {
    println!();
    io::stdout().flush()?;
    if crossterm::terminal::is_raw_mode_enabled().unwrap_or(false)
        && super::kitty_image::is_native_kitty_terminal()
        && super::kitty_image::supports_path(path)
    {
        super::kitty_image::print(path, size.as_deref())?;
        println!();
        io::stdout().flush()?;
        return Ok(());
    }
    let mut command = Command::new("chafa");
    if crossterm::terminal::is_raw_mode_enabled().unwrap_or(false) {
        command.args(["--probe", "off", "--relative", "off"]);
    }
    if let Some(size) = size {
        command.arg("--size").arg(size);
    }
    command.kill_on_drop(true);
    let status = command
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .with_context(|| "failed to run chafa; install chafa or disable terminal image printing")?;
    if !status.success() {
        bail!("chafa exited with status {status}")
    }
    println!();
    io::stdout().flush()?;
    Ok(())
}

pub fn configured_print_size(print_config: &PrintImagePluginConfig) -> Option<String> {
    let (cols, rows) = crossterm::terminal::size().ok()?;
    let width = ((cols as u32 * print_config.width_percent as u32) / 100).max(1);
    let height = ((rows as u32 * print_config.height_percent as u32) / 100).max(1);
    Some(format!("{}x{}", width.min(300), height.min(200)))
}

/// 模型显式要的尺寸，没要就是 None。
///
/// 和 `configured_print_size` 分开是因为两者只能在不同的地方解析：百分比
/// 依赖 `crossterm::terminal::size()`，daemon 量到的不是用户的终端，只能
/// 在 CLI 那侧算；而显式值只有 daemon 手里的工具参数才知道，必须随事件带
/// 过去，否则模型写了 width 也会被无声吃掉。
pub fn requested_print_size(args: &Value) -> Option<String> {
    let width = args
        .get("width")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(300);
    let height = args
        .get("height")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(200);
    match (width, height) {
        (0, 0) => args
            .get("size")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        (width, 0) => Some(format!("{width}x")),
        (0, height) => Some(format!("x{height}")),
        (width, height) => Some(format!("{width}x{height}")),
    }
}

fn print_size(args: &Value, print_config: &PrintImagePluginConfig) -> Option<String> {
    requested_print_size(args).or_else(|| configured_print_size(print_config))
}

async fn analyze_image(args: Value, config: AppConfig, paths: MiyuPaths) -> Result<String> {
    let vision = &config.plugins.vision;
    if !vision.enabled {
        bail!("vision plugin is disabled")
    }
    let image = args
        .get("image")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if image.is_empty() {
        bail!("image is required")
    }
    let prompt = args
        .get("prompt")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("请简洁描述这张图片，并指出重要细节。")
        .trim();
    let image_url = if image.starts_with("http://") || image.starts_with("https://") {
        image.to_string()
    } else {
        local_image_data_url(image)?
    };
    analyze_image_url_with_prompt(&config, &paths, &image_url, prompt).await
}

async fn analyze_scoped_image(
    args: Value,
    config: AppConfig,
    paths: MiyuPaths,
    state: Arc<ScopedVisionState>,
) -> Result<String> {
    let image = args
        .get("image")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if image.is_empty() {
        bail!("image is required")
    }
    if state.calls.fetch_add(1, Ordering::AcqRel) >= MAX_SCOPED_VISION_CALLS {
        bail!("vision_analyze call limit reached for the current platform turn")
    }
    let prompt = args
        .get("prompt")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("请简洁描述这张图片，并指出重要细节。")
        .trim();
    if state.context_images.contains_key(image) {
        let resolved = resolve_context_image(&paths, &state, image).await?;
        let cache_key = (resolved.digest.clone(), prompt.to_string());
        if let Some(cached) = state.analyses.lock().unwrap().get(&cache_key).cloned() {
            return Ok(cached);
        }
        let image_url = image_data_url(&resolved.image.mime, &resolved.image.data);
        let result = analyze_image_url_with_prompt(&config, &paths, &image_url, prompt).await?;
        state
            .analyses
            .lock()
            .unwrap()
            .insert(cache_key, result.clone());
        return Ok(result);
    }
    if state.allow_general_access {
        return analyze_image(args, config, paths).await;
    }
    if image.starts_with("http://") || image.starts_with("https://") {
        // QQ avatar URLs are built by our own tools from numeric IDs
        // (fixed host, digits-only parameters), so admitting them opens
        // no injection or exfiltration surface.
        if crate::platforms::avatar::is_trusted_avatar_url(image) {
            return analyze_image(args, config, paths).await;
        }
        bail!("only images attached to the current platform turn are allowed")
    }
    let image = expand_path(image)
        .canonicalize()
        .context("failed to resolve the requested image")?;
    if !state.allowed_paths.iter().any(|allowed| allowed == &image) {
        bail!("image is not attached to the current platform turn")
    }
    analyze_local_image_with_prompt(&config, &paths, &image, prompt).await
}

/// 生图参考图的引用解析器:把一个引用(本轮图片路径 / context_image_N /
/// 可信头像 URL / 普通本地路径或 URL)解析成图片字节与 MIME。
///
/// 作用域直接沿用 `vision_analyze` 的那一套:平台面只认本轮附带的图片、群
/// 聊记录里明确列出的 context_image_N、以及群查询工具返回的可信头像 URL;
/// 终端面(state=None)照旧接受任意本地路径与 http(s) URL。生图会把图片原样
/// 发到第三方 API,信任面必须和看图工具一致(08-17 决定)。
pub(crate) type ReferenceImage = (Vec<u8>, String);

pub(crate) struct ReferenceResolver {
    config: AppConfig,
    paths: MiyuPaths,
    state: Option<Arc<ScopedVisionState>>,
}

impl ReferenceResolver {
    pub(crate) fn unscoped(config: AppConfig, paths: MiyuPaths) -> Self {
        Self {
            config,
            paths,
            state: None,
        }
    }

    pub(crate) async fn resolve(&self, reference: &str) -> Result<ReferenceImage> {
        let reference = reference.trim();
        if reference.is_empty() {
            bail!("reference image must not be empty")
        }
        if let Some(state) = &self.state {
            if state.context_images.contains_key(reference) {
                let resolved = resolve_context_image(&self.paths, state, reference).await?;
                return Ok((resolved.image.data.to_vec(), resolved.image.mime.clone()));
            }
            if !state.allow_general_access {
                if reference.starts_with("http://") || reference.starts_with("https://") {
                    if crate::platforms::avatar::is_trusted_avatar_url(reference) {
                        return download_reference_image(reference).await;
                    }
                    bail!("only images attached to the current platform turn can be used as a reference")
                }
                let path = expand_path(reference)
                    .canonicalize()
                    .context("failed to resolve the requested reference image")?;
                if !state.allowed_paths.iter().any(|allowed| allowed == &path) {
                    bail!("reference image is not attached to the current platform turn")
                }
                return read_reference_file(&path);
            }
        }
        let _ = &self.config;
        if reference.starts_with("http://") || reference.starts_with("https://") {
            return download_reference_image(reference).await;
        }
        read_reference_file(&expand_path(reference))
    }
}

fn read_reference_file(path: &Path) -> Result<ReferenceImage> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("failed to stat reference image {}", path.display()))?;
    if !metadata.is_file() {
        bail!("reference image is not a file: {}", path.display())
    }
    if metadata.len() as usize > MAX_IMAGE_BYTES {
        bail!("reference image too large: {} bytes", metadata.len())
    }
    let mime = mime_from_path(path)?;
    let data = std::fs::read(path)
        .with_context(|| format!("failed to read reference image {}", path.display()))?;
    Ok((data, mime.to_string()))
}

async fn download_reference_image(url: &str) -> Result<ReferenceImage> {
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?
        .get(url)
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        bail!("failed to download reference image ({status})")
    }
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or(value).trim().to_string())
        .filter(|value| value.starts_with("image/"))
        .unwrap_or_else(|| "image/png".to_string());
    let data = response.bytes().await?.to_vec();
    if data.len() > MAX_IMAGE_BYTES {
        bail!("reference image too large: {} bytes", data.len())
    }
    Ok((data, mime))
}

async fn resolve_context_image(
    paths: &MiyuPaths,
    state: &ScopedVisionState,
    image_id: &str,
) -> Result<ResolvedContextImage> {
    if let Some(resolved) = state.resolved.lock().unwrap().get(image_id).cloned() {
        return Ok(resolved);
    }
    let _resolve_guard = state.resolve_lock.lock().await;
    if let Some(resolved) = state.resolved.lock().unwrap().get(image_id).cloned() {
        return Ok(resolved);
    }
    let source = state
        .context_images
        .get(image_id)
        .context("context image ID is not available in the current platform turn")?
        .clone();
    let context = state
        .platform_context
        .as_ref()
        .context("platform image lookup is unavailable")?;
    if state
        .fetches
        .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < MAX_SCOPED_CONTEXT_FETCHES).then_some(count + 1)
        })
        .is_err()
    {
        bail!("context image fetch limit reached for the current platform turn")
    }
    let images = match context.message_images_task(source.message_id.clone()).await {
        Ok(images) => images,
        Err(error) => {
            state.fetches.fetch_sub(1, Ordering::AcqRel);
            return Err(error).context("failed to retrieve the context image message");
        }
    };
    let image = match images.into_iter().nth(source.image_index.saturating_sub(1)) {
        Some(image) => image,
        None => {
            state.fetches.fetch_sub(1, Ordering::AcqRel);
            bail!("the requested context image is no longer available")
        }
    };
    if image.data.len() > MAX_IMAGE_BYTES {
        state.fetches.fetch_sub(1, Ordering::AcqRel);
        bail!("context image is too large: {} bytes", image.data.len())
    }
    let digest = hex::encode(Sha256::digest(&image.data));
    if let Some(existing) = state.content_images.lock().unwrap().get(&digest).cloned() {
        state
            .resolved
            .lock()
            .unwrap()
            .insert(image_id.to_string(), existing.clone());
        return Ok(existing);
    }
    let previous = state
        .total_bytes
        .fetch_add(image.data.len(), Ordering::AcqRel);
    if previous.saturating_add(image.data.len()) > MAX_SCOPED_TOTAL_BYTES {
        state
            .total_bytes
            .fetch_sub(image.data.len(), Ordering::AcqRel);
        state.fetches.fetch_sub(1, Ordering::AcqRel);
        bail!("context image byte limit reached for the current platform turn")
    }
    let cache_path = match write_image_cache_file(
        &paths.cache_dir,
        Path::new("platform_images/qq"),
        &image.mime,
        &image.data,
    ) {
        Ok(path) => path,
        Err(error) => {
            state
                .total_bytes
                .fetch_sub(image.data.len(), Ordering::AcqRel);
            state.fetches.fetch_sub(1, Ordering::AcqRel);
            return Err(error).context("failed to cache the context image");
        }
    };
    let resolved = ResolvedContextImage {
        image,
        digest,
        cache_path,
    };
    tracing::info!(
        target: "miyu::qq",
        image_id,
        message_id = %source.message_id,
        image_index = source.image_index,
        bytes = resolved.image.data.len(),
        cache_file = resolved
            .cache_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("image"),
        "{}",
        crate::i18n::text(
            "OneBot context image prepared on demand",
            "已按需准备 OneBot 上下文图片",
        )
    );
    state
        .resolved
        .lock()
        .unwrap()
        .insert(image_id.to_string(), resolved.clone());
    state
        .content_images
        .lock()
        .unwrap()
        .insert(resolved.digest.clone(), resolved.clone());
    Ok(resolved)
}

fn image_data_url(mime: &str, data: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(data);
    format!("data:{mime};base64,{encoded}")
}

pub async fn analyze_local_image_with_prompt(
    config: &AppConfig,
    paths: &MiyuPaths,
    image: &Path,
    prompt: &str,
) -> Result<String> {
    let image_url = local_image_data_url(&image.display().to_string())?;
    analyze_image_url_with_prompt(config, paths, &image_url, prompt).await
}

pub async fn analyze_image_url_with_prompt(
    config: &AppConfig,
    paths: &MiyuPaths,
    image_url: &str,
    prompt: &str,
) -> Result<String> {
    let vision = &config.plugins.vision;
    if !vision.enabled {
        bail!("vision plugin is disabled")
    }
    let client = vision_client(config, paths)?.with_request_timeouts(
        Duration::from_secs(vision.response_header_timeout_seconds.max(1)),
        Duration::from_secs(vision.stream_idle_timeout_seconds.max(1)),
    );
    let request = client.chat_stream(
        vec![
            ChatMessage::system("请基于图片内容回答，不要编造看不见的信息。"),
            ChatMessage::user_with_image(prompt, image_url.to_string()),
        ],
        Vec::new(),
        |_| Ok(()),
    );
    let result = with_image_timeout(vision.image_timeout_seconds, request).await?;
    if result.content.trim().is_empty() {
        bail!("vision model returned empty response")
    }
    Ok(result.content)
}

pub(crate) async fn with_image_timeout<T, F>(timeout_seconds: u64, future: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    let timeout = Duration::from_secs(timeout_seconds.max(1));
    tokio::time::timeout(timeout, future).await.map_err(|_| {
        anyhow::anyhow!(
            "vision model pool timed out after {} seconds",
            timeout.as_secs()
        )
    })?
}

/// 当前文本模型池自己就能看图时,`vision_analyze` 直接用它。
///
/// `prefer_current_multimodal_model` 此前只管一件事:粘贴进来的图片要不要
/// 内联发给聊天模型。`vision_analyze` 完全没看这个开关——哪怕当前文本模型
/// 自带眼睛,工具照旧把图发给另配的多模态池,既多一次跨模型往返,答案也来
/// 自一个没有对话上下文的模型(08-17 用户报的问题)。
///
/// 要求整池都支持图片输入:池是负载均衡的,只要有一个端点不认图片,这一路
/// 就可能随机落到它头上。
fn active_text_pool_for_vision(config: &AppConfig) -> Option<Vec<crate::config::ProviderModelChoice>> {
    if !config.plugins.vision.prefer_current_multimodal_model {
        return None;
    }
    let pool = config.active_provider_model_choices();
    let usable = !pool.is_empty()
        && pool.iter().all(|choice| {
            config.model_supports_any_input(&choice.provider_id, &choice.model, &["image"])
        });
    usable.then_some(pool)
}

fn vision_client(config: &AppConfig, paths: &MiyuPaths) -> Result<OpenAiCompatibleClient> {
    // An explicit global vision provider preserves its existing precedence.
    // Platform turns with a conversation override clear that single-provider
    // field in their private config clone, exposing the full routed pool here.
    if config.plugins.vision.vision_provider_id.trim().is_empty() {
        if let Some(text_pool) = active_text_pool_for_vision(config) {
            return OpenAiCompatibleClient::from_choices(config, paths, &text_pool)
                .map(|client| client.with_request_scope("vision"));
        }
        let choices = config
            .active_multimodal_provider_model_choices()
            .into_iter()
            .filter(|choice| {
                config.model_supports_any_input(&choice.provider_id, &choice.model, &["image"])
            })
            .collect::<Vec<_>>();
        if !choices.is_empty() {
            return OpenAiCompatibleClient::from_choices(config, paths, &choices)
                .map(|client| client.with_request_scope("vision"));
        }
    }
    let (provider_id, model) = config.vision_provider_choice()?;
    let mut provider = config.provider(Some(&provider_id))?.clone();
    provider.default_model = model;
    if !provider
        .models
        .iter()
        .any(|item| item == &provider.default_model)
    {
        provider.models.push(provider.default_model.clone());
    }
    OpenAiCompatibleClient::new(&provider, config, paths)
}

fn local_image_data_url(value: &str) -> Result<String> {
    let path = expand_path(value);
    let metadata = std::fs::metadata(&path)
        .with_context(|| format!("failed to stat image {}", path.display()))?;
    if !metadata.is_file() {
        bail!("image path is not a file: {}", path.display())
    }
    if metadata.len() as usize > MAX_IMAGE_BYTES {
        bail!("image too large: {} bytes", metadata.len())
    }
    let bytes =
        std::fs::read(&path).with_context(|| format!("failed to read image {}", path.display()))?;
    let mime = mime_from_path(&path)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:{mime};base64,{encoded}"))
}

fn expand_path(value: &str) -> PathBuf {
    let value = value.trim();
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()) {
            return home.join(rest);
        }
    }
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        super::workspace::effective_workdir().join(path)
    }
}

fn mime_from_path(path: &Path) -> Result<&'static str> {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => Ok("image/jpeg"),
        "png" => Ok("image/png"),
        "webp" => Ok("image/webp"),
        "gif" => Ok("image/gif"),
        value => {
            bail!("unsupported image extension: {value}; supported: jpg, jpeg, png, webp, gif")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ActiveProviderModelConfig;
    use crate::platforms::{
        ConversationKind, OutboundMessage, PlatformAdapter, PlatformConversation, SendReceipt,
    };
    use crate::state::StateStore;
    use futures_util::future::BoxFuture;

    struct ContextImageAdapter {
        calls: Arc<AtomicUsize>,
        images: Vec<PlatformImageData>,
    }

    impl PlatformAdapter for ContextImageAdapter {
        fn send<'a>(&'a self, _message: OutboundMessage) -> BoxFuture<'a, Result<SendReceipt>> {
            Box::pin(async { bail!("send is not used in this test") })
        }

        fn bot_display_name<'a>(&'a self) -> BoxFuture<'a, Result<String>> {
            Box::pin(async { Ok("Miyu".to_string()) })
        }

        fn message_images<'a>(
            &'a self,
            _message_id: &'a str,
        ) -> BoxFuture<'a, Result<Vec<PlatformImageData>>> {
            let calls = self.calls.clone();
            let images = self.images.clone();
            Box::pin(async move {
                tokio::task::yield_now().await;
                calls.fetch_add(1, Ordering::AcqRel);
                Ok(images)
            })
        }
    }

    fn test_paths(root: &Path) -> MiyuPaths {
        MiyuPaths {
            root_dir: root.to_path_buf(),
            config_dir: root.join("config"),
            config_file: root.join("config/config.jsonc"),
            skills_dir: root.join("config/skills"),
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            state_dir: root.join("state"),
            pictures_dir: root.join("pictures"),
            fish_hook_file: root.join("fish"),
            bash_hook_file: root.join("bash"),
            zsh_hook_file: root.join("zsh"),
            scripts_dir: root.join("scripts"),
            system_scripts_dir: root.join("system-scripts"),
        }
    }

    /// 平台回合的作用域不能只由看图插件把门:生图的参考图共用同一份作用域,
    /// vision 关、生图开时若不建作用域,generate_image 会留着不受限的解析器。
    #[test]
    fn scoped_registration_binds_image_generation_even_without_vision() {
        let temp = tempfile::tempdir().unwrap();
        let paths = crate::paths::MiyuPaths {
            root_dir: temp.path().to_path_buf(),
            config_dir: temp.path().join("config"),
            config_file: temp.path().join("config/config.jsonc"),
            skills_dir: temp.path().join("config/skills"),
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            state_dir: temp.path().join("state"),
            pictures_dir: temp.path().join("pictures"),
            fish_hook_file: temp.path().join("fish"),
            bash_hook_file: temp.path().join("bash"),
            zsh_hook_file: temp.path().join("zsh"),
            scripts_dir: temp.path().join("config/scripts"),
            system_scripts_dir: temp.path().join("system-scripts"),
        };
        let mut config = AppConfig::default();
        config.plugins.vision.enabled = false;
        config.plugins.image_generation.enabled = true;

        let mut registry = ToolRegistry::new();
        register_scoped_local(&mut registry, config, paths, Vec::new());
        // 看图插件关着 ⇒ 不注册 vision_analyze,但生图必须换成带作用域的版本。
        assert!(!registry.contains("vision_analyze"));
        assert!(registry.contains("generate_image"));
    }

    /// 当前文本模型自己能看图时就用它,不再绕道另配的多模态池。
    #[test]
    fn vision_uses_the_active_text_pool_when_it_can_see() {
        let mut config = AppConfig::default();
        let provider = config.providers.first_mut().unwrap();
        let provider_id = provider.id.clone();
        provider.model_modalities.insert(
            provider.default_model.clone(),
            vec!["text".to_string(), "image".to_string()],
        );
        provider
            .model_modalities
            .insert("blind-model".to_string(), vec!["text".to_string()]);
        provider.models.push("blind-model".to_string());
        assert!(active_text_pool_for_vision(&config).is_some());

        // 开关关掉就走原路。
        config.plugins.vision.prefer_current_multimodal_model = false;
        assert!(active_text_pool_for_vision(&config).is_none());
        config.plugins.vision.prefer_current_multimodal_model = true;

        // 池里只要混进一个不认图片的端点就不能用:负载均衡会随机落到它。
        config.active_provider_models = Some(vec![
            ActiveProviderModelConfig {
                provider_id: provider_id.clone(),
                model: config.providers[0].default_model.clone(),
            },
            ActiveProviderModelConfig {
                provider_id,
                model: "blind-model".to_string(),
            },
        ]);
        assert!(active_text_pool_for_vision(&config).is_none());
    }

    #[tokio::test]
    async fn image_timeout_cancels_a_stalled_model_pool() {
        let error = with_image_timeout(1, std::future::pending::<Result<()>>())
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "vision model pool timed out after 1 seconds"
        );
    }

    #[tokio::test]
    async fn context_images_reuse_resolved_ids_and_duplicate_content_cache() {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_paths(temp.path());
        let calls = Arc::new(AtomicUsize::new(0));
        let adapter = Arc::new(ContextImageAdapter {
            calls: calls.clone(),
            images: vec![PlatformImageData {
                mime: "image/png".to_string(),
                data: Arc::from(vec![1_u8, 2, 3]),
            }],
        });
        let context = Arc::new(PlatformTurnContext::new(
            PlatformConversation {
                platform: "onebot".to_string(),
                account_id: "10000".to_string(),
                kind: ConversationKind::Group,
                conversation_id: "20000".to_string(),
            },
            "30000".to_string(),
            "tester".to_string(),
            false,
            AppConfig::default(),
            paths.clone(),
            StateStore::new(&paths).unwrap(),
            adapter,
            Arc::new(crate::platforms::plugins::PlatformPluginRegistry::default()),
        ));
        let source = PlatformContextImageRef {
            id: "context_image_1".to_string(),
            message_id: "90".to_string(),
            image_index: 1,
        };
        let duplicate_source = PlatformContextImageRef {
            id: "context_image_2".to_string(),
            message_id: "91".to_string(),
            image_index: 1,
        };
        let state = ScopedVisionState {
            allowed_paths: Vec::new(),
            context_images: [
                (source.id.clone(), source),
                (duplicate_source.id.clone(), duplicate_source),
            ]
            .into(),
            platform_context: Some(context),
            allow_general_access: false,
            resolve_lock: tokio::sync::Mutex::new(()),
            resolved: Mutex::new(HashMap::new()),
            content_images: Mutex::new(HashMap::new()),
            analyses: Mutex::new(HashMap::new()),
            calls: AtomicUsize::new(0),
            fetches: AtomicUsize::new(0),
            total_bytes: AtomicUsize::new(0),
        };

        let (first, second) = tokio::join!(
            resolve_context_image(&paths, &state, "context_image_1"),
            resolve_context_image(&paths, &state, "context_image_1")
        );
        let first = first.unwrap();
        let second = second.unwrap();
        let duplicate = resolve_context_image(&paths, &state, "context_image_2")
            .await
            .unwrap();

        assert_eq!(calls.load(Ordering::Acquire), 2);
        assert_eq!(first.digest, second.digest);
        assert_eq!(first.cache_path, second.cache_path);
        assert_eq!(first.cache_path, duplicate.cache_path);
        assert_eq!(state.total_bytes.load(Ordering::Acquire), 3);
        assert!(first.cache_path.is_file());
        let error = resolve_context_image(&paths, &state, "context_image_999")
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("context image ID is not available"));
        assert_eq!(calls.load(Ordering::Acquire), 2);
    }
}
