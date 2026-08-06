mod openai_compatible;

pub(crate) use openai_compatible::{
    thinking_variant_options_for_model, ThinkingVariantPreferences,
};
pub use openai_compatible::{OpenAiCompatibleClient, ThinkingVariantOptions};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<ChatContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// DeepSeek thinking mode requires the `reasoning_content` KEY to be present
    /// on assistant tool_calls turns of subsequent requests (empty string is
    /// accepted; a missing key is a 400). Only serialized when `Some`; the
    /// provider adapter strips it for endpoints that do not understand it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// Anthropic extended-thinking signature captured from the stream; needed to
    /// rebuild the `thinking` block when replaying the assistant turn within the
    /// same tool loop. Never serialized into OpenAI-style JSON directly.
    #[serde(default, skip_serializing, skip_deserializing)]
    pub thinking_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChatContent {
    Text(String),
    Parts(Vec<ChatContentPart>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ChatContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrlContent },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrlContent {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: FunctionDefinition,
}

#[derive(Debug, Clone, Serialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl ChatMessage {
    fn base(role: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: None,
            tool_call_id: None,
            tool_calls: None,
            reasoning_content: None,
            thinking_signature: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            content: Some(ChatContent::Text(content.into())),
            ..Self::base("system")
        }
    }

    pub fn assistant(content: impl Into<String>, tool_calls: Option<Vec<ToolCall>>) -> Self {
        let text = content.into();
        let has_tool_calls = tool_calls.as_ref().map(|c| !c.is_empty()).unwrap_or(false);
        let content = if text.trim().is_empty() && has_tool_calls {
            // Keep an explicit empty string so the `content` key stays present on
            // tool_calls turns: some strict gateways 400 on a missing key.
            Some(ChatContent::Text(String::new()))
        } else {
            Some(ChatContent::Text(text))
        };
        Self {
            content,
            tool_calls,
            ..Self::base("assistant")
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            content: Some(ChatContent::Text(content.into())),
            tool_call_id: Some(tool_call_id.into()),
            ..Self::base("tool")
        }
    }

    pub fn plain(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            content: Some(ChatContent::Text(content.into())),
            ..Self::base(role)
        }
    }

    pub fn user_parts(parts: Vec<ChatContentPart>) -> Self {
        Self {
            content: Some(ChatContent::Parts(parts)),
            ..Self::base("user")
        }
    }

    pub fn user_with_image(text: impl Into<String>, image_url: impl Into<String>) -> Self {
        Self {
            content: Some(ChatContent::Parts(vec![
                ChatContentPart::Text { text: text.into() },
                ChatContentPart::ImageUrl {
                    image_url: ImageUrlContent {
                        url: image_url.into(),
                    },
                },
            ])),
            ..Self::base("user")
        }
    }
}

fn u64_is_zero(value: &u64) -> bool {
    *value == 0
}

fn bool_is_false(value: &bool) -> bool {
    !*value
}

/// OpenAI-style `usage.prompt_tokens_details`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PromptTokensDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
}

/// OpenAI-style `usage.completion_tokens_details`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompletionTokensDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    /// DeepSeek reports cache accounting at the top level of `usage`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_hit_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_miss_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens_details: Option<CompletionTokensDetails>,
    /// Normalized invariant across providers: prompt_tokens = total input,
    /// cache_read_tokens ⊆ prompt_tokens, cache_write_tokens is the portion
    /// written to cache where the provider reports it (Anthropic/OpenAI 5.6+).
    #[serde(default, skip_serializing_if = "u64_is_zero")]
    pub cache_read_tokens: u64,
    #[serde(default, skip_serializing_if = "u64_is_zero")]
    pub cache_write_tokens: u64,
    #[serde(default, skip_serializing_if = "u64_is_zero")]
    pub reasoning_tokens: u64,
    /// True when the provider reported any cache accounting for this request.
    /// Distinguishes "0 cached" from "provider does not report caching" so
    /// hit-rate stats do not treat DeepSeek cold requests as unsupported.
    #[serde(default, skip_serializing_if = "bool_is_false")]
    pub cache_reported: bool,
}

impl Usage {
    pub fn effective_total_tokens(&self) -> u64 {
        if self.total_tokens > 0 {
            self.total_tokens
        } else {
            self.prompt_tokens.saturating_add(self.completion_tokens)
        }
    }

    /// Fold provider-specific raw fields into the normalized cache columns.
    /// Idempotent; call once wherever a provider usage payload enters Miyu.
    pub fn normalize_cache_fields(&mut self) {
        if let Some(hit) = self.prompt_cache_hit_tokens {
            self.cache_read_tokens = self.cache_read_tokens.max(hit);
            self.cache_reported = true;
        }
        if self.prompt_cache_miss_tokens.is_some() {
            self.cache_reported = true;
        }
        if let Some(details) = &self.prompt_tokens_details {
            if let Some(cached) = details.cached_tokens {
                self.cache_read_tokens = self.cache_read_tokens.max(cached);
                self.cache_reported = true;
            }
            if let Some(write) = details.cache_write_tokens {
                self.cache_write_tokens = self.cache_write_tokens.max(write);
                self.cache_reported = true;
            }
        }
        if let Some(details) = &self.completion_tokens_details {
            if let Some(reasoning) = details.reasoning_tokens {
                self.reasoning_tokens = self.reasoning_tokens.max(reasoning);
            }
        }
        // Guard the invariant instead of papering over a broken adapter: a
        // cache_read larger than the whole prompt means the mapping is wrong.
        if self.cache_read_tokens > self.prompt_tokens && self.prompt_tokens > 0 {
            self.cache_read_tokens = self.prompt_tokens;
        }
    }

    pub fn uncached_prompt_tokens(&self) -> u64 {
        self.prompt_tokens.saturating_sub(self.cache_read_tokens)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ResponsesContinuation {
    pub(crate) response_id: String,
    pub(crate) endpoint_id: String,
}

#[derive(Debug, Clone)]
pub struct ChatResult {
    pub content: String,
    pub reasoning: Option<String>,
    pub usage: Option<Usage>,
    pub usage_estimated: bool,
    pub tool_calls: Vec<ToolCall>,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    /// Provider finish reason ("stop" / "length" / "tool_calls" / ...). A
    /// "length" stop with pending tool calls means the arguments may be
    /// silently truncated; the agent refuses to execute them in that case.
    pub finish_reason: Option<String>,
    /// Anthropic extended-thinking signature for this assistant turn, needed
    /// to replay the thinking block on the next request of the same tool loop.
    pub thinking_signature: Option<String>,
    /// Raw usage of the FINAL request of the turn (when `usage` holds the
    /// turn-accumulated sum). Its prompt+completion is the true provider-side
    /// context size, used for the context meter instead of a local estimate.
    pub last_request_usage: Option<Usage>,
    pub(crate) responses_continuation: Option<Box<ResponsesContinuation>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatStreamKind {
    Content,
    Reasoning,
    ReasoningReset,
    ReasoningPartStart,
    ReasoningPartEnd,
    ToolCall,
}

#[derive(Debug, Clone)]
pub struct ChatStreamChunk {
    pub kind: ChatStreamKind,
    pub text: String,
}
