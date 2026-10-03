//! Protected s6 service directories. The daemon connects to an existing scan
//! tree; the Host service manager owns its lifetime, independently of the API.

#[cfg(target_os = "linux")]
use super::{AccountName, LaunchRequest, Purpose};
use super::{CgroupRoot, ResourceLimits};
#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceDefinition {
    pub application_id: String,
    pub account: String,
    pub command: Vec<String>,
    pub working_dir: PathBuf,
    pub environment: Vec<(String, String)>,
    pub limits: ResourceLimits,
    #[serde(default)]
    pub recipe: super::mise::NativeRecipe,
    /// An optional command that must exit successfully before s6 reports ready.
    pub readiness: Option<Vec<String>>,
    pub startup_timeout_ms: u64,
    pub stop_grace_ms: u64,
}

#[cfg(target_os = "linux")]
impl ServiceDefinition {
    fn request(&self, command: Vec<String>, purpose: Purpose) -> Result<LaunchRequest> {
        let account = AccountName::parse(&self.account)?;
        let home = super::resolve(&account)?.home;
        Ok(LaunchRequest {
            application_id: self.application_id.clone(),
            account,
            command: super::mise::command(&home, &self.recipe, command),
            working_dir: self.working_dir.clone(),
            environment: super::mise::environment(&home, &self.environment),
            limits: self.limits.clone(),
            purpose,
        })
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Serialize, Deserialize)]
struct StoredService {
    definition: ServiceDefinition,
    cgroup_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceStatus {
    pub running: bool,
    pub ready: bool,
    pub intended_running: bool,
    pub runner_pid: Option<u32>,
}

/// Connecting is read-only with respect to existing processes and intent.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone)]
pub struct Supervisor {
    root: PathBuf,
    binary: PathBuf,
    cgroup_root: CgroupRoot,
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::fs::{self, OpenOptions};
    use std::io::{Read, Write};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    const POLL: Duration = Duration::from_millis(25);
    static STOP: AtomicBool = AtomicBool::new(false);
    extern "C" fn terminate(_: libc::c_int) {
        STOP.store(true, Ordering::SeqCst);
    }

    pub(super) fn require_root() -> Result<()> {
        // SAFETY: geteuid has no arguments or memory effects.
        if unsafe { libc::geteuid() } != 0 {
            bail!(
                "native supervision needs root for N1 launch setup; Application commands still run as their dedicated account"
            );
        }
        Ok(())
    }

    /// Follow no symlinks and reject every writable ancestor, not only the leaf.
    pub(super) fn protected(path: &Path) -> Result<()> {
        if !path.is_absolute()
            || path.components().any(|c| {
                !matches!(
                    c,
                    std::path::Component::RootDir | std::path::Component::Normal(_)
                )
            })
        {
            bail!(
                "supervision path must be absolute without '..': {}",
                path.display()
            );
        }
        for ancestor in path.ancestors() {
            let metadata = fs::symlink_metadata(ancestor)
                .with_context(|| format!("inspect protected path {}", ancestor.display()))?;
            if metadata.file_type().is_symlink()
                || metadata.uid() != 0
                || metadata.mode() & 0o022 != 0
            {
                bail!(
                    "{} must be root-owned, not a symlink, and not writable by group or others",
                    ancestor.display()
                );
            }
        }
        Ok(())
    }

    pub(super) fn directory(path: &Path, mode: u32) -> Result<()> {
        if !path.exists() {
            protected(path.parent().context("directory needs a parent")?)?;
            fs::create_dir(path)?;
            fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
        }
        protected(path)
    }

    pub(super) fn write(path: &Path, content: &[u8], mode: u32) -> Result<()> {
        protected(path.parent().context("file needs a parent")?)?;
        // A stale temporary file after power loss must not block the next boot.
        let temporary = path.with_extension(format!("new-{}", rand::random::<u64>()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temporary)?;
        let result = (|| {
            file.write_all(content)?;
            file.sync_all()?;
            fs::rename(&temporary, path)?;
            fs::File::open(path.parent().unwrap())?.sync_all()?;
            Ok::<_, std::io::Error>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        Ok(())
    }

    pub(super) fn tool(name: &str) -> Result<PathBuf> {
        // A release bundle is one versioned set. Never mix an incomplete
        // bundle with distribution tools or accept a user-controlled PATH.
        let bundle = Path::new("/usr/libexec/self-host/s6");
        if bundle.try_exists()? {
            let path = bundle.join(name);
            protected(&path)?;
            let metadata = fs::metadata(&path)?;
            if !metadata.is_file() || metadata.mode() & 0o111 == 0 {
                bail!("{} must be an executable file", path.display());
            }
            return Ok(path);
        }
        for directory in ["/usr/bin", "/bin", "/usr/local/bin"] {
            let path = Path::new(directory).join(name);
            // Debian's /bin is a symlink to /usr/bin. Canonicalize only a
            // system tool, never Operator-controlled supervision paths.
            if let Ok(path) = fs::canonicalize(path) {
                protected(&path)?;
                if fs::metadata(&path)?.mode() & 0o111 != 0 {
                    return Ok(path);
                }
            }
        }
        bail!(
            "missing {name}; reinstall the self-host Linux release or install the distribution's s6 package"
        )
    }

    pub(super) fn run_tool(name: &str, arguments: &[&std::ffi::OsStr]) -> Result<()> {
        let output = Command::new(tool(name)?)
            .args(arguments)
            .env_clear()
            .output()?;
        if !output.status.success() {
            bail!(
                "{name} failed: {}; check that self-host-native.service is running and its cgroup controllers are delegated",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    fn read_service(service: &Path) -> Result<StoredService> {
        require_root()?;
        let path = service.join("data/definition.json");
        protected(&path)?;
        let stored: StoredService = serde_json::from_slice(&fs::read(path)?)?;
        if service.file_name().and_then(|p| p.to_str())
            != Some(stored.definition.application_id.as_str())
        {
            bail!("service directory does not match the Application id");
        }
        Ok(stored)
    }

    pub(super) fn require_environment(
        service: &Path,
        definition: &ServiceDefinition,
    ) -> Result<()> {
        if definition.recipe.is_empty() {
            return Ok(());
        }
        let receipt = service.join("data/recipe-sha256");
        if !receipt.exists() {
            bail!(
                "native environment preparation has not succeeded; reapply the Application configuration and inspect its logs"
            );
        }
        protected(&receipt)?;
        if fs::read_to_string(receipt)? != definition.recipe.fingerprint() {
            bail!(
                "native environment recipe has changed; reapply the Application configuration before starting"
            );
        }
        Ok(())
    }

    pub fn finish_service(service: &Path) -> Result<()> {
        let stored = read_service(service)?;
        let root = CgroupRoot::at(stored.cgroup_root).context("cgroup delegation unavailable; enable cpu, memory and pids for self-host-native.service")?;
        root.application(&stored.definition.application_id)?
            .kill(Duration::from_secs(5))?;
        Ok(())
    }

    fn forward(
        reader: impl Read + Send + 'static,
        output: std::sync::mpsc::SyncSender<Vec<u8>>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buffer = [0_u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        if output.send(buffer[..count].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        })
    }

    fn pipes(
        process: &mut super::super::LaunchedProcess,
        readers: &mut Vec<std::thread::JoinHandle<()>>,
        output: &std::sync::mpsc::SyncSender<Vec<u8>>,
    ) {
        if let Some(pipe) = process.child.stdout.take() {
            readers.push(forward(pipe, output.clone()));
        }
        if let Some(pipe) = process.child.stderr.take() {
            readers.push(forward(pipe, output.clone()));
        }
    }

    pub fn run_service(service: &Path) -> Result<()> {
        let stored = read_service(service)?;
        let definition = &stored.definition;
        require_environment(service, definition)?;
        let inherited = fs::metadata("/proc/self/fd/4")
            .context("native-run must be launched by s6 with its service lock")?;
        let expected = fs::metadata(service.join("data/runner-lock"))?;
        if inherited.dev() != expected.dev() || inherited.ino() != expected.ino() {
            bail!("native-run requires the s6 service lock; direct invocation is refused");
        }
        let root = CgroupRoot::at(stored.cgroup_root).context("cgroup delegation unavailable; enable cpu, memory and pids for self-host-native.service")?;
        // s6-setlock prevents simultaneous runners. Clean orphaned trees
        // before a new launch, including a previous SIGKILL or power loss.
        root.application(&definition.application_id)?
            .kill(Duration::from_secs(5))?;
        STOP.store(false, Ordering::SeqCst);
        // SAFETY: handler only sets a lock-free atomic. No allocation or IO.
        unsafe {
            libc::signal(libc::SIGTERM, terminate as *const () as libc::sighandler_t);
            libc::signal(libc::SIGINT, terminate as *const () as libc::sighandler_t);
            // Never leak s6 notification/lock descriptors into Application code.
            libc::fcntl(3, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(4, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        let mut main = super::super::launch(&root, &definition.request(definition.command.clone(), Purpose::Main)?)
            .context("native startup refused by N1; check the account, working directory, command and cgroup delegation")?;
        let mut readers = Vec::new();
        let (output, chunks) = std::sync::mpsc::sync_channel(16);
        let redactor = super::super::redaction::Redactor::new(&definition.environment);
        let logger = std::thread::spawn(move || redactor.forward(chunks, std::io::stdout()));
        pipes(&mut main, &mut readers, &output);
        let outcome = (|| -> Result<()> {
            let deadline = Instant::now() + Duration::from_millis(definition.startup_timeout_ms);
            if let Some(command) = &definition.readiness {
                loop {
                    if STOP.load(Ordering::SeqCst) {
                        return Ok(());
                    }
                    if main.child.try_wait()?.is_some() {
                        bail!("Application exited before readiness; inspect its logs");
                    }
                    if Instant::now() >= deadline {
                        bail!(
                            "readiness timed out; check the readiness command and startup_timeout_ms"
                        );
                    }
                    let mut check = super::super::launch::launch_in(
                        &main.cgroup,
                        &definition.request(command.clone(), Purpose::Hook)?,
                    )
                    .context("readiness launch refused by N1")?;
                    pipes(&mut check, &mut readers, &output);
                    let status = loop {
                        if let Some(status) = check.child.try_wait()? {
                            break Some(status);
                        }
                        if STOP.load(Ordering::SeqCst) || Instant::now() >= deadline {
                            check.child.kill()?;
                            check.child.wait()?;
                            break None;
                        }
                        std::thread::sleep(POLL);
                    };
                    if status.is_some_and(|status| status.success()) {
                        break;
                    }
                    std::thread::sleep(POLL);
                }
            }
            if STOP.load(Ordering::SeqCst) {
                return Ok(());
            }
            // SAFETY: fd 3 is the s6 notification pipe, not Application input.
            if unsafe { libc::write(3, b"\n".as_ptr().cast(), 1) } != 1 {
                bail!(
                    "s6 readiness notification failed; run this command only through the protected service directory"
                );
            }
            unsafe {
                libc::close(3);
            }
            loop {
                if STOP.load(Ordering::SeqCst) {
                    return Ok(());
                }
                if let Some(status) = main.child.try_wait()? {
                    if !status.success() {
                        bail!("Application exited with {status}; s6 will restart it");
                    }
                    return Ok(());
                }
                std::thread::sleep(POLL);
            }
        })();
        main.stop(Duration::from_millis(definition.stop_grace_ms))
            .context("failed to clean the Application's process tree")?;
        for reader in readers {
            let _ = reader.join();
        }
        drop(output);
        logger
            .join()
            .map_err(|_| anyhow::anyhow!("native log forwarding thread failed"))?
            .context("could not forward native output to s6-log")?;
        outcome
    }

    pub fn boot_scan(root: &Path) -> Result<()> {
        require_root()?;
        protected(&std::env::current_exe()?)?;
        for name in [
            "s6-svscan",
            "s6-supervise",
            "s6-svscanctl",
            "s6-svc",
            "s6-svwait",
            "s6-svok",
            "s6-svstat",
            "s6-log",
            "s6-setlock",
        ] {
            tool(name)?;
        }
        if Path::new("/usr/libexec/self-host/s6").try_exists()? {
            tool("s6-ftrigrd")?;
        }
        directory(root, 0o700)?;
        directory(&root.join("services"), 0o700)?;
        // Move the supervisor into a leaf before enabling domain controllers.
        let membership = fs::read_to_string("/proc/self/cgroup")?;
        let relative = membership
            .lines()
            .find_map(|line| line.strip_prefix("0::"))
            .context("cgroup v2 is required; use a unified Linux cgroup hierarchy")?;
        if relative == "/" {
            bail!(
                "native-scan needs a delegated service cgroup; start it with self-host-native.service"
            );
        }
        let delegated = Path::new("/sys/fs/cgroup").join(relative.trim_start_matches('/'));
        let managers = delegated.join("supervisor");
        fs::create_dir_all(&managers)?;
        fs::write(
            managers.join("cgroup.procs"),
            std::process::id().to_string(),
        )?;
        fs::write(delegated.join("cgroup.subtree_control"), "+cpu +memory +pids")
            .context("cannot enable cgroup controllers; set Delegate=cpu memory pids on self-host-native.service")?;
        let applications = delegated.join("applications");
        fs::create_dir_all(&applications)?;
        CgroupRoot::at(&applications)?;
        write(
            &root.join("cgroup-root"),
            applications.as_os_str().as_encoded_bytes(),
            0o600,
        )?;
        let error = Command::new(tool("s6-svscan")?)
            .arg(root.join("services"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .exec();
        Err(error.into())
    }
}

impl Supervisor {
    /// Reuses an existing supervision tree, including its running processes.
    #[cfg(target_os = "linux")]
    pub fn connect(root: PathBuf, binary: PathBuf, cgroup_root: CgroupRoot) -> Result<Self> {
        linux::require_root()?;
        linux::protected(&root)?;
        linux::protected(&binary)?;
        if !cgroup_root.path().join("cgroup.kill").exists() {
            bail!(
                "delegated cgroup lacks cgroup.kill; native supervision requires a kernel that supports whole-tree teardown"
            );
        }
        for name in [
            "s6-svscanctl",
            "s6-svc",
            "s6-svwait",
            "s6-svok",
            "s6-svstat",
            "s6-log",
            "s6-setlock",
        ] {
            linux::tool(name)?;
        }
        Ok(Self {
            root,
            binary,
            cgroup_root,
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub fn connect(_: PathBuf, _: PathBuf, _: CgroupRoot) -> Result<Self> {
        bail!("native supervision runs on Linux only")
    }

    #[cfg(target_os = "linux")]
    fn service(&self, id: &str) -> Result<PathBuf> {
        self.cgroup_root.application(id)?;
        Ok(self.root.join("services").join(id))
    }

    pub(crate) fn application_cgroup(&self, id: &str) -> Result<super::ApplicationCgroup> {
        Ok(self.cgroup_root.application(id)?)
    }

    /// The receipt and bounded log are in root-controlled service storage.
    /// Only the N1 child writes configuration or packages inside the home.
    #[cfg(target_os = "linux")]
    pub(crate) fn prepare_environment(&self, definition: &ServiceDefinition) -> Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let service = self.service(&definition.application_id)?;
        linux::protected(&service.join("data"))?;
        let receipt = service.join("data/recipe-sha256");
        let fingerprint = definition.recipe.fingerprint();
        if receipt.exists() {
            linux::protected(&receipt)?;
            if std::fs::read_to_string(&receipt)? == fingerprint {
                return Ok(());
            }
            std::fs::remove_file(&receipt)?;
        }
        // A daemon exit during a previous preparation must not strand its
        // children. The caller has stopped s6 before reaching this point.
        self.cgroup_root
            .application(&definition.application_id)?
            .kill(Duration::from_secs(5))?;
        let logs = self.root.join("logs").join(&definition.application_id);
        linux::protected(&logs)?;
        let log_path = logs.join("recipe.log");
        linux::write(&log_path, b"", 0o600)?;
        let mut log = std::fs::OpenOptions::new()
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(log_path)?;
        let result = super::mise::apply(&self.cgroup_root, definition, log.try_clone()?);
        if result.is_ok() {
            linux::write(&receipt, fingerprint.as_bytes(), 0o600)?;
        } else {
            log.write_all(b"\nNative environment preparation failed.\n")?;
        }
        result
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn prepare_environment(&self, _: &ServiceDefinition) -> Result<()> {
        bail!("native environments run on Linux only")
    }

    /// Materializes a new, stopped service. Never overwrites a live definition.
    #[cfg(target_os = "linux")]
    pub fn prepare(&self, definition: &ServiceDefinition) -> Result<()> {
        use std::fs;
        let request = definition.request(definition.command.clone(), Purpose::Main)?;
        super::identity::resolve(&request.account)?;
        if definition.command.is_empty()
            || definition.startup_timeout_ms == 0
            || definition.stop_grace_ms == 0
        {
            bail!("command, startup_timeout_ms and stop_grace_ms must be nonempty/nonzero");
        }
        let services = self.root.join("services");
        linux::directory(&services, 0o700)?;
        let service = self.service(&definition.application_id)?;
        if service.exists() {
            bail!(
                "Application already has a supervision definition; stop and replace it through the lifecycle adapter"
            );
        }
        // A dot directory is invisible to s6 until the complete tree is renamed.
        let staging = services.join(format!(".{}", definition.application_id));
        linux::directory(&staging, 0o700)?;
        linux::directory(&staging.join("data"), 0o700)?;
        linux::write(&staging.join("data/runner-lock"), b"", 0o600)?;
        let stored = StoredService {
            definition: definition.clone(),
            cgroup_root: self.cgroup_root.path().into(),
        };
        linux::write(
            &staging.join("data/definition.json"),
            &serde_json::to_vec(&stored)?,
            0o600,
        )?;
        for (name, action) in [("run", "native-run"), ("finish", "native-finish")] {
            let lock = if action == "native-run" {
                format!(
                    "{} -d 4 {} ",
                    quote(&linux::tool("s6-setlock")?),
                    quote(&service.join("data/runner-lock"))
                )
            } else {
                String::new()
            };
            let script = format!(
                "#!/bin/sh\nexec 2>&1\nexec {lock}{} {action} --service {}\n",
                quote(&self.binary),
                quote(&service)
            );
            linux::write(&staging.join(name), script.as_bytes(), 0o700)?;
        }
        for (name, value) in [
            ("down", ""),
            ("notification-fd", "3\n"),
            ("timeout-finish", "10000\n"),
        ] {
            linux::write(&staging.join(name), value.as_bytes(), 0o600)?;
        }
        linux::write(
            &staging.join("timeout-kill"),
            format!("{}\n", definition.stop_grace_ms + 6000).as_bytes(),
            0o600,
        )?;
        linux::directory(&staging.join("log"), 0o700)?;
        linux::directory(&self.root.join("logs"), 0o700)?;
        let logs = self.root.join("logs").join(&definition.application_id);
        linux::directory(&logs, 0o700)?;
        let logger = format!(
            "#!/bin/sh\numask 077\nexec {} -b n10 s1048576 T {}\n",
            quote(&linux::tool("s6-log")?),
            quote(&logs)
        );
        linux::write(&staging.join("log/run"), logger.as_bytes(), 0o700)?;
        fs::rename(staging, service)?;
        fs::File::open(&services)?.sync_all()?;
        linux::run_tool(
            "s6-svscanctl",
            &[std::ffi::OsStr::new("-a"), services.as_os_str()],
        )
    }

    #[cfg(not(target_os = "linux"))]
    pub fn prepare(&self, _: &ServiceDefinition) -> Result<()> {
        bail!("native supervision runs on Linux only")
    }

    #[cfg(target_os = "linux")]
    fn change(&self, id: &str, running: bool, timeout: Duration) -> Result<()> {
        use std::ffi::OsStr;
        let service = self.service(id)?;
        linux::protected(&service)?;
        if running {
            let stored: StoredService =
                serde_json::from_slice(&std::fs::read(service.join("data/definition.json"))?)?;
            linux::require_environment(&service, &stored.definition)?;
        }
        let deadline = std::time::Instant::now() + timeout;
        while linux::run_tool("s6-svok", &[service.as_os_str()]).is_err() {
            if std::time::Instant::now() >= deadline {
                bail!("s6 has not discovered Application {id}; check self-host-native.service");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        if running {
            match std::fs::remove_file(service.join("down")) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
            std::fs::File::open(&service)?.sync_all()?;
        } else {
            linux::write(&service.join("down"), b"", 0o600)?;
        }
        linux::run_tool(
            "s6-svc",
            &[
                OsStr::new(if running { "-u" } else { "-d" }),
                service.as_os_str(),
            ],
        )?;
        let milliseconds = timeout.as_millis().to_string();
        linux::run_tool(
            "s6-svwait",
            &[
                OsStr::new(if running { "-U" } else { "-D" }),
                OsStr::new("-t"),
                OsStr::new(&milliseconds),
                service.as_os_str(),
            ],
        )
        .with_context(|| {
            format!(
                "Application {id} did not become {}; inspect {} and its startup/readiness command",
                if running { "ready" } else { "stopped" },
                self.root.join("logs").join(id).display()
            )
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn change(&self, _: &str, _: bool, _: Duration) -> Result<()> {
        bail!("native supervision runs on Linux only")
    }

    /// Reads s6's observation and the persistent intent without changing either.
    #[cfg(target_os = "linux")]
    pub fn status(&self, id: &str) -> Result<ServiceStatus> {
        let service = self.service(id)?;
        linux::protected(&service)?;
        let output = std::process::Command::new(linux::tool("s6-svstat")?)
            .args(["-o", "up,ready,pid"])
            .arg(&service)
            .env_clear()
            .output()?;
        if !output.status.success() {
            bail!("s6 cannot read Application {id}; check self-host-native.service");
        }
        let text = String::from_utf8(output.stdout)?;
        let fields: Vec<_> = text.split_whitespace().collect();
        if fields.len() != 3 {
            bail!("unexpected s6-svstat response for Application {id}");
        }
        let running = fields[0] == "true";
        let runner_pid = if running {
            Some(fields[2].parse()?)
        } else {
            None
        };
        Ok(ServiceStatus {
            running,
            ready: fields[0] == "true" && fields[1] == "true",
            intended_running: !service.join("down").exists(),
            runner_pid,
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub fn status(&self, _: &str) -> Result<ServiceStatus> {
        bail!("native supervision runs on Linux only")
    }

    pub fn start(&self, id: &str, timeout: Duration) -> Result<()> {
        self.change(id, true, timeout)
    }
    pub fn stop(&self, id: &str, timeout: Duration) -> Result<()> {
        self.change(id, false, timeout)?;
        // Preparation runs through N1 while s6 is down. A daemon crash can
        // leave that tree behind even though s6 has no runner to stop.
        self.cgroup_root.application(id)?.kill(timeout)?;
        Ok(())
    }
    pub fn restart(&self, id: &str, timeout: Duration) -> Result<()> {
        self.stop(id, timeout)?;
        self.start(id, timeout)
    }

    #[cfg(target_os = "linux")]
    pub fn exists(&self, id: &str) -> Result<bool> {
        Ok(self.service(id)?.exists())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn exists(&self, _: &str) -> Result<bool> {
        bail!("native supervision runs on Linux only")
    }

    /// Replace only after the existing tree is stopped. The down file keeps
    /// stopped intent through a daemon crash between replacement and start.
    #[cfg(target_os = "linux")]
    pub fn replace(&self, definition: &ServiceDefinition, timeout: Duration) -> Result<()> {
        definition.request(definition.command.clone(), Purpose::Main)?;
        self.stop(&definition.application_id, timeout)?;
        let service = self.service(&definition.application_id)?;
        let stored = StoredService {
            definition: definition.clone(),
            cgroup_root: self.cgroup_root.path().into(),
        };
        linux::write(
            &service.join("data/definition.json"),
            &serde_json::to_vec(&stored)?,
            0o600,
        )
    }

    #[cfg(not(target_os = "linux"))]
    pub fn replace(&self, _: &ServiceDefinition, _: Duration) -> Result<()> {
        bail!("native supervision runs on Linux only")
    }

    /// Remove generated supervision files, leaving Application data and logs.
    #[cfg(target_os = "linux")]
    pub fn remove(&self, id: &str, timeout: Duration) -> Result<()> {
        use std::ffi::OsStr;
        self.stop(id, timeout)?;
        let service = self.service(id)?;
        let hidden = self.root.join("services").join(format!(".removed-{id}"));
        std::fs::rename(&service, &hidden)?;
        std::fs::File::open(self.root.join("services"))?.sync_all()?;
        // Once hidden, svscan cannot start a replacement while the old
        // supervisor and logger exit.
        for directory in [&hidden, &hidden.join("log")] {
            linux::run_tool("s6-svc", &[OsStr::new("-dx"), directory.as_os_str()])?;
        }
        linux::run_tool(
            "s6-svscanctl",
            &[OsStr::new("-a"), self.root.join("services").as_os_str()],
        )?;
        let deadline = std::time::Instant::now() + timeout;
        while linux::run_tool("s6-svok", &[hidden.as_os_str()]).is_ok()
            || linux::run_tool("s6-svok", &[hidden.join("log").as_os_str()]).is_ok()
        {
            if std::time::Instant::now() >= deadline {
                bail!("s6 did not release removed Application {id}");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        std::fs::remove_dir_all(hidden)?;
        std::fs::File::open(self.root.join("services"))?.sync_all()?;
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn remove(&self, _: &str, _: Duration) -> Result<()> {
        bail!("native supervision runs on Linux only")
    }
}

#[cfg(any(target_os = "linux", test))]
fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[cfg(target_os = "linux")]
pub use linux::{boot_scan, finish_service, run_service};
#[cfg(not(target_os = "linux"))]
pub fn run_service(_: &Path) -> Result<()> {
    bail!("native supervision runs on Linux only")
}
#[cfg(not(target_os = "linux"))]
pub fn finish_service(_: &Path) -> Result<()> {
    bail!("native supervision runs on Linux only")
}
#[cfg(not(target_os = "linux"))]
pub fn boot_scan(_: &Path) -> Result<()> {
    bail!("native supervision runs on Linux only")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_are_shell_quoted_without_interpolation() {
        assert_eq!(
            quote(Path::new("/root/a' $(touch no)")),
            "'/root/a'\\'' $(touch no)'"
        );
    }
}
