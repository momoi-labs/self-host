use std::sync::Arc;

use axum::{
    Json, Router,
    body::Body,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response, sse},
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;

use crate::error::ErrorReport;
use crate::store::{DevelopmentApplication, StoreError};
use rand::Rng;

pub mod apps;
pub mod bootstrap;
pub mod compose_app;
pub mod config;
pub mod console;
pub mod dev_images;
pub mod dns;
pub mod docker;
pub mod error;
pub mod file_store;
pub mod host_addresses;
pub mod host_dns;
pub mod paths;
pub mod ports;
pub mod proxy;
pub mod routes;
pub mod store;
pub mod tls;

use apps::{DeployError, RemoveError};
use docker::DockerRuntime;
use routes::RouteStore;
use store::StateStore;

#[derive(Clone)]
struct AppState<S: StateStore> {
    store: S,
    docker: Arc<dyn DockerRuntime>,
    routes: Arc<dyn RouteStore>,
    dev_images: Arc<dev_images::Builds>,
}

pub fn build_app<S: StateStore>(
    store: S,
    docker: Arc<dyn DockerRuntime>,
    routes: Arc<dyn RouteStore>,
) -> Router {
    let state = AppState {
        store,
        docker,
        routes,
        dev_images: Arc::new(dev_images::Builds::default()),
    };

    let api_routes = Router::new()
        .route("/health", get(health))
        .route("/bootstrap/status", get(bootstrap_status::<S>))
        .route("/apps", get(list_apps::<S>).post(deploy_app::<S>))
        .route("/compose/inspect", post(inspect_compose))
        .route(
            "/dev-images",
            get(dev_images::list::<S>).post(dev_images::create::<S>),
        )
        .route("/dev-images/tools", get(dev_images::catalog))
        .route("/dev-images/{id}", delete(dev_images::remove::<S>))
        .route("/apps/{name}", delete(remove_app::<S>))
        .route("/apps/id/{id}", get(get_app::<S>).put(update_app::<S>))
        .route("/apps/id/{id}/start", post(start_app::<S>))
        .route("/apps/id/{id}/stop", post(stop_app::<S>))
        .route("/apps/id/{id}/restart", post(restart_app::<S>))
        .route("/apps/{name}/env", get(get_env::<S>).post(set_env::<S>))
        .route("/apps/{name}/env/{key}", delete(unset_env::<S>))
        .route("/apps/{name}/logs", get(stream_logs::<S>))
        .route("/apps/id/{id}/logs", get(stream_logs_by_id::<S>))
        .route("/apps/id/{id}/containers", get(list_app_containers::<S>))
        .route("/api-keys", get(list_keys::<S>).post(create_key::<S>))
        .route("/api-keys/{id}", delete(revoke_key::<S>))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_api_key::<S>,
        ))
        .with_state(state);

    console::console_router()
        .merge(public_ca_router(tls::ca_cert_path()))
        .merge(api_routes)
}

fn public_ca_router(path: std::path::PathBuf) -> Router {
    Router::new()
        .route("/ca.pem", get(public_ca_certificate))
        .with_state(path)
}

async fn public_ca_certificate(State(path): State<std::path::PathBuf>) -> Response {
    match std::fs::read(path) {
        Ok(ca) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/x-pem-file")
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::from(ca))
            .unwrap(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
    })
}

#[derive(Serialize)]
struct BootstrapStatusResponse {
    initialized: bool,
    host_ip: Option<String>,
    /// What the interfaces have right now, under the policy in `dns.json`.
    /// Empty when nothing is up or the scan fails: an address that is not
    /// there must not be advertised (#61).
    host_addresses: Vec<String>,
    dns_suffix: Option<String>,
}

async fn bootstrap_status<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
) -> Json<BootstrapStatusResponse> {
    let initialized = state.store.is_initialized().await.unwrap_or(false);

    let dns_suffix = if initialized {
        state.store.get_state("dns_suffix").await.ok().flatten()
    } else {
        None
    };

    let host_ip = if initialized {
        state.store.get_state("host_ip").await.ok().flatten()
    } else {
        None
    };

    let host_addresses = if initialized {
        scan_for_status().unwrap_or_default()
    } else {
        Vec::new()
    };

    Json(BootstrapStatusResponse {
        initialized,
        host_ip,
        host_addresses,
        dns_suffix,
    })
}

/// The policy lives in `dns.json`; the addresses live on the interfaces.
/// Either being unreadable means "nothing to advertise", not an error page.
fn scan_for_status() -> Option<Vec<String>> {
    let config = crate::dns::Config::load().ok()?;
    let source = crate::host_addresses::default_source().ok()?;
    let interfaces = crate::host_addresses::interfaces().ok()?;
    let addresses = crate::host_addresses::select(&config.host_addresses, source, &interfaces);
    Some(
        addresses
            .iter()
            .map(|address| address.to_string())
            .collect(),
    )
}

#[derive(Deserialize)]
struct DeployApplicationRequest {
    name: String,
    #[serde(default)]
    image: String,
    #[serde(default)]
    path: String,
    /// A Compose file, as the Operator wrote it (ADR-0014).
    #[serde(default)]
    compose: String,
    /// The Compose service and container port the Hostname routes to. Left
    /// out, the first service that publishes a port is the target.
    #[serde(default)]
    web_service: Option<String>,
    #[serde(default)]
    web_port: Option<u16>,
    #[serde(default)]
    hostname: Option<String>,
    /// Extra Hostnames the Application also answers on.
    #[serde(default)]
    aliases: Option<Vec<String>>,
    #[serde(default)]
    development: Option<DevelopmentApplicationRequest>,
}

#[derive(Clone, Deserialize)]
struct DevelopmentApplicationRequest {
    image_id: String,
    tag: String,
    command: String,
    web_port: u16,
    #[serde(default)]
    persist_data: bool,
}

impl From<&DevelopmentApplicationRequest> for DevelopmentApplication {
    fn from(value: &DevelopmentApplicationRequest) -> Self {
        Self {
            image_id: value.image_id.clone(),
            tag: value.tag.clone(),
            command: value.command.clone(),
            web_port: value.web_port,
            persist_data: value.persist_data,
        }
    }
}

#[derive(Deserialize)]
struct UpdateApplicationRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    hostname: Option<String>,
    #[serde(default)]
    aliases: Option<Vec<String>>,
    #[serde(default)]
    compose: Option<String>,
    #[serde(default)]
    web_service: Option<String>,
    #[serde(default)]
    web_port: Option<u16>,
    #[serde(default)]
    development: Option<DevelopmentApplicationRequest>,
}

/// One container of an Application, as Docker sees it right now.
#[derive(Serialize)]
struct ServiceStateResponse {
    service: String,
    container: String,
    state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restarts: Option<u32>,
}

impl From<apps::ServiceState> for ServiceStateResponse {
    fn from(s: apps::ServiceState) -> Self {
        Self {
            service: s.service,
            container: s.container,
            state: s.state,
            exit_code: s.exit_code,
            restarts: s.restarts,
        }
    }
}

#[derive(Serialize)]
struct ApplicationResponse {
    id: String,
    name: String,
    hostname: String,
    aliases: Vec<String>,
    image: String,
    status: String,
    source: String,
    /// Present when `status` is `failed`, so the console can say why.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<ErrorReport>,
    /// How many times the containers restarted, summed, when there is a
    /// container to ask about. The console has no metrics to show, but this
    /// much says whether an Application is settled or flapping.
    #[serde(skip_serializing_if = "Option::is_none")]
    restarts: Option<u32>,
    /// The Compose definition, for a Compose Application.
    #[serde(skip_serializing_if = "Option::is_none")]
    compose: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    web_service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    web_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    development: Option<DevelopmentApplicationResponse>,
    /// Every container of the Application and its state. Empty until Docker
    /// has been asked.
    #[serde(default)]
    services: Vec<ServiceStateResponse>,
}

#[derive(Serialize)]
struct DevelopmentApplicationResponse {
    image_id: String,
    tag: String,
    command: String,
    web_port: u16,
    persist_data: bool,
}

impl From<DevelopmentApplication> for DevelopmentApplicationResponse {
    fn from(value: DevelopmentApplication) -> Self {
        Self {
            image_id: value.image_id,
            tag: value.tag,
            command: value.command,
            web_port: value.web_port,
            persist_data: value.persist_data,
        }
    }
}

impl From<apps::ApplicationRecord> for ApplicationResponse {
    fn from(app: apps::ApplicationRecord) -> Self {
        Self {
            id: app.id,
            name: app.name,
            hostname: app.hostname,
            aliases: app.aliases,
            image: app.image,
            status: app.status,
            source: app.source,
            last_error: app.last_error,
            restarts: None,
            compose: app.compose,
            web_service: app.web_service,
            web_port: app.web_port,
            development: app.development.map(Into::into),
            services: Vec::new(),
        }
    }
}

/// The Application as the console sees it: the row, checked against what
/// Docker is actually running. A runtime that cannot answer leaves the
/// status as recorded and the services as `missing`.
async fn observed<S: StateStore>(
    state: &AppState<S>,
    app: apps::ApplicationRecord,
) -> ApplicationResponse {
    let services = apps::service_states(state.docker.as_ref(), &app).await;
    let (status, last_error) = apps::live_status(&app, &services);
    let restarts = services
        .iter()
        .filter_map(|s| s.restarts)
        .reduce(|a, b| a + b);
    ApplicationResponse {
        status,
        last_error,
        restarts,
        services: services.into_iter().map(Into::into).collect(),
        ..ApplicationResponse::from(app)
    }
}

#[derive(Deserialize)]
struct InspectComposeRequest {
    compose: String,
}

#[derive(Serialize)]
struct InspectedPort {
    host: Option<u16>,
    container: u16,
}

#[derive(Serialize)]
struct InspectedVolume {
    source: String,
    target: String,
    /// `named`, `data`, `host` or `anonymous`; see docs/compose-applications.md.
    kind: &'static str,
    /// For `data`, the path under the Application's directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    data_path: Option<String>,
}

#[derive(Serialize)]
struct InspectedService {
    name: String,
    image: String,
    ports: Vec<InspectedPort>,
    volumes: Vec<InspectedVolume>,
}

/// What a Compose file declares, before anything is deployed: the services,
/// their images and the ports they publish, plus the web target the Platform
/// would pick on its own. The console reads this as the Operator types, so
/// the web service and port are chosen from a list instead of typed by hand.
#[derive(Serialize)]
struct InspectComposeResponse {
    services: Vec<InspectedService>,
    web_service: Option<String>,
    web_port: Option<u16>,
}

async fn inspect_compose(Json(body): Json<InspectComposeRequest>) -> Response {
    let definition = match compose_app::ComposeDefinition::parse(&body.compose) {
        Ok(d) => d,
        Err(e) => return deploy_error_response(DeployError::InvalidCompose(e)),
    };
    let target = definition.web_target(None, None).ok();
    let services = definition
        .services
        .iter()
        .map(|s| InspectedService {
            name: s.name.clone(),
            image: s.image.clone(),
            ports: s
                .ports
                .iter()
                .map(|p| InspectedPort {
                    host: p.host,
                    container: p.container,
                })
                .collect(),
            volumes: s
                .mounts
                .iter()
                .map(|m| InspectedVolume {
                    source: m.source.clone(),
                    target: m.target.clone(),
                    kind: match m.kind {
                        compose_app::MountKind::Named => "named",
                        compose_app::MountKind::Data => "data",
                        compose_app::MountKind::Host => "host",
                        compose_app::MountKind::Anonymous => "anonymous",
                    },
                    data_path: m.data_path.clone(),
                })
                .collect(),
        })
        .collect();
    (
        StatusCode::OK,
        Json(InspectComposeResponse {
            services,
            web_service: target.as_ref().map(|t| t.service.clone()),
            web_port: target.map(|t| t.port),
        }),
    )
        .into_response()
}

/// Saves an edit and redeploys. Addressed by id, not name, because the name is
/// exactly what an edit may be changing.
async fn update_app<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<UpdateApplicationRequest>,
) -> Response {
    let current = match apps::get_application(&state.store, &id).await {
        Ok(app) => app,
        Err(error) => return deploy_error_response(error),
    };
    if body.development.is_some()
        && current.development.is_none()
        && (current.source != apps::SOURCE_COMPOSE || body.compose.is_some())
    {
        return deploy_error_response(apps::DeployError::InvalidDevelopment(
            "explicit conversion is available only for an existing Compose application without a Compose file in this request".into(),
        ));
    }
    if current.development.is_some()
        && (body.image.is_some()
            || body.compose.is_some()
            || body.web_service.is_some()
            || body.web_port.is_some())
    {
        return deploy_error_response(apps::DeployError::InvalidDevelopment(
            "development settings include the image and web target; do not send image, Compose, web service, or web port separately".into(),
        ));
    }
    let development = match body.development.as_ref() {
        Some(request) => {
            match validate_development_image(&state, request, current.development.as_ref()).await {
                Ok(settings) => Some(settings),
                Err(error) => return deploy_error_response(error),
            }
        }
        None => None,
    };
    match apps::prepare_update(
        &state.store,
        &id,
        apps::ApplicationUpdate {
            name: body.name,
            image: body.image,
            hostname: body.hostname,
            aliases: body.aliases,
            compose: body.compose,
            web_service: body.web_service,
            web_port: body.web_port,
            development,
        },
    )
    .await
    {
        Ok(pending) => accept_deploy(&state, pending).await,
        Err(e) => deploy_error_response(e),
    }
}

/// Answers with the `pending` row and finishes the deploy on a task. A
/// `docker pull` of a large image takes minutes, and holding the request open
/// for it leaves the console with nothing to show.
///
/// A deploy with no Docker work — a rename — is already done, and says so with
/// a plain `200`.
async fn accept_deploy<S: StateStore>(
    state: &AppState<S>,
    pending: apps::PendingDeploy,
) -> Response {
    let body = Json(ApplicationResponse::from(pending.record.clone()));

    // A settled deploy is only a route rewrite — fast enough to finish before
    // answering, so the caller's next request already sees the new Hostname.
    if pending.is_settled() {
        return match apps::finish_deploy(
            &state.store,
            state.docker.as_ref(),
            state.routes.as_ref(),
            pending,
        )
        .await
        {
            Ok(_) => (StatusCode::OK, body).into_response(),
            Err(e) => deploy_error_response(e),
        };
    }

    let store = state.store.clone();
    let docker = state.docker.clone();
    let routes = state.routes.clone();
    let name = pending.record.name.clone();
    tokio::spawn(async move {
        if let Err(e) = apps::finish_deploy(&store, docker.as_ref(), routes.as_ref(), pending).await
        {
            tracing::warn!(
                "deploy of Application '{name}' failed: {}",
                ErrorReport::new(&e)
            );
        }
    });

    (StatusCode::ACCEPTED, body).into_response()
}

async fn get_app<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    match apps::get_application(&state.store, &id).await {
        Ok(app) => (StatusCode::OK, Json(observed(&state, app).await)).into_response(),
        Err(e) => deploy_error_response(e),
    }
}

/// Start, stop and restart act on what is already deployed, and answer
/// with the Application as it stands afterwards.
async fn start_app<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    lifecycle_response(
        &state,
        apps::start_application(
            &state.store,
            state.docker.as_ref(),
            state.routes.as_ref(),
            &id,
        )
        .await,
    )
    .await
}

async fn stop_app<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    lifecycle_response(
        &state,
        apps::stop_application(
            &state.store,
            state.docker.as_ref(),
            state.routes.as_ref(),
            &id,
        )
        .await,
    )
    .await
}

async fn restart_app<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    lifecycle_response(
        &state,
        apps::restart_application(
            &state.store,
            state.docker.as_ref(),
            state.routes.as_ref(),
            &id,
        )
        .await,
    )
    .await
}

async fn lifecycle_response<S: StateStore>(
    state: &AppState<S>,
    result: Result<apps::ApplicationRecord, DeployError>,
) -> Response {
    match result {
        Ok(app) => (StatusCode::OK, Json(observed(state, app).await)).into_response(),
        Err(e) => deploy_error_response(e),
    }
}

async fn deploy_app<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    Json(body): Json<DeployApplicationRequest>,
) -> Response {
    let hostname = body.hostname.as_deref();
    let aliases = body.aliases.as_deref();

    let development = match body.development.as_ref() {
        Some(request) => match validate_development_image(&state, request, None).await {
            Ok(settings) => Some(settings),
            Err(error) => return deploy_error_response(error),
        },
        None => None,
    };
    let prepared = match (
        &body.image[..],
        &body.path[..],
        &body.compose[..],
        development,
    ) {
        ("", "", "", Some(settings)) => {
            apps::prepare_deploy_from_development(
                &state.store,
                &body.name,
                settings,
                hostname,
                aliases,
            )
            .await
        }
        ("", "", "", None) => Err(apps::DeployError::MissingImage),
        (image, "", "", None) => {
            apps::prepare_deploy_from_image(&state.store, &body.name, image, hostname, aliases)
                .await
        }
        ("", path, "", None) => {
            apps::prepare_deploy_from_path(&state.store, &body.name, path, hostname, aliases).await
        }
        ("", "", compose, None) => {
            apps::prepare_deploy_from_compose(
                &state.store,
                &body.name,
                compose,
                body.web_service.as_deref(),
                body.web_port,
                hostname,
                aliases,
            )
            .await
        }
        _ => Err(apps::DeployError::InvalidDevelopment(
            "send either development settings or an image, path, or Compose definition".into(),
        )),
    };

    match prepared {
        Ok(pending) => accept_deploy(&state, pending).await,
        Err(err) => deploy_error_response(err),
    }
}

/// A saved Application may keep an older tag after a later build changes the
/// image record. Reusing that exact persisted tag remains valid; selecting any
/// other tag must name a currently successful build.
async fn validate_development_image<S: StateStore>(
    state: &AppState<S>,
    request: &DevelopmentApplicationRequest,
    current: Option<&DevelopmentApplication>,
) -> Result<DevelopmentApplication, apps::DeployError> {
    let settings = DevelopmentApplication::from(request);
    if current.is_some_and(|saved| saved.image_id == settings.image_id && saved.tag == settings.tag)
    {
        return Ok(settings);
    }
    match dev_images::ready_image(state, &settings.image_id, &settings.tag).await {
        Ok(Some(_)) => Ok(settings),
        Ok(None) => Err(apps::DeployError::InvalidDevelopment(
            "select a successful development image build".into(),
        )),
        Err(error) => Err(apps::DeployError::InvalidDevelopment(format!(
            "could not read development images: {error}"
        ))),
    }
}

async fn list_apps<S: StateStore>(state: axum::extract::State<AppState<S>>) -> Response {
    match apps::list_applications(&state.store).await {
        Ok(apps) => {
            let mut body: Vec<ApplicationResponse> = Vec::with_capacity(apps.len());
            for app in apps {
                body.push(observed(&state, app).await);
            }
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(err) => deploy_error_response(err),
    }
}

async fn remove_app<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response {
    match apps::remove_application(
        &state.store,
        state.docker.as_ref(),
        state.routes.as_ref(),
        &name,
    )
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => remove_error_response(err),
    }
}

/// Every error leaves the API as `{"error": ..., "caused_by": [...]}`, so a
/// client never has to split one sentence back into layers.
fn error_response(status: StatusCode, err: &dyn std::error::Error) -> Response {
    (status, Json(ErrorReport::new(err))).into_response()
}

fn remove_error_response(err: RemoveError) -> Response {
    let status = match &err {
        RemoveError::NotFound(_) => StatusCode::NOT_FOUND,
        RemoveError::NotInitialized => StatusCode::PRECONDITION_FAILED,
        RemoveError::Docker(_) | RemoveError::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error_response(status, &err)
}

#[derive(Deserialize)]
struct SetEnvRequest {
    key: String,
    value: String,
}

async fn set_env<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(body): Json<SetEnvRequest>,
) -> Response {
    match apps::set_env(
        &state.store,
        state.docker.as_ref(),
        &name,
        &body.key,
        &body.value,
    )
    .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => env_error_response(err),
    }
}

async fn get_env<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> Response {
    match apps::get_all_env(&state.store, &name).await {
        Ok(env) => (StatusCode::OK, Json(env)).into_response(),
        Err(err) => env_error_response(err),
    }
}

async fn unset_env<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    axum::extract::Path((name, key)): axum::extract::Path<(String, String)>,
) -> Response {
    match apps::unset_env(&state.store, state.docker.as_ref(), &name, &key).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => env_error_response(err),
    }
}

fn env_error_response(err: apps::EnvError) -> Response {
    let status = match &err {
        apps::EnvError::NotFound(_) => StatusCode::NOT_FOUND,
        apps::EnvError::NotInitialized => StatusCode::PRECONDITION_FAILED,
        apps::EnvError::Docker(_) | apps::EnvError::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error_response(status, &err)
}

async fn list_app_containers<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    match state.store.get_application(&id).await {
        Ok(Some(_)) => match state.docker.application_containers(&id).await {
            Ok(names) => Json(names).into_response(),
            Err(err) => logs_error_response(&apps::LogsError::Docker(err)),
        },
        Ok(None) => logs_error_response(&apps::LogsError::NotFound(id)),
        Err(err) => logs_error_response(&apps::LogsError::Store(err)),
    }
}

#[derive(serde::Deserialize)]
struct LogQuery {
    container: Option<String>,
}

/// The CLI still addresses logs by name; the console addresses them by id.
async fn stream_logs<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<LogQuery>,
) -> Response {
    let Ok(Some(app)) = state.store.find_application_by_name(&name).await else {
        return logs_error_response(&apps::LogsError::NotFound(name));
    };
    stream_logs_for(state, app, query.container).await
}

async fn stream_logs_by_id<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<LogQuery>,
) -> Response {
    match state.store.get_application(&id).await {
        Ok(Some(app)) => stream_logs_for(state, app, query.container).await,
        Ok(None) => logs_error_response(&apps::LogsError::NotFound(id)),
        Err(err) => logs_error_response(&apps::LogsError::Store(err)),
    }
}

/// The one place a log stream is opened. The name is the mutable field
/// (ADR-0008), so a cached name can point at the wrong container; the id never
/// does.
async fn stream_logs_for<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    app: apps::ApplicationRecord,
    container: Option<String>,
) -> Response {
    let containers = match state.docker.application_containers(&app.id).await {
        Ok(names) => names,
        Err(err) => return logs_error_response(&apps::LogsError::Docker(err)),
    };
    let requested_container = container.is_some();
    let container_name = container.unwrap_or_else(|| containers[0].clone());
    if !containers.contains(&container_name) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorReport::plain(
                "container does not belong to this Application",
            )),
        )
            .into_response();
    }

    if let Some(message) = nothing_to_stream(&state, &app, &container_name).await {
        let stream = tokio_stream::once(Ok::<_, std::convert::Infallible>(
            sse::Event::default().event("notice").data(message),
        ));
        return sse::Sse::new(stream).into_response();
    }

    let logs = if app.source == apps::SOURCE_COMPOSE && !requested_container {
        match apps::project_for(&state.store, &app).await {
            Ok(project) => state.docker.stream_compose_logs(&project).await,
            Err(err) => return deploy_error_response(err),
        }
    } else {
        state.docker.stream_logs(&container_name).await
    };

    match logs {
        Ok(rx) => {
            let stream = tokio_stream::wrappers::ReceiverStream::new(rx).map(|line| {
                Ok::<_, std::convert::Infallible>(axum::response::sse::Event::default().data(line))
            });
            sse::Sse::new(stream)
                .keep_alive(
                    sse::KeepAlive::new()
                        .interval(std::time::Duration::from_secs(15))
                        .text("keepalive"),
                )
                .into_response()
        }
        Err(err) => logs_error_response(&apps::LogsError::Docker(err)),
    }
}

/// A sentence for when there is no container to stream from, rather than
/// silence or Docker's own "no such container" error dressed up as the
/// Application's output.
///
/// A container that exited still has its output, and that output is exactly
/// what says why it exited — so only a container that is not there at all
/// gets the sentence.
async fn nothing_to_stream<S: StateStore>(
    state: &AppState<S>,
    app: &apps::ApplicationRecord,
    container_name: &str,
) -> Option<String> {
    if app.status == apps::STATUS_PENDING {
        return Some(
            "This application is deploying; logs will appear once the container starts.".into(),
        );
    }
    if !matches!(state.docker.container_state(container_name).await, Ok(None)) {
        return None;
    }

    if app.status == apps::STATUS_FAILED {
        return Some("This application failed to deploy, so there are no logs to stream.".into());
    }
    Some("This application has no container, so there are no logs to stream.".into())
}

fn logs_error_response(err: &apps::LogsError) -> Response {
    let status = match err {
        apps::LogsError::NotFound(_) => StatusCode::NOT_FOUND,
        apps::LogsError::NotInitialized => StatusCode::PRECONDITION_FAILED,
        apps::LogsError::Docker(_) | apps::LogsError::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error_response(status, err)
}

#[derive(Deserialize)]
struct CreateKeyRequest {
    label: String,
}

#[derive(Serialize)]
struct ApiKeyResponse {
    id: String,
    label: String,
    created_at: String,
}

#[derive(Serialize)]
struct CreateKeyResponse {
    id: String,
    label: String,
    created_at: String,
    key: String,
}

async fn list_keys<S: StateStore>(state: axum::extract::State<AppState<S>>) -> Response {
    match state.store.list_api_keys().await {
        Ok(keys) => {
            let body: Vec<ApiKeyResponse> = keys
                .into_iter()
                .map(|k| ApiKeyResponse {
                    id: k.id,
                    label: k.label,
                    created_at: k.created_at,
                })
                .collect();
            (StatusCode::OK, Json(body)).into_response()
        }
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

async fn create_key<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    Json(body): Json<CreateKeyRequest>,
) -> Response {
    let key_bytes: [u8; 32] = rand::rng().random();
    let key: String = key_bytes.iter().map(|b| format!("{b:02x}")).collect();
    let id = format!("sk-{}", &key[..12]);

    match state.store.create_api_key(&id, &body.label).await {
        Ok(()) => {
            let response = CreateKeyResponse {
                id,
                label: body.label,
                created_at: "now".into(),
                key,
            };
            (StatusCode::CREATED, Json(response)).into_response()
        }
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

async fn revoke_key<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    match state.store.revoke_api_key(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(StoreError::NotFound(_)) => {
            error_response(StatusCode::NOT_FOUND, &StoreError::NotFound("key".into()))
        }
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

fn deploy_error_response(err: DeployError) -> Response {
    let status = match &err {
        DeployError::AlreadyExists(_) => StatusCode::CONFLICT,
        DeployError::InvalidName(_)
        | DeployError::InvalidHostname(_)
        | DeployError::MissingImage
        | DeployError::MissingPath
        | DeployError::MissingCompose
        | DeployError::InvalidDevelopment(_)
        | DeployError::InvalidCompose(_) => StatusCode::BAD_REQUEST,
        DeployError::NotFound(_) => StatusCode::NOT_FOUND,
        DeployError::NotInitialized => StatusCode::PRECONDITION_FAILED,
        DeployError::Docker(_) | DeployError::NoWebTargetPort(_) | DeployError::Store(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    error_response(status, &err)
}

async fn require_api_key<S: StateStore>(
    state: axum::extract::State<AppState<S>>,
    req: Request,
    next: Next,
) -> Response {
    let expected_key = state
        .store
        .get_api_key()
        .await
        .ok()
        .flatten()
        .unwrap_or_default();

    let auth_header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match auth_header {
        Some(key) if constant_time_eq(key, &expected_key) => next.run(req).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            Json(ErrorReport::plain("invalid api key")),
        )
            .into_response(),
    }
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();

    if a.len() != b.len() {
        return false;
    }

    a.iter().zip(b.iter()).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, header};
    use axum::response::Response;
    use docker::FakeDocker;
    use serde_json::{Value, json};
    use store::FakeStateStore;
    use tower::ServiceExt;

    async fn send(app: &Router, uri: &str, api_key: Option<&str>) -> Response {
        let mut req = Request::builder().uri(uri);
        if let Some(key) = api_key {
            req = req.header(header::AUTHORIZATION, format!("Bearer {key}"));
        }
        app.clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn public_ca_can_be_downloaded_without_an_api_key() {
        let path =
            std::env::temp_dir().join(format!("self-host-public-ca-{}.pem", std::process::id()));
        std::fs::write(&path, b"public ca").unwrap();

        let response = public_ca_router(path.clone())
            .oneshot(
                Request::builder()
                    .uri("/ca.pem")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        std::fs::remove_file(path).unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/x-pem-file"
        );
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap(),
            "public ca"
        );
    }

    async fn post_json(app: &Router, uri: &str, api_key: Option<&str>, body: Value) -> Response {
        let mut req = Request::builder().method("POST").uri(uri);
        if let Some(key) = api_key {
            req = req.header(header::AUTHORIZATION, format!("Bearer {key}"));
        }
        req = req.header(header::CONTENT_TYPE, "application/json");
        app.clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }

    async fn put_json(app: &Router, uri: &str, api_key: Option<&str>, body: Value) -> Response {
        let mut req = Request::builder().method("PUT").uri(uri);
        if let Some(key) = api_key {
            req = req.header(header::AUTHORIZATION, format!("Bearer {key}"));
        }
        req = req.header(header::CONTENT_TYPE, "application/json");
        app.clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
    }

    async fn delete_req(app: &Router, uri: &str, api_key: Option<&str>) -> Response {
        let mut req = Request::builder().method("DELETE").uri(uri);
        if let Some(key) = api_key {
            req = req.header(header::AUTHORIZATION, format!("Bearer {key}"));
        }
        app.clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    /// A deploy finishes on a task now, so a test that wants to see its
    /// outcome has to wait for it the same way the console does.
    async fn settle(store: &FakeStateStore, name: &str) -> apps::ApplicationRecord {
        for _ in 0..200 {
            let found = store.find_application_by_name(name).await.unwrap();
            if let Some(app) = found
                && app.status != apps::STATUS_PENDING
            {
                return app;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("the deploy of '{name}' never left pending");
    }

    async fn setup_app(api_key: &str) -> (Router, FakeStateStore, FakeDocker) {
        let store = FakeStateStore::new();
        store.store_state("api_key", api_key).await.unwrap();
        let docker = FakeDocker::new();
        let app = build_app(
            store.clone(),
            Arc::new(docker.clone()),
            Arc::new(routes::FakeRoutes::new()),
        );
        (app, store, docker)
    }

    async fn setup_initialized_app(api_key: &str, dns_suffix: &str) -> (Router, FakeStateStore) {
        let store = FakeStateStore::new();
        store.store_state("api_key", api_key).await.unwrap();
        store.store_state("dns_suffix", dns_suffix).await.unwrap();
        let docker = FakeDocker::new();
        let app = build_app(
            store.clone(),
            Arc::new(docker),
            Arc::new(routes::FakeRoutes::new()),
        );
        (app, store)
    }

    const HERMES: &str = "services:\n  hermes:\n    image: nousresearch/hermes-agent:latest\n    command: gateway run\n    ports:\n      - \"9119:9119\"\n    volumes:\n      - ~/.hermes:/opt/data\n";

    #[tokio::test]
    async fn deploying_from_compose_records_the_definition_and_its_web_target() {
        let (app, store) = setup_initialized_app("test-key", "home.lan").await;

        let response = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "hermes", "compose": HERMES, "web_port": 9119}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["source"], json!("compose"));
        assert_eq!(parsed["image"], json!("nousresearch/hermes-agent:latest"));
        assert_eq!(parsed["web_service"], json!("hermes"));
        assert_eq!(parsed["web_port"], json!(9119));
        assert_eq!(parsed["compose"], json!(HERMES));

        let record = settle(&store, "hermes").await;
        assert_eq!(record.status, apps::STATUS_RUNNING);

        let response = send(&app, &format!("/apps/id/{}", record.id), Some("test-key")).await;
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["status"], json!("running"));
        assert_eq!(parsed["services"][0]["service"], json!("hermes"));
        assert_eq!(
            parsed["services"][0]["container"],
            json!(format!("sf-app-{}-hermes", record.id))
        );
        assert_eq!(parsed["services"][0]["state"], json!("running"));
    }

    #[tokio::test]
    async fn inspecting_a_compose_file_lists_its_services_and_ports() {
        let (app, _) = setup_initialized_app("test-key", "home.lan").await;

        let response = post_json(
            &app,
            "/compose/inspect",
            Some("test-key"),
            json!({"compose": "services:\n  hermes:\n    image: x\n    ports:\n      - \"8642:8642\"\n      - \"9119:9119\"\n    volumes:\n      - ~/.hermes:/opt/data\n  db:\n    image: postgres\n    volumes:\n      - pgdata:/var/lib/postgresql\n"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            parsed,
            json!({
                "services": [
                    {"name": "hermes", "image": "x", "ports": [
                        {"host": 8642, "container": 8642},
                        {"host": 9119, "container": 9119}
                    ], "volumes": [
                        {"source": "~/.hermes", "target": "/opt/data", "kind": "data", "data_path": "data/.hermes"}
                    ]},
                    {"name": "db", "image": "postgres", "ports": [], "volumes": [
                        {"source": "pgdata", "target": "/var/lib/postgresql", "kind": "named"}
                    ]}
                ],
                "web_service": "hermes",
                "web_port": 8642
            })
        );

        let response = post_json(
            &app,
            "/compose/inspect",
            Some("test-key"),
            json!({"compose": "services: ["}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn an_unsupported_compose_key_is_a_bad_request_that_names_it() {
        let (app, store) = setup_initialized_app("test-key", "home.lan").await;

        let response = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "hermes", "compose": "services:\n  h:\n    image: x\n    network_mode: host\n", "web_port": 80}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["error"], json!("invalid Compose definition"));
        assert_eq!(
            parsed["caused_by"],
            json!(["service 'h': 'network_mode' is not supported"])
        );
        assert!(store.list_applications().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn stop_start_and_restart_answer_with_the_application_as_it_stands() {
        let (app, store) = setup_initialized_app("test-key", "home.lan").await;
        post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "hermes", "compose": HERMES}),
        )
        .await;
        let record = settle(&store, "hermes").await;

        let response = post_json(
            &app,
            &format!("/apps/id/{}/stop", record.id),
            Some("test-key"),
            json!({}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["status"], json!("stopped"));
        assert_eq!(parsed["services"][0]["state"], json!("exited"));

        for verb in ["start", "restart"] {
            let response = post_json(
                &app,
                &format!("/apps/id/{}/{verb}", record.id),
                Some("test-key"),
                json!({}),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), 4096).await.unwrap();
            let parsed: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(parsed["status"], json!("running"), "{verb}");
        }

        let response = post_json(&app, "/apps/id/nope/start", Some("test-key"), json!({})).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_service_that_exited_is_reported_as_failed_and_its_logs_still_stream() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        let docker = FakeDocker::new();
        let app = build_app(
            store.clone(),
            Arc::new(docker.clone()),
            Arc::new(routes::FakeRoutes::new()),
        );
        post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "hermes", "compose": HERMES}),
        )
        .await;
        let record = settle(&store, "hermes").await;

        docker.exit_container(&format!("sf-app-{}-hermes", record.id), 137);

        let response = send(&app, "/apps", Some("test-key")).await;
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed[0]["status"], json!("failed"));
        assert_eq!(
            parsed[0]["last_error"]["error"],
            json!("the Application is not running")
        );
        assert_eq!(
            parsed[0]["last_error"]["caused_by"][0],
            json!("service 'hermes' exited with code 137 after 3 restarts")
        );
        assert_eq!(parsed[0]["services"][0]["exit_code"], json!(137));

        // What the container printed before it died is the whole story.
        let response = send(
            &app,
            &format!("/apps/id/{}/logs", record.id),
            Some("test-key"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let text = String::from_utf8_lossy(&body).to_string();
        assert!(text.contains("hermes  | [fake] log line 1"), "{text}");
        assert!(!text.contains("event: notice"));
    }

    #[tokio::test]
    async fn failed_deploy_still_records_the_application() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        let docker = FakeDocker::failing_pull("no such image: nginx:typo");
        let app = build_app(
            store.clone(),
            Arc::new(docker),
            Arc::new(routes::FakeRoutes::new()),
        );

        let response = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:typo"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);

        // The deploy failed, but the Application is on record — otherwise the
        // operator would have to retype it instead of fixing the image.
        let saved = settle(&store, "blog").await;
        assert_eq!(saved.status, apps::STATUS_FAILED);
        assert_eq!(saved.image, "nginx:typo");
        // The Platform's framing and what Docker said stay apart, so the
        // console can put one in the alert's title and the other in its body.
        let report = saved.last_error.unwrap();
        assert_eq!(report.error, "failed to deploy the Application");
        assert!(report.caused_by.iter().any(|c| c.contains("nginx:typo")));
    }

    #[tokio::test]
    async fn renaming_an_application_leaves_its_container_alone() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        let docker = FakeDocker::new();
        let apps_handle = docker.apps.clone();
        let app = build_app(
            store.clone(),
            Arc::new(docker),
            Arc::new(routes::FakeRoutes::new()),
        );

        post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        settle(&store, "blog").await;
        let before = apps_handle.lock().unwrap().len();
        let id = store
            .find_application_by_name("blog")
            .await
            .unwrap()
            .unwrap()
            .id;

        let response = put_json(
            &app,
            &format!("/apps/id/{id}"),
            Some("test-key"),
            json!({"name": "weblog"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // A rename is a row update. Docker must not have been asked to start
        // anything a second time.
        assert_eq!(apps_handle.lock().unwrap().len(), before);
        let renamed = store.get_application(&id).await.unwrap().unwrap();
        assert_eq!(renamed.name, "weblog");
        assert_eq!(renamed.status, apps::STATUS_RUNNING);
    }

    #[tokio::test]
    async fn logs_by_id_streams_the_application_container() {
        let (app, store) = setup_initialized_app("test-key", "home.lan").await;

        let _ = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        let record = settle(&store, "blog").await;
        assert_eq!(record.status, apps::STATUS_RUNNING);

        let response = send(
            &app,
            &format!("/apps/id/{}/logs", record.id),
            Some("test-key"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let text = String::from_utf8_lossy(&body).to_string();
        assert!(text.contains("[fake] log line 1"));
        assert!(text.contains("[fake] log line 3"));
        assert!(!text.contains("event: notice"));
    }

    #[tokio::test]
    async fn logs_by_id_finds_a_compose_service_by_application_identity() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        let record = apps::ApplicationRecord {
            id: "compose-app".into(),
            name: "hermes".into(),
            hostname: "hermes.home.lan".into(),
            aliases: vec![],
            image: "hermes:latest".into(),
            status: apps::STATUS_RUNNING.into(),
            source: "image".into(),
            last_error: None,
            compose: None,
            web_service: None,
            web_port: None,
            web_target_port: None,
            development: None,
        };
        store.insert_application(&record).await.unwrap();
        let docker = FakeDocker::new();
        docker
            .apps
            .lock()
            .unwrap()
            .push(crate::docker::ApplicationContainer {
                name: "sf-app-compose-app-hermes".into(),
                image: record.image.clone(),
                labels: apps::identity_labels(&record.id, &record.name),
                network: crate::docker::APP_NETWORK.into(),
                ports: vec![],
                env: vec![],
            });
        {
            let mut containers = docker.apps.lock().unwrap();
            let mut worker = containers[0].clone();
            worker.name = "sf-app-compose-app-worker".into();
            containers.push(worker);
            let mut other = containers[0].clone();
            other.name = "sf-app-other-hermes".into();
            other.labels = apps::identity_labels("other", "other");
            containers.push(other);
        }
        let app = build_app(store, Arc::new(docker), Arc::new(routes::FakeRoutes::new()));
        let response = send(&app, "/apps/id/compose-app/containers", Some("test-key")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Vec<String>>(&body).unwrap(),
            ["sf-app-compose-app-hermes", "sf-app-compose-app-worker"]
        );
        let response = send(
            &app,
            "/apps/id/compose-app/logs?container=sf-app-compose-app-worker",
            Some("test-key"),
        )
        .await;
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("from sf-app-compose-app-worker"));
        for container in ["sf-app-other-hermes", "sf-system-db"] {
            let response = send(
                &app,
                &format!("/apps/id/compose-app/logs?container={container}"),
                Some("test-key"),
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
        let response = send(&app, "/apps/id/compose-app/logs", Some("test-key")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("[fake] log line 1"), "{text}");
        assert!(!text.contains("not running"), "{text}");
    }

    #[tokio::test]
    async fn logs_by_id_says_when_there_is_nothing_to_stream() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        let docker = FakeDocker::failing_pull("no such image: nginx:typo");
        let app = build_app(
            store.clone(),
            Arc::new(docker),
            Arc::new(routes::FakeRoutes::new()),
        );

        let _ = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:typo"}),
        )
        .await;
        let record = settle(&store, "blog").await;
        assert_eq!(record.status, apps::STATUS_FAILED);

        let response = send(
            &app,
            &format!("/apps/id/{}/logs", record.id),
            Some("test-key"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let text = String::from_utf8_lossy(&body).to_string();
        assert!(text.contains("event: notice"));
        assert!(text.contains("failed to deploy"));
        assert!(!text.contains("[fake] log line"));
    }

    #[tokio::test]
    async fn logs_by_id_returns_404_for_an_unknown_id() {
        let (app, _) = setup_initialized_app("test-key", "home.lan").await;

        let response = send(&app, "/apps/id/does-not-exist/logs", Some("test-key")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// The Platform ran its state store and then its proxy in containers, and
    /// the console listed them here. It runs neither now; the endpoints that
    /// served that list are gone with it.
    #[tokio::test]
    async fn nothing_of_the_platform_runs_in_a_container_any_more() {
        let (app, _) = setup_initialized_app("test-key", "home.lan").await;

        let response = send(&app, "/system", Some("test-key")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let response = send(&app, "/system/proxy/logs", Some("test-key")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn health_returns_200_with_valid_api_key() {
        let (app, _, _) = setup_app("secret-key").await;
        let response = send(&app, "/health", Some("secret-key")).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn health_returns_401_without_api_key() {
        let (app, _, _) = setup_app("secret-key").await;
        let response = send(&app, "/health", None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn health_returns_401_with_invalid_api_key() {
        let (app, _, _) = setup_app("secret-key").await;
        let response = send(&app, "/health", Some("wrong-key")).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn health_returns_json_status_ok_with_valid_api_key() {
        let (app, _, _) = setup_app("secret-key").await;
        let response = send(&app, "/health", Some("secret-key")).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed, json!({"status": "ok"}));
    }

    #[tokio::test]
    async fn unknown_route_returns_401_without_api_key() {
        let (app, _, _) = setup_app("secret-key").await;
        let response = send(&app, "/nonexistent", None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn unknown_route_returns_404_with_valid_api_key() {
        let (app, _, _) = setup_app("secret-key").await;
        let response = send(&app, "/nonexistent", Some("secret-key")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn constant_time_eq_matches_identical_strings() {
        assert!(constant_time_eq("secret", "secret"));
    }

    #[test]
    fn constant_time_eq_rejects_different_strings() {
        assert!(!constant_time_eq("secret", "wrong"));
    }

    #[test]
    fn constant_time_eq_rejects_different_lengths() {
        assert!(!constant_time_eq("short", "longer"));
    }

    #[tokio::test]
    async fn bootstrap_status_returns_initialized_false_when_not_initialized() {
        let store = FakeStateStore::new();
        let docker = FakeDocker::new();
        let app = build_app(
            store.clone(),
            Arc::new(docker),
            Arc::new(routes::FakeRoutes::new()),
        );

        // Store an API key so auth passes
        store.store_state("api_key", "test-key").await.unwrap();

        let response = send(&app, "/bootstrap/status", Some("test-key")).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["initialized"], json!(false));
        assert_eq!(parsed["dns_suffix"], json!(null));
        assert_eq!(parsed.get("host_ip"), Some(&Value::Null));
    }

    #[tokio::test]
    async fn bootstrap_status_returns_initialized_true_with_dns_suffix() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();

        let docker = FakeDocker::new();
        let app = build_app(store, Arc::new(docker), Arc::new(routes::FakeRoutes::new()));

        let response = send(&app, "/bootstrap/status", Some("test-key")).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["initialized"], json!(true));
        assert_eq!(parsed["dns_suffix"], json!("home.lan"));
        assert_eq!(parsed.get("host_ip"), Some(&Value::Null));
    }

    #[tokio::test]
    async fn bootstrap_status_returns_401_without_api_key() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        let docker = FakeDocker::new();
        let app = build_app(store, Arc::new(docker), Arc::new(routes::FakeRoutes::new()));

        let response = send(&app, "/bootstrap/status", None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn bootstrap_init_persists_state_and_is_visible_through_api() {
        let store = FakeStateStore::new();

        let result = bootstrap::BootstrapResult {
            dns_suffix: "myhost.lan".into(),
            api_key: "generated-key".into(),
            api_listen_addr: "192.168.1.100:3721".into(),
            host_ip: "192.168.1.100".into(),
            host_addresses: vec!["192.168.1.100".parse().unwrap()],
            execution_unavailable: None,
        };

        bootstrap::persist_bootstrap_state(&store, &result)
            .await
            .expect("persist should succeed");

        let docker = FakeDocker::new();
        let app = build_app(store, Arc::new(docker), Arc::new(routes::FakeRoutes::new()));

        let response = send(&app, "/bootstrap/status", Some("generated-key")).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["initialized"], json!(true));
        assert_eq!(parsed["dns_suffix"], json!("myhost.lan"));
        assert_eq!(parsed["host_ip"], json!("192.168.1.100"));
    }

    #[tokio::test]
    async fn persist_bootstrap_state_fails_when_already_initialized() {
        let store = FakeStateStore::new();

        let result1 = bootstrap::BootstrapResult {
            dns_suffix: "first.lan".into(),
            api_key: "key1".into(),
            api_listen_addr: "127.0.0.1:3721".into(),
            host_ip: "127.0.0.1".into(),
            host_addresses: vec![],
            execution_unavailable: None,
        };

        bootstrap::persist_bootstrap_state(&store, &result1)
            .await
            .expect("first persist should succeed");

        let result2 = bootstrap::BootstrapResult {
            dns_suffix: "second.lan".into(),
            api_key: "key2".into(),
            api_listen_addr: "127.0.0.1:3721".into(),
            host_ip: "127.0.0.1".into(),
            host_addresses: vec![],
            execution_unavailable: None,
        };

        let err = bootstrap::persist_bootstrap_state(&store, &result2)
            .await
            .expect_err("second persist should fail");

        assert!(matches!(err, bootstrap::BootstrapError::AlreadyInitialized));
    }

    #[tokio::test]
    async fn deploy_application_from_image_returns_hostname_under_dns_suffix() {
        let (app, store) = setup_initialized_app("test-key", "home.lan").await;

        let response = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;

        // Accepted, not done: the answer comes back with the row as written,
        // before Docker has been asked for anything.
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["name"], json!("blog"));
        assert_eq!(parsed["hostname"], json!("blog.home.lan"));
        assert_eq!(parsed["image"], json!("nginx:alpine"));
        assert_eq!(parsed["status"], json!("pending"));

        assert_eq!(settle(&store, "blog").await.status, apps::STATUS_RUNNING);
    }

    #[tokio::test]
    async fn deploy_application_accepts_hostname_aliases() {
        let (app, store) = setup_initialized_app("test-key", "home.lan").await;

        let response = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({
                "name": "blog",
                "image": "nginx:alpine",
                "hostname": "writing.home.lan",
                "aliases": ["blog.home.lan"]
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["hostname"], json!("writing.home.lan"));
        assert_eq!(parsed["aliases"], json!(["blog.home.lan"]));
        assert_eq!(settle(&store, "blog").await.status, apps::STATUS_RUNNING);
    }

    #[tokio::test]
    async fn deploy_rejects_an_alias_answered_by_another_application() {
        let (app, store) = setup_initialized_app("test-key", "home.lan").await;

        let first = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        assert_eq!(first.status(), StatusCode::ACCEPTED);
        settle(&store, "blog").await;

        let response = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({
                "name": "notes",
                "image": "nginx:alpine",
                "aliases": ["blog.home.lan"]
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            parsed["error"],
            json!(
                "invalid Application Hostname: 'blog.home.lan' is already answered by Application 'blog'"
            )
        );
    }

    #[tokio::test]
    async fn list_applications_includes_deployed_app() {
        let (app, store) = setup_initialized_app("test-key", "home.lan").await;

        let deploy = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        assert_eq!(deploy.status(), StatusCode::ACCEPTED);
        settle(&store, "blog").await;

        let response = send(&app, "/apps", Some("test-key")).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let mut parsed: Value = serde_json::from_slice(&body).unwrap();

        // The id is random; assert it is there and shaped right, then drop it
        // so the rest of the payload can be compared literally.
        let id = parsed[0]["id"].take();
        assert_eq!(id.as_str().unwrap().len(), 12);
        parsed[0].as_object_mut().unwrap().remove("id");

        assert_eq!(
            parsed,
            json!([{
                "name": "blog",
                "hostname": "blog.home.lan",
                "aliases": [],
                "image": "nginx:alpine",
                "status": "running",
                "source": "image",
                "restarts": 0,
                "services": [{
                    "service": "app",
                    "container": format!("sf-app-{}", id.as_str().unwrap()),
                    "state": "running",
                    "exit_code": 0,
                    "restarts": 0
                }]
            }])
        );
    }

    #[tokio::test]
    async fn deploy_duplicate_application_name_returns_conflict() {
        let (app, _) = setup_initialized_app("test-key", "home.lan").await;

        let first = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        assert_eq!(first.status(), StatusCode::ACCEPTED);

        let second = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        assert_eq!(second.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn deploy_requires_api_key() {
        let (app, _) = setup_initialized_app("test-key", "home.lan").await;

        let response = post_json(
            &app,
            "/apps",
            None,
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn deploy_publishes_the_web_target_on_loopback_only() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        let docker = FakeDocker::new();
        let route_store = Arc::new(routes::FakeRoutes::new());
        let app = build_app(store.clone(), Arc::new(docker.clone()), route_store.clone());

        let response = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        settle(&store, "blog").await;

        assert_eq!(docker.pulled.lock().unwrap().as_slice(), ["nginx:alpine"]);

        let deployed_app = store
            .find_application_by_name("blog")
            .await
            .unwrap()
            .unwrap();
        let id = deployed_app.id.clone();
        let host_port = deployed_app.web_target_port.expect("a Web Target port");

        {
            let deployed = docker.apps.lock().unwrap();
            assert_eq!(deployed.len(), 1);
            // The Web Target is reachable by the proxy on the Host, and by
            // nothing on the LAN.
            assert_eq!(deployed[0].ports, [format!("127.0.0.1:{host_port}:80")]);
            // Routing is decided by the route table, not a label on the
            // container.
            assert!(
                !deployed[0]
                    .labels
                    .iter()
                    .any(|(k, _)| k.starts_with("traefik."))
            );
        }
        let published = route_store.get(&id).unwrap();
        assert!(published.answers_on("blog.home.lan"));
        assert_eq!(
            published.target,
            Some(std::net::SocketAddr::from(([127, 0, 0, 1], host_port)))
        );
    }

    #[tokio::test]
    async fn each_application_keeps_its_own_web_target_port_across_redeploys() {
        let (app, store) = setup_initialized_app("test-key", "home.lan").await;

        for name in ["blog", "wiki"] {
            post_json(
                &app,
                "/apps",
                Some("test-key"),
                json!({"name": name, "image": "nginx:alpine"}),
            )
            .await;
            settle(&store, name).await;
        }

        let blog = store
            .find_application_by_name("blog")
            .await
            .unwrap()
            .unwrap();
        let wiki = store
            .find_application_by_name("wiki")
            .await
            .unwrap()
            .unwrap();
        let blog_port = blog.web_target_port.expect("a Web Target port");
        assert_ne!(blog_port, wiki.web_target_port.unwrap());

        // A redeploy is the same Application at the same address. Nothing
        // outside the Platform depends on the number, but an Operator reading
        // `docker ps` should not find it different every time.
        put_json(
            &app,
            &format!("/apps/id/{}", blog.id),
            Some("test-key"),
            json!({"image": "nginx:latest"}),
        )
        .await;
        settle(&store, "blog").await;
        assert_eq!(
            store
                .get_application(&blog.id)
                .await
                .unwrap()
                .unwrap()
                .web_target_port,
            Some(blog_port)
        );
    }

    #[tokio::test]
    async fn deploy_duplicate_returns_clear_error_message() {
        let (app, _) = setup_initialized_app("test-key", "home.lan").await;

        let _ = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;

        let second = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        assert_eq!(second.status(), StatusCode::ACCEPTED);
        let body = to_bytes(second.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["name"], json!("blog"));
        assert_eq!(parsed["source"], json!("image"));
    }

    #[tokio::test]
    async fn two_apps_on_different_hostnames_do_not_interfere() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        let docker = FakeDocker::new();
        let app = build_app(
            store.clone(),
            Arc::new(docker.clone()),
            Arc::new(routes::FakeRoutes::new()),
        );

        // Deploy two apps
        let r1 = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        assert_eq!(r1.status(), StatusCode::ACCEPTED);

        let r2 = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "files", "image": "filebrowser/filebrowser"}),
        )
        .await;
        assert_eq!(r2.status(), StatusCode::ACCEPTED);
        settle(&store, "blog").await;
        settle(&store, "files").await;

        // Both appear in list
        let list = send(&app, "/apps", Some("test-key")).await;
        let body = to_bytes(list.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        let names: Vec<&str> = parsed
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["blog", "files"]);

        // Each has its own hostname
        let hostnames: Vec<&str> = parsed
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["hostname"].as_str().unwrap())
            .collect();
        assert_eq!(hostnames, vec!["blog.home.lan", "files.home.lan"]);
    }

    #[tokio::test]
    async fn remove_application_returns_204_and_cleans_docker() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        let docker = FakeDocker::new();
        let app = build_app(
            store.clone(),
            Arc::new(docker.clone()),
            Arc::new(routes::FakeRoutes::new()),
        );

        let deploy = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        assert_eq!(deploy.status(), StatusCode::ACCEPTED);
        settle(&store, "blog").await;

        let response = delete_req(&app, "/apps/blog", Some("test-key")).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let list = send(&app, "/apps", Some("test-key")).await;
        let body = to_bytes(list.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed, json!([]));
    }

    #[tokio::test]
    async fn remove_nonexistent_application_returns_404() {
        let (app, _) = setup_initialized_app("test-key", "home.lan").await;

        let response = delete_req(&app, "/apps/nonexistent", Some("test-key")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_failure_answers_with_the_error_and_its_causes() {
        let (app, _) = setup_initialized_app("test-key", "home.lan").await;

        let response = delete_req(&app, "/apps/nonexistent", Some("test-key")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            parsed,
            json!({"error": "Application 'nonexistent' not found", "caused_by": []})
        );
    }

    /// Every name the Platform once reserved belonged to a container it ran
    /// for itself. It runs none, so an Application may be called anything and
    /// a name nothing answers to is simply not found.
    #[tokio::test]
    async fn no_application_name_is_reserved_any_more() {
        let (app, _) = setup_initialized_app("test-key", "home.lan").await;

        for name in &["traefik", "postgres", "coredns"] {
            let response = delete_req(&app, &format!("/apps/{name}"), Some("test-key")).await;
            assert_eq!(
                response.status(),
                StatusCode::NOT_FOUND,
                "'{name}' is still reserved for Platform Infra that no longer exists"
            );
        }
    }

    #[tokio::test]
    async fn remove_requires_api_key() {
        let (app, _) = setup_initialized_app("test-key", "home.lan").await;

        let response = delete_req(&app, "/apps/blog", None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn remove_cleans_container_from_docker() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        let docker = FakeDocker::new();
        let app = build_app(
            store.clone(),
            Arc::new(docker.clone()),
            Arc::new(routes::FakeRoutes::new()),
        );

        let _ = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        settle(&store, "blog").await;

        let deployed = docker.deployed_apps();
        assert_eq!(deployed.len(), 1);
        assert!(deployed[0].starts_with(apps::APP_PREFIX));

        let response = delete_req(&app, "/apps/blog", Some("test-key")).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        assert!(docker.deployed_apps().is_empty());
    }

    #[tokio::test]
    async fn remove_not_initialized_returns_412() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        let docker = FakeDocker::new();
        let app = build_app(store, Arc::new(docker), Arc::new(routes::FakeRoutes::new()));

        let response = delete_req(&app, "/apps/blog", Some("test-key")).await;
        assert_eq!(response.status(), StatusCode::PRECONDITION_FAILED);
    }
}

#[cfg(test)]
mod bootstrap_tests {
    use super::bootstrap;
    use super::docker::FakeDocker;

    #[test]
    fn validate_dns_suffix_rejects_local() {
        assert!(bootstrap::validate_dns_suffix(".local").is_err());
        assert!(bootstrap::validate_dns_suffix("my.local").is_err());
        assert!(bootstrap::validate_dns_suffix("local").is_err());
    }

    #[test]
    fn validate_dns_suffix_rejects_empty() {
        assert!(bootstrap::validate_dns_suffix("").is_err());
    }

    #[test]
    fn validate_dns_suffix_rejects_bare_name() {
        assert!(bootstrap::validate_dns_suffix("lan").is_err());
    }

    #[test]
    fn validate_dns_suffix_accepts_valid_suffixes() {
        assert!(bootstrap::validate_dns_suffix("home.lan").is_ok());
        assert!(bootstrap::validate_dns_suffix("myhost.internal").is_ok());
        assert!(bootstrap::validate_dns_suffix("example.com").is_ok());
    }

    #[test]
    fn default_dns_suffix_is_home_lan() {
        assert_eq!(bootstrap::DEFAULT_DNS_SUFFIX, "home.lan");
    }

    #[tokio::test]
    async fn run_bootstrap_with_fake_docker_succeeds() {
        let docker = FakeDocker::new();
        // An explicit address, so the test does not depend on the machine
        // having a LAN one to detect.
        let result =
            bootstrap::run_bootstrap(&docker, bootstrap::DEFAULT_DNS_SUFFIX, Some("192.168.1.10"))
                .await
                .expect("bootstrap should succeed with fake docker");

        assert_eq!(result.dns_suffix, bootstrap::DEFAULT_DNS_SUFFIX);
        assert_eq!(result.host_ip, "192.168.1.10");
        let dns = crate::dns::Config::load().expect("Bootstrap saves DNS configuration");
        assert_eq!(dns.dns_suffix, result.dns_suffix);
        assert!(
            dns.host_addresses
                .include
                .contains(&"192.168.1.10".parse().unwrap())
        );
        assert!(result.api_key.len() >= 64); // 32 bytes = 64 hex chars
        assert!(result.api_listen_addr.contains(":3721"));
    }

    #[tokio::test]
    async fn run_bootstrap_rejects_a_host_ip_that_is_not_one() {
        let docker = FakeDocker::new();
        let err = bootstrap::run_bootstrap(&docker, "test.lan", Some("the-mac"))
            .await
            .expect_err("should reject a name");

        assert!(matches!(err, bootstrap::BootstrapError::InvalidHostIp(_)));
    }

    #[tokio::test]
    async fn run_bootstrap_rejects_invalid_dns_suffix() {
        let docker = FakeDocker::new();
        let err = bootstrap::run_bootstrap(&docker, ".local", None)
            .await
            .expect_err("should reject .local");

        assert!(matches!(
            err,
            bootstrap::BootstrapError::InvalidDnsSuffix(_)
        ));
    }
}

#[cfg(test)]
mod deploy_path_tests {
    use super::*;
    use crate::docker::FakeDocker;
    use crate::store::FakeStateStore;
    use axum::body::to_bytes;
    use axum::http::StatusCode;
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn post_json(
        app: &axum::Router,
        uri: &str,
        api_key: Option<&str>,
        body: Value,
    ) -> axum::response::Response {
        use axum::body::Body;
        use axum::http::{Request, header};

        let mut req = Request::builder()
            .method("POST")
            .uri(uri)
            .header("Content-Type", "application/json");
        if let Some(key) = api_key {
            req = req.header(header::AUTHORIZATION, format!("Bearer {key}"));
        }
        app.clone()
            .oneshot(
                req.body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn deploy_application_from_path_builds_and_runs() {
        let store = FakeStateStore::new();
        store.store_state("api_key", "test-key").await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        let docker = FakeDocker::new();
        let app = build_app(
            store.clone(),
            Arc::new(docker.clone()),
            Arc::new(routes::FakeRoutes::new()),
        );

        let response = post_json(
            &app,
            "/apps",
            Some("test-key"),
            json!({"name": "api", "path": "./myapp"}),
        )
        .await;

        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["name"], json!("api"));
        assert_eq!(parsed["hostname"], json!("api.home.lan"));
        assert_eq!(parsed["image"], json!("self-host-api:latest"));
        assert_eq!(parsed["status"], json!("pending"));

        // The build runs on a task; wait for it before asking Docker what it
        // was told to do.
        for _ in 0..200 {
            let app = store.find_application_by_name("api").await.unwrap();
            if app.is_some_and(|a| a.status == apps::STATUS_RUNNING) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        let built = docker.built.lock().unwrap();
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].0, "./myapp");
        assert_eq!(built[0].1, "self-host-api:latest");

        let deployed = docker.apps.lock().unwrap();
        assert_eq!(deployed.len(), 1);
        assert_eq!(deployed[0].image, "self-host-api:latest");
    }
}
