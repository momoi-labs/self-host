//! Database operations record observations, never SQL, connection URLs or
//! Docker output, which can include rendered credentials.

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
    let result = work.await.map_err(|error| match error {
        // Runtime failures may contain Compose's rendered environment or SQL.
        // Keep the failing stage actionable without persisting that output.
        Error::Runtime(_) => Error::Runtime(format!(
            "{label} failed. Check PostgreSQL and Docker logs, then retry"
        )),
        other => other,
    });
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
