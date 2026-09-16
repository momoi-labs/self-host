//! A small, durable history of authenticated mutations and background results.
//! Only action metadata is recorded. Request bodies, credentials, environment
//! values, build output and error reports never enter the audit history.

use crate::{
    AppState,
    store::{StateStore, StoreError},
};
use axum::{
    Json,
    body::{Body, to_bytes},
    extract::{MatchedPath, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

const STATE_KEY: &str = "audit_events_v1";
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
        started_at: Some(at.clone()),
        finished_at: if is_terminal(status) {
            Some(at.clone())
        } else {
            None
        },
        updated_at: Some(at),
        api_name,
        description: description.into(),
        subject,
    }
}

fn is_terminal(status: &str) -> bool {
    matches!(status, "completed" | "failed")
}
fn timestamp() -> String {
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
    pub async fn upsert<S: StateStore>(&self, store: &S, event: Event) -> Result<(), StoreError> {
        let _guard = self.write.lock().await;
        let mut events = read(store).await?;
        if let Some(existing) = events.iter_mut().find(|existing| existing.id == event.id) {
            // A fast worker can finish before the HTTP handler returns 202.
            // The later acknowledgement must not overwrite its final result.
            if !is_terminal(&existing.status) {
                existing.status = event.status;
                existing.description = event.description;
                existing.updated_at = event.updated_at;
                existing.finished_at = event.finished_at;
            }
            if !event.subject.id.is_empty() {
                existing.subject = event.subject;
            }
            if existing.started_at.is_none() {
                existing.started_at = event.started_at;
            }
        } else {
            events.push(event);
        }
        store
            .store_state(
                STATE_KEY,
                &serde_json::to_string(&events)
                    .map_err(|e| StoreError::Serialize(e.to_string()))?,
            )
            .await
    }
}
async fn read<S: StateStore>(store: &S) -> Result<Vec<Event>, StoreError> {
    match store.get_state(STATE_KEY).await? {
        None => Ok(vec![]),
        Some(json) => serde_json::from_str(&json)
            .map(migrate_legacy)
            .map_err(|e| StoreError::Serialize(format!("could not read audit history: {e}"))),
    }
}

/// Old background work had separate acceptance and completion records. Merge
/// only unambiguous pairs; a rejected second request is not a worker result.
fn migrate_legacy(mut events: Vec<Event>) -> Vec<Event> {
    events.sort_by(|a, b| a.occurred_at.cmp(&b.occurred_at));
    let mut result: Vec<Event> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    for mut event in events {
        if event.updated_at.is_some() {
            result.push(event);
            continue;
        }
        let completion = is_terminal(&event.status)
            && [
                "Virtual machine operation ",
                "Application deployment ",
                "Custom image build ",
            ]
            .iter()
            .any(|prefix| event.description.starts_with(prefix));
        if completion {
            let matches: Vec<usize> = open
                .iter()
                .copied()
                .filter(|&index| {
                    let start = &result[index];
                    start.action == event.action
                        && start.subject.kind == event.subject.kind
                        && !start.subject.id.is_empty()
                        && start.subject.id == event.subject.id
                        && start.api_name == event.api_name
                })
                .collect();
            if let [index] = matches.as_slice() {
                let start = &mut result[*index];
                start.status = event.status;
                start.description = event.description;
                start.finished_at = Some(event.occurred_at.clone());
                start.updated_at = Some(event.occurred_at);
                open.retain(|candidate| candidate != index);
                continue;
            }
        }
        event.updated_at = Some(event.occurred_at.clone());
        if event.status == "accepted" {
            event.status = "running".into();
            event.started_at = Some(event.occurred_at.clone());
            open.push(result.len());
        } else if completion {
            event.finished_at = Some(event.occurred_at.clone());
        }
        result.push(event);
    }
    result
}

pub(crate) async fn record<S: StateStore>(state: &AppState<S>, event: Event) {
    if let Err(error) = state.audit.upsert(&state.store, event).await {
        tracing::error!(%error, "Could not persist audit event");
    }
}

pub(crate) async fn list<S: StateStore>(State(state): State<AppState<S>>) -> Response {
    match read(&state.store).await {
        Ok(mut events) => {
            let apps = state.store.list_applications().await.ok();
            let keys = state.store.list_api_keys().await.ok();
            let machines: Option<Value> =
                state
                    .store
                    .get_state("environments_v1")
                    .await
                    .ok()
                    .map(|json| {
                        json.and_then(|json| serde_json::from_str(&json).ok())
                            .unwrap_or(serde_json::json!([]))
                    });
            let images: Option<Value> =
                state
                    .store
                    .get_state("custom_images_v1")
                    .await
                    .ok()
                    .map(|json| {
                        json.and_then(|json| serde_json::from_str(&json).ok())
                            .unwrap_or(serde_json::json!([]))
                    });
            let records: Option<Vec<String>> = crate::dns_records::load(&state.store)
                .await
                .ok()
                .map(|records| records.iter().map(|record| record.key()).collect());
            for event in &mut events {
                let id = &event.subject.id;
                event.subject.available = match event.subject.kind.as_str() {
                    "application" => apps
                        .as_ref()
                        .map(|apps| apps.iter().any(|app| &app.id == id)),
                    "api-key" => keys
                        .as_ref()
                        .map(|keys| keys.iter().any(|key| &key.id == id)),
                    "virtual-machine" => machines
                        .as_ref()
                        .and_then(Value::as_array)
                        .map(|rows| rows.iter().any(|row| field(row, "/id") == *id)),
                    "custom-image" => images
                        .as_ref()
                        .and_then(Value::as_array)
                        .map(|rows| rows.iter().any(|row| field(row, "/id") == *id)),
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
        ("POST", "/apps/id/{id}/start") => ("start", "application"),
        ("POST", "/apps/id/{id}/stop") => ("stop", "application"),
        ("POST", "/apps/id/{id}/restart") => ("restart", "application"),
        ("DELETE", "/apps/{name}") => ("delete", "application"),
        ("POST", "/apps/{name}/env") => ("configure", "application"),
        ("DELETE", "/apps/{name}/env/{key}") => ("configure", "application"),
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
        let record = if route.contains("{name}") {
            state
                .store
                .find_application_by_name(parts[1])
                .await
                .ok()
                .flatten()
        } else if !id.is_empty() {
            state.store.get_application(&id).await.ok().flatten()
        } else {
            None
        };
        if route.contains("{name}") && name.is_empty() {
            name = parts[1].to_owned();
        }
        if let Some(record) = record {
            id = record.id;
            name = record.name;
        }
    } else if kind == "virtual-machine" || kind == "custom-image" {
        let key = if kind == "virtual-machine" {
            "environments_v1"
        } else {
            "custom_images_v1"
        };
        if id.is_empty() {
            id = field(body, "/id");
        }
        let records: Value = state
            .store
            .get_state(key)
            .await
            .ok()
            .flatten()
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or(Value::Null);
        let pointer = if kind == "virtual-machine" {
            "/config/name"
        } else {
            "/recipe/name"
        };
        name = records
            .as_array()
            .and_then(|records| records.iter().find(|record| field(record, "/id") == id))
            .map(|record| field(record, pointer))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| field(body, pointer));
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
    let (parts, body) = req.into_parts();
    let bytes = match to_bytes(body, 2 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let payload: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let (action, kind) = action(&method, &route, &payload).unwrap();
    let subject = subject(&state, kind, &route, &path, &payload).await;
    let mut entry = event(action, subject, "running", actor(), "Operation started.");
    if let Err(error) = state.audit.upsert(&state.store, entry.clone()).await {
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
        }
        for pointer in ["/name", "/config/name", "/recipe/name", "/label"] {
            let name = field(&result, pointer);
            if !name.is_empty() {
                entry.subject.name = name;
                break;
            }
        }
        if status == StatusCode::ACCEPTED {
            entry.status = "running".into();
            entry.description = "Operation is running in the background.".into();
        } else {
            entry.status = "completed".into();
            entry.description = "Action completed.".into();
        }
    } else {
        entry.status = "failed".into();
        entry.description = format!("Request failed (HTTP {}).", status.as_u16());
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
        for _ in 0..100 {
            if read(&store).await.unwrap().iter().any(|event| {
                event.subject.id == created["id"].as_str().unwrap() && event.status == "completed"
            }) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let entries = read(&store).await.unwrap();
        let deploys: Vec<_> = entries
            .iter()
            .filter(|e| e.subject.kind == "application")
            .collect();
        assert_eq!(deploys.len(), 1);
        assert!(
            deploys
                .iter()
                .all(|e| e.api_name.as_deref() == Some("deploy-bot"))
        );

        assert!(deploys.iter().any(|e| e.status == "completed"));
        call(
            &app,
            "POST",
            "/apps/blog/env",
            token,
            json!({"key":"PASSWORD", "value":"sensitive-env-value"}),
        )
        .await;
        let stored = store.get_state(STATE_KEY).await.unwrap().unwrap();
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
        journal.upsert(&store, start.clone()).await.unwrap();
        let mut finish = sample("completed", "Virtual machine operation completed.");
        finish.id = start.id.clone();
        journal.upsert(&store, finish.clone()).await.unwrap();
        journal.upsert(&store, start.clone()).await.unwrap();
        let events = read(&store).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].status, "completed");
        assert_eq!(events[0].started_at, start.started_at);
        assert_eq!(events[0].finished_at, finish.finished_at);
        assert_eq!(events[0].updated_at, finish.updated_at);
    }

    #[test]
    fn legacy_pairs_merge_but_unrelated_failures_do_not() {
        let mut start = sample("accepted", "Request accepted.");
        start.updated_at = None;
        start.started_at = None;
        let mut rejected = sample("failed", "Request failed (HTTP 409).");
        rejected.updated_at = None;
        rejected.finished_at = None;
        let mut finish = sample("completed", "Virtual machine operation completed.");
        finish.updated_at = None;
        finish.started_at = None;
        finish.finished_at = None;
        let migrated = migrate_legacy(vec![start.clone(), rejected.clone(), finish.clone()]);
        assert_eq!(migrated.len(), 2);
        let operation = migrated.iter().find(|event| event.id == start.id).unwrap();
        assert_eq!(operation.status, "completed");
        assert_eq!(operation.started_at.as_ref(), Some(&start.occurred_at));
        assert_eq!(operation.finished_at.as_ref(), Some(&finish.occurred_at));
        assert!(migrated.iter().any(|event| event.id == rejected.id));
        let twice = migrate_legacy(migrated.clone());
        assert_eq!(
            serde_json::to_value(twice).unwrap(),
            serde_json::to_value(migrated).unwrap()
        );
    }
}
