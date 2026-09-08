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
