//! Local, non-login accounts. The helper serializes directory mutations.

use super::*;
use anyhow::{Context, Result, bail};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

fn error(error: anyhow::Error) -> ProvisionError {
    ProvisionError::Failed {
        step: "manage the macOS Application Account".into(),
        stderr: CommandStderr(format!("{error:#}")),
    }
}

fn dscl(arguments: &[&str]) -> Result<String> {
    let output = std::process::Command::new("/usr/bin/dscl")
        .arg(".")
        .args(arguments)
        .env_clear()
        .output()?;
    if !output.status.success() {
        bail!(
            "dscl failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8(output.stdout)?)
}

/// Deleting a directory record needs Full Disk Access for the responsible
/// process. The Platform daemon has none, so macOS denies it (#168).
fn delete_record(path: &str) -> Result<()> {
    dscl(&["-delete", path]).map(|_| ()).map_err(|error| {
        if error.to_string().contains("eDSPermissionError") {
            anyhow::anyhow!(
                "macOS denied removing {path} because the Platform lacks Full Disk Access. \
                 The Application is stopped and its data is protected; the account remains. \
                 See https://github.com/momoi-labs/self-host/issues/168"
            )
        } else {
            error
        }
    })
}

/// Lookups after a dscl change can be answered from opendirectoryd's cache,
/// including a record cached while the change was half done.
fn flush_directory_cache() {
    let _ = std::process::Command::new("/usr/bin/dscacheutil")
        .arg("-flushcache")
        .env_clear()
        .status();
}

fn attribute(path: &str, name: &str) -> Result<String> {
    let text = dscl(&["-read", path, name])?;
    Ok(text
        .strip_prefix(&format!("{name}:"))
        .context("unexpected directory response")?
        .trim()
        .to_owned())
}

fn lock() -> Result<std::fs::File> {
    super::super::supervision::host::require_root()?;
    let path = Path::new(super::super::macos::ROOT).join("accounts.lock");
    super::super::supervision::host::protected(path.parent().unwrap())?;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    // SAFETY: the file is open throughout the directory transaction.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(file)
}

fn verify_inner(request: &ProvisionRequest, entry: &PasswdEntry) -> Result<()> {
    if request.account != &account_name_for(request.application_id)? {
        bail!("Application Account name does not match its id");
    }
    let user = format!("/Users/{}", request.account);
    let group = format!("/Groups/{}", request.account);
    if attribute(&user, "UserShell")? != "/usr/bin/false"
        || attribute(&user, "AuthenticationAuthority")? != ";DisabledUser;"
        || attribute(&user, "Password")? != "*"
        || attribute(&group, "RealName")? != comment_for(request.application_id)
        || attribute(&group, "PrimaryGroupID")? != entry.gid.to_string()
        || entry.uid < 5000
        || entry.gid != entry.uid
    {
        bail!("Application Account or group has an unexpected identity or login policy");
    }
    // Also reject duplicate numeric identities, including directory records
    // created outside the Platform. Never inherit another account's files.
    for (kind, field, number) in [
        ("/Users", "UniqueID", entry.uid),
        ("/Groups", "PrimaryGroupID", entry.gid),
    ] {
        let text = dscl(&["-list", kind, field])?;
        let matches: Vec<_> = text
            .lines()
            .filter(|line| {
                line.split_whitespace()
                    .last()
                    .and_then(|v| v.parse::<u32>().ok())
                    == Some(number)
            })
            .collect();
        if matches.len() != 1
            || matches[0].split_whitespace().next() != Some(request.account.as_str())
        {
            bail!("Application numeric identity is not unique");
        }
    }
    let groups = std::process::Command::new("/usr/bin/id")
        .args(["-G", request.account.as_str()])
        .env_clear()
        .output()?;
    if !groups.status.success()
        || String::from_utf8(groups.stdout)?
            .split_whitespace()
            .any(|value| matches!(value, "0" | "80"))
    {
        bail!("Application Account must not belong to wheel or admin");
    }
    Ok(())
}

pub(super) fn verify(
    request: &ProvisionRequest,
    entry: &PasswdEntry,
) -> Result<(), ProvisionError> {
    verify_inner(request, entry).map_err(error)
}

pub fn provision(request: &ProvisionRequest) -> Result<ResolvedAccount, ProvisionError> {
    (|| -> Result<ResolvedAccount> {
        let _lock = lock()?;
        if request.account != &account_name_for(request.application_id)? {
            bail!("Application Account name does not match its id");
        }
        match verify_owned(request) {
            Ok(account) => return prepare_home(request, account),
            Err(ProvisionError::Identity(IdentityError::Unknown(_))) => (),
            Err(error) => return Err(error.into()),
        }
        if request.home.exists() {
            bail!("retained Application data already exists; use a new Application id");
        }
        let user = format!("/Users/{}", request.account);
        let group = format!("/Groups/{}", request.account);
        // A leftover or foreign group is not ours to overwrite.
        if dscl(&["-read", &user]).is_ok() || dscl(&["-read", &group]).is_ok() {
            bail!("refusing to adopt an existing directory record");
        }
        let mut used = std::collections::HashSet::new();
        for (kind, field) in [("/Users", "UniqueID"), ("/Groups", "PrimaryGroupID")] {
            for line in dscl(&["-list", kind, field])?.lines() {
                if let Some(id) = line
                    .split_whitespace()
                    .last()
                    .and_then(|v| v.parse::<u32>().ok())
                {
                    used.insert(id);
                }
            }
        }
        let number = (5000..60000)
            .find(|id| !used.contains(id))
            .context("no free Application identity")?
            .to_string();
        let marker = comment_for(request.application_id);
        // Marker first: a failed transaction can remove only records it owns.
        let result = (|| -> Result<()> {
            dscl(&["-create", &group, "RealName", &marker])?;
            dscl(&["-create", &group, "PrimaryGroupID", &number])?;
            dscl(&["-create", &group, "Password", "*"])?;
            dscl(&["-create", &user, "RealName", &marker])?;
            for (key, value) in [
                ("UserShell", "/usr/bin/false"),
                ("Password", "*"),
                ("AuthenticationAuthority", ";DisabledUser;"),
                ("IsHidden", "1"),
                ("UniqueID", &number),
                ("PrimaryGroupID", &number),
                (
                    "NFSHomeDirectory",
                    request.home.to_str().context("invalid home path")?,
                ),
            ] {
                dscl(&["-create", &user, key, value])?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            for path in [&user, &group] {
                if attribute(path, "RealName").ok().as_deref() == Some(&marker) {
                    let _ = dscl(&["-delete", path]);
                }
            }
            return Err(error);
        }
        flush_directory_cache();
        prepare_home(request, verify_owned(request)?)
    })()
    .map_err(error)
}

fn prepare_home(request: &ProvisionRequest, identity: ResolvedAccount) -> Result<ResolvedAccount> {
    if !request.home.exists() {
        std::fs::create_dir(request.home)?;
    }
    let metadata = std::fs::symlink_metadata(request.home)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        bail!("Application home must be a directory");
    }
    std::os::unix::fs::chown(request.home, Some(identity.uid), Some(identity.gid))?;
    std::fs::set_permissions(request.home, std::fs::Permissions::from_mode(0o750))?;
    Ok(identity)
}

pub fn revoke(request: &ProvisionRequest) -> Result<(), ProvisionError> {
    (|| -> Result<()> {
        let _lock = lock()?;
        let account = match verify_owned(request) {
            Ok(account) => Some(account),
            Err(ProvisionError::Identity(IdentityError::Unknown(_))) => None,
            Err(error) => return Err(error.into()),
        };
        if let Some(account) = account {
            // launchd starts per-user agents (lsd, cfprefsd, trustd) for the
            // account. They outlive the record, and an account that reuses the
            // UID would inherit them. The domain may not exist, so ignore errors.
            let _ = std::process::Command::new("/bin/launchctl")
                .args(["bootout", &format!("user/{}", account.uid)])
                .env_clear()
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            delete_record(&format!("/Users/{}", request.account))?;
        }
        // A retry after deleting the user must still remove its owned group.
        if dscl(&["-list", "/Groups"])?
            .lines()
            .any(|name| name == request.account.as_str())
        {
            let group = format!("/Groups/{}", request.account);
            if attribute(&group, "RealName")? != comment_for(request.application_id) {
                bail!("refusing to delete a group without the Application ownership marker");
            }
            let gid: u32 = attribute(&group, "PrimaryGroupID")?.parse()?;
            if gid < 5000
                || dscl(&["-list", "/Users", "PrimaryGroupID"])?
                    .lines()
                    .any(|line| {
                        line.split_whitespace()
                            .last()
                            .and_then(|value| value.parse::<u32>().ok())
                            == Some(gid)
                    })
            {
                bail!("refusing to delete an Application group still used by another account");
            }
            delete_record(&group)?;
        }
        flush_directory_cache();
        Ok(())
    })()
    .map_err(error)
}
