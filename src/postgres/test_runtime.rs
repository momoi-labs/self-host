//! Fake Docker with an explicit pause at the destructive provider boundary.

use std::path::Path;

use async_trait::async_trait;
use tokio::sync::{Notify, mpsc};

use crate::{compose_app::ComposeProject, docker::*};

#[derive(Default)]
pub(super) struct PausedRemoval {
    pub inner: FakeDocker,
    pub entered: Notify,
    pub release: Notify,
    pub postgres_ready: bool,
    pub postgres_failure: Option<String>,
    pub requests: std::sync::Mutex<Vec<super::runtime::Request>>,
    pub pause_restore: bool,
    pub restore_entered: Notify,
    pub restore_release: Notify,
}

#[async_trait]
impl DockerRuntime for PausedRemoval {
    async fn postgres(&self, request: &super::runtime::Request) -> Result<String, DockerError> {
        use super::runtime::Request;
        if !self.postgres_ready {
            return self.inner.postgres(request).await;
        }
        if self.pause_restore && matches!(request, Request::Restore { .. }) {
            self.restore_entered.notify_one();
            self.restore_release.notified().await;
        }
        if let Request::Sql { sql, .. } = request {
            if sql == "SELECT 1;" {
                return Ok("1".into());
            }
            if let Some(error) = &self.postgres_failure {
                return Err(DockerError::Unavailable(error.clone()));
            }
        }
        let mut requests = self.requests.lock().unwrap();
        let restored = requests
            .iter()
            .any(|request| matches!(request, Request::Restore { .. }));
        requests.push(request.clone());
        Ok(match request {
            Request::Sql { sql, .. } if sql.starts_with("SELECT count(*)") => {
                if restored { "3" } else { "0" }.into()
            }
            Request::InspectArchive { .. } => {
                "; Dumped from database version: 17.6\n; Dumped by pg_dump version: 17.6".into()
            }
            _ => String::new(),
        })
    }
    async fn build_source(&self, build: &SourceBuild) -> Result<String, DockerError> {
        self.inner.build_source(build).await
    }
    async fn pin_source_image(
        &self,
        image: &str,
        registry_config: Option<&Path>,
    ) -> Result<String, DockerError> {
        self.inner.pin_source_image(image, registry_config).await
    }
    async fn open_terminal(
        &self,
        container: &str,
        size: crate::terminal::Size,
        user: Option<&str>,
    ) -> Result<crate::terminal::Session, DockerError> {
        self.inner.open_terminal(container, size, user).await
    }

    async fn container_images(&self) -> Result<Vec<String>, DockerError> {
        self.inner.container_images().await
    }

    async fn remove_image_repository(&self, repository: &str) -> Result<(), DockerError> {
        self.inner.remove_image_repository(repository).await
    }

    async fn ping(&self) -> Result<(), DockerError> {
        self.inner.ping().await
    }

    async fn container_running(&self, name: &str) -> Result<bool, DockerError> {
        self.inner.container_running(name).await
    }

    async fn application_containers(&self, id: &str) -> Result<Vec<String>, DockerError> {
        self.inner.application_containers(id).await
    }

    async fn restart_count(&self, name: &str) -> Result<Option<u32>, DockerError> {
        self.inner.restart_count(name).await
    }

    async fn ensure_network(&self, name: &str) -> Result<(), DockerError> {
        self.inner.ensure_network(name).await
    }

    async fn ensure_private_network(&self, name: &str) -> Result<(), DockerError> {
        self.inner.ensure_private_network(name).await
    }

    async fn sync_private_networks(
        &self,
        container: &str,
        networks: &[String],
    ) -> Result<(), DockerError> {
        self.inner.sync_private_networks(container, networks).await
    }

    async fn remove_network_if_exists(&self, name: &str) -> Result<bool, DockerError> {
        self.inner.remove_network_if_exists(name).await
    }

    async fn pull_image(&self, image: &str) -> Result<(), DockerError> {
        self.inner.pull_image(image).await
    }

    async fn image_id(&self, image: &str) -> Result<Option<String>, DockerError> {
        self.inner.image_id(image).await
    }

    async fn container_image_id(&self, name: &str) -> Result<String, DockerError> {
        self.inner.container_image_id(name).await
    }

    async fn build_image(&self, path: &str, tag: &str) -> Result<(), DockerError> {
        self.inner.build_image(path, tag).await
    }

    async fn build_image_with_logs(
        &self,
        path: &str,
        tag: &str,
        logs: mpsc::Sender<String>,
    ) -> Result<(), DockerError> {
        self.inner.build_image_with_logs(path, tag, logs).await
    }

    async fn run_application(&self, config: ApplicationContainer) -> Result<(), DockerError> {
        self.inner.run_application(config).await
    }

    async fn remove_container(&self, name: &str) -> Result<(), DockerError> {
        self.inner.remove_container(name).await
    }

    async fn stream_logs(
        &self,
        container_name: &str,
    ) -> Result<mpsc::Receiver<String>, DockerError> {
        self.inner.stream_logs(container_name).await
    }

    async fn container_state(&self, name: &str) -> Result<Option<ContainerState>, DockerError> {
        self.inner.container_state(name).await
    }

    async fn container_stats(&self) -> Result<Vec<ContainerStats>, DockerError> {
        self.inner.container_stats().await
    }

    async fn start_container(&self, name: &str) -> Result<(), DockerError> {
        self.inner.start_container(name).await
    }

    async fn stop_container(&self, name: &str) -> Result<(), DockerError> {
        self.inner.stop_container(name).await
    }

    async fn restart_container(&self, name: &str) -> Result<(), DockerError> {
        self.inner.restart_container(name).await
    }

    async fn compose_up(&self, project: &ComposeProject) -> Result<(), DockerError> {
        self.inner.compose_up(project).await
    }

    async fn compose_up_pinned(&self, project: &ComposeProject) -> Result<(), DockerError> {
        self.inner.compose_up_pinned(project).await
    }

    async fn compose_start(&self, project: &ComposeProject) -> Result<(), DockerError> {
        self.inner.compose_start(project).await
    }

    async fn compose_stop(&self, project: &ComposeProject) -> Result<(), DockerError> {
        self.inner.compose_stop(project).await
    }

    async fn compose_restart(&self, project: &ComposeProject) -> Result<(), DockerError> {
        self.inner.compose_restart(project).await
    }

    async fn compose_recreate(&self, project: &ComposeProject) -> Result<(), DockerError> {
        self.inner.compose_recreate(project).await
    }

    async fn compose_down(&self, project: &ComposeProject) -> Result<(), DockerError> {
        self.entered.notify_one();
        self.release.notified().await;
        self.inner.compose_down(project).await
    }

    async fn stream_compose_logs(
        &self,
        project: &ComposeProject,
    ) -> Result<mpsc::Receiver<String>, DockerError> {
        self.inner.stream_compose_logs(project).await
    }
}
