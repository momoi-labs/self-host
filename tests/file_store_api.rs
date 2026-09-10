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

    let (status, _) = call(
        &app,
        "POST",
        "/apps/blog/env",
        Some(json!({ "key": "TOKEN", "value": "secret" })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, stopped) = call(&app, "POST", &format!("/apps/id/{blog_id}/stop"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stopped["status"], "stopped");

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
            let (_, records) = call(app, "GET", "/dev-images", None).await;
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
        "/dev-images",
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
        format!("sf-img-{id}:309722a26f8643d88330cb249886243b")
    );
    settled_image(&app).await;

    let (status, renamed) = call(
        &app,
        "POST",
        "/dev-images",
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
        "/dev-images",
        Some(json!({
            "id": id, "name": "Renamed image", "dependencies": [{"tool": "node", "version": "22"}]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(rebuilt["id"], first["id"]);
    assert_ne!(rebuilt["image"], first["image"]);
    let saved = settled_image(&app).await;
    drop(app);
    drop(store);
    let (app, _) = boot(&dir.state()).await;
    let (status, reloaded) = call(&app, "GET", "/dev-images", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reloaded, saved);
}

#[tokio::test]
async fn development_application_keeps_its_deployed_tag_and_form_settings() {
    async fn image_is_ready(app: &Router) -> Value {
        for _ in 0..200 {
            let (_, images) = call(app, "GET", "/dev-images", None).await;
            if images[0]["status"] == "ready" {
                return images[0].clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("development image did not finish");
    }

    let dir = TempDir::new("development-application");
    let (app, store) = boot(&dir.state()).await;
    let (status, _) = call(
        &app,
        "POST",
        "/dev-images",
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
        "invalid development image: development settings include the image and web target; do not send image, Compose, web service, or web port separately"
    );

    let (status, _) = call(
        &app,
        "POST",
        "/dev-images",
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
        "invalid development image: select a successful development image build"
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
        .store_state(
            "development_images_v1",
            &json!([{
                "id": id, "name": "Saved image", "dependencies": [{"tool":"node","version":"24"}],
                "image": tag, "status": "ready", "last_error": null, "log": ""
            }])
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
    let (_, records) = call(&app, "GET", "/dev-images", None).await;
    assert_eq!(records[0]["in_use"], true);
    let (status, _) = call(&app, "DELETE", &format!("/dev-images/{id}"), None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, apps) = call(&app, "GET", "/apps", None).await;
    let app_id = apps[0]["id"].as_str().unwrap();
    settled(&app, app_id).await;
    call(&app, "POST", &format!("/apps/id/{app_id}/stop"), None).await;
    assert_eq!(
        call(&app, "DELETE", &format!("/dev-images/{id}"), None)
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(&app, "DELETE", "/apps/using-image", None).await.0,
        StatusCode::NO_CONTENT
    );

    let (status, compose) = call(&app, "POST", "/apps", Some(json!({
        "name": "using-compose", "compose": format!("services:\n  web:\n    image: {tag}\n    command: [sh, -c, 'echo hello']\n"),
        "web_service": "web", "web_port": 3000
    }))).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    settled(&app, compose["id"].as_str().unwrap()).await;
    assert_eq!(
        call(&app, "DELETE", &format!("/dev-images/{id}"), None)
            .await
            .0,
        StatusCode::CONFLICT
    );
    call(&app, "DELETE", "/apps/using-compose", None).await;
    let (_, records) = call(&app, "GET", "/dev-images", None).await;
    assert_eq!(records[0]["in_use"], false);
    assert_eq!(
        call(&app, "DELETE", &format!("/dev-images/{id}"), None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(call(&app, "GET", "/dev-images", None).await.1, json!([]));
}
