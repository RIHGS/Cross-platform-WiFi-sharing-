mod cli;
mod config;
mod ipc;
mod pairing;
mod state;

use clap::Parser;
use cli::{Cli, Command};
use config::Config;
use state::DaemonState;
use std::error::Error;
use std::net::SocketAddr;
use std::process::ExitCode;

type DaemonResult = Result<(), Box<dyn Error + Send + Sync>>;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match cli.command {
        None | Some(Command::Run) => run_daemon(cli.ipc_addr).await,
        Some(Command::Status) => cli::status(cli.ipc_addr).await,
        Some(Command::Pair) => cli::pair(cli.ipc_addr).await,
        Some(Command::Forget) => cli::forget(cli.ipc_addr).await,
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run_daemon(addr: SocketAddr) -> DaemonResult {
    let config_path = Config::path();
    let config = match Config::load() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("warning: {error}; starting with default configuration");
            Config::default()
        }
    };

    println!("hotspotlinkd v{}", env!("CARGO_PKG_VERSION"));
    println!("config: {}", config_path.display());
    println!("paired: {}", if config.paired { "yes" } else { "no" });

    let state = DaemonState::with_path(config, config_path);

    tokio::select! {
        result = ipc::serve(addr, state) => result?,
        _ = tokio::signal::ctrl_c() => println!("shutting down"),
    }

    Ok(())
}
