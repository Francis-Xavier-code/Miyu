//! OneBot v11 bridge (NapCat / QQ).
//!
//! NapCat connects to Miyu as a reverse-WebSocket client
//! (`GET /onebot/v11/ws` on the existing web server). Inbound `message`
//! events run agent turns via the platform-neutral core in the parent
//! module; replies go back as `send_private_msg` / `send_group_msg`
//! frames on the same socket. Query-style API calls (file URL lookup)
//! use an echo→oneshot table; message sends are fire-and-forget.

use super::super::{random_id, WebState};
use super::{
    download_capped, markdown_to_plain, resolve_platform_session, run_platform_turn, sniff_image_mime,
    split_reply, RateDecision, TurnDispatch,
};
use crate::config::{GroupTrigger, OneBotConfig};
use crate::i18n::text as t;
use crate::ipc::ImageAttachment;
use anyhow::{bail, Context, Result};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, Semaphore};

const MAX_INBOUND_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_INBOUND_FILE_BYTES: usize = 50 * 1024 * 1024;
const MAX_INBOUND_IMAGES: usize = 4;
const IMAGE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15);
const FILE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
const API_CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// Concurrent turns per NapCat connection; excess messages wait.
const MAX_CONCURRENT_MESSAGES: usize = 4;

// ---------------------------------------------------------------------------
// Connection registry
// ---------------------------------------------------------------------------

/// Live NapCat connections keyed by bot QQ id. NapCat reconnects on its
/// own schedule, which can leave a half-open predecessor; each new
/// connection bumps the generation and the old read loop notices it has
/// been replaced and exits, so replies are never duplicated.
#[derive(Default)]
pub(crate) struct ConnectionRegistry {
    next_generation: u64,
    connections: HashMap<i64, RegisteredConnection>,
}

struct RegisteredConnection {
    generation: u64,
    #[allow(dead_code)] // reserved for outbound-initiated messages (send_im_message tool)
    handle: ConnectionHandle,
}

impl ConnectionRegistry {
    fn register(&mut self, self_id: i64, handle: ConnectionHandle) -> u64 {
        self.next_generation += 1;
        let generation = self.next_generation;
        self.connections
            .insert(self_id, RegisteredConnection { generation, handle });
        generation
    }

    fn is_current(&self, self_id: i64, generation: u64) -> bool {
        self.connections
            .get(&self_id)
            .is_some_and(|connection| connection.generation == generation)
    }

    fn remove(&mut self, self_id: i64, generation: u64) {
        if self.is_current(self_id, generation) {
            self.connections.remove(&self_id);
        }
    }
}

/// Cheap-to-clone sender half of one connection: outbound frames plus
/// the echo table for request/response API calls.
#[derive(Clone)]
struct ConnectionHandle {
    out_tx: mpsc::UnboundedSender<String>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>,
}

impl ConnectionHandle {
    fn send_frame(&self, frame: String) {
        let _ = self.out_tx.send(frame);
    }

    /// Sends an `{action, params, echo}` frame and waits for the frame
    /// that echoes it back. Used only for query APIs (file URLs);
    /// message sends stay fire-and-forget.
    async fn call_api(&self, action: &str, params: Value) -> Result<Value> {
        let echo = random_id("act", 12);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(echo.clone(), tx);
        self.send_frame(json!({ "action": action, "params": params, "echo": echo }).to_string());
        let result = tokio::time::timeout(API_CALL_TIMEOUT, rx).await;
        self.pending.lock().unwrap().remove(&echo);
        let Ok(Ok(response)) = result else {
            bail!("OneBot API {action} timed out");
        };
        let retcode = response.get("retcode").and_then(Value::as_i64).unwrap_or(-1);
        if retcode != 0 {
            bail!("OneBot API {action} failed with retcode {retcode}");
        }
        Ok(response.get("data").cloned().unwrap_or(Value::Null))
    }
}

// ---------------------------------------------------------------------------
// WebSocket endpoint
// ---------------------------------------------------------------------------

pub(crate) async fn onebot_ws(
    State(state): State<WebState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let config = onebot_config(&state);
    if !config.enabled {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !token_matches(&headers, &config.access_token) {
        tracing::warn!("OneBot client rejected: bad access token");
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let self_id = headers
        .get("x-self-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(0);
    ws.on_upgrade(move |socket| connection_loop(state, socket, self_id))
}

fn onebot_config(state: &WebState) -> OneBotConfig {
    state.manager.lock().unwrap().config.platforms.onebot.clone()
}

/// Compares digests rather than raw strings so length/prefix timing
/// leaks nothing. An empty configured token disables the check.
fn token_matches(headers: &HeaderMap, expected: &str) -> bool {
    let expected = expected.trim();
    if expected.is_empty() {
        return true;
    }
    let supplied = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .and_then(|value| {
            value
                .strip_prefix("Bearer ")
                .or_else(|| value.strip_prefix("Token "))
                .or(Some(value))
        })
        .map(str::trim);
    let Some(supplied) = supplied else {
        return false;
    };
    Sha256::digest(supplied.as_bytes()) == Sha256::digest(expected.as_bytes())
}

async fn connection_loop(state: WebState, socket: WebSocket, self_id: i64) {
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    let handle = ConnectionHandle {
        out_tx,
        pending: Arc::new(Mutex::new(HashMap::new())),
    };
    let generation = state
        .platforms
        .onebot
        .lock()
        .unwrap()
        .register(self_id, handle.clone());
    tracing::info!(self_id, generation, "OneBot client connected");

    let (mut sink, mut stream) = socket.split();
    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if sink.send(Message::Text(frame.into())).await.is_err() {
                break;
            }
        }
    });
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_MESSAGES));

    while let Some(message) = stream.next().await {
        let message = match message {
            Ok(message) => message,
            Err(_) => break,
        };
        if !state
            .platforms
            .onebot
            .lock()
            .unwrap()
            .is_current(self_id, generation)
        {
            tracing::info!(self_id, generation, "OneBot connection replaced by a newer one");
            break;
        }
        let text = match message {
            Message::Text(text) => text,
            Message::Close(_) => break,
            _ => continue,
        };
        let Ok(frame) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if frame.get("post_type").is_none() {
            route_api_response(&handle, frame);
            continue;
        }
        if frame.get("post_type").and_then(Value::as_str) == Some("message") {
            // notice/request/meta_event are intentionally ignored.
            let state = state.clone();
            let handle = handle.clone();
            let permits = permits.clone();
            tokio::spawn(async move {
                // Acquire inside the task so the read loop (heartbeats,
                // API responses) is never blocked by turn concurrency.
                let Ok(_permit) = permits.acquire_owned().await else {
                    return;
                };
                handle_message(state, handle, frame).await;
            });
        }
    }

    state
        .platforms
        .onebot
        .lock()
        .unwrap()
        .remove(self_id, generation);
    writer.abort();
    tracing::info!(self_id, generation, "OneBot client disconnected");
}

/// Routes an API response frame to its waiting `call_api`; unmatched
/// frames (fire-and-forget sends) only get a failure log.
fn route_api_response(handle: &ConnectionHandle, frame: Value) {
    let echo = frame
        .get("echo")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(echo) = echo {
        if let Some(waiter) = handle.pending.lock().unwrap().remove(&echo) {
            let _ = waiter.send(frame);
            return;
        }
    }
    let retcode = frame.get("retcode").and_then(Value::as_i64).unwrap_or(0);
    if retcode != 0 {
        tracing::warn!(retcode, "OneBot send failed");
    }
}

// ---------------------------------------------------------------------------
// Inbound message pipeline
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Target {
    Private { user_id: i64 },
    Group { group_id: i64 },
}

#[derive(Default)]
struct InboundMessage {
    text: String,
    images: Vec<MediaRef>,
    files: Vec<FileRef>,
    at_self: bool,
}

enum MediaRef {
    Url(String),
    Bytes(Vec<u8>),
}

struct FileRef {
    file_id: Option<String>,
    name: String,
    url: Option<String>,
}

async fn handle_message(state: WebState, conn: ConnectionHandle, event: Value) {
    let config = onebot_config(&state);
    if !config.enabled {
        return;
    }
    let user_id = event.get("user_id").and_then(Value::as_i64).unwrap_or(0);
    let self_id = event.get("self_id").and_then(Value::as_i64).unwrap_or(0);
    if user_id == 0 || user_id == self_id {
        return;
    }
    let message_type = event
        .get("message_type")
        .and_then(Value::as_str)
        .unwrap_or("");
    let (target, allowed) = match message_type {
        "private" => (
            Target::Private { user_id },
            config.allow_private
                && (config.allowed_users.is_empty() || config.allowed_users.contains(&user_id)),
        ),
        "group" => {
            let group_id = event.get("group_id").and_then(Value::as_i64).unwrap_or(0);
            (
                Target::Group { group_id },
                group_id != 0
                    && config.allow_groups
                    && (config.allowed_groups.is_empty()
                        || config.allowed_groups.contains(&group_id)),
            )
        }
        _ => return,
    };
    if !allowed {
        return;
    }

    let mut parsed = parse_message(event.get("message"), event.get("raw_message"), self_id);
    if let Target::Group { .. } = target {
        let Some(text) = group_trigger_text(&config, &parsed) else {
            return;
        };
        parsed.text = text;
    }

    let decision = state.platforms.rate.lock().unwrap().check(
        &format!("qq:{user_id}"),
        config.rate_per_sender_per_min,
        config.rate_global_per_min,
    );
    match decision {
        RateDecision::Allow => {}
        RateDecision::DropSilently => return,
        RateDecision::DropWithNotice => {
            send_text(&conn, target, t("Too many messages — please slow down a little.", "消息太频繁了，请稍候再发。"));
            return;
        }
    }

    let reply_ref = event.get("message_id").cloned();
    match build_and_run_turn(&state, &conn, &config, target, &event, parsed).await {
        Ok(Some(dispatch)) => deliver_dispatch(&state, &conn, &config, target, reply_ref, dispatch),
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(error = %error, "OneBot message handling failed");
            send_text(
                &conn,
                target,
                &format!(
                    "{}{}",
                    t("Something went wrong: ", "出错了："),
                    super::super::safe_error_message(&error)
                ),
            );
        }
    }
}

/// Turns a parsed inbound message into agent input (downloading media),
/// resolves the dedicated session and runs the turn. `Ok(None)` means
/// the message needs no reply (e.g. sticker-only).
async fn build_and_run_turn(
    state: &WebState,
    conn: &ConnectionHandle,
    config: &OneBotConfig,
    target: Target,
    event: &Value,
    parsed: InboundMessage,
) -> Result<Option<TurnDispatch>> {
    let mut content = parsed.text.trim().to_string();

    let mut images: Vec<Option<ImageAttachment>> = Vec::new();
    for media in parsed.images.into_iter().take(MAX_INBOUND_IMAGES) {
        let bytes = match media {
            MediaRef::Bytes(bytes) => bytes,
            MediaRef::Url(url) => {
                match download_capped(
                    &state.platforms.http,
                    &url,
                    MAX_INBOUND_IMAGE_BYTES,
                    IMAGE_DOWNLOAD_TIMEOUT,
                )
                .await
                {
                    Ok((bytes, _)) => bytes,
                    Err(error) => {
                        tracing::warn!(error = %error, "OneBot image download failed");
                        continue;
                    }
                }
            }
        };
        let mime = sniff_image_mime(&bytes).to_string();
        images.push(Some(ImageAttachment::Binary { mime, data: bytes }));
    }

    for file in &parsed.files {
        match fetch_inbound_file(state, conn, target, file).await {
            Ok(path) => {
                if !content.is_empty() {
                    content.push('\n');
                }
                content.push_str(&format!(
                    "[{} {} {} {}]",
                    t("the user sent a file", "用户发来文件"),
                    file.name,
                    t("saved at", "已保存于"),
                    path.display()
                ));
            }
            Err(error) => {
                tracing::warn!(error = %error, file = %file.name, "OneBot file download failed");
                send_text(
                    &conn.clone(),
                    target,
                    &format!(
                        "{}{}",
                        t("Couldn't fetch the file: ", "文件接收失败："),
                        file.name
                    ),
                );
            }
        }
    }

    if content.is_empty() && images.is_empty() {
        if parsed.at_self {
            content = t("(they @-mentioned you without any text)", "（对方@了你，但没有其他内容）").to_string();
        } else {
            return Ok(None);
        }
    }

    if let Target::Group { .. } = target {
        let sender = event.get("sender");
        let name = sender
            .and_then(|sender| sender.get("card"))
            .and_then(Value::as_str)
            .filter(|card| !card.trim().is_empty())
            .or_else(|| {
                sender
                    .and_then(|sender| sender.get("nickname"))
                    .and_then(Value::as_str)
            })
            .unwrap_or("?");
        let user_id = event.get("user_id").and_then(Value::as_i64).unwrap_or(0);
        content = format!("[{} {name}({user_id})] {content}", t("group member", "群成员"));
    }

    let session_name = session_name_for(config, target, event);
    let session_id = resolve_platform_session(state, &session_name)?;
    let dispatch = run_platform_turn(state, session_id, content, images).await?;
    Ok(Some(dispatch))
}

/// Session-name key for this conversation, e.g. `qq:private:12345`,
/// `qq:group:678` or `qq:group:678:12345` (per-user isolation).
fn session_name_for(config: &OneBotConfig, target: Target, event: &Value) -> String {
    match target {
        Target::Private { user_id } => format!("qq:private:{user_id}"),
        Target::Group { group_id } => {
            if config.group_session_per_user {
                let user_id = event.get("user_id").and_then(Value::as_i64).unwrap_or(0);
                format!("qq:group:{group_id}:{user_id}")
            } else {
                format!("qq:group:{group_id}")
            }
        }
    }
}

/// Resolves a download URL for an inbound file (direct, or via the
/// NapCat file-URL APIs), downloads it capped and saves it under the
/// data dir. Returns the saved path.
async fn fetch_inbound_file(
    state: &WebState,
    conn: &ConnectionHandle,
    target: Target,
    file: &FileRef,
) -> Result<PathBuf> {
    let url = match &file.url {
        Some(url) => url.clone(),
        None => {
            let file_id = file
                .file_id
                .as_deref()
                .context("the file has no url and no file_id")?;
            let data = match target {
                Target::Group { group_id } => {
                    conn.call_api(
                        "get_group_file_url",
                        json!({ "file_id": file_id, "group_id": group_id }),
                    )
                    .await?
                }
                Target::Private { .. } => {
                    conn.call_api("get_private_file_url", json!({ "file_id": file_id }))
                        .await?
                }
            };
            data.get("url")
                .and_then(Value::as_str)
                .context("the file-url API returned no url")?
                .to_string()
        }
    };
    let (bytes, _) = download_capped(
        &state.platforms.http,
        &url,
        MAX_INBOUND_FILE_BYTES,
        FILE_DOWNLOAD_TIMEOUT,
    )
    .await?;
    save_platform_file(&state.paths.data_dir, &file.name, &bytes).await
}

/// Saves inbound bytes under `<data_dir>/platform_files/`, keeping only
/// the basename (no path traversal) and suffixing on collision.
async fn save_platform_file(data_dir: &std::path::Path, name: &str, bytes: &[u8]) -> Result<PathBuf> {
    let dir = data_dir.join("platform_files");
    tokio::fs::create_dir_all(&dir).await?;
    let safe = sanitize_file_name(name);
    let mut candidate = dir.join(&safe);
    let mut counter = 1;
    while candidate.exists() {
        let path = std::path::Path::new(&safe);
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("file");
        let suffixed = match path.extension().and_then(|ext| ext.to_str()) {
            Some(ext) => format!("{stem}-{counter}.{ext}"),
            None => format!("{stem}-{counter}"),
        };
        candidate = dir.join(suffixed);
        counter += 1;
        if counter > 1000 {
            bail!("too many files with the same name");
        }
    }
    tokio::fs::write(&candidate, bytes).await?;
    Ok(candidate)
}

fn sanitize_file_name(name: &str) -> String {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("file")
        .replace(['\0', '\n', '\r'], "");
    let trimmed = base.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return "file".to_string();
    }
    trimmed.chars().take(120).collect()
}

/// Group wake check. `Some(text)` = triggered, with any wake prefix
/// already stripped; `None` = stay silent.
fn group_trigger_text(config: &OneBotConfig, parsed: &InboundMessage) -> Option<String> {
    let trigger = config.group_trigger();
    let prefix = config.trigger_prefix.trim();
    let prefix_hit = || {
        if prefix.is_empty() {
            return None;
        }
        parsed
            .text
            .trim_start()
            .strip_prefix(prefix)
            .map(|rest| rest.trim_start().to_string())
    };
    match trigger {
        GroupTrigger::At => parsed.at_self.then(|| parsed.text.clone()),
        GroupTrigger::Prefix => prefix_hit(),
        GroupTrigger::AtOrPrefix => {
            if parsed.at_self {
                Some(parsed.text.clone())
            } else {
                prefix_hit()
            }
        }
    }
}

/// Parses the OneBot `message` field (segment array, or raw string as a
/// fallback when NapCat isn't configured for array format).
fn parse_message(message: Option<&Value>, raw_message: Option<&Value>, self_id: i64) -> InboundMessage {
    let mut parsed = InboundMessage::default();
    let Some(Value::Array(segments)) = message else {
        if let Some(raw) = message
            .and_then(Value::as_str)
            .or_else(|| raw_message.and_then(Value::as_str))
        {
            parsed.text = raw.to_string();
        }
        return parsed;
    };
    for segment in segments {
        let kind = segment.get("type").and_then(Value::as_str).unwrap_or("");
        let data = segment.get("data").cloned().unwrap_or(Value::Null);
        match kind {
            "text" => {
                if let Some(text) = data.get("text").and_then(Value::as_str) {
                    parsed.text.push_str(text);
                }
            }
            "image" => {
                let file = data.get("file").and_then(Value::as_str).unwrap_or("");
                if let Some(encoded) = file.strip_prefix("base64://") {
                    if let Ok(bytes) = BASE64.decode(encoded) {
                        parsed.images.push(MediaRef::Bytes(bytes));
                    }
                } else if let Some(url) = data
                    .get("url")
                    .and_then(Value::as_str)
                    .filter(|url| url.starts_with("http"))
                    .or_else(|| Some(file).filter(|file| file.starts_with("http")))
                {
                    parsed.images.push(MediaRef::Url(url.to_string()));
                }
            }
            "at" => {
                let qq = data.get("qq").and_then(|qq| match qq {
                    Value::String(qq) => Some(qq.clone()),
                    Value::Number(qq) => Some(qq.to_string()),
                    _ => None,
                });
                if qq.as_deref() == Some(self_id.to_string().as_str()) {
                    parsed.at_self = true;
                }
            }
            "file" => {
                let name = data
                    .get("file_name")
                    .and_then(Value::as_str)
                    .or_else(|| data.get("name").and_then(Value::as_str))
                    .or_else(|| data.get("file").and_then(Value::as_str))
                    .unwrap_or("file")
                    .to_string();
                parsed.files.push(FileRef {
                    file_id: data
                        .get("file_id")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    name,
                    url: data
                        .get("url")
                        .and_then(Value::as_str)
                        .filter(|url| url.starts_with("http"))
                        .map(str::to_string),
                });
            }
            // reply/face/record/... carry no turn input.
            _ => {}
        }
    }
    parsed
}

// ---------------------------------------------------------------------------
// Outbound
// ---------------------------------------------------------------------------

fn deliver_dispatch(
    state: &WebState,
    conn: &ConnectionHandle,
    config: &OneBotConfig,
    target: Target,
    reply_ref: Option<Value>,
    dispatch: TurnDispatch,
) {
    match dispatch {
        TurnDispatch::Queued => send_text(
            conn,
            target,
            t(
                "Got it — still finishing the previous message; I'll reply to both together.",
                "收到，上一条还在处理中，稍后一起回复。",
            ),
        ),
        TurnDispatch::Failed(message) => send_text(
            conn,
            target,
            &format!("{}{message}", t("Something went wrong: ", "出错了：")),
        ),
        TurnDispatch::Completed(outcome) => {
            let mut plain = markdown_to_plain(&outcome.text);
            if plain.trim().is_empty() && outcome.image_assets.is_empty() {
                plain = t("(this turn produced no text reply)", "（这一轮没有产生文本回复）").to_string();
            }
            let chunks = split_reply(&plain, config.max_reply_chars);
            for (index, chunk) in chunks.iter().enumerate() {
                let mut segments = Vec::new();
                if index == 0 {
                    if let (Target::Group { .. }, Some(reply)) = (target, reply_ref.as_ref()) {
                        segments.push(json!({ "type": "reply", "data": { "id": reply } }));
                    }
                }
                segments.push(text_segment(chunk));
                conn.send_frame(message_frame(target, segments));
            }
            for asset_id in &outcome.image_assets {
                match state.state_store.load_image_asset(asset_id) {
                    Ok(Some(asset)) => {
                        let segment = json!({
                            "type": "image",
                            "data": { "file": format!("base64://{}", BASE64.encode(&asset.bytes)) },
                        });
                        conn.send_frame(message_frame(target, vec![segment]));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::warn!(error = %error, asset_id, "loading an image asset for OneBot failed");
                    }
                }
            }
        }
    }
}

fn send_text(conn: &ConnectionHandle, target: Target, text: &str) {
    conn.send_frame(message_frame(target, vec![text_segment(text)]));
}

fn text_segment(text: &str) -> Value {
    json!({ "type": "text", "data": { "text": text } })
}

fn message_frame(target: Target, segments: Vec<Value>) -> String {
    let frame = match target {
        Target::Private { user_id } => json!({
            "action": "send_private_msg",
            "params": { "user_id": user_id, "message": segments },
            "echo": random_id("msg", 8),
        }),
        Target::Group { group_id } => json!({
            "action": "send_group_msg",
            "params": { "group_id": group_id, "message": segments },
            "echo": random_id("msg", 8),
        }),
    };
    frame.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with(mutate: impl FnOnce(&mut OneBotConfig)) -> OneBotConfig {
        let mut config = OneBotConfig::default();
        mutate(&mut config);
        config
    }

    #[test]
    fn parses_segment_arrays_with_mixed_content() {
        let message = json!([
            { "type": "at", "data": { "qq": "10001" } },
            { "type": "text", "data": { "text": " 你好" } },
            { "type": "image", "data": { "file": "x.jpg", "url": "https://img.example/x.jpg" } },
            { "type": "image", "data": { "file": "base64://aGk=" } },
            { "type": "file", "data": { "file_id": "f1", "file_name": "报告.pdf" } },
            { "type": "reply", "data": { "id": "5" } },
        ]);
        let parsed = parse_message(Some(&message), None, 10001);
        assert!(parsed.at_self);
        assert_eq!(parsed.text, " 你好");
        assert_eq!(parsed.images.len(), 2);
        assert!(matches!(&parsed.images[0], MediaRef::Url(url) if url == "https://img.example/x.jpg"));
        assert!(matches!(&parsed.images[1], MediaRef::Bytes(bytes) if bytes == b"hi"));
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.files[0].name, "报告.pdf");
        assert_eq!(parsed.files[0].file_id.as_deref(), Some("f1"));

        // Someone else being @-ed does not wake the bot.
        let other = json!([{ "type": "at", "data": { "qq": "999" } }]);
        assert!(!parse_message(Some(&other), None, 10001).at_self);
    }

    #[test]
    fn falls_back_to_raw_string_messages() {
        let message = json!("纯文本消息");
        let parsed = parse_message(Some(&message), None, 1);
        assert_eq!(parsed.text, "纯文本消息");

        let raw = json!("raw 兜底");
        let parsed = parse_message(None, Some(&raw), 1);
        assert_eq!(parsed.text, "raw 兜底");
    }

    #[test]
    fn group_trigger_matrix() {
        let at_only = config_with(|config| config.group_trigger = "at".into());
        let mut parsed = InboundMessage {
            text: "/cmd 查询".into(),
            ..Default::default()
        };
        assert!(group_trigger_text(&at_only, &parsed).is_none());
        parsed.at_self = true;
        assert_eq!(group_trigger_text(&at_only, &parsed).as_deref(), Some("/cmd 查询"));

        let prefix = config_with(|config| {
            config.group_trigger = "prefix".into();
            config.trigger_prefix = "/cmd".into();
        });
        parsed.at_self = false;
        assert_eq!(group_trigger_text(&prefix, &parsed).as_deref(), Some("查询"));
        parsed.text = "无前缀".into();
        assert!(group_trigger_text(&prefix, &parsed).is_none());

        // An empty prefix never fires (avoids always-on).
        let empty_prefix = config_with(|config| config.group_trigger = "prefix".into());
        assert!(group_trigger_text(&empty_prefix, &parsed).is_none());

        let either = config_with(|config| {
            config.group_trigger = "at_or_prefix".into();
            config.trigger_prefix = "喵".into();
        });
        parsed.text = "喵 早上好".into();
        assert_eq!(group_trigger_text(&either, &parsed).as_deref(), Some("早上好"));
    }

    #[test]
    fn sanitizes_file_names() {
        assert_eq!(sanitize_file_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_file_name("C:\\evil\\x.exe"), "x.exe");
        assert_eq!(sanitize_file_name(".."), "file");
        assert_eq!(sanitize_file_name("  "), "file");
        assert_eq!(sanitize_file_name("报告 v2.pdf"), "报告 v2.pdf");
    }

    #[test]
    fn outbound_frames_have_the_onebot_shape() {
        let frame: Value =
            serde_json::from_str(&message_frame(Target::Private { user_id: 42 }, vec![text_segment("hi")]))
                .unwrap();
        assert_eq!(frame["action"], "send_private_msg");
        assert_eq!(frame["params"]["user_id"], 42);
        assert_eq!(frame["params"]["message"][0]["type"], "text");
        assert_eq!(frame["params"]["message"][0]["data"]["text"], "hi");
        assert!(frame["echo"].as_str().is_some());

        let frame: Value =
            serde_json::from_str(&message_frame(Target::Group { group_id: 7 }, vec![text_segment("x")]))
                .unwrap();
        assert_eq!(frame["action"], "send_group_msg");
        assert_eq!(frame["params"]["group_id"], 7);
    }

    #[test]
    fn token_check_accepts_bearer_and_rejects_wrong() {
        let mut headers = HeaderMap::new();
        assert!(token_matches(&headers, ""));
        assert!(!token_matches(&headers, "secret"));
        headers.insert(AUTHORIZATION, "Bearer secret".parse().unwrap());
        assert!(token_matches(&headers, "secret"));
        assert!(!token_matches(&headers, "other"));
        headers.insert(AUTHORIZATION, "Token secret".parse().unwrap());
        assert!(token_matches(&headers, "secret"));
        headers.insert(AUTHORIZATION, "secret".parse().unwrap());
        assert!(token_matches(&headers, "secret"));
    }
}
