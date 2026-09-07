use serde::Serialize;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use tracing::info;

/// Where the Platform keeps everything it generates: the DNS configuration, the Traefik
/// configuration, the certificates and each Application's data.
///
/// Under `cfg(test)` this is a temporary directory instead. Bootstrap writes
/// these files as a side effect of being called at all, so a unit test that
/// renders a Corefile with a fixture Host IP used to overwrite the DNS of a
/// Platform running on the same machine — `cargo test` broke name resolution
/// on the LAN. Tests get their own directory so that cannot happen.
pub fn platform_config_dir() -> PathBuf {
    #[cfg(test)]
    {
        test_config_dir()
    }
    #[cfg(not(test))]
    {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".config")
            .join("self-host")
    }
}

#[cfg(test)]
fn test_config_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("self-host-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create the test configuration directory");
        dir
    })
    .clone()
}

#[cfg(test)]
mod config_dir_tests {
    use super::platform_config_dir;

    /// A unit test once rewrote a running Platform's Corefile with a fixture
    /// Host IP, and name resolution on the LAN stopped working until someone
    /// noticed. Nothing the suite writes may land in the Operator's home.
    #[test]
    fn tests_never_write_to_the_operators_configuration() {
        let operators = dirs::home_dir()
            .expect("a home directory")
            .join(".config")
            .join("self-host");

        assert_ne!(platform_config_dir(), operators);
    }
}

#[derive(Debug, Serialize)]
struct ComposeFile {
    services: indexmap::IndexMap<String, ComposeService>,
    #[serde(skip_serializing_if = "Option::is_none")]
    volumes: Option<indexmap::IndexMap<String, ComposeVolume>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    networks: Option<indexmap::IndexMap<String, ComposeNetwork>>,
}

#[derive(Debug, Serialize, Default)]
struct ComposeService {
    image: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    container_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ports: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    environment: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    volumes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restart: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    labels: Option<indexmap::IndexMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    networks: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extra_hosts: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Default)]
struct ComposeVolume {}

#[derive(Debug, Serialize, Default)]
struct ComposeNetwork {
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    external: bool,
}

pub struct ComposeConfig {
    pub services: Vec<ComposeServiceConfig>,
    pub networks: Vec<String>,
}

pub struct ComposeServiceConfig {
    pub name: String,
    pub image: String,
    pub ports: Vec<String>,
    pub env: Vec<String>,
    pub volumes: Vec<String>,
    pub restart_policy: String,
    pub cmd: Vec<String>,
    pub labels: Vec<(String, String)>,
    pub networks: Vec<String>,
    pub extra_hosts: Vec<String>,
}

fn render_compose(config: &ComposeConfig) -> Result<String, ComposeError> {
    use indexmap::IndexMap;

    let mut services = IndexMap::new();
    let mut volumes = IndexMap::new();
    let mut has_volumes = false;

    for svc in &config.services {
        // Without container_name the runtime name would be
        // <project>-<service>-1, which is not the name ADR-0011 promises.
        let mut service = ComposeService {
            image: svc.image.clone(),
            container_name: Some(svc.name.clone()),
            restart: Some(svc.restart_policy.clone()),
            ..Default::default()
        };

        if !svc.ports.is_empty() {
            service.ports = Some(svc.ports.clone());
        }
        if !svc.env.is_empty() {
            service.environment = Some(svc.env.clone());
        }
        if !svc.volumes.is_empty() {
            service.volumes = Some(svc.volumes.clone());
            has_volumes = true;
        }
        if !svc.cmd.is_empty() {
            service.command = Some(svc.cmd.clone());
        }
        if !svc.labels.is_empty() {
            let mut labels = IndexMap::new();
            for (k, v) in &svc.labels {
                labels.insert(k.clone(), v.clone());
            }
            service.labels = Some(labels);
        }
        if !svc.networks.is_empty() {
            service.networks = Some(svc.networks.clone());
        }
        if !svc.extra_hosts.is_empty() {
            service.extra_hosts = Some(svc.extra_hosts.clone());
        }

        services.insert(svc.name.clone(), service);

        // Collect named volumes (those without a path separator on the host side)
        for vol in &svc.volumes {
            if let Some(name) = vol.split(':').next()
                && !name.starts_with('/')
                && !name.starts_with('.')
                && !name.starts_with('~')
            {
                volumes.entry(name.to_string()).or_insert(ComposeVolume {});
            }
        }
    }

    let networks = if config.networks.is_empty() {
        None
    } else {
        let mut nets = IndexMap::new();
        for name in &config.networks {
            // Pre-created by DockerRuntime::ensure_network so apps and
            // Infra share one bridge for Traefik Docker provider routing.
            nets.insert(name.clone(), ComposeNetwork { external: true });
        }
        Some(nets)
    };

    let compose = ComposeFile {
        services,
        volumes: if has_volumes && !volumes.is_empty() {
            Some(volumes)
        } else {
            None
        },
        networks,
    };

    serde_yaml::to_string(&compose).map_err(|e| ComposeError::Serialize(e.to_string()))
}

pub struct ComposeRunner {
    compose_path: PathBuf,
}

impl ComposeRunner {
    pub fn new() -> Result<Self, ComposeError> {
        let base = platform_config_dir();

        std::fs::create_dir_all(&base)
            .map_err(|e| ComposeError::Io("create infra dir".into(), e))?;

        let compose_path = base.join("docker-compose.yml");

        Ok(ComposeRunner { compose_path })
    }

    pub fn write_compose_file(&self, config: &ComposeConfig) -> Result<(), ComposeError> {
        let yaml = render_compose(config)?;

        let mut file = std::fs::File::create(&self.compose_path)
            .map_err(|e| ComposeError::Io(self.compose_path.clone(), e))?;

        file.write_all(yaml.as_bytes())
            .map_err(|e| ComposeError::Io(self.compose_path.clone(), e))?;

        info!(
            "docker-compose.yml written to {}",
            self.compose_path.display()
        );

        Ok(())
    }

    pub fn up(&self) -> Result<(), ComposeError> {
        let output = Command::new("docker")
            .args(["compose", "-f"])
            .arg(&self.compose_path)
            // Renaming a service leaves its old container behind, holding the
            // ports the new one needs. The Platform owns this file entirely,
            // so anything it no longer lists has no reason to be running.
            .args(["up", "-d", "--remove-orphans"])
            .output()
            .map_err(|e| ComposeError::Command("docker compose up".into(), e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);

            // Detect port conflicts
            if let Some(port) = detect_port_conflict(&stderr) {
                return Err(ComposeError::PortConflict {
                    port,
                    suggestion: format!(
                        "Stop the process using port {port} (run `lsof -i :{port}` to find it), \
                         or free the port and try again"
                    ),
                });
            }

            return Err(ComposeError::ComposeError(format!(
                "docker compose failed:\n{stdout}\n{stderr}"
            )));
        }

        info!("docker compose up completed successfully");
        Ok(())
    }

    pub fn path(&self) -> &PathBuf {
        &self.compose_path
    }
}

/// The port Docker could not bind, read from the line that says so. Docker
/// has two ways of saying it:
///
/// - `Bind for 0.0.0.0:53 failed: port is already allocated`
/// - `failed to bind host port 0.0.0.0:53/tcp: address already in use`
///
/// Only that line is looked at; the stderr also carries timestamps and file
/// names full of digits and colons, and the first number found is not the
/// port.
fn detect_port_conflict(stderr: &str) -> Option<u16> {
    for line in stderr.lines() {
        let lower = line.to_lowercase();
        if !lower.contains("port is already allocated") && !lower.contains("address already in use")
        {
            continue;
        }
        let address = ["Bind for ", "host port "]
            .iter()
            .find_map(|marker| line.split_once(marker).map(|(_, rest)| rest))
            .unwrap_or(line);
        // `0.0.0.0:53/tcp:` or `0.0.0.0:53 failed`: the address ends at the
        // first space, and the port sits between the last colon and the
        // protocol, if any.
        let address = address.split(' ').next().unwrap_or(address);
        let address = address.trim_end_matches(':');
        let address = address.split('/').next().unwrap_or(address);
        if let Some(port) = address
            .rsplit(':')
            .next()
            .and_then(|p| p.parse::<u16>().ok())
            && port > 0
        {
            return Some(port);
        }
    }
    None
}

#[derive(Debug)]
pub enum ComposeError {
    Io(PathBuf, std::io::Error),
    Serialize(String),
    Command(String, std::io::Error),
    ComposeError(String),
    PortConflict { port: u16, suggestion: String },
}

impl std::fmt::Display for ComposeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComposeError::Io(path, _) => write!(f, "failed to write {}", path.display()),
            ComposeError::Serialize(msg) => write!(f, "failed to generate compose file: {msg}"),
            ComposeError::Command(cmd, _) => write!(f, "failed to run '{cmd}'"),
            ComposeError::ComposeError(msg) => write!(f, "{msg}"),
            ComposeError::PortConflict { port, suggestion } => {
                write!(f, "Port {port} is already in use.\n{suggestion}")
            }
        }
    }
}

impl std::error::Error for ComposeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ComposeError::Io(_, e) => Some(e),
            ComposeError::Command(_, e) => Some(e),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(name: &str, networks: &[&str]) -> ComposeServiceConfig {
        ComposeServiceConfig {
            name: name.to_string(),
            image: "postgres:18-alpine".into(),
            ports: vec![],
            env: vec![],
            volumes: vec![],
            restart_policy: "unless-stopped".into(),
            cmd: vec![],
            labels: vec![],
            networks: networks.iter().map(|n| n.to_string()).collect(),
            extra_hosts: vec![],
        }
    }

    #[test]
    fn a_port_conflict_is_read_from_the_bind_line_and_not_from_a_timestamp() {
        let stderr = "time=\"2026-09-05T19:15:29-03:00\" level=warning msg=\"the attribute `version` is obsolete\"\n\
                      Error response from daemon: driver failed programming external connectivity on endpoint sf-system-dns: \
                      Bind for 0.0.0.0:53 failed: port is already allocated\n";
        assert_eq!(detect_port_conflict(stderr), Some(53));
        assert_eq!(
            detect_port_conflict(
                "Error response from daemon: failed to set up container networking: \
                 driver failed programming external connectivity on endpoint sf-system-dns (5dd7ee60): \
                 failed to bind host port 0.0.0.0:53/tcp: address already in use"
            ),
            Some(53)
        );
        assert_eq!(detect_port_conflict("manifest unknown"), None);
    }

    #[test]
    fn a_service_names_its_container_instead_of_letting_compose_number_it() {
        let yaml = render_compose(&ComposeConfig {
            services: vec![service("sf-system-db", &["sf-system"])],
            networks: vec!["sf-system".into()],
        })
        .unwrap();

        // Left to compose, this container would answer to
        // <project>-sf-system-db-1, and `docker ps` would stop matching the
        // name the Platform uses everywhere else.
        assert!(yaml.contains("container_name: sf-system-db"), "{yaml}");
    }

    #[test]
    fn renders_the_linux_host_gateway_mapping() {
        let mut proxy = service("sf-system-proxy", &["sf-system"]);
        proxy.extra_hosts = vec!["host.docker.internal:host-gateway".into()];

        let yaml = render_compose(&ComposeConfig {
            services: vec![proxy],
            networks: vec!["sf-system".into()],
        })
        .unwrap();

        assert!(yaml.contains("host.docker.internal:host-gateway"), "{yaml}");
    }
}
