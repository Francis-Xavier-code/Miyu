use crate::agent::{
    archive_and_delete_visible_turns, Agent, AgentEvent, AgentMode, AgentTurnControl,
};
use crate::cli::{build_tool_registry, WebArgs};
use crate::config::{ActiveProviderModelConfig, AppConfig};
use crate::i18n::text as t;
use crate::ipc::{
    self, Command as IpcCommand, Frame as IpcFrame, ImageAttachment, Request as IpcRequest,
};
use crate::llm::{ChatResult, ChatStreamKind, OpenAiCompatibleClient, Usage};
use crate::memory::MemoryStore;
use crate::paths::MiyuPaths;
use crate::question::{self, QuestionAnswers, QuestionRequest, QuestionResponse};
use crate::state::{
    ImageAsset, QueuedPrompt, StateStore, Turn, TurnFollowup, TurnStatus, UsageSnapshot,
};
use crate::tools::{self, CommandOutputStream};
use anyhow::{Context, Result};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE, HOST, ORIGIN, REFERRER_POLICY,
    RETRY_AFTER, SET_COOKIE, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use futures_util::stream::{self, Stream};
use futures_util::StreamExt;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::convert::Infallible;
use std::future::IntoFuture;
use std::io::{self, IsTerminal, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path as FilePath, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, oneshot, Semaphore};
use tokio::task::JoinHandle as TokioJoinHandle;

const JSON_BODY_LIMIT: usize = 4 * 1024 * 1024;
const MAX_CONTENT_CHARS: usize = 20_000;
const MAX_PROMPT_DOCUMENT_CHARS: usize = 200_000;
const MAX_PROMPT_DOCUMENTS: usize = 128;
const MAX_SECRET_CHARS: usize = 100_000;
const EVENT_CAPACITY: usize = 4096;
const AUTH_COOKIE: &str = "miyu_session";
const LOGIN_WINDOW: Duration = Duration::from_secs(60);
const LOGIN_ATTEMPT_LIMIT: u8 = 5;

const INDEX_HTML: &str = include_str!("../web/index.html");
const STYLES_CSS: &str = include_str!("../web/styles.css");
const APP_JS: &str = include_str!("../web/app.js");
const MIYU_LOGO: &[u8] = include_bytes!("../pics/miyu-logo.png");
const MIYU_WALLPAPER: &[u8] = include_bytes!("../pics/miyuwallpaper.png");

#[derive(Clone)]
struct WebState {
    auth: WebAuth,
    boot_id: Arc<str>,
    web_port: u16,
    web_public: bool,
    paths: MiyuPaths,
    manager: Arc<Mutex<ManagerState>>,
    state_store: StateStore,
    events: EventHub,
    questions: QuestionBroker,
    actor_tx: mpsc::UnboundedSender<ActorCommand>,
    shutdown_tx: broadcast::Sender<()>,
}

#[derive(Clone)]
struct WebAuth {
    password_digest: Option<[u8; 32]>,
    sessions: Arc<Mutex<HashSet<String>>>,
    attempts: Arc<Mutex<HashMap<IpAddr, LoginAttempt>>>,
}

#[derive(Clone, Copy)]
struct LoginAttempt {
    window_started: Instant,
    failures: u8,
}

#[derive(Debug, Clone, Copy)]
enum LoginFailure {
    Invalid,
    RateLimited,
}

impl WebAuth {
    fn new(password: Option<&str>) -> Self {
        let password_digest = password.map(|password| {
            let mut digest = Sha256::new();
            digest.update(password.as_bytes());
            digest.finalize().into()
        });
        Self {
            password_digest,
            sessions: Arc::new(Mutex::new(HashSet::new())),
            attempts: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn required(&self) -> bool {
        self.password_digest.is_some()
    }

    fn is_authenticated(&self, supplied: Option<&str>) -> bool {
        if !self.required() {
            return true;
        }
        supplied.is_some_and(|token| self.sessions.lock().unwrap().contains(token))
    }

    fn login(&self, peer: IpAddr, password: &str) -> std::result::Result<String, LoginFailure> {
        let Some(expected) = self.password_digest else {
            return Ok(String::new());
        };
        let now = Instant::now();
        {
            let mut attempts = self.attempts.lock().unwrap();
            let entry = attempts.entry(peer).or_insert(LoginAttempt {
                window_started: now,
                failures: 0,
            });
            if now.duration_since(entry.window_started) >= LOGIN_WINDOW {
                entry.window_started = now;
                entry.failures = 0;
            }
            if entry.failures >= LOGIN_ATTEMPT_LIMIT {
                return Err(LoginFailure::RateLimited);
            }
        }

        let mut digest = Sha256::new();
        digest.update(password.as_bytes());
        let supplied: [u8; 32] = digest.finalize().into();
        if !constant_time_eq(&supplied, &expected) {
            let mut attempts = self.attempts.lock().unwrap();
            if let Some(entry) = attempts.get_mut(&peer) {
                entry.failures = entry.failures.saturating_add(1);
            }
            return Err(LoginFailure::Invalid);
        }

        let token = random_token(32);
        let mut sessions = self.sessions.lock().unwrap();
        sessions.insert(token.clone());
        if sessions.len() > 64 {
            sessions.clear();
            sessions.insert(token.clone());
        }
        Ok(token)
    }
}

/// A turn currently executing in the daemon.
struct RunInfo {
    session_id: Arc<str>,
    mode: AgentMode,
    /// Signals cancellation to the turn task; the task selects on the
    /// paired receiver.
    cancel: tokio::sync::watch::Sender<bool>,
}

impl RunInfo {
    fn request_cancel(&self) {
        let _ = self.cancel.send(true);
    }
}

struct ManagerState {
    config: AppConfig,
    /// Concurrently running turns, keyed by run id. Turns run in parallel —
    /// including several in the same session (placeholder semantics) — so
    /// this replaces the old single `active_run_id`.
    active_runs: HashMap<String, RunInfo>,
    admin_busy: bool,
    context: ContextSnapshot,
}

impl ManagerState {
    /// A run currently executing in the given session, if any (most callers
    /// only need one representative — e.g. the WebUI compat field).
    fn run_in_session(&self, session_id: &str) -> Option<&String> {
        self.active_runs
            .iter()
            .find(|(_, info)| &*info.session_id == session_id)
            .map(|(run_id, _)| run_id)
    }

    fn session_has_runs(&self, session_id: &str) -> bool {
        self.active_runs
            .values()
            .any(|info| &*info.session_id == session_id)
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
struct ContextSnapshot {
    tokens: u64,
    window: Option<usize>,
    cumulative_tokens: u64,
}

enum ActorCommand {
    StartTurn {
        run_id: String,
        session_id: Arc<str>,
        content: String,
        mode: AgentMode,
        images: Vec<Option<ImageAttachment>>,
        cwd: Option<std::path::PathBuf>,
        cancel: tokio::sync::watch::Receiver<bool>,
    },
    SetModels {
        models: Vec<ActiveProviderModelConfig>,
        reply: oneshot::Sender<std::result::Result<(), AdminFailure>>,
    },
    ApplyConfig {
        config: AppConfig,
        prompts: PromptDocuments,
        reset_conversation: bool,
        reply: oneshot::Sender<std::result::Result<(), AdminFailure>>,
    },
    ResetConversation {
        all: bool,
        reply: oneshot::Sender<std::result::Result<(), AdminFailure>>,
    },
    SwitchSession {
        session_id: String,
        reply: oneshot::Sender<std::result::Result<(), AdminFailure>>,
    },
    Undo {
        reply: oneshot::Sender<std::result::Result<Value, AdminFailure>>,
    },
    Pop {
        turn_ids: Vec<String>,
        reply: oneshot::Sender<std::result::Result<Value, AdminFailure>>,
    },
    Compact {
        reply: oneshot::Sender<std::result::Result<Value, AdminFailure>>,
    },
    Shutdown,
}

#[derive(Debug)]
enum AdminFailure {
    Invalid(String),
    Internal(String),
}

#[derive(Clone, Debug)]
struct EventRecord {
    id: u64,
    kind: String,
    data: String,
}

#[derive(Clone)]
struct EventHub {
    inner: Arc<Mutex<EventHubInner>>,
    sender: broadcast::Sender<EventRecord>,
}

struct EventHubInner {
    next_id: u64,
    records: VecDeque<EventRecord>,
}

struct EventSubscription {
    pending: VecDeque<EventRecord>,
    receiver: broadcast::Receiver<EventRecord>,
}

impl EventHub {
    fn new() -> Self {
        let (sender, _) = broadcast::channel(EVENT_CAPACITY);
        Self {
            inner: Arc::new(Mutex::new(EventHubInner {
                next_id: 1,
                records: VecDeque::with_capacity(EVENT_CAPACITY),
            })),
            sender,
        }
    }

    fn publish(&self, kind: impl Into<String>, data: Value) -> u64 {
        let mut inner = self.inner.lock().unwrap();
        let id = inner.next_id;
        inner.next_id = inner.next_id.saturating_add(1);
        let record = EventRecord {
            id,
            kind: kind.into(),
            data: serde_json::to_string(&data)
                .unwrap_or_else(|_| "{\"error\":\"event serialization failed\"}".to_string()),
        };
        if inner.records.len() == EVENT_CAPACITY {
            inner.records.pop_front();
        }
        inner.records.push_back(record.clone());
        let _ = self.sender.send(record);
        id
    }

    fn latest_id(&self) -> u64 {
        self.inner.lock().unwrap().next_id.saturating_sub(1)
    }

    fn subscribe_after(&self, after: u64) -> EventSubscription {
        let mut inner = self.inner.lock().unwrap();
        let receiver = self.sender.subscribe();
        let pending = replay_records(&mut inner, after);
        EventSubscription { pending, receiver }
    }

    fn replay_after(&self, after: u64) -> VecDeque<EventRecord> {
        replay_records(&mut self.inner.lock().unwrap(), after)
    }
}

fn replay_records(inner: &mut EventHubInner, after: u64) -> VecDeque<EventRecord> {
    if after > inner.next_id.saturating_sub(1) {
        return resync_record(inner);
    }
    let Some(oldest) = inner.records.front().map(|record| record.id) else {
        return VecDeque::new();
    };
    if after < oldest.saturating_sub(1) {
        return resync_record(inner);
    }
    inner
        .records
        .iter()
        .filter(|record| record.id > after)
        .cloned()
        .collect()
}

fn resync_record(inner: &mut EventHubInner) -> VecDeque<EventRecord> {
    let id = inner.next_id;
    inner.next_id = inner.next_id.saturating_add(1);
    VecDeque::from([EventRecord {
        id,
        kind: "resync_required".to_string(),
        data: json!({ "latest_event_id": id }).to_string(),
    }])
}

#[derive(Clone)]
struct QuestionBroker {
    pending: Arc<Mutex<HashMap<String, PendingQuestion>>>,
}

struct PendingQuestion {
    run_id: String,
    request: QuestionRequest,
    responder: oneshot::Sender<QuestionResponse>,
}

#[derive(Debug)]
enum AnswerFailure {
    NotFound,
    Invalid(String),
    Gone,
}

impl QuestionBroker {
    fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn insert(
        &self,
        run_id: &str,
        request: QuestionRequest,
        responder: oneshot::Sender<QuestionResponse>,
    ) -> String {
        let mut pending = self.pending.lock().unwrap();
        loop {
            let question_id = random_id("question", 18);
            if !pending.contains_key(&question_id) {
                pending.insert(
                    question_id.clone(),
                    PendingQuestion {
                        run_id: run_id.to_string(),
                        request,
                        responder,
                    },
                );
                return question_id;
            }
        }
    }

    fn answer<F>(
        &self,
        question_id: &str,
        answers: QuestionAnswers,
        before_resume: F,
    ) -> std::result::Result<(), AnswerFailure>
    where
        F: FnOnce(&str, &QuestionAnswers),
    {
        let mut all_pending = self.pending.lock().unwrap();
        let request = all_pending
            .get(question_id)
            .map(|pending| pending.request.clone())
            .ok_or(AnswerFailure::NotFound)?;
        let answers = normalize_answers(&request, answers).map_err(AnswerFailure::Invalid)?;
        let pending = all_pending
            .remove(question_id)
            .ok_or(AnswerFailure::NotFound)?;
        let run_id = pending.run_id;
        pending
            .responder
            .send(QuestionResponse::Answered(answers.clone()))
            .map_err(|_| AnswerFailure::Gone)?;
        before_resume(&run_id, &answers);
        Ok(())
    }

    fn cancel_run(&self, run_id: &str) {
        let cancelled = {
            let mut pending = self.pending.lock().unwrap();
            let ids = pending
                .iter()
                .filter(|(_, question)| question.run_id == run_id)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            ids.into_iter()
                .filter_map(|id| pending.remove(&id))
                .collect::<Vec<_>>()
        };
        for pending in cancelled {
            let _ = pending.responder.send(QuestionResponse::Cancelled);
        }
    }
}

struct RunEventMapper {
    run_id: String,
    events: EventHub,
    questions: QuestionBroker,
    state_store: StateStore,
    turn_id: Option<String>,
    tool_counter: u64,
    active_tool: Option<ActiveTool>,
}

struct ActiveTool {
    id: String,
    name: String,
    event_name: String,
}

impl RunEventMapper {
    fn new(
        run_id: String,
        events: EventHub,
        questions: QuestionBroker,
        state_store: StateStore,
    ) -> Self {
        Self {
            run_id,
            events,
            questions,
            state_store,
            turn_id: None,
            tool_counter: 0,
            active_tool: None,
        }
    }

    fn publish(&self, kind: &str, data: Value) {
        self.events.publish(kind, data);
    }

    fn next_tool(&mut self, event_name: String) -> ActiveTool {
        self.tool_counter = self.tool_counter.saturating_add(1);
        ActiveTool {
            id: format!("{}_tool_{}", self.run_id, self.tool_counter),
            name: real_tool_name(&event_name).to_string(),
            event_name,
        }
    }

    fn handle(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::TurnStarted { turn_id } => {
                self.turn_id = Some(turn_id.clone());
                self.publish(
                    "turn.started",
                    json!({ "run_id": self.run_id, "turn_id": turn_id }),
                );
            }
            AgentEvent::Chunk(chunk) => match chunk.kind {
                ChatStreamKind::Content => self.publish(
                    "assistant.delta",
                    json!({ "run_id": self.run_id, "delta": chunk.text }),
                ),
                ChatStreamKind::Reasoning => self.publish(
                    "reasoning.delta",
                    json!({ "run_id": self.run_id, "delta": chunk.text }),
                ),
                _ => {}
            },
            AgentEvent::ReasoningStart { .. } => {
                self.publish("reasoning.start", json!({ "run_id": self.run_id }))
            }
            AgentEvent::ReasoningReset { .. } => {
                self.publish("reasoning.reset", json!({ "run_id": self.run_id }))
            }
            AgentEvent::ReasoningPartStart { .. } => {
                self.publish("reasoning.part_start", json!({ "run_id": self.run_id }))
            }
            AgentEvent::ReasoningPartEnd { .. } => {
                self.publish("reasoning.part_end", json!({ "run_id": self.run_id }))
            }
            AgentEvent::ReasoningTitle(title) => self.publish(
                "reasoning.title",
                json!({ "run_id": self.run_id, "title": title }),
            ),
            AgentEvent::ToolCall { name, arguments } => {
                let tool = self.next_tool(name);
                self.publish(
                    "tool.started",
                    json!({
                        "run_id": self.run_id,
                        "tool_id": tool.id,
                        "name": tool.name,
                        "display_name": tools::readable_tool_name(&tool.event_name),
                        "arguments": arguments,
                    }),
                );
                self.active_tool = Some(tool);
            }
            AgentEvent::ToolProgress { name, message } => {
                let (tool_id, tool_name) = self.tool_identity(&name);
                self.publish(
                    "tool.progress",
                    json!({
                        "run_id": self.run_id,
                        "tool_id": tool_id,
                        "name": tool_name,
                        "message": message,
                    }),
                );
            }
            AgentEvent::CommandOutput {
                name,
                stream,
                chunk,
            } => {
                let (tool_id, tool_name) = self.tool_identity(&name);
                let stream = match stream {
                    CommandOutputStream::Stdout => "stdout",
                    CommandOutputStream::Stderr => "stderr",
                };
                self.publish(
                    "tool.output",
                    json!({
                        "run_id": self.run_id,
                        "tool_id": tool_id,
                        "name": tool_name,
                        "stream": stream,
                        "output": String::from_utf8_lossy(&chunk),
                    }),
                );
            }
            AgentEvent::ToolResult { name, ok, output } => {
                let tool = self
                    .active_tool
                    .take()
                    .unwrap_or_else(|| self.next_tool(name));
                self.publish(
                    "tool.finished",
                    json!({
                        "run_id": self.run_id,
                        "tool_id": tool.id,
                        "name": tool.name,
                        "ok": ok,
                        "output": output,
                    }),
                );
            }
            AgentEvent::PrepareForExternalOutput { ready } => {
                let _ = ready.send(false);
            }
            AgentEvent::Image { name, path, alt } => {
                let (tool_id, tool_name) = self.tool_identity(&name);
                let hide_caption = tool_name == "show_meme";
                let Some(turn_id) = self.turn_id.as_deref() else {
                    self.publish(
                        "tool.image",
                        json!({
                            "run_id": self.run_id,
                            "tool_id": tool_id,
                            "name": tool_name,
                            "error": "image could not be associated with the current turn",
                        }),
                    );
                    return;
                };
                match self
                    .state_store
                    .save_image_asset(turn_id, Some(&tool_id), &path, &alt)
                {
                    Ok(asset) => self.publish(
                        "tool.image",
                        json!({
                            "run_id": self.run_id,
                            "tool_id": tool_id,
                            "name": tool_name,
                            "asset": SafeImageAsset::from_asset(asset, hide_caption),
                        }),
                    ),
                    Err(error) => {
                        tracing::warn!(
                            run_id = %self.run_id,
                            tool = %tool_name,
                            error = %error,
                            "failed to persist a WebUI image"
                        );
                        self.publish(
                            "tool.image",
                            json!({
                                "run_id": self.run_id,
                                "tool_id": tool_id,
                                "name": tool_name,
                                "error": "image could not be added to the WebUI",
                            }),
                        );
                    }
                }
            }
            AgentEvent::AskQuestion { request, responder } => {
                let question_id = self
                    .questions
                    .insert(&self.run_id, request.clone(), responder);
                let (tool_id, tool_name) = self.tool_identity("ask_question");
                self.publish(
                    "question.requested",
                    json!({
                        "run_id": self.run_id,
                        "question_id": question_id,
                        "tool_id": tool_id,
                        "name": tool_name,
                        "questions": request.questions,
                    }),
                );
            }
            AgentEvent::QueuedPromptsConsumed {
                prompt_ids,
                mode,
                provider_id,
                model,
            } => self.publish(
                "queue.consumed",
                json!({
                    "run_id": self.run_id,
                    "prompt_ids": prompt_ids,
                    "mode": mode_name(mode),
                    "provider_id": provider_id,
                    "model": model,
                }),
            ),
            AgentEvent::SpinnerTick => {}
            AgentEvent::CompactStart => {
                self.publish("context.compact_start", json!({ "run_id": self.run_id }))
            }
            AgentEvent::CompactChunk(chunk) => self.publish(
                "context.compact_delta",
                json!({ "run_id": self.run_id, "delta": chunk.text }),
            ),
            AgentEvent::CompactEnd => {
                self.publish("context.compact_end", json!({ "run_id": self.run_id }))
            }
            AgentEvent::PopStart => {
                self.publish("context.pop_start", json!({ "run_id": self.run_id }))
            }
            AgentEvent::PopEnd => self.publish("context.pop_end", json!({ "run_id": self.run_id })),
        }
    }

    fn tool_identity(&self, fallback: &str) -> (String, String) {
        self.active_tool
            .as_ref()
            .map(|tool| (tool.id.clone(), tool.name.clone()))
            .unwrap_or_else(|| {
                (
                    format!(
                        "{}_tool_{}",
                        self.run_id,
                        self.tool_counter.saturating_add(1)
                    ),
                    real_tool_name(fallback).to_string(),
                )
            })
    }
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(error = %error, "WebUI request failed");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "error": { "message": self.message } })),
        )
            .into_response()
    }
}

#[derive(Default, Deserialize)]
struct EventsQuery {
    #[serde(default)]
    after: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateTurnRequest {
    content: String,
    mode: String,
    /// Target session; defaults to the global current session. The turn runs
    /// there without moving the current pointer (per-view WebUI sessions).
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueuePromptRequest {
    content: String,
    /// Target session; defaults to the global current session.
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerQuestionRequest {
    answers: QuestionAnswers,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetModelsRequest {
    models: Vec<ActiveProviderModelConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginRequest {
    password: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateConfigRequest {
    config: Value,
    #[serde(default)]
    secrets: HashMap<String, SecretMutation>,
    prompts: PromptDocuments,
    #[serde(default)]
    reset_conversation: bool,
}

#[derive(Deserialize)]
#[serde(tag = "action", content = "value", rename_all = "snake_case")]
enum SecretMutation {
    Set(String),
    Clear,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PromptDocuments {
    #[serde(default)]
    personas: Vec<PromptDocument>,
    #[serde(default)]
    identities: Vec<PromptDocument>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PromptDocument {
    name: String,
    content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    original_name: Option<String>,
}

#[derive(Serialize)]
struct ConfigResponse {
    config: Value,
    secret_states: HashMap<String, bool>,
    prompts: PromptDocuments,
    models: Vec<SafeModel>,
    multimodal_models: Vec<SafeModel>,
    display: WebDisplayConfig,
    context: ContextSnapshot,
}

#[derive(Serialize)]
struct BootstrapResponse {
    version: &'static str,
    boot_id: String,
    latest_event_id: u64,
    active_run_id: Option<String>,
    running_turn_id: Option<String>,
    external_queue_available: bool,
    turns: Vec<SafeTurn>,
    queued_prompts: Vec<SafeQueuedPrompt>,
    models: Vec<SafeModel>,
    display: WebDisplayConfig,
    context: ContextSnapshot,
    usage: SafeUsageSnapshot,
    capabilities: Capabilities,
    sessions: Vec<Value>,
    current_session_id: String,
    /// Every turn currently running, across all sessions.
    runs: Vec<Value>,
}

#[derive(Serialize)]
struct Capabilities {
    multi_conversation: bool,
    attachments: bool,
    queue: bool,
}

#[derive(Clone, Serialize)]
struct WebDisplayConfig {
    reasoning: String,
    tool_calls: String,
    readable_tool_names: bool,
    command_output_lines: usize,
    mixed_model_endpoint_display: String,
    show_mixed_model_endpoint: bool,
}

#[derive(Clone, Serialize)]
struct SafeQueuedPrompt {
    id: String,
    content: String,
    submitted_at: String,
}

#[derive(Serialize)]
struct SafeModel {
    provider_id: String,
    provider_name: String,
    model: String,
    active: bool,
}

#[derive(Serialize)]
struct SafeTurn {
    id: String,
    seq: i64,
    status: &'static str,
    active_context: bool,
    user_content: String,
    assistant_content: String,
    assistant_reasoning: Option<String>,
    provider_id: Option<String>,
    model: Option<String>,
    user_timestamp: String,
    assistant_timestamp: Option<String>,
    token_total: u64,
    token_usage_estimated: bool,
    question_exchanges: Vec<crate::question::QuestionExchange>,
    followups: Vec<SafeFollowup>,
    assets: Vec<SafeImageAsset>,
}

#[derive(Serialize)]
struct SafeFollowup {
    id: String,
    content: String,
    submitted_at: String,
    preceding_assistant_content: Option<String>,
    preceding_assistant_reasoning: Option<String>,
    provider_id: Option<String>,
    model: Option<String>,
}

#[derive(Serialize)]
struct SafeImageAsset {
    id: String,
    url: String,
    mime: String,
    width: u32,
    height: u32,
    alt: String,
    hide_caption: bool,
}

#[derive(Serialize)]
struct SafeUsageSnapshot {
    requests: u64,
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
    conversation_tokens: u64,
    last_usage: Option<Usage>,
    last_conversation_usage: Option<Usage>,
}

#[derive(Serialize)]
struct ModelResponse {
    models: Vec<SafeModel>,
    display: WebDisplayConfig,
    context: ContextSnapshot,
}

pub async fn run(paths: MiyuPaths, args: WebArgs) -> Result<()> {
    let password = resolve_web_password(&args)?;
    AppConfig::init_files(&paths)?;
    let config = AppConfig::load_or_default(&paths)?;
    crate::models_cache::try_load_active(&paths, &config);
    crate::models_cache::spawn_background_refresh_active(paths.clone(), config.clone());
    let state_store = StateStore::new(&paths)?;
    state_store.init_files()?;
    state_store.adopt_sessions_for_persona(&config.active_persona_scope())?;
    // Subagent audit sessions are kept for a week, cleaned at startup and
    // then daily while the daemon runs.
    const SUBAGENT_AUDIT_RETENTION_DAYS: i64 = 7;
    let _ = state_store.delete_subagent_sessions_older_than(SUBAGENT_AUDIT_RETENTION_DAYS);
    {
        let store = state_store.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
            interval.tick().await;
            loop {
                interval.tick().await;
                let _ = store.delete_subagent_sessions_older_than(SUBAGENT_AUDIT_RETENTION_DAYS);
            }
        });
    }
    let client = OpenAiCompatibleClient::from_config(&config, &paths)?;
    let registry = build_tool_registry(&config, &paths, AgentMode::Normal, true)?;
    let mut agent = Agent::new(
        config.clone(),
        &paths,
        state_store.clone(),
        client,
        registry,
        AgentMode::Normal,
    )?;
    agent.prepare_for_turn()?;
    let context = ContextSnapshot {
        tokens: agent.effective_context_tokens()?,
        window: agent.context_window(),
        cumulative_tokens: agent.conversation_usage_tokens()?,
    };

    // Listen on all interfaces so the WebUI is reachable from the LAN;
    // access URLs for every local address are printed below.
    let bind_ip = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    let listener = tokio::net::TcpListener::bind(SocketAddr::new(bind_ip, args.port))
        .await
        .with_context(|| format!("binding Miyu WebUI to 0.0.0.0:{}", args.port))?;
    let port = listener.local_addr()?.port();
    let boot_id: Arc<str> = random_id("boot", 18).into();
    let events = EventHub::new();
    let questions = QuestionBroker::new();
    let manager = Arc::new(Mutex::new(ManagerState {
        config: config.clone(),
        active_runs: HashMap::new(),
        admin_busy: false,
        context,
    }));
    let (actor_tx, actor_join) = spawn_actor(
        agent,
        config,
        paths.clone(),
        state_store.clone(),
        manager.clone(),
        events.clone(),
        questions.clone(),
    )?;
    let (shutdown_tx, mut shutdown_rx) = broadcast::channel(1);
    let state = WebState {
        auth: WebAuth::new(password.as_deref()),
        boot_id,
        web_port: port,
        web_public: args.public,
        paths,
        manager,
        state_store,
        events,
        questions,
        actor_tx: actor_tx.clone(),
        shutdown_tx,
    };
    let (ipc_lease, ipc_task) = start_ipc_server(&state)?;
    let app = router(state);
    let urls = ipc::web_access_urls(port);
    for url in &urls {
        println!("Miyu WebUI: {url}");
    }
    std::io::stdout().flush().ok();

    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .into_future();
    tokio::pin!(server);
    let serve_result = tokio::select! {
        result = &mut server => result,
        _ = shutdown_signal() => Ok(()),
        _ = shutdown_rx.recv() => Ok(()),
    };
    let _ = actor_tx.send(ActorCommand::Shutdown);
    ipc_task.abort();
    let _ = ipc_task.await;
    drop(ipc_lease);
    let actor_result = tokio::task::spawn_blocking(move || actor_join.join())
        .await
        .context("joining WebUI actor task")?
        .map_err(|_| anyhow::anyhow!("WebUI actor thread panicked"))?;
    serve_result.context("serving Miyu WebUI")?;
    actor_result
}

struct IpcRunGuard {
    manager: Arc<Mutex<ManagerState>>,
    run_id: String,
    finished: bool,
}

impl IpcRunGuard {
    fn finish(&mut self) {
        self.finished = true;
    }
}

impl Drop for IpcRunGuard {
    fn drop(&mut self) {
        if !self.finished {
            // Client disconnected mid-turn: cancel its run.
            if let Some(info) = self.manager.lock().unwrap().active_runs.get(&self.run_id) {
                info.request_cancel();
            }
        }
    }
}

fn start_ipc_server(state: &WebState) -> Result<(crate::ipc::WebCoreLease, TokioJoinHandle<()>)> {
    let lease = ipc::acquire_web_core(&state.paths)
        .context("another Miyu core is already running or starting")?;
    let socket_path = state.paths.ipc_socket();
    let listener = tokio::net::UnixListener::bind(&socket_path)
        .with_context(|| format!("binding Miyu IPC socket at {}", socket_path.display()))?;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;

    let server_state = state.clone();
    let permits = Arc::new(Semaphore::new(32));
    let task = tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(connection) => connection,
                Err(error) => {
                    tracing::warn!(error = %error, "Miyu IPC listener stopped");
                    break;
                }
            };
            let permit = match permits.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => break,
            };
            let connection_state = server_state.clone();
            tokio::spawn(async move {
                let _permit = permit;
                if let Err(error) = handle_ipc_connection(connection_state, stream).await {
                    tracing::debug!(error = %error, "Miyu IPC connection closed with an error");
                }
            });
        }
    });
    Ok((lease, task))
}

async fn handle_ipc_connection(state: WebState, mut stream: tokio::net::UnixStream) -> Result<()> {
    let Some(request) = tokio::time::timeout(
        Duration::from_secs(5),
        ipc::receive::<IpcRequest>(&mut stream),
    )
    .await
    .context("timed out waiting for a Miyu IPC request")??
    else {
        return Ok(());
    };
    if request.version != ipc::PROTOCOL_VERSION {
        ipc::send(
            &mut stream,
            &IpcFrame::Error {
                message: format!(
                    "unsupported IPC protocol version {}; expected {}",
                    request.version,
                    ipc::PROTOCOL_VERSION
                ),
            },
        )
        .await?;
        return Ok(());
    }

    match request.command {
        IpcCommand::Ping => {
            ipc::send(
                &mut stream,
                &IpcFrame::Ready {
                    pid: std::process::id(),
                    web_port: state.web_port,
                    web_public: state.web_public,
                    build_id: ipc::BUILD_ID.to_string(),
                },
            )
            .await?;
        }
        IpcCommand::Shutdown => {
            ipc::send(&mut stream, &IpcFrame::Ack).await?;
            let _ = state.shutdown_tx.send(());
        }
        IpcCommand::GetStatus => {
            ipc::send(
                &mut stream,
                &IpcFrame::AdminResult {
                    state: session_state(&state.manager, &state.state_store)?,
                    data: json!({}),
                },
            )
            .await?;
        }
        IpcCommand::ReloadConfig => {
            let next_config = AppConfig::load_or_default(&state.paths)?;
            let prompts = read_prompt_documents(&next_config, &state.paths)?;
            let reset_conversation = {
                let current = state.manager.lock().unwrap().config.clone();
                prompt_configuration_changed(&current, &next_config)
            };
            reserve_admin(&state.manager).map_err(|error| anyhow::anyhow!(error.message))?;
            let (reply, receiver) = oneshot::channel();
            if state
                .actor_tx
                .send(ActorCommand::ApplyConfig {
                    config: next_config,
                    prompts,
                    reset_conversation,
                    reply,
                })
                .is_err()
            {
                release_admin(&state.manager);
                anyhow::bail!("Miyu core worker is unavailable");
            }
            match receiver.await {
                Ok(Ok(())) => {
                    ipc::send(
                        &mut stream,
                        &IpcFrame::AdminResult {
                            state: session_state(&state.manager, &state.state_store)?,
                            data: json!({}),
                        },
                    )
                    .await?
                }
                Ok(Err(AdminFailure::Invalid(message) | AdminFailure::Internal(message))) => {
                    ipc::send(&mut stream, &IpcFrame::Error { message }).await?
                }
                Err(_) => {
                    release_admin(&state.manager);
                    anyhow::bail!("Miyu core stopped while reloading configuration");
                }
            }
        }
        IpcCommand::ResetConversation { all } => {
            reserve_admin_for_session(&state.manager, &state.state_store.session_id()).map_err(|error| anyhow::anyhow!(error.message))?;
            let (reply, receiver) = oneshot::channel();
            if state
                .actor_tx
                .send(ActorCommand::ResetConversation { all, reply })
                .is_err()
            {
                release_admin(&state.manager);
                anyhow::bail!("Miyu core worker is unavailable");
            }
            match receiver.await {
                Ok(Ok(())) => {
                    ipc::send(
                        &mut stream,
                        &IpcFrame::AdminResult {
                            state: session_state(&state.manager, &state.state_store)?,
                            data: json!({}),
                        },
                    )
                    .await?
                }
                Ok(Err(AdminFailure::Invalid(message) | AdminFailure::Internal(message))) => {
                    ipc::send(&mut stream, &IpcFrame::Error { message }).await?
                }
                Err(_) => {
                    release_admin(&state.manager);
                    anyhow::bail!("Miyu core stopped while resetting the conversation");
                }
            }
        }
        IpcCommand::Undo => {
            reserve_admin_for_session(&state.manager, &state.state_store.session_id()).map_err(|error| anyhow::anyhow!(error.message))?;
            let (reply, receiver) = oneshot::channel();
            if state.actor_tx.send(ActorCommand::Undo { reply }).is_err() {
                release_admin(&state.manager);
                anyhow::bail!("Miyu core worker is unavailable");
            }
            match receiver.await {
                Ok(Ok(data)) => {
                    ipc::send(
                        &mut stream,
                        &IpcFrame::AdminResult {
                            state: session_state(&state.manager, &state.state_store)?,
                            data,
                        },
                    )
                    .await?
                }
                Ok(Err(AdminFailure::Invalid(message) | AdminFailure::Internal(message))) => {
                    ipc::send(&mut stream, &IpcFrame::Error { message }).await?
                }
                Err(_) => {
                    release_admin(&state.manager);
                    anyhow::bail!("Miyu core stopped while undoing the conversation");
                }
            }
        }
        IpcCommand::Pop { turn_ids } => {
            reserve_admin_for_session(&state.manager, &state.state_store.session_id()).map_err(|error| anyhow::anyhow!(error.message))?;
            let (reply, receiver) = oneshot::channel();
            if state
                .actor_tx
                .send(ActorCommand::Pop { turn_ids, reply })
                .is_err()
            {
                release_admin(&state.manager);
                anyhow::bail!("Miyu core worker is unavailable");
            }
            match receiver.await {
                Ok(Ok(data)) => {
                    ipc::send(
                        &mut stream,
                        &IpcFrame::AdminResult {
                            state: session_state(&state.manager, &state.state_store)?,
                            data,
                        },
                    )
                    .await?
                }
                Ok(Err(AdminFailure::Invalid(message) | AdminFailure::Internal(message))) => {
                    ipc::send(&mut stream, &IpcFrame::Error { message }).await?
                }
                Err(_) => {
                    release_admin(&state.manager);
                    anyhow::bail!("Miyu core stopped while popping the conversation");
                }
            }
        }
        IpcCommand::Compact => {
            reserve_admin_for_session(&state.manager, &state.state_store.session_id()).map_err(|error| anyhow::anyhow!(error.message))?;
            let (reply, receiver) = oneshot::channel();
            if state
                .actor_tx
                .send(ActorCommand::Compact { reply })
                .is_err()
            {
                release_admin(&state.manager);
                anyhow::bail!("Miyu core worker is unavailable");
            }
            match receiver.await {
                Ok(Ok(data)) => {
                    ipc::send(
                        &mut stream,
                        &IpcFrame::AdminResult {
                            state: session_state(&state.manager, &state.state_store)?,
                            data,
                        },
                    )
                    .await?
                }
                Ok(Err(AdminFailure::Invalid(message) | AdminFailure::Internal(message))) => {
                    ipc::send(&mut stream, &IpcFrame::Error { message }).await?
                }
                Err(_) => {
                    release_admin(&state.manager);
                    anyhow::bail!("Miyu core stopped while compacting the conversation");
                }
            }
        }
        IpcCommand::StartTurn {
            content,
            mode,
            images,
            cwd,
            session_id,
        } => {
            handle_ipc_turn(&state, &mut stream, content, mode, images, cwd, session_id).await?;
        }
        IpcCommand::Cancel { run_id } => {
            let cancelled = {
                let manager = state.manager.lock().unwrap();
                manager.active_runs.get(&run_id).map(RunInfo::request_cancel)
            };
            if cancelled.is_some() {
                ipc::send(&mut stream, &IpcFrame::Ack).await?;
            } else {
                ipc::send(
                    &mut stream,
                    &IpcFrame::Error {
                        message: "active run not found".to_string(),
                    },
                )
                .await?;
            }
        }
        IpcCommand::AnswerQuestion {
            question_id,
            answers,
        } => match state
            .questions
            .answer(&question_id, answers, |run_id, answers| {
                state.events.publish(
                    "question.answered",
                    json!({
                        "run_id": run_id,
                        "question_id": question_id,
                        "answers": answers,
                    }),
                );
            }) {
            Ok(()) => ipc::send(&mut stream, &IpcFrame::Ack).await?,
            Err(error) => {
                ipc::send(
                    &mut stream,
                    &IpcFrame::Error {
                        message: match error {
                            AnswerFailure::NotFound => "pending question not found".to_string(),
                            AnswerFailure::Invalid(message) => message,
                            AnswerFailure::Gone => {
                                "pending question is no longer active".to_string()
                            }
                        },
                    },
                )
                .await?;
            }
        },
        session_command => {
            match handle_session_command(&state, session_command).await {
                Ok(data) => {
                    ipc::send(
                        &mut stream,
                        &IpcFrame::AdminResult {
                            state: session_state(&state.manager, &state.state_store)?,
                            data,
                        },
                    )
                    .await?
                }
                Err(message) => ipc::send(&mut stream, &IpcFrame::Error { message }).await?,
            }
        }
    }
    Ok(())
}

/// Handles the session-management IPC commands. Returns the `AdminResult`
/// payload on success or a user-facing error message.
async fn handle_session_command(
    state: &WebState,
    command: IpcCommand,
) -> std::result::Result<Value, String> {
    let store = &state.state_store;
    let persona = active_persona_scope(state);
    match command {
        IpcCommand::ListSessions { include_archived } => {
            let current = store.session_id();
            let sessions = store
                .list_sessions(&persona, include_archived)
                .map_err(|error| safe_error_message(&error))?;
            let sessions: Vec<Value> = sessions
                .iter()
                .map(|overview| session_overview_json(overview, &current))
                .collect();
            Ok(json!({ "current": &*current, "sessions": sessions }))
        }
        IpcCommand::CreateSession { name, switch } => {
            // No explicit name: leave it empty; the session is auto-named
            // from the first prompt when its first turn completes.
            let name = name
                .map(|name| name.trim().to_string())
                .unwrap_or_default();
            let record = store
                .create_session(&persona, &name, "user", None)
                .map_err(|error| safe_error_message(&error))?;
            state.events.publish(
                "session.created",
                json!({ "session_id": record.session_id, "name": record.name }),
            );
            if switch {
                switch_session_via_actor(state, record.session_id.clone()).await?;
            }
            Ok(json!({ "session": session_record_json(&record) }))
        }
        IpcCommand::SwitchSession { target } => {
            let record = resolve_session_ref(state, &target)?;
            if record.kind != "user" {
                return Err(t(
                    "only user sessions can be switched to",
                    "只能切换到用户会话",
                )
                .to_string());
            }
            if record.archived {
                store
                    .set_session_archived(&record.session_id, false)
                    .map_err(|error| safe_error_message(&error))?;
            }
            switch_session_via_actor(state, record.session_id.clone()).await?;
            Ok(json!({ "session": session_record_json(&record) }))
        }
        IpcCommand::RenameSession { target, name } => {
            let record = resolve_session_ref(state, &target)?;
            let name = name.trim();
            if name.is_empty() {
                return Err(t("session name cannot be empty", "会话名称不能为空").to_string());
            }
            store
                .rename_session(&record.session_id, name)
                .map_err(|error| safe_error_message(&error))?;
            state.events.publish(
                "session.renamed",
                json!({ "session_id": record.session_id, "name": name }),
            );
            Ok(json!({}))
        }
        IpcCommand::ArchiveSession { target, archived } => {
            let record = resolve_session_ref(state, &target)?;
            if archived
                && state
                    .manager
                    .lock()
                    .unwrap()
                    .session_has_runs(&record.session_id)
            {
                return Err(t(
                    "the session has a reply in progress",
                    "该会话有回复正在进行",
                )
                .to_string());
            }
            if archived && &*store.session_id() == record.session_id.as_str() {
                let fallback = fallback_session_id(state, &record.session_id)?;
                switch_session_via_actor(state, fallback).await?;
            }
            store
                .set_session_archived(&record.session_id, archived)
                .map_err(|error| safe_error_message(&error))?;
            state.events.publish(
                "session.archived",
                json!({ "session_id": record.session_id, "archived": archived }),
            );
            Ok(json!({}))
        }
        IpcCommand::DeleteSession { target } => {
            let record = resolve_session_ref(state, &target)?;
            if state
                .manager
                .lock()
                .unwrap()
                .session_has_runs(&record.session_id)
            {
                return Err(t(
                    "the session has a reply in progress",
                    "该会话有回复正在进行",
                )
                .to_string());
            }
            if &*store.session_id() == record.session_id.as_str() {
                let fallback = fallback_session_id(state, &record.session_id)?;
                switch_session_via_actor(state, fallback).await?;
            }
            store
                .delete_session(&record.session_id)
                .map_err(|error| safe_error_message(&error))?;
            state
                .events
                .publish("session.deleted", json!({ "session_id": record.session_id }));
            Ok(json!({}))
        }
        IpcCommand::SetWorkspace { target, path } => {
            let record = resolve_session_ref(state, &target)?;
            let workspace = match path {
                Some(path) => {
                    if !path.is_dir() {
                        return Err(format!(
                            "{}: {}",
                            t("workspace is not a directory", "workspace 不是目录"),
                            path.display()
                        ));
                    }
                    Some(path.to_string_lossy().into_owned())
                }
                None => None,
            };
            store
                .set_session_workspace(&record.session_id, workspace.as_deref())
                .map_err(|error| safe_error_message(&error))?;
            state.events.publish(
                "session.updated",
                json!({ "session_id": record.session_id, "workspace": workspace }),
            );
            Ok(json!({}))
        }
        _ => Err("unsupported session command".to_string()),
    }
}

fn active_persona_scope(state: &WebState) -> String {
    state.manager.lock().unwrap().config.active_persona_scope()
}

fn session_api_error(message: String) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, message)
}

#[derive(Deserialize)]
struct SessionsQuery {
    #[serde(default)]
    include_archived: bool,
}

async fn list_sessions_http(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<SessionsQuery>,
) -> std::result::Result<Response, ApiError> {
    require_auth(&headers, &state)?;
    let data = handle_session_command(
        &state,
        IpcCommand::ListSessions {
            include_archived: query.include_archived,
        },
    )
    .await
    .map_err(session_api_error)?;
    Ok(Json(data).into_response())
}

#[derive(Deserialize)]
struct CreateSessionRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    switch: bool,
}

async fn create_session_http(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(request): Json<CreateSessionRequest>,
) -> std::result::Result<Response, ApiError> {
    require_mutation(&headers, &state)?;
    let data = handle_session_command(
        &state,
        IpcCommand::CreateSession {
            name: request.name,
            switch: request.switch,
        },
    )
    .await
    .map_err(session_api_error)?;
    Ok((StatusCode::CREATED, Json(data)).into_response())
}

async fn activate_session_http(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> std::result::Result<Response, ApiError> {
    require_mutation(&headers, &state)?;
    let data = handle_session_command(
        &state,
        IpcCommand::SwitchSession {
            target: ipc::SessionRef::Id { id: session_id },
        },
    )
    .await
    .map_err(session_api_error)?;
    Ok(Json(data).into_response())
}

#[derive(Deserialize)]
struct UpdateSessionRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    archived: Option<bool>,
    /// `Some("")` unbinds the workspace; a non-empty value binds it.
    #[serde(default)]
    workspace: Option<String>,
}

async fn update_session_http(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(request): Json<UpdateSessionRequest>,
) -> std::result::Result<Response, ApiError> {
    require_mutation(&headers, &state)?;
    let target = || ipc::SessionRef::Id {
        id: session_id.clone(),
    };
    if let Some(name) = request.name {
        handle_session_command(
            &state,
            IpcCommand::RenameSession {
                target: target(),
                name,
            },
        )
        .await
        .map_err(session_api_error)?;
    }
    if let Some(archived) = request.archived {
        handle_session_command(
            &state,
            IpcCommand::ArchiveSession {
                target: target(),
                archived,
            },
        )
        .await
        .map_err(session_api_error)?;
    }
    if let Some(workspace) = request.workspace {
        let path = (!workspace.trim().is_empty()).then(|| std::path::PathBuf::from(workspace));
        handle_session_command(
            &state,
            IpcCommand::SetWorkspace {
                target: target(),
                path,
            },
        )
        .await
        .map_err(session_api_error)?;
    }
    Ok(Json(json!({})).into_response())
}

/// Read-only snapshot of one session's conversation for per-view browsing:
/// turns, queued follow-ups, and its currently running turns. Does not touch
/// the global current-session pointer.
async fn session_turns_http(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> std::result::Result<Response, ApiError> {
    require_auth(&headers, &state)?;
    if state
        .state_store
        .session_record(&session_id)
        .map_err(ApiError::internal)?
        .is_none()
    {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "session not found"));
    }
    let store = state.state_store.pinned(&session_id);
    let mut assets_by_turn = HashMap::<String, Vec<ImageAsset>>::new();
    for asset in store.load_image_assets().map_err(ApiError::internal)? {
        assets_by_turn
            .entry(asset.turn_id.clone())
            .or_default()
            .push(asset);
    }
    let turns: Vec<SafeTurn> = store
        .load_turns()
        .map_err(ApiError::internal)?
        .into_iter()
        .filter(|turn| !turn.is_summary)
        .map(|turn| {
            let assets = assets_by_turn.remove(&turn.turn_id).unwrap_or_default();
            SafeTurn::from_turn(turn, assets)
        })
        .collect();
    let running_target = store
        .running_turn_queue_target()
        .map_err(ApiError::internal)?;
    let queued_prompts: Vec<SafeQueuedPrompt> = match running_target.as_ref() {
        Some(target) => store
            .load_queued_prompts_for_target(target)
            .map_err(ApiError::internal)?,
        None => Vec::new(),
    }
    .into_iter()
    .map(SafeQueuedPrompt::from)
    .collect();
    let runs: Vec<Value> = state
        .manager
        .lock()
        .unwrap()
        .active_runs
        .iter()
        .filter(|(_, info)| &*info.session_id == session_id.as_str())
        .map(|(run_id, info)| {
            json!({
                "run_id": run_id,
                "session_id": &*info.session_id,
                "mode": mode_name(info.mode),
            })
        })
        .collect();
    let mut response = Json(json!({
        "session_id": session_id,
        "turns": turns,
        "queued_prompts": queued_prompts,
        "running_turn_id": running_target.as_ref().map(|target| target.turn_id.as_str()),
        "runs": runs,
    }))
    .into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn delete_session_http(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> std::result::Result<Response, ApiError> {
    require_mutation(&headers, &state)?;
    let data = handle_session_command(
        &state,
        IpcCommand::DeleteSession {
            target: ipc::SessionRef::Id { id: session_id },
        },
    )
    .await
    .map_err(session_api_error)?;
    Ok(Json(data).into_response())
}

fn resolve_session_ref(
    state: &WebState,
    target: &ipc::SessionRef,
) -> std::result::Result<crate::state::SessionRecord, String> {
    let store = &state.state_store;
    let record = match target {
        ipc::SessionRef::Current => store
            .session_record(&store.session_id())
            .map_err(|error| safe_error_message(&error))?,
        ipc::SessionRef::Id { id } => store
            .session_record(id)
            .map_err(|error| safe_error_message(&error))?,
        ipc::SessionRef::Name { name } => store
            .find_session_by_name(&active_persona_scope(state), name)
            .map_err(|error| safe_error_message(&error))?,
    };
    record.ok_or_else(|| t("session not found", "找不到该会话").to_string())
}

/// Most recently updated other unarchived user session, or a fresh default
/// session when none is left.
fn fallback_session_id(
    state: &WebState,
    exclude: &str,
) -> std::result::Result<String, String> {
    let persona = active_persona_scope(state);
    let sessions = state
        .state_store
        .list_sessions(&persona, false)
        .map_err(|error| safe_error_message(&error))?;
    if let Some(overview) = sessions
        .iter()
        .find(|overview| overview.record.session_id != exclude)
    {
        return Ok(overview.record.session_id.clone());
    }
    let record = state
        .state_store
        .create_session(&persona, t("Default session", "默认会话"), "user", None)
        .map_err(|error| safe_error_message(&error))?;
    state.events.publish(
        "session.created",
        json!({ "session_id": record.session_id, "name": record.name }),
    );
    Ok(record.session_id)
}

async fn switch_session_via_actor(
    state: &WebState,
    session_id: String,
) -> std::result::Result<(), String> {
    reserve_admin_light(&state.manager).map_err(|error| error.message)?;
    let (reply, receiver) = oneshot::channel();
    if state
        .actor_tx
        .send(ActorCommand::SwitchSession { session_id, reply })
        .is_err()
    {
        release_admin(&state.manager);
        return Err("Miyu core worker is unavailable".to_string());
    }
    match receiver.await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(AdminFailure::Invalid(message) | AdminFailure::Internal(message))) => Err(message),
        Err(_) => {
            release_admin(&state.manager);
            Err("Miyu core stopped while switching sessions".to_string())
        }
    }
}

fn session_record_json(record: &crate::state::SessionRecord) -> Value {
    json!({
        "session_id": record.session_id,
        "name": record.name,
        "kind": record.kind,
        "workspace": record.workspace,
        "archived": record.archived,
        "created_at": record.created_at,
        "updated_at": record.updated_at,
    })
}

fn session_overview_json(overview: &crate::state::SessionOverview, current: &str) -> Value {
    let mut value = session_record_json(&overview.record);
    value["turn_count"] = json!(overview.turn_count);
    value["last_user_content"] = json!(overview.last_user_content);
    value["is_current"] = json!(overview.record.session_id == current);
    value
}

/// Resolves an optional turn-target session id: validates existence and that
/// it is a user session; `None` falls back to the global current session.
fn resolve_turn_session(
    state: &WebState,
    session_id: Option<String>,
) -> std::result::Result<Arc<str>, String> {
    match session_id {
        None => Ok(state.state_store.session_id()),
        Some(session_id) => {
            let record = state
                .state_store
                .session_record(&session_id)
                .map_err(|error| safe_error_message(&error))?
                .ok_or_else(|| t("session not found", "找不到该会话").to_string())?;
            if record.kind != "user" {
                return Err(t(
                    "turns can only run in user sessions",
                    "只能在用户会话中发起对话",
                )
                .to_string());
            }
            Ok(record.session_id.into())
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_ipc_turn(
    state: &WebState,
    stream: &mut tokio::net::UnixStream,
    content: String,
    mode: String,
    images: Vec<Option<ImageAttachment>>,
    cwd: Option<std::path::PathBuf>,
    session_id: Option<String>,
) -> Result<()> {
    let content = match validate_content(content) {
        Ok(content) => content,
        Err(error) => {
            ipc::send(
                stream,
                &IpcFrame::Error {
                    message: error.message,
                },
            )
            .await?;
            return Ok(());
        }
    };
    let mode = match parse_mode(&mode) {
        Ok(mode) => mode,
        Err(error) => {
            ipc::send(
                stream,
                &IpcFrame::Error {
                    message: error.message,
                },
            )
            .await?;
            return Ok(());
        }
    };
    // Turns run in parallel — several may be active at once, including in
    // the same session (placeholder semantics). The only rejection is a
    // transient admin mutation window.
    let run_id = random_id("run", 18);
    let session_id = match resolve_turn_session(state, session_id) {
        Ok(session_id) => session_id,
        Err(message) => {
            ipc::send(stream, &IpcFrame::Error { message }).await?;
            return Ok(());
        }
    };
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let busy = {
        let mut manager = state.manager.lock().unwrap();
        if manager.admin_busy {
            true
        } else {
            manager.active_runs.insert(
                run_id.clone(),
                RunInfo {
                    session_id: session_id.clone(),
                    mode,
                    cancel: cancel_tx,
                },
            );
            false
        }
    };
    if busy {
        ipc::send(
            stream,
            &IpcFrame::Error {
                message: "Miyu is busy with another operation".to_string(),
            },
        )
        .await?;
        return Ok(());
    }

    let after = state.events.latest_id();
    let mut subscription = state.events.subscribe_after(after);
    if state
        .actor_tx
        .send(ActorCommand::StartTurn {
            run_id: run_id.clone(),
            session_id,
            content,
            mode,
            images,
            cwd,
            cancel: cancel_rx,
        })
        .is_err()
    {
        finish_run(&state.manager, &run_id, None);
        ipc::send(
            stream,
            &IpcFrame::Error {
                message: "Miyu core worker is unavailable".to_string(),
            },
        )
        .await?;
        return Ok(());
    }
    let mut run_guard = IpcRunGuard {
        manager: state.manager.clone(),
        run_id: run_id.clone(),
        finished: false,
    };
    ipc::send(
        stream,
        &IpcFrame::Accepted {
            run_id: run_id.clone(),
        },
    )
    .await?;

    let mut last_id = after;
    loop {
        let record = if let Some(record) = subscription.pending.pop_front() {
            record
        } else {
            match subscription.receiver.recv().await {
                Ok(record) => record,
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    subscription.pending = state.events.replay_after(last_id);
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        };
        if record.kind == "resync_required" {
            ipc::send(
                stream,
                &IpcFrame::Error {
                    message: "Miyu core event history was exhausted; the turn was cancelled"
                        .to_string(),
                },
            )
            .await?;
            break;
        }
        last_id = record.id;
        let Ok(data) = serde_json::from_str::<Value>(&record.data) else {
            continue;
        };
        if data.get("run_id").and_then(Value::as_str) != Some(run_id.as_str()) {
            continue;
        }
        let terminal = matches!(
            record.kind.as_str(),
            "run.completed" | "run.failed" | "run.cancelled"
        );
        ipc::send(
            stream,
            &IpcFrame::Event {
                id: record.id,
                kind: record.kind,
                data,
            },
        )
        .await?;
        if terminal {
            run_guard.finish();
            break;
        }
    }
    Ok(())
}

fn router(state: WebState) -> Router {
    Router::new()
        .route("/", get(index_asset))
        .route("/styles.css", get(styles_asset))
        .route("/theme.css", get(theme_css))
        .route("/app.js", get(app_asset))
        .route("/assets/miyu-logo.png", get(logo_asset))
        .route("/assets/miyuwallpaper.png", get(wallpaper_asset))
        .route("/api/health", get(health))
        .route("/api/auth/login", post(auth_login))
        .route("/api/bootstrap", get(bootstrap))
        .route("/api/config", get(get_config).put(update_config))
        .route("/api/events", get(events))
        .route("/api/assets/{asset_id}", get(image_asset))
        .route(
            "/api/sessions",
            get(list_sessions_http).post(create_session_http),
        )
        .route(
            "/api/sessions/{session_id}",
            patch(update_session_http).delete(delete_session_http),
        )
        .route(
            "/api/sessions/{session_id}/activate",
            post(activate_session_http),
        )
        .route("/api/sessions/{session_id}/turns", get(session_turns_http))
        .route("/api/turns", post(create_turn))
        .route("/api/queue", post(queue_prompt))
        .route("/api/queue/{prompt_id}", delete(remove_queue_prompt))
        .route("/api/runs/{run_id}/cancel", post(cancel_run))
        .route("/api/questions/{question_id}/answer", post(answer_question))
        .route("/api/models/active", put(set_models))
        .route("/api/conversation/reset", post(reset_conversation))
        .layer(DefaultBodyLimit::max(JSON_BODY_LIMIT))
        .with_state(state)
}

async fn index_asset() -> Response {
    text_asset(INDEX_HTML, "text/html; charset=utf-8")
}

async fn styles_asset() -> Response {
    text_asset(STYLES_CSS, "text/css; charset=utf-8")
}

/// Optional MD3 token override generated by matugen from the wallpaper.
/// Read from disk on every request (the file is tiny and regenerated at any
/// time); 404 when absent so the WebUI falls back to the built-in palette.
async fn theme_css(State(state): State<WebState>) -> Response {
    let path = state.paths.config_dir.join("webui-theme.css");
    match tokio::fs::read(&path).await {
        Ok(bytes) => finish_asset_response(bytes.into_response(), "text/css; charset=utf-8"),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn app_asset() -> Response {
    text_asset(APP_JS, "application/javascript; charset=utf-8")
}

async fn logo_asset() -> Response {
    binary_asset(MIYU_LOGO, "image/png")
}

async fn wallpaper_asset() -> Response {
    binary_asset(MIYU_WALLPAPER, "image/png")
}

fn text_asset(content: &'static str, content_type: &'static str) -> Response {
    asset_response(content.as_bytes(), content_type)
}

fn binary_asset(content: &'static [u8], content_type: &'static str) -> Response {
    asset_response(content, content_type)
}

fn asset_response(content: &'static [u8], content_type: &'static str) -> Response {
    finish_asset_response(content.into_response(), content_type)
}

fn finish_asset_response(mut response: Response, content_type: &'static str) -> Response {
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
        .headers_mut()
        .insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    response.headers_mut().insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self'; style-src 'self'; script-src 'self'; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
        ),
    );
    response
        .headers_mut()
        .insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    response
}

async fn auth_login(
    State(state): State<WebState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> std::result::Result<Response, ApiError> {
    if !origin_is_allowed(&headers) {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "request origin is not allowed",
        ));
    }
    if !state.auth.required() {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    if request.password.chars().count() > 1_024 {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "password is too long",
        ));
    }
    let session = match state.auth.login(peer.ip(), &request.password) {
        Ok(session) => session,
        Err(LoginFailure::Invalid) => {
            return Err(ApiError::new(StatusCode::UNAUTHORIZED, "invalid password"));
        }
        Err(LoginFailure::RateLimited) => {
            let mut response = ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "too many login attempts; try again shortly",
            )
            .into_response();
            response
                .headers_mut()
                .insert(RETRY_AFTER, HeaderValue::from_static("60"));
            return Ok(response);
        }
    };
    let cookie =
        format!("{AUTH_COOKIE}={session}; HttpOnly; SameSite=Strict; Path=/; Max-Age=86400");
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(ApiError::internal)?,
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

fn resolve_web_password(args: &WebArgs) -> Result<Option<String>> {
    let password = if let Some(path) = &args.password_file {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("reading WebUI password file: {}", path.display()))?;
        Some(contents.trim_end_matches(['\r', '\n']).to_string())
    } else {
        match &args.password {
            Some(password) if !password.is_empty() => Some(password.clone()),
            Some(_) if io::stdin().is_terminal() => {
                Some(rpassword::prompt_password("WebUI password: ")?)
            }
            Some(_) => {
                anyhow::bail!("-p requires an interactive terminal or an explicit password value")
            }
            None => None,
        }
    };
    if let Some(password) = &password {
        if password.is_empty() {
            anyhow::bail!("WebUI password cannot be empty");
        }
        if password.chars().count() > 1_024 {
            anyhow::bail!("WebUI password cannot exceed 1,024 characters");
        }
    }
    Ok(password)
}


async fn health() -> Json<Value> {
    Json(json!({
        "status": "ready",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn bootstrap(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> std::result::Result<Response, ApiError> {
    require_auth(&headers, &state)?;
    state
        .state_store
        .recover_stale_turns()
        .map_err(ApiError::internal)?;
    let current_session = state.state_store.session_id();
    let (config, active_run_id, runs, context) = {
        let manager = state.manager.lock().unwrap();
        let runs: Vec<Value> = manager
            .active_runs
            .iter()
            .map(|(run_id, info)| {
                json!({
                    "run_id": run_id,
                    "session_id": &*info.session_id,
                    "mode": mode_name(info.mode),
                })
            })
            .collect();
        (
            manager.config.clone(),
            manager.run_in_session(&current_session).cloned(),
            runs,
            manager.context,
        )
    };
    let running_target = state
        .state_store
        .running_turn_queue_target()
        .map_err(ApiError::internal)?;
    let external_target = active_run_id
        .is_none()
        .then_some(running_target.as_ref())
        .flatten();
    let mut assets_by_turn = HashMap::<String, Vec<ImageAsset>>::new();
    for asset in state
        .state_store
        .load_image_assets()
        .map_err(ApiError::internal)?
    {
        assets_by_turn
            .entry(asset.turn_id.clone())
            .or_default()
            .push(asset);
    }
    let turns = state
        .state_store
        .load_turns()
        .map_err(ApiError::internal)?
        .into_iter()
        .filter(|turn| !turn.is_summary)
        .map(|turn| {
            let assets = assets_by_turn.remove(&turn.turn_id).unwrap_or_default();
            SafeTurn::from_turn(turn, assets)
        })
        .collect();
    let usage = state
        .state_store
        .usage_snapshot()
        .map_err(ApiError::internal)?
        .into();
    let queued_prompts = match external_target {
        Some(target) => state
            .state_store
            .load_queued_prompts_for_target(target)
            .map_err(ApiError::internal)?,
        None => state
            .state_store
            .load_queued_prompts()
            .map_err(ApiError::internal)?,
    }
    .into_iter()
    .map(SafeQueuedPrompt::from)
    .collect();
    let running_turn_id = running_target.as_ref().map(|target| target.turn_id.clone());
    let external_queue_available = external_target
        .is_some_and(|target| target.queue_session_id.is_some() && target.owner_pid.is_some());
    let current_session_id = state.state_store.session_id().to_string();
    let sessions = state
        .state_store
        .list_sessions(&config.active_persona_scope(), false)
        .map_err(ApiError::internal)?
        .iter()
        .map(|overview| session_overview_json(overview, &current_session_id))
        .collect();
    let mut response = Json(BootstrapResponse {
        version: env!("CARGO_PKG_VERSION"),
        boot_id: state.boot_id.to_string(),
        latest_event_id: state.events.latest_id(),
        active_run_id,
        running_turn_id,
        external_queue_available,
        turns,
        queued_prompts,
        models: safe_models(&config),
        display: web_display_config(&config),
        context,
        usage,
        capabilities: Capabilities {
            multi_conversation: true,
            attachments: false,
            queue: true,
        },
        sessions,
        current_session_id,
        runs,
    })
    .into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn get_config(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> std::result::Result<Response, ApiError> {
    require_auth(&headers, &state)?;
    let (config, context) = {
        let manager = state.manager.lock().unwrap();
        (manager.config.clone(), manager.context)
    };
    let mut response = Json(config_response(&config, context, &state.paths)?).into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

async fn update_config(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(request): Json<UpdateConfigRequest>,
) -> std::result::Result<Json<ConfigResponse>, ApiError> {
    require_mutation(&headers, &state)?;
    require_no_running_turn(&state.state_store)?;

    let current = state.manager.lock().unwrap().config.clone();
    let current_prompts =
        read_prompt_documents(&current, &state.paths).map_err(ApiError::internal)?;
    let mut candidate: AppConfig = serde_json::from_value(request.config).map_err(|error| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("invalid configuration: {}", safe_error_message(error)),
        )
    })?;
    restore_config_secrets(&mut candidate, &current, &request.secrets)?;
    validate_config_candidate(&candidate)?;
    validate_prompt_documents(&candidate, &request.prompts)?;
    let prompt_changed = prompt_configuration_changed(&current, &candidate)
        || prompt_documents_changed(&current_prompts, &request.prompts);
    if prompt_changed && !request.reset_conversation {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "prompt changes require explicit confirmation to reset the conversation",
        ));
    }

    reserve_admin(&state.manager)?;
    let (reply, receiver) = oneshot::channel();
    if state
        .actor_tx
        .send(ActorCommand::ApplyConfig {
            config: candidate,
            prompts: request.prompts,
            reset_conversation: prompt_changed,
            reply,
        })
        .is_err()
    {
        release_admin(&state.manager);
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "agent worker is unavailable",
        ));
    }
    match receiver.await {
        Ok(Ok(())) => {}
        Ok(Err(AdminFailure::Invalid(message))) => {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, message));
        }
        Ok(Err(AdminFailure::Internal(message))) => {
            tracing::error!(error = %message, "WebUI configuration update failed");
            return Err(ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                safe_error_message(&message),
            ));
        }
        Err(_) => {
            release_admin(&state.manager);
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "agent worker stopped before updating the configuration",
            ));
        }
    }
    let manager = state.manager.lock().unwrap();
    Ok(Json(config_response(
        &manager.config,
        manager.context,
        &state.paths,
    )?))
}

async fn image_asset(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(asset_id): Path<String>,
) -> std::result::Result<Response, ApiError> {
    require_auth(&headers, &state)?;
    if asset_id.len() > 96
        || asset_id.is_empty()
        || !asset_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "image asset not found",
        ));
    }
    let Some(asset) = state
        .state_store
        .load_image_asset(&asset_id)
        .map_err(ApiError::internal)?
    else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "image asset not found",
        ));
    };
    let mut response = asset.bytes.into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&asset.asset.mime).map_err(ApiError::internal)?,
    );
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400"),
    );
    response
        .headers_mut()
        .insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    Ok(response)
}

async fn events(
    State(state): State<WebState>,
    headers: HeaderMap,
    Query(query): Query<EventsQuery>,
) -> std::result::Result<Sse<impl Stream<Item = std::result::Result<Event, Infallible>>>, ApiError>
{
    require_auth(&headers, &state)?;
    let header_after = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let after = query.after.max(header_after);
    let subscription = state.events.subscribe_after(after);
    let stream_state = SseStreamState {
        pending: subscription.pending,
        receiver: subscription.receiver,
        events: state.events,
        last_id: after,
    };
    let events = stream::unfold(stream_state, |mut state| async move {
        loop {
            if let Some(record) = state.pending.pop_front() {
                if record.kind == "resync_required" {
                    state.last_id = record.id;
                    return Some((Ok(record_to_sse(record)), state));
                }
                if record.id <= state.last_id {
                    continue;
                }
                state.last_id = record.id;
                return Some((Ok(record_to_sse(record)), state));
            }
            match state.receiver.recv().await {
                Ok(record) if record.id > state.last_id => {
                    state.last_id = record.id;
                    return Some((Ok(record_to_sse(record)), state));
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    state.pending = state.events.replay_after(state.last_id);
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    let ready =
        stream::once(async { Ok::<Event, Infallible>(Event::default().comment("connected")) });
    let stream = ready.chain(events);
    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    ))
}

struct SseStreamState {
    pending: VecDeque<EventRecord>,
    receiver: broadcast::Receiver<EventRecord>,
    events: EventHub,
    last_id: u64,
}

fn record_to_sse(record: EventRecord) -> Event {
    Event::default()
        .id(record.id.to_string())
        .event(record.kind)
        .data(record.data)
}

fn enqueue_running_prompt(
    state: &WebState,
    store: &StateStore,
    session_id: &str,
    content: &str,
) -> std::result::Result<(Option<String>, Option<String>, SafeQueuedPrompt), ApiError> {
    let active_run_id = {
        let manager = state.manager.lock().unwrap();
        if manager.admin_busy {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "Miyu is busy with another operation",
            ));
        }
        manager.run_in_session(session_id).cloned()
    };
    let prompt_id = random_id("queued", 18);
    store
        .recover_stale_turns()
        .map_err(ApiError::internal)?;
    let target = store
        .running_turn_queue_target()
        .map_err(ApiError::internal)?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::CONFLICT,
                "there is no active reply to follow up",
            )
        })?;
    if target.queue_session_id.is_none() || target.owner_pid.is_none() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "the running turn cannot accept messages from this WebUI",
        ));
    }
    let prompt = store
        .enqueue_prompt_for_target(&target, &prompt_id, content, content, &[])
        .map_err(ApiError::internal)?;
    // Turns run with per-turn queue identities, so follow-ups always route
    // via the running turn's recorded queue target.
    Ok((
        active_run_id,
        Some(target.turn_id),
        SafeQueuedPrompt::from(prompt),
    ))
}

fn publish_queued_prompt(
    state: &WebState,
    run_id: Option<&str>,
    turn_id: Option<&str>,
    prompt: &SafeQueuedPrompt,
) {
    state.events.publish(
        "queue.added",
        json!({ "run_id": run_id, "turn_id": turn_id, "prompt": prompt }),
    );
}

async fn create_turn(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(request): Json<CreateTurnRequest>,
) -> std::result::Result<Response, ApiError> {
    require_mutation(&headers, &state)?;
    let content = validate_content(request.content)?;
    let mode = parse_mode(&request.mode)?;
    let session_id = resolve_turn_session(&state, request.session_id).map_err(session_api_error)?;
    state
        .state_store
        .recover_stale_turns()
        .map_err(ApiError::internal)?;
    // A running turn in the *target* session gets the message as a queued
    // follow-up (composer tray UX); other sessions run in parallel.
    let target_store = state.state_store.pinned(&session_id);
    if target_store
        .has_running_turns()
        .map_err(ApiError::internal)?
    {
        let (run_id, turn_id, prompt) =
            enqueue_running_prompt(&state, &target_store, &session_id, &content)?;
        publish_queued_prompt(&state, run_id.as_deref(), turn_id.as_deref(), &prompt);
        return Ok((
            StatusCode::ACCEPTED,
            Json(json!({
                "queued": true,
                "prompt": prompt,
                "run_id": run_id,
                "running_turn_id": turn_id,
            })),
        )
            .into_response());
    }
    let run_id = random_id("run", 18);
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    {
        let mut manager = state.manager.lock().unwrap();
        if manager.admin_busy {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "Miyu is busy with another operation",
            ));
        }
        manager.active_runs.insert(
            run_id.clone(),
            RunInfo {
                session_id: session_id.clone(),
                mode,
                cancel: cancel_tx,
            },
        );
    }
    if state
        .actor_tx
        .send(ActorCommand::StartTurn {
            run_id: run_id.clone(),
            session_id,
            content,
            mode,
            images: Vec::new(),
            cwd: None,
            cancel: cancel_rx,
        })
        .is_err()
    {
        finish_run(&state.manager, &run_id, None);
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "agent worker is unavailable",
        ));
    }
    Ok((StatusCode::ACCEPTED, Json(json!({ "run_id": run_id }))).into_response())
}

async fn queue_prompt(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(request): Json<QueuePromptRequest>,
) -> std::result::Result<Response, ApiError> {
    require_mutation(&headers, &state)?;
    let content = validate_content(request.content)?;
    let session_id =
        resolve_turn_session(&state, request.session_id).map_err(session_api_error)?;
    let store = state.state_store.pinned(&session_id);
    let (run_id, turn_id, safe) = enqueue_running_prompt(&state, &store, &session_id, &content)?;
    publish_queued_prompt(&state, run_id.as_deref(), turn_id.as_deref(), &safe);
    Ok((StatusCode::ACCEPTED, Json(safe)).into_response())
}

async fn remove_queue_prompt(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(prompt_id): Path<String>,
) -> std::result::Result<StatusCode, ApiError> {
    require_mutation(&headers, &state)?;
    if prompt_id.len() > 96
        || prompt_id.is_empty()
        || !prompt_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "queued prompt not found",
        ));
    }
    let current_session = state.state_store.session_id();
    let run_id = state
        .manager
        .lock()
        .unwrap()
        .run_in_session(&current_session)
        .cloned();
    let target = state
        .state_store
        .running_turn_queue_target()
        .map_err(ApiError::internal)?;
    let removed = match target.as_ref() {
        Some(target) => state
            .state_store
            .remove_queued_prompt_for_target(target, &prompt_id)
            .map_err(ApiError::internal)?,
        None => state
            .state_store
            .remove_queued_prompt(&prompt_id)
            .map_err(ApiError::internal)?,
    };
    if !removed {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "queued prompt not found",
        ));
    }
    state.events.publish(
        "queue.removed",
        json!({
            "run_id": run_id,
            "turn_id": target.as_ref().map(|target| target.turn_id.as_str()),
            "prompt_id": prompt_id,
        }),
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn cancel_run(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> std::result::Result<Response, ApiError> {
    require_mutation(&headers, &state)?;
    let cancelled = {
        let manager = state.manager.lock().unwrap();
        manager.active_runs.get(&run_id).map(RunInfo::request_cancel)
    };
    if cancelled.is_none() {
        return Err(ApiError::new(StatusCode::NOT_FOUND, "active run not found"));
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "run_id": run_id,
            "cancellation_requested": true,
        })),
    )
        .into_response())
}

async fn answer_question(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path(question_id): Path<String>,
    Json(request): Json<AnswerQuestionRequest>,
) -> std::result::Result<StatusCode, ApiError> {
    require_mutation(&headers, &state)?;
    match state
        .questions
        .answer(&question_id, request.answers, |run_id, answers| {
            state.events.publish(
                "question.answered",
                json!({
                    "run_id": run_id,
                    "question_id": question_id,
                    "answers": answers,
                }),
            );
        }) {
        Ok(()) => {}
        Err(AnswerFailure::NotFound) => {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "pending question not found",
            ));
        }
        Err(AnswerFailure::Invalid(message)) => {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, message));
        }
        Err(AnswerFailure::Gone) => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "the question is no longer awaiting an answer",
            ));
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn set_models(
    State(state): State<WebState>,
    headers: HeaderMap,
    Json(request): Json<SetModelsRequest>,
) -> std::result::Result<Json<ModelResponse>, ApiError> {
    require_mutation(&headers, &state)?;
    let models = validate_model_selection(request.models)?;
    require_no_running_turn(&state.state_store)?;
    reserve_admin(&state.manager)?;
    let (reply, receiver) = oneshot::channel();
    if state
        .actor_tx
        .send(ActorCommand::SetModels { models, reply })
        .is_err()
    {
        release_admin(&state.manager);
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "agent worker is unavailable",
        ));
    }
    match receiver.await {
        Ok(Ok(())) => {}
        Ok(Err(AdminFailure::Invalid(message))) => {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, message));
        }
        Ok(Err(AdminFailure::Internal(message))) => {
            tracing::error!(error = %message, "WebUI model update failed");
            return Err(ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                safe_error_message(&message),
            ));
        }
        Err(_) => {
            release_admin(&state.manager);
            return Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "agent worker stopped before updating the model",
            ));
        }
    }
    let manager = state.manager.lock().unwrap();
    Ok(Json(ModelResponse {
        models: safe_models(&manager.config),
        display: web_display_config(&manager.config),
        context: manager.context,
    }))
}

async fn reset_conversation(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> std::result::Result<StatusCode, ApiError> {
    require_mutation(&headers, &state)?;
    if state
        .state_store
        .has_running_turns()
        .map_err(ApiError::internal)?
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "a conversation turn is already running",
        ));
    }
    reserve_admin_for_session(&state.manager, &state.state_store.session_id())?;
    let (reply, receiver) = oneshot::channel();
    if state
        .actor_tx
        .send(ActorCommand::ResetConversation { all: false, reply })
        .is_err()
    {
        release_admin(&state.manager);
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "agent worker is unavailable",
        ));
    }
    match receiver.await {
        Ok(Ok(())) => Ok(StatusCode::NO_CONTENT),
        Ok(Err(AdminFailure::Invalid(message))) => {
            Err(ApiError::new(StatusCode::CONFLICT, message))
        }
        Ok(Err(AdminFailure::Internal(message))) => {
            tracing::error!(error = %message, "WebUI conversation reset failed");
            Err(ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                safe_error_message(&message),
            ))
        }
        Err(_) => {
            release_admin(&state.manager);
            Err(ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "agent worker stopped before resetting the conversation",
            ))
        }
    }
}

fn spawn_actor(
    agent: Agent,
    config: AppConfig,
    paths: MiyuPaths,
    state_store: StateStore,
    manager: Arc<Mutex<ManagerState>>,
    events: EventHub,
    questions: QuestionBroker,
) -> Result<(mpsc::UnboundedSender<ActorCommand>, JoinHandle<Result<()>>)> {
    let (sender, receiver) = mpsc::unbounded_channel();
    let join = std::thread::Builder::new()
        .name("miyu-web-agent".to_string())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .context("building WebUI agent runtime")?;
            // Turns are spawned as local tasks so several can run
            // concurrently on this thread (they are IO-bound); LocalSet
            // avoids imposing Send on the agent futures.
            let local = tokio::task::LocalSet::new();
            runtime.block_on(local.run_until(actor_loop(
                agent,
                config,
                paths,
                state_store,
                manager,
                events,
                questions,
                receiver,
            )));
            Ok(())
        })
        .context("starting WebUI agent thread")?;
    Ok((sender, join))
}

#[allow(clippy::too_many_arguments)]
async fn actor_loop(
    mut agent: Agent,
    mut config: AppConfig,
    paths: MiyuPaths,
    state_store: StateStore,
    manager: Arc<Mutex<ManagerState>>,
    events: EventHub,
    questions: QuestionBroker,
    mut receiver: mpsc::UnboundedReceiver<ActorCommand>,
) {
    while let Some(command) = receiver.recv().await {
        match command {
            ActorCommand::StartTurn {
                run_id,
                session_id,
                content,
                mode,
                images,
                cwd,
                cancel,
            } => {
                // Serial turn-entry maintenance in the scheduler: stale-turn
                // recovery is owner-pid safe, and the prompt-change reset is
                // only attempted while this is the sole active run (it wipes
                // session history and must never race a concurrent turn).
                let _ = state_store.recover_stale_turns();
                if manager.lock().unwrap().active_runs.len() <= 1 {
                    if let Ok(prompt) = config.system_prompt(&paths) {
                        let _ = state_store
                            .pinned(&session_id)
                            .reset_if_prompt_changed(&prompt);
                    }
                }
                let store = state_store.pinned_for_turn(&session_id);
                // Per-turn workspace: a workspace bound to the session wins,
                // otherwise the calling client's cwd, otherwise the daemon
                // process cwd. The resolved path scopes the whole turn task.
                let workspace = store
                    .session_record(&session_id)
                    .ok()
                    .flatten()
                    .and_then(|record| record.workspace.map(std::path::PathBuf::from))
                    .filter(|path| path.is_dir())
                    .or_else(|| cwd.filter(|path| path.is_dir()))
                    .or_else(|| std::env::current_dir().ok())
                    .unwrap_or_else(|| std::path::PathBuf::from("."));
                let task = run_turn_task(
                    config.clone(),
                    paths.clone(),
                    store,
                    state_store.clone(),
                    manager.clone(),
                    events.clone(),
                    questions.clone(),
                    run_id,
                    session_id.clone(),
                    content,
                    mode,
                    images,
                    cancel,
                );
                tokio::task::spawn_local(crate::tools::workspace::with_workspace(
                    workspace,
                    crate::tools::workspace::with_session(session_id, task),
                ));
            }
            ActorCommand::SetModels { models, reply } => {
                let result = rebuild_for_models(
                    &mut agent,
                    &mut config,
                    &paths,
                    &state_store,
                    &manager,
                    &models,
                );
                release_admin(&manager);
                let _ = reply.send(result);
            }
            ActorCommand::ApplyConfig {
                config: next_config,
                prompts,
                reset_conversation,
                reply,
            } => {
                let result = rebuild_for_config(
                    &mut agent,
                    &mut config,
                    &paths,
                    &state_store,
                    &manager,
                    &events,
                    next_config,
                    &prompts,
                    reset_conversation,
                );
                release_admin(&manager);
                let _ = reply.send(result);
            }
            ActorCommand::ResetConversation { all, reply } => {
                let result = reset_actor_conversation(
                    &mut agent,
                    &config,
                    &paths,
                    &state_store,
                    &manager,
                    &events,
                    all,
                );
                release_admin(&manager);
                let _ = reply.send(result);
            }
            ActorCommand::SwitchSession { session_id, reply } => {
                let result =
                    switch_actor_session(&mut agent, &state_store, &manager, &events, &session_id);
                release_admin(&manager);
                let _ = reply.send(result);
            }
            ActorCommand::Shutdown => {
                // Cancel every running turn, then drain briefly so they can
                // persist their interrupted state before the runtime drops.
                for info in manager.lock().unwrap().active_runs.values() {
                    info.request_cancel();
                }
                for _ in 0..100 {
                    if manager.lock().unwrap().active_runs.is_empty() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                break;
            }
            ActorCommand::Undo { reply } => {
                let result = (|| -> std::result::Result<Value, AdminFailure> {
                    let (removed, prompt) = state_store
                        .undo_last_turn()
                        .map_err(|error| AdminFailure::Internal(safe_error_message(&error)))?;
                    manager.lock().unwrap().context = current_context(&agent)
                        .map_err(|error| AdminFailure::Internal(safe_error_message(&error)))?;
                    Ok(json!({ "removed": removed, "prompt": prompt }))
                })();
                release_admin(&manager);
                let _ = reply.send(result);
            }
            ActorCommand::Pop { turn_ids, reply } => {
                let result = (|| -> std::result::Result<Value, AdminFailure> {
                    if turn_ids.is_empty() {
                        return Ok(json!({ "turns": 0, "archived": false }));
                    }
                    let turns = state_store
                        .oldest_evictable_visible_turns(usize::MAX)
                        .map_err(|error| AdminFailure::Internal(safe_error_message(&error)))?;
                    let selected = turns
                        .into_iter()
                        .filter(|turn| turn_ids.iter().any(|id| id == &turn.turn_id))
                        .collect::<Vec<_>>();
                    if selected.len() != turn_ids.len() {
                        return Err(AdminFailure::Invalid(
                            "one or more conversation turns are no longer available".to_string(),
                        ));
                    }
                    let memory = MemoryStore::new(&config, &paths);
                    let memory_config = config.memory_config();
                    archive_and_delete_visible_turns(&state_store, &memory, &selected)
                        .map_err(|error| AdminFailure::Internal(safe_error_message(&error)))?;
                    manager.lock().unwrap().context = current_context(&agent)
                        .map_err(|error| AdminFailure::Internal(safe_error_message(&error)))?;
                    let data = json!({
                        "turns": selected.len(),
                        "archived": memory_config.enabled && memory_config.evicted_context_enabled
                    });
                    events.publish("conversation.pop", data.clone());
                    Ok(data)
                })();
                release_admin(&manager);
                let _ = reply.send(result);
            }
            ActorCommand::Compact { reply } => {
                let result = (|| async {
                    let compact = agent
                        .compact_now(|_| Ok(()))
                        .await
                        .map_err(|error| AdminFailure::Internal(safe_error_message(&error)))?;
                    manager.lock().unwrap().context = current_context(&agent)
                        .map_err(|error| AdminFailure::Internal(safe_error_message(&error)))?;
                    Ok::<Value, AdminFailure>(json!({
                        "compacted": compact.is_some(),
                        "usage": compact.as_ref().and_then(|result| result.usage.clone()),
                        "usage_estimated": compact
                            .as_ref()
                            .map(|result| result.usage_estimated)
                            .unwrap_or(false)
                    }))
                })()
                .await;
                release_admin(&manager);
                let _ = reply.send(result);
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn trim_process_memory() {
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(not(target_os = "linux"))]
fn trim_process_memory() {}

#[allow(clippy::too_many_arguments)]
/// Executes one turn as a self-contained task. Multiple turn tasks run
/// concurrently on the actor's LocalSet — each with its own Agent, a
/// StateStore pinned to the turn's session, and an independent cancel signal.
#[allow(clippy::too_many_arguments)]
async fn run_turn_task(
    config: AppConfig,
    paths: MiyuPaths,
    store: StateStore,
    base_store: StateStore,
    manager: Arc<Mutex<ManagerState>>,
    events: EventHub,
    questions: QuestionBroker,
    run_id: String,
    session_id: Arc<str>,
    content: String,
    mode: AgentMode,
    images: Vec<Option<ImageAttachment>>,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) {
    let manager = &manager;
    let events = &events;
    let questions = &questions;
    let run_id = run_id.as_str();
    events.publish(
        "run.started",
        json!({ "run_id": run_id, "session_id": &*session_id, "mode": mode_name(mode) }),
    );
    let title_seed: String = content.chars().take(80).collect();
    let setup = (|| -> Result<(Agent, AgentTurnControl)> {
        let normal_tools = build_tool_registry(&config, &paths, AgentMode::Normal, true)?;
        let plan_tools = build_tool_registry(&config, &paths, AgentMode::Plan, true)?;
        let chat_tools = build_tool_registry(&config, &paths, AgentMode::Chat, true)?;
        let active_tools = match mode {
            AgentMode::Normal => normal_tools.clone(),
            AgentMode::Plan => plan_tools.clone(),
            AgentMode::Chat => chat_tools.clone(),
        };
        let client = OpenAiCompatibleClient::from_config(&config, &paths)?;
        let agent = Agent::new(
            config.clone(),
            &paths,
            store.clone(),
            client,
            active_tools,
            mode,
        )?;
        Ok((
            agent,
            AgentTurnControl::new(mode, normal_tools, plan_tools, chat_tools),
        ))
    })();
    let (mut agent, control) = match setup {
        Ok(setup) => setup,
        Err(error) => {
            questions.cancel_run(run_id);
            finish_run(manager, run_id, None);
            let message = safe_error_message(&error);
            tracing::error!(run_id, error = %error, "WebUI agent run setup failed");
            events.publish(
                "run.failed",
                json!({ "run_id": run_id, "session_id": &*session_id, "message": message }),
            );
            return;
        }
    };
    // The daemon-wide context snapshot tracks the *current* session; a turn
    // for another session must not overwrite it.
    let updates_context = || &*base_store.session_id() == &*session_id;
    let agent = &mut agent;

    let mapper = Arc::new(Mutex::new(RunEventMapper::new(
        run_id.to_string(),
        events.clone(),
        questions.clone(),
        store.clone(),
    )));
    let chat_outcome = {
        let callback_mapper = mapper.clone();
        let images = images
            .into_iter()
            .map(|image| {
                image.map(|image| match image {
                    ImageAttachment::Binary { mime, data } => {
                        crate::clipboard::PastedImage::Binary(
                            crate::clipboard::ClipboardImage::new(mime, data),
                        )
                    }
                    ImageAttachment::Path { path } => crate::clipboard::PastedImage::Path(path),
                })
            })
            .collect::<Vec<_>>();
        let chat = agent.chat_stream_with_control(&content, &images, &control, move |event| {
            callback_mapper.lock().unwrap().handle(event);
            Ok(())
        });
        tokio::pin!(chat);
        loop {
            tokio::select! {
                biased;
                result = &mut chat => break TurnOutcome::Finished(result),
                changed = cancel.changed() => {
                    if changed.is_err() || *cancel.borrow() {
                        questions.cancel_run(run_id);
                        break TurnOutcome::Cancelled;
                    }
                }
            }
        }
    };

    let result = match chat_outcome {
        TurnOutcome::Cancelled => {
            finish_cancelled_run(
                manager,
                events,
                agent,
                run_id,
                &session_id,
                updates_context(),
            );
            finish_turn_task(&store, &title_seed, events, false);
            return;
        }
        TurnOutcome::Finished(Err(error)) if question::is_question_cancelled(&error) => {
            questions.cancel_run(run_id);
            finish_cancelled_run(
                manager,
                events,
                agent,
                run_id,
                &session_id,
                updates_context(),
            );
            finish_turn_task(&store, &title_seed, events, false);
            return;
        }
        TurnOutcome::Finished(Err(error)) => {
            finish_failed_run(
                manager,
                events,
                questions,
                agent,
                run_id,
                &session_id,
                updates_context(),
                &error,
            );
            finish_turn_task(&store, &title_seed, events, false);
            return;
        }
        TurnOutcome::Finished(Ok(result)) => result,
    };

    questions.cancel_run(run_id);
    let context_tokens = match agent.effective_context_tokens() {
        Ok(tokens) => tokens,
        Err(error) => {
            finish_completed_with_context_error(
                manager,
                events,
                agent,
                run_id,
                &session_id,
                updates_context(),
                &result,
                &error,
            );
            finish_turn_task(&store, &title_seed, events, true);
            return;
        }
    };
    let overflow_outcome = {
        let callback_mapper = mapper;
        let overflow = agent.handle_overflow_after_turn(context_tokens, move |event| {
            callback_mapper.lock().unwrap().handle(event);
            Ok(())
        });
        tokio::pin!(overflow);
        loop {
            tokio::select! {
                biased;
                result = &mut overflow => break OverflowOutcome::Finished(result),
                changed = cancel.changed() => {
                    if changed.is_err() || *cancel.borrow() {
                        break OverflowOutcome::Cancelled;
                    }
                }
            }
        }
    };
    match overflow_outcome {
        OverflowOutcome::Cancelled => {
            let context =
                current_context(agent).unwrap_or_else(|_| manager.lock().unwrap().context);
            finish_run(manager, run_id, updates_context().then_some(context));
            publish_completed(events, run_id, &session_id, &result, context);
            finish_turn_task(&store, &title_seed, events, true);
            return;
        }
        OverflowOutcome::Finished(Err(error)) => {
            finish_completed_with_context_error(
                manager,
                events,
                agent,
                run_id,
                &session_id,
                updates_context(),
                &result,
                &error,
            );
            finish_turn_task(&store, &title_seed, events, true);
            return;
        }
        OverflowOutcome::Finished(Ok(_)) => {}
    }
    let context = match current_context(agent) {
        Ok(context) => context,
        Err(error) => {
            finish_completed_with_context_error(
                manager,
                events,
                agent,
                run_id,
                &session_id,
                updates_context(),
                &result,
                &error,
            );
            finish_turn_task(&store, &title_seed, events, true);
            return;
        }
    };
    finish_run(manager, run_id, updates_context().then_some(context));
    publish_completed(events, run_id, &session_id, &result, context);
    finish_turn_task(&store, &title_seed, events, true);
}

/// Shared per-turn cleanup: auto-naming, activity timestamp, queue-identity
/// cleanup, and allocator trimming. `store` is the turn's pinned store, so
/// session-scoped operations hit the turn's own session.
fn finish_turn_task(store: &StateStore, title_seed: &str, events: &EventHub, completed: bool) {
    if completed {
        maybe_auto_name_session(store, events, title_seed);
        let _ = store.touch_session(&store.session_id());
    }
    let _ = store.discard_queued_prompts();
    trim_process_memory();
}

enum TurnOutcome {
    Finished(Result<ChatResult>),
    Cancelled,
}

enum OverflowOutcome {
    Finished(Result<Option<ChatResult>>),
    Cancelled,
}

#[allow(clippy::too_many_arguments)]
fn rebuild_for_models(
    agent: &mut Agent,
    config: &mut AppConfig,
    paths: &MiyuPaths,
    state_store: &StateStore,
    manager: &Arc<Mutex<ManagerState>>,
    models: &[ActiveProviderModelConfig],
) -> std::result::Result<(), AdminFailure> {
    let mut next_config = config.clone();
    next_config
        .set_active_provider_models(models)
        .map_err(|error| AdminFailure::Invalid(safe_error_message(&error)))?;
    if next_config.active_provider_models == config.active_provider_models {
        return Ok(());
    }
    crate::models_cache::try_load_active(paths, &next_config);
    let client = OpenAiCompatibleClient::from_config(&next_config, paths)
        .map_err(|error| AdminFailure::Internal(safe_error_message(&error)))?;
    let registry = build_tool_registry(&next_config, paths, AgentMode::Normal, true)
        .map_err(|error| AdminFailure::Internal(safe_error_message(&error)))?;
    let next_agent = Agent::new(
        next_config.clone(),
        paths,
        state_store.clone(),
        client,
        registry,
        AgentMode::Normal,
    )
    .map_err(|error| AdminFailure::Internal(safe_error_message(&error)))?;
    let context = current_context(&next_agent)
        .map_err(|error| AdminFailure::Internal(safe_error_message(&error)))?;
    next_config
        .save(paths)
        .map_err(|error| AdminFailure::Internal(safe_error_message(&error)))?;
    *agent = next_agent;
    *config = next_config.clone();
    let mut manager = manager.lock().unwrap();
    manager.config = next_config;
    manager.context = context;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn rebuild_for_config(
    agent: &mut Agent,
    config: &mut AppConfig,
    paths: &MiyuPaths,
    state_store: &StateStore,
    manager: &Arc<Mutex<ManagerState>>,
    events: &EventHub,
    next_config: AppConfig,
    prompts: &PromptDocuments,
    reset_conversation: bool,
) -> std::result::Result<(), AdminFailure> {
    let previous_prompts = read_prompt_documents(config, paths)
        .map_err(|error| AdminFailure::Internal(safe_error_message(error)))?;
    let prompt_backups =
        apply_prompt_documents(config, &next_config, &previous_prompts, prompts, paths)
            .map_err(|error| AdminFailure::Internal(safe_error_message(error)))?;
    let scope_backups = match apply_persona_scope_changes(
        config,
        &next_config,
        &previous_prompts,
        prompts,
        paths,
    ) {
        Ok(backups) => backups,
        Err(error) => {
            restore_file_backups(&prompt_backups);
            return Err(AdminFailure::Internal(safe_error_message(error)));
        }
    };
    let config_backup = FileBackup {
        path: paths.config_file.clone(),
        content: std::fs::read(&paths.config_file).ok(),
    };
    let system_prompt_backup = next_config.system_prompt.as_ref().map(|_| FileBackup {
        path: next_config.system_prompt_path(paths),
        content: std::fs::read(next_config.system_prompt_path(paths)).ok(),
    });

    let build_agent = || -> Result<Agent> {
        crate::models_cache::try_load_active(paths, &next_config);
        let client = OpenAiCompatibleClient::from_config(&next_config, paths)?;
        let registry = build_tool_registry(&next_config, paths, AgentMode::Normal, true)?;
        Agent::new(
            next_config.clone(),
            paths,
            state_store.clone(),
            client,
            registry,
            AgentMode::Normal,
        )
    };
    let mut next_agent = match build_agent() {
        Ok(agent) => agent,
        Err(error) => {
            restore_file_backups(&prompt_backups);
            restore_persona_scope_backups(&scope_backups);
            return Err(AdminFailure::Invalid(safe_error_message(error)));
        }
    };
    let mut context = match current_context(&next_agent) {
        Ok(context) => context,
        Err(error) => {
            restore_file_backups(&prompt_backups);
            restore_persona_scope_backups(&scope_backups);
            return Err(AdminFailure::Invalid(safe_error_message(error)));
        }
    };
    if let Err(error) = next_config.save(paths) {
        restore_file_backups(&prompt_backups);
        restore_persona_scope_backups(&scope_backups);
        restore_file_backups(std::slice::from_ref(&config_backup));
        if let Some(backup) = &system_prompt_backup {
            restore_file_backups(std::slice::from_ref(backup));
        }
        return Err(AdminFailure::Internal(safe_error_message(error)));
    }

    if reset_conversation {
        let reset = (|| -> Result<()> {
            state_store.reset_conversation()?;
            let memory = MemoryStore::new(&next_config, paths);
            memory.clear_evicted_context()?;
            memory.clear_pending_events()?;
            tools::clear_aur_review_state(paths)?;
            next_agent.reset_memory()?;
            next_agent.prepare_for_turn()?;
            context = current_context(&next_agent)?;
            Ok(())
        })();
        if let Err(error) = reset {
            restore_file_backups(&prompt_backups);
            restore_persona_scope_backups(&scope_backups);
            restore_file_backups(std::slice::from_ref(&config_backup));
            if let Some(backup) = &system_prompt_backup {
                restore_file_backups(std::slice::from_ref(backup));
            }
            return Err(AdminFailure::Internal(safe_error_message(error)));
        }
    }

    *agent = next_agent;
    *config = next_config.clone();
    let mut manager = manager.lock().unwrap();
    manager.config = next_config;
    manager.context = context;
    drop(manager);
    if reset_conversation {
        events.publish("conversation.reset", json!({}));
    }
    finalize_persona_scope_backups(&scope_backups);
    Ok(())
}

/// Auto-names a still-unnamed session from its first prompt once a turn has
/// run in it. Explicit names (given at creation or via rename) are never
/// overwritten.
fn maybe_auto_name_session(state_store: &StateStore, events: &EventHub, seed: &str) {
    let session_id = state_store.session_id();
    let Ok(Some(record)) = state_store.session_record(&session_id) else {
        return;
    };
    if !record.name.trim().is_empty() {
        return;
    }
    let title = session_title_from_prompt(seed);
    if title.is_empty() {
        return;
    }
    if state_store
        .rename_session(&record.session_id, &title)
        .is_ok()
    {
        events.publish(
            "session.renamed",
            json!({ "session_id": record.session_id, "name": title }),
        );
    }
}

fn session_title_from_prompt(prompt: &str) -> String {
    let cleaned = prompt
        .trim()
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut title: String = cleaned.chars().take(20).collect();
    if cleaned.chars().count() > 20 {
        title.push('…');
    }
    title
}

fn switch_actor_session(
    agent: &mut Agent,
    state_store: &StateStore,
    manager: &Arc<Mutex<ManagerState>>,
    events: &EventHub,
    session_id: &str,
) -> std::result::Result<(), AdminFailure> {
    // Notes: switching deliberately does not touch updated_at (viewing must
    // not reorder the session list), and runs no turn-entry maintenance —
    // switching is allowed while turns are running, so a prompt-change reset
    // here could wipe a session mid-turn.
    let switch = || -> Result<ContextSnapshot> {
        state_store.switch_session(session_id)?;
        current_context(agent)
    };
    let context = switch().map_err(|error| AdminFailure::Internal(safe_error_message(&error)))?;
    manager.lock().unwrap().context = context;
    events.publish(
        "session.current_changed",
        json!({ "session_id": session_id }),
    );
    Ok(())
}

fn reset_actor_conversation(
    agent: &mut Agent,
    config: &AppConfig,
    paths: &MiyuPaths,
    state_store: &StateStore,
    manager: &Arc<Mutex<ManagerState>>,
    events: &EventHub,
    all: bool,
) -> std::result::Result<(), AdminFailure> {
    let mut reset = || -> Result<ContextSnapshot> {
        state_store.reset_conversation()?;
        let memory = MemoryStore::new(config, paths);
        if all {
            memory.reset_all(false)?;
        } else {
            memory.clear_evicted_context()?;
            memory.clear_pending_events()?;
        }
        tools::clear_aur_review_state(paths)?;
        agent.reset_memory()?;
        agent.prepare_for_turn()?;
        current_context(agent)
    };
    let context = reset().map_err(|error| AdminFailure::Internal(safe_error_message(&error)))?;
    manager.lock().unwrap().context = context;
    events.publish("conversation.reset", json!({}));
    Ok(())
}

fn finish_cancelled_run(
    manager: &Arc<Mutex<ManagerState>>,
    events: &EventHub,
    agent: &Agent,
    run_id: &str,
    session_id: &str,
    updates_context: bool,
) {
    let context = current_context(agent).ok().filter(|_| updates_context);
    finish_run(manager, run_id, context);
    events.publish(
        "run.cancelled",
        json!({ "run_id": run_id, "session_id": session_id }),
    );
}

#[allow(clippy::too_many_arguments)]
fn finish_failed_run(
    manager: &Arc<Mutex<ManagerState>>,
    events: &EventHub,
    questions: &QuestionBroker,
    agent: &Agent,
    run_id: &str,
    session_id: &str,
    updates_context: bool,
    error: &anyhow::Error,
) {
    questions.cancel_run(run_id);
    let context = current_context(agent).ok().filter(|_| updates_context);
    finish_run(manager, run_id, context);
    let message = safe_error_message(error);
    tracing::error!(run_id, error = %error, "WebUI agent run failed");
    events.publish(
        "run.failed",
        json!({ "run_id": run_id, "session_id": session_id, "message": message }),
    );
}

#[allow(clippy::too_many_arguments)]
fn finish_completed_with_context_error(
    manager: &Arc<Mutex<ManagerState>>,
    events: &EventHub,
    agent: &Agent,
    run_id: &str,
    session_id: &str,
    updates_context: bool,
    result: &ChatResult,
    error: &anyhow::Error,
) {
    let message = safe_error_message(error);
    tracing::error!(run_id, error = %error, "WebUI post-turn context maintenance failed");
    events.publish(
        "context.error",
        json!({ "run_id": run_id, "session_id": session_id, "message": message }),
    );
    let context = current_context(agent).unwrap_or_else(|_| manager.lock().unwrap().context);
    finish_run(manager, run_id, updates_context.then_some(context));
    publish_completed(events, run_id, session_id, result, context);
}

fn finish_run(manager: &Arc<Mutex<ManagerState>>, run_id: &str, context: Option<ContextSnapshot>) {
    let mut manager = manager.lock().unwrap();
    if let Some(context) = context {
        manager.context = context;
    }
    manager.active_runs.remove(run_id);
}

fn publish_completed(
    events: &EventHub,
    run_id: &str,
    session_id: &str,
    result: &ChatResult,
    context: ContextSnapshot,
) {
    events.publish(
        "run.completed",
        json!({
            "run_id": run_id,
            "session_id": session_id,
            "usage": result.usage,
            "usage_estimated": result.usage_estimated,
            "provider_id": result.provider_id,
            "model": result.model,
            "context_tokens": context.tokens,
            "context_window": context.window,
            "cumulative_tokens": context.cumulative_tokens,
        }),
    );
}

fn current_context(agent: &Agent) -> Result<ContextSnapshot> {
    Ok(ContextSnapshot {
        tokens: agent.effective_context_tokens()?,
        window: agent.context_window(),
        cumulative_tokens: agent.conversation_usage_tokens()?,
    })
}

fn session_state(
    manager: &Arc<Mutex<ManagerState>>,
    state_store: &StateStore,
) -> Result<ipc::SessionState> {
    let context = manager.lock().unwrap().context;
    let session_id = state_store.session_id();
    let record = state_store.session_record(&session_id)?;
    Ok(ipc::SessionState {
        context_tokens: context.tokens,
        context_window: context.window,
        cumulative_tokens: context.cumulative_tokens,
        session_id: session_id.to_string(),
        session_name: record
            .as_ref()
            .map(|record| record.name.clone())
            .unwrap_or_default(),
        workspace: record.and_then(|record| record.workspace),
    })
}

/// Global admin reservation (config/model changes): requires that no turn is
/// running in any session.
fn reserve_admin(manager: &Arc<Mutex<ManagerState>>) -> std::result::Result<(), ApiError> {
    let mut manager = manager.lock().unwrap();
    if !manager.active_runs.is_empty() || manager.admin_busy {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "Miyu is busy with another operation",
        ));
    }
    manager.admin_busy = true;
    Ok(())
}

/// Per-session admin reservation (reset/undo/pop/compact/delete/archive):
/// only the target session must be idle; turns in other sessions keep
/// running.
fn reserve_admin_for_session(
    manager: &Arc<Mutex<ManagerState>>,
    session_id: &str,
) -> std::result::Result<(), ApiError> {
    let mut manager = manager.lock().unwrap();
    if manager.admin_busy || manager.session_has_runs(session_id) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "Miyu is busy with another operation",
        ));
    }
    manager.admin_busy = true;
    Ok(())
}

/// Light admin reservation (session switching): serializes against other
/// admin operations but is allowed while turns are running.
fn reserve_admin_light(manager: &Arc<Mutex<ManagerState>>) -> std::result::Result<(), ApiError> {
    let mut manager = manager.lock().unwrap();
    if manager.admin_busy {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "Miyu is busy with another operation",
        ));
    }
    manager.admin_busy = true;
    Ok(())
}

fn require_no_running_turn(state_store: &StateStore) -> std::result::Result<(), ApiError> {
    if state_store
        .has_any_running_turns()
        .map_err(ApiError::internal)?
    {
        Err(ApiError::new(
            StatusCode::CONFLICT,
            "a conversation turn is already running",
        ))
    } else {
        Ok(())
    }
}

fn release_admin(manager: &Arc<Mutex<ManagerState>>) {
    manager.lock().unwrap().admin_busy = false;
}

fn config_response(
    config: &AppConfig,
    context: ContextSnapshot,
    paths: &MiyuPaths,
) -> std::result::Result<ConfigResponse, ApiError> {
    let mut redacted = config.clone();
    let mut secret_states = HashMap::new();
    for (index, provider) in redacted.providers.iter_mut().enumerate() {
        secret_states.insert(
            format!("providers.{index}.api_key"),
            provider
                .api_key
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty()),
        );
        provider.api_key = None;
    }
    redact_secret_list(
        &mut secret_states,
        "plugins.web.tavily_api_keys",
        &mut redacted.plugins.web.tavily_api_keys,
    );
    redact_secret_list(
        &mut secret_states,
        "plugins.web.firecrawl_api_keys",
        &mut redacted.plugins.web.firecrawl_api_keys,
    );
    redact_secret_list(
        &mut secret_states,
        "plugins.web.anysearch_api_keys",
        &mut redacted.plugins.web.anysearch_api_keys,
    );
    secret_states.insert(
        "plugins.exchange_rate.api_key".to_string(),
        !redacted.plugins.exchange_rate.api_key.trim().is_empty(),
    );
    redacted.plugins.exchange_rate.api_key.clear();
    redact_secret_list(
        &mut secret_states,
        "plugins.image_generation.api_keys",
        &mut redacted.plugins.image_generation.api_keys,
    );
    let mut config_value = serde_json::to_value(&redacted).map_err(ApiError::internal)?;
    if let Value::Object(config_object) = &mut config_value {
        config_object.insert(
            "memory".to_string(),
            serde_json::to_value(redacted.memory_config()).map_err(ApiError::internal)?,
        );
    }
    let prompts = read_prompt_documents(config, paths).map_err(ApiError::internal)?;
    Ok(ConfigResponse {
        config: config_value,
        secret_states,
        prompts,
        models: safe_models(config),
        multimodal_models: safe_multimodal_models(config),
        display: web_display_config(config),
        context,
    })
}

fn redact_secret_list(states: &mut HashMap<String, bool>, key: &str, values: &mut Vec<String>) {
    states.insert(
        key.to_string(),
        values.iter().any(|value| !value.trim().is_empty()),
    );
    values.clear();
}

fn restore_config_secrets(
    candidate: &mut AppConfig,
    current: &AppConfig,
    mutations: &HashMap<String, SecretMutation>,
) -> std::result::Result<(), ApiError> {
    let mut recognized = HashSet::new();
    for (index, provider) in candidate.providers.iter_mut().enumerate() {
        let key = format!("providers.{index}.api_key");
        recognized.insert(key.clone());
        let existing = current
            .providers
            .iter()
            .find(|item| item.id == provider.id)
            .and_then(|item| item.api_key.clone());
        provider.api_key = match mutations.get(&key) {
            Some(SecretMutation::Set(value)) => normalize_single_secret(value, &key)?,
            Some(SecretMutation::Clear) => None,
            None => existing,
        };
    }

    restore_secret_list(
        candidate,
        current,
        mutations,
        &mut recognized,
        "plugins.web.tavily_api_keys",
        |config| &mut config.plugins.web.tavily_api_keys,
        |config| &config.plugins.web.tavily_api_keys,
    )?;
    restore_secret_list(
        candidate,
        current,
        mutations,
        &mut recognized,
        "plugins.web.firecrawl_api_keys",
        |config| &mut config.plugins.web.firecrawl_api_keys,
        |config| &config.plugins.web.firecrawl_api_keys,
    )?;
    restore_secret_list(
        candidate,
        current,
        mutations,
        &mut recognized,
        "plugins.web.anysearch_api_keys",
        |config| &mut config.plugins.web.anysearch_api_keys,
        |config| &config.plugins.web.anysearch_api_keys,
    )?;
    restore_secret_list(
        candidate,
        current,
        mutations,
        &mut recognized,
        "plugins.image_generation.api_keys",
        |config| &mut config.plugins.image_generation.api_keys,
        |config| &config.plugins.image_generation.api_keys,
    )?;

    let exchange_key = "plugins.exchange_rate.api_key";
    recognized.insert(exchange_key.to_string());
    candidate.plugins.exchange_rate.api_key = match mutations.get(exchange_key) {
        Some(SecretMutation::Set(value)) => {
            normalize_single_secret(value, exchange_key)?.unwrap_or_default()
        }
        Some(SecretMutation::Clear) => String::new(),
        None => current.plugins.exchange_rate.api_key.clone(),
    };

    if let Some(key) = mutations.keys().find(|key| !recognized.contains(*key)) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("unknown secret field: {key}"),
        ));
    }
    Ok(())
}

fn restore_secret_list<Mut, Ref>(
    candidate: &mut AppConfig,
    current: &AppConfig,
    mutations: &HashMap<String, SecretMutation>,
    recognized: &mut HashSet<String>,
    key: &str,
    candidate_values: Mut,
    current_values: Ref,
) -> std::result::Result<(), ApiError>
where
    Mut: FnOnce(&mut AppConfig) -> &mut Vec<String>,
    Ref: FnOnce(&AppConfig) -> &Vec<String>,
{
    recognized.insert(key.to_string());
    *candidate_values(candidate) = match mutations.get(key) {
        Some(SecretMutation::Set(value)) => parse_secret_list(value, key)?,
        Some(SecretMutation::Clear) => Vec::new(),
        None => current_values(current).clone(),
    };
    Ok(())
}

fn normalize_single_secret(
    value: &str,
    field: &str,
) -> std::result::Result<Option<String>, ApiError> {
    validate_secret_text(value, field)?;
    Ok(Some(value.trim().to_string()).filter(|value| !value.is_empty()))
}

fn parse_secret_list(value: &str, field: &str) -> std::result::Result<Vec<String>, ApiError> {
    validate_secret_text(value, field)?;
    Ok(value
        .split(|character| matches!(character, ',' | '\n' | '\r'))
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect())
}

fn validate_secret_text(value: &str, field: &str) -> std::result::Result<(), ApiError> {
    if value.chars().count() > MAX_SECRET_CHARS
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("{field} is invalid"),
        ));
    }
    Ok(())
}

fn validate_config_candidate(config: &AppConfig) -> std::result::Result<(), ApiError> {
    config.validate().map_err(|error| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("invalid configuration: {}", safe_error_message(error)),
        )
    })?;
    let mut provider_ids = HashSet::with_capacity(config.providers.len());
    for provider in &config.providers {
        if !provider_ids.insert(provider.id.trim()) {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("duplicate provider id: {}", provider.id),
            ));
        }
    }
    if let Some(active) = &config.active_provider_models {
        let mut checked = config.clone();
        checked
            .set_active_provider_models(active)
            .map_err(|error| ApiError::new(StatusCode::BAD_REQUEST, safe_error_message(error)))?;
    }
    if let Some(active) = &config.active_multimodal_provider_models {
        let choices = config.provider_model_choices();
        let mut seen = HashSet::with_capacity(active.len());
        for model in active {
            if !seen.insert((&model.provider_id, &model.model))
                || !choices.iter().any(|choice| {
                    choice.provider_id == model.provider_id && choice.model == model.model
                })
            {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    format!(
                        "invalid multimodal provider/model: {} / {}",
                        model.provider_id, model.model
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn validate_prompt_documents(
    config: &AppConfig,
    prompts: &PromptDocuments,
) -> std::result::Result<(), ApiError> {
    validate_prompt_document_list("persona", &prompts.personas)?;
    validate_prompt_document_list("identity", &prompts.identities)?;
    if !config.prompt.active_persona.trim().is_empty()
        && !prompts
            .personas
            .iter()
            .any(|document| document.name == config.prompt.active_persona)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "the active persona does not exist",
        ));
    }
    if !config.prompt.active_identity.trim().is_empty()
        && !prompts
            .identities
            .iter()
            .any(|document| document.name == config.prompt.active_identity)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "the active identity does not exist",
        ));
    }
    Ok(())
}

fn validate_prompt_document_list(
    kind: &str,
    documents: &[PromptDocument],
) -> std::result::Result<(), ApiError> {
    if documents.len() > MAX_PROMPT_DOCUMENTS {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("at most {MAX_PROMPT_DOCUMENTS} {kind} documents are allowed"),
        ));
    }
    let mut names = HashSet::with_capacity(documents.len());
    let mut original_names = HashSet::with_capacity(documents.len());
    for document in documents {
        validate_prompt_document_name(&document.name, kind)?;
        if !names.insert(document.name.as_str()) {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                format!("duplicate {kind} document: {}", document.name),
            ));
        }
        if document.content.chars().count() > MAX_PROMPT_DOCUMENT_CHARS
            || document.content.contains('\0')
        {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("{kind} document is too large: {}", document.name),
            ));
        }
        if let Some(original) = document.original_name.as_deref() {
            validate_prompt_document_name(original, kind)?;
            if !original_names.insert(original) {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    format!("duplicate original {kind} document: {original}"),
                ));
            }
        }
    }
    Ok(())
}

fn validate_prompt_document_name(name: &str, kind: &str) -> std::result::Result<(), ApiError> {
    let valid = name == name.trim()
        && name.ends_with(".md")
        && name.len() <= 240
        && name.len() > 3
        && !name.starts_with('.')
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
        && FilePath::new(name)
            .file_name()
            .and_then(|value| value.to_str())
            == Some(name);
    if !valid {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("invalid {kind} document name: {name}"),
        ));
    }
    Ok(())
}

fn read_prompt_documents(config: &AppConfig, paths: &MiyuPaths) -> Result<PromptDocuments> {
    Ok(PromptDocuments {
        personas: read_prompt_document_dir(&config.prompts_dir_path(paths))?,
        identities: read_prompt_document_dir(&config.identities_dir_path(paths))?,
    })
}

fn read_prompt_document_dir(dir: &FilePath) -> Result<Vec<PromptDocument>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut documents = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".md") {
            continue;
        }
        let content = std::fs::read_to_string(entry.path())?;
        documents.push(PromptDocument {
            original_name: Some(name.clone()),
            name,
            content,
        });
    }
    documents.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(documents)
}

fn prompt_configuration_changed(current: &AppConfig, candidate: &AppConfig) -> bool {
    serde_json::to_value(&current.prompt).ok() != serde_json::to_value(&candidate.prompt).ok()
        || current.system_prompt_file != candidate.system_prompt_file
        || current.system_prompt != candidate.system_prompt
}

fn prompt_documents_changed(current: &PromptDocuments, candidate: &PromptDocuments) -> bool {
    canonical_prompt_documents(&current.personas) != canonical_prompt_documents(&candidate.personas)
        || canonical_prompt_documents(&current.identities)
            != canonical_prompt_documents(&candidate.identities)
}

fn canonical_prompt_documents(documents: &[PromptDocument]) -> Vec<(String, String)> {
    let mut values = documents
        .iter()
        .map(|document| (document.name.clone(), document.content.clone()))
        .collect::<Vec<_>>();
    values.sort_by(|left, right| left.0.cmp(&right.0));
    values
}

struct FileBackup {
    path: PathBuf,
    content: Option<Vec<u8>>,
}

struct PersonaScopeBackup {
    original: PathBuf,
    staged: PathBuf,
    destination: Option<PathBuf>,
}

fn apply_prompt_documents(
    current_config: &AppConfig,
    next_config: &AppConfig,
    current: &PromptDocuments,
    next: &PromptDocuments,
    paths: &MiyuPaths,
) -> Result<Vec<FileBackup>> {
    let mut mutations = HashMap::<PathBuf, Option<Vec<u8>>>::new();
    collect_prompt_file_mutations(
        &current.personas,
        &next.personas,
        &current_config.prompts_dir_path(paths),
        &next_config.prompts_dir_path(paths),
        &mut mutations,
    );
    collect_prompt_file_mutations(
        &current.identities,
        &next.identities,
        &current_config.identities_dir_path(paths),
        &next_config.identities_dir_path(paths),
        &mut mutations,
    );
    let backups = mutations
        .keys()
        .map(|path| FileBackup {
            path: path.clone(),
            content: std::fs::read(path).ok(),
        })
        .collect::<Vec<_>>();
    for (path, content) in mutations {
        let result = if let Some(content) = content {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, content)
        } else if path.exists() {
            std::fs::remove_file(&path)
        } else {
            Ok(())
        };
        if let Err(error) = result {
            restore_file_backups(&backups);
            return Err(error.into());
        }
    }
    Ok(backups)
}

fn apply_persona_scope_changes(
    current_config: &AppConfig,
    next_config: &AppConfig,
    current: &PromptDocuments,
    next: &PromptDocuments,
    paths: &MiyuPaths,
) -> Result<Vec<PersonaScopeBackup>> {
    let mut changes = Vec::<(String, Option<String>)>::new();
    for document in &current.personas {
        let represented = next.personas.iter().find(|next_document| {
            next_document.original_name.as_deref() == Some(document.name.as_str())
                || next_document.original_name.is_none() && next_document.name == document.name
        });
        match represented {
            Some(next_document) if next_document.name != document.name => {
                changes.push((document.name.clone(), Some(next_document.name.clone())));
            }
            None => changes.push((document.name.clone(), None)),
            _ => {}
        }
    }

    let mut backups = Vec::new();
    let stage_result = (|| -> Result<()> {
        for (change_index, (old_name, new_name)) in changes.iter().enumerate() {
            let old_paths = [
                current_config.persona_memory_data_dir(paths, old_name),
                current_config.persona_memory_state_dir(paths, old_name),
                current_config.persona_skills_dir(paths, old_name),
            ];
            let new_paths = new_name.as_ref().map(|name| {
                [
                    next_config.persona_memory_data_dir(paths, name),
                    next_config.persona_memory_state_dir(paths, name),
                    next_config.persona_skills_dir(paths, name),
                ]
            });
            for (scope_index, original) in old_paths.into_iter().enumerate() {
                if !original.exists() {
                    continue;
                }
                let parent = original
                    .parent()
                    .context("persona scope path has no parent")?;
                let staged = parent.join(format!(
                    ".miyu-web-scope-{}-{change_index}-{scope_index}",
                    random_token(10)
                ));
                std::fs::rename(&original, &staged)?;
                backups.push(PersonaScopeBackup {
                    original,
                    staged,
                    destination: new_paths.as_ref().map(|paths| paths[scope_index].clone()),
                });
            }
        }
        Ok(())
    })();
    if let Err(error) = stage_result {
        restore_persona_scope_backups(&backups);
        return Err(error);
    }

    let result = (|| -> Result<()> {
        for backup in &backups {
            let Some(destination) = &backup.destination else {
                continue;
            };
            if destination.exists() {
                anyhow::bail!(
                    "persona scope destination already exists: {}",
                    destination.display()
                );
            }
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(&backup.staged, destination)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        restore_persona_scope_backups(&backups);
        return Err(error);
    }
    Ok(backups)
}

fn restore_persona_scope_backups(backups: &[PersonaScopeBackup]) {
    for backup in backups.iter().rev() {
        if let Some(destination) = &backup.destination {
            if destination.exists() && !backup.staged.exists() {
                let _ = std::fs::rename(destination, &backup.staged);
            }
        }
        if backup.staged.exists() {
            if let Some(parent) = backup.original.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::rename(&backup.staged, &backup.original);
        }
    }
}

fn finalize_persona_scope_backups(backups: &[PersonaScopeBackup]) {
    for backup in backups {
        if backup.destination.is_none() && backup.staged.exists() {
            let _ = std::fs::remove_dir_all(&backup.staged);
        }
    }
}

fn collect_prompt_file_mutations(
    current: &[PromptDocument],
    next: &[PromptDocument],
    current_dir: &FilePath,
    next_dir: &FilePath,
    mutations: &mut HashMap<PathBuf, Option<Vec<u8>>>,
) {
    for document in next {
        let content = document.content.trim_end();
        let content = if content.is_empty() {
            Vec::new()
        } else {
            format!("{content}\n").into_bytes()
        };
        mutations.insert(next_dir.join(&document.name), Some(content));
    }
    for document in current {
        let represented = next.iter().find(|next_document| {
            next_document.original_name.as_deref() == Some(document.name.as_str())
                || next_document.original_name.is_none() && next_document.name == document.name
        });
        let old_path = current_dir.join(&document.name);
        let retained_at_same_path = represented
            .map(|next_document| next_dir.join(&next_document.name) == old_path)
            .unwrap_or(false);
        if !retained_at_same_path {
            mutations.entry(old_path).or_insert(None);
        }
    }
}

fn restore_file_backups(backups: &[FileBackup]) {
    for backup in backups {
        restore_optional_file(&backup.path, backup.content.as_deref());
    }
}

fn restore_optional_file(path: &FilePath, content: Option<&[u8]>) {
    if let Some(content) = content {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, content);
    } else if path.exists() {
        let _ = std::fs::remove_file(path);
    }
}

fn safe_models(config: &AppConfig) -> Vec<SafeModel> {
    config
        .provider_model_choices()
        .into_iter()
        .map(|choice| SafeModel {
            active: config.is_active_provider_model(&choice.provider_id, &choice.model),
            provider_id: choice.provider_id,
            provider_name: choice.provider_name,
            model: choice.model,
        })
        .collect()
}

fn web_display_config(config: &AppConfig) -> WebDisplayConfig {
    let mixed_model_endpoint_display = config.display.mixed_model_endpoint_display.clone();
    WebDisplayConfig {
        reasoning: config.display.reasoning.clone(),
        tool_calls: config.display.tool_calls.clone(),
        readable_tool_names: config.display.readable_tool_names,
        command_output_lines: config.display.command_output_lines,
        show_mixed_model_endpoint: config.active_provider_model_choices().len() > 1
            && matches!(mixed_model_endpoint_display.as_str(), "interactive" | "all"),
        mixed_model_endpoint_display,
    }
}

fn safe_multimodal_models(config: &AppConfig) -> Vec<SafeModel> {
    config
        .multimodal_provider_model_choices()
        .into_iter()
        .map(|choice| SafeModel {
            active: config.is_active_multimodal_provider_model(&choice.provider_id, &choice.model),
            provider_id: choice.provider_id,
            provider_name: choice.provider_name,
            model: choice.model,
        })
        .collect()
}

impl SafeTurn {
    fn from_turn(turn: Turn, assets: Vec<ImageAsset>) -> Self {
        let assets = assets
            .into_iter()
            .map(|asset| {
                let hide_caption = meme_asset_caption_hidden(&asset, &turn.tool_reports);
                SafeImageAsset::from_asset(asset, hide_caption)
            })
            .collect();
        Self {
            id: turn.turn_id,
            seq: turn.seq,
            status: match turn.status {
                TurnStatus::Running => "running",
                TurnStatus::Completed => "completed",
                TurnStatus::Interrupted => "interrupted",
            },
            active_context: !turn.hidden,
            user_content: turn.user_content,
            assistant_content: redact_internal_assistant_text(&turn.assistant_content),
            assistant_reasoning: turn
                .assistant_reasoning
                .map(|reasoning| redact_internal_assistant_text(&reasoning)),
            provider_id: turn.assistant_provider_id,
            model: turn.assistant_model,
            user_timestamp: turn.user_timestamp,
            assistant_timestamp: turn.assistant_timestamp,
            token_total: turn.token_total,
            token_usage_estimated: turn.token_usage_estimated,
            question_exchanges: turn.question_exchanges,
            followups: turn.followups.into_iter().map(SafeFollowup::from).collect(),
            assets,
        }
    }
}

impl SafeImageAsset {
    fn from_asset(asset: ImageAsset, hide_caption: bool) -> Self {
        Self {
            url: format!("/api/assets/{}", asset.asset_id),
            id: asset.asset_id,
            mime: asset.mime,
            width: asset.width,
            height: asset.height,
            alt: asset.alt,
            hide_caption,
        }
    }
}

impl From<ImageAsset> for SafeImageAsset {
    fn from(asset: ImageAsset) -> Self {
        Self::from_asset(asset, false)
    }
}

fn meme_asset_caption_hidden(asset: &ImageAsset, reports: &[String]) -> bool {
    const MAX_DESCRIPTION_CHARS: usize = 120;

    let description = asset.alt.split_whitespace().collect::<Vec<_>>().join(" ");
    if description.is_empty() {
        return false;
    }
    let mut characters = description.chars();
    let mut compact = characters
        .by_ref()
        .take(MAX_DESCRIPTION_CHARS)
        .collect::<String>();
    if characters.next().is_some() {
        compact.push('…');
    }
    let escaped = compact
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let marker = format!("description={escaped}</sent_meme>");
    reports
        .iter()
        .any(|report| report.starts_with("<sent_meme>") && report.contains(&marker))
}

impl From<TurnFollowup> for SafeFollowup {
    fn from(followup: TurnFollowup) -> Self {
        Self {
            id: followup.prompt_id,
            content: followup.display_content,
            submitted_at: followup.submitted_at,
            preceding_assistant_content: followup
                .preceding_assistant_content
                .map(|content| redact_internal_assistant_text(&content)),
            preceding_assistant_reasoning: followup
                .preceding_assistant_reasoning
                .map(|reasoning| redact_internal_assistant_text(&reasoning)),
            provider_id: followup.preceding_assistant_provider_id,
            model: followup.preceding_assistant_model,
        }
    }
}

impl From<QueuedPrompt> for SafeQueuedPrompt {
    fn from(prompt: QueuedPrompt) -> Self {
        Self {
            id: prompt.prompt_id,
            content: prompt.display_content,
            submitted_at: prompt.submitted_at,
        }
    }
}

impl From<UsageSnapshot> for SafeUsageSnapshot {
    fn from(usage: UsageSnapshot) -> Self {
        Self {
            requests: usage.requests,
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
            conversation_tokens: usage.conversation_tokens,
            last_usage: usage.last_usage,
            last_conversation_usage: usage.last_conversation_usage,
        }
    }
}

fn redact_internal_assistant_text(value: &str) -> String {
    value
        .replace(crate::state::pending_placeholder(), "")
        .replace(crate::state::interrupted_text(), "")
}

fn normalize_answers(
    request: &QuestionRequest,
    mut answers: QuestionAnswers,
) -> std::result::Result<QuestionAnswers, String> {
    for answer in &mut answers {
        for value in answer {
            *value = value.trim().to_string();
            if value.chars().any(char::is_control) {
                return Err("answers cannot contain control characters".to_string());
            }
        }
    }
    question::validate_answers(request, &answers).map_err(|error| safe_error_message(&error))?;
    Ok(answers)
}

fn validate_content(content: String) -> std::result::Result<String, ApiError> {
    let content = content.trim().to_string();
    if content.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "content cannot be empty",
        ));
    }
    if content.chars().count() > MAX_CONTENT_CHARS {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("content cannot exceed {MAX_CONTENT_CHARS} characters"),
        ));
    }
    if content
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "content contains unsupported control characters",
        ));
    }
    Ok(content)
}

fn validate_short_field(
    value: String,
    field: &str,
    max_chars: usize,
) -> std::result::Result<String, ApiError> {
    let value = value.trim().to_string();
    if value.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("{field} cannot be empty"),
        ));
    }
    if value.chars().count() > max_chars || value.chars().any(char::is_control) {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!("{field} is invalid"),
        ));
    }
    Ok(value)
}

fn validate_model_selection(
    models: Vec<ActiveProviderModelConfig>,
) -> std::result::Result<Vec<ActiveProviderModelConfig>, ApiError> {
    if models.is_empty() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "at least one model endpoint must remain active",
        ));
    }
    if models.len() > 64 {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "at most 64 model endpoints can be active",
        ));
    }
    let mut seen = HashSet::with_capacity(models.len());
    let mut validated = Vec::with_capacity(models.len());
    for model in models {
        let provider_id = validate_short_field(model.provider_id, "provider_id", 200)?;
        let model = validate_short_field(model.model, "model", 500)?;
        if !seen.insert((provider_id.clone(), model.clone())) {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "duplicate provider/model selection",
            ));
        }
        validated.push(ActiveProviderModelConfig { provider_id, model });
    }
    Ok(validated)
}

fn parse_mode(mode: &str) -> std::result::Result<AgentMode, ApiError> {
    match mode {
        "normal" => Ok(AgentMode::Normal),
        "plan" => Ok(AgentMode::Plan),
        "chat" => Ok(AgentMode::Chat),
        _ => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "mode must be normal, plan, or chat",
        )),
    }
}

fn mode_name(mode: AgentMode) -> &'static str {
    match mode {
        AgentMode::Normal => "normal",
        AgentMode::Plan => "plan",
        AgentMode::Chat => "chat",
    }
}

fn real_tool_name(event_name: &str) -> &str {
    if event_name.starts_with("load_skill:") {
        "load_skill"
    } else if event_name.starts_with("load_tools:") {
        "load_tools"
    } else {
        event_name
    }
}

fn require_auth(headers: &HeaderMap, state: &WebState) -> std::result::Result<(), ApiError> {
    if state
        .auth
        .is_authenticated(cookie_value(headers, AUTH_COOKIE))
    {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            "authentication required",
        ))
    }
}

fn require_mutation(headers: &HeaderMap, state: &WebState) -> std::result::Result<(), ApiError> {
    require_auth(headers, state)?;
    if origin_is_allowed(headers) {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "request origin is not allowed",
        ))
    }
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    for header in headers.get_all(COOKIE) {
        let Ok(header) = header.to_str() else {
            continue;
        };
        for pair in header.split(';') {
            let Some((key, value)) = pair.trim().split_once('=') else {
                continue;
            };
            if key.trim() == name {
                return Some(value.trim());
            }
        }
    }
    None
}

fn origin_is_allowed(headers: &HeaderMap) -> bool {
    let mut origins = headers.get_all(ORIGIN).iter();
    let Some(origin) = origins.next() else {
        return true;
    };
    if origins.next().is_some() {
        return false;
    }
    let Some(host) = headers.get(HOST).and_then(|host| host.to_str().ok()) else {
        return false;
    };
    let expected = format!("http://{host}");
    origin.to_str().is_ok_and(|origin| origin == expected)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let length = left.len().max(right.len());
    for index in 0..length {
        let left = left.get(index).copied().unwrap_or(0);
        let right = right.get(index).copied().unwrap_or(0);
        difference |= usize::from(left ^ right);
    }
    difference == 0
}

fn random_token(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    OsRng.fill_bytes(&mut buffer);
    URL_SAFE_NO_PAD.encode(buffer)
}

fn random_id(prefix: &str, bytes: usize) -> String {
    format!("{prefix}_{}", random_token(bytes))
}

fn safe_error_message(error: impl std::fmt::Display) -> String {
    let message = error
        .to_string()
        .chars()
        .filter(|character| !character.is_control() || *character == '\n' || *character == '\t')
        .take(1000)
        .collect::<String>();
    if message.trim().is_empty() {
        "operation failed".to_string()
    } else {
        message
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::question::{QuestionOption, QuestionPrompt};

    fn manager_with_run(
        run_id: &str,
    ) -> (
        Arc<Mutex<ManagerState>>,
        tokio::sync::watch::Receiver<bool>,
    ) {
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let manager = Arc::new(Mutex::new(ManagerState {
            config: AppConfig::default(),
            active_runs: HashMap::from([(
                run_id.to_string(),
                RunInfo {
                    session_id: "default".into(),
                    mode: AgentMode::Normal,
                    cancel: cancel_tx,
                },
            )]),
            admin_busy: false,
            context: ContextSnapshot {
                tokens: 0,
                window: None,
                cumulative_tokens: 0,
            },
        }));
        (manager, cancel_rx)
    }

    #[test]
    fn dropped_ipc_turn_cancels_its_core_run() {
        let (manager, cancel_rx) = manager_with_run("run_test");
        drop(IpcRunGuard {
            manager,
            run_id: "run_test".to_string(),
            finished: false,
        });
        assert!(*cancel_rx.borrow());
    }

    #[test]
    fn completed_ipc_turn_does_not_send_a_late_cancel() {
        let (manager, cancel_rx) = manager_with_run("run_test");
        let mut guard = IpcRunGuard {
            manager,
            run_id: "run_test".to_string(),
            finished: false,
        };
        guard.finish();
        drop(guard);
        assert!(!*cancel_rx.borrow());
    }

    #[test]
    fn assistant_sentinels_are_never_exposed() {
        assert_eq!(
            redact_internal_assistant_text(crate::state::pending_placeholder()),
            ""
        );
        assert_eq!(
            redact_internal_assistant_text(crate::state::interrupted_text()),
            ""
        );
        let combined = format!("before {} after", crate::state::interrupted_text());
        let redacted = redact_internal_assistant_text(&combined);
        assert_eq!(redacted, "before  after");
        assert!(!redacted.contains("system-reminder"));
    }

    #[test]
    fn persisted_meme_assets_hide_their_descriptive_caption() {
        let asset = ImageAsset {
            asset_id: "img_test".to_string(),
            turn_id: "turn_test".to_string(),
            tool_id: Some("tool_test".to_string()),
            mime: "image/png".to_string(),
            width: 64,
            height: 64,
            alt: "猫猫 开心 & <得意>".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let reports = vec![
            "<sent_meme>发送了一个表情包：id=sha256:test；description=猫猫 开心 &amp; &lt;得意&gt;</sent_meme>"
                .to_string(),
        ];

        assert!(meme_asset_caption_hidden(&asset, &reports));
        assert!(!meme_asset_caption_hidden(
            &asset,
            &["normal tool output".to_string()]
        ));
    }

    #[test]
    fn cookie_parser_matches_an_exact_cookie_name() {
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static("other=1; miyu_session=secret-token; suffix=2"),
        );
        assert_eq!(cookie_value(&headers, AUTH_COOKIE), Some("secret-token"));
        assert_eq!(cookie_value(&headers, "session"), None);
    }

    #[test]
    fn origin_check_accepts_absent_or_current_host_origin() {
        let mut headers = HeaderMap::new();
        assert!(origin_is_allowed(&headers));
        headers.insert(HOST, HeaderValue::from_static("192.168.1.20:4096"));
        headers.insert(ORIGIN, HeaderValue::from_static("http://127.0.0.1:4096"));
        assert!(!origin_is_allowed(&headers));
        headers.insert(ORIGIN, HeaderValue::from_static("http://192.168.1.20:4096"));
        assert!(origin_is_allowed(&headers));
        headers.append(ORIGIN, HeaderValue::from_static("http://192.168.1.20:4096"));
        assert!(!origin_is_allowed(&headers));
    }

    #[test]
    fn optional_password_auth_issues_server_side_sessions_and_limits_failures() {
        let disabled = WebAuth::new(None);
        assert!(disabled.is_authenticated(None));

        let auth = WebAuth::new(Some("correct horse"));
        let peer = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert!(!auth.is_authenticated(None));
        assert!(matches!(
            auth.login(peer, "wrong"),
            Err(LoginFailure::Invalid)
        ));
        let token = auth.login(peer, "correct horse").unwrap();
        assert!(auth.is_authenticated(Some(&token)));

        let limited = WebAuth::new(Some("secret"));
        for _ in 0..LOGIN_ATTEMPT_LIMIT {
            assert!(matches!(
                limited.login(peer, "wrong"),
                Err(LoginFailure::Invalid)
            ));
        }
        assert!(matches!(
            limited.login(peer, "secret"),
            Err(LoginFailure::RateLimited)
        ));
    }

    #[test]
    fn model_selection_rejects_empty_and_duplicate_pools() {
        assert!(validate_model_selection(Vec::new()).is_err());
        let model = ActiveProviderModelConfig {
            provider_id: "provider".to_string(),
            model: "model".to_string(),
        };
        assert!(validate_model_selection(vec![model.clone()]).is_ok());
        assert!(validate_model_selection(vec![model.clone(), model]).is_err());
    }

    #[test]
    fn config_response_never_serializes_secret_values() {
        let mut config = AppConfig::default();
        config.providers[0].api_key = Some("provider-secret".to_string());
        config.plugins.web.tavily_api_keys = vec!["tavily-secret".to_string()];
        config.plugins.exchange_rate.api_key = "exchange-secret".to_string();
        config.plugins.image_generation.api_keys = vec!["image-secret".to_string()];
        let paths = tempfile::tempdir().unwrap();
        let paths = MiyuPaths {
            config_dir: paths.path().join("config"),
            config_file: paths.path().join("config/config.jsonc"),
            skills_dir: paths.path().join("config/skills"),
            data_dir: paths.path().join("data"),
            cache_dir: paths.path().join("cache"),
            state_dir: paths.path().join("state"),
            pictures_dir: paths.path().join("pictures"),
            fish_hook_file: paths.path().join("fish"),
            bash_hook_file: paths.path().join("bash"),
            zsh_hook_file: paths.path().join("zsh"),
            scripts_dir: paths.path().join("scripts"),
            system_scripts_dir: paths.path().join("system-scripts"),
        };
        let response = config_response(
            &config,
            ContextSnapshot {
                tokens: 0,
                window: None,
                cumulative_tokens: 0,
            },
            &paths,
        )
        .unwrap();
        let serialized = serde_json::to_string(&response).unwrap();
        assert!(!serialized.contains("provider-secret"));
        assert!(!serialized.contains("tavily-secret"));
        assert!(!serialized.contains("exchange-secret"));
        assert!(!serialized.contains("image-secret"));
        assert_eq!(response.secret_states["providers.0.api_key"], true);
        assert_eq!(response.secret_states["plugins.web.tavily_api_keys"], true);
        assert!(response.config.get("memory").is_some());
    }

    #[test]
    fn omitted_provider_secret_does_not_follow_array_position_after_rename() {
        let mut current = AppConfig::default();
        current.providers[0].id = "first".to_string();
        current.providers[0].api_key = Some("first-secret".to_string());
        let mut candidate = current.clone();
        candidate.providers[0].id = "renamed".to_string();
        candidate.providers[0].api_key = None;
        restore_config_secrets(&mut candidate, &current, &HashMap::new()).unwrap();
        assert_eq!(candidate.providers[0].api_key, None);
    }

    #[test]
    fn explicit_secret_clear_removes_a_provider_key() {
        let mut current = AppConfig::default();
        current.providers[0].api_key = Some("secret".to_string());
        let mut candidate = current.clone();
        candidate.providers[0].api_key = None;
        let mutations = HashMap::from([("providers.0.api_key".to_string(), SecretMutation::Clear)]);
        restore_config_secrets(&mut candidate, &current, &mutations).unwrap();
        assert_eq!(candidate.providers[0].api_key, None);
    }

    #[test]
    fn stale_event_cursor_receives_resync_marker() {
        let events = EventHub::new();
        for index in 0..=EVENT_CAPACITY {
            events.publish("test", json!({ "index": index }));
        }
        let replay = events.replay_after(0);
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].kind, "resync_required");
        assert_eq!(replay[0].id, events.latest_id());
        let next = events.publish("after-resync", json!({}));
        assert!(next > replay[0].id);
    }

    #[test]
    fn replay_after_cursor_is_ordered_and_exclusive() {
        let events = EventHub::new();
        events.publish("one", json!({}));
        events.publish("two", json!({}));
        events.publish("three", json!({}));
        let replay = events.replay_after(1);
        assert_eq!(
            replay.iter().map(|record| record.id).collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn future_event_cursor_requests_resync_after_server_restart() {
        let events = EventHub::new();
        let replay = events.replay_after(42);
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].kind, "resync_required");
    }

    #[test]
    fn answer_validation_trims_values_and_rejects_control_characters() {
        let request = sample_question();
        assert_eq!(
            normalize_answers(&request, vec![vec!["  All  ".to_string()]]).unwrap(),
            vec![vec!["All".to_string()]]
        );
        assert!(normalize_answers(&request, vec![vec!["bad\nanswer".to_string()]]).is_err());
    }

    #[test]
    fn invalid_answer_keeps_question_pending() {
        let broker = QuestionBroker::new();
        let (responder, mut response) = oneshot::channel();
        let question_id = broker.insert("run_test", sample_question(), responder);
        let invalid = broker.answer(&question_id, vec![Vec::new()], |_, _| {
            panic!("invalid answer must not be published")
        });
        assert!(matches!(invalid, Err(AnswerFailure::Invalid(_))));
        assert!(broker.pending.lock().unwrap().contains_key(&question_id));

        broker
            .answer(
                &question_id,
                vec![vec![" All ".to_string()]],
                |run_id, answers| {
                    assert_eq!(run_id, "run_test");
                    assert_eq!(answers, &vec![vec!["All".to_string()]]);
                },
            )
            .unwrap();
        assert!(matches!(
            response.try_recv().unwrap(),
            QuestionResponse::Answered(answers) if answers == vec![vec!["All".to_string()]]
        ));
    }

    #[test]
    fn closed_question_responder_does_not_publish_an_answer() {
        let broker = QuestionBroker::new();
        let (responder, response) = oneshot::channel();
        drop(response);
        let question_id = broker.insert("run_test", sample_question(), responder);
        let mut published = false;
        let result = broker.answer(&question_id, vec![vec!["All".to_string()]], |_, _| {
            published = true
        });
        assert!(matches!(result, Err(AnswerFailure::Gone)));
        assert!(!published);
    }

    fn sample_question() -> QuestionRequest {
        QuestionRequest {
            questions: vec![QuestionPrompt {
                header: "Scope".to_string(),
                question: "Which scope?".to_string(),
                options: vec![QuestionOption {
                    label: "All".to_string(),
                    description: String::new(),
                }],
                multiple: false,
                custom: true,
            }],
        }
    }

    #[test]
    fn content_limit_counts_characters() {
        assert!(validate_content("x".repeat(MAX_CONTENT_CHARS)).is_ok());
        let error = validate_content("界".repeat(MAX_CONTENT_CHARS + 1)).unwrap_err();
        assert_eq!(error.status, StatusCode::PAYLOAD_TOO_LARGE);
    }
}
