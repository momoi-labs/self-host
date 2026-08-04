use serde::Serialize;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use tracing::info;

pub fn platform_config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("self-host")
}

#[derive(Debug, Serialize)]
struct ComposeFile {
    version: String,
    services: indexmap::IndexMap<String, ComposeService>,
    volumes: Option<indexmap::IndexMap<String, ComposeVolume>>,
}

#[derive(Debug, Serialize, Default)]
struct ComposeService {
    image: String,
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
}

#[derive(Debug, Serialize, Default)]
struct ComposeVolume {}

pub struct ComposeConfig {
    pub services: Vec<ComposeServiceConfig>,
}

pub struct ComposeServiceConfig {
    pub name: String,
    pub image: String,
    pub ports: Vec<String>,
    pub env: Vec<String>,
    pub volumes: Vec<String>,
    pub restart_policy: String,
    pub cmd: Vec<String>,
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
        use indexmap::IndexMap;

        let mut services = IndexMap::new();
        let mut volumes = IndexMap::new();
        let mut has_volumes = false;

        for svc in &config.services {
            let mut service = ComposeService {
                image: svc.image.clone(),
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

        let compose = ComposeFile {
            version: "3.8".to_string(),
            services,
            volumes: if has_volumes && !volumes.is_empty() {
                Some(volumes)
            } else {
                None
            },
        };

        let yaml =
            serde_yaml::to_string(&compose).map_err(|e| ComposeError::Serialize(e.to_string()))?;

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
            .args(["up", "-d"])
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

fn detect_port_conflict(stderr: &str) -> Option<u16> {
    let lower = stderr.to_lowercase();

    if !lower.contains("port is already allocated")
        && !lower.contains("already in use")
        && !lower.contains("bind for")
        && !lower.contains("failed: port")
    {
        return None;
    }

    for part in stderr.split_whitespace() {
        let port_str = part.split(':').next_back()?;
        let Ok(port) = port_str
            .trim_end_matches(|c: char| !c.is_ascii_digit())
            .parse::<u16>()
        else {
            continue;
        };
        if port > 0 {
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
            ComposeError::Io(path, e) => write!(f, "failed to write {}: {e}", path.display()),
            ComposeError::Serialize(msg) => write!(f, "failed to generate compose file: {msg}"),
            ComposeError::Command(cmd, e) => write!(f, "failed to run '{cmd}': {e}"),
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
