//! 流式回合：排队、打断、并发与超越。

use super::shared::*;
use crate::agent::*;
use crate::config::AppConfig;
use crate::tools::{empty_parameters, ToolSpec};
use tokio::net::TcpListener;

#[test]
fn tool_call_stream_announces_preparation_for_slow_argument_tools() {
    let mut filter = ReasoningTitleFilter::default();
    let mut prepared = Vec::new();
    let mut streamed = Vec::new();
    let mut on_event = |event| {
        match event {
            AgentEvent::ToolPreparing { name, .. } => prepared.push(name),
            AgentEvent::Chunk(chunk) if chunk.kind == ChatStreamKind::ToolCall => {
                streamed.push(chunk.text)
            }
            _ => {}
        }
        Ok(())
    };
    let names = [
        "apply_patch",
        "apply_artifact_patch",
        "write_file",
        "edit_string",
        "run_command",
        "task",
        "ask_question",
        // Arguments arrive in one chunk: a hint here would only flicker.
        "read_file",
    ];
    for name in names {
        emit_filtered_chunk(
            ChatStreamChunk {
                kind: ChatStreamKind::ToolCall,
                text: name.to_string(),
            },
            &mut filter,
            &mut on_event,
        )
        .unwrap();
    }
    assert_eq!(
        prepared,
        [
            "apply_patch",
            "apply_artifact_patch",
            "write_file",
            "edit_string",
            "run_command",
            "task",
            "ask_question"
        ]
    );
    assert_eq!(streamed, names);
}

/// 上一个用例每次调用都新起一个计数器，测的是「单个工具够不够慢」。
/// 这里共用一个计数器，模拟同一条 assistant 消息里连着来的多个调用。
#[test]
fn tool_call_stream_announces_preparation_for_later_calls_in_a_batch() {
    let mut filter = ReasoningTitleFilter::default();
    let mut seen = 0usize;
    let mut prepared = Vec::new();
    let mut on_event = |event| {
        if let AgentEvent::ToolPreparing { name, batch } = event {
            prepared.push((name, batch));
        }
        Ok(())
    };
    for name in ["read_file", "read_file", "glob"] {
        emit_filtered_chunk_at(
            ChatStreamChunk {
                kind: ChatStreamKind::ToolCall,
                text: name.to_string(),
            },
            Instant::now(),
            &mut filter,
            &mut seen,
            &mut on_event,
        )
        .unwrap();
    }
    // 第一个调用照旧不提示——单看 read_file 的参数一个 chunk 就到了,
    // 提示只会闪一下。后面两个才知道这是批量。
    assert_eq!(
        prepared,
        [("read_file".to_string(), true), ("glob".to_string(), true)]
    );
}

#[test]
fn structured_tool_business_failure_marks_the_event_failed() {
    assert!(!tool_output_succeeded(r#"{"success":false}"#));
    assert!(!tool_output_succeeded(r#"{"ok":false}"#));
    assert!(tool_output_succeeded(r#"{"success":true}"#));
    assert!(tool_output_succeeded("plain tool output"));
}

#[tokio::test]
async fn parallel_task_calls_run_concurrently_and_map_outputs() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_paths(temp.path());
    let config = AppConfig::default();
    let state = StateStore::new(&paths).unwrap();
    state.init_files().unwrap();
    let client =
        OpenAiCompatibleClient::new(config.provider(None).unwrap(), &config, &paths).unwrap();
    let mut registry = ToolRegistry::new();
    registry.register(crate::tools::ToolSpec::new(
        "task",
        "stub subagent",
        crate::tools::empty_parameters(),
        |args| async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            Ok(format!(
                "done:{}",
                args.get("n")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("?")
            ))
        },
    ));
    let agent = Agent::new(
        config,
        &paths,
        state.clone(),
        client,
        registry,
        AgentMode::Normal,
    )
    .unwrap();

    let calls: Vec<crate::llm::ToolCall> = (0..3)
        .map(|index| crate::llm::ToolCall {
            id: format!("call_{index}"),
            kind: "function".to_string(),
            function: crate::llm::ToolCallFunction {
                name: "task".to_string(),
                arguments: format!(r#"{{"n":"{index}"}}"#),
            },
        })
        .collect();
    let mut events = Vec::new();
    let started = std::time::Instant::now();
    let outputs = agent
        .execute_parallel_task_calls(&calls, &std::collections::BTreeSet::new(), &mut |event| {
            match &event {
                AgentEvent::ToolCall { call_id, .. } => events.push((call_id.clone(), "call")),
                AgentEvent::ToolResult {
                    call_id, ok: true, ..
                } => events.push((call_id.clone(), "ok")),
                AgentEvent::ToolResult {
                    call_id, ok: false, ..
                } => events.push((call_id.clone(), "err")),
                _ => {}
            }
            Ok(())
        })
        .await
        .unwrap();
    let elapsed = started.elapsed();

    assert_eq!(outputs.len(), 3);
    for index in 0..3 {
        assert_eq!(outputs[&index].output, format!("done:{index}"));
    }
    // Three 80ms tasks run concurrently, not sequentially (~240ms).
    assert!(
        elapsed < Duration::from_millis(200),
        "tasks did not run in parallel: {elapsed:?}"
    );
    for index in 0..3 {
        let call_id = format!("call_{index}");
        assert!(events.contains(&(call_id.clone(), "call")));
        assert!(events.contains(&(call_id, "ok")));
    }

    // Fewer than two task calls: empty map, serial path handles it.
    let single = agent
        .execute_parallel_task_calls(&calls[..1], &std::collections::BTreeSet::new(), &mut |_| {
            Ok(())
        })
        .await
        .unwrap();
    assert!(single.is_empty());
}

#[tokio::test]
async fn responses_tool_round_uses_previous_response_id_and_only_new_input() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_paths(temp.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let mut config = queue_test_config(base_url);
    config.tools.enabled = true;
    config.tools.loading_mode = "full".to_string();
    config.skills.enabled = false;
    config.memory.enabled = false;
    config.providers[0].protocol = "openai-responses".to_string();
    config.providers[0].models = vec!["gpt-5".to_string()];
    config.providers[0].default_model = "gpt-5".to_string();

    let mut tools = ToolRegistry::new();
    tools.register(ToolSpec::new(
        "responses_continuation_tool",
        "returns a fixed result",
        empty_parameters(),
        |_| async { Ok("tool finished".to_string()) },
    ));
    let control = AgentTurnControl::new(AgentMode::Normal, tools.clone(), tools.clone());
    let server_control = control.clone();

    let (first_request_tx, first_request_rx) = oneshot::channel();
    let (second_request_tx, second_request_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let first_request = read_test_http_request(&mut first).await;
        let _ = first_request_tx.send(first_request);
        server_control.set_mode(AgentMode::Dev);
        write_test_sse(
            &mut first,
            concat!(
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\n",
                "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"id\":\"item_1\",\"call_id\":\"call_1\",\"name\":\"responses_continuation_tool\",\"arguments\":\"\"}}\n\n",
                "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"item_1\",\"delta\":\"{}\"}\n\n",
                "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"id\":\"item_1\",\"call_id\":\"call_1\",\"name\":\"responses_continuation_tool\",\"arguments\":\"{}\"}}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"usage\":{\"input_tokens\":5,\"output_tokens\":2,\"total_tokens\":7}}}\n\n"
            ),
        )
        .await;

        let (mut second, _) = listener.accept().await.unwrap();
        let second_request = read_test_http_request(&mut second).await;
        let _ = second_request_tx.send(second_request);
        write_test_sse(
            &mut second,
            concat!(
                "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_2\"}}\n\n",
                "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_2\",\"delta\":\"final answer\"}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_2\"}}\n\n"
            ),
        )
        .await;
    });

    let state = StateStore::new(&paths).unwrap();
    state.init_files().unwrap();
    let provider = config.provider(None).unwrap().clone();
    let client = OpenAiCompatibleClient::new(&provider, &config, &paths).unwrap();
    let mut agent = Agent::new(
        config,
        &paths,
        state.clone(),
        client,
        tools,
        AgentMode::Normal,
    )
    .unwrap();
    state
        .enqueue_prompt("q1", "queued followup", "queued followup", &[])
        .unwrap();

    let result = agent
        .chat_stream_with_control("initial prompt", &[], &control, |_| Ok(()))
        .await
        .unwrap();

    assert_eq!(result.content, "final answer");
    assert_eq!(agent.mode(), AgentMode::Dev);
    assert!(result.responses_continuation.is_none());
    assert!(result.usage_estimated);
    let tool_only_tokens =
        overflow::estimate_messages_tokens(&[ChatMessage::tool("call_1", "tool finished")]) as u64;
    assert!(result.usage.as_ref().unwrap().prompt_tokens > 5 + tool_only_tokens);
    let first_request: Value = serde_json::from_slice(&first_request_rx.await.unwrap()).unwrap();
    assert!(first_request.get("previous_response_id").is_none());
    assert!(first_request["input"].as_array().is_some_and(|input| {
        input.iter().any(|item| item["role"] == "user")
            && input.iter().any(|item| item["role"] == "system")
    }));

    let second_request: Value = serde_json::from_slice(&second_request_rx.await.unwrap()).unwrap();
    assert_eq!(second_request["previous_response_id"], "resp_1");
    let input = second_request["input"].as_array().unwrap();
    let function_output = input
        .iter()
        .find(|item| item["type"] == "function_call_output")
        .unwrap();
    assert_eq!(function_output["call_id"], "call_1");
    assert_eq!(function_output["output"], "tool finished");
    let function_index = input
        .iter()
        .position(|item| item["type"] == "function_call_output")
        .unwrap();
    // Responses-style user items carry their text as `input_text` parts,
    // so the block has to be read through both shapes.
    let item_text = |item: &Value| -> String {
        match &item["content"] {
            Value::String(text) => text.clone(),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join(""),
            _ => String::new(),
        }
    };
    let is_mode_update = |item: &Value| {
        let text = item_text(item);
        item["role"] == "user" && text.contains("<mode-update active=\"dev\">")
    };
    let mode_index = input.iter().position(is_mode_update).unwrap();
    assert!(input.iter().any(is_mode_update));
    let queued_index = input
        .iter()
        .position(|item| {
            item["role"] == "user"
                && item["content"].as_array().is_some_and(|parts| {
                    parts.iter().any(|part| {
                        part["type"] == "input_text" && part["text"] == "queued followup"
                    })
                })
        })
        .unwrap();
    assert!(input.iter().any(|item| {
        item["role"] == "user"
            && item["content"].as_array().is_some_and(|parts| {
                parts
                    .iter()
                    .any(|part| part["type"] == "input_text" && part["text"] == "queued followup")
            })
    }));
    assert!(function_index < mode_index && mode_index < queued_index);
    assert!(!serde_json::to_string(input)
        .unwrap()
        .contains("initial prompt"));
    assert!(second_request["tools"].as_array().is_some_and(|tools| {
        tools
            .iter()
            .any(|tool| tool["name"] == "responses_continuation_tool")
    }));
    assert_eq!(
        state.load_turns().unwrap()[0].assistant_content,
        "final answer"
    );
    server.await.unwrap();
}

/// guard 拒绝是软失败:命令拒绝子串拦下 run_command,回给模型一条
/// tool error 让它换路,轮次存活拿到最终回答——而不是炸掉整轮。
#[tokio::test]
async fn guard_denied_tool_soft_fails_and_turn_continues() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_paths(temp.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let mut config = queue_test_config(base_url);
    config.tools.enabled = true;
    config.skills.enabled = false;
    config.memory.enabled = false;

    let mut normal_tools = ToolRegistry::new();
    normal_tools.register(ToolSpec::new(
        "run_command",
        "runs commands",
        empty_parameters(),
        |_| async { Ok("should never run".to_string()) },
    ));
    normal_tools.add_guard(crate::tools::command_deny_guard(vec![
        "rm -rf /".to_string()
    ]));
    let control = AgentTurnControl::new(
        AgentMode::Normal,
        normal_tools.clone(),
        normal_tools.clone(),
    );
    let (request_tx, request_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let _ = read_test_http_request(&mut first).await;
        write_test_sse(
            &mut first,
            concat!(
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"run_command\",\"arguments\":\"{\\\"command\\\":\\\"sudo rm -rf /\\\"}\"}}]}}]}\n\n",
                "data: {\"choices\":[{\"finish_reason\":\"tool_calls\",\"delta\":{}}]}\n\n",
                "data: [DONE]\n\n"
            ),
        )
        .await;

        let (mut second, _) = listener.accept().await.unwrap();
        let request = read_test_http_request(&mut second).await;
        let _ = request_tx.send(request);
        write_test_sse(
            &mut second,
            concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"recovered answer\"}}]}\n\n",
                "data: {\"choices\":[{\"finish_reason\":\"stop\",\"delta\":{}}]}\n\n",
                "data: [DONE]\n\n"
            ),
        )
        .await;
    });

    let state = StateStore::new(&paths).unwrap();
    state.init_files().unwrap();
    let provider = config.provider(None).unwrap().clone();
    let client = OpenAiCompatibleClient::new(&provider, &config, &paths).unwrap();
    let mut agent = Agent::new(
        config,
        &paths,
        state.clone(),
        client,
        normal_tools,
        AgentMode::Normal,
    )
    .unwrap();

    let result = agent
        .chat_stream_with_control("initial prompt", &[], &control, |_| Ok(()))
        .await
        .unwrap();

    assert_eq!(result.content, "recovered answer");
    let request: serde_json::Value = serde_json::from_slice(&request_rx.await.unwrap()).unwrap();
    let messages = request["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| {
        message["role"] == "tool"
            && message["content"].as_str().is_some_and(|content| {
                content.contains("denied pattern") || content.contains("被禁止的模式")
            })
    }));
    server.await.unwrap();
}

/// 回合内每次模型请求结束都发射 RoundUsage(provider 未报 usage 时走
/// 估算路径),这是 footer/WebUI 逐请求刷新计量的事件源。
#[tokio::test]
async fn round_usage_event_fires_per_model_request() {
    let temp = tempfile::tempdir().unwrap();
    let paths = test_paths(temp.path());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let mut config = queue_test_config(base_url);
    config.tools.enabled = false;
    let server = tokio::spawn(async move {
        let (mut chat, _) = listener.accept().await.unwrap();
        let _ = read_test_http_request(&mut chat).await;
        write_test_sse(
            &mut chat,
            concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"回答\"}}]}\n\n",
                "data: {\"choices\":[{\"finish_reason\":\"stop\",\"delta\":{}}],\"usage\":{\"prompt_tokens\":120,\"completion_tokens\":8,\"total_tokens\":128}}\n\n",
                "data: [DONE]\n\n"
            ),
        )
        .await;
    });
    let state = StateStore::new(&paths).unwrap();
    state.init_files().unwrap();
    let provider = config.provider(None).unwrap().clone();
    let client = OpenAiCompatibleClient::new(&provider, &config, &paths).unwrap();
    let mut agent = Agent::new(
        config,
        &paths,
        state,
        client,
        ToolRegistry::new(),
        AgentMode::Normal,
    )
    .unwrap();
    let rounds = std::cell::RefCell::new(Vec::new());
    agent
        .chat_stream("你好", |event| {
            if let AgentEvent::RoundUsage {
                round,
                turn,
                estimated,
            } = &event
            {
                rounds
                    .borrow_mut()
                    .push((round.prompt_tokens, turn.total, *estimated));
            }
            Ok(())
        })
        .await
        .unwrap();
    let rounds = rounds.into_inner();
    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0].0, 120);
    assert_eq!(rounds[0].1, 128);
    assert!(!rounds[0].2);
    server.await.unwrap();
}
