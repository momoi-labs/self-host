//! An embedded HTTP/HTTPS proxy, replacing Traefik (ADR-0019).
//!
//! `admin.<suffix>` is served in-process from the console/API `Router`, with
//! no network hop. Every other known Hostname is reverse-proxied to a Web
//! Target resolved by a [`Publisher`]. An unknown Hostname is a `404` that
//! never falls through to the admin API or another Application; a known
//! Application with no reachable target right now is a `503`.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::Context as _;
use axum::Router;
use axum::body::{Body as AxumBody, Bytes};
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::header::{self, HeaderValue};
use hyper::{Request, Response, StatusCode, Uri};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;

type BoxError = Box<dyn std::error::Error + Send + Sync>;
// `UnsyncBoxBody`, not `BoxBody`: axum's own response body isn't `Sync`
// (streaming bodies generally aren't), and every branch here has to box into
// the same type.
type ProxyBody = http_body_util::combinators::UnsyncBoxBody<Bytes, BoxError>;

fn empty_body() -> ProxyBody {
    Empty::<Bytes>::new().map_err(BoxError::from).boxed_unsync()
}

fn text_body(text: impl Into<Bytes>) -> ProxyBody {
    Full::new(text.into())
        .map_err(BoxError::from)
        .boxed_unsync()
}

/// Every Hostname an Application answers on, and where it currently resolves.
///
/// `publish`/`withdraw` are the whole contract execution needs to keep the
/// route table honest: an Application's identity, its Hostnames, and a
/// reachable Web Target — never a container name or a Compose service. A
/// `target` of `None` means the Application is known but has nothing to
/// answer with right now; the caller still gets a bounded `503`, not a `404`
/// that reads as "no such Application".
pub trait Publisher: Send + Sync + 'static {
    fn publish(&self, id: &str, hostnames: &[String], target: Option<SocketAddr>);
    fn withdraw(&self, id: &str);
}

/// The route table the proxy reads on every request. Kept in memory: a
/// Hostname or alias change is a write here, never a container recreation.
#[derive(Clone, Default)]
pub struct RouteTable {
    inner: Arc<RwLock<Tables>>,
}

#[derive(Default)]
struct Tables {
    targets: HashMap<String, Option<SocketAddr>>,
    hostnames_by_id: HashMap<String, Vec<String>>,
}

impl RouteTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// `None`: no Application answers on this Hostname. `Some(None)`: an
    /// Application does, but has no reachable target right now.
    pub fn target_for(&self, host: &str) -> Option<Option<SocketAddr>> {
        self.inner
            .read()
            .unwrap()
            .targets
            .get(&host.to_ascii_lowercase())
            .copied()
    }
}

impl Publisher for RouteTable {
    fn publish(&self, id: &str, hostnames: &[String], target: Option<SocketAddr>) {
        let mut tables = self.inner.write().unwrap();
        if let Some(previous) = tables.hostnames_by_id.remove(id) {
            for hostname in previous {
                tables.targets.remove(&hostname);
            }
        }
        let normalized: Vec<String> = hostnames.iter().map(|h| h.to_ascii_lowercase()).collect();
        for hostname in &normalized {
            tables.targets.insert(hostname.clone(), target);
        }
        tables.hostnames_by_id.insert(id.to_string(), normalized);
    }

    fn withdraw(&self, id: &str) {
        let mut tables = self.inner.write().unwrap();
        if let Some(hostnames) = tables.hostnames_by_id.remove(id) {
            for hostname in hostnames {
                tables.targets.remove(&hostname);
            }
        }
    }
}

pub struct ProxyConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub bind_ip: IpAddr,
    pub https_port: u16,
    pub http_port: u16,
    /// `admin.<suffix>` — dispatched to `admin_router` with no proxy hop.
    pub admin_hostname: String,
}

struct Context {
    admin_hostname: String,
    admin_router: Router,
    table: RouteTable,
    client: Client<HttpConnector, ProxyBody>,
}

/// Both listeners, bound but not yet serving. Bind before serving so a port
/// conflict on either one fails startup instead of half-starting — the same
/// split `dns::bind`/`dns::start` uses.
pub struct Bound {
    pub https_addr: SocketAddr,
    pub http_addr: SocketAddr,
    https_listener: TcpListener,
    http_listener: TcpListener,
    acceptor: TlsAcceptor,
    ctx: Arc<Context>,
    http_setup_router: Router,
}

pub async fn bind(
    config: ProxyConfig,
    admin_router: Router,
    table: RouteTable,
) -> anyhow::Result<Bound> {
    let tls_config = load_tls_config(&config.cert_path, &config.key_path)?;
    let acceptor = TlsAcceptor::from(Arc::new(tls_config));

    let mut connector = HttpConnector::new();
    connector.set_connect_timeout(Some(Duration::from_secs(5)));
    let client: Client<HttpConnector, ProxyBody> =
        Client::builder(TokioExecutor::new()).build(connector);

    let https_listener = TcpListener::bind(SocketAddr::new(config.bind_ip, config.https_port))
        .await
        .context("bind HTTPS proxy listener")?;
    let http_listener = TcpListener::bind(SocketAddr::new(config.bind_ip, config.http_port))
        .await
        .context("bind HTTP listener")?;

    let http_setup_router = crate::console::http_setup_router(
        config.admin_hostname.clone(),
        config.cert_path.with_file_name("ca.pem"),
    );

    Ok(Bound {
        https_addr: https_listener.local_addr()?,
        http_addr: http_listener.local_addr()?,
        https_listener,
        http_listener,
        acceptor,
        http_setup_router,
        ctx: Arc::new(Context {
            admin_hostname: config.admin_hostname,
            admin_router,
            table,
            client,
        }),
    })
}

impl Bound {
    /// Serves both listeners forever.
    pub async fn run(self) -> anyhow::Result<()> {
        tracing::info!(https = %self.https_addr, http = %self.http_addr, "proxy listening");
        tokio::spawn(serve_http(self.http_listener, self.http_setup_router));
        serve_https(self.https_listener, self.acceptor, self.ctx).await
    }
}

/// Binds the HTTP and HTTPS listeners and runs both forever.
pub async fn serve(
    config: ProxyConfig,
    admin_router: Router,
    table: RouteTable,
) -> anyhow::Result<()> {
    bind(config, admin_router, table).await?.run().await
}

async fn serve_https(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    ctx: Arc<Context>,
) -> anyhow::Result<()> {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(error) => {
                tracing::warn!("proxy accept failed: {error}");
                continue;
            }
        };
        let acceptor = acceptor.clone();
        let ctx = ctx.clone();
        tokio::spawn(async move {
            let tls_stream = match acceptor.accept(stream).await {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::debug!(%peer, "TLS handshake failed: {error}");
                    return;
                }
            };
            let io = TokioIo::new(tls_stream);
            let service = hyper::service::service_fn(move |req| {
                let ctx = ctx.clone();
                async move { Ok::<_, Infallible>(handle(req, ctx, peer.ip()).await) }
            });
            // HTTP/2 or HTTP/1.1, whichever the client negotiated over ALPN.
            // Upgrades stay available on the HTTP/1.1 side, which is where a
            // browser opens a WebSocket anyway.
            if let Err(error) = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(io, service)
                .await
            {
                tracing::debug!(%peer, "proxy connection error: {error}");
            }
        });
    }
}

async fn serve_http(listener: TcpListener, setup_router: Router) {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(error) => {
                tracing::warn!("HTTP listener accept failed: {error}");
                continue;
            }
        };
        let setup_router = setup_router.clone();
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let service = hyper::service::service_fn(move |req: Request<Incoming>| {
                let setup_router = setup_router.clone();
                async move {
                    let path = req.uri().path();
                    let is_setup = path == "/setup"
                        || path.starts_with("/setup/")
                        || path.starts_with("/console/assets/");
                    let response = if is_setup && requested_by_ip(&req) {
                        serve_router(&setup_router, req).await
                    } else {
                        redirect_to_https(&req)
                    };
                    Ok::<_, Infallible>(response)
                }
            });
            if let Err(error) = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, service)
                .await
            {
                tracing::debug!(%peer, "HTTP connection error: {error}");
            }
        });
    }
}

fn requested_by_ip(req: &Request<Incoming>) -> bool {
    req.headers()
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .and_then(|host| host.parse::<hyper::http::uri::Authority>().ok())
        .is_some_and(|authority| {
            authority
                .host()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<IpAddr>()
                .is_ok()
        })
}

fn redirect_to_https(req: &Request<Incoming>) -> Response<ProxyBody> {
    let Some(host) = host_of(req) else {
        return bad_request("missing Host header");
    };
    let path = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    let location = format!("https://{host}{path}");
    Response::builder()
        .status(StatusCode::MOVED_PERMANENTLY)
        .header(header::LOCATION, location)
        .body(empty_body())
        .unwrap()
}

/// The Hostname a client asked for: `Host` header first (what browsers and
/// almost everything else sends on HTTP/1.1), the request's own authority as
/// a fallback. A port suffix, if present, is not part of the Hostname.
fn host_of(req: &Request<Incoming>) -> Option<String> {
    let raw = req
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| req.uri().host());
    raw.map(|host| host.split(':').next().unwrap_or(host).to_ascii_lowercase())
}

async fn handle(req: Request<Incoming>, ctx: Arc<Context>, peer_ip: IpAddr) -> Response<ProxyBody> {
    let Some(host) = host_of(&req) else {
        return bad_request("missing Host header");
    };

    if host == ctx.admin_hostname {
        return serve_router(&ctx.admin_router, req).await;
    }

    match ctx.table.target_for(&host) {
        None => not_found(),
        Some(None) => service_unavailable(),
        Some(Some(target)) => proxy_to(&ctx.client, req, &host, target, peer_ip).await,
    }
}

async fn serve_router(router: &Router, req: Request<Incoming>) -> Response<ProxyBody> {
    let req = req.map(AxumBody::new);
    let response = router
        .clone()
        .oneshot(req)
        .await
        .unwrap_or_else(|never| match never {});
    let (parts, body) = response.into_parts();
    Response::from_parts(parts, body.map_err(BoxError::from).boxed_unsync())
}

/// Header names hop-by-hop between us and one side of the connection, and
/// never meaningful to forward as-is (RFC 7230 §6.1). `Connection` and
/// `Upgrade` are kept when the request is itself an upgrade, since the
/// backend needs to see them to complete its half of the handshake.
const HOP_BY_HOP: &[&str] = &[
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
];

/// Headers a client could use to lie about who it is. They are dropped from
/// every forwarded request and replaced with what this connection actually
/// observed, so an Application never receives an identity it did not earn.
const FORWARDED_IDENTITY: &[&str] = &[
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-proto",
    "x-forwarded-host",
    "x-real-ip",
];

async fn proxy_to(
    client: &Client<HttpConnector, ProxyBody>,
    mut req: Request<Incoming>,
    host: &str,
    target: SocketAddr,
    peer_ip: IpAddr,
) -> Response<ProxyBody> {
    let is_upgrade = req
        .headers()
        .get(header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_ascii_lowercase().contains("upgrade"))
        .unwrap_or(false);

    let client_upgrade = is_upgrade.then(|| hyper::upgrade::on(&mut req));

    let (mut parts, body) = req.into_parts();
    for name in HOP_BY_HOP {
        parts.headers.remove(*name);
    }
    if !is_upgrade {
        parts.headers.remove(header::CONNECTION);
        parts.headers.remove(header::UPGRADE);
    }
    for name in FORWARDED_IDENTITY {
        parts.headers.remove(*name);
    }
    parts
        .headers
        .insert("x-forwarded-proto", HeaderValue::from_static("https"));
    if let Ok(value) = HeaderValue::from_str(host) {
        parts.headers.insert("x-forwarded-host", value);
    }
    if let Ok(value) = HeaderValue::from_str(&peer_ip.to_string()) {
        parts.headers.insert("x-forwarded-for", value);
    }

    // An HTTP/2 request carries its Hostname as `:authority` and has no
    // `Host` header at all. Downgrading it to HTTP/1.1 without putting one
    // back would hand the Application the target's address as its Hostname,
    // and an Application that builds URLs from `Host` would answer with links
    // to a loopback port.
    if !parts.headers.contains_key(header::HOST)
        && let Some(authority) = parts.uri.authority()
        && let Ok(value) = HeaderValue::from_str(authority.as_str())
    {
        parts.headers.insert(header::HOST, value);
    }

    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    parts.uri = match format!("http://{target}{path_and_query}").parse::<Uri>() {
        Ok(uri) => uri,
        Err(_) => return bad_request("invalid request target"),
    };
    // Applications are reached over HTTP/1.1, as they were behind Traefik.
    parts.version = hyper::Version::HTTP_11;

    let outgoing = Request::from_parts(parts, body.map_err(BoxError::from).boxed_unsync());

    let mut backend_response = match client.request(outgoing).await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%target, "proxy request failed: {error}");
            return bad_gateway();
        }
    };

    if backend_response.status() == StatusCode::SWITCHING_PROTOCOLS
        && let Some(client_upgrade) = client_upgrade
    {
        let backend_upgrade = hyper::upgrade::on(&mut backend_response);
        tokio::spawn(async move {
            match (client_upgrade.await, backend_upgrade.await) {
                (Ok(client_io), Ok(backend_io)) => {
                    let mut client_io = TokioIo::new(client_io);
                    let mut backend_io = TokioIo::new(backend_io);
                    if let Err(error) =
                        tokio::io::copy_bidirectional(&mut client_io, &mut backend_io).await
                    {
                        tracing::debug!("upgraded connection closed: {error}");
                    }
                }
                _ => tracing::debug!("upgrade handshake did not complete on both sides"),
            }
        });
    }

    let (mut parts, body) = backend_response.into_parts();
    // What the Application said to us about this one connection is not for
    // the Consumer, and HTTP/2 forbids these headers outright. An upgrade is
    // the exception: its `Connection` and `Upgrade` are the handshake.
    if parts.status != StatusCode::SWITCHING_PROTOCOLS {
        for name in HOP_BY_HOP {
            parts.headers.remove(*name);
        }
        parts.headers.remove(header::CONNECTION);
        parts.headers.remove(header::UPGRADE);
        parts.headers.remove(header::TRANSFER_ENCODING);
    }
    Response::from_parts(parts, body.map_err(BoxError::from).boxed_unsync())
}

fn not_found() -> Response<ProxyBody> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(text_body("not found"))
        .unwrap()
}

fn service_unavailable() -> Response<ProxyBody> {
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .body(text_body("this Application has no reachable target"))
        .unwrap()
}

fn bad_gateway() -> Response<ProxyBody> {
    Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .body(text_body("upstream did not answer"))
        .unwrap()
}

fn bad_request(message: &'static str) -> Response<ProxyBody> {
    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .body(text_body(message))
        .unwrap()
}

fn load_tls_config(cert_path: &Path, key_path: &Path) -> anyhow::Result<rustls::ServerConfig> {
    let _ = rustls::crypto::ring::default_provider().install_default();

    let certs = load_certs(cert_path)?;
    let key = load_key(key_path)?;
    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("build TLS server config")?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

fn load_certs(path: &Path) -> anyhow::Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("parse certificates in {}", path.display()))
}

fn load_key(path: &Path) -> anyhow::Result<rustls::pki_types::PrivateKeyDer<'static>> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    rustls_pemfile::private_key(&mut reader)
        .with_context(|| format!("parse private key in {}", path.display()))?
        .ok_or_else(|| anyhow::anyhow!("no private key found in {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    #[test]
    fn unknown_hostname_resolves_to_nothing() {
        let table = RouteTable::new();
        assert_eq!(table.target_for("blog.home.lan"), None);
    }

    #[test]
    fn publish_makes_every_hostname_and_alias_resolve() {
        let table = RouteTable::new();
        let target = addr(8080);
        table.publish(
            "abc",
            &["blog.home.lan".into(), "writing.home.lan".into()],
            Some(target),
        );
        assert_eq!(table.target_for("blog.home.lan"), Some(Some(target)));
        assert_eq!(table.target_for("writing.home.lan"), Some(Some(target)));
        assert_eq!(table.target_for("BLOG.HOME.LAN"), Some(Some(target)));
    }

    #[test]
    fn a_target_of_none_is_a_known_but_unavailable_application() {
        let table = RouteTable::new();
        table.publish("abc", &["blog.home.lan".into()], None);
        assert_eq!(table.target_for("blog.home.lan"), Some(None));
    }

    #[test]
    fn republishing_drops_a_removed_alias() {
        let table = RouteTable::new();
        let target = addr(8080);
        table.publish(
            "abc",
            &["blog.home.lan".into(), "old.home.lan".into()],
            Some(target),
        );
        table.publish("abc", &["blog.home.lan".into()], Some(target));
        assert_eq!(table.target_for("blog.home.lan"), Some(Some(target)));
        assert_eq!(table.target_for("old.home.lan"), None);
    }

    #[test]
    fn withdraw_removes_every_hostname_the_id_published() {
        let table = RouteTable::new();
        table.publish(
            "abc",
            &["blog.home.lan".into(), "writing.home.lan".into()],
            Some(addr(8080)),
        );
        table.withdraw("abc");
        assert_eq!(table.target_for("blog.home.lan"), None);
        assert_eq!(table.target_for("writing.home.lan"), None);
        // Withdrawing what is already gone is not an error.
        table.withdraw("abc");
    }

    #[test]
    fn two_applications_never_see_each_others_hostname_removed() {
        let table = RouteTable::new();
        table.publish("a", &["a.home.lan".into()], Some(addr(1)));
        table.publish("b", &["b.home.lan".into()], Some(addr(2)));
        table.withdraw("a");
        assert_eq!(table.target_for("a.home.lan"), None);
        assert_eq!(table.target_for("b.home.lan"), Some(Some(addr(2))));
    }
}

/// End to end: a real TLS listener, a real plain-HTTP backend, and requests
/// sent over real sockets. Everything Traefik used to do that this module
/// must keep doing without it.
#[cfg(test)]
mod integration {
    use super::*;
    use http_body_util::Full;
    use hyper::client::conn::http1 as client_http1;
    use rustls::pki_types::ServerName;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    /// A self-signed cert for `localhost`, used as its own trust root: the
    /// point of these tests is the proxy, not the CA chain `tls.rs` already
    /// covers on its own.
    fn generate_test_cert(dir: &Path) -> rustls::pki_types::CertificateDer<'static> {
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        std::fs::write(dir.join("cert.pem"), certified.cert.pem()).unwrap();
        std::fs::write(dir.join("key.pem"), certified.key_pair.serialize_pem()).unwrap();
        certified.cert.der().clone()
    }

    fn client_config(
        trusted: rustls::pki_types::CertificateDer<'static>,
    ) -> Arc<rustls::ClientConfig> {
        client_config_for(trusted, b"http/1.1")
    }

    fn client_config_for(
        trusted: rustls::pki_types::CertificateDer<'static>,
        protocol: &[u8],
    ) -> Arc<rustls::ClientConfig> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(trusted).unwrap();
        let mut config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = vec![protocol.to_vec()];
        Arc::new(config)
    }

    /// One request/response over a fresh TLS connection, `Host` set from
    /// `host` regardless of what `addr` actually is — the same shape a real
    /// client presents when Consumer DNS and the proxy's bind address differ.
    async fn request(
        addr: SocketAddr,
        config: Arc<rustls::ClientConfig>,
        host: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (StatusCode, hyper::HeaderMap, Vec<u8>) {
        let tcp = TcpStream::connect(addr).await.unwrap();
        let connector = tokio_rustls::TlsConnector::from(config);
        let tls = connector
            .connect(ServerName::try_from("localhost").unwrap(), tcp)
            .await
            .unwrap();
        let (mut sender, conn) = client_http1::handshake(TokioIo::new(tls)).await.unwrap();
        tokio::spawn(conn.with_upgrades());

        let method = if body.is_empty() { "GET" } else { "POST" };
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, host);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let req = builder.body(Full::new(Bytes::from(body))).unwrap();
        let response = sender.send_request(req).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec();
        (status, headers, body)
    }

    async fn spawn_backend() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                tokio::spawn(async move {
                    let io = TokioIo::new(stream);
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, hyper::service::service_fn(backend_handle))
                        .with_upgrades()
                        .await;
                });
            }
        });
        addr
    }

    /// `/reflect` answers with the request headers it saw, one per line — so
    /// a test can check what the proxy forwarded and what it rewrote.
    /// `/echo` answers with the request body it received, unchanged. `/hang`
    /// upgrades and echoes whatever bytes arrive on the raw connection
    /// afterwards, standing in for a WebSocket backend.
    async fn backend_handle(mut req: Request<Incoming>) -> Result<Response<ProxyBody>, Infallible> {
        match req.uri().path() {
            "/reflect" => {
                let mut lines = String::new();
                lines.push_str(&format!("version={:?}\n", req.version()));
                for name in [
                    "host",
                    "x-custom",
                    "x-forwarded-for",
                    "x-forwarded-proto",
                    "x-forwarded-host",
                    "connection",
                ] {
                    if let Some(value) = req.headers().get(name) {
                        lines.push_str(&format!("{name}={}\n", value.to_str().unwrap_or("")));
                    }
                }
                Ok(Response::new(text_body(lines)))
            }
            "/echo" => {
                let bytes = req.into_body().collect().await.unwrap().to_bytes();
                Ok(Response::new(text_body(bytes)))
            }
            "/upgrade" => {
                let upgrade = hyper::upgrade::on(&mut req);
                tokio::spawn(async move {
                    if let Ok(upgraded) = upgrade.await {
                        let mut io = TokioIo::new(upgraded);
                        let mut buf = [0u8; 5];
                        if io.read_exact(&mut buf).await.is_ok() {
                            let _ = io.write_all(&buf).await;
                        }
                    }
                });
                Ok(Response::builder()
                    .status(StatusCode::SWITCHING_PROTOCOLS)
                    .header(header::CONNECTION, "upgrade")
                    .header(header::UPGRADE, "test-protocol")
                    .body(empty_body())
                    .unwrap())
            }
            _ => Ok(not_found()),
        }
    }

    static PORT_HINT: AtomicUsize = AtomicUsize::new(0);

    async fn spawn_proxy(
        admin_router: Router,
        table: RouteTable,
    ) -> (
        SocketAddr,
        SocketAddr,
        rustls::pki_types::CertificateDer<'static>,
    ) {
        let dir = std::env::temp_dir().join(format!(
            "self-host-proxy-test-{}-{}",
            std::process::id(),
            PORT_HINT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cert_der = generate_test_cert(&dir);
        std::fs::copy(dir.join("cert.pem"), dir.join("ca.pem")).unwrap();

        let bound = bind(
            ProxyConfig {
                cert_path: dir.join("cert.pem"),
                key_path: dir.join("key.pem"),
                bind_ip: "127.0.0.1".parse().unwrap(),
                https_port: 0,
                http_port: 0,
                admin_hostname: "admin.home.lan".into(),
            },
            admin_router,
            table,
        )
        .await
        .unwrap();
        let (https_addr, http_addr) = (bound.https_addr, bound.http_addr);
        tokio::spawn(bound.run());
        (https_addr, http_addr, cert_der)
    }

    #[tokio::test]
    async fn an_unknown_hostname_is_not_found() {
        let (https_addr, _, cert) = spawn_proxy(Router::new(), RouteTable::new()).await;
        let (status, _, _) = request(
            https_addr,
            client_config(cert),
            "nope.home.lan",
            "/",
            &[],
            vec![],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn admin_is_served_in_process_with_no_backend_at_all() {
        let admin = Router::new().route("/", axum::routing::get(|| async { "admin console" }));
        let (https_addr, _, cert) = spawn_proxy(admin, RouteTable::new()).await;
        let (status, _, body) = request(
            https_addr,
            client_config(cert),
            "admin.home.lan",
            "/",
            &[],
            vec![],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"admin console");
    }

    #[tokio::test]
    async fn a_known_application_with_no_target_is_unavailable() {
        let table = RouteTable::new();
        table.publish("abc", &["blog.home.lan".into()], None);
        let (https_addr, _, cert) = spawn_proxy(Router::new(), table).await;
        let (status, _, _) = request(
            https_addr,
            client_config(cert),
            "blog.home.lan",
            "/",
            &[],
            vec![],
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn a_known_application_proxies_and_headers_carry_a_trustworthy_identity() {
        let backend = spawn_backend().await;
        let table = RouteTable::new();
        table.publish("abc", &["blog.home.lan".into()], Some(backend));
        let (https_addr, _, cert) = spawn_proxy(Router::new(), table).await;

        let (status, _, body) = request(
            https_addr,
            client_config(cert),
            "blog.home.lan",
            "/reflect",
            &[
                ("x-custom", "kept"),
                // A client claiming to be someone else is not believed.
                ("x-forwarded-for", "1.2.3.4"),
                ("x-forwarded-proto", "http"),
            ],
            vec![],
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        let body = String::from_utf8(body).unwrap();
        assert!(body.contains("x-custom=kept"), "{body}");
        assert!(body.contains("x-forwarded-proto=https"), "{body}");
        assert!(!body.contains("1.2.3.4"), "{body}");
        assert!(body.contains("x-forwarded-for=127.0.0.1"), "{body}");
        assert!(body.contains("x-forwarded-host=blog.home.lan"), "{body}");
    }

    #[tokio::test]
    async fn a_client_that_asks_for_http2_gets_it_and_the_application_still_sees_http1() {
        let backend = spawn_backend().await;
        let table = RouteTable::new();
        table.publish("abc", &["blog.home.lan".into()], Some(backend));
        let (https_addr, _, cert) = spawn_proxy(Router::new(), table).await;

        let tcp = TcpStream::connect(https_addr).await.unwrap();
        let connector = tokio_rustls::TlsConnector::from(client_config_for(cert, b"h2"));
        let tls = connector
            .connect(ServerName::try_from("localhost").unwrap(), tcp)
            .await
            .unwrap();
        assert_eq!(
            tls.get_ref().1.alpn_protocol(),
            Some(&b"h2"[..]),
            "the proxy did not negotiate HTTP/2"
        );

        let (mut sender, conn) =
            hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls))
                .await
                .unwrap();
        tokio::spawn(conn);

        // HTTP/2 carries the Hostname as `:authority`, with no Host header.
        let req = Request::builder()
            .uri("https://blog.home.lan/reflect")
            .body(Full::new(Bytes::new()))
            .unwrap();
        let response = sender.send_request(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.version(), hyper::Version::HTTP_2);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(body.to_vec()).unwrap();
        // The Application is reached over HTTP/1.1 and told which Hostname
        // the Consumer asked for, not the loopback address behind it.
        assert!(body.contains("version=HTTP/1.1"), "{body}");
        assert!(body.contains("host=blog.home.lan"), "{body}");
        assert!(body.contains("x-forwarded-host=blog.home.lan"), "{body}");
    }

    #[tokio::test]
    async fn a_large_body_round_trips_unchanged() {
        let backend = spawn_backend().await;
        let table = RouteTable::new();
        table.publish("abc", &["blog.home.lan".into()], Some(backend));
        let (https_addr, _, cert) = spawn_proxy(Router::new(), table).await;

        let payload = vec![7u8; 4 * 1024 * 1024];
        let (status, _, body) = request(
            https_addr,
            client_config(cert),
            "blog.home.lan",
            "/echo",
            &[],
            payload.clone(),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, payload);
    }

    #[tokio::test]
    async fn setup_is_public_over_http_by_ip_without_exposing_the_console() {
        let admin = Router::new().route("/apps", axum::routing::get(|| async { "private" }));
        let (https_addr, http_addr, cert) = spawn_proxy(admin, RouteTable::new()).await;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .unwrap();
        let base = format!("http://{http_addr}");
        let response = client.get(format!("{base}/setup")).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(header::LOCATION).is_none());
        let page = response.text().await.unwrap();
        assert!(page.contains("<div id=\"root\"></div>"));

        // Every script and stylesheet needed for setup must also load over HTTP.
        for attribute in ["src=\"", "href=\""] {
            for path in page
                .split(attribute)
                .skip(1)
                .map(|part| part.split('"').next().unwrap())
            {
                if path.starts_with("/console/assets/") {
                    let response = client.get(format!("{base}{path}")).send().await.unwrap();
                    assert_eq!(response.status(), StatusCode::OK, "{path}");
                }
            }
        }

        let response = client
            .get(format!("{base}/setup/info"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let info: serde_json::Value = response.json().await.unwrap();
        assert_eq!(info["admin_hostname"], "admin.home.lan");
        assert_eq!(info["dns_suffix"], "home.lan");
        assert_eq!(info.as_object().unwrap().len(), 3);
        let ca = client
            .get(format!("{base}/setup/ca.pem"))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(
            info["fingerprint"],
            crate::tls::ca_sha256_fingerprint_from_pem(&ca).unwrap()
        );

        for path in ["/", "/console", "/apps"] {
            let response = client.get(format!("{base}{path}")).send().await.unwrap();
            assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY, "{path}");
            assert!(
                !response.headers()[header::LOCATION]
                    .to_str()
                    .unwrap()
                    .contains("/setup")
            );
        }
        for path in [
            "/setup/private",
            "/setup/ca-key.pem",
            "/setup/../setup/key.pem",
        ] {
            let response = client.get(format!("{base}{path}")).send().await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
        let response = client.post(format!("{base}/setup")).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);

        // Application hostnames retain their existing HTTP redirect behavior.
        let response = client
            .get(format!("{base}/setup"))
            .header(header::HOST, "blog.home.lan")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);

        let (status, _, _) = request(
            https_addr,
            client_config(cert),
            "admin.home.lan",
            "/setup",
            &[],
            vec![],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_plain_http_request_redirects_to_https() {
        let (_, http_addr, _) = spawn_proxy(Router::new(), RouteTable::new()).await;
        let tcp = TcpStream::connect(http_addr).await.unwrap();
        let (mut sender, conn) = client_http1::handshake(TokioIo::new(tcp)).await.unwrap();
        tokio::spawn(conn.with_upgrades());
        let req = Request::builder()
            .uri("/blog")
            .header(header::HOST, "blog.home.lan")
            .body(Full::new(Bytes::new()))
            .unwrap();
        let response = sender.send_request(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::MOVED_PERMANENTLY);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "https://blog.home.lan/blog"
        );
    }

    #[tokio::test]
    async fn an_upgraded_connection_carries_raw_bytes_both_ways() {
        let backend = spawn_backend().await;
        let table = RouteTable::new();
        table.publish("abc", &["blog.home.lan".into()], Some(backend));
        let (https_addr, _, cert) = spawn_proxy(Router::new(), table).await;

        let tcp = TcpStream::connect(https_addr).await.unwrap();
        let connector = tokio_rustls::TlsConnector::from(client_config(cert));
        let tls = connector
            .connect(ServerName::try_from("localhost").unwrap(), tcp)
            .await
            .unwrap();
        let (mut sender, conn) = client_http1::handshake(TokioIo::new(tls)).await.unwrap();
        tokio::spawn(conn.with_upgrades());

        let req = Request::builder()
            .uri("/upgrade")
            .header(header::HOST, "blog.home.lan")
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, "test-protocol")
            .body(Full::new(Bytes::new()))
            .unwrap();
        // The client learns whether an upgrade actually happened from the
        // response, not the request it sent.
        let mut response = sender.send_request(req).await.unwrap();
        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);

        let upgraded = hyper::upgrade::on(&mut response).await.unwrap();
        let mut io = TokioIo::new(upgraded);
        io.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        io.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
    }
}
