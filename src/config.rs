use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CliConfig {
    pub api_base_url: String,
    pub api_key: String,
}

impl CliConfig {
    pub fn config_path() -> std::path::PathBuf {
        crate::paths::platform_config_dir().join("config.json")
    }

    pub fn load() -> Result<Option<Self>, CliConfigError> {
        let path = Self::config_path();
        if !path.exists() {
            return Ok(None);
        }

        let content =
            std::fs::read_to_string(&path).map_err(|e| CliConfigError::Read(path.clone(), e))?;
        // Older releases wrote it 0644, readable through a data directory
        // others may traverse. Best effort: a copy someone else owns stays.
        restrict(&path);

        let config: CliConfig =
            serde_json::from_str(&content).map_err(|e| CliConfigError::Parse(path.clone(), e))?;

        Ok(Some(config))
    }

    pub fn save(&self) -> Result<(), CliConfigError> {
        let path = Self::config_path();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CliConfigError::Write(path.clone(), e))?;
        }

        let content = serde_json::to_string_pretty(self).map_err(CliConfigError::Serialize)?;

        // It holds the Operator's API key: only its owner reads it.
        {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt as _;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&path)
                .map_err(|e| CliConfigError::Write(path.clone(), e))?;
            file.write_all(content.as_bytes())
                .map_err(|e| CliConfigError::Write(path.clone(), e))?;
        }
        restrict(&path);

        Ok(())
    }
}

fn restrict(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt as _;
    if let Ok(metadata) = std::fs::metadata(path)
        && metadata.permissions().mode() & 0o077 != 0
    {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

#[derive(Debug)]
pub enum CliConfigError {
    Read(std::path::PathBuf, std::io::Error),
    Write(std::path::PathBuf, std::io::Error),
    Parse(std::path::PathBuf, serde_json::Error),
    Serialize(serde_json::Error),
}

impl std::fmt::Display for CliConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliConfigError::Read(path, e) => write!(f, "failed to read {}: {e}", path.display()),
            CliConfigError::Write(path, e) => write!(f, "failed to write {}: {e}", path.display()),
            CliConfigError::Parse(path, e) => write!(f, "failed to parse {}: {e}", path.display()),
            CliConfigError::Serialize(e) => write!(f, "failed to serialize config: {e}"),
        }
    }
}

impl std::error::Error for CliConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CliConfigError::Read(_, e) => Some(e),
            CliConfigError::Write(_, e) => Some(e),
            CliConfigError::Parse(_, e) => Some(e),
            CliConfigError::Serialize(e) => Some(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    /// The file holds the Operator's API key, and a data directory may be
    /// one Application accounts traverse.
    #[test]
    fn only_its_owner_reads_the_saved_api_key() {
        let path = CliConfig::config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        CliConfig {
            api_base_url: "https://admin.home.lan".into(),
            api_key: "secret".into(),
        }
        .save()
        .unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
