mod scan;

use scan::ScanManager;
use std::sync::Arc;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Arc::new(ScanManager::new()))
        .invoke_handler(tauri::generate_handler![
            scan::get_adapter_info,
            scan::start_scan,
            scan::stop_scan,
            scan::get_devices,
            scan::clear_devices,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
