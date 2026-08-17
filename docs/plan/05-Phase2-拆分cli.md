# 05 · Phase 2：拆分 `src/cli.rs`

目标：把 16.5k 行的 `cli.rs` 拆成 `src/cli/` 目录，入口 `mod.rs` 保持在 500 行以内。

## 1. 建议执行顺序（按依赖，从叶子到入口）

1. `args.rs`（已在 Phase 1 部分完成）
2. `shell_bridge.rs`
3. `data_cmds.rs`
4. `migrate_cmds.rs`
5. `model_cmds.rs`
6. `init_cmds.rs`
7. `daemon_cmds.rs`
8. `repl/` 内部
9. `one_shot.rs`
10. `mod.rs`

每个步骤独立提交；**禁止一次性全拆后提交**。

## 2. `src/cli/args.rs`

从 `cli.rs` 搬入并保持函数名不变：

| 原位置 | 内容 |
|---|---|
| 379–415 | `Cli` 根参数 |
| 417–486 | `parse/parse_args/extract_debug_flag/localized_command` |
| 488–548 | `root_help_template` |
| 550–627 | `apply_localized_help_flags/apply_chinese_help_template/localize_top_args` |
| 628–1070 | `localize_subcommands` 与各 `localize_*` 函数 |
| 1071–1434 | `Command`、全部 `*Args`、`DaemonCommand`、`ConfigCommand`、子命令枚举 |

要求：

- `WebArgs/DaemonArgs/DaemonLogsArgs` 若 Phase 1 已下沉 `crate::args`，这里直接 `use` 并 re-export，不再重复定义。
- `t()`/`is_zh()` 本地化 helper 保持原调用。
- 所有 clap 帮助文案一字不改。

## 3. `src/cli/shell_bridge.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 4940–4994 | `run_clipboard_paste`、`shell_pasted_text_index` |
| 5009–5064 | `shell_message_from_input`、`run_shell_classify`、`run_shell_intercept` |
| 5065–5143 | `expand_shell_pasted_text_placeholders`、`extract_image_placeholders` |
| 5144–5250 | `run_chat_with_images` |
| 5251–5345 | `drain_stdin`、`append_stdin_if_piped` |

注意：图片占位符格式 `[Image N: filename]` 是 shell hook 与 REPL 共用的协议，只移动实现，不改格式。

## 4. `src/cli/data_cmds.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 15934–15977 | `run_history`、`run_history_with_state` |
| 15978–16039 | `run_kb` |
| 16040–16079 | `run_update_default_kb`、`write_default_kb_update_progress` |
| 16080–16173 | `run_memory`、`run_skills`、`skill_names`、`skill_dir` |
| 16206–16293 | `run_reset`、`run_reset_memory_command`、`run_wipe`、`wipe_summary`、`print_wipe_message`、`print_reset_message` |
| 16306–16336 | `join_message` |

## 5. `src/cli/migrate_cmds.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 3901–3934 | `default_export_name`、`readable_bytes`、`owned` |
| 3935–3996 | `run_export` |
| 3997–4100 | `run_import` |

同时搬走相关测试；`export/import` 的提示文案与归档命名规则保持逐字节一致。

## 6. `src/cli/model_cmds.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 3564–3671 | `run_models`、`run_models_for_session` |
| 3672–3691 | `list_persona_files` |
| 3692–3806 | `run_persona_picker`、`compact_watermark_text`、`usage_overview_text` |
| 4101–4209 | `run_list_models`、`print_model_choices`、`session_model_override_snapshot`、`set_session_models` |
| 7114–7208 | `run_variant`、`execute_variant`、`resolve_variant_name`、`print_variant_updated` |

依赖 `cli::repl::inline_select`（见下）时用 `pub(super)` 暴露选择器函数。

## 7. `src/cli/init_cmds.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 2908–2996 | `run_init`、`print_init_step`、`remove_shell_hooks` |
| 2997–3077 | `run_alarm_worker`、`play_alarm_once`、`terminal_bell_fallback`、`append_alarm_log`、`alarm_worker_paths` |
| 3084–3563 | `run_pop`、`run_pop_via_daemon`、`execute_pop`、pop 校验与 inline pop 选择 |

## 8. `src/cli/daemon_cmds.rs`

搬入：

| 原位置 | 函数 |
|---|---|
| 1640–1721 | `initialize_models_cache`、`run_web`、`web_launch_config`、`daemon_web_access_urls` |
| 1722–2021 | `run_daemon_command`、`stop_daemon`、`print_daemon_status`、`daemon_web_status_lines`、`run_request_monitor`、`run_daemon_logs` |
| 2022–2586 | 日志解析/格式化/快照/follow 全套 |
| 2587–2699 | reload 全套 |
| 2700–2907 | `run_tool`、`run_tool_call` |

拆分时日志解析器可进一步独立成 `daemon_logs.rs`，但本阶段先整体搬入。

## 9. `src/cli/repl/` 拆分

### `repl/slash.rs`

| 原位置 | 函数 |
|---|---|
| 13442–13622 | `ReplSlashCommand`、`REPL_COMMAND_TABLE`、`repl_command_spec` 等定义 |
| 13623–13720 | `parse_repl_input`、`repl_commands`、补全、解析、建议 |

### `repl/remote.rs`

| 原位置 | 函数 |
|---|---|
| 5601–6287 | `is_remote_turn_detached/cancelled`、`detect_origin_tty`、`try_run_remote_chat` |
| 6288–7222 | IPC 发送/校验、会话寻址、`run_remote_repl`、`apply_repl_session_switch`、远端图片/token/溢出 |
| 8855–8955 | `reload_repl_config`、`footer_config_for_session`、`apply_session_model_override` |

### `repl/direct.rs`

| 原位置 | 函数 |
|---|---|
| 8263–8854 | `direct_mode_requested`、`run_direct_repl` 及直连命令分发 |

### `repl/editor.rs`

| 原位置 | 函数 |
|---|---|
| 9497–9904 | `LiveReplEditor` 与编辑操作 |
| 12067–13060 | `read_repl_input`、渲染、光标/换行/历史处理 |
| 13061–13441 | 插入/删除/粘贴占位符等纯编辑 helper |

### `repl/tail.rs`

| 原位置 | 函数 |
|---|---|
| 9906–10768 | `LiveReplTail`、终端布局、frame tracker、spinner/job 行、回放 |
| 10769–10999 | 渲染辅助、job 行、队列行、通知、`session_replay_frame` |
| 11000–11174 | 排队提交持久化、raw mode |

### `repl/input.rs`

| 原位置 | 函数 |
|---|---|
| 11425–12066 | hangup watchdog、`read_live_repl_input`、`follow_wake_run`、`handle_live_agent_event`、`run_live_agent_turn` |
| 11233–11424 | jobs feed/poll |

### `repl/variant_menu.rs`

| 原位置 | 函数 |
|---|---|
| 4211–4872 | 通用 inline fuzzy select |
| 8894–9346 | variant 菜单、模型选择 UI |
| 9347–9434 | `split_repl_command`、REPL 历史文件/加载/帮助 |

### `cli/one_shot.rs`

| 原位置 | 函数 |
|---|---|
| 5347–5600 | 一次性会话、临时会话、`run_chat_with_options` |

### `cli/mod.rs`（最终入口）

保留：

- `mod` 声明与 re-export；
- `Cli`/`Command` 的 re-export（兼容 `cli::WebArgs` 旧引用，稳定后逐步替换）；
- `run()` 分发逻辑（1436–1638）。

## 10. 可见性调整规则

- 拆到子模块后，原本同文件私有互调改为 `pub(super)`。
- 跨 `cli` 子模块互调改为 `pub(crate)` 或 `pub(super)`（目录内用 `crate::cli::...`）。
- 禁止新增 `pub` 暴露；`cli` 对外只保留当前已公开的类型。

## 11. 验收

- [ ] `src/cli.rs` 已删除；`src/cli/mod.rs` 及各子文件编译通过。
- [ ] 每个新文件（除 args）< 1500 行，`repl/` 子文件 < 1200 行。
- [ ] `cargo test cli::` 全部通过。
- [ ] `miyu -h`、各子命令 `-h` 与基线逐字一致。
- [ ] REPL 冒烟：进入、`/help`、`/new`、`/session`、`/config`、`/history`、`/pop`、`/compact`、`/reset` 与基线一致。
