//! Import uploads the console can resume. The file arrives in chunks into a
//! session in private storage; after a dropped connection or a page refresh
//! the console asks how many bytes the session holds and sends the rest.
//! A complete session queues the import as a one-request upload does.

use std::{
    collections::HashMap,
    path::{Path as FsPath, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::{Deserialize, Serialize};

use super::{api::failure, *};

/// A session nobody sent a byte to for this long is removed, file included.
const ABANDONED_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// A chunk that sends nothing for this long ends, keeping what arrived.
const IDLE: Duration = Duration::from_secs(60);

pub(crate) fn router<S: StateStore>() -> Router<AppState<S>> {
    Router::new()
        .route("/databases/{id}/imports/uploads", post(start::<S>))
        .route(
            "/databases/{id}/imports/uploads/{session}",
            axum::routing::get(status::<S>)
                .put(append::<S>)
                .delete(cancel::<S>)
                .layer(DefaultBodyLimit::disable()),
        )
        .route(
            "/databases/{id}/imports/uploads/{session}/import",
            post(finish::<S>),
        )
}

/// What a session is for, kept beside its bytes.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Session {
    id: String,
    database_application_id: String,
    connection_id: String,
    file_name: String,
    size: u64,
    /// The console's mark for the file, so a resume picks the same one.
    fingerprint: String,
    #[serde(default)]
    format: Option<ArchiveFormat>,
    all_or_nothing: bool,
    /// The import a finished session queued. The record stays a while so a
    /// retry whose answer was lost gets this Task instead of a second one.
    #[serde(default)]
    task_id: Option<String>,
}

fn sessions_dir() -> PathBuf {
    crate::file_store::state_dir()
        .join("postgres-imports")
        .join("sessions")
}

fn valid_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The session's bytes sit where an import reads an upload, under the
/// session's id, so queueing the import moves nothing.
fn bytes_path(id: &str) -> PathBuf {
    crate::file_store::state_dir()
        .join("postgres-imports")
        .join(id)
}

fn record_path(id: &str) -> PathBuf {
    sessions_dir().join(format!("{id}.json"))
}

/// Creates private session storage: the daemon alone reads it.
fn private_storage() -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    let sessions = sessions_dir();
    std::fs::create_dir_all(&sessions)
        .map_err(|_| Error::Runtime("Could not create private import storage".into()))?;
    for directory in [sessions.parent().unwrap(), &sessions] {
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| Error::Runtime("Could not protect import storage".into()))?;
    }
    Ok(())
}

fn read_session(database: &str, id: &str) -> Result<Session, Error> {
    if !valid_id(id) {
        return Err(Error::Missing);
    }
    let body = std::fs::read(record_path(id)).map_err(|_| Error::Missing)?;
    serde_json::from_slice::<Session>(&body)
        .ok()
        .filter(|session| session.database_application_id == database)
        .ok_or(Error::Missing)
}

fn write_session(session: &Session) -> Result<(), Error> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let save = || -> std::io::Result<()> {
        let temporary = sessions_dir().join(format!("{}.json.new", session.id));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec(session).map_err(std::io::Error::other)?)?;
        file.sync_all()?;
        std::fs::rename(temporary, record_path(&session.id))?;
        // The rename lasts through a power loss only once its directory is
        // on disk too.
        std::fs::File::open(sessions_dir())?.sync_all()
    };
    save().map_err(|_| Error::Runtime("Could not save the upload session".into()))
}

/// Bytes the session holds: its file is only ever appended to, in order.
fn received(id: &str) -> u64 {
    std::fs::metadata(bytes_path(id))
        .map(|meta| meta.len())
        .unwrap_or(0)
}

fn remove_session(id: &str) {
    let _ = std::fs::remove_file(record_path(id));
    let _ = std::fs::remove_file(bytes_path(id));
}

/// One writer per session at a time; a second request waits its turn and
/// then finds the offset it must start from.
fn session_lock(id: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: Mutex<Option<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = Mutex::new(None);
    let mut locks = LOCKS.lock().unwrap_or_else(|e| e.into_inner());
    let locks = locks.get_or_insert_with(HashMap::new);
    // A lock nobody holds or waits for is only taking room.
    locks.retain(|_, lock| Arc::strong_count(lock) > 1);
    locks.entry(id.into()).or_default().clone()
}

/// Session starts and lookups take turns, so two starts for one file find
/// one session.
static STARTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Serialize)]
struct Progress {
    id: String,
    size: u64,
    received: u64,
}

fn progress(session: &Session) -> Progress {
    Progress {
        id: session.id.clone(),
        size: session.size,
        received: if session.task_id.is_some() {
            session.size
        } else {
            received(&session.id)
        },
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    connection_id: String,
    name: String,
    size: u64,
    fingerprint: String,
    #[serde(default)]
    format: Option<ArchiveFormat>,
    #[serde(default = "super::all_or_nothing_default")]
    all_or_nothing: bool,
}

/// Starts a session for a file, or finds the one already started for the
/// same file into the same connection, with the bytes it holds.
async fn start<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
    Json(request): Json<Start>,
) -> Response {
    let result = async {
        database(&state.store, &id).await?;
        let c = connection(&state.store, &id, &request.connection_id).await?;
        if c.status != "ready" {
            return Err(Error::Conflict(
                "Wait for the connection to become ready before importing".into(),
            ));
        }
        if request.size == 0 {
            return Err(Error::Invalid("The file is empty".into()));
        }
        if request.fingerprint.is_empty()
            || request.fingerprint.len() > 128
            || !request
                .fingerprint
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_:.".contains(&b))
        {
            return Err(Error::Invalid("The file fingerprint is not valid".into()));
        }
        if request.format == Some(ArchiveFormat::Directory) {
            return Err(Error::Invalid(
                "A directory dump cannot be uploaded; import it from its path on the Host".into(),
            ));
        }
        private_storage()?;
        let file_name = super::api::shown_name(&request.name);
        let matches = |session: &Session| {
            session.database_application_id == id
                && session.connection_id == request.connection_id
                && session.file_name == file_name
                && session.size == request.size
                && session.fingerprint == request.fingerprint
        };
        let found = {
            let _starts = STARTS.lock().await;
            // A queued session is not resumed: the same file picked again is
            // a new import. One whose Task never got queued is open again.
            let mut open = None;
            for mut session in sessions().into_iter().filter(matches) {
                if session.task_id.is_some() {
                    let lock = session_lock(&session.id);
                    let _writing = lock.lock().await;
                    let Ok(mut current) = read_session(&id, &session.id) else {
                        continue;
                    };
                    reclaim(&state, &mut current).await?;
                    session = current;
                }
                if session.task_id.is_none() {
                    open = Some(session);
                    break;
                }
            }
            match open {
                Some(session) => session,
                None => {
                    let session = Session {
                        id: secret(),
                        database_application_id: id.clone(),
                        connection_id: request.connection_id.clone(),
                        file_name: file_name.clone(),
                        size: request.size,
                        fingerprint: request.fingerprint.clone(),
                        format: request.format,
                        all_or_nothing: request.all_or_nothing,
                        task_id: None,
                    };
                    create_bytes(&session.id)?;
                    if let Err(error) = write_session(&session) {
                        remove_session(&session.id);
                        return Err(error);
                    }
                    return Ok(Json(progress(&session)));
                }
            }
        };
        // The Operator may choose differently this time; the last choice is
        // the one the import takes. A chunk being written finishes first.
        let lock = session_lock(&found.id);
        let _writing = lock.lock().await;
        let mut session = read_session(&id, &found.id)
            .ok()
            .filter(|session| session.task_id.is_none())
            .ok_or_else(|| {
                Error::Conflict("This upload just finished or was removed; start it again".into())
            })?;
        session.format = request.format;
        session.all_or_nothing = request.all_or_nothing;
        // A daemon stopped halfway through handing it over leaves no bytes.
        if !bytes_path(&session.id).exists() {
            create_bytes(&session.id)?;
        }
        write_session(&session)?;
        Ok(Json(progress(&session)))
    }
    .await;
    match result {
        Ok(value) => value.into_response(),
        Err(error) => failure(error),
    }
}

fn create_bytes(id: &str) -> Result<(), Error> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(bytes_path(id))
        .map(|_| ())
        .map_err(|_| Error::Runtime("Could not create private import file".into()))
}

/// Every session on disk, unreadable ones skipped.
fn sessions() -> Vec<Session> {
    let Ok(entries) = std::fs::read_dir(sessions_dir()) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let id = name.strip_suffix(".json")?;
            valid_id(id).then_some(())?;
            serde_json::from_slice(&std::fs::read(entry.path()).ok()?).ok()
        })
        .collect()
}

async fn status<S: StateStore>(
    State(_state): State<AppState<S>>,
    Path((id, session)): Path<(String, String)>,
) -> Response {
    match read_session(&id, &session) {
        Ok(session) => crate::no_store(Json(progress(&session)).into_response()),
        Err(error) => failure(error),
    }
}

/// `bytes start-end/total` from `Content-Range`.
fn content_range(headers: &HeaderMap) -> Option<(u64, u64, u64)> {
    let value = headers.get("content-range")?.to_str().ok()?;
    let (range, total) = value.strip_prefix("bytes ")?.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    let (start, end, total) = (start.parse().ok()?, end.parse().ok()?, total.parse().ok()?);
    (start <= end).then_some((start, end, total))
}

/// The offset a chunk must start from, for the console to go on from.
fn out_of_order(message: String, received: u64) -> Response {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({ "error": message, "caused_by": [], "received": received })),
    )
        .into_response()
}

/// Appends one chunk. It must start where the held bytes end and stay inside
/// the declared size, so the file never has a gap, an overlap or more bytes
/// than the file it copies. A chunk cut short keeps what arrived.
async fn append<S: StateStore>(
    State(_state): State<AppState<S>>,
    Path((id, session)): Path<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Response {
    let session = match read_session(&id, &session) {
        Ok(session) => session,
        Err(error) => return failure(error),
    };
    let Some((first, last, total)) = content_range(&headers) else {
        return failure(Error::Invalid(
            "Send Content-Range: bytes start-end/size with each chunk".into(),
        ));
    };
    if total != session.size || last >= session.size {
        return failure(Error::Invalid(format!(
            "This upload is {} bytes; the chunk goes past it",
            session.size
        )));
    }
    let lock = session_lock(&session.id);
    let _writing = lock.lock().await;
    // The session may have been finished or removed while this waited.
    if read_session(&id, &session.id).is_err() {
        return failure(Error::Missing);
    }
    if session.task_id.is_some() {
        return out_of_order("This upload is already imported".into(), session.size);
    }
    let held = received(&session.id);
    if first != held {
        return out_of_order(
            format!("The upload holds {held} bytes; send the chunk that starts there"),
            held,
        );
    }
    match write_chunk(&bytes_path(&session.id), body, last - first + 1).await {
        Ok(()) => Json(progress(&session)).into_response(),
        Err(error) => failure(error),
    }
}

/// Appends `body` to `path`, refusing more than `expected` bytes.
async fn write_chunk(path: &FsPath, body: axum::body::Body, expected: u64) -> Result<(), Error> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .await
        .map_err(|_| Error::Missing)?;
    let mut stream = body.into_data_stream();
    let mut written = 0u64;
    let mut outcome = Ok(());
    loop {
        // A connection that went quiet holds the session's lock; it lets go
        // so a resumed upload is not kept waiting on it.
        let chunk = match tokio::time::timeout(IDLE, stream.next()).await {
            Ok(None) => break,
            Ok(Some(Ok(chunk))) => chunk,
            Ok(Some(Err(error))) => {
                outcome = Err(Error::Invalid(format!("The upload stopped: {error}")));
                break;
            }
            Err(_) => {
                outcome = Err(Error::Invalid(
                    "The upload stopped: nothing arrived for a minute".into(),
                ));
                break;
            }
        };
        if written + chunk.len() as u64 > expected {
            outcome = Err(Error::Invalid(
                "The chunk is longer than its Content-Range".into(),
            ));
            break;
        }
        if file.write_all(&chunk).await.is_err() {
            outcome = Err(Error::Runtime("Could not save the import".into()));
            break;
        }
        written += chunk.len() as u64;
    }
    if outcome.is_ok() && written != expected {
        outcome = Err(Error::Invalid(
            "The chunk is shorter than its Content-Range".into(),
        ));
    }
    // What arrived stays: it is the start of the chunk, and the next one
    // resumes after it.
    file.sync_data()
        .await
        .map_err(|_| Error::Runtime("Could not save the import".into()))?;
    outcome
}

/// Queues the import of a complete session, as a one-request upload does.
/// A refusal keeps the session, so the Operator can fix it and finish.
async fn finish<S: StateStore>(
    State(state): State<AppState<S>>,
    Path((id, session)): Path<(String, String)>,
) -> Response {
    let result = async {
        let session = read_session(&id, &session)?;
        let lock = session_lock(&session.id);
        let _writing = lock.lock().await;
        let mut session = read_session(&id, &session.id)?;
        reclaim(&state, &mut session).await?;
        if let Some(task_id) = &session.task_id {
            return Ok((
                StatusCode::ACCEPTED,
                Json(serde_json::json!({"task_id":task_id,"database_application_id":id})),
            ));
        }
        let (_, app) = database(&state.store, &id).await?;
        let c = connection(&state.store, &id, &session.connection_id).await?;
        if c.status != "ready" {
            return Err(Error::Conflict(
                "Wait for the connection to become ready before importing".into(),
            ));
        }
        let held = received(&session.id);
        if held != session.size {
            return Err(Error::Conflict(format!(
                "The upload holds {held} of {} bytes; send the rest first",
                session.size
            )));
        }
        let bytes = bytes_path(&session.id);
        let format = session
            .format
            .or_else(|| ArchiveFormat::detect(&bytes))
            .ok_or_else(|| Error::Invalid(super::api::UNKNOWN_FORMAT.into()))?;
        if format == ArchiveFormat::Directory {
            return Err(Error::Invalid(
                "A directory dump cannot be uploaded; import it from its path on the Host".into(),
            ));
        }
        // The receipt is written first, so a retry after any failure or
        // restart finds the queued Task, or no Task and the file to queue.
        // Once queued the file is the import's: the worker removes it.
        let task_id = format!("event-{:032x}", rand::random::<u128>());
        let open = session.clone();
        session.task_id = Some(task_id.clone());
        write_session(&session)?;
        let give_back = || {
            let _ = write_session(&open);
        };
        remember_import(
            &state.store,
            &Import {
                task_id: task_id.clone(),
                database_application_id: app.id.clone(),
                connection_id: session.connection_id.clone(),
                consumer_application_id: c.consumer_application_id.clone(),
                file_name: session.file_name.clone(),
                file_bytes: session.size,
                created_at: audit::timestamp(),
                imported_objects: None,
            },
        )
        .await
        .inspect_err(|_| give_back())?;
        let queued = audit::EVENT_ID
            .scope(
                Some(task_id.clone()),
                tasks::enqueue(
                    &state,
                    "import",
                    audit::Subject::new("database", app.id.clone(), app.name.clone()),
                    tasks::Work::Postgres {
                        operation: Operation::Import {
                            connection_id: session.connection_id.clone(),
                            upload_id: session.id.clone(),
                            path: None,
                            format: Some(format),
                            all_or_nothing: session.all_or_nothing,
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
                give_back();
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

/// Opens a session again when its receipt names a Task that never got
/// queued, because the daemon stopped in between. A Task that is queued,
/// running or ended keeps the receipt. The caller holds the session's lock.
async fn reclaim<S: StateStore>(state: &AppState<S>, session: &mut Session) -> Result<(), Error> {
    let Some(task_id) = session.task_id.clone() else {
        return Ok(());
    };
    // The queue first: a Task leaves it only after its event says it ended,
    // so reading the event second cannot miss one that just finished.
    // A Task that ran took its file with it; when the bytes are gone, the
    // event only expired from history, and the import stays listed.
    let handed = tasks::is_queued(&state.store, &task_id).await?
        || state
            .store
            .get_audit_event(&task_id)
            .await?
            .and_then(|body| serde_json::from_str::<audit::Event>(&body).ok())
            .is_some_and(|event| event.status != "pending")
        || !bytes_path(&session.id).exists();
    if handed {
        return Ok(());
    }
    let _ = IMPORTS.remove(&state.store, &task_id).await;
    session.task_id = None;
    write_session(session)
}

/// Removes a session the Operator gave up on, file included, unless an
/// import it queued owns the file now.
async fn cancel<S: StateStore>(
    State(state): State<AppState<S>>,
    Path((id, session)): Path<(String, String)>,
) -> Response {
    let session = match read_session(&id, &session) {
        Ok(session) => session,
        Err(error) => return failure(error),
    };
    let lock = session_lock(&session.id);
    let _writing = lock.lock().await;
    match remove_unless_queued(&state, &session.id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => failure(error),
    }
}

/// Removes a session's record and its bytes, but leaves both to an import
/// it queued that is still waiting or running. The caller holds the
/// session's lock.
async fn remove_unless_queued<S: StateStore>(state: &AppState<S>, id: &str) -> Result<(), Error> {
    let record = std::fs::read(record_path(id))
        .ok()
        .and_then(|body| serde_json::from_slice::<Session>(&body).ok());
    if let Some(mut session) = record {
        reclaim(state, &mut session).await?;
        // A Task that ended without removing the file, as one a restart
        // interrupted, leaves it here to remove.
        if let Some(task_id) = &session.task_id
            && tasks::is_queued(&state.store, task_id).await?
        {
            // The receipt stays with the file, so a later sweep finds it
            // once the Task ends, even one a restart cut short.
            return Ok(());
        }
    }
    remove_session(id);
    Ok(())
}

/// Removes every session nobody wrote to for `ABANDONED_AFTER`, and any
/// stray file in session storage as old.
pub(crate) async fn sweep<S: StateStore>(state: &AppState<S>, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(sessions_dir()) else {
        return;
    };
    // A session's files: its record, a record being written, its bytes.
    let mut groups: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let id = name.split('.').next().unwrap_or_default().to_string();
        groups.entry(id).or_default().push(entry.path());
    }
    for (id, mut files) in groups {
        let lock = session_lock(&id);
        // A writer holding it is not abandoned; with the lock held, no one
        // can touch the files between this check and their removal.
        let Ok(_writing) = lock.try_lock() else {
            continue;
        };
        if valid_id(&id) {
            files.push(bytes_path(&id));
        }
        // A session's age is its last chunk's, or its start's before one came.
        let last = files
            .iter()
            .filter_map(|path| std::fs::metadata(path).and_then(|m| m.modified()).ok())
            .max();
        let abandoned = last
            .and_then(|last| now.duration_since(last).ok())
            .is_some_and(|age| age >= ABANDONED_AFTER);
        if !abandoned {
            continue;
        }
        if valid_id(&id)
            && let Err(error) = remove_unless_queued(state, &id).await
        {
            tracing::warn!(%error, "could not sweep an import upload");
            continue;
        }
        let _ = std::fs::remove_file(sessions_dir().join(format!("{id}.json.new")));
        if !valid_id(&id) {
            for path in files {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Sweeps abandoned sessions once an hour while the daemon runs.
pub(crate) fn spawn_sweep<S: StateStore>(state: AppState<S>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60 * 60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            sweep(&state, SystemTime::now()).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    use super::*;
    use crate::postgres::{progress_tests, test_runtime, tests::reference};

    async fn call(router: &Router, request: Request<Body>) -> (StatusCode, serde_json::Value) {
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap_or_default())
    }

    fn start_request(fingerprint: &str, size: usize) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/databases/provider/imports/uploads")
            .header("authorization", "Bearer fixture-key")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "connection_id": "reference", "name": "big.dump", "size": size,
                    "fingerprint": fingerprint, "format": "custom", "all_or_nothing": false
                })
                .to_string(),
            ))
            .unwrap()
    }

    fn chunk(id: &str, first: usize, last: usize, size: usize, body: Body) -> Request<Body> {
        Request::builder()
            .method("PUT")
            .uri(format!("/databases/provider/imports/uploads/{id}"))
            .header("authorization", "Bearer fixture-key")
            .header("content-range", format!("bytes {first}-{last}/{size}"))
            .body(body)
            .unwrap()
    }

    #[tokio::test]
    async fn an_interrupted_upload_resumes_where_it_stopped_and_imports() {
        let docker = Arc::new(test_runtime::PausedRemoval {
            postgres_ready: true,
            ..Default::default()
        });
        let (router, state) = progress_tests::fixture_with_router(docker.clone()).await;
        progress_tests::operation(
            &state,
            Operation::Provision {
                id: "provider".into(),
                recreate: false,
            },
        )
        .await;
        CONNECTIONS
            .upsert(&state.store, &reference("ready"))
            .await
            .unwrap();
        let dump: Vec<u8> = b"PGDMP"
            .iter()
            .copied()
            .chain((0..995).map(|n| (n % 251) as u8))
            .collect();
        let size = dump.len();
        let fingerprint = format!("{:032x}", rand::random::<u128>());
        let (status, started) = call(&router, start_request(&fingerprint, size)).await;
        assert_eq!(status, StatusCode::OK);
        let id = started["id"].as_str().unwrap().to_string();
        assert_eq!(started["received"], 0);
        let mode = |path: PathBuf| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(sessions_dir()), 0o700);
        assert_eq!(mode(bytes_path(&id)), 0o600);
        assert_eq!(mode(record_path(&id)), 0o600);

        // The connection drops halfway through the only chunk.
        let half = dump[..500].to_vec();
        let cut = futures_util::stream::iter([
            Ok(axum::body::Bytes::from(half)),
            Err(std::io::Error::other("connection reset")),
        ]);
        let (status, _) = call(
            &router,
            chunk(&id, 0, size - 1, size, Body::from_stream(cut)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Picking the same file again resumes the session from what it holds.
        let (_, resumed) = call(&router, start_request(&fingerprint, size)).await;
        assert_eq!(resumed["id"], id.as_str());
        assert_eq!(resumed["received"], 500);
        // Another file starts over.
        let (_, other) = call(&router, start_request("another-file", size)).await;
        assert_ne!(other["id"], id.as_str());
        assert_eq!(other["received"], 0);

        // A chunk that overlaps, leaves a gap or passes the size is refused.
        let (status, refused) =
            call(&router, chunk(&id, 0, 9, size, Body::from(vec![0; 10]))).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(refused["received"], 500);
        let (status, _) = call(&router, chunk(&id, 600, 609, size, Body::from(vec![0; 10]))).await;
        assert_eq!(status, StatusCode::CONFLICT);
        let (status, _) = call(
            &router,
            chunk(&id, 500, size, size + 1, Body::from(vec![0; 501])),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = call(&router, chunk(&id, 500, 509, size, Body::from(vec![0; 11]))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(received(&id), 500, "nothing past the range is written");

        let finish = || {
            Request::builder()
                .method("POST")
                .uri(format!("/databases/provider/imports/uploads/{id}/import"))
                .header("authorization", "Bearer fixture-key")
                .body(Body::empty())
                .unwrap()
        };
        let (status, _) = call(&router, finish()).await;
        assert_eq!(status, StatusCode::CONFLICT, "half a file does not import");

        // Only the rest is sent.
        let rest = dump[500..].to_vec();
        let (status, progress) =
            call(&router, chunk(&id, 500, size - 1, size, Body::from(rest))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(progress["received"], size);
        assert_eq!(std::fs::read(bytes_path(&id)).unwrap(), dump);

        let (status, accepted) = call(&router, finish()).await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let task = accepted["task_id"].as_str().unwrap();
        assert_eq!(
            progress_tests::finished(&state, task).await.status,
            "completed"
        );
        assert!(!bytes_path(&id).exists(), "the file went to the import");
        // A retry whose answer was lost gets the same import, not a second.
        let (status, again) = call(&router, finish()).await;
        assert_eq!(
            (status, &again["task_id"]),
            (StatusCode::ACCEPTED, &accepted["task_id"])
        );
        // The same file picked again is a new import.
        let (_, anew) = call(&router, start_request(&fingerprint, size)).await;
        assert_ne!(anew["id"], id.as_str());
        assert_eq!(anew["received"], 0);
        remove_session(&id);
        remove_session(anew["id"].as_str().unwrap());
        let import = IMPORTS.get(&state.store, task).await.unwrap().unwrap();
        assert_eq!(
            (import.file_name.as_str(), import.file_bytes),
            ("big.dump", size as u64)
        );
        let all_or_nothing =
            docker
                .requests
                .lock()
                .unwrap()
                .iter()
                .find_map(|request| match request {
                    runtime::Request::Restore { all_or_nothing, .. } => Some(*all_or_nothing),
                    _ => None,
                });
        assert_eq!(
            all_or_nothing,
            Some(false),
            "the choice reaches the restore"
        );
        remove_session(other["id"].as_str().unwrap());
    }

    #[tokio::test]
    async fn a_session_nobody_writes_to_for_a_day_is_removed() {
        let (router, state) =
            crate::postgres::tests::fixture_with_docker(Arc::new(crate::docker::FakeDocker::new()))
                .await;
        CONNECTIONS
            .upsert(&state.store, &reference("ready"))
            .await
            .unwrap();
        let mut ids = Vec::new();
        for _ in 0..2 {
            let fingerprint = format!("{:032x}", rand::random::<u128>());
            let (_, started) = call(&router, start_request(&fingerprint, 10)).await;
            let id = started["id"].as_str().unwrap().to_string();
            let (status, _) = call(&router, chunk(&id, 0, 4, 10, Body::from(vec![1; 5]))).await;
            assert_eq!(status, StatusCode::OK);
            ids.push(id);
        }
        let (old, fresh) = (&ids[0], &ids[1]);
        let day_ago = SystemTime::now() - ABANDONED_AFTER - Duration::from_secs(60);
        for path in [bytes_path(old), record_path(old)] {
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(day_ago)
                .unwrap();
        }
        sweep(&state, SystemTime::now()).await;
        assert!(!bytes_path(old).exists() && !record_path(old).exists());
        assert!(bytes_path(fresh).exists() && record_path(fresh).exists());
        remove_session(fresh);
    }
}
