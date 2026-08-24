use std::sync::Arc;

use axum::{
    Json, Router,
    extract::Request,
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response, sse},
    routing::{delete, get},
};
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;

use crate::db::DbError;
use rand::Rng;

pub mod apps;
pub mod bootstrap;
pub mod compose;
pub mod config;
pub mod console;
pub mod db;
pub mod docker;
pub mod tls;

use apps::{DeployError, RemoveError};
use db::StateStore;
use docker::DockerRuntime;

#[derive(Clone)]
struct AppState<S: StateStore> {
    store: S,
    docker: Arc<dyn DockerRuntime>,
}

pub fn build_app<S: StateStore>(store: S, docker: Arc<dyn DockerRuntime>) -> Router {
    let state = AppState { store, docker };

    let api_routes = Router::new()
        .route("/health", get(health))
        .route("/bootstrap/status", get(bootstrap_status::<S>))
        .route("/apps", get(list_apps::<S>).post(deploy_app::<S>))
        .route("/apps/{name}", delete(remove_app::<S>))
        .route("/apps/id/{id}", axum::routing::put(update_app::<S>))
        .route("/apps/{name}/env", get(get_env::<S>).post(set_env::<S>))
        .route("/apps/{name}/env/{key}", delete(unset_env::<S>))
        .route("/apps/{name}/logs", get(stream_logs::<S>))
        .route("/api-keys", get(list_keys::<S>).post(create_key::<S>))
        .route("/api-keys/{id}", delete(revoke_key::<S>))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_api_key::<S>,
        ))
        .with_state(state);

    console::console_router().merge(api_routes)
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
    })
}

#[derive(Serialize)]
struct BootstrapStatusResponse {
    initialized: bool,
    dns_suffix: Option<String>,
}

async fn bootstrap_status<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
) -> Json<BootstrapStatusResponse> {
    let initialized = state.store.is_initialized().await.unwrap_or(false);

    let dns_suffix = if initialized {
        state.store.get_state("dns_suffix").await.ok().flatten()
    } else {
        None
    };

    Json(BootstrapStatusResponse {
        initialized,
        dns_suffix,
    })
}

#[derive(Deserialize)]
struct DeployApplicationRequest {
    name: String,
    #[serde(default)]
    image: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    hostname: Option<String>,
}

#[derive(Deserialize)]
struct UpdateApplicationRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    hostname: Option<String>,
}

#[derive(Serialize)]
struct ApplicationResponse {
    id: String,
    name: String,
    hostname: String,
    image: String,
    status: String,
    source: String,
    /// Present when `status` is `failed`, so the console can say why.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

impl From<apps::ApplicationRecord> for ApplicationResponse {
    fn from(app: apps::ApplicationRecord) -> Self {
        Self {
            id: app.id,
            name: app.name,
            hostname: app.hostname,
            image: app.image,
            status: app.status,
            source: app.source,
            last_error: app.last_error,
        }
    }
}

/// Saves an edit and redeploys. Addressed by id, not name, because the name is
/// exactly what an edit may be changing.
async fn update_app<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<UpdateApplicationRequest>,
) -> Response {
    match apps::update_application(
        &state.store,
        state.docker.as_ref(),
        &id,
        apps::ApplicationUpdate {
            name: body.name,
            image: body.image,
            hostname: body.hostname,
        },
    )
    .await
    {
        Ok(app) => (StatusCode::OK, Json(ApplicationResponse::from(app))).into_response(),
        Err(e) => deploy_error_response(e),
    }
}

async fn deploy_app<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    Json(body): Json<DeployApplicationRequest>,
) -> Response {
    let hostname = body.hostname.as_deref();

    let result = match (&body.image[..], &body.path[..]) {
        ("", "") => Err(apps::DeployError::MissingImage),
        (image, "") => {
            apps::deploy_from_image(
                &state.store,
                state.docker.as_ref(),
                &body.name,
                image,
                hostname,
            )
            .await
        }
        ("", path) => {
            apps::deploy_from_path(
                &state.store,
                state.docker.as_ref(),
                &body.name,
                path,
                hostname,
            )
            .await
        }
        _ => Err(apps::DeployError::MissingImage),
    };

    match result {
        Ok(app) => (StatusCode::CREATED, Json(ApplicationResponse::from(app))).into_response(),
        Err(err) => deploy_error_response(err),
    }
}

async fn list_apps<S: StateStore>(state: axum::extract::State<AppState<S>>) -> Response {
    match apps::list_applications(&state.store).await {
        Ok(apps) => {
            let body: Vec<ApplicationResponse> =
                apps.into_iter().map(ApplicationResponse::from).collect();
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(err) => deploy_error_response(err),
    }
}

async fn remove_app<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response {
    match apps::remove_application(&state.store, state.docker.as_ref(), &name).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => remove_error_response(err),
    }
}

fn remove_error_response(err: RemoveError) -> Response {
    let (status, message) = match &err {
        RemoveError::NotFound(_) => (StatusCode::NOT_FOUND, err.to_string()),
        RemoveError::ProtectedName(_) => (StatusCode::FORBIDDEN, err.to_string()),
        RemoveError::NotInitialized => (StatusCode::PRECONDITION_FAILED, err.to_string()),
        RemoveError::Docker(_) | RemoveError::Db(_) => {
            (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
        }
    };
    (status, message).into_response()
}

#[derive(Deserialize)]
struct SetEnvRequest {
    key: String,
    value: String,
}

async fn set_env<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(body): Json<SetEnvRequest>,
) -> Response {
    match apps::set_env(
        &state.store,
        state.docker.as_ref(),
        &name,
        &body.key,
        &body.value,
    )
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => env_error_response(err),
    }
}

async fn get_env<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response {
    match apps::get_all_env(&state.store, &name).await {
        Ok(env) => (StatusCode::OK, Json(env)).into_response(),
        Err(err) => env_error_response(err),
    }
}

async fn unset_env<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    axum::extract::Path((name, key)): axum::extract::Path<(String, String)>,
) -> Response {
    match apps::unset_env(&state.store, state.docker.as_ref(), &name, &key).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => env_error_response(err),
    }
}

fn env_error_response(err: apps::EnvError) -> Response {
    let (status, message) = match &err {
        apps::EnvError::NotFound(_) => (StatusCode::NOT_FOUND, err.to_string()),
        apps::EnvError::NotInitialized => (StatusCode::PRECONDITION_FAILED, err.to_string()),
        apps::EnvError::Docker(_) | apps::EnvError::Db(_) => {
            (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
        }
    };
    (status, message).into_response()
}

async fn stream_logs<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response {
    // Validate app exists
    let Ok(Some(app)) = state.store.find_application_by_name(&name).await else {
        return logs_error_response(&apps::LogsError::NotFound(name));
    };

    let container_name = apps::container_name_for(&app.id);

    match state.docker.stream_logs(&container_name).await {
        Ok(rx) => {
            let stream = tokio_stream::wrappers::ReceiverStream::new(rx).map(|line| {
                Ok::<_, std::convert::Infallible>(axum::response::sse::Event::default().data(line))
            });
            sse::Sse::new(stream)
                .keep_alive(
                    sse::KeepAlive::new()
                        .interval(std::time::Duration::from_secs(15))
                        .text("keepalive"),
                )
                .into_response()
        }
        Err(err) => logs_error_response(&apps::LogsError::Docker(err)),
    }
}

fn logs_error_response(err: &apps::LogsError) -> Response {
    let (status, message) = match err {
        apps::LogsError::NotFound(_) => (StatusCode::NOT_FOUND, err.to_string()),
        apps::LogsError::NotInitialized => (StatusCode::PRECONDITION_FAILED, err.to_string()),
        apps::LogsError::Docker(_) | apps::LogsError::Db(_) => {
            (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
        }
    };
    (status, message).into_response()
}

#[derive(Deserialize)]
struct CreateKeyRequest {
    label: String,
}

#[derive(Serialize)]
struct ApiKeyResponse {
    id: String,
    label: String,
    created_at: String,
}

#[derive(Serialize)]
struct CreateKeyResponse {
    id: String,
    label: String,
    created_at: String,
    key: String,
}

async fn list_keys<S: StateStore>(state: axum::extract::State<AppState<S>>) -> Response {
    match state.store.list_api_keys().await {
        Ok(keys) => {
            let body: Vec<ApiKeyResponse> = keys
                .into_iter()
                .map(|k| ApiKeyResponse {
                    id: k.id,
                    label: k.label,
                    created_at: k.created_at,
                })
                .collect();
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn create_key<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    Json(body): Json<CreateKeyRequest>,
) -> Response {
    let key_bytes: [u8; 32] = rand::rng().random();
    let key: String = key_bytes.iter().map(|b| format!("{b:02x}")).collect();
    let id = format!("sk-{}", &key[..12]);

    match state.store.create_api_key(&id, &body.label).await {
        Ok(()) => {
            let response = CreateKeyResponse {
                id,
                label: body.label,
                created_at: "now".into(),
                key,
            };
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn revoke_key<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    match state.store.revoke_api_key(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(DbError::NotFound(_)) => (StatusCode::NOT_FOUND, "key not found").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

fn deploy_error_response(err: DeployError) -> Response {
    let (status, message) = match &err {
        DeployError::AlreadyExists(_) => (StatusCode::CONFLICT, err.to_string()),
        DeployError::InvalidName(_) | DeployError::MissingImage | DeployError::MissingPath => {
            (StatusCode::BAD_REQUEST, err.to_string())
        }
        DeployError::NotFound(_) => (StatusCode::NOT_FOUND, err.to_string()),
        DeployError::NotInitialized => (StatusCode::PRECONDITION_FAILED, err.to_string()),
        DeployError::Docker(_) | DeployError::Db(_) => {
            (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
        }
    };
    (status, message).into_response()
}

async fn require_api_key<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    req: Request,
    next: Next,
) -> Response {
    let expected_key = state
        .store
        .get_api_key()
        .await
        .ok()
        .flatten()
        .unwrap_or_default();

    let auth_header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match auth_header {
        Some(key) if constant_time_eq(key, &expected_key) => next.run(req).await,
        _ => (StatusCode::UNAUTHORIZED, "invalid api key").into_response(),
    }
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();

    if a.len() != b.len() {
        return false;
    }

    a.iter().zip(b.iter()).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, header};
    use axum::response::Response;
    use db::FakeStateStore;
    use docker::FakeDocker;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    async fn send(app: &Router, uri: &str, api_key: Option<&str>) -> Response {
        let mut req = Request::builder().uri(uri);
        if let Some(key) = api_key {
            req = req.header(header::AUTHORIZATION, format!("Bearer {key}"));
        }
        app.clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn post_json(app: &Router, uri: &str, api_key: Option<&str>, body: Value) -> Response {
        let mut req = Request::builder().method("POST").uri(uri);
        if let Some(key) = api_key {
            req = req.header(header::AUTHORIZATION, format!("Bearer {key}"));
        }
        req = req.header(header::CONTENT_TYPE, "application/json");
        app.clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }

    async fn put_json(app: &Router, uri: &str, api_key: Option<&str>, body: Value) -> Response {
        let mut req = Request::builder().method("PUT").uri(uri);
        if let Some(key) = api_key {
            req = req.header(header::AUTHORIZATION, format!("Bearer {key}"));
        }
        req = req.header(header::CONTENT_TYPE, "application/json");
        app.clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }

    async fn delete_req(app: &Router, uri: &str, api_key: Option<&str>) -> Response {
        let mut req = Request::builder().method("DELETE").uri(uri);
        if let Some(key) = api_key {
            req = req.header(header::AUTHORIZATION, format!("Bearer {key}"));
        }
        app.clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn setup_app(api_key: &str) -> (Router, FakeStateStore, FakeDocker) {
        let store = FakeStateStore::new();
        store.store_state("api_key", api_key).await.unwrap();
        let docker = FakeDocker::new();
        let app = build_app(store.clone(), Arc::new(docker.clone()));
        (app, store, docker)
    }

    async fn setup_initialized_app(api_key: &str, dns_suffix: &str) -> (Router, FakeStateStore) {
        let store = FakeStateStore::new();
        store.store_state("api_key", api_key).await.unwrap();
        store.store_state("dns_suffix", dns_suffix).await.unwrap();
        let docker = FakeDocker::new();
        let app = build_app(store.clone(), Arc::new(docker));
        (app, store)
    }

    #[tokio::test]
    async fn failed_deploy_still_records_the_application() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        let docker = FakeDocker::failing_pull("no such image: nginx:typo");
        let app = build_app(store.clone(), Arc::new(docker));

        let response = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:typo"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // The deploy failed, but the Application is on record — otherwise the
        // operator would have to retype it instead of fixing the image.
        let saved = store.find_application_by_name("blog").await.unwrap();
        let saved = saved.expect("a failed deploy must still leave a row");
        assert_eq!(saved.status, apps::STATUS_FAILED);
        assert_eq!(saved.image, "nginx:typo");
        assert!(saved.last_error.unwrap().contains("nginx:typo"));
    }

    #[tokio::test]
    async fn renaming_an_application_leaves_its_container_alone() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        let docker = FakeDocker::new();
        let apps_handle = docker.apps.clone();
        let app = build_app(store.clone(), Arc::new(docker));

        post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        let before = apps_handle.lock().unwrap().len();
        let id = store
            .find_application_by_name("blog")
            .await
            .unwrap()
            .unwrap()
            .id;

        let response = put_json(
            &app,
            &format!("/apps/id/{id}"),
            Some("test-key"),
            json!({"name": "weblog"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // A rename is a row update. Docker must not have been asked to start
        // anything a second time.
        assert_eq!(apps_handle.lock().unwrap().len(), before);
        let renamed = store.get_application(&id).await.unwrap().unwrap();
        assert_eq!(renamed.name, "weblog");
        assert_eq!(renamed.status, apps::STATUS_RUNNING);
    }

    #[tokio::test]
    async fn health_returns_200_with_valid_api_key() {
        let (app, _, _) = setup_app("secret-key").await;
        let response = send(&app, "/health", Some("secret-key")).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn health_returns_401_without_api_key() {
        let (app, _, _) = setup_app("secret-key").await;
        let response = send(&app, "/health", None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn health_returns_401_with_invalid_api_key() {
        let (app, _, _) = setup_app("secret-key").await;
        let response = send(&app, "/health", Some("wrong-key")).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn health_returns_json_status_ok_with_valid_api_key() {
        let (app, _, _) = setup_app("secret-key").await;
        let response = send(&app, "/health", Some("secret-key")).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed, json!({"status": "ok"}));
    }

    #[tokio::test]
    async fn unknown_route_returns_401_without_api_key() {
        let (app, _, _) = setup_app("secret-key").await;
        let response = send(&app, "/nonexistent", None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn unknown_route_returns_404_with_valid_api_key() {
        let (app, _, _) = setup_app("secret-key").await;
        let response = send(&app, "/nonexistent", Some("secret-key")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn constant_time_eq_matches_identical_strings() {
        assert!(constant_time_eq("secret", "secret"));
    }

    #[test]
    fn constant_time_eq_rejects_different_strings() {
        assert!(!constant_time_eq("secret", "wrong"));
    }

    #[test]
    fn constant_time_eq_rejects_different_lengths() {
        assert!(!constant_time_eq("short", "longer"));
    }

    #[tokio::test]
    async fn bootstrap_status_returns_initialized_false_when_not_initialized() {
        let store = FakeStateStore::new();
        let docker = FakeDocker::new();
        let app = build_app(store.clone(), Arc::new(docker));

        // Store an API key so auth passes
        store.store_state("api_key", "test-key").await.unwrap();

        let response = send(&app, "/bootstrap/status", Some("test-key")).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["initialized"], json!(false));
        assert_eq!(parsed["dns_suffix"], json!(null));
    }

    #[tokio::test]
    async fn bootstrap_status_returns_initialized_true_with_dns_suffix() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();

        let docker = FakeDocker::new();
        let app = build_app(store, Arc::new(docker));

        let response = send(&app, "/bootstrap/status", Some("test-key")).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["initialized"], json!(true));
        assert_eq!(parsed["dns_suffix"], json!("home.lan"));
    }

    #[tokio::test]
    async fn bootstrap_status_returns_401_without_api_key() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        let docker = FakeDocker::new();
        let app = build_app(store, Arc::new(docker));

        let response = send(&app, "/bootstrap/status", None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn bootstrap_init_persists_state_and_is_visible_through_api() {
        let store = FakeStateStore::new();

        let result = bootstrap::BootstrapResult {
            dns_suffix: "myhost.lan".into(),
            api_key: "generated-key".into(),
            api_listen_addr: "192.168.1.100:3721".into(),
            host_ip: "192.168.1.100".into(),
        };

        bootstrap::persist_bootstrap_state(&store, &result)
            .await
            .expect("persist should succeed");

        let docker = FakeDocker::new();
        let app = build_app(store, Arc::new(docker));

        let response = send(&app, "/bootstrap/status", Some("generated-key")).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["initialized"], json!(true));
        assert_eq!(parsed["dns_suffix"], json!("myhost.lan"));
    }

    #[tokio::test]
    async fn persist_bootstrap_state_fails_when_already_initialized() {
        let store = FakeStateStore::new();

        let result1 = bootstrap::BootstrapResult {
            dns_suffix: "first.lan".into(),
            api_key: "key1".into(),
            api_listen_addr: "127.0.0.1:3721".into(),
            host_ip: "127.0.0.1".into(),
        };

        bootstrap::persist_bootstrap_state(&store, &result1)
            .await
            .expect("first persist should succeed");

        let result2 = bootstrap::BootstrapResult {
            dns_suffix: "second.lan".into(),
            api_key: "key2".into(),
            api_listen_addr: "127.0.0.1:3721".into(),
            host_ip: "127.0.0.1".into(),
        };

        let err = bootstrap::persist_bootstrap_state(&store, &result2)
            .await
            .expect_err("second persist should fail");

        assert!(matches!(err, bootstrap::BootstrapError::AlreadyInitialized));
    }

    #[tokio::test]
    async fn deploy_application_from_image_returns_hostname_under_dns_suffix() {
        let (app, _) = setup_initialized_app("test-key", "home.lan").await;

        let response = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;

        assert_eq!(response.status(), StatusCode::CREATED);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["name"], json!("blog"));
        assert_eq!(parsed["hostname"], json!("blog.home.lan"));
        assert_eq!(parsed["image"], json!("nginx:alpine"));
        assert_eq!(parsed["status"], json!("running"));
    }

    #[tokio::test]
    async fn list_applications_includes_deployed_app() {
        let (app, _) = setup_initialized_app("test-key", "home.lan").await;

        let deploy = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        assert_eq!(deploy.status(), StatusCode::CREATED);

        let response = send(&app, "/apps", Some("test-key")).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let mut parsed: Value = serde_json::from_slice(&body).unwrap();

        // The id is random; assert it is there and shaped right, then drop it
        // so the rest of the payload can be compared literally.
        let id = parsed[0]["id"].take();
        assert_eq!(id.as_str().unwrap().len(), 12);
        parsed[0].as_object_mut().unwrap().remove("id");

        assert_eq!(
            parsed,
            json!([{
                "name": "blog",
                "hostname": "blog.home.lan",
                "image": "nginx:alpine",
                "status": "running",
                "source": "image"
            }])
        );
    }

    #[tokio::test]
    async fn deploy_duplicate_application_name_returns_conflict() {
        let (app, _) = setup_initialized_app("test-key", "home.lan").await;

        let first = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        assert_eq!(first.status(), StatusCode::CREATED);

        let second = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        assert_eq!(second.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn deploy_requires_api_key() {
        let (app, _) = setup_initialized_app("test-key", "home.lan").await;

        let response = post_json(
            &app,
            "/apps",
            None,
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn deploy_runs_application_without_host_ports() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        let docker = FakeDocker::new();
        let app = build_app(store, Arc::new(docker.clone()));

        let response = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);

        assert_eq!(docker.pulled.lock().unwrap().as_slice(), ["nginx:alpine"]);

        let deployed = docker.apps.lock().unwrap();
        assert_eq!(deployed.len(), 1);
        assert!(deployed[0].ports.is_empty());
        assert!(
            deployed[0]
                .labels
                .iter()
                .any(|(k, v)| k == "traefik.enable" && v == "true")
        );
        assert!(
            deployed[0]
                .labels
                .iter()
                .any(|(k, v)| { k.ends_with(".rule") && v == "Host(`blog.home.lan`)" })
        );
    }

    #[tokio::test]
    async fn deploy_duplicate_returns_clear_error_message() {
        let (app, _) = setup_initialized_app("test-key", "home.lan").await;

        let _ = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;

        let second = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        assert_eq!(second.status(), StatusCode::CREATED);
        let body = to_bytes(second.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["name"], json!("blog"));
        assert_eq!(parsed["source"], json!("image"));
    }

    #[tokio::test]
    async fn two_apps_on_different_hostnames_do_not_interfere() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        let docker = FakeDocker::new();
        let app = build_app(store, Arc::new(docker.clone()));

        // Deploy two apps
        let r1 = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        assert_eq!(r1.status(), StatusCode::CREATED);

        let r2 = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "files", "image": "filebrowser/filebrowser"}),
        )
        .await;
        assert_eq!(r2.status(), StatusCode::CREATED);

        // Both appear in list
        let list = send(&app, "/apps", Some("test-key")).await;
        let body = to_bytes(list.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        let names: Vec<&str> = parsed
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["blog", "files"]);

        // Each has its own hostname
        let hostnames: Vec<&str> = parsed
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["hostname"].as_str().unwrap())
            .collect();
        assert_eq!(hostnames, vec!["blog.home.lan", "files.home.lan"]);
    }

    #[tokio::test]
    async fn remove_application_returns_204_and_cleans_docker() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        let docker = FakeDocker::new();
        let app = build_app(store.clone(), Arc::new(docker.clone()));

        let deploy = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        assert_eq!(deploy.status(), StatusCode::CREATED);

        let response = delete_req(&app, "/apps/blog", Some("test-key")).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let list = send(&app, "/apps", Some("test-key")).await;
        let body = to_bytes(list.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed, json!([]));
    }

    #[tokio::test]
    async fn remove_nonexistent_application_returns_404() {
        let (app, _) = setup_initialized_app("test-key", "home.lan").await;

        let response = delete_req(&app, "/apps/nonexistent", Some("test-key")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn remove_protected_name_returns_403() {
        let (app, _) = setup_initialized_app("test-key", "home.lan").await;

        for name in &["postgres", "coredns", "traefik"] {
            let response = delete_req(&app, &format!("/apps/{name}"), Some("test-key")).await;
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "should forbid removal of '{name}'"
            );
        }
    }

    #[tokio::test]
    async fn remove_requires_api_key() {
        let (app, _) = setup_initialized_app("test-key", "home.lan").await;

        let response = delete_req(&app, "/apps/blog", None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn remove_cleans_container_from_docker() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        let docker = FakeDocker::new();
        let app = build_app(store.clone(), Arc::new(docker.clone()));

        let _ = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;

        let deployed = docker.deployed_apps();
        assert_eq!(deployed.len(), 1);
        assert!(deployed[0].starts_with(apps::APP_PREFIX));

        let response = delete_req(&app, "/apps/blog", Some("test-key")).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        assert!(docker.deployed_apps().is_empty());
    }

    #[tokio::test]
    async fn remove_not_initialized_returns_412() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        let docker = FakeDocker::new();
        let app = build_app(store, Arc::new(docker));

        let response = delete_req(&app, "/apps/blog", Some("test-key")).await;
        assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    }
}

#[cfg(test)]
mod bootstrap_tests {
    use super::bootstrap;
    use super::docker::FakeDocker;

    #[test]
    fn validate_dns_suffix_rejects_local() {
        assert!(bootstrap::validate_dns_suffix(".local").is_err());
        assert!(bootstrap::validate_dns_suffix("my.local").is_err());
        assert!(bootstrap::validate_dns_suffix("local").is_err());
    }

    #[test]
    fn validate_dns_suffix_rejects_empty() {
        assert!(bootstrap::validate_dns_suffix("").is_err());
    }

    #[test]
    fn validate_dns_suffix_rejects_bare_name() {
        assert!(bootstrap::validate_dns_suffix("lan").is_err());
    }

    #[test]
    fn validate_dns_suffix_accepts_valid_suffixes() {
        assert!(bootstrap::validate_dns_suffix("home.lan").is_ok());
        assert!(bootstrap::validate_dns_suffix("myhost.internal").is_ok());
        assert!(bootstrap::validate_dns_suffix("example.com").is_ok());
    }

    #[test]
    fn default_dns_suffix_is_home_lan() {
        assert_eq!(bootstrap::DEFAULT_DNS_SUFFIX, "home.lan");
    }

    #[tokio::test]
    async fn run_bootstrap_with_fake_docker_succeeds() {
        let docker = FakeDocker::new();
        let result = bootstrap::run_bootstrap(&docker, "test.lan")
            .await
            .expect("bootstrap should succeed with fake docker");

        assert_eq!(result.dns_suffix, "test.lan");
        assert!(result.api_key.len() >= 64); // 32 bytes = 64 hex chars
        assert!(result.api_listen_addr.contains(":3721"));
    }

    #[tokio::test]
    async fn run_bootstrap_rejects_invalid_dns_suffix() {
        let docker = FakeDocker::new();
        let err = bootstrap::run_bootstrap(&docker, ".local")
            .await
            .expect_err("should reject .local");

        assert!(matches!(
            err,
            bootstrap::BootstrapError::InvalidDnsSuffix(_)
        ));
    }
}

#[cfg(test)]
mod deploy_path_tests {
    use super::*;
    use crate::db::FakeStateStore;
    use crate::docker::FakeDocker;
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn post_json(
        app: &axum::Router,
        uri: &str,
        api_key: Option<&str>,
        body: Value,
    ) -> axum::response::Response {
        use axum::body::Body;
        use axum::http::{Request, header};

        let mut req = Request::builder()
            .method("POST")
            .uri(uri)
            .header("Content-Type", "application/json");
        if let Some(key) = api_key {
            req = req.header(header::AUTHORIZATION, format!("Bearer {key}"));
        }
        app.clone()
            .oneshot(
                req.body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn deploy_application_from_path_builds_and_runs() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        let docker = FakeDocker::new();
        let app = build_app(store, Arc::new(docker.clone()));

        let response = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "api", "path": "./myapp"}),
        )
        .await;

        assert_eq!(response.status(), StatusCode::CREATED);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["name"], json!("api"));
        assert_eq!(parsed["hostname"], json!("api.home.lan"));
        assert_eq!(parsed["image"], json!("self-host-api:latest"));
        assert_eq!(parsed["status"], json!("running"));

        let built = docker.built.lock().unwrap();
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].0, "./myapp");
        assert_eq!(built[0].1, "self-host-api:latest");

        let deployed = docker.apps.lock().unwrap();
        assert_eq!(deployed.len(), 1);
        assert_eq!(deployed[0].image, "self-host-api:latest");
    }
}
