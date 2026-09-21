//! The shape of `platform.db`, and how an older file reaches it.
//!
//! One migration takes a database from version `i` to `i + 1`, inside its own
//! transaction. The list is append-only: a shipped migration is never edited,
//! and a mistake gets a new one. A file that declares a newer version than
//! this binary knows is refused with the path and both numbers, never guessed
//! at. Most changes need no migration at all: a new optional field on a serde
//! struct reads old bodies with `#[serde(default)]`, and that is the default
//! choice. A migration is for a new table, column or index, a field old rows
//! carry under another name, or data computed once for existing rows.

use std::path::Path;

use rusqlite::{Connection, Transaction};

use crate::store::StoreError;

/// The version this binary writes and the highest it reads.
pub const CURRENT: u32 = 1;

type Migration = fn(&Transaction) -> rusqlite::Result<()>;

/// Index `i` upgrades a database at version `i` to version `i + 1`.
const MIGRATIONS: &[Migration] = &[v1_initial];

/// Brings `conn` to [`CURRENT`], one migration and one transaction at a time.
pub fn migrate(conn: &mut Connection, path: &Path) -> Result<(), StoreError> {
    let io = |e: rusqlite::Error| StoreError::Io(path.to_path_buf(), e.to_string());
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);
         INSERT INTO schema_version (version)
             SELECT 0 WHERE NOT EXISTS (SELECT 1 FROM schema_version);",
    )
    .map_err(io)?;
    let found = current_version(conn).map_err(io)?;
    check(path, found)?;
    for (index, step) in MIGRATIONS.iter().enumerate().skip(found as usize) {
        let target = (index + 1) as u32;
        let tx = conn.transaction().map_err(io)?;
        step(&tx).map_err(|e| {
            StoreError::Io(
                path.to_path_buf(),
                format!("schema migration to version {target} failed: {e}"),
            )
        })?;
        tx.execute("UPDATE schema_version SET version = ?1", [target])
            .map_err(io)?;
        tx.commit().map_err(io)?;
    }
    Ok(())
}

/// Refuses a database written by a newer Platform. A reader that cannot
/// migrate calls this on its own.
pub fn check(path: &Path, found: u32) -> Result<(), StoreError> {
    if found > CURRENT {
        return Err(StoreError::UnsupportedVersion {
            path: path.to_path_buf(),
            found,
            supported: CURRENT,
        });
    }
    Ok(())
}

pub fn current_version(conn: &Connection) -> rusqlite::Result<u32> {
    conn.query_row("SELECT version FROM schema_version", [], |row| row.get(0))
}

fn v1_initial(tx: &Transaction) -> rusqlite::Result<()> {
    tx.execute_batch(
        "CREATE TABLE settings (
             key   TEXT PRIMARY KEY,
             value TEXT NOT NULL
         );
         CREATE TABLE api_keys (
             id         TEXT PRIMARY KEY,
             label      TEXT NOT NULL,
             created_at TEXT NOT NULL
         );
         CREATE TABLE records (
             kind       TEXT NOT NULL,
             id         TEXT NOT NULL,
             body       TEXT NOT NULL CHECK (json_valid(body)),
             updated_at TEXT NOT NULL,
             PRIMARY KEY (kind, id)
         );
         CREATE TABLE audit_events (
             id           TEXT PRIMARY KEY,
             status       TEXT NOT NULL,
             subject_kind TEXT NOT NULL,
             subject_id   TEXT NOT NULL,
             occurred_at  TEXT NOT NULL,
             updated_at   TEXT NOT NULL,
             body         TEXT NOT NULL CHECK (json_valid(body))
         );
         CREATE INDEX audit_events_updated_at ON audit_events (updated_at);",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_database_reaches_the_current_version() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn, Path::new("memory")).unwrap();
        assert_eq!(current_version(&conn).unwrap(), CURRENT);
        // Running again is a no-op.
        migrate(&mut conn, Path::new("memory")).unwrap();
        assert_eq!(current_version(&conn).unwrap(), CURRENT);
    }

    #[test]
    fn a_newer_database_is_refused_rather_than_guessed_at() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn, Path::new("memory")).unwrap();
        conn.execute("UPDATE schema_version SET version = ?1", [CURRENT + 1])
            .unwrap();
        let error = migrate(&mut conn, Path::new("memory")).unwrap_err();
        assert!(
            matches!(error, StoreError::UnsupportedVersion { found, supported, .. }
                if found == CURRENT + 1 && supported == CURRENT),
            "{error}"
        );
    }
}
