use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{HeaderMap, Request, StatusCode, header},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tower::ServiceExt;
use webauthn_authenticator_rs::{WebauthnAuthenticator, softpasskey::SoftPasskey};
use webauthn_rs::prelude::{CreationChallengeResponse, RequestChallengeResponse, Url};

use crate::{
    AppState, account, build_platform,
    dns_records::FakeZone,
    docker::FakeDocker,
    environments::FakeVmRuntime,
    metrics::Metrics,
    routes::FakeRoutes,
    store::{ApiKeyRecord, FakeStateStore, StateStore},
};

const ROOT: &str = "root-secret";
const PERSON: &str = "personkey000-secret";
const HOST: &str = "console.home.lan";
const ORIGIN: &str = "https://console.home.lan";
const RECOVERY: &str = "recovery-secret";

struct Platform {
    app: Router,
    state: AppState<FakeStateStore>,
}

async fn platform() -> Platform {
    let store = FakeStateStore::new();
    store.store_state("api_key", ROOT).await.unwrap();
    store.store_state("dns_suffix", "home.lan").await.unwrap();
    store
        .store_state(account::RECOVERY_SECRET_STATE, RECOVERY)
        .await
        .unwrap();
    crate::agent_tests::insert_key(
        &store,
        PERSON,
        ApiKeyRecord {
            label: "laptop".into(),
            kind: "human".into(),
            ..Default::default()
        },
    )
    .await;
    let (app, state) = build_platform(
        store,
        Arc::new(FakeDocker::new()),
        Arc::new(FakeRoutes::new()),
        Metrics::new(),
        Arc::new(FakeVmRuntime),
        Arc::new(FakeZone::default()),
        None,
    );
    Platform { app, state }
}

fn origin() -> Url {
    Url::parse(ORIGIN).unwrap()
}

#[derive(Default, Clone)]
struct Call<'a> {
    bearer: Option<&'a str>,
    cookie: Option<&'a str>,
    host: Option<&'a str>,
    origin: Option<&'a str>,
    peer: Option<&'a str>,
    recovery: Option<&'a str>,
}

impl<'a> Call<'a> {
    fn console() -> Self {
        Call {
            host: Some(HOST),
            origin: Some(ORIGIN),
            ..Default::default()
        }
    }

    fn bearer(key: &'a str) -> Self {
        Call {
            bearer: Some(key),
            ..Call::console()
        }
    }

    fn session(cookie: &'a str) -> Self {
        Call {
            cookie: Some(cookie),
            ..Call::console()
        }
    }
}

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    call: Call<'_>,
    body: Value,
) -> (StatusCode, HeaderMap, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(key) = call.bearer {
        request = request.header(header::AUTHORIZATION, format!("Bearer {key}"));
    }
    if let Some(cookie) = call.cookie {
        request = request.header(header::COOKIE, cookie);
    }
    if let Some(host) = call.host {
        request = request.header(header::HOST, host);
    }
    if let Some(origin) = call.origin {
        request = request.header(header::ORIGIN, origin);
    }
    if let Some(secret) = call.recovery {
        request = request.header(account::RECOVERY_HEADER, secret);
    }
    let mut request = request
        .body(Body::from(if body.is_null() {
            String::new()
        } else {
            body.to_string()
        }))
        .unwrap();
    if let Some(peer) = call.peer {
        request
            .extensions_mut()
            .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    }
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn register(app: &Router, key: &str, passkey: &mut SoftPasskey, name: &str) -> StatusCode {
    let (status, _, started) = send(
        app,
        "POST",
        "/account/setup/start",
        Call::bearer(key),
        json!({"name": name}),
    )
    .await;
    if status != StatusCode::OK {
        return status;
    }
    let options: CreationChallengeResponse =
        serde_json::from_value(started["options"].clone()).unwrap();
    let credential = passkey.do_registration(origin(), options).unwrap();
    let (status, _, _) = send(
        app,
        "POST",
        "/account/setup/finish",
        Call::bearer(key),
        json!({"registration": started["registration"], "credential": credential}),
    )
    .await;
    status
}

fn session_cookie(headers: &HeaderMap) -> String {
    let set = headers
        .get(header::SET_COOKIE)
        .expect("a login sets the session cookie")
        .to_str()
        .unwrap()
        .to_owned();
    set.split(';').next().unwrap().to_owned()
}

async fn login(app: &Router, passkey: &mut SoftPasskey) -> (StatusCode, HeaderMap, Value) {
    let (status, headers, started) = send(
        app,
        "POST",
        "/account/login/start",
        Call::console(),
        Value::Null,
    )
    .await;
    if status != StatusCode::OK {
        return (status, headers, started);
    }
    let options: RequestChallengeResponse =
        serde_json::from_value(started["options"].clone()).unwrap();
    let credential = passkey.do_authentication(origin(), options).unwrap();
    send(
        app,
        "POST",
        "/account/login/finish",
        Call::console(),
        json!({"login": started["login"], "credential": credential}),
    )
    .await
}

async fn operator(app: &Router) -> (SoftPasskey, String) {
    let mut passkey = SoftPasskey::new(true);
    assert_eq!(
        register(app, ROOT, &mut passkey, "laptop").await,
        StatusCode::CREATED
    );
    let (status, headers, body) = login(app, &mut passkey).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let cookie = session_cookie(&headers);
    (passkey, cookie)
}

async fn step_up(app: &Router, cookie: &str, passkey: &mut SoftPasskey, request: Value) -> Value {
    let (status, _, started) = send(
        app,
        "POST",
        "/account/step-up",
        Call::session(cookie),
        request,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let options: RequestChallengeResponse =
        serde_json::from_value(started["options"].clone()).unwrap();
    let credential = passkey.do_authentication(origin(), options).unwrap();
    json!({"id": started["step_up"], "credential": credential})
}

#[tokio::test]
async fn the_root_key_registers_the_first_passkey_and_a_passkey_login_opens_a_session() {
    let Platform { app, .. } = platform().await;
    let mut passkey = SoftPasskey::new(true);
    assert_eq!(
        register(&app, ROOT, &mut passkey, "laptop").await,
        StatusCode::CREATED
    );

    let (status, headers, body) = login(&app, &mut passkey).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let set = headers.get(header::SET_COOKIE).unwrap().to_str().unwrap();
    assert!(set.starts_with(&format!("{}=", account::SESSION_COOKIE)));
    for attribute in [
        "Path=/",
        "Secure",
        "HttpOnly",
        "SameSite=Strict",
        "Max-Age=",
    ] {
        assert!(set.contains(attribute), "{set} lacks {attribute}");
    }
    assert!(!set.contains("Domain"), "{set}");
    let cookie = session_cookie(&headers);

    let (status, _, whoami) =
        send(&app, "GET", "/whoami", Call::session(&cookie), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(whoami["label"], "operator");
    let (status, _, _) = send(&app, "GET", "/apps", Call::session(&cookie), Value::Null).await;
    assert_eq!(status, StatusCode::OK);

    let mut second = SoftPasskey::new(true);
    assert_eq!(
        register(&app, ROOT, &mut second, "phone").await,
        StatusCode::UNAUTHORIZED,
        "the first passkey login retired the root key"
    );

    let (status, headers, _) = send(
        &app,
        "POST",
        "/account/logout",
        Call::session(&cookie),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        headers
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    let (status, _, _) = send(&app, "GET", "/apps", Call::session(&cookie), Value::Null).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_passkey_without_user_verification_is_refused() {
    let Platform { app, .. } = platform().await;
    let (status, _, started) = send(
        &app,
        "POST",
        "/account/setup/start",
        Call::bearer(ROOT),
        json!({"name": "key"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let options: CreationChallengeResponse =
        serde_json::from_value(started["options"].clone()).unwrap();
    assert!(
        SoftPasskey::new(false)
            .do_registration(origin(), options)
            .is_err(),
        "the registration asks for user verification"
    );
}

#[tokio::test]
async fn a_session_opens_a_websocket_only_from_the_console_origin() {
    let Platform { app, .. } = platform().await;
    let (_, cookie) = operator(&app).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let url = format!("ws://{address}/host/terminal");

    let upgrade = |cookie: Option<&str>, origin: &str| {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut request = url.clone().into_client_request().unwrap();
        request
            .headers_mut()
            .insert(header::ORIGIN, origin.parse().unwrap());
        if let Some(cookie) = cookie {
            request
                .headers_mut()
                .insert(header::COOKIE, cookie.parse().unwrap());
        }
        request
    };
    let first = |request| async move {
        let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            json!({"cols": 80, "rows": 24}).to_string().into(),
        ))
        .await
        .unwrap();
        let message = tokio::time::timeout(Duration::from_secs(2), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let reply: Value = serde_json::from_str(message.to_text().unwrap()).unwrap();
        reply["report"]["error"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };

    assert!(
        first(upgrade(Some(&cookie), ORIGIN))
            .await
            .contains("Host terminal is off"),
        "the session passed authentication"
    );
    assert!(
        first(upgrade(None, ORIGIN))
            .await
            .contains("Invalid API key")
    );
    match tokio_tungstenite::connect_async(upgrade(Some(&cookie), "https://app.home.lan")).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            assert_eq!(response.status(), StatusCode::FORBIDDEN)
        }
        other => panic!("a cross-origin upgrade opened: {:?}", other.map(|_| ())),
    }
    server.abort();
}

#[tokio::test]
async fn a_session_refuses_cross_origin_state_changes() {
    let Platform { app, .. } = platform().await;
    let (_, cookie) = operator(&app).await;
    for origin in [Some("https://app.home.lan"), None] {
        let call = Call {
            origin,
            ..Call::session(&cookie)
        };
        let (status, _, body) = send(
            &app,
            "POST",
            "/apps",
            call.clone(),
            json!({"name": "blog", "image": "nginx"}),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{origin:?}: {body}");
        let (status, _, _) = send(&app, "POST", "/account/logout", call, Value::Null).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{origin:?}");
    }
    let (status, _, _) = send(
        &app,
        "GET",
        "/apps",
        Call {
            origin: Some("https://app.home.lan"),
            ..Call::session(&cookie)
        },
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = send(
        &app,
        "GET",
        "/apps",
        Call {
            origin: None,
            ..Call::session(&cookie)
        },
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn passkeys_work_only_on_the_canonical_console_name() {
    let Platform { app, .. } = platform().await;
    let mut passkey = SoftPasskey::new(true);
    assert_eq!(
        register(&app, ROOT, &mut passkey, "laptop").await,
        StatusCode::CREATED
    );
    for host in ["localhost", "host.tail1.ts.net", "console.other.lan"] {
        let (status, _, body) = send(
            &app,
            "POST",
            "/account/login/start",
            Call {
                host: Some(host),
                origin: None,
                ..Default::default()
            },
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{host}");
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("https://console.home.lan/console"),
            "{body}"
        );
    }

    let (status, _, body) = send(
        &app,
        "POST",
        "https://console.home.lan/account/login/start",
        Call {
            host: None,
            ..Call::console()
        },
        Value::Null,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "HTTP/2 names the host in the URI, not in Host: {body}"
    );
}

#[tokio::test]
async fn a_bearer_key_alone_cannot_create_a_person_or_host_key() {
    let Platform { app, .. } = platform().await;
    for key in [ROOT, PERSON] {
        for request in [
            json!({"label": "person", "kind": "human", "scopes": ["read"]}),
            json!({"label": "default-person"}),
            json!({"label": "host-agent", "kind": "agent", "scopes": ["read", "host"]}),
        ] {
            let (status, _, body) = send(
                &app,
                "POST",
                "/api-keys",
                Call::bearer(key),
                request.clone(),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{key} {request}: {body}");
        }
    }
    let (status, _, body) = send(
        &app,
        "POST",
        "/api-keys",
        Call::bearer(ROOT),
        json!({"label": "bot", "kind": "agent"}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert!(body.get("authorized_by").is_none());
}

#[tokio::test]
async fn a_session_with_a_bound_passkey_assertion_creates_a_powerful_key_once() {
    let Platform { app, .. } = platform().await;
    let (mut passkey, cookie) = operator(&app).await;
    let request = json!({
        "label": "ops",
        "kind": "human",
        "scopes": ["read", "host"],
        "expires_in_days": "never"
    });

    let (status, _, _) = send(
        &app,
        "POST",
        "/api-keys",
        Call::session(&cookie),
        request.clone(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a session alone is not enough"
    );

    let proof = step_up(
        &app,
        &cookie,
        &mut passkey,
        json!({"purpose": "create_key", "key": request}),
    )
    .await;
    let mut confirmed = request.clone();
    confirmed["step_up"] = proof.clone();
    let (status, _, created) = send(
        &app,
        "POST",
        "/api-keys",
        Call::session(&cookie),
        confirmed.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["authorized_by"]["passkey_name"], "laptop");
    assert!(created["authorized_by"]["account"].is_string());
    assert!(created.get("expires_at").is_none());

    let (_, _, events) = send(&app, "GET", "/events", Call::session(&cookie), Value::Null).await;
    let events = events.to_string();
    assert!(events.contains("authorizedBy"), "{events}");
    assert!(events.contains("\"passkey_name\":\"laptop\""), "{events}");

    let (_, _, keys) = send(
        &app,
        "GET",
        "/api-keys",
        Call::session(&cookie),
        Value::Null,
    )
    .await;
    let listed = keys
        .as_array()
        .unwrap()
        .iter()
        .find(|key| key["label"] == "ops")
        .expect("the new key is listed");
    assert_eq!(listed["authorized_by"], created["authorized_by"]);

    confirmed["label"] = json!("ops-again");
    let (status, _, body) =
        send(&app, "POST", "/api-keys", Call::session(&cookie), confirmed).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a replay: {body}");
}

#[tokio::test]
async fn a_step_up_for_one_request_does_not_authorize_another() {
    let Platform { app, state } = platform().await;
    let (mut passkey, cookie) = operator(&app).await;
    let narrow = json!({"label": "ops", "kind": "human", "scopes": ["read"]});
    let proof = step_up(
        &app,
        &cookie,
        &mut passkey,
        json!({"purpose": "create_key", "key": narrow}),
    )
    .await;
    let (status, _, body) = send(
        &app,
        "POST",
        "/api-keys",
        Call::session(&cookie),
        json!({"label": "ops", "kind": "human", "scopes": ["read", "host"], "step_up": proof}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let mut retry = narrow.clone();
    retry["step_up"] = proof;
    let (status, _, _) = send(&app, "POST", "/api-keys", Call::session(&cookie), retry).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a refused proof is spent, not kept for a second try"
    );

    let proof = step_up(
        &app,
        &cookie,
        &mut passkey,
        json!({"purpose": "create_key", "key": narrow}),
    )
    .await;
    account::expire_ceremonies(&state.account);
    let mut late = narrow;
    late["step_up"] = proof;
    let (status, _, _) = send(&app, "POST", "/api-keys", Call::session(&cookie), late).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "an expired step-up");
}

#[tokio::test]
async fn a_bearer_key_cannot_start_a_step_up() {
    let Platform { app, .. } = platform().await;
    operator(&app).await;
    let (status, _, _) = send(
        &app,
        "POST",
        "/account/step-up",
        Call::bearer(PERSON),
        json!({"purpose": "create_key", "key": {"label": "ops"}}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn the_last_passkey_cannot_be_removed() {
    let Platform { app, .. } = platform().await;
    let (mut laptop, cookie) = operator(&app).await;

    let proof = step_up(
        &app,
        &cookie,
        &mut laptop,
        json!({"purpose": "add_passkey", "name": "phone"}),
    )
    .await;
    let (status, _, started) = send(
        &app,
        "POST",
        "/account/passkeys/start",
        Call::session(&cookie),
        json!({"name": "phone", "step_up": proof}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let options: CreationChallengeResponse =
        serde_json::from_value(started["options"].clone()).unwrap();
    let mut phone = SoftPasskey::new(true);
    let credential = phone.do_registration(origin(), options).unwrap();
    let (status, _, _) = send(
        &app,
        "POST",
        "/account/passkeys/finish",
        Call::session(&cookie),
        json!({"registration": started["registration"], "credential": credential}),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (_, _, account) = send(&app, "GET", "/account", Call::session(&cookie), Value::Null).await;
    let ids: Vec<String> = account["passkeys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids.len(), 2);

    let (status, _, _) = send(
        &app,
        "DELETE",
        &format!("/account/passkeys/{}", ids[0]),
        Call::bearer(PERSON),
        Value::Null,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a Bearer key removes no passkey"
    );
    let (status, _, _) = send(
        &app,
        "DELETE",
        &format!("/account/passkeys/{}", ids[0]),
        Call::session(&cookie),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, _) = send(
        &app,
        "DELETE",
        &format!("/account/passkeys/{}", ids[1]),
        Call::session(&cookie),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

async fn recover(app: &Router, peer: Option<&str>, secret: Option<&str>) -> (StatusCode, Value) {
    let (status, _, body) = send(
        app,
        "POST",
        "/account/recover",
        Call {
            peer,
            recovery: secret,
            ..Default::default()
        },
        Value::Null,
    )
    .await;
    (status, body)
}

#[tokio::test]
async fn recovery_answers_only_on_loopback_with_the_state_secret() {
    let Platform { app, .. } = platform().await;
    for (peer, secret) in [
        (None, Some(RECOVERY)),
        (Some("192.168.1.20:5000"), Some(RECOVERY)),
        (Some("[::ffff:100.64.0.1]:5000"), Some(RECOVERY)),
        (Some("127.0.0.1:5000"), None),
        (Some("127.0.0.1:5000"), Some("wrong")),
    ] {
        let (status, _) = recover(&app, peer, secret).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{peer:?} {secret:?}");
    }
    for peer in ["127.0.0.1:5000", "[::1]:5000", "[::ffff:127.0.0.1]:5000"] {
        let (status, body) = recover(&app, Some(peer), Some(RECOVERY)).await;
        assert_eq!(status, StatusCode::OK, "{peer}");
        assert_eq!(body["expires_in_seconds"], 900);
        assert_eq!(body["console_url"], "https://console.home.lan/console");
    }
}

#[tokio::test]
async fn a_recovery_key_registers_one_passkey_once_and_opens_nothing_else() {
    let Platform { app, state } = platform().await;
    let (_, old_cookie) = operator(&app).await;
    let (_, body) = recover(&app, Some("127.0.0.1:5000"), Some(RECOVERY)).await;
    let setup = body["setup_key"].as_str().unwrap().to_owned();

    for (method, path) in [("GET", "/apps"), ("GET", "/account"), ("POST", "/api-keys")] {
        let (status, _, _) = send(
            &app,
            method,
            path,
            Call::bearer(&setup),
            json!({"label": "x", "kind": "agent"}),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
    }

    let mut new = SoftPasskey::new(true);
    assert_eq!(
        register(&app, &setup, &mut new, "new laptop").await,
        StatusCode::CREATED
    );
    let mut another = SoftPasskey::new(true);
    assert_eq!(
        register(&app, &setup, &mut another, "again").await,
        StatusCode::UNAUTHORIZED,
        "a setup key works once"
    );
    let (status, _, _) = login(&app, &mut new).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, account) = send(
        &app,
        "GET",
        "/account",
        Call::session(&old_cookie),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(account["passkeys"].as_array().unwrap().len(), 2);

    let (_, body) = recover(&app, Some("127.0.0.1:5000"), Some(RECOVERY)).await;
    let late = body["setup_key"].as_str().unwrap().to_owned();
    account::expire_ceremonies(&state.account);
    assert_eq!(
        register(&app, &late, &mut SoftPasskey::new(true), "late").await,
        StatusCode::UNAUTHORIZED,
        "a setup key expires"
    );
}

#[tokio::test]
async fn a_dns_suffix_change_voids_the_passkeys_until_recovery() {
    let Platform { app, state } = platform().await;
    operator(&app).await;
    state
        .store
        .store_state("dns_suffix", "int.example.com")
        .await
        .unwrap();
    let call = Call {
        host: Some("console.int.example.com"),
        origin: Some("https://console.int.example.com"),
        ..Default::default()
    };
    let (status, _, body) = send(&app, "POST", "/account/login/start", call, Value::Null).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("self-host account recover")
    );
}

#[tokio::test]
async fn the_first_passkey_login_retires_the_root_key_for_good() {
    let Platform { app, state } = platform().await;
    let (status, _, _) = send(&app, "GET", "/apps", Call::bearer(ROOT), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let mut passkey = SoftPasskey::new(true);
    assert_eq!(
        register(&app, ROOT, &mut passkey, "laptop").await,
        StatusCode::CREATED
    );
    let (status, _, _) = send(&app, "GET", "/apps", Call::bearer(ROOT), Value::Null).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "registering alone does not retire the root key"
    );

    let (status, _, body) = login(&app, &mut passkey).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["retired_root_key"], true);
    let (status, _, _) = send(&app, "GET", "/apps", Call::bearer(ROOT), Value::Null).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, body) = login(&app, &mut passkey).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["retired_root_key"], false);

    account::seed_root_key(&state.store, ROOT).await;
    let (status, _, _) = send(&app, "GET", "/apps", Call::bearer(ROOT), Value::Null).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a restart with SELF_HOST_API_KEY set does not bring the root key back"
    );
    let (status, _, keys) = send(&app, "GET", "/api-keys", Call::bearer(PERSON), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        keys.as_array()
            .unwrap()
            .iter()
            .all(|key| key["primary"] != true),
        "{keys}"
    );
    let (status, _, status_body) =
        send(&app, "GET", "/account/status", Call::console(), Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(status_body["root_key_retired"], true);
    assert_eq!(status_body["usable"], true);
    assert_eq!(status_body["canonical"], true);
}

#[tokio::test]
async fn the_status_tells_the_console_where_passkeys_work() {
    let Platform { app, .. } = platform().await;
    let (status, _, body) = send(
        &app,
        "GET",
        "/account/status",
        Call {
            host: Some("venusaur.example.ts.net"),
            ..Default::default()
        },
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["canonical"], false);
    assert_eq!(body["exists"], false);
    assert_eq!(body["root_key_retired"], false);
    assert_eq!(body["console_url"], "https://console.home.lan/console");
}
