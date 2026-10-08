//! Managed PostgreSQL remains an unpublished container Application (ADR-0029).

pub(crate) mod api;
mod progress;
#[cfg(test)]
mod progress_tests;
pub mod runtime;
#[cfg(test)]
mod test_runtime;

use std::{
    collections::{BTreeSet, HashMap},
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::{
    AppState, apps, audit,
    collection::{Collection, Keyed},
    compose_app::{PublishedTarget, WebTarget},
    error::ErrorReport,
    store::{ApplicationRecord, NetworkPolicy, Publication, Runtime, StateStore, StoreError},
    tasks,
};

pub const MAJORS: [u16; 3] = [16, 17, 18];
pub const MAX_IMPORT_BYTES: usize = 64 * 1024 * 1024;
const ADMIN: &str = "sf_admin";
const PASSWORD: &str = "SF_POSTGRES_PASSWORD";
pub(crate) static DATABASES: Collection<Database> = Collection::new("managed-postgres");
pub(crate) static CONNECTIONS: Collection<Connection> = Collection::new("postgres-connection");
pub(crate) static CONNECTION_WRITE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

type DatabaseLocks = std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>;
static DATABASE_LOCKS: OnceLock<DatabaseLocks> = OnceLock::new();

async fn lock_database(id: &str) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = DATABASE_LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(id.into())
        .or_default()
        .clone();
    lock.lock_owned().await
}

/// Provider start/stop/restart and connection/import work share this lock.
/// Imports also occupy their consumer's task queue.
pub(crate) async fn lock_if_managed(
    store: &impl StateStore,
    id: &str,
) -> Result<Option<tokio::sync::OwnedMutexGuard<()>>, StoreError> {
    if DATABASES.exists(store, id).await? {
        Ok(Some(lock_database(id).await))
    } else {
        Ok(None)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Database {
    pub application_id: String,
    pub major: u16,
    pub volume: String,
    pub native_port: Option<u16>,
}
impl Keyed for Database {
    fn key(&self) -> String {
        self.application_id.clone()
    }
}

/// A connection's password is private state, never response metadata.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Connection {
    pub id: String,
    pub database_application_id: String,
    pub consumer_application_id: String,
    pub variable: String,
    pub database: String,
    pub role: String,
    pub password: String,
    pub status: String,
    /// Written only after PostgreSQL confirms NOLOGIN and session termination.
    #[serde(default)]
    pub sql_revoked: bool,
    pub native: bool,
    pub task_id: Option<String>,
    pub imported_tables: Option<u64>,
}
impl Keyed for Connection {
    fn key(&self) -> String {
        self.id.clone()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Operation {
    Provision {
        id: String,
        #[serde(default)]
        recreate: bool,
    },
    Connect {
        connection_id: String,
    },
    Disconnect {
        connection_id: String,
    },
    ApplyVariable {
        connection_id: String,
    },
    Import {
        connection_id: String,
        upload_id: String,
    },
    Remove {
        id: String,
        delete_data: bool,
    },
}

#[derive(Debug)]
pub enum Error {
    Invalid(String),
    Conflict(String),
    Missing,
    Store(StoreError),
    Runtime(String),
    /// Docker could not do what a step needed, said in the Platform's words.
    /// Unlike `Runtime`, it carries no Docker output, so a stage records it.
    Unavailable(String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(s) | Self::Conflict(s) | Self::Runtime(s) | Self::Unavailable(s) => {
                f.write_str(s)
            }
            Self::Missing => f.write_str("Managed database or connection not found"),
            Self::Store(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for Error {}
impl From<StoreError> for Error {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

pub async fn metadata(store: &impl StateStore, id: &str) -> Result<Option<Database>, StoreError> {
    DATABASES.get(store, id).await
}

/// Compose failures can repeat interpolated credentials. Sanitize before
/// either Application state or deployment history persists the failure.
pub(crate) async fn runtime_report(
    store: &impl StateStore,
    id: &str,
    report: ErrorReport,
) -> ErrorReport {
    let managed = async {
        Ok::<_, StoreError>(
            DATABASES.exists(store, id).await? || active_reference(store, id).await?,
        )
    }
    .await;
    if matches!(managed, Ok(false)) {
        report
    } else {
        // A failed metadata read must not expose a possibly managed password.
        ErrorReport::plain(
            "The database runtime operation failed. Check PostgreSQL and Application logs, then retry",
        )
    }
}

pub async fn active_reference(store: &impl StateStore, id: &str) -> Result<bool, StoreError> {
    Ok(CONNECTIONS.list(store).await?.iter().any(|c| {
        c.status != "revoked"
            && (c.database_application_id == id || c.consumer_application_id == id)
    }))
}

pub async fn managed_variable(
    store: &impl StateStore,
    id: &str,
    key: &str,
) -> Result<bool, StoreError> {
    Ok(DATABASES.exists(store, id).await?
        || CONNECTIONS
            .list(store)
            .await?
            .iter()
            .any(|c| c.status != "revoked" && c.consumer_application_id == id && c.variable == key))
}

pub async fn reserved_ports(store: &impl StateStore) -> Result<Vec<u16>, StoreError> {
    Ok(DATABASES
        .list(store)
        .await?
        .iter()
        .filter_map(|d| d.native_port)
        .collect())
}

/// Database transport is separate from HTTP Publication. Only a validated
/// Native Application reference enables this loopback-only socket.
pub async fn native_publication(
    store: &impl StateStore,
    id: &str,
) -> Result<Option<PublishedTarget>, StoreError> {
    Ok(DATABASES
        .get(store, id)
        .await?
        .and_then(|d| d.native_port)
        .map(|host_port| PublishedTarget {
            target: WebTarget {
                service: "database".into(),
                port: 5432,
            },
            host_port,
        }))
}

pub fn template(major: u16) -> Result<String, Error> {
    if !MAJORS.contains(&major) {
        return Err(Error::Invalid(
            "Choose PostgreSQL major version 16, 17 or 18".into(),
        ));
    }
    // PostgreSQL 18 stores its cluster in 18/docker under the image's new
    // volume root. Keep the original mount for existing 16/17 Applications.
    let volume_path = if major == 18 {
        "/var/lib/postgresql"
    } else {
        "/var/lib/postgresql/data"
    };
    Ok(format!(
        "services:\n  database:\n    image: postgres:{major}-bookworm\n    environment:\n      POSTGRES_USER: {ADMIN}\n      POSTGRES_PASSWORD: ${{{PASSWORD}:?Managed PostgreSQL credentials are missing}}\n      POSTGRES_DB: postgres\n      POSTGRES_INITDB_ARGS: --auth-host=scram-sha-256\n    volumes:\n      - data:{volume_path}\n    healthcheck:\n      test: [CMD, pg_isready, -h, 127.0.0.1, -U, {ADMIN}, -d, postgres]\n      interval: 2s\n      timeout: 3s\n      retries: 30\n      start_period: 10s\nvolumes:\n  data: {{}}\n"
    ))
}

pub(crate) fn secret() -> String {
    format!(
        "{:032x}{:032x}",
        rand::random::<u128>(),
        rand::random::<u128>()
    )
}

pub(crate) async fn database(
    store: &impl StateStore,
    id: &str,
) -> Result<(Database, ApplicationRecord), Error> {
    let definition = DATABASES.get(store, id).await?.ok_or(Error::Missing)?;
    let app = store.get_application(id).await?.ok_or(Error::Missing)?;
    Ok((definition, app))
}

pub(crate) async fn connection(
    store: &impl StateStore,
    provider: &str,
    id: &str,
) -> Result<Connection, Error> {
    CONNECTIONS
        .get(store, id)
        .await?
        .filter(|c| c.database_application_id == provider)
        .ok_or(Error::Missing)
}

pub(crate) fn url(connection: &Connection, definition: &Database) -> Result<String, Error> {
    let endpoint = if connection.native {
        format!(
            "127.0.0.1:{}",
            definition
                .native_port
                .ok_or_else(|| Error::Conflict("Native database endpoint is not ready".into()))?
        )
    } else {
        format!("{}:5432", container(&definition.application_id))
    };
    // Every URL component is generated from lowercase hexadecimal identifiers.
    Ok(format!(
        "postgresql://{}:{}@{endpoint}/{}",
        connection.role, connection.password, connection.database
    ))
}

fn container(id: &str) -> String {
    crate::compose_app::container_name(&apps::project_name_for(id), "database")
}

async fn sql<S: StateStore>(
    state: &AppState<S>,
    provider: &str,
    db: &str,
    role: &str,
    sql: String,
) -> Result<String, Error> {
    state
        .docker
        .postgres(&runtime::Request::Sql {
            container: container(provider),
            database: db.into(),
            role: role.into(),
            sql,
        })
        .await
        .map_err(|e| Error::Runtime(e.to_string()))
}

async fn ready<S: StateStore>(state: &AppState<S>, id: &str) -> Result<(), Error> {
    for _ in 0..60 {
        let status = state
            .docker
            .container_state(&container(id))
            .await
            .map_err(|e| Error::Runtime(e.to_string()))?;
        if let Some(status) = status {
            if status.status != "running" {
                return Err(Error::Runtime(
                    "PostgreSQL stopped during startup. Inspect the Application log and retry"
                        .into(),
                ));
            }
            if status.health.as_deref() == Some("healthy") {
                sql(state, id, "postgres", ADMIN, "SELECT 1;".into()).await?;
                return Ok(());
            }
            if status.health.as_deref() == Some("unhealthy") {
                break;
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Err(Error::Runtime(
        "PostgreSQL did not become ready within two minutes. Inspect its log and restart".into(),
    ))
}

async fn ensure_image<S: StateStore>(state: &AppState<S>, image: &str) -> Result<(), Error> {
    progress::step(
        state,
        "image",
        "Preparing PostgreSQL image",
        "PostgreSQL image is available on the Host.",
        async {
            let prepared = async {
                if state.docker.image_id(image).await?.is_none() {
                    state.docker.pull_image(image).await?;
                }
                Ok::<_, crate::docker::DockerError>(())
            }
            .await;
            if prepared.is_ok() {
                return Ok(());
            }
            // Docker's answer is never recorded (see `progress`), so the
            // failure names its cause in the Platform's words instead.
            Err(Error::Unavailable(match state.docker.ping().await {
                Err(crate::docker::DockerError::Unavailable(reason)) => reason,
                _ => format!(
                    "Docker could not pull {image}. Run 'docker pull {image}' on the Host to see why, then retry"
                ),
            }))
        },
    )
    .await
}

async fn wait_ready<S: StateStore>(state: &AppState<S>, id: &str) -> Result<(), Error> {
    progress::step(
        state,
        "ready",
        "Waiting for PostgreSQL readiness",
        "Container health is healthy and PostgreSQL accepted a test query.",
        ready(state, id),
    )
    .await
}

async fn sync_network<S: StateStore>(state: &AppState<S>, id: &str) -> Result<(), Error> {
    progress::step(
        state,
        "network",
        "Applying private database network",
        "Provider network and loopback access match the current consumers.",
        sync_provider(state, id),
    )
    .await?;
    progress::step(
        state,
        "network-ready",
        "Checking updated database readiness",
        "Updated container is healthy and PostgreSQL accepted a test query.",
        ready(state, id),
    )
    .await
}

/// Generic Application lifecycle routes use the same database progress and
/// readiness checks. The scheduler already holds the provider lock.
pub(crate) async fn operate<S: StateStore>(
    state: &AppState<S>,
    id: &str,
    action: &str,
    pull: bool,
) -> Result<Vec<audit::Change>, ErrorReport> {
    let result = async {
        let (_, app) = database(&state.store, id).await?;
        if action == "start" {
            ensure_image(state, &app.image).await?;
        }
        let (label, completed) = match action {
            "stop" => ("Stopping PostgreSQL container", "PostgreSQL container stopped. Its data volume was retained."),
            "restart" if pull => ("Pulling image and recreating PostgreSQL", "Current PostgreSQL image pulled and the container recreated with its persistent volume."),
            "restart" => ("Restarting PostgreSQL container", "PostgreSQL container restarted with its persistent volume."),
            _ => ("Starting PostgreSQL container", "PostgreSQL container started with its persistent volume."),
        };
        let changes = progress::step(state, "container", label, completed, async {
            match action {
                "stop" => apps::stop_application(&state.store, state.docker.as_ref(), state.routes.as_ref(), id).await.map(|_| Vec::new()),
                "restart" => apps::restart_application(&state.store, state.docker.as_ref(), state.routes.as_ref(), id, pull).await.map(|(_, changes)| changes),
                _ => apps::start_application(&state.store, state.docker.as_ref(), state.routes.as_ref(), id).await.map(|_| Vec::new()),
            }.map_err(|e| Error::Runtime(e.to_string()))
        }).await?;
        if action != "stop"
            && let Err(error) = wait_ready(state, id).await {
                state.store.set_application_outcome(id, "failed", Some(ErrorReport::new(&error))).await?;
                return Err(error);
        }
        Ok(changes)
    }.await;
    if let Err(error) = &result {
        // Preserve lifecycle behavior: a failed image pull leaves the current
        // container running, while a failed start already recorded its state.
        if let Ok(Some(app)) = state.store.get_application(id).await {
            let _ = state
                .store
                .set_application_outcome(id, &app.status, Some(ErrorReport::new(error)))
                .await;
        }
    }
    result.map_err(|error| ErrorReport::new(&error))
}

pub(crate) async fn run<S: StateStore>(
    state: &AppState<S>,
    operation: Operation,
) -> Result<(), ErrorReport> {
    let provider = match &operation {
        Operation::Provision { id, .. } | Operation::Remove { id, .. } => Some(id.clone()),
        Operation::Connect { connection_id }
        | Operation::Disconnect { connection_id }
        | Operation::Import { connection_id, .. } => CONNECTIONS
            .get(&state.store, connection_id)
            .await
            .map_err(|e| ErrorReport::new(&e))?
            .map(|c| c.database_application_id),
        Operation::ApplyVariable { .. } => None,
    };
    let _operation = match provider {
        Some(id) => Some(lock_database(&id).await),
        None => None,
    };
    let result = execute(state, operation.clone()).await;
    if let Err(error) = &result {
        match &operation {
            Operation::Provision { id, .. } => {
                let _ = state
                    .store
                    .set_application_outcome(id, "failed", Some(ErrorReport::new(error)))
                    .await;
            }
            Operation::Connect { connection_id } | Operation::ApplyVariable { connection_id } => {
                let _write = CONNECTION_WRITE.lock().await;
                if let Ok(Some(mut c)) = CONNECTIONS.get(&state.store, connection_id).await
                    && c.status != "revoking"
                    && c.status != "revoked"
                {
                    c.status = "failed".into();
                    let _ = CONNECTIONS.upsert(&state.store, &c).await;
                }
            }
            _ => {}
        }
    }
    result.map_err(|e| ErrorReport::new(&e))
}

async fn execute<S: StateStore>(state: &AppState<S>, operation: Operation) -> Result<(), Error> {
    use progress::step;
    let store = &state.store;
    match operation {
        Operation::Provision { id, recreate } => {
            let (_, app) = database(store, &id).await?;
            ensure_image(state, &app.image).await?;
            let label = if recreate {
                "Recreating PostgreSQL container"
            } else {
                "Starting PostgreSQL container"
            };
            step(
                state,
                "container",
                label,
                "PostgreSQL container started with its persistent volume.",
                async {
                    if recreate {
                        let project = apps::project_for(store, &app)
                            .await
                            .map_err(|e| Error::Runtime(e.to_string()))?;
                        state
                            .docker
                            .compose_recreate(&project)
                            .await
                            .map_err(|e| Error::Runtime(e.to_string()))?;
                        store.set_application_outcome(&id, "running", None).await?;
                    } else {
                        apps::finish_deploy(
                            store,
                            state.docker.as_ref(),
                            state.routes.as_ref(),
                            apps::PendingDeploy::managed_compose(app),
                        )
                        .await
                        .map_err(|e| Error::Runtime(e.to_string()))?;
                    }
                    Ok(())
                },
            )
            .await?;
            wait_ready(state, &id).await?;
        }
        Operation::Connect { connection_id } => {
            let c = CONNECTIONS
                .get(store, &connection_id)
                .await?
                .ok_or(Error::Missing)?;
            if c.status == "revoking" || c.status == "revoked" {
                return Ok(());
            }
            let (_, provider) = database(store, &c.database_application_id).await?;
            if provider.status != "running" {
                return Err(Error::Conflict(
                    "Start PostgreSQL before connecting an Application".into(),
                ));
            }
            wait_ready(state, &provider.id).await?;
            step(state, "access", "Creating restricted database access", "Consumer database and restricted login role created. Public access revoked.", async {
            // Identifiers/passwords are generated, never interpolated Operator input.
            sql(state, &provider.id, "postgres", ADMIN, format!("SET log_statement = 'none'; SET log_min_error_statement = 'panic'; CREATE ROLE {} LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD '{}';", c.role, c.password)).await?;
            sql(
                state,
                &provider.id,
                "postgres",
                ADMIN,
                format!(
                    "CREATE DATABASE {} OWNER {} ALLOW_CONNECTIONS false;",
                    c.database, c.role
                ),
            )
            .await?;
            sql(
                state,
                &provider.id,
                "postgres",
                ADMIN,
                format!(
                    "REVOKE ALL ON DATABASE {} FROM PUBLIC; GRANT CONNECT ON DATABASE {} TO {}; ALTER DATABASE {} ALLOW_CONNECTIONS true;",
                    c.database, c.database, c.role, c.database
                ),
            )
            .await?;
            Ok(())
            }).await?;
            if !mark_applying(store, &connection_id).await? {
                return Ok(());
            }
            sync_network(state, &provider.id).await?;
            step(
                state,
                "consumer",
                "Queuing consumer Variable update",
                "Consumer Variable update queued as a separate Application task.",
                queue_variable(state, &c),
            )
            .await?;
        }
        Operation::Disconnect { connection_id } => {
            let c = CONNECTIONS
                .get(store, &connection_id)
                .await?
                .ok_or(Error::Missing)?;
            let (_, provider) = database(store, &c.database_application_id).await?;
            wait_ready(state, &provider.id).await?;
            step(state, "revoke", "Revoking database access", "Login disabled, password removed and active sessions terminated.", async {
            // NOLOGIN plus terminating sessions revokes access before network or
            // Variable changes. Keep the role/database so revocation keeps data.
            sql(state, &provider.id, "postgres", ADMIN, format!("DO $body$ BEGIN IF EXISTS (SELECT FROM pg_roles WHERE rolname = '{}') THEN ALTER ROLE {} NOLOGIN PASSWORD NULL; END IF; END $body$; SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE usename = '{}' AND pid <> pg_backend_pid();", c.role, c.role, c.role)).await?;
            mark_sql_revoked(store, &connection_id).await?;
            Ok(())
            }).await?;
            sync_network(state, &provider.id).await?;
            step(
                state,
                "consumer",
                "Queuing consumer Variable cleanup",
                "Consumer Variable cleanup queued as a separate Application task.",
                queue_variable(state, &c),
            )
            .await?;
        }
        Operation::ApplyVariable { connection_id } => {
            step(state, "variable", "Applying database connection Variable", "Managed Variable update finished. The consumer's running or stopped intent was preserved.", async {
            let c = CONNECTIONS
                .get(store, &connection_id)
                .await?
                .ok_or(Error::Missing)?;
            let (definition, _) = database(store, &c.database_application_id).await?;
            let consumer = store
                .get_application(&c.consumer_application_id)
                .await?
                .ok_or(Error::Missing)?;
            let value = if matches!(c.status.as_str(), "revoking" | "revoked") {
                None
            } else {
                Some(url(&c, &definition)?)
            };
            let existing = store.get_env(&consumer.id, &c.variable).await?;
            let owned = existing.as_ref().is_some_and(|value| {
                value.starts_with(&format!("postgresql://{}:{}@", c.role, c.password))
                    && value.ends_with(&format!("/{}", c.database))
            });
            if value.is_some() && existing.is_some() && !owned {
                return Err(Error::Conflict("The consumer Variable changed before the connection was applied. Its value was preserved".into()));
            }
            // A Variable changed outside this managed connection belongs to the Operator.
            let should_apply = value.is_some() || owned;
            if should_apply && matches!(consumer.runtime, Runtime::Native(_)) {
                crate::native::lifecycle::environment_by_id(
                    store,
                    state.native.as_ref(),
                    state.routes.as_ref(),
                    &consumer.id,
                    &c.variable,
                    value.as_deref(),
                )
                .await
                .map_err(|e| Error::Runtime(e.to_string()))?;
            } else if should_apply && consumer.status == apps::STATUS_STOPPED {
                require_stopped_consumer(state, &consumer).await?;
                match value.as_deref() {
                    Some(value) => store.set_env(&consumer.id, &c.variable, value).await?,
                    None => store.unset_env(&consumer.id, &c.variable).await?,
                }
                // Docker cannot change an existing container's environment.
                // Leave it absent so the next explicit Start uses current
                // Variables. remove_container preserves its named volumes.
                let name = apps::container_name_for(&consumer.id);
                if consumer.compose.is_none()
                    && state
                        .docker
                        .container_state(&name)
                        .await
                        .map_err(|e| Error::Runtime(e.to_string()))?
                        .is_some()
                {
                    state
                        .docker
                        .remove_container(&name)
                        .await
                        .map_err(|e| Error::Runtime(e.to_string()))?;
                }
            } else if let Some(value) = value.as_ref() {
                apps::change_env(
                    store,
                    state.docker.as_ref(),
                    &consumer.id,
                    &c.variable,
                    Some(value),
                )
                .await
                .map_err(|e| Error::Runtime(e.to_string()))?;
            } else if should_apply {
                apps::change_env(store, state.docker.as_ref(), &consumer.id, &c.variable, None)
                    .await
                    .map_err(|e| Error::Runtime(e.to_string()))?;
            }
            if finish_variable(store, &connection_id, value.is_some()).await? {
                queue_variable(state, &c).await?;
            }
            Ok(())
            }).await?;
        }
        Operation::Import {
            connection_id,
            upload_id,
        } => {
            let c = CONNECTIONS
                .get(store, &connection_id)
                .await?
                .ok_or(Error::Missing)?;
            let file = import_path(&upload_id)?;
            let result = import(state, &c, &file).await;
            let _ = tokio::fs::remove_file(file).await;
            result?;
        }
        Operation::Remove { id, delete_data } => {
            // Connect uses this same namespace. Keep the guard through Docker
            // teardown and metadata deletion so it cannot accept a reference
            // after this recheck and before the provider disappears.
            let _namespace = state.dns_records.lock_namespace().await;
            if active_reference(store, &id).await? {
                return Err(Error::Conflict(
                    "Disconnect all consumers before removing PostgreSQL".into(),
                ));
            }
            let (definition, app) = database(store, &id).await?;
            step(
                state,
                "container",
                "Removing PostgreSQL container",
                "PostgreSQL container removed.",
                async {
                    let project = apps::project_for(store, &app)
                        .await
                        .map_err(|e| Error::Runtime(e.to_string()))?;
                    state
                        .docker
                        .compose_down(&project)
                        .await
                        .map_err(|e| Error::Runtime(e.to_string()))?;
                    Ok(())
                },
            )
            .await?;
            if delete_data {
                step(
                    state,
                    "volume",
                    "Deleting PostgreSQL data volume",
                    "PostgreSQL data volume deleted.",
                    async {
                        state
                            .docker
                            .postgres(&runtime::Request::RemoveVolume {
                                name: definition.volume,
                            })
                            .await
                            .map_err(|e| Error::Runtime(e.to_string()))?;
                        Ok(())
                    },
                )
                .await?;
            }
            step(
                state,
                "record",
                "Removing database record",
                if delete_data {
                    "Database record removed."
                } else {
                    "Database record removed. The data volume was retained."
                },
                async {
                    store.delete_application(&id).await?;
                    DATABASES.remove(store, &id).await?;
                    for c in CONNECTIONS
                        .list(store)
                        .await?
                        .into_iter()
                        .filter(|c| c.database_application_id == id)
                    {
                        CONNECTIONS.remove(store, &c.id).await?;
                    }
                    Ok(())
                },
            )
            .await?;
        }
    }
    Ok(())
}

async fn mark_applying(store: &impl StateStore, id: &str) -> Result<bool, Error> {
    let _write = CONNECTION_WRITE.lock().await;
    let mut c = CONNECTIONS.get(store, id).await?.ok_or(Error::Missing)?;
    if matches!(c.status.as_str(), "revoking" | "revoked") {
        return Ok(false);
    }
    c.status = "applying".into();
    CONNECTIONS.upsert(store, &c).await?;
    Ok(true)
}

async fn mark_sql_revoked(store: &impl StateStore, id: &str) -> Result<(), Error> {
    let _write = CONNECTION_WRITE.lock().await;
    let mut c = CONNECTIONS.get(store, id).await?.ok_or(Error::Missing)?;
    c.sql_revoked = true;
    CONNECTIONS.upsert(store, &c).await?;
    Ok(())
}

/// True means a revocation accepted during consumer redeploy needs cleanup.
async fn finish_variable(store: &impl StateStore, id: &str, applied: bool) -> Result<bool, Error> {
    let _write = CONNECTION_WRITE.lock().await;
    let mut c = CONNECTIONS.get(store, id).await?.ok_or(Error::Missing)?;
    if applied && matches!(c.status.as_str(), "revoking" | "revoked") {
        return Ok(true);
    }
    if !applied && !c.sql_revoked {
        // Connect may have queued this cleanup before Disconnect reaches SQL.
        // Keep the reference active if SQL revocation fails or is still queued.
        return Ok(false);
    }
    c.status = if applied { "ready" } else { "revoked" }.into();
    CONNECTIONS.upsert(store, &c).await?;
    Ok(false)
}

async fn queue_variable<S: StateStore>(
    state: &AppState<S>,
    connection: &Connection,
) -> Result<(), Error> {
    let _write = CONNECTION_WRITE.lock().await;
    let app = state
        .store
        .get_application(&connection.consumer_application_id)
        .await?
        .ok_or(Error::Missing)?;
    // This consumer work is a child of the provider task. Reusing its event
    // scope would overwrite the parent's persisted task and audit outcome.
    let task_id = audit::EVENT_ID
        .scope(
            None,
            tasks::enqueue(
                state,
                "configure",
                audit::Subject::new("application", app.id, app.name),
                tasks::Work::Postgres {
                    operation: Operation::ApplyVariable {
                        connection_id: connection.id.clone(),
                    },
                },
            ),
        )
        .await?;
    // Change only the task id on the latest row; a concurrent revocation wins.
    if let Some(mut c) = CONNECTIONS.get(&state.store, &connection.id).await? {
        c.task_id = Some(task_id);
        CONNECTIONS.upsert(&state.store, &c).await?;
    }
    Ok(())
}

async fn sync_provider<S: StateStore>(state: &AppState<S>, id: &str) -> Result<(), Error> {
    let _namespace = state.dns_records.lock_namespace().await;
    let (mut definition, mut app) = database(&state.store, id).await?;
    let references: Vec<_> = CONNECTIONS
        .list(&state.store)
        .await?
        .into_iter()
        .filter(|c| {
            c.database_application_id == id
                && !matches!(c.status.as_str(), "revoking" | "revoked" | "failed")
        })
        .collect();
    let consumers: BTreeSet<_> = references
        .iter()
        .filter(|c| !c.native)
        .map(|c| c.consumer_application_id.clone())
        .collect();
    app.network_policy = NetworkPolicy::Private {
        consumers: consumers.into_iter().collect(),
    };
    let needs_loopback = references.iter().any(|c| c.native);
    if needs_loopback && definition.native_port.is_none() {
        let mut taken: BTreeSet<_> = state
            .store
            .list_applications()
            .await?
            .iter()
            .filter_map(|a| a.web_target_port)
            .collect();
        taken.extend(reserved_ports(&state.store).await?);
        definition.native_port =
            Some(crate::ports::allocate(&taken).map_err(|e| Error::Runtime(e.to_string()))?);
    } else if !needs_loopback {
        definition.native_port = None;
    }
    DATABASES.upsert(&state.store, &definition).await?;
    state.store.insert_application(&app).await?;
    apps::finish_deploy(
        &state.store,
        state.docker.as_ref(),
        state.routes.as_ref(),
        apps::PendingDeploy::managed_compose(app),
    )
    .await
    .map_err(|e| Error::Runtime(e.to_string()))?;
    Ok(())
}

async fn import<S: StateStore>(
    state: &AppState<S>,
    c: &Connection,
    file: &std::path::Path,
) -> Result<(), Error> {
    use progress::step;
    step(
        state,
        "consumer",
        "Checking import safety",
        "Connection is ready and the consumer is stopped.",
        async {
            if c.status != "ready" {
                return Err(Error::Conflict(
                    "Wait for the connection to become ready before importing".into(),
                ));
            }
            let consumer = state
                .store
                .get_application(&c.consumer_application_id)
                .await?
                .ok_or(Error::Missing)?;
            require_stopped_consumer(state, &consumer).await?;
            Ok(())
        },
    )
    .await?;
    wait_ready(state, &c.database_application_id).await?;
    step(
        state,
        "archive",
        "Validating PostgreSQL archive",
        "Archive format and PostgreSQL major version are compatible.",
        async {
            let (definition, _) = database(&state.store, &c.database_application_id).await?;
            let manifest = state
                .docker
                .postgres(&runtime::Request::InspectArchive {
                    container: container(&c.database_application_id),
                    file: file.into(),
                })
                .await
                .map_err(|e| Error::Runtime(e.to_string()))?;
            validate_archive_version(&manifest, definition.major)?;
            Ok(())
        },
    )
    .await?;
    let count_sql = "SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname NOT IN ('pg_catalog','information_schema') AND n.nspname NOT LIKE 'pg_toast%' AND c.relkind IN ('r','p','v','m','S','f');";
    step(
        state,
        "empty",
        "Checking target database",
        "Consumer database is empty and ready for import.",
        async {
            let count = sql(
                state,
                &c.database_application_id,
                &c.database,
                &c.role,
                count_sql.into(),
            )
            .await?;
            if count.trim() != "0" {
                return Err(Error::Conflict(
                    "Import requires an empty consumer database. Existing data was preserved"
                        .into(),
                ));
            }
            Ok(())
        },
    )
    .await?;
    step(
        state,
        "restore",
        "Restoring PostgreSQL archive",
        "Archive restored using the consumer's restricted database role.",
        async {
            state
                .docker
                .postgres(&runtime::Request::Restore {
                    container: container(&c.database_application_id),
                    database: c.database.clone(),
                    role: c.role.clone(),
                    file: file.into(),
                })
                .await
                .map_err(|e| Error::Runtime(e.to_string()))?;
            Ok(())
        },
    )
    .await?;
    step(
        state,
        "validate",
        "Validating imported database",
        "Imported database object count verified and saved.",
        async {
            let count = sql(
                state,
                &c.database_application_id,
                &c.database,
                &c.role,
                count_sql.into(),
            )
            .await?;
            let _write = CONNECTION_WRITE.lock().await;
            let mut current = CONNECTIONS
                .get(&state.store, &c.id)
                .await?
                .ok_or(Error::Missing)?;
            current.imported_tables = Some(count.parse().map_err(|_| {
                Error::Runtime("Import completed but object validation failed".into())
            })?);
            CONNECTIONS.upsert(&state.store, &current).await?;
            Ok(())
        },
    )
    .await
}

async fn require_stopped_consumer<S: StateStore>(
    state: &AppState<S>,
    consumer: &ApplicationRecord,
) -> Result<(), Error> {
    let stopped = if consumer.status != apps::STATUS_STOPPED {
        false
    } else if matches!(consumer.runtime, Runtime::Native(_)) {
        let observed = state
            .native
            .status(consumer)
            .await
            .map_err(|e| Error::Runtime(e.to_string()))?;
        !observed.running && !observed.intended_running
    } else {
        let mut containers: BTreeSet<_> = apps::containers_of(consumer)
            .into_iter()
            .map(|(_, name)| name)
            .collect();
        containers.extend(
            state
                .docker
                .application_containers(&consumer.id)
                .await
                .map_err(|e| Error::Runtime(e.to_string()))?,
        );
        let mut stopped = true;
        for name in containers {
            let observed = state
                .docker
                .container_state(&name)
                .await
                .map_err(|e| Error::Runtime(e.to_string()))?;
            stopped &=
                observed.is_none_or(|s| matches!(s.status.as_str(), "created" | "exited" | "dead"));
        }
        stopped
    };
    if !stopped {
        return Err(Error::Conflict(
            "Stop the consumer Application and wait for its processes to exit before continuing"
                .into(),
        ));
    }
    Ok(())
}

pub(crate) fn import_path(id: &str) -> Result<PathBuf, Error> {
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Invalid("Invalid private import identifier".into()));
    }
    Ok(crate::file_store::state_dir()
        .join("postgres-imports")
        .join(id))
}

fn validate_archive_version(manifest: &str, target: u16) -> Result<(), Error> {
    for prefix in [
        "Dumped from database version:",
        "Dumped by pg_dump version:",
    ] {
        let major = manifest
            .lines()
            .filter_map(|line| line.trim().strip_prefix(';'))
            .find_map(|line| line.trim_start().strip_prefix(prefix))
            .and_then(|version| {
                version
                    .trim_start()
                    .split(|c: char| !c.is_ascii_digit())
                    .next()
            })
            .and_then(|major| major.parse::<u16>().ok())
            .ok_or_else(|| {
                Error::Invalid("The import archive does not declare its PostgreSQL version".into())
            })?;
        if major > target {
            return Err(Error::Invalid("The archive was produced by a newer PostgreSQL major version. No data was imported".into()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        docker::{DockerRuntime, FakeDocker},
        store::FakeStateStore,
    };

    pub(super) fn reference(status: &str) -> Connection {
        Connection {
            id: "reference".into(),
            database_application_id: "provider".into(),
            consumer_application_id: "consumer".into(),
            variable: "DATABASE_URL".into(),
            database: "db_fixture".into(),
            role: "role_fixture".into(),
            password: "private-fixture".into(),
            status: status.into(),
            sql_revoked: false,
            native: false,
            task_id: None,
            imported_tables: None,
        }
    }

    async fn fixture() -> (axum::Router, AppState<FakeStateStore>) {
        fixture_with_docker(Arc::new(FakeDocker::new())).await
    }

    pub(super) async fn fixture_with_docker(
        docker: Arc<dyn DockerRuntime>,
    ) -> (axum::Router, AppState<FakeStateStore>) {
        let store = FakeStateStore::new();
        store
            .store_state("dns_suffix", "example.invalid")
            .await
            .unwrap();
        store.store_state("api_key", "fixture-key").await.unwrap();
        for id in ["provider", "consumer"] {
            let app: ApplicationRecord = serde_json::from_value(serde_json::json!({
                "id":id,"name":id,"hostname":"","aliases":[],"image":"postgres:17-bookworm","status":"stopped","source":"compose","compose":"services:\n  app:\n    image: postgres:17-bookworm\n    command: sleep infinity\n","web_service":null,"web_port":null,"web_target_port":null,"development":null,"last_error":null,"publication":{"kind":"unpublished"},"network_policy":{"kind":"private","consumers":[]}
            })).unwrap();
            store.insert_application(&app).await.unwrap();
        }
        DATABASES
            .upsert(
                &store,
                &Database {
                    application_id: "provider".into(),
                    major: 17,
                    volume: "fixture_data".into(),
                    native_port: None,
                },
            )
            .await
            .unwrap();
        crate::build_platform(
            store,
            docker,
            Arc::new(crate::routes::FakeRoutes::new()),
            crate::metrics::Metrics::new(),
            Arc::new(crate::vms::LimaRuntime::default()),
            Arc::new(crate::dns_records::UnservedZone),
            None,
        )
    }

    #[tokio::test]
    async fn revoke_accepted_during_connect_or_variable_apply_cannot_be_overwritten() {
        let store = FakeStateStore::new();
        for status in ["revoking", "revoked"] {
            CONNECTIONS
                .upsert(&store, &reference(status))
                .await
                .unwrap();
            // These are the late completions after SQL or a consumer redeploy.
            assert!(!mark_applying(&store, "reference").await.unwrap());
            assert!(finish_variable(&store, "reference", true).await.unwrap());
            assert_eq!(
                CONNECTIONS
                    .get(&store, "reference")
                    .await
                    .unwrap()
                    .unwrap()
                    .status,
                status
            );
        }
    }

    #[tokio::test]
    async fn queued_apply_after_revocation_removes_only_its_own_variable() {
        let (_, state) = fixture().await;
        let mut c = reference("revoking");
        c.sql_revoked = true;
        CONNECTIONS.upsert(&state.store, &c).await.unwrap();
        let definition = DATABASES
            .get(&state.store, "provider")
            .await
            .unwrap()
            .unwrap();
        state
            .store
            .set_env("consumer", "DATABASE_URL", &url(&c, &definition).unwrap())
            .await
            .unwrap();
        execute(
            &state,
            Operation::ApplyVariable {
                connection_id: c.id.clone(),
            },
        )
        .await
        .unwrap();
        assert!(
            state
                .store
                .get_env("consumer", "DATABASE_URL")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            CONNECTIONS
                .get(&state.store, &c.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            "revoked"
        );
        // A queued duplicate cleanup cannot erase an Operator's newer value.
        state
            .store
            .set_env("consumer", "DATABASE_URL", "operator-owned")
            .await
            .unwrap();
        execute(
            &state,
            Operation::ApplyVariable {
                connection_id: c.id,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            state
                .store
                .get_env("consumer", "DATABASE_URL")
                .await
                .unwrap()
                .as_deref(),
            Some("operator-owned")
        );
    }

    #[tokio::test]
    async fn stopped_consumers_stay_stopped_until_start_applies_the_changed_variable() {
        for compose in [false, true] {
            let docker = Arc::new(FakeDocker::new());
            let (_, state) = fixture_with_docker(docker.clone()).await;
            let mut consumer = state
                .store
                .get_application("consumer")
                .await
                .unwrap()
                .unwrap();
            if !compose {
                consumer.compose = None;
                consumer.source = "image".into();
                state.store.insert_application(&consumer).await.unwrap();
            }
            apps::start_application(
                &state.store,
                docker.as_ref(),
                state.routes.as_ref(),
                "consumer",
            )
            .await
            .unwrap();
            apps::stop_application(
                &state.store,
                docker.as_ref(),
                state.routes.as_ref(),
                "consumer",
            )
            .await
            .unwrap();
            let mut c = reference("applying");
            CONNECTIONS.upsert(&state.store, &c).await.unwrap();
            let definition = DATABASES
                .get(&state.store, "provider")
                .await
                .unwrap()
                .unwrap();
            let expected = url(&c, &definition).unwrap();
            execute(
                &state,
                Operation::ApplyVariable {
                    connection_id: c.id.clone(),
                },
            )
            .await
            .unwrap();
            assert_eq!(
                state
                    .store
                    .get_env("consumer", "DATABASE_URL")
                    .await
                    .unwrap(),
                Some(expected.clone())
            );
            let stopped = state
                .store
                .get_application("consumer")
                .await
                .unwrap()
                .unwrap();
            require_stopped_consumer(&state, &stopped).await.unwrap();
            if !compose {
                assert!(
                    docker
                        .container_state(&apps::container_name_for("consumer"))
                        .await
                        .unwrap()
                        .is_none()
                );
            }
            apps::start_application(
                &state.store,
                docker.as_ref(),
                state.routes.as_ref(),
                "consumer",
            )
            .await
            .unwrap();
            if !compose {
                assert!(
                    docker.apps.lock().unwrap().iter().any(|container| container
                        .env
                        .contains(&format!("DATABASE_URL={expected}")))
                );
            } else {
                assert!(
                    docker
                        .projects
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|project| project.yaml.contains(&expected))
                );
            }

            apps::stop_application(
                &state.store,
                docker.as_ref(),
                state.routes.as_ref(),
                "consumer",
            )
            .await
            .unwrap();
            c.status = "revoking".into();
            c.sql_revoked = true;
            CONNECTIONS.upsert(&state.store, &c).await.unwrap();
            execute(
                &state,
                Operation::ApplyVariable {
                    connection_id: c.id.clone(),
                },
            )
            .await
            .unwrap();
            assert!(
                state
                    .store
                    .get_env("consumer", "DATABASE_URL")
                    .await
                    .unwrap()
                    .is_none()
            );
            let stopped = state
                .store
                .get_application("consumer")
                .await
                .unwrap()
                .unwrap();
            require_stopped_consumer(&state, &stopped).await.unwrap();
            apps::start_application(
                &state.store,
                docker.as_ref(),
                state.routes.as_ref(),
                "consumer",
            )
            .await
            .unwrap();
            if !compose {
                assert!(docker.apps.lock().unwrap().iter().all(|container| {
                    container
                        .env
                        .iter()
                        .all(|value| !value.starts_with("DATABASE_URL="))
                }));
            } else {
                assert!(
                    docker
                        .projects
                        .lock()
                        .unwrap()
                        .iter()
                        .all(|project| !project.yaml.contains(&expected))
                );
            }
        }
    }

    #[tokio::test]
    async fn import_refuses_a_running_consumer_with_a_stale_stopped_record() {
        let docker = Arc::new(FakeDocker::new());
        let (_, state) = fixture_with_docker(docker.clone()).await;
        let consumer = state
            .store
            .get_application("consumer")
            .await
            .unwrap()
            .unwrap();
        let project = apps::project_for(&state.store, &consumer).await.unwrap();
        docker.compose_up(&project).await.unwrap();
        let result = import(
            &state,
            &reference("ready"),
            std::path::Path::new("unused.dump"),
        )
        .await;
        assert!(
            matches!(result, Err(Error::Conflict(message)) if message.contains("processes to exit"))
        );
        docker.compose_stop(&project).await.unwrap();
        require_stopped_consumer(&state, &consumer).await.unwrap();
    }

    #[tokio::test]
    async fn consumer_tasks_get_their_own_id_inside_a_provider_task() {
        let (_, state) = fixture().await;
        let c = reference("applying");
        CONNECTIONS.upsert(&state.store, &c).await.unwrap();
        audit::EVENT_ID
            .scope(Some("provider-task".into()), async {
                queue_variable(&state, &c).await.unwrap();
                assert_eq!(audit::event_id().as_deref(), Some("provider-task"));
            })
            .await;
        let child = CONNECTIONS
            .get(&state.store, &c.id)
            .await
            .unwrap()
            .unwrap()
            .task_id
            .unwrap();
        assert_ne!(child, "provider-task");
        assert!(state.store.get_audit_event(&child).await.unwrap().is_some());
        assert!(
            state
                .store
                .get_audit_event("provider-task")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn cleanup_before_failed_sql_revocation_keeps_the_reference_active() {
        let docker = Arc::new(FakeDocker::new());
        let (_, state) = fixture_with_docker(docker.clone()).await;
        let mut provider = state
            .store
            .get_application("provider")
            .await
            .unwrap()
            .unwrap();
        provider.compose = Some(template(17).unwrap());
        state.store.insert_application(&provider).await.unwrap();
        state
            .store
            .set_env("provider", PASSWORD, "fixture-admin")
            .await
            .unwrap();
        let project = apps::project_for(&state.store, &provider).await.unwrap();
        docker.compose_up(&project).await.unwrap();
        docker.compose_stop(&project).await.unwrap();
        let c = reference("revoking");
        CONNECTIONS.upsert(&state.store, &c).await.unwrap();
        let definition = DATABASES
            .get(&state.store, "provider")
            .await
            .unwrap()
            .unwrap();
        state
            .store
            .set_env("consumer", "DATABASE_URL", &url(&c, &definition).unwrap())
            .await
            .unwrap();

        // Connect's consumer task can run before the provider's Disconnect.
        execute(
            &state,
            Operation::ApplyVariable {
                connection_id: c.id.clone(),
            },
        )
        .await
        .unwrap();
        assert!(
            execute(
                &state,
                Operation::Disconnect {
                    connection_id: c.id.clone()
                }
            )
            .await
            .is_err()
        );
        let current = CONNECTIONS.get(&state.store, &c.id).await.unwrap().unwrap();
        assert_eq!(current.status, "revoking");
        assert!(!current.sql_revoked);
        assert!(active_reference(&state.store, "provider").await.unwrap());
        assert!(
            managed_variable(&state.store, "consumer", "DATABASE_URL")
                .await
                .unwrap()
        );
        assert!(
            state
                .store
                .get_env("consumer", "DATABASE_URL")
                .await
                .unwrap()
                .is_none()
        );

        // A successful SQL retry still needs its following consumer cleanup.
        mark_sql_revoked(&state.store, &c.id).await.unwrap();
        assert_eq!(
            CONNECTIONS
                .get(&state.store, &c.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            "revoking"
        );
        execute(
            &state,
            Operation::ApplyVariable {
                connection_id: c.id.clone(),
            },
        )
        .await
        .unwrap();
        assert_eq!(
            CONNECTIONS
                .get(&state.store, &c.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            "revoked"
        );
    }

    #[tokio::test]
    async fn connect_cannot_be_accepted_during_provider_removal() {
        use axum::{
            body::Body,
            http::{Request, StatusCode},
        };
        use tower::ServiceExt;

        let docker = Arc::new(test_runtime::PausedRemoval::default());
        let (router, state) = fixture_with_docker(docker.clone()).await;
        state
            .store
            .set_application_outcome("provider", "running", None)
            .await
            .unwrap();
        let remove_state = state.clone();
        let removal = tokio::spawn(async move {
            execute(
                &remove_state,
                Operation::Remove {
                    id: "provider".into(),
                    delete_data: false,
                },
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(1), docker.entered.notified())
            .await
            .unwrap();
        let request = Request::builder()
            .method("POST")
            .uri("/databases/provider/connections")
            .header("authorization", "Bearer fixture-key")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"consumer_application_id":"consumer","variable":"DATABASE_URL"}"#,
            ))
            .unwrap();
        let mut connect = Box::pin(router.oneshot(request));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut connect)
                .await
                .is_err()
        );
        docker.release.notify_one();
        removal.await.unwrap().unwrap();
        assert_eq!(connect.await.unwrap().status(), StatusCode::NOT_FOUND);
        assert!(CONNECTIONS.list(&state.store).await.unwrap().is_empty());
        assert!(
            state
                .store
                .get_application("provider")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn connection_reveal_requires_authentication_and_metadata_has_no_password() {
        use axum::{
            body::{Body, to_bytes},
            http::{Request, StatusCode},
        };
        use tower::ServiceExt;
        let (router, state) = fixture().await;
        CONNECTIONS
            .upsert(&state.store, &reference("ready"))
            .await
            .unwrap();
        let request = Request::builder()
            .method("POST")
            .uri("/databases/provider/connections/reference/reveal")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            router.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let request = Request::builder()
            .uri("/databases/provider")
            .header("authorization", "Bearer fixture-key")
            .body(Body::empty())
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1 << 20).await.unwrap();
        assert!(!String::from_utf8_lossy(&body).contains("private-fixture"));
        let request = Request::builder()
            .method("POST")
            .uri("/databases/provider/connections/reference/reveal")
            .header("authorization", "Bearer fixture-key")
            .body(Body::empty())
            .unwrap();
        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response.headers()["cache-control"]
                .to_str()
                .unwrap()
                .contains("no-store")
        );
    }

    #[tokio::test]
    async fn managed_database_cannot_be_overwritten_or_removed_through_ordinary_app_paths() {
        let (_, state) = fixture().await;
        assert!(matches!(
            apps::prepare_update(&state.store, "provider", apps::ApplicationUpdate::default())
                .await,
            Err(apps::DeployError::Connectivity(_))
        ));
        let provider = state
            .store
            .get_application("provider")
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            apps::require_removable(&state.store, &provider).await,
            Err(apps::RemoveError::ManagedDatabase)
        ));
        CONNECTIONS
            .upsert(&state.store, &reference("connecting"))
            .await
            .unwrap();
        let consumer = state
            .store
            .get_application("consumer")
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            apps::require_removable(&state.store, &consumer).await,
            Err(apps::RemoveError::ConnectionsExist)
        ));
    }
    #[test]
    fn template_has_durable_storage_and_no_listener_or_plain_credentials() {
        assert_eq!(MAJORS, [16, 17, 18]);
        for (major, mount) in [
            (16, "data:/var/lib/postgresql/data"),
            (17, "data:/var/lib/postgresql/data"),
            (18, "data:/var/lib/postgresql"),
        ] {
            let yaml = template(major).unwrap();
            let value: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(
                value["services"]["database"]["image"],
                format!("postgres:{major}-bookworm")
            );
            assert!(value["services"]["database"].get("ports").is_none());
            assert_eq!(
                value["services"]["database"]["volumes"],
                serde_yaml::to_value(vec![mount]).unwrap()
            );
            assert!(
                value["services"]["database"]["environment"]
                    .get("PGDATA")
                    .is_none()
            );
            assert!(yaml.contains("${SF_POSTGRES_PASSWORD:?"));
        }
        assert!(template(15).is_err());
        assert!(template(19).is_err());
    }
    #[test]
    fn upload_ids_cannot_escape_private_storage() {
        assert!(import_path("../../other").is_err());
        assert!(import_path(&secret()).is_ok());
    }
    #[test]
    fn archives_from_newer_servers_or_dump_tools_are_refused() {
        let manifest = ";\n;     Dump Version: 1.16-0\n;     Dumped from database version: 16.4\n;     Dumped by pg_dump version: 17.2\n;";
        assert!(validate_archive_version(manifest, 17).is_ok());
        assert!(validate_archive_version(manifest, 16).is_err());
        assert!(
            validate_archive_version(
                ";     Dumped from database version: 18.1\n;     Dumped by pg_dump version: 17.2",
                17
            )
            .is_err()
        );
        assert!(validate_archive_version("not an archive", 17).is_err());
    }

    #[test]
    fn postgres_18_accepts_compatible_archives_without_allowing_newer_majors() {
        for (server, tool, accepted) in [
            (16, 16, true),
            (17, 17, true),
            (17, 18, true),
            (18, 18, true),
            (18, 19, false),
            (19, 18, false),
        ] {
            let manifest = format!(
                ";     Dumped from database version: {server}.1\n;     Dumped by pg_dump version: {tool}.1"
            );
            assert_eq!(validate_archive_version(&manifest, 18).is_ok(), accepted);
            if server == 18 || tool == 18 {
                assert!(validate_archive_version(&manifest, 17).is_err());
            }
        }
    }

    #[test]
    fn archive_versions_accept_the_real_pg_restore_header_and_require_both_versions() {
        let server = ";     Dumped from database version: 17.11 (Debian 17.11-1.pgdg12+2)";
        let tool = ";     Dumped by pg_dump version: 17.11 (Debian 17.11-1.pgdg12+2)";
        assert!(validate_archive_version(&format!("{server}\n{tool}"), 17).is_ok());
        assert!(validate_archive_version(&format!("{server}\n{tool}"), 16).is_err());
        assert!(validate_archive_version(server, 17).is_err());
        assert!(validate_archive_version(tool, 17).is_err());
        assert!(
            validate_archive_version(
                &format!("{server}\n;     Dumped by pg_dump version: unknown"),
                17
            )
            .is_err()
        );
    }
}
