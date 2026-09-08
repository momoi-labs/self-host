use crate::compose_app::ComposeProject;
use async_trait::async_trait;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum DockerError {
    /// Docker is not there to talk to: not installed, or the daemon is down.
    Unavailable(String),
    /// A `docker` command did not work out. The step is ours to name; the
    /// reason belongs to whatever refused, and stays underneath instead of
    /// being glued into our sentence.
    Command(String, Box<dyn std::error::Error + Send + Sync>),
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
}

impl std::fmt::Display for DockerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DockerError::Unavailable(msg) => write!(f, "Docker unavailable: {msg}"),
            DockerError::Command(step, _) => write!(f, "{step}"),
        }
    }
}

impl std::error::Error for DockerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DockerError::Command(_, e) => Some(&**e),
            _ => None,
        }
    }
}

/// The bridge Applications share. The Platform is not on it: it reaches an
/// Application through the Host port its Web Target publishes (ADR-0019),
/// which is also why an Application can no longer open a socket on anything
/// of the Platform's.
pub const APP_NETWORK: &str = "sf-apps";

/// Application container: routed by Hostname, and publishing its Web Target
/// on loopback so the Host's proxy can reach it (ADR-0019).
#[derive(Debug, Clone)]
pub struct ApplicationContainer {
    pub name: String,
    pub image: String,
    pub labels: Vec<(String, String)>,
    pub network: String,
    pub ports: Vec<String>,
    pub env: Vec<String>,
}

/// What Docker says about one container, as far as the console needs to know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerState {
    /// Docker's own word: `running`, `exited`, `restarting`, `created`, …
    pub status: String,
    pub exit_code: i64,
    pub restarts: u32,
}

impl ContainerState {
    pub fn is_running(&self) -> bool {
        self.status == "running"
    }
}

#[async_trait]
pub trait DockerRuntime: Send + Sync {
    async fn ping(&self) -> Result<(), DockerError>;
    async fn container_running(&self, name: &str) -> Result<bool, DockerError>;
    /// Sorted names owned by the Application, or its legacy name when no labels match.
    /// Always returns at least one name; a name does not imply a running container.
    async fn application_containers(&self, id: &str) -> Result<Vec<String>, DockerError>;
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

    /// The container's state, or `None` when there is no such container.
    async fn container_state(&self, name: &str) -> Result<Option<ContainerState>, DockerError>;
    async fn start_container(&self, name: &str) -> Result<(), DockerError>;
    async fn stop_container(&self, name: &str) -> Result<(), DockerError>;
    async fn restart_container(&self, name: &str) -> Result<(), DockerError>;

    /// Writes the project and brings it up. Images are pulled as needed and
    /// only the services whose definition changed are recreated.
    async fn compose_up(&self, project: &ComposeProject) -> Result<(), DockerError>;
    async fn compose_start(&self, project: &ComposeProject) -> Result<(), DockerError>;
    async fn compose_stop(&self, project: &ComposeProject) -> Result<(), DockerError>;
    async fn compose_restart(&self, project: &ComposeProject) -> Result<(), DockerError>;
    /// Removes the containers and the project network. Volumes stay: they
    /// are the Application's data, and removing an Application is not
    /// permission to delete what it wrote.
    async fn compose_down(&self, project: &ComposeProject) -> Result<(), DockerError>;
    /// Every service's output, interleaved and prefixed by service name.
    async fn stream_compose_logs(
        &self,
        project: &ComposeProject,
    ) -> Result<mpsc::Receiver<String>, DockerError>;
}

/// Docker, as the Platform drives it: the `docker` CLI on this Host.
#[derive(Default)]
pub struct ComposeDocker;

impl ComposeDocker {
    pub fn new() -> Self {
        ComposeDocker
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

    async fn container_running(&self, name: &str) -> Result<bool, DockerError> {
        let output = std::process::Command::new("docker")
            .args(["inspect", "-f", "{{.State.Running}}", name])
            .output()
            .map_err(|e| DockerError::Unavailable(e.to_string()))?;

        // A container that is not there at all makes `inspect` exit non-zero,
        // which is an answer, not an error.
        Ok(output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "true")
    }

    async fn application_containers(&self, id: &str) -> Result<Vec<String>, DockerError> {
        let output = std::process::Command::new("docker")
            .args([
                "ps",
                "-a",
                "--filter",
                &format!("label=sf.app.id={id}"),
                "--format",
                "{{.Names}}",
            ])
            .output()
            .map_err(|e| DockerError::spawn("failed to list Application containers", e))?;
        if !output.status.success() {
            return Err(DockerError::refused(
                "failed to list Application containers",
                &output,
            ));
        }
        let mut names: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_owned)
            .collect();
        names.sort();
        // Older Applications predate identity labels.
        if names.is_empty() {
            names.push(crate::apps::container_name_for(id));
        }
        Ok(names)
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
                    _ = tx.closed() => break,
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

    async fn container_state(&self, name: &str) -> Result<Option<ContainerState>, DockerError> {
        let output = std::process::Command::new("docker")
            .args([
                "inspect",
                "-f",
                "{{.State.Status}} {{.State.ExitCode}} {{.RestartCount}}",
                name,
            ])
            .output()
            .map_err(|e| DockerError::spawn(format!("failed to inspect container '{name}'"), e))?;

        // No such container is an answer, not an error.
        if !output.status.success() {
            return Ok(None);
        }

        let text = String::from_utf8_lossy(&output.stdout);
        let mut parts = text.split_whitespace();
        let status = parts.next().unwrap_or("unknown").to_string();
        let exit_code = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        let restarts = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        Ok(Some(ContainerState {
            status,
            exit_code,
            restarts,
        }))
    }

    async fn start_container(&self, name: &str) -> Result<(), DockerError> {
        docker(
            &["start", name],
            format!("failed to start container '{name}'"),
        )
    }

    async fn stop_container(&self, name: &str) -> Result<(), DockerError> {
        docker(
            &["stop", name],
            format!("failed to stop container '{name}'"),
        )
    }

    async fn restart_container(&self, name: &str) -> Result<(), DockerError> {
        docker(
            &["restart", name],
            format!("failed to restart container '{name}'"),
        )
    }

    async fn compose_up(&self, project: &ComposeProject) -> Result<(), DockerError> {
        write_project(project)?;
        compose(
            project,
            &["up", "-d", "--remove-orphans"],
            "failed to bring the Application up",
        )
    }

    async fn compose_start(&self, project: &ComposeProject) -> Result<(), DockerError> {
        compose(project, &["start"], "failed to start the Application")
    }

    async fn compose_stop(&self, project: &ComposeProject) -> Result<(), DockerError> {
        compose(project, &["stop"], "failed to stop the Application")
    }

    async fn compose_restart(&self, project: &ComposeProject) -> Result<(), DockerError> {
        compose(project, &["restart"], "failed to restart the Application")
    }

    async fn compose_down(&self, project: &ComposeProject) -> Result<(), DockerError> {
        if !project_file(project).exists() {
            return Ok(());
        }
        compose(
            project,
            &["down", "--remove-orphans"],
            "failed to remove the Application",
        )
    }

    async fn stream_compose_logs(
        &self,
        project: &ComposeProject,
    ) -> Result<mpsc::Receiver<String>, DockerError> {
        let file = project_file(project);
        let mut args = compose_args(project, &file);
        args.extend(
            ["logs", "--follow", "--no-color", "--tail", "200"]
                .iter()
                .map(|s| s.to_string()),
        );
        Ok(spawn_log_stream("docker", args))
    }
}

/// Runs one `docker` command and reports a refusal in Docker's words.
fn docker(args: &[&str], step: String) -> Result<(), DockerError> {
    let output = std::process::Command::new("docker")
        .args(args)
        .output()
        .map_err(|e| DockerError::spawn(step.clone(), e))?;
    if !output.status.success() {
        return Err(DockerError::refused(step, &output));
    }
    Ok(())
}

fn project_file(project: &ComposeProject) -> std::path::PathBuf {
    project.dir.join("compose.yml")
}

fn compose_args(project: &ComposeProject, file: &Path) -> Vec<String> {
    vec![
        "compose".into(),
        "--project-name".into(),
        project.name.clone(),
        "--project-directory".into(),
        project.dir.display().to_string(),
        "-f".into(),
        file.display().to_string(),
    ]
}

/// Writes the project file and creates every directory it bind-mounts, so
/// the mounts are owned by the Platform's user and not by whoever Docker
/// runs as.
fn write_project(project: &ComposeProject) -> Result<(), DockerError> {
    let io = |what: &str, e: std::io::Error| {
        DockerError::Command(
            format!("failed to write the Application project: {what}"),
            Box::new(e),
        )
    };
    std::fs::create_dir_all(&project.dir).map_err(|e| io("create project directory", e))?;
    for dir in &project.bind_dirs {
        std::fs::create_dir_all(dir).map_err(|e| io("create data directory", e))?;
    }
    let path = project_file(project);
    std::fs::write(&path, &project.yaml).map_err(|e| io("write compose.yml", e))?;

    // The rendered project carries the Application's environment, which is
    // where an Operator's API tokens and database passwords end up. It is
    // generated from state that is already `0600`; it must not be the copy
    // anyone on the Host can read. The bind-mounted data directories keep
    // their own permissions: a container runs as its own user and has to be
    // able to reach them.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| io("restrict compose.yml", e))
}

fn compose(project: &ComposeProject, verb: &[&str], step: &str) -> Result<(), DockerError> {
    let file = project_file(project);
    let mut args = compose_args(project, &file);
    args.extend(verb.iter().map(|s| s.to_string()));

    let output = std::process::Command::new("docker")
        .args(&args)
        .output()
        .map_err(|e| DockerError::spawn(step, e))?;

    if !output.status.success() {
        return Err(DockerError::refused(step, &output));
    }
    Ok(())
}

/// Runs a command and hands its stdout and stderr back line by line, until
/// the reader goes away.
fn spawn_log_stream(program: &str, args: Vec<String>) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel::<String>(64);
    let program = program.to_string();

    tokio::task::spawn(async move {
        let mut child = match tokio::process::Command::new(&program)
            .args(&args)
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
        let mut stdout_open = true;
        let mut stderr_open = true;

        while stdout_open || stderr_open {
            tokio::select! {
                _ = tx.closed() => break,
                result = stdout_lines.next_line(), if stdout_open => {
                    match result {
                        Ok(Some(line)) => {
                            if tx.send(line).await.is_err() {
                                break; // receiver dropped (client disconnected)
                            }
                        }
                        _ => stdout_open = false,
                    }
                }
                result = stderr_lines.next_line(), if stderr_open => {
                    match result {
                        Ok(Some(line)) => {
                            if tx.send(line).await.is_err() {
                                break;
                            }
                        }
                        _ => stderr_open = false,
                    }
                }
            }
        }

        let _ = child.kill().await;
    });

    rx
}

#[derive(Clone, Default)]
pub struct FakeDocker {
    pub apps: std::sync::Arc<std::sync::Mutex<Vec<ApplicationContainer>>>,
    pub pulled: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    pub built: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
    /// When set, `pull_image` fails with this message — the everyday case of a
    /// typo in an image tag.
    pub pull_failure: Option<String>,
    /// When set, the runtime is not there to talk to: Docker is not installed
    /// on the Host, or its daemon is down.
    pub unreachable: Option<String>,
    /// Compose projects brought up, latest definition per name.
    pub projects: std::sync::Arc<std::sync::Mutex<Vec<ComposeProject>>>,
    /// Containers the Operator stopped, by name.
    pub stopped: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    /// Containers that exist but are not running, with their exit code — a
    /// service that fell over at startup.
    pub exited: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, i64>>>,
    /// When set, `compose_up` fails with this message.
    pub compose_failure: Option<String>,
    /// When set, `run_application` fails with this message — a Host that
    /// cannot start the container it was asked for.
    pub run_failure: Option<String>,
}

impl FakeDocker {
    pub fn new() -> Self {
        FakeDocker {
            apps: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            pulled: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            built: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            pull_failure: None,
            unreachable: None,
            projects: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            stopped: std::sync::Arc::new(std::sync::Mutex::new(Default::default())),
            exited: std::sync::Arc::new(std::sync::Mutex::new(Default::default())),
            compose_failure: None,
            run_failure: None,
        }
    }

    pub fn failing_compose(message: &str) -> Self {
        FakeDocker {
            compose_failure: Some(message.to_string()),
            ..FakeDocker::new()
        }
    }

    pub fn failing_run(message: &str) -> Self {
        FakeDocker {
            run_failure: Some(message.to_string()),
            ..FakeDocker::new()
        }
    }

    pub fn project(&self, name: &str) -> Option<ComposeProject> {
        self.projects
            .lock()
            .unwrap()
            .iter()
            .find(|p| p.name == name)
            .cloned()
    }

    /// Makes a container look like it fell over, as a service with a bad
    /// command does.
    pub fn exit_container(&self, name: &str, code: i64) {
        self.exited.lock().unwrap().insert(name.to_string(), code);
    }

    fn known_container(&self, name: &str) -> bool {
        self.apps.lock().unwrap().iter().any(|a| a.name == name)
            || self
                .projects
                .lock()
                .unwrap()
                .iter()
                .any(|p| p.containers.iter().any(|(_, c)| c == name))
    }

    pub fn failing_pull(message: &str) -> Self {
        FakeDocker {
            pull_failure: Some(message.to_string()),
            ..FakeDocker::new()
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
        match &self.unreachable {
            Some(reason) => Err(DockerError::Unavailable(reason.clone())),
            None => Ok(()),
        }
    }

    async fn container_running(&self, name: &str) -> Result<bool, DockerError> {
        Ok(self.apps.lock().unwrap().iter().any(|a| a.name == name))
    }

    async fn application_containers(&self, id: &str) -> Result<Vec<String>, DockerError> {
        let mut names: Vec<String> = self
            .apps
            .lock()
            .unwrap()
            .iter()
            .filter(|app| {
                app.labels
                    .iter()
                    .any(|(key, value)| key == "sf.app.id" && value == id)
            })
            .map(|app| app.name.clone())
            .collect();
        if let Some(project) = self.project(&crate::apps::container_name_for(id)) {
            names.extend(project.containers.iter().map(|(_, name)| name.clone()));
        }
        names.sort();
        if names.is_empty() {
            names.push(crate::apps::container_name_for(id));
        }
        Ok(names)
    }

    async fn restart_count(&self, name: &str) -> Result<Option<u32>, DockerError> {
        let running = self.apps.lock().unwrap().iter().any(|a| a.name == name);
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
        if let Some(message) = &self.run_failure {
            return Err(DockerError::Unavailable(message.clone()));
        }
        // Mirror the production contract: an Application publishes its Web
        // Target for the proxy on the Host and nothing on the LAN.
        if let Some(port) = config
            .ports
            .iter()
            .find(|port| !port.starts_with("127.0.0.1:"))
        {
            return Err(DockerError::Unavailable(format!(
                "Application containers may only publish on loopback, not '{port}'"
            )));
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
        container_name: &str,
    ) -> Result<mpsc::Receiver<String>, DockerError> {
        let (tx, rx) = mpsc::channel::<String>(8);
        let _ = tx.try_send(format!("[fake] log line 1 from {container_name}"));
        let _ = tx.try_send("[fake] log line 2".into());
        let _ = tx.try_send("[fake] log line 3".into());
        Ok(rx)
    }

    async fn container_state(&self, name: &str) -> Result<Option<ContainerState>, DockerError> {
        if !self.known_container(name) {
            return Ok(None);
        }
        if let Some(code) = self.exited.lock().unwrap().get(name) {
            return Ok(Some(ContainerState {
                status: "exited".into(),
                exit_code: *code,
                restarts: 3,
            }));
        }
        let stopped = self.stopped.lock().unwrap().contains(name);
        Ok(Some(ContainerState {
            status: if stopped {
                "exited".into()
            } else {
                "running".into()
            },
            exit_code: 0,
            restarts: 0,
        }))
    }

    async fn start_container(&self, name: &str) -> Result<(), DockerError> {
        self.stopped.lock().unwrap().remove(name);
        Ok(())
    }

    async fn stop_container(&self, name: &str) -> Result<(), DockerError> {
        self.stopped.lock().unwrap().insert(name.to_string());
        Ok(())
    }

    async fn restart_container(&self, name: &str) -> Result<(), DockerError> {
        self.stopped.lock().unwrap().remove(name);
        Ok(())
    }

    async fn compose_up(&self, project: &ComposeProject) -> Result<(), DockerError> {
        if let Some(message) = &self.compose_failure {
            return Err(DockerError::Command(
                "failed to bring the Application up".into(),
                Box::new(DockerRefusal::parse(message).expect("a compose failure says something")),
            ));
        }
        let mut projects = self.projects.lock().unwrap();
        projects.retain(|p| p.name != project.name);
        projects.push(project.clone());
        let mut stopped = self.stopped.lock().unwrap();
        for (_, container) in &project.containers {
            stopped.remove(container);
        }
        Ok(())
    }

    async fn compose_start(&self, project: &ComposeProject) -> Result<(), DockerError> {
        let mut stopped = self.stopped.lock().unwrap();
        for (_, container) in &project.containers {
            stopped.remove(container);
        }
        Ok(())
    }

    async fn compose_stop(&self, project: &ComposeProject) -> Result<(), DockerError> {
        let mut stopped = self.stopped.lock().unwrap();
        for (_, container) in &project.containers {
            stopped.insert(container.clone());
        }
        Ok(())
    }

    async fn compose_restart(&self, project: &ComposeProject) -> Result<(), DockerError> {
        self.compose_start(project).await
    }

    async fn compose_down(&self, project: &ComposeProject) -> Result<(), DockerError> {
        self.projects
            .lock()
            .unwrap()
            .retain(|p| p.name != project.name);
        Ok(())
    }

    async fn stream_compose_logs(
        &self,
        project: &ComposeProject,
    ) -> Result<mpsc::Receiver<String>, DockerError> {
        let (tx, rx) = mpsc::channel::<String>(8);
        for (service, _) in &project.containers {
            let _ = tx.try_send(format!("{service}  | [fake] log line 1"));
        }
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

    async fn container_running(&self, name: &str) -> Result<bool, DockerError> {
        (**self).container_running(name).await
    }

    async fn application_containers(&self, id: &str) -> Result<Vec<String>, DockerError> {
        (**self).application_containers(id).await
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

    async fn container_state(&self, name: &str) -> Result<Option<ContainerState>, DockerError> {
        (**self).container_state(name).await
    }

    async fn start_container(&self, name: &str) -> Result<(), DockerError> {
        (**self).start_container(name).await
    }

    async fn stop_container(&self, name: &str) -> Result<(), DockerError> {
        (**self).stop_container(name).await
    }

    async fn restart_container(&self, name: &str) -> Result<(), DockerError> {
        (**self).restart_container(name).await
    }

    async fn compose_up(&self, project: &ComposeProject) -> Result<(), DockerError> {
        (**self).compose_up(project).await
    }

    async fn compose_start(&self, project: &ComposeProject) -> Result<(), DockerError> {
        (**self).compose_start(project).await
    }

    async fn compose_stop(&self, project: &ComposeProject) -> Result<(), DockerError> {
        (**self).compose_stop(project).await
    }

    async fn compose_restart(&self, project: &ComposeProject) -> Result<(), DockerError> {
        (**self).compose_restart(project).await
    }

    async fn compose_down(&self, project: &ComposeProject) -> Result<(), DockerError> {
        (**self).compose_down(project).await
    }

    async fn stream_compose_logs(
        &self,
        project: &ComposeProject,
    ) -> Result<mpsc::Receiver<String>, DockerError> {
        (**self).stream_compose_logs(project).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorReport;

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
