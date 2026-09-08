//! Exercise both CLI entry points against HTTPS without changing system trust.

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair, KeyUsagePurpose};
use std::convert::Infallible;
use std::path::PathBuf;
use std::process::Output;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

struct Host {
    home: PathBuf,
    ca: String,
    server: tokio::task::JoinHandle<()>,
}

impl Host {
    async fn start(cert_name: &str) -> Self {
        let home = std::env::temp_dir().join(format!(
            "self-host-cli-tls-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(home.join(".config/self-host/certs")).unwrap();
        std::fs::create_dir(home.join("empty-roots")).unwrap();
        std::fs::write(home.join("native.pem"), "").unwrap();

        let ca_key = KeyPair::generate().unwrap();
        let mut ca_params = CertificateParams::default();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let key = KeyPair::generate().unwrap();
        let cert = CertificateParams::new(vec![cert_name.into()])
            .unwrap()
            .signed_by(&key, &ca, &ca_key)
            .unwrap();
        let _ = rustls::crypto::ring::default_provider().install_default();
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.der().clone()],
                rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
            )
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = serde_json::json!({
            "api_base_url": format!("https://{}", listener.local_addr().unwrap()),
            "api_key": "cli-test-key",
        });
        std::fs::write(
            home.join(".config/self-host/config.json"),
            config.to_string(),
        )
        .unwrap();
        let server = tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(stream) = acceptor.accept(socket).await else {
                        return;
                    };
                    let service = service_fn(
                        |request: hyper::Request<hyper::body::Incoming>| async move {
                            assert_eq!(request.headers()["authorization"], "Bearer cli-test-key");
                            let body = match request.uri().path() {
                                "/apps" => {
                                    r#"[{"name":"hermes","hostname":"hermes.home.lan","source":"compose","status":"running"}]"#
                                }
                                "/apps/hermes/logs" => "data: hermes is ready\n\n",
                                path => panic!("unexpected CLI request: {path}"),
                            };
                            Ok::<_, Infallible>(hyper::Response::new(Full::new(
                                Bytes::from_static(body.as_bytes()),
                            )))
                        },
                    );
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Self {
            home,
            ca: ca.pem(),
            server,
        }
    }

    fn ca_path(&self) -> PathBuf {
        self.home.join(".config/self-host/certs/ca.pem")
    }

    async fn run(&self, args: &[&str]) -> Output {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_self-host"));
        command
            .args(args)
            .env("HOME", &self.home)
            .env("SSL_CERT_FILE", self.home.join("native.pem"))
            .env("SSL_CERT_DIR", self.home.join("empty-roots"))
            .env("NO_PROXY", "*")
            .env("no_proxy", "*")
            .env("RUST_BACKTRACE", "0")
            .kill_on_drop(true);
        tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
            .await
            .expect("CLI request timed out")
            .expect("run the CLI")
    }

    async fn assert_commands_succeed(&self) {
        for (args, expected) in [
            (["apps", "list"], "running"),
            (["logs", "hermes"], "hermes is ready"),
        ] {
            let output = self.run(&args).await;
            assert!(
                output.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains(expected));
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

#[tokio::test]
async fn cli_trusts_the_local_ca_without_system_trust() {
    let host = Host::start("127.0.0.1").await;
    std::fs::write(host.ca_path(), &host.ca).unwrap();
    host.assert_commands_succeed().await;
}

#[tokio::test]
async fn cli_preserves_native_trust_when_the_local_ca_is_absent() {
    let host = Host::start("127.0.0.1").await;
    std::fs::write(host.home.join("native.pem"), &host.ca).unwrap();
    host.assert_commands_succeed().await;
}

#[tokio::test]
async fn cli_rejects_an_unknown_ca_and_a_wrong_hostname() {
    for cert_name in ["127.0.0.1", "wrong.home.lan"] {
        let host = Host::start(cert_name).await;
        let local_ca = if cert_name == "127.0.0.1" {
            rcgen::generate_simple_self_signed(vec!["other.home.lan".into()])
                .unwrap()
                .cert
                .pem()
        } else {
            host.ca.clone()
        };
        std::fs::write(host.ca_path(), local_ca).unwrap();
        for args in [["apps", "list"], ["logs", "hermes"]] {
            let output = host.run(&args).await;
            assert!(!output.status.success(), "{args:?} accepted {cert_name}");
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("invalid peer certificate"),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
