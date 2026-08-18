//! REPL 斜杠命令的解析与补全。

// 被测的东西散在 cli::mod 与 repl 的兄弟模块里，这里全都要够到。
use crate::cli::repl::editor::*;
use crate::cli::repl::width::*;
use crate::cli::*;
#[test]
fn reset_is_a_repl_command() {
    assert!(repl_commands().contains(&"/reset"));
}

#[test]
fn compact_is_a_repl_command() {
    assert!(repl_commands().contains(&"/compact"));
}

#[test]
fn usage_and_persona_are_repl_commands() {
    assert!(repl_commands().contains(&"/usage"));
    assert!(repl_commands().contains(&"/persona"));
    assert_eq!(resolve_repl_command("/us"), "/usage");
    assert_eq!(split_repl_command("/persona Alice.md"), ("/persona", "Alice.md"));
}

#[test]
fn command_suggestions_are_prefixed_and_truncated() {
    let suggestions = repl_command_suggestions("/");
    let line = repl_command_suggestions_line(&suggestions, 24);
    assert!(line.starts_with("/new"));
    assert!(visible_width(&line) <= 24);

    let line = repl_command_suggestions_line(&["/compact"], 40);
    assert_eq!(line, "/compact");
}

#[test]
fn truncation_respects_very_narrow_widths() {
    assert_eq!(truncate_visible_width("abcdef", 0), "");
    assert_eq!(truncate_visible_width("abcdef", 1), ".");
    assert_eq!(truncate_visible_width("abcdef", 2), "..");
    assert_eq!(truncate_visible_width("abcdef", 3), "...");
}

#[test]
fn shortcut_hint_line_is_bar_aligned_and_truncated() {
    // Tab 切换模式已随闲聊模式删除,提示行首个词条现在是换行快捷键。
    let line = repl_shortcut_hint_line(AgentMode::Normal, 24);
    assert!(strip_terminal_control_sequences(&line).contains("Shift+Enter"));
    assert!(visible_width(&line) <= 24);
}

#[test]
fn inline_fuzzy_lines_are_bar_aligned_and_truncated() {
    let header = inline_fuzzy_header("big", 12);
    assert!(strip_terminal_control_sequences(&header).contains(t("Select", "选择模型")));
    assert!(visible_width(&header) <= 12);

    let item = inline_fuzzy_item_line("opencode Zen / big-pickle", true, false, 16);
    let item_plain = strip_terminal_control_sequences(&item);
    assert!(item_plain.starts_with("› [ ]"));
    assert!(item_plain.contains("open"));
    assert!(visible_width(&item) <= 16);

    let item = inline_fuzzy_item_line("opencode Zen / big-pickle", false, true, 18);
    let item_plain = strip_terminal_control_sequences(&item);
    assert!(item_plain.starts_with("  [*]"));
    assert!(item_plain.contains("opencode"));
    assert!(visible_width(&item) <= 18);

    let help = inline_fuzzy_help_line(40);
    let help_plain = strip_terminal_control_sequences(&help);
    assert!(help_plain.contains("j/k"));
    assert!(visible_width(&help) <= 40);
}

#[test]
fn wipe_is_its_own_command_not_a_suffix_on_reset() {
    // `/reset` and `/reset all` differed by one word and by everything
    // else: one starts a conversation over, the other erased memory, every
    // session and the generated skills. They answer under separate names
    // now, and `/wipe` is far enough from `/w…` prefixes to be typed on
    // purpose.
    assert!(matches!(
        parse_repl_input("/wipe"),
        ReplInput::Slash(ReplSlashCommand::Wipe, "")
    ));
    assert!(matches!(
        parse_repl_input("/reset"),
        ReplInput::Slash(ReplSlashCommand::Reset, "")
    ));
    assert!(matches!(
        parse_repl_input("/reset all"),
        ReplInput::Slash(ReplSlashCommand::Reset, "all")
    ));
}

#[test]
fn partial_slash_command_resolves_unique_match() {
    assert_eq!(resolve_repl_command("/model"), "/models");
    assert_eq!(resolve_repl_command("/compa"), "/compact");
    assert_eq!(resolve_repl_command("/co"), "/co");
    assert_eq!(resolve_repl_command("hello"), "hello");
}

#[test]
fn parse_repl_input_dispatches_by_table() {
    assert!(matches!(parse_repl_input("hello"), ReplInput::Chat));
    assert!(matches!(
        parse_repl_input("/models"),
        ReplInput::Slash(ReplSlashCommand::Models, "")
    ));
    // Unique prefix resolves.
    assert!(matches!(
        parse_repl_input("/compa"),
        ReplInput::Slash(ReplSlashCommand::Compact, "")
    ));
    // Exact match wins over ambiguous prefixes of longer names.
    assert!(matches!(
        parse_repl_input("/reset all"),
        ReplInput::Slash(ReplSlashCommand::Reset, "all")
    ));
    // Case-insensitive.
    assert!(matches!(
        parse_repl_input("/POP 3"),
        ReplInput::Slash(ReplSlashCommand::Pop, "3")
    ));
    // Ambiguous prefix stays unknown.
    assert!(matches!(
        parse_repl_input("/co"),
        ReplInput::UnknownSlash("/co")
    ));
    assert!(matches!(
        parse_repl_input("/nope"),
        ReplInput::UnknownSlash("/nope")
    ));
}

#[test]
fn every_repl_slash_command_has_a_table_entry() {
    // repl_command_spec panics on a missing entry; touch every variant.
    for spec in REPL_COMMAND_TABLE {
        assert_eq!(repl_command_spec(spec.command).command, spec.command);
    }
}
