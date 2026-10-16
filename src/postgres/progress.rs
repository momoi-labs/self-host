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
            let log = super::redact(&super::restore_log(&commands.join("\n")), &secrets);
            state.audit.stage_log(&state.store, event, id, &log).await?;
        }
    }
    let result = match result {
        // Runtime failures may contain Compose's rendered environment or SQL.
        // The stage keeps what Docker said, with every managed password cut
        // out; without the passwords to cut, it keeps only the label.
        Err(Error::Runtime(reason)) => {
            Err(Error::Runtime(match super::secrets(&state.store).await {
                Ok(secrets) => format!("{label} failed: {}", super::redact(&reason, &secrets)),
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
