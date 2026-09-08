use crate::compose;
use crate::docker::{APP_NETWORK, DockerError, DockerRuntime};
use crate::error::ErrorReport;
use crate::store::{StateStore, StoreError};
use crate::tls;
use rand::Rng;
use std::net::UdpSocket;
use tracing::info;

/// The Platform Infra the Platform starts for itself, as `(role, image)`
/// pairs. There is none left: state is files the Platform owns (ADR-0018),
/// DNS is served from the binary (ADR-0017) and so is HTTP (ADR-0019). Docker
/// runs Applications, and nothing of the Platform's own.
pub const SYSTEM_CONTAINERS: &[(&str, &str)] = &[];

/// The container an older Platform ran Traefik in. Nothing starts it any
/// more; it is only recognised so an upgrade can take ports 80 and 443 back
/// from it.
pub const LEGACY_PROXY_CONTAINER_ROLE: &str = "proxy";

pub const DEFAULT_DNS_SUFFIX: &str = "home.lan";
pub const OPERATOR_API_PORT: u16 = 3721;

/// The container an older Platform ran PostgreSQL in. Nothing starts it any
/// more; it is only recognised so an upgrade can say what to do with it.
pub const LEGACY_STATE_CONTAINER_ROLE: &str = "db";

#[derive(Debug)]
pub struct BootstrapResult {
    pub dns_suffix: String,
    pub api_key: String,
    pub api_listen_addr: String,
    pub host_ip: String,
    /// Why the Platform Infra could not be started, when Docker was missing or
    /// unreachable. Configuration and Platform state are complete either way;
    /// only Application execution and HTTPS have to wait for Docker.
    pub execution_unavailable: Option<ErrorReport>,
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

    dns_config
        .save()
        .map_err(|e| BootstrapError::ConfigWrite(e.to_string()))?;

    // Docker runs Applications and nothing else now. It is not where the
    // Platform keeps its state, nor how it serves DNS or HTTPS, so a Host
    // without it still gets a configured Platform and a working console.
    let execution_unavailable = match start_infra_containers(docker).await {
        Ok(()) => None,
        Err(BootstrapError::Docker(e)) => {
            let report = ErrorReport::new(&e);
            tracing::warn!("Application execution is unavailable: {report}");
            Some(report)
        }
        Err(e) => return Err(e),
    };

    let api_listen_addr = format!("0.0.0.0:{OPERATOR_API_PORT}");

    info!("bootstrap complete: dns_suffix={dns_suffix}, api_listen={api_listen_addr}");

    Ok(BootstrapResult {
        dns_suffix: dns_suffix.to_string(),
        api_key,
        api_listen_addr,
        host_ip,
        execution_unavailable,
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

    // The console's route is not a file any more: the Platform answers
    // `admin.<suffix>` from the same process that serves this check.
    Ok(())
}

pub async fn persist_bootstrap_state(
    store: &impl StateStore,
    result: &BootstrapResult,
) -> Result<(), BootstrapError> {
    store.initialize().await?;

    if store.is_initialized().await? {
        return Err(BootstrapError::AlreadyInitialized);
    }

    store.store_state("api_key", &result.api_key).await?;
    store.store_state("dns_suffix", &result.dns_suffix).await?;
    store.store_state("host_ip", &result.host_ip).await?;

    Ok(())
}

/// Readies Docker to run Applications, or says why it cannot. Bootstrap
/// completes without it, so this is also how a Host that had no Docker at
/// `init` time is repaired once it does.
pub async fn prepare_execution(docker: &impl DockerRuntime) -> Result<(), BootstrapError> {
    start_infra_containers(docker).await
}

async fn start_infra_containers(docker: &impl DockerRuntime) -> Result<(), BootstrapError> {
    docker.ping().await?;
    // The one network the Platform still needs: the bridge Applications share
    // so a Compose project can talk to itself.
    docker.ensure_network(APP_NETWORK).await?;
    Ok(())
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

    if let Some(reason) = &result.execution_unavailable {
        println!("--- Application execution is unavailable ---");
        println!("{reason}");
        println!();
        println!("The Platform is configured and its state is saved. DNS, the Operator API");
        println!("and the console work without Docker. Install or start Docker, then run");
        println!("'self-host init' again to bring up HTTPS and deploy Applications.");
        println!();
    }

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
    Store(StoreError),
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

impl From<StoreError> for BootstrapError {
    fn from(e: StoreError) -> Self {
        BootstrapError::Store(e)
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
            BootstrapError::Store(e) => write!(f, "{e}"),
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
            BootstrapError::Store(e) => Some(e),
            BootstrapError::Tls(e) => Some(e),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Platform used to run its state store and its proxy in containers,
    /// so a Host without Docker had no console, no configuration and no way
    /// in. Both are served from this process now, and nothing is left for
    /// Docker to hold on the Platform's behalf.
    #[test]
    fn the_platform_runs_no_infra_container_of_its_own() {
        assert!(SYSTEM_CONTAINERS.is_empty());
    }

    #[tokio::test]
    async fn preflight_rejects_a_different_dns_suffix_before_writes() {
        let store = crate::store::FakeStateStore::new();
        store.store_state("dns_suffix", "home.lan").await.unwrap();

        let error = preflight_initialized(&store, "test.lan").await.unwrap_err();

        assert!(matches!(error, BootstrapError::DnsSuffixMismatch { .. }));
    }
}
