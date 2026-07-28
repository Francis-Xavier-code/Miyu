//! OneBot v11 bridge (NapCat / QQ).
//!
//! NapCat connects to Miyu as a reverse-WebSocket client
//! (`GET /ws` on the existing web server; `/onebot/v11/ws` remains an
//! alias). Inbound `message`
//! events run agent turns via the platform-neutral core in the parent
//! module; replies go back as `send_private_msg` / `send_group_msg`
//! frames on the same socket. Query-style API calls (file URL lookup)
//! use an echo-to-oneshot table. Sends are acknowledged before plugin
//! success hooks run, so transformations can safely persist delivery state.

use super::{
    commands, download_capped, markdown_to_plain, resolve_platform_session, run_platform_turn,
    sniff_image_mime, split_reply, ConversationKind, ForwardNode, OutboundBody, OutboundMessage,
    OutboundOrigin, OutboundSegment, PlatformAdapter, PlatformConversation, PlatformTurnContext,
    RateDecision, SendReceipt, TurnDispatch, TurnProfile,
};
use crate::config::OneBotConfig;
use crate::i18n::text as t;
use crate::ipc::ImageAttachment;
use crate::web::{
    clear_platform_session_content, random_id, safe_error_message, DaemonState,
    PlatformSessionResetError,
};
use anyhow::{bail, Context, Result};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::{
    header::{AUTHORIZATION, HOST},
    HeaderMap, StatusCode,
};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch, Semaphore};
use tokio::task::JoinHandle;

const MAX_INBOUND_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const MAX_INBOUND_FILE_BYTES: usize = 50 * 1024 * 1024;
const MAX_INBOUND_IMAGES: usize = 4;
const MAX_INBOUND_FILES: usize = 4;
const MAX_OUTBOUND_IMAGE_BYTES: usize = 20 * 1024 * 1024;
const MAX_OUTBOUND_FILE_BYTES: usize = 50 * 1024 * 1024;
const MAX_BASE64_FILE_BYTES: usize = 16 * 1024 * 1024;
const IMAGE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15);
const FILE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
const API_CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// Concurrent turns per NapCat connection. Excess messages receive one
/// throttled busy notice instead of growing an unbounded task queue.
const MAX_CONCURRENT_MESSAGES: usize = 4;
const BUSY_NOTICE_COOLDOWN: Duration = Duration::from_secs(5);
const PLATFORM_FILE_STORAGE_BYTES: u64 = 1024 * 1024 * 1024;
const PLATFORM_FILE_STORAGE_ENTRIES: usize = 4096;
const PLATFORM_FILE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

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
    handle: ConnectionHandle,
}

impl ConnectionRegistry {
    fn register(&mut self, self_id: i64, handle: ConnectionHandle) -> u64 {
        self.next_generation += 1;
        let generation = self.next_generation;
        if self_id != 0 {
            self.connections
                .insert(self_id, RegisteredConnection { generation, handle });
        }
        generation
    }

    fn bind(&mut self, self_id: i64, generation: u64, handle: ConnectionHandle) -> bool {
        if self_id == 0
            || self
                .connections
                .get(&self_id)
                .is_some_and(|connection| connection.generation > generation)
        {
            return false;
        }
        self.connections
            .insert(self_id, RegisteredConnection { generation, handle });
        true
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

    fn handle(&self, self_id: i64) -> Option<ConnectionHandle> {
        self.connections
            .get(&self_id)
            .map(|connection| connection.handle.clone())
    }

    pub(crate) fn connected_accounts(&self) -> Vec<i64> {
        let mut accounts = self.connections.keys().copied().collect::<Vec<_>>();
        accounts.sort_unstable();
        accounts
    }

    pub(crate) fn disconnect_all(&mut self) {
        for connection in self.connections.values() {
            let _ = connection.handle.shutdown.send(true);
        }
        self.connections.clear();
    }
}

/// Cheap-to-clone sender half of one connection: outbound frames plus
/// the echo table for request/response API calls.
#[derive(Clone)]
struct ConnectionHandle {
    out_tx: mpsc::UnboundedSender<String>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>,
    bot_name: Arc<Mutex<Option<String>>>,
    asset_base_url: Option<String>,
    assets: super::assets::AssetLeaseStore,
    busy_notice_pending: Arc<AtomicBool>,
    shutdown: watch::Sender<bool>,
}

impl ConnectionHandle {
    fn send_frame(&self, frame: String) -> Result<()> {
        self.out_tx
            .send(frame)
            .map_err(|_| anyhow::anyhow!("OneBot connection writer is closed"))
    }

    /// Sends an `{action, params, echo}` frame and waits for the frame
    /// that echoes it back.
    async fn call_api(&self, action: &str, params: Value) -> Result<Value> {
        self.call_api_with_timeout(action, params, API_CALL_TIMEOUT)
            .await
    }

    async fn call_api_with_timeout(
        &self,
        action: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        let echo = random_id("act", 12);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(echo.clone(), tx);
        if let Err(error) = self.send_frame(api_frame(action, params, &echo)) {
            self.pending.lock().unwrap().remove(&echo);
            return Err(error);
        }
        let result = tokio::time::timeout(timeout, rx).await;
        self.pending.lock().unwrap().remove(&echo);
        let Ok(Ok(response)) = result else {
            bail!("OneBot API {action} timed out");
        };
        let retcode = response
            .get("retcode")
            .and_then(Value::as_i64)
            .unwrap_or(-1);
        if retcode != 0 {
            bail!("OneBot API {action} failed with retcode {retcode}");
        }
        Ok(response.get("data").cloned().unwrap_or(Value::Null))
    }
}

#[derive(Clone, Default)]
pub(crate) struct QqListenerManager {
    inner: Arc<Mutex<QqListenerState>>,
}

#[derive(Default)]
struct QqListenerState {
    active_port: Option<u16>,
    task: Option<JoinHandle<()>>,
}

pub(crate) struct PreparedQqListener {
    manager: QqListenerManager,
    state: DaemonState,
    desired_port: Option<u16>,
    listener: Option<tokio::net::TcpListener>,
    disconnect_connections: bool,
}

impl QqListenerManager {
    pub(crate) fn active_port(&self) -> Option<u16> {
        self.inner.lock().unwrap().active_port
    }

    pub(crate) async fn prepare(
        &self,
        state: &DaemonState,
        current: Option<&OneBotConfig>,
        next: &OneBotConfig,
    ) -> Result<PreparedQqListener> {
        let desired_port = next.enabled.then_some(next.reverse_ws_port);
        let active_port = self.inner.lock().unwrap().active_port;
        let needs_dedicated_bind =
            desired_port.is_some_and(|port| port != state.web_port && Some(port) != active_port);
        let listener = if needs_dedicated_bind {
            let port = desired_port.expect("dedicated bind requires a port");
            Some(
                tokio::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], port)))
                    .await
                    .with_context(|| {
                        format!("binding Tencent QQ reverse WebSocket to 0.0.0.0:{port}")
                    })?,
            )
        } else {
            None
        };
        let disconnect_connections = current.is_some_and(|current| {
            current.enabled != next.enabled
                || current.reverse_ws_port != next.reverse_ws_port
                || current.access_token != next.access_token
        });
        Ok(PreparedQqListener {
            manager: self.clone(),
            state: state.clone(),
            desired_port,
            listener,
            disconnect_connections,
        })
    }

    pub(crate) fn shutdown(&self, state: &DaemonState) {
        let task = {
            let mut inner = self.inner.lock().unwrap();
            inner.active_port = None;
            inner.task.take()
        };
        if let Some(task) = task {
            task.abort();
        }
        state.platforms.onebot.lock().unwrap().disconnect_all();
    }
}

impl PreparedQqListener {
    pub(crate) fn commit(mut self) {
        let previous_port = self.manager.active_port();
        let previous_task = {
            let mut inner = self.manager.inner.lock().unwrap();
            if inner.active_port == self.desired_port {
                None
            } else {
                let previous = inner.task.take();
                inner.active_port = self.desired_port;
                inner.task = self.listener.take().map(|listener| {
                    let app = qq_listener_router(self.state.clone());
                    tokio::spawn(async move {
                        if let Err(error) = axum::serve(
                            listener,
                            app.into_make_service_with_connect_info::<SocketAddr>(),
                        )
                        .await
                        {
                            tracing::error!(target: "miyu::qq", error = %error, "Tencent QQ listener stopped");
                        }
                    })
                });
                previous
            }
        };
        if let Some(task) = previous_task {
            task.abort();
        }
        if self.disconnect_connections {
            self.state.platforms.onebot.lock().unwrap().disconnect_all();
        }
        if previous_port != self.desired_port {
            match self.desired_port {
                Some(port) => {
                    tracing::info!(target: "miyu::qq", port, path = "/ws", "Tencent QQ listener ready")
                }
                None => tracing::info!(target: "miyu::qq", "Tencent QQ listener disabled"),
            }
        }
    }
}

fn qq_listener_router(state: DaemonState) -> Router {
    Router::new()
        .route("/ws", get(onebot_ws))
        .route("/onebot/v11/ws", get(onebot_ws))
        .route("/api/platform-assets/{token}", get(super::platform_asset))
        .with_state(state)
}

fn api_frame(action: &str, params: Value, echo: &str) -> String {
    json!({ "action": action, "params": params, "echo": echo }).to_string()
}

// ---------------------------------------------------------------------------
// WebSocket endpoint
// ---------------------------------------------------------------------------

pub(crate) async fn onebot_ws(
    State(state): State<DaemonState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let config = onebot_config(&state);
    if !config.enabled {
        return StatusCode::NOT_FOUND.into_response();
    }
    if !connection_authorized(&headers, &config.access_token, peer) {
        if config.access_token.trim().is_empty() {
            tracing::warn!(target: "miyu::qq", %peer, reason = "non_loopback_without_token", "OneBot client rejected");
        } else {
            tracing::warn!(target: "miyu::qq", %peer, reason = "bad_token", "OneBot client rejected");
        }
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let self_id = headers
        .get("x-self-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(0);
    let asset_base_url = resolve_asset_base_url(&headers, &config);
    ws.on_upgrade(move |socket| connection_loop(state, socket, self_id, asset_base_url))
}

pub(crate) async fn onebot_ws_on_web_port(
    State(state): State<DaemonState>,
    peer: ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if onebot_config(&state).reverse_ws_port != state.web_port {
        return StatusCode::NOT_FOUND.into_response();
    }
    onebot_ws(State(state), peer, headers, ws).await
}

fn connection_authorized(headers: &HeaderMap, expected: &str, peer: SocketAddr) -> bool {
    let expected = expected.trim();
    if expected.is_empty() {
        peer.ip().is_loopback()
    } else {
        token_matches(headers, expected)
    }
}

fn resolve_asset_base_url(headers: &HeaderMap, config: &OneBotConfig) -> Option<String> {
    let configured = config.asset_base_url.trim().trim_end_matches('/');
    if configured.starts_with("http://") || configured.starts_with("https://") {
        return Some(configured.to_string());
    }
    headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|host| {
            !host.is_empty()
                && host
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b".-:[]".contains(&byte))
        })
        .map(|host| format!("http://{host}"))
}

fn onebot_config(state: &DaemonState) -> OneBotConfig {
    state.manager.lock().unwrap().config.platforms.qq.clone()
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

async fn connection_loop(
    state: DaemonState,
    socket: WebSocket,
    self_id: i64,
    asset_base_url: Option<String>,
) {
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<String>();
    let (shutdown, mut shutdown_rx) = watch::channel(false);
    let handle = ConnectionHandle {
        out_tx,
        pending: Arc::new(Mutex::new(HashMap::new())),
        bot_name: Arc::new(Mutex::new(None)),
        asset_base_url,
        assets: state.platforms.assets.clone(),
        busy_notice_pending: Arc::new(AtomicBool::new(false)),
        shutdown,
    };
    let generation = state
        .platforms
        .onebot
        .lock()
        .unwrap()
        .register(self_id, handle.clone());
    tracing::info!(target: "miyu::qq", self_id, generation, "OneBot client connected");

    let (mut sink, mut stream) = socket.split();
    let writer = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if sink.send(Message::Text(frame.into())).await.is_err() {
                break;
            }
        }
    });
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT_MESSAGES));
    let mut bound_self_id = self_id;

    loop {
        let message = tokio::select! {
            changed = shutdown_rx.changed() => {
                if changed.is_err() || *shutdown_rx.borrow() {
                    break;
                }
                continue;
            }
            message = stream.next() => {
                let Some(message) = message else { break; };
                message
            }
        };
        let message = match message {
            Ok(message) => message,
            Err(_) => break,
        };
        if bound_self_id != 0
            && !state
                .platforms
                .onebot
                .lock()
                .unwrap()
                .is_current(bound_self_id, generation)
        {
            tracing::info!(target: "miyu::qq",
                self_id,
                generation,
                "OneBot connection replaced by a newer one"
            );
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
        if let Some(event_self_id) = frame
            .get("self_id")
            .and_then(Value::as_i64)
            .filter(|id| *id != 0)
        {
            if bound_self_id == 0 {
                bound_self_id = event_self_id;
                let bound = state.platforms.onebot.lock().unwrap().bind(
                    bound_self_id,
                    generation,
                    handle.clone(),
                );
                if !bound {
                    tracing::info!(target: "miyu::qq",
                    self_id = bound_self_id,
                    generation,
                    "OneBot connection identity is already owned by a newer connection"
                    );
                    break;
                }
                tracing::info!(target: "miyu::qq",
                    self_id = bound_self_id,
                    generation,
                    "OneBot connection identity bound from event"
                );
            } else if bound_self_id != event_self_id {
                tracing::warn!(target: "miyu::qq",
                    expected = bound_self_id,
                    received = event_self_id,
                    "OneBot connection changed self_id"
                );
                break;
            }
        }
        if frame.get("post_type").is_none() {
            route_api_response(&handle, frame);
            continue;
        }
        if frame.get("post_type").and_then(Value::as_str) == Some("message") {
            // notice/request/meta_event are intentionally ignored.
            let connection_permit = match permits.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    tracing::warn!(target: "miyu::qq",
                        self_id = bound_self_id,
                        "OneBot connection concurrency is full; rejecting a message"
                    );
                    notify_busy(&handle, &frame);
                    continue;
                }
            };
            let global_permit = match state.platforms.turn_permits.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => {
                    tracing::warn!(target: "miyu::qq",
                        self_id = bound_self_id,
                        "OneBot global concurrency is full; rejecting a message"
                    );
                    drop(connection_permit);
                    notify_busy(&handle, &frame);
                    continue;
                }
            };
            let state = state.clone();
            let handle = handle.clone();
            tokio::spawn(async move {
                let _connection_permit = connection_permit;
                let _global_permit = global_permit;
                handle_message(state, handle, frame).await;
            });
        }
    }

    state
        .platforms
        .onebot
        .lock()
        .unwrap()
        .remove(bound_self_id, generation);
    writer.abort();
    tracing::info!(target: "miyu::qq",
        self_id = bound_self_id,
        generation,
        "OneBot client disconnected"
    );
}

/// Routes an API response frame to its waiting `call_api`; unmatched
/// response failures still get a diagnostic.
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

fn notify_busy(handle: &ConnectionHandle, event: &Value) {
    let target = match event.get("message_type").and_then(Value::as_str) {
        Some("private") => event
            .get("user_id")
            .and_then(Value::as_i64)
            .filter(|id| *id > 0)
            .map(|user_id| ("send_private_msg", json!({ "user_id": user_id }))),
        Some("group") => event
            .get("group_id")
            .and_then(Value::as_i64)
            .filter(|id| *id > 0)
            .map(|group_id| ("send_group_msg", json!({ "group_id": group_id }))),
        _ => None,
    };
    let Some((action, mut params)) = target else {
        return;
    };
    if handle.busy_notice_pending.swap(true, Ordering::AcqRel) {
        return;
    }
    params["message"] = Value::Array(vec![text_segment(t(
        "Miyu is handling several messages; please try again shortly.",
        "Miyu 正在处理多条消息，请稍后再试。",
    ))]);
    let handle = handle.clone();
    tokio::spawn(async move {
        if let Err(error) = handle.call_api(action, params).await {
            tracing::warn!(error = %error, "sending OneBot busy notice failed");
        }
        tokio::time::sleep(BUSY_NOTICE_COOLDOWN).await;
        handle.busy_notice_pending.store(false, Ordering::Release);
    });
}

// ---------------------------------------------------------------------------
// Inbound message pipeline
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Target {
    Private { user_id: i64 },
    Group { group_id: i64 },
}

impl Target {
    fn kind(self) -> &'static str {
        match self {
            Self::Private { .. } => "private",
            Self::Group { .. } => "group",
        }
    }

    fn conversation_id(self) -> i64 {
        match self {
            Self::Private { user_id } => user_id,
            Self::Group { group_id } => group_id,
        }
    }
}

struct Admission {
    allowed: bool,
    rate_key: Option<String>,
    rate_limit: u32,
}

fn admission_for(config: &OneBotConfig, target: Target, self_id: i64, user_id: i64) -> Admission {
    if config.admin_users.contains(&user_id) {
        return Admission {
            allowed: true,
            rate_key: None,
            rate_limit: 0,
        };
    }
    match target {
        Target::Private { user_id } => {
            if config.private_chats.whitelist.contains(&user_id) {
                Admission {
                    allowed: true,
                    rate_key: None,
                    rate_limit: 0,
                }
            } else {
                Admission {
                    allowed: config.private_chats.allow_non_whitelist,
                    rate_key: Some(format!("qq:{self_id}:private:{user_id}")),
                    rate_limit: config.private_chats.non_whitelist_rate_per_minute,
                }
            }
        }
        Target::Group { group_id } => {
            let whitelisted = config.group_chats.whitelist.contains(&group_id);
            Admission {
                allowed: whitelisted || config.group_chats.allow_non_whitelist,
                rate_key: Some(format!("qq:{self_id}:group:{group_id}")),
                rate_limit: if whitelisted {
                    config.group_chats.whitelist_rate_per_minute
                } else {
                    config.group_chats.non_whitelist_rate_per_minute
                },
            }
        }
    }
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

async fn handle_message(state: DaemonState, conn: ConnectionHandle, event: Value) {
    let app_config = state.manager.lock().unwrap().config.clone();
    let config = app_config.platforms.qq.clone();
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
    let target = match message_type {
        "private" => Target::Private { user_id },
        "group" => {
            let group_id = event.get("group_id").and_then(Value::as_i64).unwrap_or(0);
            if group_id == 0 {
                return;
            }
            Target::Group { group_id }
        }
        _ => return,
    };
    let admission = admission_for(&config, target, self_id, user_id);
    if !admission.allowed {
        return;
    }

    let mut parsed = parse_message(event.get("message"), event.get("raw_message"), self_id);
    let context = match platform_turn_context(&state, conn.clone(), target, &event, app_config) {
        Ok(context) => Arc::new(context),
        Err(error) => {
            tracing::warn!(target: "miyu::qq", error = %error, "OneBot platform runtime initialization failed");
            return;
        }
    };

    // Classify group traffic before charging rate limits. Busy groups often
    // produce many messages that do not wake Miyu and must not starve actual
    // mentions or prefix commands.
    let parsed_command = commands::parse(&context.config.platforms, parsed.text.trim());
    // Built-in commands own their registered names. Unknown prefixed input is
    // offered to legacy plugin commands before the core reports it as unknown.
    let plugin_command_response = if matches!(
        parsed_command,
        Some(commands::ParsedPlatformCommand::Reset { .. })
    ) {
        None
    } else {
        context.handle_command(parsed.text.trim()).await
    };
    let builtin_command = if plugin_command_response.is_none() {
        parsed_command
    } else {
        None
    };
    if plugin_command_response.is_none() && builtin_command.is_none() {
        if let Target::Group { .. } = target {
            let Some(text) = group_trigger_text(&config, &parsed) else {
                return;
            };
            parsed.text = text;
        }
    }

    let message_id = event
        .get("message_id")
        .and_then(value_id_string)
        .unwrap_or_default();
    tracing::info!(
        target: "miyu::qq",
        self_id,
        sender_id = user_id,
        conversation_kind = target.kind(),
        conversation_id = target.conversation_id(),
        %message_id,
        text_chars = parsed.text.chars().count(),
        images = parsed.images.len(),
        files = parsed.files.len(),
        command = plugin_command_response.is_some() || builtin_command.is_some(),
        "OneBot message accepted"
    );

    let decision = admission
        .rate_key
        .as_deref()
        .map_or(RateDecision::Allow, |key| {
            state
                .platforms
                .rate
                .lock()
                .unwrap()
                .check(key, admission.rate_limit)
        });
    match decision {
        RateDecision::Allow => {}
        RateDecision::DropSilently => {
            tracing::info!(
                target: "miyu::qq",
                self_id,
                sender_id = user_id,
                conversation_kind = target.kind(),
                conversation_id = target.conversation_id(),
                "OneBot message rate-limited"
            );
            return;
        }
        RateDecision::DropWithNotice => {
            tracing::info!(
                target: "miyu::qq",
                self_id,
                sender_id = user_id,
                conversation_kind = target.kind(),
                conversation_id = target.conversation_id(),
                "OneBot message rate-limited with notice"
            );
            let _ = context
                .send_bypass_plugins(OutboundMessage::text(
                    OutboundOrigin::Command,
                    t(
                        "Too many messages — please slow down a little.",
                        "消息太频繁了，请稍候再发。",
                    ),
                ))
                .await;
            return;
        }
    }

    // Platform commands are independent of the LLM group wake trigger.
    if let Some(response) = plugin_command_response {
        if let Err(error) = context.send_bypass_plugins(response).await {
            tracing::warn!(target: "miyu::qq", error = %error, "OneBot plugin command response failed");
        } else {
            tracing::info!(target: "miyu::qq", self_id, sender_id = user_id, "OneBot plugin command response sent");
        }
        return;
    }
    if let Some(command) = builtin_command {
        let response = execute_builtin_command(&state, &context, target, &event, command).await;
        if let Err(error) = context.send_bypass_plugins(response).await {
            tracing::warn!(target: "miyu::qq", error = %error, "OneBot built-in command response failed");
        } else {
            tracing::info!(target: "miyu::qq", self_id, sender_id = user_id, "OneBot built-in command response sent");
        }
        return;
    }

    let reply_ref = (!message_id.is_empty()).then_some(message_id);
    match build_and_run_turn(&state, &conn, target, &event, parsed, context.clone()).await {
        Ok(Some(dispatch)) => {
            if let Err(error) = deliver_dispatch(&state, &context, reply_ref, dispatch).await {
                tracing::warn!(target: "miyu::qq", error = %error, "OneBot reply delivery failed");
            } else {
                tracing::info!(
                    target: "miyu::qq",
                    self_id,
                    sender_id = user_id,
                    conversation_kind = target.kind(),
                    conversation_id = target.conversation_id(),
                    "OneBot reply delivered"
                );
            }
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(target: "miyu::qq", error = %error, "OneBot message handling failed");
            let _ = context
                .send_bypass_plugins(OutboundMessage::text(
                    OutboundOrigin::Command,
                    format!(
                        "{}{}",
                        t("Something went wrong: ", "出错了："),
                        safe_error_message(&error)
                    ),
                ))
                .await;
        }
    }
}

async fn execute_builtin_command(
    state: &DaemonState,
    context: &PlatformTurnContext,
    target: Target,
    event: &Value,
    command: commands::ParsedPlatformCommand,
) -> OutboundMessage {
    let response = match command {
        commands::ParsedPlatformCommand::Unknown => {
            commands::unknown_command_message(&context.config.platforms)
        }
        commands::ParsedPlatformCommand::Reset { has_arguments } => {
            let descriptor = commands::descriptor(commands::RESET_COMMAND_ID)
                .expect("the reset command descriptor is registered");
            if !commands::is_allowed(&context.config.platforms, descriptor, context.is_admin) {
                commands::permission_denied_message(&context.config.platforms, descriptor)
            } else if has_arguments {
                commands::reset_usage_message(&context.config.platforms)
            } else {
                match resolve_onebot_session(state, context, target, event) {
                    Err(error) => {
                        tracing::warn!(target: "miyu::qq", error = %error, "resolving the QQ session for reset failed");
                        t(
                            "The conversation could not be reset. Check the daemon logs for details.",
                            "无法重置当前会话，请查看 daemon 日志。",
                        )
                        .to_string()
                    }
                    Ok(session_id) => {
                        match clear_platform_session_content(state, session_id.clone()).await {
                            Ok(()) => {
                                tracing::info!(
                                    target: "miyu::qq",
                                    session_id = %session_id,
                                    sender_id = %context.sender_id,
                                    "QQ conversation reset"
                                );
                                t(
                                    "The current conversation has been reset.",
                                    "当前会话已重置。",
                                )
                                .to_string()
                            }
                            Err(PlatformSessionResetError::Busy) => t(
                                "This conversation is replying right now. Try resetting it again after the reply finishes.",
                                "当前会话正在回复，请在回复结束后重试。",
                            )
                            .to_string(),
                            Err(PlatformSessionResetError::Unavailable) => t(
                                "The Miyu core is unavailable, so the conversation was not reset.",
                                "Miyu 核心当前不可用，会话未重置。",
                            )
                            .to_string(),
                            Err(PlatformSessionResetError::Internal(error)) => {
                                tracing::warn!(target: "miyu::qq", session_id = %session_id, error = %error, "resetting the QQ conversation failed");
                                t(
                                    "The conversation could not be reset. Check the daemon logs for details.",
                                    "无法重置当前会话，请查看 daemon 日志。",
                                )
                                .to_string()
                            }
                        }
                    }
                }
            }
        }
    };
    OutboundMessage::text(OutboundOrigin::Command, response)
}

fn platform_turn_context(
    state: &DaemonState,
    conn: ConnectionHandle,
    target: Target,
    event: &Value,
    config: crate::config::AppConfig,
) -> Result<PlatformTurnContext> {
    let self_id = event.get("self_id").and_then(Value::as_i64).unwrap_or(0);
    let user_id = event.get("user_id").and_then(Value::as_i64).unwrap_or(0);
    let conversation = match target {
        Target::Private { user_id } => PlatformConversation {
            platform: "onebot".to_string(),
            account_id: self_id.to_string(),
            kind: ConversationKind::Private,
            conversation_id: user_id.to_string(),
        },
        Target::Group { group_id } => PlatformConversation {
            platform: "onebot".to_string(),
            account_id: self_id.to_string(),
            kind: ConversationKind::Group,
            conversation_id: group_id.to_string(),
        },
    };
    let sender = event.get("sender");
    let sender_display_name = sender
        .and_then(|sender| sender.get("card"))
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            sender
                .and_then(|sender| sender.get("nickname"))
                .and_then(Value::as_str)
        })
        .unwrap_or("?")
        .to_string();
    let is_admin = config.platforms.qq.admin_users.contains(&user_id);
    let adapter = Arc::new(OneBotAdapter {
        conn,
        registry: state.platforms.onebot.clone(),
        self_id,
        target,
        max_reply_chars: config.platforms.qq.max_reply_chars,
    });
    Ok(PlatformTurnContext::new(
        conversation,
        user_id.to_string(),
        sender_display_name,
        is_admin,
        config,
        state.state_store.clone(),
        adapter,
        state.platforms.plugins()?,
    ))
}

fn value_id_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

/// Turns a parsed inbound message into agent input (downloading media),
/// resolves the dedicated session and runs the turn. `Ok(None)` means
/// the message needs no reply (e.g. sticker-only).
async fn build_and_run_turn(
    state: &DaemonState,
    conn: &ConnectionHandle,
    target: Target,
    event: &Value,
    parsed: InboundMessage,
    context: Arc<PlatformTurnContext>,
) -> Result<Option<TurnDispatch>> {
    let mut content = parsed.text.trim().to_string();

    let mut images: Vec<Option<ImageAttachment>> = Vec::new();
    for media in parsed.images.into_iter().take(MAX_INBOUND_IMAGES) {
        let bytes = match media {
            MediaRef::Bytes(bytes) => bytes,
            MediaRef::Url(url) => {
                let http = state.platforms.http_client()?;
                match download_capped(&http, &url, MAX_INBOUND_IMAGE_BYTES, IMAGE_DOWNLOAD_TIMEOUT)
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
                let _ = context
                    .send_bypass_plugins(OutboundMessage::text(
                        OutboundOrigin::Command,
                        format!(
                            "{}{}",
                            t("Couldn't fetch the file: ", "文件接收失败："),
                            file.name
                        ),
                    ))
                    .await;
            }
        }
    }

    if content.is_empty() && images.is_empty() {
        if parsed.at_self {
            content = t(
                "(they @-mentioned you without any text)",
                "（对方@了你，但没有其他内容）",
            )
            .to_string();
        } else {
            return Ok(None);
        }
    }

    if let Target::Group { .. } = target {
        let user_id = event.get("user_id").and_then(Value::as_i64).unwrap_or(0);
        content = format!(
            "[{} {}({user_id})] {content}",
            t("group member", "群成员"),
            context.sender_display_name
        );
    }

    let prepared = context.prepare_turn(content).await;
    let content = prepared.content;
    let session_id = resolve_onebot_session(state, &context, target, event)?;
    let route = context.config.platforms.model_route(
        match context.conversation.kind {
            ConversationKind::Private => crate::config::PlatformConversationKind::Private,
            ConversationKind::Group => crate::config::PlatformConversationKind::Group,
        },
        &context.conversation.conversation_id,
    );
    let mut system_context = Vec::new();
    if let Some(prompt) = route
        .map(|route| route.extra_prompt.trim())
        .filter(|prompt| !prompt.is_empty())
    {
        system_context.push(format!(
            "<qq-conversation-instructions>\n{prompt}\n</qq-conversation-instructions>"
        ));
    }
    system_context.extend(prepared.system_context);
    let profile = TurnProfile {
        text_models: route.and_then(|route| route.text_models.clone()),
        multimodal_models: route.and_then(|route| route.multimodal_models.clone()),
        system_context,
        platform: Some(context),
    };
    let dispatch = run_platform_turn(state, session_id, content, images, profile).await?;
    Ok(Some(dispatch))
}

fn resolve_onebot_session(
    state: &DaemonState,
    context: &PlatformTurnContext,
    target: Target,
    event: &Value,
) -> Result<Arc<str>> {
    let session_name = session_name_for(target, event);
    let legacy_name = legacy_session_name_for(target);
    resolve_platform_session(
        state,
        &context.conversation,
        None,
        &session_name,
        Some(&legacy_name),
    )
}

/// Session-name key for this conversation. Group history is always shared by
/// the whole group; the bot account still isolates multiple QQ adapters.
fn session_name_for(target: Target, event: &Value) -> String {
    let self_id = event.get("self_id").and_then(Value::as_i64).unwrap_or(0);
    match target {
        Target::Private { user_id } => format!("qq:{self_id}:private:{user_id}"),
        Target::Group { group_id } => format!("qq:{self_id}:group:{group_id}"),
    }
}

fn legacy_session_name_for(target: Target) -> String {
    match target {
        Target::Private { user_id } => format!("qq:private:{user_id}"),
        Target::Group { group_id } => format!("qq:group:{group_id}"),
    }
}

/// Resolves a download URL for an inbound file (direct, or via the
/// NapCat file-URL APIs), downloads it capped and saves it under the
/// data dir. Returns the saved path.
async fn fetch_inbound_file(
    state: &DaemonState,
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
    let _file_store_guard = state.platforms.file_store_lock.lock().await;
    ensure_platform_file_capacity(
        &state.paths.data_dir,
        MAX_INBOUND_FILE_BYTES as u64,
        PLATFORM_FILE_STORAGE_BYTES,
        PLATFORM_FILE_STORAGE_ENTRIES,
        PLATFORM_FILE_TTL,
    )
    .await?;
    let http = state.platforms.http_client()?;
    download_platform_file_capped(
        &http,
        &url,
        &state.paths.data_dir,
        &file.name,
        MAX_INBOUND_FILE_BYTES,
        FILE_DOWNLOAD_TIMEOUT,
    )
    .await
}

async fn ensure_platform_file_capacity(
    data_dir: &std::path::Path,
    reserve: u64,
    max_bytes: u64,
    max_entries: usize,
    ttl: Duration,
) -> Result<()> {
    let dir = data_dir.join("platform_files");
    tokio::fs::create_dir_all(&dir).await?;
    let mut entries = tokio::fs::read_dir(&dir).await?;
    let mut bytes = 0_u64;
    let mut count = 0usize;
    while let Some(entry) = entries.next_entry().await? {
        let metadata = match entry.metadata().await {
            Ok(metadata) if metadata.is_file() => metadata,
            _ => continue,
        };
        let expired = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > ttl);
        if expired {
            let _ = tokio::fs::remove_file(entry.path()).await;
            continue;
        }
        bytes = bytes
            .checked_add(metadata.len())
            .context("platform file storage size overflow")?;
        count = count.saturating_add(1);
    }
    if count >= max_entries || bytes.saturating_add(reserve) > max_bytes {
        bail!("platform file storage quota is full");
    }
    Ok(())
}

async fn download_platform_file_capped(
    client: &reqwest::Client,
    url: &str,
    data_dir: &std::path::Path,
    name: &str,
    max_bytes: usize,
    timeout: Duration,
) -> Result<PathBuf> {
    let response = client
        .get(url)
        .timeout(timeout)
        .send()
        .await
        .with_context(|| format!("requesting {url}"))?
        .error_for_status()
        .with_context(|| format!("downloading {url}"))?;
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        bail!(
            "the file is larger than the {}MB limit",
            max_bytes / 1024 / 1024
        );
    }
    let (path, mut output) = create_platform_file(data_dir, name).await?;
    let result = async {
        let mut total = 0usize;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.with_context(|| format!("reading {url}"))?;
            total = total
                .checked_add(chunk.len())
                .context("platform file size overflow")?;
            if total > max_bytes {
                bail!(
                    "the file is larger than the {}MB limit",
                    max_bytes / 1024 / 1024
                );
            }
            output.write_all(&chunk).await?;
        }
        output.flush().await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if let Err(error) = result {
        drop(output);
        let _ = tokio::fs::remove_file(&path).await;
        return Err(error);
    }
    Ok(path)
}

/// Saves inbound bytes under `<data_dir>/platform_files/`, keeping only
/// the basename (no path traversal) and suffixing on collision.
async fn save_platform_file(
    data_dir: &std::path::Path,
    name: &str,
    bytes: &[u8],
) -> Result<PathBuf> {
    let (path, mut output) = create_platform_file(data_dir, name).await?;
    if let Err(error) = output.write_all(bytes).await {
        drop(output);
        let _ = tokio::fs::remove_file(&path).await;
        return Err(error).context("writing the inbound platform file");
    }
    Ok(path)
}

async fn create_platform_file(
    data_dir: &std::path::Path,
    name: &str,
) -> Result<(PathBuf, tokio::fs::File)> {
    let dir = data_dir.join("platform_files");
    tokio::fs::create_dir_all(&dir).await?;
    let safe = sanitize_file_name(name);
    for counter in 0..=1000 {
        let path = std::path::Path::new(&safe);
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("file");
        let file_name = match (counter, path.extension().and_then(|ext| ext.to_str())) {
            (0, _) => safe.clone(),
            (_, Some(ext)) => format!("{stem}-{counter}.{ext}"),
            (_, None) => format!("{stem}-{counter}"),
        };
        let candidate = dir.join(file_name);
        let output = match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
            .await
        {
            Ok(output) => output,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("creating the inbound platform file"),
        };
        return Ok((candidate, output));
    }
    bail!("too many files with the same name")
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
    if parsed.at_self {
        return Some(parsed.text.clone());
    }
    let text = parsed.text.trim_start();
    let keyword = config
        .group_chats
        .trigger_keywords
        .iter()
        .filter(|keyword| text.starts_with(keyword.as_str()))
        .max_by_key(|keyword| keyword.chars().count())?;
    let rest = &text[keyword.len()..];
    Some(
        rest.trim_start_matches(|ch: char| {
            ch.is_whitespace() || matches!(ch, ':' | '：' | ',' | '，')
        })
        .to_string(),
    )
}

fn decode_cq_text(text: &str) -> String {
    text.replace("&#91;", "[")
        .replace("&#93;", "]")
        .replace("&amp;", "&")
}

fn parse_cq_string(raw: &str, self_id: i64) -> InboundMessage {
    let mut parsed = InboundMessage::default();
    let mut remaining = raw;
    while let Some(start) = remaining.find("[CQ:") {
        parsed.text.push_str(&decode_cq_text(&remaining[..start]));
        let segment = &remaining[start + 4..];
        let Some(end) = segment.find(']') else {
            parsed.text.push_str(&decode_cq_text(&remaining[start..]));
            return parsed;
        };
        let body = &segment[..end];
        let mut fields = body.split(',');
        if fields.next() == Some("at") {
            let self_id = self_id.to_string();
            parsed.at_self |= fields.any(|field| {
                field
                    .strip_prefix("qq=")
                    .is_some_and(|qq| qq == self_id.as_str())
            });
        }
        remaining = &segment[end + 1..];
    }
    parsed.text.push_str(&decode_cq_text(remaining));
    parsed
}

/// Parses the OneBot `message` field (segment array, or raw string as a
/// fallback when NapCat isn't configured for array format).
fn parse_message(
    message: Option<&Value>,
    raw_message: Option<&Value>,
    self_id: i64,
) -> InboundMessage {
    let mut parsed = InboundMessage::default();
    let Some(Value::Array(segments)) = message else {
        if let Some(raw) = message
            .and_then(Value::as_str)
            .or_else(|| raw_message.and_then(Value::as_str))
        {
            return parse_cq_string(raw, self_id);
        }
        return parsed;
    };
    for segment in segments {
        let kind = segment.get("type").and_then(Value::as_str).unwrap_or("");
        let data = segment.get("data").unwrap_or(&Value::Null);
        match kind {
            "text" => {
                if let Some(text) = data.get("text").and_then(Value::as_str) {
                    parsed.text.push_str(text);
                }
            }
            "image" => {
                if parsed.images.len() >= MAX_INBOUND_IMAGES {
                    continue;
                }
                let file = data.get("file").and_then(Value::as_str).unwrap_or("");
                if let Some(encoded) = file.strip_prefix("base64://") {
                    let max_encoded = MAX_INBOUND_IMAGE_BYTES
                        .saturating_add(2)
                        .div_ceil(3)
                        .saturating_mul(4);
                    if encoded.len() <= max_encoded {
                        if let Ok(bytes) = BASE64.decode(encoded) {
                            if bytes.len() <= MAX_INBOUND_IMAGE_BYTES {
                                parsed.images.push(MediaRef::Bytes(bytes));
                            }
                        }
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
                if parsed.files.len() >= MAX_INBOUND_FILES {
                    continue;
                }
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

struct OneBotAdapter {
    conn: ConnectionHandle,
    registry: Arc<Mutex<ConnectionRegistry>>,
    self_id: i64,
    target: Target,
    max_reply_chars: usize,
}

impl PlatformAdapter for OneBotAdapter {
    fn send<'a>(&'a self, message: OutboundMessage) -> BoxFuture<'a, Result<SendReceipt>> {
        Box::pin(async move { self.send_message(message).await })
    }

    fn bot_display_name<'a>(&'a self) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let conn = self.connection();
            if let Some(name) = conn.bot_name.lock().unwrap().clone() {
                return Ok(name);
            }
            let data = conn.call_api("get_login_info", json!({})).await?;
            let name = data
                .get("nickname")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or("Bot")
                .to_string();
            *conn.bot_name.lock().unwrap() = Some(name.clone());
            Ok(name)
        })
    }
}

impl OneBotAdapter {
    fn connection(&self) -> ConnectionHandle {
        self.registry
            .lock()
            .unwrap()
            .handle(self.self_id)
            .unwrap_or_else(|| self.conn.clone())
    }

    async fn send_message(&self, message: OutboundMessage) -> Result<SendReceipt> {
        match message.body {
            OutboundBody::Segments(segments) => {
                self.send_segments(segments, message.reply_to.as_deref())
                    .await
            }
            OutboundBody::Forward(nodes) => self.send_forward(nodes).await,
        }
    }

    async fn send_segments(
        &self,
        segments: Vec<OutboundSegment>,
        reply_to: Option<&str>,
    ) -> Result<SendReceipt> {
        let mut frames: Vec<Vec<Value>> = Vec::new();
        let mut current = Vec::new();
        let mut files = Vec::new();
        for segment in segments {
            match segment {
                OutboundSegment::Markdown(text) => {
                    append_text_chunks(
                        &mut frames,
                        &mut current,
                        &markdown_to_plain(&text),
                        self.max_reply_chars,
                    );
                }
                OutboundSegment::Text(text) => {
                    append_text_chunks(&mut frames, &mut current, &text, self.max_reply_chars)
                }
                OutboundSegment::Mention(user_id) => current.push(json!({
                    "type": "at",
                    "data": { "qq": user_id },
                })),
                OutboundSegment::ImageBytes { data, .. } => {
                    if data.len() > MAX_OUTBOUND_IMAGE_BYTES {
                        bail!("outbound image exceeds the 20 MiB limit");
                    }
                    current.push(image_segment(&data));
                }
                OutboundSegment::ImagePath { path, .. } => {
                    let bytes = read_file_capped(&path, MAX_OUTBOUND_IMAGE_BYTES).await?;
                    // Decode dimensions before giving untrusted/generated bytes
                    // to the adapter, matching WebUI image safety expectations.
                    image::load_from_memory(&bytes)
                        .with_context(|| format!("decoding image {}", path.display()))?;
                    current.push(image_segment(&bytes));
                }
                OutboundSegment::FilePath { path, name } => {
                    if !current.is_empty() {
                        frames.push(std::mem::take(&mut current));
                    }
                    files.push((path, name));
                }
            }
        }
        if !current.is_empty() {
            frames.push(current);
        }

        let mut receipt = SendReceipt::default();
        for (index, mut segments) in frames.into_iter().enumerate() {
            if index == 0 {
                if let (Target::Group { .. }, Some(reply_to)) = (self.target, reply_to) {
                    segments.insert(0, json!({ "type": "reply", "data": { "id": reply_to } }));
                }
            }
            let data = self.send_message_segments(segments).await?;
            if let Some(id) = data.get("message_id").and_then(value_id_string) {
                receipt.message_ids.push(id);
            }
        }
        for (path, name) in files {
            let id = self.upload_file(&path, name.as_deref()).await?;
            if let Some(id) = id {
                receipt.message_ids.push(id);
            }
        }
        Ok(receipt)
    }

    async fn send_forward(&self, nodes: Vec<ForwardNode>) -> Result<SendReceipt> {
        if nodes.is_empty() {
            bail!("a forward message needs at least one node");
        }
        let mut messages = Vec::with_capacity(nodes.len());
        for node in nodes {
            let mut content = Vec::new();
            for segment in node.segments {
                match segment {
                    OutboundSegment::Markdown(text) => {
                        content.push(text_segment(&markdown_to_plain(&text)));
                    }
                    OutboundSegment::Text(text) => content.push(text_segment(&text)),
                    OutboundSegment::Mention(user_id) => content.push(json!({
                        "type": "at",
                        "data": { "qq": user_id },
                    })),
                    OutboundSegment::ImageBytes { data, .. } => {
                        if data.len() > MAX_OUTBOUND_IMAGE_BYTES {
                            bail!("outbound image exceeds the 20 MiB limit");
                        }
                        content.push(image_segment(&data));
                    }
                    OutboundSegment::ImagePath { path, .. } => {
                        let bytes = read_file_capped(&path, MAX_OUTBOUND_IMAGE_BYTES).await?;
                        image::load_from_memory(&bytes)
                            .with_context(|| format!("decoding image {}", path.display()))?;
                        content.push(image_segment(&bytes));
                    }
                    OutboundSegment::FilePath { .. } => {
                        bail!("files cannot be embedded in a OneBot forward node")
                    }
                }
            }
            messages.push(json!({
                "type": "node",
                "data": {
                    "uin": node.user_id,
                    "name": node.display_name,
                    "content": content,
                }
            }));
        }
        let (action, params) = match self.target {
            Target::Private { user_id } => (
                "send_private_forward_msg",
                json!({ "user_id": user_id, "messages": messages }),
            ),
            Target::Group { group_id } => (
                "send_group_forward_msg",
                json!({ "group_id": group_id, "messages": messages }),
            ),
        };
        let data = self.connection().call_api(action, params).await?;
        Ok(SendReceipt {
            message_ids: data
                .get("message_id")
                .and_then(value_id_string)
                .into_iter()
                .collect(),
        })
    }

    async fn send_message_segments(&self, segments: Vec<Value>) -> Result<Value> {
        let (action, params) = match self.target {
            Target::Private { user_id } => (
                "send_private_msg",
                json!({ "user_id": user_id, "message": segments }),
            ),
            Target::Group { group_id } => (
                "send_group_msg",
                json!({ "group_id": group_id, "message": segments }),
            ),
        };
        self.connection().call_api(action, params).await
    }

    async fn upload_file(
        &self,
        path: &std::path::Path,
        name: Option<&str>,
    ) -> Result<Option<String>> {
        let metadata = tokio::fs::metadata(path)
            .await
            .with_context(|| format!("reading outbound file metadata: {}", path.display()))?;
        if !metadata.is_file() {
            bail!(
                "outbound attachment is not a regular file: {}",
                path.display()
            );
        }
        if metadata.len() > MAX_OUTBOUND_FILE_BYTES as u64 {
            bail!("outbound attachment exceeds the 50 MiB limit");
        }
        let name = name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .or_else(|| path.file_name().and_then(|name| name.to_str()))
            .unwrap_or("file");
        let name = sanitize_file_name(name);
        let conn = self.connection();
        if let Some(base_url) = conn.asset_base_url.as_deref() {
            let lease = conn.assets.create(base_url, path, &name).await?;
            match self.upload_file_source(&lease.url, &name).await {
                Ok(id) => return Ok(id),
                Err(error) => tracing::warn!(
                    error = %error,
                    "NapCat could not fetch streamed file; considering base64 fallback"
                ),
            }
        }
        if metadata.len() > MAX_BASE64_FILE_BYTES as u64 {
            bail!(
                "NapCat could not fetch the temporary file URL and the file exceeds the 16 MiB base64 fallback limit"
            );
        }
        let bytes = read_file_capped(path, MAX_BASE64_FILE_BYTES).await?;
        self.upload_file_source(&format!("base64://{}", BASE64.encode(bytes)), &name)
            .await
    }

    async fn upload_file_source(&self, source: &str, name: &str) -> Result<Option<String>> {
        let (action, params) = match self.target {
            Target::Private { user_id } => (
                "upload_private_file",
                json!({ "user_id": user_id, "file": source, "name": name }),
            ),
            Target::Group { group_id } => (
                "upload_group_file",
                json!({ "group_id": group_id, "file": source, "name": name }),
            ),
        };
        let data = self
            .conn
            .call_api_with_timeout(action, params, FILE_DOWNLOAD_TIMEOUT)
            .await?;
        Ok(data.get("file_id").and_then(value_id_string))
    }
}

fn append_text_chunks(
    frames: &mut Vec<Vec<Value>>,
    current: &mut Vec<Value>,
    text: &str,
    max_reply_chars: usize,
) {
    let chunks = split_reply(text, max_reply_chars);
    let count = chunks.len();
    for (index, chunk) in chunks.into_iter().enumerate() {
        current.push(text_segment(&chunk));
        if index + 1 < count {
            frames.push(std::mem::take(current));
        }
    }
}

fn image_segment(bytes: &[u8]) -> Value {
    json!({
        "type": "image",
        "data": { "file": format!("base64://{}", BASE64.encode(bytes)) },
    })
}

async fn read_file_capped(path: &std::path::Path, cap: usize) -> Result<Vec<u8>> {
    let file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("opening attachment: {}", path.display()))?;
    let metadata = file
        .metadata()
        .await
        .with_context(|| format!("reading attachment metadata: {}", path.display()))?;
    if !metadata.is_file() {
        bail!("attachment is not a regular file: {}", path.display());
    }
    if metadata.len() > cap as u64 {
        bail!("attachment exceeds the {} MiB limit", cap / 1024 / 1024);
    }
    let limit = u64::try_from(cap.saturating_add(1)).unwrap_or(u64::MAX);
    let mut reader = file.take(limit);
    let mut bytes = Vec::with_capacity(metadata.len().min(cap as u64) as usize);
    reader
        .read_to_end(&mut bytes)
        .await
        .with_context(|| format!("reading attachment: {}", path.display()))?;
    if bytes.len() > cap {
        bail!("attachment exceeds the {} MiB limit", cap / 1024 / 1024);
    }
    Ok(bytes)
}

async fn deliver_dispatch(
    state: &DaemonState,
    context: &Arc<PlatformTurnContext>,
    reply_ref: Option<String>,
    dispatch: TurnDispatch,
) -> Result<()> {
    match dispatch {
        TurnDispatch::Queued => {
            context
                .send_bypass_plugins(OutboundMessage::text(
                    OutboundOrigin::Command,
                    t(
                "Got it — still finishing the previous message; I'll reply to both together.",
                "收到，上一条还在处理中，稍后一起回复。",
                    ),
                ))
                .await?;
        }
        TurnDispatch::Failed(message) => {
            context
                .send_bypass_plugins(OutboundMessage::text(
                    OutboundOrigin::Command,
                    format!("{}{message}", t("Something went wrong: ", "出错了：")),
                ))
                .await?;
        }
        TurnDispatch::Completed(outcome) => {
            let mut segments = Vec::new();
            let reply_text = final_reply_text(&outcome);
            if !reply_text.trim().is_empty() {
                segments.push(OutboundSegment::Markdown(reply_text));
            }
            for asset_id in &outcome.image_assets {
                match state.state_store.load_image_asset(asset_id) {
                    Ok(Some(asset)) => {
                        segments.push(OutboundSegment::ImageBytes {
                            mime: asset.asset.mime,
                            data: Arc::from(asset.bytes),
                            alt: asset.asset.alt,
                        });
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::warn!(error = %error, asset_id, "loading an image asset for OneBot failed");
                    }
                }
            }
            if segments.is_empty() {
                if outcome.final_reply_already_sent {
                    return Ok(());
                }
                segments.push(OutboundSegment::Text(
                    t(
                        "(this turn produced no text reply)",
                        "（这一轮没有产生文本回复）",
                    )
                    .to_string(),
                ));
            }
            let mut message = OutboundMessage::segments(OutboundOrigin::FinalReply, segments);
            message.reply_to = reply_ref;
            context.send(message).await?;
        }
    }
    Ok(())
}

fn final_reply_text(outcome: &super::TurnOutcome) -> String {
    if outcome.suppressed_reply_ranges.is_empty() {
        return outcome.text.clone();
    }
    let mut text = String::with_capacity(outcome.text.len());
    let mut cursor = 0;
    for &(start, end) in &outcome.suppressed_reply_ranges {
        let start = start.clamp(cursor, outcome.text.len());
        let end = end.clamp(start, outcome.text.len());
        let (Some(prefix), Some(_suppressed)) = (
            outcome.text.get(cursor..start),
            outcome.text.get(start..end),
        ) else {
            continue;
        };
        text.push_str(prefix);
        cursor = end;
    }
    if let Some(suffix) = outcome.text.get(cursor..) {
        text.push_str(suffix);
    }
    text
}

fn text_segment(text: &str) -> Value {
    json!({ "type": "text", "data": { "text": text } })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::MiyuPaths;

    fn test_paths(root: &std::path::Path) -> MiyuPaths {
        MiyuPaths {
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

    fn test_web_state(root: &std::path::Path, web_port: u16) -> DaemonState {
        DaemonState::for_test(test_paths(root), web_port).unwrap()
    }

    fn config_with(mutate: impl FnOnce(&mut OneBotConfig)) -> OneBotConfig {
        let mut config = OneBotConfig::default();
        mutate(&mut config);
        config
    }

    #[tokio::test]
    async fn listener_rebind_is_transactional_and_reuses_the_web_port() {
        let temp = tempfile::tempdir().unwrap();
        let web_listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let web_port = web_listener.local_addr().unwrap().port();
        let state = test_web_state(temp.path(), web_port);
        let listener = state.platforms.qq_listener.clone();

        let shared = config_with(|config| {
            config.enabled = true;
            config.reverse_ws_port = web_port;
        });
        listener
            .prepare(&state, None, &shared)
            .await
            .unwrap()
            .commit();
        {
            let inner = listener.inner.lock().unwrap();
            assert_eq!(inner.active_port, Some(web_port));
            assert!(inner.task.is_none());
        }

        let available = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let dedicated_port = available.local_addr().unwrap().port();
        drop(available);
        let dedicated = config_with(|config| {
            config.enabled = true;
            config.reverse_ws_port = dedicated_port;
        });
        listener
            .prepare(&state, Some(&shared), &dedicated)
            .await
            .unwrap()
            .commit();
        {
            let inner = listener.inner.lock().unwrap();
            assert_eq!(inner.active_port, Some(dedicated_port));
            assert!(inner.task.is_some());
        }

        let occupied = tokio::net::TcpListener::bind(("0.0.0.0", 0)).await.unwrap();
        let occupied_port = occupied.local_addr().unwrap().port();
        let conflict = config_with(|config| {
            config.enabled = true;
            config.reverse_ws_port = occupied_port;
        });
        assert!(listener
            .prepare(&state, Some(&dedicated), &conflict)
            .await
            .is_err());
        {
            let inner = listener.inner.lock().unwrap();
            assert_eq!(inner.active_port, Some(dedicated_port));
            assert!(inner.task.is_some());
        }

        let disabled = OneBotConfig::default();
        listener
            .prepare(&state, Some(&dedicated), &disabled)
            .await
            .unwrap()
            .commit();
        let inner = listener.inner.lock().unwrap();
        assert_eq!(inner.active_port, None);
        assert!(inner.task.is_none());
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
        assert!(
            matches!(&parsed.images[0], MediaRef::Url(url) if url == "https://img.example/x.jpg")
        );
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

        let reply_command = json!("[CQ:reply,id=5][CQ:at,qq=10001] /reset");
        let parsed = parse_message(Some(&reply_command), None, 10001);
        assert!(parsed.at_self);
        assert_eq!(parsed.text, " /reset");
        assert_eq!(
            commands::parse(&crate::config::PlatformsConfig::default(), &parsed.text),
            Some(commands::ParsedPlatformCommand::Reset {
                has_arguments: false
            })
        );

        let escaped_literal = json!("&#91;CQ:reply,id=5&#93;/reset");
        let parsed = parse_message(Some(&escaped_literal), None, 1);
        assert_eq!(parsed.text, "[CQ:reply,id=5]/reset");
    }

    #[test]
    fn inbound_parser_caps_media_segment_counts() {
        let message = Value::Array(
            (0..8)
                .flat_map(|index| {
                    [
                        json!({
                            "type": "image",
                            "data": { "url": format!("https://img.example/{index}.png") }
                        }),
                        json!({
                            "type": "file",
                            "data": { "file_id": format!("f{index}"), "file_name": "x.txt" }
                        }),
                    ]
                })
                .collect(),
        );
        let parsed = parse_message(Some(&message), None, 1);
        assert_eq!(parsed.images.len(), MAX_INBOUND_IMAGES);
        assert_eq!(parsed.files.len(), MAX_INBOUND_FILES);
    }

    #[test]
    fn confirmed_direct_send_only_suppresses_later_assistant_text() {
        let outcome = super::super::TurnOutcome {
            text: "首条消息的回答\n工具发送后的重复确认".to_string(),
            image_assets: Vec::new(),
            suppressed_reply_ranges: vec![(
                "首条消息的回答".len(),
                "首条消息的回答\n工具发送后的重复确认".len(),
            )],
            final_reply_already_sent: true,
        };
        assert_eq!(final_reply_text(&outcome), "首条消息的回答");

        let unsuppressed = super::super::TurnOutcome {
            suppressed_reply_ranges: Vec::new(),
            final_reply_already_sent: false,
            ..outcome
        };
        assert_eq!(
            final_reply_text(&unsuppressed),
            "首条消息的回答\n工具发送后的重复确认"
        );
    }

    #[test]
    fn direct_send_suppression_keeps_a_later_queued_answer() {
        let prefix = "首条回答";
        let duplicate = "工具确认";
        let queued = "排队消息的回答";
        let text = format!("{prefix}{duplicate}{queued}");
        let outcome = super::super::TurnOutcome {
            text,
            image_assets: Vec::new(),
            suppressed_reply_ranges: vec![(prefix.len(), prefix.len() + duplicate.len())],
            final_reply_already_sent: false,
        };
        assert_eq!(final_reply_text(&outcome), format!("{prefix}{queued}"));
    }

    #[test]
    fn group_trigger_matrix() {
        let at_only = OneBotConfig::default();
        let mut parsed = InboundMessage {
            text: "/cmd 查询".into(),
            ..Default::default()
        };
        assert!(group_trigger_text(&at_only, &parsed).is_none());
        parsed.at_self = true;
        assert_eq!(
            group_trigger_text(&at_only, &parsed).as_deref(),
            Some("/cmd 查询")
        );

        let prefix = config_with(|config| {
            config.group_chats.trigger_keywords = vec!["/cmd".into()];
        });
        parsed.at_self = false;
        assert_eq!(
            group_trigger_text(&prefix, &parsed).as_deref(),
            Some("查询")
        );
        parsed.text = "无前缀".into();
        assert!(group_trigger_text(&prefix, &parsed).is_none());

        // An empty keyword list never fires (avoids always-on).
        let empty_prefix = OneBotConfig::default();
        assert!(group_trigger_text(&empty_prefix, &parsed).is_none());

        let either = config_with(|config| {
            config.group_chats.trigger_keywords = vec!["喵".into(), "喵喵".into()];
        });
        parsed.text = "喵喵：早上好".into();
        assert_eq!(
            group_trigger_text(&either, &parsed).as_deref(),
            Some("早上好")
        );
    }

    #[test]
    fn admission_matrix_uses_private_and_group_conversation_buckets() {
        let mut config = OneBotConfig::default();
        config.admin_users.push(1);
        config.private_chats.whitelist.push(2);
        config.group_chats.whitelist.push(10);

        let admin = admission_for(&config, Target::Group { group_id: 99 }, 100, 1);
        assert!(admin.allowed);
        assert!(admin.rate_key.is_none());

        let private_whitelist = admission_for(&config, Target::Private { user_id: 2 }, 100, 2);
        assert!(private_whitelist.allowed);
        assert!(private_whitelist.rate_key.is_none());

        let private_guest = admission_for(&config, Target::Private { user_id: 3 }, 100, 3);
        assert!(private_guest.allowed);
        assert_eq!(private_guest.rate_limit, 3);
        assert_eq!(private_guest.rate_key.as_deref(), Some("qq:100:private:3"));

        let group_whitelist = admission_for(&config, Target::Group { group_id: 10 }, 100, 2);
        assert!(group_whitelist.allowed);
        assert_eq!(group_whitelist.rate_limit, 30);
        assert_eq!(group_whitelist.rate_key.as_deref(), Some("qq:100:group:10"));

        let group_guest = admission_for(&config, Target::Group { group_id: 11 }, 100, 3);
        assert!(group_guest.allowed);
        assert_eq!(group_guest.rate_limit, 10);
        assert_eq!(group_guest.rate_key.as_deref(), Some("qq:100:group:11"));

        config.private_chats.allow_non_whitelist = false;
        config.group_chats.allow_non_whitelist = false;
        assert!(!admission_for(&config, Target::Private { user_id: 3 }, 100, 3).allowed);
        assert!(!admission_for(&config, Target::Group { group_id: 11 }, 100, 3).allowed);
    }

    #[tokio::test]
    async fn reset_command_uses_configured_admins_and_clears_the_bound_session() {
        let temp = tempfile::tempdir().unwrap();
        let (state, actor_join) =
            DaemonState::for_test_with_actor(test_paths(temp.path()), 8300).unwrap();
        let target = Target::Group { group_id: 99 };
        let event = json!({
            "self_id": 10000,
            "user_id": 42,
            "message_type": "group",
            "group_id": 99,
            "message_id": 7,
            "message": [{ "type": "text", "data": { "text": "/reset extra" } }],
            "sender": { "nickname": "Alice", "role": "owner" }
        });
        state.manager.lock().unwrap().config.platforms.qq.enabled = true;
        let (connection, mut frames) = test_connection(None);
        let persona = state.manager.lock().unwrap().config.active_persona_scope();
        let sessions_before = state
            .state_store
            .list_sessions(&persona, true)
            .unwrap()
            .len();

        // QQ group roles never grant Miyu command administration.
        let denied = tokio::spawn(handle_message(
            state.clone(),
            connection.clone(),
            event.clone(),
        ));
        let denied_frame: Value = serde_json::from_str(&frames.recv().await.unwrap()).unwrap();
        assert_eq!(denied_frame["action"], "send_group_msg");
        let expected_denial = commands::permission_denied_message(
            &state.manager.lock().unwrap().config.platforms,
            commands::descriptor(commands::RESET_COMMAND_ID).unwrap(),
        );
        assert_eq!(
            denied_frame["params"]["message"][0]["data"]["text"],
            expected_denial
        );
        route_api_response(
            &connection,
            json!({
                "status": "ok",
                "retcode": 0,
                "data": { "message_id": 70 },
                "echo": denied_frame["echo"],
            }),
        );
        denied.await.unwrap();
        assert_eq!(
            state
                .state_store
                .list_sessions(&persona, true)
                .unwrap()
                .len(),
            sessions_before
        );

        state
            .manager
            .lock()
            .unwrap()
            .config
            .platforms
            .qq
            .admin_users
            .push(42);
        let context = platform_turn_context(
            &state,
            connection.clone(),
            target,
            &event,
            state.manager.lock().unwrap().config.clone(),
        )
        .unwrap();
        assert!(context.is_admin);
        let session_id = resolve_onebot_session(&state, &context, target, &event).unwrap();
        let store = state.state_store.pinned(&session_id);
        store
            .start_turn("qq_history", "hello", std::process::id())
            .unwrap();
        store.complete_turn("qq_history", "world", None).unwrap();

        let mut raw_reset_event = event.clone();
        raw_reset_event["message"] = json!("[CQ:reply,id=6]/reset");
        let reset = tokio::spawn(handle_message(
            state.clone(),
            connection.clone(),
            raw_reset_event,
        ));
        let reset_frame: Value = serde_json::from_str(&frames.recv().await.unwrap()).unwrap();
        assert_eq!(reset_frame["action"], "send_group_msg");
        route_api_response(
            &connection,
            json!({
                "status": "ok",
                "retcode": 0,
                "data": { "message_id": 71 },
                "echo": reset_frame["echo"],
            }),
        );
        reset.await.unwrap();
        assert!(store.load_turns().unwrap().is_empty());
        assert_eq!(
            resolve_onebot_session(&state, &context, target, &event).unwrap(),
            session_id
        );
        assert!(!state.manager.lock().unwrap().admin_busy);

        state
            .actor_tx
            .send(crate::web::ActorCommand::Shutdown)
            .unwrap();
        actor_join.join().unwrap().unwrap();
    }

    #[test]
    fn sanitizes_file_names() {
        assert_eq!(sanitize_file_name("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_file_name("C:\\evil\\x.exe"), "x.exe");
        assert_eq!(sanitize_file_name(".."), "file");
        assert_eq!(sanitize_file_name("  "), "file");
        assert_eq!(sanitize_file_name("报告 v2.pdf"), "报告 v2.pdf");
    }

    #[tokio::test]
    async fn concurrent_inbound_files_with_the_same_name_do_not_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let first = save_platform_file(temp.path(), "report.txt", b"first");
        let second = save_platform_file(temp.path(), "report.txt", b"second");
        let (first, second) = tokio::join!(first, second);
        let first = first.unwrap();
        let second = second.unwrap();

        assert_ne!(first, second);
        let mut contents = vec![
            tokio::fs::read(first).await.unwrap(),
            tokio::fs::read(second).await.unwrap(),
        ];
        contents.sort();
        assert_eq!(contents, vec![b"first".to_vec(), b"second".to_vec()]);
    }

    #[tokio::test]
    async fn inbound_file_store_enforces_a_total_capacity() {
        let temp = tempfile::tempdir().unwrap();
        save_platform_file(temp.path(), "existing.bin", b"12345678")
            .await
            .unwrap();

        assert!(
            ensure_platform_file_capacity(temp.path(), 2, 10, 10, Duration::from_secs(60),)
                .await
                .is_ok()
        );
        assert!(
            ensure_platform_file_capacity(temp.path(), 3, 10, 10, Duration::from_secs(60),)
                .await
                .is_err()
        );
    }

    #[test]
    fn outbound_frames_have_the_onebot_shape() {
        let frame: Value = serde_json::from_str(&api_frame(
            "send_private_msg",
            json!({ "user_id": 42, "message": [text_segment("hi")] }),
            "test",
        ))
        .unwrap();
        assert_eq!(frame["action"], "send_private_msg");
        assert_eq!(frame["params"]["user_id"], 42);
        assert_eq!(frame["params"]["message"][0]["type"], "text");
        assert_eq!(frame["params"]["message"][0]["data"]["text"], "hi");
        assert!(frame["echo"].as_str().is_some());

        let frame: Value = serde_json::from_str(&api_frame(
            "send_group_msg",
            json!({ "group_id": 7, "message": [text_segment("x")] }),
            "test",
        ))
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

    #[test]
    fn empty_token_only_authorizes_loopback_connections() {
        let headers = HeaderMap::new();
        assert!(connection_authorized(
            &headers,
            "",
            "127.0.0.1:1234".parse().unwrap()
        ));
        assert!(connection_authorized(
            &headers,
            "",
            "[::1]:1234".parse().unwrap()
        ));
        assert!(!connection_authorized(
            &headers,
            "",
            "192.168.1.5:1234".parse().unwrap()
        ));
    }

    fn test_connection(
        asset_base_url: Option<String>,
    ) -> (ConnectionHandle, mpsc::UnboundedReceiver<String>) {
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let (shutdown, _shutdown_rx) = watch::channel(false);
        (
            ConnectionHandle {
                out_tx,
                pending: Arc::new(Mutex::new(HashMap::new())),
                bot_name: Arc::new(Mutex::new(None)),
                asset_base_url,
                assets: super::super::assets::AssetLeaseStore::new(),
                busy_notice_pending: Arc::new(AtomicBool::new(false)),
                shutdown,
            },
            out_rx,
        )
    }

    fn test_adapter(handle: ConnectionHandle, target: Target) -> OneBotAdapter {
        let mut registry = ConnectionRegistry::default();
        registry.register(10000, handle.clone());
        OneBotAdapter {
            conn: handle,
            registry: Arc::new(Mutex::new(registry)),
            self_id: 10000,
            target,
            max_reply_chars: 0,
        }
    }

    #[test]
    fn late_identity_binding_cannot_replace_a_newer_connection() {
        let (older, _older_frames) = test_connection(None);
        let (newer, _newer_frames) = test_connection(None);
        let mut registry = ConnectionRegistry::default();
        let older_generation = registry.register(0, older.clone());
        let newer_generation = registry.register(0, newer.clone());

        assert!(registry.bind(10000, newer_generation, newer));
        assert!(!registry.bind(10000, older_generation, older));
        assert!(registry.is_current(10000, newer_generation));
        assert!(!registry.is_current(10000, older_generation));
    }

    #[tokio::test]
    async fn api_calls_wait_for_the_matching_echo() {
        let (handle, mut frames) = test_connection(None);
        let caller = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.call_api("get_login_info", json!({})).await })
        };
        let frame: Value = serde_json::from_str(&frames.recv().await.unwrap()).unwrap();
        assert_eq!(frame["action"], "get_login_info");
        let echo = frame["echo"].as_str().unwrap().to_string();

        // An unrelated response must not resolve this request.
        route_api_response(
            &handle,
            json!({ "status": "ok", "retcode": 0, "data": null, "echo": "other" }),
        );
        assert!(!caller.is_finished());
        route_api_response(
            &handle,
            json!({
                "status": "ok",
                "retcode": 0,
                "data": { "nickname": "Miyu" },
                "echo": echo,
            }),
        );
        let data = caller.await.unwrap().unwrap();
        assert_eq!(data["nickname"], "Miyu");
        assert!(handle.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn api_call_fails_immediately_when_the_writer_is_closed() {
        let (handle, frames) = test_connection(None);
        drop(frames);
        let started = tokio::time::Instant::now();

        assert!(handle.call_api("get_status", json!({})).await.is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(handle.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn adapter_uses_the_new_connection_after_reconnect() {
        let (old_handle, mut old_frames) = test_connection(None);
        let adapter = Arc::new(test_adapter(old_handle, Target::Private { user_id: 42 }));
        let (new_handle, mut new_frames) = test_connection(None);
        adapter
            .registry
            .lock()
            .unwrap()
            .register(adapter.self_id, new_handle.clone());

        let send = {
            let adapter = adapter.clone();
            tokio::spawn(async move {
                adapter
                    .send_message_segments(vec![text_segment("hello")])
                    .await
            })
        };
        let frame: Value = serde_json::from_str(&new_frames.recv().await.unwrap()).unwrap();
        assert!(old_frames.try_recv().is_err());
        route_api_response(
            &new_handle,
            json!({
                "status": "ok",
                "retcode": 0,
                "data": { "message_id": 1 },
                "echo": frame["echo"],
            }),
        );
        assert!(send.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn file_upload_falls_back_to_base64_after_url_failure() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("sample.txt");
        tokio::fs::write(&path, b"hello").await.unwrap();
        let (handle, mut frames) = test_connection(Some("http://miyu.test:8300".to_string()));
        let adapter = test_adapter(handle.clone(), Target::Private { user_id: 42 });
        let upload = tokio::spawn(async move { adapter.upload_file(&path, None).await });

        let first: Value = serde_json::from_str(&frames.recv().await.unwrap()).unwrap();
        assert_eq!(first["action"], "upload_private_file");
        assert!(first["params"]["file"]
            .as_str()
            .unwrap()
            .starts_with("http://miyu.test:8300/api/platform-assets/"));
        route_api_response(
            &handle,
            json!({
                "status": "failed",
                "retcode": 100,
                "data": null,
                "echo": first["echo"],
            }),
        );

        let second: Value = serde_json::from_str(&frames.recv().await.unwrap()).unwrap();
        assert_eq!(second["action"], "upload_private_file");
        assert_eq!(second["params"]["file"], "base64://aGVsbG8=");
        route_api_response(
            &handle,
            json!({
                "status": "ok",
                "retcode": 0,
                "data": { "file_id": "file-1" },
                "echo": second["echo"],
            }),
        );
        assert_eq!(upload.await.unwrap().unwrap().as_deref(), Some("file-1"));
    }

    #[tokio::test]
    async fn adapter_smoke_test_sends_replies_images_and_forward_nodes() {
        let (handle, mut frames) = test_connection(None);
        let adapter = Arc::new(test_adapter(handle.clone(), Target::Group { group_id: 42 }));
        let mut message = OutboundMessage::segments(
            OutboundOrigin::FinalReply,
            vec![
                OutboundSegment::Text("hello".to_string()),
                OutboundSegment::ImageBytes {
                    mime: "image/png".to_string(),
                    data: Arc::from([1_u8, 2, 3]),
                    alt: "sample".to_string(),
                },
            ],
        );
        message.reply_to = Some("99".to_string());
        let send = {
            let adapter = adapter.clone();
            tokio::spawn(async move { adapter.send_message(message).await })
        };
        let frame: Value = serde_json::from_str(&frames.recv().await.unwrap()).unwrap();
        assert_eq!(frame["action"], "send_group_msg");
        assert_eq!(frame["params"]["group_id"], 42);
        assert_eq!(frame["params"]["message"][0]["type"], "reply");
        assert_eq!(frame["params"]["message"][1]["data"]["text"], "hello");
        assert_eq!(
            frame["params"]["message"][2]["data"]["file"],
            "base64://AQID"
        );
        route_api_response(
            &handle,
            json!({
                "status": "ok",
                "retcode": 0,
                "data": { "message_id": 123 },
                "echo": frame["echo"],
            }),
        );
        assert_eq!(send.await.unwrap().unwrap().message_ids, vec!["123"]);

        let forward = OutboundMessage {
            body: OutboundBody::Forward(vec![ForwardNode {
                user_id: "10000".to_string(),
                display_name: "Miyu".to_string(),
                segments: vec![OutboundSegment::Markdown("**long**".to_string())],
            }]),
            reply_to: Some("ignored".to_string()),
            origin: OutboundOrigin::Plugin,
            metadata: Default::default(),
        };
        let send = {
            let adapter = adapter.clone();
            tokio::spawn(async move { adapter.send_message(forward).await })
        };
        let frame: Value = serde_json::from_str(&frames.recv().await.unwrap()).unwrap();
        assert_eq!(frame["action"], "send_group_forward_msg");
        assert_eq!(frame["params"]["messages"][0]["type"], "node");
        assert_eq!(
            frame["params"]["messages"][0]["data"]["content"][0]["data"]["text"],
            "long"
        );
        route_api_response(
            &handle,
            json!({
                "status": "ok",
                "retcode": 0,
                "data": { "message_id": "forward-1" },
                "echo": frame["echo"],
            }),
        );
        assert_eq!(send.await.unwrap().unwrap().message_ids, vec!["forward-1"]);
    }
}
