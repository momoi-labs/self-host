//! Opt-in credentials that can deploy one Application's configured Git branch.
//! Tokens are returned once, stored as hashes, and never placed in Task payloads.

use crate::{
    AppState, apps, audit,
    collection::{Collection, Keyed},
    store::{Runtime, StateStore},
    tasks,
};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const TRIGGERS: Collection<Trigger> = Collection::new("deploy-trigger");
pub(crate) const DELIVERIES: Collection<Delivery> = Collection::new("deploy-delivery");

#[derive(Clone, Serialize, Deserialize)]
struct Trigger {
    application_id: String,
    generation: String,
    token_hash: String,
    source_hash: String,
    branch: String,
    created_at: String,
}
impl Keyed for Trigger {
    fn key(&self) -> String {
        self.application_id.clone()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Delivery {
    pub id: String,
    pub fingerprint: String,
    pub task_id: String,
}
impl Keyed for Delivery {
    fn key(&self) -> String {
        self.id.clone()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Enable {
    branch: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    delivery_id: String,
    branch: String,
    #[serde(default)]
    revision: Option<String>,
}

fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn branch(value: &str) -> &str {
    value.strip_prefix("refs/heads/").unwrap_or(value)
}
fn invalid(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(crate::error::ErrorReport::plain(message)),
    )
        .into_response()
}
fn internal(error: &crate::store::StoreError) -> Response {
    crate::error_response(StatusCode::INTERNAL_SERVER_ERROR, error)
}

pub(crate) async fn status<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
) -> Response {
    if let Err(error) = apps::get_application(&state.store, &id).await {
        return crate::deploy_error_response(error);
    }
    match TRIGGERS.get(&state.store, &id).await {
        Ok(Some(trigger)) => Json(serde_json::json!({"enabled":true,"branch":trigger.branch,"created_at":trigger.created_at})).into_response(),
        Ok(None) => Json(serde_json::json!({"enabled":false})).into_response(),
        Err(error) => internal(&error),
    }
}

pub(crate) async fn enable<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
    Json(request): Json<Enable>,
) -> Response {
    let app = match apps::get_application(&state.store, &id).await {
        Ok(app) => app,
        Err(error) => return crate::deploy_error_response(error),
    };
    let Some(source) = app
        .git
        .as_ref()
        .filter(|_| app.runtime == Runtime::Container)
    else {
        return invalid("deploy triggers require a Git container Application");
    };
    if request.branch.is_empty()
        || request.branch == "HEAD"
        || request.branch.starts_with("refs/tags/")
        || branch(&request.branch) != branch(&source.git_ref)
        || source.revision.is_some()
    {
        return invalid("choose the configured Git branch without an explicit commit pin");
    }
    let token = format!(
        "deploy_{:032x}{:032x}",
        rand::random::<u128>(),
        rand::random::<u128>()
    );
    let trigger = Trigger {
        application_id: id,
        generation: format!("{:032x}", rand::random::<u128>()),
        token_hash: hash(&token),
        source_hash: hash(&serde_json::to_string(source).expect("source serializes")),
        branch: branch(&request.branch).into(),
        created_at: audit::timestamp(),
    };
    match TRIGGERS.upsert(&state.store, &trigger).await {
        Ok(()) => (
            [(header::CACHE_CONTROL, "no-store")],
            Json(serde_json::json!({"enabled":true,"branch":trigger.branch,"token":token})),
        )
            .into_response(),
        Err(error) => internal(&error),
    }
}

pub(crate) async fn disable<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
) -> Response {
    match TRIGGERS.remove(&state.store, &id).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => internal(&error),
    }
}

pub(crate) async fn deliver<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> Response {
    let trigger = match TRIGGERS.get(&state.store, &id).await {
        Ok(Some(trigger)) => trigger,
        Ok(None) => return StatusCode::UNAUTHORIZED.into_response(),
        Err(error) => return internal(&error),
    };
    let supplied = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));
    if supplied.is_none_or(|token| !crate::constant_time_eq(&hash(token), &trigger.token_hash)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if request.delivery_id.is_empty()
        || request.delivery_id.len() > 128
        || !request
            .delivery_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.:".contains(&c))
    {
        return invalid(
            "delivery_id must contain 1 to 128 ASCII letters, digits, dots, colons, underscores or hyphens",
        );
    }
    // Authenticate even ignored events. Nothing is queued for another branch.
    if branch(&request.branch) != trigger.branch {
        return Json(serde_json::json!({"ignored":true,"reason":"branch does not match"}))
            .into_response();
    }
    let app = match apps::get_application(&state.store, &id).await {
        Ok(app) => app,
        Err(error) => return crate::deploy_error_response(error),
    };
    let Some(source) = app.git.clone().filter(|source| {
        branch(&source.git_ref) == trigger.branch
            && source.revision.is_none()
            && hash(&serde_json::to_string(source).expect("source serializes"))
                == trigger.source_hash
    }) else {
        return invalid("the configured Git source changed; enable a new trigger");
    };
    if app.runtime != Runtime::Container {
        return invalid("deploy triggers require a Git container Application");
    }
    let mut check = source.clone();
    check.revision = request.revision.clone();
    if let Err(error) = crate::source::validate(&check) {
        return crate::source_error_response(error);
    }
    let delivery = Delivery {
        id: hash(&format!(
            "{}:{}:{}",
            id, trigger.generation, request.delivery_id
        )),
        fingerprint: hash(&serde_json::to_string(&request).expect("trigger request serializes")),
        task_id: String::new(),
    };
    let work = tasks::Work::TriggerGitApplication {
        id: id.clone(),
        source: Box::new(source),
        revision: request.revision,
    };
    let outcome = audit::ACTOR
        .scope(
            Some("Application deploy trigger".into()),
            tasks::enqueue_once(
                &state,
                audit::Subject::new("application", &id, &app.name),
                work,
                delivery,
            ),
        )
        .await;
    match outcome {
        Ok((task_id, duplicate)) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({"task_id":task_id,"duplicate":duplicate})),
        )
            .into_response(),
        Err(crate::store::StoreError::AlreadyExists(_)) => (
            StatusCode::CONFLICT,
            Json(crate::error::ErrorReport::plain(
                "delivery_id was already used with a different payload",
            )),
        )
            .into_response(),
        Err(error) => internal(&error),
    }
}

pub(crate) async fn execute(
    store: &impl StateStore,
    docker: &(impl crate::docker::DockerRuntime + ?Sized),
    routes: &(impl crate::routes::RouteStore + ?Sized),
    id: &str,
    expected_source: &crate::source::GitSource,
    revision: Option<String>,
) -> Result<Vec<audit::Change>, apps::DeployError> {
    let current = apps::get_application(store, id).await?;
    let source = current
        .git
        .clone()
        .filter(|source| source == expected_source && source.revision.is_none())
        .ok_or_else(|| {
            apps::DeployError::Readiness(
                "the configured Git branch changed before this delivery ran".into(),
            )
        })?;
    let mut branch_source = source.clone();
    branch_source.git_ref = format!("refs/heads/{}", branch(&source.git_ref));
    let inspected =
        crate::source::inspection::inspect(&crate::source::private_root(), &branch_source).await?;
    if revision
        .as_deref()
        .is_some_and(|requested| !requested.eq_ignore_ascii_case(&inspected.revision))
    {
        return Err(apps::DeployError::Readiness(
            "the delivered commit is no longer the configured branch head; send a new delivery"
                .into(),
        ));
    }
    let mut pending = apps::prepare_git_update(
        store,
        current,
        source,
        true,
        apps::ApplicationUpdate::default(),
    )
    .await?;
    // A trigger is an explicit opt-in to accept commits from this branch. The
    // reviewed-update compare-and-swap still applies to the manual endpoint.
    pending.source_revision = Some(inspected.revision);
    apps::finish_git_deploy(store, docker, routes, pending)
        .await
        .map(|(_, changes)| changes)
}
