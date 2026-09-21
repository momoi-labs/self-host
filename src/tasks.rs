//! One scheduler for every action that changes an Application, a Virtual
//! machine or a custom image.
//!
//! A handler validates the request, writes what the record needs, and hands
//! the rest to a task. The task is persisted before the handler answers
//! `202 Accepted`, and its id is the id of the audit event that records it:
//! the console follows one id from `pending` to `running` to `completed` or
//! `failed`.
//!
//! Tasks on the same object run one at a time, oldest first. Objects do not
//! wait on each other. A restart requeues what never started and fails what
//! was running, because a half-done Docker or Lima operation must not be run
//! twice. Nothing is retried on its own.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::{
    AppState, apps, audit,
    audit::Subject,
    collection::{Keyed, TASKS},
    custom_images, environments,
    error::ErrorReport,
    store::{StateStore, StoreError},
};

const INTERRUPTED: &str = "The Platform restarted before this task finished.";

/// What a task does, with everything it needs to do it after a restart.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Work {
    DeployApplication {
        pending: apps::PendingDeploy,
    },
    StartApplication {
        id: String,
    },
    StopApplication {
        id: String,
    },
    RestartApplication {
        id: String,
    },
    RemoveApplication {
        id: String,
    },
    SetEnvironment {
        name: String,
        key: String,
        value: String,
    },
    UnsetEnvironment {
        name: String,
        key: String,
    },
    OperateVirtualMachine {
        id: String,
        action: String,
    },
    BuildCustomImage {
        image: custom_images::Image,
    },
    RemoveCustomImage {
        id: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    /// One of the six console actions, as the audit history names them.
    pub action: String,
    pub subject: Subject,
    /// The custom API key that asked, or `None` for the Operator's own key.
    pub api_name: Option<String>,
    /// `pending` until a worker picks it up, `running` from then on. A task
    /// that finished is no longer on file; its event carries the outcome.
    pub status: String,
    pub work: Work,
}

/// Which object a task acts on. Tasks with the same key are serialized.
type Key = (String, String);

#[derive(Default)]
struct Queues {
    waiting: HashMap<Key, VecDeque<Task>>,
    /// Keys a worker is draining right now.
    busy: HashSet<Key>,
}

#[derive(Default)]
pub struct Scheduler {
    queues: Mutex<Queues>,
    write: tokio::sync::Mutex<()>,
    /// Set once what a restart left behind has been settled or requeued. A
    /// new task waits for it, so the queue on file is never read twice.
    recovered: tokio::sync::OnceCell<()>,
}

/// The `202` body of an action that has nothing else to answer with.
#[derive(Serialize)]
pub(crate) struct Accepted {
    pub task_id: String,
}

/// The worker panicked or was cancelled. ADR-0010: the cause stays a cause.
#[derive(Debug)]
struct Stopped(tokio::task::JoinError);
impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "The task stopped before it finished")
    }
}
impl std::error::Error for Stopped {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

#[derive(Debug)]
struct CouldNotStart(StoreError);
impl std::fmt::Display for CouldNotStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "The task could not be recorded as running, so it did not start"
        )
    }
}
impl std::error::Error for CouldNotStart {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

fn key(subject: &Subject) -> Key {
    (subject.kind.clone(), subject.id.clone())
}

fn event(task: &Task, status: &str, description: impl Into<String>) -> audit::Event {
    let mut event = audit::event(
        task.action.clone(),
        task.subject.clone(),
        status,
        task.api_name.clone(),
        description,
    );
    event.id = task.id.clone();
    event
}

/// Records the task and queues it. Answers with the task id the caller puts
/// in its `202` body; the audit middleware reads it back to know the task,
/// and not the request, owns the event from here.
pub(crate) async fn enqueue<S: StateStore>(
    state: &AppState<S>,
    action: &str,
    subject: Subject,
    work: Work,
) -> Result<String, StoreError> {
    recover(state).await;
    let task = Task {
        id: audit::event_id().unwrap_or_else(|| format!("event-{:032x}", rand::random::<u128>())),
        action: action.into(),
        subject,
        api_name: audit::actor(),
        status: "pending".into(),
        work,
    };
    let retention = crate::settings::audit_events_max_age(state).await;
    state
        .audit
        .upsert(
            &state.store,
            event(&task, "pending", "Operation queued."),
            retention,
        )
        .await?;
    persist(state, |tasks| tasks.push(task.clone())).await?;
    schedule(state, task.clone());
    Ok(task.id)
}

fn schedule<S: StateStore>(state: &AppState<S>, task: Task) {
    let key = key(&task.subject);
    let mut queues = state.tasks.queues.lock().unwrap();
    queues
        .waiting
        .entry(key.clone())
        .or_default()
        .push_back(task);
    if queues.busy.insert(key.clone()) {
        tokio::spawn(drain(state.clone(), key));
    }
}

/// Runs every task queued for one object, oldest first, then lets the key go.
async fn drain<S: StateStore>(state: AppState<S>, key: Key) {
    loop {
        let next = {
            let mut queues = state.tasks.queues.lock().unwrap();
            let next = queues.waiting.get_mut(&key).and_then(VecDeque::pop_front);
            if next.is_none() {
                queues.waiting.remove(&key);
                queues.busy.remove(&key);
            }
            next
        };
        let Some(task) = next else { return };
        run(&state, task).await;
    }
}

async fn run<S: StateStore>(state: &AppState<S>, task: Task) {
    // A task that starts without saying so on file would be requeued by the
    // next restart, and its side effects run twice. Better not to start.
    if let Err(error) = persist(state, |tasks| {
        if let Some(stored) = tasks.iter_mut().find(|stored| stored.id == task.id) {
            stored.status = "running".into();
        }
    })
    .await
    {
        tracing::error!(%error, task = %task.id, "Could not mark the task running");
        let mut failed = event(&task, "failed", "Action failed.");
        failed.error = Some(ErrorReport::new(&CouldNotStart(error)));
        audit::record(state, failed).await;
        return;
    }
    audit::record(state, event(&task, "running", "Operation is running.")).await;
    let scoped = audit::EVENT_ID.scope(
        Some(task.id.clone()),
        audit::ACTOR.scope(
            task.api_name.clone(),
            execute(state.clone(), task.work.clone()),
        ),
    );
    let result = match tokio::spawn(scoped).await {
        Ok(result) => result,
        Err(error) => Err(ErrorReport::new(&Stopped(error))),
    };
    let mut outcome = match &result {
        Ok(()) => event(&task, "completed", "Action completed."),
        Err(_) => event(&task, "failed", "Action failed."),
    };
    if let Err(error) = result {
        tracing::warn!(task = %task.id, action = %task.action, subject = %task.subject.name, "{error}");
        outcome.error = Some(error);
    }
    audit::record(state, outcome).await;
    if let Err(error) = persist(state, |tasks| tasks.retain(|stored| stored.id != task.id)).await {
        tracing::error!(%error, task = %task.id, "Could not retire the task");
    }
}

async fn execute<S: StateStore>(state: AppState<S>, work: Work) -> Result<(), ErrorReport> {
    let store = &state.store;
    let docker = state.docker.as_ref();
    let routes = state.routes.as_ref();
    match work {
        Work::DeployApplication { pending } => apps::finish_deploy(store, docker, routes, pending)
            .await
            .map(drop)
            .map_err(|e| ErrorReport::new(&e)),
        Work::StartApplication { id } => apps::start_application(store, docker, routes, &id)
            .await
            .map(drop)
            .map_err(|e| ErrorReport::new(&e)),
        Work::StopApplication { id } => apps::stop_application(store, docker, routes, &id)
            .await
            .map(drop)
            .map_err(|e| ErrorReport::new(&e)),
        Work::RestartApplication { id } => apps::restart_application(store, docker, routes, &id)
            .await
            .map(drop)
            .map_err(|e| ErrorReport::new(&e)),
        // By id, then by the name it has now: a rename in between is not a
        // reason to leave the Application behind.
        Work::RemoveApplication { id } => match apps::get_application(store, &id).await {
            Ok(app) => apps::remove_application(store, docker, routes, &app.name)
                .await
                .map_err(|e| ErrorReport::new(&e)),
            Err(e) => Err(ErrorReport::new(&e)),
        },
        Work::SetEnvironment { name, key, value } => {
            apps::set_env(store, docker, &name, &key, &value)
                .await
                .map_err(|e| ErrorReport::new(&e))
        }
        Work::UnsetEnvironment { name, key } => apps::unset_env(store, docker, &name, &key)
            .await
            .map_err(|e| ErrorReport::new(&e)),
        Work::OperateVirtualMachine { id, action } => {
            environments::run_operation(&state, &id, &action).await
        }
        Work::BuildCustomImage { image } => custom_images::run_build(&state, image).await,
        Work::RemoveCustomImage { id } => custom_images::run_remove(&state, &id).await,
    }
}

async fn load<S: StateStore>(store: &S) -> Result<Vec<Task>, StoreError> {
    TASKS.list(store).await
}

impl Keyed for Task {
    fn key(&self) -> String {
        self.id.clone()
    }
}

async fn persist<S: StateStore>(
    state: &AppState<S>,
    change: impl FnOnce(&mut Vec<Task>),
) -> Result<(), StoreError> {
    let _guard = state.tasks.write.lock().await;
    let mut tasks = load(&state.store).await?;
    change(&mut tasks);
    TASKS.replace_all(&state.store, &tasks).await
}

/// Applications with a deploy still waiting in the queue. `reconcile` leaves
/// their `pending` row alone: the scheduler carries the deploy out.
pub async fn queued_application_ids<S: StateStore>(store: &S) -> Result<Vec<String>, StoreError> {
    Ok(load(store)
        .await?
        .into_iter()
        .filter(|task| {
            task.status == "pending" && matches!(task.work, Work::DeployApplication { .. })
        })
        .map(|task| task.subject.id)
        .collect())
}

/// Settles what a restart interrupted, then queues again what never started.
///
/// A `running` task was mid-way through a side effect nobody can resume; it
/// fails with the reason and its object's own recovery says what is left. A
/// `pending` task did nothing yet and goes back on its queue, in the order it
/// arrived. A `running` event with no task behind it is one the daemon wrote
/// before it had a queue, and it fails the same way.
pub(crate) async fn recover<S: StateStore>(state: &AppState<S>) {
    state.tasks.recovered.get_or_init(|| requeue(state)).await;
}

async fn requeue<S: StateStore>(state: &AppState<S>) {
    let tasks = match load(&state.store).await {
        Ok(tasks) => tasks,
        Err(error) => {
            tracing::error!(%error, "Could not read the task queue; nothing is requeued");
            return;
        }
    };
    let (running, pending): (Vec<Task>, Vec<Task>) =
        tasks.into_iter().partition(|task| task.status == "running");
    for task in &running {
        let mut failed = event(task, "failed", "Action failed.");
        failed.error = Some(ErrorReport::plain(INTERRUPTED));
        audit::record(state, failed).await;
    }
    if let Ok(events) = audit::read(&state.store).await {
        for orphan in events.into_iter().filter(|event| {
            event.status == "running" && !pending.iter().any(|task| task.id == event.id)
        }) {
            let mut failed = orphan.clone();
            failed.status = "failed".into();
            failed.description = "Action failed.".into();
            failed.error = Some(ErrorReport::plain(INTERRUPTED));
            let at = Some(audit::timestamp());
            failed.finished_at = at.clone();
            failed.updated_at = at;
            audit::record(state, failed).await;
        }
    }
    if !running.is_empty()
        && let Err(error) =
            persist(state, |tasks| tasks.retain(|task| task.status != "running")).await
    {
        tracing::error!(%error, "Could not retire the interrupted tasks");
    }
    for task in pending {
        tracing::info!(task = %task.id, action = %task.action, subject = %task.subject.name, "requeued after a restart");
        schedule(state, task);
    }
}
