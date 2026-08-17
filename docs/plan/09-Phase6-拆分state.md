# 09 · Phase 6：拆分 `state/`

`state/conversation_db.rs` 5.9k 行混合了全部数据访问；`state/mod.rs` 5.1k 行里包含大量测试。本阶段按“数据聚合”拆分。

## 1. `src/state/db/schema.rs`

搬入：

- 所有表名/列名常量；
- `SESSION_COLUMNS` 与 turn 固定列序定义；
- `map_turn_row` 及列位置解析；
- `PENDING_PLACEHOLDER`、`INTERRUPTED_TEXT`、journal/replay 常量。

**注意**：turn 的固定 22 列 SELECT 是数据库兼容性的关键，必须与迁移版本同步锁定，并加注释禁止重排。

## 2. `src/state/db/sessions.rs`

搬入会话相关 SQL/方法：

- `resolve_current_session`、`set_current_session`、persona 指针；
- `list_sessions`、`list_local_sessions`；
- `create_session`、`create_or_get_platform_session`；
- `rename/delete/touch`、会话模型覆盖、会话 token 汇总；
- subagent/ask 隐藏会话与清理。

## 3. `src/state/db/turns.rs`

搬入回合相关：

- `start_turn`、`complete_turn_with_usage`、`complete_turn_revision_with_usage`；
- `interrupt_turn`、`interrupt_turn_revision`、中断投影；
- `load_visible_turns`、history/visible 查询；
- `undo_last_turn`、`archive_and_delete_visible_turns`、`replace_visible_with_summary`；
- stale turn 恢复、僵尸队列清理。

## 4. `src/state/db/queue.rs`

搬入 `queued_prompts` 全部操作：

- `enqueue_prompt`、`load_queued_prompts`、`consume_*`、`discard/delete`；
- 跨进程运行中回合靶子 `running_turn_queue_target`；
- 排队 checkpoint。

## 5. `src/state/db/assets.rs`

搬入：

- `user_attachments` 生命周期（插入、保留、释放、清理）；
- `image_assets` 保存/加载；
- `artifact_assets` upsert/加载/删除；
- `question_exchanges` 追加/读取。

## 6. `src/state/db/platform.rs`

搬入：

- `platform_session_bindings`；
- `platform_plugin_kv`；
- `platform_meme_refs`；
- `platform_access_grants/audit`；
- 平台会话重置与授权缓存联动。

## 7. `src/state/db/journal.rs`

搬入：

- `turn_journal_segments`、`turn_journal_events`；
- `append_turn_journal_event`、`supersede_turn_journal_segment`；
- journal 投影与恢复装配。

## 8. `src/state/db/redo.rs`

搬入：

- redo checkpoint、redo backups（turns/questions/images/artifacts）；
- `redo_candidate`、`begin_redo`、`restore_redo_backup_locked`；
- redo 完成路径。

## 9. `src/state/db/replay.rs`

搬入：

- `store_replay_journal`、`session_replay`；
- `ReplayEntry`、`TurnReplay` 类型与格式化。

## 10. `src/state/db/mod.rs`

`ConversationDb` 只保留：

- 连接管理、PRAGMA、打开/关闭；
- 各子模块方法的一层转发；
- 跨子模块事务辅助函数（如 `next_seq_locked`、`bump_completion_seq_locked` 若无法归入 turns）。

## 11. `src/state/mod.rs`

`StateStore` 门面继续对外提供当前所有方法；内部调用 `self.conv_db.xxx()`。原测试按主题搬入 `src/state/tests/` 或各 `db` 子模块测试。

## 12. 迁移文件保持独立

`migrations.rs` 不拆分；它是历史记录，**禁止重排、合并或改写任何已发布迁移**。只允许新增 v25+ 迁移。

## 13. 验收

- [ ] `cargo test state::` 全绿。
- [ ] 旧数据库迁移测试：v1 → 当前最新版本逐版升级通过；用基线导出的真实 DB 做打开/读写冒烟。
- [ ] 会话/队列/redo/平台绑定行为与基线一致。
- [ ] 文件规模：`conversation_db` 消失，各 `db/*.rs` < 1200 行，`state/mod.rs` < 1500 行。
