//! Native launch on a real Linux Host, as root.
//!
//! Every test here is `#[ignore]`. Each one provisions accounts and cgroups
//! on the machine it runs on, so it belongs on a disposable Host and needs
//! root:
//!
//! ```sh
//! sudo -E cargo test --test native_linux -- --ignored --test-threads=1
//! ```
//!
//! An ordinary `cargo test` neither runs these nor counts them as passing.
//! Each test creates its own cgroup root under `/sys/fs/cgroup` and its own
//! accounts, and removes both when it ends, also when it fails.

#![cfg(target_os = "linux")]

use self_host::native::{
    AccountName, CgroupError, CgroupRoot, IdentityError, LaunchError, LaunchRequest,
    LaunchedProcess, ProvisionError, ProvisionRequest, Purpose, ResolvedAccount, ResourceLimits,
    launch, provision, resolve,
};
use std::collections::HashMap;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

const GRACE: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(20);

/// Prints what test 2 and test 10 check: the ids, the process status and
/// the environment, separated so the assertions can find each part.
const IDENTITY_SCRIPT: &str = "id -u; id -g; echo ---; cat /proc/self/status; echo ---; env";

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// One test's slice of the Host: a cgroup root, the accounts it provisioned
/// and any extra path it created. `Drop` takes all of it away again.
struct Fixture {
    n: u32,
    root_path: PathBuf,
    root: CgroupRoot,
    accounts: Vec<(AccountName, PathBuf)>,
    extra_paths: Vec<PathBuf>,
}

#[derive(Clone)]
struct Provisioned {
    application_id: String,
    account: ResolvedAccount,
}

struct Finished {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl Fixture {
    fn new() -> Self {
        // SAFETY: geteuid takes no arguments and cannot fail.
        if unsafe { libc::geteuid() } != 0 {
            panic!("these tests provision accounts and cgroups and need root on a disposable Host");
        }
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root_path = PathBuf::from(format!("/sys/fs/cgroup/sf-test-{}-{n}", std::process::id()));
        std::fs::create_dir(&root_path).expect("create the test's cgroup root");
        let root = CgroupRoot::at(&root_path).expect("the test's cgroup root is usable");
        Fixture {
            n,
            root_path,
            root,
            accounts: Vec::new(),
            extra_paths: Vec::new(),
        }
    }

    fn account_name(&self, k: u32) -> AccountName {
        AccountName::parse(&format!("sf-t{}{}{k}", std::process::id(), self.n)).unwrap()
    }

    /// Provisions the k-th account of this test and remembers it for cleanup.
    fn account(&mut self, k: u32) -> Provisioned {
        let name = self.account_name(k);
        let home = PathBuf::from(format!("/home/{}", name.as_str()));
        let application_id = format!("t{}{}{k}", std::process::id(), self.n);
        self.accounts.push((name.clone(), home.clone()));
        let account = provision(&ProvisionRequest {
            application_id: &application_id,
            account: &name,
            home: &home,
        })
        .expect("provision the account");
        Provisioned {
            application_id,
            account,
        }
    }

    fn request(&self, who: &Provisioned, command: &[&str]) -> LaunchRequest {
        LaunchRequest {
            application_id: who.application_id.clone(),
            account: who.account.name.clone(),
            command: command.iter().map(|s| s.to_string()).collect(),
            working_dir: who.account.home.clone(),
            environment: Vec::new(),
            limits: ResourceLimits::NONE,
            purpose: Purpose::Main,
        }
    }

    fn launch(&self, request: &LaunchRequest) -> LaunchedProcess {
        launch(&self.root, request).expect("launch")
    }

    /// Launches, waits for the command to end on its own, then tears down.
    fn run_to_end(&self, request: &LaunchRequest) -> Finished {
        let mut process = self.launch(request);
        let finished = collect(&mut process);
        process.stop(GRACE).expect("stop");
        finished
    }

    fn child_cgroups(&self) -> Vec<PathBuf> {
        std::fs::read_dir(&self.root_path)
            .expect("read the cgroup root")
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.path())
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for path in self.child_cgroups() {
            let _ = std::fs::write(path.join("cgroup.kill"), "1");
            let deadline = Instant::now() + GRACE;
            while Instant::now() < deadline {
                let procs = std::fs::read_to_string(path.join("cgroup.procs")).unwrap_or_default();
                if procs.trim().is_empty() {
                    break;
                }
                std::thread::sleep(POLL);
            }
            let _ = std::fs::remove_dir(&path);
        }
        let _ = std::fs::remove_dir(&self.root_path);
        for (name, home) in &self.accounts {
            let _ = Command::new("userdel")
                .args(["--remove", name.as_str()])
                .output();
            let _ = std::fs::remove_dir_all(home);
        }
        for path in &self.extra_paths {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

/// Reads both pipes to their end and reaps the leader. Only for commands
/// that end on their own and leave no child holding the pipes.
fn collect(process: &mut LaunchedProcess) -> Finished {
    let mut out = process.child.stdout.take().expect("stdout is piped");
    let mut err = process.child.stderr.take().expect("stderr is piped");
    let stderr = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let mut stdout = String::new();
    out.read_to_string(&mut stdout).expect("read stdout");
    let stderr = stderr.join().expect("stderr reader");
    let status = process.child.wait().expect("wait for the leader");
    Finished {
        status,
        stdout,
        stderr,
    }
}

fn passwd_line(name: &str) -> Vec<String> {
    std::fs::read_to_string("/etc/passwd")
        .expect("read /etc/passwd")
        .lines()
        .map(|line| line.split(':').map(str::to_string).collect::<Vec<_>>())
        .find(|fields| fields.first().map(String::as_str) == Some(name))
        .unwrap_or_else(|| panic!("{name} is not in /etc/passwd"))
}

/// Every group that lists the account as a member, from /etc/group.
fn supplementary_groups(name: &str) -> Vec<String> {
    std::fs::read_to_string("/etc/group")
        .expect("read /etc/group")
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            let members = fields.get(3)?.split(',');
            members
                .clone()
                .any(|m| m == name)
                .then(|| fields[0].to_string())
        })
        .collect()
}

fn set_leaky_environment() {
    for (name, value) in [
        ("SUDO_USER", "operator"),
        ("SSH_AUTH_SOCK", "/tmp/agent.sock"),
        ("DOCKER_HOST", "unix:///var/run/docker.sock"),
        ("SELF_HOST_API_KEY", "change-me"),
    ] {
        // SAFETY: the test binary runs these tests one at a time and no other
        // thread reads the environment while this runs.
        unsafe { std::env::set_var(name, value) };
    }
}

fn assert_identity(finished: &Finished, account: &ResolvedAccount) {
    assert!(
        finished.status.success(),
        "identity script failed: {}",
        finished.stderr
    );
    let mut sections = finished.stdout.split("---\n");
    let ids = sections.next().expect("ids");
    let status = sections.next().expect("status");
    let env = sections.next().expect("env");

    let mut id_lines = ids.lines();
    let uid: u32 = id_lines.next().unwrap().trim().parse().unwrap();
    let gid: u32 = id_lines.next().unwrap().trim().parse().unwrap();
    assert_ne!(uid, 0);
    assert_ne!(gid, 0);
    assert_eq!(uid, account.uid);
    assert_eq!(gid, account.gid);

    let field = |name: &str| -> String {
        status
            .lines()
            .find(|l| l.starts_with(name))
            .map(|l| l[name.len()..].trim().to_string())
            .unwrap_or_else(|| panic!("no {name} in /proc/self/status"))
    };
    for cap in ["CapEff:", "CapPrm:", "CapBnd:", "CapAmb:"] {
        assert_eq!(field(cap), "0000000000000000", "{cap} should be empty");
    }
    assert_eq!(field("NoNewPrivs:"), "1");
    assert_eq!(field("Groups:"), "", "no supplementary groups");

    let vars: HashMap<&str, &str> = env.lines().filter_map(|l| l.split_once('=')).collect();
    assert_eq!(vars.get("HOME"), Some(&account.home.to_str().unwrap()));
    assert_eq!(vars.get("USER"), Some(&account.name.as_str()));
    assert_eq!(vars.get("LOGNAME"), Some(&account.name.as_str()));
    for leaked in [
        "SUDO_USER",
        "SSH_AUTH_SOCK",
        "DOCKER_HOST",
        "SELF_HOST_API_KEY",
    ] {
        assert!(
            !vars.contains_key(leaked),
            "{leaked} leaked into the Application"
        );
    }
}

fn read_event(path: &Path, key: &str) -> u64 {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
        .lines()
        .find_map(|line| {
            let (k, v) = line.split_once(' ')?;
            (k == key).then(|| v.trim().parse().ok()).flatten()
        })
        .unwrap_or_else(|| panic!("no {key} in {}", path.display()))
}

fn wait_until(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(POLL);
    }
}

fn is_gone(pid: u32) -> bool {
    // SAFETY: signal 0 delivers nothing; it only asks whether the pid exists.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    rc != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

// 1. Provisioning
#[test]
#[ignore = "needs root on a disposable Linux Host"]
fn provisioning_makes_a_locked_down_system_account_that_other_applications_cannot_read() {
    let mut fixture = Fixture::new();
    let first = fixture.account(1);
    let second = fixture.account(2);

    let account = &first.account;
    assert_ne!(account.uid, 0);
    assert_ne!(account.gid, 0);
    assert_ne!(first.account.uid, second.account.uid);
    assert_ne!(first.account.name, second.account.name);

    let passwd = passwd_line(account.name.as_str());
    assert_eq!(passwd[6], "/usr/sbin/nologin", "shell");
    assert!(
        passwd[2].parse::<u32>().unwrap() < 1000,
        "a system account, uid {}",
        passwd[2]
    );

    let home = std::fs::metadata(&account.home).expect("home exists");
    assert_eq!(home.mode() & 0o777, 0o750);
    assert_eq!(home.uid(), account.uid);
    assert_eq!(home.gid(), account.gid);

    let groups = supplementary_groups(account.name.as_str());
    assert!(groups.is_empty(), "supplementary groups: {groups:?}");

    // Provisioning again is a no-op, and a name the Platform did not create
    // is refused.
    let again = provision(&ProvisionRequest {
        application_id: &first.application_id,
        account: &account.name,
        home: &account.home,
    })
    .expect("provision twice");
    assert_eq!(again.uid, account.uid);
    let stolen = provision(&ProvisionRequest {
        application_id: &second.application_id,
        account: &account.name,
        home: &account.home,
    });
    assert!(
        matches!(stolen, Err(ProvisionError::NotOurs(_))),
        "{stolen:?}"
    );
    let nobody = AccountName::parse("nobody").unwrap();
    let taken = provision(&ProvisionRequest {
        application_id: &first.application_id,
        account: &nobody,
        home: Path::new("/nonexistent"),
    });
    assert!(
        matches!(taken, Err(ProvisionError::NotOurs(_))),
        "{taken:?}"
    );

    // The second Application cannot read the first one's home.
    let secret = account.home.join("secret");
    std::fs::write(&secret, "change-me").unwrap();
    std::os::unix::fs::chown(&secret, Some(account.uid), Some(account.gid)).unwrap();
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o640)).unwrap();
    let finished =
        fixture.run_to_end(&fixture.request(&second, &["cat", secret.to_str().unwrap()]));
    assert!(
        !finished.status.success(),
        "cat should fail: {}",
        finished.stdout
    );
    assert!(
        finished.stderr.contains("Permission denied"),
        "stderr: {}",
        finished.stderr
    );
    assert!(finished.stdout.is_empty());
}

// 2. Identity in the launched process
#[test]
#[ignore = "needs root on a disposable Linux Host"]
fn a_launched_process_is_the_account_with_no_capabilities_no_groups_and_a_clean_environment() {
    set_leaky_environment();
    let mut fixture = Fixture::new();
    let who = fixture.account(1);
    let finished = fixture.run_to_end(&fixture.request(&who, &["sh", "-c", IDENTITY_SCRIPT]));
    assert_identity(&finished, &who.account);
}

// 3. Refusals with nothing spawned
#[test]
#[ignore = "needs root on a disposable Linux Host"]
fn every_refusal_happens_before_anything_is_spawned() {
    let mut fixture = Fixture::new();
    let who = fixture.account(1);

    assert!(matches!(AccountName::parse(""), Err(IdentityError::Empty)));
    assert!(matches!(
        AccountName::parse("root"),
        Err(IdentityError::Root(_))
    ));

    let unknown = fixture.account_name(9);
    assert!(matches!(resolve(&unknown), Err(IdentityError::Unknown(_))));
    let mut request = fixture.request(&who, &["sleep", "30"]);
    request.account = unknown;
    let result = launch(&fixture.root, &request);
    assert!(
        matches!(
            result,
            Err(LaunchError::Identity(IdentityError::Unknown(_)))
        ),
        "{result:?}"
    );

    let mut request = fixture.request(&who, &[]);
    request.command.clear();
    let result = launch(&fixture.root, &request);
    assert!(
        matches!(result, Err(LaunchError::EmptyCommand)),
        "{result:?}"
    );

    let mut request = fixture.request(&who, &["sleep", "30"]);
    request.working_dir = PathBuf::from("/tmp");
    let result = launch(&fixture.root, &request);
    assert!(
        matches!(result, Err(LaunchError::WorkingDirOutsideHome { .. })),
        "{result:?}"
    );

    let mut request = fixture.request(&who, &["sleep", "30"]);
    request.environment.push(("HOME".into(), "/tmp".into()));
    let result = launch(&fixture.root, &request);
    assert!(
        matches!(&result, Err(LaunchError::ReservedVariable(name)) if name == "HOME"),
        "{result:?}"
    );

    let not_cgroup =
        std::env::temp_dir().join(format!("sf-test-not-cgroup-{}", std::process::id()));
    std::fs::create_dir_all(&not_cgroup).unwrap();
    fixture.extra_paths.push(not_cgroup.clone());
    let result = CgroupRoot::at(&not_cgroup);
    assert!(
        matches!(result, Err(CgroupError::NotCgroup2(_))),
        "{result:?}"
    );

    assert!(
        fixture.child_cgroups().is_empty(),
        "no cgroup was created, so nothing ran"
    );
}

// 4. Descendants
#[test]
#[ignore = "needs root on a disposable Linux Host"]
fn stop_takes_the_detached_grandchild_with_it_and_removes_the_cgroup() {
    let mut fixture = Fixture::new();
    let who = fixture.account(1);
    let mut process =
        fixture.launch(&fixture.request(&who, &["sh", "-c", "setsid sleep 300 & sleep 300"]));
    let cgroup_path = process.cgroup.path().to_path_buf();

    let mut pids = Vec::new();
    wait_until("the tree to fill", GRACE, || {
        pids = process.cgroup.processes().unwrap();
        pids.len() >= 3
    });
    assert!(pids.len() >= 2, "pids: {pids:?}");
    assert!(pids.contains(&process.pid));

    process.stop(GRACE).expect("stop");

    assert!(!cgroup_path.exists(), "the cgroup directory is gone");
    for pid in pids {
        wait_until(&format!("pid {pid} to be gone"), GRACE, || is_gone(pid));
    }
}

// 5. Task limit
#[test]
#[ignore = "needs root on a disposable Linux Host"]
fn a_task_limit_stops_a_fork_loop_at_eight() {
    let mut fixture = Fixture::new();
    let who = fixture.account(1);
    let mut request = fixture.request(
        &who,
        &[
            "sh",
            "-c",
            "i=0; while [ $i -lt 30 ]; do sleep 30 & i=$((i+1)); done; wait",
        ],
    );
    request.limits.max_tasks = Some(8);
    let mut process = fixture.launch(&request);
    let cgroup_path = process.cgroup.path().to_path_buf();

    assert_eq!(
        std::fs::read_to_string(cgroup_path.join("pids.max"))
            .unwrap()
            .trim(),
        "8"
    );
    let mut most_seen = 0;
    let deadline = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < deadline {
        let count = process.cgroup.processes().unwrap().len();
        assert!(count <= 8, "{count} pids in the cgroup");
        most_seen = most_seen.max(count);
        std::thread::sleep(POLL);
    }
    assert!(most_seen >= 2, "the loop did fork: {most_seen}");
    wait_until("pids.events to count a refused fork", GRACE, || {
        read_event(&cgroup_path.join("pids.events"), "max") > 0
    });
    if let Ok(peak) = std::fs::read_to_string(cgroup_path.join("pids.peak")) {
        assert!(peak.trim().parse::<u32>().unwrap() <= 8, "pids.peak {peak}");
    }

    process.stop(GRACE).expect("stop");
    assert!(!cgroup_path.exists());
}

// 6. Memory limit
#[test]
#[ignore = "needs root on a disposable Linux Host"]
fn a_memory_limit_kills_the_tree_when_it_allocates_past_it() {
    let mut fixture = Fixture::new();
    let who = fixture.account(1);
    let mut request = fixture.request(
        &who,
        &[
            "python3",
            "-c",
            "x = b'x' * (128 * 1024 * 1024); print(len(x))",
        ],
    );
    request.limits.memory_bytes = Some(32 * 1024 * 1024);
    let mut process = fixture.launch(&request);
    let cgroup_path = process.cgroup.path().to_path_buf();

    assert_eq!(
        std::fs::read_to_string(cgroup_path.join("memory.max"))
            .unwrap()
            .trim(),
        "33554432"
    );
    assert_eq!(
        std::fs::read_to_string(cgroup_path.join("memory.oom.group"))
            .unwrap()
            .trim(),
        "1"
    );
    let finished = collect(&mut process);
    assert_eq!(
        finished.status.signal(),
        Some(libc::SIGKILL),
        "status {:?}, stdout {:?}, stderr {:?}",
        finished.status,
        finished.stdout,
        finished.stderr
    );
    assert!(read_event(&cgroup_path.join("memory.events"), "oom_kill") > 0);

    process.stop(GRACE).expect("stop");
    assert!(!cgroup_path.exists());
}

// 7. CPU limit
#[test]
#[ignore = "needs root on a disposable Linux Host"]
fn a_cpu_limit_is_written_as_a_quota_over_the_period() {
    let mut fixture = Fixture::new();
    let who = fixture.account(1);
    let mut request = fixture.request(&who, &["sleep", "30"]);
    request.limits.cpu_percent = Some(50);
    let mut process = fixture.launch(&request);
    let cpu_max = std::fs::read_to_string(process.cgroup.path().join("cpu.max")).unwrap();
    assert_eq!(cpu_max.trim(), "50000 100000");
    process.stop(GRACE).expect("stop");
}

// 8. Platform credentials
#[test]
#[ignore = "needs root on a disposable Linux Host"]
fn an_application_cannot_read_the_platforms_credentials() {
    let mut fixture = Fixture::new();
    let who = fixture.account(1);
    let dir = PathBuf::from(format!(
        "/var/lib/sf-test-{}-{}-credentials",
        std::process::id(),
        fixture.n
    ));
    std::fs::create_dir_all(&dir).unwrap();
    fixture.extra_paths.push(dir.clone());
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let file = dir.join("api-key");
    std::fs::write(&file, "change-me").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(std::fs::metadata(&file).unwrap().uid(), 0);

    let finished = fixture.run_to_end(&fixture.request(&who, &["cat", file.to_str().unwrap()]));
    assert!(!finished.status.success());
    assert!(finished.stdout.is_empty(), "stdout: {}", finished.stdout);
    assert!(
        finished.stderr.contains("Permission denied"),
        "stderr: {}",
        finished.stderr
    );
}

// 9. No container engine or sudo
#[test]
#[ignore = "needs root on a disposable Linux Host"]
fn an_application_account_reaches_neither_docker_nor_sudo() {
    let mut fixture = Fixture::new();
    let who = fixture.account(1);
    let groups = supplementary_groups(who.account.name.as_str());
    assert!(!groups.iter().any(|g| g == "sudo"), "{groups:?}");
    assert!(!groups.iter().any(|g| g == "docker"), "{groups:?}");

    if Path::new("/var/run/docker.sock").exists() {
        let finished = fixture
            .run_to_end(&fixture.request(&who, &["sh", "-c", "test -w /var/run/docker.sock"]));
        assert!(!finished.status.success(), "the Docker socket is writable");
    }

    if Path::new("/usr/bin/sudo").exists() {
        let finished = fixture.run_to_end(&fixture.request(&who, &["sudo", "-n", "true"]));
        assert!(
            !finished.status.success(),
            "sudo succeeded: {}",
            finished.stdout
        );
    }
}

// 10. Hooks, builds and terminals
#[test]
#[ignore = "needs root on a disposable Linux Host"]
fn every_purpose_runs_under_the_same_identity() {
    set_leaky_environment();
    let mut fixture = Fixture::new();
    let who = fixture.account(1);
    for purpose in Purpose::ALL {
        let mut request = fixture.request(&who, &["sh", "-c", IDENTITY_SCRIPT]);
        request.purpose = purpose;
        let finished = fixture.run_to_end(&request);
        assert_identity(&finished, &who.account);
    }
}
