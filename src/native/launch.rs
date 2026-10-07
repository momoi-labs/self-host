//! Launching a command as an Application Account inside its cgroup.
//!
//! Every check happens before anything is forked: the account resolves and
//! is not root, the working directory is inside its home, the command and
//! the environment are acceptable, the cgroup exists with its limits. Then
//! the child joins the cgroup, drops every privilege and only then runs the
//! command. A step that fails makes the child exit without running anything;
//! there is no fallback and never a run as root.

use super::ResourceLimits;
#[cfg(target_os = "linux")]
use super::cgroup::ApplicationCgroup;
use super::cgroup::CgroupError;
#[cfg(not(target_os = "macos"))]
use super::cgroup::CgroupRoot;
use super::identity::{AccountName, IdentityError, ResolvedAccount};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

/// The `PATH` an Application starts with. The request may replace it; it
/// may not replace the identity variables.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const DEFAULT_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// Variables the Platform sets from the Application Account and a request
/// may not override.
const RESERVED_VARIABLES: [&str; 3] = ["HOME", "USER", "LOGNAME"];

/// How often `stop` looks at the leader while waiting for it to exit.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// What a launch is for. Every purpose runs under the same identity policy;
/// a hook, a build or a terminal gets no more than the Application does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Main,
    Hook,
    Build,
    Terminal,
}

impl Purpose {
    pub const ALL: [Purpose; 4] = [
        Purpose::Main,
        Purpose::Hook,
        Purpose::Build,
        Purpose::Terminal,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRequest {
    pub application_id: String,
    pub account: AccountName,
    /// argv; argv[0] is the program. Non-empty.
    pub command: Vec<String>,
    /// Absolute, and inside or equal to the account's home.
    pub working_dir: PathBuf,
    /// Added on top of what the Platform sets. May not set `HOME`, `USER`
    /// or `LOGNAME`.
    pub environment: Vec<(String, String)>,
    pub limits: ResourceLimits,
    pub purpose: Purpose,
}

/// A running command: its leader and the cgroup its whole tree lives in.
#[derive(Debug)]
pub struct LaunchedProcess {
    pub pid: u32,
    pub child: std::process::Child,
    #[cfg(target_os = "linux")]
    pub cgroup: ApplicationCgroup,
}

impl LaunchedProcess {
    /// SIGTERM to the leader, up to `grace` for it to leave, then
    /// `cgroup.kill` for whatever remains. Ends with the cgroup empty and
    /// removed, and the leader reaped.
    pub fn stop(&mut self, grace: Duration) -> Result<(), StopError> {
        let running = self.child.try_wait().map_err(StopError::Wait)?.is_none();
        if running {
            // SAFETY: the pid is our own child and it has not been reaped, so
            // the kernel cannot have handed the number to anyone else.
            let rc = unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGTERM) };
            if rc != 0 {
                let e = std::io::Error::last_os_error();
                if e.raw_os_error() != Some(libc::ESRCH) {
                    return Err(StopError::Signal(e));
                }
            }
            let deadline = std::time::Instant::now() + grace;
            while self.child.try_wait().map_err(StopError::Wait)?.is_none() {
                if std::time::Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
        }
        #[cfg(target_os = "linux")]
        self.cgroup.kill(grace).map_err(StopError::Cgroup)?;
        #[cfg(target_os = "macos")]
        if self.child.try_wait().map_err(StopError::Wait)?.is_none() {
            self.child.kill().map_err(StopError::Signal)?;
        }
        self.child.wait().map_err(StopError::Wait)?;
        Ok(())
    }
}

#[derive(Debug)]
pub enum LaunchError {
    Identity(IdentityError),
    Cgroup(CgroupError),
    EmptyCommand,
    WorkingDirOutsideHome {
        home: PathBuf,
        requested: PathBuf,
    },
    /// The request tried to set a variable the Platform owns.
    ReservedVariable(String),
    /// The fork, a step before exec, or exec itself failed.
    Spawn(std::io::Error),
    /// Not Linux.
    Unsupported,
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LaunchError::Identity(_) => {
                write!(f, "the Application Account cannot run the command")
            }
            LaunchError::Cgroup(_) => write!(f, "the Application's cgroup could not be prepared"),
            LaunchError::EmptyCommand => write!(f, "the command is empty"),
            LaunchError::WorkingDirOutsideHome { home, requested } => write!(
                f,
                "the working directory '{}' is outside the Application Account's home '{}'",
                requested.display(),
                home.display()
            ),
            LaunchError::ReservedVariable(name) => write!(
                f,
                "the environment may not set '{name}'; the Platform sets it from the Application Account"
            ),
            LaunchError::Spawn(_) => write!(f, "failed to start the command"),
            LaunchError::Unsupported => {
                write!(f, "the requested native launch is unsupported on this Host")
            }
        }
    }
}

impl std::error::Error for LaunchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LaunchError::Identity(e) => Some(e),
            LaunchError::Cgroup(e) => Some(e),
            LaunchError::Spawn(e) => Some(e),
            _ => None,
        }
    }
}

impl From<IdentityError> for LaunchError {
    fn from(e: IdentityError) -> Self {
        LaunchError::Identity(e)
    }
}

impl From<CgroupError> for LaunchError {
    fn from(e: CgroupError) -> Self {
        LaunchError::Cgroup(e)
    }
}

#[derive(Debug)]
pub enum StopError {
    Signal(std::io::Error),
    Wait(std::io::Error),
    Cgroup(CgroupError),
}

impl std::fmt::Display for StopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StopError::Signal(_) => write!(f, "failed to signal the Application's process"),
            StopError::Wait(_) => write!(f, "failed to wait for the Application's process"),
            StopError::Cgroup(_) => write!(f, "the Application's cgroup could not be torn down"),
        }
    }
}

impl std::error::Error for StopError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            StopError::Signal(e) | StopError::Wait(e) => Some(e),
            StopError::Cgroup(e) => Some(e),
        }
    }
}

/// The checks that need nothing from the Host: a command to run and an
/// environment that leaves the identity variables alone.
fn validate(request: &LaunchRequest) -> Result<(), LaunchError> {
    if request.command.is_empty() || request.command[0].is_empty() {
        return Err(LaunchError::EmptyCommand);
    }
    for (name, _) in &request.environment {
        if RESERVED_VARIABLES.contains(&name.as_str()) {
            return Err(LaunchError::ReservedVariable(name.clone()));
        }
    }
    Ok(())
}

/// The working directory must be absolute, spelled without `..`, and be the
/// home or something under it. The check is on the path's components,
/// so `/home/a-other` is not inside `/home/a`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn working_dir_within(home: &Path, requested: &Path) -> Result<(), LaunchError> {
    let outside = || LaunchError::WorkingDirOutsideHome {
        home: home.to_path_buf(),
        requested: requested.to_path_buf(),
    };
    if !requested.is_absolute() || !home.is_absolute() {
        return Err(outside());
    }
    let plain = requested
        .components()
        .all(|c| matches!(c, Component::RootDir | Component::Normal(_)));
    if !plain || !requested.starts_with(home) {
        return Err(outside());
    }
    Ok(())
}

/// The environment the command starts with: the account's identity, a
/// fixed `PATH`, then the request's variables on top. A request may replace
/// `PATH`; `validate` already refused the identity variables.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn environment_for(account: &ResolvedAccount, request: &LaunchRequest) -> Vec<(String, String)> {
    let mut environment: Vec<(String, String)> = vec![
        ("HOME".into(), account.home.display().to_string()),
        ("USER".into(), account.name.as_str().to_string()),
        ("LOGNAME".into(), account.name.as_str().to_string()),
        ("PATH".into(), DEFAULT_PATH.into()),
    ];
    for (name, value) in &request.environment {
        environment.retain(|(existing, _)| existing != name);
        environment.push((name.clone(), value.clone()));
    }
    environment
}

/// s6 owns the Darwin session and cleans its foreground process group.
#[cfg(target_os = "macos")]
pub fn launch(request: &LaunchRequest) -> Result<LaunchedProcess, LaunchError> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let account = super::identity::resolve(&request.account)?;
    working_dir_within(&account.home, &request.working_dir)?;
    validate(request)?;
    if request.limits != ResourceLimits::NONE {
        return Err(LaunchError::Unsupported);
    }
    let directory = std::ffi::CString::new(request.working_dir.as_os_str().as_bytes())
        .map_err(|_| LaunchError::Spawn(std::io::Error::from_raw_os_error(libc::EINVAL)))?;
    let mut command = Command::new(&request.command[0]);
    command
        .args(&request.command[1..])
        .env_clear()
        .envs(environment_for(&account, request))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: only system calls and stack data are used between fork and exec.
    unsafe {
        command.pre_exec(move || {
            become_account_macos(account.uid, account.gid)?;
            if libc::chdir(directory.as_ptr()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn().map_err(LaunchError::Spawn)?;
    Ok(LaunchedProcess {
        pid: child.id(),
        child,
    })
}

#[cfg(target_os = "macos")]
fn become_account_macos(uid: u32, gid: u32) -> std::io::Result<()> {
    // Darwin represents an empty group list as group 0. An explicit primary
    // group also opts this process out of memberd's supplementary expansion.
    unsafe {
        if uid == 0 || gid == 0 || libc::geteuid() != 0 {
            return Err(std::io::Error::from_raw_os_error(libc::EPERM));
        }
        if libc::setgroups(1, &gid) != 0 || libc::setgid(gid) != 0 || libc::setuid(uid) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = std::mem::size_of_val(&info) as i32;
        let mut groups = [u32::MAX; 2];
        if libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        ) != size
            || [info.pbi_ruid, info.pbi_uid, info.pbi_svuid] != [uid; 3]
            || [info.pbi_rgid, info.pbi_gid, info.pbi_svgid] != [gid; 3]
            || libc::getgroups(2, groups.as_mut_ptr()) != 1
            || groups[0] != gid
        {
            return Err(std::io::Error::from_raw_os_error(libc::EPERM));
        }
        libc::umask(0o027);
    }
    Ok(())
}

/// Runs the request's command as its Application Account inside a fresh
/// cgroup under `root`. Everything is checked first; a refusal spawns
/// nothing and leaves no cgroup behind.
#[cfg(target_os = "linux")]
pub fn launch(root: &CgroupRoot, request: &LaunchRequest) -> Result<LaunchedProcess, LaunchError> {
    let account = super::identity::resolve(&request.account)?;
    working_dir_within(&account.home, &request.working_dir)?;
    validate(request)?;
    let cgroup = root.create(&request.application_id, &request.limits)?;
    match launch_in(&cgroup, request) {
        Ok(process) => Ok(process),
        Err(error) => {
            let _ = cgroup.remove();
            Err(error)
        }
    }
}

/// Readiness runs in the main command's existing resource context, with
/// exactly the same identity checks and privilege drop as a fresh launch.
#[cfg(target_os = "linux")]
pub(crate) fn launch_in(
    cgroup: &ApplicationCgroup,
    request: &LaunchRequest,
) -> Result<LaunchedProcess, LaunchError> {
    use std::process::Stdio;
    let child = command_in(cgroup, request)?
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(LaunchError::Spawn)?;
    Ok(LaunchedProcess {
        pid: child.id(),
        child,
        cgroup: cgroup.clone(),
    })
}

/// Main processes and terminals share the exact privilege-drop boundary.
/// Opening an existing cgroup never rewrites its limits or its membership.
#[cfg(target_os = "linux")]
pub(crate) fn command_in(
    cgroup: &ApplicationCgroup,
    request: &LaunchRequest,
) -> Result<std::process::Command, LaunchError> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::process::CommandExt;
    let account = super::identity::resolve(&request.account)?;
    working_dir_within(&account.home, &request.working_dir)?;
    validate(request)?;
    let last_cap = read_cap_last_cap()?;
    let procs_path = cgroup.path().join("cgroup.procs");
    let procs = std::fs::OpenOptions::new()
        .write(true)
        .open(&procs_path)
        .map_err(|source| CgroupError::Io {
            path: procs_path,
            source,
        })?;
    let mut command = std::process::Command::new(&request.command[0]);
    command
        .args(&request.command[1..])
        .env_clear()
        .envs(environment_for(&account, request));
    let directory = std::ffi::CString::new(request.working_dir.as_os_str().as_bytes())
        .map_err(|_| LaunchError::Spawn(std::io::Error::from_raw_os_error(libc::EINVAL)))?;
    let (uid, gid) = (account.uid, account.gid);
    // SAFETY: only raw system calls run after fork. The captured file keeps
    // the cgroup fd open until spawn completes and is closed on exec.
    unsafe {
        command.pre_exec(move || {
            become_account(procs.as_raw_fd(), uid, gid, last_cap)?;
            // Resolve the working directory only after dropping privileges.
            if libc::chdir(directory.as_ptr()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(command)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn launch(_root: &CgroupRoot, request: &LaunchRequest) -> Result<LaunchedProcess, LaunchError> {
    validate(request)?;
    Err(LaunchError::Unsupported)
}

/// The highest capability number this kernel knows, read before forking so
/// the child has nothing to open.
#[cfg(target_os = "linux")]
fn read_cap_last_cap() -> Result<u32, LaunchError> {
    let text =
        std::fs::read_to_string("/proc/sys/kernel/cap_last_cap").map_err(LaunchError::Spawn)?;
    text.trim().parse().map_err(|_| {
        LaunchError::Spawn(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "/proc/sys/kernel/cap_last_cap is not a number",
        ))
    })
}

/// Runs in the child between fork and exec. Every step fails closed: an
/// error here makes the child exit and `spawn` report it, and the command
/// never runs. Only raw OS errors survive the trip back to the parent, so
/// the checks report `EPERM` rather than a sentence.
///
/// The bounding set is dropped before the uid changes because
/// `PR_CAPBSET_DROP` needs `CAP_SETPCAP`, which the uid change takes away.
#[cfg(target_os = "linux")]
fn become_account(procs_fd: i32, uid: u32, gid: u32, last_cap: u32) -> std::io::Result<()> {
    write_own_pid(procs_fd)?;

    // SAFETY: prctl with these arguments reads nothing from memory.
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }

    for cap in 0..=last_cap {
        // SAFETY: prctl with these arguments reads nothing from memory.
        if unsafe { libc::prctl(libc::PR_CAPBSET_DROP, cap, 0, 0, 0) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }

    // SAFETY: a zero-length group list; the pointer is never dereferenced.
    if unsafe { libc::setgroups(0, std::ptr::null()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: plain system calls on integers.
    if unsafe { libc::setresgid(gid, gid, gid) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: plain system calls on integers.
    if unsafe { libc::setresuid(uid, uid, uid) } != 0 {
        return Err(std::io::Error::last_os_error());
    }

    let (mut ruid, mut euid, mut suid) = (u32::MAX, u32::MAX, u32::MAX);
    let (mut rgid, mut egid, mut sgid) = (u32::MAX, u32::MAX, u32::MAX);
    // SAFETY: the out-pointers are valid stack locations for the call.
    if unsafe { libc::getresuid(&mut ruid, &mut euid, &mut suid) } != 0
        || unsafe { libc::getresgid(&mut rgid, &mut egid, &mut sgid) } != 0
    {
        return Err(std::io::Error::last_os_error());
    }
    let ids = [ruid, euid, suid, rgid, egid, sgid];
    let expected = [uid, uid, uid, gid, gid, gid];
    if ids.contains(&0) || ids != expected {
        return Err(std::io::Error::from_raw_os_error(libc::EPERM));
    }

    // SAFETY: prctl with these arguments reads nothing from memory.
    if unsafe {
        libc::prctl(
            libc::PR_CAP_AMBIENT,
            libc::PR_CAP_AMBIENT_CLEAR_ALL,
            0,
            0,
            0,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error());
    }

    // SAFETY: umask cannot fail.
    unsafe {
        libc::umask(0o027);
    }
    Ok(())
}

/// Writes the child's pid to the already open `cgroup.procs`, formatting it
/// on the stack: no allocation between fork and exec.
#[cfg(target_os = "linux")]
fn write_own_pid(procs_fd: i32) -> std::io::Result<()> {
    // SAFETY: getpid cannot fail.
    let pid = unsafe { libc::getpid() };
    let mut digits = [0u8; 20];
    let mut start = digits.len();
    let mut value = pid.unsigned_abs();
    loop {
        start -= 1;
        digits[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    let text = &digits[start..];
    // SAFETY: the buffer is valid for `text.len()` bytes and the descriptor
    // was opened by the parent for writing.
    let written = unsafe { libc::write(procs_fd, text.as_ptr().cast(), text.len()) };
    if written < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if written as usize != text.len() {
        return Err(std::io::Error::from_raw_os_error(libc::EIO));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> LaunchRequest {
        LaunchRequest {
            application_id: "k3n8qz4v2x1p".into(),
            account: AccountName::parse("sf-app-k3n8qz4v2x1p").unwrap(),
            command: vec!["/opt/api/bin/serve".into(), "--port".into(), "8080".into()],
            working_dir: PathBuf::from("/var/lib/self-host/apps/k3n8qz4v2x1p"),
            environment: vec![("PORT".into(), "8080".into())],
            limits: ResourceLimits::NONE,
            purpose: Purpose::Main,
        }
    }

    fn account() -> ResolvedAccount {
        ResolvedAccount {
            name: AccountName::parse("sf-app-k3n8qz4v2x1p").unwrap(),
            uid: 998,
            gid: 998,
            home: PathBuf::from("/var/lib/self-host/apps/k3n8qz4v2x1p"),
        }
    }

    #[test]
    fn a_complete_request_validates() {
        assert!(validate(&request()).is_ok());
    }

    #[test]
    fn an_empty_command_is_refused() {
        let mut r = request();
        r.command.clear();
        assert!(matches!(validate(&r), Err(LaunchError::EmptyCommand)));
        r.command = vec![String::new()];
        assert!(matches!(validate(&r), Err(LaunchError::EmptyCommand)));
    }

    #[test]
    fn the_identity_variables_may_not_be_overridden() {
        for name in ["HOME", "USER", "LOGNAME"] {
            let mut r = request();
            r.environment.push((name.into(), "/tmp".into()));
            assert!(
                matches!(validate(&r), Err(LaunchError::ReservedVariable(n)) if n == name),
                "{name} should be reserved"
            );
        }
    }

    #[test]
    fn the_home_itself_and_anything_under_it_are_inside() {
        let home = Path::new("/var/lib/self-host/apps/k3n8qz4v2x1p");
        assert!(working_dir_within(home, home).is_ok());
        assert!(working_dir_within(home, &home.join("src")).is_ok());
        assert!(working_dir_within(home, &home.join("a/b/c")).is_ok());
    }

    #[test]
    fn a_sibling_a_parent_a_relative_path_or_a_dot_dot_escape_is_outside() {
        let home = Path::new("/var/lib/self-host/apps/k3n8qz4v2x1p");
        for requested in [
            "/var/lib/self-host/apps/other",
            "/var/lib/self-host/apps/k3n8qz4v2x1p-other",
            "/var/lib/self-host/apps",
            "/tmp",
            "/",
            "src",
            "/var/lib/self-host/apps/k3n8qz4v2x1p/../other",
        ] {
            let result = working_dir_within(home, Path::new(requested));
            assert!(
                matches!(
                    &result,
                    Err(LaunchError::WorkingDirOutsideHome { home: h, requested: r })
                        if h == home && r == Path::new(requested)
                ),
                "{requested} should be outside, got {result:?}"
            );
        }
    }

    #[test]
    fn a_relative_home_puts_everything_outside() {
        assert!(working_dir_within(Path::new("apps/x"), Path::new("/apps/x")).is_err());
    }

    #[test]
    fn the_environment_starts_from_the_account_and_a_fixed_path() {
        let env = environment_for(&account(), &request());
        assert_eq!(
            env,
            vec![
                (
                    "HOME".to_string(),
                    "/var/lib/self-host/apps/k3n8qz4v2x1p".to_string()
                ),
                ("USER".to_string(), "sf-app-k3n8qz4v2x1p".to_string()),
                ("LOGNAME".to_string(), "sf-app-k3n8qz4v2x1p".to_string()),
                ("PATH".to_string(), DEFAULT_PATH.to_string()),
                ("PORT".to_string(), "8080".to_string()),
            ]
        );
    }

    #[test]
    fn the_request_may_replace_path_and_a_repeated_name_keeps_the_last_value() {
        let mut r = request();
        r.environment = vec![
            ("PATH".into(), "/opt/api/bin".into()),
            ("PORT".into(), "1".into()),
            ("PORT".into(), "2".into()),
        ];
        let env = environment_for(&account(), &r);
        assert_eq!(env.iter().filter(|(n, _)| n == "PATH").count(), 1);
        assert!(env.contains(&("PATH".to_string(), "/opt/api/bin".to_string())));
        assert_eq!(env.iter().filter(|(n, _)| n == "PORT").count(), 1);
        assert!(env.contains(&("PORT".to_string(), "2".to_string())));
    }

    #[test]
    fn every_purpose_is_listed_once() {
        assert_eq!(Purpose::ALL.len(), 4);
        assert!(Purpose::ALL.contains(&Purpose::Main));
        assert!(Purpose::ALL.contains(&Purpose::Hook));
        assert!(Purpose::ALL.contains(&Purpose::Build));
        assert!(Purpose::ALL.contains(&Purpose::Terminal));
    }

    #[test]
    fn errors_state_their_own_layer_and_keep_the_cause_underneath() {
        let e = LaunchError::Identity(IdentityError::Unknown("sf-app-k3n8qz4v2x1p".into()));
        let report = crate::error::ErrorReport::new(&e);
        assert_eq!(
            report.error,
            "the Application Account cannot run the command"
        );
        assert_eq!(
            report.caused_by,
            ["the Application Account 'sf-app-k3n8qz4v2x1p' does not exist on the Host"]
        );

        let e = LaunchError::Spawn(std::io::Error::from_raw_os_error(libc::EPERM));
        let report = crate::error::ErrorReport::new(&e);
        assert_eq!(report.error, "failed to start the command");
        assert_eq!(report.caused_by.len(), 1);

        let e = StopError::Cgroup(CgroupError::Unsupported);
        let report = crate::error::ErrorReport::new(&e);
        assert_eq!(
            report.error,
            "the Application's cgroup could not be torn down"
        );
        assert_eq!(report.caused_by, ["cgroups are available on Linux only"]);
    }
}
