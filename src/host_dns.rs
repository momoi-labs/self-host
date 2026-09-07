//! Persistent DNS routing on a Linux Host running systemd-resolved.

use std::net::IpAddr;

pub fn service_unit(suffix: &str, host_ip: &str) -> anyhow::Result<String> {
    crate::bootstrap::validate_dns_suffix(suffix).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        suffix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-'),
        "DNS Suffix must contain only letters, digits, dots and hyphens"
    );
    let host_ip: IpAddr = host_ip.parse()?;
    // resolved needs an IPv4 address on the dummy link to enable its DNS scope.
    Ok(format!(
        "[Unit]\n\
Description=DNS routing for the self-host Host\n\
Requires=systemd-resolved.service\n\
After=systemd-resolved.service\n\
PartOf=systemd-resolved.service\n\
\n\
[Service]\n\
Type=oneshot\n\
RemainAfterExit=yes\n\
ExecStart=-ip link add self-host-dns type dummy\n\
ExecStart=ip address replace 169.254.53.53/32 dev self-host-dns\n\
ExecStart=ip link set self-host-dns up\n\
ExecStart=resolvectl dns self-host-dns {host_ip}\n\
ExecStart=resolvectl domain self-host-dns ~{suffix}\n\
ExecStart=resolvectl default-route self-host-dns no\n\
ExecStop=-ip link delete self-host-dns\n\
\n\
[Install]\n\
WantedBy=multi-user.target\n"
    ))
}

#[cfg(target_os = "linux")]
pub fn install(suffix: &str, host_ip: &str) -> anyhow::Result<()> {
    use anyhow::Context;
    use std::io::{IsTerminal, Write};
    use std::process::Command;

    let unit = service_unit(suffix, host_ip)?;
    anyhow::ensure!(
        Command::new("systemctl")
            .args(["is-active", "--quiet", "systemd-resolved"])
            .status()?
            .success(),
        "setup-dns requires an active systemd-resolved service"
    );
    for program in ["ip", "resolvectl"] {
        Command::new(program)
            .arg("--help")
            .output()
            .with_context(|| format!("setup-dns requires {program}"))?;
    }

    let path =
        std::env::temp_dir().join(format!("self-host-dns-{}.service", rand::random::<u64>()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let result = (|| -> anyhow::Result<()> {
        file.write_all(unit.as_bytes())?;
        drop(file);

        // Read Platform state before elevation: sudo may change HOME. Only
        // the system service installation needs administrator privileges.
        let elevation = if std::io::stdin().is_terminal() {
            "sudo"
        } else {
            "pkexec"
        };
        let status = Command::new(elevation)
            .args(["sh", "-c", INSTALL_SCRIPT, "self-host-setup-dns"])
            .arg(&path)
            .status()
            .context("could not request administrator privileges for DNS setup")?;
        anyhow::ensure!(
            status.success(),
            "DNS service installation failed ({status})"
        );
        Ok(())
    })();
    let _ = std::fs::remove_file(path);
    result
}

#[cfg(target_os = "linux")]
const INSTALL_SCRIPT: &str = "set -eu\n\
install -m 644 -- \"$1\" /etc/systemd/system/self-host-dns.service\n\
systemctl daemon-reload\n\
systemctl enable self-host-dns.service\n\
systemctl restart self-host-dns.service";

pub async fn check(suffix: &str, host_ip: &str) -> anyhow::Result<()> {
    let expected: IpAddr = host_ip.parse()?;
    let name = format!("admin.{suffix}");
    let addresses = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        tokio::net::lookup_host((name.as_str(), 443)),
    )
    .await??;
    anyhow::ensure!(
        addresses
            .into_iter()
            .any(|address| address.ip() == expected),
        "{name} does not resolve to {expected}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_values_that_could_inject_systemd_directives() {
        assert!(service_unit("home.lan\nExecStart=bad", "192.168.1.74").is_err());
        assert!(service_unit("home.lan", "192.168.1.74\nExecStart=bad").is_err());
        assert!(service_unit("home.lan", "admin.home.lan").is_err());
    }

    #[test]
    fn routes_only_the_configured_suffix_and_survives_resolver_restarts() {
        let unit = service_unit("test.lan", "192.168.1.74").unwrap();
        assert!(unit.contains("ip address replace 169.254.53.53/32 dev self-host-dns\n"));
        assert!(unit.contains("resolvectl dns self-host-dns 192.168.1.74\n"));
        assert!(unit.contains("resolvectl domain self-host-dns ~test.lan\n"));
        assert!(unit.contains("resolvectl default-route self-host-dns no\n"));
        assert!(unit.contains("PartOf=systemd-resolved.service\n"));
        assert!(unit.contains("WantedBy=multi-user.target\n"));
        assert!(!unit.contains("home.lan"));
        assert!(!unit.contains("self-host serve"));
    }

    #[tokio::test]
    async fn check_rejects_a_name_resolving_to_a_different_address() {
        assert!(check("localhost", "192.0.2.1").await.is_err());
    }
}
