use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use clap::Parser;

mod cli;
mod config;
mod ipc;

#[derive(Parser)]
#[command(name = "hotspotlinkd")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(clap::Subcommand)]
enum Commands {
    Status,
    Pair,
    Forget,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let config = config::Config::load().await.unwrap_or_default();

    match cli.command {
        Some(Commands::Status) => {
            println!("daemon running, config loaded: {:?}", config);
        }
        Some(Commands::Pair) => {
            println!("PIN: 123456");
            println!("Scan QR code or enter PIN in phone app to pair");
        }
        Some(Commands::Forget) => {
            println!("Forgot paired device");
        }
        None => {
            // Run as daemon with TCP IPC socket
            start_ipc_server().await?;
        }
    }

    Ok(())
}

async fn start_ipc_server() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:45678").await?;
    println!("IPC server listening on 127.0.0.1:45678");

    loop {
        let (mut socket, _) = listener.accept().await?;
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            if let Ok(n) = socket.read(&mut buf).await {
                let msg = String::from_utf8_lossy(&buf[..n]).trim().to_string();
                if msg == "status" {
                    let response = "daemon running";
                    let _ = socket.write(response.as_bytes()).await;
                }
            }
        });
    }
}