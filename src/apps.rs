use crate::compose_app::{
    self, ComposeDefinition, ComposeDefinitionError, ComposeProject, Resolution,
};
use crate::docker::{ApplicationContainer, DockerError, DockerRuntime};
use crate::error::ErrorReport;
use crate::ports;
use crate::routes::RouteStore;
use crate::store::{
    DevelopmentApplication, Publication, Runtime, StateStore, StoreError, VariableDelivery,
};
use serde::{Deserialize, Serialize};

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
pub const SOURCE_GIT: &str = "git";
pub const SOURCE_NATIVE: &str = "native";

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
    Readiness(String),
    InvalidDevelopment(String),
    InvalidCompose(ComposeDefinitionError),
    Docker(DockerError),
    /// No Host port left for the Application's Web Target to answer on.
    NoWebTargetPort(crate::ports::NoPortAvailable),
    /// The request asked for a native Runtime, which the Platform records
    /// but cannot run yet (ADR-0028).
    NativeUnavailable,
    InvalidNative(String),
    Native(anyhow::Error),
    /// The request asked to change a Publication chosen at creation.
    PublicationFixed,
    Route(crate::routes::RouteError),
    Connectivity(crate::connectivity::ConnectivityError),
    Store(StoreError),
    Source(crate::source::SourceError),
}

/// Why an unpublished Application refuses a Hostname, an alias or a Web
/// Target in a request.
const UNPUBLISHED_HAS_NO_HOSTNAME: &str = "an unpublished Application has no Hostname";

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
            DeployError::Readiness(message) => write!(f, "{message}"),
            DeployError::MissingCompose => write!(f, "a Compose definition is required"),
            DeployError::InvalidDevelopment(message) => {
                write!(f, "invalid custom image: {message}")
            }
            DeployError::InvalidCompose(_) => write!(f, "invalid Compose definition"),
            DeployError::Docker(_) => write!(f, "failed to deploy the Application"),
            DeployError::NoWebTargetPort(_) => {
                write!(f, "failed to publish the Application on the LAN")
            }
            DeployError::InvalidNative(message) => {
                write!(f, "invalid native Application: {message}")
            }
            DeployError::Native(_) => write!(f, "native Application operation failed"),
            DeployError::NativeUnavailable => write!(
                f,
                "native execution is not available yet; the Application runtime must be container"
            ),
            DeployError::PublicationFixed => {
                write!(f, "publication cannot be changed after creation yet")
            }
            DeployError::Route(error) => write!(f, "{error}"),
            DeployError::Connectivity(error) => write!(f, "{error}"),
            DeployError::Store(_) => write!(f, "failed to record the Application"),
            DeployError::Source(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for DeployError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DeployError::InvalidCompose(e) => Some(e),
            DeployError::Native(e) => Some(e.as_ref()),
            DeployError::Docker(e) => Some(e),
            DeployError::NoWebTargetPort(e) => Some(e),
            DeployError::Store(e) => Some(e),
            DeployError::Source(e) => Some(e),
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

impl From<crate::source::SourceError> for DeployError {
    fn from(error: crate::source::SourceError) -> Self {
        Self::Source(error)
    }
}

impl From<ComposeDefinitionError> for DeployError {
    fn from(e: ComposeDefinitionError) -> Self {
        DeployError::InvalidCompose(e)
    }
}

pub fn validate_app_name(name: &str) -> Result<(), DeployError> {
    if name.trim().is_empty() {
        return Err(DeployError::InvalidName("must not be blank".into()));
    }
    if name.chars().count() > 63 {
        return Err(DeployError::InvalidName(
            "must be at most 63 characters".into(),
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(DeployError::InvalidName(
            "must not contain control characters".into(),
        ));
    }
    Ok(())
}

pub fn default_hostname(label: &str, dns_suffix: &str) -> String {
    format!("{label}.{dns_suffix}")
}

/// Display names never become machine identifiers verbatim. DNS labels use
/// only ASCII; a name with no ASCII letters or digits uses its stable id.
fn technical_label(name: &str, id: &str) -> String {
    let mut label = String::new();
    let mut separator = false;
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !label.is_empty() {
                label.push('-');
            }
            label.push(character.to_ascii_lowercase());
            separator = false;
        } else {
            separator = true;
        }
    }
    if label.is_empty() {
        id.into()
    } else {
        label.truncate(63);
        label.trim_end_matches('-').into()
    }
}

/// Called only for a new, published Application with no explicit Hostname.
/// The API holds the shared DNS namespace lock through its record write.
async fn generated_hostname(
    store: &impl StateStore,
    name: &str,
    id: &str,
    suffix: &str,
) -> Result<String, DeployError> {
    let mut occupied = std::collections::HashSet::from([format!("admin.{suffix}")]);
    for app in store.list_applications().await? {
        occupied.extend(
            crate::routes::hostnames(&app)
                .into_iter()
                .map(str::to_owned),
        );
    }
    occupied.extend(
        crate::dns_records::load(store)
            .await?
            .into_iter()
            .map(|record| format!("{}.{suffix}", record.name)),
    );
    occupied.extend(
        crate::environments::load(store)
            .await?
            .into_iter()
            .map(|machine| machine.hostname),
    );
    let label = technical_label(name, id);
    for attempt in 0..=occupied.len() {
        let candidate = if attempt == 0 {
            label.clone()
        } else {
            let tail = if attempt == 1 {
                id.into()
            } else {
                format!("{id}-{attempt}")
            };
            let prefix = label.chars().take(63 - tail.len() - 1).collect::<String>();
            format!("{}-{tail}", prefix.trim_end_matches('-'))
        };
        let hostname = default_hostname(&candidate, suffix);
        if !occupied.contains(&hostname) {
            return Ok(hostname);
        }
    }
    unreachable!("there are more candidates than occupied Hostnames")
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
/// An unpublished Application has nothing to check and takes no name from
/// the Zone (ADR-0028).
pub(crate) async fn validate_routing(
    store: &impl StateStore,
    record: &ApplicationRecord,
) -> Result<(), DeployError> {
    let suffix = store
        .get_state("dns_suffix")
        .await?
        .ok_or(DeployError::NotInitialized)?;
    let records = crate::dns_records::load(store).await?;
    let machines = crate::environments::load(store).await?;
    for hostname in crate::routes::hostnames(record) {
        validate_hostname(hostname)?;
        if hostname == suffix {
            return Err(DeployError::InvalidHostname(format!(
                "'{suffix}' is the apex of the Zone"
            )));
        }
        if let Some(taken) = records
            .iter()
            .find(|existing| hostname == format!("{}.{}", existing.name, suffix))
        {
            return Err(DeployError::InvalidHostname(format!(
                "'{hostname}' is already answered by Record '{}'",
                taken.key()
            )));
        }
        if let Some(machine) = machines.iter().find(|machine| machine.hostname == hostname) {
            return Err(DeployError::InvalidHostname(format!(
                "'{hostname}' is already answered by Virtual machine '{}'",
                machine.config.name
            )));
        }
    }

    crate::routes::validate_rules(
        record,
        &store.list_applications().await?,
        &format!("admin.{suffix}"),
    )
    .map_err(DeployError::Route)?;

    Ok(())
}

async fn validate_connectivity(
    store: &impl StateStore,
    record: &ApplicationRecord,
) -> Result<(), DeployError> {
    crate::connectivity::validate(record, &store.list_applications().await?)
        .map_err(DeployError::Connectivity)?;
    if let Some(compose) = &record.compose {
        let env = store.get_all_env(&record.id).await?;
        let definition = ComposeDefinition::parse_with(compose, &env, Resolution::Check)?;
        crate::connectivity::validate_definition(&record.network_policy, &definition)
            .map_err(DeployError::Connectivity)?;
    }
    Ok(())
}

async fn ensure_application_networks(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    record: &ApplicationRecord,
) -> Result<crate::connectivity::NetworkPlan, DeployError> {
    let plan = crate::connectivity::plan(record, &store.list_applications().await?);
    for name in &plan.internal_networks {
        docker.ensure_private_network(name).await?;
    }
    if !plan.internal_networks.contains(&plan.primary_network)
        && (record.compose.is_none() || plan.shared)
    {
        docker.ensure_network(&plan.primary_network).await?;
    }
    Ok(plan)
}

async fn reconcile_private_connections(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
) -> Result<(), DeployError> {
    for consumer in store.list_applications().await? {
        if consumer.runtime != Runtime::Container {
            continue;
        }
        let plan = ensure_application_networks(store, docker, &consumer).await?;
        for (_, container) in containers_of(&consumer) {
            if docker.container_state(&container).await?.is_some() {
                docker
                    .sync_private_networks(&container, &plan.internal_networks)
                    .await?;
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
    record: ApplicationRecord,
    result: Result<(), DeployError>,
) -> Result<ApplicationRecord, DeployError> {
    match result {
        Ok(()) => Ok(store
            .set_application_outcome(&record.id, STATUS_RUNNING, None)
            .await?),
        Err(e) => {
            // The deploy error is what the caller needs; a failure to write the
            // reason down must not replace it.
            let report =
                crate::postgres::runtime_report(store, &record.id, ErrorReport::new(&e)).await;
            let _ = store
                .set_application_outcome(&record.id, STATUS_FAILED, Some(report))
                .await;
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

    let plan = ensure_application_networks(store, docker, record).await?;
    let mut labels = identity_labels(&record.id, &record.name);
    if record.git.is_some() {
        labels.push(("sf.source".into(), "git".into()));
    }
    docker
        .run_application(ApplicationContainer {
            name: container_name_for(&record.id),
            image: record.image.clone(),
            labels,
            network: plan.primary_network,
            additional_networks: plan.additional_networks,
            ports: web_target_publication(record),
            env,
        })
        .await?;
    Ok(())
}

/// What a deploy request says beyond the Application's definition: where it
/// answers and how it runs (ADR-0028). A field left unsaid means what every
/// Application meant before the field existed, except `variable_delivery`,
/// which a new Application reads as `Referenced`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeployOptions {
    /// The Hostname, instead of `<name>.<suffix>`.
    pub hostname: Option<String>,
    /// Extra Hostnames the Application also answers on.
    pub aliases: Option<Vec<String>>,
    pub runtime: Option<Runtime>,
    pub publication: Option<Publication>,
    pub variable_delivery: Option<VariableDelivery>,
    pub route_rules: Option<Vec<crate::store::RouteRule>>,
    pub network_policy: Option<crate::store::NetworkPolicy>,
}

/// Publication is chosen at creation (ADR-0028): a deploy under a name
/// already on record keeps the row's, and one asking for the other kind is
/// refused. A new Application is published unless it says otherwise.
fn settle_publication(
    requested: Option<Publication>,
    current: Option<Publication>,
) -> Result<Publication, DeployError> {
    match (requested, current) {
        (Some(requested), Some(current)) if requested != current => {
            Err(DeployError::PublicationFixed)
        }
        (Some(requested), _) => Ok(requested),
        (None, Some(current)) => Ok(current),
        (None, None) => Ok(Publication::default()),
    }
}

/// The Publication a deploy under `name` ends up with, settled before the
/// definition is checked so an unpublished file is not asked for a Web
/// Target it does not have.
async fn publication_for(
    store: &impl StateStore,
    name: &str,
    options: &DeployOptions,
) -> Result<Publication, DeployError> {
    let current = store
        .find_application_by_name(name)
        .await?
        .map(|app| app.publication);
    settle_publication(options.publication, current)
}

/// An unpublished Application has nothing to route, so a request that names
/// a Hostname, an alias or a Web Target for one is refused rather than
/// recorded and ignored. An empty value is how a form says nothing.
pub(crate) fn refuse_routing_for_unpublished(
    hostname: Option<&str>,
    aliases: Option<&[String]>,
    web_service: Option<&str>,
    web_port: Option<u16>,
) -> Result<(), DeployError> {
    let names_routing = hostname.is_some_and(|h| !h.trim().is_empty())
        || aliases.is_some_and(|a| !a.is_empty())
        || web_service.is_some_and(|s| !s.trim().is_empty())
        || web_port.is_some();
    if names_routing {
        return Err(DeployError::InvalidHostname(
            UNPUBLISHED_HAS_NO_HOSTNAME.into(),
        ));
    }
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
    definition: Option<ComposeSpec>,
    options: &DeployOptions,
) -> Result<ApplicationRecord, DeployError> {
    let dns_suffix = store
        .get_state("dns_suffix")
        .await?
        .ok_or(DeployError::NotInitialized)?;

    let existing = store.find_application_by_name(name).await?;
    if let Some(app) = &existing
        && crate::postgres::metadata(store, &app.id).await?.is_some()
    {
        return Err(DeployError::Connectivity(
            crate::connectivity::ConnectivityError(
                "Use the managed PostgreSQL controls to preserve its version, credentials and data"
                    .into(),
            ),
        ));
    }
    let id = existing
        .as_ref()
        .map(|a| a.id.clone())
        .unwrap_or_else(generate_app_id);
    let mut runtime = options
        .runtime
        .clone()
        .or_else(|| existing.as_ref().map(|a| a.runtime.clone()))
        .unwrap_or_default();
    if existing
        .as_ref()
        .is_some_and(|a| std::mem::discriminant(&a.runtime) != std::mem::discriminant(&runtime))
    {
        return Err(DeployError::InvalidNative(
            "Application Runtime cannot be changed".into(),
        ));
    }
    if source != SOURCE_NATIVE && matches!(runtime, Runtime::Native(_)) {
        return Err(DeployError::NativeUnavailable);
    }
    let publication = settle_publication(
        options.publication,
        existing.as_ref().map(|app| app.publication),
    )?;
    let variable_delivery = options
        .variable_delivery
        .or_else(|| existing.as_ref().map(|app| app.variable_delivery))
        .unwrap_or(VariableDelivery::Referenced);

    if let Runtime::Native(definition) = &mut runtime {
        crate::native::lifecycle::validate_definition(&id, definition, publication)
            .map_err(|e| DeployError::InvalidNative(e.to_string()))?;
        crate::native::lifecycle::validate_port(store, &id, definition.port).await?;
    }
    let (hostname, aliases, web_target_port) = match publication {
        // No Hostname, no Record, no route, no Host port.
        Publication::Unpublished => {
            refuse_routing_for_unpublished(
                options.hostname.as_deref(),
                options.aliases.as_deref(),
                definition.as_ref().and_then(|d| d.web_service.as_deref()),
                definition.as_ref().and_then(|d| d.web_port),
            )?;
            (String::new(), Vec::new(), None)
        }
        Publication::Web => {
            let hostname = match (&options.hostname, &existing) {
                (Some(h), _) => h.clone(),
                (None, Some(app)) => app.hostname.clone(),
                (None, None) => generated_hostname(store, name, &id, &dns_suffix).await?,
            };
            let aliases = match (&options.aliases, &existing) {
                (Some(a), _) => a.clone(),
                (None, Some(app)) => app.aliases.clone(),
                (None, None) => vec![],
            };
            let port = match &runtime {
                Runtime::Native(definition) => {
                    definition.port.expect("validated native Web Target port")
                }
                Runtime::Container => match existing.as_ref().and_then(|app| app.web_target_port) {
                    Some(port) => port,
                    None => allocate_web_target_port(store).await?,
                },
            };
            (hostname, aliases, Some(port))
        }
    };

    let record = ApplicationRecord {
        id,
        name: name.to_string(),
        hostname,
        aliases,
        image,
        status: STATUS_PENDING.into(),
        source: source.to_string(),
        git: None,
        git_build: None,
        last_error: None,
        compose: definition.as_ref().map(|d| d.compose.clone()),
        web_service: definition.as_ref().and_then(|d| d.web_service.clone()),
        web_port: definition.as_ref().and_then(|d| d.web_port),
        web_target_port,
        development: None,
        runtime,
        publication,
        variable_delivery,
        route_rules: options
            .route_rules
            .clone()
            .or_else(|| existing.as_ref().map(|app| app.route_rules.clone()))
            .unwrap_or_default(),
        rewrite_host: existing.as_ref().and_then(|app| app.rewrite_host),
        network_policy: options
            .network_policy
            .clone()
            .or_else(|| existing.as_ref().map(|app| app.network_policy.clone()))
            .unwrap_or_else(crate::store::NetworkPolicy::private),
    };

    if let Some(previous) = &existing
        && previous.compose.is_none()
        && previous.network_policy != record.network_policy
    {
        return Err(DeployError::Connectivity(
            crate::connectivity::ConnectivityError(
                "network policy changes require a Compose Application".into(),
            ),
        ));
    }
    validate_routing(store, &record).await?;
    validate_connectivity(store, &record).await?;

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
    let mut taken: std::collections::BTreeSet<u16> = store
        .list_applications()
        .await?
        .iter()
        .filter_map(|app| app.web_target_port)
        .collect();
    taken.extend(crate::postgres::reserved_ports(store).await?);
    ports::allocate(&taken).map_err(DeployError::NoWebTargetPort)
}

/// Where the Web Target answers on the Host, as Docker publishes it. Empty
/// until the Application has a port, which is every Application deployed
/// before the proxy moved into the binary, and always empty for an
/// unpublished Application, which has no Web Target.
fn web_target_publication(record: &ApplicationRecord) -> Vec<String> {
    if record.publication == Publication::Unpublished {
        return Vec::new();
    }
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
/// report the outcome — or hand it to a task and answer straight away. It
/// serializes so a queued deploy survives a restart of the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingDeploy {
    pub record: ApplicationRecord,
    work: DeployWork,
}

impl PendingDeploy {
    pub(crate) fn managed_compose(record: ApplicationRecord) -> Self {
        Self {
            record,
            work: DeployWork::ComposeUp,
        }
    }
    /// True when there is nothing left for Docker to do, and therefore nothing
    /// to wait for.
    pub fn is_settled(&self) -> bool {
        matches!(self.work, DeployWork::Settled)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum DeployWork {
    /// Nothing for Docker to do: a rename, or a change of Hostname or
    /// aliases. The container is keyed by id and the route is a table entry,
    /// so only the route has to be rewritten.
    Settled,
    /// Apply network grants without starting an Application.
    NetworksOnly,
    Pull,
    Native {
        running: bool,
    },
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
    /// Pulls each image a registry serves first, then `docker compose up`,
    /// which also recreates the services whose image changed.
    PullComposeUp,
    RecoverCompose,
}

/// Accepts a native definition without touching Docker or starting code.
pub async fn prepare_deploy_native(
    store: &impl StateStore,
    name: &str,
    options: DeployOptions,
    environment: Vec<(String, String)>,
) -> Result<PendingDeploy, DeployError> {
    validate_app_name(name)?;
    if !store.is_initialized().await? {
        return Err(DeployError::NotInitialized);
    }
    if !matches!(options.runtime, Some(Runtime::Native(_))) {
        return Err(DeployError::InvalidNative(
            "native source requires native Runtime".into(),
        ));
    }
    crate::native::lifecycle::validate_environment(&environment)
        .map_err(|e| DeployError::InvalidNative(e.to_string()))?;
    let previous = store.find_application_by_name(name).await?;
    let record = pending_record(store, name, String::new(), SOURCE_NATIVE, None, &options).await?;
    let running = previous.is_none_or(|a| a.status != STATUS_STOPPED);
    store.insert_application(&record).await?;
    for (key, value) in environment {
        store.set_env(&record.id, &key, &value).await?;
    }
    Ok(PendingDeploy {
        record,
        work: DeployWork::Native { running },
    })
}

impl PendingDeploy {
    pub fn native_intent(&self) -> Option<bool> {
        match self.work {
            DeployWork::Native { running } => Some(running),
            _ => None,
        }
    }
}

async fn prepare_native_update(
    store: &impl StateStore,
    current: ApplicationRecord,
    update: ApplicationUpdate,
) -> Result<PendingDeploy, DeployError> {
    settle_publication(update.publication, Some(current.publication))?;
    if update.image.is_some()
        || update.compose.is_some()
        || update.development.is_some()
        || update.web_service.is_some()
        || update.web_port.is_some()
        || update.network_policy.is_some()
    {
        return Err(DeployError::InvalidNative(
            "native updates accept command, working_dir, limits and loopback port through runtime"
                .into(),
        ));
    }
    if matches!(update.runtime, Some(Runtime::Container)) {
        return Err(DeployError::InvalidNative(
            "Application Runtime cannot be changed".into(),
        ));
    }
    let mut record = current.clone();
    record.name = update.name.unwrap_or_else(|| current.name.clone());
    validate_app_name(&record.name)?;
    if let Some(clash) = store.find_application_by_name(&record.name).await?
        && clash.id != record.id
    {
        return Err(DeployError::AlreadyExists(record.name));
    }
    record.hostname = update.hostname.unwrap_or_else(|| current.hostname.clone());
    record.aliases = update.aliases.unwrap_or_else(|| current.aliases.clone());
    record.route_rules = update
        .route_rules
        .unwrap_or_else(|| current.route_rules.clone());
    record.runtime = update.runtime.unwrap_or_else(|| current.runtime.clone());
    let Runtime::Native(definition) = &mut record.runtime else {
        unreachable!()
    };
    crate::native::lifecycle::validate_definition(&record.id, definition, record.publication)
        .map_err(|e| DeployError::InvalidNative(e.to_string()))?;
    crate::native::lifecycle::validate_port(store, &record.id, definition.port).await?;
    record.web_target_port = definition.port;
    if record.publication == Publication::Unpublished {
        refuse_routing_for_unpublished(
            Some(&record.hostname),
            Some(&record.aliases),
            None,
            definition.port,
        )?;
    }
    validate_routing(store, &record).await?;
    let changed = record.runtime != current.runtime;
    let running = current.status != STATUS_STOPPED;
    if changed {
        record.status = STATUS_PENDING.into();
        record.last_error = None;
    }
    store.insert_application(&record).await?;
    Ok(PendingDeploy {
        record,
        work: if changed {
            DeployWork::Native { running }
        } else {
            DeployWork::Settled
        },
    })
}

/// Writes the `pending` row for an image deploy. Nothing has reached Docker
/// yet; `finish_deploy` is what does the pulling.
pub async fn prepare_deploy_from_image(
    store: &impl StateStore,
    name: &str,
    image: &str,
    options: DeployOptions,
) -> Result<PendingDeploy, DeployError> {
    validate_app_name(name)?;

    if image.is_empty() {
        return Err(DeployError::MissingImage);
    }

    if !store.is_initialized().await? {
        return Err(DeployError::NotInitialized);
    }

    let record =
        pending_record(store, name, image.to_string(), SOURCE_IMAGE, None, &options).await?;
    store.insert_application(&record).await?;

    Ok(PendingDeploy {
        record,
        work: DeployWork::Pull,
    })
}

/// Checks a Compose definition and resolves where its Hostname points. The
/// resolved target is what gets recorded, so the route never has to guess
/// again: what the console shows is what the proxy uses. An unpublished
/// Application has no Web Target to resolve; its image is the first
/// service's, so the listing still says what it runs. `variables` are the
/// Application's, when it already exists; a new one has none yet.
fn check_compose(
    compose: &str,
    web_service: Option<&str>,
    web_port: Option<u16>,
    publication: Publication,
    variables: &[(String, String)],
) -> Result<ComposeSpec, DeployError> {
    if compose.trim().is_empty() {
        return Err(DeployError::MissingCompose);
    }
    // The Operator may still be about to set the Variables the file names,
    // so a requirement is not raised here; `project_for` raises it before
    // anything runs (ADR-0030).
    let definition = ComposeDefinition::parse_with(compose, variables, Resolution::Check)?;
    // A form sends the field it shows, empty or not. Empty means "the
    // default", the same as leaving it out.
    let web_service = web_service.map(str::trim).filter(|s| !s.is_empty());
    if publication == Publication::Unpublished {
        refuse_routing_for_unpublished(None, None, web_service, web_port)?;
        return Ok(ComposeSpec {
            compose: compose.to_string(),
            image: definition
                .services
                .first()
                .map(|s| s.image.clone())
                .unwrap_or_default(),
            web_service: None,
            web_port: None,
        });
    }
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
    options: DeployOptions,
) -> Result<PendingDeploy, DeployError> {
    validate_app_name(name)?;

    let publication = publication_for(store, name, &options).await?;
    let variables = variables_for(store, name).await?;
    let spec = check_compose(compose, web_service, web_port, publication, &variables)?;

    if !store.is_initialized().await? {
        return Err(DeployError::NotInitialized);
    }

    let record = pending_record(
        store,
        name,
        spec.image.clone(),
        SOURCE_COMPOSE,
        Some(spec),
        &options,
    )
    .await?;
    store.insert_application(&record).await?;

    Ok(PendingDeploy {
        record,
        work: DeployWork::ComposeUp,
    })
}

/// The Variables a deploy under `name` can already count on: the existing
/// Application's, when the name is taken, and none for a new one.
async fn variables_for(
    store: &impl StateStore,
    name: &str,
) -> Result<Vec<(String, String)>, DeployError> {
    match store.find_application_by_name(name).await? {
        Some(existing) => Ok(store.get_all_env(&existing.id).await?),
        None => Ok(Vec::new()),
    }
}

/// Records a development Application as generated Compose plus explicit form
/// metadata. This path is the only one that attaches that metadata.
pub async fn prepare_deploy_from_development(
    store: &impl StateStore,
    name: &str,
    development: DevelopmentApplication,
    options: DeployOptions,
) -> Result<PendingDeploy, DeployError> {
    validate_app_name(name)?;
    validate_development(&development)?;
    let compose = development_compose(&development);
    // A development Application is a Web Target by construction, so an
    // unpublished one is refused here for naming a Web Target.
    let publication = publication_for(store, name, &options).await?;
    let spec = check_compose(
        &compose,
        Some("web"),
        Some(development.web_port),
        publication,
        &[],
    )?;
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
        Some(spec),
        &options,
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
/// Application's Variables resolved into the file (ADR-0030). This is the
/// point where a `${VAR:?}` requirement is enforced: nothing reaches Docker
/// with a required Variable missing.
pub async fn project_for(
    store: &impl StateStore,
    record: &ApplicationRecord,
) -> Result<ComposeProject, DeployError> {
    let compose = record
        .compose
        .as_deref()
        .ok_or(DeployError::MissingCompose)?;
    let env = store.get_all_env(&record.id).await?;
    let definition = ComposeDefinition::parse_with(compose, &env, Resolution::Run)?;
    // Only a published Application has a Web Target to put on loopback; an
    // unpublished project stays on the Application network (ADR-0028).
    let mut published = match (record.publication, record.web_target_port) {
        (Publication::Web, Some(host_port)) => Some(compose_app::PublishedTarget {
            target: definition.web_target(record.web_service.as_deref(), record.web_port)?,
            host_port,
        }),
        _ => None,
    };
    if published.is_none() {
        published = crate::postgres::native_publication(store, &record.id).await?;
    }
    let mut overrides = record
        .development
        .as_ref()
        .map(|_| {
            compose_app::RenderOverrides::service_hostname(
                "web",
                technical_label(&record.name, &record.id),
            )
        })
        .unwrap_or_default();
    overrides.git_nonroot = record.git.is_some();
    crate::connectivity::validate_definition(&record.network_policy, &definition)
        .map_err(DeployError::Connectivity)?;
    overrides.network_plan = Some(crate::connectivity::plan(
        record,
        &store.list_applications().await?,
    ));
    Ok(definition.render_with_overrides(
        &project_name_for(&record.id),
        &project_dir_for(&record.id),
        &identity_labels(&record.id, &record.name),
        &env,
        record.variable_delivery,
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
    finish_deploy_reporting(store, docker, routes, pending)
        .await
        .map(|(record, _)| record)
}

/// `finish_deploy`, answering also with the image id each pull of an
/// existing image went from and to, for the event of the task that ran it.
pub async fn finish_deploy_reporting(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    routes: &(impl RouteStore + ?Sized),
    pending: PendingDeploy,
) -> Result<(ApplicationRecord, Vec<crate::audit::Change>), DeployError> {
    let PendingDeploy { record, work } = pending;

    if matches!(work, DeployWork::Settled | DeployWork::NetworksOnly) {
        if matches!(work, DeployWork::NetworksOnly) {
            reconcile_private_connections(store, docker).await?;
        }
        let current = get_application(store, &record.id).await?;
        if current.status == STATUS_RUNNING {
            routes.publish(&current);
        } else {
            routes.withdraw(&current.id);
        }
        return Ok((current, Vec::new()));
    }

    let mut deployment = crate::deployments::begin(store, &record).await?;
    let mut changes = Vec::new();
    let result = async {
        reconcile_private_connections(store, docker).await?;
        ensure_application_networks(store, docker, &record).await?;
        match &work {
            DeployWork::Pull => docker.pull_image(&record.image).await?,
            DeployWork::Build { path } => docker.build_image(path, &record.image).await?,
            DeployWork::Recreate { pull } => {
                if *pull {
                    changes = pull_images(docker, std::slice::from_ref(&record.image)).await?;
                }
                let _ = docker
                    .remove_container(&container_name_for(&record.id))
                    .await;
            }
            DeployWork::ComposeUp | DeployWork::PullComposeUp | DeployWork::RecoverCompose => {
                if matches!(work, DeployWork::PullComposeUp) {
                    changes = pull_images(docker, &registry_images(&record)).await?;
                }
                let project = project_for(store, &record).await?;
                if matches!(work, DeployWork::RecoverCompose) {
                    docker.compose_up_pinned(&project).await?;
                } else {
                    docker.compose_up(&project).await?;
                }
                return crate::deployments::complete(store, docker, &mut deployment).await;
            }
            DeployWork::Native { .. } => return Err(DeployError::NativeUnavailable),
            DeployWork::Settled | DeployWork::NetworksOnly => unreachable!(),
        }
        start_container(store, docker, &record).await?;
        crate::deployments::complete(store, docker, &mut deployment).await
    }
    .await;

    if let Err(error) = &result {
        crate::deployments::failed(store, &mut deployment, error).await;
        routes.withdraw(&record.id);
    }
    let current = record_outcome(store, record, result).await?;
    // Publish after the container is up, using the names currently on record.
    routes.publish(&current);
    Ok((current, changes))
}

/// Reuses an immutable snapshot. The caller verified every local image first.
pub(crate) async fn restore_snapshot(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    routes: &(impl RouteStore + ?Sized),
    mut snapshot: ApplicationRecord,
) -> Result<(), DeployError> {
    validate_routing(store, &snapshot).await?;
    validate_connectivity(store, &snapshot).await?;
    if snapshot.compose.is_some() {
        project_for(store, &snapshot).await?;
    }
    if snapshot.status == STATUS_STOPPED {
        let current = get_application(store, &snapshot.id).await?;
        if current.compose.is_some() {
            docker
                .compose_down(&project_for(store, &current).await?)
                .await?;
        } else if docker
            .container_state(&container_name_for(&current.id))
            .await?
            .is_some()
        {
            docker
                .remove_container(&container_name_for(&current.id))
                .await?;
        }
        snapshot.last_error = None;
        store.insert_application(&snapshot).await?;
        routes.withdraw(&snapshot.id);
        return Ok(());
    }
    snapshot.status = STATUS_PENDING.into();
    snapshot.last_error = None;
    let work = if snapshot.compose.is_some() {
        DeployWork::RecoverCompose
    } else {
        DeployWork::Recreate { pull: false }
    };
    store.insert_application(&snapshot).await?;
    finish_deploy(
        store,
        docker,
        routes,
        PendingDeploy {
            record: snapshot,
            work,
        },
    )
    .await
    .map(drop)
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

    if published.compose.is_some() {
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
    crate::deployments::recover(store).await?;
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

    // A deploy still waiting in the task queue was never started, so there is
    // nothing to settle: the scheduler carries it out after this.
    let queued = crate::tasks::queued_application_ids(store).await?;
    if executor_available {
        reconcile_private_connections(store, docker).await?;
    }
    for mut app in store.list_applications().await? {
        if matches!(app.runtime, Runtime::Native(_)) {
            continue;
        }
        if app.status == STATUS_PENDING && executor_available && !queued.contains(&app.id) {
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

        // An unpublished Application never gets a Host port: nothing routes
        // to it (ADR-0028).
        if app.status == STATUS_RUNNING
            && app.publication == Publication::Web
            && app.web_target_port.is_none()
            && executor_available
        {
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
    options: DeployOptions,
) -> Result<ApplicationRecord, DeployError> {
    let pending = prepare_deploy_from_image(store, name, image, options).await?;
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
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
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
    /// Pull each image a registry serves before redeploying, so a moving tag
    /// picks up what the registry has now. A redeploy that pulls is Docker's
    /// business even when nothing else changed.
    pub pull: bool,
    /// A native Runtime is refused; a container one is what the row has.
    pub runtime: Option<Runtime>,
    /// Must match what the row has: Publication is chosen at creation.
    pub publication: Option<Publication>,
    /// May change either way. On a Compose Application the rendered project
    /// changes with it, so Docker is asked.
    pub variable_delivery: Option<VariableDelivery>,
    pub route_rules: Option<Vec<crate::store::RouteRule>>,
    pub network_policy: Option<crate::store::NetworkPolicy>,
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
    if crate::postgres::metadata(store, id).await?.is_some() {
        return Err(DeployError::Connectivity(
            crate::connectivity::ConnectivityError(
                "Use the managed PostgreSQL controls to preserve its version, credentials and data"
                    .into(),
            ),
        ));
    }

    if matches!(current.runtime, Runtime::Native(_)) {
        return prepare_native_update(store, current, update).await;
    }
    if matches!(update.runtime, Some(Runtime::Native(_))) {
        return Err(DeployError::InvalidNative(
            "Application Runtime cannot be changed".into(),
        ));
    }
    // Same value or unsaid: a no-op. The other kind: refused (ADR-0028).
    settle_publication(update.publication, Some(current.publication))?;
    if current.publication == Publication::Unpublished {
        refuse_routing_for_unpublished(
            update.hostname.as_deref(),
            update.aliases.as_deref(),
            update.web_service.as_deref(),
            update.web_port,
        )?;
    }
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
        variable_delivery: update
            .variable_delivery
            .unwrap_or(current.variable_delivery),
        route_rules: update
            .route_rules
            .clone()
            .unwrap_or_else(|| current.route_rules.clone()),
        network_policy: update
            .network_policy
            .clone()
            .unwrap_or_else(|| current.network_policy.clone()),
        ..current.clone()
    };

    // A Compose Application shows the image behind its Hostname; the file is
    // what the Operator edits.
    if current.compose.is_some() {
        let variables = store.get_all_env(&current.id).await?;
        let spec = check_compose(
            record.compose.as_deref().unwrap_or_default(),
            record.web_service.as_deref(),
            record.web_port,
            current.publication,
            &variables,
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
    validate_connectivity(store, &record).await?;

    let image_changed = record.image != current.image;
    let file_changed = record.compose != current.compose;
    let delivery_changed = record.variable_delivery != current.variable_delivery;
    let network_changed = record.network_policy != current.network_policy;
    if network_changed && current.compose.is_none() {
        return Err(DeployError::Connectivity(
            crate::connectivity::ConnectivityError(
                "network policy changes require a Compose Application".into(),
            ),
        ));
    }
    let pull = update.pull && !registry_images(&record).is_empty();

    store.insert_application(&record).await?;

    // Only the image is Docker's business. A rename, a new Hostname, an
    // added alias and a new web target are all a route rewrite, which costs
    // no downtime. How Variables reach a project is part of the rendered
    // file, so that change is Docker's too.
    let needs_docker = pull
        || network_changed
        || if current.compose.is_some() {
            file_changed
                || delivery_changed
                || (record.development.is_some() && record.name != current.name)
        } else {
            image_changed
        };
    if !needs_docker || (current.status == STATUS_STOPPED && current.compose.is_some()) {
        record.status = current.status.clone();
        record.last_error = current.last_error.clone();
        store.insert_application(&record).await?;
        return Ok(PendingDeploy {
            record,
            work: if update.network_policy.is_some() {
                DeployWork::NetworksOnly
            } else {
                DeployWork::Settled
            },
        });
    }

    if current.compose.is_some() {
        return Ok(PendingDeploy {
            record,
            work: if pull {
                DeployWork::PullComposeUp
            } else {
                DeployWork::ComposeUp
            },
        });
    }

    Ok(PendingDeploy {
        record,
        work: DeployWork::Recreate {
            pull: pull || (image_changed && current.source == SOURCE_IMAGE),
        },
    })
}

/// Saves new aliases or path rules and nothing else. What runs is untouched,
/// so nothing is built, pulled or restarted, whatever the Application is
/// made from: the deploy is settled, and finishing it rewrites the routes.
pub async fn prepare_routes(
    store: &impl StateStore,
    id: &str,
    aliases: Option<Vec<String>>,
    route_rules: Option<Vec<crate::store::RouteRule>>,
    rewrite_host: Option<Option<bool>>,
) -> Result<PendingDeploy, DeployError> {
    if !store.is_initialized().await? {
        return Err(DeployError::NotInitialized);
    }
    let mut record = get_application(store, id).await?;
    if record.publication == Publication::Unpublished {
        refuse_routing_for_unpublished(None, aliases.as_deref(), None, None)?;
    }
    if let Some(aliases) = aliases {
        record.aliases = aliases;
    }
    if let Some(rules) = route_rules {
        record.route_rules = rules;
    }
    if let Some(rewrite_host) = rewrite_host {
        record.rewrite_host = rewrite_host;
    }
    validate_routing(store, &record).await?;
    store.insert_application(&record).await?;
    Ok(PendingDeploy {
        record,
        work: DeployWork::Settled,
    })
}

/// Writes the `pending` row for a build-from-source deploy. The path rides
/// along in the work: it is the caller's, not something the record keeps.
pub async fn prepare_deploy_from_path(
    store: &impl StateStore,
    name: &str,
    path: &str,
    options: DeployOptions,
) -> Result<PendingDeploy, DeployError> {
    validate_app_name(name)?;

    if path.is_empty() {
        return Err(DeployError::MissingPath);
    }

    if !store.is_initialized().await? {
        return Err(DeployError::NotInitialized);
    }

    // Preserve legacy build tags on redeploy. New tags use immutable identity
    // so human names and later renames cannot collide in Docker's registry.
    let image_tag = store
        .find_application_by_name(name)
        .await?
        .filter(|app| app.source == SOURCE_PATH)
        .map(|app| app.image)
        .unwrap_or_default();
    let mut record = pending_record(store, name, image_tag, SOURCE_PATH, None, &options).await?;
    if record.image.is_empty() {
        record.image = format!("self-host-{}:latest", record.id);
    }
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
    options: DeployOptions,
) -> Result<ApplicationRecord, DeployError> {
    let pending = prepare_deploy_from_path(store, name, path, options).await?;
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
    ConnectionsExist,
    ManagedDatabase,
    NotInitialized,
    NotFound(String),
    Docker(DockerError),
    Store(StoreError),
}

impl std::fmt::Display for RemoveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoveError::ManagedDatabase => write!(
                f,
                "remove PostgreSQL through its database controls and choose whether to keep its data"
            ),
            RemoveError::ConnectionsExist => write!(
                f,
                "remove private connection grants before removing the Application"
            ),
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

pub async fn require_removable(
    store: &impl StateStore,
    app: &ApplicationRecord,
) -> Result<(), RemoveError> {
    if crate::postgres::metadata(store, &app.id).await?.is_some() {
        return Err(RemoveError::ManagedDatabase);
    }
    let applications = store.list_applications().await?;
    let grants = |record: &ApplicationRecord| match &record.network_policy {
        crate::store::NetworkPolicy::Private { consumers } => consumers.clone(),
        _ => Vec::new(),
    };
    if crate::postgres::active_reference(store, &app.id).await?
        || !grants(app).is_empty()
        || applications
            .iter()
            .any(|other| grants(other).contains(&app.id))
    {
        return Err(RemoveError::ConnectionsExist);
    }
    Ok(())
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

    require_removable(store, &app).await?;

    // The project is rendered before the row goes, because rendering reads
    // the row's environment.
    let project = if app.compose.is_some() {
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
    let app = get_application(store, id).await?;

    routes.withdraw(&app.id);
    if app.compose.is_some() {
        docker
            .compose_stop(&project_for(store, &app).await?)
            .await?;
    } else {
        docker.stop_container(&container_name_for(&app.id)).await?;
    }

    Ok(store
        .set_application_outcome(id, STATUS_STOPPED, None)
        .await?)
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
        reconcile_private_connections(store, docker).await?;
        if app.compose.is_some() {
            docker.compose_up(&project_for(store, &app).await?).await?;
        } else if docker
            .container_state(&container_name_for(&app.id))
            .await?
            .is_none()
        {
            start_container(store, docker, &app).await?;
        } else {
            docker.start_container(&container_name_for(&app.id)).await?;
        }
        Ok(())
    }
    .await;
    let current = record_outcome(store, app, result).await?;
    routes.publish(&current);
    Ok(current)
}

/// Restarts the Application. With `pull`, each image a registry serves is
/// pulled first and the containers are recreated from it, so a moving tag
/// such as `:latest` picks up what the registry has now. Answers with the
/// image id each pull went from and to.
///
/// A pull that fails ends the restart before anything is touched: the
/// containers keep running on the images they had.
pub async fn restart_application(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    routes: &(impl RouteStore + ?Sized),
    id: &str,
    pull: bool,
) -> Result<(ApplicationRecord, Vec<crate::audit::Change>), DeployError> {
    let app = get_application(store, id).await?;
    let images = if pull {
        registry_images(&app)
    } else {
        Vec::new()
    };
    let changes = pull_images(docker, &images).await?;
    let mut deployment = if images.is_empty() {
        None
    } else {
        let deployment = crate::deployments::begin(store, &app).await?;
        store
            .set_application_outcome(id, STATUS_PENDING, None)
            .await?;
        Some(deployment)
    };
    let result = async {
        match (app.compose.is_some(), images.is_empty()) {
            (true, true) => {
                docker
                    .compose_restart(&project_for(store, &app).await?)
                    .await?
            }
            (true, false) => {
                docker
                    .compose_recreate(&project_for(store, &app).await?)
                    .await?
            }
            (false, true) => {
                docker
                    .restart_container(&container_name_for(&app.id))
                    .await?
            }
            (false, false) => {
                let _ = docker.remove_container(&container_name_for(&app.id)).await;
                start_container(store, docker, &app).await?;
            }
        }
        if let Some(deployment) = &mut deployment {
            crate::deployments::complete(store, docker, deployment).await?;
        }
        Ok(())
    }
    .await;
    if let (Some(deployment), Err(error)) = (&mut deployment, &result) {
        crate::deployments::failed(store, deployment, error).await;
        routes.withdraw(&app.id);
    }
    let current = record_outcome(store, app, result).await?;
    routes.publish(&current);
    Ok((current, changes))
}

/// The images a registry serves for this Application, once each. An image
/// built on the Host, from a path, a custom image or a development image,
/// has nowhere to be pulled from.
fn registry_images(app: &ApplicationRecord) -> Vec<String> {
    let images = match app.source.as_str() {
        SOURCE_IMAGE => vec![app.image.clone()],
        SOURCE_COMPOSE => app
            .compose
            .as_deref()
            .and_then(|compose| ComposeDefinition::parse(compose).ok())
            .map(|definition| {
                definition
                    .services
                    .into_iter()
                    .map(|service| service.image)
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut unique: Vec<String> = Vec::new();
    for image in images {
        if !crate::docker::built_on_host(&image) && !unique.contains(&image) {
            unique.push(image);
        }
    }
    unique
}

/// Pulls each image and says which id it had before and has now, so a pull
/// that changed nothing reads as such.
async fn pull_images(
    docker: &(impl DockerRuntime + ?Sized),
    images: &[String],
) -> Result<Vec<crate::audit::Change>, DeployError> {
    let mut changes = Vec::new();
    for image in images {
        let before = docker.image_id(image).await?;
        docker.pull_image(image).await?;
        let after = docker.image_id(image).await?;
        changes.push(crate::audit::Change {
            setting: image.clone(),
            from: short_image_id(before),
            to: short_image_id(after),
        });
    }
    Ok(changes)
}

/// `sha256:` and the first twelve digits, the way `docker images` prints an
/// id. `none` when the Host did not have the image.
fn short_image_id(id: Option<String>) -> String {
    match id {
        Some(id) => {
            let digits = id.strip_prefix("sha256:").unwrap_or(&id);
            format!("sha256:{}", &digits[..digits.len().min(12)])
        }
        None => "none".into(),
    }
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
    pub health: Option<String>,
}

/// The containers an Application is made of, as `(service, container)`. A
/// single-container Application has one service, called `app`.
pub fn containers_of(record: &ApplicationRecord) -> Vec<(String, String)> {
    if record.compose.is_none() {
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
                health: s.health,
            },
            None => ServiceState {
                service,
                container,
                state: "missing".into(),
                exit_code: None,
                restarts: None,
                health: None,
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
        .filter(|s| s.state != "running" || s.health.as_deref() == Some("unhealthy"))
        .map(|s| match s.state.as_str() {
            "running" if s.health.as_deref() == Some("unhealthy") => {
                format!("service '{}' is unhealthy", s.service)
            }
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
    /// The Compose project could not be rendered with the Variables as they
    /// now are: a required one is still missing, or the file will not parse.
    Definition(DeployError),
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
            EnvError::Definition(_) => {
                write!(f, "failed to render the Application with its Variables")
            }
            EnvError::Store(_) => write!(f, "failed to read or write the Application environment"),
        }
    }
}

impl std::error::Error for EnvError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EnvError::Docker(e) => Some(e),
            EnvError::Definition(e) => Some(e),
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

    change_env(store, docker, &app.id, key, Some(value)).await
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

    change_env(store, docker, &app.id, key, None).await
}

/// Task identity stays pinned even if the display name changes while queued.
pub(crate) async fn change_env(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    app_id: &str,
    key: &str,
    value: Option<&str>,
) -> Result<(), EnvError> {
    if !store.is_initialized().await? {
        return Err(EnvError::NotInitialized);
    }
    if store.get_application(app_id).await?.is_none() {
        return Err(EnvError::NotFound(app_id.into()));
    }
    match value {
        Some(value) => store.set_env(app_id, key, value).await?,
        None => store.unset_env(app_id, key).await?,
    }
    recreate_with_env(store, docker, app_id).await
}

async fn recreate_with_env(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    app_id: &str,
) -> Result<(), EnvError> {
    let app = store
        .get_application(app_id)
        .await?
        .ok_or_else(|| EnvError::NotFound(app_id.into()))?;
    let app = &app;

    ensure_application_networks(store, docker, app)
        .await
        .map_err(EnvError::Definition)?;
    if app.compose.is_some() {
        let project = project_for(store, app)
            .await
            .map_err(EnvError::Definition)?;
        docker.compose_up(&project).await?;
    } else {
        docker
            .remove_container(&container_name_for(&app.id))
            .await?;
        start_container(store, docker, app)
            .await
            .map_err(EnvError::Definition)?;
    }

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

/// A Git candidate waits in its Task. An existing release stays authoritative
/// until all checkout, build and definition checks succeed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingGitDeploy {
    pub record: ApplicationRecord,
    pub source: crate::source::GitSource,
    pub refresh_source: bool,
    pub update: Option<ApplicationUpdate>,
    /// A reviewed candidate applies to this Task, never to the saved source ref.
    #[serde(default)]
    pub source_revision: Option<String>,
    #[serde(default)]
    pub expected_git_revision: Option<String>,
}

pub async fn prepare_git_create(
    store: &impl StateStore,
    name: &str,
    source: crate::source::GitSource,
    web_service: Option<String>,
    web_port: Option<u16>,
    options: DeployOptions,
) -> Result<PendingGitDeploy, DeployError> {
    prepare_git_create_reviewed(store, name, source, web_service, web_port, options, None).await
}

#[allow(clippy::too_many_arguments)]
pub async fn prepare_git_create_reviewed(
    store: &impl StateStore,
    name: &str,
    source: crate::source::GitSource,
    web_service: Option<String>,
    web_port: Option<u16>,
    options: DeployOptions,
    source_revision: Option<String>,
) -> Result<PendingGitDeploy, DeployError> {
    crate::source::validate(&source)?;
    validate_reviewed_revision(&source, source_revision.as_deref())?;
    validate_app_name(name)?;
    if !store.is_initialized().await? {
        return Err(DeployError::NotInitialized);
    }
    if store.find_application_by_name(name).await?.is_some() {
        return Err(DeployError::AlreadyExists(name.into()));
    }
    if matches!(options.runtime, Some(Runtime::Native(_))) {
        return Err(DeployError::Source(crate::source::SourceError::Invalid(
            "Git builds require container Runtime".into(),
        )));
    }
    let mut record = pending_record(store, name, String::new(), SOURCE_GIT, None, &options).await?;
    if record.publication == Publication::Unpublished {
        refuse_routing_for_unpublished(None, None, web_service.as_deref(), web_port)?;
    }
    record.git = Some(source.clone());
    record.web_service = web_service;
    record.web_port = web_port;
    store.insert_application(&record).await?;
    Ok(PendingGitDeploy {
        record,
        source,
        refresh_source: false,
        update: None,
        source_revision,
        expected_git_revision: None,
    })
}

pub async fn prepare_git_update(
    store: &impl StateStore,
    current: ApplicationRecord,
    source: crate::source::GitSource,
    refresh_source: bool,
    update: ApplicationUpdate,
) -> Result<PendingGitDeploy, DeployError> {
    prepare_git_update_reviewed(store, current, source, refresh_source, update, None, None).await
}

pub async fn prepare_git_update_reviewed(
    store: &impl StateStore,
    current: ApplicationRecord,
    source: crate::source::GitSource,
    refresh_source: bool,
    update: ApplicationUpdate,
    source_revision: Option<String>,
    expected_git_revision: Option<String>,
) -> Result<PendingGitDeploy, DeployError> {
    crate::source::validate(&source)?;
    validate_reviewed_revision(&source, source_revision.as_deref())?;
    if source_revision.is_some() != expected_git_revision.is_some() {
        return Err(crate::source::SourceError::Invalid(
            "reviewed updates need both candidate and deployed Git revisions".into(),
        )
        .into());
    }
    if let Some(expected) = expected_git_revision.as_deref() {
        let mut unpinned = source.clone();
        unpinned.revision = None;
        validate_reviewed_revision(&unpinned, Some(expected))?;
        let saved = get_application(store, &current.id).await?;
        check_expected_git_revision(&saved, Some(expected))?;
    }
    if current.git.is_none() || current.runtime != Runtime::Container {
        return Err(DeployError::Source(crate::source::SourceError::Invalid(
            "Git source updates require an existing Git Application".into(),
        )));
    }
    if current
        .git
        .as_ref()
        .is_some_and(|old| old.compose_path.is_some() != source.compose_path.is_some())
    {
        return Err(DeployError::Source(crate::source::SourceError::Invalid(
            "Git build type cannot change after creation".into(),
        )));
    }
    if update.image.is_some()
        || update.compose.is_some()
        || update.development.is_some()
        || matches!(update.runtime, Some(Runtime::Native(_)))
    {
        return Err(DeployError::Source(crate::source::SourceError::Invalid(
            "Git source accepts no image, inline Compose, development definition or native Runtime"
                .into(),
        )));
    }
    let mut record = current.clone();
    apply_git_update(store, &mut record, &update).await?;
    validate_routing(store, &record).await?;
    validate_connectivity(store, &record).await?;
    Ok(PendingGitDeploy {
        record,
        source,
        refresh_source,
        update: Some(update),
        source_revision,
        expected_git_revision,
    })
}

fn validate_reviewed_revision(
    source: &crate::source::GitSource,
    revision: Option<&str>,
) -> Result<(), DeployError> {
    let Some(revision) = revision else {
        return Ok(());
    };
    if source
        .revision
        .as_deref()
        .is_some_and(|pin| !pin.eq_ignore_ascii_case(revision))
    {
        return Err(crate::source::SourceError::Invalid(
            "reviewed commit conflicts with the explicit source pin".into(),
        )
        .into());
    }
    let mut candidate = source.clone();
    candidate.revision = Some(revision.into());
    crate::source::validate(&candidate)?;
    Ok(())
}

fn check_expected_git_revision(
    record: &ApplicationRecord,
    expected: Option<&str>,
) -> Result<(), DeployError> {
    if let Some(expected) = expected
        && !record
            .git_build
            .as_ref()
            .is_some_and(|build| build.revision.eq_ignore_ascii_case(expected))
    {
        return Err(crate::source::SourceError::Invalid(
            "the deployed Git revision changed; check for updates again".into(),
        )
        .into());
    }
    Ok(())
}

async fn apply_git_update(
    store: &impl StateStore,
    record: &mut ApplicationRecord,
    update: &ApplicationUpdate,
) -> Result<(), DeployError> {
    settle_publication(update.publication, Some(record.publication))?;
    if record.publication == Publication::Unpublished {
        refuse_routing_for_unpublished(
            update.hostname.as_deref(),
            update.aliases.as_deref(),
            update.web_service.as_deref(),
            update.web_port,
        )?;
    }
    if let Some(name) = &update.name {
        validate_app_name(name)?;
        if store
            .find_application_by_name(name)
            .await?
            .is_some_and(|other| other.id != record.id)
        {
            return Err(DeployError::AlreadyExists(name.clone()));
        }
        record.name = name.clone();
    }
    if let Some(hostname) = &update.hostname {
        record.hostname = hostname.clone();
    }
    if let Some(aliases) = &update.aliases {
        record.aliases = aliases.clone();
    }
    if let Some(service) = &update.web_service {
        record.web_service = Some(service.clone());
    }
    if let Some(port) = update.web_port {
        record.web_port = Some(port);
    }
    if let Some(delivery) = update.variable_delivery {
        record.variable_delivery = delivery;
    }
    if let Some(rules) = &update.route_rules {
        record.route_rules = rules.clone();
    }
    if let Some(policy) = &update.network_policy {
        record.network_policy = policy.clone();
    }
    Ok(())
}

pub async fn finish_git_deploy(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    routes: &(impl RouteStore + ?Sized),
    pending: PendingGitDeploy,
) -> Result<(ApplicationRecord, Vec<crate::audit::Change>), DeployError> {
    let id = pending.record.id.clone();
    let result = finish_git_deploy_inner(store, docker, routes, pending).await;
    if let Err(error) = &result
        && let Ok(current) = get_application(store, &id).await
        && current.git_build.is_none()
        && current.status == STATUS_PENDING
    {
        let _ = store
            .set_application_outcome(&id, STATUS_FAILED, Some(ErrorReport::new(error)))
            .await;
        routes.withdraw(&id);
    }
    result
}

async fn finish_git_deploy_inner(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    routes: &(impl RouteStore + ?Sized),
    pending: PendingGitDeploy,
) -> Result<(ApplicationRecord, Vec<crate::audit::Change>), DeployError> {
    let current = get_application(store, &pending.record.id).await?;
    check_expected_git_revision(&current, pending.expected_git_revision.as_deref())?;
    validate_reviewed_revision(&pending.source, pending.source_revision.as_deref())?;
    let unchanged_selection = current.git.as_ref().is_some_and(|saved| {
        saved.repository == pending.source.repository && saved.git_ref == pending.source.git_ref
    });
    let pinned = if let Some(reviewed) = pending.source_revision.as_deref() {
        Some(reviewed)
    } else if !pending.refresh_source && unchanged_selection {
        current
            .git_build
            .as_ref()
            .map(|build| build.revision.as_str())
    } else {
        None
    };
    let result = crate::source::build(
        &crate::source::private_root(),
        &current.id,
        &pending.source,
        pinned,
        docker,
    )
    .await;
    let built = match result {
        Ok(built) => built,
        Err(error) => {
            if current.git_build.is_none() && current.status == STATUS_PENDING {
                store
                    .set_application_outcome(
                        &current.id,
                        STATUS_FAILED,
                        Some(ErrorReport::new(&error)),
                    )
                    .await?;
                routes.withdraw(&current.id);
            }
            return Err(error.into());
        }
    };
    let mut record = get_application(store, &current.id).await?;
    check_expected_git_revision(&record, pending.expected_git_revision.as_deref())?;
    if let Some(update) = &pending.update {
        apply_git_update(store, &mut record, update).await?;
    }
    record.image = built.image;
    record.compose = built.compose;
    record.git = Some(pending.source);
    record.git_build = Some(built.build);
    record.source = SOURCE_GIT.into();
    if let Some(compose) = &record.compose {
        let spec = check_compose(
            compose,
            record.web_service.as_deref(),
            record.web_port,
            record.publication,
            &store.get_all_env(&record.id).await?,
        )?;
        record.image = spec.image;
        record.web_service = spec.web_service;
        record.web_port = spec.web_port;
    } else {
        if record
            .web_service
            .as_deref()
            .is_some_and(|service| !service.is_empty())
        {
            return Err(DeployError::Source(crate::source::SourceError::Invalid(
                "a Dockerfile source has no Compose web service".into(),
            )));
        }
        if record.web_port == Some(0) {
            return Err(DeployError::Source(crate::source::SourceError::Invalid(
                "web port must be nonzero".into(),
            )));
        }
    }
    validate_routing(store, &record).await?;
    validate_connectivity(store, &record).await?;
    let mut changes = Vec::new();
    if let (Some(before), Some(after)) = (&current.git_build, &record.git_build)
        && before.revision != after.revision
    {
        changes.push(crate::audit::Change {
            setting: "Git revision".into(),
            from: before.revision.clone(),
            to: after.revision.clone(),
        });
    }
    if current.status == STATUS_STOPPED {
        if current.compose.is_some() {
            docker
                .compose_down(&project_for(store, &current).await?)
                .await?;
        } else if docker
            .container_state(&container_name_for(&current.id))
            .await?
            .is_some()
        {
            docker
                .remove_container(&container_name_for(&current.id))
                .await?;
        }
        record.status = STATUS_STOPPED.into();
        record.last_error = None;
        store.insert_application(&record).await?;
        routes.withdraw(&record.id);
        return Ok((record, changes));
    }
    record.status = STATUS_PENDING.into();
    record.last_error = None;
    let work = if record.compose.is_some() {
        DeployWork::ComposeUp
    } else {
        DeployWork::Recreate { pull: false }
    };
    store.insert_application(&record).await?;
    let (record, mut runtime_changes) =
        finish_deploy_reporting(store, docker, routes, PendingDeploy { record, work }).await?;
    changes.append(&mut runtime_changes);
    Ok((record, changes))
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

    async fn private_fixture() -> (
        FakeStateStore,
        FakeDocker,
        FakeRoutes,
        ApplicationRecord,
        ApplicationRecord,
    ) {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let consumer = deploy_from_image(
            &store,
            &docker,
            &routes,
            "consumer",
            "nginx",
            DeployOptions::default(),
        )
        .await
        .unwrap();
        let pending = prepare_deploy_from_compose(
            &store,
            "database",
            "services:\n  db:\n    image: postgres:17\n",
            None,
            None,
            DeployOptions {
                publication: Some(Publication::Unpublished),
                network_policy: Some(crate::store::NetworkPolicy::Private {
                    consumers: vec![consumer.id.clone()],
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let provider = finish_deploy(&store, &docker, &routes, pending)
            .await
            .unwrap();
        (store, docker, routes, consumer, provider)
    }

    #[tokio::test]
    async fn revoking_a_grant_on_a_stopped_provider_keeps_it_stopped() {
        let (store, docker, routes, consumer, provider) = private_fixture().await;
        stop_application(&store, &docker, &routes, &provider.id)
            .await
            .unwrap();
        let previous = docker.recreated.lock().unwrap().len();
        let saved = update_application(
            &store,
            &docker,
            &routes,
            &provider.id,
            ApplicationUpdate {
                network_policy: Some(crate::store::NetworkPolicy::private()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(saved.status, STATUS_STOPPED);
        assert_eq!(docker.recreated.lock().unwrap().len(), previous);
        assert_eq!(
            docker
                .container_state(&compose_app::container_name(
                    &project_name_for(&provider.id),
                    "db"
                ))
                .await
                .unwrap()
                .unwrap()
                .status,
            "exited"
        );
        assert!(
            docker
                .network_syncs
                .lock()
                .unwrap()
                .contains(&(container_name_for(&consumer.id), vec![]))
        );
        assert!(routes.get(&provider.id).is_none());
        let started = start_application(&store, &docker, &routes, &provider.id)
            .await
            .unwrap();
        assert_eq!(started.status, STATUS_RUNNING);
        let project = docker.project(&project_name_for(&provider.id)).unwrap();
        assert!(
            !project
                .yaml
                .contains(&crate::connectivity::provider_network(&provider.id))
        );
    }

    #[tokio::test]
    async fn revocation_precedes_failed_deploy_and_is_retried_from_saved_policy() {
        let (store, mut docker, routes, consumer, provider) = private_fixture().await;
        docker.compose_failure = Some("synthetic deployment failure".into());
        let result = update_application(
            &store,
            &docker,
            &routes,
            &provider.id,
            ApplicationUpdate {
                network_policy: Some(crate::store::NetworkPolicy::private()),
                ..Default::default()
            },
        )
        .await;
        assert!(result.is_err());
        let detached = (container_name_for(&consumer.id), vec![]);
        assert!(docker.network_syncs.lock().unwrap().contains(&detached));
        docker.network_syncs.lock().unwrap().clear();
        docker.compose_failure = None;
        update_application(
            &store,
            &docker,
            &routes,
            &provider.id,
            ApplicationUpdate {
                network_policy: Some(crate::store::NetworkPolicy::private()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(docker.network_syncs.lock().unwrap().contains(&detached));
        docker.network_syncs.lock().unwrap().clear();
        reconcile(&store, &docker, &routes).await.unwrap();
        assert!(docker.network_syncs.lock().unwrap().contains(&detached));
    }

    #[tokio::test]
    async fn an_unpublished_database_reports_health_without_a_route() {
        let (store, docker, routes, _, provider) = private_fixture().await;
        let container = compose_app::container_name(&project_name_for(&provider.id), "db");
        docker
            .health
            .lock()
            .unwrap()
            .insert(container, "unhealthy".into());
        let services = service_states(&docker, &provider).await;
        assert_eq!(services[0].health.as_deref(), Some("unhealthy"));
        assert_eq!(live_status(&provider, &services).0, STATUS_FAILED);
        assert!(routes.get(&provider.id).is_none());
        assert!(
            store
                .get_application(&provider.id)
                .await
                .unwrap()
                .unwrap()
                .web_target_port
                .is_none()
        );
    }

    #[tokio::test]
    async fn editing_routes_on_a_stopped_application_keeps_routes_withdrawn() {
        let (store, docker, routes, consumer, _) = private_fixture().await;
        stop_application(&store, &docker, &routes, &consumer.id)
            .await
            .unwrap();
        let saved = update_application(
            &store,
            &docker,
            &routes,
            &consumer.id,
            ApplicationUpdate {
                route_rules: Some(vec![crate::store::RouteRule {
                    hostname: consumer.hostname.clone(),
                    path_prefix: "/app".into(),
                    target: Some("127.0.0.1:28001".parse().unwrap()),
                    strip_prefix: false,
                }]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(saved.status, STATUS_STOPPED);
        assert!(routes.get(&consumer.id).is_none());
        assert_eq!(
            docker
                .container_state(&container_name_for(&consumer.id))
                .await
                .unwrap()
                .unwrap()
                .status,
            "exited"
        );
    }

    #[tokio::test]
    async fn preparing_a_deploy_records_it_without_touching_docker() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();

        let routes = FakeRoutes::new();

        let pending =
            prepare_deploy_from_image(&store, "blog", "nginx:alpine", DeployOptions::default())
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

        deploy_from_image(
            &store,
            &docker,
            &routes,
            "blog",
            "nginx:alpine",
            DeployOptions::default(),
        )
        .await
        .unwrap();

        // Off the Platform Infra bridge, an Application cannot open a socket
        // on the state store, whose password is the same on every install.
        let containers = docker.apps.lock().unwrap();
        assert!(containers[0].network.starts_with("sf-app-"));
        assert_ne!(containers[0].network, crate::docker::APP_NETWORK);
    }

    #[tokio::test]
    async fn reconcile_keeps_a_pending_application_whose_container_is_up() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        deploy_from_image(
            &store,
            &docker,
            &routes,
            "blog",
            "nginx:alpine",
            DeployOptions::default(),
        )
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
        deploy_from_image(
            &store,
            &docker,
            &routes,
            "blog",
            "nginx:alpine",
            DeployOptions::default(),
        )
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
        deploy_from_image(
            &store,
            &docker,
            &routes,
            "blog",
            "nginx:alpine",
            DeployOptions::default(),
        )
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
        deploy_from_image(
            &store,
            &docker,
            &routes,
            "blog",
            "nginx:alpine",
            DeployOptions::default(),
        )
        .await
        .unwrap();
        deploy_from_image(
            &store,
            &docker,
            &routes,
            "journal",
            "nginx:alpine",
            DeployOptions::default(),
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
        let pending =
            prepare_deploy_from_image(&store, "blog", "nginx:alpine", DeployOptions::default())
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

    #[tokio::test]
    async fn display_names_keep_their_spelling_while_hostnames_and_identity_stay_separate() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let name = "  T3 Code / API $1  ";
        let app = deploy_from_image(
            &store,
            &docker,
            &routes,
            name,
            "nginx",
            DeployOptions::default(),
        )
        .await
        .unwrap();
        assert_eq!(app.name, name);
        assert_eq!(app.hostname, "t3-code-api-1.home.lan");
        assert_eq!(
            docker.apps.lock().unwrap()[0].name,
            container_name_for(&app.id)
        );
        assert!(
            docker.apps.lock().unwrap()[0]
                .labels
                .contains(&("sf.app.name".into(), name.into()))
        );
        let renamed = update_application(
            &store,
            &docker,
            &routes,
            &app.id,
            ApplicationUpdate {
                name: Some("Outra Aplicação!".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(renamed.name, "Outra Aplicação!");
        assert_eq!(renamed.id, app.id);
        assert_eq!(renamed.hostname, app.hostname);
        assert_eq!(docker.apps.lock().unwrap().len(), 1);

        let first = prepare_deploy_from_image(&store, "Teste", "nginx", DeployOptions::default())
            .await
            .unwrap()
            .record;
        let second = prepare_deploy_from_image(&store, "teste", "nginx", DeployOptions::default())
            .await
            .unwrap()
            .record;
        assert_eq!(first.hostname, "teste.home.lan");
        assert_eq!(second.hostname, format!("teste-{}.home.lan", second.id));
        assert_ne!(first.id, second.id);
        assert_eq!(
            store
                .find_application_by_name("Teste")
                .await
                .unwrap()
                .unwrap()
                .id,
            first.id
        );
        assert_eq!(
            store
                .find_application_by_name("teste")
                .await
                .unwrap()
                .unwrap()
                .id,
            second.id
        );
        let unicode =
            prepare_deploy_from_image(&store, "数据库", "nginx", DeployOptions::default())
                .await
                .unwrap()
                .record;
        assert_eq!(unicode.name, "数据库");
        assert_eq!(unicode.hostname, format!("{}.home.lan", unicode.id));
    }

    #[tokio::test]
    async fn automatic_hostnames_avoid_dns_records_and_reserved_names_without_changing_explicit_names()
     {
        let store = initialized_store().await;
        crate::collection::RECORDS
            .upsert(
                &store,
                &crate::dns_records::Record {
                    name: "office".into(),
                    record_type: crate::dns_records::RecordType::A,
                    value: "192.0.2.5".parse().unwrap(),
                    ttl: 60,
                    description: None,
                    owner: crate::dns_records::Owner::Operator,
                },
            )
            .await
            .unwrap();
        for name in ["Office", "Admin"] {
            let app = prepare_deploy_from_image(&store, name, "nginx", DeployOptions::default())
                .await
                .unwrap()
                .record;
            assert_eq!(
                app.hostname,
                format!("{}-{}.home.lan", name.to_ascii_lowercase(), app.id)
            );
        }
        let explicit = prepare_deploy_from_image(
            &store,
            "Explicit Name",
            "nginx",
            DeployOptions {
                hostname: Some("office.home.lan".into()),
                ..Default::default()
            },
        )
        .await;
        assert!(matches!(explicit, Err(DeployError::InvalidHostname(_))));
        let unique = prepare_deploy_from_image(
            &store,
            "Separate Name",
            "nginx",
            DeployOptions {
                hostname: Some("chosen.home.lan".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .record;
        assert_eq!(unique.hostname, "chosen.home.lan");
    }

    #[tokio::test]
    async fn native_git_compose_and_path_sources_preserve_names_and_use_safe_runtime_identifiers() {
        let store = initialized_store().await;
        let native = prepare_deploy_native(
            &store,
            "T3 Code",
            DeployOptions {
                runtime: Some(Runtime::Native(
                    serde_json::from_value(serde_json::json!({"command":["/bin/sleep","1"]}))
                        .unwrap(),
                )),
                publication: Some(Publication::Unpublished),
                ..Default::default()
            },
            Vec::new(),
        )
        .await
        .unwrap()
        .record;
        assert_eq!(native.name, "T3 Code");
        let Runtime::Native(definition) = native.runtime else {
            unreachable!()
        };
        assert_eq!(definition.account, format!("sf-app-{}", native.id));

        let git = prepare_git_create(
            &store,
            "Git / Production",
            serde_json::from_value(
                serde_json::json!({"repository":"https://example.invalid/repo.git"}),
            )
            .unwrap(),
            None,
            None,
            DeployOptions::default(),
        )
        .await
        .unwrap()
        .record;
        assert_eq!(git.name, "Git / Production");
        assert_eq!(git.hostname, "git-production.home.lan");

        let compose = prepare_deploy_from_compose(
            &store,
            "My $API / Service",
            "services:\n  web:\n    image: nginx\n    ports: ['80']\n",
            None,
            None,
            DeployOptions::default(),
        )
        .await
        .unwrap()
        .record;
        let project = project_for(&store, &compose).await.unwrap();
        assert_eq!(project.name, format!("sf-app-{}", compose.id));
        assert!(project.dir.ends_with(&compose.id));
        let yaml: serde_yaml::Value = serde_yaml::from_str(&project.yaml).unwrap();
        assert_eq!(
            yaml["services"]["web"]["labels"]["sf.app.name"],
            "My $$API / Service"
        );

        let path = prepare_deploy_from_path(
            &store,
            "Local Build #1",
            "/synthetic/build",
            DeployOptions::default(),
        )
        .await
        .unwrap()
        .record;
        assert_eq!(path.name, "Local Build #1");
        assert_eq!(path.image, format!("self-host-{}:latest", path.id));
        let mut legacy = path.clone();
        legacy.name = "legacy".into();
        legacy.image = "self-host-legacy:latest".into();
        store.insert_application(&legacy).await.unwrap();
        let legacy_again = prepare_deploy_from_path(
            &store,
            "legacy",
            "/synthetic/build",
            DeployOptions::default(),
        )
        .await
        .unwrap()
        .record;
        assert_eq!(legacy_again.id, legacy.id);
        assert_eq!(legacy_again.image, legacy.image);
        assert_eq!(legacy_again.hostname, legacy.hostname);
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

        let app = deploy_from_image(
            &store,
            &docker,
            &routes,
            "blog",
            "nginx:alpine",
            DeployOptions::default(),
        )
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

        let app = deploy_from_image(
            &store,
            &docker,
            &routes,
            "blog",
            "nginx:alpine",
            DeployOptions::default(),
        )
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

        let app = deploy_from_image(
            &store,
            &docker,
            &routes,
            "blog",
            "nginx:alpine",
            DeployOptions::default(),
        )
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

        deploy_from_image(
            &store,
            &docker,
            &routes,
            "blog",
            "nginx:alpine",
            DeployOptions::default(),
        )
        .await
        .unwrap();
        let shop = deploy_from_image(
            &store,
            &docker,
            &routes,
            "shop",
            "nginx:alpine",
            DeployOptions::default(),
        )
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

        assert!(matches!(
            clash,
            Err(DeployError::Route(crate::routes::RouteError::Conflict(_)))
        ));
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
        let pending = prepare_deploy_from_compose(
            store,
            "hermes",
            HERMES,
            None,
            Some(9119),
            DeployOptions::default(),
        )
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
            prepare_deploy_from_development(
                &store,
                "t3",
                settings.clone(),
                DeployOptions::default(),
            )
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
            prepare_deploy_from_development(&store, "t3", settings, DeployOptions::default())
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

        let pending = prepare_deploy_from_compose(
            &store,
            "hermes",
            HERMES,
            Some(""),
            Some(9119),
            DeployOptions::default(),
        )
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
            DeployOptions::default(),
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

        let pending = prepare_deploy_from_compose(
            &store,
            "hermes",
            HERMES,
            None,
            Some(9119),
            DeployOptions::default(),
        )
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
    async fn a_restart_that_pulls_refreshes_registry_images_and_recreates_the_project() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let compose = format!(
            "{HERMES}  tools:\n    image: sf-img-tools:abc\n  worker:\n    image: nousresearch/hermes-agent:latest\n"
        );
        let pending = prepare_deploy_from_compose(
            &store,
            "hermes",
            &compose,
            None,
            Some(9119),
            DeployOptions::default(),
        )
        .await
        .unwrap();
        let app = finish_deploy(&store, &docker, &routes, pending)
            .await
            .unwrap();
        let hermes = "nousresearch/hermes-agent:latest";
        docker
            .images
            .lock()
            .unwrap()
            .insert(hermes.into(), format!("sha256:{}", "a".repeat(64)));
        docker
            .registry
            .lock()
            .unwrap()
            .insert(hermes.into(), format!("sha256:{}", "b".repeat(64)));

        let (restarted, changes) = restart_application(&store, &docker, &routes, &app.id, true)
            .await
            .unwrap();

        assert_eq!(restarted.status, STATUS_RUNNING);
        // Once per image, and never an image the Host built itself.
        assert_eq!(docker.pulled.lock().unwrap().as_slice(), [hermes]);
        assert_eq!(
            changes,
            [crate::audit::Change {
                setting: hermes.into(),
                from: "sha256:aaaaaaaaaaaa".into(),
                to: "sha256:bbbbbbbbbbbb".into(),
            }]
        );
        assert_eq!(
            docker.recreated.lock().unwrap().as_slice(),
            [project_name_for(&app.id)]
        );
    }

    #[tokio::test]
    async fn a_redeploy_that_pulls_refreshes_the_images_even_with_nothing_edited() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let app = deploy_hermes(&store, &docker, &routes).await;
        let hermes = "nousresearch/hermes-agent:latest";
        docker
            .registry
            .lock()
            .unwrap()
            .insert(hermes.into(), format!("sha256:{}", "d".repeat(64)));

        // Nothing edited and no pull: only the route, as before.
        let pending = prepare_update(&store, &app.id, ApplicationUpdate::default())
            .await
            .unwrap();
        assert!(pending.is_settled());

        let pending = prepare_update(
            &store,
            &app.id,
            ApplicationUpdate {
                pull: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(!pending.is_settled());
        let (redeployed, changes) = finish_deploy_reporting(&store, &docker, &routes, pending)
            .await
            .unwrap();

        assert_eq!(redeployed.status, STATUS_RUNNING);
        assert_eq!(docker.pulled.lock().unwrap().as_slice(), [hermes]);
        assert_eq!(changes[0].to, "sha256:dddddddddddd");
    }

    #[tokio::test]
    async fn a_restart_without_pull_restarts_what_is_there() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let app = deploy_hermes(&store, &docker, &routes).await;

        let (_, changes) = restart_application(&store, &docker, &routes, &app.id, false)
            .await
            .unwrap();

        assert!(changes.is_empty());
        assert!(docker.pulled.lock().unwrap().is_empty());
        assert!(docker.recreated.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_pull_that_fails_leaves_the_application_running_as_it_was() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let app = deploy_hermes(&store, &docker, &routes).await;
        let docker = FakeDocker {
            pull_failure: Some("Error response from daemon: toomanyrequests".into()),
            ..docker
        };

        let err = restart_application(&store, &docker, &routes, &app.id, true)
            .await
            .unwrap_err();

        assert!(
            ErrorReport::new(&err)
                .caused_by
                .iter()
                .any(|c| c.contains("toomanyrequests"))
        );
        assert!(docker.recreated.lock().unwrap().is_empty());
        let saved = store.get_application(&app.id).await.unwrap().unwrap();
        assert_eq!(saved.status, STATUS_RUNNING);
        assert!(saved.last_error.is_none());
    }

    #[tokio::test]
    async fn a_restart_that_pulls_replaces_a_single_container() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let pending =
            prepare_deploy_from_image(&store, "blog", "nginx:alpine", DeployOptions::default())
                .await
                .unwrap();
        let app = finish_deploy(&store, &docker, &routes, pending)
            .await
            .unwrap();
        docker
            .registry
            .lock()
            .unwrap()
            .insert("nginx:alpine".into(), format!("sha256:{}", "c".repeat(64)));

        let (restarted, changes) = restart_application(&store, &docker, &routes, &app.id, true)
            .await
            .unwrap();

        assert_eq!(restarted.status, STATUS_RUNNING);
        assert_eq!(changes[0].from, "none");
        assert_eq!(changes[0].to, "sha256:cccccccccccc");
        assert_eq!(docker.deployed_apps(), [container_name_for(&app.id)]);
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
            "HERMES_DASHBOARD=1\n      - HERMES_DASHBOARD_BASIC_AUTH_USERNAME=operator",
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
                .contains("HERMES_DASHBOARD_BASIC_AUTH_USERNAME: operator")
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

    /// A new Application gets its Variables by reference (ADR-0030): one the
    /// file never names stays out of every service.
    #[tokio::test]
    async fn a_variable_the_file_never_names_stays_out_of_a_referenced_project() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let app = deploy_hermes(&store, &docker, &routes).await;
        assert_eq!(app.variable_delivery, VariableDelivery::Referenced);

        set_env(&store, &docker, "hermes", "OPENROUTER_API_KEY", "sk-test")
            .await
            .unwrap();

        let project = docker.project(&format!("sf-app-{}", app.id)).unwrap();
        assert!(
            !project.yaml.contains("OPENROUTER_API_KEY"),
            "{}",
            project.yaml
        );
        assert!(
            project.yaml.contains("HERMES_DASHBOARD: '1'"),
            "{}",
            project.yaml
        );
    }

    #[tokio::test]
    async fn a_variable_the_file_references_reaches_only_that_service() {
        const TWO_SERVICES: &str = "services:\n  web:\n    image: nginx\n    ports:\n      - \"8080:80\"\n    environment:\n      - OPENROUTER_API_KEY\n      - MODEL=${MODEL:-default-model}\n  side:\n    image: alpine\n";
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let pending = prepare_deploy_from_compose(
            &store,
            "vars",
            TWO_SERVICES,
            None,
            Some(80),
            DeployOptions::default(),
        )
        .await
        .unwrap();
        let app = finish_deploy(&store, &docker, &routes, pending)
            .await
            .unwrap();

        set_env(&store, &docker, "vars", "OPENROUTER_API_KEY", "sk-test")
            .await
            .unwrap();
        set_env(&store, &docker, "vars", "MODEL", "a$b")
            .await
            .unwrap();

        let project = docker.project(&format!("sf-app-{}", app.id)).unwrap();
        let doc: serde_yaml::Value = serde_yaml::from_str(&project.yaml).unwrap();
        assert_eq!(
            doc["services"]["web"]["environment"]["OPENROUTER_API_KEY"],
            "sk-test"
        );
        // Resolved by the Platform, and written so `docker compose` leaves
        // the dollar alone.
        assert_eq!(doc["services"]["web"]["environment"]["MODEL"], "a$$b");
        assert!(
            doc["services"]["side"]["environment"].is_null(),
            "{}",
            project.yaml
        );
    }

    /// An Application kept on broadcast delivery still gets every Variable
    /// on every service, the way it did before ADR-0030.
    #[tokio::test]
    async fn an_application_kept_on_broadcast_still_gets_every_variable() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let pending = prepare_deploy_from_compose(
            &store,
            "hermes",
            HERMES,
            None,
            Some(9119),
            DeployOptions {
                variable_delivery: Some(VariableDelivery::Broadcast),
                ..DeployOptions::default()
            },
        )
        .await
        .unwrap();
        let app = finish_deploy(&store, &docker, &routes, pending)
            .await
            .unwrap();

        set_env(&store, &docker, "hermes", "OPENROUTER_API_KEY", "sk-test")
            .await
            .unwrap();

        let project = docker.project(&format!("sf-app-{}", app.id)).unwrap();
        assert!(project.yaml.contains("OPENROUTER_API_KEY: sk-test"));
    }

    /// `${VAR:?}` is checked when the project is about to run, not when the
    /// file is saved, so the Operator can save first and set the Variable
    /// after. Until then the deploy fails with the Variable's name.
    #[tokio::test]
    async fn a_required_variable_is_checked_when_the_project_runs_not_when_the_file_is_saved() {
        const REQUIRED: &str = "services:\n  web:\n    image: nginx\n    environment:\n      TOKEN: ${TOKEN:?set TOKEN first}\n";
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();

        let pending = prepare_deploy_from_compose(
            &store,
            "gate",
            REQUIRED,
            None,
            Some(80),
            DeployOptions::default(),
        )
        .await
        .expect("saving the file does not need the Variable");
        let id = pending.record.id.clone();

        let error = finish_deploy(&store, &docker, &routes, pending)
            .await
            .unwrap_err();
        let report = ErrorReport::new(&error);
        assert!(
            report.caused_by.iter().any(|c| c.contains("TOKEN")),
            "{report}"
        );
        let failed = store.get_application(&id).await.unwrap().unwrap();
        assert_eq!(failed.status, STATUS_FAILED);
        assert!(docker.project(&format!("sf-app-{id}")).is_none());

        // The same refusal, with its cause, when a Variable change tries to
        // render the project while another required one is still missing.
        let error = set_env(&store, &docker, "gate", "OTHER", "x")
            .await
            .unwrap_err();
        assert!(
            ErrorReport::new(&error)
                .caused_by
                .iter()
                .any(|c| c.contains("TOKEN")),
            "{}",
            ErrorReport::new(&error)
        );

        set_env(&store, &docker, "gate", "TOKEN", "change-me")
            .await
            .unwrap();
        let project = docker.project(&format!("sf-app-{id}")).unwrap();
        assert!(
            project.yaml.contains("TOKEN: change-me"),
            "{}",
            project.yaml
        );
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
    fn validate_app_name_preserves_human_names() {
        for name in [
            "Blog",
            "T3 Code",
            "Produção / API #1",
            "数据库",
            "  Name  ",
            ".",
            "..",
        ] {
            assert!(validate_app_name(name).is_ok(), "{name:?}");
        }
        assert!(validate_app_name(&"界".repeat(63)).is_ok());
        assert!(validate_app_name(&"界".repeat(64)).is_err());
        for name in ["", " \u{2003} ", "line\nbreak", "tab\tname", "null\0name"] {
            assert!(validate_app_name(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn validate_app_name_accepts_simple_name() {
        assert!(validate_app_name("blog").is_ok());
    }

    /// A Compose file with nothing to route to: a worker.
    const WORKER: &str = "services:\n  worker:\n    image: alpine\n    command: sleep infinity\n";

    fn unpublished() -> DeployOptions {
        DeployOptions {
            publication: Some(Publication::Unpublished),
            ..Default::default()
        }
    }

    fn native() -> Runtime {
        Runtime::Native(crate::store::NativeDefinition {
            account: "sf-app-api".into(),
            command: vec!["/opt/api/bin/serve".into()],
            working_dir: None,
            port: Some(8080),
            limits: Default::default(),
            recipe: Default::default(),
        })
    }

    #[tokio::test]
    async fn an_unpublished_compose_deploy_has_no_hostname_no_port_and_no_route() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        // The Operator already answers on the name the Application would
        // have taken. An unpublished one takes nothing from the Zone.
        crate::collection::RECORDS
            .upsert(
                &store,
                &crate::dns_records::Record {
                    name: "worker".into(),
                    record_type: crate::dns_records::RecordType::A,
                    value: std::net::Ipv4Addr::new(192, 0, 2, 30),
                    ttl: crate::dns_records::TTL,
                    description: None,
                    owner: crate::dns_records::Owner::Operator,
                },
            )
            .await
            .unwrap();

        let pending =
            prepare_deploy_from_compose(&store, "worker", WORKER, None, None, unpublished())
                .await
                .unwrap();
        assert_eq!(pending.record.publication, Publication::Unpublished);
        assert_eq!(pending.record.hostname, "");
        assert!(pending.record.aliases.is_empty());
        assert_eq!(pending.record.web_service, None);
        assert_eq!(pending.record.web_port, None);
        assert_eq!(pending.record.web_target_port, None);
        assert_eq!(pending.record.image, "alpine");
        assert_eq!(pending.record.runtime, Runtime::Container);
        assert_eq!(
            pending.record.variable_delivery,
            VariableDelivery::Referenced
        );

        let app = finish_deploy(&store, &docker, &routes, pending)
            .await
            .unwrap();
        assert_eq!(app.status, STATUS_RUNNING);
        assert_eq!(
            routes.get(&app.id),
            None,
            "nothing routes to an unpublished Application"
        );
        let project = docker.project(&project_name_for(&app.id)).unwrap();
        assert!(
            !project.yaml.contains("127.0.0.1:"),
            "no loopback publication in the project:\n{}",
            project.yaml
        );

        // A restart neither gives it a port nor a route.
        reconcile(&store, &docker, &routes).await.unwrap();
        let after = store.get_application(&app.id).await.unwrap().unwrap();
        assert_eq!(after.status, STATUS_RUNNING);
        assert_eq!(after.web_target_port, None);
        assert_eq!(routes.get(&app.id), None);
    }

    #[tokio::test]
    async fn an_unpublished_image_deploy_publishes_no_port_and_no_route() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();

        let app = deploy_from_image(&store, &docker, &routes, "worker", "alpine", unpublished())
            .await
            .unwrap();

        assert_eq!(app.status, STATUS_RUNNING);
        assert_eq!(app.hostname, "");
        assert_eq!(app.web_target_port, None);
        assert_eq!(routes.get(&app.id), None);
        let containers = docker.apps.lock().unwrap();
        assert!(containers[0].ports.is_empty(), "{:?}", containers[0].ports);
    }

    #[tokio::test]
    async fn an_unpublished_application_refuses_a_hostname_an_alias_or_a_web_target() {
        let store = initialized_store().await;
        let expected = "invalid Application Hostname: an unpublished Application has no Hostname";

        let with_hostname = DeployOptions {
            hostname: Some("worker.home.lan".into()),
            ..unpublished()
        };
        let err = prepare_deploy_from_image(&store, "worker", "alpine", with_hostname)
            .await
            .unwrap_err();
        assert!(matches!(err, DeployError::InvalidHostname(_)), "{err}");
        assert_eq!(err.to_string(), expected);

        let with_alias = DeployOptions {
            aliases: Some(vec!["jobs.home.lan".into()]),
            ..unpublished()
        };
        let err = prepare_deploy_from_image(&store, "worker", "alpine", with_alias)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), expected);

        let err = prepare_deploy_from_compose(
            &store,
            "worker",
            WORKER,
            Some("worker"),
            None,
            unpublished(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.to_string(), expected);

        let err =
            prepare_deploy_from_compose(&store, "worker", WORKER, None, Some(8080), unpublished())
                .await
                .unwrap_err();
        assert_eq!(err.to_string(), expected);

        assert!(store.list_applications().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_native_runtime_is_refused_before_anything_is_recorded() {
        let store = initialized_store().await;
        let options = DeployOptions {
            runtime: Some(native()),
            ..Default::default()
        };

        let err = prepare_deploy_from_image(&store, "api", "alpine", options)
            .await
            .unwrap_err();

        assert!(matches!(err, DeployError::NativeUnavailable), "{err}");
        assert_eq!(
            err.to_string(),
            "native execution is not available yet; the Application runtime must be container"
        );
        assert!(store.list_applications().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn publication_is_fixed_at_creation_and_the_runtime_stays_container() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let app = deploy_from_image(
            &store,
            &docker,
            &routes,
            "blog",
            "nginx:alpine",
            DeployOptions::default(),
        )
        .await
        .unwrap();

        let err = prepare_update(
            &store,
            &app.id,
            ApplicationUpdate {
                publication: Some(Publication::Unpublished),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DeployError::PublicationFixed), "{err}");
        assert_eq!(
            err.to_string(),
            "publication cannot be changed after creation yet"
        );

        // A deploy under the same name is a redeploy, and keeps the row's.
        let err = prepare_deploy_from_image(&store, "blog", "nginx:alpine", unpublished())
            .await
            .unwrap_err();
        assert!(matches!(err, DeployError::PublicationFixed), "{err}");

        // Saying the same thing again changes nothing.
        let pending = prepare_update(
            &store,
            &app.id,
            ApplicationUpdate {
                publication: Some(Publication::Web),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(pending.is_settled());
        assert_eq!(pending.record.publication, Publication::Web);

        let err = prepare_update(
            &store,
            &app.id,
            ApplicationUpdate {
                runtime: Some(native()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DeployError::InvalidNative(_)), "{err}");
        let saved = store.get_application(&app.id).await.unwrap().unwrap();
        assert_eq!(saved.runtime, Runtime::Container);
        assert_eq!(saved.publication, Publication::Web);
        assert_eq!(saved.status, STATUS_RUNNING);
    }

    #[tokio::test]
    async fn changing_variable_delivery_on_a_compose_application_reaches_docker() {
        let store = initialized_store().await;
        let docker = FakeDocker::new();
        let routes = FakeRoutes::new();
        let app = deploy_hermes(&store, &docker, &routes).await;
        assert_eq!(app.variable_delivery, VariableDelivery::Referenced);

        let pending = prepare_update(
            &store,
            &app.id,
            ApplicationUpdate {
                variable_delivery: Some(VariableDelivery::Broadcast),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(
            !pending.is_settled(),
            "the rendered project changes with the delivery"
        );
        assert_eq!(
            pending.record.variable_delivery,
            VariableDelivery::Broadcast
        );
        let switched = finish_deploy(&store, &docker, &routes, pending)
            .await
            .unwrap();
        assert_eq!(switched.variable_delivery, VariableDelivery::Broadcast);

        // Saying what the row already says is a route rewrite at most.
        let same = prepare_update(
            &store,
            &app.id,
            ApplicationUpdate {
                variable_delivery: Some(VariableDelivery::Broadcast),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(same.is_settled());
    }

    #[tokio::test]
    async fn reviewed_git_updates_reject_stale_previews_before_checkout() {
        let store = initialized_store().await;
        let source: crate::source::GitSource = serde_json::from_value(serde_json::json!({
            "repository": "https://example.invalid/fixture.git", "git_ref": "main"
        }))
        .unwrap();
        let pending = prepare_git_create(
            &store,
            "reviewed-worker",
            source.clone(),
            None,
            None,
            DeployOptions {
                publication: Some(Publication::Unpublished),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let first = "1".repeat(40);
        let second = "2".repeat(40);
        let third = "3".repeat(40);
        let mut current = pending.record;
        current.status = STATUS_RUNNING.into();
        current.git_build = Some(crate::source::GitBuild {
            revision: first.clone(),
            images: Default::default(),
            status: "completed".into(),
        });
        store.insert_application(&current).await.unwrap();
        let candidate = prepare_git_update_reviewed(
            &store,
            current.clone(),
            source.clone(),
            true,
            ApplicationUpdate::default(),
            Some(second.clone()),
            Some(first.clone()),
        )
        .await
        .unwrap();
        assert!(
            candidate.source.revision.is_none(),
            "a reviewed commit is transient"
        );
        assert_eq!(candidate.source_revision.as_deref(), Some(second.as_str()));
        let mut old_task = serde_json::to_value(&candidate).unwrap();
        old_task.as_object_mut().unwrap().remove("source_revision");
        old_task
            .as_object_mut()
            .unwrap()
            .remove("expected_git_revision");
        let recovered: PendingGitDeploy = serde_json::from_value(old_task).unwrap();
        assert!(recovered.source_revision.is_none());
        assert!(recovered.expected_git_revision.is_none());
        let mut advanced = current.clone();
        advanced.git_build.as_mut().unwrap().revision = third.clone();
        store.insert_application(&advanced).await.unwrap();
        assert!(
            prepare_git_update_reviewed(
                &store,
                current,
                source,
                true,
                ApplicationUpdate::default(),
                Some(second),
                Some(first)
            )
            .await
            .is_err()
        );
        let docker = FakeDocker::new();
        assert!(
            finish_git_deploy(&store, &docker, &FakeRoutes::new(), candidate)
                .await
                .is_err()
        );
        assert!(docker.built.lock().unwrap().is_empty());
        assert_eq!(
            store.get_application(&advanced.id).await.unwrap().unwrap(),
            advanced
        );
    }

    #[tokio::test]
    async fn reviewed_git_create_rejects_conflicting_explicit_pins_without_recording() {
        let store = initialized_store().await;
        let source: crate::source::GitSource = serde_json::from_value(serde_json::json!({
            "repository": "https://example.invalid/fixture.git", "revision": "1".repeat(40)
        }))
        .unwrap();
        assert!(
            prepare_git_create_reviewed(
                &store,
                "conflicting-pin",
                source,
                None,
                None,
                DeployOptions {
                    publication: Some(Publication::Unpublished),
                    ..Default::default()
                },
                Some("2".repeat(40))
            )
            .await
            .is_err()
        );
        assert!(store.list_applications().await.unwrap().is_empty());
    }
}
