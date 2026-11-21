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
    audit, build_app_with_vm_runtime,
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

async fn rpc(app: &Router, key: &str, method: &str, params: Value) -> (StatusCode, Value) {
    call(
        app,
        "POST",
        "/mcp",
        key,
        json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}),
        tailnet(NODE_A),
    )
    .await
}

#[tokio::test]
async fn mcp_lists_the_allowed_tools_and_runs_them_as_the_agent() {
    let (app, store) = setup().await;
    let id = deploy_blog(&app).await;
    let key = agent_key(&app, json!({"label": "claude", "kind": "agent"})).await;

    let (status, answer) = rpc(
        &app,
        &key,
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(answer["result"]["protocolVersion"], "2025-06-18");

    let (_, answer) = rpc(&app, &key, "tools/list", json!({})).await;
    let names: Vec<&str> = answer["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"set_routes"), "{names:?}");
    assert!(!names.contains(&"delete_app"), "{names:?}");

    let (_, answer) = rpc(
        &app,
        &key,
        "tools/call",
        json!({"name": "set_routes", "arguments": {"id": id, "aliases": ["docs.home.lan"]}}),
    )
    .await;
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    let (_, saved) = call(
        &app,
        "GET",
        &format!("/apps/id/{id}"),
        ROOT,
        Value::Null,
        None,
    )
    .await;
    assert_eq!(saved["aliases"], json!(["docs.home.lan"]), "{saved}");
    let events = audit::read(&store).await.unwrap();
    let event = events
        .iter()
        .find(|event| event.api_name.as_deref() == Some("claude"))
        .expect("the tool call is in the audit log");
    assert_eq!(event.actor_kind.as_deref(), Some("agent"));
    assert_eq!(event.via.as_deref(), Some("mcp"));
    assert_eq!(event.tool.as_deref(), Some("set_routes"));

    let (_, answer) = rpc(
        &app,
        &key,
        "tools/call",
        json!({"name": "delete_app", "arguments": {"id": id}}),
    )
    .await;
    assert_eq!(answer["result"]["isError"], true);
    let text = answer["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("403"), "{text}");
}

#[tokio::test]
async fn update_app_refuses_an_unknown_field_and_names_set_routes() {
    let (app, _) = setup().await;
    let id = deploy_blog(&app).await;
    let key = agent_key(&app, json!({"label": "claude", "kind": "agent"})).await;

    let (_, answer) = rpc(
        &app,
        &key,
        "tools/call",
        json!({"name": "update_app", "arguments": {"id": id, "aliases": "teste.home.lan"}}),
    )
    .await;
    assert_eq!(answer["result"]["isError"], true, "{answer}");
    let text = answer["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("aliases") && text.contains("set_routes"),
        "{text}"
    );
    assert!(!text.starts_with("422"), "{text}");
    let (_, saved) = call(
        &app,
        "GET",
        &format!("/apps/id/{id}"),
        ROOT,
        Value::Null,
        None,
    )
    .await;
    assert_eq!(saved["aliases"], json!([]), "{saved}");
}

#[tokio::test]
async fn mcp_answers_only_on_the_tailnet_name_and_its_own_origin() {
    let (app, _) = setup().await;
    let key = agent_key(&app, json!({"label": "claude", "kind": "agent"})).await;
    let request = |origin: &str| {
        let mut request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("authorization", format!("Bearer {key}"))
            .header("content-type", "application/json")
            .header("host", "host.tail1.ts.net:8443")
            .header("origin", origin)
            .body(Body::from(
                json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}).to_string(),
            ))
            .unwrap();
        request.extensions_mut().insert(tailnet(NODE_A).unwrap());
        request
    };
    let status = |request: Request<Body>| {
        let app = app.clone();
        async move { app.oneshot(request).await.unwrap().status() }
    };
    assert_eq!(
        status(request("https://evil.example")).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(request("https://host.tail1.ts.net")).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(request("https://host.tail1.ts.net:8443")).await,
        StatusCode::OK
    );
    let mut without_host = request("https://host.tail1.ts.net:8443");
    without_host.headers_mut().remove("host");
    *without_host.uri_mut() = "https://host.tail1.ts.net:8443/mcp".parse().unwrap();
    assert_eq!(status(without_host).await, StatusCode::OK);
    let (status, _) = call(
        &app,
        "POST",
        "/mcp",
        ROOT,
        json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
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

async fn insert_git_app(store: &FakeStateStore) {
    let git: crate::source::GitSource = serde_json::from_value(json!({
        "repository": "https://git.example.invalid/yard.git",
        "git_ref": "main",
        "build_args": {"PG_YARD_EXTRA_CA": "pem-body", "KEEP": "me"},
        "build_target": "runtime"
    }))
    .unwrap();
    store
        .insert_application(&crate::apps::ApplicationRecord {
            id: "yard".into(),
            name: "yard".into(),
            hostname: "yard.home.lan".into(),
            aliases: vec![],
            image: "self-host-yard:built".into(),
            status: crate::apps::STATUS_RUNNING.into(),
            source: crate::apps::SOURCE_GIT.into(),
            git: Some(git),
            git_build: None,
            last_error: None,
            compose: None,
            web_service: None,
            web_port: None,
            web_target_port: Some(20001),
            development: None,
            runtime: Default::default(),
            publication: Default::default(),
            variable_delivery: crate::store::VariableDelivery::Referenced,
            route_rules: Vec::new(),
            redirects: Vec::new(),
            rewrite_host: None,
            network_policy: Default::default(),
        })
        .await
        .unwrap();
}

async fn host_agent_key(app: &Router) -> String {
    agent_key(
        app,
        json!({"label": "claude", "kind": "agent", "scopes": ["read", "apps:write", "host"]}),
    )
    .await
}

async fn tool_names(app: &Router, key: &str) -> Vec<String> {
    let (_, answer) = rpc(app, key, "tools/list", json!({})).await;
    answer["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn update_source_removes_one_build_arg_and_keeps_the_rest() {
    let (app, store) = setup().await;
    insert_git_app(&store).await;
    let key = host_agent_key(&app).await;
    assert!(
        tool_names(&app, &key)
            .await
            .contains(&"update_source".to_owned())
    );

    let (_, answer) = rpc(
        &app,
        &key,
        "tools/call",
        json!({"name": "update_source", "arguments": {"id": "yard", "build_args": {"PG_YARD_EXTRA_CA": null}}}),
    )
    .await;
    assert_eq!(answer["result"]["isError"], false, "{answer}");
    let output = answer["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        !output.contains("pem-body") && !output.contains("\"me\""),
        "{output}"
    );
    let task_id = answer["result"]["structuredContent"]["task_id"]
        .as_str()
        .unwrap();

    let queued = crate::collection::TASKS.list(&store).await.unwrap();
    let task = queued
        .iter()
        .find(|task| task.id == task_id)
        .expect("the rebuild is queued");
    assert_eq!(task.api_name.as_deref(), Some("claude"));
    let crate::tasks::Work::BuildGitApplication { pending } = &task.work else {
        panic!("expected a git build, got {:?}", task.work);
    };
    let git = &pending.source;
    assert_eq!(git.build_args.len(), 1);
    assert_eq!(git.build_args["KEEP"], "me");
    assert_eq!(git.git_ref, "main");
    assert_eq!(git.build_target.as_deref(), Some("runtime"));
    assert_eq!(git.repository, "https://git.example.invalid/yard.git");
}

#[tokio::test]
async fn update_source_refuses_unknown_fields() {
    let (app, store) = setup().await;
    insert_git_app(&store).await;
    let key = host_agent_key(&app).await;
    let (_, answer) = rpc(
        &app,
        &key,
        "tools/call",
        json!({"name": "update_source", "arguments": {"id": "yard", "image": "nginx"}}),
    )
    .await;
    assert_eq!(answer["result"]["isError"], true, "{answer}");
    let text = answer["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("image") && text.contains("update_source"),
        "{text}"
    );
}

#[tokio::test]
async fn an_agent_key_without_host_neither_sees_nor_uses_update_source() {
    let (app, store) = setup().await;
    insert_git_app(&store).await;
    let key = agent_key(&app, json!({"label": "claude", "kind": "agent"})).await;
    assert!(
        !tool_names(&app, &key)
            .await
            .contains(&"update_source".to_owned())
    );
    let (_, answer) = rpc(
        &app,
        &key,
        "tools/call",
        json!({"name": "update_source", "arguments": {"id": "yard", "build_args": {"KEEP": null}}}),
    )
    .await;
    assert_eq!(answer["result"]["isError"], true, "{answer}");
    let text = answer["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("403"), "{text}");
}
