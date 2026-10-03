//! A small, durable history of authenticated mutations and task results.
//! Only action metadata, safe progress observations and failures are recorded. Request
//! bodies, credentials, environment values and build output never enter the
//! audit history.
//!
//! An event is the record of a task: it is `pending` from the moment the
//! request is accepted, `running` once the scheduler starts it, and
//! `completed` or `failed` when the work is done (see `tasks`).

use crate::{
    AppState,
    collection::{IMAGES, MACHINES},
    error::ErrorReport,
    store::{AuditRow, StateStore, StoreError},
};
use axum::{
    Json,
    body::{Body, to_bytes},
    extract::{FromRequestParts, MatchedPath, RawPathParams, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

tokio::task_local! { pub static ACTOR: Option<String>; }
tokio::task_local! { pub static EVENT_ID: Option<String>; }

pub fn event_id() -> Option<String> {
    EVENT_ID.try_with(Clone::clone).ok().flatten()
}

pub fn actor() -> Option<String> {
    ACTOR.try_with(Clone::clone).ok().flatten()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subject {
    pub kind: String,
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
}

impl Subject {
    pub fn new(kind: &str, id: impl Into<String>, name: impl Into<String>) -> Self {
        Subject {
            kind: kind.into(),
            id: id.into(),
            name: name.into(),
            available: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub id: String,
    pub action: String,
    pub status: String,
    pub occurred_at: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    pub api_name: Option<String>,
    pub description: String,
    pub subject: Subject,
    /// Why a `failed` task failed, in the shape every error leaves the API in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorReport>,
    /// What a `configure` changed, setting by setting, so the history says
    /// not only that something was changed but from what to what.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes: Option<Vec<Change>>,
    /// Stages the worker actually began, with bounded, credential-free output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<RunProgress>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RunProgress {
    pub stages: Vec<RunStage>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStage {
    pub id: String,
    pub label: String,
    pub status: String,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    pub output: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorReport>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub setting: String,
    pub from: String,
    pub to: String,
}

pub fn event(
    action: impl Into<String>,
    subject: Subject,
    status: &str,
    api_name: Option<String>,
    description: impl Into<String>,
) -> Event {
    let at = timestamp();
    Event {
        id: event_id().unwrap_or_else(|| format!("event-{:032x}", rand::random::<u128>())),
        action: action.into(),
        status: status.into(),
        occurred_at: at.clone(),
        started_at: if status == "pending" {
            None
        } else {
            Some(at.clone())
        },
        finished_at: if is_terminal(status) {
            Some(at.clone())
        } else {
            None
        },
        updated_at: Some(at),
        api_name,
        description: description.into(),
        subject,
        error: None,
        changes: None,
        progress: None,
    }
}

fn is_terminal(status: &str) -> bool {
    matches!(status, "completed" | "failed")
}

/// A task only moves forward: `pending`, `running`, then `completed` or
/// `failed`. A write that would move it back is a late acknowledgement or a
/// duplicate result, and is ignored.
fn rank(status: &str) -> u8 {
    match status {
        "pending" => 0,
        "running" => 1,
        _ => 2,
    }
}
pub(crate) fn timestamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:09}Z",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.nanosecond()
    )
}

#[derive(Default)]
pub struct Journal {
    write: Mutex<()>,
}
impl Journal {
    /// Records `event`, merging it into the row with the same id, then drops
    /// every event last updated more than `retention` ago. Retention rides on
    /// every write so a changed setting applies on the next event.
    pub async fn upsert<S: StateStore>(
        &self,
        store: &S,
        event: Event,
        retention: std::time::Duration,
    ) -> Result<(), StoreError> {
        let _guard = self.write.lock().await;
        let mut merged = match store.get_audit_event(&event.id).await? {
            Some(body) => {
                let mut existing = parse(&body)?;
                // A fast worker can finish before the HTTP handler returns 202.
                // The later acknowledgement must not overwrite its final result.
                if rank(&event.status) > rank(&existing.status) {
                    existing.status = event.status;
                    existing.description = event.description;
                    existing.updated_at = event.updated_at;
                    existing.finished_at = event.finished_at;
                    existing.error = event.error;
                }
                if event.changes.is_some() {
                    existing.changes = event.changes;
                }
                if !event.subject.id.is_empty()
                    && !(existing.subject.kind == "database" && event.subject.kind == "application")
                {
                    existing.subject = event.subject;
                }
                if existing.started_at.is_none() {
                    existing.started_at = event.started_at;
                }
                existing
            }
            None => event,
        };
        // Restart recovery and cancelled workers finish the current stage too.
        // A late stage update cannot leave a terminal Task looking active.
        if is_terminal(&merged.status)
            && let Some(progress) = &mut merged.progress
        {
            for stage in &mut progress.stages {
                if stage.status == "running" {
                    stage.status = merged.status.clone();
                    stage.finished_at = merged.finished_at.clone();
                    stage.error = merged.error.clone();
                }
            }
        }
        store.put_audit_event(&row(&merged)?).await?;
        store.prune_audit_events(&cutoff(retention)).await?;
        Ok(())
    }

    /// Changes only a running Task. Stage writes share the outcome lock, so a
    /// late worker cannot revive an interrupted or completed operation.
    pub(crate) async fn stage<S: StateStore>(
        &self,
        store: &S,
        event_id: &str,
        id: &str,
        label: &str,
        outcome: Option<Result<&str, &ErrorReport>>,
    ) -> Result<(), StoreError> {
        let _guard = self.write.lock().await;
        let Some(body) = store.get_audit_event(event_id).await? else {
            return Ok(());
        };
        let mut event = parse(&body)?;
        if event.status != "running" {
            return Ok(());
        }
        let at = timestamp();
        let progress = event.progress.get_or_insert_with(Default::default);
        if let Some(outcome) = outcome {
            let Some(stage) = progress
                .stages
                .iter_mut()
                .rev()
                .find(|stage| stage.id == id && stage.status == "running")
            else {
                return Ok(());
            };
            stage.finished_at = Some(at.clone());
            match outcome {
                Ok(output) => {
                    stage.status = "completed".into();
                    if !output.is_empty() {
                        stage.output.push(output.chars().take(1024).collect());
                    }
                }
                Err(error) => {
                    stage.status = "failed".into();
                    stage.error = Some(error.clone());
                }
            }
        } else {
            if progress.stages.len() >= 32
                || progress
                    .stages
                    .iter()
                    .any(|stage| stage.status == "running")
            {
                return Ok(());
            }
            progress.stages.push(RunStage {
                id: id.chars().take(64).collect(),
                label: label.chars().take(128).collect(),
                status: "running".into(),
                started_at: at.clone(),
                finished_at: None,
                output: vec![format!("{label}.").chars().take(1024).collect()],
                error: None,
            });
            event.description = label.chars().take(128).collect();
        }
        event.updated_at = Some(at);
        store.put_audit_event(&row(&event)?).await
    }
}

fn row(event: &Event) -> Result<AuditRow, StoreError> {
    Ok(AuditRow {
        id: event.id.clone(),
        status: event.status.clone(),
        subject_kind: event.subject.kind.clone(),
        subject_id: event.subject.id.clone(),
        occurred_at: event.occurred_at.clone(),
        updated_at: event
            .updated_at
            .clone()
            .unwrap_or_else(|| event.occurred_at.clone()),
        body: serde_json::to_string(event).map_err(|e| StoreError::Serialize(e.to_string()))?,
    })
}

fn parse(body: &str) -> Result<Event, StoreError> {
    serde_json::from_str(body)
        .map_err(|e| StoreError::Serialize(format!("could not read audit history: {e}")))
}

/// The timestamp `retention` before now, in the text form events carry, so
/// the store compares it as text.
fn cutoff(retention: std::time::Duration) -> String {
    let at = time::OffsetDateTime::now_utc() - retention;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:09}Z",
        at.year(),
        at.month() as u8,
        at.day(),
        at.hour(),
        at.minute(),
        at.second(),
        at.nanosecond()
    )
}

pub(crate) async fn read<S: StateStore>(store: &S) -> Result<Vec<Event>, StoreError> {
    store
        .list_audit_events()
        .await?
        .iter()
        .map(|body| parse(body))
        .collect()
}

pub(crate) async fn record<S: StateStore>(state: &AppState<S>, event: Event) {
    let retention = crate::settings::audit_events_max_age(state).await;
    if let Err(error) = state.audit.upsert(&state.store, event, retention).await {
        tracing::error!(%error, "Could not persist audit event");
    }
}

pub(crate) async fn list<S: StateStore>(State(state): State<AppState<S>>) -> Response {
    match read(&state.store).await {
        Ok(mut events) => {
            let apps = state.store.list_applications().await.ok();
            let databases = crate::postgres::DATABASES.list(&state.store).await.ok();
            let mut known_databases: std::collections::HashSet<String> = events
                .iter()
                .filter(|event| event.subject.kind == "database")
                .map(|event| event.subject.id.clone())
                .collect();
            if let Some(databases) = &databases {
                known_databases.extend(
                    databases
                        .iter()
                        .map(|database| database.application_id.clone()),
                );
            }
            let keys = state.store.list_api_keys().await.ok();
            let machines: Option<Vec<String>> = MACHINES
                .list(&state.store)
                .await
                .ok()
                .map(|rows| rows.into_iter().map(|row| row.id).collect());
            let images: Option<Vec<String>> = IMAGES
                .list(&state.store)
                .await
                .ok()
                .map(|rows| rows.into_iter().map(|row| row.id).collect());
            let records: Option<Vec<String>> = crate::dns_records::load(&state.store)
                .await
                .ok()
                .map(|records| records.iter().map(|record| record.key()).collect());
            for event in &mut events {
                let id = &event.subject.id;
                // Older releases recorded database work as Application work.
                // Retained database events also identify a deleted provider.
                // Normalize the response without rewriting stored history.
                if event.subject.kind == "application" && known_databases.contains(id) {
                    event.subject.kind = "database".into();
                }
                event.subject.available = match event.subject.kind.as_str() {
                    "database" => databases.as_ref().map(|databases| {
                        databases
                            .iter()
                            .any(|database| &database.application_id == id)
                    }),
                    "application" => apps
                        .as_ref()
                        .map(|apps| apps.iter().any(|app| &app.id == id)),
                    "api-key" => keys
                        .as_ref()
                        .map(|keys| keys.iter().any(|key| &key.id == id)),
                    "virtual-machine" => machines.as_ref().map(|ids| ids.contains(id)),
                    "custom-image" => images.as_ref().map(|ids| ids.contains(id)),
                    "settings" => Some(true),
                    "dns-record" => records.as_ref().map(|ids| ids.contains(id)),
                    _ => Some(false),
                };
            }
            events.sort_by(|a, b| {
                b.updated_at
                    .as_ref()
                    .unwrap_or(&b.occurred_at)
                    .cmp(a.updated_at.as_ref().unwrap_or(&a.occurred_at))
            });
            Json(events).into_response()
        }
        Err(error) => crate::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

fn action(method: &str, route: &str, body: &Value) -> Option<(&'static str, &'static str)> {
    Some(match (method, route) {
        ("POST", "/apps") => ("create", "application"),
        ("PUT", "/apps/id/{id}") => ("configure", "application"),
        ("POST", "/apps/id/{id}/deploy-trigger")
        | ("DELETE", "/apps/id/{id}/deploy-trigger")
        | ("POST", "/apps/id/{id}/deployments/{deployment}/restore") => {
            ("configure", "application")
        }
        ("POST", "/apps/id/{id}/start") => ("start", "application"),
        ("POST", "/apps/id/{id}/stop") => ("stop", "application"),
        ("POST", "/apps/id/{id}/restart") => ("restart", "application"),
        ("DELETE", "/apps/{name}") | ("DELETE", "/apps/id/{id}") => ("delete", "application"),
        ("POST", "/apps/{name}/env") | ("POST", "/apps/id/{id}/env") => {
            ("configure", "application")
        }
        ("DELETE", "/apps/{name}/env/{key}") | ("DELETE", "/apps/id/{id}/env/{key}") => {
            ("configure", "application")
        }
        ("POST", "/environments") => ("create", "virtual-machine"),
        ("PUT", "/environments/{id}") => ("configure", "virtual-machine"),
        ("POST", "/environments/{id}/actions") => (
            machine_action(body.get("action").and_then(Value::as_str).unwrap_or("")),
            "virtual-machine",
        ),
        ("POST", "/custom-images") => (
            if body.get("id").and_then(Value::as_str).is_some() {
                "configure"
            } else {
                "create"
            },
            "custom-image",
        ),
        ("DELETE", "/custom-images/{id}") => ("delete", "custom-image"),
        ("POST", "/api-keys") => ("create", "api-key"),
        ("DELETE", "/api-keys/{id}") => ("delete", "api-key"),
        ("POST", "/dns/records") => ("create", "dns-record"),
        ("PUT", "/dns/records/{name}/{type}") => ("configure", "dns-record"),
        ("DELETE", "/dns/records/{name}/{type}") => ("delete", "dns-record"),
        ("PUT", "/settings") => ("configure", "settings"),
        _ => return None,
    })
}
pub fn machine_action(action: &str) -> &'static str {
    match action.trim().to_ascii_lowercase().as_str() {
        "create" => "create",
        "start" => "start",
        "stop" => "stop",
        "restart" => "restart",
        "retry" => "configure",
        "delete" => "delete",
        "bootstrap" => "configure",
        "update" | "apply_update" => "configure",
        _ => "configure",
    }
}
fn field(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .chars()
        .take(256)
        .collect()
}

async fn subject<S: StateStore>(
    state: &AppState<S>,
    kind: &str,
    route: &str,
    path: &str,
    decoded_name: Option<&str>,
    body: &Value,
) -> Subject {
    let parts: Vec<_> = path.trim_matches('/').split('/').collect();
    let mut id = if route.contains("{id}") {
        parts[if kind == "application" { 2 } else { 1 }].to_owned()
    } else {
        String::new()
    };
    let mut name = field(body, "/name");
    if kind == "application" {
        let path_name = decoded_name.unwrap_or(parts.get(1).copied().unwrap_or_default());
        let record = if route.contains("{name}") {
            state
                .store
                .find_application_by_name(path_name)
                .await
                .ok()
                .flatten()
        } else if !id.is_empty() {
            state.store.get_application(&id).await.ok().flatten()
        } else {
            None
        };
        if route.contains("{name}") && name.is_empty() {
            name = path_name.to_owned();
        }
        if let Some(record) = record {
            id = record.id;
            name = record.name;
        }
    } else if kind == "virtual-machine" || kind == "custom-image" {
        if id.is_empty() {
            id = field(body, "/id");
        }
        let (found, pointer) = if kind == "virtual-machine" {
            (
                MACHINES
                    .get(&state.store, &id)
                    .await
                    .ok()
                    .flatten()
                    .map(|machine| machine.config.name),
                "/config/name",
            )
        } else {
            (
                IMAGES
                    .get(&state.store, &id)
                    .await
                    .ok()
                    .flatten()
                    .map(|image| image.recipe.name),
                "/recipe/name",
            )
        };
        name = found
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| field(body, pointer));
    } else if kind == "settings" {
        let (setting, label) = crate::settings::subject(body);
        id = setting.into();
        name = label.into();
    } else if kind == "api-key" {
        name = state
            .store
            .list_api_keys()
            .await
            .ok()
            .and_then(|keys| keys.into_iter().find(|key| key.id == id))
            .map(|key| key.label)
            .unwrap_or_else(|| field(body, "/label"));
    } else if kind == "dns-record" {
        // The key as the API would normalize it, so `nas.home.lan` and `NAS`
        // land on the same subject as `nas`. A name that does not parse is
        // kept as typed: the event says what was attempted.
        let (typed_name, typed_type) = if route.contains("{name}") {
            (parts[2].to_owned(), parts[3].to_owned())
        } else {
            (field(body, "/name"), field(body, "/type"))
        };
        let suffix = state
            .store
            .get_state("dns_suffix")
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        name = crate::dns_records::parse_name(&typed_name, &suffix)
            .unwrap_or_else(|_| typed_name.trim().to_ascii_lowercase());
        id = crate::dns_records::key(&name, &typed_type.trim().to_ascii_uppercase());
    }
    if name.is_empty() {
        name = if id.is_empty() {
            "Unknown resource".into()
        } else {
            id.clone()
        };
    }
    let kind = if kind == "application"
        && crate::postgres::DATABASES
            .exists(&state.store, &id)
            .await
            .unwrap_or(false)
    {
        "database"
    } else {
        kind
    };
    Subject {
        kind: kind.into(),
        id,
        name,
        available: None,
    }
}

pub(crate) async fn capture<S: StateStore>(
    State(state): State<AppState<S>>,
    req: Request,
    next: Next,
) -> Response {
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_default();
    let method = req.method().as_str().to_owned();
    if action(&method, &route, &Value::Null).is_none() {
        return next.run(req).await;
    }
    let path = req.uri().path().to_owned();
    let (mut parts, body) = req.into_parts();
    // The exact-name routes remain supported. Use the router's decoded value
    // so punctuation and Unicode identify the same Application in history.
    let parameters = RawPathParams::from_request_parts(&mut parts, &state)
        .await
        .ok();
    let decoded_name = parameters.as_ref().and_then(|parameters| {
        parameters
            .iter()
            .find_map(|(key, value)| (key == "name").then_some(value))
    });
    let bytes = match to_bytes(body, 2 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let payload: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let (action, kind) = action(&method, &route, &payload).unwrap();
    let subject = subject(&state, kind, &route, &path, decoded_name, &payload).await;
    let mut entry = event(action, subject, "pending", actor(), "Operation queued.");
    let retention = crate::settings::audit_events_max_age(&state).await;
    if let Err(error) = state
        .audit
        .upsert(&state.store, entry.clone(), retention)
        .await
    {
        return crate::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    let response = EVENT_ID
        .scope(
            Some(entry.id.clone()),
            next.run(Request::from_parts(parts, Body::from(bytes))),
        )
        .await;
    let status = response.status();
    // These mutation endpoints return bounded JSON, never a stream. Rebuild the
    // original response unchanged after reading only the resource identity.
    let (parts, body) = response.into_parts();
    let bytes = match to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(error) => return crate::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    if status.is_success() {
        let result: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let id = field(&result, "/id");
        if !id.is_empty() {
            entry.subject.id = id;
        } else if kind == "dns-record" && result.get("name").is_some() {
            entry.subject.id =
                crate::dns_records::key(&field(&result, "/name"), &field(&result, "/type"));
        }
        for pointer in ["/name", "/config/name", "/recipe/name", "/label"] {
            let name = field(&result, pointer);
            if !name.is_empty() {
                entry.subject.name = name;
                break;
            }
        }
        // A handler that scheduled a task names it in the body. The task owns
        // every transition from here; this write only settles the subject.
        if status == StatusCode::ACCEPTED && field(&result, "/task_id") == entry.id {
            entry.status = "pending".into();
        } else {
            entry.status = "completed".into();
            entry.description = "Action completed.".into();
        }
    } else {
        entry.status = "failed".into();
        entry.description = format!("Request failed (HTTP {}).", status.as_u16());
        entry.error = serde_json::from_slice::<ErrorReport>(&bytes).ok();
    }
    if is_terminal(&entry.status) {
        let at = timestamp();
        entry.finished_at = Some(at.clone());
        entry.updated_at = Some(at);
    }
    record(&state, entry).await;
    Response::from_parts(parts, Body::from(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        build_app, docker::FakeDocker, metrics::Metrics, routes::FakeRoutes, store::FakeStateStore,
    };
    use axum::{Router, http::Request as HttpRequest};
    use serde_json::json;
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn setup() -> (Router, FakeStateStore) {
        let store = FakeStateStore::new();
        store.store_state("api_key", "root-secret").await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        (
            build_app(
                store.clone(),
                Arc::new(FakeDocker::new()),
                Arc::new(FakeRoutes::new()),
                Metrics::new(),
            ),
            store,
        )
    }
    async fn call(
        app: &Router,
        method: &str,
        path: &str,
        key: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let response = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method(method)
                    .uri(path)
                    .header("authorization", format!("Bearer {key}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn reads_and_unauthenticated_mutations_do_not_create_events() {
        let (app, store) = setup().await;
        assert_eq!(
            call(&app, "GET", "/events", "wrong", Value::Null).await.0,
            StatusCode::UNAUTHORIZED
        );
        call(&app, "DELETE", "/apps/missing", "wrong", Value::Null).await;
        call(&app, "GET", "/health", "root-secret", Value::Null).await;
        call(
            &app,
            "POST",
            "/compose/inspect",
            "root-secret",
            json!({"compose":""}),
        )
        .await;
        assert!(read(&store).await.unwrap().is_empty());
        call(&app, "DELETE", "/apps/missing", "root-secret", Value::Null).await;
        let events = read(&store).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].status, "failed");
        assert_eq!(events[0].api_name, None);
    }

    #[tokio::test]
    async fn custom_keys_authenticate_and_keep_actor_without_recording_secrets() {
        let (app, store) = setup().await;
        let (status, key) = call(
            &app,
            "POST",
            "/api-keys",
            "root-secret",
            json!({"label":"deploy-bot"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let token = key["key"].as_str().unwrap();
        let (status, created) = call(
            &app,
            "POST",
            "/apps",
            token,
            json!({"name":"blog","image":"nginx:alpine"}),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let task_id = created["task_id"].as_str().unwrap();
        // The task keeps its id and the key that asked, from pending through
        // running to the end.
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..200 {
            let entries = read(&store).await.unwrap();
            let task = entries.iter().find(|event| event.id == task_id).unwrap();
            assert_eq!(task.api_name.as_deref(), Some("deploy-bot"));
            assert_eq!(task.subject.id, created["id"].as_str().unwrap());
            seen.insert(task.status.clone());
            if task.status == "completed" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        assert!(seen.contains("completed"), "{seen:?}");
        let entries = read(&store).await.unwrap();
        let deploys: Vec<_> = entries
            .iter()
            .filter(|e| e.subject.kind == "application")
            .collect();
        assert_eq!(deploys.len(), 1);
        assert_eq!(deploys[0].id, task_id);
        call(
            &app,
            "POST",
            "/apps/blog/env",
            token,
            json!({"key":"PASSWORD", "value":"sensitive-env-value"}),
        )
        .await;
        let stored = store.list_audit_events().await.unwrap().join("\n");
        for secret in [token, "root-secret", "sensitive-env-value", "PASSWORD"] {
            assert!(!stored.contains(secret));
        }
        call(
            &app,
            "DELETE",
            &format!("/api-keys/{}", key["id"].as_str().unwrap()),
            "root-secret",
            Value::Null,
        )
        .await;
        assert_eq!(
            call(&app, "GET", "/events", token, Value::Null).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert!(
            !store
                .get_state(&format!("api_key_digest:{}", key["id"].as_str().unwrap()))
                .await
                .unwrap()
                .unwrap()
                .contains(token)
        );
    }

    #[tokio::test]
    async fn concurrent_actions_keep_every_event() {
        let (app, store) = setup().await;
        let mut tasks = vec![];
        for index in 0..20 {
            let app = app.clone();
            tasks.push(tokio::spawn(async move {
                call(
                    &app,
                    "POST",
                    "/api-keys",
                    "root-secret",
                    json!({"label":format!("client-{index}")}),
                )
                .await
            }));
        }
        for task in tasks {
            assert_eq!(task.await.unwrap().0, StatusCode::CREATED);
        }
        assert_eq!(read(&store).await.unwrap().len(), 20);
    }

    #[test]
    fn mutations_use_only_the_six_requested_actions() {
        for (method, route, expected) in [
            ("POST", "/apps", "create"),
            ("DELETE", "/apps/{name}", "delete"),
            ("POST", "/apps/id/{id}/stop", "stop"),
            ("POST", "/apps/id/{id}/start", "start"),
            ("POST", "/apps/id/{id}/restart", "restart"),
            ("PUT", "/apps/id/{id}", "configure"),
            ("POST", "/apps/{name}/env", "configure"),
            ("DELETE", "/apps/{name}/env/{key}", "configure"),
            ("POST", "/environments", "create"),
            ("PUT", "/environments/{id}", "configure"),
            ("POST", "/custom-images", "create"),
            ("DELETE", "/custom-images/{id}", "delete"),
            ("POST", "/api-keys", "create"),
            ("DELETE", "/api-keys/{id}", "delete"),
            ("POST", "/dns/records", "create"),
            ("PUT", "/dns/records/{name}/{type}", "configure"),
            ("DELETE", "/dns/records/{name}/{type}", "delete"),
        ] {
            assert_eq!(action(method, route, &Value::Null).unwrap().0, expected);
        }
        for verb in ["create", "delete", "stop", "start", "restart"] {
            assert_eq!(
                action(
                    "POST",
                    "/environments/{id}/actions",
                    &json!({"action":verb})
                )
                .unwrap()
                .0,
                verb
            );
        }
        assert_eq!(
            action("POST", "/custom-images", &json!({"id":"existing"}))
                .unwrap()
                .0,
            "configure"
        );
    }
    const RETENTION: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 3600);

    fn sample(status: &str, description: &str) -> Event {
        event(
            "create",
            Subject {
                kind: "virtual-machine".into(),
                id: "vm-1".into(),
                name: "machine".into(),
                available: None,
            },
            status,
            None,
            description,
        )
    }

    #[tokio::test]
    async fn late_acknowledgement_cannot_revert_a_completed_event() {
        let store = FakeStateStore::new();
        let journal = Journal::default();
        let start = sample("running", "Operation started.");
        journal
            .upsert(&store, start.clone(), RETENTION)
            .await
            .unwrap();
        let mut finish = sample("completed", "Virtual machine operation completed.");
        finish.id = start.id.clone();
        journal
            .upsert(&store, finish.clone(), RETENTION)
            .await
            .unwrap();
        journal
            .upsert(&store, start.clone(), RETENTION)
            .await
            .unwrap();
        let events = read(&store).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].status, "completed");
        assert_eq!(events[0].started_at, start.started_at);
        assert_eq!(events[0].finished_at, finish.finished_at);
        assert_eq!(events[0].updated_at, finish.updated_at);
    }

    #[tokio::test]
    async fn a_task_only_moves_forward() {
        let store = FakeStateStore::new();
        let journal = Journal::default();
        let queued = sample("pending", "Operation queued.");
        journal
            .upsert(&store, queued.clone(), RETENTION)
            .await
            .unwrap();
        assert_eq!(read(&store).await.unwrap()[0].started_at, None);
        let mut running = sample("running", "Operation is running.");
        running.id = queued.id.clone();
        journal
            .upsert(&store, running.clone(), RETENTION)
            .await
            .unwrap();
        journal
            .upsert(&store, queued.clone(), RETENTION)
            .await
            .unwrap();
        let events = read(&store).await.unwrap();
        assert_eq!(events[0].status, "running");
        assert_eq!(events[0].started_at, running.started_at);
        let mut failed = sample("failed", "Action failed.");
        failed.id = queued.id.clone();
        failed.error = Some(ErrorReport::plain("no such image"));
        journal
            .upsert(&store, failed.clone(), RETENTION)
            .await
            .unwrap();
        journal.upsert(&store, running, RETENTION).await.unwrap();
        let events = read(&store).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].status, "failed");
        assert_eq!(events[0].error, failed.error);
        assert_eq!(events[0].finished_at, failed.finished_at);
    }

    #[tokio::test]
    async fn interrupted_progress_finishes_and_late_stage_writes_cannot_revive_it() {
        let store = FakeStateStore::new();
        let journal = Journal::default();
        let mut running = sample("running", "PostgreSQL provisioning running.");
        running.subject = Subject::new("database", "provider", "fixture");
        journal
            .upsert(&store, running.clone(), RETENTION)
            .await
            .unwrap();
        journal
            .stage(
                &store,
                &running.id,
                "image",
                "Preparing PostgreSQL image",
                None,
            )
            .await
            .unwrap();
        journal
            .stage(
                &store,
                &running.id,
                "image",
                "",
                Some(Ok("Image is available.")),
            )
            .await
            .unwrap();
        journal
            .stage(
                &store,
                &running.id,
                "ready",
                "Waiting for PostgreSQL readiness",
                None,
            )
            .await
            .unwrap();
        let before = read(&store).await.unwrap().remove(0);
        assert_eq!(
            before.progress.as_ref().unwrap().stages[1].status,
            "running"
        );

        let mut failed = running.clone();
        failed.status = "failed".into();
        failed.finished_at = Some(timestamp());
        failed.error = Some(ErrorReport::plain(
            "The Platform restarted before this task finished.",
        ));
        journal
            .upsert(&store, failed.clone(), RETENTION)
            .await
            .unwrap();
        journal
            .stage(&store, &running.id, "ready", "", Some(Ok("Late success")))
            .await
            .unwrap();
        journal
            .stage(&store, &running.id, "late", "Must not start", None)
            .await
            .unwrap();
        running.subject.kind = "application".into();
        journal.upsert(&store, running, RETENTION).await.unwrap();
        let final_event = read(&store).await.unwrap().remove(0);
        let stages = &final_event.progress.unwrap().stages;
        assert_eq!(final_event.subject.kind, "database");
        assert_eq!(stages.len(), 2);
        assert_eq!(stages[0].status, "completed");
        assert_eq!(
            stages[0].output,
            ["Preparing PostgreSQL image.", "Image is available."]
        );
        assert_eq!(stages[1].status, "failed");
        assert_eq!(stages[1].error, failed.error);
        assert_eq!(stages[1].finished_at, failed.finished_at);
        assert_eq!(stages[1].output, ["Waiting for PostgreSQL readiness."]);
    }

    #[tokio::test]
    async fn progress_is_bounded_and_does_not_invent_future_stages() {
        let store = FakeStateStore::new();
        let journal = Journal::default();
        let running = sample("running", "Operation is running.");
        journal
            .upsert(&store, running.clone(), RETENTION)
            .await
            .unwrap();
        for number in 0..40 {
            let id = number.to_string();
            journal
                .stage(&store, &running.id, &id, "Actual step", None)
                .await
                .unwrap();
            journal
                .stage(&store, &running.id, &id, "", Some(Ok(&"x".repeat(2000))))
                .await
                .unwrap();
        }
        let event = read(&store).await.unwrap().remove(0);
        let stages = event.progress.unwrap().stages;
        assert_eq!(stages.len(), 32);
        assert!(stages.iter().all(|stage| stage.status == "completed"
            && stage.output.last().unwrap().len() == 1024
            && stage.output.len() == 2));
    }

    #[tokio::test]
    async fn historical_database_subjects_are_normalized_without_rewriting_history() {
        let (app, store) = setup().await;
        crate::postgres::DATABASES
            .upsert(
                &store,
                &crate::postgres::Database {
                    application_id: "provider".into(),
                    major: 17,
                    volume: "fixture_data".into(),
                    native_port: None,
                },
            )
            .await
            .unwrap();
        let mut old = sample("completed", "Action completed.");
        old.subject = Subject::new("application", "provider", "fixture");
        Journal::default()
            .upsert(&store, old.clone(), RETENTION)
            .await
            .unwrap();
        let (status, events) = call(&app, "GET", "/events", "root-secret", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        let event = events
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["id"] == old.id)
            .unwrap();
        assert_eq!(event["subject"]["kind"], "database");
        assert_eq!(event["subject"]["available"], true);
        assert!(event.get("progress").is_none());
        assert_eq!(read(&store).await.unwrap()[0].subject.kind, "application");

        crate::postgres::DATABASES
            .remove(&store, "provider")
            .await
            .unwrap();
        let deleted = super::event(
            "delete",
            Subject::new("database", "provider", "fixture"),
            "completed",
            None,
            "Database removed.",
        );
        Journal::default()
            .upsert(&store, deleted, RETENTION)
            .await
            .unwrap();
        let (_, events) = call(&app, "GET", "/events", "root-secret", Value::Null).await;
        let historic = events
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["id"] == old.id)
            .unwrap();
        assert_eq!(historic["subject"]["kind"], "database");
        assert_eq!(historic["subject"]["available"], false);
    }

    #[tokio::test]
    async fn events_older_than_the_retention_leave_on_the_next_write() {
        let store = FakeStateStore::new();
        let journal = Journal::default();
        let mut old = sample("completed", "Virtual machine operation completed.");
        old.occurred_at = "2020-01-01T00:00:00.000000000Z".into();
        old.updated_at = Some(old.occurred_at.clone());
        journal
            .upsert(&store, old.clone(), RETENTION * 400)
            .await
            .unwrap();
        assert_eq!(read(&store).await.unwrap().len(), 1);

        let fresh = sample("completed", "Virtual machine operation completed.");
        journal
            .upsert(&store, fresh.clone(), RETENTION)
            .await
            .unwrap();

        let events = read(&store).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, fresh.id);
    }
}
