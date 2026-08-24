use crate::db::{DbError, StateStore};
use crate::docker::{ApplicationContainer, DockerError, DockerRuntime, PLATFORM_NETWORK};
use crate::routes::{RouteError, RouteStore};

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
    InvalidHostname(String),
    MissingImage,
    MissingPath,
    Docker(DockerError),
    Routing(RouteError),
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
            DeployError::InvalidHostname(msg) => {
                write!(f, "invalid Application Hostname: {msg}")
            }
            DeployError::MissingImage => write!(f, "image is required"),
            DeployError::MissingPath => write!(f, "path is required"),
            DeployError::Docker(e) => write!(f, "{e}"),
            DeployError::Routing(e) => write!(f, "{e}"),
            DeployError::Db(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DeployError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DeployError::Docker(e) => Some(e),
            DeployError::Routing(e) => Some(e),
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

impl From<RouteError> for DeployError {
    fn from(e: RouteError) -> Self {
        DeployError::Routing(e)
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

/// A Hostname ends up inside a Traefik `Host(`…`)` rule, so anything that is
/// not a DNS name is refused here rather than written into the router file.
pub fn validate_hostname(hostname: &str) -> Result<(), DeployError> {
    if hostname.is_empty() {
        return Err(DeployError::InvalidHostname("must not be empty".into()));
    }
    if hostname.len() > 253 {
        return Err(DeployError::InvalidHostname(
            "must be at most 253 characters".into(),
        ));
    }
    let valid = hostname
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.');
    if !valid {
        return Err(DeployError::InvalidHostname(
            "must be lowercase alphanumeric, hyphens and dots".into(),
        ));
    }
    if hostname.starts_with(['-', '.']) || hostname.ends_with(['-', '.']) {
        return Err(DeployError::InvalidHostname(
            "must not start or end with a hyphen or a dot".into(),
        ));
    }
    Ok(())
}

/// Checks the Hostname and every alias an Application is about to be saved
/// with, and refuses an alias that another Application already answers on.
async fn validate_routing(
    store: &impl StateStore,
    record: &ApplicationRecord,
) -> Result<(), DeployError> {
    for hostname in crate::routes::hostnames(record) {
        validate_hostname(hostname)?;
    }

    for other in store.list_applications().await? {
        if other.id == record.id {
            continue;
        }
        for taken in crate::routes::hostnames(&other) {
            if crate::routes::hostnames(record).contains(&taken) {
                return Err(DeployError::InvalidHostname(format!(
                    "'{taken}' is already answered by Application '{}'",
                    other.name
                )));
            }
        }
    }

    Ok(())
}

/// What the container says about itself. Routing is not in here: it lives in
/// a Traefik file-provider route (ADR-0009), so that changing where an
/// Application answers never has to recreate a container.
pub fn identity_labels(id: &str, name: &str) -> Vec<(String, String)> {
    vec![
        ("sf.app.id".into(), id.to_string()),
        ("sf.app.name".into(), name.to_string()),
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
            labels: identity_labels(&record.id, &record.name),
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
    aliases: Option<&[String]>,
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
    let aliases = match (aliases, &existing) {
        (Some(a), _) => a.to_vec(),
        (None, Some(app)) => app.aliases.clone(),
        (None, None) => vec![],
    };

    let record = ApplicationRecord {
        id: existing
            .as_ref()
            .map(|a| a.id.clone())
            .unwrap_or_else(generate_app_id),
        name: name.to_string(),
        hostname,
        aliases,
        image,
        status: STATUS_PENDING.into(),
        source: source.to_string(),
        last_error: None,
    };

    validate_routing(store, &record).await?;

    Ok(record)
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
    /// Nothing for Docker to do: a rename, or a change of Hostname or
    /// aliases. The container is keyed by id and the router is a file, so
    /// only the route has to be rewritten.
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
    aliases: Option<&[String]>,
) -> Result<PendingDeploy, DeployError> {
    validate_app_name(name)?;

    if image.is_empty() {
        return Err(DeployError::MissingImage);
    }

    if !store.is_initialized().await? {
        return Err(DeployError::NotInitialized);
    }

    let record = pending_record(
        store,
        name,
        image.to_string(),
        "image",
        hostname_override,
        aliases,
    )
    .await?;
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
    routes: &(impl RouteStore + ?Sized),
    pending: PendingDeploy,
) -> Result<ApplicationRecord, DeployError> {
    let PendingDeploy { record, work } = pending;

    // Nothing for Docker, but the route may be exactly what changed.
    if matches!(work, DeployWork::Settled) {
        routes.publish(&record)?;
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
        start_container(store, docker, &record).await?;
        // Published after the container is up, so Traefik never routes at a
        // backend that is not there yet.
        routes.publish(&record)?;
        Ok(())
    }
    .await;

    record_outcome(store, record, result).await
}

/// Brings Docker and Traefik back in line with the database at boot.
///
/// A row left `pending` by a process that died mid-deploy is indistinguishable
/// from one still being pulled, so settle them: whatever Docker is actually
/// running is `running`, and the rest are `failed` — on record, with a reason,
/// and one click from being deployed again.
///
/// Every Application that ends up running then has its route rewritten. The
/// dynamic directory is a projection of the database (ADR-0009), so losing it
/// costs a restart rather than a redeploy, and an install upgrading from
/// label-based routing gets its route files without touching a container.
pub async fn reconcile(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    routes: &(impl RouteStore + ?Sized),
) -> Result<(), DeployError> {
    for mut app in store.list_applications().await? {
        if app.status == STATUS_PENDING {
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

        if app.status == STATUS_RUNNING {
            routes.publish(&app)?;
        }
    }

    Ok(())
}

pub async fn deploy_from_image(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    routes: &(impl RouteStore + ?Sized),
    name: &str,
    image: &str,
    hostname_override: Option<&str>,
    aliases: Option<&[String]>,
) -> Result<ApplicationRecord, DeployError> {
    let pending = prepare_deploy_from_image(store, name, image, hostname_override, aliases).await?;
    finish_deploy(store, docker, routes, pending).await
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
    pub aliases: Option<Vec<String>>,
}

/// Saves the change and redeploys.
///
/// A rename is only a row update, and so is a change of Hostname or aliases:
/// the container is keyed by id and the router is a file Traefik watches. Only
/// a new image recreates the container.
pub async fn update_application(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    routes: &(impl RouteStore + ?Sized),
    id: &str,
    update: ApplicationUpdate,
) -> Result<ApplicationRecord, DeployError> {
    let pending = prepare_update(store, id, update).await?;
    finish_deploy(store, docker, routes, pending).await
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
        aliases: update
            .aliases
            .clone()
            .unwrap_or_else(|| current.aliases.clone()),
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

    validate_routing(store, &record).await?;

    let image_changed = record.image != current.image;

    store.insert_application(&record).await?;

    // Only the image is Docker's business. A rename, a new Hostname and an
    // added alias are all a route rewrite, which costs no downtime.
    if !image_changed && current.status == STATUS_RUNNING {
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
    aliases: Option<&[String]>,
) -> Result<PendingDeploy, DeployError> {
    validate_app_name(name)?;

    if path.is_empty() {
        return Err(DeployError::MissingPath);
    }

    if !store.is_initialized().await? {
        return Err(DeployError::NotInitialized);
    }

    let image_tag = format!("self-host-{name}:latest");
    let record = pending_record(store, name, image_tag, "path", hostname_override, aliases).await?;
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
    routes: &(impl RouteStore + ?Sized),
    name: &str,
    path: &str,
    hostname_override: Option<&str>,
    aliases: Option<&[String]>,
) -> Result<ApplicationRecord, DeployError> {
    let pending = prepare_deploy_from_path(store, name, path, hostname_override, aliases).await?;
    finish_deploy(store, docker, routes, pending).await
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
    Routing(RouteError),
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
            RemoveError::Routing(e) => write!(f, "{e}"),
            RemoveError::Db(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RemoveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RemoveError::Docker(e) => Some(e),
            RemoveError::Routing(e) => Some(e),
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

impl From<RouteError> for RemoveError {
    fn from(e: RouteError) -> Self {
        RemoveError::Routing(e)
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
    routes: &(impl RouteStore + ?Sized),
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

    // The route goes first: a Hostname still answering for an Application that
    // is on its way out is worse than one that stops a moment early.
    routes.withdraw(&app.id)?;

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

    let container_name = container_name_for(&app.id);

    docker.remove_container(&container_name).await?;
    docker
        .run_application(ApplicationContainer {
            name: container_name,
            image: app.image.clone(),
            labels: identity_labels(&app.id, &app.name),
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
    use crate::routes::FakeRoutes;

    async fn initialized_store() -> FakeStateStore {
        let store = FakeStateStore::new();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        store
    }

    #[tokio::test]
    async fn preparing_a_deploy_records_it_without_touching_docker() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();

        let routes = FakeRoutes::new();

        let pending = prepare_deploy_from_image(&store, "blog", "nginx:alpine", None, None)
            .await
            .unwrap();

        assert_eq!(pending.record.status, STATUS_PENDING);
        assert!(!pending.is_settled());
        assert!(docker.pulled.lock().unwrap().is_empty());
        assert!(docker.deployed_apps().is_empty());
        assert!(routes.ids().is_empty());

        let deployed = finish_deploy(&store, &docker, &routes, pending)
            .await
            .unwrap();
        assert_eq!(deployed.status, STATUS_RUNNING);
        assert_eq!(docker.pulled.lock().unwrap().as_slice(), ["nginx:alpine"]);
        assert!(routes.get(&deployed.id).unwrap().contains("blog.home.lan"));
    }

    #[tokio::test]
    async fn reconcile_keeps_a_pending_application_whose_container_is_up() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        deploy_from_image(&store, &docker, &routes, "blog", "nginx:alpine", None, None)
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

        reconcile(&store, &docker, &routes).await.unwrap();

        let settled = store
            .find_application_by_name("blog")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(settled.status, STATUS_RUNNING);
        assert_eq!(settled.last_error, None);
        assert!(routes.get(&settled.id).is_some());
    }

    #[tokio::test]
    async fn reconcile_fails_a_pending_application_with_no_container() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let pending = prepare_deploy_from_image(&store, "blog", "nginx:alpine", None, None)
            .await
            .unwrap();
        // The row is written; the deploy never ran.
        drop(pending);

        reconcile(&store, &docker, &routes).await.unwrap();

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
    fn a_container_carries_its_identity_and_no_routing() {
        let labels = identity_labels("k3n8qz4v2x1p", "blog");
        assert!(
            labels
                .iter()
                .any(|(k, v)| k == "sf.app.id" && v == "k3n8qz4v2x1p")
        );
        assert!(
            labels
                .iter()
                .any(|(k, v)| k == "sf.app.name" && v == "blog")
        );
        assert!(
            !labels.iter().any(|(k, _)| k.starts_with("traefik.")),
            "routing belongs to the file provider, not to a label"
        );
    }

    #[tokio::test]
    async fn changing_the_hostname_rewrites_the_route_and_keeps_the_container() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();

        let app = deploy_from_image(&store, &docker, &routes, "blog", "nginx:alpine", None, None)
            .await
            .unwrap();
        let deploys_before = docker.deployed_apps().len();

        let updated = update_application(
            &store,
            &docker,
            &routes,
            &app.id,
            ApplicationUpdate {
                hostname: Some("writing.home.lan".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(updated.status, STATUS_RUNNING);
        assert_eq!(
            docker.deployed_apps().len(),
            deploys_before,
            "a Hostname change must not recreate the container"
        );
        assert!(routes.get(&app.id).unwrap().contains("writing.home.lan"));
    }

    #[tokio::test]
    async fn an_alias_keeps_the_old_hostname_answering_after_a_hostname_change() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();

        let app = deploy_from_image(&store, &docker, &routes, "blog", "nginx:alpine", None, None)
            .await
            .unwrap();

        update_application(
            &store,
            &docker,
            &routes,
            &app.id,
            ApplicationUpdate {
                hostname: Some("writing.home.lan".into()),
                aliases: Some(vec!["blog.home.lan".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let route = routes.get(&app.id).unwrap();
        assert!(route.contains("Host(`writing.home.lan`) || Host(`blog.home.lan`)"));
    }

    #[tokio::test]
    async fn removing_an_application_takes_its_route_down() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();

        let app = deploy_from_image(&store, &docker, &routes, "blog", "nginx:alpine", None, None)
            .await
            .unwrap();
        assert!(routes.get(&app.id).is_some());

        remove_application(&store, &docker, &routes, "blog")
            .await
            .unwrap();

        assert!(routes.get(&app.id).is_none());
    }

    #[tokio::test]
    async fn an_alias_another_application_already_answers_on_is_refused() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();

        deploy_from_image(&store, &docker, &routes, "blog", "nginx:alpine", None, None)
            .await
            .unwrap();
        let shop = deploy_from_image(&store, &docker, &routes, "shop", "nginx:alpine", None, None)
            .await
            .unwrap();

        let clash = update_application(
            &store,
            &docker,
            &routes,
            &shop.id,
            ApplicationUpdate {
                aliases: Some(vec!["blog.home.lan".into()]),
                ..Default::default()
            },
        )
        .await;

        assert!(matches!(clash, Err(DeployError::InvalidHostname(_))));
    }

    #[test]
    fn validate_hostname_rejects_a_traefik_rule_escape() {
        assert!(validate_hostname("blog.home.lan`) || Host(`admin.home.lan").is_err());
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
