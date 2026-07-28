//! IM platform bridges.
//!
//! This module is the platform-neutral core: turn driving against the
//! agent actor, session resolution, rate limiting and reply shaping.
//! Each protocol lives in its own submodule (`onebot` = NapCat / QQ);
//! later platforms (Telegram, QQ official, WeChat) add submodules and
//! reuse everything here without touching the web core.

mod assets;
pub(crate) mod commands;
pub(crate) mod onebot;
pub(crate) mod plugins;
mod tool;
mod types;

pub(crate) use types::{
    ConversationKind, ForwardNode, OutboundBody, OutboundMessage, OutboundOrigin, OutboundSegment,
    PlatformAdapter, PlatformConversation, SendReceipt,
};

use crate::agent::AgentMode;
use crate::config::{ActiveProviderModelConfig, AppConfig};
use crate::ipc::ImageAttachment;
use crate::state::{PlatformSessionBindingKey, QueuedPromptAttachment, StateStore};
use crate::web::{
    enqueue_running_prompt, publish_queued_prompt, random_id, validate_content, ActorCommand,
    DaemonState, IpcRunGuard, RunInfo,
};
use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// Hard ceiling for one platform-driven turn; beyond this the run is
/// cancelled so a wedged turn cannot pin the bridge task forever.
const PLATFORM_TURN_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const RATE_WINDOW: Duration = Duration::from_secs(60);
const MAX_CONCURRENT_PLATFORM_TURNS: usize = 16;

/// Shared state for all IM bridges, hung off `DaemonState`. Cheap to clone;
/// everything inside is reference counted.
#[derive(Clone)]
pub(crate) struct PlatformRuntime {
    http: Arc<OnceLock<std::result::Result<reqwest::Client, String>>>,
    pub(crate) onebot: Arc<Mutex<onebot::ConnectionRegistry>>,
    pub(crate) qq_listener: onebot::QqListenerManager,
    pub(crate) rate: Arc<Mutex<RateWindow>>,
    plugins: Arc<OnceLock<std::result::Result<Arc<plugins::PlatformPluginRegistry>, String>>>,
    pub(crate) assets: assets::AssetLeaseStore,
    pub(crate) turn_permits: Arc<tokio::sync::Semaphore>,
    pub(crate) file_store_lock: Arc<tokio::sync::Mutex<()>>,
}

impl PlatformRuntime {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self {
            http: Arc::new(OnceLock::new()),
            onebot: Arc::new(Mutex::new(onebot::ConnectionRegistry::default())),
            qq_listener: onebot::QqListenerManager::default(),
            rate: Arc::new(Mutex::new(RateWindow::new())),
            plugins: Arc::new(OnceLock::new()),
            assets: assets::AssetLeaseStore::new(),
            turn_permits: Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_PLATFORM_TURNS)),
            file_store_lock: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    pub(crate) fn http_client(&self) -> Result<reqwest::Client> {
        self.http
            .get_or_init(|| {
                reqwest::Client::builder()
                    .connect_timeout(Duration::from_secs(10))
                    .build()
                    .map_err(|error| error.to_string())
            })
            .as_ref()
            .cloned()
            .map_err(|error| anyhow!("building the IM platform HTTP client: {error}"))
    }

    pub(crate) fn plugins(&self) -> Result<Arc<plugins::PlatformPluginRegistry>> {
        self.plugins
            .get_or_init(|| {
                plugins::PlatformPluginRegistry::built_in()
                    .map(Arc::new)
                    .map_err(|error| error.to_string())
            })
            .as_ref()
            .cloned()
            .map_err(|error| anyhow!("building the IM platform plugin registry: {error}"))
    }
}

pub(crate) use assets::platform_asset;

#[derive(Clone, Default)]
pub(crate) struct TurnProfile {
    pub(crate) text_models: Option<Vec<ActiveProviderModelConfig>>,
    pub(crate) multimodal_models: Option<Vec<ActiveProviderModelConfig>>,
    pub(crate) system_context: Vec<String>,
    pub(crate) platform: Option<Arc<PlatformTurnContext>>,
}

pub(crate) struct PlatformTurnContext {
    pub(crate) conversation: PlatformConversation,
    pub(crate) sender_id: String,
    pub(crate) sender_display_name: String,
    pub(crate) is_admin: bool,
    pub(crate) config: AppConfig,
    pub(crate) state_store: StateStore,
    adapter: Arc<dyn PlatformAdapter>,
    plugins: Arc<plugins::PlatformPluginRegistry>,
    pending_final_reply_suppression: AtomicBool,
}

impl PlatformTurnContext {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        conversation: PlatformConversation,
        sender_id: String,
        sender_display_name: String,
        is_admin: bool,
        config: AppConfig,
        state_store: StateStore,
        adapter: Arc<dyn PlatformAdapter>,
        plugins: Arc<plugins::PlatformPluginRegistry>,
    ) -> Self {
        Self {
            conversation,
            sender_id,
            sender_display_name,
            is_admin,
            config,
            state_store,
            adapter,
            plugins,
            pending_final_reply_suppression: AtomicBool::new(false),
        }
    }

    pub(crate) fn plugin_enabled(&self, id: &str, default_enabled: bool) -> bool {
        self.config
            .platforms
            .qq
            .plugins
            .get(id)
            .and_then(|plugin| plugin.enabled)
            .unwrap_or(default_enabled)
    }

    pub(crate) fn host_tools_allowed(&self) -> bool {
        if self.is_admin {
            return true;
        }
        self.conversation.kind == ConversationKind::Private
            && self.config.platforms.qq.allow_non_admin_host_tools
            && self.sender_id.parse::<i64>().ok().is_some_and(|sender| {
                self.config
                    .platforms
                    .qq
                    .private_chats
                    .whitelist
                    .contains(&sender)
            })
    }

    pub(crate) async fn handle_command(&self, text: &str) -> Option<OutboundMessage> {
        self.plugins.handle_command(self, text).await
    }

    pub(crate) async fn prepare_turn(&self, content: String) -> plugins::PlatformTurnInput {
        let mut input = plugins::PlatformTurnInput {
            content,
            system_context: Vec::new(),
        };
        self.plugins.before_turn(self, &mut input).await;
        input
    }

    pub(crate) async fn send(&self, message: OutboundMessage) -> Result<SendReceipt> {
        let prepared = self.plugins.before_send(self, message).await;
        let primary = prepared.primary.clone();
        let receipt = match self.adapter.send(prepared.primary).await {
            Ok(receipt) => receipt,
            Err(error) => match prepared.fallback {
                Some(fallback) => {
                    tracing::warn!(error = %error, "transformed platform message failed; sending fallback");
                    return self.adapter.send(fallback).await;
                }
                None => return Err(error),
            },
        };
        self.plugins.after_send(self, &primary, &receipt).await;
        for message in prepared.after_success {
            if let Err(error) = self.adapter.send(message).await {
                tracing::warn!(error = %error, "platform plugin follow-up send failed");
            }
        }
        if prepared.suppress_final_reply && primary.origin == OutboundOrigin::Tool {
            self.pending_final_reply_suppression
                .store(true, Ordering::Release);
        }
        Ok(receipt)
    }

    pub(crate) async fn send_bypass_plugins(
        &self,
        message: OutboundMessage,
    ) -> Result<SendReceipt> {
        self.adapter.send(message).await
    }

    pub(crate) async fn bot_display_name(&self) -> Result<String> {
        self.adapter.bot_display_name().await
    }

    pub(crate) fn take_final_reply_suppression(&self) -> bool {
        self.pending_final_reply_suppression
            .swap(false, Ordering::AcqRel)
    }
}

pub(crate) fn register_platform_tools(
    registry: &mut crate::tools::ToolRegistry,
    context: Arc<PlatformTurnContext>,
) {
    tool::register(registry, context);
}

/// Fixed one-minute-window rate limiter shared by all platforms. Empty
/// whitelists allow everyone, so this is the backstop against strangers
/// (or an accidental bot loop) draining the LLM quota.
pub(crate) struct RateWindow {
    window_start: Instant,
    conversations: HashMap<String, SenderWindow>,
}

#[derive(Default)]
struct SenderWindow {
    count: u32,
    notified: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RateDecision {
    Allow,
    /// Over quota and already warned this window.
    DropSilently,
    /// Over quota for the first time this window: send one notice.
    DropWithNotice,
}

impl RateWindow {
    pub(crate) fn new() -> Self {
        Self {
            window_start: Instant::now(),
            conversations: HashMap::new(),
        }
    }

    pub(crate) fn check(&mut self, conversation: &str, limit: u32) -> RateDecision {
        self.check_at(Instant::now(), conversation, limit)
    }

    fn check_at(&mut self, now: Instant, conversation: &str, limit: u32) -> RateDecision {
        if now.duration_since(self.window_start) >= RATE_WINDOW {
            self.window_start = now;
            self.conversations.clear();
        }
        if limit == 0 {
            return RateDecision::Allow;
        }
        let entry = self
            .conversations
            .entry(conversation.to_string())
            .or_default();
        if entry.count < limit {
            entry.count += 1;
            return RateDecision::Allow;
        }
        if entry.notified {
            return RateDecision::DropSilently;
        }
        entry.notified = true;
        RateDecision::DropWithNotice
    }
}

/// Finds or creates the dedicated user session for a stable external
/// conversation identity. The visible session name can be edited freely;
/// routing never depends on it after the binding has been created.
pub(crate) fn resolve_platform_session(
    state: &DaemonState,
    conversation: &PlatformConversation,
    participant_id: Option<String>,
    name: &str,
    legacy_name: Option<&str>,
) -> Result<Arc<str>> {
    let persona = state.manager.lock().unwrap().config.active_persona_scope();
    let key = PlatformSessionBindingKey {
        platform: conversation.platform.clone(),
        account_id: conversation.account_id.clone(),
        conversation_kind: conversation.kind.as_str().to_string(),
        conversation_id: conversation.conversation_id.clone(),
        participant_id,
        persona: persona.clone(),
    };
    if let Some(session_id) = state.state_store.find_platform_session_binding(&key)? {
        let record = state
            .state_store
            .session_record(&session_id)?
            .with_context(|| format!("bound platform session is missing: {session_id}"))?;
        if record.archived {
            state
                .state_store
                .set_session_archived(&record.session_id, false)?;
        }
        return Ok(record.session_id.into());
    }

    // Adopt the pre-binding name only when it identifies exactly one session.
    // If multiple bot accounts race for the same legacy name, the first bind
    // wins and every later account gets a fresh, correctly isolated session.
    let mut candidates = state
        .state_store
        .list_sessions(&persona, true)?
        .into_iter()
        .filter(|overview| {
            overview.record.kind == "user"
                && (overview.record.name == name
                    || legacy_name.is_some_and(|legacy| overview.record.name == legacy))
        })
        .map(|overview| overview.record)
        .collect::<Vec<_>>();
    if candidates.len() == 1 {
        let record = candidates.pop().expect("length checked");
        match state
            .state_store
            .claim_platform_session(&key, &record.session_id)
        {
            Ok(session_id) if session_id == record.session_id => {
                if record.archived {
                    state
                        .state_store
                        .set_session_archived(&record.session_id, false)?;
                }
                return Ok(record.session_id.into());
            }
            Ok(session_id) => return Ok(session_id.into()),
            Err(error) => {
                tracing::warn!(error = %error, session_id = %record.session_id, "legacy platform session could not be bound");
                if let Some(session_id) = state.state_store.find_platform_session_binding(&key)? {
                    return Ok(session_id.into());
                }
            }
        }
    } else if candidates.len() > 1 {
        tracing::warn!(
            name,
            "legacy platform session name is ambiguous; creating a new session"
        );
    }

    let record = state
        .state_store
        .create_session(&persona, name, "user", None)?;
    match state
        .state_store
        .claim_platform_session(&key, &record.session_id)
    {
        Ok(session_id) if session_id != record.session_id => {
            let _ = state.state_store.delete_session(&record.session_id);
            return Ok(session_id.into());
        }
        Ok(_) => {}
        Err(error) => {
            let _ = state.state_store.delete_session(&record.session_id);
            return Err(error);
        }
    }
    state.events.publish(
        "session.created",
        serde_json::json!({
            "session_id": record.session_id,
            "name": record.name,
            "platform": conversation.platform,
            "account_id": conversation.account_id,
            "conversation_kind": conversation.kind.as_str(),
            "conversation_id": conversation.conversation_id,
        }),
    );
    Ok(record.session_id.into())
}

pub(crate) struct TurnOutcome {
    pub(crate) text: String,
    /// Image asset ids published during the turn (`tool.image` events);
    /// bridges load the bytes and re-send them platform-natively.
    pub(crate) image_assets: Vec<String>,
    /// Byte ranges produced after confirmed direct long-image tool sends.
    /// A queued prompt closes the current range before its own answer starts,
    /// so direct-send acknowledgements are removed without losing later turns.
    pub(crate) suppressed_reply_ranges: Vec<(usize, usize)>,
    /// The last response segment was delivered by a successful direct tool
    /// send, so an otherwise empty platform reply must not add a placeholder.
    pub(crate) final_reply_already_sent: bool,
}

#[derive(Default)]
struct ReplySuppression {
    ranges: Vec<(usize, usize)>,
    open_at: Option<usize>,
    final_reply_already_sent: bool,
}

impl ReplySuppression {
    fn direct_send_succeeded(&mut self, text_len: usize) {
        self.open_at.get_or_insert(text_len);
        self.final_reply_already_sent = true;
    }

    fn queued_prompt_consumed(&mut self, text_len: usize) {
        self.close_range(text_len);
        // The direct send answered the preceding prompt, not the newly
        // consumed one. Preserve its reply, including an empty placeholder.
        self.final_reply_already_sent = false;
    }

    fn finish(mut self, text_len: usize) -> (Vec<(usize, usize)>, bool) {
        self.close_range(text_len);
        (self.ranges, self.final_reply_already_sent)
    }

    fn close_range(&mut self, text_len: usize) {
        if let Some(start) = self.open_at.take() {
            if start < text_len {
                self.ranges.push((start, text_len));
            }
        }
    }
}

pub(crate) enum TurnDispatch {
    Completed(TurnOutcome),
    /// The target session already had a running turn; the message was
    /// queued as a follow-up and its answer arrives with that run.
    Queued,
    Failed(String),
}

/// Drives one agent turn for an inbound IM message and waits for the
/// final result. Mirrors `handle_ipc_turn`, minus the client stream.
pub(crate) async fn run_platform_turn(
    state: &DaemonState,
    session_id: Arc<str>,
    content: String,
    images: Vec<Option<ImageAttachment>>,
    profile: TurnProfile,
) -> Result<TurnDispatch> {
    let content = validate_content(content).map_err(|error| anyhow!(error.message))?;
    state.state_store.recover_stale_turns()?;
    let target = state.state_store.pinned(&session_id);
    if target.has_running_turns()? {
        let attachments = queued_prompt_attachments(&images);
        let (run_id, turn_id, prompt) =
            enqueue_running_prompt(state, &target, &session_id, &content, &attachments)
                .map_err(|error| anyhow!(error.message))?;
        publish_queued_prompt(state, run_id.as_deref(), turn_id.as_deref(), &prompt);
        return Ok(TurnDispatch::Queued);
    }

    let run_id = random_id("run", 18);
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    {
        let mut manager = state.manager.lock().unwrap();
        if manager.admin_busy {
            bail!("Miyu is busy with another operation");
        }
        manager.active_runs.insert(
            run_id.clone(),
            RunInfo {
                session_id: session_id.clone(),
                mode: AgentMode::Normal,
                cancel: cancel_tx,
            },
        );
    }
    let after = state.events.latest_id();
    let mut subscription = state.events.subscribe_after(after);
    let platform_context = profile.platform.clone();
    if state
        .actor_tx
        .send(ActorCommand::StartTurn {
            run_id: run_id.clone(),
            session_id,
            content,
            mode: AgentMode::Normal,
            images,
            cwd: None,
            profile: Some(profile),
            cancel: cancel_rx,
        })
        .is_err()
    {
        crate::web::finish_run(&state.manager, &run_id, None);
        bail!("Miyu core worker is unavailable");
    }
    // Cancels the run if this task dies before the turn settles.
    let mut run_guard = IpcRunGuard {
        manager: state.manager.clone(),
        run_id: run_id.clone(),
        finished: false,
    };

    let deadline = tokio::time::Instant::now() + PLATFORM_TURN_TIMEOUT;
    let mut text = String::new();
    let mut image_assets = Vec::new();
    let mut reply_suppression = ReplySuppression::default();
    let mut last_id = after;
    let dispatch = loop {
        let record = if let Some(record) = subscription.pending.pop_front() {
            record
        } else {
            match tokio::time::timeout_at(deadline, subscription.receiver.recv()).await {
                Err(_) => {
                    break TurnDispatch::Failed(
                        crate::i18n::text("the reply timed out", "回复超时，本轮已取消")
                            .to_string(),
                    );
                }
                Ok(Ok(record)) => record,
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => {
                    subscription.pending = state.events.replay_after(last_id);
                    continue;
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => {
                    break TurnDispatch::Failed(
                        crate::i18n::text("Miyu core stopped", "Miyu 核心已停止").to_string(),
                    );
                }
            }
        };
        if record.kind == "resync_required" {
            break TurnDispatch::Failed(
                crate::i18n::text(
                    "event history was exhausted; the turn was cancelled",
                    "事件缓冲耗尽，本轮已取消",
                )
                .to_string(),
            );
        }
        last_id = record.id;
        let Ok(data) = serde_json::from_str::<Value>(&record.data) else {
            continue;
        };
        if data.get("run_id").and_then(Value::as_str) != Some(run_id.as_str()) {
            continue;
        }
        match record.kind.as_str() {
            "assistant.delta" => {
                if let Some(delta) = data.get("delta").and_then(Value::as_str) {
                    text.push_str(delta);
                }
            }
            "tool.image" => {
                if let Some(id) = data
                    .get("asset")
                    .and_then(|asset| asset.get("id"))
                    .and_then(Value::as_str)
                {
                    image_assets.push(id.to_string());
                }
            }
            "tool.finished" => {
                let direct_send_succeeded = data.get("name").and_then(Value::as_str)
                    == Some("send_message_to_user")
                    && data.get("ok").and_then(Value::as_bool) == Some(true)
                    && platform_context
                        .as_ref()
                        .is_some_and(|context| context.take_final_reply_suppression());
                if direct_send_succeeded {
                    reply_suppression.direct_send_succeeded(text.len());
                }
            }
            "queue.consumed" => {
                reply_suppression.queued_prompt_consumed(text.len());
            }
            "run.completed" => {
                run_guard.finish();
                let (suppressed_reply_ranges, final_reply_already_sent) =
                    reply_suppression.finish(text.len());
                break TurnDispatch::Completed(TurnOutcome {
                    text,
                    image_assets,
                    suppressed_reply_ranges,
                    final_reply_already_sent,
                });
            }
            "run.failed" => {
                run_guard.finish();
                let message = data
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
                    .to_string();
                break TurnDispatch::Failed(message);
            }
            "run.cancelled" => {
                run_guard.finish();
                break TurnDispatch::Failed(
                    crate::i18n::text("the turn was cancelled", "本轮被取消了").to_string(),
                );
            }
            _ => {}
        }
    };
    Ok(dispatch)
}

fn queued_prompt_attachments(images: &[Option<ImageAttachment>]) -> Vec<QueuedPromptAttachment> {
    use base64::Engine as _;
    images
        .iter()
        .filter_map(|image| match image {
            Some(ImageAttachment::Binary { mime, data }) => Some(QueuedPromptAttachment::Binary {
                mime: mime.clone(),
                data_base64: base64::engine::general_purpose::STANDARD.encode(data),
            }),
            Some(ImageAttachment::Path { path }) => {
                Some(QueuedPromptAttachment::Path { path: path.clone() })
            }
            None => None,
        })
        .collect()
}

/// Strips markdown decoration for plain-text IM surfaces (QQ renders no
/// markup). Deliberately conservative: fenced code bodies are kept
/// verbatim, single `*` stays (could be math), lists and newlines stay.
pub(crate) fn markdown_to_plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let stripped = if trimmed.starts_with('#') {
            trimmed.trim_start_matches('#').trim_start()
        } else if let Some(rest) = trimmed.strip_prefix("> ") {
            rest
        } else {
            line
        };
        out.push_str(&strip_inline_markup(stripped));
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// Removes `**`, `__`, backticks and rewrites `[text](url)` → `text (url)`.
fn strip_inline_markup(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if chars.get(i + 1) == Some(&'*') => i += 2,
            '_' if chars.get(i + 1) == Some(&'_') => i += 2,
            '`' => i += 1,
            '[' => {
                // Try [text](url); anything else is emitted verbatim.
                let close = chars[i + 1..].iter().position(|&c| c == ']');
                let parsed = close.and_then(|offset| {
                    let close = i + 1 + offset;
                    if chars.get(close + 1) == Some(&'(') {
                        let end = chars[close + 2..].iter().position(|&c| c == ')');
                        end.map(|len| {
                            let text: String = chars[i + 1..close].iter().collect();
                            let url: String = chars[close + 2..close + 2 + len].iter().collect();
                            (close + 2 + len + 1, text, url)
                        })
                    } else {
                        None
                    }
                });
                match parsed {
                    Some((next, text, url)) => {
                        out.push_str(&text);
                        if !url.is_empty() && url != text {
                            out.push_str(" (");
                            out.push_str(&url);
                            out.push(')');
                        }
                        i = next;
                    }
                    None => {
                        out.push('[');
                        i += 1;
                    }
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// Splits an over-long reply on paragraph, then line, then raw char
/// boundaries. Char-based so CJK never gets cut mid-codepoint.
pub(crate) fn split_reply(text: &str, max_chars: usize) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    if max_chars == 0 || text.chars().count() <= max_chars {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_chars = 0;
    let flush = |current: &mut String, current_chars: &mut usize, chunks: &mut Vec<String>| {
        let piece = current.trim();
        if !piece.is_empty() {
            chunks.push(piece.to_string());
        }
        current.clear();
        *current_chars = 0;
    };
    for paragraph in text.split("\n\n") {
        let unit_chars = paragraph.chars().count();
        if unit_chars > max_chars {
            flush(&mut current, &mut current_chars, &mut chunks);
            // Oversized paragraph: pack by lines, hard-split huge lines.
            for line in paragraph.lines() {
                let line_chars = line.chars().count();
                if line_chars > max_chars {
                    flush(&mut current, &mut current_chars, &mut chunks);
                    let mut buffer = String::new();
                    let mut count = 0;
                    for c in line.chars() {
                        buffer.push(c);
                        count += 1;
                        if count == max_chars {
                            chunks.push(buffer.clone());
                            buffer.clear();
                            count = 0;
                        }
                    }
                    if !buffer.trim().is_empty() {
                        chunks.push(buffer.trim().to_string());
                    }
                    continue;
                }
                if current_chars + line_chars + 1 > max_chars {
                    flush(&mut current, &mut current_chars, &mut chunks);
                }
                if !current.is_empty() {
                    current.push('\n');
                    current_chars += 1;
                }
                current.push_str(line);
                current_chars += line_chars;
            }
            flush(&mut current, &mut current_chars, &mut chunks);
            continue;
        }
        if current_chars + unit_chars + 2 > max_chars {
            flush(&mut current, &mut current_chars, &mut chunks);
        }
        if !current.is_empty() {
            current.push_str("\n\n");
            current_chars += 2;
        }
        current.push_str(paragraph);
        current_chars += unit_chars;
    }
    flush(&mut current, &mut current_chars, &mut chunks);
    chunks
}

/// Sniffs the mime type of downloaded image bytes by magic numbers.
pub(crate) fn sniff_image_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.len() > 11 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "image/png"
    }
}

/// Downloads a URL with a byte cap enforced while streaming, so an
/// oversized (or length-less) body can never balloon memory.
pub(crate) async fn download_capped(
    client: &reqwest::Client,
    url: &str,
    max_bytes: usize,
    timeout: Duration,
) -> Result<(Vec<u8>, Option<String>)> {
    let response = client
        .get(url)
        .timeout(timeout)
        .send()
        .await
        .with_context(|| format!("requesting {url}"))?
        .error_for_status()
        .with_context(|| format!("downloading {url}"))?;
    if let Some(length) = response.content_length() {
        if length as usize > max_bytes {
            bail!(
                "the file is larger than the {}MB limit",
                max_bytes / 1024 / 1024
            );
        }
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| format!("reading {url}"))?;
        if bytes.len() + chunk.len() > max_bytes {
            bail!(
                "the file is larger than the {}MB limit",
                max_bytes / 1024 / 1024
            );
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((bytes, content_type))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::MiyuPaths;
    use futures_util::future::BoxFuture;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    struct SuppressingToolPlugin;

    impl plugins::PlatformPlugin for SuppressingToolPlugin {
        fn descriptor(&self) -> plugins::PluginDescriptor {
            plugins::PluginDescriptor {
                id: "test_suppress",
                priority: 1,
                default_enabled: true,
            }
        }

        fn before_send<'a>(
            &'a self,
            _context: &'a PlatformTurnContext,
            message: OutboundMessage,
        ) -> BoxFuture<'a, Result<plugins::PreparedSend>> {
            Box::pin(async move {
                Ok(plugins::PreparedSend {
                    primary: message.clone(),
                    after_success: Vec::new(),
                    fallback: Some(message),
                    suppress_final_reply: true,
                })
            })
        }
    }

    struct CountingAdapter {
        calls: AtomicUsize,
        fail_first: bool,
    }

    impl PlatformAdapter for CountingAdapter {
        fn send<'a>(&'a self, _message: OutboundMessage) -> BoxFuture<'a, Result<SendReceipt>> {
            Box::pin(async move {
                let call = self.calls.fetch_add(1, AtomicOrdering::Relaxed);
                if self.fail_first && call == 0 {
                    anyhow::bail!("injected primary failure");
                }
                Ok(SendReceipt::default())
            })
        }

        fn bot_display_name<'a>(&'a self) -> BoxFuture<'a, Result<String>> {
            Box::pin(async { Ok("Miyu".to_string()) })
        }
    }

    fn test_turn_context(fail_first: bool) -> (tempfile::TempDir, PlatformTurnContext) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let paths = MiyuPaths {
            config_dir: root.join("config"),
            config_file: root.join("config/config.jsonc"),
            skills_dir: root.join("config/skills"),
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            state_dir: root.join("state"),
            pictures_dir: root.join("pictures"),
            fish_hook_file: root.join("fish/miyu.fish"),
            bash_hook_file: root.join("shell/bash-hook.sh"),
            zsh_hook_file: root.join("shell/zsh-hook.zsh"),
            scripts_dir: root.join("config/scripts"),
            system_scripts_dir: PathBuf::new(),
        };
        let context = PlatformTurnContext::new(
            PlatformConversation {
                platform: "onebot".to_string(),
                account_id: "10000".to_string(),
                kind: ConversationKind::Private,
                conversation_id: "20000".to_string(),
            },
            "20000".to_string(),
            "tester".to_string(),
            false,
            AppConfig::default(),
            StateStore::new(&paths).unwrap(),
            Arc::new(CountingAdapter {
                calls: AtomicUsize::new(0),
                fail_first,
            }),
            Arc::new(plugins::PlatformPluginRegistry::new(vec![Arc::new(
                SuppressingToolPlugin,
            )])),
        );
        (temp, context)
    }

    #[test]
    fn rate_window_allows_then_drops_with_single_notice() {
        let mut window = RateWindow::new();
        let start = Instant::now();
        for _ in 0..3 {
            assert_eq!(window.check_at(start, "group:1", 3), RateDecision::Allow);
        }
        assert_eq!(
            window.check_at(start, "group:1", 3),
            RateDecision::DropWithNotice
        );
        assert_eq!(
            window.check_at(start, "group:1", 3),
            RateDecision::DropSilently
        );
        // Another conversation is unaffected by the first group's quota.
        assert_eq!(window.check_at(start, "group:2", 3), RateDecision::Allow);
        // The window resets after a minute.
        let later = start + Duration::from_secs(61);
        assert_eq!(window.check_at(later, "group:1", 3), RateDecision::Allow);
    }

    #[test]
    fn rate_window_zero_is_unlimited() {
        let mut unlimited = RateWindow::new();
        let start = Instant::now();
        for i in 0..100 {
            assert_eq!(
                unlimited.check_at(start, &format!("group:{i}"), 0),
                RateDecision::Allow
            );
        }
    }

    #[test]
    fn markdown_to_plain_strips_decoration_keeps_content() {
        let input = "# 标题\n\n**加粗** 与 `代码` 和 [链接](https://a.b)\n\n```rust\nlet x = 1; // **不动**\n```\n\n- 列表项\n> 引用";
        let plain = markdown_to_plain(input);
        assert_eq!(
            plain,
            "标题\n\n加粗 与 代码 和 链接 (https://a.b)\n\nlet x = 1; // **不动**\n\n- 列表项\n引用"
        );
    }

    #[test]
    fn markdown_link_edge_cases() {
        assert_eq!(strip_inline_markup("[a](b"), "[a](b");
        assert_eq!(strip_inline_markup("纯 [文本] 括号"), "纯 [文本] 括号");
        // Identical text/url collapses to one copy.
        assert_eq!(
            strip_inline_markup("[https://x.y](https://x.y)"),
            "https://x.y"
        );
    }

    #[test]
    fn split_reply_paragraph_line_and_hard_boundaries() {
        assert_eq!(split_reply("短", 10), vec!["短"]);
        assert!(split_reply("  ", 10).is_empty());
        // 0 disables splitting.
        let long = "a".repeat(50);
        assert_eq!(split_reply(&long, 0), vec![long.clone()]);

        let text = "第一段落。\n\n第二段落。";
        let chunks = split_reply(text, 6);
        assert_eq!(chunks, vec!["第一段落。", "第二段落。"]);

        // CJK hard split never panics and keeps every char.
        let cjk = "汉".repeat(25);
        let chunks = split_reply(&cjk, 10);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks.join(""), cjk);
    }

    #[test]
    fn sniff_image_mime_by_magic() {
        assert_eq!(sniff_image_mime(&[0x89, b'P', b'N', b'G', 0]), "image/png");
        assert_eq!(sniff_image_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), "image/jpeg");
        assert_eq!(sniff_image_mime(b"GIF89a"), "image/gif");
        assert_eq!(sniff_image_mime(b"RIFF\0\0\0\0WEBPVP8 "), "image/webp");
        assert_eq!(sniff_image_mime(b"????"), "image/png");
    }

    #[test]
    fn queued_platform_images_keep_binary_and_path_attachments() {
        let attachments = queued_prompt_attachments(&[
            Some(ImageAttachment::Binary {
                mime: "image/png".to_string(),
                data: b"png".to_vec(),
            }),
            None,
            Some(ImageAttachment::Path {
                path: "/tmp/input.jpg".to_string(),
            }),
        ]);
        assert_eq!(
            attachments,
            vec![
                QueuedPromptAttachment::Binary {
                    mime: "image/png".to_string(),
                    data_base64: "cG5n".to_string(),
                },
                QueuedPromptAttachment::Path {
                    path: "/tmp/input.jpg".to_string(),
                },
            ]
        );
    }

    #[tokio::test]
    async fn direct_final_suppression_requires_primary_send_success() {
        let (_temp, success) = test_turn_context(false);
        success
            .send(OutboundMessage::text(OutboundOrigin::Tool, "sent"))
            .await
            .unwrap();
        assert!(success.take_final_reply_suppression());
        assert!(!success.take_final_reply_suppression());

        let (_temp, fallback) = test_turn_context(true);
        fallback
            .send(OutboundMessage::text(OutboundOrigin::Tool, "fallback"))
            .await
            .unwrap();
        assert!(!fallback.take_final_reply_suppression());
    }

    #[test]
    fn queued_prompt_closes_only_text_already_emitted_after_direct_send() {
        let mut suppression = ReplySuppression::default();
        suppression.direct_send_succeeded(8);
        suppression.queued_prompt_consumed(8);
        let (ranges, already_sent) = suppression.finish(24);
        assert!(ranges.is_empty());
        assert!(!already_sent);

        let mut suppression = ReplySuppression::default();
        suppression.direct_send_succeeded(8);
        suppression.queued_prompt_consumed(14);
        let (ranges, already_sent) = suppression.finish(24);
        assert_eq!(ranges, vec![(8, 14)]);
        assert!(!already_sent);
    }

    #[test]
    fn direct_send_without_later_prompt_covers_an_empty_final_reply() {
        let mut suppression = ReplySuppression::default();
        suppression.direct_send_succeeded(8);
        let (ranges, already_sent) = suppression.finish(8);
        assert!(ranges.is_empty());
        assert!(already_sent);
    }

    #[test]
    fn host_tools_follow_admin_and_private_whitelist_policy() {
        let (_temp, mut context) = test_turn_context(false);
        assert!(!context.host_tools_allowed());
        context.is_admin = true;
        assert!(context.host_tools_allowed());

        context.is_admin = false;
        context.config.platforms.qq.allow_non_admin_host_tools = true;
        assert!(!context.host_tools_allowed());
        context
            .config
            .platforms
            .qq
            .private_chats
            .whitelist
            .push(20_000);
        assert!(context.host_tools_allowed());

        context.conversation.kind = ConversationKind::Group;
        assert!(!context.host_tools_allowed());
    }

    #[test]
    fn untrusted_send_tool_schema_does_not_expose_local_attachments() {
        let (_temp, context) = test_turn_context(false);
        let mut registry = crate::tools::ToolRegistry::new();
        register_platform_tools(&mut registry, Arc::new(context));
        let parameters = &registry.get("send_message_to_user").unwrap().parameters;

        assert!(parameters["properties"].get("text").is_some());
        assert!(parameters["properties"].get("images").is_none());
        assert!(parameters["properties"].get("files").is_none());
    }
}
