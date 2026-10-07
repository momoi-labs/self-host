//! Operator-side client and the installer's exact-command root entry point.

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

use super::lifecycle::{NativeRuntime, S6Runtime};
use super::supervision::{ServiceStatus, host};
use crate::apps::ApplicationRecord;
use crate::store::Runtime;

pub const ROOT: &str = "/Library/Application Support/self-host/native";
pub const DATA: &str = "/Library/Application Support/self-host/native-data";
pub const HELPER: &str = "/Library/PrivilegedHelperTools/dev.momoi.self-host";
pub const S6: &str = "/Library/PrivilegedHelperTools/dev.momoi.self-host.s6";
const MAX_REQUEST: u64 = 256 * 1024;
const DEADLINE: Duration = Duration::from_secs(960);

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Deploy {
        record: Box<ApplicationRecord>,
        environment: Vec<(String, String)>,
        running: bool,
    },
    Start {
        id: String,
    },
    Stop {
        id: String,
    },
    Restart {
        id: String,
    },
    Remove {
        id: String,
    },
    Status {
        id: String,
    },
    Logs {
        id: String,
    },
}

impl Request {
    fn id(&self) -> &str {
        match self {
            Self::Deploy { record, .. } => &record.id,
            Self::Start { id }
            | Self::Stop { id }
            | Self::Restart { id }
            | Self::Remove { id }
            | Self::Status { id }
            | Self::Logs { id } => id,
        }
    }

    fn validate(&mut self) -> Result<()> {
        validate_id(self.id())?;
        if let Self::Deploy {
            record,
            environment,
            ..
        } = self
        {
            let Runtime::Native(definition) = &mut record.runtime else {
                bail!("only native definitions are accepted");
            };
            if record.source != crate::apps::SOURCE_NATIVE
                || record.git.is_some()
                || record.compose.is_some()
                || record.development.is_some()
            {
                bail!("macOS native Applications require a direct command");
            }
            super::lifecycle::validate_definition(&record.id, definition, record.publication)?;
            super::lifecycle::validate_environment(environment)?;
        }
        Ok(())
    }
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 25
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        bail!("invalid native Application id");
    }
    super::account_name_for(id)?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct Reply {
    error: Option<String>,
    status: Option<ServiceStatus>,
}

#[derive(Debug, Default)]
pub struct HelperRuntime {}

impl HelperRuntime {
    async fn spawn(request: &Request) -> Result<tokio::process::Child> {
        let bytes = serde_json::to_vec(request)?;
        if bytes.len() as u64 > MAX_REQUEST {
            bail!("native request exceeds 256 KiB");
        }
        host::protected(Path::new(HELPER))
            .context("macOS native helper is not installed; rerun the self-host installer")?;
        let mut child = tokio::process::Command::new("/usr/bin/sudo")
            .args(["-n", HELPER, "native-control"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let mut input = child.stdin.take().context("missing helper stdin")?;
        tokio::time::timeout(Duration::from_secs(5), async {
            input.write_all(&bytes).await?;
            input.shutdown().await
        })
        .await
        .context("native helper did not read its request")??;
        Ok(child)
    }

    async fn call(request: Request) -> Result<Reply> {
        let mut child = Self::spawn(&request).await?;
        let mut stdout = child.stdout.take().unwrap().take(1024 * 1024);
        let mut stderr = child.stderr.take().unwrap().take(64 * 1024);
        let mut output = Vec::new();
        let mut errors = Vec::new();
        let result = tokio::time::timeout(DEADLINE, async {
            tokio::try_join!(
                stdout.read_to_end(&mut output),
                stderr.read_to_end(&mut errors),
                child.wait()
            )
        })
        .await
        .context("native helper timed out")??;
        if !result.2.success() {
            bail!(
                "native helper failed: {}",
                String::from_utf8_lossy(&errors).trim()
            );
        }
        let reply: Reply =
            serde_json::from_slice(&output).context("invalid native helper response")?;
        if let Some(error) = &reply.error {
            bail!("{error}");
        }
        Ok(reply)
    }
}

#[async_trait]
impl NativeRuntime for HelperRuntime {
    async fn deploy(
        &self,
        record: &ApplicationRecord,
        environment: Vec<(String, String)>,
        running: bool,
    ) -> Result<()> {
        Self::call(Request::Deploy {
            record: Box::new(record.clone()),
            environment,
            running,
        })
        .await?;
        Ok(())
    }
    async fn start(&self, record: &ApplicationRecord) -> Result<()> {
        Self::call(Request::Start {
            id: record.id.clone(),
        })
        .await?;
        Ok(())
    }
    async fn stop(&self, record: &ApplicationRecord) -> Result<()> {
        Self::call(Request::Stop {
            id: record.id.clone(),
        })
        .await?;
        Ok(())
    }
    async fn restart(&self, record: &ApplicationRecord) -> Result<()> {
        Self::call(Request::Restart {
            id: record.id.clone(),
        })
        .await?;
        Ok(())
    }
    async fn remove(&self, record: &ApplicationRecord) -> Result<()> {
        Self::call(Request::Remove {
            id: record.id.clone(),
        })
        .await?;
        Ok(())
    }
    async fn status(&self, record: &ApplicationRecord) -> Result<ServiceStatus> {
        Self::call(Request::Status {
            id: record.id.clone(),
        })
        .await?
        .status
        .context("missing native status")
    }
    async fn logs(
        &self,
        record: &ApplicationRecord,
    ) -> Result<tokio::sync::mpsc::Receiver<String>> {
        let mut child = Self::spawn(&Request::Logs {
            id: record.id.clone(),
        })
        .await?;
        let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        // The initial reply reports missing records and permissions before
        // returning a stream to the HTTP handler.
        let line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await??
            .context("native log helper did not respond")?;
        let reply: Reply = serde_json::from_str(&line)?;
        if let Some(error) = reply.error {
            bail!("{error}");
        }
        let (sender, receiver) = tokio::sync::mpsc::channel(64);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = sender.closed() => break,
                    line = lines.next_line() => match line {
                        Ok(Some(line)) if !line.is_empty() => {
                            let Ok(line) = serde_json::from_str::<String>(&line) else { break; };
                            if sender.send(line).await.is_err() { break; }
                        }
                        Ok(Some(_)) => (),
                        _ => break,
                    }
                }
            }
            // Closing stdout also makes the root helper's next heartbeat exit.
            drop(lines);
            let _ = child.kill().await;
        });
        Ok(receiver)
    }
}

fn operation_lock(id: &str) -> Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    let locks = Path::new(ROOT).join("locks");
    host::directory(&locks, 0o700)?;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(locks.join(id))?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!("another native operation is in progress for this Application");
    }
    Ok(file)
}

/// This is the only entry point granted in sudoers. No paths, executables
/// for root, account names, or environment options can be supplied as args.
pub async fn control() -> Result<()> {
    host::require_root()?;
    if std::env::args_os().count() != 2 || std::env::current_exe()? != Path::new(HELPER) {
        bail!("native-control requires the installed protected helper");
    }
    host::protected(Path::new(HELPER))?;
    host::protected(Path::new(ROOT))?;
    let mut bytes = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        tokio::io::stdin()
            .take(MAX_REQUEST + 1)
            .read_to_end(&mut bytes),
    )
    .await??;
    if bytes.len() as u64 > MAX_REQUEST {
        bail!("native request exceeds 256 KiB");
    }
    let mut request: Request = serde_json::from_slice(&bytes)?;
    request.validate()?;
    let runtime = S6Runtime::default();
    let records = Path::new(ROOT).join("records");
    host::directory(&records, 0o700)?;
    let record_path = records.join(format!("{}.json", request.id()));
    let _lock = if matches!(request, Request::Status { .. } | Request::Logs { .. }) {
        None
    } else {
        Some(operation_lock(request.id())?)
    };
    let result = async {
        if let Request::Deploy { record, environment, running } = &request {
            // Save before side effects so a failed setup can be stopped or
            // removed after the Platform restarts.
            host::write(&record_path, &serde_json::to_vec(record)?, 0o600)?;
            runtime.deploy(record, environment.clone(), *running).await?;
            return Ok::<_, anyhow::Error>(None);
        }
        if matches!(request, Request::Remove { .. }) && !record_path.exists() {
            return Ok(None);
        }
        host::protected(&record_path)?;
        let record: ApplicationRecord = serde_json::from_slice(&std::fs::read(&record_path)?)?;
        match request {
            Request::Start { .. } => runtime.start(&record).await?,
            Request::Stop { .. } => runtime.stop(&record).await?,
            Request::Restart { .. } => runtime.restart(&record).await?,
            Request::Remove { .. } => {
                runtime.remove(&record).await?;
                std::fs::remove_file(&record_path)?;
            }
            Request::Status { .. } => return Ok(Some(runtime.status(&record).await?)),
            Request::Logs { .. } => {
                let mut receiver = runtime.logs(&record).await?;
                write_reply(&Reply { error: None, status: None }).await?;
                let mut stdout = tokio::io::stdout();
                loop {
                    let line = tokio::select! {
                        line = receiver.recv() => match line { Some(line) => serde_json::to_string(&line)?, None => break },
                        _ = tokio::time::sleep(Duration::from_secs(1)) => String::new(),
                    };
                    stdout.write_all(line.as_bytes()).await?;
                    stdout.write_all(b"\n").await?;
                    stdout.flush().await?;
                }
            }
            Request::Deploy { .. } => unreachable!(),
        }
        Ok(None)
    }.await;
    write_reply(&match result {
        Ok(status) => Reply {
            error: None,
            status,
        },
        Err(error) => Reply {
            error: Some(format!("{error:#}")),
            status: None,
        },
    })
    .await
}

async fn write_reply(reply: &Reply) -> Result<()> {
    let mut bytes = serde_json::to_vec(reply)?;
    bytes.push(b'\n');
    let mut stdout = tokio::io::stdout();
    stdout.write_all(&bytes).await?;
    stdout.flush().await?;
    Ok(())
}

/// Called by uninstall.sh as root before removing the scan LaunchDaemon.
pub async fn uninstall() -> Result<()> {
    host::require_root()?;
    host::protected(Path::new(HELPER))?;
    let records = Path::new(ROOT).join("records");
    if !records.exists() {
        return Ok(());
    }
    host::protected(&records)?;
    for entry in std::fs::read_dir(records)? {
        let path = entry?.path();
        // Atomic writes can leave an unfinished temporary after power loss.
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        host::protected(&path)?;
        let record: ApplicationRecord = serde_json::from_slice(&std::fs::read(&path)?)?;
        validate_id(&record.id)?;
        if path.file_stem().and_then(|name| name.to_str()) != Some(&record.id) {
            bail!("invalid native record filename");
        }
        let _lock = operation_lock(&record.id)?;
        S6Runtime::default().remove(&record).await?;
        std::fs::remove_file(path)?;
    }
    Ok(())
}

pub(crate) fn drain_readers(readers: Vec<std::thread::JoinHandle<()>>) {
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    while readers.iter().any(|reader| !reader.is_finished()) && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    for reader in readers {
        if reader.is_finished() {
            let _ = reader.join();
        }
    }
}

pub(crate) fn private_listener(port: u16, uid: u32, process_group: u32) -> Result<bool> {
    use std::io::Read;
    let mut child = std::process::Command::new("/usr/sbin/lsof")
        .args(["-nP", &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-F0pgun"])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(256 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("native socket inspection timed out");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let bytes = reader
        .join()
        .map_err(|_| anyhow::anyhow!("socket inspection failed"))??;
    if bytes.len() > 256 * 1024 {
        bail!("native socket inspection exceeded its output limit");
    }
    if !status.success() && !(status.code() == Some(1) && bytes.is_empty()) {
        bail!("native socket inspection failed");
    }
    parse_listeners(&String::from_utf8(bytes)?, port, uid, process_group)
}

fn parse_listeners(text: &str, port: u16, uid: u32, process_group: u32) -> Result<bool> {
    let (mut owner, mut group, mut ready) = (None, None, false);
    for field in text
        .split('\0')
        .map(|v| v.trim_start_matches('\n'))
        .filter(|v| !v.is_empty())
    {
        let (kind, value) = field.split_at(1);
        match kind {
            "p" => {
                value.parse::<u32>()?;
                owner = None;
                group = None;
            }
            "u" => owner = Some(value.parse::<u32>()?),
            "g" => group = Some(value.parse::<u32>()?),
            "n" => {
                if owner != Some(uid)
                    || group != Some(process_group)
                    || (value != format!("127.0.0.1:{port}") && value != format!("[::1]:{port}"))
                {
                    bail!(
                        "native Web Target must listen only on loopback under its Application Account and active process group"
                    );
                }
                ready |= value == format!("127.0.0.1:{port}");
            }
            _ => (),
        }
    }
    Ok(ready)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helper_rejects_paths_extra_fields_and_unknown_operations() {
        for input in [
            r#"{"operation":"start","id":"../root"}"#,
            r#"{"operation":"start","id":""}"#,
        ] {
            assert!(
                serde_json::from_str::<Request>(input)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        assert!(
            serde_json::from_str::<Request>(r#"{"operation":"start","id":"app","root":"/tmp"}"#)
                .is_err()
        );
        assert!(serde_json::from_str::<Request>(r#"{"operation":"exec","command":"id"}"#).is_err());
    }
    #[test]
    fn readiness_requires_loopback_owner_and_active_group() {
        assert!(
            parse_listeners("p42\0g42\0u5001\0\nf3\0n127.0.0.1:8080\0\n", 8080, 5001, 42).unwrap()
        );
        for socket in ["*:8080", "192.168.1.2:8080"] {
            assert!(
                parse_listeners(&format!("p42\0g42\0u5001\0n{socket}\0"), 8080, 5001, 42).is_err()
            );
        }
        assert!(parse_listeners("p42\0g43\0u5001\0n127.0.0.1:8080\0", 8080, 5001, 42).is_err());
        assert!(parse_listeners("p42\0g42\0u5002\0n127.0.0.1:8080\0", 8080, 5001, 42).is_err());
        assert!(!parse_listeners("", 8080, 5001, 42).unwrap());
    }

    #[test]
    fn installed_lsof_observes_a_real_loopback_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let uid = unsafe { libc::getuid() };
        let group = unsafe { libc::getpgrp() } as u32;
        assert!(private_listener(port, uid, group).unwrap());
        assert!(private_listener(port, uid + 1, group).is_err());
    }
}
