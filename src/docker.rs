use crate::compose::{ComposeConfig, ComposeError, ComposeRunner, ComposeServiceConfig};
use async_trait::async_trait;
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum DockerError {
    /// Docker is not there to talk to: not installed, or the daemon is down.
    Unavailable(String),
    /// A `docker` command did not work out. The step is ours to name; the
    /// reason belongs to whatever refused, and stays underneath instead of
    /// being glued into our sentence.
    Command(String, Box<dyn std::error::Error + Send + Sync>),
    Compose(ComposeError),
}

/// Docker answered and refused, in Docker's own words.
///
/// Docker is written in Go, and Go wraps an error by writing `outer: inner`.
/// A refusal is therefore already a chain, serialised onto one line — so it
/// gets taken apart into the layers Docker had, rather than reworded by us.
#[derive(Debug)]
pub struct DockerRefusal {
    message: String,
    inner: Option<Box<DockerRefusal>>,
}

impl DockerRefusal {
    /// Rebuilds the layers Docker flattened.
    ///
    /// The separator is a colon *and a space*, which is the whole reason this
    /// is safe: `nginx:1.2.3` and `1.2.3.4:443` have no space after the colon
    /// and come through untouched.
    ///
    /// Output spanning several lines is not a wrapped error — it is a build
    /// log — so it stays as it is.
    fn parse(stderr: &str) -> Option<Self> {
        let stderr = stderr.trim();
        if stderr.is_empty() {
            return None;
        }

        let layers: Vec<&str> = if stderr.lines().count() > 1 {
            vec![stderr]
        } else {
            stderr.split(": ").map(str::trim).collect()
        };

        // Fold from the innermost layer out, so each one points at its cause.
        layers
            .into_iter()
            .filter(|layer| !layer.is_empty())
            .rev()
            .fold(None, |inner, message| {
                Some(DockerRefusal {
                    message: message.to_string(),
                    inner: inner.map(Box::new),
                })
            })
    }
}

impl std::fmt::Display for DockerRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for DockerRefusal {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner.as_deref().map(|e| e as &dyn std::error::Error)
    }
}

impl DockerError {
    /// A command that ran and was refused, with Docker's own layers underneath.
    fn refused(step: impl Into<String>, output: &std::process::Output) -> Self {
        let stderr = String::from_utf8_lossy(&output.stderr);
        match DockerRefusal::parse(&stderr) {
            Some(refusal) => DockerError::Command(step.into(), Box::new(refusal)),
            // Docker refused and said nothing. The exit code is all there is.
            None => DockerError::Command(
                step.into(),
                Box::new(DockerRefusal {
                    message: "docker exited without saying why".into(),
                    inner: None,
                }),
            ),
        }
    }

    /// A command that never started.
    fn spawn(step: impl Into<String>, e: std::io::Error) -> Self {
        DockerError::Command(step.into(), Box::new(e))
    }

    pub fn from_compose_error(err: ComposeError) -> Self {
        match err {
            ComposeError::PortConflict { port, suggestion } => {
                DockerError::Unavailable(format!("Port {port} is already in use.\n{suggestion}"))
            }
            ComposeError::Command(_, e) if e.kind() == std::io::ErrorKind::NotFound => {
                DockerError::Unavailable(
                    "Docker is not installed or not in PATH. Install Docker and try again.".into(),
                )
            }
            // The daemon is the usual suspect, but the io error underneath is
            // what actually knows, so it says so itself.
            ComposeError::Command(_cmd, e) => {
                DockerError::spawn("Failed to run Docker. Is the Docker daemon running?", e)
            }
            other => DockerError::Compose(other),
        }
    }
}

impl std::fmt::Display for DockerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DockerError::Unavailable(msg) => write!(f, "Docker unavailable: {msg}"),
            DockerError::Command(step, _) => write!(f, "{step}"),
            DockerError::Compose(_) => write!(f, "the Docker Compose command failed"),
        }
    }
}

impl std::error::Error for DockerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DockerError::Command(_, e) => Some(&**e),
            DockerError::Compose(e) => Some(e),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
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

/// Two bridges, so an Application cannot open a socket on Platform Infra.
/// Traefik sits on both: it is the only component that must reach across.
pub const SYSTEM_NETWORK: &str = "sf-system";
pub const APP_NETWORK: &str = "sf-apps";

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
    /// How many times the runtime restarted the container. `None` when there
    /// is no container to ask about.
    async fn restart_count(&self, name: &str) -> Result<Option<u32>, DockerError>;
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
                    DockerError::spawn("failed to run 'docker info'", e)
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

    async fn container_running(&self, name: &str) -> Result<bool, DockerError> {
        let output = std::process::Command::new("docker")
            .args(["inspect", "-f", "{{.State.Running}}", name])
            .output()
            .map_err(|e| DockerError::Unavailable(e.to_string()))?;

        // A container that is not there at all makes `inspect` exit non-zero,
        // which is an answer, not an error.
        Ok(output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "true")
    }

    async fn restart_count(&self, name: &str) -> Result<Option<u32>, DockerError> {
        let output = std::process::Command::new("docker")
            .args(["inspect", "-f", "{{.RestartCount}}", name])
            .output()
            .map_err(|e| DockerError::Unavailable(e.to_string()))?;

        // As with `container_running`, a missing container is an answer.
        if !output.status.success() {
            return Ok(None);
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().parse().ok())
    }

    async fn ensure_network(&self, name: &str) -> Result<(), DockerError> {
        let output = std::process::Command::new("docker")
            .args(["network", "inspect", name])
            .output()
            .map_err(|e| DockerError::spawn("failed to inspect the Docker network", e))?;

        if output.status.success() {
            return Ok(());
        }

        let create = std::process::Command::new("docker")
            .args(["network", "create", name])
            .output()
            .map_err(|e| DockerError::spawn("failed to create the Docker network", e))?;

        if !create.status.success() {
            return Err(DockerError::refused(
                format!("failed to create the Docker network '{name}'"),
                &create,
            ));
        }

        Ok(())
    }

    async fn pull_image(&self, image: &str) -> Result<(), DockerError> {
        let output = std::process::Command::new("docker")
            .args(["pull", image])
            .output()
            .map_err(|e| DockerError::spawn(format!("failed to pull image '{image}'"), e))?;

        if !output.status.success() {
            return Err(DockerError::refused(
                format!("failed to pull image '{image}'"),
                &output,
            ));
        }

        Ok(())
    }

    async fn build_image(&self, path: &str, tag: &str) -> Result<(), DockerError> {
        let output = std::process::Command::new("docker")
            .args(["build", "-t", tag, path])
            .output()
            .map_err(|e| DockerError::spawn(format!("failed to build image from '{path}'"), e))?;

        if !output.status.success() {
            return Err(DockerError::refused(
                format!("failed to build image from '{path}'"),
                &output,
            ));
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
            .map_err(|e| {
                DockerError::spawn(
                    format!("failed to start Application container '{}'", config.name),
                    e,
                )
            })?;

        if !output.status.success() {
            return Err(DockerError::refused(
                format!("failed to start Application container '{}'", config.name),
                &output,
            ));
        }

        Ok(())
    }

    async fn remove_container(&self, name: &str) -> Result<(), DockerError> {
        let output = std::process::Command::new("docker")
            .args(["rm", "-f", name])
            .output()
            .map_err(|e| DockerError::spawn(format!("failed to remove container '{name}'"), e))?;

        if !output.status.success() {
            return Err(DockerError::refused(
                format!("failed to remove container '{name}'"),
                &output,
            ));
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
    pub infra: std::sync::Arc<std::sync::Mutex<Vec<ContainerConfig>>>,
    pub pulled: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    pub built: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    /// When set, `pull_image` fails with this message — the everyday case of a
    /// typo in an image tag.
    pub pull_failure: Option<String>,
}

impl FakeDocker {
    pub fn new() -> Self {
        FakeDocker {
            apps: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            infra: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            pulled: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            built: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            pull_failure: None,
        }
    }

    pub fn failing_pull(message: &str) -> Self {
        FakeDocker {
            pull_failure: Some(message.to_string()),
            ..FakeDocker::new()
        }
    }

    pub fn infra_containers(&self) -> Vec<ContainerConfig> {
        self.infra.lock().unwrap().clone()
    }

    pub fn deployed_apps(&self) -> Vec<String> {
        self.apps
            .lock()
            .unwrap()
            .iter()
            .map(|a| a.name.clone())
            .collect()
    }

    /// Makes the Platform Infra containers look running, so tests of `/system`
    /// see the running state without going through `run_bootstrap`.
    pub fn seed_system_containers(&self) {
        let mut infra = self.infra.lock().unwrap();
        for (role, image) in crate::bootstrap::SYSTEM_CONTAINERS {
            infra.push(ContainerConfig {
                image: image.to_string(),
                name: crate::apps::system_container_name(role),
                ports: vec![],
                env: vec![],
                volumes: vec![],
                restart_policy: "unless-stopped".into(),
                cmd: vec![],
                labels: vec![],
                networks: vec![],
            });
        }
    }
}

#[async_trait]
impl DockerRuntime for FakeDocker {
    async fn ping(&self) -> Result<(), DockerError> {
        Ok(())
    }

    async fn ensure_container_running(&self, config: ContainerConfig) -> Result<(), DockerError> {
        self.infra.lock().unwrap().push(config);
        Ok(())
    }

    async fn commit(&self) -> Result<(), DockerError> {
        Ok(())
    }

    async fn container_running(&self, name: &str) -> Result<bool, DockerError> {
        Ok(self.apps.lock().unwrap().iter().any(|a| a.name == name)
            || self.infra.lock().unwrap().iter().any(|c| c.name == name))
    }

    async fn restart_count(&self, name: &str) -> Result<Option<u32>, DockerError> {
        let running = self.apps.lock().unwrap().iter().any(|a| a.name == name)
            || self.infra.lock().unwrap().iter().any(|c| c.name == name);
        Ok(running.then_some(0))
    }

    async fn ensure_network(&self, _name: &str) -> Result<(), DockerError> {
        Ok(())
    }

    async fn pull_image(&self, image: &str) -> Result<(), DockerError> {
        if let Some(message) = &self.pull_failure {
            return Err(DockerError::Command(
                format!("failed to pull image '{image}'"),
                Box::new(DockerRefusal::parse(message).expect("a pull failure says something")),
            ));
        }
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

    async fn restart_count(&self, name: &str) -> Result<Option<u32>, DockerError> {
        (**self).restart_count(name).await
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
    use crate::error::ErrorReport;

    #[test]
    fn container_config_to_compose_service() {
        let config = ContainerConfig {
            image: "postgres:18-alpine".into(),
            name: "test-pg".into(),
            ports: vec!["15432:5432".into()],
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

    /// Builds the error a refused `docker pull` produces, from real stderr.
    fn refused_pull(stderr: &str) -> DockerError {
        DockerError::Command(
            "failed to pull image 'b'".into(),
            Box::new(DockerRefusal::parse(stderr).unwrap()),
        )
    }

    #[test]
    fn a_refused_command_keeps_our_step_apart_from_dockers_answer() {
        let err = refused_pull(
            "Error response from daemon: pull access denied for b, \
             repository does not exist or may require 'docker login': \
             denied: requested access to the resource is denied",
        );

        let report = ErrorReport::new(&err);

        assert_eq!(report.error, "failed to pull image 'b'");
        assert_eq!(
            report.caused_by,
            [
                "Error response from daemon",
                "pull access denied for b, repository does not exist or may require 'docker login'",
                "denied",
                "requested access to the resource is denied",
            ]
        );
    }

    /// The reason the separator is a colon *and a space*: an image tag is not
    /// a layer, and neither is a port.
    #[test]
    fn a_tag_survives_being_taken_apart() {
        let err = refused_pull(
            "Error response from daemon: manifest for nginx:1.2.3 not found: manifest unknown",
        );

        let report = ErrorReport::new(&err);

        assert_eq!(
            report.caused_by,
            [
                "Error response from daemon",
                "manifest for nginx:1.2.3 not found",
                "manifest unknown",
            ]
        );
    }

    /// A failed build writes its log to stderr. That is output, not a wrapped
    /// error, and chopping it on every colon would be nonsense.
    #[test]
    fn output_spanning_lines_is_left_alone() {
        let log = "Step 1/3 : FROM alpine\n ---> aded1e1a5b37\nStep 2/3 : RUN false";

        let report = ErrorReport::new(&refused_pull(log));

        assert_eq!(report.caused_by, [log]);
    }

    #[test]
    fn a_command_that_never_ran_blames_the_io_error() {
        let err = DockerError::spawn(
            "failed to pull image 'b'",
            std::io::Error::from(std::io::ErrorKind::NotFound),
        );

        let report = ErrorReport::new(&err);

        assert_eq!(report.error, "failed to pull image 'b'");
        assert_eq!(report.caused_by.len(), 1);
    }
}
