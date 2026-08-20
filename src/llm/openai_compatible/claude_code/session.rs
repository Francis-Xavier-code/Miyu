//! claude 会话与 Miyu 消息前缀的对应关系(进程内)。
//!
//! 键是「逐消息哈希链」:chain[i] = 前 i 条会话消息的链哈希,种子掺入
//! provider/model/system prompt。Miyu 的历史回放是字节级 append-only 的,
//! 所以"本次请求延续上次"⇔"上次记录的 (长度, 链哈希) 是本次链的前缀"。
//! 匹配不上(redo/compact/系统提示词变更/daemon 重启)就重开会话全量重放
//! ——只损失效率,不损失正确性,因此状态不做持久化。

use crate::llm::openai_compatible::*;
use std::hash::{DefaultHasher, Hash, Hasher};

struct SessionEntry {
    provider_id: String,
    model: String,
    /// 该 claude 会话已覆盖的会话消息数(含预测的 assistant 回填)。
    prefix_len: usize,
    prefix_hash: u64,
    claude_session: String,
}

static SESSIONS: Mutex<Vec<SessionEntry>> = Mutex::new(Vec::new());

/// clear-at-cap 定式:超上限整表清空,宁可全量重放一轮,不做 LRU 簿记。
const SESSION_CAP: usize = 64;

fn hash_step(previous: u64, bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    previous.hash(&mut hasher);
    bytes.hash(&mut hasher);
    hasher.finish()
}

fn message_bytes(message: &ChatMessage) -> Vec<u8> {
    serde_json::to_vec(message).unwrap_or_default()
}

pub(super) fn prefix_chain(
    provider_id: &str,
    model: &str,
    system_prompt: &str,
    conversation: &[ChatMessage],
) -> Vec<u64> {
    let seed = {
        let mut hasher = DefaultHasher::new();
        provider_id.hash(&mut hasher);
        model.hash(&mut hasher);
        system_prompt.hash(&mut hasher);
        hasher.finish()
    };
    let mut chain = Vec::with_capacity(conversation.len() + 1);
    chain.push(seed);
    let mut current = seed;
    for message in conversation {
        current = hash_step(current, &message_bytes(message));
        chain.push(current);
    }
    chain
}

pub(super) fn extend_chain(chain_end: u64, message: &ChatMessage) -> u64 {
    hash_step(chain_end, &message_bytes(message))
}

/// 找可续传的最长前缀:返回 (claude 会话 id, 已覆盖的消息数)。要求严格短于
/// 本次会话消息数——增量为空说明不是正常的新一轮,按全量重放处理。
pub(super) fn find_resumable(
    provider_id: &str,
    model: &str,
    chain: &[u64],
    conversation_len: usize,
) -> Option<(String, usize)> {
    let sessions = SESSIONS.lock().ok()?;
    sessions
        .iter()
        .filter(|entry| entry.provider_id == provider_id && entry.model == model)
        .filter(|entry| {
            entry.prefix_len < conversation_len && chain[entry.prefix_len] == entry.prefix_hash
        })
        .max_by_key(|entry| entry.prefix_len)
        .map(|entry| (entry.claude_session.clone(), entry.prefix_len))
}

pub(super) fn record_session(
    provider_id: &str,
    model: &str,
    prefix_len: usize,
    prefix_hash: u64,
    claude_session: String,
) {
    let Ok(mut sessions) = SESSIONS.lock() else {
        return;
    };
    if sessions.len() >= SESSION_CAP {
        sessions.clear();
    }
    // 同一 claude 会话只保留最新指针:续传成功后旧前缀已被新前缀覆盖。
    sessions.retain(|entry| entry.claude_session != claude_session);
    sessions.push(SessionEntry {
        provider_id: provider_id.to_string(),
        model: model.to_string(),
        prefix_len,
        prefix_hash,
        claude_session,
    });
}

pub(super) fn forget_session(claude_session: &str) {
    if let Ok(mut sessions) = SESSIONS.lock() {
        sessions.retain(|entry| entry.claude_session != claude_session);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: &str, text: &str) -> ChatMessage {
        ChatMessage::plain(role, text)
    }

    /// 续传判定的核心不变量:append-only 延伸能匹配,任何前缀改写都匹配不上。
    #[test]
    fn prefix_matching_follows_append_only_history() {
        let base = vec![message("user", "hi"), message("assistant", "hello")];
        let chain = prefix_chain("p", "m", "sys", &base);
        record_session("p", "m", 2, chain[2], "sess-1".to_string());

        // 纯追加:匹配,且增量从第 2 条之后开始。
        let mut extended = base.clone();
        extended.push(message("user", "next"));
        let chain = prefix_chain("p", "m", "sys", &extended);
        assert_eq!(
            find_resumable("p", "m", &chain, extended.len()),
            Some(("sess-1".to_string(), 2))
        );

        // 改写历史(redo):第 2 条字节变了,匹配不上。
        let mut rewritten = vec![message("user", "hi"), message("assistant", "changed")];
        rewritten.push(message("user", "next"));
        let chain = prefix_chain("p", "m", "sys", &rewritten);
        assert_eq!(find_resumable("p", "m", &chain, rewritten.len()), None);

        // 系统提示词变更:种子不同,匹配不上。
        let chain = prefix_chain("p", "m", "other-sys", &extended);
        assert_eq!(find_resumable("p", "m", &chain, extended.len()), None);

        // 增量为空(长度相同)不算续传。
        let chain = prefix_chain("p", "m", "sys", &base);
        assert_eq!(find_resumable("p", "m", &chain, base.len()), None);

        forget_session("sess-1");
        let chain = prefix_chain("p", "m", "sys", &extended);
        assert_eq!(find_resumable("p", "m", &chain, extended.len()), None);
    }
}
