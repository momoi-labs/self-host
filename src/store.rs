//! What the Platform persists, and the contract it persists it through.
//!
//! `StateStore` is the seam: everything the Operator configured goes through
//! it, and the deploy path never learns where it lands. [`crate::file_store`]
//! is the implementation the Platform ships; `FakeStateStore` is the one the
//! suite runs on.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::error::ErrorReport;

#[derive(Debug, Clone, PartialEq, Eq)]
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
}

#[derive(Debug, Clone)]
pub struct ApiKeyRecord {
    pub id: String,
    pub label: String,
    pub created_at: String,
}

#[derive(Clone)]
pub struct FakeStateStore {
    data: Arc<RwLock<HashMap<String, String>>>,
    apps: Arc<RwLock<HashMap<String, ApplicationRecord>>>,
    env: Arc<RwLock<HashMap<String, HashMap<String, String>>>>,
    api_keys: Arc<RwLock<Vec<ApiKeyRecord>>>,
}

impl FakeStateStore {
    pub fn new() -> Self {
        FakeStateStore {
            data: Arc::new(RwLock::new(HashMap::new())),
            apps: Arc::new(RwLock::new(HashMap::new())),
            env: Arc::new(RwLock::new(HashMap::new())),
            api_keys: Arc::new(RwLock::new(Vec::new())),
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
}
