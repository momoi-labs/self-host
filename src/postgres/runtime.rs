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
        format: super::ArchiveFormat,
    },
    RemoveVolume {
        name: String,
    },
}

pub async fn execute(request: &Request) -> Result<String, DockerError> {
    if let Request::Restore {
        container,
        database,
        role,
        file,
        format,
    } = request
    {
        return restore(container, database, role, file, *format).await;
    }
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
        Request::RemoveVolume { name } => {
            command.args(["volume", "rm", name]);
            None
        }
        Request::Restore { .. } => unreachable!("restores run in restore()"),
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
    let output = tokio::time::timeout(Duration::from_secs(900), child.wait_with_output())
        .await
        .map_err(|_| unavailable("The database operation timed out"))?
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
    if !output.status.success() {
        // PostgreSQL repeats SQL, credentials and source data in error output.
        // Return a fixed diagnosis instead of recording stderr in the Task.
        return Err(unavailable(match request {
            Request::RemoveVolume { .. } => {
                "Could not remove the database volume. Check that no container still uses it"
            }
            _ => "PostgreSQL refused the operation. Check database readiness and retry",
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Restores `file` into `database` as `role`. A file streams into the tool's
/// stdin, so its size never sits in memory; a directory is copied into the
/// container first and removed after. Nothing times it.
async fn restore(
    container: &str,
    database: &str,
    role: &str,
    file: &std::path::Path,
    format: super::ArchiveFormat,
) -> Result<String, DockerError> {
    use super::ArchiveFormat;
    let staged = format!("/tmp/sf-import-{:032x}", rand::random::<u128>());
    let source = if format == ArchiveFormat::Directory {
        let copied = run(
            &[
                "cp".into(),
                file.to_string_lossy().into_owned(),
                format!("{container}:{staged}"),
            ],
            None,
        )
        .await?;
        if !copied.status.success() {
            return Err(failed("docker cp", &copied));
        }
        let owned = run(
            &[
                "exec".into(),
                "--user".into(),
                "root".into(),
                container.into(),
                "chown".into(),
                "--recursive".into(),
                "postgres".into(),
                staged.clone(),
            ],
            None,
        )
        .await?;
        if !owned.status.success() {
            return Err(failed("chown", &owned));
        }
        staged.clone()
    } else {
        "-".into()
    };
    let (tool, mut args): (&str, Vec<String>) = match format {
        ArchiveFormat::Sql => (
            "psql",
            vec![
                "psql".into(),
                "--no-psqlrc".into(),
                "--no-password".into(),
                "--echo-errors".into(),
                "--set".into(),
                "ON_ERROR_STOP=1".into(),
                "--single-transaction".into(),
                "--dbname".into(),
                database.into(),
                "--username".into(),
                role.into(),
                "--file".into(),
                source.clone(),
            ],
        ),
        other => (
            "pg_restore",
            vec![
                "pg_restore".into(),
                "--verbose".into(),
                "--no-password".into(),
                format!(
                    "--format={}",
                    match other {
                        ArchiveFormat::Tar => "tar",
                        ArchiveFormat::Directory => "directory",
                        _ => "custom",
                    }
                ),
                "--no-owner".into(),
                "--no-privileges".into(),
                "--single-transaction".into(),
                "--exit-on-error".into(),
                "--dbname".into(),
                database.into(),
                "--username".into(),
                role.into(),
            ],
        ),
    };
    if format == ArchiveFormat::Directory {
        args.push(source.clone());
    }
    let streamed = format != ArchiveFormat::Directory;
    let mut command: Vec<String> = vec!["exec".into(), "--user".into(), "postgres".into()];
    if streamed {
        command.push("--interactive".into());
    }
    command.push(container.into());
    command.extend(args);
    let output = run(&command, streamed.then_some(file)).await;
    if format == ArchiveFormat::Directory {
        let _ = run(
            &[
                "exec".into(),
                "--user".into(),
                "root".into(),
                container.into(),
                "rm".into(),
                "--recursive".into(),
                "--force".into(),
                staged,
            ],
            None,
        )
        .await;
    }
    let output = output?;
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() {
        Ok(log)
    } else {
        Err(failed(tool, &output))
    }
}

/// Runs one docker command, its stdin fed from `input` when there is one,
/// and records it for the stage.
async fn run(
    args: &[String],
    input: Option<&std::path::Path>,
) -> Result<std::process::Output, DockerError> {
    let mut command = tokio::process::Command::new("docker");
    command
        .args(args)
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let mut child = command
        .spawn()
        .map_err(|_| unavailable("Could not start Docker for the database operation"))?;
    let writer = match input {
        Some(path) => {
            let mut file = tokio::fs::File::open(path)
                .await
                .map_err(|e| unavailable(&format!("Could not read {}: {e}", path.display())))?;
            let mut stdin = child.stdin.take().expect("piped stdin");
            Some(tokio::spawn(async move {
                tokio::io::copy(&mut file, &mut stdin).await
            }))
        }
        None => None,
    };
    let output = child
        .wait_with_output()
        .await
        .map_err(|_| unavailable("Could not complete the database operation"))?;
    if let Some(writer) = writer {
        let _ = writer.await;
    }
    crate::command_log::record(args, &output);
    Ok(output)
}

/// A failed restore command, named by the last error line it printed.
fn failed(tool: &str, output: &std::process::Output) -> DockerError {
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let last = log
        .lines()
        .rev()
        .find(|line| line.contains("error:") || line.contains("ERROR:"))
        .or_else(|| log.lines().rev().find(|line| !line.trim().is_empty()))
        .unwrap_or("it printed nothing")
        .trim()
        .to_owned();
    DockerError::Command(format!("{tool} failed: {last}"), Box::new(RestoreLog(log)))
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
