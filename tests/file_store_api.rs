//! The Operator API, against the state store the Platform actually ships.
//!
//! The unit suite runs the same handlers on `FakeStateStore`, a `HashMap`.
//! What it cannot answer is whether the contract still holds once every write
//! has to survive a restart: the API is unchanged only if a Host with no
//! database can create Applications and credentials, be restarted, and hand
//! back the same records.

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use self_host::docker::FakeDocker;
use self_host::file_store::FileStateStore;
use self_host::routes::FakeRoutes;
use self_host::store::StateStore;
use self_host::{build_app, routes};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tower::ServiceExt;

const API_KEY: &str = "test-key";

#[tokio::test]
async fn concurrent_record_and_application_claims_have_one_winner() {
    let temp = TempDir::new("dns-namespace");
    let (app, _) = boot(&temp.state()).await;
    let (record, application) = tokio::join!(
        call(
            &app,
            "POST",
            "/dns/records",
            Some(json!({"name": "nas", "type": "A", "value": "192.168.1.30"}))
        ),
        call(
            &app,
            "POST",
            "/apps",
            Some(json!({"name": "nas", "image": "nginx"}))
        ),
    );
    assert_ne!(
        record.0.is_success(),
        application.0.is_success(),
        "{record:?} {application:?}"
    );
}

#[tokio::test]
async fn late_deploy_keeps_the_edited_namespace_on_disk() {
    let temp = TempDir::new("dns-late-deploy");
    let (app, store) = boot(&temp.state()).await;
    let pending = self_host::apps::prepare_deploy_from_image(&store, "blog", "nginx", None, None)
        .await
        .unwrap();
    let id = pending.record.id.clone();
    let updated = self_host::apps::prepare_update(
        &store,
        &id,
        self_host::apps::ApplicationUpdate {
            hostname: Some("news.home.lan".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let routes = FakeRoutes::new();
    self_host::apps::finish_deploy(&store, &FakeDocker::new(), &routes, updated)
        .await
        .unwrap();
    let (status, _) = call(
        &app,
        "POST",
        "/dns/records",
        Some(json!({"name": "blog", "type": "A", "value": "192.168.1.30"})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    self_host::apps::finish_deploy(&store, &FakeDocker::new(), &routes, pending)
        .await
        .unwrap();
    drop(app);
    drop(store);
    let (app, _) = boot(&temp.state()).await;
    let (_, application) = call(&app, "GET", &format!("/apps/id/{id}"), None).await;
    assert_eq!(application["hostname"], "news.home.lan");
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("self-host-api-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the test state directory");
        TempDir(path)
    }

    fn state(&self) -> PathBuf {
        self.0.join("state")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Starts the Platform against a state directory, the way `serve` does.
async fn boot(state: &Path) -> (Router, FileStateStore) {
    let store = FileStateStore::open(state).expect("open the Platform state");
    store.initialize().await.unwrap();
    store.store_state("dns_suffix", "home.lan").await.unwrap();
    store.store_state("api_key", API_KEY).await.unwrap();
    let app = build_app(
        store.clone(),
        Arc::new(FakeDocker::new()),
        Arc::new(FakeRoutes::new()),
        self_host::metrics::Metrics::new(),
    );
    (app, store)
}

async fn call(app: &Router, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("Authorization", format!("Bearer {API_KEY}"));
    let request = match body {
        Some(json) => request
            .header("content-type", "application/json")
            .body(Body::from(json.to_string()))
            .unwrap(),
        None => request.body(Body::empty()).unwrap(),
    };

    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// The same call for a route that answers with plain text.
async fn call_text(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, String) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("Authorization", format!("Bearer {API_KEY}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

/// The task's event once the scheduler has run it, whichever way it went.
async fn finished(app: &Router, task_id: &str) -> Value {
    for _ in 0..200 {
        let (_, events) = call(app, "GET", "/events", None).await;
        if let Some(event) = events.as_array().and_then(|events| {
            events.iter().find(|event| {
                event["id"] == task_id
                    && (event["status"] == "completed" || event["status"] == "failed")
            })
        }) {
            return event.clone();
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("task '{task_id}' never finished");
}

/// Sends a mutation the scheduler carries out and waits for its task.
async fn call_task(
    app: &Router,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (status, accepted) = call(app, method, path, body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    let task_id = accepted["task_id"].as_str().expect("a 202 names its task");
    (status, finished(app, task_id).await)
}

/// Deploys settle on a background task in the handler the same way they do in the
/// unit suite; the record is readable as soon as it is committed.
async fn settled(app: &Router, id: &str) -> Value {
    for _ in 0..200 {
        let (status, body) = call(app, "GET", &format!("/apps/id/{id}"), None).await;
        if status == StatusCode::OK && body["status"] != "pending" {
            return body;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("the Application never settled");
}

#[tokio::test]
async fn a_host_with_no_database_keeps_what_the_operator_deployed_across_a_restart() {
    let dir = TempDir::new("restart");
    let (app, store) = boot(&dir.state()).await;

    let (status, blog) = call(
        &app,
        "POST",
        "/apps",
        Some(json!({
            "name": "blog",
            "compose": "services:\n  web:\n    image: nginx\n    ports:\n      - \"8080:80\"\n",
            "aliases": ["www.home.lan"],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{blog}");
    let blog_id = blog["id"].as_str().unwrap().to_string();
    settled(&app, &blog_id).await;

    let (_, configured) = call_task(
        &app,
        "POST",
        "/apps/blog/env",
        Some(json!({ "key": "TOKEN", "value": "secret" })),
    )
    .await;
    assert_eq!(configured["status"], "completed");

    let (_, stopped) = call_task(&app, "POST", &format!("/apps/id/{blog_id}/stop"), None).await;
    assert_eq!(stopped["status"], "completed");
    let (_, blog) = call(&app, "GET", &format!("/apps/id/{blog_id}"), None).await;
    assert_eq!(blog["status"], "stopped");

    let (status, key) = call(
        &app,
        "POST",
        "/api-keys",
        Some(json!({ "label": "laptop" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{key}");

    // Restart the Platform: a new process, the same directory.
    drop(app);
    drop(store);
    let store = FileStateStore::open(dir.state()).expect("reopen the Platform state");
    let app = build_app(
        store,
        Arc::new(FakeDocker::new()),
        Arc::new(routes::FakeRoutes::new()),
        self_host::metrics::Metrics::new(),
    );

    let (status, reread) = call(&app, "GET", &format!("/apps/id/{blog_id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reread["name"], "blog");
    assert_eq!(reread["hostname"], "blog.home.lan");
    assert_eq!(reread["aliases"], json!(["www.home.lan"]));
    assert_eq!(reread["source"], "compose");
    assert_eq!(reread["web_service"], "web");
    // Docker is unreachable, and an intentionally stopped Application stays
    // stopped rather than being re-observed into something else.
    assert_eq!(reread["status"], "stopped");
    assert!(
        reread["compose"].as_str().unwrap().contains("image: nginx"),
        "the Operator's Compose definition did not survive: {reread}"
    );

    let (status, env) = call(&app, "GET", "/apps/blog/env", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(env, json!([["TOKEN", "secret"]]));

    let (status, keys) = call(&app, "GET", "/api-keys", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(keys.as_array().unwrap().len(), 1);
    assert_eq!(keys[0]["label"], "laptop");
}

#[tokio::test]
async fn the_api_answers_the_same_errors_as_before() {
    let dir = TempDir::new("errors");
    let (app, _store) = boot(&dir.state()).await;

    let unauthorized = app
        .clone()
        .oneshot(Request::builder().uri("/apps").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let (status, missing) = call(&app, "GET", "/apps/id/does-not-exist", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(missing["error"].is_string(), "{missing}");

    let (status, body) = call(
        &app,
        "POST",
        "/apps",
        Some(json!({ "name": "blog", "image": "nginx" })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    settled(&app, &id).await;

    // A deploy onto a name that is taken is a redeploy and keeps the identity,
    // as it did on PostgreSQL.
    let (status, redeployed) = call(
        &app,
        "POST",
        "/apps",
        Some(json!({ "name": "blog", "image": "nginx:alpine" })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(redeployed["id"], id);

    let (status, conflict) = call(
        &app,
        "POST",
        "/apps",
        Some(json!({ "name": "journal", "image": "nginx", "hostname": "blog.home.lan" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{conflict}");
    assert!(
        conflict["error"]
            .as_str()
            .unwrap()
            .contains("blog.home.lan"),
        "{conflict}"
    );

    let (status, empty) = call(&app, "POST", "/apps", Some(json!({ "name": "nothing" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{empty}");
}

/// Every request that names a new Application generates a fresh identity, so
/// eight of them arriving at once used to commit eight Applications called
/// "blog". The name is unique at the commit, not only in the check above it.
#[tokio::test]
async fn concurrent_deploys_cannot_both_claim_one_name() {
    let dir = TempDir::new("race");
    let (app, store) = boot(&dir.state()).await;

    let mut requests = Vec::new();
    for _ in 0..8 {
        let app = app.clone();
        requests.push(tokio::spawn(async move {
            call(
                &app,
                "POST",
                "/apps",
                Some(json!({ "name": "blog", "image": "nginx" })),
            )
            .await
            .0
        }));
    }
    for request in requests {
        assert_eq!(request.await.unwrap(), StatusCode::ACCEPTED);
    }

    for _ in 0..200 {
        if store
            .list_applications()
            .await
            .unwrap()
            .iter()
            .all(|a| a.status != "pending")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    let committed = store.list_applications().await.unwrap();
    assert_eq!(committed.len(), 1, "{committed:?}");
    assert_eq!(committed[0].name, "blog");
}

#[tokio::test]
async fn development_image_identity_survives_renaming_rebuilding_and_restart() {
    async fn settled_image(app: &Router) -> Value {
        for _ in 0..200 {
            let (_, records) = call(app, "GET", "/custom-images", None).await;
            if records[0]["status"] == "ready" {
                return records;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("image build did not finish");
    }

    let dir = TempDir::new("development-image-identity");
    let (app, store) = boot(&dir.state()).await;
    let name = "Ambiente de desenvolvimento com acentos e um nome longo: São Paulo";
    let (status, first) = call(
        &app,
        "POST",
        "/custom-images",
        Some(json!({
            "name": name, "dependencies": [{"tool": "node", "version": "24"}]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let id = first["id"].as_str().unwrap();
    assert_eq!(id.len(), 12);
    assert!(
        id.bytes()
            .all(|byte| b"abcdefghijkmnpqrstuvwxyz23456789".contains(&byte))
    );
    assert_eq!(first["name"], name);
    assert_eq!(
        first["image"],
        format!("sf-img-{id}:e40f5fd794b982f958b5c581efd83dc1")
    );
    settled_image(&app).await;

    let (status, renamed) = call(
        &app,
        "POST",
        "/custom-images",
        Some(json!({
            "id": id, "name": "Renamed image", "dependencies": [{"tool": "node", "version": "24"}]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(renamed["id"], first["id"]);
    assert_eq!(renamed["image"], first["image"]);
    assert_eq!(settled_image(&app).await.as_array().unwrap().len(), 1);

    let (status, rebuilt) = call(
        &app,
        "POST",
        "/custom-images",
        Some(json!({
            "id": id, "name": "Renamed image", "dependencies": [{"tool": "node", "version": "22"}],
            "build_checks": ["node --version"], "template_id": "t3-code"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(rebuilt["id"], first["id"]);
    assert_ne!(rebuilt["image"], first["image"]);
    assert_eq!(rebuilt["build_checks"], json!(["node --version"]));
    assert_eq!(rebuilt["template_id"], "t3-code");

    settled_image(&app).await;
    let (status, with_setup) = call(
        &app,
        "POST",
        "/custom-images",
        Some(json!({
            "id": id, "name": "Renamed image", "dependencies": [{"tool": "node", "version": "22"}],
            "setup": ["curl -fsSL https://example.test/install.sh | bash"],
            "build_checks": ["node --version"], "template_id": "t3-code"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_ne!(with_setup["image"], rebuilt["image"]);
    assert_eq!(
        with_setup["setup"],
        json!(["curl -fsSL https://example.test/install.sh | bash"])
    );
    let saved = settled_image(&app).await;
    drop(app);
    drop(store);
    let (app, _) = boot(&dir.state()).await;
    let (status, reloaded) = call(&app, "GET", "/custom-images", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reloaded, saved);
}

#[tokio::test]
async fn an_edited_dockerfile_is_stored_verbatim_and_the_host_renders_the_generated_one() {
    let dir = TempDir::new("development-image-dockerfile");
    let (app, store) = boot(&dir.state()).await;

    let (status, text) = call_text(
        &app,
        "POST",
        "/custom-images/dockerfile",
        json!({
            "name": "", "dependencies": [{"tool": "node", "version": "24"}],
            "setup": ["curl -fsSL https://example.test/install.sh | bash"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(text.starts_with("FROM debian:13-slim\n"));
    assert!(text.contains("RUN curl -fsSL https://example.test/install.sh | bash\n"));
    assert!(text.contains("COPY <<'MISE_CONFIG_EOF' /opt/mise/config/config.toml\n"));

    let (status, refused) = call(
        &app,
        "POST",
        "/custom-images/dockerfile",
        Some(json!({"name": "", "dependencies": []})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(refused["error"].as_str().unwrap().contains("dependency"));

    let dockerfile = format!("{text}# edited by hand\n");
    let (status, saved) = call(
        &app,
        "POST",
        "/custom-images",
        Some(json!({"name": "hand written", "dependencies": [], "dockerfile": dockerfile})),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(saved["dockerfile"], dockerfile);
    assert!(saved["dependencies"].as_array().unwrap().is_empty());

    let (status, rejected) = call(
        &app,
        "POST",
        "/custom-images",
        Some(json!({"name": "no from", "dependencies": [], "dockerfile": "RUN true\n"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(rejected["error"].as_str().unwrap().contains("FROM"));

    for _ in 0..200 {
        let (_, records) = call(&app, "GET", "/custom-images", None).await;
        if records[0]["status"] == "ready" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let (_, before) = call(&app, "GET", "/custom-images", None).await;
    drop(app);
    drop(store);
    let (app, _) = boot(&dir.state()).await;
    let (status, reloaded) = call(&app, "GET", "/custom-images", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reloaded, before);
    assert_eq!(reloaded[0]["dockerfile"], dockerfile);
}

#[tokio::test]
async fn development_application_keeps_its_deployed_tag_and_form_settings() {
    async fn image_is_ready(app: &Router) -> Value {
        for _ in 0..200 {
            let (_, images) = call(app, "GET", "/custom-images", None).await;
            if images[0]["status"] == "ready" {
                return images[0].clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("custom image did not finish");
    }

    let dir = TempDir::new("development-application");
    let (app, store) = boot(&dir.state()).await;
    let (status, _) = call(
        &app,
        "POST",
        "/custom-images",
        Some(json!({
            "name": "T3", "dependencies": [{"tool": "node", "version": "24"}]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let first = image_is_ready(&app).await;
    let settings = json!({
        "image_id": first["id"],
        "tag": first["image"],
        "command": "t3 serve --host 0.0.0.0 --port 3000",
        "web_port": 3000,
        "persist_data": true
    });
    let (status, created) = call(
        &app,
        "POST",
        "/apps",
        Some(json!({"name": "t3", "development": settings})),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    let deployed = settled(&app, &id).await;
    assert_eq!(deployed["development"]["tag"], first["image"]);
    assert_eq!(
        deployed["development"]["command"],
        "t3 serve --host 0.0.0.0 --port 3000"
    );
    assert_eq!(deployed["development"]["persist_data"], true);

    let (status, mixed) = call(
        &app,
        "PUT",
        &format!("/apps/id/{id}"),
        Some(json!({"development": deployed["development"], "web_port": 9999})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        mixed["error"],
        "invalid custom image: development settings include the image and web target; do not send image, Compose, web service, or web port separately"
    );

    let (status, _) = call(
        &app,
        "POST",
        "/custom-images",
        Some(json!({
            "id": first["id"], "name": "T3", "dependencies": [{"tool": "node", "version": "22"}]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let newer = image_is_ready(&app).await;
    assert_ne!(newer["image"], first["image"]);

    let (status, saved) = call(
        &app,
        "PUT",
        &format!("/apps/id/{id}"),
        Some(json!({"development": deployed["development"]})),
    )
    .await;
    assert!(status.is_success(), "{saved}");
    let (status, rejected) = call(
        &app,
        "POST",
        "/apps",
        Some(json!({"name": "other", "development": deployed["development"]})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        rejected["error"],
        "invalid custom image: select a successful custom image build"
    );

    drop(app);
    drop(store);
    let (app, _) = boot(&dir.state()).await;
    let (status, reloaded) = call(&app, "GET", &format!("/apps/id/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reloaded["development"]["tag"], first["image"]);
}

#[tokio::test]
async fn development_image_delete_checks_usage_including_old_tags_and_compose() {
    let dir = TempDir::new("development-image-delete");
    let (app, store) = boot(&dir.state()).await;
    let id = "abcdefgh2345";
    let tag = format!("sf-img-{id}:new-version");
    store
        .put_record(
            "custom-image",
            id,
            &json!({
                "id": id, "name": "Saved image", "dependencies": [{"tool":"node","version":"24"}],
                "image": tag, "status": "ready", "last_error": null, "log": ""
            })
            .to_string(),
        )
        .await
        .unwrap();
    let (status, _) = call(
        &app,
        "POST",
        "/apps",
        Some(json!({
            "name": "using-image", "image": format!("sf-img-{id}:old-version")
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (_, records) = call(&app, "GET", "/custom-images", None).await;
    assert_eq!(records[0]["in_use"], true);
    let (status, _) = call(&app, "DELETE", &format!("/custom-images/{id}"), None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, apps) = call(&app, "GET", "/apps", None).await;
    let app_id = apps[0]["id"].as_str().unwrap();
    settled(&app, app_id).await;
    call_task(&app, "POST", &format!("/apps/id/{app_id}/stop"), None).await;
    assert_eq!(
        call(&app, "DELETE", &format!("/custom-images/{id}"), None)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call_task(&app, "DELETE", "/apps/using-image", None).await.1["status"],
        "completed"
    );

    let (status, compose) = call(&app, "POST", "/apps", Some(json!({
        "name": "using-compose", "compose": format!("services:\n  web:\n    image: {tag}\n    command: [sh, -c, 'echo hello']\n"),
        "web_service": "web", "web_port": 3000
    }))).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    settled(&app, compose["id"].as_str().unwrap()).await;
    assert_eq!(
        call(&app, "DELETE", &format!("/custom-images/{id}"), None)
            .await
            .0,
        StatusCode::CONFLICT
    );
    call_task(&app, "DELETE", "/apps/using-compose", None).await;
    let (_, records) = call(&app, "GET", "/custom-images", None).await;
    assert_eq!(records[0]["in_use"], false);
    assert_eq!(
        call_task(&app, "DELETE", &format!("/custom-images/{id}"), None)
            .await
            .1["status"],
        "completed"
    );
    assert_eq!(call(&app, "GET", "/custom-images", None).await.1, json!([]));
}

#[tokio::test]
async fn audit_keeps_identity_and_history_after_deletion_and_restart() {
    let dir = TempDir::new("audit-history");
    let (app, store) = boot(&dir.state()).await;
    let (status, created) = call(
        &app,
        "POST",
        "/apps",
        Some(json!({"name":"audited","image":"nginx:alpine"})),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let id = created["id"].as_str().unwrap();
    settled(&app, id).await;
    for action in ["stop", "start", "restart"] {
        let (_, event) = call_task(&app, "POST", &format!("/apps/id/{id}/{action}"), None).await;
        assert_eq!(event["status"], "completed", "{action}");
    }
    assert!(
        call(
            &app,
            "PUT",
            &format!("/apps/id/{id}"),
            Some(json!({"name":"renamed"}))
        )
        .await
        .0
        .is_success()
    );
    assert_eq!(
        call_task(&app, "DELETE", "/apps/renamed", None).await.1["status"],
        "completed"
    );
    let (status, before) = call(&app, "GET", "/events", None).await;
    assert_eq!(status, StatusCode::OK);
    let rows = before.as_array().unwrap();
    for action in ["create", "delete", "stop", "start", "restart", "configure"] {
        assert!(
            rows.iter()
                .any(|event| event["action"] == action && event["subject"]["id"] == id),
            "missing {action}"
        );
    }
    assert!(
        rows.iter()
            .all(|event| event["subject"]["available"] == false)
    );
    assert!(
        rows.iter()
            .any(|event| event["subject"]["name"] == "audited")
    );
    assert!(
        rows.iter()
            .any(|event| event["subject"]["name"] == "renamed")
    );
    drop(app);
    drop(store);
    let (restarted, _) = boot(&dir.state()).await;
    let (status, after) = call(&restarted, "GET", "/events", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(after, before);
}

/// Boots the way `serve` does, settling and requeuing the task queue first.
async fn boot_recovered(state: &Path, docker: FakeDocker) -> (Router, FileStateStore) {
    let store = FileStateStore::open(state).expect("open the Platform state");
    store.initialize().await.unwrap();
    let app = self_host::boot_app_with_vm_runtime(
        store.clone(),
        Arc::new(docker),
        Arc::new(FakeRoutes::new()),
        self_host::metrics::Metrics::new(),
        Arc::new(self_host::vms::LimaRuntime::default()),
        Arc::new(self_host::dns_records::UnservedZone),
        None,
    )
    .await;
    (app, store)
}

/// A restart runs again what never started, and fails what was running: a
/// half-done side effect is not repeated. Both keep their id and actor.
#[tokio::test]
async fn a_restart_requeues_pending_tasks_and_fails_running_ones() {
    use self_host::audit::Subject;
    use self_host::tasks::{Task, Work};
    let dir = TempDir::new("task-recovery");
    let (app, store) = boot(&dir.state()).await;
    let (status, blog) = call(
        &app,
        "POST",
        "/apps",
        Some(json!({"name": "blog", "image": "nginx:alpine"})),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{blog}");
    let id = blog["id"].as_str().unwrap().to_owned();
    settled(&app, &id).await;
    let subject = Subject {
        kind: "application".into(),
        id: id.clone(),
        name: "blog".into(),
        available: None,
    };
    let task = |task_id: &str, action: &str, status: &str, work: Work| Task {
        id: task_id.into(),
        action: action.into(),
        subject: subject.clone(),
        api_name: Some("laptop".into()),
        status: status.into(),
        work,
    };
    let queue = [
        task(
            "event-running",
            "restart",
            "running",
            Work::RestartApplication { id: id.clone() },
        ),
        task(
            "event-pending",
            "stop",
            "pending",
            Work::StopApplication { id: id.clone() },
        ),
    ];
    // What the daemon had on file when it went down: both events and both
    // tasks, one of them mid-way.
    let mut events: Vec<Value> = call(&app, "GET", "/events", None)
        .await
        .1
        .as_array()
        .unwrap()
        .clone();
    for (task, status, started) in [(&queue[0], "running", true), (&queue[1], "pending", false)] {
        events.push(json!({
            "id": task.id, "action": task.action, "status": status,
            "occurredAt": "2026-09-21T10:00:00.000000000Z",
            "startedAt": if started { json!("2026-09-21T10:00:01.000000000Z") } else { Value::Null },
            "finishedAt": null, "updatedAt": "2026-09-21T10:00:01.000000000Z",
            "apiName": "laptop", "description": "Operation queued.",
            "subject": {"kind": "application", "id": id, "name": "blog"}
        }));
    }
    events.push(json!({
        "id": "event-orphan", "action": "create", "status": "running",
        "occurredAt": "2026-09-21T09:00:00.000000000Z", "startedAt": "2026-09-21T09:00:00.000000000Z",
        "finishedAt": null, "updatedAt": "2026-09-21T09:00:00.000000000Z",
        "apiName": null, "description": "Operation is running in the background.",
        "subject": {"kind": "custom-image", "id": "image-gone", "name": "old"}
    }));
    for event in &events {
        store
            .put_audit_event(&self_host::store::AuditRow {
                id: event["id"].as_str().unwrap().into(),
                status: event["status"].as_str().unwrap().into(),
                subject_kind: event["subject"]["kind"].as_str().unwrap().into(),
                subject_id: event["subject"]["id"].as_str().unwrap().into(),
                occurred_at: event["occurredAt"].as_str().unwrap().into(),
                updated_at: event["updatedAt"].as_str().unwrap().into(),
                body: event.to_string(),
            })
            .await
            .unwrap();
    }
    store
        .replace_records(
            "task",
            &queue
                .iter()
                .map(|task| (task.id.clone(), serde_json::to_string(task).unwrap()))
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
    drop(app);
    drop(store);

    let docker = FakeDocker::new();
    let (app, store) = boot_recovered(&dir.state(), docker.clone()).await;
    let stopped = finished(&app, "event-pending").await;
    assert_eq!(stopped["status"], "completed");
    assert_eq!(stopped["apiName"], "laptop");
    assert_eq!(stopped["subject"]["id"], id);
    assert!(stopped["startedAt"].is_string());
    let (_, blog) = call(&app, "GET", &format!("/apps/id/{id}"), None).await;
    assert_eq!(blog["status"], "stopped");
    assert_eq!(docker.stopped.lock().unwrap().len(), 1);

    let interrupted = finished(&app, "event-running").await;
    assert_eq!(interrupted["status"], "failed");
    assert_eq!(interrupted["apiName"], "laptop");
    assert_eq!(
        interrupted["error"]["error"],
        "The Platform restarted before this task finished."
    );
    assert_eq!(interrupted["startedAt"], "2026-09-21T10:00:01.000000000Z");
    let orphan = finished(&app, "event-orphan").await;
    assert_eq!(orphan["status"], "failed");
    assert_eq!(
        orphan["error"]["error"],
        "The Platform restarted before this task finished."
    );
    assert!(store.list_records("task").await.unwrap().is_empty());
}

/// A task that fails says why, in the same shape the API answers errors in.
#[tokio::test]
async fn a_failed_task_reports_its_reason() {
    let dir = TempDir::new("task-failure");
    let store = FileStateStore::open(dir.state()).expect("open the Platform state");
    store.initialize().await.unwrap();
    store.store_state("dns_suffix", "home.lan").await.unwrap();
    store.store_state("api_key", API_KEY).await.unwrap();
    let app = build_app(
        store.clone(),
        Arc::new(FakeDocker {
            pull_failure: Some("manifest for nginx:nope not found".into()),
            ..FakeDocker::new()
        }),
        Arc::new(FakeRoutes::new()),
        self_host::metrics::Metrics::new(),
    );
    let (_, event) = call_task(
        &app,
        "POST",
        "/apps",
        Some(json!({"name": "blog", "image": "nginx:nope"})),
    )
    .await;
    assert_eq!(event["status"], "failed");
    assert_eq!(event["action"], "create");
    assert_eq!(event["error"]["error"], "failed to deploy the Application");
    assert!(
        event["error"]["caused_by"]
            .as_array()
            .unwrap()
            .iter()
            .any(|cause| cause.as_str().unwrap().contains("nginx:nope")),
        "{event}"
    );
}
