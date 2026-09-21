//! Native development environments and their durable lifecycle state.

use async_trait::async_trait;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::net::Ipv4Addr;
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};

use sha2::{Digest, Sha256};

use crate::{
    AppState,
    custom_images::Recipe,
    error::ErrorReport,
    store::{StateStore, StoreError},
};

pub mod events;

const STATE_KEY: &str = "environments_v1";
const MAX_LOG_BYTES: usize = 64 * 1024;
/// How much of the event log one request returns. Enough to cover a whole
/// bootstrap, short of asking the browser to render a machine's whole history.
const EVENTS_TAIL_BYTES: u64 = 256 * 1024;
const INSPECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The persisted environment configuration. Resource changes are deliberately
/// rejected; each record retains the resources selected at creation.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct VmConfig {
    pub name: String,
    pub cpus: u16,
    pub memory_gib: u16,
    pub disk_gib: u16,
    pub ssh_public_key: String,
    pub recipe: Recipe,
    pub command: String,
    pub web_port: u16,
}

impl VmConfig {
    fn validate(&mut self) -> Result<(), String> {
        self.name = self.name.trim().to_owned();
        self.ssh_public_key = self.ssh_public_key.trim().to_owned();
        self.command = self.command.trim().to_owned();
        if self.name.is_empty() || self.name.chars().count() > 128 {
            return Err("Use an environment name of 1 to 128 characters.".into());
        }
        if self.cpus == 0 || self.cpus > 128 {
            return Err("CPUs must be between 1 and 128.".into());
        }
        if self.memory_gib == 0 || self.disk_gib == 0 {
            return Err("Memory and disk must be greater than zero.".into());
        }
        // A key is only needed to reach the machine from outside the console,
        // which opens its own terminal. Absent is a choice, not an omission.
        if self.ssh_public_key.len() > 16 * 1024 {
            return Err("That SSH public key is too long.".into());
        }
        if self.command.len() > 4096 {
            return Err("A service command must be under 4096 bytes.".into());
        }
        // A machine with no service needs no port. One with a service does.
        if !self.command.is_empty() && self.web_port == 0 {
            return Err("Enter the port the service listens on.".into());
        }
        // The recipe is a custom image's shape, and an image must be named and
        // must install something. A machine's recipe is neither: it is not
        // named separately, and a plain Ubuntu with no tools is a workspace an
        // Operator may well have asked for. An empty recipe is no recipe, so
        // there is nothing to check.
        if self.recipe.name.trim().is_empty() {
            self.recipe.name = self.name.clone();
        }
        if !self.recipe.dependencies.is_empty()
            || !self.recipe.setup.is_empty()
            || !self.recipe.build_checks.is_empty()
            || self.recipe.template_id.is_some()
        {
            self.recipe.validate_machine()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum VmState {
    Missing,
    Stopped,
    Running,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OperationStatus {
    Running,
    Succeeded,
    Failed,
    Interrupted,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Operation {
    pub action: String,
    pub status: OperationStatus,
    #[serde(default)]
    pub step: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorReport>,
}

/// The API and state representation of an environment. `config` is the
/// desired draft and `applied_config` records the last successful update.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EnvironmentRecord {
    pub id: String,
    pub config: VmConfig,
    /// The name the machine answers on, `<name>.<suffix>`, kept here rather
    /// than in the configuration so it never recreates the machine
    /// (ADR-0025). Empty only for a machine from before names that could not
    /// be given one on load: no DNS Suffix yet, or a name no DNS name can be
    /// made from.
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub applied_config: Option<VmConfig>,
    pub state: VmState,
    pub service_ready: bool,
    #[serde(default)]
    pub operation: Option<Operation>,
    #[serde(default)]
    pub log: String,
    #[serde(default)]
    pub ssh_command: Option<String>,
    #[serde(default)]
    pub tunnel_command: Option<String>,
    #[serde(default)]
    pub web_url: Option<String>,
    #[serde(default)]
    pub base_image: Option<String>,
    #[serde(default)]
    pub installed_versions: Option<Value>,
    #[serde(default)]
    pub mac_address: Option<String>,
    #[serde(default)]
    pub lan_address: Option<String>,
}

impl EnvironmentRecord {
    /// The lease the machine holds on the LAN, as the Zone answers with it.
    pub fn lease(&self) -> Option<Ipv4Addr> {
        self.lan_address.as_deref()?.parse().ok()
    }

    /// How the Operator reaches the machine: by its name, straight to its LAN
    /// address (ADR-0025). The runtime reports loopback forwards, which say
    /// the machine is there to be reached; the name is what gets copied. A
    /// machine without a name keeps the forwards.
    fn reach_by_name(&mut self, ssh_command: Option<String>, web_url: Option<String>) {
        if self.hostname.is_empty() {
            self.ssh_command = ssh_command;
            self.web_url = web_url;
            return;
        }
        self.ssh_command = ssh_command.map(|_| format!("ssh dev@{}", self.hostname));
        let web_port = self
            .applied_config
            .as_ref()
            .unwrap_or(&self.config)
            .web_port;
        self.web_url = web_url.map(|_| format!("http://{}:{web_port}", self.hostname));
    }
}

/// A machine's name under the DNS Suffix, or why it cannot be one.
fn default_hostname(name: &str, suffix: &str) -> Result<String, String> {
    let name = crate::dns_records::parse_name(name, suffix)?;
    Ok(format!("{name}.{suffix}"))
}

#[derive(Clone, Debug, Deserialize)]
pub struct CreateRequest {
    pub request_id: String,
    pub config: VmConfig,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ActionRequest {
    pub action: String,
    #[serde(default)]
    pub confirm_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UpdateRequest {
    #[serde(flatten)]
    pub config: VmConfig,
}

/// A runner returns an observation after each operation. It owns all runner
/// commands. The API layer owns serialization and operation state only.
#[derive(Clone, Debug, Default)]
pub struct RunnerObservation {
    pub state: VmState,
    pub service_ready: bool,
    pub step: Option<String>,
    pub log: String,
    pub ssh_command: Option<String>,
    pub tunnel_command: Option<String>,
    pub web_url: Option<String>,
    pub base_image: Option<String>,
    pub installed_versions: Option<Value>,
    pub mac_address: Option<String>,
    pub lan_address: Option<String>,
}

/// Which of the machine's own logs to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogSource {
    /// The kernel and systemd, from this boot. What a virtual machine prints
    /// on its console, which is what the Operator asked to see.
    Boot,
    /// What cloud-init ran to turn a stock image into this workspace.
    Provisioning,
}

impl LogSource {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "boot" => Some(Self::Boot),
            "provisioning" => Some(Self::Provisioning),
            _ => None,
        }
    }
}

/// One reading of a machine's own resource use, in the same terms the
/// Applications are measured in: percent of a core, bytes, and counters that
/// only ever climb while the machine is up.
#[derive(Clone, Copy, Debug, Default)]
pub struct MachineUsage {
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub memory_limit_bytes: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

#[derive(Clone, Debug, Default)]
pub struct RuntimeProgress {
    pub step: Option<String>,
    /// The step summary the list screens read. Kept short: it lives in
    /// Platform State and every line here rewrites the store.
    pub log: String,
    /// Whatever the guest printed. It goes to the event log, not the record.
    pub output: String,
}

#[async_trait]
pub trait VmRuntime: Send + Sync + 'static {
    async fn inspect(&self, id: &str) -> Result<RunnerObservation, String>;
    async fn execute(
        &self,
        id: &str,
        action: &str,
        config: &VmConfig,
    ) -> Result<RunnerObservation, String>;

    async fn execute_with_progress(
        &self,
        id: &str,
        action: &str,
        config: &VmConfig,
        _progress: mpsc::UnboundedSender<RuntimeProgress>,
    ) -> Result<RunnerObservation, String> {
        self.execute(id, action, config).await
    }

    /// A log the machine itself keeps: `boot` is the kernel and systemd
    /// coming up, `provisioning` is what cloud-init ran. Neither exists until
    /// the machine does.
    async fn logs(&self, _id: &str, _source: LogSource) -> Result<String, String> {
        Err("This runtime keeps no logs.".into())
    }

    /// What the machine is using right now, read from inside it. A runtime
    /// that cannot measure says so rather than reporting zeroes.
    async fn sample(&self, _id: &str) -> Result<MachineUsage, String> {
        Err("This runtime cannot measure a machine.".into())
    }

    /// An interactive shell inside the machine, as the Operator's `dev` user.
    /// A runtime that cannot offer one says so rather than pretending.
    async fn open_terminal(
        &self,
        _id: &str,
        _size: crate::terminal::Size,
    ) -> Result<crate::terminal::Session, String> {
        Err("This runtime cannot open a terminal.".into())
    }
}

/// `Ok` when the machine exists and is running, which is what a terminal
/// needs before anything is spawned against it.
pub(crate) async fn running<S: StateStore>(state: &AppState<S>, id: &str) -> Result<(), String> {
    let records = state.environments.records(&state.store).await?;
    let record = records
        .as_ref()
        .unwrap()
        .iter()
        .find(|record| record.id == id)
        .ok_or("Virtual machine not found.")?;
    if record.state != VmState::Running {
        return Err("The virtual machine is not running.".into());
    }
    Ok(())
}

#[derive(Default)]
pub(crate) struct Environments {
    records: Mutex<Option<Vec<EnvironmentRecord>>>,
}

impl Environments {
    async fn records<S: StateStore>(
        &self,
        store: &S,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<Vec<EnvironmentRecord>>>, String> {
        let mut records = self.records.lock().await;
        if records.is_none() {
            let mut loaded: Vec<EnvironmentRecord> = match store.get_state(STATE_KEY).await {
                Ok(Some(json)) => serde_json::from_str(&json).map_err(|e| e.to_string())?,
                Ok(None) => Vec::new(),
                Err(error) => return Err(error.to_string()),
            };
            let mut dirty = false;
            // A machine from before names gets its own, so the Operator does
            // not recreate it to reach it by name. Claimed like a new one: a
            // name someone already answers on stays with them.
            if let Some(suffix) = store
                .get_state("dns_suffix")
                .await
                .map_err(|e| e.to_string())?
            {
                for index in 0..loaded.len() {
                    if !loaded[index].hostname.is_empty() {
                        continue;
                    }
                    let Ok(hostname) = default_hostname(&loaded[index].config.name, &suffix) else {
                        continue;
                    };
                    match holder(store, &loaded, &hostname, &suffix).await? {
                        None => {
                            loaded[index].hostname = hostname;
                            dirty = true;
                        }
                        Some(holder) => tracing::warn!(
                            machine = %loaded[index].config.name,
                            "not named '{hostname}': already answered by {holder}"
                        ),
                    }
                }
            }
            for record in &mut loaded {
                let old_log_len = record.log.len();
                append_log(&mut record.log, "");
                dirty |= old_log_len != record.log.len();
                if record
                    .operation
                    .as_ref()
                    .is_some_and(|op| op.status == OperationStatus::Running)
                {
                    if let Some(operation) = record.operation.as_mut() {
                        operation.status = OperationStatus::Interrupted;
                        operation.step = None;
                        operation.error = Some(ErrorReport::plain(
                            "The Platform restarted before this operation finished.",
                        ));
                    }
                    dirty = true;
                }
            }
            if dirty {
                save(store, &loaded).await?;
            }
            *records = Some(loaded);
        }
        Ok(records)
    }
}

/// A runner with no machines behind it, for the tests and the local harness
/// that have no hypervisor. It follows [`FakeDocker`](crate::docker::FakeDocker).
#[derive(Default)]
pub struct FakeVmRuntime;

#[async_trait]
impl VmRuntime for FakeVmRuntime {
    async fn inspect(&self, _id: &str) -> Result<RunnerObservation, String> {
        Ok(RunnerObservation {
            state: VmState::Missing,
            ..Default::default()
        })
    }

    async fn execute(
        &self,
        _id: &str,
        _action: &str,
        _config: &VmConfig,
    ) -> Result<RunnerObservation, String> {
        Err("No hypervisor is configured on this Host.".into())
    }
}

/// Every machine, read straight from the store. For the Records and the
/// Applications, which check names against machines while holding their own
/// lock, and for the start sequence, which runs before the API does.
pub async fn load<S: StateStore>(store: &S) -> Result<Vec<EnvironmentRecord>, StoreError> {
    match store.get_state(STATE_KEY).await? {
        None => Ok(Vec::new()),
        Some(json) => serde_json::from_str(&json)
            .map_err(|e| StoreError::Serialize(format!("could not read the machines: {e}"))),
    }
}

/// The machines the collector should measure: the ones Lima reports running.
/// Read straight from the store, because the collector runs beside the API
/// rather than inside a request.
pub async fn running_ids<S: StateStore>(store: &S) -> Vec<String> {
    let Ok(Some(raw)) = store.get_state(STATE_KEY).await else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<EnvironmentRecord>>(&raw)
        .unwrap_or_default()
        .into_iter()
        .filter(|record| record.state == VmState::Running)
        .map(|record| record.id)
        .collect()
}

async fn save<S: StateStore>(store: &S, records: &[EnvironmentRecord]) -> Result<(), String> {
    let json = serde_json::to_string(records).map_err(|e| e.to_string())?;
    store
        .store_state(STATE_KEY, &json)
        .await
        .map_err(|e| e.to_string())
}

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(ErrorReport::plain(message.into()))).into_response()
}

pub(crate) async fn list<S: StateStore>(State(state): State<AppState<S>>) -> Response {
    let snapshot = match state.environments.records(&state.store).await {
        Ok(records) => records.as_ref().unwrap().clone(),
        Err(message) => return error(StatusCode::INTERNAL_SERVER_ERROR, message),
    };
    for record in &snapshot {
        refresh(&state, &record.id).await;
    }
    match state.environments.records(&state.store).await {
        Ok(records) => Json(records.as_ref().unwrap().clone()).into_response(),
        Err(message) => error(StatusCode::INTERNAL_SERVER_ERROR, message),
    }
}

/// A log the machine keeps about itself. Unlike the event log, which is the
/// Platform's account, this is the machine's.
pub(crate) async fn machine_log<S: StateStore>(
    State(state): State<AppState<S>>,
    Path((id, source)): Path<(String, String)>,
) -> Response {
    let Some(source) = LogSource::parse(&source) else {
        return error(StatusCode::NOT_FOUND, "Unknown log.");
    };
    if let Err(message) = running(&state, &id).await {
        return error(StatusCode::CONFLICT, message);
    }
    match state.vm_runtime.logs(&id, source).await {
        Ok(text) => (
            StatusCode::OK,
            [(
                axum::http::header::CONTENT_TYPE,
                "text/plain; charset=utf-8",
            )],
            text,
        )
            .into_response(),
        Err(message) => error(StatusCode::BAD_GATEWAY, message),
    }
}

/// The guest's own output, newest last. The record carries the steps; this
/// carries what produced them, which is the only thing worth reading when a
/// bootstrap fails before SSH exists to go and look.
pub(crate) async fn events_log<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
) -> Response {
    let exists = match state.environments.records(&state.store).await {
        Ok(records) => records
            .as_ref()
            .unwrap()
            .iter()
            .any(|record| record.id == id),
        Err(message) => return error(StatusCode::INTERNAL_SERVER_ERROR, message),
    };
    if !exists {
        return error(StatusCode::NOT_FOUND, "Environment not found.");
    }
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        events::tail(&id, EVENTS_TAIL_BYTES).await,
    )
        .into_response()
}

pub(crate) async fn get<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
) -> Response {
    let exists = match state.environments.records(&state.store).await {
        Ok(records) => records
            .as_ref()
            .unwrap()
            .iter()
            .any(|record| record.id == id),
        Err(message) => return error(StatusCode::INTERNAL_SERVER_ERROR, message),
    };
    if !exists {
        return error(StatusCode::NOT_FOUND, "Environment not found.");
    }
    refresh(&state, &id).await;
    let records = match state.environments.records(&state.store).await {
        Ok(records) => records,
        Err(message) => return error(StatusCode::INTERNAL_SERVER_ERROR, message),
    };
    match records
        .as_ref()
        .unwrap()
        .iter()
        .find(|record| record.id == id)
    {
        Some(record) => Json(record.clone()).into_response(),
        None => error(StatusCode::NOT_FOUND, "Environment not found."),
    }
}

async fn refresh<S: StateStore>(state: &AppState<S>, id: &str) {
    let expected_operation = {
        let Ok(records) = state.environments.records(&state.store).await else {
            return;
        };
        let Some(record) = records
            .as_ref()
            .unwrap()
            .iter()
            .find(|record| record.id == id)
        else {
            return;
        };
        record.operation.clone()
    };
    let observation = tokio::time::timeout(INSPECT_TIMEOUT, state.vm_runtime.inspect(id)).await;
    let Ok(mut records) = state.environments.records(&state.store).await else {
        return;
    };
    let mut next = records.as_ref().unwrap().clone();
    let Some(record) = next.iter_mut().find(|record| record.id == id) else {
        return;
    };
    // An inspection runs outside the state lock. Do not let a stale result
    // overwrite a progress update or completed operation that raced it.
    if record.operation != expected_operation {
        return;
    }
    match observation {
        Ok(Ok(observation)) => {
            record.state = observation.state;
            record.service_ready = observation.service_ready;
            if !observation.log.is_empty() {
                append_log(&mut record.log, &observation.log);
            }
            record.tunnel_command = observation.tunnel_command;
            record.base_image = observation.base_image;
            record.installed_versions = observation.installed_versions;
            record.mac_address = observation.mac_address;
            record.lan_address = observation.lan_address;
            record.reach_by_name(observation.ssh_command, observation.web_url);
        }
        Ok(Err(_)) | Err(_) => {
            record.state = VmState::Unknown;
            record.service_ready = false;
        }
    }
    let (hostname, lease) = (record.hostname.clone(), record.lease());
    if save(&state.store, &next).await.is_err() {
        return;
    }
    *records = Some(next);
    // State first, Zone second, and both under the lock, so the Zone sees
    // saves in the order they happened.
    state
        .dns_records
        .follow_lease(&state.store, &hostname, lease)
        .await;
}

pub(crate) async fn create<S: StateStore>(
    State(state): State<AppState<S>>,
    Json(mut request): Json<CreateRequest>,
) -> Response {
    if request.request_id.trim().is_empty() || request.request_id.len() > 256 {
        return error(StatusCode::BAD_REQUEST, "request_id is required.");
    }
    if let Err(message) = request.config.validate() {
        return error(StatusCode::BAD_REQUEST, message);
    }
    let suffix = match state.store.get_state("dns_suffix").await {
        Ok(Some(suffix)) => suffix,
        Ok(None) => {
            return error(
                StatusCode::PRECONDITION_FAILED,
                "The Platform is not initialized; run 'self-host init' first.",
            );
        }
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let hostname = match default_hostname(&request.config.name, &suffix) {
        Ok(hostname) => hostname,
        Err(message) => return error(StatusCode::BAD_REQUEST, message),
    };
    // Held until the record is saved, so a Record or an Application cannot
    // claim the name in between.
    let namespace = state.dns_records.lock_namespace().await;
    let mut records = match state.environments.records(&state.store).await {
        Ok(records) => records,
        Err(message) => return error(StatusCode::INTERNAL_SERVER_ERROR, message),
    };
    let normalized_request_id = request.request_id.trim().to_owned();
    let id = request_id(&normalized_request_id);
    if let Some(existing) = records.as_ref().unwrap().iter().find(|r| r.id == id) {
        if serde_json::to_value(&existing.config).ok() != serde_json::to_value(&request.config).ok()
        {
            return error(
                StatusCode::CONFLICT,
                "request_id was already used for a different configuration.",
            );
        }
        return (StatusCode::ACCEPTED, Json(existing.clone())).into_response();
    }
    if let Some(existing) = records
        .as_ref()
        .unwrap()
        .iter()
        .find(|r| r.config.name == request.config.name)
    {
        return error(
            StatusCode::CONFLICT,
            format!(
                "An environment with this name already exists ({}).",
                existing.id
            ),
        );
    }
    match holder(&state.store, records.as_ref().unwrap(), &hostname, &suffix).await {
        Ok(None) => {}
        Ok(Some(holder)) => {
            return error(
                StatusCode::CONFLICT,
                format!("'{hostname}' is already answered by {holder}."),
            );
        }
        Err(message) => return error(StatusCode::INTERNAL_SERVER_ERROR, message),
    }
    let record = EnvironmentRecord {
        id: id.clone(),
        config: request.config.clone(),
        hostname,
        applied_config: None,
        state: VmState::Missing,
        service_ready: false,
        operation: Some(Operation {
            action: "create".into(),
            status: OperationStatus::Running,
            step: None,
            error: None,
        }),
        log: String::new(),
        ssh_command: None,
        tunnel_command: None,
        web_url: None,
        base_image: None,
        installed_versions: None,
        mac_address: None,
        lan_address: None,
    };
    let mut next = records.as_ref().unwrap().clone();
    next.push(record.clone());
    if let Err(message) = save(&state.store, &next).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, message);
    }
    *records = Some(next);
    drop(records);
    drop(namespace);
    accept_operation(&state, record, "create".into()).await
}

/// Queues the operation and answers `202` with the record and the task id.
async fn accept_operation<S: StateStore>(
    state: &AppState<S>,
    record: EnvironmentRecord,
    action: String,
) -> Response {
    let subject = crate::audit::Subject::new(
        "virtual-machine",
        record.id.clone(),
        record.config.name.clone(),
    );
    let work = crate::tasks::Work::OperateVirtualMachine {
        id: record.id.clone(),
        action: action.clone(),
    };
    match crate::tasks::enqueue(state, crate::audit::machine_action(&action), subject, work).await {
        Ok(task_id) => {
            let mut body = serde_json::to_value(&record).unwrap_or(Value::Null);
            body["task_id"] = Value::String(task_id);
            (StatusCode::ACCEPTED, Json(body)).into_response()
        }
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Who already answers on `hostname`, if anyone: a Record, an Application or
/// another machine. One namespace, checked from every side (ADR-0025); the
/// Records and the Applications run the same check against machines.
async fn holder<S: StateStore>(
    store: &S,
    machines: &[EnvironmentRecord],
    hostname: &str,
    suffix: &str,
) -> Result<Option<String>, String> {
    let name = hostname
        .strip_suffix(&format!(".{suffix}"))
        .unwrap_or(hostname);
    if let Some(record) = crate::dns_records::load(store)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|record| record.name == name)
    {
        return Ok(Some(format!("Record '{}'", record.key())));
    }
    for application in store.list_applications().await.map_err(|e| e.to_string())? {
        if crate::routes::hostnames(&application).contains(&hostname) {
            return Ok(Some(format!(
                "Application '{}' ({})",
                application.name, application.id
            )));
        }
    }
    if let Some(machine) = machines.iter().find(|machine| machine.hostname == hostname) {
        return Ok(Some(format!(
            "Virtual machine '{}' ({})",
            machine.config.name, machine.id
        )));
    }
    Ok(None)
}

fn request_id(request_id: &str) -> String {
    let digest = Sha256::digest(request_id.as_bytes());
    let suffix: String = digest[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("env-{suffix}")
}

fn append_log(log: &mut String, text: &str) {
    log.push_str(text);
    if log.len() > MAX_LOG_BYTES {
        let excess = log.len() - MAX_LOG_BYTES;
        let split = log
            .char_indices()
            .find(|(index, _)| *index >= excess)
            .map(|(index, _)| index)
            .unwrap_or(log.len());
        log.drain(..split);
    }
}

pub(crate) async fn update<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
    Json(mut request): Json<UpdateRequest>,
) -> Response {
    if let Err(message) = request.config.validate() {
        return error(StatusCode::BAD_REQUEST, message);
    }
    let mut records = match state.environments.records(&state.store).await {
        Ok(records) => records,
        Err(message) => return error(StatusCode::INTERNAL_SERVER_ERROR, message),
    };
    let Some(record) = records.as_ref().unwrap().iter().find(|r| r.id == id) else {
        return error(StatusCode::NOT_FOUND, "Environment not found.");
    };
    if record
        .operation
        .as_ref()
        .is_some_and(|op| op.status == OperationStatus::Running)
    {
        return error(
            StatusCode::CONFLICT,
            "An operation is already running for this environment.",
        );
    }
    if record.config.cpus != request.config.cpus
        || record.config.memory_gib != request.config.memory_gib
        || record.config.disk_gib != request.config.disk_gib
    {
        return error(
            StatusCode::CONFLICT,
            "CPU, memory, and disk changes are not supported yet.",
        );
    }
    if records
        .as_ref()
        .unwrap()
        .iter()
        .any(|other| other.id != id && other.config.name == request.config.name)
    {
        return error(
            StatusCode::CONFLICT,
            "An environment with this name already exists.",
        );
    }
    let mut next = records.as_ref().unwrap().clone();
    let item = next.iter_mut().find(|r| r.id == id).unwrap();
    item.config = request.config;
    let result = item.clone();
    if let Err(message) = save(&state.store, &next).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, message);
    }
    *records = Some(next);
    Json(result).into_response()
}

pub(crate) async fn action<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
    Json(request): Json<ActionRequest>,
) -> Response {
    let action = request.action.trim().to_ascii_lowercase();
    const ACTIONS: &[&str] = &[
        "create",
        "retry",
        "bootstrap",
        "start",
        "stop",
        "restart",
        "update",
        "apply_update",
        "delete",
    ];
    if !ACTIONS.contains(&action.as_str()) {
        return error(StatusCode::BAD_REQUEST, "Unknown environment action.");
    }
    let mut records = match state.environments.records(&state.store).await {
        Ok(records) => records,
        Err(message) => return error(StatusCode::INTERNAL_SERVER_ERROR, message),
    };
    let Some(record) = records.as_ref().unwrap().iter().find(|r| r.id == id) else {
        return error(StatusCode::NOT_FOUND, "Environment not found.");
    };
    if action == "retry"
        && record.operation.as_ref().is_some_and(|operation| {
            matches!(
                operation.status,
                OperationStatus::Succeeded | OperationStatus::Running
            )
        })
    {
        return error(
            StatusCode::CONFLICT,
            "Only failed or interrupted operations can be retried.",
        );
    }
    let effective_action = if action == "retry" {
        record
            .operation
            .as_ref()
            .map(|op| op.action.as_str())
            .unwrap_or("create")
            .to_owned()
    } else if action == "apply_update" {
        "update".to_owned()
    } else {
        action.clone()
    };
    if effective_action == "delete"
        && request.confirm_name.as_deref() != Some(record.config.name.as_str())
    {
        return error(
            StatusCode::BAD_REQUEST,
            "Type the environment name to confirm deletion.",
        );
    }
    // The record shows the operation that is executing. When one is, this
    // one waits its turn in the queue and the worker writes it when it starts.
    let mut next = records.as_ref().unwrap().clone();
    let index = next.iter().position(|r| r.id == id).unwrap();
    if !next[index]
        .operation
        .as_ref()
        .is_some_and(|op| op.status == OperationStatus::Running)
    {
        next[index].operation = Some(Operation {
            action: effective_action.clone(),
            status: OperationStatus::Running,
            step: None,
            error: None,
        });
        if let Err(message) = save(&state.store, &next).await {
            return error(StatusCode::INTERNAL_SERVER_ERROR, message);
        }
    }
    let accepted = next[index].clone();
    *records = Some(next);
    drop(records);
    accept_operation(&state, accepted, effective_action).await
}

/// Carries one operation out on a machine, on the scheduler's worker. The
/// record says `running` from here until the runtime answers, then carries
/// the outcome; the audit event belongs to the task.
pub(crate) async fn run_operation<S: StateStore>(
    state: &AppState<S>,
    id: &str,
    action: &str,
) -> Result<(), ErrorReport> {
    let id = id.to_owned();
    let action = action.to_owned();
    let config = {
        let mut records = state
            .environments
            .records(&state.store)
            .await
            .map_err(ErrorReport::plain)?;
        let mut next = records.as_ref().unwrap().clone();
        let Some(record) = next.iter_mut().find(|r| r.id == id) else {
            return Err(ErrorReport::plain("Environment not found."));
        };
        record.operation = Some(Operation {
            action: action.clone(),
            status: OperationStatus::Running,
            step: None,
            error: None,
        });
        let config = record.config.clone();
        save(&state.store, &next)
            .await
            .map_err(ErrorReport::plain)?;
        *records = Some(next);
        config
    };
    {
        tracing::info!(environment = %id, %action, name = %config.name, "environment operation started");
        events::append(&id, &action, &format!("--- {action} started ---")).await;
        // Inspect before create so an unknown runner state fails safely. The
        // runtime owns checking for a partial environment and resuming bootstrap.
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        let runtime = state.vm_runtime.clone();
        let operation = action.clone();
        let run_id = id.clone();
        let run_config = config.clone();
        let run = async move {
            if operation == "create" {
                match runtime.inspect(&run_id).await {
                    Ok(observation) if observation.state != VmState::Unknown => {
                        runtime
                            .execute_with_progress(&run_id, &operation, &run_config, progress_tx)
                            .await
                    }
                    Ok(_) => Err("Could not determine the environment state before create.".into()),
                    Err(message) => Err(format!(
                        "Could not inspect environment before create: {message}"
                    )),
                }
            } else {
                runtime
                    .execute_with_progress(&run_id, &operation, &run_config, progress_tx)
                    .await
            }
        };
        tokio::pin!(run);
        let mut progress_open = true;
        let result = loop {
            tokio::select! {
                result = &mut run => break result,
                progress = progress_rx.recv(), if progress_open => {
                    let Some(progress) = progress else { progress_open = false; continue };
                    persist_progress(state, &id, &action, progress).await;
                }
            }
        };
        while let Ok(progress) = progress_rx.try_recv() {
            persist_progress(state, &id, &action, progress).await;
        }
        let mut records = state
            .environments
            .records(&state.store)
            .await
            .map_err(ErrorReport::plain)?;
        let mut next = records.as_ref().unwrap().clone();
        let Some(record) = next.iter_mut().find(|r| r.id == id) else {
            return Err(ErrorReport::plain("Environment not found."));
        };
        let mut outcome = result
            .as_ref()
            .map(drop)
            .map_err(|message| ErrorReport::plain(message.clone()));
        let mut remove = false;
        match result {
            Ok(observation) => {
                let progress_step = record
                    .operation
                    .as_ref()
                    .and_then(|operation| operation.step.clone());
                record.state = observation.state;
                record.service_ready = observation.service_ready;
                let step = observation.step.or(progress_step);
                append_log(&mut record.log, &observation.log);
                record.tunnel_command = observation.tunnel_command;
                record.base_image = observation.base_image;
                record.installed_versions = observation.installed_versions;
                record.mac_address = observation.mac_address;
                record.lan_address = observation.lan_address;
                if action == "create" || action == "bootstrap" || action == "update" {
                    record.applied_config = Some(config);
                }
                record.reach_by_name(observation.ssh_command, observation.web_url);
                record.operation = Some(Operation {
                    action: action.clone(),
                    status: OperationStatus::Succeeded,
                    step,
                    error: None,
                });
                remove = action == "delete" && record.state == VmState::Missing;
                tracing::info!(environment = %id, %action, state = ?record.state, service_ready = record.service_ready, "environment operation succeeded");
                events::append(&id, &action, &format!("--- {action} succeeded ---")).await;
            }
            Err(message) => {
                let step = record
                    .operation
                    .as_ref()
                    .and_then(|operation| operation.step.clone());
                tracing::error!(environment = %id, %action, step = ?step, %message, "environment operation failed");
                events::append(&id, &action, &format!("--- {action} failed: {message} ---")).await;
                record.operation = Some(Operation {
                    action: action.clone(),
                    status: OperationStatus::Failed,
                    step,
                    error: Some(ErrorReport::plain(message)),
                });
            }
        }
        // A deleted machine has no lease, so the same step withdraws its name.
        let (hostname, lease) = (record.hostname.clone(), record.lease());
        if remove {
            next.retain(|record| record.id != id);
            events::discard(&id).await;
        }
        if let Err(error) = save(&state.store, &next).await {
            outcome = Err(ErrorReport::plain(format!(
                "Could not save operation result: {error}"
            )));
            tracing::error!(%error, "Could not save environment operation result");
            // A completed side effect must never leave a durable `running`
            // operation. Keep the record for retry when persistence fails.
            let mut recovery = records.as_ref().unwrap().clone();
            if let Some(record) = recovery.iter_mut().find(|r| r.id == id) {
                let step = record
                    .operation
                    .as_ref()
                    .and_then(|operation| operation.step.clone());
                record.operation = Some(Operation {
                    action: action.clone(),
                    status: OperationStatus::Failed,
                    step,
                    error: Some(ErrorReport::plain(format!(
                        "Could not save operation result: {error}"
                    ))),
                });
            }
            if save(&state.store, &recovery).await.is_ok() {
                *records = Some(recovery);
            } else {
                // Keep the mutex usable and the record visible in this
                // process even when the store remains unavailable.
                *records = Some(recovery);
            }
        } else {
            *records = Some(next);
            // Under the lock, so the Zone sees saves in the order they
            // happened.
            state
                .dns_records
                .follow_lease(&state.store, &hostname, lease)
                .await;
        }
        drop(records);
        outcome
    }
}

async fn persist_progress<S: StateStore>(
    state: &AppState<S>,
    id: &str,
    action: &str,
    progress: RuntimeProgress,
) {
    events::append(id, action, &progress.output).await;
    // The guest prints thousands of lines and a step a minute. Only a step is
    // worth a full rewrite of Platform State.
    if progress.step.is_none() && progress.log.is_empty() {
        return;
    }
    if let Some(step) = &progress.step {
        tracing::info!(environment = id, action, step, "environment step");
    }
    let Ok(mut records) = state.environments.records(&state.store).await else {
        return;
    };
    let mut next = records.as_ref().unwrap().clone();
    let Some(record) = next.iter_mut().find(|record| record.id == id) else {
        return;
    };
    if let Some(step) = progress.step
        && let Some(operation) = record.operation.as_mut()
    {
        operation.step = Some(step);
    }
    append_log(&mut record.log, &progress.log);
    if save(&state.store, &next).await.is_ok() {
        *records = Some(next);
    }
}

#[cfg(test)]
mod running_tests {
    use super::*;
    use crate::store::FakeStateStore;

    fn record(id: &str, state: VmState) -> EnvironmentRecord {
        EnvironmentRecord {
            id: id.into(),
            config: VmConfig {
                name: id.into(),
                cpus: 2,
                memory_gib: 4,
                disk_gib: 20,
                ssh_public_key: "ssh-ed25519 AAAA test".into(),
                recipe: Recipe {
                    name: "T3".into(),
                    template_id: None,
                    dependencies: vec![],
                    setup: vec![],
                    build_checks: vec![],
                    dockerfile: None,
                },
                command: "t3 serve".into(),
                web_port: 3000,
            },
            hostname: format!("{id}.home.lan"),
            applied_config: None,
            state,
            service_ready: false,
            operation: None,
            log: String::new(),
            ssh_command: None,
            tunnel_command: None,
            web_url: None,
            base_image: None,
            installed_versions: None,
            mac_address: None,
            lan_address: None,
        }
    }

    /// The collector measures machines from inside them, so it may only ask
    /// the ones that are up. A stopped machine answers nothing, and a
    /// missing one is not there to ask.
    #[tokio::test]
    async fn only_running_machines_are_offered_to_the_collector() {
        let store = FakeStateStore::new();
        let records = vec![
            record("env-up", VmState::Running),
            record("env-down", VmState::Stopped),
            record("env-gone", VmState::Missing),
        ];
        save(&store, &records).await.unwrap();

        assert_eq!(running_ids(&store).await, vec!["env-up".to_string()]);
    }

    /// A Host with no machines yet is not an error, and neither is a store
    /// that has never held the key.
    #[tokio::test]
    async fn a_host_without_machines_offers_none() {
        let store = FakeStateStore::new();
        assert!(running_ids(&store).await.is_empty());
    }
}
