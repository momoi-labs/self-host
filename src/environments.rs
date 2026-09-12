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
use std::time::Duration;
use tokio::sync::{Mutex, mpsc};

use sha2::{Digest, Sha256};

use crate::{AppState, dev_images::Recipe, error::ErrorReport, store::StateStore};

const STATE_KEY: &str = "environments_v1";
const MAX_LOG_BYTES: usize = 64 * 1024;
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
        if self.ssh_public_key.is_empty() || self.ssh_public_key.len() > 16 * 1024 {
            return Err("Enter an SSH public key.".into());
        }
        if self.command.is_empty() || self.command.len() > 4096 {
            return Err("Enter a service command of 1 to 4096 bytes.".into());
        }
        if self.web_port == 0 {
            return Err("Web port must be between 1 and 65535.".into());
        }
        self.recipe.validate()?;
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
}

#[derive(Clone, Debug, Default)]
pub struct RuntimeProgress {
    pub step: Option<String>,
    pub log: String,
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
            record.ssh_command = observation.ssh_command;
            record.tunnel_command = observation.tunnel_command;
            record.web_url = observation.web_url;
            record.base_image = observation.base_image;
            record.installed_versions = observation.installed_versions;
        }
        Ok(Err(_)) | Err(_) => {
            record.state = VmState::Unknown;
            record.service_ready = false;
        }
    }
    if save(&state.store, &next).await.is_ok() {
        *records = Some(next);
    }
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
    let record = EnvironmentRecord {
        id: id.clone(),
        config: request.config.clone(),
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
    };
    let mut next = records.as_ref().unwrap().clone();
    next.push(record.clone());
    if let Err(message) = save(&state.store, &next).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, message);
    }
    *records = Some(next);
    drop(records);
    spawn_operation(state, id, "create".into());
    (StatusCode::ACCEPTED, Json(record)).into_response()
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
        && record
            .operation
            .as_ref()
            .is_some_and(|operation| operation.status == OperationStatus::Succeeded)
    {
        return error(
            StatusCode::CONFLICT,
            "Only failed or interrupted operations can be retried.",
        );
    }
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
    let mut next = records.as_ref().unwrap().clone();
    let item = next.iter_mut().find(|r| r.id == id).unwrap();
    item.operation = Some(Operation {
        action: effective_action.clone(),
        status: OperationStatus::Running,
        step: None,
        error: None,
    });
    let accepted = item.clone();
    if let Err(message) = save(&state.store, &next).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, message);
    }
    *records = Some(next);
    drop(records);
    spawn_operation(state, id, effective_action);
    (StatusCode::ACCEPTED, Json(accepted)).into_response()
}

fn spawn_operation<S: StateStore>(state: AppState<S>, id: String, action: String) {
    tokio::spawn(async move {
        let config = match state.environments.records(&state.store).await {
            Ok(records) => records
                .as_ref()
                .unwrap()
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.config.clone()),
            Err(_) => None,
        };
        let Some(config) = config else { return };
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
                    persist_progress(&state, &id, progress).await;
                }
            }
        };
        while let Ok(progress) = progress_rx.try_recv() {
            persist_progress(&state, &id, progress).await;
        }
        let Ok(mut records) = state.environments.records(&state.store).await else {
            return;
        };
        let mut next = records.as_ref().unwrap().clone();
        let Some(record) = next.iter_mut().find(|r| r.id == id) else {
            return;
        };
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
                record.ssh_command = observation.ssh_command;
                record.tunnel_command = observation.tunnel_command;
                record.web_url = observation.web_url;
                record.base_image = observation.base_image;
                record.installed_versions = observation.installed_versions;
                if action == "create" || action == "bootstrap" || action == "update" {
                    record.applied_config = Some(config);
                }
                record.operation = Some(Operation {
                    action: action.clone(),
                    status: OperationStatus::Succeeded,
                    step,
                    error: None,
                });
                remove = action == "delete" && record.state == VmState::Missing;
            }
            Err(message) => {
                let step = record
                    .operation
                    .as_ref()
                    .and_then(|operation| operation.step.clone());
                record.operation = Some(Operation {
                    action: action.clone(),
                    status: OperationStatus::Failed,
                    step,
                    error: Some(ErrorReport::plain(message)),
                });
            }
        }
        if remove {
            next.retain(|record| record.id != id);
        }
        if let Err(error) = save(&state.store, &next).await {
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
                    action,
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
        }
    });
}

async fn persist_progress<S: StateStore>(state: &AppState<S>, id: &str, progress: RuntimeProgress) {
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
