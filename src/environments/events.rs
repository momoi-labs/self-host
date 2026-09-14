//! An append-only record of what happened to each Virtual machine.
//!
//! The lifecycle record in Platform State keeps the steps, because that is
//! what the list screens read on every poll. The guest's own output belongs
//! here instead: a create prints megabytes of `apt` and `mise`, which would
//! blow through the record's cap and rewrite the whole store line by line.
//!
//! A file also survives the daemon. When a bootstrap fails before SSH exists,
//! this is the only copy of the reason that the Operator can still reach.

use std::path::PathBuf;

use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::paths::platform_config_dir;

/// One generation is kept before the current file, so a failure is still
/// readable after a long retry has pushed it out of the live file.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Where the event logs live. The local harness points its state somewhere
/// temporary, and its logs follow it there rather than landing in the
/// Operator's real Platform State.
fn dir() -> PathBuf {
    std::env::var_os("SELF_HOST_ENVIRONMENTS_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(platform_config_dir)
        .join("environments")
}

/// `None` for anything that is not an environment identifier this crate
/// issued, so a crafted id cannot name a file outside the directory.
fn file(id: &str) -> Option<PathBuf> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return None;
    }
    Some(dir().join(format!("{id}.log")))
}

fn stamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

/// Records `text` against `id`, one output line per file line, each carrying
/// the time and the action that produced it. Writing an event must never fail
/// an operation, so every error here is dropped on purpose.
pub async fn append(id: &str, action: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    let Some(path) = file(id) else { return };
    let at = stamp();
    let mut buffer = String::with_capacity(text.len() + 64);
    for line in text.lines() {
        buffer.push_str(&format!("{at} {action} {line}\n"));
    }
    if buffer.is_empty() {
        return;
    }
    if tokio::fs::create_dir_all(&path.parent().unwrap())
        .await
        .is_err()
    {
        return;
    }
    rotate(&path).await;
    let Ok(mut handle) = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
    else {
        return;
    };
    let _ = handle.write_all(buffer.as_bytes()).await;
}

/// The whole history of one machine would be a surprise to read and to serve,
/// so callers take the end of it. The first line is trimmed to a whole one.
pub async fn tail(id: &str, bytes: u64) -> String {
    let Some(path) = file(id) else {
        return String::new();
    };
    let Ok(mut handle) = tokio::fs::File::open(&path).await else {
        return String::new();
    };
    let Ok(metadata) = handle.metadata().await else {
        return String::new();
    };
    let size = metadata.len();
    let from = size.saturating_sub(bytes);
    if handle.seek(std::io::SeekFrom::Start(from)).await.is_err() {
        return String::new();
    }
    let mut read = Vec::with_capacity(bytes.min(size) as usize);
    if handle.read_to_end(&mut read).await.is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&read).into_owned();
    if from == 0 {
        return text;
    }
    match text.find('\n') {
        Some(index) => text[index + 1..].to_string(),
        None => String::new(),
    }
}

/// Deleting a machine takes its record with it, and the events describe a
/// machine that no longer exists.
pub async fn discard(id: &str) {
    let Some(path) = file(id) else { return };
    let _ = tokio::fs::remove_file(&path).await;
    let _ = tokio::fs::remove_file(path.with_extension("log.1")).await;
}

async fn rotate(path: &std::path::Path) {
    let Ok(metadata) = tokio::fs::metadata(path).await else {
        return;
    };
    if metadata.len() < MAX_FILE_BYTES {
        return;
    }
    let _ = tokio::fs::rename(path, path.with_extension("log.1")).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An identifier that walks out of the directory names no file at all.
    #[test]
    fn only_an_issued_identifier_names_a_file() {
        assert!(file("env-8f5e32b21b48").is_some());
        assert!(file("../../etc/passwd").is_none());
        assert!(file("env/../escape").is_none());
        assert!(file("").is_none());
    }

    #[tokio::test]
    async fn every_output_line_carries_its_time_and_action() {
        let id = "env-testappend01";
        discard(id).await;
        append(id, "create", "installing\ndone\n").await;

        let written = tail(id, 4096).await;
        let lines: Vec<&str> = written.lines().collect();

        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with(" create installing"));
        assert!(lines[1].ends_with(" create done"));
        assert!(lines[0].starts_with("20"));
        discard(id).await;
    }

    /// A short read starts at a line boundary, so the Operator never sees the
    /// tail of a line whose beginning was cut off.
    #[tokio::test]
    async fn a_partial_read_drops_the_line_it_landed_inside() {
        let id = "env-testtail0001";
        discard(id).await;
        append(id, "create", "first\nsecond\nthird\n").await;

        let all = tail(id, 4096).await;
        let end = tail(id, 12).await;

        assert_eq!(all.lines().count(), 3);
        assert!(end.lines().all(|line| line.contains("third")));
        discard(id).await;
    }
}
