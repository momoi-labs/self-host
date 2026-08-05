use crate::db::{DbError, StateStore};
use crate::docker::{ApplicationContainer, DockerError, DockerRuntime, PLATFORM_NETWORK};

pub const APP_CONTAINER_PORT: u16 = 80;

pub use crate::db::ApplicationRecord;

/// Names reserved for Platform Infra — remove must reject these.
const PROTECTED_NAMES: &[&str] = &["postgres", "dnsmasq", "traefik"];

#[derive(Debug)]
pub enum DeployError {
    NotInitialized,
    AlreadyExists(String),
    InvalidName(String),
    MissingImage,
    MissingPath,
    Docker(DockerError),
    Db(DbError),
}

impl std::fmt::Display for DeployError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeployError::NotInitialized => {
                write!(f, "platform is not initialized; run 'self-host init' first")
            }
            DeployError::AlreadyExists(name) => {
                write!(f, "Application '{name}' already exists")
            }
            DeployError::InvalidName(msg) => write!(f, "invalid Application name: {msg}"),
            DeployError::MissingImage => write!(f, "image is required"),
            DeployError::MissingPath => write!(f, "path is required"),
            DeployError::Docker(e) => write!(f, "{e}"),
            DeployError::Db(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DeployError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DeployError::Docker(e) => Some(e),
            DeployError::Db(e) => Some(e),
            _ => None,
        }
    }
}

impl From<DockerError> for DeployError {
    fn from(e: DockerError) -> Self {
        DeployError::Docker(e)
    }
}

impl From<DbError> for DeployError {
    fn from(e: DbError) -> Self {
        DeployError::Db(e)
    }
}

pub fn validate_app_name(name: &str) -> Result<(), DeployError> {
    if name.is_empty() {
        return Err(DeployError::InvalidName("must not be empty".into()));
    }
    if name.len() > 63 {
        return Err(DeployError::InvalidName(
            "must be at most 63 characters".into(),
        ));
    }
    let valid = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        return Err(DeployError::InvalidName(
            "must be lowercase alphanumeric and hyphens".into(),
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(DeployError::InvalidName(
            "must not start or end with a hyphen".into(),
        ));
    }
    Ok(())
}

pub fn default_hostname(name: &str, dns_suffix: &str) -> String {
    format!("{name}.{dns_suffix}")
}

pub fn traefik_labels(name: &str, hostname: &str) -> Vec<(String, String)> {
    vec![
        ("traefik.enable".into(), "true".into()),
        (
            format!("traefik.http.routers.{name}.rule"),
            format!("Host(`{hostname}`)"),
        ),
        (
            format!("traefik.http.routers.{name}.entrypoints"),
            "web".into(),
        ),
        (
            format!("traefik.http.services.{name}.loadbalancer.server.port"),
            APP_CONTAINER_PORT.to_string(),
        ),
    ]
}

pub async fn deploy_from_image(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    name: &str,
    image: &str,
    hostname_override: Option<&str>,
) -> Result<ApplicationRecord, DeployError> {
    validate_app_name(name)?;

    if image.is_empty() {
        return Err(DeployError::MissingImage);
    }

    if !store.is_initialized().await? {
        return Err(DeployError::NotInitialized);
    }

    let dns_suffix = store
        .get_state("dns_suffix")
        .await?
        .ok_or(DeployError::NotInitialized)?;

    let hostname = hostname_override
        .map(|h| h.to_string())
        .unwrap_or_else(|| default_hostname(name, &dns_suffix));

    let app_existed = store.application_exists(name).await?;

    docker.ensure_network(PLATFORM_NETWORK).await?;
    docker.pull_image(image).await?;

    let container_name = container_name_for(name);
    docker
        .run_application(ApplicationContainer {
            name: container_name.clone(),
            image: image.to_string(),
            labels: traefik_labels(name, &hostname),
            network: PLATFORM_NETWORK.to_string(),
            ports: vec![],
            env: if app_existed {
                store
                    .get_all_env(name)
                    .await?
                    .into_iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect()
            } else {
                vec![]
            },
        })
        .await?;

    let record = ApplicationRecord {
        name: name.to_string(),
        hostname,
        image: image.to_string(),
        status: "running".into(),
        source: "image".into(),
    };

    store.insert_application(&record).await?;

    Ok(record)
}

pub async fn list_applications(
    store: &impl StateStore,
) -> Result<Vec<ApplicationRecord>, DeployError> {
    Ok(store.list_applications().await?)
}

pub async fn deploy_from_path(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    name: &str,
    path: &str,
    hostname_override: Option<&str>,
) -> Result<ApplicationRecord, DeployError> {
    validate_app_name(name)?;

    if path.is_empty() {
        return Err(DeployError::MissingPath);
    }

    if !store.is_initialized().await? {
        return Err(DeployError::NotInitialized);
    }

    let dns_suffix = store
        .get_state("dns_suffix")
        .await?
        .ok_or(DeployError::NotInitialized)?;

    let hostname = hostname_override
        .map(|h| h.to_string())
        .unwrap_or_else(|| default_hostname(name, &dns_suffix));

    let image_tag = format!("self-host-{name}:latest");
    let app_existed = store.application_exists(name).await?;

    docker.ensure_network(PLATFORM_NETWORK).await?;
    docker.build_image(path, &image_tag).await?;

    let container_name = container_name_for(name);
    docker
        .run_application(ApplicationContainer {
            name: container_name.clone(),
            image: image_tag.clone(),
            labels: traefik_labels(name, &hostname),
            network: PLATFORM_NETWORK.to_string(),
            ports: vec![],
            env: if app_existed {
                store
                    .get_all_env(name)
                    .await?
                    .into_iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect()
            } else {
                vec![]
            },
        })
        .await?;

    let record = ApplicationRecord {
        name: name.to_string(),
        hostname,
        image: image_tag,
        status: "running".into(),
        source: "path".into(),
    };

    store.insert_application(&record).await?;

    Ok(record)
}

pub fn container_name_for(app_name: &str) -> String {
    format!("self-host-app-{app_name}")
}

#[derive(Debug)]
pub enum RemoveError {
    NotInitialized,
    NotFound(String),
    ProtectedName(String),
    Docker(DockerError),
    Db(DbError),
}

impl std::fmt::Display for RemoveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoveError::NotInitialized => {
                write!(f, "platform is not initialized; run 'self-host init' first")
            }
            RemoveError::NotFound(name) => write!(f, "Application '{name}' not found"),
            RemoveError::ProtectedName(name) => {
                write!(
                    f,
                    "'{name}' is Platform Infra and cannot be removed as an Application"
                )
            }
            RemoveError::Docker(e) => write!(f, "{e}"),
            RemoveError::Db(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RemoveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RemoveError::Docker(e) => Some(e),
            RemoveError::Db(e) => Some(e),
            _ => None,
        }
    }
}

impl From<DockerError> for RemoveError {
    fn from(e: DockerError) -> Self {
        RemoveError::Docker(e)
    }
}

impl From<DbError> for RemoveError {
    fn from(e: DbError) -> Self {
        match e {
            DbError::NotFound(name) => RemoveError::NotFound(name),
            other => RemoveError::Db(other),
        }
    }
}

pub async fn remove_application(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    name: &str,
) -> Result<(), RemoveError> {
    if PROTECTED_NAMES.contains(&name) {
        return Err(RemoveError::ProtectedName(name.to_string()));
    }

    if !store.is_initialized().await? {
        return Err(RemoveError::NotInitialized);
    }

    store.delete_application(name).await?;

    let container_name = container_name_for(name);
    let _ = docker.remove_container(&container_name).await;

    Ok(())
}

// ── Env ────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum EnvError {
    NotInitialized,
    NotFound(String),
    Docker(DockerError),
    Db(DbError),
}

impl std::fmt::Display for EnvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnvError::NotInitialized => {
                write!(f, "platform is not initialized; run 'self-host init' first")
            }
            EnvError::NotFound(name) => write!(f, "Application '{name}' not found"),
            EnvError::Docker(e) => write!(f, "{e}"),
            EnvError::Db(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for EnvError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EnvError::Docker(e) => Some(e),
            EnvError::Db(e) => Some(e),
            _ => None,
        }
    }
}

impl From<DockerError> for EnvError {
    fn from(e: DockerError) -> Self {
        EnvError::Docker(e)
    }
}

impl From<DbError> for EnvError {
    fn from(e: DbError) -> Self {
        EnvError::Db(e)
    }
}

pub async fn set_env(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    app_name: &str,
    key: &str,
    value: &str,
) -> Result<(), EnvError> {
    if !store.is_initialized().await? {
        return Err(EnvError::NotInitialized);
    }

    if !store.application_exists(app_name).await? {
        return Err(EnvError::NotFound(app_name.to_string()));
    }

    store.set_env(app_name, key, value).await?;

    // Apply: recreate container with updated env
    recreate_with_env(store, docker, app_name).await?;

    Ok(())
}

pub async fn get_all_env(
    store: &impl StateStore,
    app_name: &str,
) -> Result<Vec<(String, String)>, EnvError> {
    if !store.application_exists(app_name).await? {
        return Err(EnvError::NotFound(app_name.to_string()));
    }
    Ok(store.get_all_env(app_name).await?)
}

pub async fn unset_env(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    app_name: &str,
    key: &str,
) -> Result<(), EnvError> {
    if !store.is_initialized().await? {
        return Err(EnvError::NotInitialized);
    }

    if !store.application_exists(app_name).await? {
        return Err(EnvError::NotFound(app_name.to_string()));
    }

    store.unset_env(app_name, key).await?;

    // Apply: recreate container with updated env
    recreate_with_env(store, docker, app_name).await?;

    Ok(())
}

async fn recreate_with_env(
    store: &impl StateStore,
    docker: &(impl DockerRuntime + ?Sized),
    app_name: &str,
) -> Result<(), EnvError> {
    let apps = store.list_applications().await?;
    let app = apps
        .iter()
        .find(|a| a.name == app_name)
        .ok_or_else(|| EnvError::NotFound(app_name.to_string()))?;

    let env_vars = store
        .get_all_env(app_name)
        .await?
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>();

    let hostname = &app.hostname;
    let container_name = container_name_for(app_name);

    docker.remove_container(&container_name).await?;
    docker
        .run_application(ApplicationContainer {
            name: container_name,
            image: app.image.clone(),
            labels: traefik_labels(app_name, hostname),
            network: PLATFORM_NETWORK.to_string(),
            ports: vec![],
            env: env_vars,
        })
        .await?;

    Ok(())
}

// ── Logs ───────────────────────────────────────────────────────

#[derive(Debug)]
pub enum LogsError {
    NotInitialized,
    NotFound(String),
    Docker(DockerError),
    Db(DbError),
}

impl std::fmt::Display for LogsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogsError::NotInitialized => {
                write!(f, "platform is not initialized; run 'self-host init' first")
            }
            LogsError::NotFound(name) => write!(f, "Application '{name}' not found"),
            LogsError::Docker(e) => write!(f, "{e}"),
            LogsError::Db(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LogsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LogsError::Docker(e) => Some(e),
            LogsError::Db(e) => Some(e),
            _ => None,
        }
    }
}

impl From<DockerError> for LogsError {
    fn from(e: DockerError) -> Self {
        LogsError::Docker(e)
    }
}

impl From<DbError> for LogsError {
    fn from(e: DbError) -> Self {
        LogsError::Db(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_hostname_uses_dns_suffix() {
        assert_eq!(default_hostname("blog", "home.lan"), "blog.home.lan");
    }

    #[test]
    fn traefik_labels_enable_host_routing_without_implying_host_ports() {
        let labels = traefik_labels("blog", "blog.home.lan");
        assert!(
            labels
                .iter()
                .any(|(k, v)| k == "traefik.enable" && v == "true")
        );
        assert!(labels.iter().any(|(k, v)| {
            k == "traefik.http.routers.blog.rule" && v == "Host(`blog.home.lan`)"
        }));
    }

    #[test]
    fn validate_app_name_rejects_uppercase() {
        assert!(validate_app_name("Blog").is_err());
    }

    #[test]
    fn validate_app_name_accepts_simple_name() {
        assert!(validate_app_name("blog").is_ok());
    }
}
