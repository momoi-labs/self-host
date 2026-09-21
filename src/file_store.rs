//! The Platform's authoritative state, kept in files the Platform owns.
//!
//! `platform.db` is one SQLite file under `state/`: the Platform settings, the
//! Operator's credentials, every collection (Applications, Records, Virtual
//! machines, custom images) and the audit history. A mutation is one
//! transaction, so nothing the Platform owns spans two commit units and there
//! is no multi-file protocol to recover from (ADR-0018, ADR-0027).
//!
//! The Operator's Compose definition is the one thing kept outside the
//! database, verbatim, under a name derived from its digest. It lands first,
//! and only then does the row that references it commit. A Compose file
//! nothing references is work that was interrupted before it counted, and
//! startup collects it.
//!
//! `state/` is authoritative and nothing else is. Rendered Compose projects,
//! the route table and Docker's own view of the world are all rebuildable,
//! and live outside it.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::ErrorReport;
use crate::schema;
use crate::store::{
    ApiKeyRecord, ApplicationRecord, AuditRow, DevelopmentApplication, StateStore, StoreError,
};

pub const DB_FILE: &str = "platform.db";
const LOCK_FILE: &str = "platform.lock";
const APPLICATIONS_DIR: &str = "applications";
/// What the Platform wrote before SQLite. Its presence stops the daemon; a
/// script imports it (ADR-0027).
const LEGACY_PLATFORM_FILE: &str = "platform.json";
const APPLICATION_KIND: &str = "application";

const DIR_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;

/// Where the authoritative state lives, under the Platform configuration
/// directory it shares with generated files.
pub fn state_dir() -> PathBuf {
    crate::paths::platform_config_dir().join("state")
}

/// One Application as its row holds it. `compose_file` names the file next
/// to the row's directory that holds the Operator's Compose definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApplicationBody {
    id: String,
    name: String,
    hostname: String,
    #[serde(default)]
    aliases: Vec<String>,
    image: String,
    status: String,
    source: String,
    #[serde(default)]
    last_error: Option<ErrorReport>,
    #[serde(default)]
    compose_file: Option<String>,
    #[serde(default)]
    web_service: Option<String>,
    #[serde(default)]
    web_port: Option<u16>,
    #[serde(default)]
    web_target_port: Option<u16>,
    #[serde(default)]
    development: Option<DevelopmentApplication>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

struct Inner {
    root: PathBuf,
    db: Mutex<Connection>,
    /// Held for as long as the store is open. Dropping the handle releases the
    /// advisory lock, so a second daemon can take over after this one exits.
    _lock: Option<File>,
}

/// The Platform state, read from and written to `platform.db` under one
/// directory.
///
/// Cloning shares the same open database: the daemon hands copies to Axum
/// handlers and they all serialize behind the same connection.
#[derive(Clone)]
pub struct FileStateStore {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for FileStateStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FileStateStore({})", self.inner.root.display())
    }
}

impl FileStateStore {
    /// Opens the store as the writer. Fails when another process already holds
    /// the directory, rather than letting two daemons interleave writes.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        create_dir(&root)?;
        create_dir(&root.join(APPLICATIONS_DIR))?;
        let lock = acquire_lock(&root)?;
        let db_path = root.join(DB_FILE);
        let mut conn = open_db(&db_path, false)?;
        schema::migrate(&mut conn, &db_path)?;
        // The database exists at the current version before this fires, so
        // the import script has a schema to write into.
        let legacy = root.join(LEGACY_PLATFORM_FILE);
        if legacy.exists() {
            return Err(StoreError::PreviousFormat(legacy));
        }
        collect_orphans(&root, &conn);
        Ok(FileStateStore {
            inner: Arc::new(Inner {
                root,
                db: Mutex::new(conn),
                _lock: Some(lock),
            }),
        })
    }

    /// Opens the store for reading only, without taking the writer lock, so a
    /// CLI command can report what is configured while the daemon is running.
    /// Every mutation on this handle fails. A directory the daemon never
    /// opened reads as an empty Platform.
    pub fn read_only(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        let db_path = root.join(DB_FILE);
        let conn = if db_path.exists() {
            let conn = open_db(&db_path, true)?;
            let found = schema::current_version(&conn).map_err(|e| db_error(&db_path, e))?;
            schema::check(&db_path, found)?;
            conn
        } else {
            let mut conn = Connection::open_in_memory()
                .map_err(|e| StoreError::Io(db_path.clone(), e.to_string()))?;
            schema::migrate(&mut conn, &db_path)?;
            conn
        };
        Ok(FileStateStore {
            inner: Arc::new(Inner {
                root,
                db: Mutex::new(conn),
                _lock: None,
            }),
        })
    }

    fn writable(&self) -> Result<(), StoreError> {
        if self.inner._lock.is_none() {
            return Err(StoreError::ReadOnly);
        }
        Ok(())
    }

    fn db_path(&self) -> PathBuf {
        self.inner.root.join(DB_FILE)
    }

    fn app_dir(&self, id: &str) -> PathBuf {
        self.inner.root.join(APPLICATIONS_DIR).join(id)
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.inner
            .db
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Runs `f` against the connection and reports a database failure with
    /// the file's path.
    fn query<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, StoreError> {
        let conn = self.conn();
        f(&conn).map_err(|e| db_error(&self.db_path(), e))
    }

    /// Runs `f` inside one transaction, committing only when it succeeds.
    fn transaction<T>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.writable()?;
        let mut conn = self.conn();
        let path = self.db_path();
        let tx = conn.transaction().map_err(|e| db_error(&path, e))?;
        let value = f(&tx)?;
        tx.commit().map_err(|e| db_error(&path, e))?;
        Ok(value)
    }

    fn read_application_body(
        &self,
        conn: &Connection,
        id: &str,
    ) -> Result<Option<ApplicationBody>, StoreError> {
        let body: Option<String> = conn
            .query_row(
                "SELECT body FROM records WHERE kind = ?1 AND id = ?2",
                params![APPLICATION_KIND, id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| db_error(&self.db_path(), e))?;
        body.map(|body| self.parse_application(&body)).transpose()
    }

    fn parse_application(&self, body: &str) -> Result<ApplicationBody, StoreError> {
        serde_json::from_str(body)
            .map_err(|e| StoreError::Corrupt(self.db_path(), format!("an Application row: {e}")))
    }

    fn write_application_body(
        &self,
        conn: &Connection,
        body: &ApplicationBody,
    ) -> Result<(), StoreError> {
        let json = serde_json::to_string(body)
            .map_err(|e| StoreError::Serialize(format!("Application {}: {e}", body.id)))?;
        conn.execute(
            "INSERT INTO records (kind, id, body, updated_at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (kind, id) DO UPDATE SET body = excluded.body,
                                                 updated_at = excluded.updated_at",
            params![APPLICATION_KIND, body.id, json, now_iso()],
        )
        .map_err(|e| db_error(&self.db_path(), e))?;
        Ok(())
    }

    /// Turns a row back into the record the rest of the Platform reads,
    /// reading the Compose definition from the file the row names.
    fn record_from(&self, body: ApplicationBody) -> Result<ApplicationRecord, StoreError> {
        let compose = match &body.compose_file {
            Some(name) => {
                let path = self.app_dir(&body.id).join(name);
                Some(std::fs::read_to_string(&path).map_err(|e| {
                    StoreError::Corrupt(
                        self.db_path(),
                        format!(
                            "Application {} names a Compose definition {} that is unreadable: {e}",
                            body.id,
                            path.display()
                        ),
                    )
                })?)
            }
            None => None,
        };
        Ok(ApplicationRecord {
            id: body.id,
            name: body.name,
            hostname: body.hostname,
            aliases: body.aliases,
            image: body.image,
            status: body.status,
            source: body.source,
            last_error: body.last_error,
            compose,
            web_service: body.web_service,
            web_port: body.web_port,
            web_target_port: body.web_target_port,
            development: body.development,
        })
    }

    /// Writes the Compose definition the record carries, if any, before the
    /// row that will reference it. Re-committing the same text writes nothing.
    fn commit_compose(&self, app: &ApplicationRecord) -> Result<Option<String>, StoreError> {
        let Some(compose) = &app.compose else {
            return Ok(None);
        };
        let dir = self.app_dir(&app.id);
        create_dir(&dir)?;
        let name = compose_file_name(compose);
        let path = dir.join(&name);
        if !path.exists() {
            write_atomic(&path, compose.as_bytes())?;
        }
        Ok(Some(name))
    }
}

fn body_from(app: &ApplicationRecord, compose_file: Option<String>) -> ApplicationBody {
    ApplicationBody {
        id: app.id.clone(),
        name: app.name.clone(),
        hostname: app.hostname.clone(),
        aliases: app.aliases.clone(),
        image: app.image.clone(),
        status: app.status.clone(),
        source: app.source.clone(),
        last_error: app.last_error.clone(),
        compose_file,
        web_service: app.web_service.clone(),
        web_port: app.web_port,
        web_target_port: app.web_target_port,
        development: app.development.clone(),
        env: BTreeMap::new(),
    }
}

/// Names a Compose file after what is in it, so re-committing the same
/// definition rewrites nothing and two definitions never collide.
fn compose_file_name(compose: &str) -> String {
    let digest = Sha256::digest(compose.as_bytes());
    let hex: String = digest.iter().take(16).map(|b| format!("{b:02x}")).collect();
    format!("compose-{hex}.yaml")
}

#[async_trait]
impl StateStore for FileStateStore {
    async fn initialize(&self) -> Result<(), StoreError> {
        self.writable()?;
        create_dir(&self.inner.root)?;
        create_dir(&self.inner.root.join(APPLICATIONS_DIR))?;
        Ok(())
    }

    async fn is_initialized(&self) -> Result<bool, StoreError> {
        Ok(self.get_state("dns_suffix").await?.is_some())
    }

    async fn store_state(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.transaction(|tx| {
            tx.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|e| db_error(&self.db_path(), e))?;
            Ok(())
        })
    }

    async fn get_state(&self, key: &str) -> Result<Option<String>, StoreError> {
        self.query(|conn| {
            conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
        })
    }

    async fn insert_application(&self, app: &ApplicationRecord) -> Result<(), StoreError> {
        self.writable()?;
        let compose_file = self.commit_compose(app)?;
        self.transaction(|tx| {
            // PostgreSQL held this as a UNIQUE constraint, and validation
            // upstream races with a second request. The check belongs where
            // the commit is, and the connection serializes it.
            let taken: Option<String> = tx
                .query_row(
                    "SELECT id FROM records
                     WHERE kind = ?1 AND json_extract(body, '$.name') = ?2 AND id <> ?3",
                    params![APPLICATION_KIND, app.name, app.id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| db_error(&self.db_path(), e))?;
            if taken.is_some() {
                return Err(StoreError::AlreadyExists(app.name.clone()));
            }
            let mut body = body_from(app, compose_file);
            // An update keeps the environment already committed for this
            // identity.
            if let Some(existing) = self.read_application_body(tx, &app.id)? {
                body.env = existing.env;
            }
            self.write_application_body(tx, &body)
        })
    }

    async fn get_application(&self, id: &str) -> Result<Option<ApplicationRecord>, StoreError> {
        let body = {
            let conn = self.conn();
            self.read_application_body(&conn, id)?
        };
        body.map(|body| self.record_from(body)).transpose()
    }

    async fn set_application_outcome(
        &self,
        id: &str,
        status: &str,
        error: Option<crate::error::ErrorReport>,
    ) -> Result<ApplicationRecord, StoreError> {
        let body = self.transaction(|tx| {
            let mut body = self
                .read_application_body(tx, id)?
                .ok_or_else(|| StoreError::NotFound(id.into()))?;
            body.status = status.into();
            body.last_error = error;
            self.write_application_body(tx, &body)?;
            Ok(body)
        })?;
        self.record_from(body)
    }

    async fn find_application_by_name(
        &self,
        name: &str,
    ) -> Result<Option<ApplicationRecord>, StoreError> {
        let body: Option<String> = self.query(|conn| {
            conn.query_row(
                "SELECT body FROM records
                 WHERE kind = ?1 AND json_extract(body, '$.name') = ?2",
                params![APPLICATION_KIND, name],
                |row| row.get(0),
            )
            .optional()
        })?;
        body.map(|body| self.record_from(self.parse_application(&body)?))
            .transpose()
    }

    async fn list_applications(&self) -> Result<Vec<ApplicationRecord>, StoreError> {
        let bodies: Vec<String> = self.query(|conn| {
            let mut statement = conn.prepare(
                "SELECT body FROM records WHERE kind = ?1
                 ORDER BY json_extract(body, '$.name')",
            )?;
            let rows = statement.query_map([APPLICATION_KIND], |row| row.get(0))?;
            rows.collect()
        })?;
        bodies
            .iter()
            .map(|body| self.record_from(self.parse_application(body)?))
            .collect()
    }

    async fn delete_application(&self, id: &str) -> Result<(), StoreError> {
        self.transaction(|tx| {
            let removed = tx
                .execute(
                    "DELETE FROM records WHERE kind = ?1 AND id = ?2",
                    params![APPLICATION_KIND, id],
                )
                .map_err(|e| db_error(&self.db_path(), e))?;
            if removed == 0 {
                return Err(StoreError::NotFound(id.to_string()));
            }
            Ok(())
        })?;
        // The row went first: once it is gone the Application is deleted, and
        // whatever is left in the directory is unreferenced.
        let dir = self.app_dir(id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .map_err(|e| StoreError::Io(dir.clone(), e.to_string()))?;
            sync_dir(&self.inner.root.join(APPLICATIONS_DIR));
        }
        Ok(())
    }

    async fn set_env(&self, app_id: &str, key: &str, value: &str) -> Result<(), StoreError> {
        self.transaction(|tx| {
            let mut body = self
                .read_application_body(tx, app_id)?
                .ok_or_else(|| StoreError::NotFound(app_id.to_string()))?;
            body.env.insert(key.to_string(), value.to_string());
            self.write_application_body(tx, &body)
        })
    }

    async fn get_env(&self, app_id: &str, key: &str) -> Result<Option<String>, StoreError> {
        let conn = self.conn();
        Ok(self
            .read_application_body(&conn, app_id)?
            .and_then(|body| body.env.get(key).cloned()))
    }

    async fn get_all_env(&self, app_id: &str) -> Result<Vec<(String, String)>, StoreError> {
        let conn = self.conn();
        Ok(self
            .read_application_body(&conn, app_id)?
            .map(|body| body.env.into_iter().collect())
            .unwrap_or_default())
    }

    async fn unset_env(&self, app_id: &str, key: &str) -> Result<(), StoreError> {
        self.transaction(|tx| {
            let Some(mut body) = self.read_application_body(tx, app_id)? else {
                return Ok(());
            };
            if body.env.remove(key).is_none() {
                return Ok(());
            }
            self.write_application_body(tx, &body)
        })
    }

    async fn list_api_keys(&self) -> Result<Vec<ApiKeyRecord>, StoreError> {
        self.query(|conn| {
            let mut statement =
                conn.prepare("SELECT id, label, created_at FROM api_keys ORDER BY rowid DESC")?;
            let rows = statement.query_map([], |row| {
                Ok(ApiKeyRecord {
                    id: row.get(0)?,
                    label: row.get(1)?,
                    created_at: row.get(2)?,
                })
            })?;
            rows.collect()
        })
    }

    async fn create_api_key(&self, id: &str, label: &str) -> Result<(), StoreError> {
        self.transaction(|tx| {
            tx.execute(
                "INSERT INTO api_keys (id, label, created_at) VALUES (?1, ?2, ?3)",
                params![id, label, now_text()],
            )
            .map_err(|e| db_error(&self.db_path(), e))?;
            Ok(())
        })
    }

    async fn revoke_api_key(&self, id: &str) -> Result<(), StoreError> {
        self.transaction(|tx| {
            let removed = tx
                .execute("DELETE FROM api_keys WHERE id = ?1", [id])
                .map_err(|e| db_error(&self.db_path(), e))?;
            if removed == 0 {
                return Err(StoreError::NotFound(id.to_string()));
            }
            Ok(())
        })
    }

    async fn list_records(&self, kind: &str) -> Result<Vec<String>, StoreError> {
        self.query(|conn| {
            let mut statement =
                conn.prepare("SELECT body FROM records WHERE kind = ?1 ORDER BY rowid")?;
            let rows = statement.query_map([kind], |row| row.get(0))?;
            rows.collect()
        })
    }

    async fn get_record(&self, kind: &str, id: &str) -> Result<Option<String>, StoreError> {
        self.query(|conn| {
            conn.query_row(
                "SELECT body FROM records WHERE kind = ?1 AND id = ?2",
                params![kind, id],
                |row| row.get(0),
            )
            .optional()
        })
    }

    async fn put_record(&self, kind: &str, id: &str, body: &str) -> Result<(), StoreError> {
        self.transaction(|tx| {
            tx.execute(
                "INSERT INTO records (kind, id, body, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (kind, id) DO UPDATE SET body = excluded.body,
                                                     updated_at = excluded.updated_at",
                params![kind, id, body, now_iso()],
            )
            .map_err(|e| db_error(&self.db_path(), e))?;
            Ok(())
        })
    }

    async fn delete_record(&self, kind: &str, id: &str) -> Result<bool, StoreError> {
        self.transaction(|tx| {
            let removed = tx
                .execute(
                    "DELETE FROM records WHERE kind = ?1 AND id = ?2",
                    params![kind, id],
                )
                .map_err(|e| db_error(&self.db_path(), e))?;
            Ok(removed > 0)
        })
    }

    async fn replace_records(
        &self,
        kind: &str,
        items: &[(String, String)],
    ) -> Result<(), StoreError> {
        self.transaction(|tx| {
            let io = |e| db_error(&self.db_path(), e);
            tx.execute("DELETE FROM records WHERE kind = ?1", [kind])
                .map_err(io)?;
            let now = now_iso();
            let mut insert = tx
                .prepare("INSERT INTO records (kind, id, body, updated_at) VALUES (?1, ?2, ?3, ?4)")
                .map_err(io)?;
            for (id, body) in items {
                insert.execute(params![kind, id, body, now]).map_err(io)?;
            }
            Ok(())
        })
    }

    async fn get_audit_event(&self, id: &str) -> Result<Option<String>, StoreError> {
        self.query(|conn| {
            conn.query_row("SELECT body FROM audit_events WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()
        })
    }

    async fn put_audit_event(&self, row: &AuditRow) -> Result<(), StoreError> {
        self.transaction(|tx| {
            tx.execute(
                "INSERT INTO audit_events
                     (id, status, subject_kind, subject_id, occurred_at, updated_at, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (id) DO UPDATE SET
                     status = excluded.status,
                     subject_kind = excluded.subject_kind,
                     subject_id = excluded.subject_id,
                     occurred_at = excluded.occurred_at,
                     updated_at = excluded.updated_at,
                     body = excluded.body",
                params![
                    row.id,
                    row.status,
                    row.subject_kind,
                    row.subject_id,
                    row.occurred_at,
                    row.updated_at,
                    row.body
                ],
            )
            .map_err(|e| db_error(&self.db_path(), e))?;
            Ok(())
        })
    }

    async fn list_audit_events(&self) -> Result<Vec<String>, StoreError> {
        self.query(|conn| {
            let mut statement =
                conn.prepare("SELECT body FROM audit_events ORDER BY updated_at DESC")?;
            let rows = statement.query_map([], |row| row.get(0))?;
            rows.collect()
        })
    }

    async fn prune_audit_events(&self, before: &str) -> Result<u64, StoreError> {
        self.transaction(|tx| {
            let removed = tx
                .execute("DELETE FROM audit_events WHERE updated_at < ?1", [before])
                .map_err(|e| db_error(&self.db_path(), e))?;
            Ok(removed as u64)
        })
    }
}

/// Opens `platform.db` with the settings the Platform relies on. `DELETE`
/// journaling keeps "copy the directory with the daemon stopped" a valid
/// backup; `synchronous = FULL` and `fullfsync` make a commit survive a power
/// cut, on macOS too, where a plain `fsync` is acknowledged before the drive
/// has the data.
fn open_db(path: &Path, read_only: bool) -> Result<Connection, StoreError> {
    let flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
    };
    let conn = Connection::open_with_flags(path, flags).map_err(|e| db_error(path, e))?;
    if !read_only {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(FILE_MODE))
            .map_err(|e| StoreError::Io(path.to_path_buf(), e.to_string()))?;
    }
    conn.execute_batch(
        "PRAGMA journal_mode = DELETE;
         PRAGMA synchronous = FULL;
         PRAGMA fullfsync = ON;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )
    .map_err(|e| db_error(path, e))?;
    Ok(conn)
}

/// A database failure, with the file's path. A file that is not a database
/// is corrupt state, reported as such rather than as an I/O problem.
fn db_error(path: &Path, error: rusqlite::Error) -> StoreError {
    use rusqlite::ErrorCode;
    match &error {
        rusqlite::Error::SqliteFailure(failure, _)
            if matches!(
                failure.code,
                ErrorCode::NotADatabase | ErrorCode::DatabaseCorrupt
            ) =>
        {
            StoreError::Corrupt(path.to_path_buf(), error.to_string())
        }
        _ => StoreError::Io(path.to_path_buf(), error.to_string()),
    }
}

/// PostgreSQL rendered `timestamptz` as text for this field and the console
/// prints it as it arrives, so the shape stays the same.
fn now_text() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}+00",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

fn now_iso() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

/// Removes Compose definitions and temporary files nothing committed refers to.
/// They are the residue of a write that was interrupted before its row
/// landed, and keeping them would be the only way state could grow forever.
fn collect_orphans(root: &Path, conn: &Connection) {
    let referenced: BTreeMap<String, Option<String>> = {
        let Ok(mut statement) = conn.prepare(
            "SELECT id, json_extract(body, '$.compose_file') FROM records WHERE kind = ?1",
        ) else {
            return;
        };
        let Ok(rows) = statement.query_map([APPLICATION_KIND], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        }) else {
            return;
        };
        rows.flatten().collect()
    };
    let applications = root.join(APPLICATIONS_DIR);
    let Ok(entries) = std::fs::read_dir(&applications) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        let Some(compose_file) = referenced.get(&id) else {
            // No committed row: the whole directory is uncommitted work.
            let _ = std::fs::remove_dir_all(&dir);
            continue;
        };
        let Ok(files) = std::fs::read_dir(&dir) else {
            continue;
        };
        for file in files.flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            if Some(&name) != compose_file.as_ref() {
                let _ = std::fs::remove_file(file.path());
            }
        }
    }
}

fn create_dir(path: &Path) -> Result<(), StoreError> {
    std::fs::create_dir_all(path).map_err(|e| StoreError::Io(path.to_path_buf(), e.to_string()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(DIR_MODE))
        .map_err(|e| StoreError::Io(path.to_path_buf(), e.to_string()))
}

/// Replaces a file with new contents, or leaves the old one untouched.
///
/// The temporary file is flushed before the rename so the rename cannot
/// publish a partially written file, and the directory is flushed after it so
/// the rename itself survives a power cut. Rename alone makes the replacement
/// atomic for a reader; it says nothing about durability.
fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), StoreError> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let temporary = dir.join(format!(
        ".{}.tmp{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));

    let write = || -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(FILE_MODE)
            .open(&temporary)?;
        file.write_all(contents)?;
        file.flush()?;
        sync_file(&file)?;
        Ok(())
    };

    if let Err(e) = write() {
        let _ = std::fs::remove_file(&temporary);
        return Err(StoreError::Io(temporary, e.to_string()));
    }

    if let Err(e) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(StoreError::Io(path.to_path_buf(), e.to_string()));
    }

    sync_dir(dir);
    Ok(())
}

/// macOS acknowledges `fsync` once the drive has the data in its own cache;
/// `F_FULLFSYNC` is what actually makes it durable there.
#[cfg(target_os = "macos")]
fn sync_file(file: &File) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    // SAFETY: the descriptor is owned by `file` and valid for this call.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) } == -1 {
        return file.sync_all();
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn sync_file(file: &File) -> std::io::Result<()> {
    file.sync_all()
}

/// Flushing the directory is what makes a rename durable. A filesystem that
/// refuses the request has nothing to flush, so the failure is not fatal.
fn sync_dir(dir: &Path) {
    if let Ok(handle) = File::open(dir) {
        let _ = handle.sync_all();
    }
}

/// Takes the writer lock for this directory. The lock is advisory and released
/// by the kernel when the process exits, so a daemon killed without cleanup
/// does not leave the Platform locked out of its own state.
fn acquire_lock(root: &Path) -> Result<File, StoreError> {
    use std::os::unix::io::AsRawFd;

    let path = root.join(LOCK_FILE);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(FILE_MODE)
        .open(&path)
        .map_err(|e| StoreError::Io(path.clone(), e.to_string()))?;

    // SAFETY: the descriptor is owned by `file` and valid for this call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(StoreError::Locked(path));
    }

    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::{STATUS_RUNNING, STATUS_STOPPED};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "self-host-store-{}-{name}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create the test store directory");
            TempDir(path)
        }

        fn path(&self) -> PathBuf {
            self.0.join("state")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn record(id: &str, name: &str) -> ApplicationRecord {
        ApplicationRecord {
            id: id.into(),
            name: name.into(),
            hostname: format!("{name}.home.lan"),
            aliases: vec![],
            image: "nginx:latest".into(),
            status: STATUS_RUNNING.into(),
            source: "image".into(),
            last_error: None,
            compose: None,
            web_service: None,
            web_port: None,
            web_target_port: None,
            development: None,
        }
    }

    #[tokio::test]
    async fn a_restart_reads_back_what_the_operator_configured() {
        let dir = TempDir::new("roundtrip");

        let store = FileStateStore::open(dir.path()).unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        store.store_state("api_key", "secret").await.unwrap();

        let mut app = record("k3n8qz4v2x1p", "blog");
        app.aliases = vec!["www.home.lan".into()];
        app.source = "compose".into();
        app.compose = Some("services:\n  web:\n    image: nginx\n".into());
        app.web_service = Some("web".into());
        app.web_port = Some(8080);
        // The proxy reads the Web Target's Host port back from here after a
        // restart, rather than asking Docker where the container ended up.
        app.web_target_port = Some(20001);
        app.status = STATUS_STOPPED.into();
        app.development = Some(DevelopmentApplication {
            image_id: "dev-image".into(),
            tag: "sf-img-dev-image:old".into(),
            command: "t3 serve --port 3000".into(),
            web_port: 3000,
            persist_data: true,
        });
        store.insert_application(&app).await.unwrap();
        store.set_env(&app.id, "TOKEN", "value").await.unwrap();
        store.create_api_key("key-1", "laptop").await.unwrap();
        store
            .put_record("dns-record", "nas/A", r#"{"name":"nas"}"#)
            .await
            .unwrap();
        drop(store);

        let store = FileStateStore::open(dir.path()).unwrap();
        assert_eq!(
            store.get_state("dns_suffix").await.unwrap().as_deref(),
            Some("home.lan")
        );
        assert_eq!(
            store.get_api_key().await.unwrap().as_deref(),
            Some("secret")
        );
        assert_eq!(store.get_application(&app.id).await.unwrap(), Some(app));
        assert_eq!(
            store.get_all_env("k3n8qz4v2x1p").await.unwrap(),
            [("TOKEN".to_string(), "value".to_string())]
        );
        assert_eq!(store.list_api_keys().await.unwrap()[0].label, "laptop");
        assert_eq!(
            store.list_records("dns-record").await.unwrap(),
            vec![r#"{"name":"nas"}"#.to_string()]
        );
    }

    #[tokio::test]
    async fn the_operators_compose_definition_stays_readable_on_disk() {
        let dir = TempDir::new("compose");
        let store = FileStateStore::open(dir.path()).unwrap();

        let mut app = record("k3n8qz4v2x1p", "blog");
        let compose = "services:\n  web:\n    image: nginx\n";
        app.compose = Some(compose.into());
        store.insert_application(&app).await.unwrap();

        let app_dir = dir.path().join(APPLICATIONS_DIR).join("k3n8qz4v2x1p");
        let written = std::fs::read_dir(&app_dir)
            .unwrap()
            .flatten()
            .find(|e| e.file_name().to_string_lossy().starts_with("compose-"))
            .expect("the Compose definition was written next to the row");

        assert_eq!(std::fs::read_to_string(written.path()).unwrap(), compose);
    }

    #[tokio::test]
    async fn a_second_daemon_is_refused_the_same_directory() {
        let dir = TempDir::new("lock");
        let _first = FileStateStore::open(dir.path()).unwrap();

        let error = FileStateStore::open(dir.path()).unwrap_err();

        assert!(matches!(error, StoreError::Locked(_)), "{error}");
    }

    #[tokio::test]
    async fn a_reader_does_not_take_the_directory_from_the_daemon() {
        let dir = TempDir::new("readonly");
        let daemon = FileStateStore::open(dir.path()).unwrap();
        daemon.store_state("dns_suffix", "home.lan").await.unwrap();

        let reader = FileStateStore::read_only(dir.path()).unwrap();

        assert_eq!(
            reader.get_state("dns_suffix").await.unwrap().as_deref(),
            Some("home.lan")
        );
        assert!(matches!(
            reader.store_state("dns_suffix", "other.lan").await,
            Err(StoreError::ReadOnly)
        ));
    }

    #[tokio::test]
    async fn a_directory_the_daemon_never_opened_reads_as_empty() {
        let dir = TempDir::new("never-opened");
        let reader = FileStateStore::read_only(dir.path()).unwrap();
        assert!(!reader.is_initialized().await.unwrap());
        assert!(!dir.path().join(DB_FILE).exists());
    }

    #[tokio::test]
    async fn two_applications_cannot_claim_the_same_name() {
        let dir = TempDir::new("unique");
        let store = FileStateStore::open(dir.path()).unwrap();

        store
            .insert_application(&record("id-a", "blog"))
            .await
            .unwrap();
        let error = store
            .insert_application(&record("id-b", "blog"))
            .await
            .unwrap_err();

        assert!(matches!(error, StoreError::AlreadyExists(name) if name == "blog"));
    }

    #[tokio::test]
    async fn a_rename_keeps_the_identity_and_its_environment() {
        let dir = TempDir::new("rename");
        let store = FileStateStore::open(dir.path()).unwrap();

        let mut app = record("k3n8qz4v2x1p", "blog");
        store.insert_application(&app).await.unwrap();
        store.set_env(&app.id, "TOKEN", "value").await.unwrap();

        app.name = "journal".into();
        store.insert_application(&app).await.unwrap();

        assert_eq!(
            store.get_all_env(&app.id).await.unwrap(),
            [("TOKEN".to_string(), "value".to_string())]
        );
        assert!(
            store
                .find_application_by_name("blog")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store
                .find_application_by_name("journal")
                .await
                .unwrap()
                .map(|found| found.id),
            Some(app.id)
        );
    }

    #[tokio::test]
    async fn an_interrupted_write_leaves_the_committed_record_in_place() {
        let dir = TempDir::new("interrupted");
        let store = FileStateStore::open(dir.path()).unwrap();
        let mut app = record("k3n8qz4v2x1p", "blog");
        app.compose = Some("services: {}\n".into());
        store.insert_application(&app).await.unwrap();
        drop(store);

        // What a crash between the temporary file and the rename leaves behind.
        let app_dir = dir.path().join(APPLICATIONS_DIR).join("k3n8qz4v2x1p");
        std::fs::write(app_dir.join(".compose-x.yaml.tmp999"), "{ truncated").unwrap();
        std::fs::write(app_dir.join("compose-deadbeef.yaml"), "orphan").unwrap();

        let store = FileStateStore::open(dir.path()).unwrap();

        assert_eq!(store.get_application(&app.id).await.unwrap(), Some(app));
        assert!(!app_dir.join(".compose-x.yaml.tmp999").exists());
        assert!(!app_dir.join("compose-deadbeef.yaml").exists());
    }

    #[tokio::test]
    async fn a_create_that_never_committed_is_not_half_an_application() {
        let dir = TempDir::new("uncommitted");
        let _store = FileStateStore::open(dir.path()).unwrap();
        let app_dir = dir.path().join(APPLICATIONS_DIR).join("k3n8qz4v2x1p");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(app_dir.join("compose-deadbeef.yaml"), "orphan").unwrap();
        drop(_store);

        let store = FileStateStore::open(dir.path()).unwrap();

        assert!(store.list_applications().await.unwrap().is_empty());
        assert!(!app_dir.exists());
    }

    #[tokio::test]
    async fn deleting_an_application_removes_its_directory() {
        let dir = TempDir::new("delete");
        let store = FileStateStore::open(dir.path()).unwrap();
        let mut app = record("k3n8qz4v2x1p", "blog");
        app.compose = Some("services: {}\n".into());
        store.insert_application(&app).await.unwrap();

        store.delete_application(&app.id).await.unwrap();

        assert!(store.get_application(&app.id).await.unwrap().is_none());
        assert!(!dir.path().join(APPLICATIONS_DIR).join(&app.id).exists());
        assert!(matches!(
            store.delete_application(&app.id).await,
            Err(StoreError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn corrupt_state_is_reported_and_never_read_as_an_empty_platform() {
        let dir = TempDir::new("corrupt");
        let store = FileStateStore::open(dir.path()).unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        drop(store);

        let path = dir.path().join(DB_FILE);
        std::fs::write(&path, "{ not a database, and long enough to be read as one").unwrap();

        let error = FileStateStore::open(dir.path()).unwrap_err();

        assert!(
            matches!(error, StoreError::Corrupt(ref p, _) if *p == path),
            "{error}"
        );
        assert!(error.to_string().contains(&path.display().to_string()));
    }

    #[tokio::test]
    async fn state_from_a_newer_platform_is_refused_rather_than_guessed_at() {
        let dir = TempDir::new("version");
        let store = FileStateStore::open(dir.path()).unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        drop(store);

        let conn = Connection::open(dir.path().join(DB_FILE)).unwrap();
        conn.execute(
            "UPDATE schema_version SET version = ?1",
            [schema::CURRENT + 1],
        )
        .unwrap();
        drop(conn);

        let error = FileStateStore::open(dir.path()).unwrap_err();
        assert!(
            matches!(error, StoreError::UnsupportedVersion { .. }),
            "{error}"
        );
        let error = FileStateStore::read_only(dir.path()).unwrap_err();
        assert!(
            matches!(error, StoreError::UnsupportedVersion { .. }),
            "{error}"
        );
    }

    #[tokio::test]
    async fn state_from_the_previous_format_stops_the_daemon_and_names_the_script() {
        let dir = TempDir::new("previous-format");
        std::fs::create_dir_all(dir.path()).unwrap();
        let legacy = dir.path().join(LEGACY_PLATFORM_FILE);
        std::fs::write(
            &legacy,
            r#"{"version":1,"state":{"dns_suffix":"home.lan"}}"#,
        )
        .unwrap();

        let error = FileStateStore::open(dir.path()).unwrap_err();

        assert!(
            matches!(error, StoreError::PreviousFormat(ref p) if *p == legacy),
            "{error}"
        );
        assert!(error.to_string().contains("import-json-state.py"));
        // The database exists at the current version for the script to fill.
        let conn = Connection::open(dir.path().join(DB_FILE)).unwrap();
        assert_eq!(schema::current_version(&conn).unwrap(), schema::CURRENT);
    }

    #[tokio::test]
    async fn state_is_not_readable_by_other_accounts_on_the_host() {
        let dir = TempDir::new("permissions");
        let store = FileStateStore::open(dir.path()).unwrap();
        store.store_state("api_key", "secret").await.unwrap();
        let mut app = record("k3n8qz4v2x1p", "blog");
        app.compose = Some("services: {}\n".into());
        store.insert_application(&app).await.unwrap();
        store.set_env(&app.id, "TOKEN", "value").await.unwrap();

        fn mode(path: &Path) -> u32 {
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777
        }

        assert_eq!(mode(&dir.path()), DIR_MODE);
        assert_eq!(mode(&dir.path().join(DB_FILE)), FILE_MODE);
        let app_dir = dir.path().join(APPLICATIONS_DIR).join("k3n8qz4v2x1p");
        assert_eq!(mode(&app_dir), DIR_MODE);
        for file in std::fs::read_dir(&app_dir).unwrap().flatten() {
            assert_eq!(mode(&file.path()), FILE_MODE, "{:?}", file.path());
        }
    }

    #[tokio::test]
    async fn audit_rows_are_listed_newest_first_and_pruned_by_age() {
        let dir = TempDir::new("audit");
        let store = FileStateStore::open(dir.path()).unwrap();
        let row = |id: &str, updated_at: &str| AuditRow {
            id: id.into(),
            status: "completed".into(),
            subject_kind: "application".into(),
            subject_id: "app".into(),
            occurred_at: updated_at.into(),
            updated_at: updated_at.into(),
            body: format!(r#"{{"id":"{id}"}}"#),
        };
        store
            .put_audit_event(&row("old", "2026-08-01T00:00:00Z"))
            .await
            .unwrap();
        store
            .put_audit_event(&row("new", "2026-09-01T00:00:00Z"))
            .await
            .unwrap();

        assert_eq!(
            store.list_audit_events().await.unwrap(),
            vec![r#"{"id":"new"}"#.to_string(), r#"{"id":"old"}"#.to_string()]
        );
        assert_eq!(
            store
                .prune_audit_events("2026-08-15T00:00:00Z")
                .await
                .unwrap(),
            1
        );
        assert_eq!(store.get_audit_event("old").await.unwrap(), None);
        assert!(store.get_audit_event("new").await.unwrap().is_some());
    }
}
