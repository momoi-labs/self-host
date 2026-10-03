//! A running process is not evidence that its declared health check passed.

use crate::{
    apps::{self, ServiceState},
    docker::DockerRuntime,
    store::ApplicationRecord,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Readiness {
    Ready,
    Checking,
    Failed,
    Unknown,
}

pub fn observed(services: &[ServiceState]) -> Readiness {
    if services.is_empty() {
        return Readiness::Unknown;
    }
    if services.iter().any(|s| {
        s.health.as_deref() == Some("unhealthy") || matches!(s.state.as_str(), "exited" | "dead")
    }) {
        return Readiness::Failed;
    }
    if services
        .iter()
        .any(|s| s.state != "running" || s.health.as_deref() == Some("starting"))
    {
        return Readiness::Checking;
    }
    if services
        .iter()
        .all(|s| s.health.as_deref() == Some("healthy"))
    {
        Readiness::Ready
    } else {
        Readiness::Unknown
    }
}

/// A container without a healthcheck must be running, but remains unverified.
/// Only declared checks gate a deployment. The loop is bounded even if Docker
/// cannot report a state. One-shot workloads are not supported here.
pub async fn wait(
    docker: &(impl DockerRuntime + ?Sized),
    app: &ApplicationRecord,
    timeout: Duration,
) -> Result<Readiness, String> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let services = tokio::time::timeout(remaining, apps::service_states(docker, app))
            .await
            .map_err(|_| "deployment health checks timed out".to_owned())?;
        let readiness = observed(&services);
        if readiness == Readiness::Failed {
            let failed = services
                .iter()
                .filter(|s| s.health.as_deref() == Some("unhealthy") || s.state != "running")
                .map(|s| {
                    format!(
                        "{}: {}{}",
                        s.service,
                        s.state,
                        s.health
                            .as_ref()
                            .map(|h| format!(" ({h})"))
                            .unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!("deployment health check failed: {failed}"));
        }
        if !services.is_empty()
            && services.iter().all(|s| s.state == "running")
            && matches!(readiness, Readiness::Ready | Readiness::Unknown)
        {
            return Ok(readiness);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("deployment health checks timed out".into());
        }
        tokio::time::sleep(
            Duration::from_millis(250)
                .min(deadline.saturating_duration_since(tokio::time::Instant::now())),
        )
        .await;
    }
}
