use crate::compose;
use crate::db::{DbError, StateStore};
use crate::docker::{ContainerConfig, DockerError, DockerRuntime, PLATFORM_NETWORK};
use rand::Rng;
use std::io::Write;
use tracing::info;

const PG_IMAGE: &str = "postgres:18-alpine";
const PG_CONTAINER: &str = "self-host-pg";
const COREDNS_IMAGE: &str = "coredns/coredns:1.11.1";
const COREDNS_CONTAINER: &str = "self-host-coredns";
const TRAEFIK_IMAGE: &str = "traefik:v3";
const TRAEFIK_CONTAINER: &str = "self-host-traefik";
pub const DEFAULT_DNS_SUFFIX: &str = "home.lan";
pub const OPERATOR_API_PORT: u16 = 3721;
pub const PG_DB_URL: &str = "postgres://selfhost:selfhost@localhost:5432/selfhost";

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

    start_infra_containers(docker, dns_suffix, &host_ip).await?;

    let api_listen_addr = format!("{host_ip}:{OPERATOR_API_PORT}");

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
    docker.ensure_network(PLATFORM_NETWORK).await?;

    info!("configuring PostgreSQL 18 container");
    docker
        .ensure_container_running(ContainerConfig {
            image: PG_IMAGE.to_string(),
            name: PG_CONTAINER.to_string(),
            ports: vec!["5432:5432".into()],
            env: vec![
                "POSTGRES_USER=selfhost".into(),
                "POSTGRES_PASSWORD=selfhost".into(),
                "POSTGRES_DB=selfhost".into(),
            ],
            volumes: vec!["self-host-pg-data:/var/lib/postgresql".into()],
            restart_policy: "unless-stopped".into(),
            cmd: vec![],
            labels: vec![],
            networks: vec![PLATFORM_NETWORK.to_string()],
        })
        .await?;

    info!("configuring CoreDNS container");
    let coredns_config = generate_coredns_config(dns_suffix, host_ip);
    write_coredns_config(&coredns_config)?;
    docker
        .ensure_container_running(ContainerConfig {
            image: COREDNS_IMAGE.to_string(),
            name: COREDNS_CONTAINER.to_string(),
            ports: vec!["53:53/tcp".into(), "53:53/udp".into()],
            env: vec![],
            volumes: vec![format!(
                "{}:/etc/coredns/Corefile:ro",
                coredns_config_path().display()
            )],
            restart_policy: "unless-stopped".into(),
            cmd: vec!["-conf".into(), "/etc/coredns/Corefile".into()],
            labels: vec![],
            networks: vec![PLATFORM_NETWORK.to_string()],
        })
        .await?;

    info!("configuring Traefik container");
    docker
        .ensure_container_running(ContainerConfig {
            image: TRAEFIK_IMAGE.to_string(),
            name: TRAEFIK_CONTAINER.to_string(),
            ports: vec!["80:80".into()],
            env: vec![],
            volumes: vec!["/var/run/docker.sock:/var/run/docker.sock:ro".into()],
            restart_policy: "unless-stopped".into(),
            cmd: vec![
                "--providers.docker=true".into(),
                "--providers.docker.exposedbydefault=false".into(),
                "--entrypoints.web.address=:80".into(),
            ],
            labels: vec![],
            networks: vec![PLATFORM_NETWORK.to_string()],
        })
        .await?;

    info!("applying docker compose configuration");
    docker.commit().await?;

    info!("infra containers started successfully");
    Ok(())
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

pub fn print_bootstrap_instructions(result: &BootstrapResult) {
    println!();
    println!("=== Bootstrap complete ===");
    println!();
    println!("DNS Suffix: {}", result.dns_suffix);
    println!();
    println!("--- Consumer DNS Setup ---");
    println!("Point your LAN devices (or router) to use this Host as DNS server:");
    println!("  DNS server: {} (port 53)", result.host_ip);
    println!();
    println!("Applications will be reachable at:");
    println!("  http://<name>.{}", result.dns_suffix);
    println!();
    println!("--- Operator API ---");
    println!("API listen address: http://{}", result.api_listen_addr);
    println!("API key: {}", result.api_key);
    println!();
    println!("Consumer traffic enters on port 80 (Traefik).");
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
