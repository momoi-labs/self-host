use crate::compose::{ComposeConfig, ComposeError, ComposeRunner, ComposeServiceConfig};
use async_trait::async_trait;

#[derive(Debug)]
pub enum DockerError {
    Unavailable(String),
    Compose(ComposeError),
}

impl DockerError {
    pub fn from_compose_error(err: ComposeError) -> Self {
        match &err {
            ComposeError::PortConflict { port, suggestion } => {
                DockerError::Unavailable(format!("Port {port} is already in use.\n{suggestion}"))
            }
            ComposeError::Command(_cmd, e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    DockerError::Unavailable(
                        "Docker is not installed or not in PATH. Install Docker and try again."
                            .into(),
                    )
                } else {
                    DockerError::Unavailable(format!(
                        "Failed to run Docker. Is the Docker daemon running?\nDetails: {e}"
                    ))
                }
            }
            _ => DockerError::Compose(err),
        }
    }
}

impl std::fmt::Display for DockerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DockerError::Unavailable(msg) => write!(f, "Docker unavailable: {msg}"),
            DockerError::Compose(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DockerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DockerError::Compose(e) => Some(e),
            _ => None,
        }
    }
}

pub struct ContainerConfig {
    pub image: String,
    pub name: String,
    pub ports: Vec<String>,
    pub env: Vec<String>,
    pub volumes: Vec<String>,
    pub restart_policy: String,
    pub cmd: Vec<String>,
}

impl ContainerConfig {
    fn to_compose_service(&self) -> ComposeServiceConfig {
        ComposeServiceConfig {
            name: self.name.clone(),
            image: self.image.clone(),
            ports: self.ports.clone(),
            env: self.env.clone(),
            volumes: self.volumes.clone(),
            restart_policy: self.restart_policy.clone(),
            cmd: self.cmd.clone(),
        }
    }
}

#[async_trait]
pub trait DockerRuntime: Send + Sync {
    async fn ping(&self) -> Result<(), DockerError>;
    async fn ensure_container_running(&self, config: ContainerConfig) -> Result<(), DockerError>;
    async fn commit(&self) -> Result<(), DockerError>;
    async fn container_running(&self, name: &str) -> Result<bool, DockerError>;
}

pub struct ComposeDocker {
    runner: ComposeRunner,
    services: std::sync::Mutex<Vec<ContainerConfig>>,
}

impl ComposeDocker {
    pub fn new() -> Result<Self, DockerError> {
        let runner = ComposeRunner::new().map_err(DockerError::Compose)?;
        Ok(ComposeDocker {
            runner,
            services: std::sync::Mutex::new(Vec::new()),
        })
    }

    fn flush(&self) -> Result<(), DockerError> {
        let services = self.services.lock().unwrap();
        if services.is_empty() {
            return Ok(());
        }

        let compose_services: Vec<ComposeServiceConfig> =
            services.iter().map(|s| s.to_compose_service()).collect();

        let config = ComposeConfig {
            services: compose_services,
        };

        self.runner
            .write_compose_file(&config)
            .map_err(DockerError::Compose)?;

        self.runner.up().map_err(DockerError::from_compose_error)?;

        Ok(())
    }

    pub fn compose_path(&self) -> std::path::PathBuf {
        self.runner.path().clone()
    }
}

#[async_trait]
impl DockerRuntime for ComposeDocker {
    async fn ping(&self) -> Result<(), DockerError> {
        // Check if docker is available
        let output = std::process::Command::new("docker")
            .args(["info"])
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    DockerError::Unavailable(
                        "Docker is not installed. Install Docker and try again.".into(),
                    )
                } else {
                    DockerError::Unavailable(format!("failed to run docker: {e}"))
                }
            })?;

        if !output.status.success() {
            return Err(DockerError::Unavailable(
                "Docker daemon is not running. Start Docker and try again.".into(),
            ));
        }

        Ok(())
    }

    async fn ensure_container_running(&self, config: ContainerConfig) -> Result<(), DockerError> {
        let mut services = self.services.lock().unwrap();

        // Replace existing service with same name
        services.retain(|s| s.name != config.name);
        services.push(config);
        Ok(())
    }

    async fn commit(&self) -> Result<(), DockerError> {
        self.flush()
    }

    async fn container_running(&self, _name: &str) -> Result<bool, DockerError> {
        Ok(true)
    }
}

pub struct FakeDocker;

impl FakeDocker {
    pub fn new() -> Self {
        FakeDocker
    }
}

impl Default for FakeDocker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DockerRuntime for FakeDocker {
    async fn ping(&self) -> Result<(), DockerError> {
        Ok(())
    }

    async fn ensure_container_running(&self, _config: ContainerConfig) -> Result<(), DockerError> {
        Ok(())
    }

    async fn commit(&self) -> Result<(), DockerError> {
        Ok(())
    }

    async fn container_running(&self, _name: &str) -> Result<bool, DockerError> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_config_to_compose_service() {
        let config = ContainerConfig {
            image: "postgres:18-alpine".into(),
            name: "test-pg".into(),
            ports: vec!["5432:5432".into()],
            env: vec!["POSTGRES_USER=test".into()],
            volumes: vec!["pg-data:/var/lib/postgresql".into()],
            restart_policy: "unless-stopped".into(),
            cmd: vec![],
        };

        let svc = config.to_compose_service();
        assert_eq!(svc.name, "test-pg");
        assert_eq!(svc.image, "postgres:18-alpine");
        assert_eq!(svc.restart_policy, "unless-stopped");
    }
}
