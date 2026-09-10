//! Operator-supplied Compose definitions (ADR-0014, ADR-0015).
//!
//! The Operator pastes a Compose file into the console. The Platform does not
//! run it as written: it reads the supported subset, refuses the rest by
//! name, and renders a project of its own that names every container after
//! the Application, joins the Application network and keeps data where the
//! Platform can find it. What is supported is written down in
//! `docs/compose-applications.md`; this module is that document as code.

use indexmap::IndexMap;
use serde_yaml::{Mapping, Value};
use std::path::{Path, PathBuf};

use crate::docker::APP_NETWORK;

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

#[derive(Debug)]
pub enum ComposeDefinitionError {
    Yaml(String),
    NoServices,
    UnsupportedTopLevel(String),
    InvalidServiceName(String),
    MissingImage(String),
    UnsupportedKey { service: String, key: String },
    Invalid { service: String, what: String },
    ReservedPort { service: String, port: u16 },
    ExternalVolume(String),
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

/// The Operator's Compose file, read and checked, not yet rendered.
#[derive(Debug, Clone)]
pub struct ComposeDefinition {
    pub services: Vec<ServiceDefinition>,
    declared_volumes: Vec<String>,
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
    pub fn parse(yaml: &str) -> Result<Self, ComposeDefinitionError> {
        let doc: Value =
            serde_yaml::from_str(yaml).map_err(|e| ComposeDefinitionError::Yaml(e.to_string()))?;
        let Value::Mapping(top) = doc else {
            return Err(ComposeDefinitionError::Yaml(
                "expected a mapping with a 'services' key".into(),
            ));
        };

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
            parsed.push(parse_service(name, body.clone())?);
        }

        let declared_volumes = match top.get("volumes") {
            None | Some(Value::Null) => vec![],
            Some(Value::Mapping(m)) => {
                let mut names = Vec::with_capacity(m.len());
                for (name, body) in m {
                    let name = key_name(name);
                    if let Value::Mapping(b) = body
                        && b.get("external").is_some_and(|v| v.as_bool() == Some(true))
                    {
                        return Err(ComposeDefinitionError::ExternalVolume(name));
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
        })
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
    /// identity labels, the Application network next to the project's own,
    /// a restart policy if it had none, and the Platform's environment on top
    /// of its own. Host paths are pinned under the Application's directory.
    /// `published` is the Web Target's Host port, when it has one. Only that
    /// one service gets it, and only on loopback: the rest of the project
    /// stays reachable on the Application's own network and nowhere else.
    /// Ports the Operator published themselves are left exactly as written.
    pub fn render(
        &self,
        name: &str,
        dir: &Path,
        labels: &[(String, String)],
        env: &[(String, String)],
        published: Option<&PublishedTarget>,
    ) -> ComposeProject {
        self.render_with_overrides(
            name,
            dir,
            labels,
            env,
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
    pub fn render_with_overrides(
        &self,
        name: &str,
        dir: &Path,
        labels: &[(String, String)],
        env: &[(String, String)],
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
            for (k, v) in env {
                environment.insert(k.clone(), v.clone());
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

        ComposeProject {
            name: name.to_string(),
            dir: dir.to_path_buf(),
            yaml: serde_yaml::to_string(&Value::Mapping(top))
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

fn parse_service(name: String, body: Mapping) -> Result<ServiceDefinition, ComposeDefinitionError> {
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
                let target = rest.split(':').next().unwrap_or(rest);
                if target.is_empty() {
                    return Err(invalid(format!("volume '{spec}' has no container path")));
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

    match body.get("environment") {
        None | Some(Value::Null) | Some(Value::Mapping(_)) => {}
        Some(Value::Sequence(items)) => {
            for item in items {
                let Value::String(pair) = item else {
                    return Err(invalid("environment entries must be KEY=value".into()));
                };
                if pair.trim().is_empty() {
                    return Err(invalid("environment entries must be KEY=value".into()));
                }
            }
        }
        Some(_) => return Err(invalid("environment must be a list or a mapping".into())),
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
            None,
        )
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
    fn platform_environment_goes_on_top_of_the_services_own() {
        let project = hermes().render(
            "sf-app-x",
            Path::new("/cfg/apps/x"),
            &[],
            &[
                ("HERMES_DASHBOARD".into(), "0".into()),
                ("OPENROUTER_API_KEY".into(), "sk-test".into()),
            ],
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
            "services:\n  web:\n    image: nginx\n    volumes:\n      - /Users/seba/Music:/music:ro\n",
        )
        .unwrap();
        let project = render(&def);
        assert!(project.yaml.contains("/Users/seba/Music:/music:ro"));
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
}
