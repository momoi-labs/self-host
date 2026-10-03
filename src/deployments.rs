//! Deployment outcomes and immutable images for an explicit recovery.
//! Snapshots stay in private Platform State. The API exposes only metadata.

pub mod readiness;
#[cfg(test)]
mod tests;
pub(crate) mod triggers;

use crate::{
    AppState,
    apps::{self, DeployError},
    audit,
    collection::{Collection, Keyed},
    docker::DockerRuntime,
    error::ErrorReport,
    store::{ApplicationRecord, Runtime, StateStore, StoreError},
    tasks,
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use readiness::Readiness;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};

const DEPLOYMENTS: Collection<Deployment> = Collection::new("deployment");

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Deployment {
    pub id: String,
    pub application_id: String,
    pub created_at: String,
    pub status: String,
    pub readiness: Readiness,
    pub images: BTreeMap<String, String>,
    pub revision: Option<String>,
    pub error: Option<ErrorReport>,
    pub snapshot: ApplicationRecord,
}
impl Keyed for Deployment {
    fn key(&self) -> String {
        self.id.clone()
    }
}

#[derive(Serialize)]
struct Summary {
    id: String,
    created_at: String,
    status: String,
    readiness: Readiness,
    images: BTreeMap<String, String>,
    revision: Option<String>,
    error: Option<ErrorReport>,
    recoverable: bool,
}
impl From<Deployment> for Summary {
    fn from(d: Deployment) -> Self {
        Self {
            recoverable: d.status == "completed" && !d.images.is_empty(),
            id: d.id,
            created_at: d.created_at,
            status: d.status,
            readiness: d.readiness,
            images: d.images,
            revision: d.revision,
            error: d.error,
        }
    }
}

pub fn immutable_image(image: &str) -> bool {
    image
        .strip_prefix("sha256:")
        .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|c| c.is_ascii_hexdigit()))
}

pub async fn begin(
    store: &impl StateStore,
    app: &ApplicationRecord,
) -> Result<Deployment, StoreError> {
    let deployment = Deployment {
        id: audit::event_id().unwrap_or_else(|| format!("deploy-{:032x}", rand::random::<u128>())),
        application_id: app.id.clone(),
        created_at: audit::timestamp(),
        status: "checking".into(),
        readiness: Readiness::Checking,
        images: BTreeMap::new(),
        revision: app.git_build.as_ref().map(|b| b.revision.clone()),
        error: None,
        snapshot: app.clone(),
    };
    DEPLOYMENTS.upsert(store, &deployment).await?;
    Ok(deployment)
}

pub async fn health_timeout(store: &impl StateStore) -> Result<Duration, StoreError> {
    let seconds = store
        .get_state("deployment_health_timeout_seconds")
        .await?
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(120)
        .clamp(1, 600);
    Ok(Duration::from_secs(seconds))
}

pub async fn complete(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    deployment: &mut Deployment,
) -> Result<(), DeployError> {
    deployment.readiness =
        readiness::wait(docker, &deployment.snapshot, health_timeout(store).await?)
            .await
            .map_err(DeployError::Readiness)?;
    for (service, container) in apps::containers_of(&deployment.snapshot) {
        let image = tokio::time::timeout(
            Duration::from_secs(5),
            docker.container_image_id(&container),
        )
        .await
        .map_err(|_| DeployError::Readiness("recording the deployed image timed out".into()))??;
        if !immutable_image(&image) {
            return Err(DeployError::Readiness(
                "Docker did not report an immutable deployed image".into(),
            ));
        }
        deployment.images.insert(service, image);
    }
    if deployment.images.is_empty() {
        return Err(DeployError::Readiness(
            "the deployment has no running images".into(),
        ));
    }
    deployment.snapshot = pinned_snapshot(&deployment.snapshot, &deployment.images)?;
    deployment.status = "completed".into();
    DEPLOYMENTS.upsert(store, deployment).await?;
    Ok(())
}

fn pinned_snapshot(
    app: &ApplicationRecord,
    images: &BTreeMap<String, String>,
) -> Result<ApplicationRecord, DeployError> {
    let mut snapshot = app.clone();
    if let Some(compose) = &app.compose {
        let mut yaml: serde_yaml::Value = serde_yaml::from_str(compose)
            .map_err(|_| DeployError::Readiness("could not pin the Compose definition".into()))?;
        let services = yaml["services"]
            .as_mapping_mut()
            .ok_or_else(|| DeployError::Readiness("the Compose snapshot has no services".into()))?;
        for (key, service) in services {
            let name = key.as_str().unwrap_or_default();
            let image = images.get(name).ok_or_else(|| {
                DeployError::Readiness("a Compose service has no immutable image".into())
            })?;
            service["image"] = serde_yaml::Value::String(image.clone());
        }
        snapshot.compose =
            Some(serde_yaml::to_string(&yaml).map_err(|_| {
                DeployError::Readiness("could not save the Compose snapshot".into())
            })?);
        snapshot.image = images
            .get(app.web_service.as_deref().unwrap_or_default())
            .or_else(|| images.values().next())
            .cloned()
            .unwrap_or_default();
    } else {
        snapshot.image = images.get("app").cloned().ok_or_else(|| {
            DeployError::Readiness("the deployment has no immutable image".into())
        })?;
    }
    if let Some(build) = &mut snapshot.git_build {
        build.images = images.clone();
    }
    Ok(snapshot)
}

pub async fn failed(store: &impl StateStore, deployment: &mut Deployment, error: &DeployError) {
    deployment.status = "failed".into();
    deployment.readiness = Readiness::Failed;
    deployment.error = Some(
        crate::postgres::runtime_report(store, &deployment.application_id, ErrorReport::new(error))
            .await,
    );
    if let Err(error) = DEPLOYMENTS.upsert(store, deployment).await {
        tracing::error!(%error, "could not record a failed deployment");
    }
}

pub async fn history(store: &impl StateStore, id: &str) -> Result<Vec<Deployment>, StoreError> {
    let mut entries: Vec<_> = DEPLOYMENTS
        .list(store)
        .await?
        .into_iter()
        .filter(|d| d.application_id == id)
        .collect();
    entries.reverse();
    Ok(entries)
}

pub(crate) async fn list<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
) -> Response {
    if let Err(error) = apps::get_application(&state.store, &id).await {
        return crate::deploy_error_response(error);
    }
    match history(&state.store, &id).await {
        Ok(entries) => {
            Json(entries.into_iter().map(Summary::from).collect::<Vec<_>>()).into_response()
        }
        Err(error) => crate::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

pub(crate) async fn request_restore<S: StateStore>(
    State(state): State<AppState<S>>,
    Path((id, deployment)): Path<(String, String)>,
) -> Response {
    let app = match apps::get_application(&state.store, &id).await {
        Ok(app) => app,
        Err(error) => return crate::deploy_error_response(error),
    };
    if let Err(error) = recovery_snapshot(&state.store, &app, &deployment).await {
        return crate::deploy_error_response(error);
    }
    crate::accepted_task(
        tasks::enqueue(
            &state,
            "configure",
            audit::Subject::new("application", &id, &app.name),
            tasks::Work::RecoverApplication { id, deployment },
        )
        .await,
    )
}

async fn recovery_snapshot(
    store: &impl StateStore,
    current: &ApplicationRecord,
    id: &str,
) -> Result<ApplicationRecord, DeployError> {
    let deployment = DEPLOYMENTS
        .get(store, id)
        .await?
        .filter(|d| {
            d.application_id == current.id && d.status == "completed" && !d.images.is_empty()
        })
        .ok_or_else(|| {
            DeployError::Readiness("the requested successful deployment is unavailable".into())
        })?;
    if current.runtime != Runtime::Container {
        return Err(DeployError::Readiness(
            "immutable image recovery requires a container Application".into(),
        ));
    }
    let snapshot = pinned_snapshot(&deployment.snapshot, &deployment.images)?;
    // Recover code, not the Hostname, network grants, Variables or data.
    Ok(ApplicationRecord {
        image: snapshot.image,
        compose: snapshot.compose,
        source: snapshot.source,
        git: snapshot.git,
        git_build: snapshot.git_build,
        development: snapshot.development,
        web_service: snapshot.web_service,
        web_port: snapshot.web_port,
        variable_delivery: snapshot.variable_delivery,
        ..current.clone()
    })
}

pub async fn restore(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    routes: &(impl crate::routes::RouteStore + ?Sized),
    id: &str,
    deployment: &str,
) -> Result<(), DeployError> {
    let current = apps::get_application(store, id).await?;
    let snapshot = recovery_snapshot(store, &current, deployment).await?;
    // Resolve every image before changing the running Application. An image
    // removed by an external prune is a visible refusal, never a fresh pull.
    let stored = DEPLOYMENTS
        .get(store, deployment)
        .await?
        .ok_or_else(|| DeployError::NotFound(deployment.into()))?;
    for image in stored.images.values() {
        if !immutable_image(image) || docker.image_id(image).await?.as_deref() != Some(image) {
            return Err(DeployError::Readiness(
                "a recovery image is unavailable locally; no images were pulled".into(),
            ));
        }
    }
    apps::restore_snapshot(store, docker, routes, snapshot).await
}

/// Interrupted readiness never becomes a successful deployment on reboot.
pub(crate) async fn recover(store: &impl StateStore) -> Result<(), StoreError> {
    for mut deployment in DEPLOYMENTS.list(store).await? {
        if deployment.status != "checking" {
            continue;
        }
        let report =
            ErrorReport::plain("The Platform restarted before deployment readiness finished.");
        deployment.status = "failed".into();
        deployment.readiness = Readiness::Failed;
        deployment.error = Some(report.clone());
        DEPLOYMENTS.upsert(store, &deployment).await?;
        if let Some(app) = store.get_application(&deployment.application_id).await?
            && app.status == apps::STATUS_PENDING
        {
            store
                .set_application_outcome(&app.id, apps::STATUS_FAILED, Some(report))
                .await?;
        }
    }
    Ok(())
}
