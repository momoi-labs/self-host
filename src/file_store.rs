//! The Platform's authoritative state, kept as files on the Host.
//!
//! One JSON file is one commit unit. `platform.json` holds the Platform
//! settings and the Operator's credentials; `applications/<id>/application.json`
//! holds everything about one Application, including its environment and the
//! digest of its Compose definition. Nothing the Platform owns spans two files,
//! so a mutation is a single atomic replace and there is no multi-file commit
//! protocol to recover from.
//!
//! The Operator's Compose definition is the one exception, and it is written
//! the other way round: the verbatim file lands first, under a name derived
//! from its digest, and only then does the record that references it commit.
//! A Compose file nothing references is work that was interrupted before it
//! counted, and startup collects it.
//!
//! `state/` is authoritative and nothing else is. Rendered Compose projects,
//! the route table and Docker's own view of the world are all rebuildable,
//! and live outside it.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::error::ErrorReport;
use crate::store::{
    ApiKeyRecord, ApplicationRecord, DevelopmentApplication, StateStore, StoreError,
};

/// The format the Platform writes. A file that declares a higher version was
/// written by a newer Platform and is not guessed at.
pub const FORMAT_VERSION: u32 = 1;

const PLATFORM_FILE: &str = "platform.json";
const LOCK_FILE: &str = "platform.lock";
const APPLICATIONS_DIR: &str = "applications";
const APPLICATION_FILE: &str = "application.json";

const DIR_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;

/// Where the authoritative state lives, under the Platform configuration
/// directory it shares with generated files.
pub fn state_dir() -> PathBuf {
    crate::paths::platform_config_dir().join("state")
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PlatformFile {
    version: u32,
    #[serde(default)]
    state: BTreeMap<String, String>,
    #[serde(default)]
    api_keys: Vec<ApiKeyRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApiKeyRow {
    id: String,
    label: String,
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ApplicationFile {
    version: u32,
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
    /// The file next to this one holding the Operator's Compose definition,
    /// verbatim. `None` for an Application that is not a Compose project.
    #[serde(default)]
    compose_file: Option<String>,
    #[serde(default)]
    web_service: Option<String>,
    #[serde(default)]
    web_port: Option<u16>,
    #[serde(default)]
    web_target_port: Option<u16>,
    #[serde(default)]
    development: Option<DevelopmentApplicationFile>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DevelopmentApplicationFile {
    image_id: String,
    tag: String,
    command: String,
    web_port: u16,
    persist_data: bool,
}

/// One Application as it sits in memory: the committed record plus the Compose
/// text that was read from the file the record names.
#[derive(Debug, Clone)]
struct AppEntry {
    file: ApplicationFile,
    compose: Option<String>,
}

#[derive(Debug, Default)]
struct Snapshot {
    platform: PlatformFile,
    apps: BTreeMap<String, AppEntry>,
}

struct Inner {
    root: PathBuf,
    snapshot: RwLock<Snapshot>,
    /// Held for as long as the store is open. Dropping the handle releases the
    /// advisory lock, so a second daemon can take over after this one exits.
    _lock: Option<File>,
}

/// The Platform state, read from and written to files under one directory.
///
/// Cloning shares the same open directory: the daemon hands copies to Axum
/// handlers and they all serialize behind the same lock.
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
        let snapshot = load(&root)?;
        collect_orphans(&root, &snapshot);
        Ok(FileStateStore {
            inner: Arc::new(Inner {
                root,
                snapshot: RwLock::new(snapshot),
                _lock: Some(lock),
            }),
        })
    }

    /// Opens the store for reading only, without taking the writer lock, so a
    /// CLI command can report what is configured while the daemon is running.
    /// Every mutation on this handle fails.
    pub fn read_only(root: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let root = root.into();
        let snapshot = if root.exists() {
            load(&root)?
        } else {
            Snapshot::default()
        };
        Ok(FileStateStore {
            inner: Arc::new(Inner {
                root,
                snapshot: RwLock::new(snapshot),
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

    fn app_dir(&self, id: &str) -> PathBuf {
        self.inner.root.join(APPLICATIONS_DIR).join(id)
    }

    /// Commits one Application. The record lands last, so a crash anywhere
    /// before the rename leaves the previously committed record in place.
    fn commit_app(&self, entry: &AppEntry) -> Result<(), StoreError> {
        let dir = self.app_dir(&entry.file.id);
        create_dir(&dir)?;
        if let (Some(name), Some(compose)) = (&entry.file.compose_file, &entry.compose) {
            let path = dir.join(name);
            if !path.exists() {
                write_atomic(&path, compose.as_bytes())?;
            }
        }
        let body = serde_json::to_vec_pretty(&entry.file)
            .map_err(|e| StoreError::Serialize(format!("Application {}: {e}", entry.file.id)))?;
        write_atomic(&dir.join(APPLICATION_FILE), &body)
    }

    fn commit_platform(&self, platform: &PlatformFile) -> Result<(), StoreError> {
        let body = serde_json::to_vec_pretty(platform)
            .map_err(|e| StoreError::Serialize(e.to_string()))?;
        write_atomic(&self.inner.root.join(PLATFORM_FILE), &body)
    }
}

impl From<&AppEntry> for ApplicationRecord {
    fn from(entry: &AppEntry) -> Self {
        let f = &entry.file;
        ApplicationRecord {
            id: f.id.clone(),
            name: f.name.clone(),
            hostname: f.hostname.clone(),
            aliases: f.aliases.clone(),
            image: f.image.clone(),
            status: f.status.clone(),
            source: f.source.clone(),
            last_error: f.last_error.clone(),
            compose: entry.compose.clone(),
            web_service: f.web_service.clone(),
            web_port: f.web_port,
            web_target_port: f.web_target_port,
            development: f.development.as_ref().map(development_from_file),
        }
    }
}

fn entry_from(app: &ApplicationRecord) -> AppEntry {
    AppEntry {
        file: ApplicationFile {
            version: FORMAT_VERSION,
            id: app.id.clone(),
            name: app.name.clone(),
            hostname: app.hostname.clone(),
            aliases: app.aliases.clone(),
            image: app.image.clone(),
            status: app.status.clone(),
            source: app.source.clone(),
            last_error: app.last_error.clone(),
            compose_file: app.compose.as_deref().map(compose_file_name),
            web_service: app.web_service.clone(),
            web_port: app.web_port,
            web_target_port: app.web_target_port,
            development: app.development.as_ref().map(development_to_file),
            env: BTreeMap::new(),
        },
        compose: app.compose.clone(),
    }
}

fn development_from_file(value: &DevelopmentApplicationFile) -> DevelopmentApplication {
    DevelopmentApplication {
        image_id: value.image_id.clone(),
        tag: value.tag.clone(),
        command: value.command.clone(),
        web_port: value.web_port,
        persist_data: value.persist_data,
    }
}

fn development_to_file(value: &DevelopmentApplication) -> DevelopmentApplicationFile {
    DevelopmentApplicationFile {
        image_id: value.image_id.clone(),
        tag: value.tag.clone(),
        command: value.command.clone(),
        web_port: value.web_port,
        persist_data: value.persist_data,
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
        let snapshot = self.snapshot_write().await;
        if !self.inner.root.join(PLATFORM_FILE).exists() {
            self.commit_platform(&snapshot.platform)?;
        }
        Ok(())
    }

    async fn is_initialized(&self) -> Result<bool, StoreError> {
        Ok(self.get_state("dns_suffix").await?.is_some())
    }

    async fn store_state(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.writable()?;
        let mut snapshot = self.snapshot_write().await;
        let mut platform = clone_platform(&snapshot.platform);
        platform.state.insert(key.to_string(), value.to_string());
        self.commit_platform(&platform)?;
        snapshot.platform = platform;
        Ok(())
    }

    async fn get_state(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .inner
            .snapshot
            .read()
            .await
            .platform
            .state
            .get(key)
            .cloned())
    }

    async fn insert_application(&self, app: &ApplicationRecord) -> Result<(), StoreError> {
        self.writable()?;
        let mut snapshot = self.snapshot_write().await;

        // PostgreSQL held this as a UNIQUE constraint, and validation upstream
        // races with a second request. The check belongs where the commit is.
        if snapshot
            .apps
            .values()
            .any(|e| e.file.name == app.name && e.file.id != app.id)
        {
            return Err(StoreError::AlreadyExists(app.name.clone()));
        }

        let mut entry = entry_from(app);
        // An update keeps the environment already committed for this identity.
        if let Some(existing) = snapshot.apps.get(&app.id) {
            entry.file.env = existing.file.env.clone();
        }
        self.commit_app(&entry)?;
        snapshot.apps.insert(app.id.clone(), entry);
        Ok(())
    }

    async fn get_application(&self, id: &str) -> Result<Option<ApplicationRecord>, StoreError> {
        Ok(self
            .inner
            .snapshot
            .read()
            .await
            .apps
            .get(id)
            .map(ApplicationRecord::from))
    }

    async fn find_application_by_name(
        &self,
        name: &str,
    ) -> Result<Option<ApplicationRecord>, StoreError> {
        Ok(self
            .inner
            .snapshot
            .read()
            .await
            .apps
            .values()
            .find(|e| e.file.name == name)
            .map(ApplicationRecord::from))
    }

    async fn list_applications(&self) -> Result<Vec<ApplicationRecord>, StoreError> {
        let snapshot = self.inner.snapshot.read().await;
        let mut list: Vec<ApplicationRecord> = snapshot
            .apps
            .values()
            .map(ApplicationRecord::from)
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(list)
    }

    async fn delete_application(&self, id: &str) -> Result<(), StoreError> {
        self.writable()?;
        let mut snapshot = self.snapshot_write().await;
        if !snapshot.apps.contains_key(id) {
            return Err(StoreError::NotFound(id.to_string()));
        }
        let dir = self.app_dir(id);
        // The record goes first: once it is gone the Application is deleted,
        // and whatever is left in the directory is unreferenced.
        remove_file(&dir.join(APPLICATION_FILE))?;
        std::fs::remove_dir_all(&dir).map_err(|e| StoreError::Io(dir.clone(), e.to_string()))?;
        sync_dir(&self.inner.root.join(APPLICATIONS_DIR));
        snapshot.apps.remove(id);
        Ok(())
    }

    async fn set_env(&self, app_id: &str, key: &str, value: &str) -> Result<(), StoreError> {
        self.writable()?;
        let mut snapshot = self.snapshot_write().await;
        let Some(existing) = snapshot.apps.get(app_id) else {
            return Err(StoreError::NotFound(app_id.to_string()));
        };
        let mut entry = existing.clone();
        entry.file.env.insert(key.to_string(), value.to_string());
        self.commit_app(&entry)?;
        snapshot.apps.insert(app_id.to_string(), entry);
        Ok(())
    }

    async fn get_env(&self, app_id: &str, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .inner
            .snapshot
            .read()
            .await
            .apps
            .get(app_id)
            .and_then(|e| e.file.env.get(key).cloned()))
    }

    async fn get_all_env(&self, app_id: &str) -> Result<Vec<(String, String)>, StoreError> {
        Ok(self
            .inner
            .snapshot
            .read()
            .await
            .apps
            .get(app_id)
            .map(|e| {
                e.file
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn unset_env(&self, app_id: &str, key: &str) -> Result<(), StoreError> {
        self.writable()?;
        let mut snapshot = self.snapshot_write().await;
        let Some(existing) = snapshot.apps.get(app_id) else {
            return Ok(());
        };
        if !existing.file.env.contains_key(key) {
            return Ok(());
        }
        let mut entry = existing.clone();
        entry.file.env.remove(key);
        self.commit_app(&entry)?;
        snapshot.apps.insert(app_id.to_string(), entry);
        Ok(())
    }

    async fn list_api_keys(&self) -> Result<Vec<ApiKeyRecord>, StoreError> {
        let snapshot = self.inner.snapshot.read().await;
        Ok(snapshot
            .platform
            .api_keys
            .iter()
            .rev()
            .map(|k| ApiKeyRecord {
                id: k.id.clone(),
                label: k.label.clone(),
                created_at: k.created_at.clone(),
            })
            .collect())
    }

    async fn create_api_key(&self, id: &str, label: &str) -> Result<(), StoreError> {
        self.writable()?;
        let mut snapshot = self.snapshot_write().await;
        let mut platform = clone_platform(&snapshot.platform);
        platform.api_keys.push(ApiKeyRow {
            id: id.to_string(),
            label: label.to_string(),
            created_at: now_text(),
        });
        self.commit_platform(&platform)?;
        snapshot.platform = platform;
        Ok(())
    }

    async fn revoke_api_key(&self, id: &str) -> Result<(), StoreError> {
        self.writable()?;
        let mut snapshot = self.snapshot_write().await;
        let mut platform = clone_platform(&snapshot.platform);
        let before = platform.api_keys.len();
        platform.api_keys.retain(|k| k.id != id);
        if platform.api_keys.len() == before {
            return Err(StoreError::NotFound(id.to_string()));
        }
        self.commit_platform(&platform)?;
        snapshot.platform = platform;
        Ok(())
    }
}

impl FileStateStore {
    async fn snapshot_write(&self) -> tokio::sync::RwLockWriteGuard<'_, Snapshot> {
        self.inner.snapshot.write().await
    }
}

fn clone_platform(platform: &PlatformFile) -> PlatformFile {
    PlatformFile {
        version: FORMAT_VERSION,
        state: platform.state.clone(),
        api_keys: platform.api_keys.clone(),
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

fn load(root: &Path) -> Result<Snapshot, StoreError> {
    let platform_path = root.join(PLATFORM_FILE);
    let platform: PlatformFile = if platform_path.exists() {
        let file: PlatformFile = read_json(&platform_path)?;
        check_version(&platform_path, file.version)?;
        file
    } else {
        PlatformFile {
            version: FORMAT_VERSION,
            ..PlatformFile::default()
        }
    };

    let mut apps = BTreeMap::new();
    let applications = root.join(APPLICATIONS_DIR);
    if applications.exists() {
        let entries = std::fs::read_dir(&applications)
            .map_err(|e| StoreError::Io(applications.clone(), e.to_string()))?;
        for entry in entries {
            let entry = entry.map_err(|e| StoreError::Io(applications.clone(), e.to_string()))?;
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let record_path = dir.join(APPLICATION_FILE);
            // A directory with no record is a create that never committed.
            if !record_path.exists() {
                continue;
            }
            let file: ApplicationFile = read_json(&record_path)?;
            check_version(&record_path, file.version)?;
            let compose = match &file.compose_file {
                Some(name) => {
                    let path = dir.join(name);
                    Some(std::fs::read_to_string(&path).map_err(|e| {
                        StoreError::Corrupt(
                            record_path.clone(),
                            format!(
                                "its Compose definition {} is unreadable: {e}",
                                path.display()
                            ),
                        )
                    })?)
                }
                None => None,
            };
            apps.insert(file.id.clone(), AppEntry { file, compose });
        }
    }

    Ok(Snapshot { platform, apps })
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, StoreError> {
    let bytes =
        std::fs::read(path).map_err(|e| StoreError::Io(path.to_path_buf(), e.to_string()))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| StoreError::Corrupt(path.to_path_buf(), e.to_string()))
}

fn check_version(path: &Path, version: u32) -> Result<(), StoreError> {
    if version > FORMAT_VERSION {
        return Err(StoreError::UnsupportedVersion {
            path: path.to_path_buf(),
            found: version,
            supported: FORMAT_VERSION,
        });
    }
    Ok(())
}

/// Removes Compose definitions and temporary files nothing committed refers to.
/// They are the residue of a write that was interrupted before its record
/// landed, and keeping them would be the only way state could grow forever.
fn collect_orphans(root: &Path, snapshot: &Snapshot) {
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
        let Some(app) = snapshot.apps.get(&id) else {
            // No committed record: the whole directory is uncommitted work.
            let _ = std::fs::remove_dir_all(&dir);
            continue;
        };
        let Ok(files) = std::fs::read_dir(&dir) else {
            continue;
        };
        for file in files.flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            let referenced =
                name == APPLICATION_FILE || Some(&name) == app.file.compose_file.as_ref();
            if !referenced {
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

fn remove_file(path: &Path) -> Result<(), StoreError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(StoreError::Io(path.to_path_buf(), e.to_string())),
    }
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
            .expect("the Compose definition was written next to the record");

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
    }

    #[tokio::test]
    async fn an_interrupted_write_leaves_the_committed_record_in_place() {
        let dir = TempDir::new("interrupted");
        let store = FileStateStore::open(dir.path()).unwrap();
        let app = record("k3n8qz4v2x1p", "blog");
        store.insert_application(&app).await.unwrap();
        drop(store);

        // What a crash between the temporary file and the rename leaves behind.
        let app_dir = dir.path().join(APPLICATIONS_DIR).join("k3n8qz4v2x1p");
        std::fs::write(app_dir.join(".application.json.tmp999"), "{ truncated").unwrap();
        std::fs::write(app_dir.join("compose-deadbeef.yaml"), "orphan").unwrap();

        let store = FileStateStore::open(dir.path()).unwrap();

        assert_eq!(store.get_application(&app.id).await.unwrap(), Some(app));
        assert!(!app_dir.join(".application.json.tmp999").exists());
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
    async fn corrupt_state_is_reported_and_never_read_as_an_empty_platform() {
        let dir = TempDir::new("corrupt");
        let store = FileStateStore::open(dir.path()).unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        drop(store);

        let path = dir.path().join(PLATFORM_FILE);
        std::fs::write(&path, "{ not json").unwrap();

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

        let path = dir.path().join(PLATFORM_FILE);
        let body = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            body.replace(
                &format!("\"version\": {FORMAT_VERSION}"),
                &format!("\"version\": {}", FORMAT_VERSION + 1),
            ),
        )
        .unwrap();

        let error = FileStateStore::open(dir.path()).unwrap_err();

        assert!(
            matches!(error, StoreError::UnsupportedVersion { .. }),
            "{error}"
        );
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
        assert_eq!(mode(&dir.path().join(PLATFORM_FILE)), FILE_MODE);
        let app_dir = dir.path().join(APPLICATIONS_DIR).join("k3n8qz4v2x1p");
        assert_eq!(mode(&app_dir), DIR_MODE);
        for file in std::fs::read_dir(&app_dir).unwrap().flatten() {
            assert_eq!(mode(&file.path()), FILE_MODE, "{:?}", file.path());
        }
    }

    #[tokio::test]
    async fn deleting_an_application_takes_its_environment_with_it() {
        let dir = TempDir::new("delete");
        let store = FileStateStore::open(dir.path()).unwrap();
        let app = record("k3n8qz4v2x1p", "blog");
        store.insert_application(&app).await.unwrap();
        store.set_env(&app.id, "TOKEN", "value").await.unwrap();

        store.delete_application(&app.id).await.unwrap();

        assert!(store.get_all_env(&app.id).await.unwrap().is_empty());
        assert!(!dir.path().join(APPLICATIONS_DIR).join(&app.id).exists());
        assert!(matches!(
            store.delete_application(&app.id).await,
            Err(StoreError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn concurrent_requests_do_not_lose_a_committed_change() {
        let dir = TempDir::new("concurrent");
        let store = FileStateStore::open(dir.path()).unwrap();
        store
            .insert_application(&record("k3n8qz4v2x1p", "blog"))
            .await
            .unwrap();

        let mut tasks = Vec::new();
        for i in 0..16 {
            let store = store.clone();
            tasks.push(tokio::spawn(async move {
                store
                    .set_env("k3n8qz4v2x1p", &format!("KEY_{i}"), "value")
                    .await
                    .unwrap();
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        drop(store);

        let store = FileStateStore::open(dir.path()).unwrap();
        assert_eq!(store.get_all_env("k3n8qz4v2x1p").await.unwrap().len(), 16);
    }
}
