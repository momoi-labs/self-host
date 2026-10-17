//! Database operations record observations, and Docker or PostgreSQL output
//! only with every managed password cut out.

use super::Error;
use crate::{AppState, audit, error::ErrorReport, store::StateStore};

pub(super) async fn step<S: StateStore, T>(
    state: &AppState<S>,
    id: &str,
    label: &str,
    completed: &str,
    work: impl std::future::Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    let event = audit::event_id();
    if let Some(event) = &event {
        state
            .audit
            .stage(&state.store, event, id, label, None)
            .await?;
    }
    let (result, commands) = crate::command_log::scope(work).await;
    if let (Some(event), false) = (&event, commands.is_empty()) {
        // The commands this stage ran and what they printed, so a failure
        // shows its cause. Without the passwords to cut out, none of it.
        if let Ok(secrets) = super::secrets(&state.store).await {
            let log = super::redact(&commands.join("\n"), &secrets);
            state.audit.stage_log(&state.store, event, id, &log).await?;
        }
    }
    let result = match result {
        // Runtime failures may contain Compose's rendered environment or SQL.
        // The stage keeps what Docker said, with every managed password cut
        // out; without the passwords to cut, it keeps only the label.
        Err(Error::Runtime(reason)) => {
            Err(Error::Runtime(match super::secrets(&state.store).await {
                Ok(secrets) => {
                    // The last line the failing command printed says why;
                    // the stage log has the rest.
                    let cause = failed_line(&commands)
                        .map(|line| format!(": {line}"))
                        .unwrap_or_default();
                    super::redact(&format!("{label} failed: {reason}{cause}"), &secrets)
                }
                Err(_) => format!("{label} failed. Check PostgreSQL and Docker logs, then retry"),
            }))
        }
        other => other,
    };
    if let Some(event) = &event {
        let report = result.as_ref().err().map(|error| ErrorReport::new(error));
        let outcome = match &report {
            Some(error) => Err(error),
            None => Ok(completed),
        };
        state
            .audit
            .stage(&state.store, event, id, label, Some(outcome))
            .await?;
    }
    result
}

/// The last line a failed command printed before its exit status.
fn failed_line(commands: &[String]) -> Option<&str> {
    let end = commands
        .iter()
        .rposition(|line| line.starts_with("exit status "))?;
    commands[..end]
        .iter()
        .rev()
        .take_while(|line| !line.starts_with("$ docker "))
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_failed_line_is_the_last_one_the_failing_command_printed() {
        let commands: Vec<String> = [
            "$ docker inspect db",
            "running",
            "$ docker compose up -d",
            " Network x Creating",
            "failed to create network x: all predefined address pools have been fully subnetted",
            "exit status 1",
        ]
        .map(String::from)
        .into();
        assert_eq!(
            super::failed_line(&commands),
            Some(
                "failed to create network x: all predefined address pools have been fully subnetted"
            )
        );
        assert_eq!(super::failed_line(&commands[..2]), None);
    }
}
