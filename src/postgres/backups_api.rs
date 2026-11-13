//! The backup routes (ADR-0035): the Host's backup storage and maintenance
//! window, and each database's backups.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use super::{
    Error,
    api::failure,
    backups::{
        self, Archiving, Backup, BackupSettings, Kind, MaintenanceWindow, REPOSITORIES, Restore,
        SETTINGS, SettingsChange, Verification,
    },
    database,
    storage::{self, BackupStorage, STORAGE, STORAGE_ID},
};
use crate::{AppState, store::StateStore};

pub(crate) fn router<S: StateStore>() -> Router<AppState<S>> {
    Router::new()
        .route(
            "/backups/storage",
            get(storage_detail::<S>)
                .put(save_storage::<S>)
                .delete(remove_storage::<S>),
        )
        .route("/backups/storage/test", post(test_storage::<S>))
        .route("/backups", get(summary::<S>))
        .route(
            "/maintenance-window",
            get(window_detail::<S>).put(save_window::<S>),
        )
        .route(
            "/databases/{id}/backups/settings",
            get(settings_detail::<S>).put(save_settings::<S>),
        )
        .route(
            "/databases/{id}/backups",
            get(list::<S>).post(back_up_now::<S>),
        )
        .route("/databases/{id}/backups/restore", post(restore::<S>))
        .route("/databases/{id}/backups/verify", post(verify::<S>))
}

fn answer<T: Serialize>(result: Result<T, Error>) -> Response {
    match result {
        Ok(value) => crate::no_store(Json(value).into_response()),
        Err(error) => failure(error),
    }
}

fn accepted(result: Result<String, Error>) -> Response {
    match result {
        Ok(task_id) => crate::accepted_task(Ok(task_id)),
        Err(error) => failure(error),
    }
}

/// The storage as the API shows it: everything but the secret key.
#[derive(Serialize)]
struct StorageView {
    configured: bool,
    endpoint: String,
    region: String,
    bucket: String,
    prefix: String,
    path_style: bool,
    verify_tls: bool,
    access_key_id: String,
    has_secret: bool,
}

fn storage_view(storage: Option<BackupStorage>) -> StorageView {
    match storage {
        Some(storage) => StorageView {
            configured: true,
            endpoint: storage.endpoint,
            region: storage.region,
            bucket: storage.bucket,
            prefix: storage.prefix,
            path_style: storage.path_style,
            verify_tls: storage.verify_tls,
            access_key_id: storage.access_key_id,
            has_secret: !storage.secret_access_key.is_empty(),
        },
        None => StorageView {
            configured: false,
            endpoint: String::new(),
            region: String::new(),
            bucket: String::new(),
            prefix: String::new(),
            path_style: true,
            verify_tls: true,
            access_key_id: String::new(),
            has_secret: false,
        },
    }
}

async fn storage_detail<S: StateStore>(State(state): State<AppState<S>>) -> Response {
    answer(
        STORAGE
            .get(&state.store, STORAGE_ID)
            .await
            .map(storage_view)
            .map_err(Error::Store),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StorageRequest {
    #[serde(default)]
    endpoint: String,
    region: String,
    bucket: String,
    #[serde(default)]
    prefix: String,
    #[serde(default = "path_style_default")]
    path_style: bool,
    #[serde(default = "storage::verify_tls_default")]
    verify_tls: bool,
    access_key_id: String,
    /// Left out, the saved secret stays.
    #[serde(default)]
    secret_access_key: Option<String>,
}

fn path_style_default() -> bool {
    true
}

/// What the Operator typed as a storage, with the saved secret when the
/// request leaves it out.
async fn requested(
    store: &impl StateStore,
    request: StorageRequest,
) -> Result<BackupStorage, Error> {
    let secret = match request
        .secret_access_key
        .filter(|secret| !secret.is_empty())
    {
        Some(secret) => secret,
        None => STORAGE
            .get(store, STORAGE_ID)
            .await?
            .map(|saved| saved.secret_access_key)
            .unwrap_or_default(),
    };
    let storage = BackupStorage {
        id: STORAGE_ID.into(),
        endpoint: request.endpoint.trim().trim_end_matches('/').to_owned(),
        region: request.region.trim().to_owned(),
        bucket: request.bucket.trim().to_owned(),
        prefix: storage::clean_prefix(&request.prefix),
        path_style: request.path_style,
        verify_tls: request.verify_tls,
        access_key_id: request.access_key_id.trim().to_owned(),
        secret_access_key: secret,
    };
    storage::validate(&storage)?;
    Ok(storage)
}

async fn save_storage<S: StateStore>(
    State(state): State<AppState<S>>,
    Json(request): Json<StorageRequest>,
) -> Response {
    let result = async {
        let storage = requested(&state.store, request).await?;
        let _move = backups::STORAGE_MOVE.lock().await;
        // A server keeps archiving to the repository it backed up to: a new
        // location has no stanza and no base backup for it. Keys may change.
        let moved = STORAGE
            .get(&state.store, STORAGE_ID)
            .await?
            .is_some_and(|saved| {
                (&saved.endpoint, &saved.bucket, &saved.prefix)
                    != (&storage.endpoint, &storage.bucket, &storage.prefix)
            });
        if moved
            && SETTINGS
                .list(&state.store)
                .await?
                .iter()
                .any(|settings| settings.enabled || settings.change.is_some())
        {
            return Err(Error::Conflict(
                "Turn backups off on every server before changing the endpoint, bucket or prefix"
                    .into(),
            ));
        }
        STORAGE.upsert(&state.store, &storage).await?;
        // pgBackRest reads its file on every run: no restart.
        backups::rewrite_confs(&state.store).await?;
        Ok(storage_view(Some(storage)))
    }
    .await;
    answer(result)
}

/// Tests the storage in the request, or the saved one when there is no body.
/// A storage that fails answers `422` with what the bucket said.
async fn test_storage<S: StateStore>(
    State(state): State<AppState<S>>,
    request: Option<Json<StorageRequest>>,
) -> Response {
    let result = async {
        let storage = match request {
            Some(Json(request)) => requested(&state.store, request).await?,
            None => storage::configured(&state.store).await?,
        };
        storage::test(&storage).await
    }
    .await;
    match result {
        Ok(()) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(Error::Unavailable(reason)) => failure(Error::Unsupported(reason)),
        Err(error) => failure(error),
    }
}

async fn remove_storage<S: StateStore>(State(state): State<AppState<S>>) -> Response {
    let result = async {
        if SETTINGS
            .list(&state.store)
            .await?
            .iter()
            .any(|settings| settings.enabled)
        {
            return Err(Error::Conflict(
                "Turn backups off on every server before removing the backup storage".into(),
            ));
        }
        STORAGE.remove(&state.store, STORAGE_ID).await?;
        Ok(())
    }
    .await;
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => failure(error),
    }
}

#[derive(Serialize)]
struct WindowView {
    #[serde(flatten)]
    window: MaintenanceWindow,
    /// The Host's offset from UTC now, such as `-03:00`.
    utc_offset: String,
}

fn window_view(window: MaintenanceWindow) -> WindowView {
    let offset = backups::local_now().offset();
    let (hours, minutes, _) = offset.as_hms();
    WindowView {
        window,
        utc_offset: format!(
            "{}{:02}:{:02}",
            if offset.is_negative() { '-' } else { '+' },
            hours.unsigned_abs(),
            minutes.unsigned_abs()
        ),
    }
}

async fn window_detail<S: StateStore>(State(state): State<AppState<S>>) -> Response {
    answer(
        backups::window(&state.store)
            .await
            .map(window_view)
            .map_err(Error::Store),
    )
}

async fn save_window<S: StateStore>(
    State(state): State<AppState<S>>,
    Json(window): Json<MaintenanceWindow>,
) -> Response {
    answer(
        backups::set_window(&state.store, window)
            .await
            .map(window_view),
    )
}

/// The line the console shows when a server's backups need the Operator.
fn problem(settings: &BackupSettings, archiving: &Archiving) -> Option<String> {
    if settings.enabled && archiving.failing {
        return Some(
            "WAL is not reaching the backup storage. PostgreSQL keeps it on the data volume and retries, so the disk fills until the bucket answers".into(),
        );
    }
    settings.error.clone()
}

#[derive(Serialize)]
struct SettingsView {
    #[serde(flatten)]
    settings: BackupSettings,
    /// When the next scheduled backup starts; null when backups are off.
    next_run_at: Option<String>,
    next_kind: Option<Kind>,
    storage_configured: bool,
    archiving: Archiving,
    verification: Option<Verification>,
    problem: Option<String>,
    /// The Task that turns backups on or off, when this request queued it.
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
}

async fn settings_view(
    store: &impl StateStore,
    settings: BackupSettings,
    task_id: Option<String>,
) -> Result<SettingsView, Error> {
    let window = backups::window(store).await?;
    let repository = backups::repository(store, &settings.database_application_id).await?;
    let next = backups::next_run(&settings, window, backups::local_now());
    Ok(SettingsView {
        next_run_at: next.as_ref().map(|next| next.0.clone()),
        next_kind: next.map(|next| next.1),
        storage_configured: STORAGE.exists(store, STORAGE_ID).await?,
        problem: problem(&settings, &repository.archiving),
        archiving: repository.archiving,
        verification: repository.verification,
        settings,
        task_id,
    })
}

async fn settings_detail<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
) -> Response {
    let result = async {
        database(&state.store, &id).await?;
        settings_view(
            &state.store,
            backups::settings(&state.store, &id).await?,
            None,
        )
        .await
    }
    .await;
    answer(result)
}

async fn save_settings<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
    Json(change): Json<SettingsChange>,
) -> Response {
    let result = async {
        let (settings, task_id) = backups::change_settings(&state, &id, change).await?;
        settings_view(&state.store, settings, task_id).await
    }
    .await;
    answer(result)
}

async fn list<S: StateStore>(State(state): State<AppState<S>>, Path(id): Path<String>) -> Response {
    let result = async {
        database(&state.store, &id).await?;
        Ok(backups::repository(&state.store, &id).await?.backups)
    }
    .await;
    answer(result)
}

/// One row per server that has backups on or a repository: what the list of
/// databases shows as its last backup, and whether something is wrong.
#[derive(Serialize)]
struct Summary {
    database_application_id: String,
    enabled: bool,
    last: Option<Backup>,
    problem: Option<String>,
}

async fn summary<S: StateStore>(State(state): State<AppState<S>>) -> Response {
    let result = async {
        let store = &state.store;
        let mut rows = Vec::new();
        for settings in SETTINGS.list(store).await? {
            let repository = REPOSITORIES
                .get(store, &settings.database_application_id)
                .await?
                .unwrap_or_default();
            rows.push(Summary {
                problem: problem(&settings, &repository.archiving),
                last: repository.backups.into_iter().next(),
                enabled: settings.enabled,
                database_application_id: settings.database_application_id,
            });
        }
        Ok::<_, Error>(rows)
    }
    .await;
    answer(result)
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupRequest {
    /// Left out, an incremental backup.
    #[serde(default)]
    kind: Option<Kind>,
}

async fn back_up_now<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    // The console sends a JSON content type with no body, which the `Json`
    // extractor refuses. No body means the default kind.
    let request = if body.iter().all(u8::is_ascii_whitespace) {
        BackupRequest::default()
    } else {
        match serde_json::from_slice::<BackupRequest>(&body) {
            Ok(request) => request,
            Err(error) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(crate::error::ErrorReport::plain(format!(
                        "The backup request is not valid JSON: {error}"
                    ))),
                )
                    .into_response();
            }
        }
    };
    let kind = request.kind.unwrap_or(Kind::Incr);
    accepted(backups::start(&state, &id, kind, false).await)
}

async fn restore<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
    Json(request): Json<Restore>,
) -> Response {
    match backups::start_restore(&state, &id, request).await {
        Ok((id, task_id)) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({"id": id, "task_id": task_id})),
        )
            .into_response(),
        Err(error) => failure(error),
    }
}

async fn verify<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
) -> Response {
    accepted(backups::start_verify(&state, &id).await)
}
