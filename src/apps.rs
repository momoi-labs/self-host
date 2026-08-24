use crate::db::{DbError, StateStore};
use crate::docker::{ApplicationContainer, DockerError, DockerRuntime, PLATFORM_NETWORK};

pub const APP_CONTAINER_PORT: u16 = 80;

/// Container name prefixes. Applications are keyed by their surrogate id, so a
/// rename never touches Docker; Platform Infra uses fixed, well-known names.
pub const APP_PREFIX: &str = "sf-app-";
pub const SYSTEM_PREFIX: &str = "sf-system-";

const ID_ALPHABET: &[u8] = b"abcdefghijkmnpqrstuvwxyz23456789";
const ID_LEN: usize = 12;

/// A short, stable, random identity. Lowercase base32 without the characters
/// that read ambiguously in a terminal (l/1, o/0), so an id copied out of
/// `docker ps` by eye lands correctly.
pub fn generate_app_id() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    (0..ID_LEN)
        .map(|_| ID_ALPHABET[rng.random_range(0..ID_ALPHABET.len())] as char)
        .collect()
}

pub use crate::db::ApplicationRecord;

/// The three states an Application row can be in. `pending` is written before
/// any Docker work starts, so a crash mid-deploy is visible rather than silent.
pub const STATUS_PENDING: &str = "pending";
pub const STATUS_RUNNING: &str = "running";
pub const STATUS_FAILED: &str = "failed";

/// Names reserved for Platform Infra — remove must reject these.
const PROTECTED_NAMES: &[&str] = &["postgres", "coredns", "traefik"];

#[derive(Debug)]
pub enum DeployError {
    NotInitialized,
    AlreadyExists(String),
    NotFound(String),
    InvalidName(String),
    MissingImage,
    MissingPath,
    Docker(DockerError),
    Db(DbError),
}

impl std::fmt::Display for DeployError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeployError::NotInitialized => {
                write!(f, "platform is not initialized; run 'self-host init' first")
            }
            DeployError::AlreadyExists(name) => {
                write!(f, "Application '{name}' already exists")
            }
            DeployError::NotFound(id) => write!(f, "Application '{id}' not found"),
            DeployError::InvalidName(msg) => write!(f, "invalid Application name: {msg}"),
            DeployError::MissingImage => write!(f, "image is required"),
            DeployError::MissingPath => write!(f, "path is required"),
            DeployError::Docker(e) => write!(f, "{e}"),
            DeployError::Db(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DeployError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DeployError::Docker(e) => Some(e),
            DeployError::Db(e) => Some(e),
            _ => None,
        }
    }
}

impl From<DockerError> for DeployError {
    fn from(e: DockerError) -> Self {
        DeployError::Docker(e)
    }
}

impl From<DbError> for DeployError {
    fn from(e: DbError) -> Self {
        DeployError::Db(e)
    }
}

pub fn validate_app_name(name: &str) -> Result<(), DeployError> {
    if name.is_empty() {
        return Err(DeployError::InvalidName("must not be empty".into()));
    }
    if name.len() > 63 {
        return Err(DeployError::InvalidName(
            "must be at most 63 characters".into(),
        ));
    }
    let valid = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        return Err(DeployError::InvalidName(
            "must be lowercase alphanumeric and hyphens".into(),
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(DeployError::InvalidName(
            "must not start or end with a hyphen".into(),
        ));
    }
    Ok(())
}

pub fn default_hostname(name: &str, dns_suffix: &str) -> String {
    format!("{name}.{dns_suffix}")
}

/// `id` keys the Traefik router, `name` rides along as a label so an
/// Application is still findable in `docker ps` after a rename.
pub fn traefik_labels(id: &str, name: &str, hostname: &str) -> Vec<(String, String)> {
    vec![
        ("traefik.enable".into(), "true".into()),
        ("sf.app.id".into(), id.to_string()),
        ("sf.app.name".into(), name.to_string()),
        (
            format!("traefik.http.routers.{id}.rule"),
            format!("Host(`{hostname}`)"),
        ),
        (
            format!("traefik.http.routers.{id}.entrypoints"),
            "websecure".into(),
        ),
        (format!("traefik.http.routers.{id}.tls"), "true".into()),
        (
            format!("traefik.http.services.{id}.loadbalancer.server.port"),
            APP_CONTAINER_PORT.to_string(),
        ),
    ]
}

/// Records the outcome of a deploy against the row that was written *before*
/// the deploy started, then hands back the caller's result.
///
/// A failed deploy still leaves a row behind. That is the point: the user
/// typed a name and an image, and losing that on a bad image tag would mean
/// retyping it instead of fixing one field.
async fn record_outcome(
    store: &impl StateStore,
    mut record: ApplicationRecord,
    result: Result<(), DeployError>,
) -> Result<ApplicationRecord, DeployError> {
    match result {
        Ok(()) => {
            record.status = STATUS_RUNNING.into();
            record.last_error = None;
            store.insert_application(&record).await?;
            Ok(record)
        }
        Err(e) => {
            record.status = STATUS_FAILED.into();
            record.last_error = Some(e.to_string());
            // The deploy error is what the caller needs; a failure to write the
            // reason down must not replace it.
            let _ = store.insert_application(&record).await;
            Err(e)
        }
    }
}

async fn start_container(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    record: &ApplicationRecord,
) -> Result<(), DeployError> {
    let env = store
        .get_all_env(&record.id)
        .await?
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();

    docker
        .run_application(ApplicationContainer {
            name: container_name_for(&record.id),
            image: record.image.clone(),
            labels: traefik_labels(&record.id, &record.name, &record.hostname),
            network: PLATFORM_NETWORK.to_string(),
            ports: vec![],
            env,
        })
        .await?;
    Ok(())
}

/// Builds the row for a deploy, reusing the existing Application when the name
/// is already taken so that a redeploy keeps its id, and therefore its
/// container and its environment.
async fn pending_record(
    store: &impl StateStore,
    name: &str,
    image: String,
    source: &str,
    hostname_override: Option<&str>,
) -> Result<ApplicationRecord, DeployError> {
    let dns_suffix = store
        .get_state("dns_suffix")
        .await?
        .ok_or(DeployError::NotInitialized)?;

    let existing = store.find_application_by_name(name).await?;
    let hostname = match (hostname_override, &existing) {
        (Some(h), _) => h.to_string(),
        (None, Some(app)) => app.hostname.clone(),
        (None, None) => default_hostname(name, &dns_suffix),
    };

    Ok(ApplicationRecord {
        id: existing
            .as_ref()
            .map(|a| a.id.clone())
            .unwrap_or_else(generate_app_id),
        name: name.to_string(),
        hostname,
        image,
        status: STATUS_PENDING.into(),
        source: source.to_string(),
        last_error: None,
    })
}

/// A deploy that is on record but not yet carried out. The row is already
/// `pending`, so the caller can either finish it inline — the CLI, which must
/// report the outcome — or hand it to a task and answer straight away.
pub struct PendingDeploy {
    pub record: ApplicationRecord,
    work: DeployWork,
}

impl PendingDeploy {
    /// True when there is nothing left for Docker to do, and therefore nothing
    /// to wait for.
    pub fn is_settled(&self) -> bool {
        matches!(self.work, DeployWork::Settled)
    }
}

enum DeployWork {
    /// A rename of a running Application: the container and the Traefik router
    /// are keyed by id, so Docker sees no change at all.
    Settled,
    Pull,
    Build {
        path: String,
    },
    /// An existing container has to go before the new one can take its name.
    Recreate {
        pull: bool,
    },
}

/// Writes the `pending` row for an image deploy. Nothing has reached Docker
/// yet; `finish_deploy` is what does the pulling.
pub async fn prepare_deploy_from_image(
    store: &impl StateStore,
    name: &str,
    image: &str,
    hostname_override: Option<&str>,
) -> Result<PendingDeploy, DeployError> {
    validate_app_name(name)?;

    if image.is_empty() {
        return Err(DeployError::MissingImage);
    }

    if !store.is_initialized().await? {
        return Err(DeployError::NotInitialized);
    }

    let record = pending_record(store, name, image.to_string(), "image", hostname_override).await?;
    store.insert_application(&record).await?;

    Ok(PendingDeploy {
        record,
        work: DeployWork::Pull,
    })
}

/// Carries out the Docker half of a deploy and records how it went.
pub async fn finish_deploy(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    pending: PendingDeploy,
) -> Result<ApplicationRecord, DeployError> {
    let PendingDeploy { record, work } = pending;

    if matches!(work, DeployWork::Settled) {
        return Ok(record);
    }

    let result = async {
        docker.ensure_network(PLATFORM_NETWORK).await?;
        match &work {
            DeployWork::Pull => docker.pull_image(&record.image).await?,
            DeployWork::Build { path } => docker.build_image(path, &record.image).await?,
            DeployWork::Recreate { pull } => {
                if *pull {
                    docker.pull_image(&record.image).await?;
                }
                let _ = docker
                    .remove_container(&container_name_for(&record.id))
                    .await;
            }
            DeployWork::Settled => unreachable!(),
        }
        start_container(store, docker, &record).await
    }
    .await;

    record_outcome(store, record, result).await
}

/// A row left `pending` by a process that died mid-deploy is indistinguishable
/// from one still being pulled, so settle them at boot: whatever Docker is
/// actually running is `running`, and the rest are `failed` — on record, with
/// a reason, and one click from being deployed again.
pub async fn reconcile_pending(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
) -> Result<(), DeployError> {
    for mut app in store.list_applications().await? {
        if app.status != STATUS_PENDING {
            continue;
        }

        let running = docker
            .container_running(&container_name_for(&app.id))
            .await
            .unwrap_or(false);

        if running {
            app.status = STATUS_RUNNING.into();
            app.last_error = None;
        } else {
            tracing::warn!("Application {} was left mid-deploy by a restart", app.name);
            app.status = STATUS_FAILED.into();
            app.last_error = Some("deploy was interrupted by a platform restart".into());
        }

        store.insert_application(&app).await?;
    }

    Ok(())
}

pub async fn deploy_from_image(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    name: &str,
    image: &str,
    hostname_override: Option<&str>,
) -> Result<ApplicationRecord, DeployError> {
    let pending = prepare_deploy_from_image(store, name, image, hostname_override).await?;
    finish_deploy(store, docker, pending).await
}

pub async fn list_applications(
    store: &impl StateStore,
) -> Result<Vec<ApplicationRecord>, DeployError> {
    Ok(store.list_applications().await?)
}

pub async fn get_application(
    store: &impl StateStore,
    id: &str,
) -> Result<ApplicationRecord, DeployError> {
    store
        .get_application(id)
        .await?
        .ok_or_else(|| DeployError::NotFound(id.to_string()))
}

/// What the console is allowed to change on an existing Application. `None`
/// means "leave alone", so a form that only touches the image sends only the
/// image.
#[derive(Debug, Default)]
pub struct ApplicationUpdate {
    pub name: Option<String>,
    pub image: Option<String>,
    pub hostname: Option<String>,
}

/// Saves the change and redeploys.
///
/// A rename is only a row update: the container and the Traefik router are
/// keyed by id. Changing the image or the hostname does recreate the
/// container, because Docker cannot rewrite labels on a running one.
pub async fn update_application(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    id: &str,
    update: ApplicationUpdate,
) -> Result<ApplicationRecord, DeployError> {
    let pending = prepare_update(store, id, update).await?;
    finish_deploy(store, docker, pending).await
}

/// Saves the change and writes the row back as `pending`; the redeploy itself
/// is `finish_deploy`.
pub async fn prepare_update(
    store: &impl StateStore,
    id: &str,
    update: ApplicationUpdate,
) -> Result<PendingDeploy, DeployError> {
    if !store.is_initialized().await? {
        return Err(DeployError::NotInitialized);
    }

    let current = get_application(store, id).await?;
    let mut record = ApplicationRecord {
        name: update.name.clone().unwrap_or_else(|| current.name.clone()),
        image: update
            .image
            .clone()
            .unwrap_or_else(|| current.image.clone()),
        hostname: update
            .hostname
            .clone()
            .unwrap_or_else(|| current.hostname.clone()),
        status: STATUS_PENDING.into(),
        last_error: None,
        ..current.clone()
    };

    if record.name != current.name {
        validate_app_name(&record.name)?;
        if let Some(clash) = store.find_application_by_name(&record.name).await?
            && clash.id != current.id
        {
            return Err(DeployError::AlreadyExists(record.name.clone()));
        }
    }

    let image_changed = record.image != current.image;
    let hostname_changed = record.hostname != current.hostname;

    store.insert_application(&record).await?;

    // A rename alone changes nothing Docker can see, so leave the container be.
    if !image_changed && !hostname_changed && current.status == STATUS_RUNNING {
        record.status = STATUS_RUNNING.into();
        store.insert_application(&record).await?;
        return Ok(PendingDeploy {
            record,
            work: DeployWork::Settled,
        });
    }

    Ok(PendingDeploy {
        record,
        work: DeployWork::Recreate {
            pull: image_changed && current.source == "image",
        },
    })
}

/// Writes the `pending` row for a build-from-source deploy. The path rides
/// along in the work: it is the caller's, not something the record keeps.
pub async fn prepare_deploy_from_path(
    store: &impl StateStore,
    name: &str,
    path: &str,
    hostname_override: Option<&str>,
) -> Result<PendingDeploy, DeployError> {
    validate_app_name(name)?;

    if path.is_empty() {
        return Err(DeployError::MissingPath);
    }

    if !store.is_initialized().await? {
        return Err(DeployError::NotInitialized);
    }

    let image_tag = format!("self-host-{name}:latest");
    let record = pending_record(store, name, image_tag, "path", hostname_override).await?;
    store.insert_application(&record).await?;

    Ok(PendingDeploy {
        record,
        work: DeployWork::Build {
            path: path.to_string(),
        },
    })
}

pub async fn deploy_from_path(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    name: &str,
    path: &str,
    hostname_override: Option<&str>,
) -> Result<ApplicationRecord, DeployError> {
    let pending = prepare_deploy_from_path(store, name, path, hostname_override).await?;
    finish_deploy(store, docker, pending).await
}

pub fn container_name_for(app_id: &str) -> String {
    format!("{APP_PREFIX}{app_id}")
}

#[derive(Debug)]
pub enum RemoveError {
    NotInitialized,
    NotFound(String),
    ProtectedName(String),
    Docker(DockerError),
    Db(DbError),
}

impl std::fmt::Display for RemoveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoveError::NotInitialized => {
                write!(f, "platform is not initialized; run 'self-host init' first")
            }
            RemoveError::NotFound(name) => write!(f, "Application '{name}' not found"),
            RemoveError::ProtectedName(name) => {
                write!(
                    f,
                    "'{name}' is Platform Infra and cannot be removed as an Application"
                )
            }
            RemoveError::Docker(e) => write!(f, "{e}"),
            RemoveError::Db(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RemoveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RemoveError::Docker(e) => Some(e),
            RemoveError::Db(e) => Some(e),
            _ => None,
        }
    }
}

impl From<DockerError> for RemoveError {
    fn from(e: DockerError) -> Self {
        RemoveError::Docker(e)
    }
}

impl From<DbError> for RemoveError {
    fn from(e: DbError) -> Self {
        match e {
            DbError::NotFound(name) => RemoveError::NotFound(name),
            other => RemoveError::Db(other),
        }
    }
}

pub async fn remove_application(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    name: &str,
) -> Result<(), RemoveError> {
    if PROTECTED_NAMES.contains(&name) {
        return Err(RemoveError::ProtectedName(name.to_string()));
    }

    if !store.is_initialized().await? {
        return Err(RemoveError::NotInitialized);
    }

    let app = store
        .find_application_by_name(name)
        .await?
        .ok_or_else(|| RemoveError::NotFound(name.to_string()))?;

    store.delete_application(&app.id).await?;

    let container_name = container_name_for(&app.id);
    let _ = docker.remove_container(&container_name).await;

    Ok(())
}

// ── Env ────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum EnvError {
    NotInitialized,
    NotFound(String),
    Docker(DockerError),
    Db(DbError),
}

impl std::fmt::Display for EnvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnvError::NotInitialized => {
                write!(f, "platform is not initialized; run 'self-host init' first")
            }
            EnvError::NotFound(name) => write!(f, "Application '{name}' not found"),
            EnvError::Docker(e) => write!(f, "{e}"),
            EnvError::Db(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for EnvError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EnvError::Docker(e) => Some(e),
            EnvError::Db(e) => Some(e),
            _ => None,
        }
    }
}

impl From<DockerError> for EnvError {
    fn from(e: DockerError) -> Self {
        EnvError::Docker(e)
    }
}

impl From<DbError> for EnvError {
    fn from(e: DbError) -> Self {
        EnvError::Db(e)
    }
}

pub async fn set_env(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    app_name: &str,
    key: &str,
    value: &str,
) -> Result<(), EnvError> {
    if !store.is_initialized().await? {
        return Err(EnvError::NotInitialized);
    }

    if !store.application_exists(app_name).await? {
        return Err(EnvError::NotFound(app_name.to_string()));
    }

    store.set_env(app_name, key, value).await?;

    // Apply: recreate container with updated env
    recreate_with_env(store, docker, app_name).await?;

    Ok(())
}

pub async fn get_all_env(
    store: &impl StateStore,
    app_name: &str,
) -> Result<Vec<(String, String)>, EnvError> {
    if !store.application_exists(app_name).await? {
        return Err(EnvError::NotFound(app_name.to_string()));
    }
    Ok(store.get_all_env(app_name).await?)
}

pub async fn unset_env(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    app_name: &str,
    key: &str,
) -> Result<(), EnvError> {
    if !store.is_initialized().await? {
        return Err(EnvError::NotInitialized);
    }

    if !store.application_exists(app_name).await? {
        return Err(EnvError::NotFound(app_name.to_string()));
    }

    store.unset_env(app_name, key).await?;

    // Apply: recreate container with updated env
    recreate_with_env(store, docker, app_name).await?;

    Ok(())
}

async fn recreate_with_env(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    app_name: &str,
) -> Result<(), EnvError> {
    let apps = store.list_applications().await?;
    let app = apps
        .iter()
        .find(|a| a.name == app_name)
        .ok_or_else(|| EnvError::NotFound(app_name.to_string()))?;

    let env_vars = store
        .get_all_env(&app.id)
        .await?
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>();

    let hostname = &app.hostname;
    let container_name = container_name_for(&app.id);

    docker.remove_container(&container_name).await?;
    docker
        .run_application(ApplicationContainer {
            name: container_name,
            image: app.image.clone(),
            labels: traefik_labels(&app.id, &app.name, hostname),
            network: PLATFORM_NETWORK.to_string(),
            ports: vec![],
            env: env_vars,
        })
        .await?;

    Ok(())
}

// ── Logs ───────────────────────────────────────────────────────

#[derive(Debug)]
pub enum LogsError {
    NotInitialized,
    NotFound(String),
    Docker(DockerError),
    Db(DbError),
}

impl std::fmt::Display for LogsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogsError::NotInitialized => {
                write!(f, "platform is not initialized; run 'self-host init' first")
            }
            LogsError::NotFound(name) => write!(f, "Application '{name}' not found"),
            LogsError::Docker(e) => write!(f, "{e}"),
            LogsError::Db(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LogsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LogsError::Docker(e) => Some(e),
            LogsError::Db(e) => Some(e),
            _ => None,
        }
    }
}

impl From<DockerError> for LogsError {
    fn from(e: DockerError) -> Self {
        LogsError::Docker(e)
    }
}

impl From<DbError> for LogsError {
    fn from(e: DbError) -> Self {
        LogsError::Db(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::FakeStateStore;
    use crate::docker::FakeDocker;

    async fn initialized_store() -> FakeStateStore {
        let store = FakeStateStore::new();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        store
    }

    #[tokio::test]
    async fn preparing_a_deploy_records_it_without_touching_docker() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();

        let pending = prepare_deploy_from_image(&store, "blog", "nginx:alpine", None)
            .await
            .unwrap();

        assert_eq!(pending.record.status, STATUS_PENDING);
        assert!(!pending.is_settled());
        assert!(docker.pulled.lock().unwrap().is_empty());
        assert!(docker.deployed_apps().is_empty());

        let deployed = finish_deploy(&store, &docker, pending).await.unwrap();
        assert_eq!(deployed.status, STATUS_RUNNING);
        assert_eq!(docker.pulled.lock().unwrap().as_slice(), ["nginx:alpine"]);
    }

    #[tokio::test]
    async fn reconcile_keeps_a_pending_application_whose_container_is_up() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        deploy_from_image(&store, &docker, "blog", "nginx:alpine", None)
            .await
            .unwrap();

        // Back to pending, as a process that died mid-deploy would leave it.
        let mut app = store
            .find_application_by_name("blog")
            .await
            .unwrap()
            .unwrap();
        app.status = STATUS_PENDING.into();
        store.insert_application(&app).await.unwrap();

        reconcile_pending(&store, &docker).await.unwrap();

        let settled = store
            .find_application_by_name("blog")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(settled.status, STATUS_RUNNING);
        assert_eq!(settled.last_error, None);
    }

    #[tokio::test]
    async fn reconcile_fails_a_pending_application_with_no_container() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let pending = prepare_deploy_from_image(&store, "blog", "nginx:alpine", None)
            .await
            .unwrap();
        // The row is written; the deploy never ran.
        drop(pending);

        reconcile_pending(&store, &docker).await.unwrap();

        let settled = store
            .find_application_by_name("blog")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(settled.status, STATUS_FAILED);
        assert!(settled.last_error.unwrap().contains("restart"));
    }

    #[test]
    fn default_hostname_uses_dns_suffix() {
        assert_eq!(default_hostname("blog", "home.lan"), "blog.home.lan");
    }

    #[test]
    fn traefik_labels_enable_host_routing_without_implying_host_ports() {
        let labels = traefik_labels("k3n8qz4v2x1p", "blog", "blog.home.lan");
        assert!(
            labels
                .iter()
                .any(|(k, v)| k == "traefik.enable" && v == "true")
        );
        assert!(labels.iter().any(|(k, v)| {
            k == "traefik.http.routers.k3n8qz4v2x1p.rule" && v == "Host(`blog.home.lan`)"
        }));
        assert!(labels.iter().any(|(k, v)| {
            k == "traefik.http.routers.k3n8qz4v2x1p.entrypoints" && v == "websecure"
        }));
        assert!(
            labels
                .iter()
                .any(|(k, v)| { k == "traefik.http.routers.k3n8qz4v2x1p.tls" && v == "true" })
        );
    }

    #[test]
    fn validate_app_name_rejects_uppercase() {
        assert!(validate_app_name("Blog").is_err());
    }

    #[test]
    fn validate_app_name_accepts_simple_name() {
        assert!(validate_app_name("blog").is_ok());
    }
}
