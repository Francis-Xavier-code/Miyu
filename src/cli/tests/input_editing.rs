//! 输入编辑：光标、换行、粘贴、历史。

// 被测的东西散在 cli::mod 与 repl 的兄弟模块里，这里全都要够到。
use crate::cli::repl::editor::*;
use crate::cli::*;
use super::shared::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// 回归：daemon 模式下终端图片印成满屏。
///
/// 真正画图的是终端这一侧，而尺寸此前只给表情包工具算，别的工具一律
/// 传 None——`parse_size(None)` 的语义正是「铺满整个终端」。
#[test]
fn every_tool_image_gets_a_size_not_just_memes() {
    let config = crate::config::AppConfig::default();
    // 没有一个工具可以拿着 None 去调 print_image_file。
    for name in [
        "generate_image",
        "print_image",
        "search_web_images",
        "use_meme",
        "",
    ] {
        assert!(
            remote_tool_image_size(name, "", &config).is_some(),
            "{name} 没拿到尺寸，会印成满屏"
        );
    }
    // 模型显式要的尺寸优先，且不被百分比覆盖。
    assert_eq!(
        remote_tool_image_size("print_image", "40x12", &config),
        Some("40x12".to_string())
    );
    // 空白不算「要了」，仍走配置百分比。
    assert_ne!(
        remote_tool_image_size("print_image", "   ", &config),
        Some("   ".to_string())
    );
}

/// 回归(PR#31):会话内历史无上限,常开 REPL 线性增长。
#[test]
fn repl_history_is_capped() {
    let mut history = Vec::new();
    for i in 0..(REPL_HISTORY_LIMIT + 100) {
        push_history_capped(&mut history, &format!("entry-{i}"));
    }
    assert_eq!(history.len(), REPL_HISTORY_LIMIT);
    assert_eq!(history.first().map(String::as_str), Some("entry-100"));
    assert_eq!(history.last().map(String::as_str), Some("entry-599"));
}

#[test]
fn models_is_the_cli_model_selector() {
    let matches = localized_command()
        .try_get_matches_from(["miyu", "models", "1"])
        .unwrap();
    let cli = Cli::from_arg_matches(&matches).unwrap();

    assert!(matches!(
        cli.command,
        Some(Command::Models(ModelsArgs { target: Some(ref target), global: false })) if target == "1"
    ));
    let old_matches = localized_command()
        .try_get_matches_from(["miyu", "providers"])
        .unwrap();
    let old_cli = Cli::from_arg_matches(&old_matches).unwrap();
    assert!(old_cli.command.is_none());
    assert_eq!(old_cli.message, ["providers"]);
}

#[tokio::test]
async fn one_shot_turns_default_to_a_throwaway_session() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let paths = MiyuPaths {
        root_dir: root.to_path_buf(),
        config_dir: root.join("config"),
        config_file: root.join("config/config.jsonc"),
        skills_dir: root.join("config/skills"),
        data_dir: root.join("data"),
        cache_dir: root.join("cache"),
        state_dir: root.join("state"),
        pictures_dir: root.join("pictures"),
        fish_hook_file: root.join("fish"),
        bash_hook_file: root.join("bash"),
        zsh_hook_file: root.join("zsh"),
        scripts_dir: root.join("scripts"),
        system_scripts_dir: root.join("system-scripts"),
    };

    // Neither flag: `miyu ask` / `miyu '<message>'` must not touch a real
    // conversation. `--continue` opts back into the terminal session.
    // Both resolve without contacting the daemon.
    assert_eq!(
        one_shot_session(&paths, None, false).await.unwrap(),
        TurnSession::Ephemeral
    );
    assert_eq!(
        one_shot_session(&paths, None, true).await.unwrap(),
        TurnSession::Current
    );
}

#[tokio::test]
async fn config_reload_retries_busy_responses_until_success() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();

    retry_config_reload(4, Duration::ZERO, move || {
        let attempts = attempts.clone();
        async move {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(if attempt < 3 {
                ConfigReloadResponse::Busy
            } else {
                ConfigReloadResponse::Reloaded
            })
        }
    })
    .await
    .unwrap();

    assert_eq!(observed.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn config_reload_stops_after_the_attempt_limit() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();

    let error = retry_config_reload(3, Duration::ZERO, move || {
        let attempts = attempts.clone();
        async move {
            attempts.fetch_add(1, Ordering::SeqCst);
            Ok(ConfigReloadResponse::Busy)
        }
    })
    .await
    .unwrap_err();

    assert_eq!(observed.load(Ordering::SeqCst), 3);
    assert!(error.to_string().contains('3'));
}

#[tokio::test]
async fn config_reload_retries_coded_busy_frames_over_ipc() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("reload.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let server = tokio::spawn(async move {
        for attempt in 1..=3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = ipc::receive::<IpcRequest>(&mut stream)
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(request.command, IpcCommand::ReloadConfig));
            let response = if attempt < 3 {
                IpcFrame::coded_error(ipc::ErrorCode::Busy, ipc::ADMIN_BUSY_MESSAGE)
            } else {
                IpcFrame::Ack
            };
            ipc::send(&mut stream, &response).await.unwrap();
        }
    });

    retry_config_reload(4, Duration::ZERO, || {
        request_config_reload_at(&socket, Duration::from_secs(1))
    })
    .await
    .unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn config_reload_request_times_out_when_daemon_does_not_respond() {
    let temp = tempfile::tempdir().unwrap();
    let socket = temp.path().join("reload-timeout.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = ipc::receive::<IpcRequest>(&mut stream)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(request.command, IpcCommand::ReloadConfig));
        std::future::pending::<()>().await;
    });

    let error = request_config_reload_at(&socket, Duration::from_millis(100))
        .await
        .unwrap_err();
    assert!(error
        .downcast_ref::<tokio::time::error::Elapsed>()
        .is_some());
    server.abort();
    let _ = server.await;
}

#[test]
fn live_editor_restores_clear_screen_and_double_escape_controls() {
    let temp = tempfile::tempdir().unwrap();
    let paths = pop_test_paths(temp.path());
    let escape = || Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    let mut editor = LiveReplEditor::new(AgentMode::Normal, Vec::new());
    editor.input = "draft".to_string();
    assert!(matches!(
        editor.handle_event(escape(), &paths, true).unwrap(),
        LiveEditorAction::Redraw
    ));
    // Arming the interrupt must not clear the typed draft.
    assert_eq!(editor.input, "draft");
    assert!(matches!(
        editor.handle_event(escape(), &paths, true).unwrap(),
        LiveEditorAction::Interrupt
    ));
    assert_eq!(editor.input, "draft");

    let clear = Event::Key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL));
    assert!(matches!(
        editor.handle_event(clear, &paths, true).unwrap(),
        LiveEditorAction::ClearScreen
    ));

    assert!(matches!(
        editor.handle_event(escape(), &paths, false).unwrap(),
        LiveEditorAction::Redraw
    ));
    assert!(matches!(
        editor.handle_event(escape(), &paths, false).unwrap(),
        LiveEditorAction::Redraw
    ));
    // Esc no longer clears drafts anywhere; empty the editor manually
    // before asserting the empty-submit path.
    assert_eq!(editor.input, "draft");
    editor.clear();

    assert!(matches!(
        editor
            .handle_event(
                Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
                &paths,
                false,
            )
            .unwrap(),
        LiveEditorAction::EmptySubmit
    ));
    assert!(editor.history.is_empty());

    editor.input = "/help".to_string();
    editor.cursor = editor.input.chars().count();
    assert!(matches!(
        editor
            .handle_event(
                Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
                &paths,
                false,
            )
            .unwrap(),
        LiveEditorAction::Submit(_)
    ));
    assert!(editor.history.is_empty());
    editor.record_history("ordinary prompt");
    assert_eq!(editor.history, ["ordinary prompt"]);
}

#[test]
fn live_editor_shift_enter_inserts_newline_without_submit() {
    let temp = tempfile::tempdir().unwrap();
    let paths = pop_test_paths(temp.path());
    let mut editor = LiveReplEditor::new(AgentMode::Normal, Vec::new());
    editor.input = "hello".to_string();
    editor.cursor = 5;
    assert!(matches!(
        editor
            .handle_event(
                Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
                &paths,
                false,
            )
            .unwrap(),
        LiveEditorAction::Redraw
    ));
    assert_eq!(editor.input, "hello\n");
    assert_eq!(editor.cursor, 6);

    assert!(matches!(
        editor
            .handle_event(
                Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL)),
                &paths,
                false,
            )
            .unwrap(),
        LiveEditorAction::Redraw
    ));
    assert_eq!(editor.input, "hello\n\n");
    assert_eq!(editor.cursor, 7);
}

#[test]
fn prompt_rows_wrap_at_terminal_width() {
    assert_eq!(repl_prompt_rows_for_cols("", &["1234567".into()], 10), 1);
    assert_eq!(repl_prompt_rows_for_cols("", &["1234567890".into()], 10), 2);
    assert_eq!(
        repl_prompt_rows_for_cols("", &["123".into(), "456".into()], 10),
        2
    );
}

#[test]
fn cursor_position_wraps_at_terminal_width() {
    assert_eq!(repl_cursor_position_for_cols("", "1234567", 7, 10), (7, 0));
    assert_eq!(
        repl_cursor_position_for_cols("", "1234567890", 10, 10),
        (0, 1)
    );
    assert_eq!(repl_cursor_position_for_cols("", "123\n456", 7, 10), (3, 1));
    assert_eq!(repl_cursor_position_for_cols("", "1234567", 3, 10), (3, 0));
}

#[test]
fn cursor_position_keeps_prefix_after_newline() {
    assert_eq!(repl_cursor_position_for_cols("  ", "123\n", 4, 10), (2, 1));
    assert_eq!(
        repl_cursor_position_for_cols("  ", "123\n456", 7, 10),
        (5, 1)
    );
}

#[test]
fn prompt_rows_include_prefix_on_each_line() {
    assert_eq!(
        repl_prompt_rows_for_cols("  ", &["12".into(), "34".into()], 5),
        2
    );
    assert_eq!(
        repl_prompt_rows_for_cols("  ", &["123".into(), "34".into()], 5),
        3
    );
}

#[test]
fn wrapped_input_rows_keep_prefix_outside_content_width() {
    assert_eq!(
        repl_wrapped_input_rows_for_cols("  ", &["123456789".into()], 10),
        vec!["12345678".to_string(), "9".to_string()]
    );
    assert_eq!(
        repl_wrapped_input_rows_for_cols("  ", &["12345678".into()], 10),
        vec!["12345678".to_string(), String::new()]
    );
    assert_eq!(
        repl_cursor_position_for_cols("  ", "12345678", 8, 10),
        (2, 1)
    );
}

#[test]
fn history_browsing_requires_empty_or_clean_history_input() {
    let history = vec!["first".to_string(), "second".to_string()];

    assert!(repl_should_browse_history("", &history, None));
    assert!(repl_should_browse_history("second", &history, Some(1)));
    assert!(!repl_should_browse_history("draft", &history, None));
    assert!(!repl_should_browse_history(
        "second edited",
        &history,
        Some(1)
    ));
}

#[test]
fn vertical_cursor_move_uses_soft_wrapped_rows() {
    assert_eq!(
        repl_move_cursor_vertical_for_cols("  ", "123456789", 9, -1, 10),
        1
    );
    assert_eq!(
        repl_move_cursor_vertical_for_cols("  ", "123456789", 1, 1, 10),
        9
    );
}

#[test]
fn vertical_cursor_move_handles_explicit_newlines() {
    assert_eq!(
        repl_move_cursor_vertical_for_cols("  ", "abc\ndef", 6, -1, 20),
        2
    );
    assert_eq!(
        repl_move_cursor_vertical_for_cols("  ", "abc\ndef", 2, 1, 20),
        6
    );
}

#[test]
fn vertical_cursor_move_handles_wide_chars_near_wrap() {
    assert_eq!(
        repl_cursor_position_for_cols("  ", "1234567你", 8, 11),
        (2, 1)
    );
    assert_eq!(
        repl_cursor_position_for_cols("  ", "12345678你", 9, 11),
        (4, 1)
    );
    assert_eq!(
        repl_move_cursor_vertical_for_cols("  ", "12345678你好", 9, -1, 11),
        2
    );
}

#[test]
fn drain_stdin_does_not_panic() {
    drain_stdin();
}

#[test]
fn input_helpers_edit_at_cursor() {
    let mut input = "abcd".to_string();
    let mut cursor = 2;
    insert_char_at_cursor(&mut input, &mut cursor, '中');
    assert_eq!(input, "ab中cd");
    assert_eq!(cursor, 3);

    remove_char_before_cursor(&mut input, &mut cursor);
    assert_eq!(input, "abcd");
    assert_eq!(cursor, 2);

    remove_char_at_cursor(&mut input, cursor);
    assert_eq!(input, "abd");
    assert_eq!(cursor, 2);
}

#[test]
fn input_helpers_remove_word_before_cursor() {
    let mut input = "hello world  ".to_string();
    let mut cursor = input.chars().count();
    remove_word_before_cursor(&mut input, &mut cursor);
    assert_eq!(input, "hello ");
    assert_eq!(cursor, 6);

    let mut input = "前面 中间 后面".to_string();
    let mut cursor = 6;
    remove_word_before_cursor(&mut input, &mut cursor);
    assert_eq!(input, "前面 后面");
    assert_eq!(cursor, 3);
}

#[test]
fn input_helpers_insert_paste_at_cursor() {
    let mut input = "前后".to_string();
    let mut cursor = 1;
    insert_str_at_cursor(&mut input, &mut cursor, "中间");
    assert_eq!(input, "前中间后");
    assert_eq!(cursor, 3);
}

#[test]
fn input_helpers_insert_newline_at_cursor() {
    let mut input = "前后".to_string();
    let mut cursor = 1;
    insert_newline_at_cursor(&mut input, &mut cursor);
    assert_eq!(input, "前\n后");
    assert_eq!(cursor, 2);
}

#[test]
fn long_paste_visible_lines_are_collapsed() {
    let lines = (0..20)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>();
    let visible = repl_visible_input_lines("[NORMAL] > ", &lines, 12, true);

    assert_eq!(visible.len(), 3);
    assert_eq!(visible[0], "line 0");
    assert!(visible[1].contains("18") || visible[1].contains("已隐藏 18"));
    assert_eq!(visible[2], "line 19");
    assert_eq!(lines.len(), 20);
}

#[test]
fn long_paste_is_replaced_with_placeholder_and_expanded() {
    let text = "alpha\nbeta\ngamma".to_string();
    let placeholder = pasted_text_placeholder(1, pasted_text_line_count(&text));
    let input = format!("请分析 {placeholder}谢谢");
    let pasted_texts = vec![Some(PastedText { text: text.clone() })];

    assert!(should_summarize_pasted_text(&text));
    assert_eq!(
        expand_pasted_text_placeholders(&input, &pasted_texts),
        "请分析 alpha\nbeta\ngamma谢谢"
    );
}

#[test]
fn short_paste_is_not_summarized() {
    assert!(!should_summarize_pasted_text("short paste"));
}

#[test]
fn insert_pasted_text_summarizes_long_clipboard_text() {
    let mut input = "前后".to_string();
    let mut cursor = 1;
    let mut pasted_texts = Vec::new();

    insert_pasted_text_at_cursor(
        &mut input,
        &mut cursor,
        "alpha\nbeta\ngamma".to_string(),
        &mut pasted_texts,
    );

    assert!(
        input == "前[Pasted 1: ~3 lines]后" || input == "前[粘贴 1: ~3 行]后",
        "unexpected localized placeholder: {input}"
    );
    assert_eq!(pasted_texts.len(), 1);
    assert_eq!(cursor, input.chars().count() - 1);
}

#[test]
fn pasted_placeholder_is_treated_as_atomic_token() {
    let input = "前[Pasted 1: ~3 lines] 后";
    assert_eq!(placeholder_at_cursor(input, 3), Some((1, 21)));
    assert_eq!(placeholder_before_cursor(input, 21), Some((1, 21)));
    assert_eq!(placeholder_after_cursor(input, 1), Some((1, 21)));
    assert_eq!(placeholder_before_or_at_cursor(input, 3), Some((1, 21)));
    assert_eq!(placeholder_after_or_at_cursor(input, 3), Some((1, 21)));
}

#[test]
fn chinese_pasted_placeholder_is_supported() {
    let input = "前[粘贴 1: ~3 行] 后";
    let placeholder = find_pasted_text_placeholders(input);

    assert_eq!(placeholder, vec![(1, 13, 1)]);
    assert_eq!(placeholder_at_cursor(input, 3), Some((1, 13)));
    assert_eq!(placeholder_before_cursor(input, 13), Some((1, 13)));
    assert_eq!(placeholder_after_cursor(input, 1), Some((1, 13)));
}

#[test]
fn colorizes_image_and_pasted_placeholders() {
    let colored = colorize_repl_placeholders("[Image 1] [Pasted 1: ~3 lines]");
    assert!(colored.contains("\x1b[35m[Image 1]\x1b[0m"));
    assert!(colored.contains("\x1b[35m[Pasted 1: ~3 lines]\x1b[0m"));
}

#[test]
fn placeholder_text_near_cursor_expands_pasted_placeholder() {
    let input = "前[Pasted 1: ~3 lines]后";
    let pasted_texts = vec![Some(PastedText {
        text: "alpha\nbeta\ngamma".to_string(),
    })];

    assert_eq!(
        placeholder_text_near_cursor(input, 3, &pasted_texts),
        Some("alpha\nbeta\ngamma".to_string())
    );
}

#[test]
fn strips_terminal_control_sequences_from_repl_text() {
    assert_eq!(
        strip_terminal_control_sequences("\x1b[E表情包\x1b[0m\x07 ok"),
        "表情包 ok"
    );
    assert_eq!(
        strip_terminal_control_sequences("line1\nline2\tend"),
        "line1\nline2\tend"
    );
}

#[test]
fn repl_history_loads_user_messages_from_state() {
    let temp = tempfile::tempdir().unwrap();
    let paths = MiyuPaths {
        root_dir: PathBuf::new(),
        config_dir: PathBuf::new(),
        config_file: PathBuf::new(),
        skills_dir: PathBuf::new(),
        data_dir: PathBuf::new(),
        cache_dir: PathBuf::new(),
        state_dir: temp.path().to_path_buf(),
        pictures_dir: PathBuf::new(),
        fish_hook_file: PathBuf::new(),
        bash_hook_file: PathBuf::new(),
        zsh_hook_file: PathBuf::new(),
        scripts_dir: PathBuf::new(),
        system_scripts_dir: PathBuf::new(),
    };
    let state = StateStore::new(&paths).unwrap();
    state.start_turn("turn_1", "first", 999999).unwrap();
    state.complete_turn("turn_1", "reply", None).unwrap();
    state.start_turn("turn_2", "second", 999999).unwrap();

    assert_eq!(
        load_repl_input_history(&state, &paths).unwrap(),
        vec!["first".to_string(), "second".to_string()]
    );
}
