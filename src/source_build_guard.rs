//! A trusted static launcher keeps every source-build descendant unprivileged.

use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
const LAUNCHER: &str = include_str!("../assets/source-build-guard.c");

/// Where a release installs the launcher, built static next to its s6 tools.
#[cfg(target_os = "linux")]
const BUNDLED: &str = "/usr/libexec/self-host/s6/source-build-guard";

/// The release's launcher when one is installed, otherwise one compiled here
/// from the Platform's fixed source, never repository instructions. Either is
/// static so LD_PRELOAD cannot replace its checks.
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
    if let Some(bundled) = bundled(Path::new(BUNDLED))? {
        return Ok(bundled);
    }
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

/// The installed launcher, if any. One that exists but is not a root-owned
/// executable nobody else can replace is refused rather than skipped.
#[cfg(target_os = "linux")]
fn bundled(path: &Path) -> anyhow::Result<Option<PathBuf>> {
    use std::os::unix::fs::PermissionsExt;

    if !path.try_exists()? {
        return Ok(None);
    }
    crate::native::supervision::host::protected(path)?;
    let metadata = std::fs::metadata(path)?;
    anyhow::ensure!(
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0,
        "{} must be an executable file",
        path.display()
    );
    Ok(Some(path.to_path_buf()))
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

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::bundled;
    use std::path::Path;

    #[test]
    fn a_missing_launcher_falls_back_to_compiling() {
        let missing = Path::new("/usr/libexec/self-host-test-missing/source-build-guard");
        assert!(bundled(missing).unwrap().is_none());
    }

    #[test]
    fn a_root_owned_executable_is_used_as_installed() {
        let installed = Path::new("/usr/bin/true");
        assert_eq!(bundled(installed).unwrap().as_deref(), Some(installed));
    }

    #[test]
    fn a_launcher_someone_else_could_replace_is_refused() {
        let directory =
            std::env::temp_dir().join(format!("self-host-guard-{:032x}", rand::random::<u128>()));
        std::fs::create_dir(&directory).unwrap();
        let launcher = directory.join("source-build-guard");
        std::fs::copy("/usr/bin/true", &launcher).unwrap();
        let error = bundled(&launcher).unwrap_err();
        let _ = std::fs::remove_dir_all(&directory);
        assert!(error.to_string().contains("root-owned"), "{error}");
    }
}
