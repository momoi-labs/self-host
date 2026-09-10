use std::{
    io::{Read, Write},
    time::Duration,
};

use axum::{
    extract::{
        Path, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::Response,
};
use futures_util::SinkExt;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::{AppState, apps, docker::DockerError, error::ErrorReport, store::StateStore};

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}
impl Size {
    fn valid(self) -> bool {
        (2..=500).contains(&self.cols) && (1..=300).contains(&self.rows)
    }
    fn pty(self) -> PtySize {
        PtySize {
            rows: self.rows,
            cols: self.cols,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

#[derive(Deserialize)]
struct Connect {
    key: String,
    container: String,
    #[serde(flatten)]
    size: Size,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Input {
    Input {
        data: String,
    },
    Resize {
        #[serde(flatten)]
        size: Size,
    },
}
pub enum Output {
    Data(Vec<u8>),
    Exit(u32),
    Error(String),
}

pub struct Session {
    pub input: mpsc::Sender<Input>,
    pub output: mpsc::Receiver<Output>,
    killer: Option<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
}
impl Drop for Session {
    fn drop(&mut self) {
        if let Some(mut killer) = self.killer.take() {
            let _ = killer.kill();
        }
    }
}
impl Session {
    pub(crate) fn fake() -> Self {
        let (input, _) = mpsc::channel(1);
        let (tx, output) = mpsc::channel(1);
        tx.try_send(Output::Exit(0)).unwrap_or_default();
        Self {
            input,
            output,
            killer: None,
        }
    }
}

// Browsers cannot set a Bearer header on WebSocket upgrades. Authenticate the
// first frame, before inspecting containers or starting any process. No key in URLs.
pub(crate) async fn upgrade<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.max_message_size(64 * 1024)
        .max_frame_size(64 * 1024)
        .on_upgrade(move |socket| connected(socket, state, id))
}
async fn fail(socket: &mut WebSocket, message: impl ToString) {
    let report = ErrorReport::plain(message.to_string());
    let _ = socket
        .send(Message::Text(
            serde_json::json!({"type":"error", "report":report})
                .to_string()
                .into(),
        ))
        .await;
}
async fn connected<S: StateStore>(mut socket: WebSocket, state: AppState<S>, id: String) {
    let Ok(Some(Ok(Message::Text(first)))) =
        tokio::time::timeout(Duration::from_secs(5), socket.recv()).await
    else {
        return;
    };
    let Ok(request) = serde_json::from_str::<Connect>(&first) else {
        fail(&mut socket, "Invalid terminal request.").await;
        return;
    };
    let expected = state
        .store
        .get_api_key()
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    if expected.is_empty() || !crate::constant_time_eq(&request.key, &expected) {
        fail(&mut socket, "Invalid API key.").await;
        return;
    }
    let user = match validate(&state, &id, &request.container, request.size).await {
        Ok(user) => user,
        Err(message) => {
            fail(&mut socket, message).await;
            return;
        }
    };
    let mut session = match state
        .docker
        .open_terminal(&request.container, request.size, user)
        .await
    {
        Ok(session) => session,
        Err(error) => {
            fail(&mut socket, error).await;
            return;
        }
    };
    let _ = socket
        .send(Message::Text("{\"type\":\"ready\"}".into()))
        .await;
    let mut heartbeat = tokio::time::interval(Duration::from_secs(15));
    let mut last_seen = tokio::time::Instant::now();
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if last_seen.elapsed() > Duration::from_secs(45)
                    || !send(&mut socket, Message::Ping(Vec::new().into())).await { break; }
            }
            event = session.output.recv() => {
                let message = match event {
                    Some(Output::Data(data)) => Message::Binary(data.into()),
                    Some(Output::Exit(code)) => { let _ = socket.send(Message::Text(serde_json::json!({"type":"exit", "code":code}).to_string().into())).await; break; },
                    Some(Output::Error(error)) => { fail(&mut socket, error).await; break; },
                    None => break,
                };
                if !send(&mut socket, message).await { break; }
            }
            message = socket.recv() => match message {
                Some(Ok(Message::Text(data))) => {
                    last_seen = tokio::time::Instant::now();
                    let input = match serde_json::from_str::<Input>(&data) {
                        Ok(Input::Input { data }) if data.len() <= 16384 => Input::Input { data },
                        Ok(Input::Resize { size }) if size.valid() => Input::Resize { size },
                        _ => { fail(&mut socket, "Invalid terminal input.").await; break; }
                    };
                    if session.input.send(input).await.is_err() { break; }
                },
                Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => { last_seen = tokio::time::Instant::now(); },
                _ => break,
            }
        }
    }
    drop(session);
    let _ = socket.close().await;
}

async fn send(socket: &mut WebSocket, message: Message) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_secs(10), socket.send(message)).await,
        Ok(Ok(()))
    )
}

async fn validate<S: StateStore>(
    state: &AppState<S>,
    id: &str,
    container: &str,
    size: Size,
) -> Result<Option<&'static str>, String> {
    if !size.valid() {
        return Err("Invalid terminal dimensions.".into());
    }
    let app = apps::get_application(&state.store, id)
        .await
        .map_err(|e| e.to_string())?;
    let containers = state
        .docker
        .application_containers(id)
        .await
        .map_err(|e| e.to_string())?;
    if !containers.iter().any(|name| name == container) {
        return Err("Container does not belong to this Application.".into());
    }
    if !state
        .docker
        .container_state(container)
        .await
        .map_err(|e| e.to_string())?
        .is_some_and(|state| state.is_running())
    {
        return Err("The container is not running.".into());
    }
    Ok(app.development.as_ref().map(|_| "dev"))
}

pub(crate) async fn open(
    container: &str,
    size: Size,
    user: Option<&str>,
) -> Result<Session, DockerError> {
    let container = container.to_owned();
    let user = user.map(str::to_owned);
    tokio::task::spawn_blocking(move || spawn(&container, size, user.as_deref()))
        .await
        .map_err(|e| DockerError::Command("Could not open terminal.".into(), Box::new(e)))?
        .map_err(|e| DockerError::Command("Could not open terminal.".into(), e.into()))
}

const CLEANUP: &str = r#"
for environment in /proc/[0-9]*/environ; do
    test -r "$environment" || continue
    while IFS= read -r -d '' entry; do
        if test "$entry" = "$1"; then
            pid=${environment#/proc/}
            pid=${pid%/environ}
            kill -HUP "$pid" 2>/dev/null || :
            break
        fi
    done < "$environment" 2>/dev/null
done
"#;

fn spawn(container: &str, size: Size, user: Option<&str>) -> anyhow::Result<Session> {
    let pair = native_pty_system().openpty(size.pty())?;
    let marker = format!("SF_TERMINAL_SESSION={:032x}", rand::random::<u128>());
    let mut command = CommandBuilder::new("docker");
    command.args([
        "exec",
        "-it",
        "--env",
        "TERM=xterm-256color",
        "--env",
        &marker,
    ]);
    if let Some(user) = user {
        command.args(["--user", user]);
    }
    command.args(["--", container, "bash", "-i"]);
    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;
    let mut child = pair.slave.spawn_command(command)?;
    drop(pair.slave);
    let killer = child.clone_killer();
    let (input, mut inputs) = mpsc::channel::<Input>(32);
    let (tx, output) = mpsc::channel(32);
    let errors = tx.clone();
    std::thread::spawn(move || {
        while let Some(input) = inputs.blocking_recv() {
            let result = match input {
                Input::Input { data } => writer
                    .write_all(data.as_bytes())
                    .map_err(anyhow::Error::from),
                Input::Resize { size } => pair.master.resize(size.pty()),
            };
            if let Err(error) = result {
                let _ = errors.blocking_send(Output::Error(error.to_string()));
                break;
            }
        }
    });
    let container = container.to_owned();
    let user = user.map(str::to_owned);
    std::thread::spawn(move || {
        let mut buffer = [0; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if tx
                        .blocking_send(Output::Data(buffer[..count].to_vec()))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                // PTYs on Linux return EIO when their slave closes.
                Err(_) => break,
            }
        }
        let status = child.wait();
        // Docker keeps exec processes alive after its client disconnects. Signal
        // only processes carrying this session's random environment marker.
        let mut cleanup = std::process::Command::new("docker");
        cleanup.arg("exec");
        if let Some(user) = user {
            cleanup.args(["--user", &user]);
        }
        cleanup.args(["--", &container, "bash", "-c", CLEANUP, "bash", &marker]);
        cleanup
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Ok(mut process) = cleanup.spawn() {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                if !matches!(process.try_wait(), Ok(None)) {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    let _ = process.kill();
                    let _ = process.wait();
                    tracing::warn!("Timed out closing a container terminal");
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        match status {
            Ok(status) => {
                let _ = tx.blocking_send(Output::Exit(status.exit_code()));
            }
            Err(error) => {
                let _ = tx.blocking_send(Output::Error(error.to_string()));
            }
        }
    });
    Ok(Session {
        input,
        output,
        killer: Some(killer),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        docker::{ApplicationContainer, FakeDocker},
        routes::FakeRoutes,
        store::{ApplicationRecord, DevelopmentApplication, FakeStateStore},
    };
    use futures_util::StreamExt;
    use serde_json::{Value, json};
    use std::sync::Arc;

    async fn fixture(development: bool) -> (String, FakeDocker, tokio::task::JoinHandle<()>) {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        let app = ApplicationRecord {
            id: "sample".into(),
            name: "sample".into(),
            hostname: "sample.home.lan".into(),
            aliases: vec![],
            image: "example".into(),
            status: "running".into(),
            source: "image".into(),
            last_error: None,
            compose: None,
            web_service: None,
            web_port: None,
            web_target_port: None,
            development: development.then(|| DevelopmentApplication {
                image_id: "image".into(),
                tag: "tag".into(),
                command: "sleep 300".into(),
                web_port: 3000,
                persist_data: false,
            }),
        };
        store.insert_application(&app).await.unwrap();
        let docker = FakeDocker::new();
        docker.apps.lock().unwrap().push(ApplicationContainer {
            name: "owned".into(),
            image: "example".into(),
            labels: apps::identity_labels("sample", "sample"),
            network: "test".into(),
            ports: vec![],
            env: vec![],
        });
        let router = crate::build_app(
            store,
            Arc::new(docker.clone()),
            Arc::new(FakeRoutes::new()),
            crate::metrics::Metrics::new(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "ws://{}/apps/id/sample/terminal",
            listener.local_addr().unwrap()
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (url, docker, server)
    }

    async fn first(url: &str, request: Value) -> Value {
        let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        ws.send(tokio_tungstenite::tungstenite::Message::Text(
            request.to_string().into(),
        ))
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
    async fn authenticates_before_opening_and_rejects_foreign_stopped_or_invalid_requests() {
        let (url, docker, server) = fixture(false).await;
        for request in [
            json!({"key":"wrong", "container":"owned", "cols":80,"rows":24}),
            json!({"key":"test-key", "container":"foreign", "cols":80,"rows":24}),
            json!({"key":"test-key", "container":"owned", "cols":0,"rows":24}),
        ] {
            assert_eq!(first(&url, request).await["type"], "error");
        }
        docker.stopped.lock().unwrap().insert("owned".into());
        assert_eq!(
            first(
                &url,
                json!({"key":"test-key", "container":"owned", "cols":80,"rows":24})
            )
            .await["type"],
            "error"
        );
        assert!(docker.terminals.lock().unwrap().is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn selects_container_and_preserves_configured_user_or_development_user() {
        for development in [false, true] {
            let (url, docker, server) = fixture(development).await;
            assert_eq!(
                first(
                    &url,
                    json!({"key":"test-key", "container":"owned", "cols":100,"rows":32})
                )
                .await["type"],
                "ready"
            );
            let calls = docker.terminals.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, "owned");
            assert_eq!(calls[0].1.cols, 100);
            assert_eq!(calls[0].1.rows, 32);
            assert_eq!(calls[0].2.as_deref(), development.then_some("dev"));
            server.abort();
        }
    }
}
