use crate::compose_app::{self, ComposeDefinition, ComposeDefinitionError, ComposeProject};
use crate::docker::{APP_NETWORK, ApplicationContainer, DockerError, DockerRuntime};
use crate::error::ErrorReport;
use crate::ports;
use crate::routes::RouteStore;
use crate::store::{DevelopmentApplication, StateStore, StoreError};

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

pub use crate::store::ApplicationRecord;

/// The states an Application row can be in. `pending` is written before any
/// Docker work starts, so a crash mid-deploy is visible rather than silent.
/// `stopped` is the Operator's doing and survives a Platform restart: a
/// stopped Application is not brought back until asked.
pub const STATUS_PENDING: &str = "pending";
pub const STATUS_RUNNING: &str = "running";
pub const STATUS_FAILED: &str = "failed";
pub const STATUS_STOPPED: &str = "stopped";

/// How an Application was defined. A Compose Application (ADR-0014) carries
/// its definition on the row; the other two carry an image reference.
pub const SOURCE_IMAGE: &str = "image";
pub const SOURCE_PATH: &str = "path";
pub const SOURCE_COMPOSE: &str = "compose";

/// Names reserved for Platform Infra — remove must reject these. Only the
/// proxy is left: DNS moved into the binary (ADR-0017) and the state store
/// into files (ADR-0018), so "coredns" and "postgres" are names an Operator
/// may now give an Application of their own.

#[derive(Debug)]
pub enum DeployError {
    NotInitialized,
    AlreadyExists(String),
    NotFound(String),
    InvalidName(String),
    InvalidHostname(String),
    MissingImage,
    MissingPath,
    MissingCompose,
    InvalidDevelopment(String),
    InvalidCompose(ComposeDefinitionError),
    Docker(DockerError),
    /// No Host port left for the Application's Web Target to answer on.
    NoWebTargetPort(crate::ports::NoPortAvailable),
    Store(StoreError),
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
            DeployError::MissingCompose => write!(f, "a Compose definition is required"),
            DeployError::InvalidDevelopment(message) => {
                write!(f, "invalid development image: {message}")
            }
            DeployError::InvalidCompose(_) => write!(f, "invalid Compose definition"),
            DeployError::Docker(_) => write!(f, "failed to deploy the Application"),
            DeployError::NoWebTargetPort(_) => {
                write!(f, "failed to publish the Application on the LAN")
            }
            DeployError::Store(_) => write!(f, "failed to record the Application"),
        }
    }
}

impl std::error::Error for DeployError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DeployError::InvalidCompose(e) => Some(e),
            DeployError::Docker(e) => Some(e),
            DeployError::NoWebTargetPort(e) => Some(e),
            DeployError::Store(e) => Some(e),
            _ => None,
        }
    }
}

impl From<DockerError> for DeployError {
    fn from(e: DockerError) -> Self {
        DeployError::Docker(e)
    }
}

impl From<StoreError> for DeployError {
    fn from(e: StoreError) -> Self {
        DeployError::Store(e)
    }
}

impl From<ComposeDefinitionError> for DeployError {
    fn from(e: ComposeDefinitionError) -> Self {
        DeployError::InvalidCompose(e)
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

/// A Hostname is matched against the `Host` header of every request, so
/// anything that is
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
/// the Platform's route table (ADR-0019), so that changing where an
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
            record.last_error = Some(ErrorReport::new(&e));
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
            network: APP_NETWORK.to_string(),
            ports: web_target_publication(record),
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
    definition: Option<ComposeSpec>,
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

    let web_target_port = match existing.as_ref().and_then(|app| app.web_target_port) {
        Some(port) => port,
        None => allocate_web_target_port(store).await?,
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
        compose: definition.as_ref().map(|d| d.compose.clone()),
        web_service: definition.as_ref().and_then(|d| d.web_service.clone()),
        web_port: definition.as_ref().and_then(|d| d.web_port),
        web_target_port: Some(web_target_port),
        development: None,
    };

    validate_routing(store, &record).await?;

    Ok(record)
}

/// The generated Compose file for a development Application. It is stored for
/// deployment, while `DevelopmentApplication` retains the form settings.
pub fn development_compose(settings: &DevelopmentApplication) -> String {
    let command = settings.command.replace('$', "$$");
    [
        "services:".to_string(),
        "  web:".to_string(),
        format!(
            "    image: {}",
            serde_json::to_string(&settings.tag).unwrap()
        ),
        format!(
            "    command: [\"sh\", \"-c\", {}]",
            serde_json::to_string(&command).unwrap()
        ),
        format!("    expose: [{}]", settings.web_port),
        if settings.persist_data {
            "    volumes:\n      - data:/data\nvolumes:\n  data: {}".to_string()
        } else {
            String::new()
        },
        String::new(),
    ]
    .join("\n")
}

fn validate_development(settings: &DevelopmentApplication) -> Result<(), DeployError> {
    if settings.image_id.is_empty() || settings.image_id.len() > 128 {
        return Err(DeployError::InvalidDevelopment(
            "image ID is required".into(),
        ));
    }
    if settings.tag.is_empty() || settings.tag.len() > 512 {
        return Err(DeployError::InvalidDevelopment(
            "image tag is required".into(),
        ));
    }
    if settings.command.trim().is_empty() || settings.command.len() > 16 * 1024 {
        return Err(DeployError::InvalidDevelopment(
            "start command is required".into(),
        ));
    }
    if settings.web_port == 0 {
        return Err(DeployError::InvalidDevelopment(
            "web port must be between 1 and 65535".into(),
        ));
    }
    Ok(())
}

/// A Host port for this Application's Web Target, avoiding every port
/// already handed out. A redeploy keeps the port it was given: nothing
/// outside the Platform depends on the number, but an Operator reading
/// `docker ps` should not find it different every time.
async fn allocate_web_target_port(store: &impl StateStore) -> Result<u16, DeployError> {
    let taken = store
        .list_applications()
        .await?
        .iter()
        .filter_map(|app| app.web_target_port)
        .collect();
    ports::allocate(&taken).map_err(DeployError::NoWebTargetPort)
}

/// Where the Web Target answers on the Host, as Docker publishes it. Empty
/// until the Application has a port, which is every Application deployed
/// before the proxy moved into the binary.
fn web_target_publication(record: &ApplicationRecord) -> Vec<String> {
    record
        .web_target_port
        .map(|host_port| {
            vec![ports::publication(
                host_port,
                record.web_port.unwrap_or(APP_CONTAINER_PORT),
            )]
        })
        .unwrap_or_default()
}

/// What a Compose deploy carries besides a name: the file, the image behind
/// the Hostname and where in the file the Hostname points.
struct ComposeSpec {
    compose: String,
    image: String,
    web_service: Option<String>,
    web_port: Option<u16>,
}

/// A deploy that is on record but not yet carried out. The row is already
/// `pending`, so the caller can either finish it inline — the CLI, which must
/// report the outcome — or hand it to a task and answer straight away.
#[derive(Debug)]
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

#[derive(Debug)]
enum DeployWork {
    /// Nothing for Docker to do: a rename, or a change of Hostname or
    /// aliases. The container is keyed by id and the route is a table entry,
    /// so only the route has to be rewritten.
    Settled,
    Pull,
    Build {
        path: String,
    },
    /// An existing container has to go before the new one can take its name.
    Recreate {
        pull: bool,
    },
    /// `docker compose up`: Compose pulls what is missing and recreates only
    /// the services whose definition changed.
    ComposeUp,
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
        SOURCE_IMAGE,
        hostname_override,
        aliases,
        None,
    )
    .await?;
    store.insert_application(&record).await?;

    Ok(PendingDeploy {
        record,
        work: DeployWork::Pull,
    })
}

/// Checks a Compose definition and resolves where its Hostname points. The
/// resolved target is what gets recorded, so the route never has to guess
/// again: what the console shows is what the proxy uses.
fn check_compose(
    compose: &str,
    web_service: Option<&str>,
    web_port: Option<u16>,
) -> Result<ComposeSpec, DeployError> {
    if compose.trim().is_empty() {
        return Err(DeployError::MissingCompose);
    }
    let definition = ComposeDefinition::parse(compose)?;
    // A form sends the field it shows, empty or not. Empty means "the
    // default", the same as leaving it out.
    let web_service = web_service.map(str::trim).filter(|s| !s.is_empty());
    let target = definition.web_target(web_service, web_port)?;
    let image = definition
        .service(&target.service)
        .map(|s| s.image.clone())
        .unwrap_or_default();
    Ok(ComposeSpec {
        compose: compose.to_string(),
        image,
        web_service: Some(target.service),
        web_port: Some(target.port),
    })
}

/// Writes the `pending` row for a Compose deploy. The definition is checked
/// here, before anything is recorded, so a file the Platform will not run is
/// refused with the reason and nothing to clean up.
pub async fn prepare_deploy_from_compose(
    store: &impl StateStore,
    name: &str,
    compose: &str,
    web_service: Option<&str>,
    web_port: Option<u16>,
    hostname_override: Option<&str>,
    aliases: Option<&[String]>,
) -> Result<PendingDeploy, DeployError> {
    validate_app_name(name)?;

    let spec = check_compose(compose, web_service, web_port)?;

    if !store.is_initialized().await? {
        return Err(DeployError::NotInitialized);
    }

    let record = pending_record(
        store,
        name,
        spec.image.clone(),
        SOURCE_COMPOSE,
        hostname_override,
        aliases,
        Some(spec),
    )
    .await?;
    store.insert_application(&record).await?;

    Ok(PendingDeploy {
        record,
        work: DeployWork::ComposeUp,
    })
}

/// Records a development Application as generated Compose plus explicit form
/// metadata. This path is the only one that attaches that metadata.
pub async fn prepare_deploy_from_development(
    store: &impl StateStore,
    name: &str,
    development: DevelopmentApplication,
    hostname_override: Option<&str>,
    aliases: Option<&[String]>,
) -> Result<PendingDeploy, DeployError> {
    validate_app_name(name)?;
    validate_development(&development)?;
    let compose = development_compose(&development);
    let spec = check_compose(&compose, Some("web"), Some(development.web_port))?;
    if spec.image != development.tag {
        return Err(DeployError::InvalidDevelopment(
            "the image tag does not match its generated Compose definition".into(),
        ));
    }
    if !store.is_initialized().await? {
        return Err(DeployError::NotInitialized);
    }
    let mut record = pending_record(
        store,
        name,
        spec.image.clone(),
        SOURCE_COMPOSE,
        hostname_override,
        aliases,
        Some(spec),
    )
    .await?;
    record.development = Some(development);
    store.insert_application(&record).await?;
    Ok(PendingDeploy {
        record,
        work: DeployWork::ComposeUp,
    })
}

/// The Compose project name doubles as the container prefix, so `docker ps`
/// reads `sf-app-<id>-<service>` for every service of the Application.
pub fn project_name_for(app_id: &str) -> String {
    container_name_for(app_id)
}

/// Where a Compose Application keeps its file and its data.
pub fn project_dir_for(app_id: &str) -> std::path::PathBuf {
    crate::paths::platform_config_dir()
        .join("apps")
        .join(app_id)
}

/// Renders the project the runtime runs for a Compose Application, with the
/// Platform's environment on top of the file's own.
pub async fn project_for(
    store: &impl StateStore,
    record: &ApplicationRecord,
) -> Result<ComposeProject, DeployError> {
    let compose = record
        .compose
        .as_deref()
        .ok_or(DeployError::MissingCompose)?;
    let definition = ComposeDefinition::parse(compose)?;
    let env = store.get_all_env(&record.id).await?;
    let published = match record.web_target_port {
        Some(host_port) => Some(compose_app::PublishedTarget {
            target: definition.web_target(record.web_service.as_deref(), record.web_port)?,
            host_port,
        }),
        None => None,
    };
    let overrides = record
        .development
        .as_ref()
        .map(|_| compose_app::RenderOverrides::service_hostname("web", &record.name))
        .unwrap_or_default();
    Ok(definition.render_with_overrides(
        &project_name_for(&record.id),
        &project_dir_for(&record.id),
        &identity_labels(&record.id, &record.name),
        &env,
        published.as_ref(),
        &overrides,
    ))
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
        routes.publish(&record);
        return Ok(record);
    }

    let result = async {
        docker.ensure_network(APP_NETWORK).await?;
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
            DeployWork::ComposeUp => {
                let project = project_for(store, &record).await?;
                docker.compose_up(&project).await?;
                routes.publish(&record);
                return Ok(());
            }
            DeployWork::Settled => unreachable!(),
        }
        start_container(store, docker, &record).await?;
        // Published after the container is up, so the proxy never routes at
        // a backend that is not there yet.
        routes.publish(&record);
        Ok(())
    }
    .await;

    record_outcome(store, record, result).await
}

/// Gives an Application deployed before the Platform served HTTP itself a Host
/// port for its Web Target, and recreates its workload so Docker publishes it.
///
/// A container cannot start publishing a port it was created without, so this
/// is a recreate — of the workload only. Volumes, the Application's data and
/// everything on record survive it, which is the whole difference between
/// this and asking the Operator to deploy again.
async fn give_web_target_port(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    app: &mut ApplicationRecord,
) -> Result<u16, DeployError> {
    let port = allocate_web_target_port(store).await?;
    // On a copy until it works: a port on the record that nothing answers on
    // would route every Consumer at a closed socket, which is worse than the
    // 503 they get from a record with no port at all.
    let mut published = app.clone();
    published.web_target_port = Some(port);

    if published.source == SOURCE_COMPOSE {
        docker
            .compose_up(&project_for(store, &published).await?)
            .await?;
    } else {
        // Already gone is the state the recreate wanted.
        let _ = docker
            .remove_container(&container_name_for(&published.id))
            .await;
        start_container(store, docker, &published).await?;
    }

    store.insert_application(&published).await?;
    *app = published;
    Ok(port)
}

/// Brings Docker and the route table back in line with what is on record at
/// boot.
///
/// A row left `pending` by a process that died mid-deploy is indistinguishable
/// from one still being pulled, so settle them: whatever Docker is actually
/// running is `running`, and the rest are `failed` — on record, with a reason,
/// and one click from being deployed again.
///
/// Every Application that ends up running then has its route published. The
/// table is built from what is on record and lives only in memory, so this is
/// also the only thing that puts it there after a restart.
pub async fn reconcile(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    routes: &(impl RouteStore + ?Sized),
) -> Result<(), DeployError> {
    // An executor that cannot be reached observes nothing. Settling a deploy
    // against that silence would report every Application as failed because
    // Docker is down, so an interrupted deploy stays pending and stays visible.
    let executor_available = docker.ping().await.is_ok();
    if !executor_available {
        tracing::warn!(
            "Application execution is unavailable, so interrupted deploys stay pending; \
             saved configuration is untouched"
        );
    }

    for mut app in store.list_applications().await? {
        if app.status == STATUS_PENDING && executor_available {
            let states = service_states(docker, &app).await;
            let running = !states.is_empty() && states.iter().all(|s| s.state == "running");

            if running {
                app.status = STATUS_RUNNING.into();
                app.last_error = None;
            } else {
                tracing::warn!("Application {} was left mid-deploy by a restart", app.name);
                app.status = STATUS_FAILED.into();
                app.last_error = Some(ErrorReport::plain(
                    "deploy was interrupted by a platform restart",
                ));
            }

            store.insert_application(&app).await?;
        }

        if app.status == STATUS_RUNNING && app.web_target_port.is_none() && executor_available {
            match give_web_target_port(store, docker, &mut app).await {
                Ok(port) => tracing::info!(
                    "Application {} answers on Host port {port} now; its workload was recreated \
                     to publish it",
                    app.name
                ),
                Err(e) => tracing::warn!(
                    "Application {} has no reachable Web Target yet, so it answers 503: {}",
                    app.name,
                    ErrorReport::new(&e)
                ),
            }
        }

        if app.status == STATUS_RUNNING {
            routes.publish(&app);
        } else {
            // A stopped or failed Application must not keep a route: its
            // Hostname would answer with a gateway error.
            routes.withdraw(&app.id);
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
    pub compose: Option<String>,
    /// An empty string means "back to the default", so the console can clear
    /// the field.
    pub web_service: Option<String>,
    pub web_port: Option<u16>,
    /// Explicit development metadata. Omitted values preserve existing
    /// development Applications, while ordinary Compose stays ordinary.
    pub development: Option<DevelopmentApplication>,
}

/// Saves the change and redeploys.
///
/// A rename is only a row update, and so is a change of Hostname or aliases:
/// the container is keyed by id and the route is an entry in a table. Only a
/// new image recreates the container.
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
    let development = update
        .development
        .clone()
        .or_else(|| current.development.clone());
    if let Some(settings) = &development {
        validate_development(settings)?;
        if update.image.is_some()
            || update.compose.is_some()
            || update.web_service.is_some()
            || update.web_port.is_some()
        {
            return Err(DeployError::InvalidDevelopment(
                "development settings include the image and web target; do not send image, Compose, web service, or web port separately".into(),
            ));
        }
    }
    let (web_service, web_port) = match &development {
        Some(settings) => (Some("web".into()), Some(settings.web_port)),
        None => (
            match update.web_service.clone() {
                Some(s) if s.trim().is_empty() => None,
                Some(s) => Some(s),
                None => current.web_service.clone(),
            },
            update.web_port.or(current.web_port),
        ),
    };
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
        compose: development
            .as_ref()
            .map(development_compose)
            .or_else(|| update.compose.clone().or_else(|| current.compose.clone())),
        web_service,
        web_port,
        development,
        status: STATUS_PENDING.into(),
        last_error: None,
        ..current.clone()
    };

    // A Compose Application shows the image behind its Hostname; the file is
    // what the Operator edits.
    if current.source == SOURCE_COMPOSE {
        let spec = check_compose(
            record.compose.as_deref().unwrap_or_default(),
            record.web_service.as_deref(),
            record.web_port,
        )?;
        record.image = spec.image;
        record.web_service = spec.web_service;
        record.web_port = spec.web_port;
    }

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
    let file_changed = record.compose != current.compose;

    store.insert_application(&record).await?;

    // Only the image is Docker's business. A rename, a new Hostname, an
    // added alias and a new web target are all a route rewrite, which costs
    // no downtime.
    let needs_docker = if current.source == SOURCE_COMPOSE {
        file_changed || (record.development.is_some() && record.name != current.name)
    } else {
        image_changed
    };
    if !needs_docker && current.status == STATUS_RUNNING {
        record.status = STATUS_RUNNING.into();
        store.insert_application(&record).await?;
        return Ok(PendingDeploy {
            record,
            work: DeployWork::Settled,
        });
    }

    if current.source == SOURCE_COMPOSE {
        return Ok(PendingDeploy {
            record,
            work: DeployWork::ComposeUp,
        });
    }

    Ok(PendingDeploy {
        record,
        work: DeployWork::Recreate {
            pull: image_changed && current.source == SOURCE_IMAGE,
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
    let record = pending_record(
        store,
        name,
        image_tag,
        SOURCE_PATH,
        hostname_override,
        aliases,
        None,
    )
    .await?;
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

/// Platform Infra containers are named after the role they play for the
/// Platform, not after the product that fills it.
pub fn system_container_name(role: &str) -> String {
    format!("{SYSTEM_PREFIX}{role}")
}

#[derive(Debug)]
pub enum RemoveError {
    NotInitialized,
    NotFound(String),
    Docker(DockerError),
    Store(StoreError),
}

impl std::fmt::Display for RemoveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoveError::NotInitialized => {
                write!(f, "platform is not initialized; run 'self-host init' first")
            }
            RemoveError::NotFound(name) => write!(f, "Application '{name}' not found"),
            RemoveError::Docker(_) => write!(f, "failed to remove the Application container"),
            RemoveError::Store(_) => write!(f, "failed to delete the Application record"),
        }
    }
}

impl std::error::Error for RemoveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RemoveError::Docker(e) => Some(e),
            RemoveError::Store(e) => Some(e),
            _ => None,
        }
    }
}

impl From<DockerError> for RemoveError {
    fn from(e: DockerError) -> Self {
        RemoveError::Docker(e)
    }
}

impl From<StoreError> for RemoveError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound(name) => RemoveError::NotFound(name),
            other => RemoveError::Store(other),
        }
    }
}

pub async fn remove_application(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    routes: &(impl RouteStore + ?Sized),
    name: &str,
) -> Result<(), RemoveError> {
    if !store.is_initialized().await? {
        return Err(RemoveError::NotInitialized);
    }

    let app = store
        .find_application_by_name(name)
        .await?
        .ok_or_else(|| RemoveError::NotFound(name.to_string()))?;

    // The project is rendered before the row goes, because rendering reads
    // the row's environment.
    let project = if app.source == SOURCE_COMPOSE {
        project_for(store, &app).await.ok()
    } else {
        None
    };

    // The route goes first: a Hostname still answering for an Application that
    // is on its way out is worse than one that stops a moment early.
    routes.withdraw(&app.id);

    store.delete_application(&app.id).await?;

    match project {
        // Containers go; named volumes and the data directory stay. Removing
        // an Application is not permission to delete what it wrote.
        Some(project) => {
            let _ = docker.compose_down(&project).await;
        }
        None => {
            let _ = docker.remove_container(&container_name_for(&app.id)).await;
        }
    }

    Ok(())
}

// ── Lifecycle ──────────────────────────────────────────────────

/// Stops the Application's containers and takes its route down. The row says
/// `stopped`, so a Platform restart leaves it that way.
pub async fn stop_application(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    routes: &(impl RouteStore + ?Sized),
    id: &str,
) -> Result<ApplicationRecord, DeployError> {
    let mut app = get_application(store, id).await?;

    routes.withdraw(&app.id);
    if app.source == SOURCE_COMPOSE {
        docker
            .compose_stop(&project_for(store, &app).await?)
            .await?;
    } else {
        docker.stop_container(&container_name_for(&app.id)).await?;
    }

    app.status = STATUS_STOPPED.into();
    app.last_error = None;
    store.insert_application(&app).await?;
    Ok(app)
}

/// Starts what is already there. A failed Application has nothing to start
/// and is redeployed instead; this is for one the Operator stopped, or one
/// whose containers fell over.
pub async fn start_application(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    routes: &(impl RouteStore + ?Sized),
    id: &str,
) -> Result<ApplicationRecord, DeployError> {
    let app = get_application(store, id).await?;
    let result = async {
        if app.source == SOURCE_COMPOSE {
            docker
                .compose_start(&project_for(store, &app).await?)
                .await?;
        } else {
            docker.start_container(&container_name_for(&app.id)).await?;
        }
        routes.publish(&app);
        Ok(())
    }
    .await;
    record_outcome(store, app, result).await
}

pub async fn restart_application(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    routes: &(impl RouteStore + ?Sized),
    id: &str,
) -> Result<ApplicationRecord, DeployError> {
    let app = get_application(store, id).await?;
    let result = async {
        if app.source == SOURCE_COMPOSE {
            docker
                .compose_restart(&project_for(store, &app).await?)
                .await?;
        } else {
            docker
                .restart_container(&container_name_for(&app.id))
                .await?;
        }
        routes.publish(&app);
        Ok(())
    }
    .await;
    record_outcome(store, app, result).await
}

// ── State ──────────────────────────────────────────────────────

/// One container of an Application, as Docker sees it right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceState {
    pub service: String,
    pub container: String,
    /// Docker's word for it, or `missing` when there is no such container.
    pub state: String,
    pub exit_code: Option<i64>,
    pub restarts: Option<u32>,
}

/// The containers an Application is made of, as `(service, container)`. A
/// single-container Application has one service, called `app`.
pub fn containers_of(record: &ApplicationRecord) -> Vec<(String, String)> {
    if record.source != SOURCE_COMPOSE {
        return vec![("app".to_string(), container_name_for(&record.id))];
    }
    let project = project_name_for(&record.id);
    record
        .compose
        .as_deref()
        .and_then(|c| ComposeDefinition::parse(c).ok())
        .map(|definition| {
            definition
                .services
                .iter()
                .map(|s| {
                    (
                        s.name.clone(),
                        compose_app::container_name(&project, &s.name),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Asks Docker about every container of the Application. A runtime that
/// cannot answer reads as `missing` rather than taking the listing down.
pub async fn service_states(
    docker: &(impl DockerRuntime + ?Sized),
    record: &ApplicationRecord,
) -> Vec<ServiceState> {
    let mut states = Vec::new();
    for (service, container) in containers_of(record) {
        let state = docker.container_state(&container).await.unwrap_or(None);
        states.push(match state {
            Some(s) => ServiceState {
                service,
                container,
                state: s.status,
                exit_code: Some(s.exit_code),
                restarts: Some(s.restarts),
            },
            None => ServiceState {
                service,
                container,
                state: "missing".into(),
                exit_code: None,
                restarts: None,
            },
        });
    }
    states
}

/// The status the console shows, which is the row's word checked against
/// Docker: an Application on record as running whose container has exited
/// is failing, and the reason is the container's, not the deploy's.
pub fn live_status(
    record: &ApplicationRecord,
    services: &[ServiceState],
) -> (String, Option<ErrorReport>) {
    if record.status != STATUS_RUNNING || services.is_empty() {
        return (record.status.clone(), record.last_error.clone());
    }

    let reasons: Vec<String> = services
        .iter()
        .filter(|s| s.state != "running")
        .map(|s| match s.state.as_str() {
            "missing" => format!("service '{}' has no container", s.service),
            "exited" => format!(
                "service '{}' exited with code {} after {} restarts",
                s.service,
                s.exit_code.unwrap_or_default(),
                s.restarts.unwrap_or_default()
            ),
            other => format!("service '{}' is {other}", s.service),
        })
        .collect();

    if reasons.is_empty() {
        return (STATUS_RUNNING.into(), None);
    }

    (
        STATUS_FAILED.into(),
        Some(ErrorReport {
            error: "the Application is not running".into(),
            caused_by: reasons,
        }),
    )
}

// ── Env ────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum EnvError {
    NotInitialized,
    NotFound(String),
    Docker(DockerError),
    Store(StoreError),
}

impl std::fmt::Display for EnvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnvError::NotInitialized => {
                write!(f, "platform is not initialized; run 'self-host init' first")
            }
            EnvError::NotFound(name) => write!(f, "Application '{name}' not found"),
            EnvError::Docker(_) => write!(f, "failed to restart the Application container"),
            EnvError::Store(_) => write!(f, "failed to read or write the Application environment"),
        }
    }
}

impl std::error::Error for EnvError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EnvError::Docker(e) => Some(e),
            EnvError::Store(e) => Some(e),
            _ => None,
        }
    }
}

impl From<DockerError> for EnvError {
    fn from(e: DockerError) -> Self {
        EnvError::Docker(e)
    }
}

impl From<StoreError> for EnvError {
    fn from(e: StoreError) -> Self {
        EnvError::Store(e)
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

    // Env is keyed by id (ADR-0008); the CLI still speaks in names.
    let app = store
        .find_application_by_name(app_name)
        .await?
        .ok_or_else(|| EnvError::NotFound(app_name.to_string()))?;

    store.set_env(&app.id, key, value).await?;

    // Apply: recreate container with updated env
    recreate_with_env(store, docker, app_name).await?;

    Ok(())
}

pub async fn get_all_env(
    store: &impl StateStore,
    app_name: &str,
) -> Result<Vec<(String, String)>, EnvError> {
    let app = store
        .find_application_by_name(app_name)
        .await?
        .ok_or_else(|| EnvError::NotFound(app_name.to_string()))?;
    Ok(store.get_all_env(&app.id).await?)
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

    let app = store
        .find_application_by_name(app_name)
        .await?
        .ok_or_else(|| EnvError::NotFound(app_name.to_string()))?;

    store.unset_env(&app.id, key).await?;

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

    if app.source == SOURCE_COMPOSE {
        // The environment is rendered into the project; `up` recreates only
        // the services whose definition changed.
        let project = project_for(store, app)
            .await
            .map_err(|e| EnvError::Docker(DockerError::Unavailable(e.to_string())))?;
        docker.compose_up(&project).await?;
        return Ok(());
    }

    let container_name = container_name_for(&app.id);

    docker.remove_container(&container_name).await?;
    docker
        .run_application(ApplicationContainer {
            name: container_name,
            image: app.image.clone(),
            labels: identity_labels(&app.id, &app.name),
            network: APP_NETWORK.to_string(),
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
    Store(StoreError),
}

impl std::fmt::Display for LogsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogsError::NotInitialized => {
                write!(f, "platform is not initialized; run 'self-host init' first")
            }
            LogsError::NotFound(name) => write!(f, "Application '{name}' not found"),
            LogsError::Docker(_) => write!(f, "failed to stream the Application logs"),
            LogsError::Store(_) => write!(f, "failed to look up the Application"),
        }
    }
}

impl std::error::Error for LogsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LogsError::Docker(e) => Some(e),
            LogsError::Store(e) => Some(e),
            _ => None,
        }
    }
}

impl From<DockerError> for LogsError {
    fn from(e: DockerError) -> Self {
        LogsError::Docker(e)
    }
}

impl From<StoreError> for LogsError {
    fn from(e: StoreError) -> Self {
        LogsError::Store(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker::FakeDocker;
    use crate::routes::FakeRoutes;
    use crate::store::FakeStateStore;
    use std::net::SocketAddr;

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
        assert!(
            routes
                .get(&deployed.id)
                .unwrap()
                .answers_on("blog.home.lan")
        );
    }

    #[tokio::test]
    async fn a_deployed_application_only_joins_the_application_network() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();

        deploy_from_image(&store, &docker, &routes, "blog", "nginx:alpine", None, None)
            .await
            .unwrap();

        // Off the Platform Infra bridge, an Application cannot open a socket
        // on the state store, whose password is the same on every install.
        let containers = docker.apps.lock().unwrap();
        assert_eq!(containers[0].network, APP_NETWORK);
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

    /// An Application deployed before the Platform served HTTP itself has no
    /// Host port, so nothing outside Docker can reach it. Startup is where it
    /// gets one, at the cost of recreating the workload once.
    #[tokio::test]
    async fn reconcile_gives_an_older_application_a_reachable_web_target() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        deploy_from_image(&store, &docker, &routes, "blog", "nginx:alpine", None, None)
            .await
            .unwrap();

        // As the store would have it after an upgrade: running, on record,
        // and answering nowhere the Host can reach.
        let mut app = store
            .find_application_by_name("blog")
            .await
            .unwrap()
            .unwrap();
        app.web_target_port = None;
        store.insert_application(&app).await.unwrap();
        docker.apps.lock().unwrap().clear();

        reconcile(&store, &docker, &routes).await.unwrap();

        let migrated = store
            .find_application_by_name("blog")
            .await
            .unwrap()
            .unwrap();
        let port = migrated.web_target_port.expect("a Host port");
        assert_eq!(migrated.status, STATUS_RUNNING);
        let containers = docker.apps.lock().unwrap();
        assert_eq!(containers.len(), 1, "the workload was recreated once");
        assert_eq!(containers[0].ports, [format!("127.0.0.1:{port}:80")]);
    }

    /// The port is recorded only once something answers on it. A record
    /// pointing at a closed socket routes Consumers into a dead end, where a
    /// record with no port at all is an honest 503.
    #[tokio::test]
    async fn a_failed_recreate_leaves_the_application_without_a_port() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        deploy_from_image(&store, &docker, &routes, "blog", "nginx:alpine", None, None)
            .await
            .unwrap();
        let mut app = store
            .find_application_by_name("blog")
            .await
            .unwrap()
            .unwrap();
        app.web_target_port = None;
        store.insert_application(&app).await.unwrap();

        let broken = FakeDocker::failing_run("no space left on device");
        reconcile(&store, &broken, &routes).await.unwrap();

        let untouched = store
            .find_application_by_name("blog")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(untouched.web_target_port, None);
        assert_eq!(untouched.status, STATUS_RUNNING);
    }

    /// A Host whose Docker is gone observes nothing. Reporting every
    /// Application as failed, or worse as stopped, would turn an outage into a
    /// change the Operator never asked for.
    #[tokio::test]
    async fn reconcile_leaves_saved_state_alone_when_the_executor_is_unreachable() {
        let store = initialized_store().await;
        let routes = FakeRoutes::new();
        let docker = FakeDocker::new();
        deploy_from_image(&store, &docker, &routes, "blog", "nginx:alpine", None, None)
            .await
            .unwrap();
        deploy_from_image(
            &store,
            &docker,
            &routes,
            "journal",
            "nginx:alpine",
            None,
            None,
        )
        .await
        .unwrap();

        // One deploy interrupted by a restart, one Application the Operator
        // stopped on purpose.
        let mut pending = store
            .find_application_by_name("blog")
            .await
            .unwrap()
            .unwrap();
        pending.status = STATUS_PENDING.into();
        store.insert_application(&pending).await.unwrap();
        let mut stopped = store
            .find_application_by_name("journal")
            .await
            .unwrap()
            .unwrap();
        stopped.status = STATUS_STOPPED.into();
        store.insert_application(&stopped).await.unwrap();

        let gone = FakeDocker {
            unreachable: Some("Cannot connect to the Docker daemon".into()),
            ..FakeDocker::new()
        };
        reconcile(&store, &gone, &routes).await.unwrap();

        assert_eq!(
            store.get_application(&pending.id).await.unwrap().unwrap(),
            pending,
            "an unreachable executor settled a deploy it never observed"
        );
        assert_eq!(
            store.get_application(&stopped.id).await.unwrap().unwrap(),
            stopped
        );
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
        let report = settled.last_error.unwrap();
        assert!(report.error.contains("restart"));
        assert!(report.caused_by.is_empty());
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
            "routing belongs to the route table, not to a label a proxy reads"
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
        assert!(routes.get(&app.id).unwrap().answers_on("writing.home.lan"));
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

        // The old Hostname answers alongside the new one, so a rename
        // breaks no bookmark on the LAN.
        assert_eq!(
            routes.get(&app.id).unwrap().hostnames,
            ["writing.home.lan", "blog.home.lan"]
        );
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

    const HERMES: &str = r#"
services:
  hermes:
    image: nousresearch/hermes-agent:latest
    command: gateway run
    ports:
      - "8642:8642"
      - "9119:9119"
    volumes:
      - ~/.hermes:/opt/data
    environment:
      - HERMES_DASHBOARD=1
"#;

    async fn deploy_hermes(
        store: &FakeStateStore,
        docker: &FakeDocker,
        routes: &FakeRoutes,
    ) -> ApplicationRecord {
        let pending =
            prepare_deploy_from_compose(store, "hermes", HERMES, None, Some(9119), None, None)
                .await
                .unwrap();
        finish_deploy(store, docker, routes, pending).await.unwrap()
    }

    #[tokio::test]
    async fn a_compose_deploy_brings_the_project_up_named_after_the_application() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();

        let app = deploy_hermes(&store, &docker, &routes).await;

        assert_eq!(app.status, STATUS_RUNNING);
        assert_eq!(app.source, SOURCE_COMPOSE);
        assert_eq!(app.image, "nousresearch/hermes-agent:latest");
        // Left unsaid, the web service is the one that publishes a port; the
        // port was said, because 8642 is the gateway and 9119 the chat.
        assert_eq!(app.web_service.as_deref(), Some("hermes"));
        assert_eq!(app.web_port, Some(9119));

        let project = docker
            .project(&format!("sf-app-{}", app.id))
            .expect("the project was brought up");
        assert_eq!(
            project.containers,
            vec![("hermes".to_string(), format!("sf-app-{}-hermes", app.id))]
        );
        assert!(project.dir.ends_with(format!("apps/{}", app.id)));

        // The route points at the Host port the web service publishes, not
        // at a container the proxy has no network to reach.
        assert_eq!(
            routes.get(&app.id).unwrap().target,
            Some(SocketAddr::from((
                [127, 0, 0, 1],
                app.web_target_port.unwrap()
            )))
        );
    }

    #[tokio::test]
    async fn a_development_application_rename_recreates_its_hostname() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let settings = DevelopmentApplication {
            image_id: "dev-image".into(),
            tag: "sf-img-dev-image:old".into(),
            command: "t3 serve --host 0.0.0.0 --port 3000".into(),
            web_port: 3000,
            persist_data: true,
        };

        let app = finish_deploy(
            &store,
            &docker,
            &routes,
            prepare_deploy_from_development(&store, "t3", settings.clone(), None, None)
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        let before = docker.project(&project_name_for(&app.id)).unwrap();
        let before_yaml: serde_yaml::Value = serde_yaml::from_str(&before.yaml).unwrap();
        assert_eq!(before_yaml["services"]["web"]["hostname"], "t3");

        let pending = prepare_update(
            &store,
            &app.id,
            ApplicationUpdate {
                name: Some("renamed".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(
            !pending.is_settled(),
            "renaming must recreate the development container"
        );
        let renamed = finish_deploy(&store, &docker, &routes, pending)
            .await
            .unwrap();
        assert_eq!(renamed.id, app.id);
        assert_eq!(renamed.development, Some(settings));
        let after = docker.project(&project_name_for(&app.id)).unwrap();
        let after_yaml: serde_yaml::Value = serde_yaml::from_str(&after.yaml).unwrap();
        assert_eq!(after_yaml["services"]["web"]["hostname"], "renamed");
    }

    #[tokio::test]
    async fn changing_a_development_port_updates_its_web_target() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let settings = DevelopmentApplication {
            image_id: "dev-image".into(),
            tag: "sf-img-dev-image:old".into(),
            command: "t3 serve --host 0.0.0.0 --port 3000".into(),
            web_port: 3000,
            persist_data: true,
        };
        let app = finish_deploy(
            &store,
            &docker,
            &routes,
            prepare_deploy_from_development(&store, "t3", settings, None, None)
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        let updated_settings = DevelopmentApplication {
            web_port: 4000,
            ..app.development.clone().unwrap()
        };

        let updated = update_application(
            &store,
            &docker,
            &routes,
            &app.id,
            ApplicationUpdate {
                development: Some(updated_settings.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(updated.web_service.as_deref(), Some("web"));
        assert_eq!(updated.web_port, Some(4000));
        assert_eq!(updated.development, Some(updated_settings));
        let project = docker.project(&project_name_for(&app.id)).unwrap();
        let yaml: serde_yaml::Value = serde_yaml::from_str(&project.yaml).unwrap();
        let expected = crate::ports::publication(app.web_target_port.unwrap(), 4000);
        assert!(
            yaml["services"]["web"]["ports"]
                .as_sequence()
                .unwrap()
                .iter()
                .any(|port| port.as_str() == Some(expected.as_str()))
        );
    }

    #[tokio::test]
    async fn an_empty_web_service_means_the_default_one() {
        let store = initialized_store().await;

        let pending =
            prepare_deploy_from_compose(&store, "hermes", HERMES, Some(""), Some(9119), None, None)
                .await
                .unwrap();

        assert_eq!(pending.record.web_service.as_deref(), Some("hermes"));
    }

    #[tokio::test]
    async fn a_compose_file_the_platform_will_not_run_is_refused_before_anything_is_recorded() {
        let store = initialized_store().await;

        let err = prepare_deploy_from_compose(
            &store,
            "hermes",
            "services:\n  hermes:\n    image: x\n    privileged: true\n",
            None,
            Some(80),
            None,
            None,
        )
        .await
        .unwrap_err();

        assert!(matches!(err, DeployError::InvalidCompose(_)));
        let report = ErrorReport::new(&err);
        assert_eq!(report.error, "invalid Compose definition");
        assert_eq!(
            report.caused_by,
            ["service 'hermes': 'privileged' is not supported"]
        );
        assert!(store.list_applications().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_compose_up_that_fails_leaves_the_application_on_record_with_the_reason() {
        let store = initialized_store().await;
        let docker = FakeDocker::failing_compose("Error response from daemon: manifest unknown");
        let routes = FakeRoutes::new();

        let pending =
            prepare_deploy_from_compose(&store, "hermes", HERMES, None, Some(9119), None, None)
                .await
                .unwrap();
        let err = finish_deploy(&store, &docker, &routes, pending)
            .await
            .unwrap_err();
        assert!(matches!(err, DeployError::Docker(_)));

        let saved = store
            .find_application_by_name("hermes")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.status, STATUS_FAILED);
        assert_eq!(saved.compose.as_deref(), Some(HERMES));
        let report = saved.last_error.unwrap();
        assert!(
            report
                .caused_by
                .iter()
                .any(|c| c.contains("manifest unknown"))
        );
        assert!(
            routes.get(&saved.id).is_none(),
            "no route for a project that is not up"
        );
    }

    #[tokio::test]
    async fn stopping_takes_the_route_down_and_starting_puts_it_back() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let app = deploy_hermes(&store, &docker, &routes).await;
        let container = format!("sf-app-{}-hermes", app.id);

        let stopped = stop_application(&store, &docker, &routes, &app.id)
            .await
            .unwrap();
        assert_eq!(stopped.status, STATUS_STOPPED);
        assert!(routes.get(&app.id).is_none());
        assert_eq!(
            docker
                .container_state(&container)
                .await
                .unwrap()
                .unwrap()
                .status,
            "exited"
        );

        let started = start_application(&store, &docker, &routes, &app.id)
            .await
            .unwrap();
        assert_eq!(started.status, STATUS_RUNNING);
        assert!(routes.get(&app.id).is_some());
        assert_eq!(
            docker
                .container_state(&container)
                .await
                .unwrap()
                .unwrap()
                .status,
            "running"
        );

        // A stopped Application stays stopped across a Platform restart: its
        // route is withdrawn again, not republished.
        stop_application(&store, &docker, &routes, &app.id)
            .await
            .unwrap();
        reconcile(&store, &docker, &routes).await.unwrap();
        assert!(routes.get(&app.id).is_none());
        assert_eq!(
            store
                .get_application(&app.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            STATUS_STOPPED
        );
    }

    #[tokio::test]
    async fn a_service_that_fell_over_shows_as_failed_with_its_exit_code() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let app = deploy_hermes(&store, &docker, &routes).await;

        // The row says running: the deploy itself went fine.
        docker.exit_container(&format!("sf-app-{}-hermes", app.id), 1);

        let services = service_states(&docker, &app).await;
        assert_eq!(services.len(), 1);
        assert_eq!(services[0].service, "hermes");
        assert_eq!(services[0].state, "exited");
        assert_eq!(services[0].exit_code, Some(1));

        let (status, reason) = live_status(&app, &services);
        assert_eq!(status, STATUS_FAILED);
        let reason = reason.unwrap();
        assert_eq!(reason.error, "the Application is not running");
        assert_eq!(
            reason.caused_by,
            ["service 'hermes' exited with code 1 after 3 restarts"]
        );
        // The row itself is untouched: this is Docker's word, not the deploy's.
        assert_eq!(
            store
                .get_application(&app.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            STATUS_RUNNING
        );
    }

    #[tokio::test]
    async fn changing_the_compose_file_brings_the_project_up_again() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let app = deploy_hermes(&store, &docker, &routes).await;

        let edited = HERMES.replace(
            "HERMES_DASHBOARD=1",
            "HERMES_DASHBOARD=1\n      - HERMES_DASHBOARD_BASIC_AUTH_USERNAME=seba",
        );
        let updated = update_application(
            &store,
            &docker,
            &routes,
            &app.id,
            ApplicationUpdate {
                compose: Some(edited.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(updated.status, STATUS_RUNNING);
        assert_eq!(updated.compose.as_deref(), Some(edited.as_str()));
        let project = docker.project(&format!("sf-app-{}", app.id)).unwrap();
        assert!(
            project
                .yaml
                .contains("HERMES_DASHBOARD_BASIC_AUTH_USERNAME: seba")
        );
    }

    #[tokio::test]
    async fn changing_only_the_web_port_rewrites_the_route_without_docker() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let app = deploy_hermes(&store, &docker, &routes).await;
        let before = docker.project(&format!("sf-app-{}", app.id)).unwrap();

        let pending = prepare_update(
            &store,
            &app.id,
            ApplicationUpdate {
                web_port: Some(8642),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(pending.is_settled());
        finish_deploy(&store, &docker, &routes, pending)
            .await
            .unwrap();

        assert_eq!(
            routes.get(&app.id).unwrap().target,
            Some(SocketAddr::from((
                [127, 0, 0, 1],
                app.web_target_port.unwrap()
            )))
        );
        assert_eq!(
            docker.project(&format!("sf-app-{}", app.id)).unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn platform_environment_is_rendered_into_the_project() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let app = deploy_hermes(&store, &docker, &routes).await;

        set_env(&store, &docker, "hermes", "OPENROUTER_API_KEY", "sk-test")
            .await
            .unwrap();

        let project = docker.project(&format!("sf-app-{}", app.id)).unwrap();
        assert!(project.yaml.contains("OPENROUTER_API_KEY: sk-test"));
    }

    #[tokio::test]
    async fn removing_a_compose_application_takes_the_project_down() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let app = deploy_hermes(&store, &docker, &routes).await;

        remove_application(&store, &docker, &routes, "hermes")
            .await
            .unwrap();

        assert!(docker.project(&format!("sf-app-{}", app.id)).is_none());
        assert!(routes.get(&app.id).is_none());
        assert!(store.get_application(&app.id).await.unwrap().is_none());
    }

    #[test]
    fn validate_hostname_rejects_anything_that_is_not_a_hostname() {
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
