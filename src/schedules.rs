//! Scheduled tasks (#201): work the Platform runs on its own, on a day or a
//! week, at an hour. A due schedule is enqueued as an ordinary Task, so each
//! run has a status and an audit event like any console action (ADR-0027).
//!
//! The schedules are built in; the Operator turns each on or off and picks
//! when it runs. What they chose is kept in Platform State.

use crate::AppState;
use crate::audit::Subject;
use crate::collection::{Collection, Keyed, TASKS};
use crate::store::{StateStore, StoreError};
use crate::tasks::{self, Work};
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime, Time};

const SCHEDULES: Collection<Schedule> = Collection::new("schedule");

/// How often a schedule runs. A weekly one runs on Sundays.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Every {
    Day,
    Week,
}

/// A schedule as the Operator set it, and what it last did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schedule {
    pub id: String,
    pub enabled: bool,
    pub every: Every,
    /// The hour it runs, 0 to 23, in UTC.
    pub hour: u8,
    /// When it was turned on or changed. A slot before this is not a run
    /// that was missed, so turning a schedule on does not run it at once.
    pub since: String,
    pub last_run_at: Option<String>,
    pub last_task: Option<String>,
}

impl Keyed for Schedule {
    fn key(&self) -> String {
        self.id.clone()
    }
}

/// A schedule the Platform knows how to run.
struct Builtin {
    id: &'static str,
    title: &'static str,
    description: &'static str,
    every: Every,
    hour: u8,
    action: &'static str,
    subject: fn() -> Subject,
    work: fn() -> Work,
}

/// The subject a `prune` run is recorded under, scheduled or asked for.
/// Named for what it cleans up after, so the Events row does not repeat the
/// action ("Remove unused data").
pub fn prune_subject() -> Subject {
    Subject::new("platform", "prune", "Removed applications")
}

const BUILTINS: &[Builtin] = &[Builtin {
    id: "prune",
    title: "Remove unused data",
    description: "Runs prune: removes what removed Applications left behind and builds no deployment keeps.",
    every: Every::Week,
    hour: 3,
    action: "prune",
    subject: prune_subject,
    work: || Work::Prune,
}];

/// A schedule as the console shows it.
#[derive(Clone, Debug, Serialize)]
pub struct View {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub enabled: bool,
    pub every: Every,
    pub hour: u8,
    pub last_run_at: Option<String>,
    pub last_task: Option<String>,
    /// The last run is still queued or running.
    pub running: bool,
    /// When it runs next; `None` while it is off.
    pub next_run_at: Option<String>,
}

fn builtin(id: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|builtin| builtin.id == id)
}

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

fn format(at: OffsetDateTime) -> String {
    at.format(&Rfc3339).unwrap_or_default()
}

fn parse(at: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(at, &Rfc3339).ok()
}

/// The most recent time the schedule was due, at or before `now`.
fn latest_slot(now: OffsetDateTime, every: Every, hour: u8) -> OffsetDateTime {
    let at = Time::from_hms(hour.min(23), 0, 0).unwrap_or(Time::MIDNIGHT);
    let mut slot = now.replace_time(at);
    if let Every::Week = every {
        slot -= Duration::days(i64::from(now.weekday().number_days_from_sunday()));
    }
    let period = period(every);
    while slot > now {
        slot -= period;
    }
    slot
}

fn period(every: Every) -> Duration {
    match every {
        Every::Day => Duration::days(1),
        Every::Week => Duration::weeks(1),
    }
}

/// Whether a schedule should run now: it is on, a slot passed since it was
/// turned on and since its last run, and that run is not still going.
fn due(schedule: &Schedule, now: OffsetDateTime, running: bool) -> bool {
    if !schedule.enabled || running {
        return false;
    }
    let slot = latest_slot(now, schedule.every, schedule.hour);
    let since = parse(&schedule.since).unwrap_or(now);
    let last = schedule.last_run_at.as_deref().and_then(parse);
    slot > since && last.is_none_or(|last| slot > last)
}

fn default_for(builtin: &Builtin) -> Schedule {
    Schedule {
        id: builtin.id.into(),
        enabled: false,
        every: builtin.every,
        hour: builtin.hour,
        since: format(now()),
        last_run_at: None,
        last_task: None,
    }
}

async fn stored(store: &impl StateStore, builtin: &Builtin) -> Result<Schedule, StoreError> {
    Ok(SCHEDULES
        .get(store, builtin.id)
        .await?
        .unwrap_or_else(|| default_for(builtin)))
}

async fn running(store: &impl StateStore, schedule: &Schedule) -> Result<bool, StoreError> {
    match &schedule.last_task {
        // A task that finished is no longer on file.
        Some(task) => TASKS.exists(store, task).await,
        None => Ok(false),
    }
}

async fn view(store: &impl StateStore, builtin: &'static Builtin) -> Result<View, StoreError> {
    let schedule = stored(store, builtin).await?;
    let running = running(store, &schedule).await?;
    let next_run_at = schedule.enabled.then(|| {
        format(latest_slot(now(), schedule.every, schedule.hour) + period(schedule.every))
    });
    Ok(View {
        id: builtin.id,
        title: builtin.title,
        description: builtin.description,
        enabled: schedule.enabled,
        every: schedule.every,
        hour: schedule.hour,
        last_run_at: schedule.last_run_at,
        last_task: schedule.last_task,
        running,
        next_run_at,
    })
}

pub async fn list(store: &impl StateStore) -> Result<Vec<View>, StoreError> {
    let mut views = Vec::new();
    for builtin in BUILTINS {
        views.push(view(store, builtin).await?);
    }
    Ok(views)
}

/// What the Operator may change about a schedule.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub enabled: bool,
    pub every: Every,
    pub hour: u8,
}

#[derive(Debug)]
pub enum Error {
    NotFound(String),
    InvalidHour(u8),
    Store(StoreError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotFound(id) => write!(f, "there is no schedule '{id}'"),
            Error::InvalidHour(hour) => write!(f, "hour {hour} is not between 0 and 23"),
            Error::Store(_) => write!(f, "could not read or save the schedule"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Store(error) => Some(error),
            _ => None,
        }
    }
}

impl From<StoreError> for Error {
    fn from(error: StoreError) -> Self {
        Error::Store(error)
    }
}

/// Saves what the Operator chose. The next slot counts from now, so turning
/// a schedule on or moving its hour does not run it at once.
pub async fn update(store: &impl StateStore, id: &str, change: Change) -> Result<View, Error> {
    let builtin = builtin(id).ok_or_else(|| Error::NotFound(id.into()))?;
    if change.hour > 23 {
        return Err(Error::InvalidHour(change.hour));
    }
    let mut schedule = stored(store, builtin).await?;
    schedule.enabled = change.enabled;
    schedule.every = change.every;
    schedule.hour = change.hour;
    schedule.since = format(now());
    SCHEDULES.upsert(store, &schedule).await?;
    Ok(view(store, builtin).await?)
}

/// Enqueues the schedule's work now, scheduled or asked for, and records the
/// run so the next one counts from it.
pub(crate) async fn run<S: StateStore>(state: &AppState<S>, id: &str) -> Result<String, Error> {
    let builtin = builtin(id).ok_or_else(|| Error::NotFound(id.into()))?;
    let task = tasks::enqueue(state, builtin.action, (builtin.subject)(), (builtin.work)()).await?;
    let mut schedule = stored(&state.store, builtin).await?;
    schedule.last_run_at = Some(format(now()));
    schedule.last_task = Some(task.clone());
    SCHEDULES.upsert(&state.store, &schedule).await?;
    Ok(task)
}

/// Runs what is due, once now and then every minute. A run the Host missed
/// while it was off runs once on start, not once per slot it missed.
pub(crate) fn spawn<S: StateStore>(state: AppState<S>) {
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(60));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            for builtin in BUILTINS {
                let schedule = match stored(&state.store, builtin).await {
                    Ok(schedule) => schedule,
                    Err(error) => {
                        tracing::warn!("could not read schedule {}: {error}", builtin.id);
                        continue;
                    }
                };
                let running = running(&state.store, &schedule).await.unwrap_or(true);
                if due(&schedule, now(), running)
                    && let Err(error) = run(&state, builtin.id).await
                {
                    tracing::warn!("could not run schedule {}: {error}", builtin.id);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn datetime(at: &str) -> OffsetDateTime {
        parse(at).unwrap()
    }

    fn schedule(
        every: Every,
        hour: u8,
        since: OffsetDateTime,
        last: Option<OffsetDateTime>,
    ) -> Schedule {
        Schedule {
            id: "prune".into(),
            enabled: true,
            every,
            hour,
            since: format(since),
            last_run_at: last.map(format),
            last_task: None,
        }
    }

    #[test]
    fn the_latest_slot_is_the_last_hour_or_sunday_passed() {
        // 2026-10-13 is a Tuesday.
        let now = datetime("2026-10-13T10:30:00Z");
        assert_eq!(
            latest_slot(now, Every::Day, 3),
            datetime("2026-10-13T03:00:00Z")
        );
        assert_eq!(
            latest_slot(now, Every::Day, 11),
            datetime("2026-10-12T11:00:00Z")
        );
        assert_eq!(
            latest_slot(now, Every::Week, 3),
            datetime("2026-10-11T03:00:00Z")
        );
        let sunday = datetime("2026-10-11T02:00:00Z");
        assert_eq!(
            latest_slot(sunday, Every::Week, 3),
            datetime("2026-10-04T03:00:00Z")
        );
    }

    #[test]
    fn turning_on_waits_for_the_next_slot() {
        let now = datetime("2026-10-13T10:30:00Z");
        let on = schedule(Every::Day, 3, now - Duration::minutes(5), None);
        assert!(!due(&on, now, false));
        assert!(due(&on, now + Duration::days(1), false));
    }

    #[test]
    fn a_missed_slot_runs_once_and_a_running_one_does_not_overlap() {
        let now = datetime("2026-10-13T10:30:00Z");
        let since = now - Duration::days(30);
        // Off for days: the last run is a week old, so one run is due now.
        let missed = schedule(Every::Day, 3, since, Some(now - Duration::days(7)));
        assert!(due(&missed, now, false));
        assert!(!due(&missed, now, true));
        // Once it ran after the slot, nothing more is due until the next one.
        let ran = schedule(Every::Day, 3, since, Some(datetime("2026-10-13T03:01:00Z")));
        assert!(!due(&ran, now, false));
        let mut off = missed.clone();
        off.enabled = false;
        assert!(!due(&off, now, false));
    }

    #[tokio::test]
    async fn a_schedule_starts_off_and_keeps_what_the_operator_chose() {
        let store = crate::store::FakeStateStore::new();
        let listed = list(&store).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].enabled);
        assert_eq!(listed[0].next_run_at, None);

        let view = update(
            &store,
            "prune",
            Change {
                enabled: true,
                every: Every::Day,
                hour: 4,
            },
        )
        .await
        .unwrap();
        assert!(view.enabled);
        assert_eq!((view.every, view.hour), (Every::Day, 4));
        assert!(view.next_run_at.is_some());
        assert!(matches!(
            update(
                &store,
                "prune",
                Change {
                    enabled: true,
                    every: Every::Day,
                    hour: 24
                }
            )
            .await,
            Err(Error::InvalidHour(24))
        ));
        assert!(matches!(
            update(
                &store,
                "nope",
                Change {
                    enabled: true,
                    every: Every::Day,
                    hour: 1
                }
            )
            .await,
            Err(Error::NotFound(_))
        ));
        assert!(list(&store).await.unwrap()[0].enabled);
    }
}
