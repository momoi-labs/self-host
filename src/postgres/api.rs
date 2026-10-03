use std::os::unix::fs::OpenOptionsExt;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use super::*;
use crate::store::VariableDelivery;

pub(crate) fn router<S: StateStore>() -> Router<AppState<S>> {
    Router::new()
        .route("/databases", post(create::<S>))
        .route("/databases/{id}", get(detail::<S>).delete(remove::<S>))
        .route("/databases/{id}/redeploy", post(redeploy::<S>))
        .route("/databases/{id}/connections", post(connect::<S>))
        .route(
            "/databases/{id}/connections/{connection}",
            axum::routing::delete(disconnect::<S>),
        )
        .route(
            "/databases/{id}/connections/{connection}/reveal",
            post(reveal::<S>),
        )
        .route(
            "/databases/{id}/connections/{connection}/import",
            post(upload::<S>).layer(DefaultBodyLimit::max(MAX_IMPORT_BYTES)),
        )
}

async fn redeploy<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
) -> Response {
    match database(&state.store, &id).await {
        Ok((_, app)) => crate::accepted_task(
            tasks::enqueue(
                &state,
                "configure",
                audit::Subject::new("database", app.id.clone(), app.name),
                tasks::Work::Postgres {
                    operation: Operation::Provision {
                        id: app.id,
                        recreate: true,
                    },
                },
            )
            .await,
        ),
        Err(error) => failure(error),
    }
}

pub(crate) fn failure(error: Error) -> Response {
    let status = match &error {
        Error::Invalid(_) => StatusCode::BAD_REQUEST,
        Error::Conflict(_) => StatusCode::CONFLICT,
        Error::Missing => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    crate::error_response(status, &error)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    name: String,
    major: u16,
}

async fn create<S: StateStore>(
    State(state): State<AppState<S>>,
    Json(request): Json<Create>,
) -> Response {
    let result = async {
        let definition = template(request.major)?;
        let _namespace = state.dns_records.lock_namespace().await;
        let _write = CONNECTION_WRITE.lock().await;
        if state.store.application_exists(&request.name).await? {
            return Err(Error::Conflict(
                "An Application already uses that name".into(),
            ));
        }
        let pending = apps::prepare_deploy_from_compose(
            &state.store,
            &request.name,
            &definition,
            None,
            None,
            apps::DeployOptions {
                publication: Some(Publication::Unpublished),
                variable_delivery: Some(VariableDelivery::Referenced),
                network_policy: Some(NetworkPolicy::private()),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| Error::Invalid(e.to_string()))?;
        let app = &pending.record;
        state.store.set_env(&app.id, PASSWORD, &secret()).await?;
        DATABASES
            .upsert(
                &state.store,
                &Database {
                    application_id: app.id.clone(),
                    major: request.major,
                    volume: format!("{}_data", apps::project_name_for(&app.id)),
                    native_port: None,
                },
            )
            .await?;
        let task_id = tasks::enqueue(
            &state,
            "create",
            audit::Subject::new("database", app.id.clone(), app.name.clone()),
            tasks::Work::Postgres {
                operation: Operation::Provision {
                    id: app.id.clone(),
                    recreate: false,
                },
            },
        )
        .await?;
        Ok((
            StatusCode::ACCEPTED,
            Json(serde_json::json!({"id": app.id, "task_id": task_id})),
        ))
    }
    .await;
    match result {
        Ok(value) => value.into_response(),
        Err(error) => failure(error),
    }
}

#[derive(Serialize)]
struct ConnectionView {
    id: String,
    consumer_application_id: String,
    variable: String,
    database: String,
    role: String,
    status: String,
    task_id: Option<String>,
    imported_objects: Option<u64>,
}
impl From<Connection> for ConnectionView {
    fn from(c: Connection) -> Self {
        Self {
            id: c.id,
            consumer_application_id: c.consumer_application_id,
            variable: c.variable,
            database: c.database,
            role: c.role,
            status: c.status,
            task_id: c.task_id,
            imported_objects: c.imported_tables,
        }
    }
}

async fn detail<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
) -> Response {
    let result = async {
        let (definition, app) = database(&state.store, &id).await?;
        let connections: Vec<_> = CONNECTIONS.list(&state.store).await?.into_iter().filter(|c| c.database_application_id == id).map(ConnectionView::from).collect();
        let readiness = state.docker.container_state(&container(&id)).await.ok().flatten().and_then(|s| s.health).unwrap_or_else(|| "unknown".into());
        Ok(Json(serde_json::json!({"application_id": id, "major": definition.major, "volume": definition.volume, "readiness": readiness, "status": app.status, "connections": connections})))
    }.await;
    match result {
        Ok(value) => crate::no_store(value.into_response()),
        Err(error) => failure(error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Connect {
    consumer_application_id: String,
    variable: String,
}

fn valid_variable(variable: &str) -> bool {
    let mut bytes = variable.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && variable.len() <= 128
        && !matches!(variable, "HOME" | "USER" | "LOGNAME")
}

async fn connect<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
    Json(request): Json<Connect>,
) -> Response {
    let result = async {
        let _namespace = state.dns_records.lock_namespace().await;
        let _write = CONNECTION_WRITE.lock().await;
        let (_, provider) = database(&state.store, &id).await?;
        if provider.status != "running" {
            return Err(Error::Conflict(
                "Start PostgreSQL before connecting an Application".into(),
            ));
        }
        if !valid_variable(&request.variable) {
            return Err(Error::Invalid(
                "Choose a valid Application Variable name".into(),
            ));
        }
        let consumer = state
            .store
            .get_application(&request.consumer_application_id)
            .await?
            .ok_or(Error::Missing)?;
        if consumer.id == id || DATABASES.exists(&state.store, &consumer.id).await? {
            return Err(Error::Invalid(
                "Choose a consumer Application, not a database".into(),
            ));
        }
        if state
            .store
            .get_env(&consumer.id, &request.variable)
            .await?
            .is_some()
            || managed_variable(&state.store, &consumer.id, &request.variable).await?
        {
            return Err(Error::Conflict(
                "That Variable already has a value or a managed connection. Choose another name"
                    .into(),
            ));
        }
        let native = matches!(consumer.runtime, Runtime::Native(_));
        if let Runtime::Native(definition) = &consumer.runtime {
            let name = crate::native::identity::AccountName::parse(&definition.account)
                .map_err(|e| Error::Invalid(e.to_string()))?;
            let account = crate::native::identity::resolve(&name)
                .map_err(|e| Error::Invalid(e.to_string()))?;
            crate::connectivity::NativeEndpoint::new(
                &consumer.id,
                &account,
                "database",
                5432,
                crate::ports::FIRST,
            )
            .map_err(|e| Error::Invalid(e.to_string()))?;
        }
        let token = format!("{:024x}", rand::random::<u128>());
        let c = Connection {
            id: token.clone(),
            database_application_id: id.clone(),
            consumer_application_id: consumer.id,
            variable: request.variable,
            database: format!("db_{token}"),
            role: format!("role_{token}"),
            password: secret(),
            status: "connecting".into(),
            sql_revoked: false,
            native,
            task_id: None,
            imported_tables: None,
        };
        CONNECTIONS.upsert(&state.store, &c).await?;
        let task_id = tasks::enqueue(
            &state,
            "configure",
            audit::Subject::new("database", provider.id, provider.name),
            tasks::Work::Postgres {
                operation: Operation::Connect {
                    connection_id: c.id.clone(),
                },
            },
        )
        .await?;
        Ok((
            StatusCode::ACCEPTED,
            Json(serde_json::json!({"connection_id": c.id, "task_id": task_id})),
        ))
    }
    .await;
    match result {
        Ok(value) => value.into_response(),
        Err(error) => failure(error),
    }
}

async fn reveal<S: StateStore>(
    State(state): State<AppState<S>>,
    Path((id, connection_id)): Path<(String, String)>,
) -> Response {
    let result = async {
        let (definition, _) = database(&state.store, &id).await?;
        let c = connection(&state.store, &id, &connection_id).await?;
        if c.status != "ready" {
            return Err(Error::Conflict(
                "Only a ready connection can be revealed".into(),
            ));
        }
        Ok(Json(serde_json::json!({"url": url(&c, &definition)?})))
    }
    .await;
    match result {
        Ok(value) => crate::no_store(value.into_response()),
        Err(error) => failure(error),
    }
}

async fn disconnect<S: StateStore>(
    State(state): State<AppState<S>>,
    Path((id, connection_id)): Path<(String, String)>,
) -> Response {
    let result = async {
        let _namespace = state.dns_records.lock_namespace().await;
        let _write = CONNECTION_WRITE.lock().await;
        let (_, provider) = database(&state.store, &id).await?;
        let mut c = connection(&state.store, &id, &connection_id).await?;
        if c.status == "revoked" {
            return Err(Error::Conflict("This connection is already revoked".into()));
        }
        c.status = "revoking".into();
        CONNECTIONS.upsert(&state.store, &c).await?;
        let task_id = tasks::enqueue(
            &state,
            "configure",
            audit::Subject::new("database", provider.id, provider.name),
            tasks::Work::Postgres {
                operation: Operation::Disconnect { connection_id },
            },
        )
        .await?;
        Ok((StatusCode::ACCEPTED, Json(tasks::Accepted { task_id })))
    }
    .await;
    match result {
        Ok(value) => value.into_response(),
        Err(error) => failure(error),
    }
}

async fn upload<S: StateStore>(
    State(state): State<AppState<S>>,
    Path((id, connection_id)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    let result = async {
        let (_, app) = database(&state.store, &id).await?;
        let c = connection(&state.store, &id, &connection_id).await?;
        if c.status != "ready" {
            return Err(Error::Conflict(
                "Wait for the connection to become ready before importing".into(),
            ));
        }
        if !body.starts_with(b"PGDMP") {
            return Err(Error::Invalid(
                "Upload a pg_dump custom archive created with --format=custom".into(),
            ));
        }
        let upload_id = secret();
        let path = import_path(&upload_id)?;
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory)
            .map_err(|_| Error::Runtime("Could not create private import storage".into()))?;
        use std::{io::Write, os::unix::fs::PermissionsExt};
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| Error::Runtime("Could not protect import storage".into()))?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|_| Error::Runtime("Could not create private import file".into()))?;
        file.write_all(&body)
            .and_then(|()| file.sync_all())
            .map_err(|_| Error::Runtime("Could not save the import".into()))?;
        // The scheduler keeps this database operation on the consumer queue.
        let queued = tasks::enqueue(
            &state,
            "configure",
            audit::Subject::new("database", app.id.clone(), app.name.clone()),
            tasks::Work::Postgres {
                operation: Operation::Import {
                    connection_id,
                    upload_id,
                },
            },
        )
        .await;
        match queued {
            Ok(task_id) => Ok((
                StatusCode::ACCEPTED,
                Json(serde_json::json!({"task_id":task_id,"database_application_id":app.id})),
            )),
            Err(error) => {
                let _ = std::fs::remove_file(path);
                Err(Error::Store(error))
            }
        }
    }
    .await;
    match result {
        Ok(value) => value.into_response(),
        Err(error) => failure(error),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Remove {
    confirm_name: String,
    #[serde(default)]
    delete_data: bool,
}

async fn remove<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
    Json(request): Json<Remove>,
) -> Response {
    let result = async {
        let _namespace = state.dns_records.lock_namespace().await;
        let _write = CONNECTION_WRITE.lock().await;
        let (_, app) = database(&state.store, &id).await?;
        if request.confirm_name != app.name {
            return Err(Error::Invalid(
                "Type the database Application name to confirm removal".into(),
            ));
        }
        if active_reference(&state.store, &id).await? {
            return Err(Error::Conflict(
                "Disconnect every consumer before removing PostgreSQL".into(),
            ));
        }
        let task_id = tasks::enqueue(
            &state,
            "delete",
            audit::Subject::new("database", app.id, app.name),
            tasks::Work::Postgres {
                operation: Operation::Remove {
                    id,
                    delete_data: request.delete_data,
                },
            },
        )
        .await?;
        Ok((StatusCode::ACCEPTED, Json(tasks::Accepted { task_id })))
    }
    .await;
    match result {
        Ok(value) => value.into_response(),
        Err(error) => failure(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn connection_view_never_serializes_credentials() {
        let c = Connection {
            id: "one".into(),
            database_application_id: "db".into(),
            consumer_application_id: "app".into(),
            variable: "DATABASE_URL".into(),
            database: "db_one".into(),
            role: "role_one".into(),
            password: "secret-value".into(),
            status: "ready".into(),
            sql_revoked: false,
            native: false,
            task_id: None,
            imported_tables: None,
        };
        let output = serde_json::to_string(&ConnectionView::from(c)).unwrap();
        assert!(!output.contains("password"));
        assert!(!output.contains("secret-value"));
        assert!(!output.contains("postgresql://"));
    }
    #[test]
    fn variable_names_refuse_reserved_identity_and_invalid_values() {
        assert!(valid_variable("DATABASE_URL"));
        for invalid in [
            "",
            "1DATABASE",
            "DATABASE-URL",
            "HOME",
            "USER",
            "LOGNAME",
            "X\nY",
        ] {
            assert!(!valid_variable(invalid));
        }
    }
}
