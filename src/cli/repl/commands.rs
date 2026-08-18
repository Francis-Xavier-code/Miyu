//! REPL 的斜杠命令表。
//!
//! `REPL_COMMAND_TABLE` 是单一事实来源：补全、帮助、解析全从它派生，加命令只
//! 用改这一处。

use crate::cli::*;

pub(in crate::cli) fn print_mode_help() {
    if crate::i18n::is_zh() {
        println!("请选择模式。想让裸 miyu 命令直接进某个模式,可以在设置中修改(config.jsonc 的 default_mode)。\n");
        println!("  miyu normal   普通模式。可使用全部工具,适合日常使用。支持角色扮演、娱乐聊天、记忆、技能等全部能力。");
        println!("  miyu dev      开发模式。与普通模式明确区分,用于开发工作;移除与开发无关的角色扮演与娱乐工具,提示词极简可编辑,记忆独立。");
        println!("  miyu '<your_prompts>'   使用普通模式进行一次性对话");
    } else {
        println!("Pick a mode. To make bare `miyu` enter one directly, set default_mode in config.jsonc.\n");
        println!("  miyu normal   full-capability mode: persona, memory, every tool.");
        println!("  miyu dev      development mode: minimal editable prompt, coding tools only, separate memory.");
        println!("  miyu '<your_prompts>'   one-shot ask in normal mode");
    }
}

pub(in crate::cli) fn split_repl_command(input: &str) -> (&str, &str) {
    let Some((command, args)) = input.split_once(char::is_whitespace) else {
        return (input, "");
    };
    (command, args)
}

pub(in crate::cli) fn print_repl_help() {
    println!("{}", t("commands:", "命令:"));
    let width = REPL_COMMAND_TABLE
        .iter()
        .map(|spec| {
            spec.name.len()
                + if spec.arg_hint.is_empty() {
                    0
                } else {
                    spec.arg_hint.len() + 1
                }
        })
        .max()
        .unwrap_or(0);
    for spec in REPL_COMMAND_TABLE {
        let invocation = if spec.arg_hint.is_empty() {
            spec.name.to_string()
        } else {
            format!("{} {}", spec.name, spec.arg_hint)
        };
        println!("  {invocation:<width$}  {}", t(spec.help_en, spec.help_zh));
    }
    println!("{}", t("keys:", "快捷键:"));
    println!(
        "  Tab         {}",
        t(
            "cycle NORMAL/CHAT, or complete slash commands",
            "循环切换 普通/闲聊，或补全斜杠菜单"
        )
    );
    println!("  Enter       {}", t("send message", "发送消息"));
    println!("  Shift+Enter {}", t("insert newline", "插入换行"));
    println!(
        "  Ctrl+J      {}",
        t(
            "insert newline, same as Shift+Enter",
            "插入换行，与 Shift+Enter 相同"
        )
    );
    println!(
        "  Ctrl+V      {}",
        t(
            "paste image or text from clipboard",
            "从剪贴板粘贴图片或文本"
        )
    );
    println!("  Ctrl+L      {}", t("clear screen", "清屏"));
    println!(
        "  Up/Down     {}",
        t("browse input history", "切换输入历史")
    );
    println!(
        "  Esc Esc     {}",
        t("interrupt running reply", "中断当前回复")
    );
    println!(
        "  Ctrl+C      {}",
        t(
            "clear the draft, else interrupt the reply, else stop background tasks, else exit",
            "先清空输入；输入为空则中断回复；再无回复则停止后台任务；都没有则退出"
        )
    );
    println!("  Ctrl+D      {}", t("exit", "退出"));
}

/// Identity of a REPL slash command, dispatched via `parse_repl_input`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::cli) enum ReplSlashCommand {
    New,
    Session,
    Rename,
    Delete,
    Workspace,
    Models,
    Persona,
    Usage,
    Config,
    Variant,
    Undo,
    Pop,
    Compact,
    Reset,
    ResetMemory,
    Wipe,
    History,
    Clear,
    Help,
    Exit,
}

pub(in crate::cli) struct ReplCommandSpec {
    pub(in crate::cli) name: &'static str,
    pub(in crate::cli) command: ReplSlashCommand,
    /// Argument hint rendered in /help, e.g. "[count]"; empty when the
    /// command takes no arguments (enforced at dispatch).
    pub(in crate::cli) arg_hint: &'static str,
    pub(in crate::cli) help_en: &'static str,
    pub(in crate::cli) help_zh: &'static str,
}

/// Single source of truth for REPL slash commands: drives Tab completion,
/// prefix resolution, /help output, and dispatch in the REPL loop.
pub(in crate::cli) const REPL_COMMAND_TABLE: &[ReplCommandSpec] = &[
    ReplCommandSpec {
        name: "/new",
        command: ReplSlashCommand::New,
        arg_hint: "[name]",
        help_en: "create a new session and switch to it",
        help_zh: "创建新会话并切换过去",
    },
    ReplCommandSpec {
        name: "/session",
        command: ReplSlashCommand::Session,
        arg_hint: "[name|index]",
        help_en: "list sessions, or switch to one (Ctrl+D deletes in the picker)",
        help_zh: "列出会话，或切换到指定会话（菜单内 Ctrl+D 删除）",
    },
    ReplCommandSpec {
        name: "/rename",
        command: ReplSlashCommand::Rename,
        arg_hint: "<name>",
        help_en: "rename the current session",
        help_zh: "重命名当前会话",
    },
    ReplCommandSpec {
        name: "/delete",
        command: ReplSlashCommand::Delete,
        arg_hint: "[name|index]",
        help_en: "delete a session (current by default)",
        help_zh: "删除会话（默认当前会话）",
    },
    ReplCommandSpec {
        name: "/workspace",
        command: ReplSlashCommand::Workspace,
        arg_hint: "[path|clear]",
        help_en: "show, bind, or unbind the session workspace",
        help_zh: "查看、绑定或解绑会话工作目录",
    },
    ReplCommandSpec {
        name: "/models",
        command: ReplSlashCommand::Models,
        arg_hint: "[index|provider/model|default]",
        help_en: "switch this session's model",
        help_zh: "切换当前会话使用的模型",
    },
    ReplCommandSpec {
        name: "/persona",
        command: ReplSlashCommand::Persona,
        arg_hint: "[name]",
        help_en: "switch the active persona",
        help_zh: "切换当前人格",
    },
    ReplCommandSpec {
        name: "/usage",
        command: ReplSlashCommand::Usage,
        arg_hint: "",
        help_en: "show token usage details",
        help_zh: "显示 Token 用量详情",
    },
    ReplCommandSpec {
        name: "/config",
        command: ReplSlashCommand::Config,
        arg_hint: "",
        help_en: "open configuration UI",
        help_zh: "打开配置界面",
    },
    ReplCommandSpec {
        name: "/variant",
        command: ReplSlashCommand::Variant,
        arg_hint: "[name]",
        help_en: "view or switch thinking level",
        help_zh: "查看或切换思考档位",
    },
    ReplCommandSpec {
        name: "/undo",
        command: ReplSlashCommand::Undo,
        arg_hint: "",
        help_en: "undo the last turn or context compaction",
        help_zh: "撤销上一轮或上下文压缩",
    },
    ReplCommandSpec {
        name: "/pop",
        command: ReplSlashCommand::Pop,
        arg_hint: "[count]",
        help_en: "pop selected turns or the oldest count from active context",
        help_zh: "从当前上下文弹出所选轮次或最旧的指定轮数",
    },
    ReplCommandSpec {
        name: "/compact",
        command: ReplSlashCommand::Compact,
        arg_hint: "",
        help_en: "compact current conversation context now",
        help_zh: "立即压缩当前会话上下文",
    },
    ReplCommandSpec {
        name: "/reset",
        command: ReplSlashCommand::Reset,
        arg_hint: "",
        help_en: "start this conversation over",
        help_zh: "重新开始当前会话",
    },
    ReplCommandSpec {
        name: "/reset-memory",
        command: ReplSlashCommand::ResetMemory,
        arg_hint: "",
        help_en: "erase this mode's long-term memory",
        help_zh: "清空当前模式的长期记忆",
    },
    ReplCommandSpec {
        name: "/wipe",
        command: ReplSlashCommand::Wipe,
        arg_hint: "",
        help_en: "erase memory, every conversation, group contexts and generated skills",
        help_zh: "抹掉记忆、所有会话内容、群聊上下文和自动技能",
    },
    ReplCommandSpec {
        name: "/history",
        command: ReplSlashCommand::History,
        arg_hint: "",
        help_en: "show recent conversation history",
        help_zh: "显示最近的会话历史",
    },
    ReplCommandSpec {
        name: "/clear",
        command: ReplSlashCommand::Clear,
        arg_hint: "",
        help_en: "clear the screen",
        help_zh: "清屏",
    },
    ReplCommandSpec {
        name: "/help",
        command: ReplSlashCommand::Help,
        arg_hint: "",
        help_en: "show this help",
        help_zh: "显示此帮助",
    },
    ReplCommandSpec {
        name: "/exit",
        command: ReplSlashCommand::Exit,
        arg_hint: "",
        help_en: "leave REPL",
        help_zh: "退出 REPL",
    },
];

pub(in crate::cli) fn repl_command_spec(command: ReplSlashCommand) -> &'static ReplCommandSpec {
    REPL_COMMAND_TABLE
        .iter()
        .find(|spec| spec.command == command)
        .expect("every ReplSlashCommand has a table entry")
}

/// Parsed REPL input: plain chat, a resolved slash command with its argument
/// string, or an unknown/ambiguous slash command.
/// 一行输入的归类。**没有「未知命令」这一类**——`/` 开头但不命中命令表的输入
/// 就是聊天（见 `parse_repl_input` 的文档）。
pub(in crate::cli) enum ReplInput<'a> {
    Chat,
    Slash(ReplSlashCommand, &'a str),
}

pub(in crate::cli) fn repl_commands() -> Vec<&'static str> {
    REPL_COMMAND_TABLE.iter().map(|spec| spec.name).collect()
}

pub(in crate::cli) fn repl_command_suggestions(input: &str) -> Vec<&'static str> {
    if !input.starts_with('/') {
        return Vec::new();
    }
    repl_commands()
        .into_iter()
        .filter(|command| command.starts_with(input))
        .collect()
}

pub(in crate::cli) fn complete_repl_command(input: &str) -> Option<&'static str> {
    let suggestions = repl_command_suggestions(input);
    if suggestions.len() == 1 {
        suggestions.first().copied()
    } else {
        None
    }
}

/// 命令表里有没有这个**完整**名字。执行前的唯一判定入口——直连道的 if 链和
/// 泄漏守门都问它，不再走前缀展开（理由见 `parse_repl_input`）。
pub(in crate::cli) fn is_repl_command(name: &str) -> bool {
    REPL_COMMAND_TABLE
        .iter()
        .any(|spec| spec.name.eq_ignore_ascii_case(name))
}

pub(in crate::cli) fn repl_command_suggestions_line(suggestions: &[&str], max_width: usize) -> String {
    let line = if suggestions.len() == 1 {
        suggestions[0].to_string()
    } else {
        suggestions.join("  ")
    };
    truncate_visible_width(&line, max_width)
}
