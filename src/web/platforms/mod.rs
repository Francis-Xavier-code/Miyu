//! IM platform bridges.
//!
//! This module is the platform-neutral core: turn driving against the
//! agent actor, session resolution, rate limiting and reply shaping.
//! Each protocol lives in its own submodule (`onebot` = NapCat / QQ);
//! later platforms (Telegram, QQ official, WeChat) add submodules and
//! reuse everything here without touching the web core.

pub(crate) mod onebot;

use super::{
    enqueue_running_prompt, publish_queued_prompt, random_id, validate_content, ActorCommand,
    IpcRunGuard, RunInfo, WebState,
};
use crate::agent::AgentMode;
use crate::ipc::ImageAttachment;
use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// Hard ceiling for one platform-driven turn; beyond this the run is
/// cancelled so a wedged turn cannot pin the bridge task forever.
const PLATFORM_TURN_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const RATE_WINDOW: Duration = Duration::from_secs(60);

/// Shared state for all IM bridges, hung off `WebState`. Cheap to clone;
/// everything inside is reference counted.
#[derive(Clone)]
pub(crate) struct PlatformRuntime {
    /// Media downloads (inbound images/files) for every platform.
    pub(crate) http: reqwest::Client,
    pub(crate) onebot: Arc<Mutex<onebot::ConnectionRegistry>>,
    pub(crate) rate: Arc<Mutex<RateWindow>>,
}

impl PlatformRuntime {
    pub(crate) fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .context("building the IM platform HTTP client")?;
        Ok(Self {
            http,
            onebot: Arc::new(Mutex::new(onebot::ConnectionRegistry::default())),
            rate: Arc::new(Mutex::new(RateWindow::new())),
        })
    }
}

/// Fixed one-minute-window rate limiter shared by all platforms. Empty
/// whitelists allow everyone, so this is the backstop against strangers
/// (or an accidental bot loop) draining the LLM quota.
pub(crate) struct RateWindow {
    window_start: Instant,
    global_count: u32,
    global_notified: bool,
    senders: HashMap<String, SenderWindow>,
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
            global_count: 0,
            global_notified: false,
            senders: HashMap::new(),
        }
    }

    pub(crate) fn check(&mut self, sender: &str, per_sender: u32, global: u32) -> RateDecision {
        self.check_at(Instant::now(), sender, per_sender, global)
    }

    fn check_at(
        &mut self,
        now: Instant,
        sender: &str,
        per_sender: u32,
        global: u32,
    ) -> RateDecision {
        if now.duration_since(self.window_start) >= RATE_WINDOW {
            self.window_start = now;
            self.global_count = 0;
            self.global_notified = false;
            self.senders.clear();
        }
        let global_exceeded = global > 0 && self.global_count >= global;
        let entry = self.senders.entry(sender.to_string()).or_default();
        let sender_exceeded = per_sender > 0 && entry.count >= per_sender;
        if !global_exceeded && !sender_exceeded {
            entry.count += 1;
            self.global_count += 1;
            return RateDecision::Allow;
        }
        if sender_exceeded {
            if entry.notified {
                return RateDecision::DropSilently;
            }
            entry.notified = true;
            return RateDecision::DropWithNotice;
        }
        if self.global_notified {
            RateDecision::DropSilently
        } else {
            self.global_notified = true;
            RateDecision::DropWithNotice
        }
    }
}

/// Finds or creates the dedicated user session for an IM conversation
/// (session name = conversation key, e.g. `qq:private:12345`), scoped to
/// the active persona. Archived sessions are revived in place so the
/// conversation history survives housekeeping.
pub(crate) fn resolve_platform_session(state: &WebState, name: &str) -> Result<Arc<str>> {
    let persona = state
        .manager
        .lock()
        .unwrap()
        .config
        .active_persona_scope();
    if let Some(record) = state.state_store.find_session_by_name(&persona, name)? {
        if record.archived {
            state
                .state_store
                .set_session_archived(&record.session_id, false)?;
        }
        return Ok(record.session_id.into());
    }
    let record = state
        .state_store
        .create_session(&persona, name, "user", None)?;
    state.events.publish(
        "session.created",
        serde_json::json!({ "session_id": record.session_id, "name": record.name }),
    );
    Ok(record.session_id.into())
}

pub(crate) struct TurnOutcome {
    pub(crate) text: String,
    /// Image asset ids published during the turn (`tool.image` events);
    /// bridges load the bytes and re-send them platform-natively.
    pub(crate) image_assets: Vec<String>,
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
    state: &WebState,
    session_id: Arc<str>,
    content: String,
    images: Vec<Option<ImageAttachment>>,
) -> Result<TurnDispatch> {
    let content = validate_content(content).map_err(|error| anyhow!(error.message))?;
    state.state_store.recover_stale_turns()?;
    let target = state.state_store.pinned(&session_id);
    if target.has_running_turns()? {
        let (run_id, turn_id, prompt) =
            enqueue_running_prompt(state, &target, &session_id, &content)
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
    if state
        .actor_tx
        .send(ActorCommand::StartTurn {
            run_id: run_id.clone(),
            session_id,
            content,
            mode: AgentMode::Normal,
            images,
            cwd: None,
            cancel: cancel_rx,
        })
        .is_err()
    {
        super::finish_run(&state.manager, &run_id, None);
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
    let mut last_id = after;
    let dispatch = loop {
        let record = if let Some(record) = subscription.pending.pop_front() {
            record
        } else {
            match tokio::time::timeout_at(deadline, subscription.receiver.recv()).await {
                Err(_) => {
                    break TurnDispatch::Failed(
                        crate::i18n::text("the reply timed out", "回复超时，本轮已取消").to_string(),
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
            "run.completed" => {
                run_guard.finish();
                break TurnDispatch::Completed(TurnOutcome { text, image_assets });
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
                            let url: String =
                                chars[close + 2..close + 2 + len].iter().collect();
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
            bail!("the file is larger than the {}MB limit", max_bytes / 1024 / 1024);
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
            bail!("the file is larger than the {}MB limit", max_bytes / 1024 / 1024);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((bytes, content_type))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_window_allows_then_drops_with_single_notice() {
        let mut window = RateWindow::new();
        let start = Instant::now();
        for _ in 0..3 {
            assert_eq!(window.check_at(start, "u1", 3, 10), RateDecision::Allow);
        }
        assert_eq!(window.check_at(start, "u1", 3, 10), RateDecision::DropWithNotice);
        assert_eq!(window.check_at(start, "u1", 3, 10), RateDecision::DropSilently);
        // Another sender is unaffected by u1's quota.
        assert_eq!(window.check_at(start, "u2", 3, 10), RateDecision::Allow);
        // The window resets after a minute.
        let later = start + Duration::from_secs(61);
        assert_eq!(window.check_at(later, "u1", 3, 10), RateDecision::Allow);
    }

    #[test]
    fn rate_window_global_cap_and_unlimited_zero() {
        let mut window = RateWindow::new();
        let start = Instant::now();
        assert_eq!(window.check_at(start, "a", 0, 2), RateDecision::Allow);
        assert_eq!(window.check_at(start, "b", 0, 2), RateDecision::Allow);
        assert_eq!(window.check_at(start, "c", 0, 2), RateDecision::DropWithNotice);
        assert_eq!(window.check_at(start, "d", 0, 2), RateDecision::DropSilently);

        let mut unlimited = RateWindow::new();
        for i in 0..100 {
            assert_eq!(
                unlimited.check_at(start, &format!("u{i}"), 0, 0),
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
}
