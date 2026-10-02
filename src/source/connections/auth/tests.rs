use super::*;
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, Uri},
    response::Response,
};
use serde_json::{Value, json};
use std::{collections::VecDeque, os::unix::fs::PermissionsExt, process::Command};

struct Workspace(PathBuf);
impl Workspace {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "self-host-provider-auth-{:032x}",
            rand::random::<u128>()
        ));
        super::super::super::private_directory(&root).unwrap();
        Self(root)
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Reply {
    path: String,
    status: u16,
    body: Value,
    delay: bool,
    gate: Option<Arc<Gate>>,
}
#[derive(Default)]
struct Gate {
    arrived: tokio::sync::Notify,
    proceed: tokio::sync::Notify,
}
impl Reply {
    fn ok(path: &str, body: Value) -> Self {
        Self {
            path: path.into(),
            status: 200,
            body,
            delay: false,
            gate: None,
        }
    }
}
struct Seen {
    path: String,
    headers: HeaderMap,
    body: String,
}
struct Script {
    replies: VecDeque<Reply>,
    seen: Vec<Seen>,
}
struct Fixture {
    client: Client,
    script: Arc<Mutex<Script>>,
    task: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new(replies: Vec<Reply>) -> Self {
        async fn handler(
            State(script): State<Arc<Mutex<Script>>>,
            uri: Uri,
            headers: HeaderMap,
            body: Bytes,
        ) -> Response {
            let reply = {
                let mut script = script.lock().unwrap();
                script.seen.push(Seen {
                    path: uri.to_string(),
                    headers,
                    body: String::from_utf8(body.to_vec()).unwrap(),
                });
                let reply = script
                    .replies
                    .pop_front()
                    .expect("unexpected fixture request");
                assert_eq!(uri.path(), reply.path);
                reply
            };
            if reply.delay {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            if let Some(gate) = reply.gate {
                gate.arrived.notify_one();
                gate.proceed.notified().await;
            }
            Response::builder()
                .status(reply.status)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(reply.body.to_string()))
                .unwrap()
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let script = Arc::new(Mutex::new(Script {
            replies: replies.into(),
            seen: vec![],
        }));
        let router = Router::new().fallback(handler).with_state(script.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let mut client = Client::new().unwrap();
        client.endpoints = Endpoints {
            github_api: base.clone(),
            github_token: format!("{base}/github/token"),
            gitlab_api: format!("{base}/api/v4"),
            gitlab_token: format!("{base}/oauth/token"),
            gitlab_info: format!("{base}/oauth/token/info"),
        };
        Self {
            client,
            script,
            task,
        }
    }
    fn assert_consumed(&self) {
        assert!(self.script.lock().unwrap().replies.is_empty());
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn completion(
    state: String,
    code: Option<&str>,
    installation_id: Option<u64>,
) -> AuthorizationCompletion {
    AuthorizationCompletion {
        state,
        code: code.map(String::from),
        installation_id,
        error: None,
    }
}
fn configure_gitlab(root: &Path) {
    save_gitlab(
        root,
        GitlabIntegrationRequest {
            console_url: "http://127.0.0.1:23456/console/".into(),
            client_id: "fixture-client".into(),
            client_secret: "synthetic-client-secret".into(),
        },
    )
    .unwrap();
}
fn oauth_replies(token: &str, refresh: &str, account: u64) -> Vec<Reply> {
    vec![
        Reply::ok(
            "/oauth/token",
            json!({"access_token":token,"refresh_token":refresh,"token_type":"bearer","expires_in":7200}),
        ),
        Reply::ok(
            "/oauth/token/info",
            json!({"resource_owner_id":account,"scope":GITLAB_SCOPES,"expires_in":7199,"application":{"uid":"fixture-client"}}),
        ),
        Reply::ok("/api/v4/user", json!({"id":account})),
    ]
}
async fn connect_gitlab(root: &Path, fixture: &Fixture) -> Connection {
    let started = start(
        root,
        AuthorizationRequest {
            use_existing_installation: false,
            provider: Provider::Gitlab,
            name: "Fixture GitLab".into(),
            connection_id: None,
        },
    )
    .unwrap();
    complete_with(
        root,
        completion(started.state, Some("synthetic-code"), None),
        &fixture.client,
    )
    .await
    .unwrap()
    .connection
    .unwrap()
}
fn expire(root: &Path, connection: &Connection) {
    let path = grant_path(root, &connection.id).unwrap();
    let mut grant: Grant = read_private(&path).unwrap().unwrap();
    match &mut grant {
        Grant::Github { expires_at, .. } | Grant::Gitlab { expires_at, .. } => {
            *expires_at = now() - 1
        }
    }
    write_private(&path, &grant).unwrap();
}
fn github_config(root: &Path) -> (GithubConfig, Vec<u8>) {
    let key = root.join("fixture-key.pem");
    let output = Command::new("openssl")
        .args(["genrsa", "-out"])
        .arg(&key)
        .arg("2048")
        .output()
        .unwrap();
    assert!(output.status.success());
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
    let public = Command::new("openssl")
        .args(["rsa", "-in"])
        .arg(&key)
        .args(["-RSAPublicKey_out", "-outform", "DER"])
        .output()
        .unwrap();
    assert!(public.status.success());
    let config = GithubConfig {
        generation: random(),
        console_url: "http://127.0.0.1:23456/console/".into(),
        app_id: 77,
        app_slug: "fixture-self-host".into(),
        client_id: "fixture-gh-client".into(),
        client_secret: "synthetic-github-secret".into(),
        private_key: std::fs::read_to_string(key).unwrap(),
    };
    (config, public.stdout)
}
fn installation_token(token: &str) -> Value {
    json!({"token":token,"expires_at":time::OffsetDateTime::from_unix_timestamp(now()+3600).unwrap().format(&time::format_description::well_known::Rfc3339).unwrap(),"permissions":{"contents":"read","metadata":"read"}})
}
fn github_replies(account: u64, installations: Value) -> Vec<Reply> {
    vec![
        Reply::ok(
            "/github/token",
            json!({"access_token":"synthetic-github-user-token","token_type":"bearer","scope":"","expires_in":28800}),
        ),
        Reply::ok("/user", json!({"id":account})),
        Reply::ok(
            "/user/installations",
            json!({"total_count":installations.as_array().unwrap().len(),"installations":installations}),
        ),
    ]
}

fn existing_github_request(connection_id: Option<&str>) -> AuthorizationRequest {
    serde_json::from_value(json!({
        "provider": "github",
        "name": "Existing GitHub",
        "connection_id": connection_id,
        "use_existing_installation": true
    }))
    .unwrap()
}

#[tokio::test]
async fn existing_github_installation_authorizes_with_pkce_and_user_proof() {
    let workspace = Workspace::new();
    let (config, public_key) = github_config(&workspace.0);
    write_private(&configuration(&workspace.0, "github"), &config).unwrap();
    let mut replies = github_replies(42, json!([{"id":123,"app_id":77},{"id":999,"app_id":100}]));
    replies.push(Reply::ok(
        "/app/installations/123/access_tokens",
        installation_token("synthetic-existing-installation-token"),
    ));
    let fixture = Fixture::new(replies).await;
    let started = start(&workspace.0, existing_github_request(None)).unwrap();
    let authorize = reqwest::Url::parse(started.url.as_ref().unwrap()).unwrap();
    assert_eq!(authorize.path(), "/login/oauth/authorize");
    let params: BTreeMap<_, _> = authorize.query_pairs().into_owned().collect();
    assert_eq!(params["state"], started.state);
    assert_eq!(params["client_id"], config.client_id);
    assert_eq!(params["redirect_uri"], callback(&config.console_url));
    assert_eq!(params["code_challenge_method"], "S256");
    let connection = complete_with(
        &workspace.0,
        completion(started.state.clone(), Some("synthetic-user-code"), None),
        &fixture.client,
    )
    .await
    .unwrap()
    .connection
    .unwrap();
    assert_eq!(connection.authentication, Authentication::GithubApp);
    fixture.assert_consumed();
    {
        let script = fixture.script.lock().unwrap();
        let oauth: BTreeMap<_, _> =
            reqwest::Url::parse(&format!("https://fixture.invalid/?{}", script.seen[0].body))
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect();
        assert_eq!(
            params["code_challenge"],
            URL_SAFE_NO_PAD.encode(Sha256::digest(oauth["code_verifier"].as_bytes()))
        );
        verify_jwt(
            script.seen[3].headers["authorization"]
                .to_str()
                .unwrap()
                .strip_prefix("Bearer ")
                .unwrap(),
            &public_key,
        );
    }
    assert!(
        complete_with(
            &workspace.0,
            completion(started.state, Some("synthetic-user-code"), None),
            &fixture.client,
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn existing_github_installation_rejects_missing_unrelated_and_ambiguous_access() {
    for installations in [
        json!([]),
        json!([{"id":123,"app_id":900}]),
        json!([{"id":123,"app_id":77},{"id":456,"app_id":77}]),
    ] {
        let workspace = Workspace::new();
        let (config, _) = github_config(&workspace.0);
        write_private(&configuration(&workspace.0, "github"), &config).unwrap();
        let fixture = Fixture::new(github_replies(42, installations)).await;
        let started = start(&workspace.0, existing_github_request(None)).unwrap();
        let error = complete_with(
            &workspace.0,
            // An unsolicited callback ID must not choose an installation.
            completion(started.state, Some("synthetic-user-code"), Some(123)),
            &fixture.client,
        )
        .await
        .err()
        .unwrap();
        assert!(error.to_string().contains("installation is not authorized"));
        fixture.assert_consumed();
        assert!(
            super::super::list_connections(&workspace.0)
                .unwrap()
                .is_empty()
        );
        assert!(!workspace.0.join("provider-grants").exists());
    }
}

#[tokio::test]
async fn existing_github_reconnect_keeps_the_authorized_account_and_installation() {
    let workspace = Workspace::new();
    let (config, _) = github_config(&workspace.0);
    write_private(&configuration(&workspace.0, "github"), &config).unwrap();
    let mut replies = github_replies(42, json!([{"id":123,"app_id":77}]));
    replies.push(Reply::ok(
        "/app/installations/123/access_tokens",
        installation_token("synthetic-first-installation-token"),
    ));
    replies.extend(github_replies(
        42,
        json!([{"id":123,"app_id":77},{"id":456,"app_id":77}]),
    ));
    replies.push(Reply::ok(
        "/app/installations/123/access_tokens",
        installation_token("synthetic-reconnected-installation-token"),
    ));
    replies.extend(github_replies(43, json!([{"id":123,"app_id":77}])));
    replies.push(Reply::ok(
        "/app/installations/123/access_tokens",
        installation_token("synthetic-wrong-account-token"),
    ));
    replies.extend(github_replies(42, json!([{"id":456,"app_id":77}])));
    let fixture = Fixture::new(replies).await;
    let started = start(&workspace.0, existing_github_request(None)).unwrap();
    let original = complete_with(
        &workspace.0,
        completion(started.state, Some("synthetic-user-code"), None),
        &fixture.client,
    )
    .await
    .unwrap()
    .connection
    .unwrap();
    let started = start(&workspace.0, existing_github_request(Some(&original.id))).unwrap();
    let reconnected = complete_with(
        &workspace.0,
        completion(started.state, Some("synthetic-reconnect-code"), None),
        &fixture.client,
    )
    .await
    .unwrap()
    .connection
    .unwrap();
    assert_eq!(reconnected.id, original.id);
    assert_eq!(reconnected.credential_id, original.credential_id);
    assert_ne!(
        reconnected.authorization_generation,
        original.authorization_generation
    );
    let path = grant_path(&workspace.0, &original.id).unwrap();
    let stored = std::fs::read(&path).unwrap();
    for _ in 0..2 {
        let started = start(&workspace.0, existing_github_request(Some(&original.id))).unwrap();
        assert!(
            complete_with(
                &workspace.0,
                completion(started.state, Some("synthetic-reconnect-code"), None),
                &fixture.client,
            )
            .await
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), stored);
        assert_eq!(
            super::super::read_connection(&workspace.0, &original.id)
                .unwrap()
                .authorization_generation,
            reconnected.authorization_generation
        );
    }
    fixture.assert_consumed();
}

#[test]
fn existing_installation_option_is_only_supported_by_github() {
    let workspace = Workspace::new();
    configure_gitlab(&workspace.0);
    let request: AuthorizationRequest = serde_json::from_value(json!({
        "provider": "gitlab",
        "name": "Fixture GitLab",
        "use_existing_installation": true
    }))
    .unwrap();
    assert!(start(&workspace.0, request).is_err());
}
fn verify_jwt(signed: &str, public_key: &[u8]) {
    let parts: Vec<_> = signed.split('.').collect();
    assert_eq!(parts.len(), 3);
    ring::signature::UnparsedPublicKey::new(
        &ring::signature::RSA_PKCS1_2048_8192_SHA256,
        public_key,
    )
    .verify(
        format!("{}.{}", parts[0], parts[1]).as_bytes(),
        &URL_SAFE_NO_PAD.decode(parts[2]).unwrap(),
    )
    .unwrap();
    let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
    assert_eq!(header["alg"], "RS256");
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
    assert_eq!(claims["iss"], "fixture-gh-client");
    assert!(claims["exp"].as_i64().unwrap() <= now() + 600);
    assert!(claims["exp"].as_i64().unwrap() > now());
    assert!(claims["iat"].as_i64().unwrap() <= now());
}

#[test]
fn public_settings_and_states_do_not_expose_secret_configuration() {
    let workspace = Workspace::new();
    configure_gitlab(&workspace.0);
    let metadata = serde_json::to_string(&integrations(&workspace.0).unwrap()).unwrap();
    assert!(!metadata.contains("synthetic-client-secret"));
    let path = configuration(&workspace.0, "gitlab");
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    for url in [
        "http://outside.invalid/console/",
        "https://user:secret@outside.invalid/console/",
        "https://outside.invalid/arbitrary/",
        "https://outside.invalid/console/?redirect=token",
        "https://outside.invalid/console/#token",
    ] {
        assert!(console_url(url).is_err());
    }
    let started = start(
        &workspace.0,
        AuthorizationRequest {
            use_existing_installation: false,
            provider: Provider::Gitlab,
            name: "Fixture".into(),
            connection_id: None,
        },
    )
    .unwrap();
    assert!(started.state.len() >= 43);
    assert_eq!(
        callback_origin(&workspace.0, &started.state).unwrap(),
        "http://127.0.0.1:23456"
    );
    let other = Workspace::new();
    assert!(callback_origin(&other.0, &started.state).is_err());
    cancel(&workspace.0, &started.state).unwrap();
    assert!(callback_origin(&workspace.0, &started.state).is_err());
    let started = start(
        &workspace.0,
        AuthorizationRequest {
            use_existing_installation: false,
            provider: Provider::Gitlab,
            name: "Fixture".into(),
            connection_id: None,
        },
    )
    .unwrap();
    let root = workspace.0.canonicalize().unwrap();
    SESSIONS
        .lock()
        .unwrap()
        .get_mut(&(root, started.state.clone()))
        .unwrap()
        .deadline = Instant::now() - Duration::from_secs(1);
    assert!(callback_origin(&workspace.0, &started.state).is_err());
}

#[tokio::test]
async fn gitlab_pkce_completion_and_single_flight_rotate_both_tokens() {
    let workspace = Workspace::new();
    configure_gitlab(&workspace.0);
    let mut replies = oauth_replies("synthetic-access-one", "synthetic-refresh-one", 91);
    let mut renewal = oauth_replies("synthetic-access-two", "synthetic-refresh-two", 91);
    renewal[0].delay = true;
    replies.extend(renewal);
    let fixture = Fixture::new(replies).await;
    let started = start(
        &workspace.0,
        AuthorizationRequest {
            use_existing_installation: false,
            provider: Provider::Gitlab,
            name: "Fixture".into(),
            connection_id: None,
        },
    )
    .unwrap();
    let url = reqwest::Url::parse(started.url.as_ref().unwrap()).unwrap();
    let query: BTreeMap<_, _> = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    assert_eq!(query["code_challenge_method"], "S256");
    let result = complete_with(
        &workspace.0,
        completion(started.state.clone(), Some("synthetic-code"), None),
        &fixture.client,
    )
    .await
    .unwrap();
    let connection = result.connection.unwrap();
    assert_eq!(connection.authentication, Authentication::Oauth);
    let public = serde_json::to_string(&connection).unwrap();
    assert!(!public.contains("synthetic-access") && !public.contains("synthetic-refresh"));
    assert!(
        complete_with(
            &workspace.0,
            completion(started.state, Some("synthetic-code"), None),
            &fixture.client
        )
        .await
        .is_err()
    );
    let request = fixture.script.lock().unwrap().seen[0].body.clone();
    let form: BTreeMap<_, _> = reqwest::Url::parse(&format!("https://fixture.invalid/?{request}"))
        .unwrap()
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    assert_eq!(
        query["code_challenge"],
        URL_SAFE_NO_PAD.encode(Sha256::digest(form["code_verifier"].as_bytes()))
    );
    assert_eq!(query["redirect_uri"], form["redirect_uri"]);
    expire(&workspace.0, &connection);
    let (first, second) = tokio::join!(
        resolve_with(&workspace.0, &connection.credential_id, &fixture.client),
        resolve_with(&workspace.0, &connection.credential_id, &fixture.client)
    );
    assert_eq!(first.unwrap().value, "synthetic-access-two");
    assert_eq!(second.unwrap().username.as_deref(), Some("oauth2"));
    fixture.assert_consumed();
    assert_eq!(fixture.script.lock().unwrap().seen.len(), 6);
    let grant: Grant = read_private(&grant_path(&workspace.0, &connection.id).unwrap())
        .unwrap()
        .unwrap();
    assert!(
        matches!(grant, Grant::Gitlab { refresh_token, .. } if refresh_token == "synthetic-refresh-two")
    );
    assert!(
        save_gitlab(
            &workspace.0,
            GitlabIntegrationRequest {
                console_url: "http://127.0.0.1:23456/console/".into(),
                client_id: "other".into(),
                client_secret: "secret".into()
            }
        )
        .is_err()
    );
    assert_eq!(
        super::super::read_connection(&workspace.0, &connection.id)
            .unwrap()
            .credential_id,
        connection.credential_id
    );
}

#[test]
fn github_registration_keeps_local_callbacks_without_configuring_a_webhook() {
    let workspace = Workspace::new();
    let callback = "http://localhost:23722/source/authorization/callback";
    for organization in [None, Some("fixture-team".into())] {
        let registration = register_github(
            &workspace.0,
            GithubRegistrationRequest {
                console_url: "http://localhost:23722/console/".into(),
                organization,
            },
        )
        .unwrap();
        let manifest: Value = serde_json::from_str(&registration.form.unwrap().manifest).unwrap();
        assert!(
            manifest.get("hook_attributes").is_none(),
            "GitHub validates webhook URLs even when delivery is inactive"
        );
        assert_eq!(manifest["default_events"], json!([]));
        assert_eq!(manifest["redirect_url"], callback);
        assert_eq!(manifest["callback_urls"], json!([callback]));
        assert_eq!(manifest["setup_url"], callback);
        cancel(&workspace.0, &registration.state).unwrap();
    }
}

#[tokio::test]
async fn github_manifest_installation_then_pkce_proves_ownership_and_signs_jwt() {
    let workspace = Workspace::new();
    let (config, public_key) = github_config(&workspace.0);
    let mut replies = vec![Reply::ok(
        "/app-manifests/synthetic-manifest-code/conversions",
        json!({"id":config.app_id,"slug":config.app_slug,"client_id":config.client_id,"client_secret":config.client_secret,"pem":config.private_key,"permissions":{"contents":"read","metadata":"read"}}),
    )];
    replies.extend(github_replies(
        42,
        json!([{"id":123,"app_id":77},{"id":456,"app_id":77},{"id":999,"app_id":100}]),
    ));
    replies.push(Reply::ok(
        "/app/installations/123/access_tokens",
        installation_token("synthetic-installation-token"),
    ));
    let fixture = Fixture::new(replies).await;
    let registration = register_github(
        &workspace.0,
        GithubRegistrationRequest {
            console_url: config.console_url.clone(),
            organization: Some("fixture-team".into()),
        },
    )
    .unwrap();
    let form = registration.form.unwrap();
    assert!(
        form.action
            .starts_with("https://github.com/organizations/fixture-team/settings/apps/new?state=")
    );
    let manifest: Value = serde_json::from_str(&form.manifest).unwrap();
    assert_eq!(manifest["request_oauth_on_install"], false);
    assert!(manifest.get("hook_attributes").is_none());
    assert!(manifest.get("setup_url").is_some());
    let result = complete_with(
        &workspace.0,
        completion(registration.state, Some("synthetic-manifest-code"), None),
        &fixture.client,
    )
    .await
    .unwrap();
    assert!(result.integrations.unwrap().github.configured);
    let start = start(
        &workspace.0,
        AuthorizationRequest {
            use_existing_installation: false,
            provider: Provider::Github,
            name: "Fixture GitHub".into(),
            connection_id: None,
        },
    )
    .unwrap();
    assert!(
        start
            .url
            .unwrap()
            .starts_with("https://github.com/apps/fixture-self-host/installations/new?state=")
    );
    let continuation = complete_with(
        &workspace.0,
        completion(start.state.clone(), None, Some(123)),
        &fixture.client,
    )
    .await
    .unwrap()
    .authorization
    .unwrap();
    assert!(
        complete_with(
            &workspace.0,
            completion(start.state, None, Some(123)),
            &fixture.client
        )
        .await
        .is_err()
    );
    let authorize = reqwest::Url::parse(continuation.url.as_ref().unwrap()).unwrap();
    let params: BTreeMap<_, _> = authorize
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    assert_eq!(params["code_challenge_method"], "S256");
    let connection = complete_with(
        &workspace.0,
        completion(continuation.state, Some("synthetic-user-code"), None),
        &fixture.client,
    )
    .await
    .unwrap()
    .connection
    .unwrap();
    assert_eq!(connection.authentication, Authentication::GithubApp);
    fixture.assert_consumed();
    {
        let script = fixture.script.lock().unwrap();
        let oauth: BTreeMap<_, _> =
            reqwest::Url::parse(&format!("https://fixture.invalid/?{}", script.seen[1].body))
                .unwrap()
                .query_pairs()
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect();
        assert_eq!(
            params["code_challenge"],
            URL_SAFE_NO_PAD.encode(Sha256::digest(oauth["code_verifier"].as_bytes()))
        );
        let signed = script.seen.last().unwrap().headers["authorization"]
            .to_str()
            .unwrap()
            .strip_prefix("Bearer ")
            .unwrap();
        verify_jwt(signed, &public_key);
        assert_eq!(
            script.seen[3].path,
            "/user/installations?per_page=100&page=1"
        );
    }
    let credential = resolve_with(&workspace.0, &connection.credential_id, &fixture.client)
        .await
        .unwrap();
    assert_eq!(credential.value, "synthetic-installation-token");
    assert_eq!(credential.username.as_deref(), Some("x-access-token"));
    assert!(
        super::super::validate_bound_repository(
            credential.server.as_deref(),
            "https://outside.invalid/app.git"
        )
        .is_err()
    );
    let serialized = serde_json::to_string(&integrations(&workspace.0).unwrap()).unwrap();
    assert!(!serialized.contains("synthetic-github-secret") && !serialized.contains("PRIVATE KEY"));

    expire(&workspace.0, &connection);
    let gate = Arc::new(Gate::default());
    let mut renewal = Reply::ok(
        "/app/installations/123/access_tokens",
        installation_token("synthetic-renewed-installation-token"),
    );
    renewal.gate = Some(gate.clone());
    fixture.script.lock().unwrap().replies.push_back(renewal);
    let (first, second, ()) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            resolve_with(&workspace.0, &connection.credential_id, &fixture.client),
            resolve_with(&workspace.0, &connection.credential_id, &fixture.client),
            async {
                gate.arrived.notified().await;
                gate.proceed.notify_one();
            }
        )
    })
    .await
    .unwrap();
    assert_eq!(first.unwrap().value, "synthetic-renewed-installation-token");
    assert_eq!(
        second.unwrap().value,
        "synthetic-renewed-installation-token"
    );
    fixture.assert_consumed();
    let script = fixture.script.lock().unwrap();
    assert_eq!(script.seen.len(), 6, "two callers must mint exactly once");
    let renewed = script.seen.last().unwrap();
    verify_jwt(
        renewed.headers["authorization"]
            .to_str()
            .unwrap()
            .strip_prefix("Bearer ")
            .unwrap(),
        &public_key,
    );
    assert_eq!(
        serde_json::from_str::<Value>(&renewed.body).unwrap()["permissions"],
        json!({"contents":"read","metadata":"read"})
    );
    let grant: Grant = read_private(&grant_path(&workspace.0, &connection.id).unwrap())
        .unwrap()
        .unwrap();
    assert!(
        matches!(grant, Grant::Github { access_token, expires_at, .. } if access_token == "synthetic-renewed-installation-token" && expires_at > now() + RENEW_BEFORE)
    );
}

#[tokio::test]
async fn forged_installation_scopes_and_denial_never_save_access() {
    let workspace = Workspace::new();
    let (config, _) = github_config(&workspace.0);
    write_private(&configuration(&workspace.0, "github"), &config).unwrap();
    let fixture = Fixture::new(github_replies(42, json!([{"id":123,"app_id":900}]))).await;
    let first = start(
        &workspace.0,
        AuthorizationRequest {
            use_existing_installation: false,
            provider: Provider::Github,
            name: "Fixture".into(),
            connection_id: None,
        },
    )
    .unwrap();
    let second = complete_with(
        &workspace.0,
        completion(first.state, None, Some(123)),
        &fixture.client,
    )
    .await
    .unwrap()
    .authorization
    .unwrap();
    assert!(
        complete_with(
            &workspace.0,
            completion(second.state, Some("synthetic-code"), None),
            &fixture.client
        )
        .await
        .is_err()
    );
    assert!(
        super::super::list_connections(&workspace.0)
            .unwrap()
            .is_empty()
    );
    fixture.assert_consumed();
    configure_gitlab(&workspace.0);
    let denied = start(
        &workspace.0,
        AuthorizationRequest {
            use_existing_installation: false,
            provider: Provider::Gitlab,
            name: "Fixture".into(),
            connection_id: None,
        },
    )
    .unwrap();
    let state = denied.state;
    let mut request = completion(state.clone(), None, None);
    request.error = Some("raw-secret-provider-error".into());
    let error = complete_with(&workspace.0, request, &fixture.client)
        .await
        .err()
        .unwrap();
    assert!(!error.to_string().contains("raw-secret-provider-error"));
    assert!(callback_origin(&workspace.0, &state).is_err());
    let mut replies = oauth_replies("synthetic-access", "synthetic-refresh", 91);
    replies[1].body["scope"] = json!(["api"]);
    let fixture = Fixture::new(replies.into_iter().take(2).collect()).await;
    let start = start(
        &workspace.0,
        AuthorizationRequest {
            use_existing_installation: false,
            provider: Provider::Gitlab,
            name: "Fixture".into(),
            connection_id: None,
        },
    )
    .unwrap();
    assert!(
        complete_with(
            &workspace.0,
            completion(start.state, Some("synthetic-code"), None),
            &fixture.client
        )
        .await
        .is_err()
    );
    assert!(
        super::super::list_connections(&workspace.0)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn renewal_failures_distinguish_revoked_access_from_transient_provider_errors() {
    for (status, body, expired) in [
        (429, json!({"error":"provider-secret-body"}), false),
        (
            400,
            json!({"error":"invalid_grant","error_description":"provider-secret-body"}),
            true,
        ),
    ] {
        let workspace = Workspace::new();
        configure_gitlab(&workspace.0);
        let mut replies = oauth_replies("synthetic-access", "synthetic-refresh", 91);
        replies.push(Reply {
            path: "/oauth/token".into(),
            status,
            body,
            delay: false,
            gate: None,
        });
        let fixture = Fixture::new(replies).await;
        let connection = connect_gitlab(&workspace.0, &fixture).await;
        expire(&workspace.0, &connection);
        let error = resolve_with(&workspace.0, &connection.credential_id, &fixture.client)
            .await
            .err()
            .unwrap();
        assert!(!error.to_string().contains("provider-secret-body"));
        assert_eq!(
            super::super::read_connection(&workspace.0, &connection.id)
                .unwrap()
                .status
                == ConnectionStatus::Expired,
            expired
        );
    }
}

#[tokio::test]
async fn reconnect_disconnect_and_config_changes_reject_obsolete_authorizations() {
    let workspace = Workspace::new();
    configure_gitlab(&workspace.0);
    let mut replies = oauth_replies("synthetic-first", "synthetic-refresh-one", 91);
    replies.extend(oauth_replies(
        "synthetic-second",
        "synthetic-refresh-two",
        91,
    ));
    let fixture = Fixture::new(replies).await;
    let connection = connect_gitlab(&workspace.0, &fixture).await;
    let reconnect = start(
        &workspace.0,
        AuthorizationRequest {
            use_existing_installation: false,
            provider: Provider::Gitlab,
            name: "Reconnected".into(),
            connection_id: Some(connection.id.clone()),
        },
    )
    .unwrap();
    let reconnected = complete_with(
        &workspace.0,
        completion(reconnect.state, Some("synthetic-code"), None),
        &fixture.client,
    )
    .await
    .unwrap()
    .connection
    .unwrap();
    assert_eq!(reconnected.id, connection.id);
    assert_eq!(reconnected.credential_id, connection.credential_id);
    super::super::delete_connection(&workspace.0, &connection.id).unwrap();
    assert!(
        resolve_with(&workspace.0, &connection.credential_id, &fixture.client)
            .await
            .is_err()
    );
    assert!(!grant_path(&workspace.0, &connection.id).unwrap().exists());
    let pending = start(
        &workspace.0,
        AuthorizationRequest {
            use_existing_installation: false,
            provider: Provider::Gitlab,
            name: "Obsolete".into(),
            connection_id: None,
        },
    )
    .unwrap();
    configure_gitlab(&workspace.0);
    assert!(
        complete_with(
            &workspace.0,
            completion(pending.state, Some("synthetic-code"), None),
            &fixture.client
        )
        .await
        .is_err()
    );
    fixture.assert_consumed();
}

#[tokio::test]
async fn late_refresh_cannot_restore_disconnected_or_replace_reconnected_access() {
    for disconnect in [false, true] {
        let workspace = Workspace::new();
        configure_gitlab(&workspace.0);
        let fixture = Fixture::new(oauth_replies(
            "synthetic-original",
            "synthetic-original-refresh",
            91,
        ))
        .await;
        let connection = connect_gitlab(&workspace.0, &fixture).await;
        expire(&workspace.0, &connection);
        let gate = Arc::new(Gate::default());
        let mut renewal = oauth_replies(
            "synthetic-obsolete-renewal",
            "synthetic-obsolete-refresh",
            91,
        );
        renewal[0].gate = Some(gate.clone());
        let initial = renewal.remove(0);
        fixture.script.lock().unwrap().replies.push_back(initial);
        let obsolete = start(
            &workspace.0,
            AuthorizationRequest {
                use_existing_installation: false,
                provider: Provider::Gitlab,
                name: "Obsolete popup".into(),
                connection_id: Some(connection.id.clone()),
            },
        )
        .unwrap();
        let replacement = if disconnect {
            None
        } else {
            fixture.script.lock().unwrap().replies.extend(oauth_replies(
                "synthetic-reconnected",
                "synthetic-reconnected-refresh",
                91,
            ));
            Some(
                start(
                    &workspace.0,
                    AuthorizationRequest {
                        use_existing_installation: false,
                        provider: Provider::Gitlab,
                        name: "Current connection".into(),
                        connection_id: Some(connection.id.clone()),
                    },
                )
                .unwrap(),
            )
        };
        fixture.script.lock().unwrap().replies.extend(renewal);
        let (late, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(
                resolve_with(&workspace.0, &connection.credential_id, &fixture.client),
                async {
                    gate.arrived.notified().await;
                    if let Some(replacement) = replacement {
                        let current = complete_with(
                            &workspace.0,
                            completion(replacement.state, Some("synthetic-new-code"), None),
                            &fixture.client,
                        )
                        .await
                        .unwrap()
                        .connection
                        .unwrap();
                        assert_eq!(current.id, connection.id);
                        assert_eq!(current.credential_id, connection.credential_id);
                    } else {
                        super::super::delete_connection(&workspace.0, &connection.id).unwrap();
                    }
                    gate.proceed.notify_one();
                }
            )
        })
        .await
        .unwrap();
        assert!(late.is_err(), "obsolete renewal must not return its token");
        fixture.assert_consumed();
        let before = fixture.script.lock().unwrap().seen.len();
        assert!(
            complete_with(
                &workspace.0,
                completion(obsolete.state.clone(), Some("synthetic-old-code"), None),
                &fixture.client
            )
            .await
            .is_err()
        );
        assert_eq!(
            fixture.script.lock().unwrap().seen.len(),
            before,
            "obsolete popup must fail before exchanging a code"
        );
        assert!(
            callback_origin(&workspace.0, &obsolete.state).is_err(),
            "obsolete callback state must still be consumed"
        );
        if disconnect {
            assert!(!grant_path(&workspace.0, &connection.id).unwrap().exists());
            assert!(
                !workspace
                    .0
                    .join("credentials")
                    .join(&connection.credential_id)
                    .exists()
            );
            assert!(
                super::super::list_connections(&workspace.0)
                    .unwrap()
                    .is_empty()
            );
        } else {
            let grant: Grant = read_private(&grant_path(&workspace.0, &connection.id).unwrap())
                .unwrap()
                .unwrap();
            assert!(
                matches!(grant, Grant::Gitlab { access_token, refresh_token, .. } if access_token == "synthetic-reconnected" && refresh_token == "synthetic-reconnected-refresh")
            );
            let current = resolve_with(&workspace.0, &connection.credential_id, &fixture.client)
                .await
                .unwrap();
            assert_eq!(current.value, "synthetic-reconnected");
        }
    }
}
