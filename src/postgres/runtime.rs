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
        /// One transaction that stops at the first error; off, the tool goes
        /// on past errors and they are listed at the end.
        all_or_nothing: bool,
        /// The database container's image; plain SQL runs in a copy of it.
        image: String,
        /// The connection role's password, for plain SQL over TCP.
        password: String,
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
        all_or_nothing,
        image,
        password,
    } = request
    {
        let sandbox = Sandbox { image, password };
        let target = Target {
            container,
            database,
            role,
        };
        return restore(&target, file, *format, *all_or_nothing, &sandbox).await;
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
/// Where plain SQL runs: a throwaway copy of the database image that shares
/// only its network. A SQL dump can carry psql meta-commands, `\!` for a
/// shell or `\connect` to another role; there they reach no database files
/// or trusted local socket, and every connection needs a password.
struct Sandbox<'a> {
    image: &'a str,
    password: &'a str,
}

/// The database a restore fills, and the role it runs as.
struct Target<'a> {
    container: &'a str,
    database: &'a str,
    role: &'a str,
}

async fn restore(
    target: &Target<'_>,
    file: &std::path::Path,
    format: super::ArchiveFormat,
    all_or_nothing: bool,
    sandbox: &Sandbox<'_>,
) -> Result<String, DockerError> {
    let container = target.container;
    let staged = format!("/tmp/sf-import-{:032x}", rand::random::<u128>());
    let result = restore_from(target, file, format, all_or_nothing, &staged, sandbox).await;
    // Whatever was staged goes, whichever step stopped.
    if format == super::ArchiveFormat::Directory {
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
            None,
        )
        .await;
    }
    result
}

async fn restore_from(
    target: &Target<'_>,
    file: &std::path::Path,
    format: super::ArchiveFormat,
    all_or_nothing: bool,
    staged: &str,
    sandbox: &Sandbox<'_>,
) -> Result<String, DockerError> {
    use super::ArchiveFormat;
    let Target {
        container,
        database,
        role,
    } = *target;
    let staged = staged.to_owned();
    let source = if format == ArchiveFormat::Directory {
        // docker cp copies a symlink as a link; the dump is what it points to.
        let file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_owned());
        let copied = run(
            &[
                "cp".into(),
                file.to_string_lossy().into_owned(),
                format!("{container}:{staged}"),
            ],
            None,
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
    let (tool, mut args) = tool_args(format, database, role, &source, all_or_nothing);
    if format == ArchiveFormat::Directory {
        args.push(source.clone());
    }
    let streamed = format != ArchiveFormat::Directory;
    let mut command: Vec<String> = if format == ArchiveFormat::Sql {
        vec![
            "run".into(),
            "--rm".into(),
            "--interactive".into(),
            "--network".into(),
            format!("container:{container}"),
            "--user".into(),
            "65534:65534".into(),
            "--env".into(),
            "PGPASSWORD".into(),
            sandbox.image.into(),
        ]
    } else {
        let mut exec: Vec<String> = vec!["exec".into(), "--user".into(), "postgres".into()];
        if streamed {
            exec.push("--interactive".into());
        }
        exec.push(container.into());
        exec
    };
    if format == ArchiveFormat::Sql && gzipped(file) {
        // gzip's failure would not reach psql, which takes the end of what
        // it got for the end of the script: check the whole file first.
        let tested = run(
            &[
                "exec".into(),
                "--interactive".into(),
                container.into(),
                "gzip".into(),
                "--test".into(),
            ],
            Some(Input {
                path: file,
                before: "",
                after: "",
            }),
            None,
        )
        .await?;
        if !tested.status.success() {
            return Err(failed("gzip", &tested));
        }
        // psql cannot read compressed input; the container's gzip unpacks it
        // on the way in. The arguments go through "$@" untouched.
        command.extend([
            "sh".into(),
            "-c".into(),
            if all_or_nothing {
                "{ printf 'BEGIN;\\n' && gzip --decompress --stdout && printf '\\nCOMMIT;\\n'; } | \"$@\""
            } else {
                "gzip --decompress --stdout | \"$@\""
            }
            .into(),
            "sh".into(),
        ]);
    }
    command.extend(args);
    // psql's transaction is a BEGIN and a COMMIT around the stream, so a
    // dump cut short never commits. A gzip dump gets them from the shell;
    // pg_restore runs its own single transaction. Without all or nothing
    // there is no transaction to wrap.
    let input = streamed.then_some(match (format, gzipped(file)) {
        (ArchiveFormat::Sql, false) if all_or_nothing => Input {
            path: file,
            before: "BEGIN;\n",
            after: "\nCOMMIT;\n",
        },
        _ => Input {
            path: file,
            before: "",
            after: "",
        },
    });
    let output = run(
        &command,
        input,
        (format == ArchiveFormat::Sql).then_some(sandbox.password),
    )
    .await;
    let output = output?;
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !all_or_nothing && let Some(error) = went_past_errors(tool, &output) {
        return Err(error);
    }
    if output.status.success() {
        Ok(log)
    } else {
        Err(failed(tool, &output))
    }
}

/// The tool and its arguments, all in long form. All or nothing is one
/// transaction that stops at the first error; without it, pg_restore and
/// psql go on past errors as they do by default.
fn tool_args(
    format: super::ArchiveFormat,
    database: &str,
    role: &str,
    source: &str,
    all_or_nothing: bool,
) -> (&'static str, Vec<String>) {
    use super::ArchiveFormat;
    if format == ArchiveFormat::Sql {
        let mut args: Vec<String> = vec![
            "psql".into(),
            "--no-psqlrc".into(),
            "--no-password".into(),
            "--echo-errors".into(),
        ];
        if all_or_nothing {
            args.extend(["--set".into(), "ON_ERROR_STOP=1".into()]);
        }
        args.extend([
            "--host".into(),
            "127.0.0.1".into(),
            "--dbname".into(),
            database.into(),
            "--username".into(),
            role.into(),
            "--file".into(),
            source.into(),
        ]);
        return ("psql", args);
    }
    let mut args: Vec<String> = vec![
        "pg_restore".into(),
        "--verbose".into(),
        "--no-password".into(),
        format!(
            "--format={}",
            match format {
                ArchiveFormat::Tar => "tar",
                ArchiveFormat::Directory => "directory",
                _ => "custom",
            }
        ),
        "--no-owner".into(),
        "--no-privileges".into(),
    ];
    if all_or_nothing {
        args.extend(["--single-transaction".into(), "--exit-on-error".into()]);
    }
    args.extend([
        "--dbname".into(),
        database.into(),
        "--username".into(),
        role.into(),
    ]);
    ("pg_restore", args)
}

/// The error lines pg_restore or psql printed for statements that failed.
fn error_lines(log: &str) -> Vec<&str> {
    log.lines()
        .map(str::trim)
        .filter(|line| {
            line.starts_with("pg_restore: error:")
                || (line.starts_with("psql:") && line.contains("ERROR:"))
        })
        .collect()
}

/// A restore that went on past errors to its end, so the rest is restored.
/// The stage log ends with the errors, and the failure names how many and
/// the last. None when it met none, or stopped for another reason.
fn went_past_errors(tool: &str, output: &std::process::Output) -> Option<DockerError> {
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let errors = error_lines(&log);
    // pg_restore counts what it went past and exits 1; psql exits 0.
    let counted = log.lines().rev().find_map(|line| {
        line.trim()
            .strip_prefix("pg_restore: warning: errors ignored on restore: ")
            .and_then(|count| count.trim().parse::<usize>().ok())
    });
    let count = match (tool, counted) {
        ("pg_restore", Some(count)) => count,
        ("psql", _) if output.status.success() && !errors.is_empty() => errors.len(),
        _ => return None,
    };
    let statements = if count == 1 {
        "statement"
    } else {
        "statements"
    };
    let last = errors.last().copied().unwrap_or("it printed none");
    crate::command_log::note(&format!(
        "{count} {statements} failed; the restore went on past them:\n{}",
        errors.join("\n")
    ));
    Some(DockerError::Command(
        format!("{tool} restored the rest, but {count} {statements} failed. The last: {last}"),
        Box::new(RestoreLog(log)),
    ))
}

/// Whether `file` starts with gzip's magic bytes.
fn gzipped(file: &std::path::Path) -> bool {
    use std::io::Read;
    let mut magic = [0u8; 2];
    std::fs::File::open(file)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_ok()
        && magic == [0x1f, 0x8b]
}

/// What a restore command reads on stdin: a file, with text before and
/// after it. `after` is written only when the whole file was read, so a dump
/// cut short never reaches the COMMIT that would keep its first half.
struct Input<'a> {
    path: &'a std::path::Path,
    before: &'static str,
    after: &'static str,
}

/// The most of each output stream a command keeps: the end, where errors are.
/// A dump restored with one INSERT per row prints a line per row.
const OUTPUT_KEPT: usize = 1024 * 1024;

/// Reads a stream to its end, keeping only its last `OUTPUT_KEPT` bytes.
async fn tail(mut stream: impl tokio::io::AsyncRead + Unpin) -> Vec<u8> {
    use tokio::io::AsyncReadExt;
    let mut kept: std::collections::VecDeque<u8> = std::collections::VecDeque::new();
    let mut dropped = 0usize;
    let mut chunk = vec![0u8; 64 * 1024];
    while let Ok(read) = stream.read(&mut chunk).await {
        if read == 0 {
            break;
        }
        kept.extend(&chunk[..read]);
        if kept.len() > OUTPUT_KEPT {
            let excess = kept.len() - OUTPUT_KEPT;
            kept.drain(..excess);
            dropped += excess;
        }
    }
    let mut out = Vec::with_capacity(kept.len() + 64);
    if dropped > 0 {
        out.extend(format!("[{dropped} earlier bytes of output not kept]\n").bytes());
    }
    out.extend(kept);
    out
}

/// Runs one docker command, its stdin fed from `input` when there is one,
/// and records it for the stage. Output is kept bounded.
async fn run(
    args: &[String],
    input: Option<Input<'_>>,
    password: Option<&str>,
) -> Result<std::process::Output, DockerError> {
    let mut command = tokio::process::Command::new("docker");
    // The password reaches `docker run --env PGPASSWORD` from this process's
    // environment, never from its arguments.
    if let Some(password) = password {
        command.env("PGPASSWORD", password);
    }
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
    let stdout = tokio::spawn(tail(child.stdout.take().expect("piped stdout")));
    let stderr = tokio::spawn(tail(child.stderr.take().expect("piped stderr")));
    let writer = match input {
        Some(Input {
            path,
            before,
            after,
        }) => {
            let mut file = tokio::fs::File::open(path)
                .await
                .map_err(|e| unavailable(&format!("Could not read {}: {e}", path.display())))?;
            let mut stdin = child.stdin.take().expect("piped stdin");
            Some(tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                stdin.write_all(before.as_bytes()).await?;
                tokio::io::copy(&mut file, &mut stdin).await?;
                stdin.write_all(after.as_bytes()).await?;
                stdin.shutdown().await
            }))
        }
        None => None,
    };
    let status = child
        .wait()
        .await
        .map_err(|_| unavailable("Could not complete the database operation"))?;
    let output = std::process::Output {
        status,
        stdout: stdout.await.unwrap_or_default(),
        stderr: stderr.await.unwrap_or_default(),
    };
    crate::command_log::record(args, &output);
    // A dump that could not be read to its end must not pass as restored.
    if let Some(writer) = writer {
        let copied = writer
            .await
            .map_err(|e| e.to_string())
            .and_then(|copied| copied.map_err(|e| e.to_string()));
        if let (Err(error), true) = (copied, output.status.success()) {
            return Err(DockerError::Command(
                format!("Reading the dump failed: {error}"),
                Box::new(RestoreLog(String::new())),
            ));
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::postgres::ArchiveFormat;
    use std::os::unix::process::ExitStatusExt;

    fn output(code: i32, stderr: &str) -> std::process::Output {
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn all_or_nothing_is_one_transaction_that_stops_at_the_first_error() {
        let (_, on) = tool_args(ArchiveFormat::Tar, "db", "role", "-", true);
        assert!(on.contains(&"--single-transaction".into()));
        assert!(on.contains(&"--exit-on-error".into()));
        assert!(on.contains(&"--verbose".into()));
        assert!(on.contains(&"--format=tar".into()));
        let (_, off) = tool_args(ArchiveFormat::Tar, "db", "role", "-", false);
        assert!(!off.contains(&"--single-transaction".into()));
        assert!(!off.contains(&"--exit-on-error".into()));
        let (tool, on) = tool_args(ArchiveFormat::Sql, "db", "role", "-", true);
        assert_eq!(tool, "psql");
        assert!(on.contains(&"ON_ERROR_STOP=1".into()));
        let (_, off) = tool_args(ArchiveFormat::Sql, "db", "role", "-", false);
        assert!(!off.contains(&"ON_ERROR_STOP=1".into()));
    }

    #[tokio::test]
    async fn a_restore_past_errors_lists_them_at_the_end_and_names_the_last() {
        let printed = "pg_restore: creating TABLE \"public.t\"\n\
            pg_restore: error: could not execute query: ERROR:  relation \"u\" already exists\n\
            Command was: CREATE TABLE public.u (id integer);\n\
            pg_restore: warning: errors ignored on restore: 1\n";
        let (error, log) = crate::command_log::scope(async {
            went_past_errors("pg_restore", &output(1, printed))
        })
        .await;
        let error = error.expect("errors were ignored").to_string();
        assert!(error.contains("pg_restore restored the rest, but 1 statement failed"));
        assert!(error.ends_with("relation \"u\" already exists"), "{error}");
        assert_eq!(
            log.last().unwrap(),
            "pg_restore: error: could not execute query: ERROR:  relation \"u\" already exists"
        );
        // psql exits 0 past errors; a fatal stop is a plain failure.
        let psql = "psql:<stdin>:4: ERROR:  relation \"u\" already exists\n";
        assert!(went_past_errors("psql", &output(0, psql)).is_some());
        assert!(went_past_errors("psql", &output(0, "CREATE TABLE\n")).is_none());
        assert!(
            went_past_errors(
                "pg_restore",
                &output(1, "pg_restore: error: connection failed\n")
            )
            .is_none()
        );
    }

    /// Runs docker and answers its stdout, failing the test when it fails.
    fn docker(args: &[&str]) -> String {
        let output = std::process::Command::new("docker")
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "docker {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn peak_memory_kib() -> u64 {
        std::fs::read_to_string("/proc/self/status")
            .unwrap()
            .lines()
            .find_map(|line| line.strip_prefix("VmHWM:"))
            .and_then(|value| value.trim().trim_end_matches("kB").trim().parse().ok())
            .unwrap()
    }

    /// Every archive type restores for real, a large Host file streams, and
    /// a restore that is not all or nothing keeps what did not fail.
    #[tokio::test]
    #[ignore = "needs Docker and the postgres:18-bookworm image"]
    async fn every_dump_type_restores_into_a_real_database() {
        let image = "postgres:18-bookworm";
        let name = format!("sf-restore-test-{:08x}", rand::random::<u32>());
        let host = std::env::temp_dir().join(&name);
        std::fs::create_dir_all(&host).unwrap();
        docker(&[
            "run",
            "--detach",
            "--name",
            &name,
            "--env",
            "POSTGRES_PASSWORD=admin",
            image,
        ]);
        let sql = |database: &str, sql: &str| {
            docker(&[
                "exec",
                "--user",
                "postgres",
                &name,
                "psql",
                "--no-psqlrc",
                "--tuples-only",
                "--no-align",
                "--dbname",
                database,
                "--command",
                sql,
            ])
        };
        // The image restarts its server once after initdb; TCP comes last.
        for _ in 0..120 {
            let ready = std::process::Command::new("docker")
                .args([
                    "exec",
                    "--user",
                    "postgres",
                    &name,
                    "psql",
                    "--host",
                    "127.0.0.1",
                    "--command",
                    "SELECT 1",
                ])
                .output()
                .unwrap();
            if ready.status.success() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        sql("postgres", "CREATE ROLE importer LOGIN PASSWORD 'secret'");
        sql("postgres", "CREATE DATABASE source");
        sql(
            "source",
            "CREATE TABLE t (id int PRIMARY KEY); INSERT INTO t SELECT generate_series(1, 5); CREATE TABLE u (id int);",
        );
        let restore = |database: &str, file: &std::path::Path, format, all_or_nothing| {
            let request = Request::Restore {
                container: name.clone(),
                database: database.into(),
                role: "importer".into(),
                file: file.into(),
                format,
                all_or_nothing,
                image: image.into(),
                password: "secret".into(),
            };
            async move { execute(&request).await }
        };
        let fresh = |database: &str| {
            sql(
                "postgres",
                &format!("CREATE DATABASE {database} OWNER importer"),
            );
        };
        for (format, flag, file) in [
            (ArchiveFormat::Custom, "--format=custom", "dump.custom"),
            (ArchiveFormat::Tar, "--format=tar", "dump.tar"),
            (ArchiveFormat::Directory, "--format=directory", "dump.dir"),
            (ArchiveFormat::Sql, "--format=plain", "dump.sql"),
        ] {
            let inside = format!("/tmp/{file}");
            docker(&[
                "exec",
                "--user",
                "postgres",
                &name,
                "pg_dump",
                flag,
                "--no-owner",
                "--no-privileges",
                "--file",
                &inside,
                "--dbname",
                "source",
            ]);
            docker(&["cp", &format!("{name}:{inside}"), host.to_str().unwrap()]);
            let path = host.join(file);
            assert_eq!(
                ArchiveFormat::detect(&path),
                Some(format),
                "{file} is told from its bytes"
            );
            let target = format!("{format:?}").to_lowercase();
            fresh(&target);
            let log = restore(&target, &path, format, true).await.unwrap();
            if format != ArchiveFormat::Sql {
                assert!(log.contains("pg_restore: creating TABLE"), "{log}");
            }
            assert_eq!(sql(&target, "SELECT count(*) FROM t"), "5", "{file}");

            // A table already there makes one statement fail.
            for (all_or_nothing, rows) in [(true, "0"), (false, "5")] {
                let target = format!("{target}_clash_{all_or_nothing}");
                fresh(&target);
                sql(
                    &target,
                    "CREATE TABLE u (id int); ALTER TABLE u OWNER TO importer",
                );
                let error = restore(&target, &path, format, all_or_nothing)
                    .await
                    .unwrap_err()
                    .to_string();
                assert!(error.contains("\"u\" already exists"), "{file}: {error}");
                if !all_or_nothing {
                    assert!(
                        error.contains("restored the rest, but 1 statement failed"),
                        "{file}: {error}"
                    );
                }
                let kept = if sql(
                    &target,
                    "SELECT count(*) FROM pg_tables WHERE tablename = 't'",
                ) == "1"
                {
                    sql(&target, "SELECT count(*) FROM t")
                } else {
                    "0".into()
                };
                assert_eq!(kept, rows, "{file} all or nothing {all_or_nothing}");
            }
        }

        // A Host file past the old 64 MiB cap streams; it is never held whole.
        let big = host.join("big.sql");
        {
            use std::io::Write;
            let mut file = std::io::BufWriter::new(std::fs::File::create(&big).unwrap());
            writeln!(
                file,
                "--\n-- PostgreSQL database dump\n--\nCREATE TABLE big (line text);\nCOPY big (line) FROM stdin;"
            )
            .unwrap();
            let row = "x".repeat(1000);
            for _ in 0..80_000 {
                writeln!(file, "{row}").unwrap();
            }
            writeln!(file, "\\.").unwrap();
        }
        assert!(std::fs::metadata(&big).unwrap().len() > 64 * 1024 * 1024);
        fresh("big");
        let before = peak_memory_kib();
        restore("big", &big, ArchiveFormat::Sql, true)
            .await
            .unwrap();
        let grown = peak_memory_kib() - before;
        assert_eq!(sql("big", "SELECT count(*) FROM big"), "80000");
        assert!(grown < 16 * 1024, "peak memory grew by {grown} KiB");

        docker(&["rm", "--force", &name]);
        std::fs::remove_dir_all(host).unwrap();
    }
}
