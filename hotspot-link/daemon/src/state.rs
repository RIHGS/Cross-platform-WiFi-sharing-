use crate::config::Config;
use crate::pairing::PairingSession;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub struct DaemonState {
    pub config: Mutex<Config>,
    pub pairing: Mutex<Option<PairingSession>>,
    pub started_at: Instant,
    pub config_path: PathBuf,
}

pub type SharedState = Arc<DaemonState>;

impl DaemonState {
    pub fn with_path(config: Config, config_path: PathBuf) -> SharedState {
        Arc::new(Self {
            config: Mutex::new(config),
            pairing: Mutex::new(None),
            started_at: Instant::now(),
            config_path,
        })
    }
}
