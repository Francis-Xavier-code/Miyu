//! 同会话长任务目标:面向模型的三件套 + `/goal` 人类命令(任务#11)。
//!
//! 状态真源在 SQLite(state::goals,任务#9);本模块补三层:
//! - **armed 激活注册表**(进程内存,绝不落库):daemon 重启后目标还在但
//!   自动续跑必须由人 resume 重新授权——dsh 的关键安全阀。
//! - **权限二元分立**(dsh tool-goal/authority 同款,靠回合来源标记而非
//!   扫会话事件):create/edit/pause/resume 只认人类发起轮;complete/
//!   blocked 额外接受"恰好当前 goal round"。
//! - **wrapup 侧信道**:自主轮里报 complete/blocked 后,主循环取走一段
//!   收尾指令注入(模型面向用户写结案陈词,而不是硬停);人类直改不触发。

use super::{ToolRegistry, ToolSpec};
use crate::paths::MiyuPaths;
use crate::state::{GoalDenied, GoalPhase, GoalRecord, StateStore};
use crate::tools::workspace::{self, TurnOrigin};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

/// 模型自报 blocked 的机械下限:同一阻塞至少持续这么多连续 goal round
/// 才收(dsh blockedAfterConsecutiveRounds 默认同值);人类直接授权不受限。
pub const BLOCKED_AFTER_CONSECUTIVE_ROUNDS: i64 = 3;

// ------------------------------ armed 注册表 ------------------------------

fn armed_sessions() -> &'static Mutex<HashSet<String>> {
    static ARMED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    ARMED.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn is_armed(session_id: &str) -> bool {
    armed_sessions().lock().unwrap().contains(session_id)
}

pub fn set_armed(session_id: &str, armed: bool) {
    let mut set = armed_sessions().lock().unwrap();
    if armed {
        set.insert(session_id.to_string());
    } else {
        set.remove(session_id);
    }
}

// ------------------------------ wrapup 侧信道 ------------------------------

fn pending_wrapups() -> &'static Mutex<HashMap<String, String>> {
    static PENDING: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 主循环在每次工具结算后取走(有则注入为回合上下文)。
pub fn take_pending_wrapup(session_id: &str) -> Option<String> {
    pending_wrapups().lock().unwrap().remove(session_id)
}

const WRAPUP_GROUNDING: &str = "Report only what earlier rounds and tool results in this \
     session actually establish; when a detail is not in the session, say so instead of \
     inventing it. ";

/// dsh wrapup.ts 同款收尾指令:自主轮终结后模型仍面向用户交代一次。
fn wrapup_text(objective: &str, blocked_reason: Option<&str>) -> String {
    let heading = format!("Objective: {}\n", json!(objective));
    match blocked_reason {
        None => format!(
            "<goal_complete>\n{heading}The goal is marked complete and this autonomous run is \
             ending. Write the closing message to the user now: state the outcome, summarize \
             what was done and how it was verified, and point to the concrete results (files, \
             commits, or other artifacts). {WRAPUP_GROUNDING}Note anything the user should \
             review or do next. Address the user directly. Do not call any more tools in this \
             run; further work waits for the user's next instruction.\n</goal_complete>"
        ),
        Some(reason) => format!(
            "<goal_blocked>\n{heading}Blocked: {}\nThe goal is marked blocked and this \
             autonomous run is ending. Write the closing message to the user now: state what \
             has been completed so far, describe the concrete blocking condition and what you \
             tried, and say exactly what you need from the user to continue. {WRAPUP_GROUNDING}\
             Address the user directly. Do not call any more tools in this run; further work \
             waits for the user's next instruction.\n</goal_blocked>",
            json!(reason)
        ),
    }
}

// ------------------------------ 渲染与权限 ------------------------------

fn store(paths: &MiyuPaths) -> Result<StateStore> {
    StateStore::new(paths)
}

fn session_for_call() -> Result<String> {
    workspace::try_session()
        .map(|session| session.to_string())
        .ok_or_else(|| anyhow::anyhow!("goal tools require a session turn"))
}

fn goal_value(goal: Option<&GoalRecord>, session_id: &str) -> Value {
    match goal {
        None => json!({ "goal": null }),
        Some(goal) => json!({
            "goal": {
                "id": goal.goal_id,
                "revision": goal.revision,
                "objective": goal.objective,
                "phase": goal.phase.as_str(),
                "roundsStarted": goal.rounds_started,
                "maxGoalRounds": goal.max_rounds,
                "blockedReason": goal.blocked_code.as_ref().map(|code| json!({
                    "code": code,
                    "message": goal.blocked_message.clone().unwrap_or_default(),
                })),
            },
            "activation": if is_armed(session_id) { "armed" } else { "disarmed" },
        }),
    }
}

fn render_goal(goal: Option<&GoalRecord>, session_id: &str) -> String {
    goal_value(goal, session_id).to_string()
}

/// dsh 严格 schema 填充容错:空串/0 视为"未提供"。
fn meaningful_text(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|text| !text.is_empty())
}

fn meaningful_rounds(value: Option<i64>) -> Option<i64> {
    value.filter(|rounds| *rounds != 0)
}

/// 本轮是否恰好是当前目标的已认领 round(自主完成/受阻权限)。
fn origin_matches_goal(origin: &TurnOrigin, goal: &GoalRecord) -> bool {
    matches!(origin, TurnOrigin::GoalRound { goal_id, revision, round }
        if *goal_id == goal.goal_id
            && *revision == goal.revision
            && *round == goal.rounds_started)
}

fn require_human(origin: &TurnOrigin, verb: &str) -> Result<()> {
    if matches!(origin, TurnOrigin::Human) {
        return Ok(());
    }
    bail!("goal {verb} requires a direct human turn (this turn was started automatically)");
}

// ------------------------------ 工具注册 ------------------------------

pub fn register(registry: &mut ToolRegistry, paths: MiyuPaths) {
    let t = crate::i18n::agent_text;
    let get_paths = paths.clone();
    registry.register(
        ToolSpec::new(
            "get_goal",
            t(
                "Read the current same-session goal: exact id/revision for compare-and-set, objective, phase, rounds used/limit, blocker when present, and whether autonomous continuation is armed. Call this before update_goal.",
                "读取本会话当前目标:用于比较并设置的精确 id/revision、目标、阶段、已用/上限轮数、阻塞原因,以及自动续跑是否已武装。调用 update_goal 前先调它。",
            ),
            super::registry::empty_parameters(),
            move |_args| {
                let paths = get_paths.clone();
                async move {
                    let session = session_for_call()?;
                    let goal = store(&paths)?.goal(&session)?;
                    Ok(render_goal(goal.as_ref(), &session))
                }
            },
        )
        .with_always_loaded(false),
    );

    let create_paths = paths.clone();
    registry.register(
        ToolSpec::new(
            "create_goal",
            t(
                "Create one persisted same-session completion goal when the current direct human request is a long-running objective that should continue across autonomous goal rounds. Infer that intent from any language; do not use this for trivial single-turn work. Rejected on automatic turns.",
                "当当前人类请求是需要跨自动轮持续推进的长任务时,创建本会话唯一的持久目标。可从任意语言的请求推断意图;琐碎单轮工作不要建目标。自动轮调用会被拒。",
            ),
            json!({
                "type": "object",
                "properties": {
                    "objective": {"type": "string", "description": t("The concrete completion objective inferred from the direct human request.", "从人类请求中提炼的具体完成目标。")},
                    "max_goal_rounds": {"type": "integer", "description": t("Optional positive limit on automatic continuation rounds.", "可选:自动续轮上限(正整数)。")}
                },
                "required": ["objective"],
                "additionalProperties": false
            }),
            move |args: Value| {
                let paths = create_paths.clone();
                async move {
                    let session = session_for_call()?;
                    require_human(&workspace::current_turn_origin(), "create")?;
                    let objective = args
                        .get("objective")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let max_rounds = meaningful_rounds(
                        args.get("max_goal_rounds").and_then(Value::as_i64),
                    );
                    let goal = store(&paths)?.create_goal(&session, objective, max_rounds)?;
                    set_armed(&session, true);
                    Ok(render_goal(Some(&goal), &session))
                }
            },
        )
        .writes()
        .with_always_loaded(false),
    );

    let update_paths = paths;
    registry.register(
        ToolSpec::new(
            "update_goal",
            t(
                "Update the exact current goal revision (call get_goal first and copy its goal_id/revision). Actions: edit | pause | resume | complete | blocked. edit/pause/resume require a direct human turn; during the current autonomous goal round, complete and blocked are also allowed. blocked needs blocked_reason and is mechanically rejected before the configured minimum consecutive rounds.",
                "更新当前目标的精确 revision(先 get_goal 并照抄 goal_id/revision)。action:edit|pause|resume|complete|blocked。edit/pause/resume 需人类发起轮;自主 goal round 内额外允许 complete 与 blocked。blocked 必须给 blocked_reason,且未达连续轮数下限会被机械拒绝。",
            ),
            json!({
                "type": "object",
                "properties": {
                    "goal_id": {"type": "string", "description": t("Exact id returned by get_goal.", "get_goal 返回的精确 id。")},
                    "revision": {"type": "integer", "description": t("Exact positive revision returned by get_goal.", "get_goal 返回的精确 revision。")},
                    "action": {"type": "string", "enum": ["edit", "pause", "resume", "complete", "blocked"]},
                    "objective": {"type": "string", "description": t("Replacement objective; only with action edit.", "替换目标文案;仅 action=edit 有效。")},
                    "max_goal_rounds": {"type": "integer", "description": t("Replacement round cap; only with action edit.", "替换轮数上限;仅 action=edit 有效。")},
                    "blocked_reason": {"type": "string", "description": t("Concrete blocking condition; required with action blocked.", "具体阻塞条件;action=blocked 时必填。")}
                },
                "required": ["goal_id", "revision", "action"],
                "additionalProperties": false
            }),
            move |args: Value| {
                let paths = update_paths.clone();
                async move { update_goal(&paths, args).await }
            },
        )
        .writes()
        .with_always_loaded(false),
    );
}

async fn update_goal(paths: &MiyuPaths, args: Value) -> Result<String> {
    let session = session_for_call()?;
    let origin = workspace::current_turn_origin();
    let store = store(paths)?;
    let goal_id = args
        .get("goal_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let revision = args.get("revision").and_then(Value::as_i64).unwrap_or(0);
    if goal_id.is_empty() || revision < 1 {
        bail!("goal_id must be non-empty and revision must be a positive integer (call get_goal)");
    }
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let objective = meaningful_text(args.get("objective").and_then(Value::as_str));
    let max_rounds = meaningful_rounds(args.get("max_goal_rounds").and_then(Value::as_i64));
    let blocked_reason = meaningful_text(args.get("blocked_reason").and_then(Value::as_str));

    match action {
        "edit" => {
            require_human(&origin, "edit")?;
            if blocked_reason.is_some() {
                bail!("blocked_reason is valid only with action blocked");
            }
            let goal = store.edit_goal(&session, &goal_id, revision, objective, max_rounds)?;
            Ok(render_goal(Some(&goal), &session))
        }
        "pause" | "resume" => {
            require_human(&origin, action)?;
            if objective.is_some() || max_rounds.is_some() || blocked_reason.is_some() {
                bail!("objective/max_goal_rounds are valid only with edit; blocked_reason only with blocked");
            }
            let goal = if action == "pause" {
                let goal = store.pause_goal(&session, &goal_id, revision)?;
                set_armed(&session, false);
                goal
            } else {
                let goal = store.resume_goal(&session, &goal_id, revision)?;
                set_armed(&session, true);
                goal
            };
            Ok(render_goal(Some(&goal), &session))
        }
        "complete" | "blocked" => {
            // 权限:人类直改,或恰好当前自主轮。
            let current = store
                .goal(&session)?
                .ok_or_else(|| anyhow::anyhow!("{}", GoalDenied::NotFound))?;
            let autonomous = origin_matches_goal(&origin, &current);
            if !autonomous {
                require_human(&origin, action)?;
            }
            if objective.is_some() || max_rounds.is_some() {
                bail!("objective/max_goal_rounds are valid only with action edit");
            }
            if action == "complete" {
                if blocked_reason.is_some() {
                    bail!("blocked_reason is valid only with action blocked");
                }
                let goal = store.complete_goal(&session, &goal_id, revision)?;
                set_armed(&session, false);
                if autonomous {
                    pending_wrapups()
                        .lock()
                        .unwrap()
                        .insert(session.clone(), wrapup_text(&goal.objective, None));
                }
                Ok(render_goal(Some(&goal), &session))
            } else {
                let Some(reason) = blocked_reason else {
                    bail!("blocked_reason is required with action blocked");
                };
                // 机械下限只约束自主轮:人类可立即停(dsh 同款)。
                if autonomous && current.rounds_started < BLOCKED_AFTER_CONSECUTIVE_ROUNDS {
                    bail!(
                        "blocked requires at least {BLOCKED_AFTER_CONSECUTIVE_ROUNDS} consecutive goal rounds; current round is {}",
                        current.rounds_started
                    );
                }
                let goal = store.block_goal(&session, &goal_id, revision, "model-reported", reason)?;
                set_armed(&session, false);
                if autonomous {
                    pending_wrapups()
                        .lock()
                        .unwrap()
                        .insert(session.clone(), wrapup_text(&goal.objective, Some(reason)));
                }
                Ok(render_goal(Some(&goal), &session))
            }
        }
        other => bail!("unknown goal action: {other}"),
    }
}

/// dsh goal-round-driver prompt.ts 同款续轮提示词:引用目标与轮号,
/// 要求以工作区/工具结果/持久状态为准、取证后才 complete、未完则保持
/// active。保留英文原文——dev 提示词就是英文,措辞贴训练分布。
pub fn goal_round_prompt(objective: &str, round: i64, max_rounds: i64) -> String {
    format!(
        "<goal_round>\nObjective: {}\nRound: {round}/{max_rounds}\n\nContinue working toward          the objective in this same session. Treat the current workspace, tool results, and          durable session state as authoritative; inspect them instead of assuming earlier          narration is still current. Make concrete progress and verify the result. Before          claiming completion, gather evidence that the whole objective is achieved, read the          current goal, and mark it complete. If work remains, leave the goal active for the          next round. Follow the configured goal-tool policy before reporting a blocker.\n         </goal_round>",
        serde_json::json!(objective)
    )
}

// ------------------------------ /goal 人类命令 ------------------------------

/// dsh command-goal 同款语法:`/goal [<objective>|clear|edit <objective>|pause|resume]`。
/// REPL 远程/直连两道共用;返回直接打印的文本,永不进模型历史。
pub fn execute_goal_command(paths: &MiyuPaths, session_id: &str, raw: &str) -> String {
    match execute_goal_command_inner(paths, session_id, raw) {
        Ok(text) => text,
        Err(error) => match error.downcast_ref::<GoalDenied>() {
            Some(denied) => format!("goal: {denied}"),
            None => format!("goal error: {error:#}"),
        },
    }
}

const GOAL_USAGE: &str = "用法: /goal [<objective>|clear|edit <objective>|pause|resume]";

fn render_goal_human(title: &str, goal: &GoalRecord, session_id: &str) -> String {
    let mut lines = vec![
        title.to_string(),
        format!("状态: {}", goal.phase.as_str()),
    ];
    if let (Some(code), Some(message)) = (&goal.blocked_code, &goal.blocked_message) {
        lines.push(format!("阻塞: {code}: {message}"));
    }
    lines.push(format!("目标: {}", goal.objective));
    lines.push(format!("轮次: {}/{}", goal.rounds_started, goal.max_rounds));
    lines.push(format!(
        "激活: {}",
        if is_armed(session_id) { "armed(自动续跑)" } else { "disarmed(等待 resume)" }
    ));
    // 验收反馈:创建后"接下来会发生什么"不说清,用户会以为没反应。
    if goal.phase == GoalPhase::Active && is_armed(session_id) {
        lines.push(
            "Miyu 会在空闲时自动开轮推进;你随时插话,人类消息优先,目标在后台继续。".to_string(),
        );
    }
    let hint = match goal.phase {
        GoalPhase::Active if is_armed(session_id) => "/goal edit <objective>、/goal pause、/goal clear",
        GoalPhase::Active | GoalPhase::Paused | GoalPhase::Blocked => {
            "/goal edit <objective>、/goal resume、/goal clear"
        }
        GoalPhase::Complete => "/goal <objective>、/goal clear",
    };
    lines.push(String::new());
    lines.push(format!("可用: {hint}"));
    lines.join("\n")
}

fn execute_goal_command_inner(paths: &MiyuPaths, session_id: &str, raw: &str) -> Result<String> {
    let store = store(paths)?;
    let input = raw.trim();
    let current = store.goal(session_id)?;
    let lower = input.to_ascii_lowercase();
    match lower.as_str() {
        "" => Ok(match &current {
            None => format!("当前没有目标。\n{GOAL_USAGE}"),
            Some(goal) => render_goal_human("当前目标", goal, session_id),
        }),
        "clear" => match current {
            None => Ok("没有可清除的目标。".to_string()),
            Some(goal) => {
                store.clear_goal(session_id, &goal.goal_id, goal.revision)?;
                set_armed(session_id, false);
                Ok("目标已清除。".to_string())
            }
        },
        "pause" => match current {
            None => Ok(format!("当前没有目标,无法 pause。{GOAL_USAGE}")),
            Some(goal) => {
                let goal = store.pause_goal(session_id, &goal.goal_id, goal.revision)?;
                set_armed(session_id, false);
                Ok(render_goal_human("目标已暂停", &goal, session_id))
            }
        },
        "resume" => match current {
            None => Ok(format!("当前没有目标,无法 resume。{GOAL_USAGE}")),
            Some(goal) => {
                let goal = store.resume_goal(session_id, &goal.goal_id, goal.revision)?;
                set_armed(session_id, true);
                Ok(render_goal_human("目标已恢复", &goal, session_id))
            }
        },
        "edit" => Ok(format!("edit 需要新的目标文案。\n{GOAL_USAGE}")),
        _ => {
            if let Some(rest) = input.strip_prefix("edit ").or_else(|| input.strip_prefix("Edit ")) {
                let objective = rest.trim();
                match current {
                    None => Ok(format!("当前没有目标,无法 edit。{GOAL_USAGE}")),
                    Some(goal) if goal.phase == GoalPhase::Complete => {
                        let goal = store.create_goal(session_id, objective, None)?;
                        set_armed(session_id, true);
                        Ok(render_goal_human("目标已创建", &goal, session_id))
                    }
                    Some(goal) => {
                        let goal = store.edit_goal(
                            session_id,
                            &goal.goal_id,
                            goal.revision,
                            Some(objective),
                            None,
                        )?;
                        Ok(render_goal_human("目标已更新", &goal, session_id))
                    }
                }
            } else {
                // 其余任意文本 = 创建目标(complete 可替换)。
                match &current {
                    Some(goal) if goal.phase != GoalPhase::Complete => Ok(format!(
                        "已有一个 {} 状态的目标。用 /goal edit <objective> 修改,或先 /goal clear。",
                        goal.phase.as_str()
                    )),
                    _ => {
                        let goal = store.create_goal(session_id, input, None)?;
                        set_armed(session_id, true);
                        Ok(render_goal_human("目标已创建", &goal, session_id))
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_paths(root: &std::path::Path) -> MiyuPaths {
        MiyuPaths {
            root_dir: root.to_path_buf(),
            config_dir: root.join("config"),
            config_file: root.join("config/config.jsonc"),
            skills_dir: root.join("config/skills"),
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            state_dir: root.join("state"),
            pictures_dir: root.join("pictures"),
            fish_hook_file: root.join("config/fish/conf.d/miyu.fish"),
            bash_hook_file: root.join("config/shell/bash-hook.sh"),
            zsh_hook_file: root.join("config/shell/zsh-hook.zsh"),
            scripts_dir: root.join("config/scripts"),
            system_scripts_dir: root.join("system-scripts"),
        }
    }

    fn setup() -> (tempfile::TempDir, MiyuPaths, String) {
        let temp = tempfile::tempdir().unwrap();
        let paths = test_paths(temp.path());
        let state = StateStore::new(&paths).unwrap();
        state.init_files().unwrap();
        let record = state
            .create_session("miyu", "goal", crate::state::USER_SESSION_KIND, None)
            .unwrap();
        (temp, paths, record.session_id)
    }

    #[test]
    fn goal_command_grammar_covers_the_five_branches() {
        let (_t, paths, session) = setup();
        assert!(execute_goal_command(&paths, &session, "").contains("当前没有目标"));
        let created = execute_goal_command(&paths, &session, "修好构建并让测试全绿");
        assert!(created.contains("目标已创建") && created.contains("armed"));
        assert!(execute_goal_command(&paths, &session, "第二个目标").contains("已有一个"));
        assert!(execute_goal_command(&paths, &session, "edit 只修构建")
            .contains("目标已更新"));
        let paused = execute_goal_command(&paths, &session, "pause");
        assert!(paused.contains("目标已暂停") && paused.contains("disarmed"));
        let resumed = execute_goal_command(&paths, &session, "resume");
        assert!(resumed.contains("目标已恢复") && resumed.contains("armed"));
        assert!(execute_goal_command(&paths, &session, "clear").contains("已清除"));
        assert!(!is_armed(&session));
    }

    #[tokio::test]
    async fn goal_tools_enforce_origin_authority_and_wrapup() {
        let (_t, paths, session) = setup();
        let mut registry = ToolRegistry::new();
        register(&mut registry, paths.clone());
        let session_arc: std::sync::Arc<str> = session.clone().into();

        // 人类轮:create ✓,拿到 armed 快照。
        let created = workspace::with_session(session_arc.clone(), async {
            registry
                .call("create_goal", r#"{"objective":"跑通端到端"}"#)
                .await
        })
        .await
        .unwrap();
        assert!(created.contains("\"armed\""));
        let value: Value = serde_json::from_str(&created).unwrap();
        let goal_id = value["goal"]["id"].as_str().unwrap().to_string();
        let revision = value["goal"]["revision"].as_i64().unwrap();

        // 自动轮(goal round 来源):create 被拒。
        let denied = workspace::with_session(session_arc.clone(), async {
            workspace::with_turn_origin(
                TurnOrigin::GoalRound {
                    goal_id: goal_id.clone(),
                    revision,
                    round: 0,
                },
                async {
                    registry
                        .call("create_goal", r#"{"objective":"另一个"}"#)
                        .await
                },
            )
            .await
        })
        .await
        .unwrap_err();
        assert!(denied.to_string().contains("direct human turn"));

        // 认领三轮后,自主轮报 blocked:轮号匹配 + 达阈值 → 收,且挂出 wrapup。
        let store = StateStore::new(&paths).unwrap();
        for round in 0..3 {
            store
                .begin_goal_round(&session, &goal_id, revision, round)
                .unwrap();
        }
        let args = format!(
            r#"{{"goal_id":"{goal_id}","revision":{revision},"action":"blocked","blocked_reason":"上游镜像挂了"}}"#
        );
        let blocked = workspace::with_session(session_arc.clone(), async {
            workspace::with_turn_origin(
                TurnOrigin::GoalRound {
                    goal_id: goal_id.clone(),
                    revision,
                    round: 3,
                },
                async { registry.call("update_goal", &args).await },
            )
            .await
        })
        .await
        .unwrap();
        assert!(blocked.contains("\"blocked\""));
        assert!(!is_armed(&session));
        let wrapup = take_pending_wrapup(&session).unwrap();
        assert!(wrapup.contains("<goal_blocked>") && wrapup.contains("上游镜像挂了"));
        assert!(take_pending_wrapup(&session).is_none(), "取走即清");
    }
}
