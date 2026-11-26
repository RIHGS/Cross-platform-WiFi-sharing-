use btleplug::api::{
    Central as _, CentralEvent, Manager as _, Peripheral as _, PeripheralProperties, ScanFilter,
};
use btleplug::platform::{Adapter, Manager, Peripheral, PeripheralId};
use futures::StreamExt;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

const DEVICE_TIMEOUT_SECS: u64 = 60;
const PRUNE_INTERVAL_SECS: u64 = 3;
const NAME_RETRY_SECS: u64 = 15;
const NAME_MAX_ATTEMPTS: u32 = 4;
const NAME_CONNECT_TIMEOUT_SECS: u64 = 10;
const NAME_LOOKUP_TIMEOUT_SECS: u64 = 5;

#[derive(Clone, Copy, Default)]
pub(crate) struct NameAttempt {
    last_ms: u64,
    count: u32,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManufacturerData {
    pub company_id: u16,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BleDevice {
    pub id: String,
    pub address: Option<String>,
    pub name: Option<String>,
    pub rssi: Option<i16>,
    pub tx_power: Option<i16>,
    pub manufacturer_data: Vec<ManufacturerData>,
    pub service_uuids: Vec<String>,
    pub last_seen: u64,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterInfo {
    pub name: String,
    pub available: bool,
}

pub struct ScanManager {
    pub cache: Arc<tokio::sync::Mutex<HashMap<String, BleDevice>>>,
    pub scanning: Arc<AtomicBool>,
    pub task: tokio::sync::Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    pub cancel: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    pub adapter: tokio::sync::Mutex<Option<Adapter>>,
    pub name_in_flight: Arc<tokio::sync::Mutex<HashSet<String>>>,
    pub name_tried: Arc<tokio::sync::Mutex<HashMap<String, NameAttempt>>>,
}

impl ScanManager {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            scanning: Arc::new(AtomicBool::new(false)),
            task: tokio::sync::Mutex::new(None),
            cancel: tokio::sync::Mutex::new(None),
            adapter: tokio::sync::Mutex::new(None),
            name_in_flight: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
            name_tried: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn to_device(id: &PeripheralId, p: &PeripheralProperties) -> BleDevice {
    let manufacturer_data: Vec<ManufacturerData> = p
        .manufacturer_data
        .iter()
        .map(|(company_id, data)| ManufacturerData {
            company_id: *company_id,
            data: data.clone(),
        })
        .collect();

    let mut service_uuids: Vec<String> = p.services.iter().map(|u| u.to_string()).collect();
    service_uuids.sort();
    service_uuids.dedup();

    BleDevice {
        id: id.to_string(),
        address: Some(p.address.to_string()),
        name: p.local_name.clone(),
        rssi: p.rssi,
        tx_power: p.tx_power_level,
        manufacturer_data,
        service_uuids,
        last_seen: now_millis(),
    }
}

async fn handle_device(
    adapter: &Adapter,
    cache: &tokio::sync::Mutex<HashMap<String, BleDevice>>,
    app: &AppHandle,
    id: &PeripheralId,
) {
    let device = match adapter.peripherals().await {
        Ok(peripherals) => {
            let mut result = None;
            for p in peripherals {
                if p.id() == *id {
                    if let Ok(Some(props)) = p.properties().await {
                        result = Some(to_device(id, &props));
                    }
                    break;
                }
            }
            result
        }
        Err(_) => None,
    };

    let id_str = id.to_string();
    let mut map = cache.lock().await;
    if let Some(mut device) = device {
        if device.name.is_none() {
            device.name = map.get(&id_str).and_then(|d| d.name.clone());
        }
        let is_new = !map.contains_key(&id_str);
        if is_new {
            let _ = app.emit("ble://device", device.clone());
        } else {
            let _ = app.emit("ble://device-updated", device.clone());
        }
        map.insert(id_str, device);
    } else if !map.contains_key(&id_str) {
        let device = BleDevice {
            id: id_str.clone(),
            address: None,
            name: None,
            rssi: None,
            tx_power: None,
            manufacturer_data: Vec::new(),
            service_uuids: Vec::new(),
            last_seen: now_millis(),
        };
        let _ = app.emit("ble://device", device.clone());
        map.insert(id_str, device);
    } else if let Some(dev) = map.get_mut(&id_str) {
        dev.last_seen = now_millis();
    }
}

#[cfg(target_os = "windows")]
async fn system_device_name(address: u64) -> Option<String> {
    let lookup = tokio::task::spawn_blocking(move || {
        use windows::Devices::Bluetooth::BluetoothLEDevice;

        let device = BluetoothLEDevice::FromBluetoothAddressAsync(address)
            .ok()?
            .get()
            .ok()?;
        let name = device.Name().ok()?.to_string();
        let _ = device.Close();
        let name = name.trim().to_string();
        if name.is_empty() {
            None
        } else {
            Some(name)
        }
    });

    match tokio::time::timeout(Duration::from_secs(NAME_LOOKUP_TIMEOUT_SECS), lookup).await {
        Ok(Ok(name)) => name,
        _ => None,
    }
}

#[cfg(not(target_os = "windows"))]
async fn system_device_name(_address: u64) -> Option<String> {
    None
}

async fn read_gatt_name(peripheral: &Peripheral) -> Option<String> {
    tokio::time::timeout(
        Duration::from_secs(NAME_CONNECT_TIMEOUT_SECS),
        peripheral.discover_services(),
    )
    .await
    .ok()?
    .ok()?;

    for service in peripheral.services() {
        for ch in service.characteristics {
            if ch.uuid != Uuid::from_u128(0x2a00) {
                continue;
            }
            let data = tokio::time::timeout(
                Duration::from_secs(NAME_LOOKUP_TIMEOUT_SECS),
                peripheral.read(&ch),
            )
            .await
            .ok()?
            .ok()?;
            return String::from_utf8(data)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
        }
    }
    None
}

async fn resolve_device_name(
    adapter: Adapter,
    cache: Arc<tokio::sync::Mutex<HashMap<String, BleDevice>>>,
    name_in_flight: Arc<tokio::sync::Mutex<HashSet<String>>>,
    app: AppHandle,
    id: PeripheralId,
) {
    let id_str = id.to_string();

    let peripheral = match adapter.peripherals().await {
        Ok(peripherals) => peripherals.into_iter().find(|p| p.id() == id),
        Err(_) => None,
    };
    let Some(peripheral) = peripheral else {
        name_in_flight.lock().await.remove(&id_str);
        return;
    };

    let mut resolved = system_device_name(peripheral.address().into()).await;

    if resolved.is_none() {
        let connected = tokio::time::timeout(
            Duration::from_secs(NAME_CONNECT_TIMEOUT_SECS),
            peripheral.connect(),
        )
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false);

        if connected {
            resolved = read_gatt_name(&peripheral).await;
            let _ = tokio::time::timeout(Duration::from_secs(2), peripheral.disconnect()).await;
        }
    }

    if let Some(name) = resolved {
        let mut map = cache.lock().await;
        if let Some(dev) = map.get_mut(&id_str) {
            let needs_name = dev.name.as_deref().map_or(true, |n| n.trim().is_empty());
            if needs_name {
                dev.name = Some(name);
                let _ = app.emit("ble://device-updated", dev.clone());
            }
        }
    }

    name_in_flight.lock().await.remove(&id_str);
}

async fn maybe_resolve_name(
    adapter: &Adapter,
    cache: &Arc<tokio::sync::Mutex<HashMap<String, BleDevice>>>,
    name_in_flight: &Arc<tokio::sync::Mutex<HashSet<String>>>,
    name_tried: &Arc<tokio::sync::Mutex<HashMap<String, NameAttempt>>>,
    app: &AppHandle,
    id: &PeripheralId,
) {
    let id_str = id.to_string();

    {
        let map = cache.lock().await;
        match map.get(&id_str) {
            Some(d) if d.name.as_deref().map_or(true, |n| n.trim().is_empty()) => {}
            _ => return,
        }
    }

    let now = now_millis();
    {
        let mut attempts = name_tried.lock().await;
        let attempt = attempts.entry(id_str.clone()).or_default();
        if attempt.count >= NAME_MAX_ATTEMPTS {
            return;
        }
        if attempt.count > 0 && now.saturating_sub(attempt.last_ms) < NAME_RETRY_SECS * 1000 {
            return;
        }
        attempt.last_ms = now;
        attempt.count += 1;
    }

    if !name_in_flight.lock().await.insert(id_str.clone()) {
        return;
    }

    let id_owned = id.clone();
    let adapter = adapter.clone();
    let cache = cache.clone();
    let name_in_flight = name_in_flight.clone();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        resolve_device_name(adapter, cache, name_in_flight, app, id_owned).await;
    });
}

async fn update_manufacturer(
    cache: &tokio::sync::Mutex<HashMap<String, BleDevice>>,
    app: &AppHandle,
    id: &PeripheralId,
    manufacturer_data: &HashMap<u16, Vec<u8>>,
) {
    let id_str = id.to_string();
    let mut map = cache.lock().await;
    if let Some(dev) = map.get_mut(&id_str) {
        dev.manufacturer_data = manufacturer_data
            .iter()
            .map(|(company_id, data)| ManufacturerData {
                company_id: *company_id,
                data: data.clone(),
            })
            .collect();
        dev.last_seen = now_millis();
        let _ = app.emit("ble://device-updated", dev.clone());
    }
}

async fn update_services(
    cache: &tokio::sync::Mutex<HashMap<String, BleDevice>>,
    app: &AppHandle,
    id: &PeripheralId,
    services: &[uuid::Uuid],
) {
    let id_str = id.to_string();
    let mut map = cache.lock().await;
    if let Some(dev) = map.get_mut(&id_str) {
        dev.service_uuids = services.iter().map(|u| u.to_string()).collect();
        dev.service_uuids.sort();
        dev.service_uuids.dedup();
        dev.last_seen = now_millis();
        let _ = app.emit("ble://device-updated", dev.clone());
    }
}

async fn prune_devices(
    cache: &tokio::sync::Mutex<HashMap<String, BleDevice>>,
    app: &AppHandle,
) {
    let now = now_millis();
    let stale: Vec<String> = {
        let map = cache.lock().await;
        map.iter()
            .filter(|(_, d)| now.saturating_sub(d.last_seen) > DEVICE_TIMEOUT_SECS * 1000)
            .map(|(k, _)| k.clone())
            .collect()
    };
    for id in stale {
        let removed = cache.lock().await.remove(&id);
        if let Some(dev) = removed {
            let _ = app.emit("ble://device-removed", dev);
        }
    }
}

#[tauri::command]
pub async fn get_adapter_info() -> Result<AdapterInfo, String> {
    let manager = Manager::new().await.map_err(|e| e.to_string())?;
    let adapters = manager.adapters().await.map_err(|e| e.to_string())?;

    let Some(adapter) = adapters.into_iter().next() else {
        return Ok(AdapterInfo {
            name: "No Bluetooth adapter".into(),
            available: false,
        });
    };

    let name = adapter
        .adapter_info()
        .await
        .unwrap_or_else(|_| "Unknown adapter".into());
    Ok(AdapterInfo { name, available: true })
}

#[tauri::command]
pub async fn start_scan(
    app: AppHandle,
    state: State<'_, Arc<ScanManager>>,
) -> Result<(), String> {
    if state.scanning.load(Ordering::SeqCst) {
        return Ok(());
    }

    let manager = Manager::new().await.map_err(|e| e.to_string())?;
    let adapters = manager.adapters().await.map_err(|e| e.to_string())?;
    let adapter = adapters
        .into_iter()
        .next()
        .ok_or_else(|| "No Bluetooth adapter found".to_string())?;

    adapter
        .start_scan(ScanFilter::default())
        .await
        .map_err(|e| format!("Failed to start BLE scanning: {e}"))?;

    {
        let mut cache = state.cache.lock().await;
        cache.clear();
    }
    state.name_in_flight.lock().await.clear();
    state.name_tried.lock().await.clear();
    let _ = app.emit("ble://cleared", ());

    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();

    state.scanning.store(true, Ordering::SeqCst);
    *state.adapter.lock().await = Some(adapter.clone());
    *state.cancel.lock().await = Some(cancel_tx);

    let cache = state.cache.clone();
    let scanning = state.scanning.clone();
    let name_in_flight = state.name_in_flight.clone();
    let name_tried = state.name_tried.clone();
    let app_inner = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        let mut events = match adapter.events().await {
            Ok(stream) => stream,
            Err(e) => {
                eprintln!("failed to subscribe to BLE events: {e}");
                scanning.store(false, Ordering::SeqCst);
                return;
            }
        };
        let mut prune = tokio::time::interval(Duration::from_secs(PRUNE_INTERVAL_SECS));
        prune.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                _ = &mut cancel_rx => {
                    let _ = adapter.stop_scan().await;
                    scanning.store(false, Ordering::SeqCst);
                    break;
                }
                event = events.next() => {
                    match event {
                        Some(CentralEvent::DeviceDiscovered(id))
                        | Some(CentralEvent::DeviceUpdated(id)) => {
                            handle_device(&adapter, &cache, &app_inner, &id).await;
                            maybe_resolve_name(
                                &adapter,
                                &cache,
                                &name_in_flight,
                                &name_tried,
                                &app_inner,
                                &id,
                            )
                            .await;
                        }
                        Some(CentralEvent::ManufacturerDataAdvertisement { id, manufacturer_data }) => {
                            update_manufacturer(&cache, &app_inner, &id, &manufacturer_data).await;
                        }
                        Some(CentralEvent::ServiceDataAdvertisement { id, .. }) => {
                            handle_device(&adapter, &cache, &app_inner, &id).await;
                        }
                        Some(CentralEvent::ServicesAdvertisement { id, services }) => {
                            update_services(&cache, &app_inner, &id, &services).await;
                        }
                        Some(CentralEvent::StateUpdate(state)) => {
                            let _ = app_inner.emit("ble://adapter-state", format!("{state:?}"));
                        }
                        Some(_) => {}
                        None => break,
                    }
                }
                _ = prune.tick() => {
                    prune_devices(&cache, &app_inner).await;
                }
            }
        }
    });

    *state.task.lock().await = Some(task);
    let _ = app.emit("ble://scan-state", true);
    Ok(())
}

#[tauri::command]
pub async fn stop_scan(
    app: AppHandle,
    state: State<'_, Arc<ScanManager>>,
) -> Result<(), String> {
    let cancel = state.cancel.lock().await.take();
    if let Some(tx) = cancel {
        let _ = tx.send(());
    }
    if let Some(task) = state.task.lock().await.take() {
        let _ = task.await;
    }
    *state.adapter.lock().await = None;
    state.scanning.store(false, Ordering::SeqCst);
    let _ = app.emit("ble://scan-state", false);
    Ok(())
}

#[tauri::command]
pub async fn get_devices(
    state: State<'_, Arc<ScanManager>>,
) -> Result<Vec<BleDevice>, String> {
    let mut devices: Vec<BleDevice> = state.cache.lock().await.values().cloned().collect();
    devices.sort_by_key(|d| -d.rssi.unwrap_or(i16::MIN));
    Ok(devices)
}

#[tauri::command]
pub async fn clear_devices(
    app: AppHandle,
    state: State<'_, Arc<ScanManager>>,
) -> Result<(), String> {
    state.cache.lock().await.clear();
    let _ = app.emit("ble://cleared", ());
    Ok(())
}
