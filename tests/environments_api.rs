use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use self_host::{
    build_app_with_vm_runtime,
    docker::FakeDocker,
    environments::{RunnerObservation, VmConfig, VmRuntime, VmState},
    metrics::Metrics,
    routes::FakeRoutes,
    store::{ApiKeyRecord, ApplicationRecord, FakeStateStore, StateStore, StoreError},
};
use serde_json::{Value, json};
use tokio::sync::{Mutex, Notify};
use tower::ServiceExt;

const KEY: &str = "test-key";

#[derive(Clone)]
struct FakeRuntime {
    inspect_error: bool,
    unknown_state: bool,
    fail_delete: bool,
    calls: Arc<Mutex<Vec<String>>>,
    release: Option<Arc<Notify>>,
}

impl FakeRuntime {
    fn new() -> Self {
        Self {
            inspect_error: false,
            unknown_state: false,
            fail_delete: false,
            calls: Arc::new(Mutex::new(Vec::new())),
            release: None,
        }
    }

    fn unknown() -> Self {
        Self {
            unknown_state: true,
            ..Self::new()
        }
    }
}

#[async_trait]
impl VmRuntime for FakeRuntime {
    async fn inspect(&self, _id: &str) -> Result<RunnerObservation, String> {
        if self.inspect_error {
            return Err("runner unavailable".into());
        }
        if self.unknown_state {
            return Ok(RunnerObservation::default());
        }
        Ok(RunnerObservation {
            state: VmState::Stopped,
            ..RunnerObservation::default()
        })
    }

    async fn execute(
        &self,
        _id: &str,
        action: &str,
        _config: &VmConfig,
    ) -> Result<RunnerObservation, String> {
        self.calls.lock().await.push(action.to_owned());
        if action == "delete" && self.fail_delete {
            return Err("delete failed".into());
        }
        if let Some(release) = &self.release {
            release.notified().await;
        }
        Ok(RunnerObservation {
            state: if action == "delete" {
                VmState::Missing
            } else {
                VmState::Running
            },
            ..RunnerObservation::default()
        })
    }
}

fn config(name: &str) -> VmConfig {
    VmConfig {
        name: name.into(),
        cpus: 2,
        memory_gib: 4,
        disk_gib: 20,
        ssh_public_key: "ssh-ed25519 AAAA test".into(),
        recipe: self_host::dev_images::Recipe {
            name: "T3".into(),
            template_id: Some("t3-code".into()),
            dependencies: vec![self_host::dev_images::Dependency {
                tool: "node".into(),
                version: "22".into(),
                allow_builds: vec![],
            }],
            build_checks: vec![],
        },
        command: "t3 serve --port 3000".into(),
        web_port: 3000,
    }
}

async fn app(runtime: FakeRuntime) -> (Router, FakeStateStore) {
    let store = FakeStateStore::new();
    store.store_state("api_key", KEY).await.unwrap();
    let app = build_app_with_vm_runtime(
        store.clone(),
        Arc::new(FakeDocker::new()),
        Arc::new(FakeRoutes::new()),
        Metrics::new(),
        Arc::new(runtime),
    );
    (app, store)
}

async fn request(
    app: &Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {KEY}"));
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Body::from(
            body.map(|value| value.to_string()).unwrap_or_default(),
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn creation_is_idempotent_and_rejects_reused_request_ids_with_new_config() {
    let (app, _store) = app(FakeRuntime::new()).await;
    let first = json!({"request_id":"request-1", "config":config("alpha")});
    let (status, record) = request(&app, Method::POST, "/environments", Some(first.clone())).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (again, same) = request(&app, Method::POST, "/environments", Some(first)).await;
    assert_eq!(again, StatusCode::ACCEPTED);
    assert_eq!(record["id"], same["id"]);
    let changed = json!({"request_id":"request-1", "config":config("different")});
    let (conflict, _) = request(&app, Method::POST, "/environments", Some(changed)).await;
    assert_eq!(conflict, StatusCode::CONFLICT);
}

#[tokio::test]
async fn retrying_delete_requires_confirmation_again() {
    let (app, _store) = app(FakeRuntime {
        fail_delete: true,
        ..FakeRuntime::new()
    })
    .await;
    let (_, created) = request(
        &app,
        Method::POST,
        "/environments",
        Some(json!({"request_id":"delete-1", "config":config("alpha")})),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let id = created["id"].as_str().unwrap();
    let (_, _) = request(
        &app,
        Method::POST,
        &format!("/environments/{id}/actions"),
        Some(json!({"action":"delete", "confirm_name":"alpha"})),
    )
    .await;
    for _ in 0..20 {
        let (_, current) = request(&app, Method::GET, &format!("/environments/{id}"), None).await;
        if current["operation"]["status"] == "failed" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let (bad_confirmation, _) = request(
        &app,
        Method::POST,
        &format!("/environments/{id}/actions"),
        Some(json!({"action":"retry"})),
    )
    .await;
    assert_eq!(bad_confirmation, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_unknown_runner_never_starts_create() {
    let runtime = FakeRuntime::unknown();
    let calls = runtime.calls.clone();
    let (app, _store) = app(runtime).await;
    let (_, created) = request(
        &app,
        Method::POST,
        "/environments",
        Some(json!({"request_id":"unknown-1", "config":config("alpha")})),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(calls.lock().await.is_empty());
    assert_eq!(created["operation"]["status"], "running");
    let (status, current) = request(
        &app,
        Method::GET,
        &format!("/environments/{}", created["id"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(current["state"], "unknown");
    assert_eq!(current["operation"]["status"], "failed");
}

#[tokio::test]
async fn saving_a_draft_keeps_the_last_applied_configuration() {
    let (app, _store) = app(FakeRuntime::new()).await;
    let (_, created) = request(
        &app,
        Method::POST,
        "/environments",
        Some(json!({"request_id":"draft-1", "config":config("alpha")})),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let id = created["id"].as_str().unwrap();
    let mut draft = config("alpha");
    draft.command = "t3 serve --port 4000".into();
    let (status, saved) = request(
        &app,
        Method::PUT,
        &format!("/environments/{id}"),
        Some(serde_json::to_value(draft).unwrap()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["config"]["command"], "t3 serve --port 4000");
    assert_eq!(saved["applied_config"]["command"], "t3 serve --port 3000");
}

#[tokio::test]
async fn restart_marks_running_operation_interrupted_without_resuming_it() {
    let store = FakeStateStore::new();
    store.store_state("api_key", KEY).await.unwrap();
    let record = json!([{
        "id":"env-recovered", "config":config("alpha"), "applied_config":null,
        "state":"missing", "service_ready":false,
        "operation":{"action":"create","status":"running","step":"creating","error":null},
        "log":"started", "ssh_command":null, "tunnel_command":null, "web_url":null,
        "base_image":null, "installed_versions":null
    }]);
    store
        .store_state("environments_v1", &record.to_string())
        .await
        .unwrap();
    let (app, _) = app_with_store(store.clone(), FakeRuntime::new()).await;
    let (status, current) = request(&app, Method::GET, "/environments/env-recovered", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(current["operation"]["status"], "interrupted");
    assert!(
        store
            .get_state("environments_v1")
            .await
            .unwrap()
            .unwrap()
            .contains("interrupted")
    );
}

async fn app_with_store(store: FakeStateStore, runtime: FakeRuntime) -> (Router, FakeStateStore) {
    let app = build_app_with_vm_runtime(
        store.clone(),
        Arc::new(FakeDocker::new()),
        Arc::new(FakeRoutes::new()),
        Metrics::new(),
        Arc::new(runtime),
    );
    (app, store)
}

#[derive(Clone)]
struct FailOnceStore {
    inner: FakeStateStore,
    fail_next: Arc<AtomicBool>,
}

#[async_trait]
impl StateStore for FailOnceStore {
    async fn initialize(&self) -> Result<(), StoreError> {
        self.inner.initialize().await
    }
    async fn is_initialized(&self) -> Result<bool, StoreError> {
        self.inner.is_initialized().await
    }
    async fn store_state(&self, key: &str, value: &str) -> Result<(), StoreError> {
        if key == "environments_v1" && self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(StoreError::Io("test".into(), "injected failure".into()));
        }
        self.inner.store_state(key, value).await
    }
    async fn get_state(&self, key: &str) -> Result<Option<String>, StoreError> {
        self.inner.get_state(key).await
    }
    async fn insert_application(&self, value: &ApplicationRecord) -> Result<(), StoreError> {
        self.inner.insert_application(value).await
    }
    async fn get_application(&self, id: &str) -> Result<Option<ApplicationRecord>, StoreError> {
        self.inner.get_application(id).await
    }
    async fn find_application_by_name(
        &self,
        name: &str,
    ) -> Result<Option<ApplicationRecord>, StoreError> {
        self.inner.find_application_by_name(name).await
    }
    async fn list_applications(&self) -> Result<Vec<ApplicationRecord>, StoreError> {
        self.inner.list_applications().await
    }
    async fn delete_application(&self, id: &str) -> Result<(), StoreError> {
        self.inner.delete_application(id).await
    }
    async fn set_env(&self, id: &str, key: &str, value: &str) -> Result<(), StoreError> {
        self.inner.set_env(id, key, value).await
    }
    async fn get_env(&self, id: &str, key: &str) -> Result<Option<String>, StoreError> {
        self.inner.get_env(id, key).await
    }
    async fn get_all_env(&self, id: &str) -> Result<Vec<(String, String)>, StoreError> {
        self.inner.get_all_env(id).await
    }
    async fn unset_env(&self, id: &str, key: &str) -> Result<(), StoreError> {
        self.inner.unset_env(id, key).await
    }
    async fn list_api_keys(&self) -> Result<Vec<ApiKeyRecord>, StoreError> {
        self.inner.list_api_keys().await
    }
    async fn create_api_key(&self, id: &str, label: &str) -> Result<(), StoreError> {
        self.inner.create_api_key(id, label).await
    }
    async fn revoke_api_key(&self, id: &str) -> Result<(), StoreError> {
        self.inner.revoke_api_key(id).await
    }
}

#[tokio::test]
async fn completion_save_failure_is_persisted_as_failed_and_keeps_record() {
    let inner = FakeStateStore::new();
    inner.store_state("api_key", KEY).await.unwrap();
    let fail_next = Arc::new(AtomicBool::new(false));
    let release = Arc::new(Notify::new());
    let runtime = FakeRuntime {
        inspect_error: false,
        unknown_state: false,
        fail_delete: false,
        calls: Arc::new(Mutex::new(Vec::new())),
        release: Some(release.clone()),
    };
    let calls = runtime.calls.clone();
    let store = FailOnceStore {
        inner: inner.clone(),
        fail_next: fail_next.clone(),
    };
    let app = build_app_with_vm_runtime(
        store,
        Arc::new(FakeDocker::new()),
        Arc::new(FakeRoutes::new()),
        Metrics::new(),
        Arc::new(runtime),
    );
    let (_, created) = request(
        &app,
        Method::POST,
        "/environments",
        Some(json!({"request_id":"save-failure", "config":config("alpha")})),
    )
    .await;
    fail_next.store(true, Ordering::SeqCst);
    for _ in 0..50 {
        if !calls.lock().await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    release.notify_waiters();
    let mut current = Value::Null;
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        let (_, next) = request(
            &app,
            Method::GET,
            &format!("/environments/{}", created["id"].as_str().unwrap()),
            None,
        )
        .await;
        current = next;
        if current["operation"]["status"] == "failed" {
            break;
        }
    }
    assert_eq!(current["operation"]["status"], "failed");
    assert!(
        inner
            .get_state("environments_v1")
            .await
            .unwrap()
            .unwrap()
            .contains("failed")
    );
}
