//! Run as root on disposable Linux with s6 and delegated cgroup v2.
//! cargo test --test native_supervision_linux -- --ignored --test-threads=1
#![cfg(target_os = "linux")]

use self_host::native::supervision::{ServiceDefinition, Supervisor};
use self_host::native::{AccountName, CgroupRoot, ProvisionRequest, ResourceLimits, provision};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(15);

struct Fixture {
    root: PathBuf,
    cgroup: PathBuf,
    home: PathBuf,
    account: AccountName,
    scanner: Child,
    supervisor: Supervisor,
}

impl Fixture {
    fn new() -> Self {
        assert_eq!(
            unsafe { libc::geteuid() },
            0,
            "use root on a disposable Linux Host"
        );
        let id = std::process::id();
        let root = PathBuf::from(format!("/var/lib/sf-s6-test-{id}"));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(root.join("services")).unwrap();
        let binary = root.join("self-host");
        fs::copy(env!("CARGO_BIN_EXE_self-host"), &binary).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let cgroup = PathBuf::from(format!("/sys/fs/cgroup/sf-s6-test-{id}"));
        fs::create_dir(&cgroup).unwrap();
        let home = PathBuf::from(format!("/home/sf-s6-test-{id}"));
        let account = AccountName::parse(&format!("sf-s6-test-{id}")).unwrap();
        provision(&ProvisionRequest {
            application_id: "fixture",
            account: &account,
            home: &home,
        })
        .unwrap();
        let scanner = Command::new("s6-svscan")
            .arg(root.join("services"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        wait_for(|| root.join("services/.s6-svscan/control").exists());
        let supervisor =
            Supervisor::connect(root.clone(), binary, CgroupRoot::at(&cgroup).unwrap()).unwrap();
        Self {
            root,
            cgroup,
            home,
            account,
            scanner,
            supervisor,
        }
    }

    fn definition(&self, id: &str) -> ServiceDefinition {
        ServiceDefinition {
            application_id: id.into(), account: self.account.as_str().into(),
            command: vec!["/bin/sh".into(), "-c".into(), "echo $$ >> starts; id -u > uid; cat /proc/self/status > identity; cat /proc/self/cgroup > cgroup; setsid /bin/sh -c 'echo $$ > descendant; exec sleep 300' & printf 'retained data' > data; yes 'synthetic log line with enough bytes to exercise rotation and bounded retention' | head -n 32000; while :; do sleep 1; done".into()],
            working_dir: self.home.clone(), environment: vec![], limits: ResourceLimits { max_tasks: Some(32), ..ResourceLimits::NONE },
            readiness: Some(vec!["/bin/sh".into(), "-c".into(), "test $(id -u) -ne 0 && cat /proc/self/status > readiness-identity && cat /proc/self/cgroup > readiness-cgroup && test -s descendant".into()]),
            startup_timeout_ms: 5000, stop_grace_ms: 100,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.supervisor.stop("fixture", WAIT);
        let _ = self.supervisor.stop("unhealthy", WAIT);
        let _ = Command::new("s6-svscanctl")
            .arg("-t")
            .arg(self.root.join("services"))
            .status();
        let _ = self.scanner.wait();
        let _ = Command::new("userdel").arg(self.account.as_str()).status();
        let _ = fs::remove_dir_all(&self.home);
        let _ = fs::remove_dir_all(&self.root);
        let _ = fs::remove_dir(&self.cgroup);
    }
}

fn wait_for(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !predicate() {
        assert!(Instant::now() < deadline, "condition timed out");
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn pid(path: &Path) -> i32 {
    fs::read_to_string(path).unwrap().trim().parse().unwrap()
}
fn count(path: &Path) -> usize {
    fs::read_to_string(path).unwrap_or_default().lines().count()
}
fn alive(pid: i32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .is_ok_and(|s| !s.split(") ").nth(1).unwrap_or("").starts_with('Z'))
}

fn assert_identity(path: &Path) {
    let status = fs::read_to_string(path).unwrap();
    for name in ["CapEff", "CapPrm", "CapBnd", "CapAmb"] {
        assert!(
            status.contains(&format!("{name}:\t0000000000000000")),
            "{status}"
        );
    }
    assert!(status.contains("NoNewPrivs:\t1"));
    assert!(
        status
            .lines()
            .find_map(|line| line.strip_prefix("Groups:"))
            .unwrap()
            .trim()
            .is_empty()
    );
}

#[test]
#[ignore = "provisions accounts, s6 and cgroups; needs root on disposable Linux"]
fn supervision_recovers_cleans_descendants_reconnects_and_keeps_stopped_intent() {
    let mut fixture = Fixture::new();
    let spec = fixture.definition("fixture");
    fixture.supervisor.prepare(&spec).unwrap();
    fixture.supervisor.start("fixture", WAIT).unwrap();
    let status = fixture.supervisor.status("fixture").unwrap();
    assert!(status.running && status.ready && status.intended_running);
    let direct = Command::new(fixture.root.join("self-host"))
        .arg("native-run")
        .arg("--service")
        .arg(fixture.root.join("services/fixture"))
        .output()
        .unwrap();
    assert!(
        !direct.status.success(),
        "direct hidden CLI invocation must be refused"
    );
    assert_ne!(pid(&fixture.home.join("uid")), 0);
    assert_identity(&fixture.home.join("identity"));
    assert_identity(&fixture.home.join("readiness-identity"));
    for path in ["cgroup", "readiness-cgroup"] {
        assert!(
            fs::read_to_string(fixture.home.join(path))
                .unwrap()
                .contains("/sf-app-fixture")
        );
    }
    assert_eq!(
        fs::read_to_string(fixture.cgroup.join("sf-app-fixture/pids.max"))
            .unwrap()
            .trim(),
        "32"
    );
    fs::set_permissions(
        fixture.root.join("self-host"),
        fs::Permissions::from_mode(0o722),
    )
    .unwrap();
    assert!(
        Supervisor::connect(
            fixture.root.clone(),
            fixture.root.join("self-host"),
            CgroupRoot::at(&fixture.cgroup).unwrap()
        )
        .is_err()
    );
    fs::set_permissions(
        fixture.root.join("self-host"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let service = fixture.root.join("services/fixture");
    for name in ["run", "finish", "data/definition.json", "log/run"] {
        let metadata = fs::metadata(service.join(name)).unwrap();
        assert_eq!(metadata.uid(), 0);
        assert_eq!(metadata.mode() & 0o077, 0);
    }
    let forbidden = Command::new("runuser")
        .args(["-u", fixture.account.as_str(), "--", "test", "-w"])
        .arg(service.join("run"))
        .status()
        .unwrap();
    assert!(!forbidden.success());
    wait_for(|| {
        fs::read_dir(fixture.root.join("logs/fixture"))
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().starts_with('@'))
    });
    let initial_count = count(&fixture.home.join("starts"));
    let original = fs::read_to_string(fixture.home.join("starts")).unwrap();
    let old_descendant = pid(&fixture.home.join("descendant"));
    let leader: i32 = original.lines().last().unwrap().parse().unwrap();
    unsafe {
        libc::kill(leader, libc::SIGKILL);
    }
    wait_for(|| count(&fixture.home.join("starts")) > initial_count);
    wait_for(|| !alive(old_descendant));
    let reconnected = Supervisor::connect(
        fixture.root.clone(),
        fixture.root.join("self-host"),
        CgroupRoot::at(&fixture.cgroup).unwrap(),
    )
    .unwrap();
    let before = count(&fixture.home.join("starts"));
    reconnected.start("fixture", WAIT).unwrap();
    assert_eq!(
        count(&fixture.home.join("starts")),
        before,
        "reconnecting must not duplicate the process"
    );
    let descendant = pid(&fixture.home.join("descendant"));
    reconnected.restart("fixture", WAIT).unwrap();
    wait_for(|| !alive(descendant));
    // Kill the privileged runner itself. The s6 finish hook must kill the
    // detached tree before a replacement can pass N1's cgroup admission.
    let runner = Command::new("s6-svstat")
        .args(["-o", "pid"])
        .arg(&service)
        .output()
        .unwrap();
    let runner: i32 = String::from_utf8(runner.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let descendant = pid(&fixture.home.join("descendant"));
    let before = count(&fixture.home.join("starts"));
    unsafe {
        libc::kill(runner, libc::SIGKILL);
    }
    wait_for(|| count(&fixture.home.join("starts")) > before);
    wait_for(|| !alive(descendant));
    reconnected.stop("fixture", WAIT).unwrap();
    let status = reconnected.status("fixture").unwrap();
    assert!(!status.running && !status.ready && !status.intended_running);
    assert_eq!(status.runner_pid, None);
    assert!(!fixture.cgroup.join("sf-app-fixture").exists());
    assert_eq!(
        fs::read_to_string(fixture.home.join("data")).unwrap(),
        "retained data"
    );
    assert!(service.join("down").exists());
    Command::new("s6-svscanctl")
        .arg("-t")
        .arg(fixture.root.join("services"))
        .status()
        .unwrap();
    fixture.scanner.wait().unwrap();
    let before = count(&fixture.home.join("starts"));
    fixture.scanner = Command::new("s6-svscan")
        .arg(fixture.root.join("services"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(
        count(&fixture.home.join("starts")),
        before,
        "stopped intent survives scanner restart"
    );
    let mut unhealthy = fixture.definition("unhealthy");
    unhealthy.command = vec!["/bin/sleep".into(), "30".into()];
    unhealthy.readiness = Some(vec!["/bin/false".into()]);
    unhealthy.startup_timeout_ms = 200;
    fixture.supervisor.prepare(&unhealthy).unwrap();
    let error = fixture
        .supervisor
        .start("unhealthy", Duration::from_secs(2))
        .unwrap_err();
    assert!(error.to_string().contains("did not become ready"));
    fixture.supervisor.stop("unhealthy", WAIT).unwrap();
}
