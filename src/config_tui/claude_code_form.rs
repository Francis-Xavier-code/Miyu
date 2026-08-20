//! 内置 Claude Code 特殊供应商的专用编辑表单。
//!
//! 单独成文件的原因:它和通用供应商表单没有共享字段——没有 HTTP 概念,
//! 只有启用总开关与 CLI 中转设置(落盘在 plugins.claude_code),混在
//! providers.rs 里只会把那个本就偏大的文件继续养胖。

use crate::config_tui::*;

/// Claude Code 特殊供应商的编辑表单。它不是 HTTP 端点,所以没有
/// base_url/协议/API Key/超时/额外请求体;取而代之的是启用总开关(同时控制
/// 订阅中转与 claude_code 委托工具)和 CLI 中转的几个开关(存 plugins.claude_code)。
pub(in crate::config_tui) fn edit_claude_code_provider_form(
    stdout: &mut io::Stdout,
    provider: ProviderConfig,
    plugin: &mut crate::config::ClaudeCodePluginConfig,
) -> Result<Option<ProviderConfig>> {
    let mut fields = vec![
        Field::new(
            t(
                "Enabled (subscription relay + claude_code tool)",
                "启用(订阅中转 + claude_code 工具)",
            ),
            provider.enabled.to_string(),
        )
        .choices(&["true", "false"]),
        Field::new(t("Display name", "显示名称"), provider.display_name.clone()),
        Field::new(
            t("claude binary (empty = PATH)", "claude 可执行文件(空=PATH)"),
            plugin.binary.clone(),
        ),
        Field::new(
            t(
                "Expose Miyu tools to claude (MCP bridge)",
                "把 Miyu 工具挂给 claude(MCP 桥)",
            ),
            plugin.expose_miyu_tools.to_string(),
        )
        .choices(&["true", "false"]),
        Field::new(
            t("Stream idle watchdog (seconds)", "流空闲看门狗(秒)"),
            plugin.idle_timeout_seconds.to_string(),
        ),
    ];
    loop {
        if !run_form(
            stdout,
            t(" EDIT CLAUDE CODE ", " 编辑 Claude Code "),
            &mut fields,
        )? {
            return Ok(None);
        }
        let enabled = match parse_bool_field(&fields[0].value) {
            Ok(value) => value,
            Err(error) => {
                message(stdout, &format!("{error:#}"))?;
                continue;
            }
        };
        let expose_miyu_tools = match parse_bool_field(&fields[3].value) {
            Ok(value) => value,
            Err(error) => {
                message(stdout, &format!("{error:#}"))?;
                continue;
            }
        };
        plugin.binary = fields[2].value.trim().to_string();
        plugin.expose_miyu_tools = expose_miyu_tools;
        plugin.idle_timeout_seconds = fields[4].value.trim().parse().unwrap_or(300);
        let mut updated = provider.clone();
        updated.enabled = enabled;
        let display_name = fields[1].value.trim();
        updated.display_name = if display_name.is_empty() {
            "Claude Code".to_string()
        } else {
            display_name.to_string()
        };
        return Ok(Some(updated));
    }
}
