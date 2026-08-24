use crate::apps;
use crate::compose;
use crate::db::{DbError, StateStore};
use crate::docker::{APP_NETWORK, ContainerConfig, DockerError, DockerRuntime, SYSTEM_NETWORK};
use crate::tls;
use rand::Rng;
use std::io::Write;
use tracing::{info, warn};

const PG_IMAGE: &str = "postgres:18-alpine";
const PG_ROLE: &str = "db";
const COREDNS_IMAGE: &str = "coredns/coredns:1.11.1";
const COREDNS_ROLE: &str = "dns";
const TRAEFIK_IMAGE: &str = "traefik:v3";
const TRAEFIK_ROLE: &str = "proxy";

/// The Platform Infra the Platform starts for itself, as `(role, image)` pairs
/// in display order: the state store, the local DNS, then the traffic proxy.
/// The role is what names the component (`system_container_name`); the image is
/// the product that fills it today.
pub const SYSTEM_CONTAINERS: &[(&str, &str)] = &[
    (PG_ROLE, PG_IMAGE),
    (COREDNS_ROLE, COREDNS_IMAGE),
    (TRAEFIK_ROLE, TRAEFIK_IMAGE),
];

pub const DEFAULT_DNS_SUFFIX: &str = "home.lan";
pub const OPERATOR_API_PORT: u16 = 3721;
pub const PG_DB_URL: &str = "postgres://selfhost:selfhost@localhost:15432/selfhost";

#[derive(Debug)]
pub struct BootstrapResult {
    pub dns_suffix: String,
    pub api_key: String,
    pub api_listen_addr: String,
    pub host_ip: String,
}

pub fn validate_dns_suffix(suffix: &str) -> Result<(), String> {
    if suffix.is_empty() {
        return Err("DNS suffix must not be empty".into());
    }

    if suffix.ends_with(".local") || suffix == "local" {
        return Err(
            "DNS suffix '.local' is reserved for mDNS and will cause conflicts. \
             Use a different suffix (e.g. 'home.lan')"
                .into(),
        );
    }

    if !suffix.contains('.') {
        return Err("DNS suffix should contain a dot (e.g. 'home.lan'), \
             not a bare name like 'local'"
            .into());
    }

    Ok(())
}

pub async fn run_bootstrap(
    docker: &impl DockerRuntime,
    dns_suffix: &str,
) -> Result<BootstrapResult, BootstrapError> {
    validate_dns_suffix(dns_suffix).map_err(BootstrapError::InvalidDnsSuffix)?;

    docker.ping().await?;

    let api_key = generate_api_key();
    let host_ip = detect_host_ip();

    match tls::generate_certificates(dns_suffix) {
        Ok(()) => info!("TLS certificates generated"),
        Err(e) => {
            warn!("Failed to generate TLS certificates: {e}");
            warn!("HTTPS will not be available.");
        }
    }

    match tls::write_traefik_config() {
        Ok(()) => info!("Traefik configuration written"),
        Err(e) => warn!("Failed to write Traefik config: {e}"),
    }

    match tls::write_admin_route(dns_suffix) {
        Ok(()) => info!("admin route configured"),
        Err(e) => warn!("Failed to write admin route: {e}"),
    }

    start_infra_containers(docker, dns_suffix, &host_ip).await?;

    let api_listen_addr = format!("0.0.0.0:{OPERATOR_API_PORT}");

    info!("bootstrap complete: dns_suffix={dns_suffix}, api_listen={api_listen_addr}");

    Ok(BootstrapResult {
        dns_suffix: dns_suffix.to_string(),
        api_key,
        api_listen_addr,
        host_ip,
    })
}

pub async fn persist_bootstrap_state(
    store: &impl StateStore,
    result: &BootstrapResult,
) -> Result<(), BootstrapError> {
    store.initialize_schema().await?;

    if store.is_initialized().await? {
        return Err(BootstrapError::AlreadyInitialized);
    }

    store.store_state("api_key", &result.api_key).await?;
    store.store_state("dns_suffix", &result.dns_suffix).await?;
    store.store_state("host_ip", &result.host_ip).await?;

    Ok(())
}

async fn start_infra_containers(
    docker: &impl DockerRuntime,
    dns_suffix: &str,
    host_ip: &str,
) -> Result<(), BootstrapError> {
    docker.ensure_network(SYSTEM_NETWORK).await?;
    docker.ensure_network(APP_NETWORK).await?;

    let coredns_config = generate_coredns_config(dns_suffix, host_ip);
    write_coredns_config(&coredns_config)?;

    for container in infra_containers() {
        info!("configuring {} container", container.name);
        docker.ensure_container_running(container).await?;
    }

    info!("applying docker compose configuration");
    docker.commit().await?;

    info!("infra containers started successfully");
    Ok(())
}

/// The Platform Infra containers, as configuration. Applications reach Traefik
/// and nothing else: only the proxy is on both networks.
fn infra_containers() -> Vec<ContainerConfig> {
    vec![
        ContainerConfig {
            image: PG_IMAGE.to_string(),
            name: apps::system_container_name(PG_ROLE),
            ports: vec!["15432:5432".into()],
            env: vec![
                "POSTGRES_USER=selfhost".into(),
                "POSTGRES_PASSWORD=selfhost".into(),
                "POSTGRES_DB=selfhost".into(),
            ],
            volumes: vec!["self-host-pg-data:/var/lib/postgresql".into()],
            restart_policy: "unless-stopped".into(),
            cmd: vec![],
            labels: vec![],
            networks: vec![SYSTEM_NETWORK.to_string()],
        },
        ContainerConfig {
            image: COREDNS_IMAGE.to_string(),
            name: apps::system_container_name(COREDNS_ROLE),
            ports: vec!["53:53/tcp".into(), "53:53/udp".into()],
            env: vec![],
            volumes: vec![format!(
                "{}:/etc/coredns/Corefile:ro",
                coredns_config_path().display()
            )],
            restart_policy: "unless-stopped".into(),
            cmd: vec!["-conf".into(), "/etc/coredns/Corefile".into()],
            labels: vec![],
            networks: vec![SYSTEM_NETWORK.to_string()],
        },
        ContainerConfig {
            image: TRAEFIK_IMAGE.to_string(),
            name: apps::system_container_name(TRAEFIK_ROLE),
            ports: vec!["80:80".into(), "443:443".into()],
            env: vec![],
            volumes: vec![
                "/var/run/docker.sock:/var/run/docker.sock:ro".into(),
                format!("{}:/certs:ro", tls::certs_dir().display()),
                format!(
                    "{}:/etc/traefik/traefik.yml:ro",
                    tls::traefik_config_path().display()
                ),
                format!(
                    "{}:/etc/traefik/dynamic:ro",
                    tls::traefik_dynamic_dir().display()
                ),
            ],
            restart_policy: "unless-stopped".into(),
            cmd: tls::traefik_args(),
            labels: vec![],
            networks: vec![SYSTEM_NETWORK.to_string(), APP_NETWORK.to_string()],
        },
    ]
}

fn generate_api_key() -> String {
    let mut rng = rand::rng();
    let bytes: [u8; 32] = rng.random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn detect_host_ip() -> String {
    "127.0.0.1".to_string()
}

fn generate_coredns_config(dns_suffix: &str, host_ip: &str) -> String {
    let escaped_suffix = dns_suffix.replace('.', "\\.");
    format!(
        r#"{} {{
    template IN A {{
        match .*\.{}\.$
        answer "{{{{ .Name }}}} 60 IN A {}"
        fallthrough
    }}
    forward . 8.8.8.8 8.8.4.4
}}"#,
        dns_suffix, escaped_suffix, host_ip
    )
}

fn coredns_config_path() -> std::path::PathBuf {
    compose::platform_config_dir().join("Corefile")
}

fn write_coredns_config(config: &str) -> Result<(), BootstrapError> {
    let path = coredns_config_path();

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| BootstrapError::ConfigWrite(format!("create config directory: {e}")))?;
    }

    let mut file = std::fs::File::create(&path)
        .map_err(|e| BootstrapError::ConfigWrite(format!("create Corefile: {e}")))?;
    file.write_all(config.as_bytes())
        .map_err(|e| BootstrapError::ConfigWrite(format!("write Corefile: {e}")))?;
    Ok(())
}

fn configure_macos_resolver(dns_suffix: &str, host_ip: &str) -> Result<(), String> {
    let resolver_dir = std::path::Path::new("/etc/resolver");
    if !resolver_dir.exists() {
        return Err("/etc/resolver directory does not exist".into());
    }

    let resolver_file = resolver_dir.join(dns_suffix);
    let content = format!("nameserver {host_ip}\n");

    std::fs::write(&resolver_file, content)
        .map_err(|e| format!("failed to write resolver file: {e}"))?;

    Ok(())
}

pub fn print_bootstrap_instructions(result: &BootstrapResult) {
    println!();
    println!("=== Bootstrap complete ===");
    println!();
    println!("DNS Suffix: {}", result.dns_suffix);
    println!();

    // Configure macOS resolver for the DNS suffix
    if cfg!(target_os = "macos") {
        match configure_macos_resolver(&result.dns_suffix, &result.host_ip) {
            Ok(()) => {
                println!("macOS resolver configured for .{}", result.dns_suffix);
            }
            Err(e) => {
                println!("Could not configure macOS resolver: {e}");
                println!("Run manually:");
                println!("  sudo mkdir -p /etc/resolver");
                println!(
                    "  echo 'nameserver {}' | sudo tee /etc/resolver/{}",
                    result.host_ip, result.dns_suffix
                );
            }
        }
        println!();
    }

    println!("--- Consumer DNS Setup ---");
    println!("Point your LAN devices (or router) to use this Host as DNS server:");
    println!("  DNS server: {} (port 53)", result.host_ip);
    println!();
    println!("Applications will be reachable at:");
    println!("  https://<name>.{}", result.dns_suffix);
    println!();
    println!("--- Admin Dashboard ---");
    println!("  https://admin.{}", result.dns_suffix);
    println!();

    let ca_path = tls::ca_cert_path();
    if ca_path.exists() {
        println!("--- HTTPS Setup ---");
        println!("To trust HTTPS certificates on other devices, install the CA certificate:");
        println!("  CA certificate: {}", ca_path.display());
        println!();
        println!(
            "  macOS:   security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain {}",
            ca_path.display()
        );
        println!(
            "  Windows: certutil -addstore -f \"ROOT\" {}",
            ca_path.display()
        );
        println!(
            "  Linux:   sudo cp {} /usr/local/share/ca-certificates/self-host-ca.crt && sudo update-ca-certificates",
            ca_path.display()
        );
        println!();
    }

    println!("--- Operator API ---");
    println!("API listen address: http://{}", result.api_listen_addr);
    println!("API key: {}", result.api_key);
    println!();
    println!("Consumer traffic enters on port 443 (HTTPS) and 80 (redirects to HTTPS).");
    println!("Operator API is on port {OPERATOR_API_PORT}.");
    println!();
    println!("Start the daemon with: self-host serve");
}

#[derive(Debug)]
pub enum BootstrapError {
    Docker(DockerError),
    Db(DbError),
    InvalidDnsSuffix(String),
    AlreadyInitialized,
    ConfigWrite(String),
}

impl From<DockerError> for BootstrapError {
    fn from(e: DockerError) -> Self {
        BootstrapError::Docker(e)
    }
}

impl From<DbError> for BootstrapError {
    fn from(e: DbError) -> Self {
        BootstrapError::Db(e)
    }
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BootstrapError::Docker(e) => write!(f, "{e}"),
            BootstrapError::Db(e) => write!(f, "{e}"),
            BootstrapError::InvalidDnsSuffix(msg) => write!(f, "invalid DNS suffix: {msg}"),
            BootstrapError::AlreadyInitialized => {
                write!(
                    f,
                    "already initialized. Use 'self-host serve' to start the daemon"
                )
            }
            BootstrapError::ConfigWrite(msg) => write!(f, "failed to write CoreDNS config: {msg}"),
        }
    }
}

impl std::error::Error for BootstrapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BootstrapError::Docker(e) => Some(e),
            BootstrapError::Db(e) => Some(e),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::SYSTEM_PREFIX;

    #[test]
    fn every_infra_container_is_named_as_platform_infra() {
        for container in infra_containers() {
            assert!(
                container.name.starts_with(SYSTEM_PREFIX),
                "{} does not say it belongs to the Platform",
                container.name
            );
        }
    }

    #[test]
    fn system_containers_names_every_infra_container() {
        let infra: std::collections::HashSet<String> =
            infra_containers().into_iter().map(|c| c.name).collect();

        for (role, _) in SYSTEM_CONTAINERS {
            assert!(
                infra.contains(&apps::system_container_name(role)),
                "role '{role}' is listed in SYSTEM_CONTAINERS but has no infra container"
            );
        }
    }

    #[test]
    fn only_the_proxy_reaches_across_to_the_applications() {
        for container in infra_containers() {
            let on_app_network = container.networks.iter().any(|n| n == APP_NETWORK);

            // An Application shares a bridge with the proxy that serves it and
            // with nothing else: the state store holds every Operator secret
            // behind a fixed password.
            if container.name == apps::system_container_name(TRAEFIK_ROLE) {
                assert!(on_app_network, "the proxy cannot reach Applications");
            } else {
                assert!(
                    !on_app_network,
                    "{} is exposed to Applications",
                    container.name
                );
            }
        }
    }

    #[test]
    fn generate_coredns_config_creates_valid_template() {
        let config = generate_coredns_config("home.lan", "192.168.1.100");

        assert!(config.contains("home.lan {"));
        assert!(config.contains("template IN A {"));
        assert!(config.contains("match .*\\.home\\.lan\\.$"));
        assert!(config.contains("answer \"{{ .Name }} 60 IN A 192.168.1.100\""));
        assert!(config.contains("forward . 8.8.8.8 8.8.4.4"));
    }

    #[test]
    fn generate_coredns_config_handles_custom_suffix() {
        let config = generate_coredns_config("custom.local", "10.0.0.1");

        assert!(config.contains("custom.local {"));
        assert!(config.contains("template IN A {"));
        assert!(config.contains("match .*\\.custom\\.local\\.$"));
        assert!(config.contains("answer \"{{ .Name }} 60 IN A 10.0.0.1\""));
    }
}
