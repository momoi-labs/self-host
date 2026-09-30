//! Operator-supplied Compose definitions (ADR-0014, ADR-0015).
//!
//! The Operator pastes a Compose file into the console. The Platform does not
//! run it as written: it reads the supported subset, refuses the rest by
//! name, and renders a project of its own that names every container after
//! the Application, joins the Application network and keeps data where the
//! Platform can find it. What is supported is written down in
//! `docs/compose-applications.md`; this module is that document as code.
//!
//! The Platform also resolves the file's `${VAR}` references itself, against
//! the Application's Variables and nothing else, and writes every `$` of the
//! rendered project as `$$` so `docker compose` never interpolates it again
//! (ADR-0030).

use indexmap::IndexMap;
use serde_yaml::{Mapping, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::docker::APP_NETWORK;
use crate::store::VariableDelivery;

/// Host ports the Platform Infra already answers on. A Compose service that
/// asks for one would either fail to start or take the Platform down with it.
pub const PLATFORM_PORTS: &[u16] = &[53, 80, 443, 3721, 15432];

/// The service keys the Platform passes through. Anything else is refused by
/// name, so the Operator learns what will not run instead of watching it get
/// dropped on the floor.
const SERVICE_KEYS: &[&str] = &[
    "image",
    "command",
    "entrypoint",
    "environment",
    "ports",
    "volumes",
    "extra_hosts",
    "restart",
    "depends_on",
    "deploy",
    "healthcheck",
    "labels",
    "working_dir",
    "user",
    "expose",
    "stop_grace_period",
    "init",
    "container_name",
];

const TOP_LEVEL_KEYS: &[&str] = &["version", "name", "services", "volumes"];

/// Options on a top-level volume declaration the Platform does not carry
/// into the rendered project. Refused by name rather than dropped, so the
/// Operator learns the volume will not be what the file says.
const UNSUPPORTED_VOLUME_KEYS: &[&str] = &["driver", "driver_opts", "name", "labels"];

/// How strictly `${VAR:?}` and `${VAR?}` are treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Create and update: the Operator may not have set the Variables yet.
    /// Requirements are not raised. Unset reads as empty.
    Check,
    /// Before execution: an unset required Variable refuses the render.
    Run,
}

#[derive(Debug)]
pub enum ComposeDefinitionError {
    Yaml(String),
    NoServices,
    UnsupportedTopLevel(String),
    InvalidServiceName(String),
    MissingImage(String),
    UnsupportedKey {
        service: String,
        key: String,
    },
    Invalid {
        service: String,
        what: String,
    },
    ReservedPort {
        service: String,
        port: u16,
    },
    ExternalVolume(String),
    UnsupportedVolumeOption {
        volume: String,
        key: String,
    },
    /// A `$` the Platform cannot read as a Variable reference. `service` is
    /// `None` when the text sits outside every service.
    Interpolation {
        service: Option<String>,
        what: String,
    },
    /// `${VAR:?}` or `${VAR?}` with nothing to give, under `Resolution::Run`.
    RequiredVariable {
        name: String,
        message: String,
    },
    UnknownWebService(String),
    NoWebPort(String),
}

impl std::fmt::Display for ComposeDefinitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Yaml(msg) => write!(f, "the Compose definition is not valid YAML: {msg}"),
            Self::NoServices => write!(f, "the Compose definition declares no services"),
            Self::UnsupportedTopLevel(key) => {
                write!(
                    f,
                    "top-level '{key}' is not supported; the Platform manages it"
                )
            }
            Self::InvalidServiceName(name) => write!(
                f,
                "service name '{name}' must be lowercase letters, digits, hyphens or underscores"
            ),
            Self::MissingImage(service) => write!(f, "service '{service}' has no image"),
            Self::UnsupportedKey { service, key } => {
                write!(f, "service '{service}': '{key}' is not supported")
            }
            Self::Invalid { service, what } => write!(f, "service '{service}': {what}"),
            Self::ReservedPort { service, port } => write!(
                f,
                "service '{service}': host port {port} belongs to the Platform"
            ),
            Self::ExternalVolume(name) => write!(
                f,
                "volume '{name}' is external; only volumes the Platform creates are supported"
            ),
            Self::UnsupportedVolumeOption { volume, key } => {
                write!(f, "volume '{volume}': '{key}' is not supported yet")
            }
            Self::Interpolation {
                service: Some(service),
                what,
            } => write!(f, "service '{service}': {what}"),
            Self::Interpolation {
                service: None,
                what,
            } => f.write_str(what),
            Self::RequiredVariable { name, message } if message.is_empty() => {
                write!(f, "required Variable '{name}' has no value")
            }
            Self::RequiredVariable { name, message } => {
                write!(f, "required Variable '{name}' has no value: {message}")
            }
            Self::UnknownWebService(name) => {
                write!(f, "web service '{name}' is not one of the services")
            }
            Self::NoWebPort(service) => write!(
                f,
                "service '{service}' publishes no port, so the web port must be given"
            ),
        }
    }
}

impl std::error::Error for ComposeDefinitionError {}

/// A port a service asks to publish. The container side is what the router
/// needs; the host side is what the LAN sees directly, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedPort {
    pub host: Option<u16>,
    pub container: u16,
}

/// Where a mount's data lives on the Host, as the Platform will arrange it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MountKind {
    /// A named volume Compose creates under the project.
    Named,
    /// A path under the Application's data directory; `~` or `./` in the file.
    Data,
    /// An absolute Host path, passed through as written.
    Host,
    /// No source at all: Docker's anonymous volume, gone with the container.
    Anonymous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub source: String,
    pub target: String,
    pub kind: MountKind,
    /// For `Data`, the path under the Application's data directory.
    pub data_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ServiceDefinition {
    pub name: String,
    pub image: String,
    pub ports: Vec<PublishedPort>,
    pub mounts: Vec<Mount>,
    /// The service as the Operator wrote it, keys already checked.
    body: Mapping,
}

/// The Operator's Compose file, read and checked, not yet rendered. Its
/// `${VAR}` references are already resolved; `referenced` remembers which
/// names they asked for.
#[derive(Debug, Clone)]
pub struct ComposeDefinition {
    pub services: Vec<ServiceDefinition>,
    declared_volumes: Vec<String>,
    referenced: BTreeSet<String>,
}

/// Where the Application answers: one service, one container port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebTarget {
    pub service: String,
    pub port: u16,
}

/// A Web Target and the Host port it answers on, so a proxy that is not on
/// the Application's network can reach it (ADR-0019).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedTarget {
    pub target: WebTarget,
    pub host_port: u16,
}

/// Values the Platform adds to selected services while rendering. The
/// Operator's Compose definition remains unchanged; callers use these only
/// for Application types with Platform-owned runtime settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderOverrides {
    /// Docker hostnames by service name. An entry replaces the service's
    /// rendered `hostname` value.
    pub service_hostnames: IndexMap<String, String>,
}

impl RenderOverrides {
    pub fn service_hostname(service: impl Into<String>, hostname: impl Into<String>) -> Self {
        Self {
            service_hostnames: [(service.into(), hostname.into())].into_iter().collect(),
        }
    }
}

/// The rendered project, ready for `docker compose`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeProject {
    /// The Compose project name; also the prefix of every container.
    pub name: String,
    /// Where the file and the Application's data live.
    pub dir: PathBuf,
    pub yaml: String,
    /// Host directories the Platform bind-mounts into the services. Created
    /// before `up`, so Docker does not create them owned by root.
    pub bind_dirs: Vec<PathBuf>,
    /// Container names, in service order.
    pub containers: Vec<(String, String)>,
}

impl ComposeDefinition {
    /// `parse_with` for a file read on its own: no Variables, requirements
    /// not raised. What create, update and `/compose/inspect` need.
    pub fn parse(yaml: &str) -> Result<Self, ComposeDefinitionError> {
        Self::parse_with(yaml, &[], Resolution::Check)
    }

    /// Reads the file the way `docker compose` would, with the Application's
    /// Variables standing in for the process environment. Anchors and `<<`
    /// merge keys are applied, `x-` extension fields dropped, every `${VAR}`
    /// resolved against `variables` and nothing else, and only then is the
    /// supported subset checked.
    pub fn parse_with(
        yaml: &str,
        variables: &[(String, String)],
        resolution: Resolution,
    ) -> Result<Self, ComposeDefinitionError> {
        let mut doc: Value =
            serde_yaml::from_str(yaml).map_err(|e| ComposeDefinitionError::Yaml(e.to_string()))?;
        doc.apply_merge()
            .map_err(|e| ComposeDefinitionError::Yaml(e.to_string()))?;
        let Value::Mapping(mut top) = doc else {
            return Err(ComposeDefinitionError::Yaml(
                "expected a mapping with a 'services' key".into(),
            ));
        };

        drop_extension_keys(&mut top);
        if let Some(Value::Mapping(services)) = top.get_mut("services") {
            for body in services.values_mut() {
                if let Value::Mapping(body) = body {
                    drop_extension_keys(body);
                }
            }
        }

        let mut interpolator = Interpolator::new(variables, resolution);
        interpolator.document(&mut top)?;
        let mut referenced = interpolator.referenced;

        for key in top.keys() {
            let key = key_name(key);
            if !TOP_LEVEL_KEYS.contains(&key.as_str()) {
                return Err(ComposeDefinitionError::UnsupportedTopLevel(key));
            }
        }

        let services = match top.get("services") {
            Some(Value::Mapping(m)) if !m.is_empty() => m,
            _ => return Err(ComposeDefinitionError::NoServices),
        };

        let mut parsed = Vec::with_capacity(services.len());
        for (name, body) in services {
            let name = key_name(name);
            validate_service_name(&name)?;
            let Value::Mapping(body) = body else {
                return Err(ComposeDefinitionError::Invalid {
                    service: name,
                    what: "must be a mapping".into(),
                });
            };
            parsed.push(parse_service(
                name,
                body.clone(),
                variables,
                &mut referenced,
            )?);
        }

        let declared_volumes = match top.get("volumes") {
            None | Some(Value::Null) => vec![],
            Some(Value::Mapping(m)) => {
                let mut names = Vec::with_capacity(m.len());
                for (name, body) in m {
                    let name = key_name(name);
                    if let Value::Mapping(b) = body {
                        if b.get("external").is_some_and(|v| v.as_bool() == Some(true)) {
                            return Err(ComposeDefinitionError::ExternalVolume(name));
                        }
                        if let Some(key) = b
                            .keys()
                            .map(key_name)
                            .find(|key| UNSUPPORTED_VOLUME_KEYS.contains(&key.as_str()))
                        {
                            return Err(ComposeDefinitionError::UnsupportedVolumeOption {
                                volume: name,
                                key,
                            });
                        }
                    }
                    names.push(name);
                }
                names
            }
            Some(_) => {
                return Err(ComposeDefinitionError::UnsupportedTopLevel(
                    "volumes (must be a mapping)".into(),
                ));
            }
        };

        Ok(ComposeDefinition {
            services: parsed,
            declared_volumes,
            referenced,
        })
    }

    /// Names of every Variable the definition references: `${VAR}` in any of
    /// its forms, `$VAR`, and a bare `VAR` environment entry.
    pub fn referenced_variables(&self) -> BTreeSet<String> {
        self.referenced.clone()
    }

    pub fn service(&self, name: &str) -> Option<&ServiceDefinition> {
        self.services.iter().find(|s| s.name == name)
    }

    /// Resolves where the Hostname should point. Left unsaid, the web service
    /// is the first one that publishes a port and the port is the container
    /// side of its first publication: the shape of most one-service files.
    pub fn web_target(
        &self,
        service: Option<&str>,
        port: Option<u16>,
    ) -> Result<WebTarget, ComposeDefinitionError> {
        let service = match service {
            Some(name) => self
                .service(name)
                .ok_or_else(|| ComposeDefinitionError::UnknownWebService(name.to_string()))?,
            None => self
                .services
                .iter()
                .find(|s| !s.ports.is_empty())
                .unwrap_or(&self.services[0]),
        };

        let port = match port {
            Some(p) => p,
            None => service
                .ports
                .first()
                .map(|p| p.container)
                .ok_or_else(|| ComposeDefinitionError::NoWebPort(service.name.clone()))?,
        };

        Ok(WebTarget {
            service: service.name.clone(),
            port,
        })
    }

    /// Renders the project the Platform runs.
    ///
    /// Every service gets its container named after the Application, the
    /// identity labels, the Application network next to the project's own
    /// and a restart policy if it had none. Host paths are pinned under the
    /// Application's directory. `published` is the Web Target's Host port,
    /// when it has one. Only that one service gets it, and only on loopback:
    /// the rest of the project stays reachable on the Application's own
    /// network and nowhere else. Ports the Operator published themselves are
    /// left exactly as written.
    ///
    /// `variables` reach the services as `delivery` says. `Broadcast` copies
    /// every one into every service's `environment`; `Referenced` copies
    /// nothing, because the file already received them when it was parsed.
    /// Either way every `$` in the rendered YAML is written as `$$`, so
    /// `docker compose` cannot interpolate the project against the daemon's
    /// environment (ADR-0030).
    pub fn render(
        &self,
        name: &str,
        dir: &Path,
        labels: &[(String, String)],
        variables: &[(String, String)],
        delivery: VariableDelivery,
        published: Option<&PublishedTarget>,
    ) -> ComposeProject {
        self.render_with_overrides(
            name,
            dir,
            labels,
            variables,
            delivery,
            published,
            &RenderOverrides::default(),
        )
    }

    /// Renders a Platform-owned runtime variant of this definition.
    ///
    /// Normal Compose Applications use [`Self::render`]. Callers that own a
    /// generated Application definition can add a small, explicit set of
    /// service overrides without writing those values back into the
    /// Operator-facing Compose text.
    #[allow(clippy::too_many_arguments)]
    pub fn render_with_overrides(
        &self,
        name: &str,
        dir: &Path,
        labels: &[(String, String)],
        variables: &[(String, String)],
        delivery: VariableDelivery,
        published: Option<&PublishedTarget>,
        overrides: &RenderOverrides,
    ) -> ComposeProject {
        let data_dir = dir.join("data");
        let mut services = Mapping::new();
        let mut volumes: IndexMap<String, Value> = self
            .declared_volumes
            .iter()
            .map(|v| (v.clone(), Value::Mapping(Mapping::new())))
            .collect();
        let mut bind_dirs = Vec::new();
        let mut containers = Vec::new();

        for service in &self.services {
            let mut body = service.body.clone();
            let container = container_name(name, &service.name);
            containers.push((service.name.clone(), container.clone()));

            body.insert("container_name".into(), container.into());
            if let Some(hostname) = overrides.service_hostnames.get(&service.name) {
                body.insert("hostname".into(), hostname.clone().into());
            }
            if !body.contains_key("restart") {
                body.insert("restart".into(), "unless-stopped".into());
            }
            body.insert(
                "networks".into(),
                Value::Sequence(vec!["default".into(), APP_NETWORK.into()]),
            );

            let mut service_labels = mapping_of_strings(body.get("labels"));
            for (k, v) in labels {
                service_labels.insert(k.clone(), v.clone());
            }
            service_labels.insert("sf.app.service".into(), service.name.clone());
            body.insert("labels".into(), to_string_mapping(&service_labels));

            let mut environment = mapping_of_strings(body.get("environment"));
            if delivery == VariableDelivery::Broadcast {
                for (k, v) in variables {
                    environment.insert(k.clone(), v.clone());
                }
            }
            if !environment.is_empty() {
                body.insert("environment".into(), to_string_mapping(&environment));
            }

            if let Some(published) = published
                && published.target.service == service.name
            {
                let mut ports = match body.get("ports") {
                    Some(Value::Sequence(declared)) => declared.clone(),
                    _ => Vec::new(),
                };
                ports.push(Value::String(crate::ports::publication(
                    published.host_port,
                    published.target.port,
                )));
                body.insert("ports".into(), Value::Sequence(ports));
            }

            if let Some(Value::Sequence(mounts)) = body.get("volumes") {
                let mut rewritten = Vec::with_capacity(mounts.len());
                for mount in mounts {
                    let spec = mount.as_str().unwrap_or_default();
                    let (source, rest) = split_mount(spec);
                    match classify_source(source) {
                        MountSource::Named(volume) => {
                            volumes
                                .entry(volume)
                                .or_insert_with(|| Value::Mapping(Mapping::new()));
                            rewritten.push(Value::String(spec.to_string()));
                        }
                        MountSource::Absolute => rewritten.push(Value::String(spec.to_string())),
                        MountSource::Anonymous => rewritten.push(Value::String(spec.to_string())),
                        MountSource::Relative(rel) => {
                            let host = data_dir.join(rel);
                            bind_dirs.push(host.clone());
                            rewritten.push(Value::String(format!("{}:{rest}", host.display())));
                        }
                    }
                }
                body.insert("volumes".into(), Value::Sequence(rewritten));
            }

            services.insert(service.name.clone().into(), Value::Mapping(body));
        }

        let mut top = Mapping::new();
        top.insert("name".into(), name.into());
        top.insert("services".into(), Value::Mapping(services));
        if !volumes.is_empty() {
            let mut m = Mapping::new();
            for (k, v) in volumes {
                m.insert(k.into(), v);
            }
            top.insert("volumes".into(), Value::Mapping(m));
        }
        let mut app_network = Mapping::new();
        app_network.insert("external".into(), Value::Bool(true));
        let mut networks = Mapping::new();
        networks.insert(APP_NETWORK.into(), Value::Mapping(app_network));
        top.insert("networks".into(), Value::Mapping(networks));

        let mut top = Value::Mapping(top);
        escape_dollars(&mut top);

        ComposeProject {
            name: name.to_string(),
            dir: dir.to_path_buf(),
            yaml: serde_yaml::to_string(&top)
                .expect("a mapping of strings and sequences serialises"),
            bind_dirs,
            containers,
        }
    }
}

/// `<project>-<service>`: the project is the Application's container prefix,
/// so `docker ps` reads the same way for one container or five.
pub fn container_name(project: &str, service: &str) -> String {
    format!("{project}-{service}")
}

fn key_name(key: &Value) -> String {
    match key {
        Value::String(s) => s.clone(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

fn validate_service_name(name: &str) -> Result<(), ComposeDefinitionError> {
    let ok = !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        && !name.starts_with(['-', '_']);
    if ok {
        Ok(())
    } else {
        Err(ComposeDefinitionError::InvalidServiceName(name.to_string()))
    }
}

/// Checks one service against the subset. `variables` fill the bare
/// `environment` entries (`- KEY`, or `KEY:` with no value), which Compose
/// reads from the process environment; the names they asked for join
/// `referenced` whether or not a value was there.
fn parse_service(
    name: String,
    mut body: Mapping,
    variables: &[(String, String)],
    referenced: &mut BTreeSet<String>,
) -> Result<ServiceDefinition, ComposeDefinitionError> {
    for key in body.keys() {
        let key = key_name(key);
        if !SERVICE_KEYS.contains(&key.as_str()) {
            return Err(ComposeDefinitionError::UnsupportedKey { service: name, key });
        }
    }

    let image = match body.get("image") {
        Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
        _ => return Err(ComposeDefinitionError::MissingImage(name)),
    };

    let invalid = |what: String| ComposeDefinitionError::Invalid {
        service: name.clone(),
        what,
    };

    let mut ports = Vec::new();
    match body.get("ports") {
        None | Some(Value::Null) => {}
        Some(Value::Sequence(items)) => {
            for item in items {
                let spec = match item {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    _ => {
                        return Err(invalid(
                            "ports must use the short syntax, e.g. \"9119:9119\"".into(),
                        ));
                    }
                };
                let port = parse_port(&spec)
                    .ok_or_else(|| invalid(format!("port '{spec}' is not HOST:CONTAINER")))?;
                if let Some(host) = port.host
                    && PLATFORM_PORTS.contains(&host)
                {
                    return Err(ComposeDefinitionError::ReservedPort {
                        service: name,
                        port: host,
                    });
                }
                ports.push(port);
            }
        }
        Some(_) => return Err(invalid("ports must be a list".into())),
    }

    let mut mounts = Vec::new();
    match body.get("volumes") {
        None | Some(Value::Null) => {}
        Some(Value::Sequence(items)) => {
            for item in items {
                let Value::String(spec) = item else {
                    return Err(invalid(
                        "volumes must use the short syntax, e.g. \"./data:/opt/data\"".into(),
                    ));
                };
                let (source, rest) = split_mount(spec);
                let (target, mode) = match rest.split_once(':') {
                    Some((target, mode)) => (target, Some(mode)),
                    None => (rest, None),
                };
                if target.is_empty() {
                    return Err(invalid(format!("volume '{spec}' has no container path")));
                }
                // SELinux labels and the Docker Desktop consistency hints
                // mean nothing on this Host and would be passed to Docker
                // unread. Only the access mode is understood.
                if let Some(mode) = mode
                    && let Some(option) = mode.split(',').find(|o| !matches!(*o, "ro" | "rw"))
                {
                    return Err(invalid(format!(
                        "volume '{spec}': mount mode '{option}' is not supported; use 'ro' or 'rw'"
                    )));
                }
                let (kind, data_path) = match classify_source(source) {
                    MountSource::Relative(rel) => {
                        if rel
                            .components()
                            .any(|c| matches!(c, std::path::Component::ParentDir))
                        {
                            return Err(invalid(format!(
                                "volume '{spec}' leaves the Application's directory"
                            )));
                        }
                        (
                            MountKind::Data,
                            Some(Path::new("data").join(rel).display().to_string()),
                        )
                    }
                    MountSource::Named(_) => (MountKind::Named, None),
                    MountSource::Absolute => (MountKind::Host, None),
                    MountSource::Anonymous => (MountKind::Anonymous, None),
                };
                mounts.push(Mount {
                    source: source.to_string(),
                    target: target.to_string(),
                    kind,
                    data_path,
                });
            }
        }
        Some(_) => return Err(invalid("volumes must be a list".into())),
    }

    match body.get("extra_hosts") {
        None | Some(Value::Null) => {}
        Some(Value::Sequence(items)) => {
            for item in items {
                let Value::String(spec) = item else {
                    return Err(invalid(
                        "extra_hosts must use the short syntax, e.g. \"unifi.local:192.168.1.1\""
                            .into(),
                    ));
                };
                let Some((name, address)) = spec.split_once(':') else {
                    return Err(invalid(format!("extra host '{spec}' is not HOST:ADDRESS")));
                };
                if name.trim().is_empty() || address.trim().is_empty() {
                    return Err(invalid(format!("extra host '{spec}' is not HOST:ADDRESS")));
                }
            }
        }
        Some(_) => return Err(invalid("extra_hosts must be a list".into())),
    }

    // A bare entry names a Variable without giving it a value. Compose reads
    // it from the process environment and drops it when unset; here the
    // Application's Variables are that environment.
    let environment = match body.get("environment") {
        None | Some(Value::Null) => None,
        Some(Value::Mapping(entries)) => {
            let mut resolved = Mapping::new();
            for (key, value) in entries {
                if !value.is_null() {
                    resolved.insert(key.clone(), value.clone());
                    continue;
                }
                let name = key_name(key);
                referenced.insert(name.clone());
                if let Some(value) = variable(variables, &name) {
                    resolved.insert(key.clone(), Value::String(value.to_string()));
                }
            }
            Some(Value::Mapping(resolved))
        }
        Some(Value::Sequence(items)) => {
            let mut resolved = Vec::with_capacity(items.len());
            for item in items {
                let Value::String(pair) = item else {
                    return Err(invalid("environment entries must be KEY=value".into()));
                };
                if pair.trim().is_empty() {
                    return Err(invalid("environment entries must be KEY=value".into()));
                }
                if pair.contains('=') {
                    resolved.push(item.clone());
                    continue;
                }
                referenced.insert(pair.clone());
                if let Some(value) = variable(variables, pair) {
                    resolved.push(Value::String(format!("{pair}={value}")));
                }
            }
            Some(Value::Sequence(resolved))
        }
        Some(_) => return Err(invalid("environment must be a list or a mapping".into())),
    };
    if let Some(environment) = environment {
        body.insert("environment".into(), environment);
    }

    Ok(ServiceDefinition {
        name,
        image,
        ports,
        mounts,
        body,
    })
}

/// `C`, `H:C`, `IP:H:C`, each optionally `/tcp` or `/udp`. Ranges are not
/// something the first Application needs, and are refused rather than
/// half-understood.
fn parse_port(spec: &str) -> Option<PublishedPort> {
    let spec = spec.trim().trim_matches('"');
    let without_proto = spec.split('/').next()?;
    let parts: Vec<&str> = without_proto.split(':').collect();
    let (host, container) = match parts.as_slice() {
        [c] => (None, *c),
        [h, c] => (Some(*h), *c),
        [_ip, h, c] => (Some(*h), *c),
        _ => return None,
    };
    let container: u16 = container.parse().ok()?;
    let host = match host {
        Some(h) => Some(h.parse::<u16>().ok()?),
        None => None,
    };
    Some(PublishedPort { host, container })
}

/// `SOURCE:TARGET[:MODE]` into the source and everything after it; a bare
/// `TARGET` has an empty source.
fn split_mount(spec: &str) -> (&str, &str) {
    match spec.split_once(':') {
        Some((source, rest)) if !rest.is_empty() => (source, rest),
        _ => ("", spec),
    }
}

enum MountSource {
    Anonymous,
    Named(String),
    Absolute,
    Relative(PathBuf),
}

/// `~` and `./x` are the Operator's laptop talking. On the Host they mean
/// "the Application's data directory", which is the only place the Platform
/// promises to keep.
fn classify_source(source: &str) -> MountSource {
    if source.is_empty() {
        return MountSource::Anonymous;
    }
    if source.starts_with('/') {
        return MountSource::Absolute;
    }
    if source == "~" {
        return MountSource::Relative(PathBuf::new());
    }
    if let Some(rest) = source.strip_prefix("~/") {
        return MountSource::Relative(PathBuf::from(rest));
    }
    if let Some(rest) = source.strip_prefix("./") {
        return MountSource::Relative(PathBuf::from(rest));
    }
    if source.starts_with("../") || source == "." || source == ".." {
        return MountSource::Relative(PathBuf::from(source));
    }
    MountSource::Named(source.to_string())
}

/// `environment` and `labels` come as a list of `K=V` or a mapping, with
/// values that YAML may have read as numbers or booleans. One shape out:
/// a mapping of strings, in the order written.
fn mapping_of_strings(value: Option<&Value>) -> IndexMap<String, String> {
    let mut out = IndexMap::new();
    match value {
        Some(Value::Sequence(items)) => {
            for item in items {
                if let Value::String(pair) = item {
                    let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                    out.insert(k.to_string(), v.to_string());
                }
            }
        }
        Some(Value::Mapping(m)) => {
            for (k, v) in m {
                out.insert(key_name(k), scalar_to_string(v));
            }
        }
        _ => {}
    }
    out
}

fn scalar_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

fn to_string_mapping(map: &IndexMap<String, String>) -> Value {
    let mut m = Mapping::new();
    for (k, v) in map {
        m.insert(k.clone().into(), Value::String(v.clone()));
    }
    Value::Mapping(m)
}

/// Keys starting with `x-` are Compose extension fields: notes, and the
/// usual home of an anchor other blocks merge from. They carry no runtime
/// meaning and are dropped once the merge has happened.
fn drop_extension_keys(mapping: &mut Mapping) {
    mapping.retain(|key, _| !key_name(key).starts_with("x-"));
}

fn variable<'a>(variables: &'a [(String, String)], name: &str) -> Option<&'a str> {
    variables
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// Every `$` becomes `$$`, so `docker compose` reads the rendered file as
/// literal text and never interpolates it against the daemon's environment.
/// Values only: Compose does not interpolate keys either.
fn escape_dollars(value: &mut Value) {
    match value {
        Value::String(text) if text.contains('$') => *text = text.replace('$', "$$"),
        Value::Sequence(items) => items.iter_mut().for_each(escape_dollars),
        Value::Mapping(entries) => entries.values_mut().for_each(escape_dollars),
        Value::Tagged(tagged) => escape_dollars(&mut tagged.value),
        _ => {}
    }
}

fn is_name_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic()
}

fn is_name_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}

/// The index just past the Variable name that starts at `start`.
fn name_end(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < bytes.len() && is_name_byte(bytes[end]) {
        end += 1;
    }
    end
}

/// The `}` that closes the `{` at `open`, counting a nested `${` as one more
/// level and stepping over `$$` so an escaped dollar opens nothing.
fn closing_brace(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'$' if bytes.get(i + 1) == Some(&b'$') => i += 2,
            b'$' if bytes.get(i + 1) == Some(&b'{') => {
                depth += 1;
                i += 2;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn unreadable(service: Option<&str>, what: String) -> ComposeDefinitionError {
    ComposeDefinitionError::Interpolation {
        service: service.map(str::to_string),
        what,
    }
}

/// Compose's interpolation, read by the Platform against the Application's
/// Variables and nothing else. String values change in place; keys are left
/// alone, as Compose leaves them. Every name looked up is remembered.
struct Interpolator<'a> {
    variables: &'a [(String, String)],
    resolution: Resolution,
    referenced: BTreeSet<String>,
}

impl<'a> Interpolator<'a> {
    fn new(variables: &'a [(String, String)], resolution: Resolution) -> Self {
        Self {
            variables,
            resolution,
            referenced: BTreeSet::new(),
        }
    }

    /// The whole file. Text inside a service reports that service's name
    /// when it cannot be read; text anywhere else reports none.
    fn document(&mut self, top: &mut Mapping) -> Result<(), ComposeDefinitionError> {
        for (key, value) in top.iter_mut() {
            match value {
                Value::Mapping(services) if key_name(key) == "services" => {
                    for (name, body) in services.iter_mut() {
                        let service = key_name(name);
                        self.value(body, Some(&service))?;
                    }
                }
                other => self.value(other, None)?,
            }
        }
        Ok(())
    }

    fn value(
        &mut self,
        value: &mut Value,
        service: Option<&str>,
    ) -> Result<(), ComposeDefinitionError> {
        match value {
            Value::String(text) => *text = self.text(text, service)?,
            Value::Sequence(items) => {
                for item in items {
                    self.value(item, service)?;
                }
            }
            Value::Mapping(entries) => {
                for entry in entries.values_mut() {
                    self.value(entry, service)?;
                }
            }
            Value::Tagged(tagged) => self.value(&mut tagged.value, service)?,
            _ => {}
        }
        Ok(())
    }

    /// One string value. `$$` is a `$`; `$VAR` and `${...}` are references;
    /// any other `$` is refused, so a typo never reaches Docker as text.
    fn text(
        &mut self,
        text: &str,
        service: Option<&str>,
    ) -> Result<String, ComposeDefinitionError> {
        let bytes = text.as_bytes();
        let mut out = String::with_capacity(text.len());
        let mut literal = 0;
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != b'$' {
                i += 1;
                continue;
            }
            out.push_str(&text[literal..i]);
            match bytes.get(i + 1) {
                Some(b'$') => {
                    out.push('$');
                    i += 2;
                }
                Some(b'{') => {
                    let Some(close) = closing_brace(bytes, i + 1) else {
                        return Err(unreadable(
                            service,
                            format!("'{}' has no closing brace", &text[i..]),
                        ));
                    };
                    out.push_str(&self.braced(&text[i..=close], service)?);
                    i = close + 1;
                }
                Some(&byte) if is_name_start(byte) => {
                    let end = name_end(bytes, i + 1);
                    out.push_str(self.lookup(&text[i + 1..end]).unwrap_or_default());
                    i = end;
                }
                Some(_) | None => {
                    let offending: String = text[i..].chars().take(2).collect();
                    return Err(unreadable(
                        service,
                        format!(
                            "'{offending}' is not a Variable reference; write '$$' for a literal dollar"
                        ),
                    ));
                }
            }
            literal = i;
        }
        out.push_str(&text[literal..]);
        Ok(out)
    }

    /// `whole` is `${...}`, braces included. The operators are Compose's:
    /// `:-` and `-` give a default, `:?` and `?` require a value, `:+` and
    /// `+` give an alternative when there is one. With a colon, an empty
    /// value counts as unset. Defaults and alternatives may themselves hold
    /// references.
    fn braced(
        &mut self,
        whole: &str,
        service: Option<&str>,
    ) -> Result<String, ComposeDefinitionError> {
        let inner = &whole[2..whole.len() - 1];
        let (name, rest) = inner.split_at(name_end(inner.as_bytes(), 0));
        if name.is_empty() || !is_name_start(name.as_bytes()[0]) {
            return Err(unreadable(
                service,
                format!("'{whole}' is not a Variable reference"),
            ));
        }
        let value = self.lookup(name);
        let filled = value.is_some_and(|v| !v.is_empty());
        let given = value.unwrap_or_default().to_string();

        let resolved = if rest.is_empty() {
            given
        } else if let Some(default) = rest.strip_prefix(":-") {
            if filled {
                given
            } else {
                self.text(default, service)?
            }
        } else if let Some(default) = rest.strip_prefix('-') {
            match value {
                Some(_) => given,
                None => self.text(default, service)?,
            }
        } else if let Some(message) = rest.strip_prefix(":?") {
            if filled {
                given
            } else {
                self.require(name, message)?
            }
        } else if let Some(message) = rest.strip_prefix('?') {
            match value {
                Some(_) => given,
                None => self.require(name, message)?,
            }
        } else if let Some(alternative) = rest.strip_prefix(":+") {
            if filled {
                self.text(alternative, service)?
            } else {
                String::new()
            }
        } else if let Some(alternative) = rest.strip_prefix('+') {
            match value {
                Some(_) => self.text(alternative, service)?,
                None => String::new(),
            }
        } else {
            return Err(unreadable(
                service,
                format!("'{whole}' is not a Variable reference"),
            ));
        };
        Ok(resolved)
    }

    fn lookup(&mut self, name: &str) -> Option<&'a str> {
        self.referenced.insert(name.to_string());
        variable(self.variables, name)
    }

    /// A required Variable with nothing to give. Under `Check` the Operator
    /// may still be about to set it, so it reads as empty; under `Run` the
    /// render is refused before anything reaches Docker.
    fn require(&self, name: &str, message: &str) -> Result<String, ComposeDefinitionError> {
        match self.resolution {
            Resolution::Check => Ok(String::new()),
            Resolution::Run => Err(ComposeDefinitionError::RequiredVariable {
                name: name.to_string(),
                message: message.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The official Hermes example, as the Operator would paste it.
    const HERMES: &str = r#"
services:
  hermes:
    image: nousresearch/hermes-agent:latest
    container_name: hermes
    restart: unless-stopped
    command: gateway run
    ports:
      - "8642:8642"
      - "9119:9119"
    volumes:
      - ~/.hermes:/opt/data
    environment:
      - HERMES_DASHBOARD=1
    deploy:
      resources:
        limits:
          memory: 4G
          cpus: "2.0"
"#;

    fn hermes() -> ComposeDefinition {
        ComposeDefinition::parse(HERMES).expect("the official example parses")
    }

    fn render(def: &ComposeDefinition) -> ComposeProject {
        def.render(
            "sf-app-k3n8qz4v2x1p",
            Path::new("/cfg/apps/k3n8qz4v2x1p"),
            &[("sf.app.id".into(), "k3n8qz4v2x1p".into())],
            &[],
            VariableDelivery::Referenced,
            None,
        )
    }

    fn pairs(variables: &[(&str, &str)]) -> Vec<(String, String)> {
        variables
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn the_official_hermes_example_is_accepted_as_written() {
        let def = hermes();
        assert_eq!(def.services.len(), 1);
        assert_eq!(def.services[0].image, "nousresearch/hermes-agent:latest");
        assert_eq!(
            def.services[0].ports,
            vec![
                PublishedPort {
                    host: Some(8642),
                    container: 8642
                },
                PublishedPort {
                    host: Some(9119),
                    container: 9119
                },
            ]
        );
    }

    #[test]
    fn every_container_is_named_after_the_application() {
        let project = render(&hermes());
        assert_eq!(
            project.containers,
            vec![(
                "hermes".to_string(),
                "sf-app-k3n8qz4v2x1p-hermes".to_string()
            )]
        );
        // The Operator's `container_name: hermes` would collide with a second
        // Application pasting the same file; ours cannot.
        assert!(
            project
                .yaml
                .contains("container_name: sf-app-k3n8qz4v2x1p-hermes")
        );
        assert!(!project.yaml.contains("container_name: hermes\n"));
    }

    #[test]
    fn a_runtime_hostname_override_changes_only_the_selected_service() {
        let project = hermes().render_with_overrides(
            "sf-app-k3n8qz4v2x1p",
            Path::new("/cfg/apps/k3n8qz4v2x1p"),
            &[("sf.app.id".into(), "k3n8qz4v2x1p".into())],
            &[],
            VariableDelivery::Referenced,
            None,
            &RenderOverrides::service_hostname("hermes", "t3"),
        );

        let doc: Value = serde_yaml::from_str(&project.yaml).unwrap();
        assert_eq!(doc["services"]["hermes"]["hostname"], "t3");
        assert_eq!(
            doc["services"]["hermes"]["container_name"],
            "sf-app-k3n8qz4v2x1p-hermes"
        );
    }

    #[test]
    fn ordinary_compose_rendering_has_no_runtime_hostname_override() {
        let project = render(&hermes());
        let doc: Value = serde_yaml::from_str(&project.yaml).unwrap();
        assert!(doc["services"]["hermes"]["hostname"].is_null());
    }

    #[test]
    fn a_mount_says_where_its_data_will_live() {
        let def = hermes();
        assert_eq!(
            def.services[0].mounts,
            vec![Mount {
                source: "~/.hermes".into(),
                target: "/opt/data".into(),
                kind: MountKind::Data,
                data_path: Some("data/.hermes".into()),
            }]
        );
    }

    #[test]
    fn the_home_directory_lands_under_the_application_data_directory() {
        let project = render(&hermes());
        assert!(
            project
                .yaml
                .contains("/cfg/apps/k3n8qz4v2x1p/data/.hermes:/opt/data"),
            "{}",
            project.yaml
        );
        assert_eq!(
            project.bind_dirs,
            vec![PathBuf::from("/cfg/apps/k3n8qz4v2x1p/data/.hermes")]
        );
    }

    #[test]
    fn services_join_the_application_network_and_their_own() {
        let project = render(&hermes());
        let doc: Value = serde_yaml::from_str(&project.yaml).unwrap();
        let networks = &doc["services"]["hermes"]["networks"];
        assert_eq!(networks[0], "default");
        assert_eq!(networks[1], APP_NETWORK);
        assert_eq!(doc["networks"][APP_NETWORK]["external"], Value::Bool(true));
        // Off the Platform Infra bridge (ADR-0012).
        assert!(!project.yaml.contains("sf-system"));
    }

    #[test]
    fn extra_hosts_is_carried_into_the_rendered_project() {
        let def = ComposeDefinition::parse(
            "services:\n  web:\n    image: nginx\n    extra_hosts:\n      - \"unifi.local:192.168.1.1\"\n      - \"host.docker.internal:host-gateway\"\n",
        )
        .expect("extra_hosts parses");
        let project = render(&def);
        let doc: Value = serde_yaml::from_str(&project.yaml).unwrap();
        let hosts = &doc["services"]["web"]["extra_hosts"];
        assert_eq!(hosts[0], "unifi.local:192.168.1.1");
        assert_eq!(hosts[1], "host.docker.internal:host-gateway");
    }

    #[test]
    fn extra_hosts_refuses_the_mapping_form() {
        let err = ComposeDefinition::parse(
            "services:\n  web:\n    image: nginx\n    extra_hosts:\n      unifi.local: 192.168.1.1\n",
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("extra_hosts must be a list"),
            "{}",
            err
        );
    }

    #[test]
    fn extra_hosts_refuses_an_entry_without_an_address() {
        let err = ComposeDefinition::parse(
            "services:\n  web:\n    image: nginx\n    extra_hosts:\n      - unifi.local\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("is not HOST:ADDRESS"), "{}", err);
    }

    #[test]
    fn extra_hosts_refuses_an_entry_without_a_name() {
        let err = ComposeDefinition::parse(
            "services:\n  web:\n    image: nginx\n    extra_hosts:\n      - \":192.168.1.1\"\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("is not HOST:ADDRESS"), "{}", err);
    }

    #[test]
    fn identity_labels_and_the_service_name_ride_on_every_container() {
        let project = render(&hermes());
        let doc: Value = serde_yaml::from_str(&project.yaml).unwrap();
        let labels = &doc["services"]["hermes"]["labels"];
        assert_eq!(labels["sf.app.id"], "k3n8qz4v2x1p");
        assert_eq!(labels["sf.app.service"], "hermes");
    }

    #[test]
    fn the_web_target_is_published_on_loopback_beside_what_the_operator_declared() {
        let def = hermes();
        let published = PublishedTarget {
            target: def.web_target(None, Some(9119)).unwrap(),
            host_port: 20001,
        };
        let project = def.render(
            "sf-app-x",
            Path::new("/cfg/apps/x"),
            &[],
            &[],
            VariableDelivery::Referenced,
            Some(&published),
        );

        let doc: Value = serde_yaml::from_str(&project.yaml).unwrap();
        let ports = doc["services"]["hermes"]["ports"].as_sequence().unwrap();
        assert_eq!(
            ports
                .iter()
                .map(|p| p.as_str().unwrap())
                .collect::<Vec<_>>(),
            // What the Operator published stays exactly as they wrote it; the
            // Web Target's own publication is added, on loopback.
            ["8642:8642", "9119:9119", "127.0.0.1:20001:9119"]
        );
    }

    #[test]
    fn only_the_web_target_service_is_published_on_the_host() {
        let def = ComposeDefinition::parse(
            "services:\n  web:\n    image: nginx\n    ports:\n      - \"8080:80\"\n  db:\n    image: postgres\n",
        )
        .unwrap();
        let published = PublishedTarget {
            target: def.web_target(Some("web"), Some(80)).unwrap(),
            host_port: 20002,
        };
        let project = def.render(
            "sf-app-x",
            Path::new("/cfg/apps/x"),
            &[],
            &[],
            VariableDelivery::Referenced,
            Some(&published),
        );

        let doc: Value = serde_yaml::from_str(&project.yaml).unwrap();
        assert_eq!(
            doc["services"]["web"]["ports"]
                .as_sequence()
                .unwrap()
                .last()
                .unwrap(),
            "127.0.0.1:20002:80"
        );
        // The database is the Application's business, and reachable only on
        // the Application's own network.
        assert!(doc["services"]["db"]["ports"].is_null());
    }

    #[test]
    fn broadcast_puts_every_variable_on_top_of_the_services_own_environment() {
        let project = hermes().render(
            "sf-app-x",
            Path::new("/cfg/apps/x"),
            &[],
            &[
                ("HERMES_DASHBOARD".into(), "0".into()),
                ("OPENROUTER_API_KEY".into(), "sk-test".into()),
            ],
            VariableDelivery::Broadcast,
            None,
        );
        let doc: Value = serde_yaml::from_str(&project.yaml).unwrap();
        let env = &doc["services"]["hermes"]["environment"];
        assert_eq!(env["HERMES_DASHBOARD"], "0");
        assert_eq!(env["OPENROUTER_API_KEY"], "sk-test");
    }

    #[test]
    fn a_service_without_a_restart_policy_gets_one() {
        let def = ComposeDefinition::parse("services:\n  web:\n    image: nginx\n").unwrap();
        let project = render(&def);
        assert!(project.yaml.contains("restart: unless-stopped"));
    }

    #[test]
    fn named_volumes_are_declared_so_compose_creates_them() {
        let def = ComposeDefinition::parse(
            "services:\n  db:\n    image: postgres\n    volumes:\n      - pgdata:/var/lib/postgresql\n",
        )
        .unwrap();
        let project = render(&def);
        let doc: Value = serde_yaml::from_str(&project.yaml).unwrap();
        assert!(doc["volumes"]["pgdata"].is_mapping());
        assert!(project.bind_dirs.is_empty());
    }

    #[test]
    fn the_web_target_defaults_to_the_first_published_port() {
        let def = hermes();
        assert_eq!(
            def.web_target(None, None).unwrap(),
            WebTarget {
                service: "hermes".into(),
                port: 8642
            }
        );
        assert_eq!(
            def.web_target(None, Some(9119)).unwrap(),
            WebTarget {
                service: "hermes".into(),
                port: 9119
            }
        );
    }

    #[test]
    fn a_web_service_that_is_not_there_is_refused() {
        let err = hermes().web_target(Some("dashboard"), None).unwrap_err();
        assert!(matches!(err, ComposeDefinitionError::UnknownWebService(_)));
    }

    #[test]
    fn a_service_with_no_ports_needs_the_web_port_said() {
        let def = ComposeDefinition::parse("services:\n  web:\n    image: nginx\n").unwrap();
        assert!(matches!(
            def.web_target(None, None),
            Err(ComposeDefinitionError::NoWebPort(_))
        ));
        assert_eq!(def.web_target(None, Some(80)).unwrap().port, 80);
    }

    #[test]
    fn a_platform_port_cannot_be_taken() {
        let err = ComposeDefinition::parse(
            "services:\n  web:\n    image: nginx\n    ports:\n      - \"443:8443\"\n",
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ComposeDefinitionError::ReservedPort { port: 443, .. }
        ));
    }

    #[test]
    fn an_unsupported_key_is_refused_by_name() {
        let err =
            ComposeDefinition::parse("services:\n  web:\n    image: nginx\n    privileged: true\n")
                .unwrap_err();
        assert_eq!(
            err.to_string(),
            "service 'web': 'privileged' is not supported"
        );
    }

    #[test]
    fn a_build_context_is_not_something_the_console_can_supply() {
        let err = ComposeDefinition::parse("services:\n  web:\n    build: .\n").unwrap_err();
        assert!(matches!(err, ComposeDefinitionError::UnsupportedKey { .. }));
    }

    #[test]
    fn operator_networks_are_refused_because_the_platform_owns_them() {
        let err = ComposeDefinition::parse(
            "services:\n  web:\n    image: nginx\nnetworks:\n  sf-system:\n    external: true\n",
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ComposeDefinitionError::UnsupportedTopLevel(_)
        ));
    }

    #[test]
    fn a_relative_path_cannot_climb_out_of_the_data_directory() {
        let err = ComposeDefinition::parse(
            "services:\n  web:\n    image: nginx\n    volumes:\n      - ../../etc:/etc/x\n",
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("leaves the Application's directory")
        );
    }

    #[test]
    fn an_absolute_host_path_is_passed_through() {
        let def = ComposeDefinition::parse(
            "services:\n  web:\n    image: nginx\n    volumes:\n      - /srv/music:/music:ro\n",
        )
        .unwrap();
        let project = render(&def);
        assert!(project.yaml.contains("/srv/music:/music:ro"));
        assert!(project.bind_dirs.is_empty());
    }

    #[test]
    fn environment_written_as_a_mapping_keeps_numbers_as_strings() {
        let def = ComposeDefinition::parse(
            "services:\n  web:\n    image: nginx\n    environment:\n      PORT: 8080\n      DEBUG: true\n",
        )
        .unwrap();
        let project = render(&def);
        let doc: Value = serde_yaml::from_str(&project.yaml).unwrap();
        assert_eq!(doc["services"]["web"]["environment"]["PORT"], "8080");
        assert_eq!(doc["services"]["web"]["environment"]["DEBUG"], "true");
    }

    #[test]
    fn not_yaml_says_so() {
        let err = ComposeDefinition::parse("services: [\n").unwrap_err();
        assert!(matches!(err, ComposeDefinitionError::Yaml(_)));
    }

    #[test]
    fn nothing_to_run_says_so() {
        assert!(matches!(
            ComposeDefinition::parse("services: {}\n"),
            Err(ComposeDefinitionError::NoServices)
        ));
        assert!(matches!(
            ComposeDefinition::parse("version: '3'\n"),
            Err(ComposeDefinitionError::NoServices)
        ));
    }

    // Variables (ADR-0030).

    /// One row of the Variables fixture: what the Operator wrote, which
    /// Variables the Application had, how strictly they were read, and what
    /// came out the other side.
    struct Case {
        name: &'static str,
        yaml: String,
        variables: &'static [(&'static str, &'static str)],
        resolution: Resolution,
        delivery: VariableDelivery,
        expect: Expect,
    }

    enum Expect {
        /// The string at this path in the rendered project.
        Value(&'static [&'static str], &'static str),
        /// Nothing at this path in the rendered project.
        Absent(&'static [&'static str]),
        /// `parse_with` refuses, and the message contains this.
        Error(&'static str),
    }

    /// A one-service file with `body` indented under the service.
    fn web(body: &str) -> String {
        format!("services:\n  web:\n    image: nginx\n{body}")
    }

    fn at<'a>(doc: &'a Value, path: &[&str]) -> &'a Value {
        path.iter().fold(doc, |value, segment| &value[*segment])
    }

    const COMMAND: &[&str] = &["services", "web", "command"];
    const GREETING: &[&str] = &["services", "web", "environment", "GREETING"];
    const SECRET: &[&str] = &["services", "web", "environment", "SECRET"];

    fn check(
        name: &'static str,
        body: &str,
        variables: &'static [(&str, &str)],
        expect: Expect,
    ) -> Case {
        Case {
            name,
            yaml: web(body),
            variables,
            resolution: Resolution::Check,
            delivery: VariableDelivery::Referenced,
            expect,
        }
    }

    fn run(
        name: &'static str,
        body: &str,
        variables: &'static [(&str, &str)],
        expect: Expect,
    ) -> Case {
        Case {
            resolution: Resolution::Run,
            ..check(name, body, variables, expect)
        }
    }

    #[test]
    fn variables_resolve_the_way_compose_documents_and_never_from_the_daemon() {
        use Expect::{Absent, Error, Value as Is};
        let hi: &[(&str, &str)] = &[("GREETING", "hi")];
        let empty: &[(&str, &str)] = &[("GREETING", "")];

        // The daemon's own environment carries a variable of the same name
        // as one the file references. It must not be what the file reads.
        // SAFETY: no other test in this binary reads or writes this name.
        unsafe { std::env::set_var("SF_D1_LEAK_TEST", "leaked from the daemon") };

        let cases = vec![
            check("unset without default reads empty in check", "    command: ${GREETING}\n", &[], Is(COMMAND, "")),
            run("unset without default reads empty in run", "    command: ${GREETING}\n", &[], Is(COMMAND, "")),
            check("colon dash keeps a set variable", "    command: ${GREETING:-hello}\n", hi, Is(COMMAND, "hi")),
            check("colon dash gives the default when unset", "    command: ${GREETING:-hello}\n", &[], Is(COMMAND, "hello")),
            check("colon dash gives the default when empty", "    command: ${GREETING:-hello}\n", empty, Is(COMMAND, "hello")),
            check("dash keeps a set variable", "    command: ${GREETING-hello}\n", hi, Is(COMMAND, "hi")),
            check("dash gives the default when unset", "    command: ${GREETING-hello}\n", &[], Is(COMMAND, "hello")),
            check("dash keeps an empty variable", "    command: ${GREETING-hello}\n", empty, Is(COMMAND, "")),
            check("colon question reads empty in check", "    command: ${GREETING:?say hello}\n", &[], Is(COMMAND, "")),
            run("colon question refuses an unset variable in run", "    command: ${GREETING:?say hello}\n", &[], Error("required Variable 'GREETING' has no value: say hello")),
            run("colon question refuses an empty variable in run", "    command: ${GREETING:?say hello}\n", empty, Error("required Variable 'GREETING' has no value: say hello")),
            run("colon question keeps a set variable in run", "    command: ${GREETING:?say hello}\n", hi, Is(COMMAND, "hi")),
            check("question reads empty in check", "    command: ${GREETING?say hello}\n", &[], Is(COMMAND, "")),
            run("question refuses an unset variable in run", "    command: ${GREETING?say hello}\n", &[], Error("required Variable 'GREETING' has no value: say hello")),
            run("question accepts an empty variable in run", "    command: ${GREETING?say hello}\n", empty, Is(COMMAND, "")),
            check("colon plus gives the alternative when set", "    command: ${GREETING:+yes}\n", hi, Is(COMMAND, "yes")),
            check("colon plus gives nothing when empty", "    command: ${GREETING:+yes}\n", empty, Is(COMMAND, "")),
            check("plus gives the alternative when empty", "    command: ${GREETING+yes}\n", empty, Is(COMMAND, "yes")),
            check("plus gives nothing when unset", "    command: ${GREETING+yes}\n", &[], Is(COMMAND, "")),
            check("a doubled dollar reaches the render doubled", "    command: echo $$HOME\n", &[], Is(COMMAND, "echo $$HOME")),
            check("the bare form resolves", "    command: $GREETING\n", hi, Is(COMMAND, "hi")),
            check("a digit after the dollar is refused", "    command: cost $1\n", &[], Error("service 'web': '$1' is not a Variable reference")),
            check("a space after the dollar is refused", "    command: \"cost $ 1\"\n", &[], Error("service 'web': '$ ' is not a Variable reference")),
            check("an unclosed brace is refused", "    command: \"${GREETING\"\n", &[], Error("service 'web': '${GREETING' has no closing brace")),
            check("a default may hold another reference", "    command: ${A:-${B}}\n", &[("B", "fallback")], Is(COMMAND, "fallback")),
            Case {
                name: "a merge key pulls an anchor out of an extension block",
                yaml: "x-common: &common\n  restart: always\nservices:\n  web:\n    <<: *common\n    image: nginx\n".into(),
                variables: &[],
                resolution: Resolution::Check,
                delivery: VariableDelivery::Referenced,
                expect: Is(&["services", "web", "restart"], "always"),
            },
            check("an extension key on a service is dropped", "    x-note: for the Operator\n", &[], Absent(&["services", "web", "x-note"])),
            check("a bare entry takes the variable when set", "    environment:\n      - GREETING\n", hi, Is(GREETING, "hi")),
            check("a bare entry is dropped when unset", "    environment:\n      - GREETING\n", &[], Absent(GREETING)),
            check("a mapping entry with no value takes the variable when set", "    environment:\n      GREETING:\n", hi, Is(GREETING, "hi")),
            check("a mapping entry with no value is dropped when unset", "    environment:\n      GREETING:\n", &[], Absent(GREETING)),
            Case {
                name: "broadcast delivers a variable the file does not mention",
                yaml: web(""),
                variables: &[("SECRET", "x")],
                resolution: Resolution::Run,
                delivery: VariableDelivery::Broadcast,
                expect: Is(SECRET, "x"),
            },
            Case {
                name: "referenced delivers nothing the file does not mention",
                yaml: web(""),
                variables: &[("SECRET", "x")],
                resolution: Resolution::Run,
                delivery: VariableDelivery::Referenced,
                expect: Absent(SECRET),
            },
            check("a volume driver is refused", "    volumes:\n      - data:/data\nvolumes:\n  data:\n    driver: local\n", &[], Error("volume 'data': 'driver' is not supported yet")),
            check("a mount mode of z is refused", "    volumes:\n      - ./x:/x:z\n", &[], Error("service 'web': volume './x:/x:z': mount mode 'z' is not supported")),
            check("a daemon variable of the same name does not leak", "    command: ${SF_D1_LEAK_TEST}\n", &[], Is(COMMAND, "")),
        ];

        for case in cases {
            let variables = pairs(case.variables);
            let parsed = ComposeDefinition::parse_with(&case.yaml, &variables, case.resolution);
            let rendered = |def: ComposeDefinition| -> Value {
                let project = def.render(
                    "sf-app-x",
                    Path::new("/cfg/apps/x"),
                    &[],
                    &variables,
                    case.delivery,
                    None,
                );
                serde_yaml::from_str(&project.yaml).unwrap()
            };
            match case.expect {
                Error(needle) => {
                    let err = parsed
                        .err()
                        .unwrap_or_else(|| panic!("{}: expected a refusal", case.name));
                    assert!(err.to_string().contains(needle), "{}: {err}", case.name);
                }
                Is(path, want) => {
                    let doc = rendered(parsed.unwrap_or_else(|e| panic!("{}: {e}", case.name)));
                    assert_eq!(
                        at(&doc, path),
                        &Value::String(want.into()),
                        "{}: {doc:?}",
                        case.name
                    );
                }
                Absent(path) => {
                    let doc = rendered(parsed.unwrap_or_else(|e| panic!("{}: {e}", case.name)));
                    assert!(at(&doc, path).is_null(), "{}: {doc:?}", case.name);
                }
            }
        }
    }

    #[test]
    fn referenced_variables_lists_every_name_the_file_asks_for() {
        let def = ComposeDefinition::parse(&web(
            "    command: $CMD ${A:-${B}}\n    environment:\n      - PORT\n      - FIXED=1\n",
        ))
        .unwrap();
        assert_eq!(
            def.referenced_variables().into_iter().collect::<Vec<_>>(),
            ["A", "B", "CMD", "PORT"]
        );
    }

    #[test]
    fn a_doubled_dollar_is_one_in_the_model_and_two_in_the_render() {
        let def = ComposeDefinition::parse(&web("    command: echo $$HOME\n")).unwrap();
        assert_eq!(
            def.services[0].body.get("command"),
            Some(&Value::String("echo $HOME".into()))
        );
        let doc: Value = serde_yaml::from_str(&render(&def).yaml).unwrap();
        assert_eq!(doc["services"]["web"]["command"], "echo $$HOME");
    }

    #[test]
    fn a_broadcast_variable_holding_a_dollar_is_escaped_too() {
        let def = ComposeDefinition::parse(&web("")).unwrap();
        let project = def.render(
            "sf-app-x",
            Path::new("/cfg/apps/x"),
            &[],
            &[("PASSWORD".into(), "pa$s".into())],
            VariableDelivery::Broadcast,
            None,
        );
        let doc: Value = serde_yaml::from_str(&project.yaml).unwrap();
        assert_eq!(doc["services"]["web"]["environment"]["PASSWORD"], "pa$$s");
    }

    #[test]
    fn an_unreadable_dollar_outside_every_service_names_none() {
        let err = ComposeDefinition::parse("name: $1\nservices:\n  web:\n    image: nginx\n")
            .unwrap_err();
        assert!(
            matches!(
                err,
                ComposeDefinitionError::Interpolation { service: None, .. }
            ),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            "'$1' is not a Variable reference; write '$$' for a literal dollar"
        );
    }

    #[test]
    fn a_required_variable_without_a_message_still_says_which_one() {
        let err =
            ComposeDefinition::parse_with(&web("    command: ${TOKEN:?}\n"), &[], Resolution::Run)
                .unwrap_err();
        assert!(matches!(
            err,
            ComposeDefinitionError::RequiredVariable { .. }
        ));
        assert_eq!(err.to_string(), "required Variable 'TOKEN' has no value");
    }

    #[test]
    fn every_volume_option_the_platform_does_not_carry_is_refused_by_name() {
        for key in UNSUPPORTED_VOLUME_KEYS {
            let yaml = format!(
                "services:\n  db:\n    image: postgres\n    volumes:\n      - data:/var/lib/postgresql\nvolumes:\n  data:\n    {key}: local\n"
            );
            let err = ComposeDefinition::parse(&yaml).unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("volume 'data': '{key}' is not supported yet")
            );
        }
    }

    #[test]
    fn mount_modes_other_than_ro_and_rw_are_refused_by_name() {
        for mode in [
            "z",
            "Z",
            "cached",
            "delegated",
            "consistent",
            "nocopy",
            "ro,z",
        ] {
            let err =
                ComposeDefinition::parse(&web(&format!("    volumes:\n      - ./x:/x:{mode}\n")))
                    .unwrap_err();
            assert!(
                matches!(err, ComposeDefinitionError::Invalid { .. }),
                "{err}"
            );
            assert!(err.to_string().contains("mount mode"), "{err}");
        }
        for mode in ["ro", "rw"] {
            ComposeDefinition::parse(&web(&format!("    volumes:\n      - ./x:/x:{mode}\n")))
                .unwrap_or_else(|e| panic!("{mode}: {e}"));
        }
    }
}
