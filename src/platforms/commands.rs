use crate::config::{PlatformCommandPermission, PlatformsConfig};
use crate::i18n::text as t;

pub(crate) const RESET_COMMAND_ID: &str = "reset";

#[derive(Clone, Copy, Debug)]
pub(crate) struct PlatformCommandDescriptor {
    pub(crate) id: &'static str,
    pub(crate) default_permission: PlatformCommandPermission,
}

pub(crate) const BUILTIN_COMMANDS: &[PlatformCommandDescriptor] = &[PlatformCommandDescriptor {
    id: RESET_COMMAND_ID,
    default_permission: PlatformCommandPermission::AdminOnly,
}];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ParsedPlatformCommand {
    Reset { has_arguments: bool },
    Unknown,
}

pub(crate) fn descriptor(id: &str) -> Option<&'static PlatformCommandDescriptor> {
    BUILTIN_COMMANDS.iter().find(|command| command.id == id)
}

pub(crate) fn parse(config: &PlatformsConfig, text: &str) -> Option<ParsedPlatformCommand> {
    let text = text.trim();
    let rest = text.strip_prefix(&config.command_prefix)?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        return Some(ParsedPlatformCommand::Unknown);
    }

    let mut parts = rest.split_whitespace();
    let command = parts.next().unwrap_or_default();
    let has_arguments = parts.next().is_some();
    if command.eq_ignore_ascii_case(RESET_COMMAND_ID) {
        Some(ParsedPlatformCommand::Reset { has_arguments })
    } else {
        Some(ParsedPlatformCommand::Unknown)
    }
}

pub(crate) fn is_allowed(
    config: &PlatformsConfig,
    command: &PlatformCommandDescriptor,
    is_admin: bool,
) -> bool {
    match config.command_permission(command.id, command.default_permission) {
        PlatformCommandPermission::Everyone => true,
        PlatformCommandPermission::AdminOnly => is_admin,
    }
}

pub(crate) fn command_text(config: &PlatformsConfig, command: &str) -> String {
    format!("{}{}", config.command_prefix, command)
}

pub(crate) fn permission_denied_message(
    config: &PlatformsConfig,
    command: &PlatformCommandDescriptor,
) -> String {
    format!(
        "{} {}{}",
        t(
            "Only platform administrators may use",
            "只有通讯平台管理员可以使用"
        ),
        command_text(config, command.id),
        t(".", "。")
    )
}

pub(crate) fn reset_usage_message(config: &PlatformsConfig) -> String {
    format!(
        "{}{}{}",
        t("Usage", "用法"),
        t(": ", "："),
        command_text(config, RESET_COMMAND_ID)
    )
}

pub(crate) fn unknown_command_message(config: &PlatformsConfig) -> String {
    let available = BUILTIN_COMMANDS
        .iter()
        .map(|command| command_text(config, command.id))
        .collect::<Vec<_>>()
        .join(t(", ", "、"));
    format!(
        "{}{}",
        t(
            "Unknown command. Available commands: ",
            "未知命令。可用命令："
        ),
        available
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PlatformCommandConfig, PlatformsConfig};

    #[test]
    fn parses_default_and_custom_prefixes_with_command_boundaries() {
        let mut config = PlatformsConfig::default();
        assert_eq!(
            parse(&config, "/reset"),
            Some(ParsedPlatformCommand::Reset {
                has_arguments: false
            })
        );
        assert_eq!(
            parse(&config, "  /RESET  "),
            Some(ParsedPlatformCommand::Reset {
                has_arguments: false
            })
        );
        assert_eq!(
            parse(&config, "/reset now"),
            Some(ParsedPlatformCommand::Reset {
                has_arguments: true
            })
        );
        assert_eq!(
            parse(&config, "/resetting"),
            Some(ParsedPlatformCommand::Unknown)
        );
        assert_eq!(
            parse(&config, "/ reset"),
            Some(ParsedPlatformCommand::Unknown)
        );
        assert_eq!(parse(&config, "please /reset"), None);

        config.command_prefix = "喵".to_string();
        assert_eq!(
            parse(&config, "喵reset"),
            Some(ParsedPlatformCommand::Reset {
                has_arguments: false
            })
        );
        assert_eq!(parse(&config, "/reset"), None);
    }

    #[test]
    fn reset_defaults_to_admin_and_supports_an_everyone_override() {
        let mut config = PlatformsConfig::default();
        let reset = descriptor(RESET_COMMAND_ID).unwrap();
        assert!(is_allowed(&config, reset, true));
        assert!(!is_allowed(&config, reset, false));

        config.commands.insert(
            RESET_COMMAND_ID.to_string(),
            PlatformCommandConfig {
                permission: PlatformCommandPermission::Everyone,
            },
        );
        assert!(is_allowed(&config, reset, false));
    }
}
