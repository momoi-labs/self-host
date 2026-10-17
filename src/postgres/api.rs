use axum::{
    Json, Router,
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
            post(upload::<S>).layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/databases/{id}/connections/{connection}/import-path",
            post(import_from_path::<S>),
        )
        .route("/databases/import-format", post(detect_format::<S>))
        .route("/databases/{id}/imports", get(imports::<S>))
}

const UNKNOWN_FORMAT: &str =
    "Could not tell what kind of dump this is. Choose its type: custom, tar, directory or sql";

/// The type the Operator chose, from `X-Import-Format`, if any.
fn chosen_format(headers: &axum::http::HeaderMap) -> Result<Option<ArchiveFormat>, Error> {
    match headers
        .get("x-import-format")
        .and_then(|value| value.to_str().ok())
    {
        None | Some("") => Ok(None),
        Some(value) => serde_json::from_value(serde_json::Value::String(value.into()))
            .map(Some)
            .map_err(|_| Error::Invalid(UNKNOWN_FORMAT.into())),
    }
}

/// Writes the request body to `path` as it arrives; answers the byte count.
async fn save_upload(path: &std::path::Path, body: axum::body::Body) -> Result<u64, Error> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .await
        .map_err(|_| Error::Runtime("Could not create private import file".into()))?;
    let mut stream = body.into_data_stream();
    let mut bytes = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| Error::Invalid(format!("The upload stopped: {e}")))?;
        bytes += chunk.len() as u64;
        file.write_all(&chunk)
            .await
            .map_err(|_| Error::Runtime("Could not save the import".into()))?;
    }
    file.sync_all()
        .await
        .map_err(|_| Error::Runtime("Could not save the import".into()))?;
    Ok(bytes)
}

/// Checks that `path` names something on this Host, saying which Host: the
/// path is read on the machine that runs self-host, not the browser's.
fn host_path(path: &std::path::Path) -> Result<(), Error> {
    if !path.is_absolute() {
        return Err(Error::Invalid(format!(
            "{} is not an absolute path; start it with /",
            path.display()
        )));
    }
    if !path.exists() {
        let mut name = [0u8; 256];
        // SAFETY: the buffer outlives the call and its length is passed.
        let host = if unsafe { libc::gethostname(name.as_mut_ptr().cast(), name.len()) } == 0 {
            let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            String::from_utf8_lossy(&name[..end]).into_owned()
        } else {
            "this Host".into()
        };
        return Err(Error::Invalid(format!(
            "Nothing at {} on {host}. The path is read on the machine that runs self-host",
            path.display()
        )));
    }
    // A pipe or device would block whoever opens it; a dump is neither.
    if !path.is_file() && !path.is_dir() {
        return Err(Error::Invalid(format!(
            "{} is not a regular file or a directory",
            path.display()
        )));
    }
    Ok(())
}

#[derive(Deserialize)]
struct PathImport {
    path: PathBuf,
    #[serde(default)]
    format: Option<ArchiveFormat>,
}

/// Bytes under `path`: a file's size, or every file's in a directory.
fn size_of(path: &std::path::Path) -> u64 {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::read_dir(path)
            .map(|entries| entries.flatten().map(|entry| size_of(&entry.path())).sum())
            .unwrap_or(0),
        Ok(meta) => meta.len(),
        Err(_) => 0,
    }
}

/// Imports a dump that already sits on the Host. The daemon reads it where it
/// is and never removes it.
async fn import_from_path<S: StateStore>(
    State(state): State<AppState<S>>,
    Path((id, connection_id)): Path<(String, String)>,
    Json(request): Json<PathImport>,
) -> Response {
    let result = async {
        let (_, app) = database(&state.store, &id).await?;
        let c = connection(&state.store, &id, &connection_id).await?;
        if c.status != "ready" {
            return Err(Error::Conflict(
                "Wait for the connection to become ready before importing".into(),
            ));
        }
        host_path(&request.path)?;
        let format = request
            .format
            .or_else(|| ArchiveFormat::detect(&request.path))
            .ok_or_else(|| Error::Invalid(UNKNOWN_FORMAT.into()))?;
        if request.path.is_dir() != (format == ArchiveFormat::Directory) {
            return Err(Error::Invalid(if request.path.is_dir() {
                "A directory imports as a directory dump".into()
            } else {
                "A directory dump is a directory, not a file".into()
            }));
        }
        let task_id = format!("event-{:032x}", rand::random::<u128>());
        remember_import(
            &state.store,
            &Import {
                task_id: task_id.clone(),
                database_application_id: app.id.clone(),
                connection_id: connection_id.clone(),
                consumer_application_id: c.consumer_application_id.clone(),
                file_name: request.path.display().to_string(),
                file_bytes: size_of(&request.path),
                created_at: audit::timestamp(),
                imported_objects: None,
            },
        )
        .await?;
        let queued = audit::EVENT_ID
            .scope(
                Some(task_id.clone()),
                tasks::enqueue(
                    &state,
                    "configure",
                    audit::Subject::new("database", app.id.clone(), app.name.clone()),
                    tasks::Work::Postgres {
                        operation: Operation::Import {
                            connection_id,
                            upload_id: String::new(),
                            path: Some(request.path),
                            format: Some(format),
                        },
                    },
                ),
            )
            .await;
        match queued {
            Ok(task_id) => Ok((
                StatusCode::ACCEPTED,
                Json(serde_json::json!({"task_id":task_id,"database_application_id":app.id})),
            )),
            Err(error) => {
                let _ = IMPORTS.remove(&state.store, &task_id).await;
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
struct FormatQuery {
    path: PathBuf,
}

/// What kind of dump sits at a Host path, for the console to preselect;
/// `format` is null when it cannot tell.
async fn detect_format<S: StateStore>(
    State(_state): State<AppState<S>>,
    Json(request): Json<FormatQuery>,
) -> Response {
    if let Err(error) = host_path(&request.path) {
        return failure(error);
    }
    Json(serde_json::json!({ "format": ArchiveFormat::detect(&request.path) })).into_response()
}

/// The file name the console sends with an upload, as something safe to show:
/// percent-decoded, without a path, control characters or more than 200
/// characters.
fn upload_name(headers: &axum::http::HeaderMap) -> String {
    let raw = headers
        .get("x-upload-name")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = |at: usize| bytes.get(at).and_then(|b| (*b as char).to_digit(16));
        match (bytes[index], hex(index + 1), hex(index + 2)) {
            (b'%', Some(high), Some(low)) => {
                decoded.push((high * 16 + low) as u8);
                index += 3;
            }
            (byte, _, _) => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    let name: String = String::from_utf8_lossy(&decoded)
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect();
    if name.trim().is_empty() {
        "archive".into()
    } else {
        name
    }
}

#[derive(Serialize)]
struct ImportView {
    task_id: String,
    connection_id: String,
    consumer_application_id: String,
    consumer_name: Option<String>,
    file_name: String,
    file_bytes: u64,
    created_at: String,
    imported_objects: Option<u64>,
    /// The Task, with its status, stages and log; null once history lost it.
    task: Option<serde_json::Value>,
}

async fn imports<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
) -> Response {
    let result = async {
        database(&state.store, &id).await?;
        let mut imports: Vec<Import> = IMPORTS
            .list(&state.store)
            .await?
            .into_iter()
            .filter(|import| import.database_application_id == id)
            .collect();
        imports.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        let mut views = Vec::with_capacity(imports.len());
        for import in imports {
            let consumer_name = state
                .store
                .get_application(&import.consumer_application_id)
                .await?
                .map(|app| app.name);
            let task = state
                .store
                .get_audit_event(&import.task_id)
                .await?
                .and_then(|body| serde_json::from_str(&body).ok());
            views.push(ImportView {
                task_id: import.task_id,
                connection_id: import.connection_id,
                consumer_application_id: import.consumer_application_id,
                consumer_name,
                file_name: import.file_name,
                file_bytes: import.file_bytes,
                created_at: import.created_at,
                imported_objects: import.imported_objects,
                task,
            });
        }
        Ok::<_, Error>(Json(views))
    }
    .await;
    match result {
        Ok(value) => value.into_response(),
        Err(error) => failure(error),
    }
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
        let volume_bytes = state.volume_sizes.of(&state.docker, &definition.volume).await;
        Ok(Json(serde_json::json!({"application_id": id, "major": definition.major, "volume": definition.volume, "volume_bytes": volume_bytes, "readiness": readiness, "status": app.status, "connections": connections})))
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
        if native && !crate::native::capabilities().managed_postgres {
            return Err(Error::Invalid("managed PostgreSQL connections for native Applications are unavailable on this Host".into()));
        }
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
    headers: axum::http::HeaderMap,
    body: axum::body::Body,
) -> Response {
    let result = async {
        let (_, app) = database(&state.store, &id).await?;
        let c = connection(&state.store, &id, &connection_id).await?;
        if c.status != "ready" {
            return Err(Error::Conflict(
                "Wait for the connection to become ready before importing".into(),
            ));
        }
        let chosen = chosen_format(&headers)?;
        let upload_id = secret();
        let path = import_path(&upload_id)?;
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory)
            .map_err(|_| Error::Runtime("Could not create private import storage".into()))?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| Error::Runtime("Could not protect import storage".into()))?;
        // The upload streams to private storage, so its size never sits in
        // memory, and the import reads it from there.
        let file_bytes = match save_upload(&path, body).await {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                return Err(error);
            }
        };
        let format = match chosen.or_else(|| ArchiveFormat::detect(&path)) {
            Some(format) => format,
            None => {
                let _ = std::fs::remove_file(&path);
                return Err(Error::Invalid(UNKNOWN_FORMAT.into()));
            }
        };
        if format == ArchiveFormat::Directory {
            let _ = std::fs::remove_file(&path);
            return Err(Error::Invalid(
                "A directory dump cannot be uploaded; import it from its path on the Host".into(),
            ));
        }
        let file_name = upload_name(&headers);
        // The record exists before the worker can run, so the validation
        // stage finds it to add the object count. The Task takes its id.
        let task_id = format!("event-{:032x}", rand::random::<u128>());
        remember_import(
            &state.store,
            &Import {
                task_id: task_id.clone(),
                database_application_id: app.id.clone(),
                connection_id: connection_id.clone(),
                consumer_application_id: c.consumer_application_id.clone(),
                file_name,
                file_bytes,
                created_at: audit::timestamp(),
                imported_objects: None,
            },
        )
        .await
        .inspect_err(|_| {
            // No Task will own the upload, so it goes now.
            let _ = std::fs::remove_file(&path);
        })?;
        // The Task waits its turn on the database's queue.
        let queued = audit::EVENT_ID
            .scope(
                Some(task_id.clone()),
                tasks::enqueue(
                    &state,
                    "configure",
                    audit::Subject::new("database", app.id.clone(), app.name.clone()),
                    tasks::Work::Postgres {
                        operation: Operation::Import {
                            connection_id,
                            upload_id,
                            path: None,
                            format: Some(format),
                        },
                    },
                ),
            )
            .await;
        if queued.is_err() {
            // The upload is removed below either way.
            let _ = IMPORTS.remove(&state.store, &task_id).await;
        }
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
