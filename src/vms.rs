//! Virtual machines on Apple's Virtualization framework, through Lima.
//!
//! One Lima instance per machine ([ADR-0023]). The Host writes the instance
//! template, Lima boots Canonical's cloud image on the hypervisor, and
//! cloud-init runs the provisioning script during that boot. Only instances
//! carrying this Platform's name prefix are managed.
//!
//! [ADR-0023]: ../docs/adr/0023-virtual-machines-run-on-apple-virtualization.md

use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddr},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    process::Command,
    sync::mpsc,
};

use crate::environments::{
    LogSource, MachineUsage, RunnerObservation, RuntimeProgress, VmConfig, VmRuntime, VmState,
};

const PROVISION: &str = include_str!("vms/bootstrap.sh");
/// Only instances under this prefix are ours. Anything else on the Host's
/// Lima belongs to the Operator and is never touched.
const PREFIX: &str = "sf-dev-";
const CONFIG_DIR: &str = "/etc/self-host-environment";

/// Canonical's images, one per architecture. Lima picks the one that matches
/// the Host and verifies the digest before it boots anything.
const IMAGES: &[(&str, &str, &str)] = &[
    (
        "aarch64",
        "https://cloud-images.ubuntu.com/releases/noble/release-20260705/ubuntu-24.04-server-cloudimg-arm64.img",
        "sha256:7df0201546f75b8bcc1044594c806c35749421ad3c9bc1be2a3ab806cfae39cc",
    ),
    (
        "x86_64",
        "https://cloud-images.ubuntu.com/releases/noble/release-20260705/ubuntu-24.04-server-cloudimg-amd64.img",
        "sha256:ffe6203da54deeb6db5d2a98a83f9ec8e55f149d3f7ba622e1abe5fa966ee3d6",
    ),
];

/// The counters one reading of `/proc` gave, kept so the next reading can be
/// turned into a rate. CPU time is cumulative; percent is a difference.
#[derive(Clone, Copy)]
struct CpuReading {
    busy: u64,
    total: u64,
}

pub struct LimaRuntime {
    executable: String,
    /// `mise ls` costs a call into the machine, and the console polls. Keep
    /// the last answer so an inspection does not pay for it every time.
    versions: Mutex<HashMap<String, Value>>,
    cpu: Mutex<HashMap<String, CpuReading>>,
}

impl Default for LimaRuntime {
    fn default() -> Self {
        let executable = std::env::var("SELF_HOST_LIMA_BIN").unwrap_or_else(|_| {
            if cfg!(target_os = "macos")
                && std::path::Path::new("/opt/homebrew/bin/limactl").exists()
            {
                "/opt/homebrew/bin/limactl".into()
            } else {
                "limactl".into()
            }
        });
        Self {
            executable,
            versions: Mutex::new(HashMap::new()),
            cpu: Mutex::new(HashMap::new()),
        }
    }
}

/// `None` for anything this crate did not issue, so a crafted identifier
/// cannot name an instance outside the owned prefix.
fn instance_name(id: &str) -> Result<String, String> {
    if id.is_empty()
        || id.len() > 48
        || !id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        return Err("Invalid virtual machine identifier.".into());
    }
    Ok(format!("{PREFIX}{id}"))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn encode(value: &str) -> String {
    use std::fmt::Write;
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = value.as_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                let _ = write!(
                    out,
                    "{}",
                    ALPHABET[(n >> (18 - 6 * i)) as usize & 63] as char
                );
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A free TCP port on the loopback, for Lima to forward the machine's web
/// service to. Asking the kernel avoids racing a range we guessed at.
fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|error| format!("Could not reserve a port for the machine: {error}"))
}

/// Reads `/proc` inside the machine. One call, because a shell into a guest
/// costs more than the three files it reads.
const USAGE_SCRIPT: &str = r#"
head -1 /proc/stat
grep -E '^(MemTotal|MemAvailable):' /proc/meminfo
awk 'NR>2 && $1 !~ /^lo:/ {gsub(/:/,"",$1); rx+=$2; tx+=$10} END {printf "NET %d %d\n", rx, tx}' /proc/net/dev
"#;

/// Turns one `/proc` reading into the same terms an Application is measured
/// in. CPU is a difference against the previous reading, so the first one
/// after a machine starts reports nothing rather than a spike.
fn parse_usage(text: &str, previous: Option<CpuReading>) -> Option<(MachineUsage, CpuReading)> {
    let mut cpu = None;
    let mut total_memory = 0u64;
    let mut available_memory = 0u64;
    let mut rx = 0u64;
    let mut tx = 0u64;
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        match fields.first() {
            Some(&"cpu") => {
                let values: Vec<u64> = fields[1..].iter().filter_map(|v| v.parse().ok()).collect();
                if values.len() < 5 {
                    return None;
                }
                let total: u64 = values.iter().sum();
                // Idle and iowait are the processor waiting, not working.
                let busy = total.saturating_sub(values[3] + values[4]);
                cpu = Some(CpuReading { busy, total });
            }
            Some(&"MemTotal:") => total_memory = fields.get(1)?.parse::<u64>().ok()? * 1024,
            Some(&"MemAvailable:") => available_memory = fields.get(1)?.parse::<u64>().ok()? * 1024,
            Some(&"NET") => {
                rx = fields.get(1)?.parse().ok()?;
                tx = fields.get(2)?.parse().ok()?;
            }
            _ => {}
        }
    }
    let reading = cpu?;
    let percent = match previous {
        Some(before) if reading.total > before.total => {
            let busy = reading.busy.saturating_sub(before.busy) as f64;
            let total = (reading.total - before.total) as f64;
            (busy / total) * 100.0
        }
        _ => 0.0,
    };
    Some((
        MachineUsage {
            cpu_percent: percent,
            memory_bytes: total_memory.saturating_sub(available_memory),
            memory_limit_bytes: total_memory,
            rx_bytes: rx,
            tx_bytes: tx,
        },
        reading,
    ))
}

impl LimaRuntime {
    async fn command(&self, args: &[&str], timeout: Duration) -> Result<String, String> {
        let output = tokio::time::timeout(
            timeout,
            Command::new(&self.executable)
                .args(args)
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| "The machine operation timed out. Inspect it before retrying.".to_string())?
        .map_err(|error| {
            format!("Cannot run Lima: {error}. Install Lima and make it reachable by the Platform.")
        })?;
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr);
            let last = message.lines().rev().find(|line| !line.trim().is_empty());
            return Err(last.unwrap_or("The machine operation failed.").to_string());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// What Lima knows about the instance, or `None` when there is none.
    ///
    /// Naming the instance would make Lima exit non-zero when it is gone,
    /// which reads as a broken Host rather than a machine that was deleted.
    /// List everything and look for ours instead.
    async fn instance(&self, id: &str) -> Result<Option<Value>, String> {
        let name = instance_name(id)?;
        let listing = self
            .command(&["list", "--json"], Duration::from_secs(20))
            .await?;
        Ok(listing
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|instance| instance["name"].as_str() == Some(name.as_str())))
    }

    /// Runs `script` inside the machine as root, and returns what it printed.
    async fn guest(&self, id: &str, script: &str, timeout: Duration) -> Result<String, String> {
        let name = instance_name(id)?;
        let mut child = Command::new(&self.executable)
            .args(["shell", &name, "--", "sudo", "bash", "-s"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("Cannot open a shell on the machine: {error}"))?;
        let mut input = child.stdin.take().unwrap();
        let text = script.to_owned();
        let write = tokio::spawn(async move { input.write_all(text.as_bytes()).await });
        let output = tokio::time::timeout(timeout, child.wait_with_output())
            .await
            .map_err(|_| "The machine did not answer in time.".to_string())?
            .map_err(|error| format!("Could not read from the machine: {error}"))?;
        let _ = write.await;
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr);
            let last = message.lines().rev().find(|line| !line.trim().is_empty());
            return Err(last
                .unwrap_or("The machine refused the command.")
                .to_string());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// The provisioning script with this machine's recipe baked in. The same
    /// text runs from cloud-init on the first boot and from a shell on every
    /// update, so there is one description of what a machine should contain.
    fn payload(&self, config: &VmConfig) -> String {
        format!(
            "#!/usr/bin/env bash\n\
             export SELF_HOST_SSH_PUBLIC_KEY_B64={}\n\
             export SELF_HOST_MISE_TOML_B64={}\n\
             export SELF_HOST_SETUP_B64={}\n\
             export SELF_HOST_COMMAND_B64={}\n\
             export SELF_HOST_WEB_PORT={}\n\
             {}",
            shell_quote(&encode(&config.ssh_public_key)),
            shell_quote(&encode(&config.recipe.mise_toml())),
            shell_quote(&encode(&config.recipe.setup.join("\n"))),
            shell_quote(&encode(&config.command)),
            config.web_port,
            PROVISION
        )
    }

    /// The Lima instance template. Nothing from the Host is mounted into the
    /// machine, and containerd stays out: this is a workspace, not a runtime.
    fn template(&self, config: &VmConfig, host_port: u16) -> String {
        let images: String = IMAGES
            .iter()
            .map(|(arch, location, digest)| {
                format!(
                    "  - location: \"{location}\"\n    arch: \"{arch}\"\n    digest: \"{digest}\"\n"
                )
            })
            .collect();
        let script: String = self
            .payload(config)
            .lines()
            .map(|line| format!("      {line}\n"))
            .collect();
        // A machine with no service has no port to publish, and Lima refuses
        // a forward to port zero.
        let forwards = if config.web_port > 0 {
            format!(
                "portForwards:\n  - guestPort: {}\n    hostPort: {host_port}\n",
                config.web_port
            )
        } else {
            String::new()
        };
        format!(
            "vmType: vz\n\
             images:\n{images}\
             cpus: {cpus}\n\
             memory: \"{memory}GiB\"\n\
             disk: \"{disk}GiB\"\n\
             mounts: []\n\
             containerd:\n  system: false\n  user: false\n\
             ssh:\n  loadDotSSHPubKeys: false\n\
             {forwards}\
             provision:\n  - mode: system\n    script: |\n{script}",
            cpus = config.cpus,
            memory = config.memory_gib,
            disk = config.disk_gib,
        )
    }

    /// Runs a Lima command, sending every line it prints to the Operator as
    /// it appears. A create spends minutes here and says so the whole time.
    async fn stream(
        &self,
        args: &[&str],
        progress: &mpsc::UnboundedSender<RuntimeProgress>,
        timeout: Duration,
    ) -> Result<(), String> {
        let mut child = Command::new(&self.executable)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("Cannot run Lima: {error}"))?;
        let mut out = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut err = BufReader::new(child.stderr.take().unwrap()).lines();
        let sender = progress.clone();
        let drain = tokio::spawn(async move {
            let mut last = String::new();
            while let Ok(Some(line)) = err.next_line().await {
                last = line.clone();
                let _ = sender.send(RuntimeProgress {
                    output: format!("{line}\n"),
                    ..Default::default()
                });
            }
            last
        });
        let wait = async {
            while let Some(line) = out.next_line().await.map_err(|e| e.to_string())? {
                let _ = progress.send(RuntimeProgress {
                    output: format!("{line}\n"),
                    ..Default::default()
                });
            }
            child.wait().await.map_err(|error| error.to_string())
        };
        let status = tokio::time::timeout(timeout, wait)
            .await
            .map_err(|_| "The machine operation timed out.".to_string())??;
        let last = drain.await.unwrap_or_default();
        if !status.success() {
            let detail = last
                .rsplit("msg=")
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches('"');
            return Err(if detail.is_empty() {
                "The machine operation failed.".into()
            } else {
                detail.to_string()
            });
        }
        Ok(())
    }

    /// Follows the guest's provisioning log while the first boot runs, so the
    /// steps reach the console as cloud-init prints them.
    ///
    /// `limactl shell` refuses until the instance is ready, and ready is
    /// exactly when provisioning ends, so it would only ever report the past.
    /// SSH answers as soon as the guest's sshd does, which is well before
    /// the boot scripts finish.
    fn follow(
        &self,
        id: &str,
        progress: mpsc::UnboundedSender<RuntimeProgress>,
        caught: Arc<AtomicBool>,
    ) -> FollowGuard {
        let executable = self.executable.clone();
        let Ok(name) = instance_name(id) else {
            return FollowGuard(None);
        };
        FollowGuard(Some(tokio::spawn(async move {
            for _ in 0..200 {
                tokio::time::sleep(Duration::from_secs(3)).await;
                let Some((dir, port)) = ssh_target(&executable, &name).await else {
                    continue;
                };
                if port == 0 {
                    continue;
                }
                let config = format!("{dir}/ssh.config");
                if !std::path::Path::new(&config).exists() {
                    continue;
                }
                let Ok(mut child) = Command::new("ssh")
                    .args([
                        "-F",
                        &config,
                        "-o",
                        "StrictHostKeyChecking=no",
                        "-o",
                        "LogLevel=ERROR",
                        &format!("lima-{name}"),
                        "sudo tail -n +1 -F /var/log/cloud-init-output.log",
                    ])
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .kill_on_drop(true)
                    .spawn()
                else {
                    continue;
                };
                let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
                let mut seen = false;
                while let Ok(Some(line)) = lines.next_line().await {
                    seen = true;
                    let step = line.strip_prefix("SF_STEP ").filter(|step| {
                        step.len() < 80
                            && step
                                .bytes()
                                .all(|b| b.is_ascii_lowercase() || b"-:".contains(&b))
                    });
                    // Only a step counts as having followed the provisioning.
                    // cloud-init writes its own chatter early and the script's
                    // own output late, so reading lines proves nothing.
                    if step.is_some() {
                        caught.store(true, Ordering::Relaxed);
                    }
                    let _ = progress.send(RuntimeProgress {
                        step: step.map(Into::into),
                        log: step.map(|step| format!("{step}\n")).unwrap_or_default(),
                        output: format!("{line}\n"),
                    });
                }
                if seen {
                    return;
                }
            }
        })))
    }

    async fn read_versions(&self, id: &str) -> Option<Value> {
        let text = self
            .guest(
                id,
                &format!("cat {CONFIG_DIR}/versions.json 2>/dev/null || true"),
                Duration::from_secs(30),
            )
            .await
            .ok()?;
        let parsed: Value = serde_json::from_str(text.trim()).ok()?;
        self.versions
            .lock()
            .ok()?
            .insert(id.to_owned(), parsed.clone());
        Some(parsed)
    }

    fn cached_versions(&self, id: &str) -> Option<Value> {
        self.versions.lock().ok()?.get(id).cloned()
    }

    /// What the console shows about a machine Lima already knows.
    fn describe(&self, id: &str, instance: &Value) -> RunnerObservation {
        let state = match instance["status"].as_str() {
            Some("Running") => VmState::Running,
            Some("Stopped") => VmState::Stopped,
            _ => VmState::Unknown,
        };
        let host_port = instance["config"]["portForwards"]
            .as_array()
            .and_then(|forwards| forwards.first())
            .and_then(|forward| forward["hostPort"].as_u64())
            .map(|port| port as u16);
        let image = instance["config"]["images"]
            .as_array()
            .and_then(|images| images.first())
            .and_then(|image| image["digest"].as_str())
            .map(str::to_owned);
        RunnerObservation {
            state,
            service_ready: false,
            step: None,
            log: String::new(),
            ssh_command: instance["sshLocalPort"]
                .as_u64()
                .filter(|port| *port > 0)
                .map(|port| format!("ssh -p {port} dev@127.0.0.1")),
            tunnel_command: None,
            web_url: host_port.map(|port| format!("http://127.0.0.1:{port}")),
            base_image: image,
            installed_versions: self.cached_versions(id),
        }
    }
}

/// Where an instance keeps its files and which port its SSH answers on, or
/// `None` while Lima has not got that far.
async fn ssh_target(executable: &str, name: &str) -> Option<(String, u64)> {
    let output = Command::new(executable)
        .args(["list", "--json"])
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|instance| instance["name"].as_str() == Some(name))
        .map(|instance| {
            (
                instance["dir"].as_str().unwrap_or_default().to_owned(),
                instance["sshLocalPort"].as_u64().unwrap_or(0),
            )
        })
}

/// Stops following the guest's log when the operation that started it ends.
struct FollowGuard(Option<tokio::task::JoinHandle<()>>);
impl Drop for FollowGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

/// Whether the machine's web service answers on the port Lima forwards. Lima
/// only publishes the port while something inside is listening, so a refused
/// connection is the service being down rather than a guess about it.
async fn answering(url: Option<&String>) -> bool {
    let Some(port) = url.and_then(|url| url.rsplit(':').next()) else {
        return false;
    };
    let Ok(port) = port.parse::<u16>() else {
        return false;
    };
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    matches!(
        tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(address)).await,
        Ok(Ok(_))
    )
}

#[async_trait]
impl VmRuntime for LimaRuntime {
    async fn inspect(&self, id: &str) -> Result<RunnerObservation, String> {
        let Some(instance) = self.instance(id).await? else {
            return Ok(RunnerObservation {
                state: VmState::Missing,
                ..Default::default()
            });
        };
        let mut observation = self.describe(id, &instance);
        if observation.state == VmState::Running {
            observation.service_ready = answering(observation.web_url.as_ref()).await;
            if observation.installed_versions.is_none() {
                observation.installed_versions = self.read_versions(id).await;
            }
        }
        Ok(observation)
    }

    async fn execute(
        &self,
        id: &str,
        action: &str,
        config: &VmConfig,
    ) -> Result<RunnerObservation, String> {
        let (progress, _) = mpsc::unbounded_channel();
        self.execute_with_progress(id, action, config, progress)
            .await
    }

    async fn execute_with_progress(
        &self,
        id: &str,
        action: &str,
        config: &VmConfig,
        progress: mpsc::UnboundedSender<RuntimeProgress>,
    ) -> Result<RunnerObservation, String> {
        let name = instance_name(id)?;
        let stage = |step: &str| {
            tracing::info!(machine = id, action, step, "virtual machine stage");
            let _ = progress.send(RuntimeProgress {
                step: Some(step.into()),
                log: format!("{step}\n"),
                output: format!("{step}\n"),
            });
        };
        let mut instance = self.instance(id).await?;

        if action == "delete" {
            if instance.is_some() {
                stage("deleting");
                self.command(&["delete", "--force", &name], Duration::from_secs(300))
                    .await?;
            }
            let _ = self.versions.lock().map(|mut cache| cache.remove(id));
            return Ok(RunnerObservation {
                state: VmState::Missing,
                ..Default::default()
            });
        }

        if action == "create" && instance.is_none() {
            stage("creating");
            let template = self.template(config, free_port()?);
            let path = std::env::temp_dir().join(format!("{name}.yaml"));
            tokio::fs::write(&path, template)
                .await
                .map_err(|error| format!("Could not write the machine's template: {error}"))?;
            let written = path.to_string_lossy().into_owned();
            let created = self
                .command(
                    &["create", "--name", &name, "--tty=false", &written],
                    Duration::from_secs(900),
                )
                .await;
            let _ = tokio::fs::remove_file(&path).await;
            created?;
            instance = self.instance(id).await?;
        }

        let known = instance.ok_or("The machine is missing. Retry creation first.")?;
        let running = known["status"].as_str() == Some("Running");

        match action {
            // The first boot provisions itself: cloud-init runs the recipe
            // before Lima reports the machine ready.
            "create" => {
                if !running {
                    stage("booting");
                    let caught = Arc::new(AtomicBool::new(false));
                    let follow = self.follow(id, progress.clone(), caught.clone());
                    let started = self
                        .stream(&["start", &name], &progress, Duration::from_secs(2400))
                        .await;
                    drop(follow);
                    started?;
                    // Provisioning can finish before SSH is up to be followed,
                    // which a machine with nothing to install usually does.
                    // Read the guest's own account so the record is complete
                    // either way.
                    if !caught.load(Ordering::Relaxed) {
                        self.replay_provisioning(id, &progress).await;
                    }
                }
            }
            "start" => {
                if !running {
                    stage("starting");
                    self.stream(&["start", &name], &progress, Duration::from_secs(900))
                        .await?;
                }
            }
            "stop" => {
                if running {
                    stage("stopping");
                    self.command(&["stop", &name], Duration::from_secs(300))
                        .await?;
                }
            }
            "restart" => {
                stage("restarting");
                if running {
                    self.command(&["stop", &name], Duration::from_secs(300))
                        .await?;
                }
                self.stream(&["start", &name], &progress, Duration::from_secs(900))
                    .await?;
            }
            // An update runs the same script the first boot ran, from a shell.
            // cloud-init only fires once, and the script is written to be run
            // again without undoing what is already there.
            "bootstrap" | "update" => {
                if !running {
                    stage("starting");
                    self.stream(&["start", &name], &progress, Duration::from_secs(900))
                        .await?;
                }
                stage("provisioning");
                self.run_provisioning(id, config, &progress).await?;
            }
            other => return Err(format!("Unknown machine action: {other}")),
        }

        // The guest announces its own `ready` when provisioning ends; saying
        // it again here would print the phase twice.
        tracing::info!(machine = id, action, "virtual machine operation finished");
        let instance = self
            .instance(id)
            .await?
            .ok_or("The machine disappeared during the operation.")?;
        let mut observation = self.describe(id, &instance);
        if observation.state == VmState::Running {
            observation.service_ready = answering(observation.web_url.as_ref()).await;
            observation.installed_versions = self.read_versions(id).await;
        }
        observation.step = Some("ready".into());
        Ok(observation)
    }

    async fn logs(&self, id: &str, source: LogSource) -> Result<String, String> {
        // Both are capped: a boot is a thousand lines and a provisioning run
        // is tens of thousands, and neither is read past its end.
        let script = match source {
            LogSource::Boot => "journalctl -b --no-pager -o short-monotonic | tail -n 2000",
            LogSource::Provisioning => "tail -n 2000 /var/log/cloud-init-output.log",
        };
        self.guest(id, script, Duration::from_secs(60)).await
    }

    async fn sample(&self, id: &str) -> Result<MachineUsage, String> {
        let text = self
            .guest(id, USAGE_SCRIPT, Duration::from_secs(20))
            .await?;
        let previous = self
            .cpu
            .lock()
            .ok()
            .and_then(|cache| cache.get(id).copied());
        let (usage, reading) =
            parse_usage(&text, previous).ok_or("The machine did not report its resource use.")?;
        if let Ok(mut cache) = self.cpu.lock() {
            cache.insert(id.to_owned(), reading);
        }
        Ok(usage)
    }

    async fn open_terminal(
        &self,
        id: &str,
        size: crate::terminal::Size,
    ) -> Result<crate::terminal::Session, String> {
        let name = instance_name(id)?;
        let executable = self.executable.clone();
        // Lima ends the command when its client goes away, so nothing has to
        // be reaped afterwards. `su -l dev` loads the profile that puts mise
        // on PATH.
        tokio::task::spawn_blocking(move || {
            let mut command = portable_pty::CommandBuilder::new(&executable);
            command.args(["shell", &name, "--", "sudo", "-i", "-u", "dev"]);
            command.env("TERM", "xterm-256color");
            crate::terminal::spawn_on_pty(command, size, None)
        })
        .await
        .map_err(|error| format!("Could not open a terminal: {error}"))?
        .map_err(|error| format!("Could not open a terminal: {error}"))
    }
}

impl LimaRuntime {
    /// Reports the provisioning the Host was not there to watch. The guest
    /// keeps its own log, so a run nobody followed is recorded rather than
    /// lost: the steps in the order they happened, then the output.
    async fn replay_provisioning(
        &self,
        id: &str,
        progress: &mpsc::UnboundedSender<RuntimeProgress>,
    ) {
        let text = match self
            .guest(
                id,
                "tail -n 4000 /var/log/cloud-init-output.log",
                Duration::from_secs(60),
            )
            .await
        {
            Ok(text) => text,
            Err(error) => {
                tracing::warn!(machine = id, %error, "could not replay provisioning");
                return;
            }
        };
        tracing::info!(
            machine = id,
            lines = text.lines().count(),
            "replaying provisioning"
        );
        for line in text.lines() {
            let step = line.strip_prefix("SF_STEP ").filter(|step| {
                step.len() < 80
                    && step
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b"-:".contains(&b))
            });
            let _ = progress.send(RuntimeProgress {
                step: step.map(Into::into),
                log: step.map(|step| format!("{step}\n")).unwrap_or_default(),
                output: format!("{line}\n"),
            });
        }
    }

    /// Pipes the provisioning script into the machine and forwards each line,
    /// so an update reports its steps the way a first boot does.
    async fn run_provisioning(
        &self,
        id: &str,
        config: &VmConfig,
        progress: &mpsc::UnboundedSender<RuntimeProgress>,
    ) -> Result<(), String> {
        let name = instance_name(id)?;
        let mut child = Command::new(&self.executable)
            .args(["shell", &name, "--", "sudo", "bash", "-s"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("Cannot open a shell on the machine: {error}"))?;
        let mut input = child.stdin.take().unwrap();
        let payload = self.payload(config);
        let write = tokio::spawn(async move {
            let _ = input.write_all(payload.as_bytes()).await;
            drop(input);
        });
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut errors = BufReader::new(child.stderr.take().unwrap()).lines();
        let sender = progress.clone();
        let drain = tokio::spawn(async move {
            while let Ok(Some(line)) = errors.next_line().await {
                let _ = sender.send(RuntimeProgress {
                    output: format!("stderr: {line}\n"),
                    ..Default::default()
                });
            }
        });
        let wait = async {
            while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
                let step = line.strip_prefix("SF_STEP ").filter(|step| {
                    step.len() < 80
                        && step
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b"-:".contains(&b))
                });
                let _ = progress.send(RuntimeProgress {
                    step: step.map(Into::into),
                    log: step.map(|step| format!("{step}\n")).unwrap_or_default(),
                    output: format!("{line}\n"),
                });
            }
            child.wait().await.map_err(|error| error.to_string())
        };
        let status = tokio::time::timeout(Duration::from_secs(2400), wait)
            .await
            .map_err(|_| {
                "Provisioning timed out. Retry once the machine has settled.".to_string()
            })??;
        let _ = write.await;
        let _ = drain.await;
        if !status.success() {
            return Err(
                "Provisioning failed. The output above is the machine's own account.".into(),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_images::{Dependency, Recipe};

    fn config() -> VmConfig {
        VmConfig {
            name: "workbench".into(),
            cpus: 4,
            memory_gib: 8,
            disk_gib: 40,
            ssh_public_key: "ssh-ed25519 AAAA test".into(),
            recipe: Recipe {
                name: "T3 Code".into(),
                template_id: Some("t3-code".into()),
                dependencies: vec![Dependency {
                    tool: "node".into(),
                    version: "24".into(),
                    allow_builds: vec![],
                }],
                setup: vec![],
                build_checks: vec![],
                dockerfile: None,
            },
            command: "t3 serve".into(),
            web_port: 3000,
        }
    }

    /// The runner only ever names instances under its own prefix, so an
    /// identifier that tries to walk out names nothing at all.
    #[test]
    fn an_identifier_cannot_escape_the_owned_prefix() {
        assert_eq!(instance_name("env-abc123").unwrap(), "sf-dev-env-abc123");
        assert!(instance_name("../../etc").is_err());
        assert!(instance_name("Env-Upper").is_err());
        assert!(instance_name("").is_err());
        assert!(instance_name(&"a".repeat(49)).is_err());
    }

    /// The recipe reaches the machine inside a quoted shell assignment, so a
    /// quote in an Operator's own text cannot start running commands.
    #[test]
    fn recipe_text_cannot_break_out_of_its_quotes() {
        let mut hostile = config();
        hostile.command = "t3 serve'; rm -rf /; echo '".into();
        let payload = LimaRuntime::default().payload(&hostile);

        assert!(!payload.contains("rm -rf /"));
        assert!(payload.contains("export SELF_HOST_COMMAND_B64='"));
    }

    /// The template is the whole contract with Lima: the Operator's sizes, a
    /// forwarded port for the web service, and nothing of the Host mounted in.
    #[test]
    fn the_template_carries_the_sizes_and_forwards_the_web_port() {
        let template = LimaRuntime::default().template(&config(), 45123);

        assert!(template.contains("vmType: vz"));
        assert!(template.contains("cpus: 4"));
        assert!(template.contains("memory: \"8GiB\""));
        assert!(template.contains("disk: \"40GiB\""));
        assert!(template.contains("mounts: []"));
        assert!(template.contains("- guestPort: 3000"));
        assert!(template.contains("hostPort: 45123"));
        // Provisioning travels in the template, so the first boot is the one
        // that installs the recipe.
        assert!(template.contains("provision:"));
        assert!(template.contains("SF_STEP"));
    }

    /// A machine is allowed to run nothing. Lima refuses a forward to port
    /// zero, so there must be no forward at all rather than an empty one.
    #[test]
    fn a_machine_without_a_service_publishes_no_port() {
        let mut bare = config();
        bare.command = String::new();
        bare.web_port = 0;
        let template = LimaRuntime::default().template(&bare, 45123);

        assert!(!template.contains("portForwards"));
        assert!(!template.contains("guestPort"));
        // It is still a machine: sized, provisioned and bootable.
        assert!(template.contains("cpus: 4"));
        assert!(template.contains("provision:"));
    }

    /// Every image Lima may pick must carry a digest, or a machine would boot
    /// whatever the mirror served that day.
    #[test]
    fn every_image_is_pinned_by_digest() {
        assert!(!IMAGES.is_empty());
        for (arch, location, digest) in IMAGES {
            assert!(location.starts_with("https://cloud-images.ubuntu.com/"));
            assert!(digest.starts_with("sha256:"));
            assert!(!arch.is_empty());
        }
    }

    fn proc(busy: u64, idle: u64, memory_available: u64) -> String {
        format!(
            "cpu  {busy} 0 0 {idle} 0 0 0 0\n\
             MemTotal:       4000000 kB\n\
             MemAvailable:   {memory_available} kB\n\
             NET 1000 2000\n"
        )
    }

    /// CPU time only climbs, so a percentage is the difference between two
    /// readings. The first one has nothing to subtract and reports none.
    #[test]
    fn cpu_is_a_difference_and_the_first_reading_has_none() {
        let (first, reading) = parse_usage(&proc(100, 900, 1000000), None).unwrap();
        assert_eq!(first.cpu_percent, 0.0);

        // Half the next interval was spent working.
        let (second, _) = parse_usage(&proc(600, 1400, 1000000), Some(reading)).unwrap();
        assert!((second.cpu_percent - 50.0).abs() < 0.001);
    }

    /// Memory is what is in use, not what is allocated: a machine that caches
    /// aggressively is not a machine that is full.
    #[test]
    fn memory_is_what_the_machine_cannot_get_back() {
        let (usage, _) = parse_usage(&proc(100, 900, 1000000), None).unwrap();

        assert_eq!(usage.memory_limit_bytes, 4000000 * 1024);
        assert_eq!(usage.memory_bytes, (4000000 - 1000000) * 1024);
        assert_eq!((usage.rx_bytes, usage.tx_bytes), (1000, 2000));
    }

    /// A reading the machine could not produce is refused rather than
    /// recorded as a machine using nothing.
    #[test]
    fn an_unreadable_machine_reports_nothing_rather_than_zero() {
        assert!(parse_usage("", None).is_none());
        assert!(parse_usage("MemTotal: 100 kB\n", None).is_none());
    }
}
