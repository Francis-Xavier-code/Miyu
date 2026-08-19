//! 目标续轮驱动器：会话闲下来之后，自动开下一轮把长任务往前推。
//!
//! 它是整个 goal 功能里唯一会**自己发起回合**的地方，所以每一道闸都是在回答
//! 同一个问题：「现在开这一轮，会不会踩到别人？」
//!
//! 1. 会话没有在飞的 run —— 只在空闲时驱动，不和正在跑的回合抢。
//! 2. 没有排队的人类输入 —— 人优先。自动轮消耗轮号，不该插在人前面。
//! 3. 目标是 active、本进程 armed、还有轮数余量 —— 余量耗尽就转 blocked。
//! 4. `begin_goal_round` 双 CAS 认领 —— 并发唤醒里只有一个能拿到这一轮。
//!
//! 平台绑定的会话 v1 不驱动：回复要送达 QQ 得合成一个平台轮，那是另一套
//! 上下文装配，和这里的会话轮不是一回事。

use crate::web::*;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

/// 我们自己起的那些续轮：run_id → session_id。
fn goal_runs() -> &'static Mutex<HashMap<String, String>> {
    static RUNS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 至少调过一次工具的 run。
fn productive_runs() -> &'static Mutex<HashSet<String>> {
    static RUNS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 上一轮什么都没干、只说了句话的会话——在人开口之前不再驱动。
fn awaiting_human() -> &'static Mutex<HashSet<String>> {
    static SESSIONS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn event_field(record: &EventRecord, field: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(&record.data)
        .ok()
        .and_then(|data| {
            data.get(field)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
}

/// 订阅事件流，在每次回合结束时看看要不要接着开一轮。
pub(in crate::web) fn spawn_goal_round_driver(state: DaemonState) {
    let mut receiver = state.events.subscribe_live();
    tokio::spawn(async move {
        loop {
            let record = match receiver.recv().await {
                Ok(record) => record,
                // 落后了就跳过：漏掉的那些回合结束事件不要紧，下一次结束
                // 还会再来，而目标本身在库里躺着不会丢。
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            let session_id = || event_field(&record, "session_id");
            match record.kind.as_str() {
                // 这一轮调过工具 = 真的在干活。判据取事件而不是回查库：
                // 驱动器本来就在订阅事件流，查库要把整个会话的回合读出来。
                "tool.started" => {
                    if let Some(run_id) = event_field(&record, "run_id") {
                        productive_runs().lock().unwrap().insert(run_id);
                    }
                }
                "run.completed" => {
                    let Some(session) = session_id() else {
                        continue;
                    };
                    let run_id = event_field(&record, "run_id").unwrap_or_default();
                    let was_goal_round = goal_runs().lock().unwrap().remove(&run_id).is_some();
                    let did_work = productive_runs().lock().unwrap().remove(&run_id);
                    if was_goal_round && !did_work {
                        // 一轮下来一个工具都没调，就是在原地说话——最常见的是
                        // 「我在等你确认」。再开下去只会把同一句话重复几十遍，
                        // 每遍都收一次钱。等人开口再说。
                        tracing::info!(
                            %session,
                            "goal round made no tool calls; pausing until the user speaks"
                        );
                        awaiting_human().lock().unwrap().insert(session.clone());
                        continue;
                    }
                    if !was_goal_round {
                        // 人（或别的来源）跑完了一轮，等待解除。
                        awaiting_human().lock().unwrap().remove(&session);
                    }
                    let state = state.clone();
                    tokio::spawn(async move {
                        maybe_continue_goal(state, session).await;
                    });
                }
                "run.failed" => {
                    if let Some(run_id) = event_field(&record, "run_id") {
                        goal_runs().lock().unwrap().remove(&run_id);
                        productive_runs().lock().unwrap().remove(&run_id);
                    }
                    // 失败之后解除武装：一个会持续失败的目标如果继续自动重试，
                    // 就是拿着用户的额度撞墙。要接着跑，人得亲自 `/goal resume`。
                    if let Some(session) = session_id() {
                        if crate::tools::goal::is_armed(&session) {
                            tracing::info!(
                                %session,
                                "goal disarmed after a failed run; resume to continue"
                            );
                            crate::tools::goal::set_armed(&session, false);
                        }
                    }
                }
                _ => {}
            }
        }
    });
}

/// 人刚动过目标，踢驱动器一脚。
///
/// `/goal resume` 之后应当立刻接着跑，而不是干等下一个回合结束事件——那个
/// 事件可能永远不来（会话正闲着）。重复踢是安全的：四道闸都在
/// `maybe_continue_goal` 里，多踢几次最多是多查几次库。
pub(in crate::web) fn nudge_goal_driver(state: &DaemonState, session_id: &str) {
    // 人动过目标就是开了口：上一轮空转设下的等待到此为止。
    awaiting_human().lock().unwrap().remove(session_id);
    if !crate::tools::goal::is_armed(session_id) {
        return;
    }
    let state = state.clone();
    let session_id = session_id.to_string();
    tokio::spawn(async move {
        maybe_continue_goal(state, session_id).await;
    });
}

/// 四道闸之后认领下一轮，起一个自动回合。
pub(in crate::web) async fn maybe_continue_goal(state: DaemonState, session_id: String) {
    // 闸 0：上一轮空转过就等人。
    if awaiting_human().lock().unwrap().contains(&session_id) {
        return;
    }
    // 闸 1：会话空闲。
    {
        let manager = state.manager.lock().unwrap();
        if manager.admin_busy
            || manager
                .active_runs
                .values()
                .any(|run| &*run.session_id == session_id.as_str())
        {
            return;
        }
    }
    // 闸 2：人在排队就让行。自动轮会消耗轮号，插在人前面既抢了额度也抢了顺序。
    let store = state.state_store.pinned(&session_id);
    match store.load_queued_prompts() {
        Ok(queued) if queued.is_empty() => {}
        _ => return,
    }
    let Ok(Some(record)) = state.state_store.session_record(&session_id) else {
        return;
    };
    // 平台绑定会话 v1 不驱动（回复送达要合成平台轮，与这里的会话轮不同构）。
    if let Ok(bindings) = state
        .state_store
        .platform_session_bindings(&record.persona, "onebot")
    {
        if bindings
            .iter()
            .any(|binding| binding.session_id == session_id)
        {
            tracing::debug!(%session_id, "goal driver skips platform-bound session (v1)");
            return;
        }
    }
    // 闸 3：目标可跑。
    let Ok(Some(goal)) = state.state_store.goal(&session_id) else {
        return;
    };
    if goal.phase != crate::state::GoalPhase::Active || !crate::tools::goal::is_armed(&session_id) {
        return;
    }
    if goal.rounds_started >= goal.max_rounds {
        // 轮数耗尽转 blocked 而不是静默停下：人得知道它为什么不动了，
        // 以及该怎么继续（/goal edit 抬高上限）。
        let _ = state.state_store.block_goal(
            &session_id,
            &goal.goal_id,
            goal.revision,
            "round-limit",
            &format!(
                "goal reached its configured limit of {} rounds",
                goal.max_rounds
            ),
        );
        crate::tools::goal::set_armed(&session_id, false);
        return;
    }
    // 闸 4：双 CAS 认领。并发唤醒里只有一个能赢，输的那个安静退出。
    let claimed = match state.state_store.begin_goal_round(
        &session_id,
        &goal.goal_id,
        goal.revision,
        goal.rounds_started,
    ) {
        Ok(claimed) => claimed,
        Err(error) => {
            tracing::debug!(%session_id, error = %error, "goal round claim lost; not continuing");
            return;
        }
    };

    let content = crate::tools::goal::goal_round_prompt(
        &claimed.objective,
        claimed.rounds_started,
        claimed.max_rounds,
    );
    let display_content = format!(
        "⟳ goal round {}/{} · {}",
        claimed.rounds_started, claimed.max_rounds, claimed.objective
    );
    let run_id = random_id("run", 18);
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    // 带上认领时的身份：工具层据此判定「本轮恰好是当前目标的这一轮」，
    // 只有这样才允许模型自己报完成/受阻。
    let origin = crate::tools::workspace::TurnOrigin::GoalRound {
        goal_id: claimed.goal_id.clone(),
        revision: claimed.revision,
        round: claimed.rounds_started,
    };
    {
        let mut manager = state.manager.lock().unwrap();
        // 认领和登记之间可能有人抢了管理锁，这里再看一眼。
        if manager.admin_busy {
            return;
        }
        manager.active_runs.insert(
            run_id.clone(),
            RunInfo {
                session_id: session_id.clone().into(),
                mode: AgentMode::Normal,
                audience: PromptAudience::Owner,
                cancel: cancel_tx,
                turn_id: None,
                queue_target: None,
                supersede: Arc::new(crate::agent::TurnSupersedeSignal::default()),
                platform_followup: None,
                operation: RunOperation::Create,
                // 复用 job_wake 这条可见性通道：REPL 客户端靠它发现并挂上
                // daemon 自己发起的回合，否则自动轮在终端里是完全看不见的。
                job_wake: true,
                turn_origin: origin.clone(),
                job_wake_label: Some(format!(
                    "goal round {}/{}",
                    claimed.rounds_started, claimed.max_rounds
                )),
            },
        );
    }
    goal_runs()
        .lock()
        .unwrap()
        .insert(run_id.clone(), session_id.clone());
    tracing::info!(
        %session_id,
        round = claimed.rounds_started,
        max = claimed.max_rounds,
        "goal driver starting an autonomous round"
    );
    if state
        .actor_tx
        .send(ActorCommand::StartTurn {
            run_id: run_id.clone(),
            session_id: session_id.clone().into(),
            content,
            display_content,
            attachment_run_id: None,
            mode: AgentMode::Normal,
            images: Vec::new(),
            cwd: None,
            origin_tty: None,
            audience: PromptAudience::Owner,
            profile: None,
            cancel: cancel_rx,
            turn_origin: Box::new(origin),
        })
        .is_err()
    {
        finish_run(&state.manager, &run_id, None);
    }
}
