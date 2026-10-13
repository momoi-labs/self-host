//! Opt-in credentials that can deploy one Application's configured Git branch,
//! or redeploy an image Application from its registry repository.
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
    /// The Git branch a delivery names, or empty for an image trigger.
    branch: String,
    /// The registry repository an image trigger binds to, without a tag or
    /// digest: CI may deliver any tag of it. Absent for a Git trigger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    image: Option<String>,
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

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct Enable {
    /// Required for a Git Application, refused for an image one.
    #[serde(default)]
    branch: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    delivery_id: String,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    revision: Option<String>,
    /// For an image trigger: the reference to deploy, in the trigger's
    /// repository. Absent, the configured reference is pulled again.
    #[serde(default)]
    image: Option<String>,
}

fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn branch(value: &str) -> &str {
    value.strip_prefix("refs/heads/").unwrap_or(value)
}
/// An image reference without its tag or digest: `ghcr.io/org/app:v4` and
/// `ghcr.io/org/app@sha256:...` are both `ghcr.io/org/app`. A colon before
/// the last slash is a registry port, not a tag.
pub(crate) fn repository(reference: &str) -> &str {
    let reference = reference.split('@').next().unwrap_or(reference);
    let name_starts = reference.rfind('/').map_or(0, |slash| slash + 1);
    match reference[name_starts..].find(':') {
        Some(colon) => &reference[..name_starts + colon],
        None => reference,
    }
}
/// Whether the Application redeploys from a registry reference alone: an
/// image Application, in a container, with nothing else defining it.
fn is_image_application(app: &crate::store::ApplicationRecord) -> bool {
    app.source == apps::SOURCE_IMAGE
        && app.runtime == Runtime::Container
        && app.git.is_none()
        && app.compose.is_none()
        && app.development.is_none()
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
        Ok(Some(trigger)) => Json(describe(&trigger)).into_response(),
        Ok(None) => Json(serde_json::json!({"enabled":false})).into_response(),
        Err(error) => internal(&error),
    }
}

/// What a trigger binds to, never its token.
fn describe(trigger: &Trigger) -> serde_json::Value {
    match &trigger.image {
        Some(image) => {
            serde_json::json!({"enabled":true,"image":image,"created_at":trigger.created_at})
        }
        None => {
            serde_json::json!({"enabled":true,"branch":trigger.branch,"created_at":trigger.created_at})
        }
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
    let (source_hash, branch, image) = if let Some(source) = app
        .git
        .as_ref()
        .filter(|_| app.runtime == Runtime::Container)
    {
        let Some(requested) = request
            .branch
            .as_deref()
            .filter(|branch| !branch.is_empty())
        else {
            return invalid("choose the configured Git branch without an explicit commit pin");
        };
        if requested == "HEAD"
            || requested.starts_with("refs/tags/")
            || branch(requested) != branch(&source.git_ref)
            || source.revision.is_some()
        {
            return invalid("choose the configured Git branch without an explicit commit pin");
        }
        (
            hash(&serde_json::to_string(source).expect("source serializes")),
            branch(requested).to_string(),
            None,
        )
    } else if is_image_application(&app) {
        if request.branch.is_some() {
            return invalid(
                "an image Application's trigger follows its registry repository, not a branch",
            );
        }
        let repository = repository(&app.image).to_string();
        (hash(&repository), String::new(), Some(repository))
    } else {
        return invalid("deploy triggers require a Git or image container Application");
    };
    let token = format!(
        "deploy_{:032x}{:032x}",
        rand::random::<u128>(),
        rand::random::<u128>()
    );
    let trigger = Trigger {
        application_id: id,
        generation: format!("{:032x}", rand::random::<u128>()),
        token_hash: hash(&token),
        source_hash,
        branch,
        image,
        created_at: audit::timestamp(),
    };
    match TRIGGERS.upsert(&state.store, &trigger).await {
        Ok(()) => {
            let mut body = describe(&trigger);
            body["token"] = token.into();
            ([(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
        }
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
    if trigger.image.is_some() {
        return deliver_image(&state, id, trigger, request).await;
    }
    if request.image.is_some() {
        return invalid("a Git trigger deploys a branch, not an image");
    }
    // Authenticate even ignored events. Nothing is queued for another branch.
    if request.branch.as_deref().map(branch) != Some(trigger.branch.as_str()) {
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
    accept(&state, id, app.name, work, delivery).await
}

/// An image trigger pulls the configured reference again, or the delivered
/// one when CI names a new tag of the same repository.
async fn deliver_image<S: StateStore>(
    state: &AppState<S>,
    id: String,
    trigger: Trigger,
    request: Request,
) -> Response {
    if request.branch.is_some() || request.revision.is_some() {
        return invalid("an image trigger deploys a registry reference, not a branch");
    }
    let app = match apps::get_application(&state.store, &id).await {
        Ok(app) => app,
        Err(error) => return crate::deploy_error_response(error),
    };
    if !is_image_application(&app) || hash(repository(&app.image)) != trigger.source_hash {
        return invalid("the configured image changed; enable a new trigger");
    }
    if let Some(image) = &request.image
        && (image.is_empty()
            || image.len() > 512
            || image.chars().any(char::is_whitespace)
            || repository(image) != trigger.image.as_deref().unwrap_or_default())
    {
        return invalid("image must be a tag or digest of the trigger's repository");
    }
    let delivery = Delivery {
        id: hash(&format!(
            "{}:{}:{}",
            id, trigger.generation, request.delivery_id
        )),
        fingerprint: hash(&serde_json::to_string(&request).expect("trigger request serializes")),
        task_id: String::new(),
    };
    let work = tasks::Work::TriggerImageApplication {
        id: id.clone(),
        image: request.image,
    };
    accept(state, id, app.name, work, delivery).await
}

async fn accept<S: StateStore>(
    state: &AppState<S>,
    id: String,
    name: String,
    work: tasks::Work,
    delivery: Delivery,
) -> Response {
    let outcome = audit::ACTOR
        .scope(
            Some("Application deploy trigger".into()),
            tasks::enqueue_once(
                state,
                audit::Subject::new("application", &id, &name),
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
        crate::source::inspection::inspect(&crate::source::private_root(), &branch_source, None)
            .await?;
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

/// Redeploys an image Application, pulling what the registry has now. The
/// trigger's repository was checked on delivery; the Application may have
/// been reconfigured since, so it is checked again here.
pub(crate) async fn execute_image(
    store: &impl StateStore,
    docker: &(impl crate::docker::DockerRuntime + ?Sized),
    routes: &(impl crate::routes::RouteStore + ?Sized),
    id: &str,
    image: Option<String>,
) -> Result<Vec<audit::Change>, apps::DeployError> {
    let current = apps::get_application(store, id).await?;
    let trigger = TRIGGERS.get(store, id).await?;
    if !is_image_application(&current)
        || trigger
            .as_ref()
            .and_then(|trigger| trigger.image.as_deref())
            != Some(repository(&current.image))
    {
        return Err(apps::DeployError::Readiness(
            "the configured image changed before this delivery ran".into(),
        ));
    }
    let update = apps::ApplicationUpdate {
        image: Some(image.unwrap_or_else(|| current.image.clone())),
        pull: true,
        ..apps::ApplicationUpdate::default()
    };
    let pending = apps::prepare_update(store, id, update).await?;
    apps::finish_deploy_reporting(store, docker, routes, pending)
        .await
        .map(|(_, changes)| changes)
}
