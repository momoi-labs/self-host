//! Bounded PostgreSQL commands. SQL and dumps enter stdin, never argv or logs.

use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::io::AsyncWriteExt;

use crate::docker::DockerError;

#[derive(Clone)]
pub enum Request {
    Sql {
        container: String,
        database: String,
        role: String,
        sql: String,
    },
    Restore {
        container: String,
        database: String,
        role: String,
        file: PathBuf,
    },
    RemoveVolume {
        name: String,
    },
}

pub async fn execute(request: &Request) -> Result<String, DockerError> {
    let mut command = tokio::process::Command::new("docker");
    command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let input = match request {
        Request::Sql {
            container,
            database,
            role,
            sql,
        } => {
            command.args([
                "exec",
                "--user",
                "postgres",
                "--env",
                "PGOPTIONS=-c statement_timeout=600000 -c lock_timeout=30000",
                "-i",
                container,
                "timeout",
                "--signal=TERM",
                "--kill-after=5s",
                "840s",
                "psql",
                "--no-psqlrc",
                "--no-password",
                "--quiet",
                "--tuples-only",
                "--no-align",
                "--set",
                "ON_ERROR_STOP=1",
                "--dbname",
                database,
                "--username",
                role,
            ]);
            Some(sql.as_bytes().to_vec())
        }
        Request::Restore {
            container,
            database,
            role,
            file,
        } => {
            // An import takes as long as its archive needs; nothing times it.
            command.args([
                "exec",
                "--user",
                "postgres",
                "--interactive",
                container,
                "pg_restore",
                "--verbose",
                "--no-password",
                "--format=custom",
                "--no-owner",
                "--no-privileges",
                "--single-transaction",
                "--exit-on-error",
                "--dbname",
                database,
                "--username",
                role,
            ]);
            Some(
                tokio::fs::read(file)
                    .await
                    .map_err(|_| unavailable("Could not read the private import file"))?,
            )
        }
        Request::RemoveVolume { name } => {
            command.args(["volume", "rm", name]);
            None
        }
    };
    command.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let mut child = command
        .spawn()
        .map_err(|_| unavailable("Could not start Docker for the database operation"))?;
    let writer = input.map(|input| {
        let mut stdin = child.stdin.take().expect("piped stdin");
        tokio::spawn(async move { stdin.write_all(&input).await })
    });
    let output = if matches!(request, Request::Restore { .. }) {
        child.wait_with_output().await
    } else {
        tokio::time::timeout(Duration::from_secs(900), child.wait_with_output())
            .await
            .map_err(|_| unavailable("The database operation timed out"))?
    }
    .map_err(|_| unavailable("Could not complete the database operation"))?;
    if let Some(writer) = writer {
        let _ = writer.await;
    }
    let args: Vec<String> = command
        .as_std()
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    crate::command_log::record(&args, &output);
    if let Request::Restore { .. } = request {
        // pg_restore reports progress and errors on stderr. The Operator
        // imports their own archive and needs that log to fix it; the role
        // signs in over the local socket, so no password appears in it.
        let log = String::from_utf8_lossy(&output.stderr).into_owned();
        return if output.status.success() {
            Ok(log)
        } else {
            // The line pg_restore stopped at, so the failure says why.
            let last = log
                .lines()
                .rev()
                .find(|line| line.contains("error:"))
                .or_else(|| log.lines().rev().find(|line| !line.trim().is_empty()))
                .unwrap_or("it printed nothing")
                .trim()
                .to_owned();
            Err(DockerError::Command(
                format!("pg_restore failed: {last}"),
                Box::new(RestoreLog(log)),
            ))
        };
    }
    if !output.status.success() {
        // PostgreSQL repeats SQL, credentials and source data in error output.
        // Return a fixed diagnosis instead of recording stderr in the Task.
        return Err(unavailable(match request {
            Request::Restore { .. } => {
                "Import did not complete successfully. Check compatibility, required extensions and the target database before retrying"
            }
            Request::RemoveVolume { .. } => {
                "Could not remove the database volume. Check that no container still uses it"
            }
            _ => "PostgreSQL refused the operation. Check database readiness and retry",
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// What pg_restore wrote before it failed.
#[derive(Debug)]
pub struct RestoreLog(pub String);

impl std::fmt::Display for RestoreLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("pg_restore failed")
    }
}

impl std::error::Error for RestoreLog {}

fn unavailable(message: &str) -> DockerError {
    DockerError::Unavailable(message.into())
}
