//! 后台任务唤醒的事件泵。
//!
//! 子代理跑完、闹钟到点这类事件会「唤醒」一个回合，daemon 把它的事件流推给
//! 终端。它和普通回合走的是两条独立的泵，各自维护分发表——08-17 那次「回合
//! 中途 footer 不刷新」就是因为只有一个泵接了 `chat.round_usage`，另一个漏了。

use crate::cli::repl::editor::*;
use crate::cli::repl::tail::*;
use crate::cli::*;

/// Attach to a daemon-initiated wake turn and render it live: streaming
/// content, reasoning, and tool activity, exactly like a user-started turn.
/// ESC detaches (the turn keeps running; the DB report is suppressed because
/// the turn was already rendered here). Typed submissions queue into the
/// wake turn as follow-ups.
pub(in crate::cli) async fn follow_wake_run(
    paths: &MiyuPaths,
    live: &mut LiveReplTail,
    run_id: &str,
    label: &str,
    jobs_feed: &JobsFeed,
    jobs_shared: &std::sync::Arc<SharedJobsFeed>,
) -> Result<()> {
    let config = AppConfig::load_or_default(paths)?;
    let mut stream = ipc::connect(&paths.ipc_socket()).await?;
    ipc::send(
        &mut stream,
        &IpcRequest::new(IpcCommand::FollowRun {
            run_id: run_id.to_string(),
        }),
    )
    .await?;
    let mut turn_id: Option<String> = match ipc::receive::<IpcFrame>(&mut stream).await? {
        Some(IpcFrame::Accepted { turn_id, .. }) => turn_id,
        // Run already finished — the DB report path will print it instead.
        _ => return Ok(()),
    };

    let mut renderer = render::StreamRenderer::new(
        render::ReasoningDisplayMode::from_config(&config.display.reasoning),
        render::ToolCallDisplayMode::from_config(&config.display.tool_calls),
        false,
        config.display.readable_tool_names,
        config.display.command_output_lines,
    );
    renderer.use_external_cursor_control();
    renderer.use_buffered_output();
    live.external_output_active = false;
    // Print the header straight into the scrollback (not the live frame):
    // it must survive the streaming render that follows.
    {
        live.suspend()?;
        let mut stdout = io::stdout();
        let header = if label.is_empty() {
            crate::i18n::text("⚙ background task finished", "⚙ 后台任务完成").to_string()
        } else {
            format!("⚙ {label}")
        };
        queue!(stdout, Print(format!("\x1b[2m{header}\x1b[0m\r\n\r\n")))?;
        stdout.flush()?;
        live.output_cursor = cursor_position_or(live.output_cursor);
        let output_cursor = live.output_cursor;
        live.resume_at(output_cursor)?;
    }
    renderer.start_waiting()?;
    live.apply_renderer_frame(&mut renderer)?;
    let mut raw = LiveRawMode::start()?;

    let mut spinner_tick = tokio::time::interval(Duration::from_millis(33));
    spinner_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    spinner_tick.tick().await;
    let mut input_tick = tokio::time::interval(Duration::from_millis(16));
    input_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    input_tick.tick().await;
    let mut follow_strip_tick: u32 = 0;

    'outer: loop {
        let recv = ipc::receive::<IpcFrame>(&mut stream);
        tokio::pin!(recv);
        let frame = loop {
            tokio::select! {
                biased;
                _ = input_tick.tick() => {
                    if terminal_hangup() {
                        let _ = send_ipc_command(paths, IpcCommand::Cancel { run_id: run_id.to_string() }).await;
                        std::process::exit(0);
                    }
                    if !event::poll(Duration::ZERO)? {
                        continue;
                    }
                    match live.editor.handle_event(event::read()?, paths, true)? {
                        LiveEditorAction::None => {}
                        LiveEditorAction::Redraw if !live.external_output_active => {
                            synchronized_terminal_update(CursorAfterUpdate::Preserve, || {
                                live.redraw()
                            })?
                        }
                        LiveEditorAction::Redraw | LiveEditorAction::ClearScreen => {}
                        LiveEditorAction::EmptySubmit => {}
                        LiveEditorAction::Submit(submission) => {
                            let Some(target_turn) = turn_id.as_deref() else {
                                continue;
                            };
                            if let Ok(prompt) = persist_remote_queued_submission(
                                paths,
                                run_id,
                                target_turn,
                                &submission,
                            )
                            .await
                            {
                                live.editor.record_history(&submission.content);
                                synchronized_terminal_update(
                                    CursorAfterUpdate::Preserve,
                                    || live.enqueue(prompt),
                                )?;
                            }
                        }
                        LiveEditorAction::Interrupt | LiveEditorAction::Exit => {
                            // Detach only: the wake turn keeps running.
                            break 'outer;
                        }
                    }
                }
                frame = &mut recv => break frame?,
                _ = spinner_tick.tick() => {
                    // SpinnerTick 经 live 路径冲刷 chunk 缓冲，流式输出靠它。
                    handle_live_agent_event(live, &mut renderer, AgentEvent::SpinnerTick)?;
                    // 状态条是 live tail 的一部分，附着期间同样要持续刷新。
                    follow_strip_tick = follow_strip_tick.wrapping_add(1);
                    if follow_strip_tick % 8 == 0 && !live.external_output_active {
                        if live.set_jobs(jobs_feed.current()) {
                            synchronized_terminal_update(CursorAfterUpdate::Preserve, || {
                                live.redraw()
                            })?;
                        } else {
                            live.tick_job_strip()?;
                        }
                    }
                }
            }
        };
        let Some(IpcFrame::Event { kind, data, .. }) = frame else {
            break;
        };
        match kind.as_str() {
            "turn.started" => {
                turn_id = Some(ipc_text(&data, "turn_id").to_string());
            }
            "assistant.delta" => handle_live_agent_event(
                live,
                &mut renderer,
                AgentEvent::Chunk(ChatStreamChunk {
                    kind: crate::llm::ChatStreamKind::Content,
                    text: ipc_text(&data, "delta").to_string(),
                }),
            )?,
            "reasoning.delta" => handle_live_agent_event(
                live,
                &mut renderer,
                AgentEvent::Chunk(ChatStreamChunk {
                    kind: crate::llm::ChatStreamKind::Reasoning,
                    text: ipc_text(&data, "delta").to_string(),
                }),
            )?,
            "reasoning.start" => handle_live_agent_event(
                live,
                &mut renderer,
                AgentEvent::ReasoningStart {
                    received_at: Instant::now(),
                },
            )?,
            "reasoning.reset" => handle_live_agent_event(
                live,
                &mut renderer,
                AgentEvent::ReasoningReset {
                    received_at: Instant::now(),
                },
            )?,
            "reasoning.part_start" => handle_live_agent_event(
                live,
                &mut renderer,
                AgentEvent::ReasoningPartStart {
                    received_at: Instant::now(),
                },
            )?,
            "reasoning.part_end" => handle_live_agent_event(
                live,
                &mut renderer,
                AgentEvent::ReasoningPartEnd {
                    received_at: Instant::now(),
                },
            )?,
            "reasoning.title" => handle_live_agent_event(
                live,
                &mut renderer,
                AgentEvent::ReasoningTitle(ipc_text(&data, "title").to_string()),
            )?,
            "tool.preparing" => handle_live_agent_event(
                live,
                &mut renderer,
                AgentEvent::ToolPreparing {
                    name: ipc_text(&data, "name").to_string(),
                    batch: data
                        .get("batch")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                },
            )?,
            "tool.started" => handle_live_agent_event(
                live,
                &mut renderer,
                AgentEvent::ToolCall {
                    call_id: ipc_text(&data, "tool_id").to_string(),
                    name: ipc_text(&data, "name").to_string(),
                    arguments: ipc_text(&data, "arguments").to_string(),
                },
            )?,
            "tool.progress" => handle_live_agent_event(
                live,
                &mut renderer,
                AgentEvent::ToolProgress {
                    call_id: ipc_text(&data, "tool_id").to_string(),
                    name: ipc_text(&data, "name").to_string(),
                    message: ipc_text(&data, "message").to_string(),
                },
            )?,
            "tool.output" => handle_live_agent_event(
                live,
                &mut renderer,
                AgentEvent::CommandOutput {
                    call_id: ipc_text(&data, "tool_id").to_string(),
                    name: ipc_text(&data, "name").to_string(),
                    stream: if ipc_text(&data, "stream") == "stderr" {
                        tools::CommandOutputStream::Stderr
                    } else {
                        tools::CommandOutputStream::Stdout
                    },
                    chunk: ipc_text(&data, "output").as_bytes().to_vec(),
                },
            )?,
            "tool.finished" => handle_live_agent_event(
                live,
                &mut renderer,
                AgentEvent::ToolResult {
                    call_id: ipc_text(&data, "tool_id").to_string(),
                    name: ipc_text(&data, "name").to_string(),
                    ok: data
                        .get("ok")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                    output: ipc_text(&data, "output").to_string(),
                },
            )?,
            "queue.consumed" => {
                let prompt_ids: Vec<String> = data
                    .get("prompt_ids")
                    .and_then(serde_json::Value::as_array)
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(|value| value.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let consumed_mode = match ipc_text(&data, "mode") {
                    "dev" => AgentMode::Dev,
                    _ => AgentMode::Normal,
                };
                renderer.prepare_for_external_output()?;
                live.apply_renderer_frame(&mut renderer)?;
                synchronized_terminal_update(CursorAfterUpdate::Preserve, || {
                    live.suspend()?;
                    live.consume_queued(&prompt_ids, consumed_mode)
                })?;
            }
            // daemon 一直在发这个事件,可这里没有对应分支,于是逐请求的
            // 计量在 IPC 这一段就掉地上了——WebUI 有(它自己解 SSE),终端
            // 直连模式也有(走本地事件),唯独日常的「终端连 daemon」要等整
            // 个回合结束才动。
            "chat.round_usage" => {
                let usage = data.get("usage").cloned().unwrap_or_default();
                // prompt+completion 即该请求结束时的上下文实际占用,与
                // 本地事件那条路取同一个口径。
                let context_tokens = ipc_u64(&usage, "prompt_tokens")
                    .saturating_add(ipc_u64(&usage, "completion_tokens"));
                live.refresh_round_usage(
                    context_tokens,
                    TurnTokens {
                        total: ipc_u64(&data, "turn_total"),
                        prompt: ipc_u64(&data, "turn_prompt"),
                        cache_read: ipc_u64(&data, "turn_cache_read"),
                    },
                )?;
            }
            "generation.superseded" => handle_live_agent_event(
                live,
                &mut renderer,
                AgentEvent::ReasoningReset {
                    received_at: Instant::now(),
                },
            )?,
            "run.completed" | "run.failed" | "run.cancelled" => {
                break;
            }
            _ => {}
        }
    }

    // Flush chunks still buffered when the terminal frame arrived — the
    // final content burst lands right before run.completed.
    live.flush_pending_chunks(&mut renderer)?;
    renderer.finish()?;
    live.apply_renderer_frame(&mut renderer)?;
    raw.handoff();
    live.raw_mode_handoff = true;
    // Suppress the duplicate DB report for a turn that was rendered live.
    if let Some(turn_id) = turn_id {
        let mut rendered = jobs_shared.rendered_turns.lock().unwrap();
        if rendered.len() >= JOBS_FEED_MARK_LIMIT {
            rendered.clear();
        }
        rendered.insert(turn_id);
    }
    Ok(())
}
