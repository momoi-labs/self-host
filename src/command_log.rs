//! The commands a database stage ran, as the Operator would have typed them,
//! with what each one printed. A stage opens a log with [`scope`]; every
//! `docker` run inside it adds itself with [`record`]. Outside a scope,
//! recording does nothing.

use std::sync::Mutex;

tokio::task_local! {
    static LOG: Mutex<Vec<Entry>>;
}

struct Entry {
    text: String,
    repeats: usize,
}

/// Runs `work` with a fresh log and hands the log back with its result.
pub async fn scope<T>(work: impl std::future::Future<Output = T>) -> (T, Vec<String>) {
    LOG.scope(Mutex::new(Vec::new()), async {
        let result = work.await;
        let lines = LOG
            .try_with(|log| {
                log.lock()
                    .map(|entries| {
                        entries
                            .iter()
                            .flat_map(|entry| {
                                let mut lines: Vec<String> =
                                    entry.text.lines().map(str::to_owned).collect();
                                if entry.repeats > 1 {
                                    lines.push(format!("(ran {} times)", entry.repeats));
                                }
                                lines
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        (result, lines)
    })
    .await
}

/// Adds one finished command: the command line, its stdout and stderr, and
/// its exit status when it failed. The same command with the same output, as
/// a readiness wait repeats it, is counted instead of written again.
pub fn record(args: &[impl AsRef<str>], output: &std::process::Output) {
    let _ = LOG.try_with(|log| {
        let mut text = format!(
            "$ docker {}",
            args.iter()
                .map(|arg| quote(arg.as_ref()))
                .collect::<Vec<_>>()
                .join(" ")
        );
        for stream in [&output.stdout, &output.stderr] {
            let printed = String::from_utf8_lossy(stream);
            let printed = printed.trim_end();
            if !printed.is_empty() {
                text.push('\n');
                text.push_str(printed);
            }
        }
        if !output.status.success() {
            text.push_str(&match output.status.code() {
                Some(code) => format!("\nexit status {code}"),
                None => "\nended by a signal".into(),
            });
        }
        if let Ok(mut entries) = log.lock() {
            match entries.last_mut() {
                Some(last) if last.text == text => last.repeats += 1,
                _ => entries.push(Entry { text, repeats: 1 }),
            }
        }
    });
}

fn quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_./:=@,+%".contains(&c))
    {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn output(code: i32, stdout: &str, stderr: &str) -> std::process::Output {
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[tokio::test]
    async fn a_scope_keeps_each_command_its_output_and_how_it_ended() {
        let ((), lines) = scope(async {
            record(&["inspect", "db"], &output(0, "starting\n", ""));
            record(&["inspect", "db"], &output(0, "starting\n", ""));
            record(
                &["compose", "up", "-d"],
                &output(1, "", "failed to create network x: all predefined address pools have been fully subnetted\n"),
            );
            record(&["exec", "db", "psql", "-c", "select 'a b'"], &output(0, "1\n", ""));
        })
        .await;
        assert_eq!(
            lines,
            [
                "$ docker inspect db",
                "starting",
                "(ran 2 times)",
                "$ docker compose up -d",
                "failed to create network x: all predefined address pools have been fully subnetted",
                "exit status 1",
                "$ docker exec db psql -c 'select '\\''a b'\\'''",
                "1",
            ]
        );
    }

    #[test]
    fn outside_a_scope_nothing_is_kept() {
        record(&["ps"], &output(0, "", ""));
    }
}
