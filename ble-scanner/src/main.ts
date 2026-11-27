import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface ManufacturerData {
  companyId: number;
  data: number[];
}

interface BleDevice {
  id: string;
  address: string | null;
  name: string | null;
  rssi: number | null;
  txPower: number | null;
  manufacturerData: ManufacturerData[];
  serviceUuids: string[];
  paired: boolean;
  lastSeen: number;
}

interface AdapterInfo {
  name: string;
  available: boolean;
}

const COMPANY_IDS: Record<number, string> = {
  0x004c: "Apple",
  0x00e0: "Google",
  0x0006: "Microsoft",
  0x0075: "Samsung",
  0x0059: "Nordic",
  0x0002: "Intel",
  0x000d: "Texas Instruments",
  0x012c: "Sony",
  0x00a0: "Laird",
  0x0489: "Meta",
};

const devices = new Map<string, BleDevice>();
const pending = new Set<string>();
let scanning = false;

const listEl = document.getElementById("device-list") as HTMLElement;
const emptyEl = document.getElementById("empty-state") as HTMLElement;
const countEl = document.getElementById("device-count") as HTMLElement;
const pillEl = document.getElementById("scanning-pill") as HTMLElement;
const btnScan = document.getElementById("btn-scan") as HTMLButtonElement;
const btnClear = document.getElementById("btn-clear") as HTMLButtonElement;
const adapterNameEl = document.getElementById("adapter-name") as HTMLElement;
const adapterStatusEl = document.getElementById("adapter-status") as HTMLElement;
const hintEl = document.getElementById("empty-hint") as HTMLElement;

function escapeHtml(s: string): string {
  const div = document.createElement("div");
  div.textContent = s;
  return div.innerHTML;
}

function hex(bytes: number[]): string {
  return bytes.map((b) => b.toString(16).padStart(2, "0").toUpperCase()).join(" ");
}

function companyName(id: number): string {
  return COMPANY_IDS[id] ?? `0x${id.toString(16).toUpperCase().padStart(4, "0")}`;
}

function rssiBars(rssi: number | null): number {
  if (rssi === null) return 0;
  if (rssi >= -50) return 5;
  if (rssi >= -60) return 4;
  if (rssi >= -70) return 3;
  if (rssi >= -80) return 2;
  if (rssi >= -90) return 1;
  return 0;
}

function timeAgo(lastSeen: number): string {
  const secs = Math.max(0, Math.floor((Date.now() - lastSeen) / 1000));
  if (secs < 1) return "now";
  if (secs < 60) return `${secs}s ago`;
  const mins = Math.floor(secs / 60);
  return `${mins}m ago`;
}

function deviceCard(d: BleDevice): string {
  const hasName = !!(d.name && d.name.trim());
  const title = hasName ? escapeHtml(d.name!.trim()) : "Unknown device";
  const rssi = d.rssi === null || d.rssi === undefined ? "&mdash;" : `${d.rssi} dBm`;
  const bars = rssiBars(d.rssi);

  const desc = d.address ? escapeHtml(d.address) : "";

  const mfg = d.manufacturerData
    .map(
      (m) =>
        `<span class="chip" title="${escapeHtml(hex(m.data))}">
          ${escapeHtml(companyName(m.companyId))}: ${escapeHtml(hex(m.data))}
        </span>`
    )
    .join("");

  const uuids = d.serviceUuids
    .map((u) => `<span class="chip chip-uuid">${escapeHtml(u)}</span>`)
    .join("");

  const busy = pending.has(d.id);
  const action = d.paired
    ? `<span class="badge">Paired</span>
       <button class="btn btn-sm btn-ghost" data-action="unpair" data-id="${escapeHtml(d.id)}" ${busy ? "disabled" : ""}>
         ${busy ? "Unpairing&hellip;" : "Unpair"}
       </button>`
    : `<button class="btn btn-sm btn-primary" data-action="pair" data-id="${escapeHtml(d.id)}" ${busy ? "disabled" : ""}>
         ${busy ? "Pairing&hellip;" : "Pair"}
       </button>`;

  return `
    <article class="device ${d.rssi === null ? "weak" : ""}">
      <div class="signal">
        ${Array.from({ length: 5 }, (_, i) =>
          `<span class="bar ${i < bars ? "on" : ""}"></span>`
        ).join("")}
      </div>
      <div class="info">
        <div class="row">
          <h2 class="dev-title">${title}</h2>
          <span class="rssi">${rssi}</span>
        </div>
        <p class="meta">
          ${desc ? `<span class="desc">${desc}</span><span class="sep">&middot;</span>` : ""}
          <span class="time">last seen ${escapeHtml(timeAgo(d.lastSeen))}</span>
        </p>
        ${mfg ? `<div class="chips">${mfg}</div>` : ""}
        ${uuids ? `<div class="chips">${uuids}</div>` : ""}
        <div class="actions">${action}</div>
      </div>
    </article>`;
}

function cardFingerprint(d: BleDevice): string {
  return JSON.stringify({
    n: d.name,
    a: d.address,
    r: d.rssi,
    m: d.manufacturerData,
    s: d.serviceUuids,
    p: d.paired,
    b: pending.has(d.id),
  });
}

function createSlot(d: BleDevice): HTMLElement {
  const el = document.createElement("div");
  el.className = "device-slot";
  el.id = `dev-${d.id}`;
  el.dataset.fp = cardFingerprint(d);
  el.innerHTML = deviceCard(d);
  return el;
}

function updateSlot(el: HTMLElement, d: BleDevice) {
  const fp = cardFingerprint(d);
  if (el.dataset.fp !== fp) {
    el.dataset.fp = fp;
    el.innerHTML = deviceCard(d);
  }
}

function refreshSlot(d: BleDevice) {
  const el = document.getElementById(`dev-${d.id}`);
  if (el) updateSlot(el, d);
}

async function togglePair(id: string, action: "pair" | "unpair") {
  const current = devices.get(id);
  if (!current || pending.has(id)) return;

  pending.add(id);
  refreshSlot(current);

  try {
    if (action === "pair") {
      const updated = await invoke<BleDevice>("pair_device", { id });
      devices.set(id, updated);
    } else {
      await invoke("unpair_device", { id });
      devices.set(id, { ...current, paired: false });
    }
  } catch (error) {
    alert(`Failed to ${action === "pair" ? "pair" : "unpair"} device: ${error}`);
  } finally {
    pending.delete(id);
    const latest = devices.get(id);
    if (latest) refreshSlot(latest);
  }
}

function updateCount() {
  countEl.textContent = `${devices.size} device${devices.size === 1 ? "" : "s"}`;
}

function upsertDevice(d: BleDevice) {
  devices.set(d.id, d);

  let el = document.getElementById(`dev-${d.id}`);
  if (el) {
    updateSlot(el, d);
  } else {
    listEl.appendChild(createSlot(d));
  }
  updateCount();
  emptyEl.style.display = devices.size === 0 ? "block" : "none";
}

function removeDevice(id: string) {
  devices.delete(id);
  document.getElementById(`dev-${id}`)?.remove();
  updateCount();
  emptyEl.style.display = devices.size === 0 ? "block" : "none";
}

function renderAll() {
  listEl.querySelectorAll(".device-slot").forEach((el) => el.remove());
  devices.forEach((d) => {
    listEl.appendChild(createSlot(d));
  });
  updateCount();
  emptyEl.style.display = devices.size === 0 ? "block" : "none";
}

function refreshTimes() {
  devices.forEach((d) => {
    const el = document.getElementById(`dev-${d.id}`);
    if (!el) return;
    const t = el.querySelector(".time") as HTMLElement | null;
    if (t) t.textContent = `last seen ${timeAgo(d.lastSeen)}`;
  });
}

function setScanning(state: boolean) {
  scanning = state;
  btnScan.textContent = state ? "Stop Scan" : "Start Scan";
  btnScan.classList.toggle("danger", state);
  pillEl.textContent = state ? "Scanning" : "Idle";
  pillEl.classList.toggle("on", state);
  pillEl.classList.toggle("off", !state);

  if (state) {
    hintEl.innerHTML =
      "Phones only advertise in short bursts. On Android, keep the screen on and open " +
      "<em>Settings &gt; Connected devices</em>. On iPhone, just keep the screen on. " +
      "If the phone was previously paired with this PC, unpair it — Windows hides paired devices from scans. " +
      "Use <strong>Pair</strong> on a device to link it.";
  } else {
    hintEl.innerHTML =
      "Press <strong>Start Scan</strong> to look for nearby Bluetooth devices.";
  }
}

async function startScan() {
  btnScan.disabled = true;
  try {
    await invoke("start_scan");
  } catch (e) {
    console.error(e);
    alert(`Failed to start scan: ${e}`);
  } finally {
    btnScan.disabled = false;
  }
}

async function stopScan() {
  btnScan.disabled = true;
  try {
    await invoke("stop_scan");
  } catch (e) {
    console.error(e);
    alert(`Failed to stop scan: ${e}`);
  } finally {
    btnScan.disabled = false;
  }
}

async function init() {
  try {
    const info = await invoke<AdapterInfo>("get_adapter_info");
    adapterNameEl.textContent = info.name;
    adapterStatusEl.classList.toggle("on", info.available);
    adapterStatusEl.classList.toggle("off", !info.available);
  } catch (e) {
    adapterNameEl.textContent = "Unavailable";
  }

  const cached = await invoke<BleDevice[]>("get_devices");
  cached.forEach((d) => devices.set(d.id, d));
  renderAll();

  await listen<BleDevice>("ble://device", (e) => {
    upsertDevice(e.payload);
  });
  await listen<BleDevice>("ble://device-updated", (e) => {
    upsertDevice(e.payload);
  });
  await listen<BleDevice>("ble://device-removed", (e) => {
    removeDevice(e.payload.id);
  });
  await listen<boolean>("ble://scan-state", (e) => setScanning(e.payload));
  await listen("ble://cleared", () => {
    devices.clear();
    renderAll();
  });

  btnScan.addEventListener("click", () => (scanning ? stopScan() : startScan()));
  btnClear.addEventListener("click", () => invoke("clear_devices").catch(console.error));
  listEl.addEventListener("click", (event) => {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("button[data-action]");
    if (!button || button.disabled) return;
    const id = button.dataset.id;
    const action = button.dataset.action;
    if (id && (action === "pair" || action === "unpair")) {
      void togglePair(id, action);
    }
  });

  setInterval(refreshTimes, 1000);
}

init();
