//! Host-side preparation for Compose bind mounts.

use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindMount {
    pub source: PathBuf,
    pub create_host_path: bool,
    /// Relative mounts must stay under this directory, including symlinks.
    pub data_root: Option<PathBuf>,
}

impl BindMount {
    pub fn prepare(&self) -> io::Result<()> {
        if let Some(root) = &self.data_root {
            std::fs::create_dir_all(root)?;
            let root = root.canonicalize()?;
            let mut existing = self.source.as_path();
            while !existing.exists() {
                // A dangling symlink must not turn into a directory.
                if std::fs::symlink_metadata(existing).is_ok() {
                    return Err(invalid("bind source contains a dangling symlink"));
                }
                existing = existing
                    .parent()
                    .ok_or_else(|| invalid("invalid bind source"))?;
            }
            if !existing.canonicalize()?.starts_with(&root) {
                return Err(invalid(
                    "bind source leaves the Application's data directory",
                ));
            }
        }
        match std::fs::metadata(&self.source) {
            Ok(metadata) if metadata.is_file() || metadata.is_dir() => Ok(()),
            Ok(_) => Err(invalid("bind source must be a regular file or directory")),
            Err(error) if error.kind() == io::ErrorKind::NotFound && self.create_host_path => {
                if std::fs::symlink_metadata(&self.source).is_ok() {
                    return Err(invalid("bind source is a dangling symlink"));
                }
                std::fs::create_dir_all(&self.source)
            }
            Err(error) => Err(error),
        }
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// A checked mount target is always a container path, never a relative path.
pub(super) fn valid_target(target: &str) -> bool {
    Path::new(target).is_absolute() && !target.contains(['\0', '\n', '\r'])
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("sf-bind-{}", rand::random::<u64>()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn mount(&self, name: &str, create: bool) -> BindMount {
            BindMount {
                source: self.0.join(name),
                create_host_path: create,
                data_root: Some(self.0.clone()),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn existing_files_and_directories_keep_their_types_and_contents() {
        let fixture = Fixture::new();
        std::fs::write(fixture.0.join("config"), "synthetic").unwrap();
        std::fs::create_dir(fixture.0.join("data")).unwrap();
        for create in [false, true] {
            fixture.mount("config", create).prepare().unwrap();
            fixture.mount("data", create).prepare().unwrap();
            assert!(fixture.0.join("config").is_file());
            assert!(fixture.0.join("data").is_dir());
            assert_eq!(
                std::fs::read_to_string(fixture.0.join("config")).unwrap(),
                "synthetic"
            );
        }
    }

    #[test]
    fn absent_sources_require_explicit_directory_creation() {
        let fixture = Fixture::new();
        assert!(fixture.mount("config", false).prepare().is_err());
        assert!(!fixture.0.join("config").exists());
        fixture.mount("data", true).prepare().unwrap();
        assert!(fixture.0.join("data").is_dir());
    }

    #[test]
    fn relative_mounts_cannot_escape_through_symlinks() {
        let fixture = Fixture::new();
        let outside = Fixture::new();
        std::os::unix::fs::symlink(&outside.0, fixture.0.join("escape")).unwrap();
        assert!(fixture.mount("escape/data", true).prepare().is_err());
        assert!(!outside.0.join("data").exists());
    }
}
