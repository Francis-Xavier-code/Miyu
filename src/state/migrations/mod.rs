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

mod baseline;
mod columns;
pub use baseline::DEFAULT_SESSION_ID;
use baseline::*;
use columns::*;

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
    Migration {
        version: 4,
        name: "platform_meme_refs",
        apply: apply_v4_platform_meme_refs,
    },
    Migration {
        version: 5,
        name: "user_attachments",
        apply: apply_v5_user_attachments,
    },
    Migration {
        version: 6,
        name: "turn_redo_checkpoints",
        apply: apply_v6_turn_redo_checkpoints,
    },
    Migration {
        version: 7,
        name: "turn_redo_backups",
        apply: apply_v7_turn_redo_backups,
    },
    Migration {
        version: 8,
        name: "artifact_assets",
        apply: apply_v8_artifact_assets,
    },
    Migration {
        version: 9,
        name: "platform_access_control",
        apply: apply_v9_platform_access_control,
    },
    Migration {
        version: 10,
        name: "turn_generation_journal",
        apply: apply_v10_turn_generation_journal,
    },
    Migration {
        version: 11,
        name: "session_model_override",
        apply: apply_v11_session_model_override,
    },
    Migration {
        version: 12,
        name: "turn_context_messages",
        apply: apply_v12_turn_context_messages,
    },
    Migration {
        version: 13,
        name: "compact_hidden_turns",
        apply: apply_v13_compact_hidden_turns,
    },
    Migration {
        version: 14,
        name: "tool_reports_archive",
        apply: apply_v14_tool_reports_archive,
    },
    Migration {
        version: 15,
        name: "session_last_request_at",
        apply: apply_v15_session_last_request_at,
    },
    Migration {
        version: 16,
        name: "turn_tool_footprint",
        apply: apply_v16_turn_tool_footprint,
    },
    Migration {
        version: 17,
        name: "turn_replay_journal",
        apply: apply_v17_turn_replay_journal,
    },
    Migration {
        version: 18,
        name: "turn_cache_tokens",
        apply: apply_v18_turn_cache_tokens,
    },
    Migration {
        version: 19,
        name: "session_cache_tokens",
        apply: apply_v19_session_cache_tokens,
    },
    Migration {
        version: 20,
        name: "turn_tool_flow",
        apply: apply_v20_turn_tool_flow,
    },
    Migration {
        version: 21,
        name: "rename_default_session",
        apply: apply_v21_rename_default_session,
    },
    Migration {
        version: 22,
        name: "session_goals",
        apply: apply_v22_session_goals,
    },
    Migration {
        version: 23,
        name: "retire_session_archiving",
        apply: apply_v23_retire_session_archiving,
    },
    Migration {
        version: 24,
        name: "retire_session_goals",
        apply: apply_v24_retire_session_goals,
    },
];

/// Latest schema version this build produces.
pub const LATEST_VERSION: i64 = 24;

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
    fn v7_repairs_v6_database_missing_redo_backup_tables() {
        let mut conn = open_migrated();
        conn.execute_batch(
            "DROP TABLE turn_redo_image_backups;
             DROP TABLE turn_redo_question_backups;
             DROP TABLE turn_redo_backups;
             PRAGMA user_version = 6;",
        )
        .unwrap();

        run_migrations(&mut conn).unwrap();

        assert_eq!(user_version(&conn).unwrap(), LATEST_VERSION);
        for table in [
            "turn_redo_checkpoints",
            "turn_redo_backups",
            "turn_redo_question_backups",
            "turn_redo_image_backups",
        ] {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists, "missing repaired table: {table}");
        }
        let violations: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(violations, 0);
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
        assert_eq!(user_version(&conn).unwrap(), LATEST_VERSION);
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

    #[test]
    fn v4_platform_meme_refs_enforce_identity_and_direction() {
        let conn = open_migrated();
        assert_eq!(user_version(&conn).unwrap(), LATEST_VERSION);
        conn.execute(
            "INSERT INTO platform_meme_refs (
                platform, account_id, conversation_kind, conversation_id,
                message_id, library, meme_id, direction, created_at
             ) VALUES ('onebot', '10000', 'group', '20000', 'message-1',
                       'default', 'meme-1', 'inbound', 'now')",
            [],
        )
        .unwrap();

        assert!(conn
            .execute(
                "INSERT INTO platform_meme_refs (
                    platform, account_id, conversation_kind, conversation_id,
                    message_id, library, meme_id, direction, created_at
                 ) VALUES ('onebot', '10000', 'group', '20000', 'message-1',
                           'default', 'meme-1', 'inbound', 'later')",
                [],
            )
            .is_err());
        assert!(conn
            .execute(
                "INSERT INTO platform_meme_refs (
                    platform, account_id, conversation_kind, conversation_id,
                    message_id, library, meme_id, direction, created_at
                 ) VALUES ('onebot', '10000', 'group', '20000', 'message-2',
                           'default', 'meme-1', 'sideways', 'now')",
                [],
            )
            .is_err());
    }

    #[test]
    fn v4_migrates_an_existing_v3_database_without_losing_platform_state() {
        let mut conn = Connection::open_in_memory().unwrap();
        apply_v1_baseline(&conn).unwrap();
        apply_v2_sessions(&conn).unwrap();
        apply_v3_platform_sessions_and_plugin_state(&conn).unwrap();
        conn.pragma_update(None, "user_version", 3).unwrap();
        conn.execute(
            "INSERT INTO platform_plugin_kv (
                plugin_id, platform, account_id, conversation_kind,
                conversation_id, key, value_json, updated_at
             ) VALUES ('reply_processor', 'onebot', '10000', 'group',
                       '20000', 'recent_images', '[]', 'now')",
            [],
        )
        .unwrap();

        run_migrations(&mut conn).unwrap();

        assert_eq!(user_version(&conn).unwrap(), LATEST_VERSION);
        let plugin_rows: i64 = conn
            .query_row("SELECT count(*) FROM platform_plugin_kv", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(plugin_rows, 1);
        let meme_table_exists: bool = conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_master
                    WHERE type = 'table' AND name = 'platform_meme_refs'
                )",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(meme_table_exists);
    }

    #[test]
    fn v9_creates_platform_access_and_audit_tables() {
        let conn = open_migrated();
        assert_eq!(user_version(&conn).unwrap(), LATEST_VERSION);
        for table in ["platform_access_grants", "platform_access_audit"] {
            let exists: bool = conn
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM sqlite_master
                        WHERE type = 'table' AND name = ?1
                    )",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists, "missing access-control table: {table}");
        }
    }
}
