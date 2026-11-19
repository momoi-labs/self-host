use super::*;
use axum::extract::State;
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
    dns: Option<Arc<DnsFixture>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

const ZONE: &str = "int.example.invalid";
const DNS_TOKEN: &str = "fixture-desec-token";

struct TxtRecord {
    values: Vec<String>,
    written: std::time::Instant,
}

struct DnsFixture {
    table: StdMutex<BTreeMap<String, TxtRecord>>,
    lags: Vec<Duration>,
    refuse: AtomicBool,
    writes: AtomicU64,
    last_ttl: AtomicU64,
}

impl DnsFixture {
    fn answers(&self, server: usize, name: &str) -> Vec<String> {
        self.table
            .lock()
            .unwrap()
            .get(name)
            .filter(|record| record.written.elapsed() >= self.lags[server])
            .map(|record| record.values.clone())
            .unwrap_or_default()
    }

    fn put(&self, name: &str, values: Vec<String>, written: std::time::Instant) {
        self.table
            .lock()
            .unwrap()
            .insert(name.into(), TxtRecord { values, written });
    }

    fn has(&self, name: &str) -> bool {
        self.table.lock().unwrap().contains_key(name)
    }
}

async fn desec_put(
    State(dns): State<Arc<DnsFixture>>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<Vec<Value>>,
) -> (StatusCode, axum::Json<Value>) {
    assert_eq!(
        headers["authorization"],
        format!("Token {DNS_TOKEN}").as_str()
    );
    if dns.refuse.load(Ordering::SeqCst) {
        return (StatusCode::FORBIDDEN, axum::Json(json!({})));
    }
    for rrset in &body {
        assert_eq!(rrset["type"], "TXT", "the token writes TXT only");
        let name = format!("{}.{ZONE}", rrset["subname"].as_str().unwrap());
        let values = rrset["records"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().trim_matches('"').to_string())
            .collect();
        dns.writes.fetch_add(1, Ordering::SeqCst);
        dns.last_ttl
            .store(rrset["ttl"].as_u64().unwrap(), Ordering::SeqCst);
        dns.put(&name, values, std::time::Instant::now());
    }
    (StatusCode::OK, axum::Json(json!(body)))
}

async fn desec_delete(
    State(dns): State<Arc<DnsFixture>>,
    axum::extract::Path((_, subname)): axum::extract::Path<(String, String)>,
) -> StatusCode {
    dns.table
        .lock()
        .unwrap()
        .remove(&format!("{subname}.{ZONE}"));
    StatusCode::NO_CONTENT
}

async fn desec_domain() -> axum::Json<Value> {
    axum::Json(json!({"minimum_ttl": 900}))
}

async fn serve_dns(dns: Arc<DnsFixture>, index: usize, socket: tokio::net::UdpSocket) {
    use hickory_server::proto::op::{Message, MessageType, OpCode};
    use hickory_server::proto::rr::{RData, Record, rdata::TXT};
    let mut buffer = vec![0; 4096];
    loop {
        let (length, peer) = socket.recv_from(&mut buffer).await.unwrap();
        let query = Message::from_vec(&buffer[..length]).unwrap();
        let mut response = Message::new(query.id, MessageType::Response, OpCode::Query);
        response.metadata.authoritative = true;
        let question = query.queries[0].clone();
        let name = question
            .name()
            .to_ascii()
            .trim_end_matches('.')
            .to_lowercase();
        for value in dns.answers(index, &name) {
            response.answers.push(Record::from_rdata(
                question.name().clone(),
                60,
                RData::TXT(TXT::new(vec![value])),
            ));
        }
        response.add_query(question);
        socket
            .send_to(&response.to_vec().unwrap(), peer)
            .await
            .unwrap();
    }
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
    names: Vec<String>,
    tokens: Vec<String>,
    valid: Vec<bool>,
    chain: Option<String>,
}

impl AcmeOrder {
    fn ready(&self) -> bool {
        self.valid.iter().all(|valid| *valid)
    }
}
struct AcmeState {
    dns_challenges: usize,
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
    dns: Option<Arc<DnsFixture>>,
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
        let identifiers: Vec<Value> = order
            .names
            .iter()
            .map(|name| json!({"type":"dns","value":name}))
            .collect();
        let authorizations: Vec<String> = (0..order.names.len())
            .map(|index| format!("{}/authz/{id}-{index}", self.base))
            .collect();
        let mut value = json!({"status": if order.chain.is_some() { "valid" } else if order.ready() { "ready" } else { "pending" }, "identifiers":identifiers, "authorizations":authorizations, "finalize":format!("{}/finalize/{id}",self.base)});
        if order.chain.is_some() {
            value["certificate"] = json!(format!("{}/certificate/{id}", self.base));
        }
        value
    }
    fn challenge(&self, id: usize, index: usize) -> Value {
        let state = self.state.lock().unwrap();
        let order = &state.orders[id];
        let kind = if self.dns.is_some() && order.names[index].ends_with(ZONE) {
            "dns-01"
        } else {
            "http-01"
        };
        json!({"type":kind,"url":format!("{}/challenge/{id}-{index}",self.base),"status": if order.valid[index] { "valid" } else { "pending" },"token":order.tokens[index]})
    }

    fn dns_value(&self, token: &str) -> String {
        let jwk = self.state.lock().unwrap().jwk.clone().unwrap();
        let canonical =
            json!({"crv":jwk["crv"],"kty":jwk["kty"],"x":jwk["x"],"y":jwk["y"]}).to_string();
        let thumbprint = URL_SAFE_NO_PAD.encode(Sha256::digest(canonical));
        URL_SAFE_NO_PAD.encode(Sha256::digest(format!("{token}.{thumbprint}")))
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
            let names: Vec<String> = payload["identifiers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|identifier| {
                    assert_eq!(identifier["type"], "dns");
                    identifier["value"].as_str().unwrap().to_string()
                })
                .collect();
            match names[0].strip_prefix("*.") {
                Some(base) => assert_eq!(names, vec![names[0].clone(), base.to_string()]),
                None => assert_eq!(names.len(), 1),
            }
            let id = {
                let mut state = self.state.lock().unwrap();
                let id = state.orders.len();
                state.orders.push(AcmeOrder {
                    tokens: (0..names.len())
                        .map(|index| format!("synthetic-token-{id}-{index}"))
                        .collect(),
                    valid: vec![false; names.len()],
                    names,
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
        let (id, index) = match id.split_once('-') {
            Some((id, index)) => (id.parse().unwrap(), index.parse().unwrap()),
            None => (id.parse::<usize>().unwrap(), 0),
        };
        match kind {
            "authz" => {
                assert!(payload.is_null());
                let (name, valid) = {
                    let state = self.state.lock().unwrap();
                    (
                        state.orders[id].names[index].clone(),
                        state.orders[id].valid[index],
                    )
                };
                let wildcard = name.starts_with("*.");
                self.json(200, json!({"identifier":{"type":"dns","value":name.trim_start_matches("*.")},"wildcard":wildcard,"status":if valid { "valid" } else { "pending" },"challenges":[self.challenge(id, index)]}), None)
            }
            "challenge" => {
                assert_eq!(payload, json!({}));
                let (name, token, jwk, siblings) = {
                    let state = self.state.lock().unwrap();
                    let order = &state.orders[id];
                    let record = order.names[index].trim_start_matches("*.").to_string();
                    let siblings: Vec<String> = order
                        .names
                        .iter()
                        .zip(&order.tokens)
                        .filter(|(name, _)| name.trim_start_matches("*.") == record)
                        .map(|(_, token)| token.clone())
                        .collect();
                    (
                        record,
                        order.tokens[index].clone(),
                        state.jwk.clone().unwrap(),
                        siblings,
                    )
                };
                if let Some(dns) = self.dns.as_ref().filter(|_| name.ends_with(ZONE)) {
                    let mut expected: Vec<String> =
                        siblings.iter().map(|token| self.dns_value(token)).collect();
                    expected.sort();
                    for server in 0..dns.lags.len() {
                        let mut served = dns.answers(server, &format!("_acme-challenge.{name}"));
                        served.sort();
                        assert_eq!(
                            served, expected,
                            "every nameserver serves exactly the order's values at validation"
                        );
                    }
                    let mut state = self.state.lock().unwrap();
                    state.orders[id].valid[index] = true;
                    state.dns_challenges += 1;
                    drop(state);
                    return self.json(200, self.challenge(id, index), None);
                }
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
                    state.orders[id].valid[index] = true;
                    state.challenges += 1;
                }
                self.json(200, self.challenge(id, index), None)
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
                let names = {
                    let state = self.state.lock().unwrap();
                    assert!(state.orders[id].ready());
                    state.orders[id].names.clone()
                };
                assert_eq!(
                    csr.params.subject_alt_names,
                    names
                        .into_iter()
                        .map(|name| rcgen::SanType::DnsName(name.try_into().unwrap()))
                        .collect::<Vec<_>>()
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
        Self::with_dns(None).await
    }

    async fn with_dns(lags: Option<Vec<Duration>>) -> Self {
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
        let mut hostnames = vec!["blog.example.invalid".into(), "alias.other.invalid".into()];
        let mut tasks = Vec::new();
        let mut dns_config = None;
        let dns = match lags {
            Some(lags) => {
                let dns = Arc::new(DnsFixture {
                    table: StdMutex::new(BTreeMap::new()),
                    lags,
                    refuse: AtomicBool::new(false),
                    writes: AtomicU64::new(0),
                    last_ttl: AtomicU64::new(0),
                });
                let mut nameservers = Vec::new();
                for index in 0..dns.lags.len() {
                    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
                    nameservers.push(socket.local_addr().unwrap());
                    tasks.push(tokio::spawn(serve_dns(dns.clone(), index, socket)));
                }
                let api = Router::new()
                    .route("/api/v1/domains/{zone}/", get(desec_domain))
                    .route(
                        "/api/v1/domains/{zone}/rrsets/",
                        axum::routing::put(desec_put),
                    )
                    .route(
                        "/api/v1/domains/{zone}/rrsets/{subname}/TXT/",
                        axum::routing::delete(desec_delete),
                    )
                    .with_state(dns.clone());
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let api_url = format!("http://{}", listener.local_addr().unwrap());
                tasks.push(tokio::spawn(async move {
                    axum::serve(listener, api).await.unwrap();
                }));
                hostnames.push(format!("yard.{ZONE}"));
                dns_config = Some(DnsConfig {
                    provider: DnsProvider::Desec,
                    zone: ZONE.into(),
                    token: DNS_TOKEN.into(),
                    api_url: Some(api_url),
                    nameservers: Some(nameservers),
                });
                Some(dns)
            }
            None => None,
        };
        atomic_json(
            &state_dir.join("public.json"),
            &CertificateConfig {
                directory_url: format!("https://localhost:{acme_port}/directory"),
                contact: vec!["mailto:operator@example.invalid".into()],
                hostnames,
                ca_cert_path: Some(dir.join("test-ca.pem")),
                dns: dns_config,
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
                console_hostname: "console.home.lan".into(),
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
                dns_challenges: 0,
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
            dns: dns.clone(),
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
            dns,
            tasks: {
                tasks.extend([proxy, task]);
                tasks
            },
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

    /// The leaf an edge proxy that trusts the origin on its own terms sees:
    /// the handshake completes whatever the certificate says.
    async fn peer_leaf_unverified(&self, hostname: &str) -> anyhow::Result<Vec<u8>> {
        let config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(TrustAnything))
            .with_no_client_auth();
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

#[derive(Debug)]
struct TrustAnything;

impl rustls::client::danger::ServerCertVerifier for TrustAnything {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
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
    let local = fixture.peer_leaf("console.home.lan", true).await.unwrap();
    for name in ["unknown.other.invalid", "deep.app.home.lan"] {
        assert!(
            fixture.peer_leaf(name, true).await.is_err(),
            "{name}: a browser still rejects the mismatch"
        );
        assert_eq!(
            fixture.peer_leaf_unverified(name).await.unwrap(),
            local,
            "{name}: a name without its own certificate gets the Platform's, so an edge proxy that trusts the origin can serve it"
        );
    }
    assert!(
        fixture.peer_leaf("console.home.lan", false).await.is_ok(),
        "no SNI keeps LAN compatibility"
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
            "/.well-known/acme-challenge/synthetic-token-0-0",
        ),
        (
            "unknown.invalid",
            "/.well-known/acme-challenge/synthetic-token-0-0",
        ),
        (
            "blog.example.invalid",
            "/.well-known/acme-challenge/%73ynthetic-token-0-0",
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
        dns: None,
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

#[tokio::test]
async fn application_hostnames_are_issued_without_restart_and_withdrawn_when_they_go() {
    let fixture = Fixture::new().await;
    let state = fixture.dir.join("state/certificates");
    let shop = || BTreeSet::from(["shop.example.invalid".to_string()]);
    fixture.manager.reconcile_for(shop()).await.unwrap();
    let issued = fixture
        .peer_leaf("shop.example.invalid", true)
        .await
        .unwrap();
    assert_eq!(fixture.service.state.lock().unwrap().orders.len(), 3);
    assert_eq!(read_status(&state).unwrap().len(), 3);

    // The Hostname left its Application: the name stops being served, and
    // the certificate stays on disk.
    fixture.manager.reconcile().await.unwrap();
    assert!(
        fixture
            .peer_leaf("shop.example.invalid", true)
            .await
            .is_err()
    );
    assert_eq!(read_status(&state).unwrap().len(), 2);
    assert!(state.join("shop.example.invalid.json").exists());

    // Back again, it serves the saved certificate without a new order.
    fixture.manager.reconcile_for(shop()).await.unwrap();
    assert_eq!(
        fixture
            .peer_leaf("shop.example.invalid", true)
            .await
            .unwrap(),
        issued
    );
    assert_eq!(fixture.service.state.lock().unwrap().orders.len(), 3);

    // A restart keeps serving it until the Applications are read again.
    let restored = CertificateManager::open(
        &state,
        &fixture.dir.join("cert.pem"),
        &fixture.dir.join("key.pem"),
    )
    .unwrap();
    assert!(
        restored
            .resolver()
            .for_name(Some("shop.example.invalid"), now())
            .is_some()
    );

    // Names the Applications cannot have asked for are not ordered.
    fixture
        .manager
        .reconcile_for(BTreeSet::from([
            "*.example.invalid".to_string(),
            "UPPER.example.invalid".to_string(),
            "127.0.0.1".to_string(),
        ]))
        .await
        .unwrap();
    assert_eq!(fixture.service.state.lock().unwrap().orders.len(), 3);

    // Removing the account stops issuance and serving at the next read.
    remove_acme(&state).unwrap();
    fixture.manager.reconcile_for(shop()).await.unwrap();
    assert!(
        fixture
            .peer_leaf("blog.example.invalid", true)
            .await
            .is_err()
    );
    assert!(read_status(&state).unwrap().is_empty());
    assert!(read_acme(&state).unwrap().is_none());
}

#[test]
fn acme_settings_round_trip_keep_the_files_extras_and_restart_the_account_for_a_new_ca() {
    let dir = std::env::temp_dir().join(format!(
        "self-host-acme-settings-{}-{}",
        std::process::id(),
        FIXTURE_ID.fetch_add(1, Ordering::SeqCst)
    ));
    assert!(read_acme(&dir).unwrap().is_none());
    let settings = AcmeSettings {
        directory_url: "https://acme-staging-v02.api.letsencrypt.org/directory".into(),
        email: "operator@example.invalid".into(),
        dns: None,
    };
    write_acme(&dir, &settings).unwrap();
    assert_eq!(
        read_acme(&dir).unwrap(),
        Some(AcmeView {
            directory_url: settings.directory_url.clone(),
            email: settings.email.clone(),
            dns: None,
        })
    );
    let config = read_optional::<CertificateConfig>(&dir.join("public.json"))
        .unwrap()
        .unwrap();
    assert_eq!(config.contact, vec!["mailto:operator@example.invalid"]);
    assert!(config.hostnames.is_empty());

    // Extra names and a private trust anchor written by hand survive a save.
    let mut extended = config.clone();
    extended.hostnames = vec!["extra.example.invalid".into()];
    extended.ca_cert_path = Some(dir.join("ca.pem"));
    atomic_json(&dir.join("public.json"), &extended).unwrap();
    std::fs::write(dir.join("account.json"), b"credentials").unwrap();
    write_acme(
        &dir,
        &AcmeSettings {
            email: "other@example.invalid".into(),
            ..settings.clone()
        },
    )
    .unwrap();
    let saved = read_optional::<CertificateConfig>(&dir.join("public.json"))
        .unwrap()
        .unwrap();
    assert_eq!(saved.hostnames, vec!["extra.example.invalid"]);
    assert_eq!(saved.ca_cert_path, Some(dir.join("ca.pem")));
    assert!(
        dir.join("account.json").exists(),
        "same CA keeps its account"
    );

    write_acme(
        &dir,
        &AcmeSettings {
            directory_url: LETS_ENCRYPT_DIRECTORY.into(),
            ..settings.clone()
        },
    )
    .unwrap();
    assert!(
        !dir.join("account.json").exists(),
        "a new CA starts a new account"
    );

    for (directory_url, email) in [
        (
            "http://acme.example.invalid/directory",
            "operator@example.invalid",
        ),
        (LETS_ENCRYPT_DIRECTORY, "not an address"),
        (LETS_ENCRYPT_DIRECTORY, ""),
    ] {
        assert!(
            write_acme(
                &dir,
                &AcmeSettings {
                    directory_url: directory_url.into(),
                    email: email.into(),
                    dns: None,
                }
            )
            .is_err(),
            "{directory_url} {email}"
        );
    }
    remove_acme(&dir).unwrap();
    remove_acme(&dir).unwrap();
    assert!(read_acme(&dir).unwrap().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn public_hostnames_are_the_applications_names_outside_the_suffix() {
    use crate::store::{ApplicationRecord, Publication, RouteRule};
    let app = |hostname: &str, aliases: &[&str], rules: &[&str]| ApplicationRecord {
        id: hostname.into(),
        name: "blog".into(),
        hostname: hostname.into(),
        aliases: aliases.iter().map(|alias| alias.to_string()).collect(),
        image: "nginx".into(),
        status: "running".into(),
        source: "image".into(),
        git: None,
        git_build: None,
        last_error: None,
        compose: None,
        web_service: None,
        web_port: None,
        web_target_port: Some(20001),
        development: None,
        runtime: Default::default(),
        publication: Publication::Web,
        variable_delivery: crate::store::VariableDelivery::Referenced,
        route_rules: rules
            .iter()
            .map(|hostname| RouteRule {
                hostname: hostname.to_string(),
                path_prefix: "/app".into(),
                target: None,
                strip_prefix: false,
            })
            .collect(),
        redirects: Vec::new(),
        rewrite_host: None,
        network_policy: Default::default(),
    };
    let mut unpublished = app("hidden.example.invalid", &[], &[]);
    unpublished.publication = Publication::Unpublished;
    let apps = vec![
        app(
            "blog.home.lan",
            &["Blog.Example.Invalid", "www.example.invalid"],
            &["labs.example.invalid"],
        ),
        app(
            "api.example.invalid",
            &["home.lan", "deep.app.home.lan"],
            &[],
        ),
        unpublished,
    ];
    assert_eq!(
        public_hostnames(&apps, "home.lan"),
        BTreeSet::from([
            "api.example.invalid".to_string(),
            "blog.example.invalid".to_string(),
            "labs.example.invalid".to_string(),
            "www.example.invalid".to_string(),
        ])
    );
}

fn first_status(fixture: &Fixture, hostname: &str) -> CertificateStatus {
    fixture
        .manager
        .status()
        .into_iter()
        .find(|status| status.hostname == hostname)
        .unwrap()
}

#[tokio::test]
async fn names_in_the_zone_use_dns_01_and_leave_no_record_while_others_keep_http_01() {
    let fixture = Fixture::with_dns(Some(vec![Duration::ZERO, Duration::ZERO])).await;
    fixture.manager.reconcile().await.unwrap();
    let dns = fixture.dns.as_ref().unwrap();
    let yard = format!("yard.{ZONE}");
    assert!(fixture.peer_leaf(&yard, true).await.is_ok());
    assert!(
        fixture
            .peer_leaf("blog.example.invalid", true)
            .await
            .is_ok()
    );
    {
        let state = fixture.service.state.lock().unwrap();
        assert_eq!(state.dns_challenges, 1);
        assert_eq!(state.challenges, 2, "the other two names use HTTP-01");
    }
    assert_eq!(dns.writes.load(Ordering::SeqCst), 1);
    assert_eq!(dns.last_ttl.load(Ordering::SeqCst), 900);
    assert!(!dns.has(&format!("_acme-challenge.{yard}")));
    assert_eq!(first_status(&fixture, &yard).state, CertificateState::Valid);

    let state = fixture.dir.join("state/certificates");
    assert_eq!(
        read_acme(&state).unwrap().unwrap().dns,
        Some(DnsView {
            provider: DnsProvider::Desec,
            zone: ZONE.into(),
            token_set: true,
        })
    );
    let view = serde_json::to_string(&read_acme(&state).unwrap()).unwrap();
    assert!(!view.contains(DNS_TOKEN));
    assert!(
        !serde_json::to_string(&fixture.manager.status())
            .unwrap()
            .contains(DNS_TOKEN)
    );
    assert!(
        !std::fs::read_to_string(state.join("status.json"))
            .unwrap()
            .contains(DNS_TOKEN)
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(state.join("public.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[tokio::test]
async fn a_nameserver_that_answers_late_is_waited_for() {
    let fixture = Fixture::with_dns(Some(vec![Duration::ZERO, Duration::from_millis(2500)])).await;
    let started = std::time::Instant::now();
    fixture.manager.reconcile().await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(2500));
    assert_eq!(
        first_status(&fixture, &format!("yard.{ZONE}")).state,
        CertificateState::Valid
    );
}

#[tokio::test]
async fn a_stale_txt_from_a_crashed_run_is_replaced_not_trusted() {
    let fixture = Fixture::with_dns(Some(vec![Duration::ZERO, Duration::ZERO])).await;
    let dns = fixture.dns.as_ref().unwrap();
    let name = format!("_acme-challenge.yard.{ZONE}");
    dns.put(
        &name,
        vec!["stale-value-from-a-crashed-run".into()],
        std::time::Instant::now() - Duration::from_secs(3600),
    );
    fixture.manager.reconcile().await.unwrap();
    assert_eq!(
        first_status(&fixture, &format!("yard.{ZONE}")).state,
        CertificateState::Valid
    );
    assert!(!dns.has(&name));
}

#[tokio::test]
async fn a_record_that_never_reaches_every_nameserver_says_so_and_is_removed() {
    let fixture = Fixture::with_dns(Some(vec![Duration::ZERO, Duration::from_secs(3600)])).await;
    let dns = fixture.dns.as_ref().unwrap();
    let yard = format!("yard.{ZONE}");
    fixture.manager.set_dns_wait(Duration::from_secs(5));
    fixture.manager.reconcile().await.unwrap();
    let status = first_status(&fixture, &yard);
    assert_eq!(
        status.last_error.as_deref(),
        Some("challenge record did not reach the nameservers")
    );
    assert_eq!(status.state, CertificateState::Pending);
    assert!(!dns.has(&format!("_acme-challenge.{yard}")));
}

#[tokio::test]
async fn a_refused_record_keeps_the_old_certificate_and_retries_in_an_hour() {
    let yard = format!("yard.{ZONE}");
    let fixture = Fixture::with_dns(Some(vec![Duration::ZERO, Duration::ZERO])).await;
    let dns = fixture.dns.as_ref().unwrap();
    fixture.manager.reconcile().await.unwrap();
    let first = fixture.peer_leaf(&yard, true).await.unwrap();
    dns.refuse.store(true, Ordering::SeqCst);
    let renewal = first_status(&fixture, &yard).renew_at.unwrap() + 1;
    fixture.manager.reconcile_at(renewal).await.unwrap();
    let status = first_status(&fixture, &yard);
    assert_eq!(
        status.last_error.as_deref(),
        Some("DNS provider refused the challenge record")
    );
    assert_eq!(status.state, CertificateState::RenewalFailed);
    assert_eq!(status.retry_at, Some(renewal + 3600));
    assert_eq!(fixture.peer_leaf(&yard, true).await.unwrap(), first);
}

fn wildcard_orders(fixture: &Fixture) -> Vec<Vec<String>> {
    fixture
        .service
        .state
        .lock()
        .unwrap()
        .orders
        .iter()
        .filter(|order| order.names[0].starts_with("*."))
        .map(|order| order.names.clone())
        .collect()
}

fn local_leaf(fixture: &Fixture) -> Vec<u8> {
    pem::parse(std::fs::read(fixture.dir.join("cert.pem")).unwrap())
        .unwrap()
        .into_contents()
}

#[tokio::test]
async fn the_suffix_inside_the_zone_gets_one_wildcard_through_a_late_nameserver() {
    let fixture = Fixture::with_dns(Some(vec![Duration::ZERO, Duration::from_millis(2500)])).await;
    let dns = fixture.dns.as_ref().unwrap();
    fixture.manager.set_suffix(ZONE);
    let started = std::time::Instant::now();
    fixture.manager.reconcile().await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(2500));
    let wildcard = format!("*.{ZONE}");
    assert_eq!(
        wildcard_orders(&fixture),
        vec![vec![wildcard.clone(), ZONE.to_string()]]
    );
    assert_eq!(
        first_status(&fixture, &wildcard).state,
        CertificateState::Valid
    );
    assert!(!dns.has(&format!("_acme-challenge.{ZONE}")));
    let app = fixture
        .peer_leaf(&format!("app.{ZONE}"), true)
        .await
        .unwrap();
    assert_eq!(fixture.peer_leaf(ZONE, true).await.unwrap(), app);
    assert_eq!(
        fixture
            .peer_leaf(&format!("console.{ZONE}"), true)
            .await
            .unwrap(),
        app
    );
    assert_ne!(
        fixture
            .peer_leaf(&format!("yard.{ZONE}"), true)
            .await
            .unwrap(),
        app,
        "a name with its own certificate keeps it"
    );
    assert_eq!(
        fixture
            .peer_leaf_unverified(&format!("deep.app.{ZONE}"))
            .await
            .unwrap(),
        local_leaf(&fixture),
        "a wildcard covers one label only"
    );
}

#[tokio::test]
async fn a_managed_name_without_a_valid_own_certificate_is_served_by_the_wildcard() {
    let fixture = Fixture::with_dns(Some(vec![Duration::ZERO, Duration::ZERO])).await;
    let yard = format!("yard.{ZONE}");
    fixture.manager.set_suffix(ZONE);
    fixture.manager.reconcile().await.unwrap();
    let own = fixture.peer_leaf(&yard, true).await.unwrap();
    let wildcard = fixture
        .peer_leaf(&format!("app.{ZONE}"), true)
        .await
        .unwrap();
    assert_ne!(own, wildcard);
    fixture
        .manager
        .resolver()
        .public
        .write()
        .unwrap()
        .remove(&yard);
    assert_eq!(fixture.peer_leaf(&yard, true).await.unwrap(), wildcard);
}

#[tokio::test]
async fn names_in_the_suffix_get_the_lan_leaf_until_the_wildcard_is_issued() {
    let fixture = Fixture::with_dns(Some(vec![Duration::ZERO, Duration::ZERO])).await;
    let dns = fixture.dns.as_ref().unwrap();
    let app = format!("app.{ZONE}");
    let wildcard = format!("*.{ZONE}");
    fixture.manager.set_suffix(ZONE);
    assert_eq!(
        fixture.peer_leaf_unverified(&app).await.unwrap(),
        local_leaf(&fixture)
    );
    dns.refuse.store(true, Ordering::SeqCst);
    let now = now();
    fixture.manager.reconcile_at(now).await.unwrap();
    let status = first_status(&fixture, &wildcard);
    assert_eq!(status.state, CertificateState::Pending);
    assert_eq!(
        status.last_error.as_deref(),
        Some("DNS provider refused the challenge record")
    );
    assert_eq!(
        fixture.peer_leaf_unverified(&app).await.unwrap(),
        local_leaf(&fixture)
    );
    dns.refuse.store(false, Ordering::SeqCst);
    fixture
        .manager
        .reconcile_at(status.retry_at.unwrap())
        .await
        .unwrap();
    assert_eq!(
        first_status(&fixture, &wildcard).state,
        CertificateState::Valid
    );
    assert_ne!(
        fixture.peer_leaf(&app, true).await.unwrap(),
        local_leaf(&fixture)
    );
}

#[tokio::test]
async fn the_wildcard_renews_at_its_renewal_time() {
    let fixture = Fixture::with_dns(Some(vec![Duration::ZERO, Duration::ZERO])).await;
    let app = format!("app.{ZONE}");
    let wildcard = format!("*.{ZONE}");
    fixture.manager.set_suffix(ZONE);
    fixture.manager.reconcile().await.unwrap();
    let first = fixture.peer_leaf(&app, true).await.unwrap();
    let status = first_status(&fixture, &wildcard);
    fixture
        .manager
        .reconcile_at(status.renew_at.unwrap() - 1)
        .await
        .unwrap();
    assert_eq!(wildcard_orders(&fixture).len(), 1);
    fixture
        .manager
        .reconcile_at(status.renew_at.unwrap() + 1)
        .await
        .unwrap();
    assert_eq!(wildcard_orders(&fixture).len(), 2);
    assert_ne!(fixture.peer_leaf(&app, true).await.unwrap(), first);
    let renewed = first_status(&fixture, &wildcard);
    assert_eq!(renewed.state, CertificateState::Valid);
    assert!(renewed.renew_at > status.renew_at);
}

#[test]
fn dns_settings_keep_the_token_replace_the_zone_and_never_show_the_token() {
    let dir = std::env::temp_dir().join(format!(
        "self-host-dns-settings-{}-{}",
        std::process::id(),
        FIXTURE_ID.fetch_add(1, Ordering::SeqCst)
    ));
    let settings = |dns: Option<Option<DnsSettings>>| AcmeSettings {
        directory_url: LETS_ENCRYPT_DIRECTORY.into(),
        email: "operator@example.invalid".into(),
        dns,
    };
    let zone = |zone: &str, token: Option<&str>| {
        Some(Some(DnsSettings {
            provider: DnsProvider::Desec,
            zone: zone.into(),
            token: token.map(Into::into),
        }))
    };
    assert!(write_acme(&dir, &settings(zone(ZONE, None))).is_err());
    assert!(write_acme(&dir, &settings(zone(ZONE, Some("bad token")))).is_err());
    assert!(write_acme(&dir, &settings(zone("not a zone", Some("secret")))).is_err());
    write_acme(&dir, &settings(zone(ZONE, Some("secret")))).unwrap();
    let saved = read_acme(&dir).unwrap().unwrap();
    assert_eq!(saved.dns.unwrap().zone, ZONE);
    assert!(
        !serde_json::to_string(&read_acme(&dir).unwrap())
            .unwrap()
            .contains("secret")
    );

    write_acme(&dir, &settings(zone("other.example.invalid", None))).unwrap();
    let config = read_config(&dir).unwrap().unwrap().dns.unwrap();
    assert_eq!(
        (config.zone.as_str(), config.token.as_str()),
        ("other.example.invalid", "secret")
    );

    write_acme(&dir, &settings(None)).unwrap();
    assert!(
        read_config(&dir).unwrap().unwrap().dns.is_some(),
        "absent keeps it"
    );
    write_acme(&dir, &settings(Some(None))).unwrap();
    assert!(
        read_config(&dir).unwrap().unwrap().dns.is_none(),
        "null removes it"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
