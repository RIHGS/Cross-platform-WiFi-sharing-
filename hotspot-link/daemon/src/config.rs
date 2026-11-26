use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Default)]
pub struct Config {
    pub paired: bool,
    pub device_address: Option<String>,
    pub shared_secret: Option<String>,
}

impl Config {
    pub async fn load() -> Result<Self, Box<dyn std::error::Error>> {
        let home_dir = dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        let config_path = format!("{}/.config/hotspotlink/config.json", home_dir);

        if std::path::Path::new(&config_path).exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let cfg: Config = serde_json::from_str(&content)?;
            Ok(cfg)
        } else {
            Ok(Default::default())
        }
    }

    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let home_dir = dirs::home_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        let config_path = format!("{}/.config/hotspotlink/config.json", home_dir);
        if let Some(parent) = std::path::Path::new(&config_path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&config_path, json)?;
        Ok(())
    }
}