//! Cgroup v2 resource control for a native Application's whole process tree.
//!
//! The Platform owns one delegated cgroup directory and creates one child
//! per Application under it. The child carries the CPU, memory and task
//! limits, and `cgroup.kill` ends every process in it at once, detached
//! grandchildren included. That is all a cgroup does here: it is not a
//! filesystem or network sandbox. Everything that touches the Host is Linux
//! only; the file contents are pure and unit tested everywhere.

use super::ResourceLimits;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The controllers an Application cgroup needs enabled on its parent.
const CONTROLLERS: [&str; 3] = ["cpu", "memory", "pids"];

/// The period `cpu.max` is written against, in microseconds. A percentage
/// times a thousand is the quota over this period.
const CPU_PERIOD_MICROS: u64 = 100_000;

/// How often `kill` looks at `cgroup.procs` while waiting for it to empty.
#[cfg(target_os = "linux")]
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// A cgroup v2 directory delegated to the Platform, checked to be on
/// cgroup2fs and to offer the cpu, memory and pids controllers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CgroupRoot(PathBuf);

impl CgroupRoot {
    pub fn at(path: impl Into<PathBuf>) -> Result<Self, CgroupError> {
        let path = path.into();
        ensure_cgroup2(&path)?;
        let controllers = read(&path.join("cgroup.controllers"))?;
        if let Some(controller) = missing_controller(&controllers) {
            return Err(CgroupError::MissingController {
                root: path,
                controller,
            });
        }
        Ok(CgroupRoot(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Enables the controllers for the root's children, creates
    /// `<root>/sf-app-<id>` and writes the limits into it, with
    /// `memory.oom.group` set so an OOM kill takes the whole tree.
    ///
    /// A directory left behind by an earlier run is taken over when it is
    /// empty; one with processes in it is `StillRunning`.
    pub fn create(
        &self,
        application_id: &str,
        limits: &ResourceLimits,
    ) -> Result<ApplicationCgroup, CgroupError> {
        write(
            &self.0.join("cgroup.subtree_control"),
            &subtree_control_enabling(),
        )?;
        let path = self.0.join(format!("sf-app-{application_id}"));
        match std::fs::create_dir(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let leftover = ApplicationCgroup { path: path.clone() };
                let pids = leftover.processes()?;
                if !pids.is_empty() {
                    return Err(CgroupError::StillRunning { path, pids });
                }
            }
            Err(source) => return Err(CgroupError::Io { path, source }),
        }
        let cgroup = ApplicationCgroup { path };
        cgroup.write_limits(limits)?;
        Ok(cgroup)
    }
}

/// One Application's cgroup: the directory its whole process tree lives in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationCgroup {
    path: PathBuf,
}

impl ApplicationCgroup {
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn write_limits(&self, limits: &ResourceLimits) -> Result<(), CgroupError> {
        write(&self.path.join("cpu.max"), &cpu_max(limits.cpu_percent))?;
        write(
            &self.path.join("memory.max"),
            &memory_max(limits.memory_bytes),
        )?;
        write(&self.path.join("pids.max"), &pids_max(limits.max_tasks))?;
        write(&self.path.join("memory.oom.group"), "1")
    }

    /// Moves a process into the cgroup. The launcher does this from inside
    /// the child before it drops privileges; this is for anything else.
    pub fn add_process(&self, pid: u32) -> Result<(), CgroupError> {
        write(&self.path.join("cgroup.procs"), &pid.to_string())
    }

    /// The pids in the cgroup right now. A process that has exited and not
    /// been reaped yet is no longer listed.
    pub fn processes(&self) -> Result<Vec<u32>, CgroupError> {
        let procs = read(&self.path.join("cgroup.procs"))?;
        Ok(parse_pids(&procs))
    }

    /// `cgroup.kill`, then wait up to `grace` for `cgroup.procs` to empty,
    /// then remove the directory. Nothing of the tree survives; if something
    /// still holds on after the grace, that is `StillRunning`. A directory
    /// that is already gone counts as done.
    #[cfg(target_os = "linux")]
    pub fn kill(&self, grace: Duration) -> Result<(), CgroupError> {
        if !self.path.exists() {
            return Ok(());
        }
        write(&self.path.join("cgroup.kill"), "1")?;
        let deadline = std::time::Instant::now() + grace;
        loop {
            let pids = self.processes()?;
            if pids.is_empty() {
                break;
            }
            if std::time::Instant::now() >= deadline {
                return Err(CgroupError::StillRunning {
                    path: self.path.clone(),
                    pids,
                });
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        remove_dir(&self.path)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn kill(&self, _grace: Duration) -> Result<(), CgroupError> {
        Err(CgroupError::Unsupported)
    }

    /// Removes an empty cgroup. One with processes in it is `StillRunning`;
    /// use `kill` for that.
    pub fn remove(self) -> Result<(), CgroupError> {
        if !self.path.exists() {
            return Ok(());
        }
        let pids = self.processes()?;
        if !pids.is_empty() {
            return Err(CgroupError::StillRunning {
                path: self.path,
                pids,
            });
        }
        remove_dir(&self.path)
    }
}

/// `cpu.max`: the quota in microseconds over a 100 ms period. 150 is a CPU
/// and a half; `None` is no limit.
pub fn cpu_max(cpu_percent: Option<u32>) -> String {
    match cpu_percent {
        Some(percent) => format!(
            "{} {CPU_PERIOD_MICROS}",
            u64::from(percent) * CPU_PERIOD_MICROS / 100
        ),
        None => "max".to_string(),
    }
}

/// `memory.max` in bytes; `None` is no limit.
pub fn memory_max(bytes: Option<u64>) -> String {
    match bytes {
        Some(bytes) => bytes.to_string(),
        None => "max".to_string(),
    }
}

/// `pids.max`; `None` is no limit.
pub fn pids_max(tasks: Option<u32>) -> String {
    match tasks {
        Some(tasks) => tasks.to_string(),
        None => "max".to_string(),
    }
}

/// What enabling the controllers for a cgroup's children is written as.
fn subtree_control_enabling() -> String {
    CONTROLLERS
        .iter()
        .map(|c| format!("+{c}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The first required controller missing from a `cgroup.controllers` list.
fn missing_controller(controllers: &str) -> Option<&'static str> {
    let offered: Vec<&str> = controllers.split_whitespace().collect();
    CONTROLLERS
        .into_iter()
        .find(|needed| !offered.contains(needed))
}

/// The pids in a `cgroup.procs` listing, one per line.
fn parse_pids(procs: &str) -> Vec<u32> {
    procs
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

#[derive(Debug)]
pub enum CgroupError {
    /// The path is not on a cgroup2 filesystem.
    NotCgroup2(PathBuf),
    /// The root does not offer a controller the Platform needs.
    MissingController {
        root: PathBuf,
        controller: &'static str,
    },
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Processes are still in the cgroup when it should be empty.
    StillRunning { path: PathBuf, pids: Vec<u32> },
    /// Not Linux.
    Unsupported,
}

impl std::fmt::Display for CgroupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CgroupError::NotCgroup2(path) => {
                write!(f, "'{}' is not a cgroup v2 directory", path.display())
            }
            CgroupError::MissingController { root, controller } => write!(
                f,
                "the cgroup '{}' does not offer the '{controller}' controller",
                root.display()
            ),
            CgroupError::Io { path, .. } => write!(f, "failed to access '{}'", path.display()),
            CgroupError::StillRunning { path, pids } => {
                let pids: Vec<String> = pids.iter().map(u32::to_string).collect();
                write!(
                    f,
                    "the cgroup '{}' still has processes: {}",
                    path.display(),
                    pids.join(", ")
                )
            }
            CgroupError::Unsupported => write!(f, "cgroups are available on Linux only"),
        }
    }
}

impl std::error::Error for CgroupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CgroupError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(target_os = "linux")]
fn ensure_cgroup2(path: &Path) -> Result<(), CgroupError> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| CgroupError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the path holds a NUL byte",
        ),
    })?;
    // SAFETY: statfs is plain data that the call fills in on success; the
    // zeroed value is never read before that.
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the duration of the call.
    if unsafe { libc::statfs(c_path.as_ptr(), &mut stat) } != 0 {
        return Err(CgroupError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::last_os_error(),
        });
    }
    if stat.f_type != libc::CGROUP2_SUPER_MAGIC {
        return Err(CgroupError::NotCgroup2(path.to_path_buf()));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn ensure_cgroup2(_path: &Path) -> Result<(), CgroupError> {
    Err(CgroupError::Unsupported)
}

fn read(path: &Path) -> Result<String, CgroupError> {
    std::fs::read_to_string(path).map_err(|source| CgroupError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn write(path: &Path, contents: &str) -> Result<(), CgroupError> {
    std::fs::write(path, contents).map_err(|source| CgroupError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn remove_dir(path: &Path) -> Result<(), CgroupError> {
    match std::fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CgroupError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_max_is_the_percentage_as_a_quota_over_a_100ms_period() {
        assert_eq!(cpu_max(Some(150)), "150000 100000");
        assert_eq!(cpu_max(Some(50)), "50000 100000");
        assert_eq!(cpu_max(Some(100)), "100000 100000");
        assert_eq!(cpu_max(None), "max");
    }

    #[test]
    fn memory_max_is_bytes_or_max() {
        assert_eq!(memory_max(Some(32 * 1024 * 1024)), "33554432");
        assert_eq!(memory_max(None), "max");
    }

    #[test]
    fn pids_max_is_a_count_or_max() {
        assert_eq!(pids_max(Some(8)), "8");
        assert_eq!(pids_max(None), "max");
    }

    #[test]
    fn the_controllers_are_enabled_together() {
        assert_eq!(subtree_control_enabling(), "+cpu +memory +pids");
    }

    #[test]
    fn a_root_that_offers_everything_is_missing_nothing() {
        assert_eq!(missing_controller("cpuset cpu io memory pids"), None);
    }

    #[test]
    fn the_first_missing_controller_is_named() {
        assert_eq!(missing_controller("cpuset io memory pids"), Some("cpu"));
        assert_eq!(missing_controller("cpu pids"), Some("memory"));
        assert_eq!(missing_controller("cpu memory"), Some("pids"));
        assert_eq!(missing_controller(""), Some("cpu"));
    }

    #[test]
    fn a_procs_listing_reads_as_pids_and_skips_blank_lines() {
        assert_eq!(parse_pids("12\n345\n\n"), vec![12, 345]);
        assert!(parse_pids("").is_empty());
    }

    #[test]
    fn errors_state_their_own_layer() {
        let e = CgroupError::StillRunning {
            path: PathBuf::from("/sys/fs/cgroup/self-host/sf-app-k3n8qz4v2x1p"),
            pids: vec![12, 34],
        };
        assert_eq!(
            e.to_string(),
            "the cgroup '/sys/fs/cgroup/self-host/sf-app-k3n8qz4v2x1p' still has processes: 12, 34"
        );
        let e = CgroupError::Io {
            path: PathBuf::from("/sys/fs/cgroup/self-host/cgroup.procs"),
            source: std::io::Error::from_raw_os_error(libc::EACCES),
        };
        let report = crate::error::ErrorReport::new(&e);
        assert_eq!(
            report.error,
            "failed to access '/sys/fs/cgroup/self-host/cgroup.procs'"
        );
        assert_eq!(report.caused_by.len(), 1);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn a_root_is_unsupported_off_linux() {
        assert!(matches!(
            CgroupRoot::at("/sys/fs/cgroup"),
            Err(CgroupError::Unsupported)
        ));
    }
}
