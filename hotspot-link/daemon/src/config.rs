use serde::{Deserialize, Serialize};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

pub const CONFIG_DIR_NAME: &str = "hotspotlink";
pub const CONFIG_FILE_NAME: &str = "config.json";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub paired: bool,
    #[serde(default)]
    pub device_address: Option<String>,
    #[serde(default)]
    pub shared_secret: Option<String>,
    #[serde(default)]
    pub paired_at: Option<u64>,
}

#[derive(Debug)]
pub enum ConfigError {
    Io(io::Error),
    Parse(serde_json::Error),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "config I/O error: {error}"),
            Self::Parse(error) => write!(f, "invalid config file: {error}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Parse(error) => Some(error),
        }
    }
}

impl From<io::Error> for ConfigError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for ConfigError {
    fn from(error: serde_json::Error) -> Self {
        Self::Parse(error)
    }
}

impl Config {
    pub fn path() -> PathBuf {
        if let Some(dir) = std::env::var_os("HOTSPOTLINK_CONFIG_DIR") {
            return PathBuf::from(dir).join(CONFIG_FILE_NAME);
        }

        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(CONFIG_DIR_NAME)
            .join(CONFIG_FILE_NAME)
    }

    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(content) => Ok(serde_json::from_str(&content)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(ConfigError::Io(error)),
        }
    }

    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;

        let json = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }

        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn clear_pairing(&mut self) {
        self.paired = false;
        self.device_address = None;
        self.shared_secret = None;
        self.paired_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_loads_default() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load_from(&dir.path().join("missing.json")).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join(CONFIG_FILE_NAME);
        let config = Config {
            paired: true,
            device_address: Some("AA:BB:CC:DD:EE:FF".into()),
            shared_secret: Some("abc123".into()),
            paired_at: Some(42),
        };

        config.save_to(&path).unwrap();
        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded, config);
    }

    #[test]
    fn clear_pairing_resets_all_fields() {
        let mut config = Config {
            paired: true,
            device_address: Some("phone".into()),
            shared_secret: Some("secret".into()),
            paired_at: Some(1),
        };

        config.clear_pairing();
        assert_eq!(config, Config::default());
    }
}
