//! A trusted static launcher keeps every source-build descendant unprivileged.

use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
const LAUNCHER: &str = include_str!("../assets/source-build-guard.c");

/// Compile only the Platform's fixed source, never repository instructions.
/// The private output is static so LD_PRELOAD cannot replace its checks.
#[cfg(target_os = "linux")]
pub fn compile(dir: &Path) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::process::{Command, Stdio};

    if let Ok(metadata) = std::fs::symlink_metadata(dir) {
        anyhow::ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "build guard storage must be a private directory"
        );
        // SAFETY: geteuid has no arguments and cannot fail.
        anyhow::ensure!(
            metadata.uid() == unsafe { libc::geteuid() },
            "build guard storage must belong to the Platform identity"
        );
    }
    std::fs::create_dir_all(dir).context("create private build guard storage")?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .context("secure build guard storage")?;
    let directory = dir.join(format!("guard-{:032x}", rand::random::<u128>()));
    std::fs::create_dir(&directory).context("create build guard compiler directory")?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;

    let result = (|| {
        let source = directory.join("launcher.c");
        let mut source_file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&source)?;
        source_file.write_all(LAUNCHER.as_bytes())?;
        source_file.sync_all()?;
        let pending = directory.join("launcher.pending");
        let status = Command::new("/usr/bin/cc")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .args(["-O2", "-static", "-o"])
            .arg(&pending)
            .arg(&source)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("Git builds require /usr/bin/cc and static libc development files on Linux")?;
        anyhow::ensure!(
            status.success(),
            "could not compile the static source build guard; install a system C compiler and static libc development files"
        );
        std::fs::set_permissions(&pending, std::fs::Permissions::from_mode(0o555))?;
        std::fs::File::open(&pending)?.sync_all()?;
        let executable = directory.join("launcher");
        std::fs::rename(pending, &executable)?;
        std::fs::File::open(&directory)?.sync_all()?;
        Ok(executable)
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(directory);
    }
    result
}

#[cfg(not(target_os = "linux"))]
pub fn compile(_dir: &Path) -> anyhow::Result<PathBuf> {
    anyhow::bail!("Git source builds require Linux to enforce non-root build descendants")
}

#[cfg(all(test, not(target_os = "linux")))]
mod tests {
    #[test]
    fn unsupported_hosts_refuse_without_creating_storage() {
        let path = std::env::temp_dir().join(format!(
            "self-host-unsupported-build-{:032x}",
            rand::random::<u128>()
        ));
        let error = super::compile(&path).unwrap_err();
        assert!(error.to_string().contains("require Linux"));
        assert!(!path.exists());
    }
}
