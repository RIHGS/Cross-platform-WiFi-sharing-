use crate::config::Config;
use crate::ipc::{self, Request, Response};
use clap::{Parser, Subcommand};
use serde_json::Value;
use std::error::Error;
use std::net::SocketAddr;

pub const DEFAULT_IPC_ADDR: &str = "127.0.0.1:45678";

type CliResult = Result<(), Box<dyn Error + Send + Sync>>;

#[derive(Debug, Parser)]
#[command(
    name = "hotspotlinkd",
    version,
    about = "HotspotLink pairing and Wi-Fi sharing daemon"
)]
pub struct Cli {
    /// Address the daemon listens on for local IPC.
    #[arg(
        long,
        global = true,
        env = "HOTSPOTLINK_IPC_ADDR",
        default_value = DEFAULT_IPC_ADDR
    )]
    pub ipc_addr: SocketAddr,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the daemon in the foreground.
    Run,
    /// Show daemon and pairing status.
    Status,
    /// Open a pairing window and print the PIN.
    Pair,
    /// Forget the paired device.
    Forget,
}

pub async fn status(addr: SocketAddr) -> CliResult {
    match ipc::request(addr, &Request::Status).await {
        Ok(response) if response.ok => {
            print_status(&response);
            Ok(())
        }
        Ok(response) => Err(response
            .error
            .unwrap_or_else(|| "status request failed".into())
            .into()),
        Err(_) => {
            print_offline_status()?;
            Ok(())
        }
    }
}

pub async fn pair(addr: SocketAddr) -> CliResult {
    let response = ipc::request(addr, &Request::PairStart)
        .await
        .map_err(|error| format!("cannot reach daemon at {addr}: {error}"))?;

    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| "pair request failed".into())
            .into());
    }

    let data = response
        .data
        .as_ref()
        .ok_or("daemon returned no pairing data")?;
    let pin = data["pin"].as_str().ok_or("daemon returned no PIN")?;
    let expires = data["expiresInSecs"].as_u64().unwrap_or(0);
    let already_active = data["alreadyActive"].as_bool().unwrap_or(false);

    if already_active {
        println!("Pairing window is already open ({expires}s remaining).");
    } else {
        println!("Pairing window open for {expires}s.");
    }
    println!("PIN: {pin}");
    println!("Enter this PIN in the HotspotLink app on your phone.");
    Ok(())
}

pub async fn forget(addr: SocketAddr) -> CliResult {
    let response = ipc::request(addr, &Request::Forget)
        .await
        .map_err(|error| format!("cannot reach daemon at {addr}: {error}"))?;

    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| "forget request failed".into())
            .into());
    }

    let was_paired = response
        .data
        .as_ref()
        .and_then(|data| data["wasPaired"].as_bool())
        .unwrap_or(false);

    if was_paired {
        println!("Forgot paired device.");
    } else {
        println!("No paired device to forget.");
    }
    Ok(())
}

fn print_status(response: &Response) {
    let Some(data) = response.data.as_ref() else {
        println!("Daemon: running");
        return;
    };

    let version = data["version"].as_str().unwrap_or("unknown");
    let uptime = data["uptimeSecs"].as_u64().unwrap_or(0);
    let paired = data["paired"].as_bool().unwrap_or(false);

    println!(
        "Daemon: running (v{version}, uptime {})",
        format_duration(uptime)
    );
    println!("Paired: {}", if paired { "yes" } else { "no" });

    if let Some(device) = data["device"].as_str() {
        println!("Device: {device}");
    }
    if let Some(paired_at) = data["pairedAt"].as_u64() {
        println!("Paired at: {}", format_ago(paired_at));
    }
    if let Some(pairing) = data["pairing"].as_object() {
        let expires = pairing
            .get("expiresInSecs")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let attempts = pairing
            .get("attemptsLeft")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        println!("Pairing window: open (expires in {expires}s, {attempts} attempts left)");
    }
}

fn print_offline_status() -> CliResult {
    let config = Config::load()?;
    println!("Daemon: not running");
    println!("Paired: {}", if config.paired { "yes" } else { "no" });
    if let Some(device) = config.device_address {
        println!("Device: {device}");
    }
    Ok(())
}

fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn format_ago(unix_secs: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format!("{} ago", format_duration(now.saturating_sub(unix_secs)))
}
