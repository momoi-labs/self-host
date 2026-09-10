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

        std::fs::write(&path, content).map_err(|e| CliConfigError::Write(path.clone(), e))?;

        Ok(())
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
