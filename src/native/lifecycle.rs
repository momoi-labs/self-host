//! Application lifecycle on the existing N1 launcher and N2 s6 tree.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use tokio::sync::mpsc;

use super::supervision::{ServiceDefinition, ServiceStatus, Supervisor};
#[cfg(target_os = "linux")]
use super::{AccountName, ProvisionRequest, provision};
use super::{CgroupRoot, account_name_for};
use crate::apps::{self, ApplicationRecord, DeployError};
use crate::error::ErrorReport;
use crate::routes::RouteStore;
use crate::store::{NativeDefinition, Publication, Runtime, StateStore};

const STARTUP: Duration = Duration::from_secs(15);
const STOP: Duration = Duration::from_secs(15);

/// Every implementation runs commands through N1/N2. The API can inject this
/// boundary without making Docker pretend to be a native process supervisor.
#[allow(
    clippy::double_must_use,
    reason = "async_trait adds must_use to methods that already return must-use Futures"
)]
#[async_trait]
pub trait NativeRuntime: Send + Sync {
    async fn deploy(
        &self,
        record: &ApplicationRecord,
        env: Vec<(String, String)>,
        running: bool,
    ) -> Result<()>;
    async fn start(&self, record: &ApplicationRecord) -> Result<()>;
    async fn stop(&self, record: &ApplicationRecord) -> Result<()>;
    async fn restart(&self, record: &ApplicationRecord) -> Result<()>;
    async fn remove(&self, record: &ApplicationRecord) -> Result<()>;
    async fn status(&self, record: &ApplicationRecord) -> Result<ServiceStatus>;
    async fn logs(&self, record: &ApplicationRecord) -> Result<mpsc::Receiver<String>>;
}

#[derive(Debug, Clone)]
pub struct S6Runtime {
    root: PathBuf,
    binary: PathBuf,
    data_root: PathBuf,
    cgroup_root: Option<PathBuf>,
}

impl Default for S6Runtime {
    fn default() -> Self {
        Self {
            root: "/var/lib/self-host/native".into(),
            binary: "/usr/local/bin/self-host".into(),
            data_root: "/var/lib/self-host/native-data".into(),
            cgroup_root: None,
        }
    }
}

impl S6Runtime {
    /// Explicit paths are for a protected, disposable Linux fixture.
    pub fn at(root: PathBuf, binary: PathBuf, cgroup_root: PathBuf, data_root: PathBuf) -> Self {
        Self {
            root,
            binary,
            data_root,
            cgroup_root: Some(cgroup_root),
        }
    }

    fn supervisor(&self) -> Result<Supervisor> {
        let cgroup = match &self.cgroup_root {
            Some(path) => path.clone(),
            None => PathBuf::from(
                std::fs::read_to_string(self.root.join("cgroup-root"))
                    .context("native supervision is unavailable; enable self-host-native.service")?
                    .trim(),
            ),
        };
        Supervisor::connect(
            self.root.clone(),
            self.binary.clone(),
            CgroupRoot::at(cgroup)?,
        )
    }

    pub fn home(&self, id: &str) -> Result<PathBuf> {
        account_name_for(id)?;
        Ok(self.data_root.join(id))
    }

    #[cfg(target_os = "linux")]
    fn setup(
        &self,
        record: &ApplicationRecord,
        env: Vec<(String, String)>,
    ) -> Result<ServiceDefinition> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let definition = definition(record)?;
        validate_environment(&env)?;
        // This parent must be traversable by Application Accounts, unlike the
        // private supervisor root. Its ancestors must remain root controlled.
        if !self.data_root.exists() {
            protected_parent(self.data_root.parent().context("data root has no parent")?)?;
            std::fs::create_dir(&self.data_root)?;
            std::fs::set_permissions(&self.data_root, std::fs::Permissions::from_mode(0o711))?;
        }
        protected_parent(&self.data_root)?;
        let home = self.home(&record.id)?;
        if home.exists() {
            let metadata = std::fs::symlink_metadata(&home)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                bail!("Application data home must be a directory, never a symlink");
            }
        }
        let account = AccountName::parse(&definition.account)?;
        match super::identity::verify_owned(&ProvisionRequest {
            application_id: &record.id,
            account: &account,
            home: &home,
        }) {
            Ok(_) | Err(super::ProvisionError::Identity(super::IdentityError::Unknown(_))) => (),
            Err(error) => return Err(error.into()),
        }
        let identity = provision(&ProvisionRequest {
            application_id: &record.id,
            account: &account,
            home: &home,
        })?;
        if identity.home != home || std::fs::symlink_metadata(&home)?.uid() != identity.uid {
            bail!("Application Account does not own its dedicated data home");
        }
        let working_dir = definition
            .working_dir
            .as_deref()
            .map(|relative| home.join(relative))
            .unwrap_or(home.clone());
        let canonical = std::fs::canonicalize(&working_dir)
            .context("native working directory does not exist")?;
        if !canonical.starts_with(&home) {
            bail!("native working directory resolves outside the Application data home");
        }
        Ok(ServiceDefinition {
            application_id: record.id.clone(),
            account: definition.account.clone(),
            command: definition.command.clone(),
            working_dir,
            environment: env,
            limits: definition.limits.clone(),
            readiness: None,
            startup_timeout_ms: STARTUP.as_millis() as u64,
            stop_grace_ms: 1000,
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn setup(&self, _: &ApplicationRecord, _: Vec<(String, String)>) -> Result<ServiceDefinition> {
        bail!("native Applications run on Linux only")
    }

    fn start_checked(&self, supervisor: &Supervisor, record: &ApplicationRecord) -> Result<()> {
        let result = supervisor
            .start(&record.id, STARTUP)
            .and_then(|_| self.wait_for_web(record));
        if result.is_err() {
            let _ = supervisor.stop(&record.id, STOP);
        }
        result
    }

    #[cfg(target_os = "linux")]
    fn wait_for_web(&self, record: &ApplicationRecord) -> Result<()> {
        if record.publication == Publication::Unpublished {
            return Ok(());
        }
        let definition = definition(record)?;
        let port = definition.port.context("native Web Target needs a port")?;
        let uid = super::resolve(&AccountName::parse(&definition.account)?)?.uid;
        let deadline = std::time::Instant::now() + STARTUP;
        loop {
            if private_listener(port, uid)? {
                std::net::TcpStream::connect_timeout(
                    &([127, 0, 0, 1], port).into(),
                    Duration::from_millis(250),
                )?;
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                bail!(
                    "native Web Target did not listen on 127.0.0.1:{port} under its Application Account; inspect Application logs"
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn wait_for_web(&self, _: &ApplicationRecord) -> Result<()> {
        bail!("native Applications run on Linux only")
    }

    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(Self) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let runtime = self.clone();
        tokio::task::spawn_blocking(move || work(runtime)).await?
    }
}

#[cfg(target_os = "linux")]
fn protected_parent(path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    if !path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
    {
        bail!("native data root must be an absolute path without '..'");
    }
    for ancestor in path.ancestors() {
        let metadata = std::fs::symlink_metadata(ancestor)?;
        if metadata.file_type().is_symlink() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0
        {
            bail!(
                "native data root and ancestors must be root-owned, without symlinks or write access for other accounts"
            );
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn private_listener(port: u16, uid: u32) -> Result<bool> {
    let mut ready = false;
    for (path, loopback) in [
        ("/proc/net/tcp", "0100007F"),
        ("/proc/net/tcp6", "00000000000000000000000001000000"),
    ] {
        for line in std::fs::read_to_string(path)?.lines().skip(1) {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() < 8 || fields[3] != "0A" {
                continue;
            }
            let Some((address, encoded_port)) = fields[1].split_once(':') else {
                continue;
            };
            if u16::from_str_radix(encoded_port, 16).ok() != Some(port) {
                continue;
            }
            if address != loopback || fields[7].parse::<u32>().ok() != Some(uid) {
                bail!(
                    "native Web Target port {port} must be bound only to loopback by its Application Account"
                );
            }
            if path == "/proc/net/tcp" {
                ready = true;
            }
        }
    }
    Ok(ready)
}

#[async_trait]
impl NativeRuntime for S6Runtime {
    async fn deploy(
        &self,
        record: &ApplicationRecord,
        env: Vec<(String, String)>,
        running: bool,
    ) -> Result<()> {
        let record = record.clone();
        self.blocking(move |runtime| {
            let supervisor = runtime.supervisor()?;
            let service = runtime.setup(&record, env)?;
            if supervisor.exists(&record.id)? {
                supervisor.replace(&service, STOP)?;
            } else {
                supervisor.prepare(&service)?;
            }
            if running {
                runtime.start_checked(&supervisor, &record)?;
            }
            Ok(())
        })
        .await
    }
    async fn start(&self, record: &ApplicationRecord) -> Result<()> {
        let record = record.clone();
        self.blocking(move |runtime| runtime.start_checked(&runtime.supervisor()?, &record))
            .await
    }
    async fn stop(&self, record: &ApplicationRecord) -> Result<()> {
        let record = record.clone();
        self.blocking(move |runtime| runtime.supervisor()?.stop(&record.id, STOP))
            .await
    }
    async fn restart(&self, record: &ApplicationRecord) -> Result<()> {
        let record = record.clone();
        self.blocking(move |runtime| {
            let supervisor = runtime.supervisor()?;
            supervisor.stop(&record.id, STOP)?;
            runtime.start_checked(&supervisor, &record)
        })
        .await
    }
    async fn remove(&self, record: &ApplicationRecord) -> Result<()> {
        let record = record.clone();
        self.blocking(move |runtime| {
            let supervisor = runtime.supervisor()?;
            if supervisor.exists(&record.id)? {
                supervisor.remove(&record.id, STOP)?;
            }
            #[cfg(target_os = "linux")]
            {
                use std::os::unix::fs::PermissionsExt;
                let home = runtime.home(&record.id)?;
                let account = account_name_for(&record.id)?;
                let request = ProvisionRequest {
                    application_id: &record.id,
                    account: &account,
                    home: &home,
                };
                match super::identity::verify_owned(&request) {
                    Ok(_) => (),
                    Err(super::ProvisionError::Identity(super::IdentityError::Unknown(_))) => (),
                    Err(error) => return Err(error.into()),
                }
                // Retained data must not become readable if userdel's uid is reused.
                if home.exists() {
                    if std::fs::symlink_metadata(&home)?.file_type().is_symlink() {
                        bail!("refusing symlink Application home");
                    }
                    std::os::unix::fs::chown(&home, Some(0), Some(0))?;
                    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700))?;
                }
                super::identity::revoke(&request)?;
            }
            Ok(())
        })
        .await
    }
    async fn status(&self, record: &ApplicationRecord) -> Result<ServiceStatus> {
        let record = record.clone();
        self.blocking(move |runtime| runtime.supervisor()?.status(&record.id))
            .await
    }
    async fn logs(&self, record: &ApplicationRecord) -> Result<mpsc::Receiver<String>> {
        // s6-log owns the directory and bounded rotation. Reading only current
        // avoids exposing arbitrary Host files or launching a privileged tail.
        let record = record.clone();
        let path = self
            .blocking(move |runtime| {
                runtime.supervisor()?;
                account_name_for(&record.id)?;
                Ok(runtime.root.join("logs").join(&record.id).join("current"))
            })
            .await?;
        let (sender, receiver) = mpsc::channel(64);
        tokio::spawn(async move {
            let mut position = 0usize;
            let mut partial = String::new();
            loop {
                tokio::select! { _ = sender.closed() => break, _ = tokio::time::sleep(Duration::from_millis(100)) => () }
                let Ok(bytes) = tokio::fs::read(&path).await else {
                    continue;
                };
                if bytes.len() < position {
                    position = 0;
                    partial.clear();
                }
                partial.push_str(&String::from_utf8_lossy(&bytes[position..]));
                position = bytes.len();
                while let Some(end) = partial.find('\n') {
                    let line = partial[..end].to_owned();
                    partial.drain(..=end);
                    if sender.send(line).await.is_err() {
                        return;
                    }
                }
            }
        });
        Ok(receiver)
    }
}

#[cfg(target_os = "linux")]
fn definition(record: &ApplicationRecord) -> Result<&NativeDefinition> {
    match &record.runtime {
        Runtime::Native(definition) => Ok(definition),
        _ => bail!("Application Runtime is not native"),
    }
}

pub fn validate_definition(
    id: &str,
    definition: &mut NativeDefinition,
    publication: Publication,
) -> Result<()> {
    let account = account_name_for(id)?.to_string();
    if definition.account.is_empty() {
        definition.account = account.clone();
    }
    if definition.account != account {
        bail!("native Application Account must be {account}");
    }
    if definition.command.is_empty()
        || definition.command[0].is_empty()
        || definition.command.iter().any(|arg| arg.contains('\0'))
    {
        bail!("native command must contain a program and no NUL bytes");
    }
    if let Some(relative) = &definition.working_dir
        && (Path::new(relative).is_absolute()
            || Path::new(relative)
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
            || relative.contains('\0')
            || relative
                .split('/')
                .any(|part| matches!(part, "." | ".." | "")))
    {
        bail!(
            "native working_dir must be relative to the Application data home without '.' or '..'"
        );
    }
    match (publication, definition.port) {
        (Publication::Web, Some(port)) if port >= 1024 && !reserved_port(port) => (),
        (Publication::Web, _) => {
            bail!("native Web Target needs an unprivileged, non-reserved loopback port")
        }
        (Publication::Unpublished, None) => (),
        (Publication::Unpublished, Some(_)) => {
            bail!("an unpublished native Application has no Web Target port")
        }
    }
    if definition.limits.cpu_percent == Some(0)
        || definition.limits.memory_bytes == Some(0)
        || definition.limits.max_tasks == Some(0)
    {
        bail!("native resource limits must be nonzero when set");
    }
    Ok(())
}

fn reserved_port(port: u16) -> bool {
    if [3721, 15432].contains(&port) {
        return true;
    }
    if ["SELF_HOST_PROXY_HTTP_PORT", "SELF_HOST_PROXY_HTTPS_PORT"]
        .iter()
        .any(|name| std::env::var(name).ok().and_then(|v| v.parse::<u16>().ok()) == Some(port))
    {
        return true;
    }
    std::env::var("SELF_HOST_LISTEN")
        .ok()
        .and_then(|v| v.parse::<std::net::SocketAddr>().ok())
        .is_some_and(|address| address.port() == port)
}

pub async fn validate_port(
    store: &impl StateStore,
    id: &str,
    port: Option<u16>,
) -> Result<(), DeployError> {
    let Some(port) = port else {
        return Ok(());
    };
    if store
        .list_applications()
        .await?
        .iter()
        .any(|a| a.id != id && a.web_target_port == Some(port))
    {
        return Err(DeployError::InvalidNative(
            "native Web Target port is already assigned to another Application".into(),
        ));
    }
    if store
        .get_state("api_listen_addr")
        .await?
        .and_then(|v| v.parse::<std::net::SocketAddr>().ok())
        .is_some_and(|address| address.port() == port)
    {
        return Err(DeployError::InvalidNative(
            "native Web Target port is reserved for the Operator API".into(),
        ));
    }
    Ok(())
}

pub fn validate_environment(env: &[(String, String)]) -> Result<()> {
    for (key, value) in env {
        if key.is_empty()
            || key.contains(['=', '\0'])
            || value.contains('\0')
            || ["HOME", "USER", "LOGNAME"].contains(&key.as_str())
        {
            bail!("invalid or reserved native Application Variable '{key}'");
        }
    }
    Ok(())
}

pub async fn deploy(
    store: &impl StateStore,
    runtime: &dyn NativeRuntime,
    routes: &dyn RouteStore,
    record: ApplicationRecord,
    running: bool,
) -> Result<ApplicationRecord, DeployError> {
    routes.withdraw(&record.id);
    let env = store.get_all_env(&record.id).await?;
    settle(
        store,
        routes,
        &record,
        runtime.deploy(&record, env, running).await,
        if running {
            apps::STATUS_RUNNING
        } else {
            apps::STATUS_STOPPED
        },
    )
    .await
}

pub async fn operate(
    store: &impl StateStore,
    runtime: &dyn NativeRuntime,
    routes: &dyn RouteStore,
    id: &str,
    action: &str,
) -> Result<ApplicationRecord, DeployError> {
    let record = apps::get_application(store, id).await?;
    routes.withdraw(id);
    let result = match action {
        "start" => runtime.start(&record).await,
        "stop" => runtime.stop(&record).await,
        "restart" => runtime.restart(&record).await,
        _ => unreachable!(),
    };
    settle(
        store,
        routes,
        &record,
        result,
        if action == "stop" {
            apps::STATUS_STOPPED
        } else {
            apps::STATUS_RUNNING
        },
    )
    .await
}

async fn settle(
    store: &impl StateStore,
    routes: &dyn RouteStore,
    record: &ApplicationRecord,
    result: Result<()>,
    status: &str,
) -> Result<ApplicationRecord, DeployError> {
    match result {
        Ok(()) => {
            let current = store
                .set_application_outcome(&record.id, status, None)
                .await?;
            if status == apps::STATUS_RUNNING {
                routes.publish(&current);
            }
            Ok(current)
        }
        Err(error) => {
            let error = DeployError::Native(error);
            let _ = store
                .set_application_outcome(
                    &record.id,
                    apps::STATUS_FAILED,
                    Some(ErrorReport::new(&error)),
                )
                .await;
            Err(error)
        }
    }
}

pub async fn remove(
    store: &impl StateStore,
    runtime: &dyn NativeRuntime,
    routes: &dyn RouteStore,
    record: &ApplicationRecord,
) -> Result<(), DeployError> {
    routes.withdraw(&record.id);
    if let Err(error) = runtime.remove(record).await {
        let error = DeployError::Native(error);
        let _ = store
            .set_application_outcome(
                &record.id,
                apps::STATUS_FAILED,
                Some(ErrorReport::new(&error)),
            )
            .await;
        return Err(error);
    }
    store.delete_application(&record.id).await?;
    Ok(())
}

pub async fn environment(
    store: &impl StateStore,
    runtime: &dyn NativeRuntime,
    routes: &dyn RouteStore,
    name: &str,
    key: &str,
    value: Option<&str>,
) -> Result<ApplicationRecord, DeployError> {
    let record = store
        .find_application_by_name(name)
        .await?
        .ok_or_else(|| DeployError::NotFound(name.into()))?;
    validate_environment(&[(key.into(), value.unwrap_or_default().into())])
        .map_err(|e| DeployError::InvalidNative(e.to_string()))?;
    let running = runtime
        .status(&record)
        .await
        .map(|status| status.intended_running)
        .unwrap_or(record.status != apps::STATUS_STOPPED);
    match value {
        Some(value) => store.set_env(&record.id, key, value).await?,
        None => store.unset_env(&record.id, key).await?,
    }
    deploy(store, runtime, routes, record, running).await
}

pub async fn reconcile(
    store: &impl StateStore,
    runtime: &dyn NativeRuntime,
    routes: &dyn RouteStore,
) -> Result<(), DeployError> {
    let queued = crate::tasks::queued_application_ids(store).await?;
    for mut record in store
        .list_applications()
        .await?
        .into_iter()
        .filter(|r| matches!(r.runtime, Runtime::Native(_)))
    {
        let observation = runtime.status(&record).await;
        if !queued.contains(&record.id)
            && let Ok(status) = &observation
        {
            if !status.intended_running {
                record.status = apps::STATUS_STOPPED.into();
                record.last_error = None;
            } else if status.running && status.ready {
                record.status = apps::STATUS_RUNNING.into();
                record.last_error = None;
            } else {
                record.status = apps::STATUS_FAILED.into();
                record.last_error = Some(ErrorReport::plain(
                    "native Application is not ready; inspect Application logs",
                ));
            }
            store.insert_application(&record).await?;
        } else if record.status == apps::STATUS_PENDING
            && !queued.contains(&record.id)
            && observation.is_err()
        {
            // An unavailable supervisor cannot tell whether a launch finished.
            tracing::warn!(application_id = %record.id, "native supervision unavailable; interrupted work stays pending");
        }
        if record.status == apps::STATUS_RUNNING
            && observation.is_ok_and(|s| s.running && s.ready && s.intended_running)
        {
            routes.publish(&record);
        } else {
            routes.withdraw(&record.id);
        }
    }
    Ok(())
}

pub async fn service_states(
    runtime: &dyn NativeRuntime,
    record: &ApplicationRecord,
) -> Vec<apps::ServiceState> {
    let status = match runtime.status(record).await {
        Ok(status) => status,
        Err(error) => {
            return vec![apps::ServiceState {
                service: "main".into(),
                container: String::new(),
                state: "unknown".into(),
                exit_code: None,
                restarts: None,
                health: Some(format!("native supervision unavailable: {error:#}")),
            }];
        }
    };
    vec![apps::ServiceState {
        service: "main".into(),
        container: String::new(),
        state: if status.running { "running" } else { "exited" }.into(),
        exit_code: None,
        restarts: None,
        health: Some(if status.ready { "healthy" } else { "unhealthy" }.into()),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    fn spec() -> NativeDefinition {
        NativeDefinition {
            account: String::new(),
            command: vec!["/bin/sleep".into(), "300".into()],
            working_dir: None,
            port: None,
            limits: Default::default(),
        }
    }
    #[test]
    fn account_is_dedicated_and_worker_has_no_port() {
        let mut definition = spec();
        validate_definition("synthetic", &mut definition, Publication::Unpublished).unwrap();
        assert_eq!(definition.account, "sf-app-synthetic");
        definition.account = "root".into();
        assert!(
            validate_definition("synthetic", &mut definition, Publication::Unpublished).is_err()
        );
    }
    #[test]
    fn native_definition_refuses_outside_paths_and_missing_web_port() {
        for path in ["/root", "../outside", "a/../outside", ".", "a/./b"] {
            let mut definition = spec();
            definition.working_dir = Some(path.into());
            assert!(
                validate_definition("synthetic", &mut definition, Publication::Unpublished)
                    .is_err(),
                "{path}"
            );
        }
        assert!(validate_definition("synthetic", &mut spec(), Publication::Web).is_err());
    }
    #[test]
    fn common_unprivileged_port_is_allowed_but_platform_ports_are_reserved() {
        let mut definition = spec();
        definition.port = Some(8080);
        validate_definition("synthetic", &mut definition, Publication::Web).unwrap();
        for port in [80, 443, 3721, 15432] {
            definition.port = Some(port);
            assert!(validate_definition("synthetic", &mut definition, Publication::Web).is_err());
        }
    }
    #[test]
    fn identity_variables_are_refused_before_any_record_or_launch() {
        for key in ["HOME", "USER", "LOGNAME", "", "A=B"] {
            assert!(validate_environment(&[(key.into(), "value".into())]).is_err());
        }
        validate_environment(&[("SYNTHETIC".into(), "value".into())]).unwrap();
    }
}
