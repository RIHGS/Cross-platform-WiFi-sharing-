use crate::pairing::{PairingSession, VerifyError, new_secret};
use crate::state::SharedState;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io;
use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

pub const MAX_LINE_LEN: usize = 64 * 1024;
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Status,
    PairStart,
    PairConfirm {
        pin: String,
        #[serde(default)]
        device: Option<String>,
    },
    Forget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl Response {
    pub fn ok(data: Value) -> Self {
        Self {
            ok: true,
            error: None,
            data: Some(data),
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(message.into()),
            data: None,
        }
    }
}

pub async fn serve(addr: SocketAddr, state: SharedState) -> io::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    println!("IPC server listening on {addr}");
    serve_listener(listener, state).await
}

pub async fn serve_listener(listener: TcpListener, state: SharedState) -> io::Result<()> {
    loop {
        let (socket, peer) = listener.accept().await?;
        let state = state.clone();

        tokio::spawn(async move {
            if let Err(error) = handle_connection(socket, state).await {
                eprintln!("ipc connection from {peer} closed: {error}");
            }
        });
    }
}

async fn handle_connection(socket: TcpStream, state: SharedState) -> io::Result<()> {
    let (read_half, mut write_half) = socket.into_split();
    let mut reader = BufReader::new(read_half);
    let mut buf = Vec::new();

    loop {
        let line = match tokio::time::timeout(IDLE_TIMEOUT, read_line(&mut reader, &mut buf)).await
        {
            Err(_) => return Ok(()),
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => return Ok(()),
            Ok(Err(error)) => return Err(error),
        };

        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => handle_request(request, &state),
            Err(error) => Response::err(format!("invalid request: {error}")),
        };

        let mut encoded = serde_json::to_vec(&response)
            .unwrap_or_else(|_| br#"{"ok":false,"error":"failed to encode response"}"#.to_vec());
        encoded.push(b'\n');
        write_half.write_all(&encoded).await?;
        write_half.flush().await?;
    }
}

pub fn handle_request(request: Request, state: &SharedState) -> Response {
    match request {
        Request::Ping => Response::ok(json!({ "pong": true })),
        Request::Status => status(state),
        Request::PairStart => pair_start(state),
        Request::PairConfirm { pin, device } => pair_confirm(state, &pin, device),
        Request::Forget => forget(state),
    }
}

fn status(state: &SharedState) -> Response {
    let (paired, device, paired_at) = {
        let config = state
            .config
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        (
            config.paired,
            config.device_address.clone(),
            config.paired_at,
        )
    };

    let pairing = {
        let mut pairing = state
            .pairing
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if pairing.as_ref().is_some_and(PairingSession::is_expired) {
            *pairing = None;
        }
        pairing.as_ref().map(|session| {
            json!({
                "active": true,
                "expiresInSecs": session.expires_in().as_secs(),
                "attemptsLeft": session.attempts_left(),
            })
        })
    };

    Response::ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "uptimeSecs": state.started_at.elapsed().as_secs(),
        "paired": paired,
        "device": device,
        "pairedAt": paired_at,
        "pairing": pairing,
    }))
}

fn pair_start(state: &SharedState) -> Response {
    let already_paired = {
        let config = state
            .config
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        config.paired
    };
    if already_paired {
        return Response::err("device is already paired; run 'forget' first");
    }

    let mut pairing = state
        .pairing
        .lock()
        .unwrap_or_else(|error| error.into_inner());

    if let Some(existing) = pairing.as_ref().filter(|session| !session.is_expired()) {
        return Response::ok(json!({
            "pin": existing.pin(),
            "expiresInSecs": existing.expires_in().as_secs(),
            "alreadyActive": true,
        }));
    }

    let session = PairingSession::start();
    let data = json!({
        "pin": session.pin(),
        "expiresInSecs": session.expires_in().as_secs(),
        "alreadyActive": false,
    });
    *pairing = Some(session);
    Response::ok(data)
}

fn pair_confirm(state: &SharedState, pin: &str, device: Option<String>) -> Response {
    let verified = {
        let mut pairing = state
            .pairing
            .lock()
            .unwrap_or_else(|error| error.into_inner());

        let Some(session) = pairing.as_mut() else {
            return Response::err("no pairing session; run 'pair' first");
        };

        match session.verify(pin) {
            Ok(()) => {
                *pairing = None;
                Ok(())
            }
            Err(error) => {
                if matches!(error, VerifyError::Expired | VerifyError::AttemptsExhausted) {
                    *pairing = None;
                }
                Err(error)
            }
        }
    };

    if let Err(error) = verified {
        return Response::err(error.to_string());
    }

    let secret = new_secret();
    let device_name = device
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown device".to_string());
    let paired_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);

    let save_result = {
        let mut config = state
            .config
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        config.paired = true;
        config.device_address = Some(device_name.clone());
        config.shared_secret = Some(secret.clone());
        config.paired_at = Some(paired_at);
        config.save_to(&state.config_path)
    };

    if let Err(error) = save_result {
        return Response::err(format!("paired but failed to save config: {error}"));
    }

    Response::ok(json!({
        "paired": true,
        "device": device_name,
        "secret": secret,
        "pairedAt": paired_at,
    }))
}

fn forget(state: &SharedState) -> Response {
    {
        let mut pairing = state
            .pairing
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *pairing = None;
    }

    let (was_paired, save_result) = {
        let mut config = state
            .config
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let was_paired = config.paired;
        config.clear_pairing();
        let result = config.save_to(&state.config_path);
        (was_paired, result)
    };

    if let Err(error) = save_result {
        return Response::err(format!("failed to save config: {error}"));
    }

    Response::ok(json!({ "paired": false, "wasPaired": was_paired }))
}

pub async fn request(addr: SocketAddr, request: &Request) -> Result<Response, String> {
    let mut stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| format!("connection to {addr} timed out"))?
        .map_err(|error| format!("could not connect to {addr}: {error}"))?;

    let mut encoded = serde_json::to_vec(request).map_err(|error| error.to_string())?;
    encoded.push(b'\n');
    stream
        .write_all(&encoded)
        .await
        .map_err(|error| error.to_string())?;
    stream.flush().await.map_err(|error| error.to_string())?;

    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    let line = tokio::time::timeout(RESPONSE_TIMEOUT, read_line(&mut reader, &mut buf))
        .await
        .map_err(|_| format!("daemon at {addr} did not respond in time"))?
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("daemon at {addr} closed the connection"))?;

    serde_json::from_str(&line).map_err(|error| format!("invalid response from daemon: {error}"))
}

async fn read_line<R>(reader: &mut BufReader<R>, buf: &mut Vec<u8>) -> io::Result<Option<String>>
where
    R: AsyncRead + Unpin,
{
    buf.clear();
    let mut limited = reader.take((MAX_LINE_LEN + 1) as u64);
    let read = limited.read_until(b'\n', buf).await?;
    if read == 0 {
        return Ok(None);
    }

    let had_newline = buf.last() == Some(&b'\n');
    if had_newline {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
    }

    if buf.len() > MAX_LINE_LEN || (!had_newline && read > MAX_LINE_LEN) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message exceeds maximum length",
        ));
    }

    Ok(Some(String::from_utf8_lossy(buf).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::state::DaemonState;
    use tempfile::TempDir;

    fn test_state() -> (SharedState, TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let state = DaemonState::with_path(Config::default(), path);
        (state, dir)
    }

    fn data(response: &Response) -> &Value {
        response.data.as_ref().expect("response data")
    }

    #[test]
    fn status_reports_unpaired() {
        let (state, _dir) = test_state();
        let response = handle_request(Request::Status, &state);
        assert!(response.ok);
        assert_eq!(data(&response)["paired"], false);
        assert!(data(&response)["pairing"].is_null());
    }

    #[test]
    fn pair_flow_persists_config() {
        let (state, dir) = test_state();

        let response = handle_request(Request::PairStart, &state);
        assert!(response.ok);
        let pin = data(&response)["pin"].as_str().unwrap().to_string();
        assert_eq!(pin.len(), 6);

        let response = handle_request(
            Request::PairConfirm {
                pin,
                device: Some("Pixel 8".into()),
            },
            &state,
        );
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(data(&response)["paired"], true);

        let secret = data(&response)["secret"].as_str().unwrap();
        assert_eq!(secret.len(), 64);

        let saved = Config::load_from(&dir.path().join("config.json")).unwrap();
        assert!(saved.paired);
        assert_eq!(saved.device_address.as_deref(), Some("Pixel 8"));
        assert_eq!(saved.shared_secret.as_deref(), Some(secret));

        let response = handle_request(Request::Status, &state);
        assert_eq!(data(&response)["paired"], true);
        assert_eq!(data(&response)["device"], "Pixel 8");
    }

    #[test]
    fn wrong_pin_is_rejected() {
        let (state, _dir) = test_state();
        let response = handle_request(Request::PairStart, &state);
        let pin = data(&response)["pin"].as_str().unwrap().to_string();
        let wrong = if pin == "000000" { "000001" } else { "000000" };

        let response = handle_request(
            Request::PairConfirm {
                pin: wrong.into(),
                device: None,
            },
            &state,
        );

        assert!(!response.ok);
        assert!(response.error.unwrap().contains("incorrect PIN"));
    }

    #[test]
    fn pair_start_requires_forget_when_paired() {
        let (state, _dir) = test_state();
        let response = handle_request(Request::PairStart, &state);
        let pin = data(&response)["pin"].as_str().unwrap().to_string();
        handle_request(Request::PairConfirm { pin, device: None }, &state);

        let response = handle_request(Request::PairStart, &state);
        assert!(!response.ok);
        assert!(response.error.unwrap().contains("already paired"));
    }

    #[test]
    fn forget_clears_pairing() {
        let (state, dir) = test_state();
        let response = handle_request(Request::PairStart, &state);
        let pin = data(&response)["pin"].as_str().unwrap().to_string();
        handle_request(
            Request::PairConfirm {
                pin,
                device: Some("Phone".into()),
            },
            &state,
        );

        let response = handle_request(Request::Forget, &state);
        assert!(response.ok);
        assert_eq!(data(&response)["wasPaired"], true);

        let saved = Config::load_from(&dir.path().join("config.json")).unwrap();
        assert!(!saved.paired);
        assert!(saved.shared_secret.is_none());
    }

    #[test]
    fn pair_confirm_without_session_fails() {
        let (state, _dir) = test_state();
        let response = handle_request(
            Request::PairConfirm {
                pin: "123456".into(),
                device: None,
            },
            &state,
        );

        assert!(!response.ok);
        assert!(response.error.unwrap().contains("no pairing session"));
    }

    #[tokio::test]
    async fn client_and_server_round_trip() {
        let (state, _dir) = test_state();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(serve_listener(listener, state));

        let response = request(addr, &Request::Ping).await.unwrap();
        assert!(response.ok);
        assert_eq!(response.data.unwrap()["pong"], true);

        let response = request(addr, &Request::Status).await.unwrap();
        assert!(response.ok);
        assert_eq!(response.data.unwrap()["paired"], false);

        server.abort();
    }
}
