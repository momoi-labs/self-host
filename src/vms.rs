//! Native development environments on a configured Incus server.
//!
//! macOS uses an Incus server inside Colima; Linux can run Incus directly.
//! Only instances carrying this environment's ownership marker are managed.

use std::{net::Ipv4Addr, process::Stdio, time::Duration};

use async_trait::async_trait;
use rand::Rng;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::mpsc,
};

use crate::environments::{RunnerObservation, RuntimeProgress, VmConfig, VmRuntime, VmState};

const BOOTSTRAP: &str = include_str!("vms/bootstrap.sh");
const OWNER: &str = "user.self-host.environment-id";

pub struct IncusRuntime {
    executable: String,
    remote: String,
    address: String,
    gateway: Option<String>,
    pool: Option<String>,
}

impl Default for IncusRuntime {
    fn default() -> Self {
        let executable = std::env::var("SELF_HOST_INCUS_BIN").unwrap_or_else(|_| {
            if cfg!(target_os = "macos") && std::path::Path::new("/opt/homebrew/bin/incus").exists()
            {
                "/opt/homebrew/bin/incus".into()
            } else {
                "incus".into()
            }
        });
        Self {
            executable,
            remote: std::env::var("SELF_HOST_INCUS_REMOTE").unwrap_or_else(|_| "local".into()),
            address: std::env::var("SELF_HOST_INCUS_ADDRESS")
                .unwrap_or_else(|_| "127.0.0.1".into()),
            gateway: std::env::var("SELF_HOST_ENVIRONMENT_HOST").ok(),
            pool: std::env::var("SELF_HOST_INCUS_POOL").ok(),
        }
    }
}

fn instance_name(id: &str) -> Result<String, String> {
    if id.is_empty()
        || id.len() > 48
        || !id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        return Err("Invalid environment identifier.".into());
    }
    Ok(format!("sf-dev-{id}"))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

impl IncusRuntime {
    fn target(&self, id: &str) -> Result<String, String> {
        if self.remote.is_empty()
            || !self
                .remote
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err("SELF_HOST_INCUS_REMOTE must name a configured Incus remote.".into());
        }
        self.address.parse::<Ipv4Addr>().map_err(|_| {
            "SELF_HOST_INCUS_ADDRESS must be the Incus server's reachable IPv4 address.".to_string()
        })?;
        Ok(format!("{}:{}", self.remote, instance_name(id)?))
    }

    async fn command(&self, args: &[&str], timeout: Duration) -> Result<String, String> {
        let output = tokio::time::timeout(timeout, Command::new(&self.executable)
            .env("INCUS_REMOTE", &self.remote)
            .args(args).stdin(Stdio::null()).kill_on_drop(true).output()).await
            .map_err(|_| "Incus operation timed out. Inspect the environment before retrying.".to_string())?
            .map_err(|e| format!("Cannot run Incus: {e}. Install Incus and configure its remote for the Platform's account."))?;
        if !output.status.success() {
            // CLI diagnostics for lifecycle commands contain no bootstrap output.
            let message = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "Incus: {}",
                message.chars().take(1500).collect::<String>().trim()
            ));
        }
        if output.stdout.len() > 2 * 1024 * 1024 {
            return Err("Incus response is too large.".into());
        }
        String::from_utf8(output.stdout).map_err(|_| "Incus returned invalid UTF-8.".into())
    }

    async fn instance(&self, id: &str) -> Result<Option<Value>, String> {
        self.target(id)?;
        let remote = format!("{}:", self.remote);
        let filter = format!("^{}$", instance_name(id)?);
        let text = self
            .command(
                &["list", &remote, &filter, "--format=json"],
                Duration::from_secs(15),
            )
            .await?;
        let instances: Vec<Value> = serde_json::from_str(&text)
            .map_err(|_| "Incus returned invalid instance data.".to_string())?;
        let name = instance_name(id)?;
        let instance = instances
            .into_iter()
            .find(|value| value["name"].as_str() == Some(&name));
        if let Some(ref instance) = instance
            && instance["config"][OWNER].as_str() != Some(id)
        {
            return Err("The matching Incus instance is not owned by this environment. No action was taken.".into());
        }
        Ok(instance)
    }

    async fn set(&self, target: &str, key: &str, value: &str) -> Result<(), String> {
        self.command(
            &["config", "set", target, key, value],
            Duration::from_secs(20),
        )
        .await
        .map(|_| ())
    }

    async fn proxy(
        &self,
        target: &str,
        name: &str,
        guest_port: u16,
        existing: Option<u16>,
    ) -> Result<u16, String> {
        let connect = format!("connect=tcp:127.0.0.1:{guest_port}");
        if let Some(port) = existing {
            self.command(
                &["config", "device", "set", target, name, &connect],
                Duration::from_secs(20),
            )
            .await?;
            return Ok(port);
        }
        for _ in 0..5 {
            let port = rand::rng().random_range(40000..60000);
            let listen = format!("listen=tcp:{}:{port}", self.address);
            match self
                .command(
                    &[
                        "config", "device", "add", target, name, "proxy", &listen, &connect,
                    ],
                    Duration::from_secs(20),
                )
                .await
            {
                Ok(_) => return Ok(port),
                Err(error) if error.contains("address already in use") => continue,
                Err(error) => return Err(error),
            }
        }
        Err("Could not allocate an environment access port.".into())
    }

    async fn access(&self, id: &str, config: &VmConfig) -> Result<(), String> {
        let target = self.target(id)?;
        let instance = self.instance(id).await?.ok_or("Environment is missing.")?;
        let existing = |name: &str| -> Option<u16> {
            instance["devices"][name]["listen"]
                .as_str()?
                .rsplit(':')
                .next()?
                .parse()
                .ok()
        };
        let ssh = self
            .proxy(&target, "self-host-ssh", 22, existing("self-host-ssh"))
            .await?;
        let web = self
            .proxy(
                &target,
                "self-host-web",
                config.web_port,
                existing("self-host-web"),
            )
            .await?;
        self.set(&target, "user.self-host.ssh-port", &ssh.to_string())
            .await?;
        self.set(&target, "user.self-host.host-web-port", &web.to_string())
            .await?;
        self.set(
            &target,
            "user.self-host.web-port",
            &config.web_port.to_string(),
        )
        .await
    }

    async fn bootstrap(
        &self,
        id: &str,
        config: &VmConfig,
        progress: &mpsc::UnboundedSender<RuntimeProgress>,
    ) -> Result<(), String> {
        let target = self.target(id)?;
        let payload = bootstrap_payload(config);
        // The guest lock and timeout also bound a process whose client disconnects.
        let mut child = Command::new(&self.executable)
            .env("INCUS_REMOTE", &self.remote)
            .args([
                "exec",
                &target,
                "--mode=non-interactive",
                "--",
                "flock",
                "-n",
                "/run/self-host-environment-bootstrap.lock",
                "timeout",
                "--kill-after=10s",
                "20m",
                "bash",
                "-s",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("Cannot start bootstrap: {e}"))?;
        let mut input = child.stdin.take().unwrap();
        let write = tokio::spawn(async move { input.write_all(payload.as_bytes()).await });
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut stderr = child.stderr.take().unwrap();
        let drain = tokio::spawn(async move {
            let mut buffer = [0u8; 4096];
            while stderr.read(&mut buffer).await.unwrap_or(0) > 0 {}
        });
        let wait = async {
            while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
                if let Some(step) = line.strip_prefix("SF_STEP ")
                    && step.len() < 80
                    && step
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b"-:".contains(&b))
                {
                    let _ = progress.send(RuntimeProgress {
                        step: Some(step.into()),
                        log: format!("{step}\n"),
                    });
                }
            }
            child.wait().await.map_err(|e| e.to_string())
        };
        let status = tokio::time::timeout(Duration::from_secs(1230), wait).await
            .map_err(|_| "Bootstrap timed out. The guest stops it after 20 minutes; retry once it has stopped.".to_string())??;
        let _ = write.await;
        let _ = drain.await;
        if !status.success() {
            return Err("Bootstrap failed. Inspect /var/log/self-host-environment-bootstrap.log inside the environment. Existing tools may have partially changed; personal data is preserved.".into());
        }
        self.set(&target, "user.self-host.bootstrapped", "true")
            .await
    }

    async fn run(
        &self,
        id: &str,
        action: &str,
        config: &VmConfig,
        progress: mpsc::UnboundedSender<RuntimeProgress>,
    ) -> Result<RunnerObservation, String> {
        let target = self.target(id)?;
        let mut instance = self.instance(id).await?;
        let stage = |step: &str| {
            let _ = progress.send(RuntimeProgress {
                step: Some(step.into()),
                log: format!("{step}\n"),
            });
        };
        if action == "delete" {
            if instance.is_some() {
                stage("deleting");
                self.command(&["delete", &target, "--force"], Duration::from_secs(120))
                    .await?;
            }
            if self.instance(id).await?.is_some() {
                return Err("Incus still reports the environment after deletion.".into());
            }
            return Ok(RunnerObservation {
                state: VmState::Missing,
                ..Default::default()
            });
        }
        if action == "create" && instance.is_none() {
            stage("creating");
            let cpu = format!("limits.cpu={}", config.cpus);
            let memory = format!("limits.memory={}GiB", config.memory_gib);
            let owner = format!("{OWNER}={id}");
            let mut args = vec![
                "init",
                "images:ubuntu/24.04",
                &target,
                "-c",
                &cpu,
                "-c",
                &memory,
                "-c",
                &owner,
            ];
            if let Some(pool) = &self.pool {
                args.extend(["--storage", pool]);
            }
            self.command(&args, Duration::from_secs(600)).await?;
            instance = self.instance(id).await?;
        }
        let instance = instance.ok_or("Environment is missing. Retry creation first.")?;
        let running = instance["status"].as_str() == Some("Running");
        match action {
            "create" => {
                let size = format!("size={}GiB", config.disk_gib);
                if instance["devices"]["root"].is_object() {
                    self.command(
                        &["config", "device", "set", &target, "root", &size],
                        Duration::from_secs(20),
                    )
                    .await?;
                } else {
                    self.command(
                        &["config", "device", "override", &target, "root", &size],
                        Duration::from_secs(20),
                    )
                    .await?;
                }
                if !running {
                    self.command(&["start", &target], Duration::from_secs(120))
                        .await?;
                }
                self.access(id, config).await?;
                self.bootstrap(id, config, &progress).await?;
            }
            "bootstrap" | "update" => {
                if !running {
                    return Err("Start the environment before applying tools or bootstrap.".into());
                }
                self.bootstrap(id, config, &progress).await?;
                self.access(id, config).await?;
            }
            "start" => {
                if !running {
                    self.command(&["start", &target], Duration::from_secs(120))
                        .await?;
                }
            }
            "stop" => {
                if running {
                    self.command(&["stop", &target, "--timeout=60"], Duration::from_secs(90))
                        .await?;
                }
            }
            "restart" => {
                self.command(
                    &["restart", &target, "--timeout=60"],
                    Duration::from_secs(120),
                )
                .await?;
            }
            _ => return Err("Unknown environment action.".into()),
        }
        if matches!(action, "start" | "restart") {
            for _ in 0..20 {
                let observation = self.inspect(id).await?;
                if observation.service_ready {
                    return Ok(observation);
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
        self.inspect(id).await
    }
}

fn bootstrap_payload(config: &VmConfig) -> String {
    // These are data assignments. Only the service command runs, as dev.
    let encode = |value: &str| -> String {
        const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut result = String::new();
        for c in value.as_bytes().chunks(3) {
            let n = ((c[0] as u32) << 16)
                | ((c.get(1).copied().unwrap_or(0) as u32) << 8)
                | c.get(2).copied().unwrap_or(0) as u32;
            result.push(TABLE[(n >> 18) as usize] as char);
            result.push(TABLE[((n >> 12) & 63) as usize] as char);
            result.push(if c.len() > 1 {
                TABLE[((n >> 6) & 63) as usize] as char
            } else {
                '='
            });
            result.push(if c.len() > 2 {
                TABLE[(n & 63) as usize] as char
            } else {
                '='
            });
        }
        result
    };
    format!(
        "export SELF_HOST_SSH_PUBLIC_KEY_B64={}\nexport SELF_HOST_MISE_TOML_B64={}\nexport SELF_HOST_COMMAND_B64={}\nexport SELF_HOST_WEB_PORT={}\n{}",
        shell_quote(&encode(&config.ssh_public_key)),
        shell_quote(&encode(&config.recipe.mise_toml())),
        shell_quote(&encode(&config.command)),
        config.web_port,
        BOOTSTRAP
    )
}

#[async_trait]
impl VmRuntime for IncusRuntime {
    async fn inspect(&self, id: &str) -> Result<RunnerObservation, String> {
        let Some(instance) = self.instance(id).await? else {
            return Ok(RunnerObservation {
                state: VmState::Missing,
                ..Default::default()
            });
        };
        let target = self.target(id)?;
        let state = match instance["status"].as_str() {
            Some("Running") => VmState::Running,
            Some("Stopped") => VmState::Stopped,
            _ => VmState::Unknown,
        };
        let port = |key: &str| {
            instance["config"][key]
                .as_str()
                .and_then(|s| s.parse::<u16>().ok())
        };
        let web_port = port("user.self-host.web-port");
        let host_web_port = port("user.self-host.host-web-port");
        let ssh_port = port("user.self-host.ssh-port");
        let mut ready = false;
        let mut versions = None;
        if state == VmState::Running
            && instance["config"]["user.self-host.bootstrapped"].as_str() == Some("true")
        {
            if let Some(port) = web_port {
                let url = format!("http://127.0.0.1:{port}/");
                ready = self
                    .command(
                        &[
                            "exec",
                            &target,
                            "--",
                            "curl",
                            "-fsS",
                            "--max-time",
                            "2",
                            "-o",
                            "/dev/null",
                            &url,
                        ],
                        Duration::from_secs(5),
                    )
                    .await
                    .is_ok();
            }
            if let Ok(text) = self
                .command(
                    &[
                        "exec",
                        &target,
                        "--",
                        "cat",
                        "/etc/self-host-environment/versions.json",
                    ],
                    Duration::from_secs(5),
                )
                .await
            {
                versions = serde_json::from_str(&text).ok();
            }
        }
        let ssh_command = ssh_port.map(|port| match &self.gateway {
            Some(host) => format!(
                "ssh -J {} -p {port} dev@{}",
                shell_quote(host),
                self.address
            ),
            None => format!("ssh -p {port} dev@{}", self.address),
        });
        let tunnel_command = host_web_port.and_then(|port| {
            self.gateway.as_ref().map(|host| {
                format!(
                    "ssh -N -L 127.0.0.1:{port}:{}:{port} {}",
                    self.address,
                    shell_quote(host)
                )
            })
        });
        Ok(RunnerObservation {
            state,
            service_ready: ready,
            step: None,
            log: String::new(),
            ssh_command,
            web_url: host_web_port.map(|port| {
                format!(
                    "http://{}:{port}",
                    if self.gateway.is_some() {
                        "localhost"
                    } else {
                        &self.address
                    }
                )
            }),
            tunnel_command,
            installed_versions: versions,
            base_image: instance["config"]["volatile.base_image"]
                .as_str()
                .map(str::to_owned),
        })
    }

    async fn execute(
        &self,
        id: &str,
        action: &str,
        config: &VmConfig,
    ) -> Result<RunnerObservation, String> {
        let (progress, _receiver) = mpsc::unbounded_channel();
        self.run(id, action, config, progress).await
    }

    async fn execute_with_progress(
        &self,
        id: &str,
        action: &str,
        config: &VmConfig,
        progress: mpsc::UnboundedSender<RuntimeProgress>,
    ) -> Result<RunnerObservation, String> {
        self.run(id, action, config, progress).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dev_images::{Dependency, Recipe};
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    fn fake_runtime(mode: &str) -> (IncusRuntime, PathBuf, PathBuf) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "self-host-vms-test-{}-{timestamp}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let log = root.join("calls");
        let script = root.join("incus");
        let list = match mode {
            "foreign" => {
                r#"printf '%s' '[{"name":"sf-dev-env","status":"Stopped","config":{"user.self-host.environment-id":"other"}}]'"#
            }
            "owned" => {
                r#"printf '%s' '[{"name":"sf-dev-env","status":"Stopped","config":{"user.self-host.environment-id":"env"}}]'"#
            }
            "unreachable" => "exit 1",
            _ => panic!("unknown fake mode"),
        };
        let body = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\ncase \"$1\" in\nlist) {};;\nexec) printf 'SF_STEP ready\\n';;\n*) :;;\nesac\n",
            shell_quote(log.to_str().unwrap()),
            list
        );
        fs::write(&script, body).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        (
            IncusRuntime {
                executable: script.to_string_lossy().into_owned(),
                remote: "test".into(),
                address: "127.0.0.1".into(),
                gateway: None,
                pool: None,
            },
            root,
            log,
        )
    }

    fn test_config() -> VmConfig {
        VmConfig {
            name: "env".into(),
            cpus: 2,
            memory_gib: 4,
            disk_gib: 20,
            ssh_public_key: "ssh-ed25519 AAAA test".into(),
            recipe: Recipe {
                name: "test".into(),
                template_id: Some("t3-code".into()),
                dependencies: vec![Dependency {
                    tool: "node".into(),
                    version: "22".into(),
                    allow_builds: vec![],
                }],
                build_checks: vec![],
            },
            command: "printf ready".into(),
            web_port: 18080,
        }
    }

    #[tokio::test]
    async fn inspection_uses_an_explicit_remote_and_separate_name_filter() {
        let (runtime, root, log) = fake_runtime("owned");
        let script = std::path::Path::new(&runtime.executable);
        let original = fs::read_to_string(script).unwrap();
        fs::write(
            script,
            original.replace(
                "#!/bin/sh\n",
                "#!/bin/sh\n[ \"$INCUS_REMOTE\" = test ] || exit 1\n",
            ),
        )
        .unwrap();
        assert_eq!(
            runtime.inspect("env").await.unwrap().state,
            VmState::Stopped
        );
        assert_eq!(
            fs::read_to_string(log).unwrap().trim(),
            "list test: ^sf-dev-env$ --format=json"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn instance_names_cannot_escape_the_owned_namespace() {
        for id in ["", "../default", "-x;rm", "default:production", "UPPER"] {
            assert!(instance_name(id).is_err());
        }
        assert_eq!(instance_name("env-abc123").unwrap(), "sf-dev-env-abc123");
    }

    #[test]
    fn shell_data_cannot_break_out_of_quotes() {
        assert_eq!(
            shell_quote("x'$(touch /tmp/unwanted)"),
            "'x'\\''$(touch /tmp/unwanted)'"
        );
    }

    #[tokio::test]
    async fn foreign_matching_instance_is_rejected_without_mutation() {
        let (runtime, root, log) = fake_runtime("foreign");
        let error = runtime
            .execute("env", "start", &test_config())
            .await
            .unwrap_err();
        assert!(error.contains("not owned"));
        let calls = fs::read_to_string(log).unwrap();
        assert!(calls.lines().all(|line| {
            !matches!(
                line.split_whitespace().next(),
                Some("start" | "stop" | "restart" | "delete" | "init")
            )
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn unreachable_server_is_observed_before_create() {
        let (runtime, root, log) = fake_runtime("unreachable");
        assert!(
            runtime
                .execute("env", "create", &test_config())
                .await
                .is_err()
        );
        let calls = fs::read_to_string(log).unwrap();
        assert!(
            calls
                .lines()
                .all(|line| line.split_whitespace().next() != Some("init"))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn owned_create_retry_does_not_initialize_again() {
        let (runtime, root, log) = fake_runtime("owned");
        let _ = runtime.execute("env", "create", &test_config()).await;
        let calls = fs::read_to_string(log).unwrap();
        assert!(
            calls
                .lines()
                .any(|line| line.split_whitespace().next() == Some("list"))
        );
        assert!(
            calls
                .lines()
                .all(|line| line.split_whitespace().next() != Some("init"))
        );
        fs::remove_dir_all(root).unwrap();
    }
}
