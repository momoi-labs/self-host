//! PostgreSQL backups (ADR-0035): a server with backups on keeps a pgBackRest
//! repository in the Host's backup storage. pgBackRest runs inside the
//! database container, from a custom image, and reads a `pgbackrest.conf`
//! the Platform mounts. A restore makes a new server at a point in time.

use std::process::Stdio;

use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime, Time, UtcOffset, format_description::well_known::Rfc3339};

use super::{
    ADMIN, CONNECTIONS, DATABASES, Error, PASSWORD, STANDALONE, StandaloneDatabase, container,
    database, progress::step, sql, storage, template, template_with, wait_ready,
};
use crate::{
    AppState, apps, audit,
    collection::{Collection, Keyed, TASKS},
    store::{StateStore, StoreError},
    tasks,
};

pub(crate) static SETTINGS: Collection<BackupSettings> =
    Collection::new("postgres-backup-settings");
/// What the Platform last read from each repository and each archiver.
pub(crate) static REPOSITORIES: Collection<Repository> =
    Collection::new("postgres-backup-repository");
static WINDOW: Collection<MaintenanceWindow> = Collection::new("maintenance-window");
const WINDOW_ID: &str = "host";
/// A server's backup records change from Tasks, the scheduler and the API.
static WRITE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
/// Held while the storage moves and while a server's backup settings change,
/// so an enable never reads the location a move is replacing.
pub(crate) static STORAGE_MOVE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) const KEEP_DAYS_DEFAULT: u32 = 14;
pub(crate) const KEEP_DAYS_MAX: u32 = 3650;

/// A pgBackRest backup type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Full,
    Diff,
    Incr,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Diff => "diff",
            Self::Incr => "incr",
        }
    }
}

/// One backup in a repository, as `pgbackrest info` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backup {
    pub label: String,
    pub kind: Kind,
    pub started_at: String,
    pub finished_at: String,
    /// The size of the server the backup holds.
    pub bytes: u64,
    /// What this backup added to the bucket.
    pub repository_bytes: u64,
}

/// What PostgreSQL's archiver last said.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Archiving {
    pub last_archived_at: Option<String>,
    pub last_failed_at: Option<String>,
    /// The last attempt failed: WAL is piling up on the data volume.
    pub failing: bool,
}

/// The last test restore of a server's latest backup.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Verification {
    /// `running`, `passed` or `failed`.
    pub status: String,
    pub task_id: String,
    pub at: String,
    /// The backup that was restored.
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Repository {
    pub database_application_id: String,
    /// Newest first.
    #[serde(default)]
    pub backups: Vec<Backup>,
    #[serde(default)]
    pub archiving: Archiving,
    #[serde(default)]
    pub verification: Option<Verification>,
}

impl Keyed for Repository {
    fn key(&self) -> String {
        self.database_application_id.clone()
    }
}

/// A Task that is turning a server's backups on or off.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub task_id: String,
    pub enable: bool,
}

/// Whether a server is backed up, and how many days it can go back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupSettings {
    pub database_application_id: String,
    pub enabled: bool,
    pub keep_days: u32,
    /// When backups were turned on; a window that opened before this is not
    /// a run that was missed.
    pub since: String,
    #[serde(default)]
    pub last_run_at: Option<String>,
    #[serde(default)]
    pub change: Option<Change>,
    /// The backup Task that is queued or running.
    #[serde(default)]
    pub running: Option<String>,
    /// Why turning backups on, or the last backup, failed.
    #[serde(default)]
    pub error: Option<String>,
}

impl Keyed for BackupSettings {
    fn key(&self) -> String {
        self.database_application_id.clone()
    }
}

/// The hours the Host may run planned database work in, in its local time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaintenanceWindow {
    /// 0 to 23.
    pub start_hour: u8,
    /// 1 to 24.
    pub hours: u8,
}

impl Default for MaintenanceWindow {
    fn default() -> Self {
        Self {
            start_hour: 2,
            hours: 4,
        }
    }
}

impl Keyed for MaintenanceWindow {
    fn key(&self) -> String {
        WINDOW_ID.into()
    }
}

pub(crate) async fn window(store: &impl StateStore) -> Result<MaintenanceWindow, StoreError> {
    Ok(WINDOW.get(store, WINDOW_ID).await?.unwrap_or_default())
}

pub(crate) async fn set_window(
    store: &impl StateStore,
    window: MaintenanceWindow,
) -> Result<MaintenanceWindow, Error> {
    if window.start_hour > 23 {
        return Err(Error::Invalid(
            "The window starts at an hour from 0 to 23".into(),
        ));
    }
    if !(1..=24).contains(&window.hours) {
        return Err(Error::Invalid("The window lasts from 1 to 24 hours".into()));
    }
    WINDOW.upsert(store, &window).await?;
    Ok(window)
}

fn default_settings(id: &str) -> BackupSettings {
    BackupSettings {
        database_application_id: id.into(),
        enabled: false,
        keep_days: KEEP_DAYS_DEFAULT,
        since: audit::timestamp(),
        last_run_at: None,
        change: None,
        running: None,
        error: None,
    }
}

pub(crate) async fn settings(store: &impl StateStore, id: &str) -> Result<BackupSettings, Error> {
    Ok(SETTINGS
        .get(store, id)
        .await?
        .unwrap_or_else(|| default_settings(id)))
}

pub(crate) async fn repository(store: &impl StateStore, id: &str) -> Result<Repository, Error> {
    Ok(REPOSITORIES
        .get(store, id)
        .await?
        .unwrap_or_else(|| Repository {
            database_application_id: id.into(),
            ..Repository::default()
        }))
}

async fn update_settings(
    store: &impl StateStore,
    id: &str,
    change: impl FnOnce(&mut BackupSettings),
) -> Result<BackupSettings, Error> {
    let _write = WRITE.lock().await;
    let mut current = settings(store, id).await?;
    change(&mut current);
    SETTINGS.upsert(store, &current).await?;
    Ok(current)
}

async fn update_repository(
    store: &impl StateStore,
    id: &str,
    change: impl FnOnce(&mut Repository),
) -> Result<Repository, Error> {
    let _write = WRITE.lock().await;
    let mut current = repository(store, id).await?;
    change(&mut current);
    REPOSITORIES.upsert(store, &current).await?;
    Ok(current)
}

fn task_id() -> String {
    format!("event-{:032x}", rand::random::<u128>())
}

/// Queues `operation` as the Task `task_id`.
async fn enqueue<S: StateStore>(
    state: &AppState<S>,
    task_id: &str,
    action: &'static str,
    subject: audit::Subject,
    operation: super::Operation,
) -> Result<(), Error> {
    audit::EVENT_ID
        .scope(
            Some(task_id.to_owned()),
            tasks::enqueue(state, action, subject, tasks::Work::Postgres { operation }),
        )
        .await
        .map(drop)
        .map_err(Error::Store)
}

/// What the Operator may change about a server's backups.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SettingsChange {
    pub enabled: Option<bool>,
    pub keep_days: Option<u32>,
}

/// Saves the days to keep, and queues the Task that turns backups on or off.
/// `enabled` changes only when that Task ends well.
pub(crate) async fn change_settings<S: StateStore>(
    state: &AppState<S>,
    id: &str,
    change: SettingsChange,
) -> Result<(BackupSettings, Option<String>), Error> {
    let store = &state.store;
    let _move = STORAGE_MOVE.lock().await;
    let (definition, app) = database(store, id).await?;
    let mut current = settings(store, id).await?;
    if let Some(keep_days) = change.keep_days {
        if !(1..=KEEP_DAYS_MAX).contains(&keep_days) {
            return Err(Error::Invalid(format!(
                "Keep backups for 1 to {KEEP_DAYS_MAX} days"
            )));
        }
        if keep_days != current.keep_days {
            current = update_settings(store, id, |settings| settings.keep_days = keep_days).await?;
            if current.enabled {
                // pgBackRest reads the file on every run: no restart.
                let storage = storage::configured(store).await?;
                storage::write_conf(
                    &storage::conf_dir(id),
                    &storage::conf(&storage, id, definition.major, keep_days),
                )?;
            }
        }
    }
    let Some(enable) = change.enabled.filter(|enable| *enable != current.enabled) else {
        return Ok((current, None));
    };
    if let Some(change) = &current.change
        && TASKS.exists(store, &change.task_id).await?
    {
        return Err(Error::Conflict(
            "This server's backups are already being turned on or off".into(),
        ));
    }
    if enable {
        storage::configured(store).await?;
    }
    if app.status != "running" {
        return Err(Error::Conflict(
            "Start PostgreSQL before turning its backups on or off".into(),
        ));
    }
    let task = task_id();
    let saved = update_settings(store, id, |settings| {
        settings.change = Some(Change {
            task_id: task.clone(),
            enable,
        });
        settings.error = None;
    })
    .await?;
    let operation = if enable {
        super::Operation::EnableBackups { id: id.into() }
    } else {
        super::Operation::DisableBackups { id: id.into() }
    };
    let queued = enqueue(
        state,
        &task,
        "configure",
        audit::Subject::new("database", app.id, app.name),
        operation,
    )
    .await;
    if let Err(error) = queued {
        let _ = update_settings(store, id, |settings| settings.change = None).await;
        return Err(error);
    }
    Ok((saved, Some(task)))
}

/// Writes every backed-up server's `pgbackrest.conf` again, after the
/// storage changed.
pub(crate) async fn rewrite_confs(store: &impl StateStore) -> Result<(), Error> {
    let storage = storage::configured(store).await?;
    for settings in SETTINGS.list(store).await? {
        let id = &settings.database_application_id;
        if settings.enabled
            && let Some(definition) = DATABASES.get(store, id).await?
        {
            storage::write_conf(
                &storage::conf_dir(id),
                &storage::conf(&storage, id, definition.major, settings.keep_days),
            )?;
        }
    }
    Ok(())
}

fn parse(at: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(at, &Rfc3339).ok()
}

fn rfc3339(at: OffsetDateTime) -> String {
    at.to_offset(UtcOffset::UTC)
        .format(&Rfc3339)
        .unwrap_or_default()
}

/// The Host's offset from UTC at `at`, as its time zone has it.
pub(crate) fn local_offset(at: OffsetDateTime) -> UtcOffset {
    let seconds = at.unix_timestamp() as libc::time_t;
    // SAFETY: `localtime_r` writes only the `tm` it is given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let offset = if unsafe { libc::localtime_r(&seconds, &mut tm) }.is_null() {
        0
    } else {
        tm.tm_gmtoff
    };
    UtcOffset::from_whole_seconds(offset as i32).unwrap_or(UtcOffset::UTC)
}

/// The most recent time the window opened, at or before `now`, in `now`'s
/// own offset.
fn window_start(now: OffsetDateTime, window: MaintenanceWindow) -> OffsetDateTime {
    let at = Time::from_hms(window.start_hour.min(23), 0, 0).unwrap_or(Time::MIDNIGHT);
    let start = now.replace_time(at);
    if start > now {
        start - Duration::days(1)
    } else {
        start
    }
}

/// The backup a window runs: a full one when it opens on a Sunday, a
/// differential on Wednesday and Saturday, an incremental on the other days.
fn kind_on(start: OffsetDateTime) -> Kind {
    match start.weekday() {
        time::Weekday::Sunday => Kind::Full,
        time::Weekday::Wednesday | time::Weekday::Saturday => Kind::Diff,
        _ => Kind::Incr,
    }
}

/// The scheduled backup that should start now, if one should: backups are
/// on, the window is open, and it opened after they were turned on and
/// after the last run. `now` carries the Host's local offset.
fn due(settings: &BackupSettings, window: MaintenanceWindow, now: OffsetDateTime) -> Option<Kind> {
    if !settings.enabled {
        return None;
    }
    let start = window_start(now, window);
    if now >= start + Duration::hours(i64::from(window.hours)) {
        return None;
    }
    let since = parse(&settings.since).unwrap_or(now);
    let last = settings.last_run_at.as_deref().and_then(parse);
    (start > since && last.is_none_or(|last| start > last)).then(|| kind_on(start))
}

/// When the next scheduled backup starts and its type, or None when backups
/// are off.
pub(crate) fn next_run(
    settings: &BackupSettings,
    window: MaintenanceWindow,
    now: OffsetDateTime,
) -> Option<(String, Kind)> {
    if !settings.enabled {
        return None;
    }
    if let Some(kind) = due(settings, window, now) {
        return Some((rfc3339(now), kind));
    }
    let start = window_start(now, window) + Duration::days(1);
    Some((rfc3339(start), kind_on(start)))
}

pub(crate) fn local_now() -> OffsetDateTime {
    let now = OffsetDateTime::now_utc();
    now.to_offset(local_offset(now))
}

/// Queues a backup of `id`. A scheduled one is queued even when it cannot
/// succeed, so the failure shows as a Task.
pub(crate) async fn start<S: StateStore>(
    state: &AppState<S>,
    id: &str,
    kind: Kind,
    scheduled: bool,
) -> Result<String, Error> {
    let store = &state.store;
    let (_, app) = database(store, id).await?;
    let current = settings(store, id).await?;
    if !scheduled {
        if !current.enabled {
            return Err(Error::Conflict(
                "Turn backups on for this server before backing it up".into(),
            ));
        }
        if app.status != "running" {
            return Err(Error::Conflict(
                "Start PostgreSQL before backing it up".into(),
            ));
        }
    }
    if let Some(running) = &current.running
        && TASKS.exists(store, running).await?
    {
        return Err(Error::Conflict(
            "A backup of this server is already queued or running".into(),
        ));
    }
    let task = task_id();
    update_settings(store, id, |settings| settings.running = Some(task.clone())).await?;
    let queued = enqueue(
        state,
        &task,
        "backup",
        audit::Subject::new("database", app.id.clone(), app.name),
        super::Operation::Backup { id: app.id, kind },
    )
    .await;
    if let Err(error) = queued {
        let _ = update_settings(store, id, |settings| settings.running = None).await;
        return Err(error);
    }
    Ok(task)
}

/// The scheduler's tick: reads each archiver, then starts the scheduled
/// backups that are due, one server at a time. The next one starts on a
/// later tick, after the one before it ended.
pub(crate) async fn run_due<S: StateStore>(state: &AppState<S>) -> Result<(), Error> {
    let store = &state.store;
    let mut all = SETTINGS.list(store).await?;
    all.sort_by(|a, b| a.database_application_id.cmp(&b.database_application_id));
    let mut busy = false;
    for settings in &all {
        let id = &settings.database_application_id;
        if !DATABASES.exists(store, id).await? {
            forget(store, id).await?;
            continue;
        }
        if settings.enabled {
            watch_archiver(state, id).await;
        }
        if let Some(running) = &settings.running {
            if TASKS.exists(store, running).await? {
                busy = true;
            } else {
                // Its Task is gone without an outcome, as after a lost queue.
                update_settings(store, id, |settings| settings.running = None).await?;
            }
        }
    }
    if busy {
        return Ok(());
    }
    let window = window(store).await?;
    let now = local_now();
    for settings in all {
        let Some(kind) = due(&settings, window, now) else {
            continue;
        };
        let id = &settings.database_application_id;
        // One server that cannot start does not hold back the others.
        match start(state, id, kind, true).await {
            Ok(_) => {
                update_settings(store, id, |settings| {
                    settings.last_run_at = Some(audit::timestamp());
                })
                .await?;
                return Ok(());
            }
            Err(error) => {
                tracing::warn!(database = %id, %error, "A scheduled backup could not start");
            }
        }
    }
    Ok(())
}

const ARCHIVER: &str = r#"SELECT coalesce(to_char(last_archived_time AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'), ''), coalesce(to_char(last_failed_time AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS"Z"'), ''), last_failed_time IS NOT NULL AND (last_archived_time IS NULL OR last_failed_time > last_archived_time) FROM pg_stat_archiver;"#;

fn archiving(row: &str) -> Option<Archiving> {
    let mut fields = row.trim().split('|');
    let mut time = || {
        fields
            .next()
            .map(|at| (!at.is_empty()).then(|| at.to_owned()))
    };
    let last_archived_at = time()?;
    let last_failed_at = time()?;
    Some(Archiving {
        last_archived_at,
        last_failed_at,
        failing: fields.next()? == "t",
    })
}

/// Records what a running server's archiver says. A server that does not
/// answer keeps what it said last.
async fn watch_archiver<S: StateStore>(state: &AppState<S>, id: &str) {
    let store = &state.store;
    let running = matches!(database(store, id).await, Ok((_, app)) if app.status == "running");
    if !running {
        return;
    }
    let Ok(row) = sql(state, id, "postgres", ADMIN, ARCHIVER.into()).await else {
        return;
    };
    let Some(seen) = archiving(&row) else {
        return;
    };
    if repository(store, id)
        .await
        .is_ok_and(|repository| repository.archiving == seen)
    {
        return;
    }
    let _ = update_repository(store, id, |repository| repository.archiving = seen).await;
}

fn exec(container: &str, tool: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "exec".into(),
        "--user".into(),
        "postgres".into(),
        container.into(),
    ];
    args.extend(tool.iter().map(|part| (*part).to_owned()));
    args
}

fn stanza() -> String {
    format!("--stanza={}", storage::STANZA)
}

/// Runs one docker command to its end and records it for the stage.
async fn run_docker(args: &[String]) -> Result<std::process::Output, Error> {
    let output = tokio::process::Command::new("docker")
        .args(args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|_| Error::Unavailable("Could not start Docker for the backup".into()))?;
    crate::command_log::record(args, &output);
    Ok(output)
}

/// Runs a docker command that must succeed. The stage log has what it said.
async fn must(args: &[String], failure: &str) -> Result<String, Error> {
    let output = run_docker(args).await?;
    if !output.status.success() {
        return Err(Error::Runtime(failure.into()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The file the Platform's custom image is built from: the official image
/// and the `pgbackrest` package of the apt repository it already uses.
fn dockerfile(major: u16) -> String {
    format!(
        "FROM postgres:{major}-bookworm\nRUN apt-get update \\\n && apt-get install -y --no-install-recommends pgbackrest \\\n && rm -rf /var/lib/apt/lists/*\n"
    )
}

/// Refuses an image that does not run the given PostgreSQL major version.
pub(super) async fn runs_major(image: &str, major: u16) -> Result<(), Error> {
    let output = run_docker(&[
        "run".into(),
        "--rm".into(),
        "--entrypoint".into(),
        "postgres".into(),
        image.into(),
        "--version".into(),
    ])
    .await?;
    let found = super::major_of(&String::from_utf8_lossy(&output.stdout));
    match found {
        Some(found) if output.status.success() && found == major => Ok(()),
        Some(found) if output.status.success() => Err(Error::Invalid(format!(
            "This custom image runs PostgreSQL {found}. Choose PostgreSQL {found}, or an image built from postgres:{major}-bookworm"
        ))),
        _ => Err(Error::Invalid(format!(
            "This custom image does not run PostgreSQL. Build it from postgres:{major}-bookworm"
        ))),
    }
}

/// Refuses a custom image without pgBackRest, and says what to add to it.
async fn has_pgbackrest(image: &str, major: u16) -> Result<(), Error> {
    let output = run_docker(&[
        "run".into(),
        "--rm".into(),
        "--entrypoint".into(),
        "pgbackrest".into(),
        image.into(),
        "version".into(),
    ])
    .await?;
    if output.status.success() {
        return Ok(());
    }
    Err(Error::Conflict(format!(
        "The custom image of this server has no pgBackRest. Add this to its Dockerfile under Custom images, save it to rebuild it, then retry:\n{}",
        dockerfile(major)
            .lines()
            .skip(1)
            .collect::<Vec<_>>()
            .join("\n")
    )))
}

/// Moves a server created from a custom image to the image's current build,
/// so an edited and rebuilt image reaches the server when it is recreated.
/// Says whether the server's compose file changed.
pub(super) async fn follow_custom_image<S: StateStore>(
    state: &AppState<S>,
    id: &str,
) -> Result<bool, Error> {
    let store = &state.store;
    let (definition, mut app) = database(store, id).await?;
    if definition.custom_image.is_none() {
        return Ok(false);
    }
    let tag = super::server_image(state, &definition).await?;
    if tag == app.image {
        return Ok(false);
    }
    step(
        state,
        "custom-image",
        "Moving to the current build of the custom image",
        "The server uses the current build of its custom image.",
        async {
            runs_major(&tag, definition.major).await?;
            let enabled = settings(store, id).await?.enabled;
            if enabled {
                has_pgbackrest(&tag, definition.major).await?;
            }
            let dir = storage::conf_dir(id);
            app.compose = Some(template_with(
                definition.major,
                &tag,
                enabled.then_some(dir.as_path()),
            )?);
            app.image = tag;
            store.insert_application(&app).await?;
            Ok(true)
        },
    )
    .await
}

/// The image a server runs while its backups are on. A server created from a
/// custom image keeps it, and that image must carry pgBackRest itself.
async fn image<S: StateStore>(
    state: &AppState<S>,
    definition: &super::Database,
) -> Result<String, Error> {
    let major = definition.major;
    if definition.custom_image.is_some() {
        return step(
            state,
            "image",
            "Checking the custom image for pgBackRest",
            "The custom image of this server has pgBackRest.",
            async {
                let tag = super::server_image(state, definition).await?;
                has_pgbackrest(&tag, major).await?;
                Ok(tag)
            },
        )
        .await;
    }
    step(
        state,
        "image",
        "Building the PostgreSQL image with pgBackRest",
        "The custom image is ready on the Host.",
        async {
            let name = format!("postgres-{major}-backups");
            Box::pin(crate::custom_images::ensure_platform_image(
                state,
                &name,
                &dockerfile(major),
            ))
            .await
            .map_err(|report| {
                Error::Unavailable(format!(
                    "The custom image {name} did not build: {}. Its log is under Custom images",
                    report.error
                ))
            })
        },
    )
    .await
}

/// Saves `compose` as the server's definition and brings the container to it.
async fn apply_compose<S: StateStore>(
    state: &AppState<S>,
    id: &str,
    compose: String,
    image: &str,
) -> Result<(), Error> {
    let store = &state.store;
    let (_, mut app) = database(store, id).await?;
    app.compose = Some(compose);
    app.image = image.into();
    store.insert_application(&app).await?;
    Box::pin(apps::finish_deploy(
        store,
        state.docker.as_ref(),
        state.routes.as_ref(),
        apps::PendingDeploy::managed_compose(app),
    ))
    .await
    .map(drop)
    .map_err(|e| Error::Runtime(e.to_string()))
}

const ARCHIVE_SETTINGS: [&str; 3] = ["archive_mode", "archive_command", "archive_timeout"];

async fn archiving_off<S: StateStore>(state: &AppState<S>, id: &str) -> Result<(), Error> {
    // `ALTER SYSTEM` cannot share a command with another statement.
    for setting in ARCHIVE_SETTINGS {
        sql(
            state,
            id,
            "postgres",
            ADMIN,
            format!("ALTER SYSTEM RESET {setting};"),
        )
        .await?;
    }
    Ok(())
}

/// Reads the repository's backups into its record.
async fn refresh<S: StateStore>(state: &AppState<S>, id: &str) -> Result<Vec<Backup>, Error> {
    let listed = must(
        &exec(
            &container(id),
            &["pgbackrest", &stanza(), "--output=json", "info"],
        ),
        "pgBackRest could not list the repository",
    )
    .await?;
    let backups = listed_backups(&listed).ok_or_else(|| {
        Error::Runtime("pgBackRest listed the repository in a form self-host does not read".into())
    })?;
    let kept = backups.clone();
    update_repository(&state.store, id, |repository| repository.backups = kept).await?;
    Ok(backups)
}

/// The backups in `pgbackrest info --output=json`, newest first.
fn listed_backups(json: &str) -> Option<Vec<Backup>> {
    let listed: serde_json::Value = serde_json::from_str(json).ok()?;
    let mut backups = Vec::new();
    for backup in listed.get(0)?.get("backup")?.as_array()? {
        let at = |key: &str| {
            backup["timestamp"][key]
                .as_i64()
                .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok())
                .map(rfc3339)
        };
        backups.push(Backup {
            label: backup["label"].as_str()?.to_owned(),
            kind: match backup["type"].as_str()? {
                "full" => Kind::Full,
                "diff" => Kind::Diff,
                _ => Kind::Incr,
            },
            started_at: at("start")?,
            finished_at: at("stop")?,
            bytes: backup["info"]["size"].as_u64().unwrap_or(0),
            repository_bytes: backup["info"]["repository"]["delta"].as_u64().unwrap_or(0),
        });
    }
    backups.sort_by(|a, b| b.finished_at.cmp(&a.finished_at));
    Some(backups)
}

/// The Task that turns backups on: the custom image, the repository, WAL
/// archiving, then the first full backup. The repository comes before
/// archiving: PostgreSQL with archiving on and no repository cannot shut
/// down cleanly.
pub(super) async fn enable<S: StateStore>(state: &AppState<S>, id: &str) -> Result<(), Error> {
    let store = &state.store;
    let result = async {
        let (definition, app) = database(store, id).await?;
        if app.status != "running" {
            return Err(Error::Conflict(
                "Start PostgreSQL before turning its backups on".into(),
            ));
        }
        let configured = step(
            state,
            "storage",
            "Checking backup storage",
            "The bucket took, returned and deleted a test object.",
            async {
                let configured = storage::configured(store).await?;
                Box::pin(storage::test(&configured)).await?;
                Ok(configured)
            },
        )
        .await?;
        let tag = image(state, &definition).await?;
        let dir = storage::conf_dir(id);
        let keep_days = settings(store, id).await?.keep_days;
        step(
            state,
            "container",
            "Recreating PostgreSQL with pgBackRest",
            "The container runs the custom image and reads the backup configuration.",
            async {
                storage::write_conf(
                    &dir,
                    &storage::conf(&configured, id, definition.major, keep_days),
                )?;
                apply_compose(
                    state,
                    id,
                    template_with(definition.major, &tag, Some(&dir))?,
                    &tag,
                )
                .await
            },
        )
        .await?;
        wait_ready(state, id).await?;
        let server = container(id);
        step(
            state,
            "repository",
            "Creating the repository",
            "The repository exists in the bucket.",
            async {
                must(
                    &exec(&server, &["pgbackrest", &stanza(), "stanza-create"]),
                    "pgBackRest could not create the repository",
                )
                .await
                .map(drop)
            },
        )
        .await?;
        let archiving = async {
            step(
                state,
                "archiving",
                "Turning WAL archiving on",
                "PostgreSQL restarted and sends every WAL segment to the repository.",
                async {
                    let command = format!("pgbackrest {} archive-push %p", stanza());
                    for (setting, value) in [
                        ("archive_mode", "on"),
                        ("archive_command", command.as_str()),
                        ("archive_timeout", "60"),
                    ] {
                        sql(
                            state,
                            id,
                            "postgres",
                            ADMIN,
                            format!("ALTER SYSTEM SET {setting} = '{value}';"),
                        )
                        .await?;
                    }
                    must(
                        &["restart".into(), server.clone()],
                        "Docker could not restart PostgreSQL",
                    )
                    .await?;
                    super::ready(state, id).await
                },
            )
            .await?;
            step(
                state,
                "check",
                "Checking that WAL reaches the repository",
                "A WAL segment written now arrived in the bucket.",
                async {
                    must(
                        &exec(&server, &["pgbackrest", &stanza(), "check"]),
                        "pgBackRest could not archive a WAL segment",
                    )
                    .await
                    .map(drop)
                },
            )
            .await
        }
        .await;
        if let Err(error) = archiving {
            // Archiving with a repository that does not take WAL fills the
            // disk. `archive_mode` takes a restart to turn off.
            if archiving_off(state, id).await.is_ok()
                && must(
                    &["restart".into(), server.clone()],
                    "Docker could not restart PostgreSQL",
                )
                .await
                .is_ok()
            {
                let _ = super::ready(state, id).await;
            }
            return Err(error);
        }
        update_settings(store, id, |settings| {
            settings.enabled = true;
            settings.since = audit::timestamp();
            settings.last_run_at = None;
        })
        .await?;
        Ok(())
    }
    .await;
    let reason = result.as_ref().err().map(ToString::to_string);
    update_settings(store, id, |settings| {
        settings.change = None;
        settings.error = reason;
    })
    .await?;
    result?;
    Box::pin(back_up(state, id, Kind::Full)).await
}

/// The Task that turns backups off: archiving stops and the container goes
/// back to the official image. The repository stays in the bucket.
pub(super) async fn disable<S: StateStore>(state: &AppState<S>, id: &str) -> Result<(), Error> {
    let store = &state.store;
    let result = async {
        let (definition, app) = database(store, id).await?;
        if app.status != "running" {
            return Err(Error::Conflict(
                "Start PostgreSQL before turning its backups off".into(),
            ));
        }
        wait_ready(state, id).await?;
        step(
            state,
            "archiving",
            "Turning WAL archiving off",
            "PostgreSQL no longer sends WAL to the repository.",
            archiving_off(state, id),
        )
        .await?;
        update_settings(store, id, |settings| settings.enabled = false).await?;
        step(
            state,
            "container",
            "Recreating PostgreSQL with the official image",
            "The container runs the official image. The repository stays in the bucket.",
            async {
                let image = super::server_image(state, &definition).await?;
                let compose = template_with(definition.major, &image, None)?;
                apply_compose(state, id, compose, &image).await?;
                let _ = std::fs::remove_dir_all(storage::conf_dir(id));
                Ok(())
            },
        )
        .await?;
        step(
            state,
            "disabled-ready",
            "Waiting for PostgreSQL readiness",
            "Container health is healthy and PostgreSQL accepted a test query.",
            super::ready(state, id),
        )
        .await
    }
    .await;
    let reason = result.as_ref().err().map(ToString::to_string);
    update_settings(store, id, |settings| {
        settings.change = None;
        settings.error = reason;
    })
    .await?;
    result
}

/// The Backup Task: one pgBackRest backup, which also expires what is older
/// than the days to keep, then the repository's list read again.
pub(super) async fn back_up<S: StateStore>(
    state: &AppState<S>,
    id: &str,
    kind: Kind,
) -> Result<(), Error> {
    let store = &state.store;
    let result = async {
        let (_, app) = database(store, id).await?;
        if !settings(store, id).await?.enabled {
            return Err(Error::Conflict("Backups are off for this server".into()));
        }
        if app.status != "running" {
            return Err(Error::Conflict(
                "Start PostgreSQL before backing it up".into(),
            ));
        }
        step(
            state,
            "backup-ready",
            "Waiting for PostgreSQL readiness",
            "Container health is healthy and PostgreSQL accepted a test query.",
            super::ready(state, id),
        )
        .await?;
        step(
            state,
            "backup",
            match kind {
                Kind::Full => "Taking a full backup",
                Kind::Diff => "Taking a differential backup",
                Kind::Incr => "Taking an incremental backup",
            },
            "The backup is in the repository. Backups past the days to keep were expired.",
            async {
                let kind = format!("--type={}", kind.name());
                must(
                    &exec(&container(id), &["pgbackrest", &stanza(), &kind, "backup"]),
                    "pgBackRest could not take the backup",
                )
                .await
                .map(drop)
            },
        )
        .await?;
        step(
            state,
            "list",
            "Reading the repository",
            "The list of backups matches the repository.",
            async { refresh(state, id).await.map(drop) },
        )
        .await
    }
    .await;
    let reason = result.as_ref().err().map(ToString::to_string);
    update_settings(store, id, |settings| {
        settings.running = None;
        settings.error = reason;
    })
    .await?;
    result
}

/// Forgets a removed server's backups. Its repository stays in the bucket.
pub(super) async fn forget(store: &impl StateStore, id: &str) -> Result<(), StoreError> {
    SETTINGS.remove(store, id).await?;
    REPOSITORIES.remove(store, id).await?;
    let _ = std::fs::remove_dir_all(storage::conf_dir(id));
    Ok(())
}

/// A restore request, as the API takes it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Restore {
    /// The new server's name.
    pub name: String,
    /// The time to restore to. Left out, the latest state in the repository.
    #[serde(default)]
    pub time: Option<String>,
}

/// Records a new managed PostgreSQL server and queues the Task that fills it
/// from `id`'s repository. Answers the new server's id and the Task's.
pub(crate) async fn start_restore<S: StateStore>(
    state: &AppState<S>,
    id: &str,
    request: Restore,
) -> Result<(String, String), Error> {
    let store = &state.store;
    let (definition, _) = database(store, id).await?;
    storage::configured(store).await?;
    let backups = repository(store, id).await?.backups;
    let Some(oldest) = backups.last() else {
        return Err(Error::Conflict(
            "This server has no backup to restore yet".into(),
        ));
    };
    if let Some(time) = &request.time {
        let at = parse(time).ok_or_else(|| {
            Error::Invalid("Give the time as RFC 3339, such as 2026-11-12T14:30:00Z".into())
        })?;
        if parse(&oldest.finished_at).is_none_or(|first| at < first) {
            return Err(Error::Invalid(format!(
                "The oldest backup ended at {}. Pick a later time",
                oldest.finished_at
            )));
        }
        if at > OffsetDateTime::now_utc() {
            return Err(Error::Invalid("Pick a time that has passed".into()));
        }
    }
    let _namespace = state.dns_records.lock_namespace().await;
    let _write = super::CONNECTION_WRITE.lock().await;
    let name = request.name.trim();
    if store.application_exists(name).await? {
        return Err(Error::Conflict(
            "An Application already uses that name".into(),
        ));
    }
    let password = store.get_env(id, PASSWORD).await?.ok_or(Error::Missing)?;
    let pending = apps::prepare_deploy_from_compose(
        store,
        name,
        &template(definition.major)?,
        None,
        None,
        apps::DeployOptions {
            publication: Some(crate::store::Publication::Unpublished),
            variable_delivery: Some(crate::store::VariableDelivery::Referenced),
            network_policy: Some(crate::store::NetworkPolicy::private()),
            ..Default::default()
        },
    )
    .await
    .map_err(|e| Error::Invalid(e.to_string()))?;
    let app = &pending.record;
    // The restored cluster keeps the source's administrator password.
    store.set_env(&app.id, PASSWORD, &password).await?;
    DATABASES
        .upsert(
            store,
            &super::Database {
                application_id: app.id.clone(),
                major: definition.major,
                volume: format!("{}_data", apps::project_name_for(&app.id)),
                native_port: None,
                custom_image: definition.custom_image.clone(),
            },
        )
        .await?;
    let task = task_id();
    enqueue(
        state,
        &task,
        "restore",
        audit::Subject::new("database", app.id.clone(), app.name.clone()),
        super::Operation::RestoreBackup {
            source: id.into(),
            id: app.id.clone(),
            time: request.time,
        },
    )
    .await?;
    Ok((app.id.clone(), task))
}

/// The arguments of a one-off container that restores a repository into the
/// volume `volume`. It makes the cluster's directory first: PostgreSQL 18
/// keeps it below the volume's root.
fn restore_container(
    name: &str,
    volume: &str,
    conf: &std::path::Path,
    major: u16,
    image: &str,
    target: &[String],
) -> Vec<String> {
    let mount = if major >= 18 {
        "/var/lib/postgresql"
    } else {
        "/var/lib/postgresql/data"
    };
    let mut args: Vec<String> = [
        "run",
        "--rm",
        "--user",
        "postgres",
        "--name",
        name,
        "--volume",
        &format!("{volume}:{mount}"),
        "--volume",
        &format!("{}:{}:ro", conf.display(), storage::CONF_MOUNT),
        "--entrypoint",
        "bash",
        image,
        "-c",
        r#"mkdir -p "$0" && chmod 700 "$0" && exec pgbackrest "$@""#,
        &storage::pg_path(major),
        &stanza(),
        "--archive-mode=off",
    ]
    .map(str::to_owned)
    .into();
    args.extend(target.iter().cloned());
    args.push("restore".into());
    args
}

/// `--type=time` and its target, as pgBackRest reads a time.
fn time_target(at: OffsetDateTime) -> Vec<String> {
    let at = at.to_offset(UtcOffset::UTC);
    vec![
        "--type=time".into(),
        format!(
            "--target={:04}-{:02}-{:02} {:02}:{:02}:{:02}+00",
            at.year(),
            at.month() as u8,
            at.day(),
            at.hour(),
            at.minute(),
            at.second()
        ),
        "--target-action=promote".into(),
    ]
}

/// Waits until the PostgreSQL in `container` ended recovery and takes
/// queries. A server that stops or restarts on the way failed its recovery.
async fn recovered(container: &str) -> Result<(), Error> {
    let probe = exec(
        container,
        &[
            "psql",
            "--no-psqlrc",
            "--quiet",
            "--tuples-only",
            "--no-align",
            "--username",
            ADMIN,
            "--dbname",
            "postgres",
            "--command",
            "SELECT pg_is_in_recovery();",
        ],
    );
    let inspect: Vec<String> = [
        "inspect",
        "--format",
        "{{.State.Status}} {{.RestartCount}}",
        container,
    ]
    .map(str::to_owned)
    .into();
    let started = std::time::Instant::now();
    loop {
        let answer = run_docker(&probe).await?;
        if answer.status.success() && String::from_utf8_lossy(&answer.stdout).trim() == "f" {
            return Ok(());
        }
        let status = run_docker(&inspect).await?;
        if String::from_utf8_lossy(&status.stdout).trim() != "running 0" {
            let _ = run_docker(&[
                "logs".into(),
                "--tail".into(),
                "30".into(),
                container.into(),
            ])
            .await;
            return Err(Error::Runtime(
                "PostgreSQL stopped while it replayed WAL. A time after the last change in the repository cannot be reached".into(),
            ));
        }
        if started.elapsed() > std::time::Duration::from_secs(12 * 60 * 60) {
            return Err(Error::Runtime(
                "PostgreSQL did not end recovery in 12 hours".into(),
            ));
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

/// The Restore Task: fills the new server `id` from `source`'s repository,
/// starts it to replay WAL up to the target, and once it promoted, detaches
/// it from the repository. The source is not touched, except for one WAL
/// switch so its latest changes are in the repository.
pub(super) async fn restore<S: StateStore>(
    state: &AppState<S>,
    source: &str,
    id: &str,
    time: Option<String>,
) -> Result<(), Error> {
    let store = &state.store;
    let configured = storage::configured(store).await?;
    let (definition, _) = database(store, id).await?;
    let major = definition.major;
    let at = time.as_deref().and_then(parse);
    if let Ok((_, origin)) = database(store, source).await
        && origin.status == "running"
        && settings(store, source).await?.enabled
    {
        step(
            state,
            "archive",
            "Archiving the source's latest changes",
            "The source's current WAL segment is in the repository.",
            async {
                // Recovery to a time stops at the first commit after it, so
                // there must be one.
                sql(
                    state,
                    source,
                    "postgres",
                    ADMIN,
                    "SELECT pg_current_xact_id();".into(),
                )
                .await?;
                must(
                    &exec(&container(source), &["pgbackrest", &stanza(), "check"]),
                    "pgBackRest could not archive the source's WAL",
                )
                .await
                .map(drop)
            },
        )
        .await?;
    }
    let tag = image(state, &definition).await?;
    let dir = storage::conf_dir(id);
    step(
        state,
        "restore",
        "Restoring the backup into a new volume",
        "The backup's files are in the new server's volume.",
        async {
            // The new server reads the source's repository until it promoted.
            storage::write_conf(
                &dir,
                &storage::conf(&configured, source, major, KEEP_DAYS_DEFAULT),
            )?;
            let project = apps::project_name_for(id);
            let _ = run_docker(&["volume".into(), "rm".into(), definition.volume.clone()]).await;
            must(
                &[
                    "volume".into(),
                    "create".into(),
                    "--label".into(),
                    format!("com.docker.compose.project={project}"),
                    "--label".into(),
                    "com.docker.compose.volume=data".into(),
                    definition.volume.clone(),
                ],
                "Docker could not create the new server's volume",
            )
            .await?;
            must(
                &restore_container(
                    &format!("sf-backup-restore-{:016x}", rand::random::<u64>()),
                    &definition.volume,
                    &dir,
                    major,
                    &tag,
                    &at.map(time_target).unwrap_or_default(),
                ),
                "pgBackRest could not restore the backup",
            )
            .await
            .map(drop)
        },
    )
    .await?;
    step(
        state,
        "container",
        "Starting the new PostgreSQL server",
        "The container started on the restored volume.",
        apply_compose(state, id, template_with(major, &tag, Some(&dir))?, &tag),
    )
    .await?;
    step(
        state,
        "recovery",
        "Replaying WAL up to the target",
        "PostgreSQL reached the target and takes queries.",
        recovered(&container(id)),
    )
    .await?;
    step(
        state,
        "detach",
        "Detaching the new server from the repository",
        "The new server runs the official image. Its backups are off.",
        async {
            for setting in ARCHIVE_SETTINGS.into_iter().chain(["restore_command"]) {
                sql(
                    state,
                    id,
                    "postgres",
                    ADMIN,
                    format!("ALTER SYSTEM RESET {setting};"),
                )
                .await?;
            }
            let image = super::server_image(state, &definition).await?;
            apply_compose(state, id, template_with(major, &image, None)?, &image).await?;
            let _ = std::fs::remove_dir_all(&dir);
            Ok(())
        },
    )
    .await?;
    wait_ready(state, id).await?;
    step(
        state,
        "records",
        "Copying the database records",
        "The new server lists the databases it holds, with the source's credentials.",
        copy_records(state, source, id),
    )
    .await
}

/// Gives the new server a record of every consumer database it holds, with
/// the role and password the source has for it. Connections are not copied:
/// no Application uses the new server. A database connections shared keeps
/// its NOLOGIN owner, so the new server still counts it as the Platform's,
/// and so does one whose connections were all disconnected.
async fn copy_records<S: StateStore>(
    state: &AppState<S>,
    source: &str,
    id: &str,
) -> Result<(), Error> {
    let store = &state.store;
    let held = sql(
        state,
        id,
        "postgres",
        ADMIN,
        "SELECT datname FROM pg_database WHERE NOT datistemplate;".into(),
    )
    .await?;
    let held: std::collections::BTreeSet<&str> = held.lines().map(str::trim).collect();
    let mut records: Vec<StandaloneDatabase> = STANDALONE
        .list(store)
        .await?
        .into_iter()
        .filter(|database| database.database_application_id == source && database.made)
        .collect();
    // A disconnected connection's database stays on the server, owned as
    // before. Its record counts only when no live connection names it.
    let mut connections: Vec<_> = CONNECTIONS
        .list(store)
        .await?
        .into_iter()
        .filter(|connection| connection.database_application_id == source)
        .collect();
    connections
        .sort_by_key(|connection| matches!(connection.status.as_str(), "revoking" | "revoked"));
    for connection in connections {
        let owner = connection.owner.filter(|owner| !owner.is_empty());
        if let Some(record) = records.iter_mut().find(|d| d.name == connection.database) {
            if record.owner.is_none() {
                record.owner = owner;
            }
            continue;
        }
        records.push(StandaloneDatabase {
            database_application_id: source.into(),
            name: connection.database,
            role: connection.role,
            password: connection.password,
            made: true,
            owner,
        });
    }
    let mut copied = 0;
    for mut record in records {
        if held.contains(record.name.as_str()) {
            record.database_application_id = id.into();
            STANDALONE.upsert(store, &record).await?;
            copied += 1;
        }
    }
    crate::command_log::note(&format!("{copied} database records copied"));
    Ok(())
}

/// Queues a test restore of `id`'s latest backup.
pub(crate) async fn start_verify<S: StateStore>(
    state: &AppState<S>,
    id: &str,
) -> Result<String, Error> {
    let store = &state.store;
    let (_, app) = database(store, id).await?;
    storage::configured(store).await?;
    let current = repository(store, id).await?;
    let Some(latest) = current.backups.first() else {
        return Err(Error::Conflict(
            "This server has no backup to verify yet".into(),
        ));
    };
    if let Some(verification) = &current.verification
        && verification.status == "running"
        && TASKS.exists(store, &verification.task_id).await?
    {
        return Err(Error::Conflict(
            "The latest backup is already being verified".into(),
        ));
    }
    let task = task_id();
    let previous = current.verification.clone();
    let running = Verification {
        status: "running".into(),
        task_id: task.clone(),
        at: audit::timestamp(),
        label: Some(latest.label.clone()),
        error: None,
    };
    update_repository(store, id, |repository| {
        repository.verification = Some(running);
    })
    .await?;
    let queued = enqueue(
        state,
        &task,
        "verify",
        audit::Subject::new("database", app.id.clone(), app.name),
        super::Operation::VerifyBackup { id: app.id },
    )
    .await;
    if let Err(error) = queued {
        let _ = update_repository(store, id, |repository| repository.verification = previous).await;
        return Err(error);
    }
    Ok(task)
}

/// The Verify Task: restores the latest backup into a throwaway volume,
/// starts a throwaway PostgreSQL on it with no published port, and queries
/// every database. Both are removed at the end.
pub(super) async fn verify<S: StateStore>(state: &AppState<S>, id: &str) -> Result<(), Error> {
    let store = &state.store;
    let name = format!("sf-backup-verify-{:016x}", rand::random::<u64>());
    let dir = storage::conf_dir(&name);
    let result = Box::pin(verify_in(state, id, &name, &dir)).await;
    let _ = run_docker(&["rm".into(), "--force".into(), name.clone()]).await;
    let _ = run_docker(&["volume".into(), "rm".into(), "--force".into(), name]).await;
    let _ = std::fs::remove_dir_all(&dir);
    let reason = result.as_ref().err().map(ToString::to_string);
    let reason = match reason {
        Some(reason) => Some(super::redact(
            &reason,
            &super::secrets(store).await.unwrap_or_default(),
        )),
        None => None,
    };
    update_repository(store, id, |repository| {
        if let Some(verification) = &mut repository.verification {
            verification.status = if reason.is_some() { "failed" } else { "passed" }.into();
            verification.at = audit::timestamp();
            verification.error = reason;
        }
    })
    .await?;
    result
}

async fn verify_in<S: StateStore>(
    state: &AppState<S>,
    id: &str,
    name: &str,
    dir: &std::path::Path,
) -> Result<(), Error> {
    let store = &state.store;
    let configured = storage::configured(store).await?;
    let (definition, _) = database(store, id).await?;
    let major = definition.major;
    let tag = image(state, &definition).await?;
    step(
        state,
        "restore",
        "Restoring the latest backup into a throwaway volume",
        "The backup's files are in a volume of their own.",
        async {
            storage::write_conf(
                dir,
                &storage::conf(&configured, id, major, KEEP_DAYS_DEFAULT),
            )?;
            must(
                &["volume".into(), "create".into(), name.into()],
                "Docker could not create the throwaway volume",
            )
            .await?;
            must(
                &restore_container(
                    &format!("{name}-restore"),
                    name,
                    dir,
                    major,
                    &tag,
                    &["--type=immediate".into(), "--target-action=promote".into()],
                ),
                "pgBackRest could not restore the backup",
            )
            .await
            .map(drop)
        },
    )
    .await?;
    step(
        state,
        "start",
        "Starting a throwaway PostgreSQL",
        "PostgreSQL replayed the backup's WAL and takes queries. It has no published port.",
        async {
            let mount = if major >= 18 {
                "/var/lib/postgresql"
            } else {
                "/var/lib/postgresql/data"
            };
            must(
                &[
                    "run".into(),
                    "--detach".into(),
                    "--name".into(),
                    name.into(),
                    "--volume".into(),
                    format!("{name}:{mount}"),
                    "--volume".into(),
                    format!("{}:{}:ro", dir.display(), storage::CONF_MOUNT),
                    tag.clone(),
                ],
                "Docker did not start the throwaway PostgreSQL",
            )
            .await?;
            recovered(name).await
        },
    )
    .await?;
    step(
        state,
        "query",
        "Querying every database",
        "Every database of the backup answered a query.",
        async {
            let query = |database: &str, sql: &str| {
                exec(
                    name,
                    &[
                        "psql",
                        "--no-psqlrc",
                        "--quiet",
                        "--tuples-only",
                        "--no-align",
                        "--username",
                        ADMIN,
                        "--dbname",
                        database,
                        "--command",
                        sql,
                    ],
                )
            };
            let listed = must(
                &query(
                    "postgres",
                    "SELECT datname FROM pg_database WHERE datallowconn;",
                ),
                "The restored PostgreSQL did not list its databases",
            )
            .await?;
            let databases: Vec<&str> = listed
                .lines()
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .collect();
            for database in &databases {
                must(
                    &query(database, "SELECT count(*) FROM pg_class;"),
                    &format!("The restored database {database} did not answer a query"),
                )
                .await?;
            }
            crate::command_log::note(&format!("{} databases answered", databases.len()));
            Ok(())
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> OffsetDateTime {
        OffsetDateTime::parse(text, &Rfc3339).unwrap()
    }

    fn on(since: &str, last: Option<&str>) -> BackupSettings {
        BackupSettings {
            enabled: true,
            since: since.into(),
            last_run_at: last.map(str::to_owned),
            ..default_settings("db")
        }
    }

    const WINDOW: MaintenanceWindow = MaintenanceWindow {
        start_hour: 2,
        hours: 4,
    };

    #[test]
    fn a_backup_is_due_only_inside_the_window_in_local_time() {
        let settings = on("2026-11-01T00:00:00Z", None);
        // 02:00 at -03:00 is 05:00 UTC.
        assert!(due(&settings, WINDOW, at("2026-11-10T01:59:00-03:00")).is_none());
        assert!(due(&settings, WINDOW, at("2026-11-10T02:00:00-03:00")).is_some());
        assert!(due(&settings, WINDOW, at("2026-11-10T05:59:00-03:00")).is_some());
        assert!(due(&settings, WINDOW, at("2026-11-10T06:00:00-03:00")).is_none());
        let off = BackupSettings {
            enabled: false,
            ..settings
        };
        assert!(due(&off, WINDOW, at("2026-11-10T02:00:00-03:00")).is_none());
    }

    #[test]
    fn a_window_runs_a_backup_once_and_a_missed_one_waits_for_the_next() {
        let ran = on("2026-11-01T00:00:00Z", Some("2026-11-10T05:01:00Z"));
        assert!(due(&ran, WINDOW, at("2026-11-10T03:00:00-03:00")).is_none());
        assert!(due(&ran, WINDOW, at("2026-11-11T02:00:00-03:00")).is_some());
        // The Host was off during the window of the 11th: nothing at 09:00.
        assert!(due(&ran, WINDOW, at("2026-11-11T09:00:00-03:00")).is_none());
    }

    #[test]
    fn turning_backups_on_inside_a_window_waits_for_the_next_one() {
        let settings = on("2026-11-10T05:30:00Z", None);
        assert!(due(&settings, WINDOW, at("2026-11-10T03:00:00-03:00")).is_none());
        assert!(due(&settings, WINDOW, at("2026-11-11T02:00:00-03:00")).is_some());
    }

    #[test]
    fn the_week_has_one_full_two_differentials_and_four_incrementals() {
        let settings = on("2026-11-01T00:00:00Z", None);
        // 2026-11-08 is a Sunday.
        let kinds: Vec<Kind> = (8..15)
            .map(|day| {
                due(
                    &settings,
                    WINDOW,
                    at(&format!("2026-11-{day:02}T02:30:00-03:00")),
                )
                .unwrap()
            })
            .collect();
        assert_eq!(
            kinds,
            [
                Kind::Full,
                Kind::Incr,
                Kind::Incr,
                Kind::Diff,
                Kind::Incr,
                Kind::Incr,
                Kind::Diff
            ]
        );
        assert_eq!(
            next_run(&settings, WINDOW, at("2026-11-13T09:00:00-03:00")),
            Some(("2026-11-14T05:00:00Z".into(), Kind::Diff))
        );
    }

    #[test]
    fn a_window_that_crosses_midnight_stays_open_after_it() {
        let late = MaintenanceWindow {
            start_hour: 23,
            hours: 3,
        };
        let settings = on("2026-11-01T00:00:00Z", None);
        assert!(due(&settings, late, at("2026-11-10T01:30:00-03:00")).is_some());
        assert!(due(&settings, late, at("2026-11-10T02:00:00-03:00")).is_none());
    }

    #[test]
    fn the_repository_list_is_read_newest_first() {
        let json = r#"[{"name":"main","backup":[
            {"label":"20261108-050001F","type":"full","timestamp":{"start":1794114001,"stop":1794114046},"info":{"size":23000000,"delta":23000000,"repository":{"delta":2900000}}},
            {"label":"20261108-050001F_20261109-050002I","type":"incr","timestamp":{"start":1794200402,"stop":1794200410},"info":{"size":23000000,"delta":8000,"repository":{"delta":900}}}
        ]}]"#;
        let backups = listed_backups(json).unwrap();
        assert_eq!(backups.len(), 2);
        assert_eq!(backups[0].kind, Kind::Incr);
        assert_eq!(backups[0].repository_bytes, 900);
        assert_eq!(backups[1].label, "20261108-050001F");
        assert_eq!(backups[1].bytes, 23_000_000);
        assert!(backups[1].finished_at.ends_with('Z'));
        assert_eq!(
            listed_backups(r#"[{"name":"main","backup":[]}]"#),
            Some(vec![])
        );
        assert!(listed_backups("no json").is_none());
    }

    #[test]
    fn the_archiver_fails_when_its_last_attempt_did() {
        assert_eq!(
            archiving("2026-11-12T10:00:00Z||f\n"),
            Some(Archiving {
                last_archived_at: Some("2026-11-12T10:00:00Z".into()),
                last_failed_at: None,
                failing: false,
            })
        );
        assert!(archiving("|2026-11-12T10:00:00Z|t").unwrap().failing);
        assert!(archiving("").is_none());
    }

    #[test]
    fn a_restore_to_a_time_names_it_in_utc_and_promotes() {
        assert_eq!(
            time_target(at("2026-11-12T11:30:05-03:00")),
            [
                "--type=time",
                "--target=2026-11-12 14:30:05+00",
                "--target-action=promote"
            ]
        );
        let args = restore_container(
            "one-off",
            "vol",
            std::path::Path::new("/conf"),
            18,
            "image:tag",
            &[],
        );
        assert!(args.contains(&"vol:/var/lib/postgresql".to_owned()));
        assert!(args.contains(&"/var/lib/postgresql/18/docker".to_owned()));
        assert_eq!(args.last().unwrap(), "restore");
    }
}
