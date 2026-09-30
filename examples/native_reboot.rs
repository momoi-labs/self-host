//! Synthetic reboot check for a disposable Linux Host. Start the supplied
//! self-host-native.service first, then run `prepare`, reboot, and run `verify`.
#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    use self_host::native::supervision::{ServiceDefinition, Supervisor};
    use self_host::native::{AccountName, CgroupRoot, ProvisionRequest, ResourceLimits, provision};
    use std::{fs, path::Path, time::Duration};
    let root = Path::new("/var/lib/self-host/native");
    let cgroup = fs::read_to_string(root.join("cgroup-root"))?;
    let supervisor = Supervisor::connect(
        root.into(),
        "/usr/local/bin/self-host".into(),
        CgroupRoot::at(cgroup.trim())?,
    )?;
    let mode = std::env::args().nth(1).unwrap_or_default();
    for (id, running) in [("reboot-running", true), ("reboot-stopped", false)] {
        let account = AccountName::parse(&format!("sf-{id}"))?;
        let home = std::path::PathBuf::from(format!("/home/sf-{id}"));
        if mode == "prepare" {
            provision(&ProvisionRequest {
                application_id: id,
                account: &account,
                home: &home,
            })?;
            let definition = ServiceDefinition {
                application_id: id.into(), account: account.as_str().into(),
                command: vec!["/bin/sh".into(), "-c".into(), "cat /proc/sys/kernel/random/boot_id >> boots; id -u > uid; echo retained > data; exec sleep 86400".into()],
                working_dir: home, environment: vec![], limits: ResourceLimits::NONE,
                readiness: Some(vec!["/bin/sh".into(), "-c".into(), "test $(id -u) -ne 0".into()]),
                startup_timeout_ms: 5000, stop_grace_ms: 100,
            };
            supervisor.prepare(&definition)?;
            supervisor.start(id, Duration::from_secs(15))?;
            if !running {
                supervisor.stop(id, Duration::from_secs(15))?;
            }
        } else if mode == "verify" {
            let status = supervisor.status(id)?;
            anyhow::ensure!(
                status.intended_running == running
                    && status.running == running
                    && status.ready == running,
                "unexpected {id} status: {status:?}"
            );
            let boots = fs::read_to_string(home.join("boots"))?;
            let boots: Vec<_> = boots.lines().collect();
            let current = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
            anyhow::ensure!(
                boots.len() == if running { 2 } else { 1 },
                "unexpected start count for {id}: {boots:?}"
            );
            anyhow::ensure!(
                boots[0] != current.trim(),
                "reboot the disposable Host before verify"
            );
            if running {
                anyhow::ensure!(boots[1] == current.trim());
            }
            anyhow::ensure!(fs::read_to_string(home.join("uid"))?.trim() != "0");
            anyhow::ensure!(fs::read_to_string(home.join("data"))? == "retained\n");
            println!(
                "{id}: persisted intent restored, dedicated uid, retained data, starts={}",
                boots.len()
            );
        } else {
            anyhow::bail!("use prepare or verify on a disposable Linux Host");
        }
    }
    Ok(())
}
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("native reboot validation requires disposable Linux");
    std::process::exit(1);
}
