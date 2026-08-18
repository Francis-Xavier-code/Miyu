//! 回复文本的整形与分段。
//!
//! 聊天平台不认 Markdown，所以要降成纯文本（`markdown_to_plain`）。
//!
//! `split_reply` 把长回复切成多条发：切点优先落在段落和句子边界上，硬切会把一
//! 句话腰斩。
//!
//! `ReplySuppression` 处理的是「工具已经把这段内容直接发出去了」——正文里就不
//! 该再出现一次。`cut_suppressed_ranges` 按区间裁，因为被抑制的可能只是中间
//! 一段。

use crate::platforms::*;

#[derive(Clone, Copy, Debug)]
pub(crate) struct AdaptiveResponseTargetPolicy {
    pub(crate) position: Option<PlatformMessagePosition>,
    pub(crate) received_at: Instant,
    pub(crate) quote_after_other_messages: u64,
    pub(crate) mention_after: Duration,
}

impl AdaptiveResponseTargetPolicy {
    pub(crate) fn new(
        position: Option<PlatformMessagePosition>,
        received_at: Instant,
        quote_after_other_messages: u64,
        mention_after_seconds: u64,
    ) -> Self {
        Self {
            position,
            received_at,
            quote_after_other_messages,
            mention_after: Duration::from_secs(mention_after_seconds),
        }
    }

    pub(crate) fn resolve(
        self,
        mut target: ResponseTarget,
        current: Option<PlatformMessagePosition>,
        now: Instant,
    ) -> Option<ResponseTarget> {
        let other_messages = self.position.zip(current).map(|(start, current)| {
            let total = current.total_messages.saturating_sub(start.total_messages);
            let same_sender = current
                .sender_messages
                .saturating_sub(start.sender_messages);
            total.saturating_sub(same_sender)
        });
        if target.quote {
            target.quote = self.quote_after_other_messages == 0
                || other_messages.is_some_and(|count| count >= self.quote_after_other_messages);
        }
        if target.mention {
            // Unknown activity preserves the original time-only mention behavior.
            target.mention = now
                .checked_duration_since(self.received_at)
                .unwrap_or_default()
                >= self.mention_after
                && other_messages.is_none_or(|count| count > 0);
        }
        target.is_effective().then_some(target)
    }
}

#[derive(Clone)]
pub(crate) struct PendingResponseTarget {
    pub(crate) target: ResponseTarget,
    pub(crate) policy: Option<AdaptiveResponseTargetPolicy>,
}

pub(crate) fn apply_resolved_response_target(
    message: &mut OutboundMessage,
    original: &ResponseTarget,
    resolved: Option<&ResponseTarget>,
) {
    if message.response_target.as_ref() == Some(original) {
        message.response_target = resolved.cloned();
    }
}

pub(crate) fn message_is_parenthetical_only(message: &OutboundMessage) -> bool {
    let OutboundBody::Segments(segments) = &message.body else {
        return false;
    };
    let mut text = String::new();
    for segment in segments {
        match segment {
            OutboundSegment::Markdown(part) | OutboundSegment::Text(part) => text.push_str(part),
            OutboundSegment::Mention(_) => {}
            OutboundSegment::ImageBytes { .. }
            | OutboundSegment::ImagePath { .. }
            | OutboundSegment::FilePath { .. } => return false,
        }
    }
    let text = text.trim();
    if text.is_empty() || !text.starts_with('（') || !text.ends_with('）') {
        return false;
    }
    let mut depth = 0_u32;
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '（' => depth = depth.saturating_add(1),
            '）' => {
                let Some(next_depth) = depth.checked_sub(1) else {
                    return false;
                };
                depth = next_depth;
                if depth == 0 && chars.peek().is_some() {
                    return false;
                }
            }
            _ if depth == 0 => return false,
            _ => {}
        }
    }
    depth == 0
}

pub(crate) fn outbound_text_for_history(message: &OutboundMessage) -> String {
    fn append(parts: &mut Vec<String>, segments: &[OutboundSegment]) {
        for segment in segments {
            match segment {
                OutboundSegment::Markdown(text) | OutboundSegment::Text(text) => {
                    if !text.trim().is_empty() {
                        parts.push(text.clone());
                    }
                }
                OutboundSegment::Mention(user_id) => parts.push(format!("@{user_id}")),
                OutboundSegment::ImageBytes { .. }
                | OutboundSegment::ImagePath { .. }
                | OutboundSegment::FilePath { .. } => {}
            }
        }
    }

    let mut parts = Vec::new();
    match &message.body {
        OutboundBody::Segments(segments) => append(&mut parts, segments),
        OutboundBody::Forward(nodes) => {
            for node in nodes {
                append(&mut parts, &node.segments);
            }
        }
    }
    parts.join("\n").trim().to_string()
}

#[derive(Default)]
pub(crate) struct ReplySuppression {
    pub(crate) ranges: Vec<(usize, usize)>,
    pub(crate) open_at: Option<usize>,
    pub(crate) final_reply_already_sent: bool,
}

impl ReplySuppression {
    pub(crate) fn direct_send_succeeded(&mut self, text_len: usize) {
        self.open_at = Some(
            self.open_at
                .map_or(text_len, |existing| existing.min(text_len)),
        );
        self.final_reply_already_sent = true;
    }

    pub(crate) fn model_started(&mut self) {
        self.ranges.clear();
        // A direct tool send answers the same prompt across its model
        // continuation, so suppress that continuation from its first byte.
        self.open_at = self.final_reply_already_sent.then_some(0);
    }

    pub(crate) fn queued_prompt_consumed(&mut self) {
        self.ranges.clear();
        self.open_at = None;
        self.final_reply_already_sent = false;
    }

    pub(crate) fn finish(mut self, text_len: usize) -> (Vec<(usize, usize)>, bool) {
        self.close_range(text_len);
        (self.ranges, self.final_reply_already_sent)
    }

    pub(crate) fn close_range(&mut self, text_len: usize) {
        if let Some(start) = self.open_at.take() {
            if start < text_len {
                self.ranges.push((start, text_len));
            }
        }
    }

    /// Ranges to cut when the current round's text is flushed mid-turn as an
    /// intermediate reply. Leaves the state untouched so the `model_started`
    /// reset that follows keeps its existing semantics.
    pub(crate) fn round_ranges(&self, text_len: usize) -> Vec<(usize, usize)> {
        let mut ranges = self.ranges.clone();
        if let Some(start) = self.open_at {
            if start < text_len {
                ranges.push((start, text_len));
            }
        }
        ranges
    }
}

pub(crate) fn cut_suppressed_ranges(text: &str, ranges: &[(usize, usize)]) -> String {
    if ranges.is_empty() {
        return text.to_string();
    }
    let mut result = String::with_capacity(text.len());
    let mut cursor = 0;
    for &(start, end) in ranges {
        let start = start.clamp(cursor, text.len());
        let end = end.clamp(start, text.len());
        let (Some(prefix), Some(_suppressed)) = (text.get(cursor..start), text.get(start..end))
        else {
            continue;
        };
        result.push_str(prefix);
        cursor = end;
    }
    if let Some(suffix) = text.get(cursor..) {
        result.push_str(suffix);
    }
    result
}

pub(crate) fn start_model_reply(text: &mut String, suppression: &mut ReplySuppression) {
    text.clear();
    suppression.model_started();
}

/// Sends the just-finished model round as its own platform message. The
/// round's direct-send suppression ranges still apply, so text a tool already
/// delivered is not repeated.
pub(crate) async fn flush_intermediate_reply(
    context: &PlatformTurnContext,
    text: &str,
    suppression: &ReplySuppression,
) {
    if context.turn_is_superseded() {
        return;
    }
    let visible = cut_suppressed_ranges(text, &suppression.round_ranges(text.len()));
    let visible = visible.trim();
    if visible.is_empty() {
        return;
    }
    match context
        .send(OutboundMessage::markdown(
            OutboundOrigin::IntermediateReply,
            visible.to_string(),
        ))
        .await
    {
        Ok(_) => tracing::info!(
            target: "miyu::qq",
            chars = visible.chars().count(),
            "{}",
            crate::i18n::text(
                "sent an intermediate platform reply",
                "已发送平台中间消息",
            )
        ),
        Err(error) => tracing::warn!(
            target: "miyu::qq",
            error = %error,
            "{}",
            crate::i18n::text(
                "sending an intermediate platform reply failed",
                "发送平台中间消息失败",
            )
        ),
    }
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
pub(crate) fn strip_inline_markup(line: &str) -> String {
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
