//! The Operator's settings, and where each value comes from.
//!
//! Precedence is PostgreSQL's, reduced to the levels that exist here: a
//! built-in default, the Operator's setting (what `ALTER SYSTEM` is there),
//! and the command line, which wins when given. A setting written while the
//! flag pins the value is kept and inert, and applies again once the flag is
//! gone. The API always says which level produced the effective value.

use std::time::Duration;

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::{AppState, error::ErrorReport, metrics::parse_duration, store::StateStore};

/// How long audit history is kept when neither the flag nor a setting says.
pub const DEFAULT_AUDIT_EVENTS_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
/// The shortest retention accepted. Below a day the history stops being one.
pub const MIN_AUDIT_EVENTS_MAX_AGE: Duration = Duration::from_secs(24 * 3600);
pub const AUDIT_EVENTS_MAX_AGE_KEY: &str = "audit_events_max_age";
/// The audit subject id for a change to this setting.
pub const AUDIT_EVENTS_MAX_AGE_SUBJECT: &str = "audit-events-max-age";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    CommandLine,
    Operator,
    Default,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Retention {
    pub effective: String,
    pub source: Source,
    /// The Operator's setting, whether or not it is the effective value.
    pub setting: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub audit_events_max_age: Retention,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Update {
    /// A duration like `45d`, or `null` to remove the setting.
    pub audit_events_max_age: Option<String>,
}

/// The retention in force right now: the flag if the daemon has one, else the
/// Operator's setting, else the default.
pub(crate) async fn audit_events_max_age<S: StateStore>(state: &AppState<S>) -> Duration {
    if let Some(flag) = state.audit_events_max_age_flag {
        return flag;
    }
    setting(state)
        .await
        .and_then(|text| parse_duration(&text).ok())
        .unwrap_or(DEFAULT_AUDIT_EVENTS_MAX_AGE)
}

/// The Operator's setting. An empty value is how a removed setting is
/// stored, and reads as none.
async fn setting<S: StateStore>(state: &AppState<S>) -> Option<String> {
    state
        .store
        .get_state(AUDIT_EVENTS_MAX_AGE_KEY)
        .await
        .ok()
        .flatten()
        .filter(|text| !text.is_empty())
}

async fn current<S: StateStore>(state: &AppState<S>) -> Settings {
    let setting = setting(state).await;
    let (effective, source) = match (state.audit_events_max_age_flag, &setting) {
        (Some(flag), _) => (flag, Source::CommandLine),
        (None, Some(text)) => match parse_duration(text) {
            Ok(value) => (value, Source::Operator),
            Err(_) => (DEFAULT_AUDIT_EVENTS_MAX_AGE, Source::Default),
        },
        (None, None) => (DEFAULT_AUDIT_EVENTS_MAX_AGE, Source::Default),
    };
    Settings {
        audit_events_max_age: Retention {
            effective: format_duration(effective),
            source,
            setting,
        },
    }
}

pub(crate) async fn get<S: StateStore>(State(state): State<AppState<S>>) -> Response {
    Json(current(&state).await).into_response()
}

pub(crate) async fn update<S: StateStore>(
    State(state): State<AppState<S>>,
    Json(update): Json<Update>,
) -> Response {
    let before = current(&state).await;
    let result = match update.audit_events_max_age.as_deref().map(str::trim) {
        Some(text) if !text.is_empty() => match validate(text) {
            Ok(value) => state
                .store
                .store_state(AUDIT_EVENTS_MAX_AGE_KEY, &format_duration(value))
                .await
                .map_err(|e| e.to_string()),
            Err(message) => {
                return (StatusCode::BAD_REQUEST, Json(ErrorReport::plain(message)))
                    .into_response();
            }
        },
        _ => state
            .store
            .store_state(AUDIT_EVENTS_MAX_AGE_KEY, "")
            .await
            .map_err(|e| e.to_string()),
    };
    if let Err(error) = result {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorReport::plain(error)),
        )
            .into_response();
    }
    // The history is trimmed to the new value at once, and the audit event
    // for this very change is what triggers it. The event carries what
    // changed; the middleware that closes it keeps a description it finds.
    let after = current(&state).await;
    let mut event = crate::audit::event(
        "configure",
        crate::audit::Subject {
            kind: "settings".into(),
            id: AUDIT_EVENTS_MAX_AGE_SUBJECT.into(),
            name: "Audit history".into(),
            available: None,
        },
        "completed",
        crate::audit::actor(),
        "Settings changed.",
    );
    event.changes = Some(vec![crate::audit::Change {
        setting: AUDIT_EVENTS_MAX_AGE_KEY.into(),
        from: describe(&before.audit_events_max_age),
        to: describe(&after.audit_events_max_age),
    }]);
    crate::audit::record(&state, event).await;
    Json(after).into_response()
}

/// A retention as the history should read it: the value, and "default" when
/// nothing the Operator set produced it.
fn describe(retention: &Retention) -> String {
    match retention.source {
        Source::Default => format!("{} (default)", retention.effective),
        _ => retention.effective.clone(),
    }
}

fn validate(text: &str) -> Result<Duration, String> {
    let value = parse_duration(text)?;
    if value < MIN_AUDIT_EVENTS_MAX_AGE {
        return Err(format!(
            "audit_events_max_age must be at least {}",
            format_duration(MIN_AUDIT_EVENTS_MAX_AGE)
        ));
    }
    Ok(value)
}

/// The shortest text `parse_duration` reads back to the same value.
pub fn format_duration(value: Duration) -> String {
    let seconds = value.as_secs();
    for (unit, size) in [('d', 86_400), ('h', 3600), ('m', 60)] {
        if seconds > 0 && seconds.is_multiple_of(size) {
            return format!("{}{unit}", seconds / size);
        }
    }
    format!("{seconds}s")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        docker::FakeDocker, environments::FakeVmRuntime, metrics::Metrics, routes::FakeRoutes,
        store::FakeStateStore,
    };
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::Request,
    };
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn setup(flag: Option<Duration>) -> (Router, FakeStateStore) {
        let store = FakeStateStore::new();
        store.store_state("api_key", "root-secret").await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        let app = crate::build_platform(
            store.clone(),
            Arc::new(FakeDocker::new()),
            Arc::new(FakeRoutes::new()),
            Metrics::new(),
            Arc::new(FakeVmRuntime),
            Arc::new(crate::dns_records::UnservedZone),
            flag,
        )
        .0;
        (app, store)
    }

    async fn call(app: &Router, method: &str, body: Value) -> (StatusCode, Value) {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri("/settings")
                    .header("authorization", "Bearer root-secret")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn the_default_is_reported_when_nothing_is_set() {
        let (app, _) = setup(None).await;
        let (status, body) = call(&app, "GET", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["auditEventsMaxAge"]["effective"], "30d");
        assert_eq!(body["auditEventsMaxAge"]["source"], "default");
        assert_eq!(body["auditEventsMaxAge"]["setting"], Value::Null);
    }

    #[tokio::test]
    async fn the_operators_setting_wins_over_the_default_and_null_removes_it() {
        let (app, _) = setup(None).await;
        let (status, body) = call(&app, "PUT", json!({"auditEventsMaxAge": "45d"})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["auditEventsMaxAge"]["effective"], "45d");
        assert_eq!(body["auditEventsMaxAge"]["source"], "operator");
        assert_eq!(body["auditEventsMaxAge"]["setting"], "45d");

        let (_, body) = call(&app, "PUT", json!({"auditEventsMaxAge": null})).await;
        assert_eq!(body["auditEventsMaxAge"]["effective"], "30d");
        assert_eq!(body["auditEventsMaxAge"]["source"], "default");
    }

    #[tokio::test]
    async fn the_command_line_pins_the_value_and_is_named_as_the_source() {
        let (app, _) = setup(Some(Duration::from_secs(7 * 86_400))).await;
        let (_, body) = call(&app, "PUT", json!({"auditEventsMaxAge": "45d"})).await;
        assert_eq!(body["auditEventsMaxAge"]["effective"], "7d");
        assert_eq!(body["auditEventsMaxAge"]["source"], "command-line");
        // Kept for when the flag goes away, as ALTER SYSTEM keeps a value a
        // command-line option overrides.
        assert_eq!(body["auditEventsMaxAge"]["setting"], "45d");
    }

    #[tokio::test]
    async fn an_invalid_or_too_short_value_is_refused() {
        let (app, _) = setup(None).await;
        let (status, _) = call(&app, "PUT", json!({"auditEventsMaxAge": "soon"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _) = call(&app, "PUT", json!({"auditEventsMaxAge": "12h"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (_, body) = call(&app, "GET", Value::Null).await;
        assert_eq!(body["auditEventsMaxAge"]["source"], "default");
    }

    #[tokio::test]
    async fn a_change_is_audited_and_a_shorter_value_prunes_at_once() {
        let (app, store) = setup(None).await;
        let old = crate::audit::event(
            "create",
            crate::audit::Subject {
                kind: "application".into(),
                id: "app".into(),
                name: "blog".into(),
                available: None,
            },
            "completed",
            None,
            "Action completed.",
        );
        let mut old = old;
        old.occurred_at = "2026-01-01T00:00:00.000000000Z".into();
        old.updated_at = Some(old.occurred_at.clone());
        crate::audit::Journal::default()
            .upsert(&store, old, Duration::from_secs(365 * 86_400))
            .await
            .unwrap();
        assert_eq!(store.list_audit_events().await.unwrap().len(), 1);

        let (status, _) = call(&app, "PUT", json!({"auditEventsMaxAge": "1d"})).await;
        assert_eq!(status, StatusCode::OK);

        let events: Vec<Value> = store
            .list_audit_events()
            .await
            .unwrap()
            .iter()
            .map(|body| serde_json::from_str(body).unwrap())
            .collect();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0]["subject"]["kind"], "settings");
        assert_eq!(events[0]["subject"]["id"], AUDIT_EVENTS_MAX_AGE_SUBJECT);
        assert_eq!(events[0]["action"], "configure");
        assert_eq!(events[0]["status"], "completed");
        assert_eq!(events[0]["description"], "Settings changed.");
        assert_eq!(
            events[0]["changes"],
            json!([{"setting": "audit_events_max_age", "from": "30d (default)", "to": "1d"}])
        );
    }

    #[test]
    fn durations_format_to_the_shortest_text_that_reads_back() {
        for (seconds, text) in [
            (30 * 86_400, "30d"),
            (36 * 3600, "36h"),
            (90 * 60, "90m"),
            (45, "45s"),
        ] {
            let value = Duration::from_secs(seconds);
            assert_eq!(format_duration(value), text);
            assert_eq!(parse_duration(text).unwrap(), value);
        }
    }
}
