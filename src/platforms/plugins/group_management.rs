use super::{require_ai_confirmation, PlatformPlugin, PlatformTurnInput, PluginDescriptor};
use crate::config::QqGroupManagementPluginSettings as Settings;
use crate::platforms::{
    BotGroupRole, ConversationKind, OutboundMessage, PlatformGroupMember, PlatformInboundEvent,
    PlatformInboundEventKind, PlatformTurnContext,
};
use crate::state::PlatformPluginScopeKey;
use crate::tools::{ToolRegistry, ToolSpec};
use anyhow::{bail, Context, Result};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const GROUP_MANAGEMENT_PLUGIN_ID: &str = "qq_group_management";
const ROLE_KEY: &str = "qq_group_management.bot_role";
const OFFENDERS_KEY: &str = "offender_history";
const KICKS_KEY: &str = "kick_history";
const MAX_TARGETS: usize = 32;
/// QQ caps a mute at 30 days; anything longer is rejected by the server, so
/// catch it here where the message can explain itself.
const MAX_BAN_SECONDS: u64 = 30 * 24 * 60 * 60;

fn settings(context: &PlatformTurnContext) -> Result<Settings> {
    context
        .config
        .platforms
        .qq
        .plugins
        .get(GROUP_MANAGEMENT_PLUGIN_ID)
        .map(Settings::from_instance)
        .transpose()
        .map(|settings| settings.unwrap_or_default())
}

#[derive(Clone, Debug, Serialize)]
struct BanRecord {
    record_id: String,
    group_id: String,
    user_id: String,
    user_name: String,
    duration: u64,
    started_at: i64,
    expires_at: i64,
    status: String,
    operator_id: String,
    reason: String,
    source: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct OffenderHistory {
    user_id: String,
    user_name: String,
    ban_count: u64,
    total_duration: u64,
    first_ban_at: i64,
    last_ban_at: i64,
    last_reason: String,
    reason_history: Vec<ReasonEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ReasonEntry {
    reason: String,
    duration: u64,
    banned_at: i64,
    operator_id: String,
    record_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct KickRecord {
    record_id: String,
    group_id: String,
    user_id: String,
    user_name: String,
    kicked_at: i64,
    operator_id: String,
    reason: String,
    reject_add_request: bool,
    source: String,
}

#[derive(Default)]
struct Runtime {
    bans: HashMap<String, Vec<BanRecord>>,
}

pub(crate) struct GroupManagementPlugin {
    runtime: Mutex<Runtime>,
}

impl GroupManagementPlugin {
    pub(crate) fn new() -> Self {
        Self {
            runtime: Mutex::new(Runtime::default()),
        }
    }

    fn scope(context: &PlatformTurnContext) -> PlatformPluginScopeKey {
        PlatformPluginScopeKey {
            plugin_id: GROUP_MANAGEMENT_PLUGIN_ID.to_string(),
            platform: context.conversation.platform.clone(),
            account_id: context.conversation.account_id.clone(),
            conversation_kind: context.conversation.kind.as_str().to_string(),
            conversation_id: context.conversation.conversation_id.clone(),
        }
    }

    fn visible(context: &PlatformTurnContext) -> bool {
        context.conversation.kind == ConversationKind::Group
    }

    async fn prepare_role(context: &PlatformTurnContext) {
        let role = match context.bot_group_role().await {
            BotGroupRole::Owner => "owner",
            BotGroupRole::Admin => "admin",
            BotGroupRole::Member => "member",
            BotGroupRole::Unknown => "unknown",
        };
        context.set_plugin_value(ROLE_KEY, Value::String(role.to_string()));
    }

    fn register(
        self: &Arc<Self>,
        registry: &mut ToolRegistry,
        context: Arc<PlatformTurnContext>,
    ) -> Result<()> {
        if !Self::visible(&context) {
            return Ok(());
        }
        let settings = settings(&context)?;
        if settings.enable_tool {
            self.register_ban(registry, context.clone());
            self.register_log_query(registry, context.clone());
            self.register_offender_query(registry, context.clone());
        }
        if settings.enable_kick_tool {
            self.register_kick(registry, context.clone(), false);
            self.register_kick(registry, context.clone(), true);
            self.register_kick_query(registry, context.clone());
        }
        if settings.enable_special_title_tool {
            self.register_title(registry, context);
        }
        Ok(())
    }

    fn register_ban(
        self: &Arc<Self>,
        registry: &mut ToolRegistry,
        context: Arc<PlatformTurnContext>,
    ) {
        let plugin = self.clone();
        registry.register(
            ToolSpec::new(
                "qq_group_manage_with_log",
                "Mute or unmute one or more members in the current QQ group and record the action. duration_seconds is in SECONDS (1 hour = 3600, 24 hours = 86400); 0 un-mutes.",
                json!({
                    "type": "object",
                    "properties": {
                        "user_id": { "type": "string", "description": "Optional QQ id or multiple QQ ids separated by spaces/commas. Falls back to mentions/reply." },
                        "duration_seconds": {
                            "type": ["integer", "null"],
                            "minimum": 0,
                            "maximum": MAX_BAN_SECONDS,
                            "description": "禁言秒数，不是分钟也不是小时：10 分钟=600，1 小时=3600，24 小时=86400，最长 30 天=2592000；0 表示解禁。"
                        },
                        "duration": {
                            "type": ["integer", "null"],
                            "minimum": 0,
                            "maximum": MAX_BAN_SECONDS,
                            "description": "Deprecated alias of duration_seconds (also seconds)."
                        },
                        "reason": { "type": "string" },
                        "confirmation_token": { "type": "string" }
                    },
                    "additionalProperties": false
                }),
                move |args| {
                    let plugin = plugin.clone();
                    let context = context.clone();
                    async move { plugin.ban(args, context).await }
                },
            )
            .writes()
            .with_display_name("QQ群禁言/解禁"),
        );
    }

    fn register_kick(
        self: &Arc<Self>,
        registry: &mut ToolRegistry,
        context: Arc<PlatformTurnContext>,
        blacklist: bool,
    ) {
        let plugin = self.clone();
        let name = if blacklist {
            "qq_group_manage_kick_black_with_log"
        } else {
            "qq_group_manage_kick_with_log"
        };
        registry.register(
            ToolSpec::new(
                name,
                if blacklist {
                    "Kick one or more members from the current QQ group and reject their future join requests. Pass every target in a single call; the result reports each target separately."
                } else {
                    "Kick one or more members from the current QQ group without blacklisting. Pass every target in a single call; the result reports each target separately."
                },
                kick_schema(),
                move |args| {
                    let plugin = plugin.clone();
                    let context = context.clone();
                    async move { plugin.kick(args, context, blacklist).await }
                },
            )
            .writes()
            .with_display_name(if blacklist {
                "QQ群踢黑"
            } else {
                "QQ群踢人"
            }),
        );
    }

    fn register_title(
        self: &Arc<Self>,
        registry: &mut ToolRegistry,
        context: Arc<PlatformTurnContext>,
    ) {
        let plugin = self.clone();
        registry.register(
            ToolSpec::new(
                "qq_group_set_special_title_with_log",
                "Set or clear one member's QQ group special title and record the action.",
                json!({
                    "type": "object",
                    "properties": {
                        "special_title": { "type": "string" },
                        "user_id": { "type": "string" },
                        "duration": { "type": "integer", "default": -1, "description": "头衔有效期秒数；-1 表示永久。" },
                        "reason": { "type": "string" },
                        "confirmation_token": { "type": "string" }
                    },
                    "required": ["special_title"],
                    "additionalProperties": false
                }),
                move |args| {
                    let plugin = plugin.clone();
                    let context = context.clone();
                    async move { plugin.title(args, context).await }
                },
            )
            .writes()
            .with_display_name("设置QQ群头衔"),
        );
    }

    fn register_log_query(
        self: &Arc<Self>,
        registry: &mut ToolRegistry,
        context: Arc<PlatformTurnContext>,
    ) {
        let plugin = self.clone();
        registry.register(ToolSpec::new(
            "qq_group_manage_log_query",
            "Query current-group mute/unmute records.",
            query_schema(true),
            move |args| {
                let plugin = plugin.clone();
                let context = context.clone();
                async move { plugin.query_logs(args, &context) }
            },
        ));
    }

    fn register_offender_query(
        self: &Arc<Self>,
        registry: &mut ToolRegistry,
        context: Arc<PlatformTurnContext>,
    ) {
        registry.register(ToolSpec::new(
            "qq_group_manage_offender_history_query",
            "Query persistent offender statistics for the current QQ group.",
            json!({
                "type": "object",
                "properties": {
                    "user_id": { "type": "string" },
                    "min_ban_count": { "type": "integer", "minimum": 1, "default": 1 },
                    "keyword": { "type": "string" },
                    "sort_by": { "type": "string", "enum": ["ban_count", "total_duration", "last_ban_at"] },
                    "sort_order": { "type": "string", "enum": ["asc", "desc"] },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
                },
                "additionalProperties": false
            }),
            move |args| {
                let context = context.clone();
                async move { query_offenders(args, &context) }
            },
        ));
    }

    fn register_kick_query(
        self: &Arc<Self>,
        registry: &mut ToolRegistry,
        context: Arc<PlatformTurnContext>,
    ) {
        registry.register(ToolSpec::new(
            "qq_group_manage_kick_history_query",
            "Query persistent kick history for the current QQ group.",
            query_schema(false),
            move |args| {
                let context = context.clone();
                async move { query_kicks(args, &context) }
            },
        ));
    }

    async fn ban(&self, args: Value, context: Arc<PlatformTurnContext>) -> Result<String> {
        let settings = settings(&context)?;
        // The parameter has always been seconds, but it used to be spelled
        // `duration` with no unit anywhere — models read it as minutes and a
        // "24 hour" mute came out as 24 minutes. The explicit name wins; the
        // old one still works.
        let duration = args
            .get("duration_seconds")
            .and_then(Value::as_u64)
            .or_else(|| args.get("duration").and_then(Value::as_u64))
            .unwrap_or(settings.default_duration_seconds);
        if duration > MAX_BAN_SECONDS {
            return json_result(
                false,
                &format!(
                    "禁言时长上限 {MAX_BAN_SECONDS} 秒（30 天），收到 {duration} 秒；注意该参数的单位是秒"
                ),
                Value::Null,
            );
        }
        let reason = bounded_reason(&args, &settings)?;
        let targets = resolve_targets(&args, &context)?;
        if let Some(prompt) = require_ai_confirmation(
            &context,
            "qq_group_manage_with_log",
            &json!({
                "arguments": args,
                "targets": targets,
                "duration": duration,
                "reason": reason,
            }),
        )
        .await?
        {
            return Ok(prompt);
        }
        let mut results = Vec::with_capacity(targets.len());
        for target in targets {
            results.push(
                self.ban_one(&context, &settings, &target, duration, &reason)
                    .await,
            );
        }
        Ok(aggregate_target_results(results).to_string())
    }

    async fn ban_one(
        &self,
        context: &PlatformTurnContext,
        settings: &Settings,
        user_id: &str,
        duration: u64,
        reason: &str,
    ) -> Value {
        let member = match validate_target(context, user_id, true).await {
            Ok(member) => member,
            Err(error) => return failure_for_target(error, user_id),
        };
        tracing::info!(
            action = if duration == 0 { "unmute" } else { "mute" },
            requester_id = %context.sender_id,
            target_id = user_id,
            duration,
            "recording QQ group management intent"
        );
        if let Err(error) = context.set_group_ban(user_id, duration).await {
            return failure_for_target(error, user_id);
        }
        let now = now_unix();
        let record_id = record_id();
        let status = if duration == 0 { "unmuted" } else { "active" };
        let record = BanRecord {
            record_id: record_id.clone(),
            group_id: context.conversation.conversation_id.clone(),
            user_id: user_id.to_string(),
            user_name: member.display_name().to_string(),
            duration,
            started_at: now,
            expires_at: now.saturating_add(duration as i64),
            status: status.to_string(),
            operator_id: context.sender_id.clone(),
            reason: reason.to_string(),
            source: "llm_tool".to_string(),
        };
        if settings.enable_record {
            let mut runtime = self.runtime.lock().unwrap();
            let records = runtime
                .bans
                .entry(context.conversation.scope_key())
                .or_default();
            if duration > 0 {
                for old in records
                    .iter_mut()
                    .filter(|old| old.user_id == user_id && old.status == "active")
                {
                    old.status = "overridden".to_string();
                }
            } else {
                for old in records
                    .iter_mut()
                    .filter(|old| old.user_id == user_id && old.status == "active")
                {
                    old.status = "unmuted".to_string();
                }
            }
            records.push(record.clone());
            trim_vec(records, settings.max_records_per_group);
        }
        let mut audit_errors = Vec::new();
        if duration > 0 && settings.enable_offender_history {
            if let Err(error) = update_offender(context, settings, &record) {
                audit_errors.push(format!("offender history: {error}"));
            }
        }
        if let Err(error) = record_real_context(
            context,
            &record_id,
            if duration == 0 { "解禁" } else { "禁言" },
            &member,
            reason,
            Some(duration),
        )
        .await
        {
            audit_errors.push(format!("real context: {error}"));
        }
        let mut result = external_operation_result(json!({ "record": record }), audit_errors);
        result["user_id"] = json!(user_id);
        // Echo the duration in words so a unit mix-up is visible in the result
        // instead of only on the victim's client.
        result["duration_seconds"] = json!(duration);
        result["duration_text"] = json!(humanize_seconds(duration));
        result
    }

    async fn kick(
        &self,
        args: Value,
        context: Arc<PlatformTurnContext>,
        blacklist: bool,
    ) -> Result<String> {
        let settings = settings(&context)?;
        let reason = bounded_reason(&args, &settings)?;
        let targets = resolve_targets(&args, &context)?;
        if targets.is_empty() {
            return json_result(false, "没有解析出踢人目标", Value::Null);
        }
        let action = if blacklist {
            "qq_group_manage_kick_black_with_log"
        } else {
            "qq_group_manage_kick_with_log"
        };
        if let Some(prompt) = require_ai_confirmation(
            &context,
            action,
            &json!({ "arguments": args, "targets": targets, "blacklist": blacklist }),
        )
        .await?
        {
            return Ok(prompt);
        }
        // Sequential on purpose: kicks are destructive and the bridge throttles
        // them anyway. Per-target results carry their own retry verdict, so one
        // failure no longer sinks the whole call.
        let mut results = Vec::with_capacity(targets.len());
        for target in &targets {
            results.push(
                self.kick_one(&context, &settings, target, blacklist, &reason)
                    .await,
            );
        }
        Ok(aggregate_target_results(results).to_string())
    }

    async fn kick_one(
        &self,
        context: &PlatformTurnContext,
        settings: &Settings,
        user_id: &str,
        blacklist: bool,
        reason: &str,
    ) -> Value {
        let member = match validate_target(context, user_id, true).await {
            Ok(member) => member,
            Err(error) => return failure_for_target(error, user_id),
        };
        tracing::info!(
            action = if blacklist { "kick_blacklist" } else { "kick" },
            requester_id = %context.sender_id,
            target_id = user_id,
            "recording QQ group management intent"
        );
        if let Err(error) = context.set_group_kick(user_id, blacklist).await {
            return failure_for_target(error, user_id);
        }
        let record = KickRecord {
            record_id: record_id(),
            group_id: context.conversation.conversation_id.clone(),
            user_id: user_id.to_string(),
            user_name: member.display_name().to_string(),
            kicked_at: now_unix(),
            operator_id: context.sender_id.clone(),
            reason: reason.to_string(),
            reject_add_request: blacklist,
            source: "llm_tool".to_string(),
        };
        let mut audit_errors = Vec::new();
        if let Err(error) = append_kick(context, &record, settings.max_kick_history_per_group) {
            audit_errors.push(format!("kick history: {error}"));
        }
        if let Err(error) = record_real_context(
            context,
            &record.record_id,
            if blacklist {
                "踢出并拉黑"
            } else {
                "踢出"
            },
            &member,
            reason,
            None,
        )
        .await
        {
            audit_errors.push(format!("real context: {error}"));
        }
        external_operation_result(json!({ "record": record }), audit_errors)
    }

    async fn title(&self, args: Value, context: Arc<PlatformTurnContext>) -> Result<String> {
        let settings = settings(&context)?;
        let targets = resolve_targets(&args, &context)?;
        if targets.len() != 1 {
            return json_result(false, "群头衔操作必须且只能指定一个目标", Value::Null);
        }
        let title = args
            .get("special_title")
            .and_then(Value::as_str)
            .context("special_title is required")?
            .trim();
        if title.chars().count() > settings.max_special_title_length {
            return json_result(false, "群头衔超过配置长度限制", Value::Null);
        }
        let duration = args.get("duration").and_then(Value::as_i64).unwrap_or(-1);
        let duration = if duration < 0 {
            -1
        } else if settings.max_special_title_duration_seconds > 0 {
            duration.min(settings.max_special_title_duration_seconds)
        } else {
            duration
        };
        let reason = bounded_reason(&args, &settings)?;
        let member = match validate_target(&context, &targets[0], false).await {
            Ok(member) => member,
            Err(error) => return json_result(false, &error.to_string(), Value::Null),
        };
        if let Some(prompt) = require_ai_confirmation(
            &context,
            "qq_group_set_special_title_with_log",
            &json!({
                "arguments": args,
                "target": targets[0],
                "special_title": title,
                "duration": duration,
                "reason": reason,
            }),
        )
        .await?
        {
            return Ok(prompt);
        }
        tracing::info!(
            action = if title.is_empty() { "clear_title" } else { "set_title" },
            requester_id = %context.sender_id,
            target_id = %targets[0],
            "recording QQ group management intent"
        );
        if let Err(error) = context
            .set_group_special_title(&targets[0], title, duration)
            .await
        {
            return json_result(false, &error.to_string(), Value::Null);
        }
        let id = record_id();
        let mut audit_errors = Vec::new();
        if let Err(error) = record_real_context(
            &context,
            &id,
            if title.is_empty() {
                "清除群头衔"
            } else {
                "设置群头衔"
            },
            &member,
            &reason,
            None,
        )
        .await
        {
            audit_errors.push(format!("real context: {error}"));
        }
        Ok(external_operation_result(
            json!({ "record_id": id, "user_id": targets[0], "special_title": title, "duration": duration }),
            audit_errors,
        )
        .to_string())
    }

    fn query_logs(&self, args: Value, context: &PlatformTurnContext) -> Result<String> {
        let include_history = args
            .get("include_history")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let user_id = optional_id(&args);
        let limit = limit(&args);
        let now = now_unix();
        let mut runtime = self.runtime.lock().unwrap();
        let records = runtime
            .bans
            .entry(context.conversation.scope_key())
            .or_default();
        for record in records
            .iter_mut()
            .filter(|record| record.status == "active" && record.expires_at <= now)
        {
            record.status = "expired".to_string();
        }
        let records = records
            .iter()
            .rev()
            .filter(|record| include_history || record.status == "active")
            .filter(|record| user_id.as_deref().is_none_or(|id| record.user_id == id))
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        json_result(
            true,
            "查询成功",
            json!({ "count": records.len(), "records": records }),
        )
    }
}

impl PlatformPlugin for Arc<GroupManagementPlugin> {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            id: GROUP_MANAGEMENT_PLUGIN_ID,
            // Resolve the bot's current group role before recall and real-context
            // plugins build their per-turn capability prompts.
            priority: 210,
            default_enabled: true,
        }
    }

    fn register_tools(
        &self,
        registry: &mut ToolRegistry,
        context: Arc<PlatformTurnContext>,
    ) -> Result<()> {
        self.register(registry, context)
    }

    fn before_turn<'a>(
        &'a self,
        context: &'a PlatformTurnContext,
        input: &'a mut PlatformTurnInput,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if context.conversation.kind == ConversationKind::Group {
                GroupManagementPlugin::prepare_role(context).await;
                input.system_context.push("<qq-group-management>执行群管理动作前必须调用对应工具；只有工具返回 success=true 后才能声称动作已经完成。普通成员触发敏感动作时，工具可能要求在本轮原样再次调用同一工具进行确认。</qq-group-management>".to_string());
            }
            Ok(())
        })
    }

    fn observe_inbound<'a>(
        &'a self,
        context: &'a PlatformTurnContext,
        event: &'a PlatformInboundEvent,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if event.operator_id.as_deref() == Some(context.conversation.account_id.as_str()) {
                return Ok(());
            }
            let settings = settings(context)?;
            match event.kind {
                PlatformInboundEventKind::GroupBan if settings.enable_record => {
                    let duration = event.duration_seconds.unwrap_or(0);
                    if duration == 0 && !settings.sync_external_unmute_notice {
                        return Ok(());
                    }
                    let now = event.timestamp.max(now_unix());
                    let record = BanRecord {
                        record_id: record_id(),
                        group_id: context.conversation.conversation_id.clone(),
                        user_id: event.sender_id.clone(),
                        user_name: event.sender_display_name.clone(),
                        duration,
                        started_at: now,
                        expires_at: now.saturating_add(duration as i64),
                        status: if duration == 0 { "unmuted" } else { "active" }.to_string(),
                        operator_id: event.operator_id.clone().unwrap_or_default(),
                        reason: String::new(),
                        source: "onebot_notice".to_string(),
                    };
                    {
                        let mut runtime = self.runtime.lock().unwrap();
                        let records = runtime
                            .bans
                            .entry(context.conversation.scope_key())
                            .or_default();
                        records.push(record.clone());
                        trim_vec(records, settings.max_records_per_group);
                    }
                    let member = notice_member(context, event);
                    record_real_context(
                        context,
                        &record.record_id,
                        if duration == 0 {
                            "外部解禁"
                        } else {
                            "外部禁言"
                        },
                        &member,
                        "",
                        Some(duration),
                    )
                    .await?;
                }
                PlatformInboundEventKind::GroupDecrease => {
                    // Whoever left is gone regardless of how: drop them from
                    // the per-turn roster cache so a later kick/mute in this
                    // same turn cannot validate against a stale entry.
                    context.forget_group_member(&event.sender_id);
                    if event.notice_sub_type.as_deref() != Some("kick") {
                        return Ok(());
                    }
                    let record = KickRecord {
                        record_id: record_id(),
                        group_id: context.conversation.conversation_id.clone(),
                        user_id: event.sender_id.clone(),
                        user_name: event.sender_display_name.clone(),
                        kicked_at: event.timestamp.max(now_unix()),
                        operator_id: event.operator_id.clone().unwrap_or_default(),
                        reason: String::new(),
                        reject_add_request: false,
                        source: "onebot_notice".to_string(),
                    };
                    append_kick(context, &record, settings.max_kick_history_per_group)?;
                    let member = notice_member(context, event);
                    record_real_context(context, &record.record_id, "外部踢出", &member, "", None)
                        .await?;
                }
                _ => {}
            }
            Ok(())
        })
    }

    fn before_send<'a>(
        &'a self,
        _context: &'a PlatformTurnContext,
        message: OutboundMessage,
    ) -> BoxFuture<'a, Result<super::PreparedSend>> {
        Box::pin(async move { Ok(super::PreparedSend::unchanged(message)) })
    }
}

async fn validate_target(
    context: &PlatformTurnContext,
    user_id: &str,
    protect_managers: bool,
) -> Result<PlatformGroupMember> {
    if user_id == context.conversation.account_id {
        bail!("不能对 Miyu 自身执行该操作");
    }
    // Fresh lookup on purpose: this gate exists to stop kicks/mutes aimed at
    // members who already left, and a cached roster cannot answer that.
    let member = context
        .group_member_fresh(user_id)
        .await?
        .context("目标不在当前群中")?;
    if protect_managers && matches!(member.role.as_str(), "owner" | "admin") {
        bail!("不能对群主或管理员执行该操作");
    }
    Ok(member)
}

fn resolve_targets(args: &Value, context: &PlatformTurnContext) -> Result<Vec<String>> {
    let mut values = Vec::new();
    if let Some(list) = args.get("user_ids").and_then(Value::as_array) {
        values.extend(
            list.iter()
                .filter_map(Value::as_str)
                .flat_map(split_ids)
                .collect::<Vec<_>>(),
        );
    }
    if let Some(explicit) = args
        .get("user_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        values.extend(split_ids(explicit));
    } else if values.is_empty() {
        // Neither form given: fall back to who was mentioned, then to whoever
        // wrote the replied-to message.
        if let Some(event) = context.inbound_event() {
            values.extend(event.mentioned_user_ids.iter().cloned());
            if values.is_empty() {
                if let Some(replied) = event.replied_message.as_ref() {
                    values.push(replied.sender_id.clone());
                }
            }
        }
    }
    let mut seen = HashSet::new();
    values.retain(|id| {
        valid_id(id) && id != &context.conversation.account_id && seen.insert(id.clone())
    });
    values.truncate(MAX_TARGETS);
    if values.is_empty() {
        bail!("未找到有效的目标 QQ 号");
    }
    Ok(values)
}

fn split_ids(value: &str) -> Vec<String> {
    value
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| (5..=12).contains(&part.len()))
        .map(str::to_string)
        .collect()
}

/// Renders a mute duration the way a person would say it, so the model can
/// sanity-check its own arithmetic against what it intended.
fn humanize_seconds(seconds: u64) -> String {
    if seconds == 0 {
        return "解禁".to_string();
    }
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (hours, rest) = (rest / 3_600, rest % 3_600);
    let (minutes, secs) = (rest / 60, rest % 60);
    let mut parts = Vec::new();
    for (value, unit) in [
        (days, "天"),
        (hours, "小时"),
        (minutes, "分钟"),
        (secs, "秒"),
    ] {
        if value > 0 {
            parts.push(format!("{value}{unit}"));
        }
    }
    parts.join("")
}

fn valid_id(value: &str) -> bool {
    (5..=12).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn notice_member(
    context: &PlatformTurnContext,
    event: &PlatformInboundEvent,
) -> PlatformGroupMember {
    PlatformGroupMember {
        group_id: context.conversation.conversation_id.clone(),
        user_id: event.sender_id.clone(),
        nickname: event.sender_display_name.clone(),
        card: String::new(),
        role: "member".to_string(),
        title: String::new(),
        joined_at: 0,
        last_active_at: 0,
    }
}

fn bounded_reason(args: &Value, settings: &Settings) -> Result<String> {
    let reason = args
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if reason.chars().count() > settings.max_reason_length {
        bail!("reason exceeds configured maximum length");
    }
    Ok(reason.to_string())
}

fn optional_id(args: &Value) -> Option<String> {
    args.get("user_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| valid_id(id))
        .map(str::to_string)
}

fn limit(args: &Value) -> usize {
    args.get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(20)
        .clamp(1, 100) as usize
}

fn update_offender(
    context: &PlatformTurnContext,
    settings: &Settings,
    record: &BanRecord,
) -> Result<()> {
    context.state_store.plugin_update_json(
        &GroupManagementPlugin::scope(context),
        OFFENDERS_KEY,
        |current: Option<HashMap<String, OffenderHistory>>| {
            let mut map = current.unwrap_or_default();
            let entry = map
                .entry(record.user_id.clone())
                .or_insert_with(|| OffenderHistory {
                    user_id: record.user_id.clone(),
                    user_name: record.user_name.clone(),
                    first_ban_at: record.started_at,
                    ..OffenderHistory::default()
                });
            entry.user_name.clone_from(&record.user_name);
            entry.ban_count = entry.ban_count.saturating_add(1);
            entry.total_duration = entry.total_duration.saturating_add(record.duration);
            entry.last_ban_at = record.started_at;
            entry.last_reason.clone_from(&record.reason);
            entry.reason_history.push(ReasonEntry {
                reason: record.reason.clone(),
                duration: record.duration,
                banned_at: record.started_at,
                operator_id: record.operator_id.clone(),
                record_id: record.record_id.clone(),
            });
            if map.len() > settings.max_offender_history_per_group {
                if let Some(remove) = map
                    .iter()
                    .min_by_key(|(_, item)| (item.ban_count, item.last_ban_at))
                    .map(|(id, _)| id.clone())
                {
                    map.remove(&remove);
                }
            }
            Ok(Some(map))
        },
    )?;
    Ok(())
}

fn append_kick(context: &PlatformTurnContext, record: &KickRecord, max: usize) -> Result<()> {
    context.state_store.plugin_update_json(
        &GroupManagementPlugin::scope(context),
        KICKS_KEY,
        |current: Option<Vec<KickRecord>>| {
            let mut records = current.unwrap_or_default();
            records.push(record.clone());
            trim_vec(&mut records, max);
            Ok(Some(records))
        },
    )?;
    Ok(())
}

fn query_offenders(args: Value, context: &PlatformTurnContext) -> Result<String> {
    let map = context
        .state_store
        .plugin_get_json::<HashMap<String, OffenderHistory>>(
            &GroupManagementPlugin::scope(context),
            OFFENDERS_KEY,
        )?
        .unwrap_or_default();
    let user_id = optional_id(&args);
    let keyword = args
        .get("keyword")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    let minimum = args
        .get("min_ban_count")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let sort_by = args
        .get("sort_by")
        .and_then(Value::as_str)
        .unwrap_or("ban_count");
    let ascending = args.get("sort_order").and_then(Value::as_str) == Some("asc");
    let mut items = map
        .into_values()
        .filter(|item| item.ban_count >= minimum)
        .filter(|item| user_id.as_deref().is_none_or(|id| item.user_id == id))
        .filter(|item| {
            keyword.is_empty()
                || item.user_name.to_lowercase().contains(&keyword)
                || item.last_reason.to_lowercase().contains(&keyword)
        })
        .collect::<Vec<_>>();
    items.sort_by_key(|item| match sort_by {
        "total_duration" => item.total_duration as i64,
        "last_ban_at" => item.last_ban_at,
        _ => item.ban_count as i64,
    });
    if !ascending {
        items.reverse();
    }
    items.truncate(limit(&args));
    json_result(
        true,
        "查询成功",
        json!({ "count": items.len(), "records": items }),
    )
}

fn query_kicks(args: Value, context: &PlatformTurnContext) -> Result<String> {
    let records = context
        .state_store
        .plugin_get_json::<Vec<KickRecord>>(&GroupManagementPlugin::scope(context), KICKS_KEY)?
        .unwrap_or_default();
    let user_id = optional_id(&args);
    let keyword = args
        .get("keyword")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    let records = records
        .into_iter()
        .rev()
        .filter(|record| user_id.as_deref().is_none_or(|id| record.user_id == id))
        .filter(|record| {
            keyword.is_empty()
                || record.user_name.to_lowercase().contains(&keyword)
                || record.reason.to_lowercase().contains(&keyword)
        })
        .take(limit(&args))
        .collect::<Vec<_>>();
    json_result(
        true,
        "查询成功",
        json!({ "count": records.len(), "records": records }),
    )
}

async fn record_real_context(
    context: &PlatformTurnContext,
    id: &str,
    action: &str,
    member: &PlatformGroupMember,
    reason: &str,
    duration: Option<u64>,
) -> Result<()> {
    let mut text = format!(
        "[System:群管理行为]\n操作：{action}\n执行者：Miyu（{}）\n对象：{}（{}）",
        context.conversation.account_id,
        member.display_name(),
        member.user_id
    );
    if let Some(duration) = duration {
        text.push_str(&format!("\n时长：{duration} 秒"));
    }
    if !reason.is_empty() {
        text.push_str(&format!("\n原因：{reason}"));
    }
    text.push_str(&format!("\n记录 ID：{id}"));
    context
        .plugins
        .try_record_external_bot_message(context, &format!("qq-management-{id}"), &text)
        .await
}

/// Kick takes a real array so batching is discoverable from the schema rather
/// than buried in prose — the scalar form stays for single targets and for
/// falling back to mentions/reply.
fn kick_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "user_ids": {
                "type": "array",
                "minItems": 1,
                "items": { "type": "string", "pattern": "^[1-9][0-9]{4,11}$" },
                "description": "QQ ids to kick. Prefer this over repeating the tool once per member."
            },
            "user_id": { "type": "string", "description": "Single QQ id, or several separated by spaces/commas. Falls back to mentions/reply when omitted." },
            "reason": { "type": "string" },
            "confirmation_token": { "type": "string" }
        },
        "additionalProperties": false
    })
}

fn reason_schema() -> Value {
    json!({ "type": "object", "properties": { "user_id": { "type": "string" }, "reason": { "type": "string" }, "confirmation_token": { "type": "string" } }, "additionalProperties": false })
}

fn query_schema(include_history: bool) -> Value {
    let mut properties = serde_json::Map::from_iter([
        ("user_id".to_string(), json!({ "type": "string" })),
        ("keyword".to_string(), json!({ "type": "string" })),
        (
            "limit".to_string(),
            json!({ "type": "integer", "minimum": 1, "maximum": 100 }),
        ),
    ]);
    if include_history {
        properties.insert(
            "include_history".to_string(),
            json!({ "type": "boolean", "default": false }),
        );
    }
    json!({ "type": "object", "properties": properties, "additionalProperties": false })
}

fn trim_vec<T>(values: &mut Vec<T>, max: usize) {
    let max = max.max(1);
    if values.len() > max {
        values.drain(..values.len() - max);
    }
}

fn record_id() -> String {
    format!("{:012x}", rand::random::<u64>() & 0xffffffffffff)
}
fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn failure(error: anyhow::Error) -> Value {
    json!({
        "success": false,
        "operation_succeeded": false,
        "audit_succeeded": false,
        "do_not_retry": false,
        "message": error.to_string()
    })
}
fn failure_for_target(error: anyhow::Error, user_id: &str) -> Value {
    let mut result = failure(error);
    result["user_id"] = json!(user_id);
    result
}
fn external_operation_result(data: Value, audit_errors: Vec<String>) -> Value {
    let audit_succeeded = audit_errors.is_empty();
    json!({
        "success": true,
        "operation_succeeded": true,
        "audit_succeeded": audit_succeeded,
        "do_not_retry": true,
        "message": if audit_succeeded {
            "操作成功".to_string()
        } else {
            "外部操作已成功，但本地审计记录失败；请勿重试外部操作".to_string()
        },
        "audit_errors": audit_errors,
        "data": data,
    })
}

/// Partial-success envelope shared by the batched admin actions.
///
/// The per-target retry verdict is the important part: without it the model
/// cannot tell a hopeless failure from a transient one and hammers the same
/// target again — which is exactly what a batch kick against departed members
/// used to do.
fn aggregate_target_results(results: Vec<Value>) -> Value {
    let mut successful_target_ids = Vec::new();
    let mut failed_target_ids = Vec::new();
    let mut audit_failed_count = 0usize;
    for result in &results {
        let target_id = result
            .get("user_id")
            .and_then(Value::as_str)
            .or_else(|| {
                result
                    .pointer("/data/record/user_id")
                    .and_then(Value::as_str)
            })
            .unwrap_or_default()
            .to_string();
        if result["operation_succeeded"] == true {
            successful_target_ids.push(target_id);
            if result["audit_succeeded"] == false {
                audit_failed_count += 1;
            }
        } else {
            failed_target_ids.push(target_id);
        }
    }
    let success_count = successful_target_ids.len();
    let failed_count = failed_target_ids.len();
    let has_failures = failed_count > 0;
    let message = if audit_failed_count > 0 || has_failures {
        format!(
            "外部操作成功 {success_count} 个、失败 {failed_count} 个；成功目标不得重试，失败目标可单独重试"
        )
    } else {
        format!("成功 {success_count} 个，失败 {failed_count} 个")
    };
    json!({
        "success": success_count > 0,
        "operation_succeeded": success_count > 0,
        "audit_succeeded": audit_failed_count == 0,
        "do_not_retry": !has_failures,
        "do_not_retry_successful_targets": true,
        "retry_failed_targets_only": has_failures,
        "message": message,
        "success_count": success_count,
        "failed_count": failed_count,
        "audit_failed_count": audit_failed_count,
        "successful_target_ids": successful_target_ids,
        "failed_target_ids": failed_target_ids,
        "results": results,
    })
}

fn json_result(success: bool, message: &str, data: Value) -> Result<String> {
    Ok(json!({ "success": success, "message": message, "data": data }).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_parser_accepts_only_qq_sized_numeric_ids() {
        assert_eq!(
            split_ids("12345, 12345 @678901 and invalid-12"),
            vec!["12345", "12345", "678901"]
        );
        assert!(valid_id("12345"));
        assert!(!valid_id("1234"));
        assert!(!valid_id("12a45"));
    }

    #[test]
    fn mute_duration_is_spelled_in_seconds_and_reads_back_in_words() {
        assert_eq!(humanize_seconds(0), "解禁");
        assert_eq!(humanize_seconds(600), "10分钟");
        assert_eq!(humanize_seconds(3_600), "1小时");
        // The exact case that shipped as 24 minutes: 24h must be 86400, and a
        // 1440 that a model meant as "minutes" must read back as 24 minutes so
        // the mistake is visible in the result.
        assert_eq!(humanize_seconds(86_400), "1天");
        assert_eq!(humanize_seconds(1_440), "24分钟");
        assert_eq!(humanize_seconds(MAX_BAN_SECONDS), "30天");
        assert_eq!(humanize_seconds(90), "1分钟30秒");
    }

    #[test]
    fn kick_targets_accept_an_array_and_still_fall_back_to_a_scalar() {
        // The array form is what the schema now advertises; the scalar and its
        // space/comma splitting stay for single targets and older habits.
        let ids = |value: &Value| -> Vec<String> {
            value
                .get("user_ids")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(Value::as_str)
                        .flat_map(split_ids)
                        .collect()
                })
                .unwrap_or_default()
        };
        assert_eq!(
            ids(&json!({ "user_ids": ["12345", "678901"] })),
            vec!["12345".to_string(), "678901".to_string()]
        );
        assert_eq!(split_ids("12345 678901"), vec!["12345", "678901"]);
    }

    #[test]
    fn batch_results_tell_the_model_which_targets_may_be_retried() {
        let aggregate = aggregate_target_results(vec![
            external_operation_result(json!({ "record": { "user_id": "12345" } }), Vec::new()),
            failure_for_target(anyhow::anyhow!("目标不在当前群中"), "678901"),
        ]);
        assert_eq!(aggregate["success_count"], 1);
        assert_eq!(aggregate["failed_count"], 1);
        // Mixed outcome: the successes must never be retried, the failure may
        // be retried on its own. Without this the model re-kicked the same
        // dead target over and over.
        assert_eq!(aggregate["do_not_retry"], false);
        assert_eq!(aggregate["do_not_retry_successful_targets"], true);
        assert_eq!(aggregate["retry_failed_targets_only"], true);
        assert_eq!(aggregate["failed_target_ids"], json!(["678901"]));

        let all_good = aggregate_target_results(vec![external_operation_result(
            json!({ "record": { "user_id": "12345" } }),
            Vec::new(),
        )]);
        assert_eq!(all_good["do_not_retry"], true);
    }

    #[test]
    fn astrbot_defaults_are_preserved() {
        let settings = Settings::default();
        assert_eq!(settings.default_duration_seconds, 600);
        assert_eq!(settings.max_reason_length, 500);
        assert_eq!(settings.max_records_per_group, 500);
    }

    #[test]
    fn response_contract_uses_success_and_message() {
        let response: Value =
            serde_json::from_str(&json_result(true, "ok", json!({ "record_id": "abc" })).unwrap())
                .unwrap();
        assert_eq!(response["success"], true);
        assert_eq!(response["message"], "ok");
    }

    #[test]
    fn audit_failure_reports_partial_success_and_forbids_retry() {
        let response = external_operation_result(
            json!({ "record_id": "abc" }),
            vec!["injected audit failure".to_string()],
        );
        assert_eq!(response["success"], true);
        assert_eq!(response["operation_succeeded"], true);
        assert_eq!(response["audit_succeeded"], false);
        assert_eq!(response["do_not_retry"], true);
        assert!(response["message"].as_str().unwrap().contains("请勿重试"));
    }

    #[test]
    fn external_failure_remains_retryable() {
        let response = failure(anyhow::anyhow!("injected external failure"));
        assert_eq!(response["success"], false);
        assert_eq!(response["operation_succeeded"], false);
        assert_eq!(response["do_not_retry"], false);
    }
}
