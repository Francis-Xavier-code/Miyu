//! v1–v10：建表与几次大改结构。
//!
//! 前十版改动大（会话表、平台绑定、附件、重做检查点、访问控制、流水），每个都
//! 是完整的建表或搬数据。**迁移一旦发布就不能改**——用户的库已经按它执行过了。

use crate::state::migrations::*;

/// v1: idempotent baseline schema. Safe to run on an empty database or on any
/// legacy database created before versioned migrations existed.
pub(in crate::state) fn apply_v1_baseline(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS turns (
            turn_id          TEXT PRIMARY KEY,
            seq              INTEGER NOT NULL UNIQUE,
            user_content     TEXT NOT NULL,
            user_timestamp   TEXT NOT NULL,
            assistant_content TEXT NOT NULL,
            assistant_reasoning TEXT,
            assistant_timestamp TEXT,
            status           TEXT NOT NULL DEFAULT 'running',
            tool_reports     TEXT NOT NULL DEFAULT '[]'
        );
        CREATE INDEX IF NOT EXISTS idx_turns_seq ON turns(seq);
        CREATE INDEX IF NOT EXISTS idx_turns_status ON turns(status);",
    )?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS question_exchanges (
            turn_id         TEXT NOT NULL,
            exchange_index  INTEGER NOT NULL,
            payload         TEXT NOT NULL,
            PRIMARY KEY (turn_id, exchange_index),
            FOREIGN KEY (turn_id) REFERENCES turns(turn_id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_question_exchanges_turn
            ON question_exchanges(turn_id, exchange_index);",
    )?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS image_assets (
            asset_id    TEXT PRIMARY KEY,
            turn_id     TEXT NOT NULL,
            tool_id     TEXT,
            mime        TEXT NOT NULL,
            width       INTEGER NOT NULL DEFAULT 0,
            height      INTEGER NOT NULL DEFAULT 0,
            alt         TEXT NOT NULL DEFAULT '',
            data        BLOB NOT NULL,
            created_at  TEXT NOT NULL,
            FOREIGN KEY (turn_id) REFERENCES turns(turn_id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_image_assets_turn
            ON image_assets(turn_id, created_at, asset_id);",
    )?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS queued_prompts (
            seq                         INTEGER PRIMARY KEY AUTOINCREMENT,
            prompt_id                   TEXT NOT NULL UNIQUE,
            content                     TEXT NOT NULL,
            display_content             TEXT NOT NULL,
            attachments                 TEXT NOT NULL DEFAULT '[]',
            status                      TEXT NOT NULL DEFAULT 'queued',
            submitted_at                TEXT NOT NULL,
            queue_session_id             TEXT,
            owner_pid                    INTEGER,
            consumed_at                 TEXT,
            turn_id                     TEXT,
            context_content              TEXT,
            preceding_assistant_content  TEXT,
            preceding_assistant_reasoning TEXT,
            FOREIGN KEY (turn_id) REFERENCES turns(turn_id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_queued_prompts_status_seq
            ON queued_prompts(status, seq);
        CREATE INDEX IF NOT EXISTS idx_queued_prompts_turn_seq
            ON queued_prompts(turn_id, seq);",
    )?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS session_loaded_items (
            kind            TEXT NOT NULL,
            name            TEXT NOT NULL,
            source_turn_id  TEXT,
            created_at      TEXT NOT NULL,
            updated_at      TEXT NOT NULL,
            PRIMARY KEY (kind, name)
        );
        CREATE INDEX IF NOT EXISTS idx_session_loaded_items_source_turn
            ON session_loaded_items(source_turn_id);",
    )?;
    add_column_if_missing(conn, "turns", "hidden", "INTEGER NOT NULL DEFAULT 0")?;
    add_column_if_missing(conn, "turns", "is_summary", "INTEGER NOT NULL DEFAULT 0")?;
    add_column_if_missing(conn, "turns", "owner_pid", "INTEGER")?;
    add_column_if_missing(conn, "turns", "queue_session_id", "TEXT")?;
    add_column_if_missing(conn, "turns", "token_total", "INTEGER NOT NULL DEFAULT 0")?;
    add_column_if_missing(
        conn,
        "turns",
        "token_usage_estimated",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_missing(
        conn,
        "turns",
        "compact_reversible",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_missing(conn, "turns", "compact_parent_summary_seq", "INTEGER")?;
    add_column_if_missing(conn, "turns", "assistant_provider_id", "TEXT")?;
    add_column_if_missing(conn, "turns", "assistant_model", "TEXT")?;
    add_column_if_missing(conn, "queued_prompts", "queue_session_id", "TEXT")?;
    add_column_if_missing(conn, "queued_prompts", "owner_pid", "INTEGER")?;
    add_column_if_missing(
        conn,
        "queued_prompts",
        "preceding_assistant_provider_id",
        "TEXT",
    )?;
    add_column_if_missing(conn, "queued_prompts", "preceding_assistant_model", "TEXT")?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_turns_visible_seq ON turns(hidden, seq);
         CREATE INDEX IF NOT EXISTS idx_turns_visible_summary_seq
             ON turns(is_summary, hidden, seq);
         CREATE INDEX IF NOT EXISTS idx_queued_prompts_session_status_seq
             ON queued_prompts(queue_session_id, status, seq);",
    )?;
    Ok(())
}

/// Well-known id of the session that pre-session history is migrated into and
/// that fresh databases start with.
pub const DEFAULT_SESSION_ID: &str = "default";

/// v2: introduce the session dimension.
///
/// - `sessions` table (persona-namespaced chat topics; `kind` distinguishes
///   user sessions from subagent audit sessions).
/// - `app_state` key-value table; `current_session` holds the global default
///   session pointer.
/// - `turns` rebuilt: `session_id` column, per-session `UNIQUE(session_id,
///   seq)` replaces the global `seq UNIQUE`, plus a per-turn `workspace`
///   column recording where the turn actually executed.
/// - `session_loaded_items` rebuilt with a `(session_id, kind, name)` key.
/// - `queued_prompts` gains a nullable `session_id`.
///
/// All existing rows are assigned to the default session. The default
/// session's `persona` starts empty; the session manager stamps the active
/// persona scope on first use.
pub(in crate::state) fn apply_v2_sessions(conn: &Connection) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute_batch(
        "CREATE TABLE sessions (
            session_id        TEXT PRIMARY KEY,
            persona           TEXT NOT NULL,
            name              TEXT NOT NULL,
            kind              TEXT NOT NULL DEFAULT 'user',
            parent_session_id TEXT REFERENCES sessions(session_id) ON DELETE CASCADE,
            workspace         TEXT,
            archived          INTEGER NOT NULL DEFAULT 0,
            created_at        TEXT NOT NULL,
            updated_at        TEXT NOT NULL,
            provider_id       TEXT,
            model             TEXT,
            context_window    INTEGER,
            prompt_tokens     INTEGER NOT NULL DEFAULT 0,
            completion_tokens INTEGER NOT NULL DEFAULT 0,
            total_tokens      INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX idx_sessions_persona
            ON sessions(persona, kind, archived, updated_at);
        CREATE INDEX idx_sessions_parent ON sessions(parent_session_id);
        CREATE TABLE app_state (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )?;
    conn.execute(
        "INSERT INTO sessions (session_id, persona, name, kind, created_at, updated_at)
         VALUES (?1, '', ?2, 'user', ?3, ?3)",
        rusqlite::params![
            DEFAULT_SESSION_ID,
            crate::i18n::text("Terminal session", "终端集成会话"),
            now
        ],
    )?;
    conn.execute(
        "INSERT INTO app_state (key, value) VALUES ('current_session', ?1)",
        [DEFAULT_SESSION_ID],
    )?;
    conn.execute_batch(&format!(
        "CREATE TABLE turns_v2 (
            turn_id          TEXT PRIMARY KEY,
            session_id       TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
            seq              INTEGER NOT NULL,
            user_content     TEXT NOT NULL,
            user_timestamp   TEXT NOT NULL,
            assistant_content TEXT NOT NULL,
            assistant_reasoning TEXT,
            assistant_timestamp TEXT,
            status           TEXT NOT NULL DEFAULT 'running',
            tool_reports     TEXT NOT NULL DEFAULT '[]',
            hidden           INTEGER NOT NULL DEFAULT 0,
            is_summary       INTEGER NOT NULL DEFAULT 0,
            owner_pid        INTEGER,
            queue_session_id TEXT,
            token_total      INTEGER NOT NULL DEFAULT 0,
            token_usage_estimated INTEGER NOT NULL DEFAULT 0,
            compact_reversible INTEGER NOT NULL DEFAULT 0,
            compact_parent_summary_seq INTEGER,
            assistant_provider_id TEXT,
            assistant_model  TEXT,
            workspace        TEXT,
            UNIQUE(session_id, seq)
        );
        INSERT INTO turns_v2 (
            turn_id, session_id, seq, user_content, user_timestamp,
            assistant_content, assistant_reasoning, assistant_timestamp,
            status, tool_reports, hidden, is_summary, owner_pid,
            queue_session_id, token_total, token_usage_estimated,
            compact_reversible, compact_parent_summary_seq,
            assistant_provider_id, assistant_model
        )
        SELECT
            turn_id, '{DEFAULT_SESSION_ID}', seq, user_content, user_timestamp,
            assistant_content, assistant_reasoning, assistant_timestamp,
            status, tool_reports, hidden, is_summary, owner_pid,
            queue_session_id, token_total, token_usage_estimated,
            compact_reversible, compact_parent_summary_seq,
            assistant_provider_id, assistant_model
        FROM turns;
        DROP TABLE turns;
        ALTER TABLE turns_v2 RENAME TO turns;
        CREATE INDEX idx_turns_status ON turns(status);
        CREATE INDEX idx_turns_visible_seq ON turns(session_id, hidden, seq);
        CREATE INDEX idx_turns_visible_summary_seq
            ON turns(session_id, is_summary, hidden, seq);"
    ))?;
    conn.execute_batch(&format!(
        "CREATE TABLE session_loaded_items_v2 (
            session_id      TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
            kind            TEXT NOT NULL,
            name            TEXT NOT NULL,
            source_turn_id  TEXT,
            created_at      TEXT NOT NULL,
            updated_at      TEXT NOT NULL,
            PRIMARY KEY (session_id, kind, name)
        );
        INSERT INTO session_loaded_items_v2 (
            session_id, kind, name, source_turn_id, created_at, updated_at
        )
        SELECT '{DEFAULT_SESSION_ID}', kind, name, source_turn_id, created_at, updated_at
        FROM session_loaded_items;
        DROP TABLE session_loaded_items;
        ALTER TABLE session_loaded_items_v2 RENAME TO session_loaded_items;
        CREATE INDEX idx_session_loaded_items_source_turn
            ON session_loaded_items(source_turn_id);"
    ))?;
    add_column_if_missing(conn, "queued_prompts", "session_id", "TEXT")?;
    conn.execute(
        "UPDATE queued_prompts SET session_id = ?1 WHERE session_id IS NULL",
        [DEFAULT_SESSION_ID],
    )?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_queued_prompts_session
            ON queued_prompts(session_id, status, seq);",
    )?;
    Ok(())
}

/// v3: stable platform-conversation bindings and platform plugin state.
///
/// Platform session bindings include the persona and optional participant so
/// chat history can remain isolated, while plugin state deliberately excludes
/// both dimensions and is shared by every persona in the external conversation.
pub(in crate::state) fn apply_v3_platform_sessions_and_plugin_state(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE platform_session_bindings (
            platform          TEXT NOT NULL,
            account_id        TEXT NOT NULL,
            conversation_kind TEXT NOT NULL,
            conversation_id   TEXT NOT NULL,
            participant_id    TEXT NOT NULL DEFAULT '',
            persona           TEXT NOT NULL,
            session_id        TEXT NOT NULL UNIQUE
                              REFERENCES sessions(session_id) ON DELETE CASCADE,
            created_at        TEXT NOT NULL,
            updated_at        TEXT NOT NULL,
            PRIMARY KEY (
                platform, account_id, conversation_kind, conversation_id,
                participant_id, persona
            )
        );

        CREATE TABLE platform_plugin_kv (
            plugin_id         TEXT NOT NULL,
            platform          TEXT NOT NULL,
            account_id        TEXT NOT NULL,
            conversation_kind TEXT NOT NULL,
            conversation_id   TEXT NOT NULL,
            key               TEXT NOT NULL,
            value_json        TEXT NOT NULL,
            updated_at        TEXT NOT NULL,
            PRIMARY KEY (
                plugin_id, platform, account_id, conversation_kind,
                conversation_id, key
            )
        );",
    )?;
    Ok(())
}

/// v4: persistent links between platform messages and meme-library entries.
pub(in crate::state) fn apply_v4_platform_meme_refs(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE platform_meme_refs (
            platform          TEXT NOT NULL,
            account_id        TEXT NOT NULL,
            conversation_kind TEXT NOT NULL,
            conversation_id   TEXT NOT NULL,
            message_id        TEXT NOT NULL,
            library           TEXT NOT NULL,
            meme_id           TEXT NOT NULL,
            direction         TEXT NOT NULL CHECK (direction IN ('inbound', 'outbound')),
            created_at        TEXT NOT NULL,
            PRIMARY KEY (
                platform, account_id, conversation_kind, conversation_id,
                message_id, library, meme_id
            )
        );
        CREATE INDEX idx_platform_meme_refs_meme
            ON platform_meme_refs(library, meme_id);",
    )?;
    Ok(())
}

/// v5: durable WebUI user attachments and separate user-visible turn text.
pub(in crate::state) fn apply_v5_user_attachments(conn: &Connection) -> Result<()> {
    add_column_if_missing(conn, "turns", "display_content", "TEXT NOT NULL DEFAULT ''")?;
    conn.execute(
        "UPDATE turns SET display_content = user_content WHERE display_content = ''",
        [],
    )?;
    conn.execute_batch(
        "CREATE TABLE user_attachments (
            attachment_id TEXT PRIMARY KEY,
            session_id    TEXT NOT NULL REFERENCES sessions(session_id) ON DELETE CASCADE,
            turn_id       TEXT REFERENCES turns(turn_id) ON DELETE CASCADE,
            prompt_id     TEXT REFERENCES queued_prompts(prompt_id) ON DELETE CASCADE,
            run_id        TEXT,
            file_name     TEXT NOT NULL,
            mime          TEXT NOT NULL,
            kind          TEXT NOT NULL CHECK (kind IN ('image', 'text')),
            size_bytes    INTEGER NOT NULL CHECK (size_bytes >= 0),
            width         INTEGER NOT NULL DEFAULT 0,
            height        INTEGER NOT NULL DEFAULT 0,
            data          BLOB NOT NULL,
            created_at    TEXT NOT NULL,
            CHECK (
                (turn_id IS NOT NULL) + (prompt_id IS NOT NULL) + (run_id IS NOT NULL) <= 1
            )
        );
        CREATE INDEX idx_user_attachments_session
            ON user_attachments(session_id, created_at, attachment_id);
        CREATE INDEX idx_user_attachments_turn
            ON user_attachments(turn_id, created_at, attachment_id);
        CREATE INDEX idx_user_attachments_prompt
            ON user_attachments(prompt_id, created_at, attachment_id);
        CREATE INDEX idx_user_attachments_run ON user_attachments(run_id);",
    )?;
    Ok(())
}

/// v6: optimistic turn revisions and a bounded replay checkpoint for redoing
/// the last consumed follow-up batch without storing the full conversation.
pub(in crate::state) fn apply_v6_turn_redo_checkpoints(conn: &Connection) -> Result<()> {
    add_column_if_missing(conn, "turns", "revision", "INTEGER NOT NULL DEFAULT 0")?;
    conn.execute_batch(
        "CREATE TABLE turn_redo_checkpoints (
            turn_id          TEXT PRIMARY KEY REFERENCES turns(turn_id) ON DELETE CASCADE,
            version          INTEGER NOT NULL,
            batch_prompt_ids TEXT NOT NULL,
            payload          BLOB,
            unavailable_reason TEXT,
            created_at       TEXT NOT NULL,
            CHECK ((payload IS NULL) != (unavailable_reason IS NULL))
        );",
    )?;
    create_turn_redo_backup_tables(conn)?;
    Ok(())
}

/// v7 repairs databases that reached v6 before redo failure backups were
/// introduced. Fresh databases already have these tables through v6.
pub(in crate::state) fn apply_v7_turn_redo_backups(conn: &Connection) -> Result<()> {
    create_turn_redo_backup_tables(conn)
}

pub(in crate::state) fn apply_v8_artifact_assets(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS artifact_assets (
            asset_id    TEXT PRIMARY KEY,
            turn_id     TEXT NOT NULL REFERENCES turns(turn_id) ON DELETE CASCADE,
            tool_id     TEXT,
            source_key  TEXT NOT NULL,
            file_name   TEXT NOT NULL,
            mime        TEXT NOT NULL,
            kind        TEXT NOT NULL,
            size_bytes  INTEGER NOT NULL,
            data        BLOB NOT NULL,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            UNIQUE(turn_id, source_key)
        );
        CREATE INDEX IF NOT EXISTS idx_artifact_assets_turn
            ON artifact_assets(turn_id, updated_at, asset_id);",
    )?;
    Ok(())
}

/// v9: durable platform access grants and an append-only audit trail.
///
/// The account scope is deliberately separate from the platform account id:
/// `*` represents a grant shared by every account on a platform, while a
/// concrete id leaves room for narrower policies later without changing the
/// schema. Miyu currently writes only the global scope for QQ.
pub(in crate::state) fn apply_v9_platform_access_control(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS platform_access_grants (
            platform                    TEXT NOT NULL,
            account_scope               TEXT NOT NULL,
            permission                  TEXT NOT NULL,
            subject_kind                TEXT NOT NULL,
            subject_id                  TEXT NOT NULL,
            granted_by_platform         TEXT NOT NULL,
            granted_by_account_id       TEXT NOT NULL,
            granted_by_user_id          TEXT NOT NULL,
            granted_conversation_kind  TEXT NOT NULL,
            granted_conversation_id    TEXT NOT NULL,
            granted_message_id         TEXT NOT NULL,
            created_at                  TEXT NOT NULL,
            PRIMARY KEY (
                platform, account_scope, permission, subject_kind, subject_id
            )
        );

        CREATE TABLE IF NOT EXISTS platform_access_audit (
            audit_id                   TEXT PRIMARY KEY,
            operation                  TEXT NOT NULL,
            platform                   TEXT NOT NULL,
            account_scope              TEXT NOT NULL,
            permission                 TEXT NOT NULL,
            subject_kind               TEXT NOT NULL,
            subject_id                 TEXT NOT NULL,
            actor_platform             TEXT NOT NULL,
            actor_account_id           TEXT NOT NULL,
            actor_user_id              TEXT NOT NULL,
            actor_conversation_kind    TEXT NOT NULL,
            actor_conversation_id      TEXT NOT NULL,
            actor_message_id           TEXT NOT NULL,
            created_at                 TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_platform_access_audit_target
            ON platform_access_audit(
                platform, account_scope, permission, subject_kind, subject_id,
                created_at
            );",
    )?;
    Ok(())
}

/// v10: append-only semantic events for streamed turn recovery.
///
/// The existing columns on `turns` remain the compatibility projection used
/// by completed conversations. These tables are the durable source for a
/// running/interrupted generation, so a partial response never requires
/// rewriting an ever-growing JSON value.
pub(in crate::state) fn apply_v10_turn_generation_journal(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS turn_journal_segments (
            turn_id       TEXT NOT NULL REFERENCES turns(turn_id) ON DELETE CASCADE,
            revision      INTEGER NOT NULL,
            segment_index INTEGER NOT NULL,
            status        TEXT NOT NULL DEFAULT 'running'
                          CHECK (status IN ('running', 'completed', 'interrupted', 'superseded')),
            started_at    TEXT NOT NULL,
            finished_at   TEXT,
            PRIMARY KEY (turn_id, revision, segment_index)
        );
        CREATE INDEX IF NOT EXISTS idx_turn_journal_segments_active
            ON turn_journal_segments(turn_id, revision, status, segment_index);

        CREATE TABLE IF NOT EXISTS turn_journal_events (
            event_id      INTEGER PRIMARY KEY,
            turn_id       TEXT NOT NULL,
            revision      INTEGER NOT NULL,
            segment_index INTEGER NOT NULL,
            kind          TEXT NOT NULL,
            call_id       TEXT,
            name          TEXT,
            text_payload  TEXT,
            blob_payload  BLOB,
            ok            INTEGER,
            created_at    TEXT NOT NULL,
            FOREIGN KEY (turn_id, revision, segment_index)
                REFERENCES turn_journal_segments(turn_id, revision, segment_index)
                ON DELETE CASCADE,
            CHECK (text_payload IS NOT NULL OR blob_payload IS NOT NULL OR kind IN (
                'reasoning_start', 'reasoning_reset', 'reasoning_part_start',
                'reasoning_part_end', 'generation_superseded'
            ))
        );
        CREATE INDEX IF NOT EXISTS idx_turn_journal_events_order
            ON turn_journal_events(turn_id, revision, segment_index, event_id);

        CREATE TABLE IF NOT EXISTS turn_redo_artifact_backups (
            turn_id    TEXT NOT NULL REFERENCES turn_redo_backups(turn_id) ON DELETE CASCADE,
            asset_id   TEXT NOT NULL,
            tool_id    TEXT,
            source_key TEXT NOT NULL,
            file_name  TEXT NOT NULL,
            mime       TEXT NOT NULL,
            kind       TEXT NOT NULL,
            size_bytes INTEGER NOT NULL,
            data       BLOB NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (turn_id, asset_id)
        );

        INSERT OR IGNORE INTO turn_journal_segments
            (turn_id, revision, segment_index, status, started_at)
        SELECT turn_id, revision, 0,
               CASE status WHEN 'completed' THEN 'completed'
                           WHEN 'interrupted' THEN 'interrupted'
                           ELSE 'running' END,
               COALESCE(user_timestamp, datetime('now'))
        FROM turns
        WHERE status IN ('running', 'interrupted');",
    )?;
    Ok(())
}

pub(in crate::state) fn create_turn_redo_backup_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS turn_redo_backups (
            turn_id    TEXT PRIMARY KEY REFERENCES turns(turn_id) ON DELETE CASCADE,
            revision   INTEGER NOT NULL,
            payload    BLOB NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS turn_redo_question_backups (
            turn_id        TEXT NOT NULL REFERENCES turn_redo_backups(turn_id) ON DELETE CASCADE,
            exchange_index INTEGER NOT NULL,
            payload        TEXT NOT NULL,
            PRIMARY KEY (turn_id, exchange_index)
        );
        CREATE TABLE IF NOT EXISTS turn_redo_image_backups (
            turn_id   TEXT NOT NULL REFERENCES turn_redo_backups(turn_id) ON DELETE CASCADE,
            asset_id  TEXT NOT NULL,
            tool_id   TEXT,
            mime      TEXT NOT NULL,
            width     INTEGER NOT NULL,
            height    INTEGER NOT NULL,
            alt       TEXT NOT NULL,
            data      BLOB NOT NULL,
            created_at TEXT NOT NULL,
            PRIMARY KEY (turn_id, asset_id)
        );",
    )?;
    Ok(())
}
