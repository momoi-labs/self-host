use crate::compose::{ComposeConfig, ComposeError, ComposeRunner, ComposeServiceConfig};
use async_trait::async_trait;
use tokio::sync::mpsc;

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
    pub labels: Vec<(String, String)>,
    pub networks: Vec<String>,
}

pub const PLATFORM_NETWORK: &str = "self-host";

/// Application container: Traefik Host routing via labels; no host port publish.
#[derive(Debug, Clone)]
pub struct ApplicationContainer {
    pub name: String,
    pub image: String,
    pub labels: Vec<(String, String)>,
    pub network: String,
    pub ports: Vec<String>,
    pub env: Vec<String>,
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
            labels: self.labels.clone(),
            networks: self.networks.clone(),
        }
    }
}

#[async_trait]
pub trait DockerRuntime: Send + Sync {
    async fn ping(&self) -> Result<(), DockerError>;
    async fn ensure_container_running(&self, config: ContainerConfig) -> Result<(), DockerError>;
    async fn commit(&self) -> Result<(), DockerError>;
    async fn container_running(&self, name: &str) -> Result<bool, DockerError>;
    async fn ensure_network(&self, name: &str) -> Result<(), DockerError>;
    async fn pull_image(&self, image: &str) -> Result<(), DockerError>;
    async fn build_image(&self, path: &str, tag: &str) -> Result<(), DockerError>;
    async fn run_application(&self, config: ApplicationContainer) -> Result<(), DockerError>;
    async fn remove_container(&self, name: &str) -> Result<(), DockerError>;
    async fn stream_logs(
        &self,
        container_name: &str,
    ) -> Result<mpsc::Receiver<String>, DockerError>;
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

        let networks: Vec<String> = services
            .iter()
            .flat_map(|s| s.networks.iter().cloned())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();

        let config = ComposeConfig {
            services: compose_services,
            networks,
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

    async fn ensure_network(&self, name: &str) -> Result<(), DockerError> {
        let output = std::process::Command::new("docker")
            .args(["network", "inspect", name])
            .output()
            .map_err(|e| DockerError::Unavailable(format!("failed to inspect network: {e}")))?;

        if output.status.success() {
            return Ok(());
        }

        let create = std::process::Command::new("docker")
            .args(["network", "create", name])
            .output()
            .map_err(|e| DockerError::Unavailable(format!("failed to create network: {e}")))?;

        if !create.status.success() {
            let stderr = String::from_utf8_lossy(&create.stderr);
            return Err(DockerError::Unavailable(format!(
                "failed to create docker network '{name}': {stderr}"
            )));
        }

        Ok(())
    }

    async fn pull_image(&self, image: &str) -> Result<(), DockerError> {
        let output = std::process::Command::new("docker")
            .args(["pull", image])
            .output()
            .map_err(|e| DockerError::Unavailable(format!("failed to pull image: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DockerError::Unavailable(format!(
                "failed to pull image '{image}': {stderr}"
            )));
        }

        Ok(())
    }

    async fn build_image(&self, path: &str, tag: &str) -> Result<(), DockerError> {
        let output = std::process::Command::new("docker")
            .args(["build", "-t", tag, path])
            .output()
            .map_err(|e| DockerError::Unavailable(format!("failed to build image: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DockerError::Unavailable(format!(
                "failed to build image from '{path}': {stderr}"
            )));
        }

        Ok(())
    }

    async fn run_application(&self, config: ApplicationContainer) -> Result<(), DockerError> {
        // Remove any previous container with the same name (recreate path).
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &config.name])
            .output();

        let mut args = vec![
            "run".to_string(),
            "-d".to_string(),
            "--name".to_string(),
            config.name.clone(),
            "--restart".to_string(),
            "unless-stopped".to_string(),
            "--network".to_string(),
            config.network.clone(),
        ];

        for (key, value) in &config.labels {
            args.push("--label".into());
            args.push(format!("{key}={value}"));
        }

        for port in &config.ports {
            args.push("-p".into());
            args.push(port.clone());
        }

        for e in &config.env {
            args.push("-e".into());
            args.push(e.clone());
        }

        args.push(config.image.clone());

        let output = std::process::Command::new("docker")
            .args(&args)
            .output()
            .map_err(|e| DockerError::Unavailable(format!("failed to run application: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DockerError::Unavailable(format!(
                "failed to start Application container '{}': {stderr}",
                config.name
            )));
        }

        Ok(())
    }

    async fn remove_container(&self, name: &str) -> Result<(), DockerError> {
        let output = std::process::Command::new("docker")
            .args(["rm", "-f", name])
            .output()
            .map_err(|e| DockerError::Unavailable(format!("failed to remove container: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DockerError::Unavailable(format!(
                "failed to remove container '{name}': {stderr}"
            )));
        }

        Ok(())
    }

    async fn stream_logs(
        &self,
        container_name: &str,
    ) -> Result<mpsc::Receiver<String>, DockerError> {
        let (tx, rx) = mpsc::channel::<String>(64);
        let name = container_name.to_string();

        tokio::task::spawn(async move {
            let mut child = match tokio::process::Command::new("docker")
                .args(["logs", "-f", &name])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(format!("failed to start log stream: {e}")).await;
                    return;
                }
            };

            use tokio::io::AsyncBufReadExt;
            let stdout = child.stdout.take().unwrap();
            let stderr = child.stderr.take().unwrap();

            let mut stdout_lines = tokio::io::BufReader::new(stdout).lines();
            let mut stderr_lines = tokio::io::BufReader::new(stderr).lines();

            loop {
                tokio::select! {
                    result = stdout_lines.next_line() => {
                        match result {
                            Ok(Some(line)) => {
                                if tx.send(line).await.is_err() {
                                    break; // receiver dropped (client disconnected)
                                }
                            }
                            Ok(None) => break,
                            Err(_) => break,
                        }
                    }
                    result = stderr_lines.next_line() => {
                        match result {
                            Ok(Some(line)) => {
                                if tx.send(line).await.is_err() {
                                    break;
                                }
                            }
                            Ok(None) => break,
                            Err(_) => break,
                        }
                    }
                }
            }

            let _ = child.kill().await;
        });

        Ok(rx)
    }
}

#[derive(Clone, Default)]
pub struct FakeDocker {
    pub apps: std::sync::Arc<std::sync::Mutex<Vec<ApplicationContainer>>>,
    pub pulled: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    pub built: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
}

impl FakeDocker {
    pub fn new() -> Self {
        FakeDocker {
            apps: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            pulled: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            built: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    pub fn deployed_apps(&self) -> Vec<String> {
        self.apps
            .lock()
            .unwrap()
            .iter()
            .map(|a| a.name.clone())
            .collect()
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

    async fn ensure_network(&self, _name: &str) -> Result<(), DockerError> {
        Ok(())
    }

    async fn pull_image(&self, image: &str) -> Result<(), DockerError> {
        self.pulled.lock().unwrap().push(image.to_string());
        Ok(())
    }

    async fn build_image(&self, path: &str, tag: &str) -> Result<(), DockerError> {
        self.built
            .lock()
            .unwrap()
            .push((path.to_string(), tag.to_string()));
        Ok(())
    }

    async fn run_application(&self, config: ApplicationContainer) -> Result<(), DockerError> {
        // Mirror production contract: Applications must not publish host ports.
        if !config.ports.is_empty() {
            return Err(DockerError::Unavailable(
                "Application containers must not publish host ports".into(),
            ));
        }
        self.apps.lock().unwrap().push(config);
        Ok(())
    }

    async fn remove_container(&self, name: &str) -> Result<(), DockerError> {
        self.apps.lock().unwrap().retain(|a| a.name != name);
        Ok(())
    }

    async fn stream_logs(
        &self,
        _container_name: &str,
    ) -> Result<mpsc::Receiver<String>, DockerError> {
        let (tx, rx) = mpsc::channel::<String>(8);
        let _ = tx.try_send("[fake] log line 1".into());
        let _ = tx.try_send("[fake] log line 2".into());
        let _ = tx.try_send("[fake] log line 3".into());
        Ok(rx)
    }
}

#[async_trait]
impl<T> DockerRuntime for std::sync::Arc<T>
where
    T: DockerRuntime + ?Sized,
{
    async fn ping(&self) -> Result<(), DockerError> {
        (**self).ping().await
    }

    async fn ensure_container_running(&self, config: ContainerConfig) -> Result<(), DockerError> {
        (**self).ensure_container_running(config).await
    }

    async fn commit(&self) -> Result<(), DockerError> {
        (**self).commit().await
    }

    async fn container_running(&self, name: &str) -> Result<bool, DockerError> {
        (**self).container_running(name).await
    }

    async fn ensure_network(&self, name: &str) -> Result<(), DockerError> {
        (**self).ensure_network(name).await
    }

    async fn pull_image(&self, image: &str) -> Result<(), DockerError> {
        (**self).pull_image(image).await
    }

    async fn build_image(&self, path: &str, tag: &str) -> Result<(), DockerError> {
        (**self).build_image(path, tag).await
    }

    async fn run_application(&self, config: ApplicationContainer) -> Result<(), DockerError> {
        (**self).run_application(config).await
    }

    async fn remove_container(&self, name: &str) -> Result<(), DockerError> {
        (**self).remove_container(name).await
    }

    async fn stream_logs(
        &self,
        container_name: &str,
    ) -> Result<mpsc::Receiver<String>, DockerError> {
        (**self).stream_logs(container_name).await
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
            ports: vec!["5433:5432".into()],
            env: vec!["POSTGRES_USER=test".into()],
            volumes: vec!["pg-data:/var/lib/postgresql".into()],
            restart_policy: "unless-stopped".into(),
            cmd: vec![],
            labels: vec![],
            networks: vec![],
        };

        let svc = config.to_compose_service();
        assert_eq!(svc.name, "test-pg");
        assert_eq!(svc.image, "postgres:18-alpine");
        assert_eq!(svc.restart_policy, "unless-stopped");
    }
}
