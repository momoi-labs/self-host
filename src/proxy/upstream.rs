use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use hyper::body::Incoming;
use hyper::{Request, Response, Uri};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::{Connected, Connection, HttpConnector};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;

use super::{BoxError, ProxyBody};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PIN_MISMATCH: &str = "upstream certificate does not match the pinned fingerprint";

pub type Fingerprint = [u8; 32];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scheme {
    Http,
    Https,
}

impl Scheme {
    fn as_str(self) -> &'static str {
        match self {
            Scheme::Http => "http",
            Scheme::Https => "https",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Upstream {
    pub scheme: Scheme,
    pub address: SocketAddr,
    pub pin: Option<Fingerprint>,
}

impl Upstream {
    pub fn http(address: SocketAddr) -> Self {
        Upstream {
            scheme: Scheme::Http,
            address,
            pin: None,
        }
    }

    pub fn host_header(&self) -> String {
        match (self.scheme, self.address) {
            (Scheme::Https, address) if address.port() == 443 => match address.ip() {
                IpAddr::V4(ip) => ip.to_string(),
                IpAddr::V6(ip) => format!("[{ip}]"),
            },
            (_, address) => address.to_string(),
        }
    }

    pub fn uri(&self, path_and_query: &str) -> String {
        format!(
            "{}://{}{path_and_query}",
            self.scheme.as_str(),
            self.address
        )
    }
}

impl std::fmt::Display for Upstream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://{}", self.scheme.as_str(), self.address)
    }
}

impl From<SocketAddr> for Upstream {
    fn from(address: SocketAddr) -> Self {
        Upstream::http(address)
    }
}

pub fn format_fingerprint(fingerprint: &Fingerprint) -> String {
    fingerprint
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

pub fn parse_fingerprint(text: &str) -> Option<Fingerprint> {
    let hex: String = text.trim().chars().filter(|c| *c != ':').collect();
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut fingerprint = [0u8; 32];
    for (index, byte) in fingerprint.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(fingerprint)
}

pub fn fingerprint_of(certificate: &[u8]) -> Fingerprint {
    Sha256::digest(certificate).into()
}

type TlsClient = Client<TlsConnector, ProxyBody>;

pub(super) struct Clients {
    plain: Client<HttpConnector, ProxyBody>,
    tls: Mutex<HashMap<Option<Fingerprint>, TlsClient>>,
}

impl Clients {
    pub(super) fn new() -> Self {
        let mut connector = HttpConnector::new();
        connector.set_connect_timeout(Some(CONNECT_TIMEOUT));
        Clients {
            plain: Client::builder(TokioExecutor::new()).build(connector),
            tls: Mutex::new(HashMap::new()),
        }
    }

    pub(super) async fn request(
        &self,
        upstream: &Upstream,
        request: Request<ProxyBody>,
    ) -> Result<Response<Incoming>, hyper_util::client::legacy::Error> {
        match upstream.scheme {
            Scheme::Http => self.plain.request(request).await,
            Scheme::Https => {
                let client = self
                    .tls
                    .lock()
                    .unwrap()
                    .entry(upstream.pin)
                    .or_insert_with(|| tls_client(upstream.pin))
                    .clone();
                client.request(request).await
            }
        }
    }
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn native_roots() -> Arc<rustls::RootCertStore> {
    static ROOTS: OnceLock<Arc<rustls::RootCertStore>> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            let found = rustls_native_certs::load_native_certs();
            for error in &found.errors {
                tracing::warn!("could not load a system root certificate: {error}");
            }
            roots.add_parsable_certificates(found.certs);
            Arc::new(roots)
        })
        .clone()
}

pub(super) fn client_config(pin: Option<Fingerprint>) -> rustls::ClientConfig {
    let provider = provider();
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .expect("the ring provider supports the default protocol versions");
    let mut config = match pin {
        Some(pin) => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PinnedCertificate {
                pin,
                algorithms: provider.signature_verification_algorithms,
            }))
            .with_no_client_auth(),
        None => builder
            .with_root_certificates(native_roots())
            .with_no_client_auth(),
    };
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    config
}

fn tls_client(pin: Option<Fingerprint>) -> TlsClient {
    let connector = TlsConnector {
        tls: tokio_rustls::TlsConnector::from(Arc::new(client_config(pin))),
    };
    Client::builder(TokioExecutor::new()).build(connector)
}

#[derive(Debug)]
struct PinnedCertificate {
    pin: Fingerprint,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinnedCertificate {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let presented = fingerprint_of(end_entity.as_ref());
        if presented == self.pin {
            return Ok(ServerCertVerified::assertion());
        }
        tracing::warn!(
            expected = %format_fingerprint(&self.pin),
            presented = %format_fingerprint(&presented),
            "{PIN_MISMATCH}"
        );
        Err(rustls::Error::General(PIN_MISMATCH.into()))
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

#[derive(Debug)]
struct CapturedCertificate {
    seen: Mutex<Option<CertificateDer<'static>>>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for CapturedCertificate {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        *self.seen.lock().unwrap() = Some(end_entity.clone().into_owned());
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

pub async fn presented_certificate(
    address: SocketAddr,
    timeout: Duration,
) -> Result<CertificateDer<'static>, String> {
    let provider = provider();
    let verifier = Arc::new(CapturedCertificate {
        seen: Mutex::new(None),
        algorithms: provider.signature_verification_algorithms,
    });
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("the ring provider supports the default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let handshake = async {
        let tcp = TcpStream::connect(address).await?;
        connector
            .connect(ServerName::IpAddress(address.ip().into()), tcp)
            .await
    };
    match tokio::time::timeout(timeout, handshake).await {
        Err(_) => Err(format!(
            "{address} did not finish a TLS handshake within {} seconds",
            timeout.as_secs()
        )),
        Ok(Err(error)) => Err(format!("{address}: {}", chain(&error))),
        Ok(Ok(_)) => verifier
            .seen
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| format!("{address} presented no certificate")),
    }
}

pub async fn probe(upstream: &Upstream, host: &str, timeout: Duration) -> Result<u16, String> {
    let request = Request::get(upstream.uri("/"))
        .header(hyper::header::HOST, host)
        .body(super::empty_body())
        .map_err(|error| error.to_string())?;
    match tokio::time::timeout(timeout, Clients::new().request(upstream, request)).await {
        Err(_) => Err(format!(
            "{upstream} did not answer within {} seconds",
            timeout.as_secs()
        )),
        Ok(Err(error)) => Err(chain(&error)),
        Ok(Ok(response)) => Ok(response.status().as_u16()),
    }
}

#[derive(Clone)]
pub(super) struct TlsConnector {
    tls: tokio_rustls::TlsConnector,
}

impl tower::Service<Uri> for TlsConnector {
    type Response = TlsConnection;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<TlsConnection, BoxError>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let tls = self.tls.clone();
        Box::pin(async move {
            let host = uri.host().ok_or("upstream URI has no host")?;
            let ip: IpAddr = host.trim_start_matches('[').trim_end_matches(']').parse()?;
            let address = SocketAddr::new(ip, uri.port_u16().unwrap_or(443));
            let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(address))
                .await
                .map_err(|_| "upstream connect timed out")??;
            tcp.set_nodelay(true)?;
            let stream = tls.connect(ServerName::IpAddress(ip.into()), tcp).await?;
            Ok(TlsConnection(TokioIo::new(stream)))
        })
    }
}

pub(super) struct TlsConnection(TokioIo<tokio_rustls::client::TlsStream<TcpStream>>);

impl Connection for TlsConnection {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

impl hyper::rt::Read for TlsConnection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl hyper::rt::Write for TlsConnection {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

pub(super) fn failure(error: &(dyn std::error::Error + 'static)) -> &'static str {
    let mut cursor = Some(error);
    while let Some(current) = cursor {
        if let Some(tls) = current.downcast_ref::<rustls::Error>() {
            return match tls {
                rustls::Error::General(message) if message == PIN_MISMATCH => {
                    "the upstream certificate does not match the pinned fingerprint"
                }
                rustls::Error::InvalidCertificate(_) => {
                    "the upstream certificate is not trusted; pin its fingerprint"
                }
                _ => "the TLS handshake with the upstream failed",
            };
        }
        cursor = match current.downcast_ref::<std::io::Error>() {
            Some(io) => io
                .get_ref()
                .map(|inner| inner as &(dyn std::error::Error + 'static)),
            None => current.source(),
        };
    }
    "upstream did not answer"
}

pub(super) fn chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![error.to_string()];
    let mut cursor = Some(error);
    while let Some(current) = cursor {
        cursor = match current.downcast_ref::<std::io::Error>() {
            Some(io) => io
                .get_ref()
                .map(|inner| inner as &(dyn std::error::Error + 'static)),
            None => current.source(),
        };
        if let Some(next) = cursor {
            let text = next.to_string();
            if parts.last() != Some(&text) {
                parts.push(text);
            }
        }
    }
    parts.join(": ")
}
