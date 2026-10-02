use crate::compose_app::ComposeProject;
use async_trait::async_trait;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tokio::sync::mpsc;

/// Checked checkout paths and declared inputs for a Git build. Credential
/// files stay outside the build context and never reach command arguments.
#[derive(Clone, Debug)]
pub struct SourceBuild {
    pub context: std::path::PathBuf,
    pub dockerfile: std::path::PathBuf,
    pub tag: String,
    pub build_args: std::collections::BTreeMap<String, String>,
    pub secret_files: std::collections::BTreeMap<String, std::path::PathBuf>,
    pub registry_config: Option<std::path::PathBuf>,
    pub base_images: Vec<String>,
}

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
    pub additional_networks: Vec<String>,
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
    pub health: Option<String>,
}

impl ContainerState {
    pub fn is_running(&self) -> bool {
        self.status == "running"
    }
}

/// One Application container's resource usage, as `docker stats` reports it.
/// Cumulative where Docker counts cumulatively: network I/O is since the
/// container started, and a collector reading two ticks apart derives the
/// rate.
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerStats {
    pub container: String,
    /// The Application the container belongs to, from its `sf.app.id` label.
    pub application: String,
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub memory_limit_bytes: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

#[async_trait]
pub trait DockerRuntime: Send + Sync {
    async fn build_source(&self, _build: &SourceBuild) -> Result<String, DockerError> {
        Err(DockerError::Unavailable(
            "Git builds are unavailable in this runtime".into(),
        ))
    }
    async fn pin_source_image(
        &self,
        _image: &str,
        _registry_config: Option<&Path>,
    ) -> Result<String, DockerError> {
        Err(DockerError::Unavailable(
            "Git image pinning is unavailable in this runtime".into(),
        ))
    }
    async fn ping(&self) -> Result<(), DockerError>;
    async fn container_running(&self, name: &str) -> Result<bool, DockerError>;
    /// Sorted names owned by the Application, or its legacy name when no labels match.
    /// Always returns at least one name; a name does not imply a running container.
    async fn application_containers(&self, id: &str) -> Result<Vec<String>, DockerError>;
    /// How many times the runtime restarted the container. `None` when there
    /// is no container to ask about.
    async fn restart_count(&self, name: &str) -> Result<Option<u32>, DockerError>;
    async fn ensure_network(&self, name: &str) -> Result<(), DockerError>;
    async fn ensure_private_network(&self, name: &str) -> Result<(), DockerError>;
    /// Reconciles only Platform private networks. Never starts a container.
    async fn sync_private_networks(
        &self,
        container: &str,
        networks: &[String],
    ) -> Result<(), DockerError>;
    /// Removes a network when it is there, and says whether it was. A network
    /// with members is left alone and reported as an error, never force-removed.
    async fn remove_network_if_exists(&self, name: &str) -> Result<bool, DockerError>;
    async fn pull_image(&self, image: &str) -> Result<(), DockerError>;
    /// The id of the image a reference points to on this Host, or `None`
    /// when the Host does not have it.
    async fn image_id(&self, image: &str) -> Result<Option<String>, DockerError>;
    async fn container_images(&self) -> Result<Vec<String>, DockerError>;
    async fn remove_image_repository(&self, repository: &str) -> Result<(), DockerError>;
    async fn build_image(&self, path: &str, tag: &str) -> Result<(), DockerError>;
    async fn build_image_with_logs(
        &self,
        path: &str,
        tag: &str,
        _logs: mpsc::Sender<String>,
    ) -> Result<(), DockerError> {
        self.build_image(path, tag).await
    }
    async fn run_application(&self, config: ApplicationContainer) -> Result<(), DockerError>;
    async fn remove_container(&self, name: &str) -> Result<(), DockerError>;
    async fn stream_logs(
        &self,
        container_name: &str,
    ) -> Result<mpsc::Receiver<String>, DockerError>;

    /// The container's state, or `None` when there is no such container.
    async fn container_state(&self, name: &str) -> Result<Option<ContainerState>, DockerError>;
    /// Resource usage of every running Application container, joined to its
    /// Application by label. Containers the Platform does not own are not
    /// the Platform's to report.
    async fn container_stats(&self) -> Result<Vec<ContainerStats>, DockerError>;
    async fn open_terminal(
        &self,
        container: &str,
        size: crate::terminal::Size,
        user: Option<&str>,
    ) -> Result<crate::terminal::Session, DockerError>;
    async fn start_container(&self, name: &str) -> Result<(), DockerError>;
    async fn stop_container(&self, name: &str) -> Result<(), DockerError>;
    async fn restart_container(&self, name: &str) -> Result<(), DockerError>;

    /// Writes the project and brings it up. Images are pulled as needed and
    /// only the services whose definition changed are recreated.
    async fn compose_up(&self, project: &ComposeProject) -> Result<(), DockerError>;
    async fn compose_start(&self, project: &ComposeProject) -> Result<(), DockerError>;
    async fn compose_stop(&self, project: &ComposeProject) -> Result<(), DockerError>;
    async fn compose_restart(&self, project: &ComposeProject) -> Result<(), DockerError>;
    /// Replaces every container of the project with a new one from the image
    /// on the Host, the way a restart would, but picking up a newer image.
    async fn compose_recreate(&self, project: &ComposeProject) -> Result<(), DockerError>;
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
pub struct CliDocker;

impl CliDocker {
    pub fn new() -> Self {
        CliDocker
    }
}

#[async_trait]
impl DockerRuntime for CliDocker {
    async fn pin_source_image(
        &self,
        image: &str,
        registry_config: Option<&Path>,
    ) -> Result<String, DockerError> {
        source_docker(registry_config, &["pull", "--quiet", image], false).await?;
        let info = source_image_info(registry_config, image).await?;
        source_image_id(&info)
    }

    async fn build_source(&self, build: &SourceBuild) -> Result<String, DockerError> {
        if build.build_args.contains_key("BUILDKIT_SYNTAX") {
            return Err(source_refusal(
                "BUILDKIT_SYNTAX cannot replace the checked Dockerfile frontend",
            ));
        }
        let mut pinned = std::collections::BTreeMap::new();
        let mut base_shells = std::collections::BTreeMap::new();
        for image in &build.base_images {
            source_docker(
                build.registry_config.as_deref(),
                &["pull", "--quiet", image],
                false,
            )
            .await?;
            let info = source_image_info(build.registry_config.as_deref(), image).await?;
            if info.get("onbuild").is_some_and(|value| {
                !value.is_null() && value.as_array().is_none_or(|values| !values.is_empty())
            }) {
                return Err(source_refusal(
                    "base images with inherited ONBUILD instructions are refused",
                ));
            }
            let digest = info
                .get("digests")
                .and_then(serde_json::Value::as_array)
                .and_then(|values| values.first())
                .and_then(serde_json::Value::as_str)
                .filter(|value| value.contains("@sha256:"))
                .ok_or_else(|| source_refusal("base image has no immutable registry digest"))?;
            pinned.insert(image.clone(), digest.to_string());
            if let Some(shell) = info
                .get("shell")
                .and_then(serde_json::Value::as_array)
                .filter(|shell| !shell.is_empty())
            {
                let shell: Option<Vec<String>> = shell
                    .iter()
                    .map(|value| value.as_str().map(str::to_owned))
                    .collect();
                base_shells.insert(
                    digest.to_string(),
                    shell.ok_or_else(|| source_refusal("base image shell metadata is invalid"))?,
                );
            }
        }
        let text = std::fs::read_to_string(&build.dockerfile)
            .map_err(|_| source_refusal("Dockerfile cannot be read"))?;
        // The source parser refuses multiline FROM and custom frontends. Only
        // literal registry image tokens change; stage names remain intact.
        let mut text = text
            .lines()
            .map(|line| {
                let trimmed = line.trim_start();
                if trimmed
                    .split_whitespace()
                    .next()
                    .is_some_and(|word| word.eq_ignore_ascii_case("FROM"))
                {
                    let words: Vec<_> = trimmed.split_whitespace().collect();
                    if let Some(digest) = words.get(1).and_then(|image| pinned.get(*image)) {
                        let tail = if words.len() == 4 {
                            format!(" AS {}", words[3])
                        } else {
                            String::new()
                        };
                        return format!("FROM {digest}{tail}");
                    }
                }
                line.to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        let needs_guard = crate::source::dockerfile_instructions(&text)
            .map_err(|_| source_refusal("Dockerfile cannot be parsed"))?
            .iter()
            .any(|line| {
                line.split_whitespace()
                    .next()
                    .is_some_and(|word| word.eq_ignore_ascii_case("RUN"))
            });
        let guard_name = format!("sf_guard_{:032x}", rand::random::<u128>());
        let mut guard_context = None;
        let mut cleanup = SourceBuildFiles::default();
        if needs_guard {
            let architecture = source_docker(
                build.registry_config.as_deref(),
                &["info", "--format", "{{.OSType}}/{{.Architecture}}"],
                true,
            )
            .await?;
            let architecture = String::from_utf8_lossy(&architecture);
            let host = match std::env::consts::ARCH {
                "aarch64" => "aarch64",
                "x86_64" => "x86_64",
                _ => "unsupported",
            };
            let server = architecture
                .trim()
                .replace("/arm64", "/aarch64")
                .replace("/amd64", "/x86_64");
            if std::env::consts::OS != "linux"
                || server != format!("linux/{host}")
                || host == "unsupported"
            {
                return Err(source_refusal(
                    "Dockerfile RUN requires a Linux Host matching the Docker architecture",
                ));
            }
            let directory = build
                .registry_config
                .as_deref()
                .and_then(Path::parent)
                .or_else(|| build.context.parent())
                .ok_or_else(|| source_refusal("Dockerfile has no private directory"))?
                .join(format!(".sf-guard-{:032x}", rand::random::<u128>()));
            cleanup.directories.push(directory.clone());
            let launcher = crate::source_build_guard::compile(&directory).map_err(|_| {
                source_refusal(
                    "Dockerfile RUN requires /usr/bin/cc and static libc on the Linux Host",
                )
            })?;
            let context = directory.join("context");
            std::fs::create_dir(&context)
                .map_err(|_| source_refusal("trusted build context cannot be created"))?;
            std::fs::set_permissions(&context, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| source_refusal("trusted build context cannot be protected"))?;
            let guard_path = context.join(&guard_name);
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o555)
                .open(&guard_path)
                .map_err(|_| {
                    source_refusal("build guard filename collided with checkout content")
                })?;
            cleanup.files.push(guard_path.clone());
            file.write_all(
                &std::fs::read(launcher)
                    .map_err(|_| source_refusal("trusted build guard cannot be read"))?,
            )
            .map_err(|_| source_refusal("trusted build guard cannot be copied"))?;
            file.set_permissions(std::fs::Permissions::from_mode(0o555))
                .map_err(|_| source_refusal("trusted build guard cannot be protected"))?;
            let mut ignore = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(context.join(".dockerignore"))
                .map_err(|_| source_refusal("trusted context ignore rules cannot be written"))?;
            ignore
                .write_all(format!("*\n!/{guard_name}\n").as_bytes())
                .map_err(|_| source_refusal("trusted context ignore rules cannot be written"))?;
            guard_context = Some(context);
            text = crate::source::guard::dockerfile(&text, &guard_name, &base_shells)
                .map_err(|_| source_refusal("Dockerfile RUN or SHELL is unsupported"))?;
        }
        let prepared = build.dockerfile.with_file_name(format!(
            ".platform-dockerfile-{:032x}",
            rand::random::<u128>()
        ));
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&prepared)
            .map_err(|_| source_refusal("prepared Dockerfile cannot be written"))?;
        cleanup.files.push(prepared.clone());
        file.write_all(text.as_bytes())
            .map_err(|_| source_refusal("prepared Dockerfile cannot be written"))?;
        {
            let source_ignore = build.dockerfile.with_file_name(format!(
                "{}.dockerignore",
                build
                    .dockerfile
                    .file_name()
                    .ok_or_else(|| source_refusal("Dockerfile name is invalid"))?
                    .to_string_lossy()
            ));
            let source_ignore = if source_ignore.exists() {
                source_ignore
            } else {
                build.context.join(".dockerignore")
            };
            let ignore = if source_ignore.exists() {
                std::fs::read_to_string(source_ignore)
                    .map_err(|_| source_refusal("Docker context ignore rules cannot be read"))?
            } else {
                String::new()
            };
            let prepared_ignore = prepared.with_file_name(format!(
                "{}.dockerignore",
                prepared.file_name().unwrap().to_string_lossy()
            ));
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&prepared_ignore)
                .map_err(|_| {
                    source_refusal("prepared Docker context ignore rules cannot be written")
                })?;
            cleanup.files.push(prepared_ignore);
            file.write_all(ignore.as_bytes()).map_err(|_| {
                source_refusal("prepared Docker context ignore rules cannot be written")
            })?;
        }
        let mut args = vec![
            "build".to_owned(),
            "--progress=plain".into(),
            "--file".into(),
            prepared.to_string_lossy().into_owned(),
            "--tag".into(),
            build.tag.clone(),
        ];
        if let Some(context) = guard_context {
            args.extend([
                "--build-context".into(),
                format!("{guard_name}={}", context.display()),
            ]);
        }
        for (name, value) in &build.build_args {
            args.extend(["--build-arg".into(), format!("{name}={value}")]);
        }
        for (name, path) in &build.secret_files {
            args.extend([
                "--secret".into(),
                format!("id={name},src={}", path.display()),
            ]);
        }
        args.push(build.context.to_string_lossy().into_owned());
        let refs: Vec<_> = args.iter().map(String::as_str).collect();
        let result = source_docker(build.registry_config.as_deref(), &refs, false).await;
        result?;
        let info = source_image_info(build.registry_config.as_deref(), &build.tag).await?;
        if !info
            .get("user")
            .and_then(serde_json::Value::as_str)
            .is_some_and(crate::source::nonroot_user)
        {
            return Err(source_refusal(
                "built image must have an explicit numeric nonzero user",
            ));
        }
        source_image_id(&info)
    }
    async fn open_terminal(
        &self,
        container: &str,
        size: crate::terminal::Size,
        user: Option<&str>,
    ) -> Result<crate::terminal::Session, DockerError> {
        crate::terminal::open(container, size, user).await
    }

    async fn container_images(&self) -> Result<Vec<String>, DockerError> {
        let step = "failed to check images used by containers";
        let output = tokio::process::Command::new("docker")
            .args(["ps", "-a", "--format", "{{.Image}}"])
            .output()
            .await
            .map_err(|error| DockerError::spawn(step, error))?;
        if !output.status.success() {
            return Err(DockerError::refused(step, &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_owned)
            .collect())
    }

    async fn remove_image_repository(&self, repository: &str) -> Result<(), DockerError> {
        let step = format!("failed to remove image '{repository}'");
        let output = tokio::process::Command::new("docker")
            .args([
                "image",
                "ls",
                "--filter",
                &format!("reference={repository}:*"),
                "--format",
                "{{.Repository}}:{{.Tag}}",
            ])
            .output()
            .await
            .map_err(|error| DockerError::spawn(&step, error))?;
        if !output.status.success() {
            return Err(DockerError::refused(&step, &output));
        }
        let tags = String::from_utf8_lossy(&output.stdout);
        if tags.trim().is_empty() {
            return Ok(());
        }
        let output = tokio::process::Command::new("docker")
            .args(["image", "rm"])
            .args(tags.lines())
            .output()
            .await
            .map_err(|error| DockerError::spawn(&step, error))?;
        if !output.status.success() {
            return Err(DockerError::refused(&step, &output));
        }
        Ok(())
    }

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

    async fn image_id(&self, image: &str) -> Result<Option<String>, DockerError> {
        let output = std::process::Command::new("docker")
            .args(["image", "inspect", "-f", "{{.Id}}", image])
            .output()
            .map_err(|e| DockerError::spawn(format!("failed to inspect image '{image}'"), e))?;

        // An image the Host does not have yet is an answer, not a failure.
        if !output.status.success() {
            return Ok(None);
        }
        let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok((!id.is_empty()).then_some(id))
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

    async fn ensure_private_network(&self, name: &str) -> Result<(), DockerError> {
        let inspect = std::process::Command::new("docker")
            .args(["network", "inspect", "--format", "{{.Internal}}", name])
            .output()
            .map_err(|error| DockerError::spawn("failed to inspect private network", error))?;
        if inspect.status.success() {
            return if String::from_utf8_lossy(&inspect.stdout).trim() == "true" {
                Ok(())
            } else {
                Err(DockerError::Unavailable(format!(
                    "network '{name}' exists but is not internal"
                )))
            };
        }
        let create = std::process::Command::new("docker")
            .args(["network", "create", "--internal", name])
            .output()
            .map_err(|error| DockerError::spawn("failed to create private network", error))?;
        if !create.status.success() {
            return Err(DockerError::refused(
                "failed to create private network",
                &create,
            ));
        }
        Ok(())
    }

    async fn sync_private_networks(
        &self,
        container: &str,
        networks: &[String],
    ) -> Result<(), DockerError> {
        if networks
            .iter()
            .any(|network| !network.starts_with("sf-private-"))
        {
            return Err(DockerError::Unavailable(
                "private network synchronization requires Platform private networks".into(),
            ));
        }
        let inspect = std::process::Command::new("docker")
            .args([
                "inspect",
                "--type",
                "container",
                "--format",
                "{{json .NetworkSettings.Networks}}",
                container,
            ])
            .output()
            .map_err(|error| DockerError::spawn("failed to inspect container networks", error))?;
        if !inspect.status.success() {
            return Err(DockerError::refused(
                "failed to inspect container networks",
                &inspect,
            ));
        }
        let current: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(&inspect.stdout).map_err(|error| {
                DockerError::Command("failed to read container networks".into(), Box::new(error))
            })?;
        for network in current
            .keys()
            .filter(|name| name.starts_with("sf-private-") && !networks.contains(name))
        {
            let output = std::process::Command::new("docker")
                .args(["network", "disconnect", network, container])
                .output()
                .map_err(|error| {
                    DockerError::spawn("failed to revoke private network access", error)
                })?;
            if !output.status.success() {
                return Err(DockerError::refused(
                    "failed to revoke private network access",
                    &output,
                ));
            }
        }
        for network in networks.iter().filter(|name| !current.contains_key(*name)) {
            let output = std::process::Command::new("docker")
                .args(["network", "connect", network, container])
                .output()
                .map_err(|error| {
                    DockerError::spawn("failed to grant private network access", error)
                })?;
            if !output.status.success() {
                return Err(DockerError::refused(
                    "failed to grant private network access",
                    &output,
                ));
            }
        }
        Ok(())
    }

    async fn remove_network_if_exists(&self, name: &str) -> Result<bool, DockerError> {
        let inspect = std::process::Command::new("docker")
            .args(["network", "inspect", name])
            .output()
            .map_err(|e| DockerError::spawn("failed to inspect the Docker network", e))?;

        if !inspect.status.success() {
            return Ok(false);
        }

        let remove = std::process::Command::new("docker")
            .args(["network", "rm", name])
            .output()
            .map_err(|e| DockerError::spawn("failed to remove the Docker network", e))?;

        if !remove.status.success() {
            return Err(DockerError::refused(
                format!("failed to remove the Docker network '{name}'"),
                &remove,
            ));
        }

        Ok(true)
    }

    async fn pull_image(&self, image: &str) -> Result<(), DockerError> {
        if built_on_host(image) {
            let step =
                format!("custom image '{image}' is not available on this Host; build it again");
            let output = std::process::Command::new("docker")
                .args(["image", "inspect", image])
                .output()
                .map_err(|error| DockerError::spawn(step.clone(), error))?;
            return if output.status.success() {
                Ok(())
            } else {
                Err(DockerError::refused(step, &output))
            };
        }
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
        let output = tokio::process::Command::new("docker")
            .args(["build", "-t", tag, path])
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| DockerError::spawn(format!("failed to build image from '{path}'"), e))?;

        if !output.status.success() {
            return Err(DockerError::refused(
                format!("failed to build image from '{path}'"),
                &output,
            ));
        }

        Ok(())
    }

    async fn build_image_with_logs(
        &self,
        path: &str,
        tag: &str,
        logs: mpsc::Sender<String>,
    ) -> Result<(), DockerError> {
        use std::process::Stdio;
        use tokio::io::AsyncReadExt;

        let step = format!("failed to build image '{tag}'");
        let mut child = tokio::process::Command::new("docker")
            .args(["build", "--progress=plain", "-t", tag, path])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| DockerError::spawn(&step, error))?;
        async fn forward(
            mut reader: impl tokio::io::AsyncRead + Unpin,
            logs: mpsc::Sender<String>,
        ) -> std::io::Result<()> {
            let mut buffer = [0; 4096];
            loop {
                let count = reader.read(&mut buffer).await?;
                if count == 0 {
                    return Ok(());
                }
                let _ = logs
                    .send(String::from_utf8_lossy(&buffer[..count]).into_owned())
                    .await;
            }
        }
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (_, _, status) = tokio::try_join!(
            forward(stdout, logs.clone()),
            forward(stderr, logs),
            child.wait()
        )
        .map_err(|error| DockerError::spawn(&step, error))?;
        if !status.success() {
            return Err(DockerError::Command(
                step,
                Box::new(std::io::Error::other(format!(
                    "Docker exited with {status}. See the build log."
                ))),
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

        for network in &config.additional_networks {
            args.extend(["--network".into(), network.clone()]);
        }

        if config
            .labels
            .iter()
            .any(|(key, value)| key == "sf.source" && value == "git")
        {
            args.extend([
                "--security-opt".into(),
                "no-new-privileges:true".into(),
                "--cap-drop".into(),
                "ALL".into(),
            ]);
        }

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
                "{{.State.Status}} {{.State.ExitCode}} {{.RestartCount}} {{if .State.Health}}{{.State.Health.Status}}{{end}}",
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
            health: parts.next().map(str::to_owned),
        }))
    }

    async fn container_stats(&self) -> Result<Vec<ContainerStats>, DockerError> {
        // Two calls, not one: `docker stats` does not print labels, and the
        // join key is the container name both outputs share.
        let stats = std::process::Command::new("docker")
            .args(["stats", "--no-stream", "--format", "{{json .}}"])
            .output()
            .map_err(|e| DockerError::spawn("failed to sample Application stats", e))?;
        if !stats.status.success() {
            return Err(DockerError::refused(
                "failed to sample Application stats",
                &stats,
            ));
        }
        let owners = std::process::Command::new("docker")
            .args([
                "ps",
                "--filter",
                "label=sf.app.id",
                "--format",
                "{{.Names}}\t{{.Label \"sf.app.id\"}}",
            ])
            .output()
            .map_err(|e| DockerError::spawn("failed to list Application containers", e))?;
        if !owners.status.success() {
            return Err(DockerError::refused(
                "failed to list Application containers",
                &owners,
            ));
        }
        let owners: Vec<(String, String)> = String::from_utf8_lossy(&owners.stdout)
            .lines()
            .filter_map(|line| line.split_once('\t').map(|(n, id)| (n.into(), id.into())))
            .collect();

        let mut result = Vec::new();
        for line in String::from_utf8_lossy(&stats.stdout).lines() {
            let Some(raw) = parse_stats_line(line) else {
                continue;
            };
            let Some(application) = owners.iter().find(|(name, _)| *name == raw.name) else {
                continue;
            };
            result.push(ContainerStats {
                container: raw.name,
                application: application.1.clone(),
                cpu_percent: raw.cpu_percent,
                memory_bytes: raw.memory_bytes,
                memory_limit_bytes: raw.memory_limit_bytes,
                rx_bytes: raw.rx_bytes,
                tx_bytes: raw.tx_bytes,
            });
        }
        Ok(result)
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

    async fn compose_recreate(&self, project: &ComposeProject) -> Result<(), DockerError> {
        compose(
            project,
            &["up", "-d", "--force-recreate", "--remove-orphans"],
            "failed to recreate the Application",
        )
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

/// Images the Platform builds itself: custom images and development images.
/// They have no registry to be pulled from.
pub fn built_on_host(image: &str) -> bool {
    image.starts_with("sf-img-")
        || image.starts_with("self-host-dev-")
        || image.starts_with("sha256:")
        || image.starts_with("sf-source-")
}

/// Runs one `docker` command and reports a refusal in Docker's words.
fn source_refusal(reason: &str) -> DockerError {
    DockerError::Command(
        "Git build runtime refused the candidate".into(),
        Box::new(std::io::Error::other(reason.to_string())),
    )
}

#[derive(Default)]
struct SourceBuildFiles {
    files: Vec<std::path::PathBuf>,
    directories: Vec<std::path::PathBuf>,
}
impl Drop for SourceBuildFiles {
    fn drop(&mut self) {
        for file in &self.files {
            let _ = std::fs::remove_file(file);
        }
        for directory in &self.directories {
            let _ = std::fs::remove_dir_all(directory);
        }
    }
}

fn fake_source_id(value: &str) -> String {
    use sha2::Digest;
    format!("sha256:{:x}", sha2::Sha256::digest(value.as_bytes()))
}

async fn source_docker(
    registry_config: Option<&Path>,
    args: &[&str],
    capture: bool,
) -> Result<Vec<u8>, DockerError> {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;
    let mut command = tokio::process::Command::new("docker");
    command
        .args(args)
        .env("DOCKER_BUILDKIT", "1")
        .env_remove("BUILDKIT_SYNTAX")
        .stderr(Stdio::null())
        .stdout(if capture {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .kill_on_drop(true);
    if let Some(path) = registry_config {
        command.env("DOCKER_CONFIG", path);
    }
    let mut child = command
        .spawn()
        .map_err(|_| source_refusal("Docker could not start"))?;
    tokio::time::timeout(std::time::Duration::from_secs(600), async {
        let mut output = Vec::new();
        if let Some(reader) = child.stdout.take() {
            reader
                .take(65537)
                .read_to_end(&mut output)
                .await
                .map_err(|_| source_refusal("Docker metadata could not be read"))?;
        }
        if output.len() > 65536 {
            return Err(source_refusal("Docker metadata exceeded the size limit"));
        }
        if !child
            .wait()
            .await
            .map_err(|_| source_refusal("Docker did not finish"))?
            .success()
        {
            return Err(source_refusal(
                "Docker command failed; repository output is withheld to protect credentials",
            ));
        }
        Ok(output)
    })
    .await
    .map_err(|_| source_refusal("Docker source build exceeded its ten-minute limit"))?
}

async fn source_image_info(
    registry_config: Option<&Path>,
    image: &str,
) -> Result<serde_json::Value, DockerError> {
    let output = source_docker(registry_config, &["image", "inspect", "--format", "{\"onbuild\":{{json (index .Config \"OnBuild\")}},\"digests\":{{json .RepoDigests}},\"id\":{{json .Id}},\"user\":{{json (index .Config \"User\")}},\"shell\":{{json (index .Config \"Shell\")}}}", image], true).await?;
    serde_json::from_slice(&output).map_err(|_| source_refusal("Docker image metadata is invalid"))
}

fn source_image_id(info: &serde_json::Value) -> Result<String, DockerError> {
    info.get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| {
            id.len() == 71
                && id.starts_with("sha256:")
                && id[7..].bytes().all(|c| c.is_ascii_hexdigit())
        })
        .map(str::to_owned)
        .ok_or_else(|| source_refusal("Docker did not report an immutable image id"))
}

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

/// One line of `docker stats --format '{{json .}}'`, taken apart. The values
/// arrive as strings with units, the way Docker prints them for people.
struct RawStats {
    name: String,
    cpu_percent: f64,
    memory_bytes: u64,
    memory_limit_bytes: u64,
    rx_bytes: u64,
    tx_bytes: u64,
}

fn parse_stats_line(line: &str) -> Option<RawStats> {
    #[derive(serde::Deserialize)]
    struct StatsLine {
        #[serde(rename = "Name", default)]
        name: String,
        #[serde(rename = "CPUPerc", default)]
        cpu_percent: String,
        #[serde(rename = "MemUsage", default)]
        mem_usage: String,
        #[serde(rename = "NetIO", default)]
        net_io: String,
    }
    let parsed: StatsLine = serde_json::from_str(line).ok()?;
    if parsed.name.is_empty() {
        return None;
    }
    let (memory, limit) = parsed.mem_usage.split_once(" / ")?;
    let (rx, tx) = parsed.net_io.split_once(" / ")?;
    Some(RawStats {
        name: parsed.name,
        cpu_percent: parsed
            .cpu_percent
            .trim_end_matches('%')
            .trim()
            .parse()
            .ok()?,
        memory_bytes: parse_size(memory)?,
        memory_limit_bytes: parse_size(limit)?,
        rx_bytes: parse_size(rx)?,
        tx_bytes: parse_size(tx)?,
    })
}

/// A size as `docker stats` prints it: `58.3MiB`, `1.2kB`, `648B`. The units
/// with an `i` are 1024-based (memory); the others are 1000-based (network).
/// A value that does not read is absent, not zero: zero is an answer about a
/// container, and unreadable is an answer about the output.
fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let (number, unit) = text.split_at(split);
    let value: f64 = number.parse().ok()?;
    let base: f64 = if unit.contains('i') { 1024.0 } else { 1000.0 };
    let multiplier = match unit.trim_end_matches("iB").trim_end_matches('B') {
        "" => 1.0,
        "K" | "k" => base,
        "M" => base.powi(2),
        "G" => base.powi(3),
        "T" => base.powi(4),
        _ => return None,
    };
    Some((value * multiplier) as u64)
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

/// Writes the project and prepares bind sources without changing existing
/// files into directories. Docker never creates a missing source itself.
fn write_project(project: &ComposeProject) -> Result<(), DockerError> {
    let io = |what: &str, e: std::io::Error| {
        DockerError::Command(
            format!("failed to write the Application project: {what}"),
            Box::new(e),
        )
    };
    std::fs::create_dir_all(&project.dir).map_err(|e| io("create project directory", e))?;
    for mount in &project.bind_mounts {
        mount.prepare().map_err(|e| io("prepare bind source", e))?;
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

/// The stats of a running Application container, for a runtime with nothing
/// to measure: fixed values an aggregation can be asserted against.
fn fake_stats(container: &str, application: &str) -> ContainerStats {
    ContainerStats {
        container: container.into(),
        application: application.into(),
        cpu_percent: 1.25,
        memory_bytes: 32 * 1024 * 1024,
        memory_limit_bytes: 4 * 1024 * 1024 * 1024,
        rx_bytes: 1024,
        tx_bytes: 2048,
    }
}

pub type RecordedTerminal = (String, crate::terminal::Size, Option<String>);
pub type RecordedNetworkSync = (String, Vec<String>);

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
    pub terminals: std::sync::Arc<std::sync::Mutex<Vec<RecordedTerminal>>>,
    /// The id each image has on the Host, by reference.
    pub images: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
    /// The id a pull would fetch, by reference: what the registry has now.
    pub registry: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
    /// Compose projects recreated, by name.
    pub recreated: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    pub network_syncs: std::sync::Arc<std::sync::Mutex<Vec<RecordedNetworkSync>>>,
    pub health: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
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
            terminals: Default::default(),
            images: Default::default(),
            registry: Default::default(),
            recreated: Default::default(),
            network_syncs: Default::default(),
            health: Default::default(),
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
    async fn build_source(&self, build: &SourceBuild) -> Result<String, DockerError> {
        self.build_image(&build.context.to_string_lossy(), &build.tag)
            .await?;
        Ok(fake_source_id(&build.tag))
    }
    async fn pin_source_image(
        &self,
        image: &str,
        _registry_config: Option<&Path>,
    ) -> Result<String, DockerError> {
        self.pull_image(image).await?;
        Ok(fake_source_id(image))
    }
    async fn open_terminal(
        &self,
        container: &str,
        size: crate::terminal::Size,
        user: Option<&str>,
    ) -> Result<crate::terminal::Session, DockerError> {
        self.ping().await?;
        self.terminals
            .lock()
            .unwrap()
            .push((container.into(), size, user.map(str::to_owned)));
        Ok(crate::terminal::Session::fake())
    }

    async fn container_images(&self) -> Result<Vec<String>, DockerError> {
        self.ping().await?;
        Ok(self
            .apps
            .lock()
            .unwrap()
            .iter()
            .map(|app| app.image.clone())
            .collect())
    }

    async fn remove_image_repository(&self, repository: &str) -> Result<(), DockerError> {
        self.built
            .lock()
            .unwrap()
            .retain(|(_, tag)| !tag.starts_with(&format!("{repository}:")));
        Ok(())
    }

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

    async fn ensure_private_network(&self, _name: &str) -> Result<(), DockerError> {
        Ok(())
    }

    async fn sync_private_networks(
        &self,
        container: &str,
        networks: &[String],
    ) -> Result<(), DockerError> {
        self.network_syncs
            .lock()
            .unwrap()
            .push((container.into(), networks.to_vec()));
        if let Some(application) = self
            .apps
            .lock()
            .unwrap()
            .iter_mut()
            .find(|application| application.name == container)
        {
            application.additional_networks = networks.to_vec();
        }
        Ok(())
    }

    async fn remove_network_if_exists(&self, _name: &str) -> Result<bool, DockerError> {
        Ok(false)
    }

    async fn pull_image(&self, image: &str) -> Result<(), DockerError> {
        if let Some(message) = &self.pull_failure {
            return Err(DockerError::Command(
                format!("failed to pull image '{image}'"),
                Box::new(DockerRefusal::parse(message).expect("a pull failure says something")),
            ));
        }
        self.pulled.lock().unwrap().push(image.to_string());
        if let Some(id) = self.registry.lock().unwrap().get(image) {
            self.images
                .lock()
                .unwrap()
                .insert(image.to_string(), id.clone());
        }
        Ok(())
    }

    async fn image_id(&self, image: &str) -> Result<Option<String>, DockerError> {
        Ok(self.images.lock().unwrap().get(image).cloned())
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
                health: self.health.lock().unwrap().get(name).cloned(),
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
            health: self.health.lock().unwrap().get(name).cloned(),
        }))
    }

    async fn container_stats(&self) -> Result<Vec<ContainerStats>, DockerError> {
        let stopped = self.stopped.lock().unwrap();
        let mut stats = Vec::new();
        for container in self.apps.lock().unwrap().iter() {
            if stopped.contains(&container.name) {
                continue;
            }
            if let Some((_, application)) =
                container.labels.iter().find(|(key, _)| key == "sf.app.id")
            {
                stats.push(fake_stats(&container.name, application));
            }
        }
        for project in self.projects.lock().unwrap().iter() {
            // The project name is the container prefix: `sf-app-<id>`.
            let Some(application) = project.name.strip_prefix(crate::apps::APP_PREFIX) else {
                continue;
            };
            for (_, container) in &project.containers {
                if !stopped.contains(container) {
                    stats.push(fake_stats(container, application));
                }
            }
        }
        Ok(stats)
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

    async fn compose_recreate(&self, project: &ComposeProject) -> Result<(), DockerError> {
        self.recreated.lock().unwrap().push(project.name.clone());
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
    async fn build_source(&self, build: &SourceBuild) -> Result<String, DockerError> {
        (**self).build_source(build).await
    }
    async fn pin_source_image(
        &self,
        image: &str,
        registry_config: Option<&Path>,
    ) -> Result<String, DockerError> {
        (**self).pin_source_image(image, registry_config).await
    }
    async fn open_terminal(
        &self,
        container: &str,
        size: crate::terminal::Size,
        user: Option<&str>,
    ) -> Result<crate::terminal::Session, DockerError> {
        (**self).open_terminal(container, size, user).await
    }

    async fn container_images(&self) -> Result<Vec<String>, DockerError> {
        (**self).container_images().await
    }

    async fn remove_image_repository(&self, repository: &str) -> Result<(), DockerError> {
        (**self).remove_image_repository(repository).await
    }

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

    async fn ensure_private_network(&self, name: &str) -> Result<(), DockerError> {
        (**self).ensure_private_network(name).await
    }

    async fn sync_private_networks(
        &self,
        container: &str,
        networks: &[String],
    ) -> Result<(), DockerError> {
        (**self).sync_private_networks(container, networks).await
    }

    async fn remove_network_if_exists(&self, name: &str) -> Result<bool, DockerError> {
        (**self).remove_network_if_exists(name).await
    }

    async fn pull_image(&self, image: &str) -> Result<(), DockerError> {
        (**self).pull_image(image).await
    }

    async fn image_id(&self, image: &str) -> Result<Option<String>, DockerError> {
        (**self).image_id(image).await
    }

    async fn build_image(&self, path: &str, tag: &str) -> Result<(), DockerError> {
        (**self).build_image(path, tag).await
    }

    async fn build_image_with_logs(
        &self,
        path: &str,
        tag: &str,
        logs: mpsc::Sender<String>,
    ) -> Result<(), DockerError> {
        (**self).build_image_with_logs(path, tag, logs).await
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

    async fn container_stats(&self) -> Result<Vec<ContainerStats>, DockerError> {
        (**self).container_stats().await
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

    async fn compose_recreate(&self, project: &ComposeProject) -> Result<(), DockerError> {
        (**self).compose_recreate(project).await
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

    #[test]
    fn sizes_read_the_way_docker_prints_them() {
        // Memory is 1024-based with the `i`, network is 1000-based without.
        assert_eq!(parse_size("58.3MiB"), Some(61_131_980));
        assert_eq!(parse_size("3.842GiB"), Some(4_125_316_087));
        assert_eq!(parse_size("1.2kB"), Some(1_200));
        assert_eq!(parse_size("648B"), Some(648));
        assert_eq!(parse_size("0B"), Some(0));
        // A value that does not read is absent, not zero.
        assert_eq!(parse_size("--"), None);
        assert_eq!(parse_size(""), None);
    }

    #[test]
    fn a_stats_line_comes_apart_into_numbers() {
        let line = r#"{"BlockIO":"0B / 0B","CPUPerc":"0.07%","Container":"x","ID":"x","MemPec":"1.42%","MemUsage":"58.3MiB / 3.842GiB","Name":"sf-app-k3n8qz4v2x1p-web-1","NetIO":"1.2kB / 648B","PIDs":"5"}"#;
        let raw = parse_stats_line(line).unwrap();
        assert_eq!(raw.name, "sf-app-k3n8qz4v2x1p-web-1");
        assert_eq!(raw.cpu_percent, 0.07);
        assert_eq!(raw.memory_bytes, 61_131_980);
        assert_eq!(raw.memory_limit_bytes, 4_125_316_087);
        assert_eq!(raw.rx_bytes, 1_200);
        assert_eq!(raw.tx_bytes, 648);

        // Not JSON, or JSON without a name: nothing to read.
        assert!(parse_stats_line("not json").is_none());
        assert!(parse_stats_line(r#"{"CPUPerc":"1%"}"#).is_none());
    }
}
