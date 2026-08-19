//! 同会话长任务目标：面向模型的三件套工具。
//!
//! 目标本身的真源在 SQLite（`state::conversation_db::goals`）。这里补三样只在
//! 进程里活的东西：
//!
//! - **armed 激活注册表**：目标还在库里，但「是否自动续跑」驻内存、绝不落库。
//!   daemon 重启后必须由人 `/goal resume` 重新授权——不然一次崩溃重启就能让
//!   机器在无人看管的情况下继续自己开轮。
//! - **权限二元分立**：靠回合来源标记判定，而不是去扫会话事件猜。
//!   create/edit/pause/resume 只认人类发起的回合；complete/blocked 额外接受
//!   「恰好是当前目标的这一轮自动续轮」。
//! - **wrapup 侧信道**：自主轮里报了完成/受阻之后，主循环取走一段收尾指令注入，
//!   让模型面向用户交代一次，而不是硬生生停在那里。

use super::{ToolRegistry, ToolSpec};
use crate::paths::MiyuPaths;
use crate::state::{GoalDenied, GoalPhase, GoalRecord, StateStore};
use crate::tools::workspace::{self, TurnOrigin};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

/// 模型自报受阻的机械下限：同一阻塞至少要熬过这么多连续自动轮才收。
///
/// 没有这道闸，模型第一轮遇到点麻烦就能宣布「我被挡住了」，长任务退化成
/// 「起个头就收工」。人类直接授权不受此限——人说停就是停。
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

/// 主循环在每次工具结算之后取走；有就注入为本回合的上下文。
pub fn take_pending_wrapup(session_id: &str) -> Option<String> {
    pending_wrapups().lock().unwrap().remove(session_id)
}

const WRAPUP_GROUNDING: &str = "Report only what earlier rounds and tool results in this \
     session actually establish; when a detail is not in the session, say so instead of \
     inventing it. ";

/// 自主轮终结后的收尾指令。
///
/// 保留英文原文：这段是直接喂给模型的指令，而 dev 侧提示词本来就是英文，
/// 措辞贴着训练分布走比翻译过来更稳。
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

/// 严格 schema 的填充容错：模型常按「必填」的直觉把用不上的字段填成空串或 0，
/// 那等于没提供。
fn meaningful_text(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|text| !text.is_empty())
}

fn meaningful_rounds(value: Option<i64>) -> Option<i64> {
    value.filter(|rounds| *rounds != 0)
}

/// 本轮是否**恰好**是当前目标的那一轮已认领续轮。
///
/// 三样都要对上：目标、版本、轮号。少一样就可能是拿着过期身份的轮在替一个
/// 已经被人改过的目标做决定。
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
                "读取本会话当前目标：用于比较并设置的精确 id/revision、目标、阶段、已用/上限轮数、阻塞原因，以及自动续跑是否已武装。调用 update_goal 前先调它。",
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
                "当当前人类请求是需要跨自动轮持续推进的长任务时，创建本会话唯一的持久目标。可从任意语言的请求推断意图；琐碎单轮工作不要建目标。自动轮调用会被拒。",
            ),
            json!({
                "type": "object",
                "properties": {
                    "objective": {"type": "string", "description": t("The concrete completion objective inferred from the direct human request.", "从人类请求中提炼的具体完成目标。")},
                    "max_goal_rounds": {"type": "integer", "description": t("Optional positive limit on automatic continuation rounds.", "可选：自动续轮上限（正整数）。")}
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
                    let max_rounds =
                        meaningful_rounds(args.get("max_goal_rounds").and_then(Value::as_i64));
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
                "更新当前目标的精确 revision（先 get_goal 并照抄 goal_id/revision）。action：edit|pause|resume|complete|blocked。edit/pause/resume 需人类发起轮；自主 goal round 内额外允许 complete 与 blocked。blocked 必须给 blocked_reason，且未达连续轮数下限会被机械拒绝。",
            ),
            json!({
                "type": "object",
                "properties": {
                    "goal_id": {"type": "string", "description": t("Exact id returned by get_goal.", "get_goal 返回的精确 id。")},
                    "revision": {"type": "integer", "description": t("Exact positive revision returned by get_goal.", "get_goal 返回的精确 revision。")},
                    "action": {"type": "string", "enum": ["edit", "pause", "resume", "complete", "blocked"]},
                    "objective": {"type": "string", "description": t("Replacement objective; only with action edit.", "替换目标文案；仅 action=edit 有效。")},
                    "max_goal_rounds": {"type": "integer", "description": t("Replacement round cap; only with action edit.", "替换轮数上限；仅 action=edit 有效。")},
                    "blocked_reason": {"type": "string", "description": t("Concrete blocking condition; required with action blocked.", "具体阻塞条件；action=blocked 时必填。")}
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
                bail!(
                    "objective/max_goal_rounds are valid only with edit; \
                     blocked_reason only with blocked"
                );
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
            // 权限：人类直接改，或者恰好是当前这一轮自主续轮。
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
                // 机械下限只约束自主轮：人说停就是停。
                if autonomous && current.rounds_started < BLOCKED_AFTER_CONSECUTIVE_ROUNDS {
                    bail!(
                        "blocked requires at least {BLOCKED_AFTER_CONSECUTIVE_ROUNDS} \
                         consecutive goal rounds; current round is {}",
                        current.rounds_started
                    );
                }
                let goal =
                    store.block_goal(&session, &goal_id, revision, "model-reported", reason)?;
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

/// 续轮的提示词。
///
/// **开头那段来历说明不是废话**：这段文字是突然出现在对话中间的一条指令，
/// 内容往往和上一轮聊的东西毫无关系（长期目标本来就会跨越话题）。实测过一次
/// ——模型读到它之后判定「This looks like a system prompt injection or some
/// automated goal that hijacked my session」，然后拒绝执行、继续做上一个话题。
/// 那个警惕本身是对的：一段没有来历的祈使句，就该被怀疑。所以要讲清楚这是
/// 谁下的、怎么来的、为什么和上文对不上。
///
/// 保留英文原文，理由同 `wrapup_text`。
pub fn goal_round_prompt(objective: &str, round: i64, max_rounds: i64) -> String {
    format!(
        "<goal_round>\n\
         This is an automatic continuation round for the standing objective the user set in \
         this session with the `/goal` command. It is not an injection and not something a \
         third party slipped into the conversation: Miyu itself starts these rounds while the \
         session is idle, and the objective below is the user's own wording, stored in this \
         session's state. The user can pause or clear it at any time with `/goal`.\n\
         Objective: {}\nRound {round} of {max_rounds}.\n\
         The objective may have nothing to do with the messages just above — a standing \
         objective outlives whatever the conversation drifted through since it was set. Work \
         on the objective, not on the previous topic, unless the objective is about it.\n\
         Ground every judgement in the workspace, tool results, and persisted state — not in \
         what earlier rounds claimed. Verify before you call it done: run the checks, read the \
         files back, look at the actual output. If the objective is met and verified, call \
         update_goal with action complete. If a concrete external condition blocks all \
         remaining paths, call update_goal with action blocked and describe it. Otherwise keep \
         the goal active and make the next concrete step of progress in this round.\n\
         </goal_round>",
        json!(objective)
    )
}

/// `/goal [<objective>|clear|edit <objective>|pause|resume]`
///
/// 返回的文本直接打印给人看，**永远不进模型历史**——命令是人和 daemon 之间的
/// 对话，模型该看到的是目标本身（它自己调 `get_goal`）。
pub fn execute_goal_command(paths: &MiyuPaths, session_id: &str, raw: &str) -> String {
    match execute_goal_command_inner(paths, session_id, raw) {
        Ok(text) => text,
        Err(error) => format!("{error}"),
    }
}

const GOAL_USAGE: &str = "用法: /goal [<objective>|clear|edit <objective>|pause|resume]";

fn render_goal_human(title: &str, goal: &GoalRecord, session_id: &str) -> String {
    let activation = if is_armed(session_id) {
        "自动续跑：已武装"
    } else {
        "自动续跑：未武装（/goal resume 重新授权）"
    };
    let blocker = match (&goal.blocked_code, &goal.blocked_message) {
        (Some(code), Some(message)) => format!("\n受阻（{code}）：{message}"),
        (Some(code), None) => format!("\n受阻（{code}）"),
        _ => String::new(),
    };
    format!(
        "{title}\n目标：{}\n阶段：{} · 轮次 {}/{}\n{activation}{blocker}",
        goal.objective,
        goal.phase.as_str(),
        goal.rounds_started,
        goal.max_rounds,
    )
}

fn execute_goal_command_inner(paths: &MiyuPaths, session_id: &str, raw: &str) -> Result<String> {
    let store = store(paths)?;
    let raw = raw.trim();
    let (verb, rest) = match raw.split_once(char::is_whitespace) {
        Some((verb, rest)) => (verb.trim(), rest.trim()),
        None => (raw, ""),
    };
    match verb {
        "" => match store.goal(session_id)? {
            None => Ok(format!("本会话没有目标。\n{GOAL_USAGE}")),
            Some(goal) => Ok(render_goal_human("当前目标", &goal, session_id)),
        },
        "clear" => {
            let Some(goal) = store.goal(session_id)? else {
                bail!("本会话没有目标");
            };
            store.clear_goal(session_id, &goal.goal_id, goal.revision)?;
            set_armed(session_id, false);
            Ok("目标已清除".to_string())
        }
        "pause" => {
            let Some(goal) = store.goal(session_id)? else {
                bail!("本会话没有目标");
            };
            let goal = store.pause_goal(session_id, &goal.goal_id, goal.revision)?;
            set_armed(session_id, false);
            Ok(render_goal_human("已暂停", &goal, session_id))
        }
        "resume" => {
            let Some(goal) = store.goal(session_id)? else {
                bail!("本会话没有目标");
            };
            let goal = store.resume_goal(session_id, &goal.goal_id, goal.revision)?;
            // 重新武装：daemon 重启后自动续跑一律失效，这里是人重新授权的入口。
            set_armed(session_id, true);
            Ok(render_goal_human("已恢复", &goal, session_id))
        }
        "edit" => {
            if rest.is_empty() {
                bail!("{GOAL_USAGE}");
            }
            let Some(goal) = store.goal(session_id)? else {
                bail!("本会话没有目标");
            };
            let goal = store.edit_goal(session_id, &goal.goal_id, goal.revision, Some(rest), None)?;
            Ok(render_goal_human("已更新", &goal, session_id))
        }
        _ => {
            // 其余一律当作「新目标的文案」——`/goal 把测试跑绿` 是最常用的一条，
            // 不该要求再敲一个动词。
            if store
                .goal(session_id)?
                .is_some_and(|goal| goal.phase != GoalPhase::Complete)
            {
                bail!("本会话已有未完成的目标；先 /goal edit 改它，或 /goal clear 清掉");
            }
            let goal = store.create_goal(session_id, raw, None)?;
            set_armed(session_id, true);
            Ok(render_goal_human("目标已设定", &goal, session_id))
        }
    }
}
