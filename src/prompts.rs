use base64::Engine;

include!(concat!(env!("OUT_DIR"), "/default_miyu_prompt.rs"));

pub const PLAN_REMINDER: &str = include_str!("prompts/plan.md");
pub const CHAT_REMINDER: &str = include_str!("prompts/chat.md");
pub const MEME_DESCRIPTION_PROMPT: &str = include_str!("prompts/meme-description.md");
pub const COMPACT_SYSTEM_PROMPT: &str = include_str!("prompts/compact.md");
pub const COMPACT_CHAT_SYSTEM_PROMPT: &str = include_str!("prompts/compact_chat.md");

pub fn default_system_prompt() -> String {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(OBFUSCATED_DEFAULT_SYSTEM_PROMPT)
        .expect("embedded default prompt must be valid base64");
    let decoded = bytes
        .into_iter()
        .enumerate()
        .map(|(index, byte)| byte ^ PROMPT_MASK[index % PROMPT_MASK.len()])
        .collect::<Vec<_>>();
    String::from_utf8(decoded).expect("embedded default prompt must be valid utf-8")
}
