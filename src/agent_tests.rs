use std::net::IpAddr;
use std::sync::Arc;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

use crate::{
    build_app_with_vm_runtime,
    dns_records::FakeZone,
    docker::FakeDocker,
    environments::FakeVmRuntime,
    metrics::Metrics,
    routes::FakeRoutes,
    store::{ApiKeyRecord, FakeStateStore, StateStore},
    tailnet::{Arrival, NodeLookup},
};

const ROOT: &str = "root-secret";
const NODE_A: &str = "100.64.0.1";
const NODE_B: &str = "100.64.0.2";

struct Nodes;

#[async_trait::async_trait]
impl NodeLookup for Nodes {
    async fn node_of(&self, peer: IpAddr) -> Option<String> {
        match peer.to_string().as_str() {
            NODE_A => Some("nodeA".into()),
            NODE_B => Some("nodeB".into()),
            _ => None,
        }
    }
}

fn tailnet(peer: &str) -> Option<Arrival> {
    Some(Arrival {
        name: "host.tail1.ts.net".into(),
        peer: peer.parse().unwrap(),
        nodes: Arc::new(Nodes),
    })
}

async fn setup() -> (Router, FakeStateStore) {
    let store = FakeStateStore::new();
    store.store_state("api_key", ROOT).await.unwrap();
    store.store_state("dns_suffix", "home.lan").await.unwrap();
    let app = build_app_with_vm_runtime(
        store.clone(),
        Arc::new(FakeDocker::new()),
        Arc::new(FakeRoutes::new()),
        Metrics::new(),
        Arc::new(FakeVmRuntime),
        Arc::new(FakeZone::default()),
    );
    (app, store)
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    key: &str,
    body: Value,
    arrival: Option<Arrival>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {key}"))
        .header("content-type", "application/json")
        .body(Body::from(if body.is_null() {
            String::new()
        } else {
            body.to_string()
        }))
        .unwrap();
    if let Some(arrival) = arrival {
        request.extensions_mut().insert(arrival);
    }
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

pub(crate) async fn insert_key(store: &FakeStateStore, secret: &str, record: ApiKeyRecord) {
    let id = format!("sk-{}", &secret[..12]);
    store
        .store_state(
            &format!("api_key_digest:{id}"),
            &format!("{:x}", Sha256::digest(secret.as_bytes())),
        )
        .await
        .unwrap();
    store
        .create_api_key(&ApiKeyRecord { id, ..record })
        .await
        .unwrap();
}

async fn agent_key(app: &Router, body: Value) -> String {
    let (status, key) = call(app, "POST", "/api-keys", ROOT, body, None).await;
    assert_eq!(status, StatusCode::CREATED, "{key}");
    key["key"].as_str().unwrap().to_owned()
}

async fn deploy_blog(app: &Router) -> String {
    let (status, created) = call(
        app,
        "POST",
        "/apps",
        ROOT,
        json!({"name": "blog", "image": "nginx:alpine"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{created}");
    created["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn an_agent_key_cannot_reach_guarded_routes() {
    let (app, _) = setup().await;
    let id = deploy_blog(&app).await;
    let key = agent_key(&app, json!({"label": "claude", "kind": "agent"})).await;
    let host_mount = "services:\n  web:\n    image: nginx\n    volumes:\n      - /etc:/host-etc\n";
    for (method, path, body) in [
        ("POST", "/api-keys".to_owned(), json!({"label": "more"})),
        ("GET", format!("/apps/id/{id}/env"), Value::Null),
        (
            "POST",
            "/databases/db/connections/c/reveal".to_owned(),
            Value::Null,
        ),
        ("DELETE", format!("/apps/id/{id}"), Value::Null),
        ("PUT", "/settings".to_owned(), json!({})),
        (
            "POST",
            "/apps".to_owned(),
            json!({"name": "probe", "compose": host_mount}),
        ),
        (
            "POST",
            "/apps".to_owned(),
            json!({"name": "probe", "path": "/srv/probe"}),
        ),
    ] {
        let (status, body) = call(&app, method, &path, &key, body, tailnet(NODE_A)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {body}");
    }
}

#[tokio::test]
async fn an_agent_key_configures_only_on_the_tailnet_name() {
    let (app, _) = setup().await;
    let id = deploy_blog(&app).await;
    let key = agent_key(&app, json!({"label": "claude", "kind": "agent"})).await;
    for (method, path, body) in [
        (
            "PUT",
            format!("/apps/id/{id}/routes"),
            json!({"aliases": ["docs.home.lan"]}),
        ),
        (
            "POST",
            "/dns/records".to_owned(),
            json!({"name": "nas", "type": "A", "value": "192.168.1.30"}),
        ),
        (
            "POST",
            "/databases".to_owned(),
            json!({"name": "main", "major": 17}),
        ),
    ] {
        let (status, _) = call(&app, method, &path, &key, body.clone(), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
        let (status, answer) = call(&app, method, &path, &key, body, tailnet(NODE_A)).await;
        assert!(status.is_success(), "{method} {path}: {status} {answer}");
    }
}

#[tokio::test]
async fn agent_keys_expire_within_a_year() {
    let (app, _) = setup().await;
    let (status, key) = call(
        &app,
        "POST",
        "/api-keys",
        ROOT,
        json!({"label": "a", "kind": "agent"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(
        key["expires_at"].as_str().unwrap()[..10],
        crate::auth::days_from_now(30)[..10]
    );
    assert_eq!(
        key["scopes"],
        json!(["read", "apps:write", "dns:write", "databases:write"])
    );
    for expiry in [json!(366), json!("never"), json!(0)] {
        let (status, _) = call(
            &app,
            "POST",
            "/api-keys",
            ROOT,
            json!({"label": "b", "kind": "agent", "expires_in_days": expiry}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{expiry}");
    }
    let (status, _) = call(
        &app,
        "POST",
        "/api-keys",
        ROOT,
        json!({"label": "c", "kind": "agent", "scopes": ["read", "admin"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = call(
        &app,
        "POST",
        "/api-keys",
        ROOT,
        json!({"label": "e", "kind": "agent", "scopes": ["dns:write"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, key) = call(
        &app,
        "POST",
        "/api-keys",
        ROOT,
        json!({"label": "d", "expires_in_days": "never"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(key.get("expires_at").is_none_or(Value::is_null));
}

#[tokio::test]
async fn expired_revoked_and_foreign_node_keys_are_refused() {
    let (app, store) = setup().await;
    let agent = |label: &str| ApiKeyRecord {
        label: label.into(),
        kind: "agent".into(),
        scopes: Some(vec!["read".into()]),
        expires_at: Some(crate::auth::days_from_now(30)),
        ..Default::default()
    };
    insert_key(
        &store,
        "expiredkey00-secret",
        ApiKeyRecord {
            expires_at: Some(crate::auth::seconds_ago(1)),
            ..agent("expired")
        },
    )
    .await;
    insert_key(
        &store,
        "revokedkey00-secret",
        ApiKeyRecord {
            revoked_at: Some(crate::auth::now()),
            ..agent("revoked")
        },
    )
    .await;
    insert_key(
        &store,
        "boundkey0000-secret",
        ApiKeyRecord {
            node_id: Some("nodeA".into()),
            ..agent("bound")
        },
    )
    .await;
    for key in ["expiredkey00-secret", "revokedkey00-secret"] {
        let (status, _) = call(&app, "GET", "/apps", key, Value::Null, tailnet(NODE_A)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{key}");
    }
    let bound = "boundkey0000-secret";
    let (status, _) = call(&app, "GET", "/apps", bound, Value::Null, tailnet(NODE_A)).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&app, "GET", "/apps", bound, Value::Null, tailnet(NODE_B)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let key = agent_key(&app, json!({"label": "claude", "kind": "agent"})).await;
    let (status, _) = call(
        &app,
        "GET",
        "/apps",
        &key,
        Value::Null,
        tailnet("192.168.1.20"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_key_from_before_scopes_and_the_root_key_keep_full_access() {
    let (app, store) = setup().await;
    insert_key(
        &store,
        "oldhumankey0-secret",
        ApiKeyRecord {
            label: "laptop".into(),
            kind: "human".into(),
            ..Default::default()
        },
    )
    .await;
    for key in ["oldhumankey0-secret", ROOT] {
        let (status, _) = call(&app, "GET", "/api-keys", key, Value::Null, None).await;
        assert_eq!(status, StatusCode::OK, "{key}");
        let (status, whoami) = call(&app, "GET", "/whoami", key, Value::Null, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(whoami["kind"], "human");
        assert_eq!(whoami["scopes"].as_array().unwrap().len(), 9);
    }
}

#[tokio::test]
async fn whoami_names_an_agent_key() {
    let (app, _) = setup().await;
    let key = agent_key(&app, json!({"label": "claude", "kind": "agent"})).await;
    let (status, whoami) = call(&app, "GET", "/whoami", &key, Value::Null, tailnet(NODE_A)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(whoami["kind"], "agent");
    assert_eq!(whoami["label"], "claude");
}

#[tokio::test]
async fn an_agent_key_cannot_change_what_a_mount_reaches() {
    let (app, _) = setup().await;
    let read_only = "services:\n  web:\n    image: nginx\n    volumes:\n      - /etc:/data:ro\n";
    let from_variable =
        "services:\n  web:\n    image: nginx\n    volumes:\n      - ${MOUNT:-./data}:/data\n";
    let target_from_variable = "services:\n  web:\n    image: nginx\n    volumes:\n      - type: bind\n        source: /etc\n        target: ${TARGET:-/data}\n        read_only: true\n";
    let tagged_target = "services:\n  web:\n    image: nginx\n    volumes:\n      - type: bind\n        source: /etc\n        target: !test ${TARGET:-/data}\n        read_only: true\n";
    let mut ids = Vec::new();
    for (name, compose) in [
        ("mounted", read_only),
        ("chosen", from_variable),
        ("aimed", target_from_variable),
        ("tagged", tagged_target),
    ] {
        let (status, created) = call(
            &app,
            "POST",
            "/apps",
            ROOT,
            json!({"name": name, "compose": compose, "web_service": "web", "web_port": 80}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{created}");
        ids.push(created["id"].as_str().unwrap().to_owned());
    }
    let key = agent_key(&app, json!({"label": "claude", "kind": "agent"})).await;
    for (id, compose) in [(&ids[0], read_only), (&ids[1], from_variable)] {
        let (status, body) = call(
            &app,
            "PUT",
            &format!("/apps/id/{id}"),
            &key,
            json!({"compose": compose}),
            tailnet(NODE_A),
        )
        .await;
        assert_ne!(status, StatusCode::FORBIDDEN, "{body}");
    }
    for (method, path, body) in [
        (
            "PUT",
            format!("/apps/id/{}", ids[0]),
            json!({"compose": read_only.replace(":ro", "")}),
        ),
        (
            "POST",
            "/apps".to_owned(),
            json!({"name": "probe", "compose": from_variable}),
        ),
        (
            "POST",
            format!("/apps/id/{}/env", ids[1]),
            json!({"key": "MOUNT", "value": "/etc"}),
        ),
        (
            "DELETE",
            format!("/apps/id/{}/env/MOUNT", ids[1]),
            Value::Null,
        ),
        (
            "POST",
            format!("/apps/id/{}/env", ids[2]),
            json!({"key": "TARGET", "value": "/other"}),
        ),
        (
            "PUT",
            format!("/apps/id/{}", ids[0]),
            json!({"compose": read_only.replace("nginx", "${IMAGE}")}),
        ),
        (
            "PUT",
            format!("/apps/id/{}", ids[0]),
            json!({"compose": read_only.replace("/etc:", "${MOUNT:-/etc}:")}),
        ),
        (
            "PUT",
            format!("/apps/id/{}", ids[0]),
            json!({"compose": tagged_target}),
        ),
        (
            "POST",
            format!("/apps/id/{}/env", ids[3]),
            json!({"key": "TARGET", "value": "/other"}),
        ),
    ] {
        let (status, body) = call(&app, method, &path, &key, body, tailnet(NODE_A)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {body}");
    }
    let (status, body) = call(
        &app,
        "POST",
        &format!("/apps/id/{}/env", ids[0]),
        &key,
        json!({"key": "GREETING", "value": "hi"}),
        tailnet(NODE_A),
    )
    .await;
    assert_ne!(status, StatusCode::FORBIDDEN, "{body}");
}

#[tokio::test]
async fn retrying_a_failed_delete_needs_the_delete_scope() {
    let (app, _) = setup().await;
    let (status, created) = call(
        &app,
        "POST",
        "/virtual-machines",
        ROOT,
        json!({"request_id": "vm-1", "config": {
            "name": "alpha", "cpus": 2, "memory_gib": 4, "disk_gib": 20,
            "ssh_public_key": "ssh-ed25519 AAAA test", "command": "", "web_port": 3000,
            "recipe": {"name": "T3", "template_id": "t3-code", "dependencies": [], "setup": [], "build_checks": []}
        }}),
        None,
    )
    .await;
    assert!(status.is_success(), "{status} {created}");
    let id = created["id"].as_str().unwrap().to_owned();
    let path = format!("/virtual-machines/{id}/actions");
    let wait_for_failure = || async {
        for _ in 0..50 {
            let (_, current) = call(
                &app,
                "GET",
                &format!("/virtual-machines/{id}"),
                ROOT,
                Value::Null,
                None,
            )
            .await;
            if current["operation"]["status"] == "failed" {
                return current;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("the operation never failed");
    };
    wait_for_failure().await;
    let delete = json!({"action": "delete", "confirm_name": "alpha"});
    let (status, body) = call(&app, "POST", &path, ROOT, delete, None).await;
    assert!(status.is_success(), "{status} {body}");
    assert_eq!(wait_for_failure().await["operation"]["action"], "delete");
    let key = agent_key(
        &app,
        json!({"label": "claude", "kind": "agent", "scopes": ["read", "vms:write"]}),
    )
    .await;
    let (status, body) = call(
        &app,
        "POST",
        &path,
        &key,
        json!({"action": "retry", "confirm_name": "alpha"}),
        tailnet(NODE_A),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}
