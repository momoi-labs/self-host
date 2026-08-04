use crate::db::{DbError, StateStore};
use crate::docker::{ApplicationContainer, DockerError, DockerRuntime, PLATFORM_NETWORK};

pub const APP_CONTAINER_PORT: u16 = 80;

pub use crate::db::ApplicationRecord;

#[derive(Debug)]
pub enum DeployError {
    NotInitialized,
    AlreadyExists(String),
    InvalidName(String),
    MissingImage,
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

    if store.application_exists(name).await? {
        return Err(DeployError::AlreadyExists(name.to_string()));
    }

    let hostname = default_hostname(name, &dns_suffix);

    docker.ensure_network(PLATFORM_NETWORK).await?;
    docker.pull_image(image).await?;

    let container_name = container_name_for(name);
    docker
        .run_application(ApplicationContainer {
            name: container_name.clone(),
            image: image.to_string(),
            labels: traefik_labels(name, &hostname),
            network: PLATFORM_NETWORK.to_string(),
            // Consumer traffic goes through Traefik — never publish host ports.
            ports: vec![],
        })
        .await?;

    let record = ApplicationRecord {
        name: name.to_string(),
        hostname,
        image: image.to_string(),
        status: "running".into(),
    };

    if let Err(e) = store.insert_application(&record).await {
        let _ = docker.remove_container(&container_name).await;
        return Err(match e {
            DbError::AlreadyExists(name) => DeployError::AlreadyExists(name),
            other => DeployError::Db(other),
        });
    }

    Ok(record)
}

pub async fn list_applications(
    store: &impl StateStore,
) -> Result<Vec<ApplicationRecord>, DeployError> {
    Ok(store.list_applications().await?)
}

pub fn container_name_for(app_name: &str) -> String {
    format!("self-host-app-{app_name}")
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
