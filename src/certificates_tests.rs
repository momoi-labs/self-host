use super::*;
use std::convert::Infallible;
use std::sync::{
    Mutex as StdMutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use axum::body::Body;
use axum::http::{Method, Request, Response};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use http_body_util::BodyExt;
use hyper_util::rt::TokioIo;
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, CertificateSigningRequestParams, IsCa,
    KeyPair,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;

struct Fixture {
    dir: PathBuf,
    manager: CertificateManager,
    service: Arc<AcmeService>,
    https: std::net::SocketAddr,
    http: std::net::SocketAddr,
    ca: Vec<u8>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct AcmeOrder {
    hostname: String,
    token: String,
    ready: bool,
    chain: Option<String>,
}
struct AcmeState {
    nonce: u64,
    nonces: BTreeSet<String>,
    jwk: Option<Value>,
    registrations: usize,
    signed_requests: usize,
    challenges: usize,
    orders: Vec<AcmeOrder>,
}
struct AcmeService {
    base: String,
    callback: std::net::SocketAddr,
    ca: Certificate,
    key: KeyPair,
    state: StdMutex<AcmeState>,
    fail: AtomicBool,
    failed_hostname: StdMutex<Option<String>>,
}

impl AcmeService {
    fn response(
        &self,
        status: u16,
        body: impl Into<Body>,
        location: Option<String>,
    ) -> Response<Body> {
        let mut state = self.state.lock().unwrap();
        state.nonce += 1;
        let nonce = format!("nonce-{}", state.nonce);
        state.nonces.insert(nonce.clone());
        let mut response = Response::builder()
            .status(status)
            .header("replay-nonce", nonce)
            .header("content-type", "application/json");
        if let Some(location) = location {
            response = response.header("location", location);
        }
        response.body(body.into()).unwrap()
    }
    fn json(&self, status: u16, value: Value, location: Option<String>) -> Response<Body> {
        self.response(status, value.to_string(), location)
    }
    fn order(&self, id: usize) -> Value {
        let state = self.state.lock().unwrap();
        let order = &state.orders[id];
        let mut value = json!({"status": if order.chain.is_some() { "valid" } else if order.ready { "ready" } else { "pending" }, "identifiers":[{"type":"dns","value":order.hostname}], "authorizations":[format!("{}/authz/{id}",self.base)], "finalize":format!("{}/finalize/{id}",self.base)});
        if order.chain.is_some() {
            value["certificate"] = json!(format!("{}/certificate/{id}", self.base));
        }
        value
    }
    fn challenge(&self, id: usize, valid: bool) -> Value {
        let state = self.state.lock().unwrap();
        json!({"type":"http-01","url":format!("{}/challenge/{id}",self.base),"status": if valid { "valid" } else { "pending" },"token":state.orders[id].token})
    }
    fn verified_payload(&self, url: &str, body: &[u8]) -> Value {
        let envelope: Value = serde_json::from_slice(body).unwrap();
        let protected = envelope["protected"].as_str().unwrap();
        let payload = envelope["payload"].as_str().unwrap();
        let header: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(protected).unwrap()).unwrap();
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["url"], url);
        let mut state = self.state.lock().unwrap();
        assert!(
            state.nonces.remove(header["nonce"].as_str().unwrap()),
            "nonce replay or missing nonce"
        );
        let jwk = match header.get("jwk") {
            Some(jwk) => {
                assert!(header.get("kid").is_none());
                jwk.clone()
            }
            None => {
                assert_eq!(header["kid"], format!("{}/account/1", self.base));
                state.jwk.clone().unwrap()
            }
        };
        assert_eq!(jwk["kty"], "EC");
        assert_eq!(jwk["crv"], "P-256");
        let mut public = vec![4];
        public.extend(URL_SAFE_NO_PAD.decode(jwk["x"].as_str().unwrap()).unwrap());
        public.extend(URL_SAFE_NO_PAD.decode(jwk["y"].as_str().unwrap()).unwrap());
        ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, public)
            .verify(
                format!("{protected}.{payload}").as_bytes(),
                &URL_SAFE_NO_PAD
                    .decode(envelope["signature"].as_str().unwrap())
                    .unwrap(),
            )
            .expect("valid signed ACME request");
        state.signed_requests += 1;
        if url.ends_with("/new-account") {
            state.jwk = Some(jwk);
        }
        if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap()
        }
    }

    async fn handle(self: Arc<Self>, req: Request<hyper::body::Incoming>) -> Response<Body> {
        let path = req.uri().path().to_string();
        if path == "/directory" {
            assert_eq!(req.method(), Method::GET);
            return self.json(200, json!({"newNonce": format!("{}/nonce", self.base), "newAccount":format!("{}/new-account",self.base), "newOrder":format!("{}/new-order", self.base)}), None);
        }
        if path == "/nonce" {
            assert_eq!(req.method(), Method::HEAD);
            return self.response(200, "", None);
        }
        assert_eq!(req.method(), Method::POST);
        let payload = self.verified_payload(
            &format!("{}{path}", self.base),
            &req.into_body().collect().await.unwrap().to_bytes(),
        );
        if path == "/new-account" {
            assert_eq!(payload["termsOfServiceAgreed"], true);
            self.state.lock().unwrap().registrations += 1;
            return self.json(
                201,
                json!({"status":"valid","orders":format!("{}/orders",self.base)}),
                Some(format!("{}/account/1", self.base)),
            );
        }
        if path == "/new-order" {
            if self.fail.load(Ordering::SeqCst)
                || self
                    .failed_hostname
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|hostname| payload["identifiers"][0]["value"] == *hostname)
            {
                return self.json(503, json!({"type":"urn:ietf:params:acme:error:serverInternal","detail":"fixture issuance unavailable"}), None);
            }
            assert_eq!(payload["identifiers"].as_array().unwrap().len(), 1);
            assert_eq!(payload["identifiers"][0]["type"], "dns");
            let id = {
                let mut state = self.state.lock().unwrap();
                let id = state.orders.len();
                state.orders.push(AcmeOrder {
                    hostname: payload["identifiers"][0]["value"].as_str().unwrap().into(),
                    token: format!("synthetic-token-{id}"),
                    ready: false,
                    chain: None,
                });
                id
            };
            return self.json(
                201,
                self.order(id),
                Some(format!("{}/order/{id}", self.base)),
            );
        }
        let (kind, id) = path.trim_start_matches('/').split_once('/').unwrap();
        let id: usize = id.parse().unwrap();
        match kind {
            "authz" => {
                assert!(payload.is_null());
                let (name, ready) = {
                    let state = self.state.lock().unwrap();
                    (state.orders[id].hostname.clone(), state.orders[id].ready)
                };
                self.json(200, json!({"identifier":{"type":"dns","value":name},"status":if ready { "valid" } else { "pending" },"challenges":[self.challenge(id, ready)]}), None)
            }
            "challenge" => {
                assert_eq!(payload, json!({}));
                let (name, token, jwk) = {
                    let state = self.state.lock().unwrap();
                    (
                        state.orders[id].hostname.clone(),
                        state.orders[id].token.clone(),
                        state.jwk.clone().unwrap(),
                    )
                };
                // An independent HTTP client fetches the actual proxy's reserved path.
                let response = reqwest::Client::new()
                    .get(format!(
                        "http://{}/.well-known/acme-challenge/{token}",
                        self.callback
                    ))
                    .header("Host", &name)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                for (host, path) in [
                    (
                        "wrong.example.invalid",
                        format!("/.well-known/acme-challenge/{token}"),
                    ),
                    (
                        name.as_str(),
                        format!("/.well-known/acme-challenge/%73{}", &token[1..]),
                    ),
                ] {
                    let missing = reqwest::Client::new()
                        .get(format!("http://{}{path}", self.callback))
                        .header("Host", host)
                        .send()
                        .await
                        .unwrap();
                    assert_eq!(
                        missing.status(),
                        StatusCode::NOT_FOUND,
                        "active challenges require the exact Hostname and unencoded token"
                    );
                }
                let canonical =
                    json!({"crv":jwk["crv"],"kty":jwk["kty"],"x":jwk["x"],"y":jwk["y"]})
                        .to_string();
                let thumbprint = URL_SAFE_NO_PAD.encode(Sha256::digest(canonical));
                assert_eq!(
                    response.text().await.unwrap(),
                    format!("{token}.{thumbprint}")
                );
                {
                    let mut state = self.state.lock().unwrap();
                    state.orders[id].ready = true;
                    state.challenges += 1;
                }
                self.json(200, self.challenge(id, true), None)
            }
            "order" => {
                assert!(payload.is_null());
                self.json(200, self.order(id), None)
            }
            "finalize" => {
                let csr = URL_SAFE_NO_PAD
                    .decode(payload["csr"].as_str().unwrap())
                    .unwrap();
                let mut csr = CertificateSigningRequestParams::from_der(&csr.into()).unwrap(); // verifies CSR signature
                let hostname = {
                    let state = self.state.lock().unwrap();
                    assert!(state.orders[id].ready);
                    state.orders[id].hostname.clone()
                };
                assert_eq!(
                    csr.params.subject_alt_names,
                    vec![rcgen::SanType::DnsName(hostname.try_into().unwrap())]
                );
                csr.params.not_before =
                    time::OffsetDateTime::now_utc() - time::Duration::seconds(60);
                csr.params.not_after = time::OffsetDateTime::now_utc() + time::Duration::hours(3);
                let certificate = csr.signed_by(&self.ca, &self.key).unwrap();
                self.state.lock().unwrap().orders[id].chain =
                    Some(format!("{}{}", certificate.pem(), self.ca.pem()));
                self.json(200, self.order(id), None)
            }
            "certificate" => {
                assert!(payload.is_null());
                let chain = self.state.lock().unwrap().orders[id].chain.clone().unwrap();
                self.response(200, chain, None)
            }
            _ => panic!("unexpected ACME resource: {path}"),
        }
    }
}

static FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

impl Fixture {
    async fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "self-host-certificates-{}-{}",
            std::process::id(),
            FIXTURE_ID.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut ca_params = CertificateParams::new(vec!["localhost".into()]).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![
            rcgen::KeyUsagePurpose::KeyCertSign,
            rcgen::KeyUsagePurpose::CrlSign,
        ];
        let ca_key = KeyPair::generate().unwrap();
        let ca = ca_params.self_signed(&ca_key).unwrap();
        std::fs::write(dir.join("test-ca.pem"), ca.pem()).unwrap();
        let tls_key = KeyPair::generate().unwrap();
        let tls_cert = CertificateParams::new(vec!["localhost".into()])
            .unwrap()
            .signed_by(&tls_key, &ca, &ca_key)
            .unwrap();
        let server_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![tls_cert.der().clone()],
                rustls::pki_types::PrivatePkcs8KeyDer::from(tls_key.serialize_der()).into(),
            )
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(server_config));
        let acme_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let acme_port = acme_listener.local_addr().unwrap().port();
        let local_key = KeyPair::generate().unwrap();
        let local_cert = CertificateParams::new(vec!["home.lan".into(), "*.home.lan".into()])
            .unwrap()
            .self_signed(&local_key)
            .unwrap();
        std::fs::write(dir.join("cert.pem"), local_cert.pem()).unwrap();
        std::fs::write(dir.join("key.pem"), local_key.serialize_pem()).unwrap();
        std::fs::write(dir.join("ca.pem"), local_cert.pem()).unwrap();
        let state_dir = dir.join("state/certificates");
        secure_dir(&state_dir).unwrap();
        atomic_json(
            &state_dir.join("public.json"),
            &CertificateConfig {
                directory_url: format!("https://localhost:{acme_port}/directory"),
                contact: vec!["mailto:operator@example.invalid".into()],
                hostnames: vec!["blog.example.invalid".into(), "alias.other.invalid".into()],
                ca_cert_path: Some(dir.join("test-ca.pem")),
            },
        )
        .unwrap();
        let manager =
            CertificateManager::open(&state_dir, &dir.join("cert.pem"), &dir.join("key.pem"))
                .unwrap();
        let table = crate::proxy::RouteTable::new();
        let bound = crate::proxy::bind(
            crate::proxy::ProxyConfig {
                cert_path: dir.join("cert.pem"),
                key_path: dir.join("key.pem"),
                bind_ip: "127.0.0.1".parse().unwrap(),
                https_port: 0,
                http_port: 0,
                admin_hostname: "admin.home.lan".into(),
            },
            Router::new().route("/", get(|| async { "management" })),
            table,
            crate::metrics::Metrics::new(),
        )
        .await
        .unwrap()
        .with_certificate_resolver(manager.resolver())
        .with_challenge_router(manager.challenge_router());
        let https = bound.https_addr;
        let http = bound.http_addr;
        let proxy = tokio::spawn(async move {
            bound.run().await.unwrap();
        });
        let service = Arc::new(AcmeService {
            base: format!("https://localhost:{acme_port}"),
            callback: http,
            ca,
            key: ca_key,
            state: StdMutex::new(AcmeState {
                nonce: 0,
                nonces: BTreeSet::new(),
                jwk: None,
                registrations: 0,
                signed_requests: 0,
                challenges: 0,
                orders: Vec::new(),
            }),
            fail: AtomicBool::new(false),
            failed_hostname: StdMutex::new(None),
        });
        let serving = service.clone();
        let task = tokio::spawn(async move {
            loop {
                let (stream, _) = acme_listener.accept().await.unwrap();
                let acceptor = acceptor.clone();
                let service = serving.clone();
                tokio::spawn(async move {
                    let stream = acceptor.accept(stream).await.unwrap();
                    let handler = hyper::service::service_fn(move |req| {
                        let service = service.clone();
                        async move { Ok::<_, Infallible>(service.handle(req).await) }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), handler)
                        .await;
                });
            }
        });
        let ca_der = service.ca.der().as_ref().to_vec();
        Self {
            dir,
            manager,
            service,
            https,
            http,
            ca: ca_der,
            tasks: vec![proxy, task],
        }
    }

    async fn peer_leaf(&self, hostname: &str, sni: bool) -> anyhow::Result<Vec<u8>> {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(self.ca.clone().into())?;
        roots.add(
            pem::parse(std::fs::read(self.dir.join("cert.pem"))?)?
                .into_contents()
                .into(),
        )?;
        let mut config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.enable_sni = sni;
        let stream = tokio::net::TcpStream::connect(self.https).await?;
        let connection = tokio_rustls::TlsConnector::from(Arc::new(config))
            .connect(
                rustls::pki_types::ServerName::try_from(hostname.to_string())?,
                stream,
            )
            .await?;
        Ok(connection.get_ref().1.peer_certificates().unwrap()[0]
            .as_ref()
            .to_vec())
    }
}

#[tokio::test]
async fn local_acme_checks_signed_requests_challenges_csr_sni_restart_and_live_renewal() {
    let fixture = Fixture::new().await;
    assert!(
        fixture
            .peer_leaf("blog.example.invalid", true)
            .await
            .is_err(),
        "missing managed certificate must refuse TLS"
    );
    assert!(
        fixture
            .peer_leaf("unknown.other.invalid", true)
            .await
            .is_err(),
        "unknown independent SNI must refuse TLS"
    );
    assert!(fixture.peer_leaf("admin.home.lan", true).await.is_ok());
    assert!(
        fixture.peer_leaf("admin.home.lan", false).await.is_ok(),
        "no SNI keeps LAN compatibility"
    );
    assert!(
        fixture.peer_leaf("deep.app.home.lan", true).await.is_err(),
        "LAN wildcard covers one label only"
    );
    fixture.manager.reconcile().await.unwrap();
    let first = fixture
        .peer_leaf("blog.example.invalid", true)
        .await
        .unwrap();
    let alias = fixture
        .peer_leaf("alias.other.invalid", true)
        .await
        .unwrap();
    assert_ne!(
        first, alias,
        "independent names receive separate certificates"
    );
    assert!(
        fixture
            .manager
            .status()
            .iter()
            .all(|status| status.state == CertificateState::Valid)
    );
    let account = std::fs::read(fixture.dir.join("state/certificates/account.json")).unwrap();
    let restored = CertificateManager::open(
        &fixture.dir.join("state/certificates"),
        &fixture.dir.join("cert.pem"),
        &fixture.dir.join("key.pem"),
    )
    .unwrap();
    assert_eq!(
        restored
            .resolver()
            .for_name(Some("blog.example.invalid"), now())
            .unwrap()
            .cert[0]
            .as_ref(),
        first
    );
    restored.reconcile().await.unwrap();
    assert_eq!(
        fixture.service.state.lock().unwrap().orders.len(),
        2,
        "restart does not reissue valid certs"
    );
    let renewal = fixture
        .manager
        .status()
        .iter()
        .map(|status| status.renew_at.unwrap())
        .max()
        .unwrap()
        + 1;
    fixture.manager.reconcile_at(renewal).await.unwrap();
    let second = fixture
        .peer_leaf("blog.example.invalid", true)
        .await
        .unwrap();
    assert_ne!(first, second, "live listener still serving first key");
    assert_eq!(
        std::fs::read(fixture.dir.join("state/certificates/account.json")).unwrap(),
        account
    );
    {
        let state = fixture.service.state.lock().unwrap();
        assert_eq!(state.registrations, 1);
        assert_eq!(state.challenges, 4);
        assert!(state.signed_requests >= 20);
    }
    let after_renewal = CertificateManager::open(
        &fixture.dir.join("state/certificates"),
        &fixture.dir.join("cert.pem"),
        &fixture.dir.join("key.pem"),
    )
    .unwrap();
    assert_eq!(
        after_renewal
            .resolver()
            .for_name(Some("blog.example.invalid"), now())
            .unwrap()
            .cert[0]
            .as_ref(),
        second
    );
    // Completed challenge tokens disappear. Host mismatches and encoded spellings never leak a response.
    for (host, path) in [
        (
            "blog.example.invalid",
            "/.well-known/acme-challenge/synthetic-token-0",
        ),
        (
            "unknown.invalid",
            "/.well-known/acme-challenge/synthetic-token-0",
        ),
        (
            "blog.example.invalid",
            "/.well-known/acme-challenge/%73ynthetic-token-0",
        ),
    ] {
        let response = reqwest::Client::new()
            .get(format!("http://{}{path}", fixture.http))
            .header("Host", host)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn failed_renewal_remains_visible_preserves_valid_keys_and_retries_after_an_hour() {
    let fixture = Fixture::new().await;
    fixture.manager.reconcile().await.unwrap();
    let first = fixture
        .peer_leaf("blog.example.invalid", true)
        .await
        .unwrap();
    let old_bundle = std::fs::read(
        fixture
            .dir
            .join("state/certificates/blog.example.invalid.json"),
    )
    .unwrap();
    fixture.service.fail.store(true, Ordering::SeqCst);
    let renewal = fixture
        .manager
        .status()
        .iter()
        .map(|status| status.renew_at.unwrap())
        .max()
        .unwrap()
        + 1;
    fixture.manager.reconcile_at(renewal).await.unwrap();
    assert_eq!(
        fixture
            .peer_leaf("blog.example.invalid", true)
            .await
            .unwrap(),
        first
    );
    assert_eq!(
        std::fs::read(
            fixture
                .dir
                .join("state/certificates/blog.example.invalid.json")
        )
        .unwrap(),
        old_bundle
    );
    let response = fixture
        .manager
        .status_router()
        .oneshot(
            Request::builder()
                .uri("/certificates")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let statuses: Vec<CertificateStatus> =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(
        statuses
            .iter()
            .all(|status| status.state == CertificateState::RenewalFailed
                && status.last_error.is_some()
                && status.retry_at == Some(renewal + 3600))
    );
    let restored = CertificateManager::open(
        &fixture.dir.join("state/certificates"),
        &fixture.dir.join("cert.pem"),
        &fixture.dir.join("key.pem"),
    )
    .unwrap();
    assert!(
        restored
            .status()
            .iter()
            .all(|status| status.last_error.is_some())
    );
    fixture.service.fail.store(false, Ordering::SeqCst);
    fixture.manager.reconcile_at(renewal + 3599).await.unwrap();
    assert_eq!(
        fixture
            .peer_leaf("blog.example.invalid", true)
            .await
            .unwrap(),
        first
    );
    fixture.manager.reconcile_at(renewal + 3600).await.unwrap();
    assert_ne!(
        fixture
            .peer_leaf("blog.example.invalid", true)
            .await
            .unwrap(),
        first
    );
    assert!(
        fixture
            .manager
            .status()
            .iter()
            .all(|status| status.last_error.is_none())
    );
    let expiration = fixture.manager.status()[0].expires_at.unwrap();
    assert!(
        fixture
            .manager
            .resolver()
            .for_name(Some("alias.other.invalid"), expiration)
            .is_none()
    );
}

#[tokio::test]
async fn persisted_credentials_and_certificate_keys_have_restricted_permissions() {
    let fixture = Fixture::new().await;
    fixture.manager.reconcile().await.unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir = fixture.dir.join("state/certificates");
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for file in [
            "public.json",
            "account.json",
            "status.json",
            "blog.example.invalid.json",
            "alias.other.invalid.json",
        ] {
            assert_eq!(
                std::fs::metadata(dir.join(file))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    let status = serde_json::to_string(&fixture.manager.status()).unwrap();
    assert!(!status.contains("PRIVATE KEY"));
    assert!(!status.contains("key_pkcs8"));
    assert!(!status.contains("BEGIN CERTIFICATE"));
}

#[test]
fn certificate_config_refuses_wildcards_ips_duplicates_path_traversal_and_credentials() {
    let config = CertificateConfig {
        directory_url: "https://acme.example.invalid/directory".into(),
        contact: vec![],
        hostnames: vec!["blog.example.invalid".into()],
        ca_cert_path: None,
    };
    for hostname in [
        "*.example.invalid",
        "127.0.0.1",
        "../../account",
        "app..invalid",
        "-app.example.invalid",
        "app_.example.invalid",
    ] {
        let mut invalid = config.clone();
        invalid.hostnames = vec![hostname.into()];
        assert!(validate_config(invalid).is_err());
    }
    let mut duplicate = config.clone();
    duplicate.hostnames.push("BLOG.EXAMPLE.INVALID".into());
    assert!(validate_config(duplicate).is_err());
    let mut credential = config.clone();
    credential.directory_url = "https://operator:password@acme.example.invalid/directory".into();
    assert!(validate_config(credential).is_err());
    let mut insecure = config;
    insecure.directory_url = "http://acme.example.invalid/directory".into();
    assert!(validate_config(insecure).is_err());
}

#[test]
fn renewal_window_tracks_short_lifetimes_and_caps_long_lifetimes() {
    let key = KeyPair::generate().unwrap();
    for (days, expected_days) in [(9, 3), (120, 30)] {
        let mut params = CertificateParams::new(vec!["blog.example.invalid".into()]).unwrap();
        params.not_before = time::OffsetDateTime::now_utc();
        params.not_after = params.not_before + time::Duration::days(days);
        let cert = params.self_signed(&key).unwrap();
        let entry = CertificateEntry::from_pem(&cert.pem(), &key.serialize_pem()).unwrap();
        assert_eq!(
            entry.expires_at - entry.renew_at(),
            expected_days * 24 * 3600
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_cannot_redirect_certificate_state_writes() {
    let fixture = Fixture::new().await;
    let state = fixture.dir.join("state/certificates");
    let target = fixture.dir.join("unrelated.json");
    std::fs::write(&target, b"preserve").unwrap();
    std::os::unix::fs::symlink(&target, state.join("account.json")).unwrap();
    fixture.manager.reconcile().await.unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"preserve");
    assert!(
        fixture
            .manager
            .status()
            .iter()
            .all(|status| status.last_error.as_deref() == Some("ACME account unavailable"))
    );
}

#[tokio::test]
async fn one_failed_alias_does_not_block_another_name_and_metadata_never_reads_keys() {
    let fixture = Fixture::new().await;
    *fixture.service.failed_hostname.lock().unwrap() = Some("alias.other.invalid".into());
    fixture.manager.reconcile().await.unwrap();
    assert!(
        fixture
            .peer_leaf("blog.example.invalid", true)
            .await
            .is_ok()
    );
    assert!(
        fixture
            .peer_leaf("alias.other.invalid", true)
            .await
            .is_err()
    );
    let state = fixture.dir.join("state/certificates");
    let statuses = read_status(&state).unwrap();
    assert_eq!(
        statuses
            .iter()
            .find(|status| status.hostname == "blog.example.invalid")
            .unwrap()
            .state,
        CertificateState::Valid
    );
    assert_eq!(
        statuses
            .iter()
            .find(|status| status.hostname == "alias.other.invalid")
            .unwrap()
            .state,
        CertificateState::Pending
    );
    std::fs::write(
        state.join("blog.example.invalid.json"),
        b"unreadable bundle",
    )
    .unwrap();
    assert_eq!(
        read_status(&state).unwrap().len(),
        2,
        "authenticated metadata endpoint must not read private bundles"
    );
    let reopened = CertificateManager::open(
        &state,
        &fixture.dir.join("cert.pem"),
        &fixture.dir.join("key.pem"),
    );
    assert!(reopened.is_err(), "malformed private state fails closed");
}

#[test]
fn status_marks_expired_and_future_certificates_unavailable() {
    let mut status = CertificateStatus::pending("blog.example.invalid".into());
    status.not_before = Some(100);
    status.expires_at = Some(300);
    refresh_status(&mut status, 99);
    assert_eq!(status.state, CertificateState::Pending);
    refresh_status(&mut status, 200);
    assert_eq!(status.state, CertificateState::Valid);
    refresh_status(&mut status, 300);
    assert_eq!(status.state, CertificateState::Expired);
}
