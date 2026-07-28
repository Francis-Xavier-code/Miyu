//! Versioned schema migrations for conversation.db.
//!
//! Uses `PRAGMA user_version` to track the applied schema version. Each
//! migration runs inside an immediate transaction; the version is bumped in
//! the same transaction so a crash mid-migration leaves the database at the
//! previous version and the migration re-runs on next open.
//!
//! Version 1 is the idempotent baseline: it absorbs the historical
//! `CREATE TABLE IF NOT EXISTS` + `add_column_if_missing` logic so that any
//! legacy database (at any historical column state) converges to the same
//! schema. Later migrations may assume the baseline and use destructive
//! operations such as table rebuilds.

use anyhow::{bail, Context, Result};
use rusqlite::{Connection, TransactionBehavior};

struct Migration {
    version: i64,
    name: &'static str,
    apply: fn(&Connection) -> Result<()>,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "baseline",
        apply: apply_v1_baseline,
    },
    Migration {
        version: 2,
        name: "sessions",
        apply: apply_v2_sessions,
    },
    Migration {
        version: 3,
        name: "platform_sessions_and_plugin_state",
        apply: apply_v3_platform_sessions_and_plugin_state,
    },
];

/// Latest schema version this build produces.
pub const LATEST_VERSION: i64 = 3;

/// Returns the schema version currently recorded in the database.
pub fn current_version(conn: &Connection) -> Result<i64> {
    user_version(conn)
}

/// Runs all pending migrations. Called from `ConversationDb::open` while the
/// connection is still exclusively owned by the caller.
///
/// Foreign-key enforcement is disabled for the duration: table rebuilds drop
/// and recreate parent tables, and with enforcement on the implicit
/// `DELETE FROM` of `DROP TABLE` would cascade into child tables. Integrity is
/// re-checked with `foreign_key_check` inside each migration's transaction.
pub fn run_migrations(conn: &mut Connection) -> Result<()> {
    let current = user_version(conn)?;
    let latest = MIGRATIONS.last().map(|m| m.version).unwrap_or(0);
    if current > latest {
        bail!(
            "conversation.db schema version {current} is newer than this build supports ({latest}); refusing to open"
        );
    }
    if current == latest {
        return Ok(());
    }
    conn.pragma_update(None, "foreign_keys", false)?;
    let result = apply_pending(conn, current);
    let restore = conn.pragma_update(None, "foreign_keys", true);
    result?;
    restore?;
    Ok(())
}

fn apply_pending(conn: &mut Connection, current: i64) -> Result<()> {
    for migration in MIGRATIONS.iter().filter(|m| m.version > current) {
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .with_context(|| format!("failed to begin migration '{}'", migration.name))?;
        (migration.apply)(&tx)
            .with_context(|| format!("schema migration '{}' failed", migration.name))?;
        let violations: i64 =
            tx.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })?;
        if violations > 0 {
            bail!(
                "schema migration '{}' left {violations} foreign-key violations; rolling back",
                migration.name
            );
        }
        tx.pragma_update(None, "user_version", migration.version)?;
        tx.commit()
            .with_context(|| format!("failed to commit migration '{}'", migration.name))?;
    }
    Ok(())
}

fn user_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

/// v1: idempotent baseline schema. Safe to run on an empty database or on any
/// legacy database created before versioned migrations existed.
fn apply_v1_baseline(conn: &Connection) -> Result<()> {
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
fn apply_v2_sessions(conn: &Connection) -> Result<()> {
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
            crate::i18n::text("Default session", "默认会话"),
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
fn apply_v3_platform_sessions_and_plugin_state(conn: &Connection) -> Result<()> {
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

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for row in rows {
        if row? == column {
            return Ok(());
        }
    }
    conn.execute(
        &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
        [],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_migrated() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        run_migrations(&mut conn).unwrap();
        conn
    }

    #[test]
    fn fresh_database_migrates_to_latest_version() {
        let conn = open_migrated();
        let version = user_version(&conn).unwrap();
        assert_eq!(version, MIGRATIONS.last().unwrap().version);
    }

    #[test]
    fn migrations_are_idempotent_on_reopen() {
        let mut conn = open_migrated();
        // A second run must be a no-op.
        run_migrations(&mut conn).unwrap();
        assert_eq!(
            user_version(&conn).unwrap(),
            MIGRATIONS.last().unwrap().version
        );
    }

    #[test]
    fn baseline_converges_legacy_database() {
        // Simulate a legacy pre-versioning database: base turns table without
        // the later ALTER-added columns and user_version 0.
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE turns (
                turn_id          TEXT PRIMARY KEY,
                seq              INTEGER NOT NULL UNIQUE,
                user_content     TEXT NOT NULL,
                user_timestamp   TEXT NOT NULL,
                assistant_content TEXT NOT NULL,
                assistant_reasoning TEXT,
                assistant_timestamp TEXT,
                status           TEXT NOT NULL DEFAULT 'running',
                tool_reports     TEXT NOT NULL DEFAULT '[]'
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO turns (turn_id, seq, user_content, user_timestamp, assistant_content)
             VALUES ('t1', 1, 'hi', 'now', 'hello')",
            [],
        )
        .unwrap();
        run_migrations(&mut conn).unwrap();
        // Legacy row survives and the ALTER-added columns exist with defaults.
        let (hidden, model): (i64, Option<String>) = conn
            .query_row(
                "SELECT hidden, assistant_model FROM turns WHERE turn_id = 't1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(hidden, 0);
        assert_eq!(model, None);
    }

    #[test]
    fn newer_database_version_is_refused() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", 9999).unwrap();
        let err = run_migrations(&mut conn).unwrap_err();
        assert!(err.to_string().contains("newer"));
    }

    #[test]
    fn v2_moves_existing_history_into_the_default_session() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        // Build a v1 database with a turn and a dependent child row.
        conn.pragma_update(None, "user_version", 0).unwrap();
        apply_v1_baseline(&conn).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute(
            "INSERT INTO turns (turn_id, seq, user_content, user_timestamp, assistant_content)
             VALUES ('t1', 7, 'hi', 'now', 'hello')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO question_exchanges (turn_id, exchange_index, payload)
             VALUES ('t1', 0, '{}')",
            [],
        )
        .unwrap();
        run_migrations(&mut conn).unwrap();

        let (session_id, seq): (String, i64) = conn
            .query_row(
                "SELECT session_id, seq FROM turns WHERE turn_id = 't1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(session_id, DEFAULT_SESSION_ID);
        assert_eq!(seq, 7);
        // The FK-off rebuild must not cascade-delete child rows.
        let exchanges: i64 = conn
            .query_row("SELECT count(*) FROM question_exchanges", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(exchanges, 1);
        let current: String = conn
            .query_row(
                "SELECT value FROM app_state WHERE key = 'current_session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(current, DEFAULT_SESSION_ID);
        // Per-session seq uniqueness: same seq in another session is fine.
        conn.execute(
            "INSERT INTO sessions (session_id, persona, name, created_at, updated_at)
             VALUES ('other', '', 'x', 'now', 'now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO turns (turn_id, session_id, seq, user_content, user_timestamp, assistant_content)
             VALUES ('t2', 'other', 7, 'hi', 'now', '')",
            [],
        )
        .unwrap();
        // …but duplicated seq within one session is rejected.
        assert!(conn
            .execute(
                "INSERT INTO turns (turn_id, session_id, seq, user_content, user_timestamp, assistant_content)
                 VALUES ('t3', 'other', 7, 'hi', 'now', '')",
                [],
            )
            .is_err());
    }

    #[test]
    fn v3_platform_tables_enforce_uniqueness_and_session_cascade() {
        let conn = open_migrated();
        assert_eq!(user_version(&conn).unwrap(), 3);
        conn.execute(
            "INSERT INTO sessions (session_id, persona, name, created_at, updated_at)
             VALUES ('platform-session', 'miyu', 'platform', 'now', 'now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO platform_session_bindings (
                platform, account_id, conversation_kind, conversation_id,
                persona, session_id, created_at, updated_at
             ) VALUES ('onebot', '10000', 'private', '20000', 'miyu',
                       'platform-session', 'now', 'now')",
            [],
        )
        .unwrap();

        // A session cannot be attached to a second external identity.
        assert!(conn
            .execute(
                "INSERT INTO platform_session_bindings (
                    platform, account_id, conversation_kind, conversation_id,
                    persona, session_id, created_at, updated_at
                 ) VALUES ('onebot', '10000', 'private', 'other', 'miyu',
                           'platform-session', 'now', 'now')",
                [],
            )
            .is_err());
        conn.execute(
            "INSERT INTO platform_plugin_kv (
                plugin_id, platform, account_id, conversation_kind,
                conversation_id, key, value_json, updated_at
             ) VALUES ('reply_processor', 'onebot', '10000', 'private',
                       '20000', 'recent_images', '[]', 'now')",
            [],
        )
        .unwrap();

        conn.execute(
            "DELETE FROM sessions WHERE session_id = 'platform-session'",
            [],
        )
        .unwrap();
        let binding_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM platform_session_bindings",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let plugin_count: i64 = conn
            .query_row("SELECT count(*) FROM platform_plugin_kv", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(binding_count, 0);
        // Plugin state is scoped to the external conversation, not a session.
        assert_eq!(plugin_count, 1);
    }
}
