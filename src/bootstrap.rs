use crate::apps;
use crate::compose;
use crate::db::{DbError, StateStore};
use crate::docker::{APP_NETWORK, ContainerConfig, DockerError, DockerRuntime, SYSTEM_NETWORK};
use crate::tls;
use rand::Rng;
use std::net::UdpSocket;
use tracing::info;

/// Also the image the schema tests run against, so a migration is never
/// proven on a Postgres the Platform does not actually ship.
pub const PG_IMAGE: &str = "postgres:18-alpine";
const PG_ROLE: &str = "db";
const TRAEFIK_IMAGE: &str = "traefik:v3";
const TRAEFIK_ROLE: &str = "proxy";

/// The Platform Infra the Platform starts for itself, as `(role, image)` pairs
/// in display order: the state store, then the traffic proxy.
/// The role is what names the component (`system_container_name`); the image is
/// the product that fills it today.
pub const SYSTEM_CONTAINERS: &[(&str, &str)] =
    &[(PG_ROLE, PG_IMAGE), (TRAEFIK_ROLE, TRAEFIK_IMAGE)];

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
    host_ip: Option<&str>,
) -> Result<BootstrapResult, BootstrapError> {
    validate_dns_suffix(dns_suffix).map_err(BootstrapError::InvalidDnsSuffix)?;

    docker.ping().await?;

    let api_key = generate_api_key();
    // Detection picks the interface the default route leaves through, which
    // is the LAN one. `--host-ip` is for the Host where that guess is wrong:
    // a Mac on a VPN, or one with a second interface.
    let host_ip = match host_ip {
        Some(ip) => ip
            .parse::<std::net::IpAddr>()
            .map_err(|_| BootstrapError::InvalidHostIp(ip.to_string()))?
            .to_string(),
        None => detect_host_ip()?,
    };

    let dns_config = crate::dns::Config::new(dns_suffix, &host_ip)
        .map_err(|e| BootstrapError::ConfigWrite(e.to_string()))?;

    tls::generate_certificates(dns_suffix)?;
    info!("TLS certificates ready");

    tls::write_traefik_config()?;
    info!("Traefik configuration written");

    tls::write_admin_route(dns_suffix)?;
    info!("admin route configured");

    dns_config
        .save()
        .map_err(|e| BootstrapError::ConfigWrite(e.to_string()))?;
    start_infra_containers(docker).await?;

    let api_listen_addr = format!("0.0.0.0:{OPERATOR_API_PORT}");

    info!("bootstrap complete: dns_suffix={dns_suffix}, api_listen={api_listen_addr}");

    Ok(BootstrapResult {
        dns_suffix: dns_suffix.to_string(),
        api_key,
        api_listen_addr,
        host_ip,
    })
}

/// Checks the authoritative state and all generated projections before
/// Bootstrap is allowed to write anything.
pub async fn preflight_initialized(
    store: &impl StateStore,
    requested_dns_suffix: &str,
) -> Result<bool, BootstrapError> {
    let Some(configured_dns_suffix) = store.get_state("dns_suffix").await? else {
        return Ok(false);
    };

    if configured_dns_suffix != requested_dns_suffix {
        return Err(BootstrapError::DnsSuffixMismatch {
            requested: requested_dns_suffix.to_string(),
            configured: configured_dns_suffix,
        });
    }

    validate_configuration(requested_dns_suffix)?;
    Ok(true)
}

pub fn platform_configuration_exists() -> bool {
    compose::platform_config_dir()
        .join("docker-compose.yml")
        .exists()
        || crate::dns::config_path().exists()
        || tls::certs_dir().exists()
}

fn validate_configuration(dns_suffix: &str) -> Result<(), BootstrapError> {
    tls::validate_certificates(dns_suffix)?;

    let dns = crate::dns::Config::load().map_err(|e| BootstrapError::ConfigRead(e.to_string()))?;
    if dns.dns_suffix != dns_suffix {
        return Err(BootstrapError::ConfigurationMismatch(format!(
            "DNS does not serve '{dns_suffix}'"
        )));
    }

    let admin_route = std::fs::read_to_string(tls::traefik_dynamic_dir().join("admin.yml"))
        .map_err(|e| BootstrapError::ConfigRead(format!("admin route: {e}")))?;
    if !admin_route.contains(&format!("Host(`admin.{dns_suffix}`)")) {
        return Err(BootstrapError::ConfigurationMismatch(format!(
            "the console route does not use '{dns_suffix}'"
        )));
    }
    Ok(())
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

async fn start_infra_containers(docker: &impl DockerRuntime) -> Result<(), BootstrapError> {
    docker.ensure_network(SYSTEM_NETWORK).await?;
    docker.ensure_network(APP_NETWORK).await?;

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
            // Loopback only. The state store holds every Operator secret
            // behind credentials fixed at `selfhost:selfhost` (ADR-0012), and
            // the only client is the Platform binary on the Host.
            ports: vec!["127.0.0.1:15432:5432".into()],
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
            extra_hosts: vec![],
        },
        ContainerConfig {
            image: TRAEFIK_IMAGE.to_string(),
            name: apps::system_container_name(TRAEFIK_ROLE),
            ports: vec!["80:80".into(), "443:443".into()],
            env: vec![],
            volumes: vec![
                "/var/run/docker.sock:/var/run/docker.sock:ro".into(),
                format!("{}:/certs/cert.pem:ro", tls::cert_path().display()),
                format!("{}:/certs/key.pem:ro", tls::key_path().display()),
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
            extra_hosts: vec!["host.docker.internal:host-gateway".into()],
        },
    ]
}

fn generate_api_key() -> String {
    let mut rng = rand::rng();
    let bytes: [u8; 32] = rng.random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn detect_host_ip() -> Result<String, BootstrapError> {
    let socket =
        UdpSocket::bind("0.0.0.0:0").map_err(|e| BootstrapError::HostIpDetection(e.to_string()))?;
    socket
        .connect("1.1.1.1:80")
        .map_err(|e| BootstrapError::HostIpDetection(e.to_string()))?;
    let address = socket
        .local_addr()
        .map_err(|e| BootstrapError::HostIpDetection(e.to_string()))?
        .ip();
    if address.is_loopback() || address.is_unspecified() {
        return Err(BootstrapError::HostIpDetection(
            "no LAN address was found; connect the Host to the LAN and retry".into(),
        ));
    }
    Ok(address.to_string())
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
    println!("  Start 'self-host serve' before configuring or checking DNS.");
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
        if let Ok(fingerprint) = tls::ca_sha256_fingerprint() {
            println!("  SHA-256 fingerprint: {fingerprint}");
            println!();
            println!("  Set up another Linux/macOS machine:");
            println!(
                "  curl -fsSL https://raw.githubusercontent.com/momoi-labs/self-host/main/install.sh | bash && self-host trust-ca --from admin.{} --fingerprint '{}'",
                result.dns_suffix, fingerprint
            );
        }
        println!();
        println!(
            "  macOS:   sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain {}",
            ca_path.display()
        );
        println!(
            "  Windows: certutil -addstore -f \"ROOT\" {}",
            ca_path.display()
        );
        println!(
            "  Debian/Ubuntu: sudo cp {} /usr/local/share/ca-certificates/self-host-ca.crt && sudo update-ca-certificates",
            ca_path.display()
        );
        println!(
            "  Arch/p11-kit: sudo trust anchor --store {}",
            ca_path.display()
        );
        println!("  This Host: self-host trust-ca");
        println!("  Transfer only ca.pem to Consumers. Never transfer a private key.");
        println!();
    }

    println!("--- Host DNS Setup ---");
    if cfg!(target_os = "linux") {
        println!("  self-host setup-dns");
        println!("  Configures persistent Host DNS on Linux with systemd-resolved.");
        println!("  If UFW is active, allow Traefik's two networks to reach the Operator API:");
        println!(
            "  sudo ufw allow from \"$(docker network inspect {SYSTEM_NETWORK} --format '{{{{(index .IPAM.Config 0).Subnet}}}}')\" to any port {OPERATOR_API_PORT} proto tcp"
        );
        println!(
            "  sudo ufw allow from \"$(docker network inspect {} --format '{{{{(index .IPAM.Config 0).Subnet}}}}')\" to any port {OPERATOR_API_PORT} proto tcp",
            crate::docker::APP_NETWORK
        );
    } else if cfg!(target_os = "macos") {
        println!(
            "  sudo mkdir -p /etc/resolver && echo 'nameserver {}' | sudo tee /etc/resolver/{} && dscacheutil -q host -a name admin.{}",
            result.host_ip, result.dns_suffix, result.dns_suffix
        );
    }
    println!();

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
    InvalidHostIp(String),
    AlreadyInitialized,
    ConfigWrite(String),
    ConfigRead(String),
    Tls(tls::TlsError),
    DnsSuffixMismatch {
        requested: String,
        configured: String,
    },
    ConfigurationMismatch(String),
    ExistingConfiguration,
    HostIpDetection(String),
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

impl From<tls::TlsError> for BootstrapError {
    fn from(e: tls::TlsError) -> Self {
        BootstrapError::Tls(e)
    }
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BootstrapError::Docker(e) => write!(f, "{e}"),
            BootstrapError::Db(e) => write!(f, "{e}"),
            BootstrapError::InvalidDnsSuffix(msg) => write!(f, "invalid DNS suffix: {msg}"),
            BootstrapError::InvalidHostIp(ip) => write!(f, "'{ip}' is not an IP address"),
            BootstrapError::AlreadyInitialized => {
                write!(
                    f,
                    "already initialized. Use 'self-host serve' to start the daemon"
                )
            }
            BootstrapError::ConfigWrite(msg) => write!(f, "failed to write DNS config: {msg}"),
            BootstrapError::ConfigRead(msg) => write!(f, "failed to read Platform config: {msg}"),
            BootstrapError::Tls(_) => write!(f, "failed to prepare HTTPS"),
            BootstrapError::DnsSuffixMismatch {
                requested,
                configured,
            } => write!(
                f,
                "Platform already uses DNS Suffix '{configured}', not '{requested}'; retry with '--dns {configured}' or run 'self-host reset'"
            ),
            BootstrapError::ConfigurationMismatch(msg) => write!(
                f,
                "Platform configuration does not match persisted state: {msg}; run 'self-host reset' to rebuild it"
            ),
            BootstrapError::ExistingConfiguration => write!(
                f,
                "Platform configuration exists but its state store is unavailable; start the existing Platform or run 'self-host reset'"
            ),
            BootstrapError::HostIpDetection(msg) => {
                write!(f, "failed to detect the Host LAN address: {msg}")
            }
        }
    }
}

impl std::error::Error for BootstrapError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BootstrapError::Docker(e) => Some(e),
            BootstrapError::Db(e) => Some(e),
            BootstrapError::Tls(e) => Some(e),
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

        assert_eq!(infra.len(), SYSTEM_CONTAINERS.len());
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
    fn the_state_store_is_not_published_to_the_lan() {
        let pg = infra_containers()
            .into_iter()
            .find(|container| container.name == apps::system_container_name(PG_ROLE))
            .unwrap();

        for port in &pg.ports {
            assert!(
                port.starts_with("127.0.0.1:"),
                "the state store is published on {port}, reachable from the LAN"
            );
        }
    }

    #[test]
    fn proxy_can_reach_the_operator_api_on_linux() {
        let proxy = infra_containers()
            .into_iter()
            .find(|container| container.name == apps::system_container_name(TRAEFIK_ROLE))
            .unwrap();

        assert_eq!(proxy.extra_hosts, ["host.docker.internal:host-gateway"]);
        assert!(
            proxy
                .volumes
                .iter()
                .all(|volume| !volume.contains("ca-key"))
        );
    }

    #[tokio::test]
    async fn preflight_rejects_a_different_dns_suffix_before_writes() {
        let store = crate::db::FakeStateStore::new();
        store.store_state("dns_suffix", "home.lan").await.unwrap();

        let error = preflight_initialized(&store, "test.lan").await.unwrap_err();

        assert!(matches!(error, BootstrapError::DnsSuffixMismatch { .. }));
    }
}
