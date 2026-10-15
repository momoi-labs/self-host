//! The Host itself, for the console: its screen and a shell on it. Both are
//! off until the Operator allows them in Settings. The screen needs a Mac; the
//! shell opens on any Host.
//!
//! The screen is macOS Screen Sharing, a VNC server; the Platform only carries
//! bytes between the console's WebSocket and that server, which asks for the
//! Mac account's password itself (ADR-0033). The shell is in `terminal`.

use std::time::Duration;

use axum::{
    Json,
    body::Bytes,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

use crate::{AppState, settings, store::StateStore, terminal};

/// Where macOS Screen Sharing listens.
const SCREEN_SHARING: &str = "127.0.0.1:5900";
/// The console's frames are key and pointer events, and a pasted clipboard.
const MAX_FRAME: usize = 1024 * 1024;

/// What the console can open on the Host, and what stands in the way.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub screen: Screen,
    pub terminal: Shell,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Screen {
    /// The Host is a Mac. Other Hosts have no screen to offer yet.
    pub supported: bool,
    /// The Operator allows the console to open the screen.
    pub allowed: bool,
    /// Screen Sharing answers on the Host.
    pub listening: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Shell {
    /// The Operator allows the console to open a shell on the Host.
    pub allowed: bool,
    /// The account the shell runs as: the one the Platform runs under.
    pub account: Option<String>,
}

pub(crate) async fn status<S: StateStore>(State(state): State<AppState<S>>) -> Response {
    let supported = cfg!(target_os = "macos");
    Json(Status {
        screen: Screen {
            supported,
            allowed: settings::remote_screen(&state.store).await,
            listening: supported && reach(SCREEN_SHARING).await.is_some(),
        },
        terminal: Shell {
            allowed: settings::host_terminal(&state.store).await,
            account: account(),
        },
    })
    .into_response()
}

/// The name of the account this process runs as.
fn account() -> Option<String> {
    // SAFETY: getpwuid returns a pointer into static storage or null; the name
    // is copied out before anything else can call it on this thread.
    unsafe {
        let entry = libc::getpwuid(libc::getuid());
        if entry.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr((*entry).pw_name)
            .to_str()
            .ok()
            .map(str::to_owned)
    }
}

async fn reach(target: &str) -> Option<TcpStream> {
    tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(target))
        .await
        .ok()
        .and_then(Result::ok)
}

#[derive(Deserialize)]
struct Connect {
    key: String,
}

// Like the terminal, the key arrives in the first frame: a browser cannot put
// a Bearer header on a WebSocket upgrade, and a key does not go in a URL.
pub(crate) async fn upgrade<S: StateStore>(
    State(state): State<AppState<S>>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.max_message_size(MAX_FRAME)
        .max_frame_size(MAX_FRAME)
        .on_upgrade(move |socket| connected(socket, state))
}

async fn connected<S: StateStore>(mut socket: WebSocket, state: AppState<S>) {
    let Ok(Some(Ok(Message::Text(first)))) =
        tokio::time::timeout(Duration::from_secs(5), socket.recv()).await
    else {
        return;
    };
    let Ok(request) = serde_json::from_str::<Connect>(&first) else {
        terminal::fail(&mut socket, "Invalid screen request.").await;
        return;
    };
    if !terminal::authentic(&state, &request.key).await {
        terminal::fail(&mut socket, "Invalid API key.").await;
        return;
    }
    if !cfg!(target_os = "macos") {
        terminal::fail(&mut socket, "The remote screen needs a macOS Host.").await;
        return;
    }
    if !settings::remote_screen(&state.store).await {
        terminal::fail(&mut socket, "Remote screen is off. Turn it on in Settings.").await;
        return;
    }
    let Some(stream) = reach(SCREEN_SHARING).await else {
        terminal::fail(
            &mut socket,
            "Screen Sharing does not answer on the Host. Turn it on in System Settings > General > Sharing.",
        )
        .await;
        return;
    };
    let ready = serde_json::json!({"type": "ready"}).to_string();
    if socket.send(Message::Text(ready.into())).await.is_err() {
        return;
    }
    pump(socket, stream).await;
}

/// Carries bytes both ways until either side closes.
async fn pump(socket: WebSocket, stream: TcpStream) {
    let (mut reader, mut writer) = stream.into_split();
    let (mut sink, mut source) = socket.split();
    let upstream = async {
        while let Some(Ok(message)) = source.next().await {
            match message {
                Message::Binary(data) => {
                    if writer.write_all(&data).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    };
    let downstream = async {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let frame = Message::Binary(Bytes::copy_from_slice(&buffer[..read]));
                    if sink.send(frame).await.is_err() {
                        break;
                    }
                }
            }
        }
        let _ = sink.close().await;
    };
    tokio::select! {
        _ = upstream => {}
        _ = downstream => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{docker::FakeDocker, routes::FakeRoutes, store::FakeStateStore};
    use axum::{Router, routing::get};
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tokio_tungstenite::tungstenite::Message as Frame;

    async fn serve(router: Router) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (address, server)
    }

    async fn first(url: &str, request: Value) -> Value {
        let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        ws.send(Frame::Text(request.to_string().into()))
            .await
            .unwrap();
        let message = tokio::time::timeout(Duration::from_secs(2), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        serde_json::from_str(message.to_text().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn refuses_a_wrong_key_and_what_the_operator_has_not_allowed() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        let settings = store.clone();
        let router = crate::build_app(
            store,
            Arc::new(FakeDocker::new()),
            Arc::new(FakeRoutes::new()),
            crate::metrics::Metrics::new(),
        );
        let (address, server) = serve(router).await;
        let url = format!("ws://{address}/host/screen");

        let refused = first(&url, json!({"key": "wrong"})).await;
        assert_eq!(refused["type"], "error");
        assert_eq!(refused["report"]["error"], "Invalid API key.");

        // Off by default; on a Linux test runner the Host is not a Mac either.
        let refused = first(&url, json!({"key": "test-key"})).await;
        assert_eq!(refused["type"], "error");

        // The shell follows the same rules, behind its own setting.
        let shell = format!("ws://{address}/host/terminal");
        let refused = first(&shell, json!({"key": "wrong", "cols": 80, "rows": 24})).await;
        assert_eq!(refused["report"]["error"], "Invalid API key.");
        let refused = first(&shell, json!({"key": "test-key", "cols": 80, "rows": 24})).await;
        assert_eq!(refused["type"], "error");

        // Allowed, it opens a real login shell, on Linux as on macOS.
        settings.store_state("host_terminal", "true").await.unwrap();
        let opened = first(&shell, json!({"key": "test-key", "cols": 80, "rows": 24})).await;
        assert_eq!(opened["type"], "ready", "{opened}");
        server.abort();
    }

    #[tokio::test]
    async fn carries_bytes_both_ways_between_the_socket_and_the_vnc_server() {
        let vnc = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = vnc.local_addr().unwrap();
        let router = Router::new().route(
            "/screen",
            get(move |ws: WebSocketUpgrade| async move {
                ws.on_upgrade(move |socket| async move {
                    let stream = TcpStream::connect(target).await.unwrap();
                    pump(socket, stream).await;
                })
            }),
        );
        let (address, server) = serve(router).await;
        let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{address}/screen"))
            .await
            .unwrap();
        let (mut peer, _) = vnc.accept().await.unwrap();

        peer.write_all(b"RFB 003.889\n").await.unwrap();
        let greeting = tokio::time::timeout(Duration::from_secs(2), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(greeting.into_data().as_ref(), b"RFB 003.889\n");

        ws.send(Frame::Binary(Bytes::from_static(b"RFB 003.008\n")))
            .await
            .unwrap();
        let mut reply = [0u8; 12];
        tokio::time::timeout(Duration::from_secs(2), peer.read_exact(&mut reply))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&reply, b"RFB 003.008\n");

        // The VNC server going away closes the console's socket.
        drop(peer);
        let closed = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match ws.next().await {
                    None | Some(Err(_)) | Some(Ok(Frame::Close(_))) => break,
                    Some(Ok(_)) => {}
                }
            }
        })
        .await;
        assert!(closed.is_ok());
        server.abort();
    }
}
