use crate::default_models::{
    OPENCODE_DEFAULT_CHAT_MODEL, OPENCODE_DEFAULT_VISION_MODEL, OPENCODE_PROVIDER_ID,
    OPENCODE_ZEN_BASE_URL,
};
use crate::paths::MiyuPaths;
use crate::prompts::default_system_prompt;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

pub const MAX_COMMAND_OUTPUT_LINES: usize = 1_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub active_provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_provider_models: Option<Vec<ActiveProviderModelConfig>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_multimodal_provider_models: Option<Vec<ActiveProviderModelConfig>>,
    pub providers: Vec<ProviderConfig>,
    #[serde(default)]
    pub context: ContextConfig,
    #[serde(default)]
    pub tools: ToolsConfig,
    #[serde(default)]
    pub mcp: McpConfig,
    #[serde(default)]
    pub skills: SkillsConfig,
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub prompt: PromptConfig,
    #[serde(default)]
    pub plugins: PluginsConfig,
    #[serde(default, skip_serializing)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub system_prompt_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "SubagentTiersConfig::is_empty")]
    pub subagent_tiers: SubagentTiersConfig,
    #[serde(default, skip_serializing_if = "PlatformsConfig::is_empty")]
    pub platforms: PlatformsConfig,
}

/// Messaging-platform settings. Public configuration is named after the
/// product users connect to; transport protocols remain implementation
/// details of each platform adapter.
pub const DEFAULT_PLATFORM_COMMAND_PREFIX: &str = "/";
pub const MAX_PLATFORM_COMMAND_PREFIX_CHARS: usize = 32;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlatformsConfig {
    #[serde(
        default = "default_platform_command_prefix",
        skip_serializing_if = "is_default_platform_command_prefix"
    )]
    pub command_prefix: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub commands: BTreeMap<String, PlatformCommandConfig>,
    #[serde(default, skip_serializing_if = "OneBotConfig::is_default")]
    pub qq: OneBotConfig,
}

impl Default for PlatformsConfig {
    fn default() -> Self {
        Self {
            command_prefix: default_platform_command_prefix(),
            commands: BTreeMap::new(),
            qq: OneBotConfig::default(),
        }
    }
}

impl PlatformsConfig {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    pub fn command_permission(
        &self,
        command: &str,
        default: PlatformCommandPermission,
    ) -> PlatformCommandPermission {
        self.commands
            .get(command)
            .map(|config| config.permission)
            .unwrap_or(default)
    }

    pub fn set_command_permission(
        &mut self,
        command: &str,
        permission: PlatformCommandPermission,
        default: PlatformCommandPermission,
    ) {
        if permission == default {
            self.commands.remove(command);
        } else {
            self.commands
                .insert(command.to_string(), PlatformCommandConfig { permission });
        }
    }

    pub fn model_route(
        &self,
        kind: PlatformConversationKind,
        conversation_id: &str,
    ) -> Option<&PlatformModelRoute> {
        self.qq
            .conversations
            .iter()
            .find(|route| route.matches(kind, conversation_id))
    }

    pub fn model_route_mut(
        &mut self,
        kind: PlatformConversationKind,
        conversation_id: &str,
    ) -> Option<&mut PlatformModelRoute> {
        self.qq
            .conversations
            .iter_mut()
            .find(|route| route.matches(kind, conversation_id))
    }

    /// Inserts a route or replaces the route with the same stable identity.
    /// Inherited pools are meaningful conversation configuration and are kept
    /// until the user explicitly removes the entry.
    pub fn upsert_model_route(&mut self, mut route: PlatformModelRoute) {
        route.normalize();
        if let Some(index) = self
            .qq
            .conversations
            .iter()
            .position(|existing| existing.identity() == route.identity())
        {
            self.qq.conversations[index] = route;
        } else {
            self.qq.conversations.push(route);
        }
    }

    pub fn remove_model_route(
        &mut self,
        kind: PlatformConversationKind,
        conversation_id: &str,
    ) -> bool {
        let old_len = self.qq.conversations.len();
        self.qq
            .conversations
            .retain(|route| !route.matches(kind, conversation_id));
        self.qq.conversations.len() != old_len
    }

    pub fn normalize_model_routes(&mut self) {
        self.command_prefix = self.command_prefix.trim().to_string();
        self.qq.admin_users.sort_unstable();
        self.qq.admin_users.dedup();
        self.qq.private_chats.whitelist.sort_unstable();
        self.qq.private_chats.whitelist.dedup();
        self.qq.group_chats.whitelist.sort_unstable();
        self.qq.group_chats.whitelist.dedup();
        let mut keywords = HashSet::with_capacity(self.qq.group_chats.trigger_keywords.len());
        self.qq.group_chats.trigger_keywords = self
            .qq
            .group_chats
            .trigger_keywords
            .drain(..)
            .map(|keyword| keyword.trim().to_string())
            .filter(|keyword| !keyword.is_empty() && keywords.insert(keyword.clone()))
            .collect();
        self.qq.asset_base_url = self
            .qq
            .asset_base_url
            .trim()
            .trim_end_matches('/')
            .to_string();
        for route in &mut self.qq.conversations {
            route.normalize();
        }
        self.qq
            .plugins
            .retain(|name, instance| !name.trim().is_empty() && !instance.is_empty());
    }

    pub fn prune_model_references(&mut self, providers: &[ProviderConfig]) {
        for route in &mut self.qq.conversations {
            route.prune_model_references(providers);
        }
        self.normalize_model_routes();
    }

    pub fn remove_model_references(&mut self, provider_id: &str, model: &str) {
        for route in &mut self.qq.conversations {
            route.remove_model_references(provider_id, model);
        }
        self.normalize_model_routes();
    }

    pub fn remove_provider_references(&mut self, provider_id: &str) {
        for route in &mut self.qq.conversations {
            for pool in [&mut route.text_models, &mut route.multimodal_models] {
                if let Some(entries) = pool {
                    entries.retain(|entry| entry.provider_id != provider_id);
                }
                normalize_route_pool(pool);
            }
        }
        self.normalize_model_routes();
    }

    pub fn rename_provider_references(&mut self, old_id: &str, new_id: &str) {
        for route in &mut self.qq.conversations {
            route.rename_provider_references(old_id, new_id);
        }
    }

    pub fn rename_model_references(&mut self, provider_id: &str, old: &str, new: &str) {
        for route in &mut self.qq.conversations {
            route.rename_model_references(provider_id, old, new);
        }
    }
}

fn default_platform_command_prefix() -> String {
    DEFAULT_PLATFORM_COMMAND_PREFIX.to_string()
}

fn is_default_platform_command_prefix(value: &String) -> bool {
    value == DEFAULT_PLATFORM_COMMAND_PREFIX
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformCommandPermission {
    Everyone,
    #[default]
    AdminOnly,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformCommandConfig {
    #[serde(default)]
    pub permission: PlatformCommandPermission,
}

pub type PlatformPluginsConfig = BTreeMap<String, PlatformPluginInstanceConfig>;

type PlatformPluginConfigValidator = fn(&PlatformPluginInstanceConfig) -> Result<()>;

const PLATFORM_PLUGIN_VALIDATORS: &[(&str, PlatformPluginConfigValidator)] =
    &[("reply_processor", validate_reply_processor_plugin_config)];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlatformPluginInstanceConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub settings: serde_json::Map<String, serde_json::Value>,
}

impl PlatformPluginInstanceConfig {
    pub fn is_empty(&self) -> bool {
        self.enabled.is_none() && self.settings.is_empty()
    }

    pub fn enabled_or(&self, default: bool) -> bool {
        self.enabled.unwrap_or(default)
    }
}

fn validate_reply_processor_plugin_config(instance: &PlatformPluginInstanceConfig) -> Result<()> {
    let settings = &instance.settings;
    for key in [
        "default_enabled",
        "followup_mention",
        "strip_period",
        "context_notice",
        "send_tool_intercept",
    ] {
        if settings.get(key).is_some_and(|value| !value.is_boolean()) {
            bail!("platform plugin reply_processor.{key} must be a boolean");
        }
    }
    for (key, min, max) in [
        ("threshold", 1_u64, 100_000_u64),
        ("max_height", 1_000, 5_000),
        ("font_size", 24, 56),
        ("code_font_size", 20, 46),
        ("padding", 36, 120),
        ("ttl_hours", 1, 168),
        ("max_records", 1, 10),
    ] {
        if let Some(value) = settings.get(key) {
            let value = value.as_u64().with_context(|| {
                format!("platform plugin reply_processor.{key} must be an unsigned integer")
            })?;
            if !(min..=max).contains(&value) {
                bail!("platform plugin reply_processor.{key} must be between {min} and {max}");
            }
        }
    }
    validate_plugin_string_choice(settings, "mode", &["image", "forward"])?;
    validate_plugin_string_choice(settings, "theme", &["paper", "light", "dark"])?;
    for key in ["font", "title_font", "code_font", "emoji_font"] {
        if let Some(value) = settings.get(key) {
            let value = value.as_str().with_context(|| {
                format!("platform plugin reply_processor.{key} must be a string")
            })?;
            if value.len() > 4_096 || value.contains('\0') {
                bail!("platform plugin reply_processor.{key} is invalid");
            }
        }
    }
    Ok(())
}

fn validate_plugin_string_choice(
    settings: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    choices: &[&str],
) -> Result<()> {
    let Some(value) = settings.get(key) else {
        return Ok(());
    };
    let value = value
        .as_str()
        .with_context(|| format!("platform plugin reply_processor.{key} must be a string"))?;
    if !choices.contains(&value) {
        bail!(
            "platform plugin reply_processor.{key} must be one of: {}",
            choices.join(", ")
        );
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformConversationKind {
    Private,
    Group,
}

impl PlatformConversationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Group => "group",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlatformConversationConfig {
    pub kind: PlatformConversationKind,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformModelRoute {
    pub conversation: PlatformConversationConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_models: Option<Vec<ActiveProviderModelConfig>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multimodal_models: Option<Vec<ActiveProviderModelConfig>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub extra_prompt: String,
}

impl PlatformModelRoute {
    pub fn identity(&self) -> (PlatformConversationKind, &str) {
        (self.conversation.kind, self.conversation.id.as_str())
    }

    pub fn matches(&self, kind: PlatformConversationKind, conversation_id: &str) -> bool {
        self.conversation.kind == kind && self.conversation.id == conversation_id
    }

    pub fn normalize(&mut self) {
        self.conversation.id = self.conversation.id.trim().to_string();
        self.extra_prompt = self.extra_prompt.trim().to_string();
        normalize_route_pool(&mut self.text_models);
        normalize_route_pool(&mut self.multimodal_models);
    }

    fn prune_model_references(&mut self, providers: &[ProviderConfig]) {
        if let Some(pool) = &mut self.text_models {
            pool.retain(|entry| active_model_exists(providers, entry));
        }
        if let Some(pool) = &mut self.multimodal_models {
            pool.retain(|entry| active_model_supports_image(providers, entry));
        }
        normalize_route_pool(&mut self.text_models);
        normalize_route_pool(&mut self.multimodal_models);
    }

    fn remove_model_references(&mut self, provider_id: &str, model: &str) {
        for pool in [&mut self.text_models, &mut self.multimodal_models] {
            if let Some(entries) = pool {
                entries.retain(|entry| !(entry.provider_id == provider_id && entry.model == model));
            }
            normalize_route_pool(pool);
        }
    }

    fn rename_provider_references(&mut self, old_id: &str, new_id: &str) {
        for entries in [&mut self.text_models, &mut self.multimodal_models]
            .into_iter()
            .flatten()
        {
            for entry in entries {
                if entry.provider_id == old_id {
                    entry.provider_id = new_id.to_string();
                }
            }
        }
    }

    fn rename_model_references(&mut self, provider_id: &str, old: &str, new: &str) {
        for entries in [&mut self.text_models, &mut self.multimodal_models]
            .into_iter()
            .flatten()
        {
            for entry in entries {
                if entry.provider_id == provider_id && entry.model == old {
                    entry.model = new.to_string();
                }
            }
        }
    }
}

fn normalize_route_pool(pool: &mut Option<Vec<ActiveProviderModelConfig>>) {
    let Some(entries) = pool else {
        return;
    };
    let mut seen = HashSet::with_capacity(entries.len());
    entries.retain_mut(|entry| {
        entry.provider_id = entry.provider_id.trim().to_string();
        entry.model = entry.model.trim().to_string();
        !entry.provider_id.is_empty()
            && !entry.model.is_empty()
            && seen.insert((entry.provider_id.clone(), entry.model.clone()))
    });
    if entries.is_empty() {
        *pool = None;
    }
}

fn rename_provider_in_pool(pool: &mut [ActiveProviderModelConfig], old_id: &str, new_id: &str) {
    for entry in pool {
        if entry.provider_id == old_id {
            entry.provider_id = new_id.to_string();
        }
    }
}

fn retain_provider_pool(pool: &mut Option<Vec<ActiveProviderModelConfig>>, provider_id: &str) {
    if let Some(entries) = pool {
        entries.retain(|entry| entry.provider_id != provider_id);
    }
    retain_nonempty_pool(pool);
}

fn retain_nonempty_pool(pool: &mut Option<Vec<ActiveProviderModelConfig>>) {
    if pool.as_ref().is_some_and(Vec::is_empty) {
        *pool = None;
    }
}

/// Tencent QQ integration implemented through a OneBot v11 reverse
/// WebSocket transport (for example NapCat).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OneBotConfig {
    pub enabled: bool,
    pub reverse_ws_port: u16,
    /// Checked against NapCat's `Authorization: Bearer` handshake header.
    /// Empty tokens are accepted only from a loopback peer.
    pub access_token: String,
    pub admin_users: Vec<i64>,
    /// Grants full host tools only to non-admin users in `private_chats.whitelist`.
    pub allow_non_admin_host_tools: bool,
    pub private_chats: QqPrivateChatsConfig,
    pub group_chats: QqGroupChatsConfig,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conversations: Vec<PlatformModelRoute>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub plugins: PlatformPluginsConfig,
    /// Public HTTP base URL NapCat can use to fetch temporary local assets.
    pub asset_base_url: String,
    /// Replies longer than this are split into multiple messages. 0 = never split.
    pub max_reply_chars: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QqPrivateChatsConfig {
    /// QQ ids whose private conversations bypass admission rate limits.
    pub whitelist: Vec<i64>,
    pub allow_non_whitelist: bool,
    /// Per private conversation. Zero disables this limit.
    pub non_whitelist_rate_per_minute: u32,
}

impl Default for QqPrivateChatsConfig {
    fn default() -> Self {
        Self {
            whitelist: Vec::new(),
            allow_non_whitelist: true,
            non_whitelist_rate_per_minute: 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QqGroupChatsConfig {
    /// Group ids that use the whitelist-group rate limit.
    pub whitelist: Vec<i64>,
    /// Additional wake prefixes. @-mentions always remain active.
    pub trigger_keywords: Vec<String>,
    /// Shared by all senders in one whitelisted group. Zero is unlimited.
    pub whitelist_rate_per_minute: u32,
    pub allow_non_whitelist: bool,
    /// Shared by all senders in one non-whitelisted group. Zero is unlimited.
    pub non_whitelist_rate_per_minute: u32,
}

impl Default for QqGroupChatsConfig {
    fn default() -> Self {
        Self {
            whitelist: Vec::new(),
            trigger_keywords: Vec::new(),
            whitelist_rate_per_minute: 30,
            allow_non_whitelist: true,
            non_whitelist_rate_per_minute: 10,
        }
    }
}

impl Default for OneBotConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            reverse_ws_port: 8300,
            access_token: String::new(),
            admin_users: Vec::new(),
            allow_non_admin_host_tools: false,
            private_chats: QqPrivateChatsConfig::default(),
            group_chats: QqGroupChatsConfig::default(),
            conversations: Vec::new(),
            plugins: PlatformPluginsConfig::new(),
            asset_base_url: String::new(),
            max_reply_chars: 3000,
        }
    }
}

impl OneBotConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Subagent model tier pools. When the main agent spawns a subagent it
/// picks a tier by task complexity (cheap/balanced/strong); requests then
/// load-balance across that tier's pool exactly like the main text-model
/// pool. Tiers are subagent-only — the main conversation and auxiliary
/// work always use the user-selected main models. An unconfigured or
/// unavailable pool falls back to the main model pool.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SubagentTiersConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cheap: Vec<ActiveProviderModelConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub balanced: Vec<ActiveProviderModelConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strong: Vec<ActiveProviderModelConfig>,
}

impl SubagentTiersConfig {
    pub fn is_empty(&self) -> bool {
        self.cheap.is_empty() && self.balanced.is_empty() && self.strong.is_empty()
    }

    pub fn pool(&self, tier: ModelTier) -> &Vec<ActiveProviderModelConfig> {
        match tier {
            ModelTier::Cheap => &self.cheap,
            ModelTier::Balanced => &self.balanced,
            ModelTier::Strong => &self.strong,
        }
    }

    pub fn pool_mut(&mut self, tier: ModelTier) -> &mut Vec<ActiveProviderModelConfig> {
        match tier {
            ModelTier::Cheap => &mut self.cheap,
            ModelTier::Balanced => &mut self.balanced,
            ModelTier::Strong => &mut self.strong,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelTier {
    Cheap,
    Balanced,
    Strong,
}

impl ModelTier {
    pub const ALL: [Self; 3] = [Self::Cheap, Self::Balanced, Self::Strong];

    pub fn from_str(value: &str) -> Option<Self> {
        match value.trim() {
            "cheap" => Some(Self::Cheap),
            "balanced" => Some(Self::Balanced),
            "strong" => Some(Self::Strong),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Cheap => "cheap",
            Self::Balanced => "balanced",
            Self::Strong => "strong",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveProviderModelConfig {
    pub provider_id: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DisplayConfig {
    #[serde(default = "default_display_language")]
    pub language: String,
    #[serde(default = "default_reasoning_display")]
    pub reasoning: String,
    #[serde(default = "default_tool_call_display")]
    pub tool_calls: String,
    #[serde(default = "default_true")]
    pub readable_tool_names: bool,
    #[serde(default)]
    pub show_token_usage: bool,
    #[serde(default = "default_mixed_model_endpoint_display")]
    pub mixed_model_endpoint_display: String,
    #[serde(default = "default_command_output_lines")]
    pub command_output_lines: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct RawDisplayConfig {
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Option<String>,
    #[serde(default)]
    show_reasoning: Option<bool>,
    #[serde(default)]
    reasoning_mode: Option<String>,
    #[serde(default)]
    show_tool_details: Option<bool>,
    #[serde(default)]
    readable_tool_names: Option<bool>,
    #[serde(default)]
    show_token_usage: Option<bool>,
    #[serde(default)]
    show_mixed_model_endpoint: Option<bool>,
    #[serde(default)]
    mixed_model_endpoint_display: Option<String>,
    #[serde(default)]
    command_output_lines: Option<usize>,
}

impl<'de> Deserialize<'de> for DisplayConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawDisplayConfig::deserialize(deserializer)?;
        let reasoning = raw.reasoning.unwrap_or_else(|| {
            if raw.show_reasoning == Some(false) {
                "hidden".to_string()
            } else {
                raw.reasoning_mode.unwrap_or_else(default_reasoning_display)
            }
        });
        let tool_calls = raw.tool_calls.unwrap_or_else(|| {
            if raw.show_tool_details == Some(true) {
                "full".to_string()
            } else {
                default_tool_call_display()
            }
        });
        Ok(Self {
            language: raw.language.unwrap_or_else(default_display_language),
            reasoning,
            tool_calls,
            readable_tool_names: raw.readable_tool_names.unwrap_or_else(default_true),
            show_token_usage: raw.show_token_usage.unwrap_or(false),
            mixed_model_endpoint_display: raw.mixed_model_endpoint_display.unwrap_or_else(|| {
                match raw.show_mixed_model_endpoint {
                    Some(true) => "all".to_string(),
                    Some(false) => "off".to_string(),
                    None => default_mixed_model_endpoint_display(),
                }
            }),
            command_output_lines: raw
                .command_output_lines
                .unwrap_or_else(default_command_output_lines),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub display_name: String,
    pub base_url: String,
    #[serde(
        default = "default_provider_protocol",
        skip_serializing_if = "is_auto_protocol"
    )]
    pub protocol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub model_context_window: HashMap<String, usize>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub model_modalities: HashMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_model: String,
    #[serde(
        default = "default_timeout",
        skip_serializing_if = "is_default_timeout"
    )]
    pub timeout_seconds: u64,
    #[serde(
        default = "default_temperature",
        skip_serializing_if = "is_default_temperature"
    )]
    pub temperature: f32,
    #[serde(
        default = "default_anthropic_max_tokens",
        skip_serializing_if = "is_default_anthropic_max_tokens"
    )]
    pub anthropic_max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra_body: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone)]
pub struct ResolvedProviderKey {
    pub index: usize,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptConfig {
    #[serde(default = "default_prompts_dir")]
    pub prompts_dir: String,
    #[serde(default = "default_identities_dir")]
    pub identities_dir: String,
    #[serde(default = "default_user_identity_file")]
    pub user_identity_file: String,
    #[serde(default)]
    pub active_persona: String,
    #[serde(default)]
    pub active_identity: String,
}

#[derive(Debug, Clone)]
pub struct ProviderModelChoice {
    pub provider_id: String,
    pub provider_name: String,
    pub model: String,
}

impl ProviderModelChoice {
    pub fn value(&self) -> String {
        format!("{}\t{}", self.provider_id, self.model)
    }

    pub fn label(&self) -> String {
        format!("{} / {}", self.provider_name, self.model)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextConfig {
    #[serde(default = "default_trim_at_ratio")]
    pub trim_at_ratio: f32,
    #[serde(default = "default_trim_batch_ratio")]
    pub trim_batch_ratio: f32,
    #[serde(default = "default_on_overflow")]
    pub on_overflow: String,
    #[serde(default = "default_context_window")]
    pub default_context_window: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub max_rounds: usize,
    #[serde(default = "default_tools_loading_mode")]
    pub loading_mode: String,
    #[serde(default = "default_true")]
    pub persist_loaded_tools: bool,
    /// How many `task` subagents from one tool batch may run concurrently.
    #[serde(default = "default_subagent_concurrency")]
    pub subagent_concurrency: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default = "default_mcp_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub allow_command_execution: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub evicted_context_enabled: bool,
    #[serde(default = "default_true")]
    pub association_enabled: bool,
    #[serde(default = "default_true")]
    pub auto_diary_enabled: bool,
    #[serde(default = "default_true")]
    pub auto_fact_enabled: bool,
    #[serde(default = "default_true")]
    pub auto_skill_enabled: bool,
    #[serde(default = "default_memory_association_facts")]
    pub association_facts: usize,
    #[serde(default = "default_memory_association_episodes")]
    pub association_episodes: usize,
    #[serde(default = "default_memory_association_max_chars")]
    pub association_max_chars: usize,
    #[serde(default = "default_memory_snippet_chars")]
    pub snippet_chars: usize,
    #[serde(default = "default_memory_forget_after_days")]
    pub forget_after_days: u64,
    #[serde(default = "default_true")]
    pub forgetting_enabled: bool,
    #[serde(default = "default_memory_half_life_days")]
    pub forgetting_half_life_days: f64,
    #[serde(default = "default_memory_min_strength")]
    pub forgetting_min_strength: f64,
    #[serde(default = "default_memory_review_boost")]
    pub forgetting_review_boost: f64,
    #[serde(default = "default_memory_min_task_chars")]
    pub learning_min_task_chars: usize,
    #[serde(default = "default_memory_min_method_chars")]
    pub learning_min_method_chars: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginsConfig {
    #[serde(default)]
    pub weather: PluginEnabledConfig,
    #[serde(default)]
    pub web: WebPluginConfig,
    #[serde(default)]
    pub web_images: WebImagesPluginConfig,
    #[serde(default)]
    pub deep_research: DeepResearchPluginConfig,
    #[serde(default)]
    pub deep_diagnose: DeepDiagnosePluginConfig,
    #[serde(default)]
    pub vision: VisionPluginConfig,
    #[serde(default)]
    pub exchange_rate: ExchangeRatePluginConfig,
    #[serde(default)]
    pub xuanxue: PluginEnabledConfig,
    #[serde(default)]
    pub image_generation: ImageGenerationPluginConfig,
    #[serde(default)]
    pub print_image: PrintImagePluginConfig,
    #[serde(default)]
    pub memes: MemesPluginConfig,
    #[serde(default)]
    pub knowledge_base: KnowledgeBasePluginConfig,
    #[serde(default)]
    pub archlinux: PluginEnabledConfig,
    #[serde(default)]
    pub man: PluginEnabledConfig,
    #[serde(default)]
    pub moegirl: PluginEnabledConfig,
    #[serde(default)]
    pub hash_codec: PluginEnabledConfig,
    #[serde(default)]
    pub calculator: CalculatorPluginConfig,
    #[serde(default)]
    pub package_advisor: PluginEnabledConfig,
    #[serde(default, alias = "linux_game_compatibility")]
    pub deep_research_linux_game_compatibility: LinuxGameCompatibilityConfig,
    #[serde(default)]
    pub diagnostics: DiagnosticsPluginConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginEnabledConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinuxGameCompatibilityConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_subagent_max_tool_steps")]
    pub max_tool_steps: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebPluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_web_search_max_results")]
    pub max_results: usize,
    #[serde(default)]
    pub tavily_api_keys: Vec<String>,
    #[serde(default)]
    pub firecrawl_api_keys: Vec<String>,
    #[serde(default)]
    pub anysearch_api_keys: Vec<String>,
    #[serde(default)]
    pub searxng_base_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebImagesPluginConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_web_images_source_mode")]
    pub source_mode: String,
    #[serde(default = "default_web_images_max_results")]
    pub max_results: usize,
    #[serde(default = "default_web_images_max_download_mb")]
    pub max_download_mb: f64,
    #[serde(default = "default_true")]
    pub safe_search: bool,
    #[serde(default = "default_true")]
    pub vision_screening_enabled: bool,
    #[serde(default = "default_true")]
    pub auto_preview: bool,
    #[serde(default = "default_web_images_preview_count")]
    pub preview_count: usize,
    #[serde(default = "default_web_images_timeout")]
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepResearchPluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_deep_research_dir")]
    pub output_dir: String,
    #[serde(default = "default_deep_research_depth")]
    pub thinking_depth: String,
    #[serde(default = "default_deep_research_max_review_revisions")]
    pub max_review_revisions: usize,
    #[serde(default = "default_deep_research_max_tool_steps")]
    pub max_tool_steps_per_round: usize,
    #[serde(default)]
    pub max_final_answer_chars: usize,
    #[serde(default = "default_deep_research_tool_timeout")]
    pub tool_call_timeout_seconds: u64,
    #[serde(default = "default_true")]
    pub show_progress: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepDiagnosePluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_deep_research_depth")]
    pub thinking_depth: String,
    #[serde(default = "default_deep_research_max_review_revisions")]
    pub max_review_revisions: usize,
    #[serde(default = "default_deep_research_max_tool_steps")]
    pub max_tool_steps_per_round: usize,
    #[serde(default)]
    pub max_final_answer_chars: usize,
    #[serde(default = "default_deep_research_tool_timeout")]
    pub tool_call_timeout_seconds: u64,
    #[serde(default = "default_subagent_max_tool_steps")]
    pub max_tool_steps: usize,
    #[serde(default = "default_true")]
    pub show_progress: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionPluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub prefer_current_multimodal_model: bool,
    #[serde(default)]
    pub vision_provider_id: String,
    #[serde(default)]
    pub vision_model: String,
    #[serde(default = "default_true")]
    pub preview_with_chafa: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExchangeRatePluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_true")]
    pub free_fallback_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageGenerationPluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_image_generation_provider_type")]
    pub provider_type: String,
    #[serde(default = "default_openai_images_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub api_keys: Vec<String>,
    #[serde(default = "default_image_generation_model")]
    pub model: String,
    #[serde(default = "default_image_generation_aspect_ratio")]
    pub default_aspect_ratio: String,
    #[serde(default = "default_image_generation_resolution")]
    pub default_resolution: String,
    #[serde(default = "default_image_generation_output_dir")]
    pub output_dir: String,
    #[serde(default)]
    pub auto_print: bool,
    #[serde(default = "default_image_generation_timeout")]
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrintImagePluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_print_image_width_percent")]
    pub width_percent: u8,
    #[serde(default = "default_print_image_height_percent")]
    pub height_percent: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemesPluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub persona_libraries: HashMap<String, String>,
    #[serde(default = "default_memes_width_percent")]
    pub width_percent: u8,
    #[serde(default = "default_memes_height_percent")]
    pub height_percent: u8,
    #[serde(default = "default_memes_max_image_mb")]
    pub max_image_mb: u64,
    #[serde(default = "default_memes_search_max_results")]
    pub search_max_results: usize,
    #[serde(default)]
    pub allow_gif_animation: bool,
    #[serde(default)]
    pub auto_send_enabled: bool,
    #[serde(default = "default_memes_auto_send_probability")]
    pub auto_send_probability: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeBasePluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub data_dir: String,
    #[serde(default = "default_kb_max_search_results")]
    pub max_search_results: usize,
    #[serde(default = "default_kb_snippet_context_chars")]
    pub snippet_context_chars: usize,
    #[serde(default = "default_kb_proximity_window_chars")]
    pub proximity_window_chars: usize,
    #[serde(default = "default_kb_max_read_lines")]
    pub max_read_lines: usize,
    #[serde(default = "default_kb_max_file_size_kb")]
    pub max_file_size_kb: usize,
    #[serde(default = "default_kb_allowed_extensions")]
    pub allowed_extensions: String,
    #[serde(default = "default_kb_allowed_filenames")]
    pub allowed_filenames: String,
    #[serde(default = "default_true")]
    pub upload_tool_enabled: bool,
    #[serde(default = "default_true")]
    pub embedding_enabled: bool,
    #[serde(default)]
    pub embedding_provider_id: String,
    #[serde(default)]
    pub embedding_model: String,
    #[serde(default = "default_kb_semantic_chunk_chars")]
    pub semantic_chunk_chars: usize,
    #[serde(default = "default_kb_semantic_chunk_overlap")]
    pub semantic_chunk_overlap: usize,
    #[serde(default = "default_kb_semantic_top_k")]
    pub semantic_top_k: usize,
    #[serde(default = "default_kb_semantic_min_score")]
    pub semantic_min_score: f32,
    #[serde(default = "default_kb_keyword_strong_score_threshold")]
    pub keyword_strong_score_threshold: f32,
    #[serde(default = "default_kb_embedding_timeout_seconds")]
    pub embedding_timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalculatorPluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_calculator_backend")]
    pub backend: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsPluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_diagnostics_timeout")]
    pub command_timeout_seconds: u64,
    #[serde(default = "default_diagnostics_max_stdout_chars")]
    pub max_stdout_chars: usize,
    #[serde(default = "default_diagnostics_max_stderr_chars")]
    pub max_stderr_chars: usize,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            active_provider: OPENCODE_PROVIDER_ID.to_string(),
            active_provider_models: None,
            active_multimodal_provider_models: None,
            providers: ProviderConfig::default_templates(),
            context: ContextConfig::default(),
            tools: ToolsConfig::default(),
            mcp: McpConfig::default(),
            skills: SkillsConfig::default(),
            display: DisplayConfig::default(),
            prompt: PromptConfig::default(),
            plugins: PluginsConfig::default(),
            memory: MemoryConfig::default(),
            system_prompt_file: Some("system-prompt.md".to_string()),
            system_prompt: None,
            subagent_tiers: SubagentTiersConfig::default(),
            platforms: PlatformsConfig::default(),
        }
    }
}

impl Default for PromptConfig {
    fn default() -> Self {
        Self {
            prompts_dir: default_prompts_dir(),
            identities_dir: default_identities_dir(),
            user_identity_file: default_user_identity_file(),
            active_persona: String::new(),
            active_identity: String::new(),
        }
    }
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            language: default_display_language(),
            reasoning: default_reasoning_display(),
            tool_calls: default_tool_call_display(),
            readable_tool_names: default_true(),
            show_token_usage: false,
            mixed_model_endpoint_display: default_mixed_model_endpoint_display(),
            command_output_lines: default_command_output_lines(),
        }
    }
}

impl Default for PluginsConfig {
    fn default() -> Self {
        Self {
            weather: PluginEnabledConfig::default(),
            web: WebPluginConfig::default(),
            web_images: WebImagesPluginConfig::default(),
            deep_research: DeepResearchPluginConfig::default(),
            deep_diagnose: DeepDiagnosePluginConfig::default(),
            vision: VisionPluginConfig::default(),
            exchange_rate: ExchangeRatePluginConfig::default(),
            xuanxue: PluginEnabledConfig::default(),
            image_generation: ImageGenerationPluginConfig::default(),
            print_image: PrintImagePluginConfig::default(),
            memes: MemesPluginConfig::default(),
            knowledge_base: KnowledgeBasePluginConfig::default(),
            archlinux: PluginEnabledConfig::default(),
            man: PluginEnabledConfig::default(),
            moegirl: PluginEnabledConfig::default(),
            hash_codec: PluginEnabledConfig::default(),
            calculator: CalculatorPluginConfig::default(),
            package_advisor: PluginEnabledConfig::default(),
            deep_research_linux_game_compatibility: LinuxGameCompatibilityConfig::default(),
            diagnostics: DiagnosticsPluginConfig::default(),
            memory: MemoryConfig::default(),
        }
    }
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            servers: Vec::new(),
        }
    }
}

impl Default for PluginEnabledConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
        }
    }
}

impl Default for LinuxGameCompatibilityConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            max_tool_steps: default_subagent_max_tool_steps(),
        }
    }
}

impl Default for WebPluginConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            max_results: default_web_search_max_results(),
            tavily_api_keys: Vec::new(),
            firecrawl_api_keys: Vec::new(),
            anysearch_api_keys: Vec::new(),
            searxng_base_url: String::new(),
        }
    }
}

impl Default for WebImagesPluginConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            source_mode: default_web_images_source_mode(),
            max_results: default_web_images_max_results(),
            max_download_mb: default_web_images_max_download_mb(),
            safe_search: default_true(),
            vision_screening_enabled: default_true(),
            auto_preview: default_true(),
            preview_count: default_web_images_preview_count(),
            timeout_seconds: default_web_images_timeout(),
        }
    }
}

impl Default for DeepResearchPluginConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            output_dir: default_deep_research_dir(),
            thinking_depth: default_deep_research_depth(),
            max_review_revisions: default_deep_research_max_review_revisions(),
            max_tool_steps_per_round: default_deep_research_max_tool_steps(),
            max_final_answer_chars: 0,
            tool_call_timeout_seconds: default_deep_research_tool_timeout(),
            show_progress: default_true(),
        }
    }
}

impl Default for DeepDiagnosePluginConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            thinking_depth: default_deep_research_depth(),
            max_review_revisions: default_deep_research_max_review_revisions(),
            max_tool_steps_per_round: default_deep_research_max_tool_steps(),
            max_final_answer_chars: 0,
            tool_call_timeout_seconds: default_deep_research_tool_timeout(),
            max_tool_steps: default_subagent_max_tool_steps(),
            show_progress: default_true(),
        }
    }
}

impl Default for VisionPluginConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            prefer_current_multimodal_model: default_true(),
            vision_provider_id: String::new(),
            vision_model: String::new(),
            preview_with_chafa: default_true(),
        }
    }
}

impl Default for ExchangeRatePluginConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: String::new(),
            free_fallback_enabled: default_true(),
        }
    }
}

impl Default for ImageGenerationPluginConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider_type: default_image_generation_provider_type(),
            base_url: default_openai_images_base_url(),
            api_keys: Vec::new(),
            model: default_image_generation_model(),
            default_aspect_ratio: default_image_generation_aspect_ratio(),
            default_resolution: default_image_generation_resolution(),
            output_dir: default_image_generation_output_dir(),
            auto_print: default_true(),
            timeout_seconds: default_image_generation_timeout(),
        }
    }
}

impl Default for PrintImagePluginConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            width_percent: default_print_image_width_percent(),
            height_percent: default_print_image_height_percent(),
        }
    }
}

impl Default for MemesPluginConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            persona_libraries: HashMap::new(),
            width_percent: default_memes_width_percent(),
            height_percent: default_memes_height_percent(),
            max_image_mb: default_memes_max_image_mb(),
            search_max_results: default_memes_search_max_results(),
            allow_gif_animation: false,
            auto_send_enabled: false,
            auto_send_probability: default_memes_auto_send_probability(),
        }
    }
}

impl MemesPluginConfig {
    pub fn library_for_persona(&self, persona: &str) -> String {
        if persona.trim().is_empty() {
            return self
                .persona_libraries
                .get("default")
                .cloned()
                .unwrap_or_else(|| "miyu".to_string());
        }
        let persona = persona_scope_name(persona);
        self.persona_libraries
            .get(&persona)
            .cloned()
            .unwrap_or(persona)
    }
}

impl Default for KnowledgeBasePluginConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            data_dir: String::new(),
            max_search_results: default_kb_max_search_results(),
            snippet_context_chars: default_kb_snippet_context_chars(),
            proximity_window_chars: default_kb_proximity_window_chars(),
            max_read_lines: default_kb_max_read_lines(),
            max_file_size_kb: default_kb_max_file_size_kb(),
            allowed_extensions: default_kb_allowed_extensions(),
            allowed_filenames: default_kb_allowed_filenames(),
            upload_tool_enabled: default_true(),
            embedding_enabled: false,
            embedding_provider_id: String::new(),
            embedding_model: String::new(),
            semantic_chunk_chars: default_kb_semantic_chunk_chars(),
            semantic_chunk_overlap: default_kb_semantic_chunk_overlap(),
            semantic_top_k: default_kb_semantic_top_k(),
            semantic_min_score: default_kb_semantic_min_score(),
            keyword_strong_score_threshold: default_kb_keyword_strong_score_threshold(),
            embedding_timeout_seconds: default_kb_embedding_timeout_seconds(),
        }
    }
}

impl Default for CalculatorPluginConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: default_calculator_backend(),
        }
    }
}

impl Default for DiagnosticsPluginConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            command_timeout_seconds: default_diagnostics_timeout(),
            max_stdout_chars: default_diagnostics_max_stdout_chars(),
            max_stderr_chars: default_diagnostics_max_stderr_chars(),
        }
    }
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            max_rounds: 0,
            loading_mode: default_tools_loading_mode(),
            persist_loaded_tools: default_true(),
            subagent_concurrency: default_subagent_concurrency(),
        }
    }
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            allow_command_execution: default_true(),
        }
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            evicted_context_enabled: default_true(),
            association_enabled: default_true(),
            auto_diary_enabled: default_true(),
            auto_fact_enabled: default_true(),
            auto_skill_enabled: false,
            association_facts: default_memory_association_facts(),
            association_episodes: default_memory_association_episodes(),
            association_max_chars: default_memory_association_max_chars(),
            snippet_chars: default_memory_snippet_chars(),
            forget_after_days: default_memory_forget_after_days(),
            forgetting_enabled: default_true(),
            forgetting_half_life_days: default_memory_half_life_days(),
            forgetting_min_strength: default_memory_min_strength(),
            forgetting_review_boost: default_memory_review_boost(),
            learning_min_task_chars: default_memory_min_task_chars(),
            learning_min_method_chars: default_memory_min_method_chars(),
        }
    }
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            trim_at_ratio: default_trim_at_ratio(),
            trim_batch_ratio: default_trim_batch_ratio(),
            on_overflow: default_on_overflow(),
            default_context_window: default_context_window(),
        }
    }
}

impl ProviderConfig {
    pub fn default_opencodezen() -> Self {
        Self {
            id: OPENCODE_PROVIDER_ID.to_string(),
            display_name: "opencode Zen".to_string(),
            base_url: OPENCODE_ZEN_BASE_URL.to_string(),
            protocol: default_provider_protocol(),
            api_key: None,
            models: vec![OPENCODE_DEFAULT_CHAT_MODEL.to_string()],
            model_context_window: HashMap::new(),
            model_modalities: HashMap::new(),
            default_model: OPENCODE_DEFAULT_CHAT_MODEL.to_string(),
            timeout_seconds: default_timeout(),
            temperature: default_temperature(),
            anthropic_max_tokens: default_anthropic_max_tokens(),
            extra_body: None,
        }
    }

    pub fn default_anthropic() -> Self {
        Self {
            id: "anthropic".to_string(),
            display_name: "Anthropic".to_string(),
            base_url: "https://api.anthropic.com/v1".to_string(),
            protocol: "anthropic".to_string(),
            api_key: Some("$env:ANTHROPIC_API_KEY".to_string()),
            models: Vec::new(),
            model_context_window: HashMap::new(),
            model_modalities: HashMap::new(),
            default_model: String::new(),
            timeout_seconds: default_timeout(),
            temperature: default_temperature(),
            anthropic_max_tokens: default_anthropic_max_tokens(),
            extra_body: None,
        }
    }

    pub fn default_templates() -> Vec<Self> {
        let mut providers = vec![Self::default_opencodezen()];
        providers.extend([
            Self::template("openai", "OpenAI", "https://api.openai.com/v1"),
            Self::default_anthropic(),
            Self::template("deepseek", "DeepSeek", "https://api.deepseek.com"),
            Self::template(
                "gemini",
                "Gemini",
                "https://generativelanguage.googleapis.com/v1beta/openai",
            ),
            Self::template(
                "xiaomi",
                "Xiaomi",
                "https://token-plan-sgp.xiaomimimo.com/v1",
            ),
            Self::template("minimax", "Minimax", "https://api.minimaxi.com/v1"),
            Self::template("openrouter", "OpenRouter", "https://openrouter.ai/api/v1"),
            Self::template("ollama", "Ollama", "http://localhost:11434/v1"),
            Self::template("lmstudio", "LMStudio", "http://localhost:1234/v1"),
        ]);
        providers
    }

    fn template(id: &str, display_name: &str, base_url: &str) -> Self {
        Self {
            id: id.to_string(),
            display_name: display_name.to_string(),
            base_url: base_url.to_string(),
            protocol: default_provider_protocol(),
            api_key: None,
            models: Vec::new(),
            model_context_window: HashMap::new(),
            model_modalities: HashMap::new(),
            default_model: String::new(),
            timeout_seconds: default_timeout(),
            temperature: default_temperature(),
            anthropic_max_tokens: default_anthropic_max_tokens(),
            extra_body: None,
        }
    }

    pub fn new_custom() -> Self {
        Self {
            id: String::new(),
            display_name: String::new(),
            base_url: String::new(),
            protocol: default_provider_protocol(),
            api_key: None,
            models: Vec::new(),
            model_context_window: HashMap::new(),
            model_modalities: HashMap::new(),
            default_model: String::new(),
            timeout_seconds: default_timeout(),
            temperature: default_temperature(),
            anthropic_max_tokens: default_anthropic_max_tokens(),
            extra_body: None,
        }
    }

    pub fn supports_vision(&self, model: &str) -> Option<bool> {
        self.input_modalities(model)
            .map(|modalities| modalities.iter().any(|m| m == "image"))
    }

    pub fn input_modalities(&self, model: &str) -> Option<Vec<String>> {
        if let Some(modalities) = self.model_modalities.get(model) {
            return Some(modalities.clone());
        }
        crate::models_cache::input_modalities(&self.id, model)
    }

    pub fn resolved_api_keys(&self, _paths: &MiyuPaths) -> Result<Vec<ResolvedProviderKey>> {
        let mut keys = Vec::new();
        if let Some(api_key) = self.api_key.as_deref() {
            append_resolved_api_keys(&mut keys, api_key)?;
        }

        if keys.is_empty() && self.is_opencode_zen() {
            keys.push(ResolvedProviderKey {
                index: 0,
                value: "public".to_string(),
            });
        }

        if keys.is_empty() {
            bail!("missing API key for provider {}", self.id)
        }
        for (index, key) in keys.iter_mut().enumerate() {
            key.index = index;
        }
        Ok(keys)
    }

    pub fn is_opencode_zen(&self) -> bool {
        matches!(self.id.as_str(), OPENCODE_PROVIDER_ID | "opencodezen")
            && self.base_url.trim_end_matches('/') == OPENCODE_ZEN_BASE_URL
    }

    fn has_configured_model(&self, model: &str) -> bool {
        let model = model.trim();
        !model.is_empty()
            && (self.default_model == model || self.models.iter().any(|item| item == model))
    }

    fn is_legacy_default_anthropic_model(&self) -> bool {
        self.id == "anthropic"
            && self.base_url.trim_end_matches('/') == "https://api.anthropic.com/v1"
            && self.protocol == "anthropic"
            && self.api_key.as_deref() == Some("$env:ANTHROPIC_API_KEY")
            && self.models == ["claude-sonnet-4-5"]
            && self.default_model == "claude-sonnet-4-5"
    }
}

fn append_resolved_api_keys(out: &mut Vec<ResolvedProviderKey>, raw: &str) -> Result<()> {
    for item in split_api_keys(raw) {
        let value = if let Some(env_name) = item.strip_prefix("$env:") {
            std::env::var(env_name)
                .with_context(|| format!("environment variable {env_name} is not set"))?
        } else {
            item.to_string()
        };
        let value = value.trim();
        if !value.is_empty() {
            out.push(ResolvedProviderKey {
                index: out.len(),
                value: value.to_string(),
            });
        }
    }
    Ok(())
}

fn split_api_keys(raw: &str) -> Vec<&str> {
    raw.lines()
        .flat_map(|line| line.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect()
}

fn active_model_exists(providers: &[ProviderConfig], active: &ActiveProviderModelConfig) -> bool {
    providers
        .iter()
        .find(|provider| provider.id == active.provider_id.trim())
        .is_some_and(|provider| provider.has_configured_model(&active.model))
}

fn active_model_supports_image(
    providers: &[ProviderConfig],
    active: &ActiveProviderModelConfig,
) -> bool {
    providers
        .iter()
        .find(|provider| provider.id == active.provider_id.trim())
        .filter(|provider| provider.has_configured_model(&active.model))
        .and_then(|provider| provider.input_modalities(&active.model))
        .is_some_and(|modalities| modalities.iter().any(|input| input == "image"))
}

fn validate_unique_existing_pool(
    providers: &[ProviderConfig],
    label: &str,
    pool: &[ActiveProviderModelConfig],
    require_image: bool,
) -> Result<()> {
    let mut seen = HashSet::with_capacity(pool.len());
    for entry in pool {
        if !seen.insert((entry.provider_id.as_str(), entry.model.as_str())) {
            bail!(
                "duplicate {label} model: {} / {}",
                entry.provider_id,
                entry.model
            );
        }
        let valid = if require_image {
            active_model_supports_image(providers, entry)
        } else {
            active_model_exists(providers, entry)
        };
        if !valid {
            let requirement = if require_image {
                "configured image-capable"
            } else {
                "configured"
            };
            bail!(
                "unknown or non-{requirement} {label} model: {} / {}",
                entry.provider_id,
                entry.model
            );
        }
    }
    Ok(())
}

fn is_positive_decimal_id(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<u64>().is_ok_and(|id| id > 0)
}

impl AppConfig {
    pub fn display_language_hint(paths: &MiyuPaths) -> Option<String> {
        let raw = std::fs::read_to_string(&paths.config_file).ok()?;
        let stripped = json_comments::StripComments::new(raw.as_bytes());
        let value: serde_json::Value = serde_json::from_reader(stripped).ok()?;
        value
            .get("display")?
            .get("language")?
            .as_str()
            .map(str::to_string)
    }

    pub fn memory_config(&self) -> &MemoryConfig {
        if self.memory != MemoryConfig::default() {
            &self.memory
        } else {
            &self.plugins.memory
        }
    }

    pub fn load(paths: &MiyuPaths) -> Result<Self> {
        // Platform multimodal routes may rely on cached models.dev
        // capabilities. Load the full cache before validation; callers can
        // compact it to their active configuration afterwards.
        crate::models_cache::try_load(paths);
        let raw = std::fs::read_to_string(&paths.config_file)
            .with_context(|| format!("failed to read {}", paths.config_file.display()))?;
        let stripped = json_comments::StripComments::new(raw.as_bytes());
        let mut config: Self = serde_json::from_reader(stripped)
            .with_context(|| format!("invalid JSONC in {}", paths.config_file.display()))?;
        config.normalize_builtin_providers();
        config.validate()?;
        Ok(config)
    }

    pub fn load_or_default(paths: &MiyuPaths) -> Result<Self> {
        if paths.config_file.exists() {
            Self::load(paths)
        } else {
            Ok(Self::default())
        }
    }

    pub fn init_files(paths: &MiyuPaths) -> Result<()> {
        paths.create_dirs()?;
        if !paths.config_file.exists() {
            Self::default().save(paths)?;
        }
        Ok(())
    }

    pub fn save(&self, paths: &MiyuPaths) -> Result<()> {
        let mut config = self.clone();
        config.normalize_platform_model_routes();
        let effective_memory = config.memory_config().clone();
        config.plugins.memory = effective_memory;
        config.memory = MemoryConfig::default();
        config.validate()?;
        paths.create_dirs()?;
        if let Some(prompt) = config.system_prompt.take() {
            let prompt_file = config.system_prompt_path(paths);
            if let Some(parent) = prompt_file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let prompt = prompt.trim_end();
            let content = if prompt.is_empty() {
                String::new()
            } else {
                format!("{prompt}\n")
            };
            std::fs::write(prompt_file, content)?;
        }
        if config
            .system_prompt_file
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            config.system_prompt_file = Some("system-prompt.md".to_string());
        }
        let raw = serde_json::to_string_pretty(&config)?;
        std::fs::write(&paths.config_file, format!("{raw}\n"))?;
        Ok(())
    }

    fn normalize_builtin_providers(&mut self) {
        for provider in ProviderConfig::default_templates() {
            if !self.providers.iter().any(|item| {
                item.id == provider.id
                    || provider.id == OPENCODE_PROVIDER_ID && item.is_opencode_zen()
            }) {
                self.providers.push(provider);
            }
        }
        if self.active_provider == "opencodezen" {
            self.active_provider = OPENCODE_PROVIDER_ID.to_string();
        }
        for provider in &mut self.providers {
            if provider.is_legacy_default_anthropic_model() {
                provider.models.clear();
                provider.default_model.clear();
            }
        }
        if let Some(active_models) = &mut self.active_provider_models {
            for active in active_models {
                if active.provider_id == "opencodezen" {
                    active.provider_id = OPENCODE_PROVIDER_ID.to_string();
                }
            }
        }
        if let Some(active_models) = &mut self.active_multimodal_provider_models {
            for active in active_models {
                if active.provider_id == "opencodezen" {
                    active.provider_id = OPENCODE_PROVIDER_ID.to_string();
                }
            }
        }
        self.platforms
            .rename_provider_references("opencodezen", OPENCODE_PROVIDER_ID);
        self.prune_stale_active_provider_models();
        self.normalize_platform_model_routes();
        if self.plugins.vision.vision_provider_id == "opencodezen" {
            self.plugins.vision.vision_provider_id = OPENCODE_PROVIDER_ID.to_string();
        }
        if self
            .provider(None)
            .map(|provider| provider.default_model.trim().is_empty())
            .unwrap_or(true)
        {
            self.active_provider = OPENCODE_PROVIDER_ID.to_string();
        }
        if self
            .active_provider_models
            .as_ref()
            .is_some_and(Vec::is_empty)
        {
            self.active_provider_models = Some(vec![ActiveProviderModelConfig {
                provider_id: OPENCODE_PROVIDER_ID.to_string(),
                model: OPENCODE_DEFAULT_CHAT_MODEL.to_string(),
            }]);
        }
    }

    fn prune_stale_active_provider_models(&mut self) {
        if let Some(active_models) = &mut self.active_provider_models {
            active_models.retain(|active| active_model_exists(&self.providers, active));
        }
        if let Some(active_models) = &mut self.active_multimodal_provider_models {
            active_models.retain(|active| active_model_supports_image(&self.providers, active));
        }
    }

    pub fn validate(&self) -> Result<()> {
        if crate::i18n::UiLanguage::parse(&self.display.language).is_none() {
            bail!(
                "{}",
                crate::i18n::text(
                    "display.language must be 'auto', 'en', or 'zh'",
                    "display.language 必须是 'auto'、'en' 或 'zh'"
                )
            );
        }
        if self.active_provider.trim().is_empty() {
            bail!("active_provider cannot be empty");
        }
        if self.providers.is_empty() {
            bail!("at least one provider is required");
        }
        let mut provider_ids = HashSet::with_capacity(self.providers.len());
        for provider in &self.providers {
            if provider.id.trim().is_empty() {
                bail!("provider id cannot be empty");
            }
            if provider.id.trim() != provider.id {
                bail!(
                    "provider id must not contain surrounding whitespace: {}",
                    provider.id
                );
            }
            if !provider_ids.insert(provider.id.as_str()) {
                bail!("duplicate provider id: {}", provider.id);
            }
            if provider.base_url.trim().is_empty() {
                bail!("provider {} base_url cannot be empty", provider.id);
            }
        }
        if !(0.1..=1.0).contains(&self.context.trim_at_ratio) {
            bail!("context.trim_at_ratio must be between 0.1 and 1.0");
        }
        if !(0.01..=0.9).contains(&self.context.trim_batch_ratio) {
            bail!("context.trim_batch_ratio must be between 0.01 and 0.9");
        }
        match self.context.on_overflow.as_str() {
            "pop" | "compact" => {}
            value => bail!("context.on_overflow must be 'pop' or 'compact', got: {value}"),
        }
        if self.display.command_output_lines > MAX_COMMAND_OUTPUT_LINES {
            bail!("display.command_output_lines must be between 0 and {MAX_COMMAND_OUTPUT_LINES}");
        }
        if self.plugins.print_image.width_percent == 0
            || self.plugins.print_image.width_percent > 100
        {
            bail!("plugins.print_image.width_percent must be between 1 and 100");
        }
        if self.plugins.print_image.height_percent == 0
            || self.plugins.print_image.height_percent > 100
        {
            bail!("plugins.print_image.height_percent must be between 1 and 100");
        }
        if self.plugins.web.max_results == 0 {
            bail!("plugins.web.max_results must be greater than 0");
        }
        match self.plugins.deep_research.thinking_depth.as_str() {
            "minimal" | "low" | "medium" | "high" | "xhigh" => {}
            value => bail!("plugins.deep_research.thinking_depth is invalid: {value}"),
        }
        match self.plugins.deep_diagnose.thinking_depth.as_str() {
            "minimal" | "low" | "medium" | "high" | "xhigh" => {}
            value => bail!("plugins.deep_diagnose.thinking_depth is invalid: {value}"),
        }
        if self.plugins.deep_diagnose.tool_call_timeout_seconds == 0 {
            bail!("plugins.deep_diagnose.tool_call_timeout_seconds must be greater than 0");
        }
        match self.plugins.image_generation.provider_type.as_str() {
            "openai" | "rightcode" => {}
            value => bail!("plugins.image_generation.provider_type is invalid: {value}"),
        }
        match self.plugins.image_generation.default_aspect_ratio.as_str() {
            "自动" | "1:1" | "2:3" | "3:2" | "3:4" | "4:3" | "4:5" | "5:4" | "9:16" | "16:9"
            | "21:9" => {}
            value => bail!("plugins.image_generation.default_aspect_ratio is invalid: {value}"),
        }
        match self.plugins.image_generation.default_resolution.as_str() {
            "1K" | "2K" | "4K" => {}
            value => bail!("plugins.image_generation.default_resolution is invalid: {value}"),
        }
        if self.plugins.image_generation.timeout_seconds == 0 {
            bail!("plugins.image_generation.timeout_seconds must be greater than 0");
        }
        if self.plugins.knowledge_base.max_search_results == 0 {
            bail!("plugins.knowledge_base.max_search_results must be greater than 0");
        }
        if self.plugins.knowledge_base.max_read_lines == 0 {
            bail!("plugins.knowledge_base.max_read_lines must be greater than 0");
        }
        if self.plugins.knowledge_base.max_file_size_kb == 0 {
            bail!("plugins.knowledge_base.max_file_size_kb must be greater than 0");
        }
        if self.plugins.knowledge_base.semantic_chunk_chars < 128 {
            bail!("plugins.knowledge_base.semantic_chunk_chars must be at least 128");
        }
        if self.plugins.knowledge_base.semantic_chunk_overlap
            >= self.plugins.knowledge_base.semantic_chunk_chars
        {
            bail!("plugins.knowledge_base.semantic_chunk_overlap must be smaller than semantic_chunk_chars");
        }
        if self.plugins.knowledge_base.semantic_top_k == 0 {
            bail!("plugins.knowledge_base.semantic_top_k must be greater than 0");
        }
        if self.plugins.knowledge_base.embedding_timeout_seconds == 0 {
            bail!("plugins.knowledge_base.embedding_timeout_seconds must be greater than 0");
        }
        if !(0.0..=2.0).contains(&self.provider(None)?.temperature) {
            bail!("provider temperature must be between 0.0 and 2.0");
        }
        for provider in &self.providers {
            if provider.timeout_seconds == 0 {
                bail!(
                    "provider {} timeout_seconds must be greater than 0",
                    provider.id
                );
            }
            if !(0.0..=2.0).contains(&provider.temperature) {
                bail!(
                    "provider {} temperature must be between 0.0 and 2.0",
                    provider.id
                );
            }
            if provider.anthropic_max_tokens == 0 {
                bail!(
                    "provider {} anthropic_max_tokens must be greater than 0",
                    provider.id
                );
            }
        }
        if !(0.0..=1.0).contains(&self.plugins.memes.auto_send_probability) {
            bail!("plugins.memes.auto_send_probability must be between 0.0 and 1.0");
        }
        if self.plugins.memes.width_percent == 0 || self.plugins.memes.width_percent > 100 {
            bail!("plugins.memes.width_percent must be between 1 and 100");
        }
        if self.plugins.memes.height_percent == 0 || self.plugins.memes.height_percent > 100 {
            bail!("plugins.memes.height_percent must be between 1 and 100");
        }
        if self.plugins.memes.search_max_results == 0 || self.plugins.memes.search_max_results > 3 {
            bail!("plugins.memes.search_max_results must be between 1 and 3");
        }
        let mem = self.memory_config();
        if mem.forgetting_half_life_days <= 0.0 {
            bail!("memory.forgetting_half_life_days must be greater than 0");
        }
        if mem.forget_after_days == 0 {
            bail!("memory.forget_after_days must be greater than 0");
        }
        if !(0.0..=1.0).contains(&self.plugins.knowledge_base.semantic_min_score) {
            bail!("plugins.knowledge_base.semantic_min_score must be between 0.0 and 1.0");
        }
        self.validate_model_references()?;
        self.validate_global_multimodal_config()?;
        self.validate_platforms()?;
        self.provider(None)?;
        Ok(())
    }

    fn validate_model_references(&self) -> Result<()> {
        if let Some(pool) = &self.active_provider_models {
            if pool.is_empty() {
                bail!("at least one model endpoint must remain active");
            }
            validate_unique_existing_pool(&self.providers, "active text", pool, false)?;
        }
        let kb_provider = self.plugins.knowledge_base.embedding_provider_id.trim();
        if !kb_provider.is_empty() {
            self.provider(Some(kb_provider))?;
        }
        Ok(())
    }

    fn validate_global_multimodal_config(&self) -> Result<()> {
        if let Some(pool) = &self.active_multimodal_provider_models {
            validate_unique_existing_pool(&self.providers, "active multimodal", pool, true)?;
        }
        if self.plugins.vision.enabled && !self.plugins.vision.vision_provider_id.trim().is_empty()
        {
            self.vision_provider_choice()?;
        }
        Ok(())
    }

    fn validate_platforms(&self) -> Result<()> {
        let command_prefix = &self.platforms.command_prefix;
        if command_prefix.is_empty()
            || command_prefix.trim() != command_prefix
            || command_prefix.chars().count() > MAX_PLATFORM_COMMAND_PREFIX_CHARS
            || command_prefix
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
        {
            bail!(
                "platforms.command_prefix must be a trimmed, non-empty value of at most {MAX_PLATFORM_COMMAND_PREFIX_CHARS} characters without whitespace"
            );
        }
        for command in self.platforms.commands.keys() {
            if command.is_empty()
                || command.len() > 64
                || !command.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'_' | b'-')
                })
            {
                bail!(
                    "platforms.commands keys must be lowercase ASCII command ids of at most 64 bytes"
                );
            }
        }
        let qq = &self.platforms.qq;
        if qq.reverse_ws_port == 0 {
            bail!("platforms.qq.reverse_ws_port must be between 1 and 65535");
        }
        for (field, ids) in [
            ("admin_users", qq.admin_users.as_slice()),
            (
                "private_chats.whitelist",
                qq.private_chats.whitelist.as_slice(),
            ),
            ("group_chats.whitelist", qq.group_chats.whitelist.as_slice()),
        ] {
            let mut seen = HashSet::with_capacity(ids.len());
            if ids.iter().any(|id| *id <= 0 || !seen.insert(*id)) {
                bail!("platforms.qq.{field} must contain unique positive QQ ids");
            }
        }
        let mut trigger_keywords = HashSet::with_capacity(qq.group_chats.trigger_keywords.len());
        for keyword in &qq.group_chats.trigger_keywords {
            if keyword.is_empty()
                || keyword.trim() != keyword
                || keyword.chars().count() > 128
                || keyword.chars().any(char::is_control)
                || !trigger_keywords.insert(keyword)
            {
                bail!(
                    "platforms.qq.group_chats.trigger_keywords must contain unique, trimmed, non-empty values of at most 128 characters"
                );
            }
        }
        let mut identities = HashSet::with_capacity(qq.conversations.len());
        for route in &qq.conversations {
            self.validate_platform_model_route(route)?;
            if !identities.insert(route.identity()) {
                bail!(
                    "duplicate QQ conversation configuration: {} / {}",
                    route.conversation.kind.as_str(),
                    route.conversation.id
                );
            }
        }
        for (plugin_id, instance) in &qq.plugins {
            if plugin_id.trim().is_empty() || plugin_id.trim() != plugin_id {
                bail!("QQ plugin ids must be non-empty and trimmed");
            }
            if let Some((_, validate)) = PLATFORM_PLUGIN_VALIDATORS
                .iter()
                .find(|(id, _)| *id == plugin_id)
            {
                validate(instance)?;
            }
        }
        Ok(())
    }

    pub fn validate_platform_model_route(&self, route: &PlatformModelRoute) -> Result<()> {
        if !is_positive_decimal_id(&route.conversation.id) {
            let label = match route.conversation.kind {
                PlatformConversationKind::Private => "QQ id",
                PlatformConversationKind::Group => "group id",
            };
            bail!("QQ conversation id must be a positive decimal {label}");
        }
        if route.extra_prompt.chars().count() > 200_000 || route.extra_prompt.contains('\0') {
            bail!("QQ conversation extra_prompt is invalid or exceeds 200000 characters");
        }
        self.validate_platform_model_pool(
            route,
            "text_models",
            route.text_models.as_deref(),
            false,
        )?;
        self.validate_platform_model_pool(
            route,
            "multimodal_models",
            route.multimodal_models.as_deref(),
            true,
        )?;
        Ok(())
    }

    fn validate_platform_model_pool(
        &self,
        route: &PlatformModelRoute,
        field: &str,
        pool: Option<&[ActiveProviderModelConfig]>,
        require_multimodal: bool,
    ) -> Result<()> {
        let Some(pool) = pool else {
            return Ok(());
        };
        let mut seen = HashSet::with_capacity(pool.len());
        for entry in pool {
            if !seen.insert((entry.provider_id.as_str(), entry.model.as_str())) {
                bail!(
                    "duplicate {} model in platform route: {} / {}",
                    field,
                    entry.provider_id,
                    entry.model
                );
            }
            if !active_model_exists(&self.providers, entry) {
                bail!(
                    "unknown {} provider/model in QQ conversation {} / {}: {} / {}",
                    field,
                    route.conversation.kind.as_str(),
                    route.conversation.id,
                    entry.provider_id,
                    entry.model
                );
            }
            if require_multimodal
                && !self.model_supports_any_input(&entry.provider_id, &entry.model, &["image"])
            {
                bail!(
                    "platform route multimodal model does not declare image input: {} / {}",
                    entry.provider_id,
                    entry.model
                );
            }
        }
        Ok(())
    }

    pub fn platform_model_route(
        &self,
        kind: PlatformConversationKind,
        conversation_id: &str,
    ) -> Option<&PlatformModelRoute> {
        self.platforms.model_route(kind, conversation_id)
    }

    pub fn normalize_platform_model_routes(&mut self) {
        self.platforms.normalize_model_routes();
    }

    pub fn prune_platform_model_routes(&mut self) {
        self.platforms.prune_model_references(&self.providers);
    }

    pub fn rename_platform_provider_references(&mut self, old_id: &str, new_id: &str) {
        self.platforms.rename_provider_references(old_id, new_id);
    }

    pub fn rename_platform_model_references(&mut self, provider_id: &str, old: &str, new: &str) {
        self.platforms
            .rename_model_references(provider_id, old, new);
    }

    pub fn rename_provider_references(&mut self, old_id: &str, new_id: &str) {
        if old_id == new_id || old_id.is_empty() || new_id.is_empty() {
            return;
        }
        if self.active_provider == old_id {
            self.active_provider = new_id.to_string();
        }
        for entries in [
            self.active_provider_models.as_mut(),
            self.active_multimodal_provider_models.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            rename_provider_in_pool(entries, old_id, new_id);
        }
        for tier in ModelTier::ALL {
            rename_provider_in_pool(self.subagent_tiers.pool_mut(tier), old_id, new_id);
        }
        self.platforms.rename_provider_references(old_id, new_id);
        if self.plugins.vision.vision_provider_id == old_id {
            self.plugins.vision.vision_provider_id = new_id.to_string();
        }
        if self.plugins.knowledge_base.embedding_provider_id == old_id {
            self.plugins.knowledge_base.embedding_provider_id = new_id.to_string();
        }
    }

    /// Removes references after a provider has been deleted from `providers`.
    pub fn remove_provider_references(&mut self, provider_id: &str) {
        retain_provider_pool(&mut self.active_provider_models, provider_id);
        retain_provider_pool(&mut self.active_multimodal_provider_models, provider_id);
        for tier in ModelTier::ALL {
            self.subagent_tiers
                .pool_mut(tier)
                .retain(|entry| entry.provider_id != provider_id);
        }
        self.platforms.remove_provider_references(provider_id);
        if self.plugins.vision.vision_provider_id == provider_id {
            self.plugins.vision.vision_provider_id.clear();
            self.plugins.vision.vision_model.clear();
        }
        if self.plugins.knowledge_base.embedding_provider_id == provider_id {
            self.plugins.knowledge_base.embedding_provider_id.clear();
            self.plugins.knowledge_base.embedding_model.clear();
        }
        if self.active_provider == provider_id {
            self.active_provider = self
                .active_provider_models
                .as_ref()
                .and_then(|pool| pool.first())
                .map(|entry| entry.provider_id.clone())
                .or_else(|| {
                    self.providers
                        .iter()
                        .find(|provider| !provider.default_model.trim().is_empty())
                        .or_else(|| self.providers.first())
                        .map(|provider| provider.id.clone())
                })
                .unwrap_or_default();
        }
    }

    /// Reconciles every model reference with the current provider models and
    /// input capabilities after an editor changes model metadata.
    pub fn prune_model_references(&mut self) {
        self.prune_stale_active_provider_models();
        retain_nonempty_pool(&mut self.active_provider_models);
        retain_nonempty_pool(&mut self.active_multimodal_provider_models);
        self.prune_subagent_tiers();
        self.prune_platform_model_routes();

        let vision_provider_id = self.plugins.vision.vision_provider_id.trim();
        if !vision_provider_id.is_empty() {
            let vision_model = self.plugins.vision.vision_model.trim();
            let valid = self
                .provider(Some(vision_provider_id))
                .ok()
                .map(|provider| {
                    let model = if vision_model.is_empty() {
                        provider.default_model.as_str()
                    } else {
                        vision_model
                    };
                    provider
                        .input_modalities(model)
                        .is_some_and(|modalities| modalities.iter().any(|item| item == "image"))
                })
                .unwrap_or(false);
            if !valid {
                self.plugins.vision.vision_provider_id.clear();
                self.plugins.vision.vision_model.clear();
            }
        }

        let kb_provider_id = self.plugins.knowledge_base.embedding_provider_id.trim();
        if !kb_provider_id.is_empty() && self.provider(Some(kb_provider_id)).is_err() {
            self.plugins.knowledge_base.embedding_provider_id.clear();
            self.plugins.knowledge_base.embedding_model.clear();
        }
    }

    pub fn provider(&self, id: Option<&str>) -> Result<&ProviderConfig> {
        let target = id.unwrap_or(&self.active_provider);
        self.providers
            .iter()
            .find(|provider| provider.id == target)
            .with_context(|| format!("provider not found: {target}"))
    }

    pub fn provider_model_choices(&self) -> Vec<ProviderModelChoice> {
        self.providers
            .iter()
            .flat_map(|provider| {
                let models =
                    if provider.models.is_empty() && !provider.default_model.trim().is_empty() {
                        vec![provider.default_model.clone()]
                    } else {
                        provider.models.clone()
                    };
                models
                    .into_iter()
                    .filter(|model| !model.trim().is_empty())
                    .map(|model| ProviderModelChoice {
                        provider_id: provider.id.clone(),
                        provider_name: provider.display_name.clone(),
                        model,
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    pub fn text_provider_model_choices(&self) -> Vec<ProviderModelChoice> {
        self.providers
            .iter()
            .flat_map(|provider| {
                provider
                    .models
                    .iter()
                    .filter(|model| !model.trim().is_empty())
                    .map(|model| ProviderModelChoice {
                        provider_id: provider.id.clone(),
                        provider_name: provider.display_name.clone(),
                        model: model.clone(),
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    pub fn active_provider_model_choices(&self) -> Vec<ProviderModelChoice> {
        match &self.active_provider_models {
            None => self
                .provider(None)
                .ok()
                .filter(|provider| !provider.default_model.trim().is_empty())
                .map(|provider| {
                    vec![ProviderModelChoice {
                        provider_id: provider.id.clone(),
                        provider_name: provider.display_name.clone(),
                        model: provider.default_model.clone(),
                    }]
                })
                .unwrap_or_default(),
            Some(active_models) => active_models
                .iter()
                .filter_map(|active| {
                    let provider = self.provider(Some(active.provider_id.trim())).ok()?;
                    let model = active.model.trim();
                    provider
                        .has_configured_model(model)
                        .then(|| ProviderModelChoice {
                            provider_id: provider.id.clone(),
                            provider_name: provider.display_name.clone(),
                            model: model.to_string(),
                        })
                })
                .collect(),
        }
    }

    pub fn multimodal_provider_model_choices(&self) -> Vec<ProviderModelChoice> {
        self.text_provider_model_choices()
            .into_iter()
            .filter(|choice| {
                self.model_supports_any_input(&choice.provider_id, &choice.model, &["image"])
            })
            .collect()
    }

    pub fn active_multimodal_provider_model_choices(&self) -> Vec<ProviderModelChoice> {
        match &self.active_multimodal_provider_models {
            Some(active_models) => active_models
                .iter()
                .filter_map(|active| {
                    let provider = self.provider(Some(active.provider_id.trim())).ok()?;
                    let model = active.model.trim();
                    (provider.has_configured_model(model)
                        && provider.input_modalities(model).is_some_and(|modalities| {
                            modalities.iter().any(|item| item == "image")
                        }))
                    .then(|| ProviderModelChoice {
                        provider_id: provider.id.clone(),
                        provider_name: provider.display_name.clone(),
                        model: model.to_string(),
                    })
                })
                .collect(),
            None => Vec::new(),
        }
    }

    pub fn is_active_multimodal_provider_model(&self, provider_id: &str, model: &str) -> bool {
        self.active_multimodal_provider_models
            .as_ref()
            .map(|active_models| {
                active_models
                    .iter()
                    .any(|active| active.provider_id == provider_id && active.model == model)
            })
            .unwrap_or(false)
    }

    pub fn remove_active_model_references(&mut self, provider_id: &str, model: &str) {
        if let Some(active_models) = &mut self.active_provider_models {
            active_models
                .retain(|active| !(active.provider_id == provider_id && active.model == model));
        }
        if let Some(active_models) = &mut self.active_multimodal_provider_models {
            active_models
                .retain(|active| !(active.provider_id == provider_id && active.model == model));
        }
        // A model gone from the text models must leave every tier pool too.
        for tier in ModelTier::ALL {
            self.subagent_tiers
                .pool_mut(tier)
                .retain(|entry| !(entry.provider_id == provider_id && entry.model == model));
        }
        self.platforms.remove_model_references(provider_id, model);
        if self.plugins.vision.vision_provider_id == provider_id
            && self.plugins.vision.vision_model == model
        {
            self.plugins.vision.vision_provider_id.clear();
            self.plugins.vision.vision_model.clear();
        }
        if self.plugins.knowledge_base.embedding_provider_id == provider_id
            && self.plugins.knowledge_base.embedding_model == model
        {
            self.plugins.knowledge_base.embedding_provider_id.clear();
            self.plugins.knowledge_base.embedding_model.clear();
        }
        retain_nonempty_pool(&mut self.active_provider_models);
        retain_nonempty_pool(&mut self.active_multimodal_provider_models);
    }

    pub fn toggle_active_multimodal_provider_model(
        &mut self,
        provider_id: &str,
        model: &str,
    ) -> Result<bool> {
        if model.trim().is_empty() {
            bail!("model cannot be empty");
        }
        if let Some(active_models) = &mut self.active_multimodal_provider_models {
            if let Some(index) = active_models
                .iter()
                .position(|active| active.provider_id == provider_id && active.model == model)
            {
                active_models.remove(index);
                return Ok(false);
            }
        }
        let provider = self.provider(Some(provider_id))?;
        if !provider.has_configured_model(model) {
            bail!("model is not configured for provider {provider_id}: {model}");
        }
        if !provider
            .input_modalities(model)
            .is_some_and(|modalities| modalities.iter().any(|item| item == "image"))
        {
            bail!("multimodal model does not declare image input: {provider_id} / {model}");
        }
        let active_models = self
            .active_multimodal_provider_models
            .get_or_insert_with(Vec::new);
        active_models.push(ActiveProviderModelConfig {
            provider_id: provider_id.to_string(),
            model: model.to_string(),
        });
        Ok(true)
    }

    pub fn model_supports_any_input(
        &self,
        provider_id: &str,
        model: &str,
        inputs: &[&str],
    ) -> bool {
        self.provider(Some(provider_id))
            .ok()
            .and_then(|provider| provider.input_modalities(model))
            .map(|modalities| {
                modalities
                    .iter()
                    .any(|m| inputs.iter().any(|input| m == input))
            })
            .unwrap_or(false)
    }

    pub fn vision_provider_choice(&self) -> Result<(String, String)> {
        let vision = &self.plugins.vision;
        if !vision.vision_provider_id.trim().is_empty() {
            let provider_id = vision.vision_provider_id.trim().to_string();
            let provider = self.provider(Some(&provider_id))?;
            let model = if vision.vision_model.trim().is_empty() {
                provider.default_model.clone()
            } else {
                vision.vision_model.trim().to_string()
            };
            if !provider
                .input_modalities(&model)
                .is_some_and(|modalities| modalities.iter().any(|item| item == "image"))
            {
                bail!("vision model does not declare image input: {provider_id} / {model}");
            }
            return Ok((provider_id, model));
        }
        if let Some(active) = self.active_multimodal_provider_models.as_ref() {
            if let Some(choice) = self
                .active_multimodal_provider_model_choices()
                .into_iter()
                .find(|choice| {
                    self.model_supports_any_input(&choice.provider_id, &choice.model, &["image"])
                })
            {
                return Ok((choice.provider_id, choice.model));
            }
            if !active.is_empty() {
                bail!("the configured multimodal model pool has no image-capable model");
            }
        }
        Ok((
            OPENCODE_PROVIDER_ID.to_string(),
            OPENCODE_DEFAULT_VISION_MODEL.to_string(),
        ))
    }

    /// A tier pool's usable model choices: configured entries filtered to
    /// models that still exist under their provider (entries whose model
    /// was removed from the text models are ignored, mirroring
    /// `active_provider_model_choices`).
    pub fn subagent_tier_choices(&self, tier: ModelTier) -> Vec<ProviderModelChoice> {
        self.subagent_tiers
            .pool(tier)
            .iter()
            .filter_map(|entry| {
                let provider = self.provider(Some(entry.provider_id.trim())).ok()?;
                let model = entry.model.trim();
                provider
                    .has_configured_model(model)
                    .then(|| ProviderModelChoice {
                        provider_id: provider.id.clone(),
                        provider_name: provider.display_name.clone(),
                        model: model.to_string(),
                    })
            })
            .collect()
    }

    pub fn is_subagent_tier_model(&self, tier: ModelTier, provider_id: &str, model: &str) -> bool {
        self.subagent_tiers
            .pool(tier)
            .iter()
            .any(|entry| entry.provider_id == provider_id && entry.model == model)
    }

    /// Adds/removes a model in a tier pool. Returns `true` when the model
    /// is in the pool after the call.
    pub fn toggle_subagent_tier_model(
        &mut self,
        tier: ModelTier,
        provider_id: &str,
        model: &str,
    ) -> Result<bool> {
        if model.trim().is_empty() {
            bail!("model cannot be empty");
        }
        self.provider(Some(provider_id))?;
        let pool = self.subagent_tiers.pool_mut(tier);
        if let Some(index) = pool
            .iter()
            .position(|entry| entry.provider_id == provider_id && entry.model == model)
        {
            pool.remove(index);
            Ok(false)
        } else {
            pool.push(ActiveProviderModelConfig {
                provider_id: provider_id.to_string(),
                model: model.to_string(),
            });
            Ok(true)
        }
    }

    /// Drops tier pool entries whose model no longer exists among the
    /// configured text models (a model removed from a provider must also
    /// leave every tier pool).
    pub fn prune_subagent_tiers(&mut self) {
        for tier in ModelTier::ALL {
            let providers = &self.providers;
            self.subagent_tiers
                .pool_mut(tier)
                .retain(|entry| active_model_exists(providers, entry));
        }
    }

    pub fn is_active_provider_model(&self, provider_id: &str, model: &str) -> bool {
        match &self.active_provider_models {
            None => self
                .provider(None)
                .map(|provider| provider.id == provider_id && provider.default_model == model)
                .unwrap_or(false),
            Some(active_models) => active_models
                .iter()
                .any(|active| active.provider_id == provider_id && active.model == model),
        }
    }

    pub fn toggle_active_provider_model(&mut self, provider_id: &str, model: &str) -> Result<bool> {
        if model.trim().is_empty() {
            bail!("model cannot be empty");
        }
        self.provider(Some(provider_id))?;
        if self.active_provider_models.is_none() {
            self.active_provider_models = Some(
                self.active_provider_model_choices()
                    .into_iter()
                    .map(|choice| ActiveProviderModelConfig {
                        provider_id: choice.provider_id,
                        model: choice.model,
                    })
                    .collect(),
            );
        }
        let active_models = self.active_provider_models.get_or_insert_with(Vec::new);
        if let Some(index) = active_models
            .iter()
            .position(|active| active.provider_id == provider_id && active.model == model)
        {
            active_models.remove(index);
            return Ok(false);
        }
        active_models.push(ActiveProviderModelConfig {
            provider_id: provider_id.to_string(),
            model: model.to_string(),
        });
        Ok(true)
    }

    pub fn set_active_provider_models(
        &mut self,
        models: &[ActiveProviderModelConfig],
    ) -> Result<()> {
        if models.is_empty() {
            bail!("at least one model endpoint must remain active");
        }
        let choices = self.provider_model_choices();
        let mut seen = std::collections::HashSet::with_capacity(models.len());
        for model in models {
            if model.provider_id.trim().is_empty() || model.model.trim().is_empty() {
                bail!("provider_id and model cannot be empty");
            }
            if !seen.insert((&model.provider_id, &model.model)) {
                bail!(
                    "duplicate active provider/model: {} / {}",
                    model.provider_id,
                    model.model
                );
            }
            if !choices.iter().any(|choice| {
                choice.provider_id == model.provider_id && choice.model == model.model
            }) {
                bail!(
                    "unknown configured provider/model: {} / {}",
                    model.provider_id,
                    model.model
                );
            }
        }
        self.active_provider_models = Some(models.to_vec());
        Ok(())
    }

    pub fn set_active_provider_model(&mut self, provider_id: &str, model: &str) -> Result<()> {
        let provider = self
            .providers
            .iter_mut()
            .find(|provider| provider.id == provider_id)
            .with_context(|| format!("provider not found: {provider_id}"))?;
        if model.trim().is_empty() {
            bail!("model cannot be empty");
        }
        self.active_provider = provider.id.clone();
        provider.default_model = model.to_string();
        self.active_provider_models = Some(vec![ActiveProviderModelConfig {
            provider_id: provider.id.clone(),
            model: model.to_string(),
        }]);
        if !provider.models.iter().any(|item| item == model) {
            provider.models.push(model.to_string());
        }
        Ok(())
    }

    pub fn remove_active_provider_model(&mut self, provider_id: &str, model: &str) -> Result<()> {
        let provider_index = self
            .providers
            .iter()
            .position(|provider| provider.id == provider_id)
            .with_context(|| format!("provider not found: {provider_id}"))?;
        if model.trim().is_empty() {
            bail!("model cannot be empty");
        }
        {
            let provider = &mut self.providers[provider_index];
            provider.models.retain(|item| item != model);
            provider.model_context_window.remove(model);
            provider.model_modalities.remove(model);
            if provider.default_model == model {
                provider.default_model = provider.models.first().cloned().unwrap_or_default();
            }
        }
        self.remove_active_model_references(provider_id, model);
        Ok(())
    }

    pub fn active_context_window(&self) -> Result<Option<usize>> {
        let choices = self.active_provider_model_choices();
        if choices.is_empty() {
            return Ok(None);
        }
        let mut windows = Vec::new();
        for choice in choices {
            let Some(window) =
                self.context_window_for_provider_model(&choice.provider_id, &choice.model)?
            else {
                return Ok(None);
            };
            windows.push(window);
        }
        Ok(windows.into_iter().min())
    }

    pub fn context_window_for_provider_model(
        &self,
        provider_id: &str,
        model: &str,
    ) -> Result<Option<usize>> {
        let provider = self.provider(Some(provider_id))?;
        if let Some(window) = provider
            .model_context_window
            .get(model)
            .copied()
            .filter(|&w| w > 0)
        {
            return Ok(Some(window));
        }
        Ok(crate::models_cache::context_window(provider_id, model)
            .map(|w| w as usize)
            .or_else(|| {
                (self.context.default_context_window > 0)
                    .then_some(self.context.default_context_window)
            }))
    }

    pub fn system_prompt(&self, paths: &MiyuPaths) -> Result<String> {
        let mut prompt = self.base_system_prompt(paths)?;
        let user_identity = self.user_identity_prompt(paths)?;
        if !user_identity.trim().is_empty() {
            prompt.push_str("\n\n<current-user-profile>\n");
            prompt.push_str("This profile describes the user currently interacting with you.\n\n");
            prompt.push_str(user_identity.trim());
            prompt.push_str("\n</current-user-profile>");
        }
        Ok(prompt)
    }

    pub fn base_system_prompt(&self, paths: &MiyuPaths) -> Result<String> {
        let persona = self.active_persona_prompt(paths)?;
        if persona.trim().is_empty() {
            Ok(default_system_prompt())
        } else {
            Ok(persona)
        }
    }

    pub fn custom_system_prompt(&self, paths: &MiyuPaths) -> Result<String> {
        if let Some(prompt) = self
            .system_prompt
            .as_deref()
            .filter(|prompt| !prompt.trim().is_empty())
        {
            return Ok(prompt.to_string());
        }
        let prompt_file = self.system_prompt_path(paths);
        if prompt_file.exists() {
            return Ok(std::fs::read_to_string(prompt_file)?);
        }
        Ok(String::new())
    }

    pub fn prompts_dir_path(&self, paths: &MiyuPaths) -> PathBuf {
        config_relative_path(paths, &self.prompt.prompts_dir)
    }

    pub fn user_identity_path(&self, paths: &MiyuPaths) -> PathBuf {
        config_relative_path(paths, &self.prompt.user_identity_file)
    }

    pub fn identities_dir_path(&self, paths: &MiyuPaths) -> PathBuf {
        config_relative_path(paths, &self.prompt.identities_dir)
    }

    pub fn persona_path(&self, paths: &MiyuPaths, name: &str) -> PathBuf {
        self.prompts_dir_path(paths).join(name)
    }

    pub fn identity_path(&self, paths: &MiyuPaths, name: &str) -> PathBuf {
        self.identities_dir_path(paths).join(name)
    }

    pub fn persona_memory_data_dir(&self, paths: &MiyuPaths, persona: &str) -> PathBuf {
        paths
            .data_dir
            .join("personas")
            .join(persona_scope_name(persona))
    }

    pub fn persona_memory_state_dir(&self, paths: &MiyuPaths, persona: &str) -> PathBuf {
        paths
            .state_dir
            .join("personas")
            .join(persona_scope_name(persona))
    }

    pub fn persona_skills_dir(&self, paths: &MiyuPaths, persona: &str) -> PathBuf {
        paths
            .skills_dir
            .join("personas")
            .join(persona_scope_name(persona))
    }

    /// Sanitized scope name of the active persona; also the namespace key for
    /// sessions and per-persona state directories.
    pub fn active_persona_scope(&self) -> String {
        persona_scope_name(self.prompt.active_persona.trim())
    }

    pub fn active_persona_memory_data_dir(&self, paths: &MiyuPaths) -> PathBuf {
        self.persona_memory_data_dir(paths, self.prompt.active_persona.trim())
    }

    pub fn active_persona_memory_state_dir(&self, paths: &MiyuPaths) -> PathBuf {
        self.persona_memory_state_dir(paths, self.prompt.active_persona.trim())
    }

    pub fn active_persona_skills_dir(&self, paths: &MiyuPaths) -> PathBuf {
        self.persona_skills_dir(paths, self.prompt.active_persona.trim())
    }

    pub fn active_persona_prompt(&self, paths: &MiyuPaths) -> Result<String> {
        if !self.prompt.active_persona.trim().is_empty() {
            let path = self.persona_path(paths, self.prompt.active_persona.trim());
            if path.exists() {
                return std::fs::read_to_string(&path)
                    .with_context(|| format!("failed to read {}", path.display()));
            }
        }
        if let Some(prompt) = self
            .system_prompt
            .as_deref()
            .filter(|prompt| !prompt.trim().is_empty())
        {
            return Ok(prompt.to_string());
        }
        let legacy = self.custom_system_prompt(paths)?;
        if legacy.trim().is_empty() {
            Ok(String::new())
        } else {
            Ok(legacy)
        }
    }

    pub fn user_identity_prompt(&self, paths: &MiyuPaths) -> Result<String> {
        if !self.prompt.active_identity.trim().is_empty() {
            let path = self.identity_path(paths, self.prompt.active_identity.trim());
            if path.exists() {
                return std::fs::read_to_string(&path)
                    .with_context(|| format!("failed to read {}", path.display()));
            }
        }
        let path = self.user_identity_path(paths);
        if path.exists() {
            return std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()));
        }
        Ok(String::new())
    }

    pub fn system_prompt_path(&self, paths: &MiyuPaths) -> PathBuf {
        let value = self
            .system_prompt_file
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("system-prompt.md");
        let path = PathBuf::from(value);
        if path.is_absolute() {
            path
        } else {
            paths.config_dir.join(path)
        }
    }

    pub fn upsert_provider(&mut self, provider: ProviderConfig) {
        self.active_provider = provider.id.clone();
        self.active_provider_models = if provider.default_model.trim().is_empty() {
            Some(vec![ActiveProviderModelConfig {
                provider_id: OPENCODE_PROVIDER_ID.to_string(),
                model: OPENCODE_DEFAULT_CHAT_MODEL.to_string(),
            }])
        } else {
            Some(vec![ActiveProviderModelConfig {
                provider_id: provider.id.clone(),
                model: provider.default_model.clone(),
            }])
        };
        match self
            .providers
            .iter()
            .position(|item| item.id == provider.id)
        {
            Some(index) => self.providers[index] = provider,
            None => self.providers.push(provider),
        }
    }
}

fn default_timeout() -> u64 {
    60
}

fn default_mcp_timeout() -> u64 {
    30
}

fn default_prompts_dir() -> String {
    "prompts".to_string()
}

fn default_identities_dir() -> String {
    "identities".to_string()
}

fn default_user_identity_file() -> String {
    "user-identity.md".to_string()
}

fn config_relative_path(paths: &MiyuPaths, value: &str) -> PathBuf {
    let path = PathBuf::from(value.trim());
    if path.is_absolute() {
        path
    } else {
        paths.config_dir.join(path)
    }
}

fn persona_scope_name(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        return "default".to_string();
    }
    let normalized = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if normalized.is_empty() {
        format!("persona-{}", &blake3::hash(name.as_bytes()).to_hex()[..12])
    } else {
        normalized
    }
}

fn default_temperature() -> f32 {
    0.7
}

fn is_default_timeout(value: &u64) -> bool {
    *value == default_timeout()
}

fn is_default_temperature(value: &f32) -> bool {
    (*value - default_temperature()).abs() < f32::EPSILON
}

fn default_anthropic_max_tokens() -> u32 {
    4096
}

fn default_context_window() -> usize {
    168_000
}

fn is_default_anthropic_max_tokens(value: &u32) -> bool {
    *value == default_anthropic_max_tokens()
}

fn default_provider_protocol() -> String {
    "auto".to_string()
}

fn is_auto_protocol(value: &str) -> bool {
    value.trim().is_empty() || value == "auto"
}

fn default_true() -> bool {
    true
}

fn default_tools_loading_mode() -> String {
    "hybrid".to_string()
}

fn default_subagent_concurrency() -> usize {
    4
}

fn default_display_language() -> String {
    "auto".to_string()
}

fn default_reasoning_display() -> String {
    "summary".to_string()
}

fn default_tool_call_display() -> String {
    "summary".to_string()
}

fn default_command_output_lines() -> usize {
    10
}

fn default_mixed_model_endpoint_display() -> String {
    "interactive".to_string()
}

fn default_memory_association_facts() -> usize {
    5
}

fn default_memory_association_episodes() -> usize {
    3
}

fn default_memory_association_max_chars() -> usize {
    1800
}

fn default_memory_snippet_chars() -> usize {
    500
}

fn default_memory_forget_after_days() -> u64 {
    90
}

fn default_memory_half_life_days() -> f64 {
    7.0
}

fn default_memory_min_strength() -> f64 {
    0.15
}

fn default_memory_review_boost() -> f64 {
    0.35
}

fn default_memory_min_task_chars() -> usize {
    16
}

fn default_memory_min_method_chars() -> usize {
    120
}

fn default_print_image_width_percent() -> u8 {
    45
}

fn default_print_image_height_percent() -> u8 {
    35
}

fn default_memes_width_percent() -> u8 {
    35
}

fn default_memes_height_percent() -> u8 {
    25
}

fn default_memes_max_image_mb() -> u64 {
    10
}

fn default_memes_search_max_results() -> usize {
    1
}

fn default_memes_auto_send_probability() -> f32 {
    0.2
}

fn default_web_search_max_results() -> usize {
    2
}

fn default_web_images_max_results() -> usize {
    5
}

fn default_web_images_source_mode() -> String {
    "auto".to_string()
}

fn default_web_images_max_download_mb() -> f64 {
    4.0
}

fn default_web_images_preview_count() -> usize {
    1
}

fn default_web_images_timeout() -> u64 {
    20
}

fn default_deep_research_dir() -> String {
    if let Some(dirs) = directories::UserDirs::new() {
        if let Some(documents) = dirs.document_dir() {
            return documents.join("Miyu/deep-thinking").display().to_string();
        }
    }
    "~/Documents/Miyu/deep-thinking".to_string()
}

fn default_deep_research_depth() -> String {
    "high".to_string()
}

fn default_deep_research_max_review_revisions() -> usize {
    0
}

fn default_deep_research_max_tool_steps() -> usize {
    0
}

fn default_deep_research_tool_timeout() -> u64 {
    90
}

fn default_subagent_max_tool_steps() -> usize {
    100
}

fn default_image_generation_provider_type() -> String {
    "openai".to_string()
}

fn default_openai_images_base_url() -> String {
    "https://api.openai.com".to_string()
}

fn default_image_generation_model() -> String {
    "gpt-image-1".to_string()
}

fn default_image_generation_aspect_ratio() -> String {
    "自动".to_string()
}

fn default_image_generation_resolution() -> String {
    "1K".to_string()
}

fn default_image_generation_output_dir() -> String {
    if let Some(dirs) = directories::UserDirs::new() {
        if let Some(pictures) = dirs.picture_dir() {
            return pictures.join("miyu/generated-images").display().to_string();
        }
    }
    "~/Pictures/miyu/generated-images".to_string()
}

fn default_image_generation_timeout() -> u64 {
    180
}

fn default_kb_max_search_results() -> usize {
    5
}

fn default_kb_snippet_context_chars() -> usize {
    240
}

fn default_kb_proximity_window_chars() -> usize {
    512
}

fn default_kb_max_read_lines() -> usize {
    200
}

fn default_kb_max_file_size_kb() -> usize {
    1024
}

fn default_kb_allowed_extensions() -> String {
    ".txt,.md,.json,.jsonc,.json5,.yaml,.yml,.csv,.log,.py,.js,.ts,.jsx,.tsx,.mjs,.cjs,.html,.css,.scss,.sass,.less,.cfg,.ini,.conf,.toml,.kdl,.desktop,.service,.timer,.socket,.target,.mount,.rules,.network,.netdev,.properties,.hjson,.ron,.rst,.xml,.sh,.bash,.zsh,.fish,.nu,.ps1,.lua,.nix,.rasi,.yuck,.sql,.rs,.go,.c,.h,.cpp,.hpp,.java,.kt,.php,.rb,.pl,.org,.adoc,.tex".to_string()
}

fn default_kb_allowed_filenames() -> String {
    ".env,.env.local,.env.example,.env.sample,.envrc,.editorconfig,.gitignore,.gitattributes,.npmrc,.vimrc,.bashrc,.zshrc,.profile,.xinitrc,.xresources,config,dockerfile,containerfile,makefile,justfile,procfile,pkgbuild".to_string()
}

fn default_kb_semantic_chunk_chars() -> usize {
    512
}

fn default_kb_semantic_chunk_overlap() -> usize {
    80
}

fn default_kb_semantic_top_k() -> usize {
    5
}

fn default_kb_semantic_min_score() -> f32 {
    0.25
}

fn default_kb_keyword_strong_score_threshold() -> f32 {
    180.0
}

fn default_kb_embedding_timeout_seconds() -> u64 {
    60
}

fn default_diagnostics_timeout() -> u64 {
    5
}

fn default_diagnostics_max_stdout_chars() -> usize {
    8_000
}

fn default_diagnostics_max_stderr_chars() -> usize {
    4_000
}

fn default_calculator_backend() -> String {
    "rust-simple".to_string()
}

fn default_trim_at_ratio() -> f32 {
    0.9
}

fn default_trim_batch_ratio() -> f32 {
    0.15
}

fn default_on_overflow() -> String {
    "compact".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_overflow_defaults_to_compact() {
        assert_eq!(ContextConfig::default().on_overflow, "compact");

        let deserialized: ContextConfig = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(deserialized.on_overflow, "compact");
    }

    #[test]
    fn provider_config_can_be_saved_without_active_model() {
        let mut config = AppConfig::default();
        config.providers[0].models.clear();
        config.providers[0].default_model.clear();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn provider_model_choices_ignore_unconfigured_models() {
        let mut config = AppConfig::default();
        let provider_id = config.providers[0].id.clone();
        config.providers[0].models.clear();
        config.providers[0].default_model.clear();

        assert!(!config
            .provider_model_choices()
            .iter()
            .any(|choice| choice.provider_id == provider_id));
    }

    #[test]
    fn active_provider_models_are_replaced_as_one_validated_pool() {
        let mut config = AppConfig::default();
        let provider_id = config.providers[0].id.clone();
        config.providers[0].models = vec!["model-a".to_string(), "model-b".to_string()];
        config.providers[0].default_model = "model-a".to_string();
        let before = config.active_provider_models.clone();

        let invalid = vec![
            ActiveProviderModelConfig {
                provider_id: provider_id.clone(),
                model: "model-a".to_string(),
            },
            ActiveProviderModelConfig {
                provider_id: provider_id.clone(),
                model: "missing".to_string(),
            },
        ];
        assert!(config.set_active_provider_models(&invalid).is_err());
        assert_eq!(config.active_provider_models, before);

        let selected = vec![
            ActiveProviderModelConfig {
                provider_id: provider_id.clone(),
                model: "model-b".to_string(),
            },
            ActiveProviderModelConfig {
                provider_id,
                model: "model-a".to_string(),
            },
        ];
        config.set_active_provider_models(&selected).unwrap();
        assert_eq!(
            config.active_provider_models.as_deref(),
            Some(selected.as_slice())
        );
    }

    #[test]
    fn empty_active_provider_models_normalizes_to_default_chat_model() {
        let mut config = AppConfig::default();
        config.active_provider_models = Some(Vec::new());

        config.normalize_builtin_providers();

        let choices = config.active_provider_model_choices();
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].provider_id, OPENCODE_PROVIDER_ID);
        assert_eq!(choices[0].model, OPENCODE_DEFAULT_CHAT_MODEL);
    }

    #[test]
    fn active_provider_model_choices_ignore_stale_models() {
        let mut config = AppConfig::default();
        let provider_id = config.providers[0].id.clone();
        config.providers[0].models = vec!["deepseek-v4-flash-free".to_string()];
        config.providers[0].default_model = "deepseek-v4-flash-free".to_string();
        config.active_provider_models = Some(vec![
            ActiveProviderModelConfig {
                provider_id: provider_id.clone(),
                model: "mimo-v2.5-free".to_string(),
            },
            ActiveProviderModelConfig {
                provider_id: provider_id.clone(),
                model: "deepseek-v4-flash-free".to_string(),
            },
        ]);

        let choices = config.active_provider_model_choices();

        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].provider_id, provider_id);
        assert_eq!(choices[0].model, "deepseek-v4-flash-free");
    }

    #[test]
    fn normalize_prunes_stale_active_provider_models() {
        let mut config = AppConfig::default();
        let provider_id = config.providers[0].id.clone();
        config.providers[0].models = vec!["deepseek-v4-flash-free".to_string()];
        config.providers[0].default_model = "deepseek-v4-flash-free".to_string();
        config.active_provider_models = Some(vec![
            ActiveProviderModelConfig {
                provider_id: provider_id.clone(),
                model: "mimo-v2.5-free".to_string(),
            },
            ActiveProviderModelConfig {
                provider_id: provider_id.clone(),
                model: "deepseek-v4-flash-free".to_string(),
            },
        ]);

        config.normalize_builtin_providers();

        assert_eq!(
            config.active_provider_models,
            Some(vec![ActiveProviderModelConfig {
                provider_id,
                model: "deepseek-v4-flash-free".to_string(),
            }])
        );
    }

    #[test]
    fn remove_active_model_references_clears_text_and_multimodal() {
        let mut config = AppConfig::default();
        let provider_id = config.providers[0].id.clone();
        config.active_provider_models = Some(vec![ActiveProviderModelConfig {
            provider_id: provider_id.clone(),
            model: "old-model".to_string(),
        }]);
        config.active_multimodal_provider_models = Some(vec![ActiveProviderModelConfig {
            provider_id: provider_id.clone(),
            model: "old-model".to_string(),
        }]);

        config.remove_active_model_references(&provider_id, "old-model");

        assert_eq!(config.active_provider_models, None);
        assert_eq!(config.active_multimodal_provider_models, None);
    }

    #[test]
    fn multimodal_provider_model_choices_use_input_modalities() {
        let mut config = AppConfig::default();
        let provider = &mut config.providers[0];
        provider.models = vec![
            "text-only".to_string(),
            "audio-only".to_string(),
            "vision-model".to_string(),
        ];
        provider
            .model_modalities
            .insert("text-only".to_string(), vec!["text".to_string()]);
        provider.model_modalities.insert(
            "audio-only".to_string(),
            vec!["text".to_string(), "audio".to_string()],
        );
        provider.model_modalities.insert(
            "vision-model".to_string(),
            vec!["text".to_string(), "image".to_string()],
        );

        let choices = config.multimodal_provider_model_choices();

        assert!(choices.iter().any(|choice| choice.model == "vision-model"));
        assert!(!choices.iter().any(|choice| choice.model == "text-only"));
        assert!(!choices.iter().any(|choice| choice.model == "audio-only"));
    }

    #[test]
    fn active_multimodal_pool_rejects_and_prunes_non_image_models() {
        let mut config = AppConfig::default();
        let provider_id = config.providers[0].id.clone();
        config.providers[0]
            .models
            .extend(["audio-only".to_string(), "vision-model".to_string()]);
        config.providers[0].model_modalities.insert(
            "audio-only".to_string(),
            vec!["text".to_string(), "audio".to_string()],
        );
        config.providers[0].model_modalities.insert(
            "vision-model".to_string(),
            vec!["text".to_string(), "image".to_string()],
        );

        assert!(config
            .toggle_active_multimodal_provider_model(&provider_id, "audio-only")
            .is_err());
        assert!(config
            .toggle_active_multimodal_provider_model(&provider_id, "vision-model")
            .unwrap());
        config
            .active_multimodal_provider_models
            .as_mut()
            .unwrap()
            .push(ActiveProviderModelConfig {
                provider_id,
                model: "audio-only".to_string(),
            });
        assert!(config.validate_global_multimodal_config().is_err());

        config.normalize_builtin_providers();

        let active = config.active_multimodal_provider_models.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].model, "vision-model");
    }

    #[test]
    fn vision_provider_choice_prefers_multimodal_pool_then_default_mimo() {
        let mut config = AppConfig::default();
        config.providers[0].models.push("vision-model".to_string());
        config.providers[0].model_modalities.insert(
            "vision-model".to_string(),
            vec!["text".to_string(), "image".to_string()],
        );
        config.active_multimodal_provider_models = Some(vec![ActiveProviderModelConfig {
            provider_id: OPENCODE_PROVIDER_ID.to_string(),
            model: "vision-model".to_string(),
        }]);

        assert_eq!(
            config.vision_provider_choice().unwrap(),
            (OPENCODE_PROVIDER_ID.to_string(), "vision-model".to_string())
        );

        config.active_multimodal_provider_models = Some(Vec::new());
        assert_eq!(
            config.vision_provider_choice().unwrap(),
            (
                OPENCODE_PROVIDER_ID.to_string(),
                OPENCODE_DEFAULT_VISION_MODEL.to_string()
            )
        );
    }

    #[test]
    fn vision_provider_choice_rejects_an_audio_only_active_pool() {
        let mut config = AppConfig::default();
        let provider_id = config.providers[0].id.clone();
        config.providers[0].models.push("audio-only".to_string());
        config.providers[0].model_modalities.insert(
            "audio-only".to_string(),
            vec!["text".to_string(), "audio".to_string()],
        );
        config.active_multimodal_provider_models = Some(vec![ActiveProviderModelConfig {
            provider_id,
            model: "audio-only".to_string(),
        }]);

        assert!(config.vision_provider_choice().is_err());
        assert!(config.validate().is_err());
    }

    #[test]
    fn vision_provider_choice_rejects_an_explicit_non_image_model() {
        let mut config = AppConfig::default();
        let provider_id = config.providers[0].id.clone();
        config.providers[0].models.push("audio-only".to_string());
        config.providers[0].model_modalities.insert(
            "audio-only".to_string(),
            vec!["text".to_string(), "audio".to_string()],
        );
        config.plugins.vision.vision_provider_id = provider_id;
        config.plugins.vision.vision_model = "audio-only".to_string();

        assert!(config.vision_provider_choice().is_err());
        assert!(config.validate().is_err());
    }

    #[test]
    fn subagent_tier_pools_toggle_filter_and_prune() {
        let mut config = AppConfig::default();
        let provider_id = config.providers[0].id.clone();
        config.providers[0].models.push("mini-a".to_string());
        config.providers[0].models.push("mini-b".to_string());

        // Unconfigured pool resolves empty.
        assert!(config.subagent_tier_choices(ModelTier::Cheap).is_empty());

        // Toggle in/out mirrors the text-model picker semantics.
        assert!(config
            .toggle_subagent_tier_model(ModelTier::Cheap, &provider_id, "mini-a")
            .unwrap());
        assert!(config
            .toggle_subagent_tier_model(ModelTier::Cheap, &provider_id, "mini-b")
            .unwrap());
        assert!(config.is_subagent_tier_model(ModelTier::Cheap, &provider_id, "mini-a"));
        let choices = config.subagent_tier_choices(ModelTier::Cheap);
        assert_eq!(
            choices.iter().map(|c| c.model.as_str()).collect::<Vec<_>>(),
            vec!["mini-a", "mini-b"]
        );
        assert!(!config
            .toggle_subagent_tier_model(ModelTier::Cheap, &provider_id, "mini-b")
            .unwrap());
        assert_eq!(config.subagent_tier_choices(ModelTier::Cheap).len(), 1);

        // Unknown provider is rejected.
        assert!(config
            .toggle_subagent_tier_model(ModelTier::Strong, "no-such", "x")
            .is_err());

        // A model removed from the text models leaves the pool too.
        config
            .toggle_subagent_tier_model(ModelTier::Balanced, &provider_id, "mini-a")
            .unwrap();
        config
            .remove_active_provider_model(&provider_id, "mini-a")
            .unwrap();
        assert!(config.subagent_tier_choices(ModelTier::Cheap).is_empty());
        assert!(config.subagent_tiers.pool(ModelTier::Cheap).is_empty());
        assert!(config.subagent_tiers.pool(ModelTier::Balanced).is_empty());

        // prune_subagent_tiers drops entries that no longer resolve.
        config
            .toggle_subagent_tier_model(ModelTier::Cheap, &provider_id, "mini-b")
            .unwrap();
        config.providers[0].models.retain(|m| m != "mini-b");
        assert!(config.subagent_tier_choices(ModelTier::Cheap).is_empty());
        config.prune_subagent_tiers();
        assert!(config.subagent_tiers.pool(ModelTier::Cheap).is_empty());
    }

    #[test]
    fn subagent_tiers_roundtrip_and_default_omission() {
        let config = AppConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        // Empty pools stay out of the serialized config.
        assert!(!json.contains("subagent_tiers"));

        let parsed: AppConfig = serde_json::from_str(
            r#"{
                "active_provider": "opencode",
                "providers": [],
                "subagent_tiers": {
                    "cheap": [ { "provider_id": "p", "model": "m" } ]
                }
            }"#,
        )
        .unwrap();
        assert_eq!(parsed.subagent_tiers.cheap.len(), 1);
        assert_eq!(parsed.subagent_tiers.cheap[0].model, "m");
        assert!(parsed.subagent_tiers.balanced.is_empty());
        // Choices filter out entries with unknown providers.
        assert!(parsed.subagent_tier_choices(ModelTier::Cheap).is_empty());
    }

    #[test]
    fn platforms_config_roundtrip_and_default_omission() {
        let config = AppConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        // An untouched platforms config stays out of the serialized file.
        assert!(!json.contains("platforms"));

        let parsed: AppConfig = serde_json::from_str(
            r#"{
                "active_provider": "opencode",
                "providers": [],
                "platforms": {
                    "command_prefix": "!",
                    "commands": {
                        "reset": { "permission": "everyone" }
                    },
                    "qq": {
                        "enabled": true,
                        "reverse_ws_port": 8400,
                        "access_token": "secret",
                        "admin_users": [9988],
                        "asset_base_url": "https://assets.example.test",
                        "private_chats": {
                            "whitelist": [12345],
                            "allow_non_whitelist": false,
                            "non_whitelist_rate_per_minute": 4
                        },
                        "group_chats": {
                            "whitelist": [54321],
                            "trigger_keywords": ["Miyu"],
                            "whitelist_rate_per_minute": 30,
                            "allow_non_whitelist": true,
                            "non_whitelist_rate_per_minute": 10
                        }
                    }
                }
            }"#,
        )
        .unwrap();
        let qq = &parsed.platforms.qq;
        assert_eq!(parsed.platforms.command_prefix, "!");
        assert_eq!(
            parsed
                .platforms
                .command_permission("reset", PlatformCommandPermission::AdminOnly),
            PlatformCommandPermission::Everyone
        );
        assert!(qq.enabled);
        assert_eq!(qq.reverse_ws_port, 8400);
        assert_eq!(qq.access_token, "secret");
        assert_eq!(qq.admin_users, vec![9988]);
        assert_eq!(qq.asset_base_url, "https://assets.example.test");
        assert_eq!(qq.private_chats.whitelist, vec![12345]);
        assert!(!qq.private_chats.allow_non_whitelist);
        assert_eq!(qq.group_chats.whitelist, vec![54321]);
        assert_eq!(qq.group_chats.trigger_keywords, vec!["Miyu"]);
        assert_eq!(qq.max_reply_chars, 3000);

        // Round-trip preserves the non-default config.
        let json = serde_json::to_string(&parsed).unwrap();
        let reparsed: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(reparsed.platforms, parsed.platforms);

        // The retired protocol-shaped key is a clean break and does not
        // silently enable Tencent QQ under the new defaults.
        let legacy: AppConfig = serde_json::from_str(
            r#"{"active_provider":"opencode","providers":[],"platforms":{"onebot":{"enabled":true}}}"#,
        )
        .unwrap();
        assert!(!legacy.platforms.qq.enabled);
        assert_eq!(legacy.platforms.command_prefix, "/");
        assert!(legacy.platforms.commands.is_empty());
    }

    #[test]
    fn platform_command_defaults_overrides_and_validation() {
        let mut config = AppConfig::default();
        assert_eq!(config.platforms.command_prefix, "/");
        assert_eq!(
            config
                .platforms
                .command_permission("reset", PlatformCommandPermission::AdminOnly),
            PlatformCommandPermission::AdminOnly
        );
        config.platforms.set_command_permission(
            "reset",
            PlatformCommandPermission::Everyone,
            PlatformCommandPermission::AdminOnly,
        );
        assert_eq!(
            config.platforms.commands["reset"].permission,
            PlatformCommandPermission::Everyone
        );
        config.platforms.set_command_permission(
            "reset",
            PlatformCommandPermission::AdminOnly,
            PlatformCommandPermission::AdminOnly,
        );
        assert!(config.platforms.commands.is_empty());

        for invalid in [
            "",
            " ",
            "/ reset",
            "\n",
            "/////////////////////////////////",
        ] {
            config.platforms.command_prefix = invalid.to_string();
            assert!(
                config.validate().is_err(),
                "prefix should be invalid: {invalid:?}"
            );
        }
        config.platforms.command_prefix = "/".to_string();
        config
            .platforms
            .commands
            .insert("Reset".to_string(), PlatformCommandConfig::default());
        assert!(config.validate().is_err());
    }

    fn route_test_config() -> AppConfig {
        let mut config = AppConfig::default();
        let provider = &mut config.providers[0];
        provider.models = vec!["text-only".to_string(), "vision".to_string()];
        provider.default_model = "text-only".to_string();
        provider
            .model_modalities
            .insert("text-only".to_string(), vec!["text".to_string()]);
        provider.model_modalities.insert(
            "vision".to_string(),
            vec!["text".to_string(), "image".to_string()],
        );
        config
    }

    fn test_route(config: &AppConfig) -> PlatformModelRoute {
        PlatformModelRoute {
            conversation: PlatformConversationConfig {
                kind: PlatformConversationKind::Group,
                id: "20002".to_string(),
            },
            text_models: Some(vec![ActiveProviderModelConfig {
                provider_id: config.providers[0].id.clone(),
                model: "text-only".to_string(),
            }]),
            multimodal_models: Some(vec![ActiveProviderModelConfig {
                provider_id: config.providers[0].id.clone(),
                model: "vision".to_string(),
            }]),
            extra_prompt: "Reply naturally in this group.".to_string(),
        }
    }

    #[test]
    fn platform_model_routes_roundtrip_lookup_and_plugin_shape() {
        let mut config = route_test_config();
        let route = test_route(&config);
        config.platforms.upsert_model_route(route.clone());
        config.platforms.qq.plugins.insert(
            "reply_processor".to_string(),
            PlatformPluginInstanceConfig {
                enabled: Some(false),
                settings: serde_json::json!({"threshold": 150})
                    .as_object()
                    .unwrap()
                    .clone(),
            },
        );

        let found = config
            .platform_model_route(PlatformConversationKind::Group, "20002")
            .unwrap();
        assert_eq!(found, &route);
        assert!(config.validate().is_ok());

        let json = serde_json::to_string(&config).unwrap();
        let reparsed: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(reparsed.platforms, config.platforms);
        assert_eq!(
            reparsed.platforms.qq.plugins["reply_processor"].enabled,
            Some(false)
        );
        assert_eq!(
            reparsed.platforms.qq.plugins["reply_processor"].settings["threshold"],
            150
        );
    }

    #[test]
    fn built_in_platform_plugin_settings_are_validated() {
        let mut config = AppConfig::default();
        config.platforms.qq.plugins.insert(
            "reply_processor".to_string(),
            PlatformPluginInstanceConfig {
                enabled: None,
                settings: serde_json::json!({"threshold": 0, "mode": "invalid"})
                    .as_object()
                    .unwrap()
                    .clone(),
            },
        );
        assert!(config.validate().is_err());

        config
            .platforms
            .qq
            .plugins
            .get_mut("reply_processor")
            .unwrap()
            .settings = serde_json::json!({
            "threshold": 150,
            "mode": "image",
            "future_option": 1
        })
        .as_object()
        .unwrap()
        .clone();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn platform_model_route_normalization_uses_none_for_inheritance() {
        let mut config = route_test_config();
        let provider_id = config.providers[0].id.clone();
        let mut route = test_route(&config);
        route.conversation.id = " 20002 ".to_string();
        route.extra_prompt = "  group prompt  ".to_string();
        route.text_models = Some(vec![
            ActiveProviderModelConfig {
                provider_id: format!(" {provider_id} "),
                model: " text-only ".to_string(),
            },
            ActiveProviderModelConfig {
                provider_id: provider_id.clone(),
                model: "text-only".to_string(),
            },
        ]);
        route.multimodal_models = Some(Vec::new());
        config.platforms.qq.conversations.push(route);
        config.normalize_platform_model_routes();

        let normalized = &config.platforms.qq.conversations[0];
        assert_eq!(normalized.conversation.id, "20002");
        assert_eq!(normalized.extra_prompt, "group prompt");
        assert_eq!(normalized.text_models.as_ref().unwrap().len(), 1);
        assert!(normalized.multimodal_models.is_none());

        config.platforms.qq.conversations[0].text_models = Some(Vec::new());
        config.normalize_platform_model_routes();
        assert_eq!(config.platforms.qq.conversations.len(), 1);
        assert!(config.platforms.qq.conversations[0].text_models.is_none());
    }

    #[test]
    fn platform_model_route_validation_rejects_bad_identity_models_and_duplicates() {
        let mut config = route_test_config();
        let mut route = test_route(&config);
        route.conversation.id = "0".to_string();
        assert!(config.validate_platform_model_route(&route).is_err());
        route.conversation.id = "not-a-qq".to_string();
        assert!(config.validate_platform_model_route(&route).is_err());

        route.conversation.id = "20002".to_string();
        route.multimodal_models.as_mut().unwrap()[0].model = "text-only".to_string();
        assert!(config.validate_platform_model_route(&route).is_err());

        route.multimodal_models = None;
        route.text_models.as_mut().unwrap()[0].model = "missing".to_string();
        assert!(config.validate_platform_model_route(&route).is_err());

        let route = test_route(&config);
        config.platforms.qq.conversations = vec![route.clone(), route];
        assert!(config.validate().is_err());
    }

    #[test]
    fn platform_model_references_are_renamed_and_pruned() {
        let mut config = route_test_config();
        let old_provider = config.providers[0].id.clone();
        config.platforms.qq.conversations.push(test_route(&config));

        config.rename_platform_provider_references(&old_provider, "renamed");
        let route = &config.platforms.qq.conversations[0];
        assert_eq!(
            route.text_models.as_ref().unwrap()[0].provider_id,
            "renamed"
        );
        assert_eq!(
            route.multimodal_models.as_ref().unwrap()[0].provider_id,
            "renamed"
        );

        config.rename_platform_provider_references("renamed", &old_provider);
        config.remove_active_model_references(&old_provider, "vision");
        assert!(config.platforms.qq.conversations[0]
            .multimodal_models
            .is_none());
        config.remove_active_model_references(&old_provider, "text-only");
        assert_eq!(config.platforms.qq.conversations.len(), 1);
        assert!(config.platforms.qq.conversations[0].text_models.is_none());
    }

    #[test]
    fn provider_reference_updates_cover_every_model_pool_and_plugin() {
        let mut config = route_test_config();
        let old_id = config.providers[0].id.clone();
        config.active_provider = old_id.clone();
        config.active_provider_models = Some(vec![ActiveProviderModelConfig {
            provider_id: old_id.clone(),
            model: "text-only".to_string(),
        }]);
        config.active_multimodal_provider_models = Some(vec![ActiveProviderModelConfig {
            provider_id: old_id.clone(),
            model: "vision".to_string(),
        }]);
        config.subagent_tiers.cheap.push(ActiveProviderModelConfig {
            provider_id: old_id.clone(),
            model: "text-only".to_string(),
        });
        config.platforms.qq.conversations.push(test_route(&config));
        config.plugins.vision.vision_provider_id = old_id.clone();
        config.plugins.vision.vision_model = "vision".to_string();
        config.plugins.knowledge_base.embedding_provider_id = old_id.clone();
        config.plugins.knowledge_base.embedding_model = "text-only".to_string();

        config.providers[0].id = "renamed".to_string();
        config.rename_provider_references(&old_id, "renamed");

        assert_eq!(config.active_provider, "renamed");
        assert_eq!(
            config.active_provider_models.as_ref().unwrap()[0].provider_id,
            "renamed"
        );
        assert_eq!(
            config.active_multimodal_provider_models.as_ref().unwrap()[0].provider_id,
            "renamed"
        );
        assert_eq!(config.subagent_tiers.cheap[0].provider_id, "renamed");
        assert_eq!(
            config.platforms.qq.conversations[0]
                .text_models
                .as_ref()
                .unwrap()[0]
                .provider_id,
            "renamed"
        );
        assert_eq!(config.plugins.vision.vision_provider_id, "renamed");
        assert_eq!(
            config.plugins.knowledge_base.embedding_provider_id,
            "renamed"
        );
        assert!(config.validate().is_ok());

        config.providers.remove(0);
        config.remove_provider_references("renamed");
        assert!(config.active_provider_models.is_none());
        assert!(config.active_multimodal_provider_models.is_none());
        assert!(config.subagent_tiers.cheap.is_empty());
        assert_eq!(config.platforms.qq.conversations.len(), 1);
        assert!(config.platforms.qq.conversations[0].text_models.is_none());
        assert!(config.plugins.vision.vision_provider_id.is_empty());
        assert!(config
            .plugins
            .knowledge_base
            .embedding_provider_id
            .is_empty());
        assert_ne!(config.active_provider, "renamed");
    }

    #[test]
    fn model_capability_pruning_clears_all_invalid_image_references() {
        let mut config = route_test_config();
        let provider_id = config.providers[0].id.clone();
        config.active_multimodal_provider_models = Some(vec![ActiveProviderModelConfig {
            provider_id: provider_id.clone(),
            model: "vision".to_string(),
        }]);
        config.platforms.qq.conversations.push(test_route(&config));
        config.plugins.vision.vision_provider_id = provider_id;
        config.plugins.vision.vision_model = "vision".to_string();
        config.providers[0]
            .model_modalities
            .insert("vision".to_string(), vec!["text".to_string()]);

        config.prune_model_references();

        assert!(config.active_multimodal_provider_models.is_none());
        assert!(config.platforms.qq.conversations[0]
            .multimodal_models
            .is_none());
        assert!(config.plugins.vision.vision_provider_id.is_empty());
        assert!(config.plugins.vision.vision_model.is_empty());
    }

    #[test]
    fn duplicate_provider_ids_are_rejected() {
        let mut config = AppConfig::default();
        config.providers.push(config.providers[0].clone());
        assert!(config.validate().is_err());
    }

    #[test]
    fn platform_multimodal_pruning_tracks_provider_capabilities() {
        let mut config = route_test_config();
        config.platforms.qq.conversations.push(test_route(&config));
        config.providers[0]
            .model_modalities
            .insert("vision".to_string(), vec!["text".to_string()]);

        config.prune_platform_model_routes();

        let route = &config.platforms.qq.conversations[0];
        assert!(route.multimodal_models.is_none());
        assert_eq!(route.text_models.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn new_custom_provider_has_no_openai_defaults() {
        let provider = ProviderConfig::new_custom();

        assert!(provider.id.is_empty());
        assert!(provider.display_name.is_empty());
        assert!(provider.base_url.is_empty());
        assert_eq!(provider.protocol, "auto");
        assert!(provider.api_key.is_none());
        assert!(provider.models.is_empty());
        assert!(provider.default_model.is_empty());
    }

    #[test]
    fn default_anthropic_provider_uses_the_global_context_window_default() {
        let mut config = AppConfig::default();
        config.active_provider = "anthropic".to_string();
        let provider = config
            .providers
            .iter_mut()
            .find(|provider| provider.id == "anthropic")
            .unwrap();
        provider.models = vec!["claude-sonnet-4-5".to_string()];
        provider.default_model = "claude-sonnet-4-5".to_string();

        assert_eq!(config.active_context_window().unwrap(), Some(168_000));
    }

    #[test]
    fn mixed_context_window_uses_the_global_default_when_model_metadata_is_missing() {
        let mut config = AppConfig::default();
        let provider = &mut config.providers[0];
        let provider_id = provider.id.clone();
        provider.models = vec![
            "miyu-known-window-model".to_string(),
            "miyu-unknown-window-model".to_string(),
        ];
        provider.default_model = provider.models[0].clone();
        provider
            .model_context_window
            .insert(provider.models[0].clone(), 200_000);
        config.active_provider_models = Some(vec![
            ActiveProviderModelConfig {
                provider_id: provider_id.clone(),
                model: provider.models[0].clone(),
            },
            ActiveProviderModelConfig {
                provider_id,
                model: provider.models[1].clone(),
            },
        ]);

        assert_eq!(config.active_context_window().unwrap(), Some(168_000));
        config.providers[0]
            .model_context_window
            .insert("miyu-unknown-window-model".to_string(), 128_000);
        assert_eq!(config.active_context_window().unwrap(), Some(128_000));
    }

    #[test]
    fn default_anthropic_provider_has_no_implicit_active_model() {
        let provider = ProviderConfig::default_anthropic();

        assert!(provider.models.is_empty());
        assert!(provider.default_model.is_empty());
    }

    #[test]
    fn normalizes_legacy_anthropic_template_model() {
        let mut config = AppConfig::default();
        let provider = config
            .providers
            .iter_mut()
            .find(|provider| provider.id == "anthropic")
            .unwrap();
        provider.models = vec!["claude-sonnet-4-5".to_string()];
        provider.default_model = "claude-sonnet-4-5".to_string();

        config.normalize_builtin_providers();
        let provider = config
            .providers
            .iter()
            .find(|provider| provider.id == "anthropic")
            .unwrap();

        assert!(provider.models.is_empty());
        assert!(provider.default_model.is_empty());
    }

    #[test]
    fn anthropic_template_does_not_hardcode_model_context_window() {
        let provider = ProviderConfig::default_anthropic();

        assert!(provider.model_context_window.is_empty());
    }

    #[test]
    fn remove_active_provider_model_clears_removed_current_model() {
        let mut config = AppConfig::default();
        let provider_id = config.providers[0].id.clone();
        config.providers[0].models = vec!["old-model".to_string(), "next-model".to_string()];
        config.providers[0].default_model = "old-model".to_string();
        config.providers[0]
            .model_context_window
            .insert("old-model".to_string(), 8192);
        config.providers[0]
            .model_modalities
            .insert("old-model".to_string(), vec!["text".to_string()]);

        config
            .remove_active_provider_model(&provider_id, "old-model")
            .unwrap();

        assert_eq!(config.providers[0].models, vec!["next-model"]);
        assert_eq!(config.providers[0].default_model, "next-model");
        assert!(!config.providers[0]
            .model_context_window
            .contains_key("old-model"));
        assert!(!config.providers[0]
            .model_modalities
            .contains_key("old-model"));
    }

    #[test]
    fn remove_active_provider_model_clears_last_current_model() {
        let mut config = AppConfig::default();
        let provider_id = config.providers[0].id.clone();
        config.providers[0].models = vec!["old-model".to_string()];
        config.providers[0].default_model = "old-model".to_string();

        config
            .remove_active_provider_model(&provider_id, "old-model")
            .unwrap();

        assert!(config.providers[0].models.is_empty());
        assert!(config.providers[0].default_model.is_empty());
        assert!(!config
            .provider_model_choices()
            .iter()
            .any(|choice| choice.provider_id == provider_id));
    }

    #[test]
    fn display_readable_tool_names_defaults_enabled() {
        let display: DisplayConfig = serde_json::from_str(r#"{"tool_calls":"summary"}"#).unwrap();
        assert_eq!(display.language, "auto");
        assert!(display.readable_tool_names);
        assert!(!display.show_token_usage);
        assert_eq!(display.mixed_model_endpoint_display, "interactive");
        assert_eq!(display.command_output_lines, 10);

        let display: DisplayConfig = serde_json::from_str(r#"{"command_output_lines":3}"#).unwrap();
        assert_eq!(display.command_output_lines, 3);
        assert!(serde_json::to_string(&display)
            .unwrap()
            .contains(r#""command_output_lines":3"#));

        let mut config = AppConfig::default();
        config.display.command_output_lines = MAX_COMMAND_OUTPUT_LINES + 1;
        assert!(config.validate().is_err());

        let display: DisplayConfig = serde_json::from_str(r#"{"show_token_usage":true}"#).unwrap();
        assert!(display.show_token_usage);

        let display: DisplayConfig =
            serde_json::from_str(r#"{"show_mixed_model_endpoint":false}"#).unwrap();
        assert_eq!(display.mixed_model_endpoint_display, "off");

        let display: DisplayConfig =
            serde_json::from_str(r#"{"show_mixed_model_endpoint":true}"#).unwrap();
        assert_eq!(display.mixed_model_endpoint_display, "all");
    }

    #[test]
    fn display_language_roundtrips_and_rejects_unknown_values() {
        let display: DisplayConfig = serde_json::from_str(r#"{"language":"zh"}"#).unwrap();
        assert_eq!(display.language, "zh");
        assert!(serde_json::to_string(&display)
            .unwrap()
            .contains(r#""language":"zh""#));

        let mut config = AppConfig::default();
        config.display.language = "fr".to_string();
        assert!(config.validate().is_err());
        config.display.language.clear();
        assert!(config.validate().is_err());
    }

    #[test]
    fn display_language_hint_reads_jsonc_without_loading_full_config() {
        let temp = tempfile::tempdir().unwrap();
        let config_file = temp.path().join("config.jsonc");
        std::fs::write(
            &config_file,
            "{\n  // UI preference\n  \"display\": { \"language\": \"en\" }\n}\n",
        )
        .unwrap();
        let paths = MiyuPaths {
            config_dir: temp.path().to_path_buf(),
            config_file,
            skills_dir: temp.path().join("skills"),
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            state_dir: temp.path().join("state"),
            pictures_dir: temp.path().join("pictures"),
            fish_hook_file: temp.path().join("miyu.fish"),
            bash_hook_file: temp.path().join("miyu.bash"),
            zsh_hook_file: temp.path().join("miyu.zsh"),
            scripts_dir: temp.path().join("scripts"),
            system_scripts_dir: temp.path().join("system-scripts"),
        };

        assert_eq!(
            AppConfig::display_language_hint(&paths).as_deref(),
            Some("en")
        );
    }

    #[test]
    fn meme_library_defaults_follow_persona() {
        let memes = MemesPluginConfig::default();
        assert_eq!(memes.library_for_persona(""), "miyu");
        assert_eq!(
            memes.library_for_persona("Custom Persona"),
            "custom-persona"
        );
        assert!(!memes.auto_send_enabled);
        assert_eq!(memes.search_max_results, 1);
        assert_eq!(memes.auto_send_probability, 0.2);
    }

    #[test]
    fn extra_body_roundtrip() {
        let original = ProviderConfig {
            id: "test".to_string(),
            display_name: "Test".to_string(),
            base_url: "https://example.com".to_string(),
            protocol: "auto".to_string(),
            api_key: None,
            models: vec![],
            model_context_window: HashMap::new(),
            model_modalities: HashMap::new(),
            default_model: String::new(),
            timeout_seconds: 60,
            temperature: 0.7,
            anthropic_max_tokens: 4096,
            extra_body: serde_json::json!({
                "enable_thinking": false,
                "reasoning_effort": "low"
            })
            .as_object()
            .cloned(),
        };

        let serialized = serde_json::to_string(&original).unwrap();
        let deserialized: ProviderConfig = serde_json::from_str(&serialized).unwrap();

        assert_eq!(original.extra_body, deserialized.extra_body);
        assert_eq!(original.id, deserialized.id);
    }

    #[test]
    fn extra_body_rejects_non_object_config_values() {
        for extra_body in [
            serde_json::json!(true),
            serde_json::json!("invalid"),
            serde_json::json!([1, 2, 3]),
        ] {
            let provider = serde_json::json!({
                "id": "test",
                "display_name": "Test",
                "base_url": "https://example.com",
                "extra_body": extra_body
            });

            assert!(serde_json::from_value::<ProviderConfig>(provider).is_err());
        }
    }
}
