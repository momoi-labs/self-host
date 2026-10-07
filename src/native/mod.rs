//! Native Applications: a process tree on the Host under a dedicated account.
//!
//! Linux uses cgroups for resource controls and whole-tree cleanup. macOS
//! uses a restricted helper and s6 foreground process groups. Both use the
//! existing lifecycle API and a separate Application Account for every app.

use serde::{Deserialize, Serialize};

pub mod cgroup;
pub mod identity;
pub mod launch;
pub mod lifecycle;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod metrics;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub mod mise;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod redaction;
pub mod supervision;
pub mod terminal;

pub use cgroup::{ApplicationCgroup, CgroupError, CgroupRoot};
pub use identity::{
    AccountName, IdentityError, ProvisionError, ProvisionRequest, ResolvedAccount,
    account_name_for, provision, resolve,
};
pub use launch::{LaunchError, LaunchRequest, LaunchedProcess, Purpose, StopError, launch};

#[derive(Debug, Serialize)]
pub struct Capabilities {
    pub available: bool,
    pub resource_limits: bool,
    pub metrics: bool,
    pub terminal: bool,
    pub managed_postgres: bool,
    pub reason: Option<&'static str>,
}

pub fn capabilities() -> Capabilities {
    let linux = cfg!(target_os = "linux");
    #[cfg(target_os = "macos")]
    let available = supervision::host::protected(std::path::Path::new(macos::HELPER)).is_ok();
    #[cfg(not(target_os = "macos"))]
    let available = linux;
    Capabilities {
        available,
        resource_limits: linux,
        metrics: linux,
        terminal: linux,
        managed_postgres: linux,
        reason: if available {
            None
        } else {
            Some(
                "Install native supervision with the self-host installer before deploying native Applications.",
            )
        },
    }
}

/// Limits on a native Application's whole process tree, applied through
/// cgroup v2 controllers. `None` leaves the controller at its default, which
/// is no limit. Cgroups control resources; they are not a filesystem or
/// network sandbox, and nothing here claims otherwise.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// CPU time as a percentage of one CPU: 100 is one CPU, 250 is two and a
    /// half. Written to `cpu.max`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_percent: Option<u32>,
    /// Bytes of memory the tree may use before the kernel reclaims or kills.
    /// Written to `memory.max`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    /// How many tasks (processes and threads) the tree may hold at once.
    /// Written to `pids.max`, which is what stops a fork from running away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tasks: Option<u32>,
}

impl ResourceLimits {
    /// No limit on anything: what an Application gets when the Operator
    /// says nothing.
    pub const NONE: ResourceLimits = ResourceLimits {
        cpu_percent: None,
        memory_bytes: None,
        max_tasks: None,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_left_unsaid_serialize_to_nothing_and_read_back_as_none() {
        let json = serde_json::to_string(&ResourceLimits::NONE).unwrap();
        assert_eq!(json, "{}");
        let parsed: ResourceLimits = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed, ResourceLimits::NONE);
    }

    #[test]
    fn every_limit_round_trips() {
        let limits = ResourceLimits {
            cpu_percent: Some(150),
            memory_bytes: Some(512 * 1024 * 1024),
            max_tasks: Some(256),
        };
        let json = serde_json::to_string(&limits).unwrap();
        assert_eq!(
            serde_json::from_str::<ResourceLimits>(&json).unwrap(),
            limits
        );
    }
}
