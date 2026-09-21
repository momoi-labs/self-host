//! What the Platform persists, and the contract it persists it through.
//!
//! `StateStore` is the seam: everything the Operator configured goes through
//! it, and the deploy path never learns where it lands. [`crate::file_store`]
//! is the implementation the Platform ships; `FakeStateStore` is the one the
//! suite runs on.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::ErrorReport;

/// The explicit settings that make an Application a development environment.
///
/// The generated Compose definition remains the deploy input, but this record
/// is authoritative for returning to the development form. Applications that
/// only happen to have similar Compose text never receive this metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevelopmentApplication {
    pub image_id: String,
    /// The exact custom image tag deployed by this Application.
    pub tag: String,
    pub command: String,
    pub web_port: u16,
    pub persist_data: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationRecord {
    /// Stable identity. The container and the route are keyed on this, never
    /// on `name`, so renaming an Application touches only a record.
    pub id: String,
    pub name: String,
    pub hostname: String,
    /// Extra Hostnames the same Application answers on, so changing the
    /// Hostname does not have to break the old one (ADR-0009).
    pub aliases: Vec<String>,
    pub image: String,
    pub status: String,
    pub source: String,
    /// Why the last deploy failed, when `status` is `failed`. Kept as a
    /// report rather than a sentence so the console can put the failure in the
    /// alert's title and the causes in its body.
    pub last_error: Option<ErrorReport>,
    /// The Operator's Compose definition, verbatim, when `source` is
    /// `compose`. What the Platform runs is rendered from it every time.
    pub compose: Option<String>,
    /// The Compose service the Hostname routes to. `None` for a
    /// single-container Application, whose only container is the target.
    pub web_service: Option<String>,
    /// The container port the Hostname routes to. `None` means the default
    /// HTTP port of a single-container Application.
    pub web_port: Option<u16>,
    /// The Host port the Web Target is published on, so the embedded proxy
    /// can reach it from outside Docker (ADR-0019). Allocated once and kept:
    /// the proxy reads it back after a restart instead of asking Docker.
    /// `None` for an Application deployed before the proxy moved in-process.
    pub web_target_port: Option<u16>,
    /// Present only for Applications created or explicitly converted through
    /// the development-image API.
    pub development: Option<DevelopmentApplication>,
}

#[derive(Debug)]
pub enum StoreError {
    /// State could not be turned into what goes on disk.
    Serialize(String),
    AlreadyInitialized,
    AlreadyExists(String),
    NotFound(String),
    /// The state directory could not be read or written.
    Io(std::path::PathBuf, String),
    /// A state file exists but does not parse. Reported with its path, and
    /// never mistaken for a Platform that was never initialized.
    Corrupt(std::path::PathBuf, String),
    /// State written by a newer Platform than this one.
    UnsupportedVersion {
        path: std::path::PathBuf,
        found: u32,
        supported: u32,
    },
    /// State written before the Platform kept it in SQLite. It is imported by
    /// a script, never read directly, and never mistaken for an empty
    /// installation.
    PreviousFormat(std::path::PathBuf),
    /// Another process already holds the state directory as its writer.
    Locked(std::path::PathBuf),
    /// A mutation was attempted on a handle opened only to read.
    ReadOnly,
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Serialize(msg) => write!(f, "failed to write Platform state: {msg}"),
            StoreError::AlreadyInitialized => write!(f, "already initialized"),
            StoreError::AlreadyExists(name) => write!(f, "Application '{name}' already exists"),
            StoreError::NotFound(name) => write!(f, "Application '{name}' not found"),
            StoreError::Io(path, msg) => write!(f, "failed to access {}: {msg}", path.display()),
            StoreError::Corrupt(path, msg) => write!(
                f,
                "{} is not valid Platform state: {msg}. Restore it from a backup, \
                 or run 'self-host reset' to start over",
                path.display()
            ),
            StoreError::UnsupportedVersion {
                path,
                found,
                supported,
            } => write!(
                f,
                "{} was written by a newer Platform (format {found}, this one reads {supported}); \
                 upgrade the Platform binary",
                path.display()
            ),
            StoreError::PreviousFormat(path) => write!(
                f,
                "{} is Platform state from a previous format; stop the daemon and run \
                 scripts/import-json-state.py to move it into platform.db",
                path.display()
            ),
            StoreError::Locked(path) => write!(
                f,
                "another self-host process already holds {}; stop it before starting a second one",
                path.display()
            ),
            StoreError::ReadOnly => write!(f, "this Platform state was opened for reading only"),
        }
    }
}

impl std::error::Error for StoreError {}

#[async_trait]
pub trait StateStore: Clone + Send + Sync + 'static {
    /// Prepares the store to be written to. Called on every start, and does
    /// nothing to state that is already there.
    async fn initialize(&self) -> Result<(), StoreError>;
    async fn is_initialized(&self) -> Result<bool, StoreError>;
    async fn store_state(&self, key: &str, value: &str) -> Result<(), StoreError>;
    async fn get_state(&self, key: &str) -> Result<Option<String>, StoreError>;
    async fn get_api_key(&self) -> Result<Option<String>, StoreError> {
        self.get_state("api_key").await
    }

    async fn insert_application(&self, app: &ApplicationRecord) -> Result<(), StoreError>;
    /// Saves an execution result atomically without restoring an older name or definition.
    async fn set_application_outcome(
        &self,
        id: &str,
        status: &str,
        error: Option<crate::error::ErrorReport>,
    ) -> Result<ApplicationRecord, StoreError>;
    async fn get_application(&self, id: &str) -> Result<Option<ApplicationRecord>, StoreError>;
    async fn find_application_by_name(
        &self,
        name: &str,
    ) -> Result<Option<ApplicationRecord>, StoreError>;
    async fn application_exists(&self, name: &str) -> Result<bool, StoreError> {
        Ok(self.find_application_by_name(name).await?.is_some())
    }
    async fn list_applications(&self) -> Result<Vec<ApplicationRecord>, StoreError>;
    async fn delete_application(&self, id: &str) -> Result<(), StoreError>;

    async fn set_env(&self, app_id: &str, key: &str, value: &str) -> Result<(), StoreError>;
    async fn get_env(&self, app_id: &str, key: &str) -> Result<Option<String>, StoreError>;
    async fn get_all_env(&self, app_id: &str) -> Result<Vec<(String, String)>, StoreError>;
    async fn unset_env(&self, app_id: &str, key: &str) -> Result<(), StoreError>;

    async fn list_api_keys(&self) -> Result<Vec<ApiKeyRecord>, StoreError>;
    async fn create_api_key(&self, id: &str, label: &str) -> Result<(), StoreError>;
    async fn revoke_api_key(&self, id: &str) -> Result<(), StoreError>;

    /// One collection of records, keyed by `kind` and `id`, each a JSON body.
    /// [`crate::collection::Collection`] is the typed face of these; nothing
    /// else calls them directly.
    async fn list_records(&self, kind: &str) -> Result<Vec<String>, StoreError>;
    async fn get_record(&self, kind: &str, id: &str) -> Result<Option<String>, StoreError>;
    async fn put_record(&self, kind: &str, id: &str, body: &str) -> Result<(), StoreError>;
    async fn delete_record(&self, kind: &str, id: &str) -> Result<bool, StoreError>;
    /// Replaces the whole collection in one commit.
    async fn replace_records(
        &self,
        kind: &str,
        items: &[(String, String)],
    ) -> Result<(), StoreError>;

    async fn get_audit_event(&self, id: &str) -> Result<Option<String>, StoreError>;
    async fn put_audit_event(&self, row: &AuditRow) -> Result<(), StoreError>;
    /// Every event body, most recently updated first.
    async fn list_audit_events(&self) -> Result<Vec<String>, StoreError>;
    /// Removes events last updated before `before` (a timestamp in the same
    /// text form the events carry) and returns how many went.
    async fn prune_audit_events(&self, before: &str) -> Result<u64, StoreError>;
}

/// One audit event as the store keeps it: the columns queries key on, and
/// the whole event as JSON.
#[derive(Debug, Clone)]
pub struct AuditRow {
    pub id: String,
    pub status: String,
    pub subject_kind: String,
    pub subject_id: String,
    pub occurred_at: String,
    pub updated_at: String,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct ApiKeyRecord {
    pub id: String,
    pub label: String,
    pub created_at: String,
}

/// One collection in the fake: `(id, body)` pairs in insertion order.
type FakeRecords = HashMap<String, Vec<(String, String)>>;

#[derive(Clone)]
pub struct FakeStateStore {
    data: Arc<RwLock<HashMap<String, String>>>,
    apps: Arc<RwLock<HashMap<String, ApplicationRecord>>>,
    env: Arc<RwLock<HashMap<String, HashMap<String, String>>>>,
    api_keys: Arc<RwLock<Vec<ApiKeyRecord>>>,
    records: Arc<RwLock<FakeRecords>>,
    audit: Arc<RwLock<Vec<AuditRow>>>,
}

impl FakeStateStore {
    pub fn new() -> Self {
        FakeStateStore {
            data: Arc::new(RwLock::new(HashMap::new())),
            apps: Arc::new(RwLock::new(HashMap::new())),
            env: Arc::new(RwLock::new(HashMap::new())),
            api_keys: Arc::new(RwLock::new(Vec::new())),
            records: Arc::new(RwLock::new(HashMap::new())),
            audit: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl Default for FakeStateStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StateStore for FakeStateStore {
    async fn initialize(&self) -> Result<(), StoreError> {
        Ok(())
    }

    async fn is_initialized(&self) -> Result<bool, StoreError> {
        let data = self.data.read().await;
        Ok(data.contains_key("dns_suffix"))
    }

    async fn store_state(&self, key: &str, value: &str) -> Result<(), StoreError> {
        let mut data = self.data.write().await;
        data.insert(key.to_string(), value.to_string());
        Ok(())
    }

    async fn get_state(&self, key: &str) -> Result<Option<String>, StoreError> {
        let data = self.data.read().await;
        Ok(data.get(key).cloned())
    }

    async fn insert_application(&self, app: &ApplicationRecord) -> Result<(), StoreError> {
        let mut apps = self.apps.write().await;
        apps.insert(app.id.clone(), app.clone());
        Ok(())
    }

    async fn get_application(&self, id: &str) -> Result<Option<ApplicationRecord>, StoreError> {
        Ok(self.apps.read().await.get(id).cloned())
    }

    async fn set_application_outcome(
        &self,
        id: &str,
        status: &str,
        error: Option<crate::error::ErrorReport>,
    ) -> Result<ApplicationRecord, StoreError> {
        let mut apps = self.apps.write().await;
        let app = apps
            .get_mut(id)
            .ok_or_else(|| StoreError::NotFound(id.into()))?;
        app.status = status.into();
        app.last_error = error;
        Ok(app.clone())
    }

    async fn find_application_by_name(
        &self,
        name: &str,
    ) -> Result<Option<ApplicationRecord>, StoreError> {
        Ok(self
            .apps
            .read()
            .await
            .values()
            .find(|a| a.name == name)
            .cloned())
    }

    async fn list_applications(&self) -> Result<Vec<ApplicationRecord>, StoreError> {
        let apps = self.apps.read().await;
        let mut list: Vec<_> = apps.values().cloned().collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    async fn delete_application(&self, id: &str) -> Result<(), StoreError> {
        let mut apps = self.apps.write().await;
        if apps.remove(id).is_none() {
            return Err(StoreError::NotFound(id.to_string()));
        }
        let mut env = self.env.write().await;
        env.remove(id);
        Ok(())
    }

    async fn set_env(&self, app_id: &str, key: &str, value: &str) -> Result<(), StoreError> {
        let mut env = self.env.write().await;
        env.entry(app_id.to_string())
            .or_default()
            .insert(key.to_string(), value.to_string());
        Ok(())
    }

    async fn get_env(&self, app_id: &str, key: &str) -> Result<Option<String>, StoreError> {
        let env = self.env.read().await;
        Ok(env.get(app_id).and_then(|m| m.get(key)).cloned())
    }

    async fn get_all_env(&self, app_id: &str) -> Result<Vec<(String, String)>, StoreError> {
        let env = self.env.read().await;
        let mut vars: Vec<_> = env
            .get(app_id)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        vars.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(vars)
    }

    async fn unset_env(&self, app_id: &str, key: &str) -> Result<(), StoreError> {
        let mut env = self.env.write().await;
        if let Some(m) = env.get_mut(app_id) {
            m.remove(key);
        }
        Ok(())
    }

    async fn list_api_keys(&self) -> Result<Vec<ApiKeyRecord>, StoreError> {
        Ok(self.api_keys.read().await.clone())
    }

    async fn create_api_key(&self, id: &str, label: &str) -> Result<(), StoreError> {
        self.api_keys.write().await.push(ApiKeyRecord {
            id: id.to_string(),
            label: label.to_string(),
            created_at: "now".into(),
        });
        Ok(())
    }

    async fn revoke_api_key(&self, id: &str) -> Result<(), StoreError> {
        let mut keys = self.api_keys.write().await;
        let len = keys.len();
        keys.retain(|k| k.id != id);
        if keys.len() == len {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    async fn list_records(&self, kind: &str) -> Result<Vec<String>, StoreError> {
        Ok(self
            .records
            .read()
            .await
            .get(kind)
            .map(|items| items.iter().map(|(_, body)| body.clone()).collect())
            .unwrap_or_default())
    }

    async fn get_record(&self, kind: &str, id: &str) -> Result<Option<String>, StoreError> {
        Ok(self.records.read().await.get(kind).and_then(|items| {
            items
                .iter()
                .find(|(key, _)| key == id)
                .map(|(_, body)| body.clone())
        }))
    }

    async fn put_record(&self, kind: &str, id: &str, body: &str) -> Result<(), StoreError> {
        let mut records = self.records.write().await;
        let items = records.entry(kind.to_string()).or_default();
        match items.iter_mut().find(|(key, _)| key == id) {
            Some(item) => item.1 = body.to_string(),
            None => items.push((id.to_string(), body.to_string())),
        }
        Ok(())
    }

    async fn delete_record(&self, kind: &str, id: &str) -> Result<bool, StoreError> {
        let mut records = self.records.write().await;
        let Some(items) = records.get_mut(kind) else {
            return Ok(false);
        };
        let before = items.len();
        items.retain(|(key, _)| key != id);
        Ok(items.len() != before)
    }

    async fn replace_records(
        &self,
        kind: &str,
        items: &[(String, String)],
    ) -> Result<(), StoreError> {
        self.records
            .write()
            .await
            .insert(kind.to_string(), items.to_vec());
        Ok(())
    }

    async fn get_audit_event(&self, id: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .audit
            .read()
            .await
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.body.clone()))
    }

    async fn put_audit_event(&self, row: &AuditRow) -> Result<(), StoreError> {
        let mut audit = self.audit.write().await;
        match audit.iter_mut().find(|existing| existing.id == row.id) {
            Some(existing) => *existing = row.clone(),
            None => audit.push(row.clone()),
        }
        Ok(())
    }

    async fn list_audit_events(&self) -> Result<Vec<String>, StoreError> {
        let mut rows = self.audit.read().await.clone();
        rows.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(rows.into_iter().map(|row| row.body).collect())
    }

    async fn prune_audit_events(&self, before: &str) -> Result<u64, StoreError> {
        let mut audit = self.audit.write().await;
        let len = audit.len();
        audit.retain(|row| row.updated_at.as_str() >= before);
        Ok((len - audit.len()) as u64)
    }
}
