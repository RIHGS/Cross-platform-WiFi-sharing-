use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

pub async fn start_server(addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(addr).await?;
    println!("IPC server listening on {}", addr);

    loop {
        let (socket, _) = listener.accept().await?;
        let socket = Arc::new(Mutex::new(socket));

        let socket_clone = socket.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            if let Ok(n) = socket.lock().await.read(&mut buf).await {
                let msg = String::from_utf8_lossy(&buf[..n]).trim().to_string();
                if msg == "status" {
                    let response = "daemon running";
                    let _ = socket_clone.lock().await.write(response.as_bytes()).await;
                }
            }
        });
    }
}