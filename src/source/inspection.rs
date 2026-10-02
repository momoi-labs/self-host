//! Read build metadata from the selected commit without executing repository code.

use super::{GitSource, SourceError, Workspace, contained_path, invalid};
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildFileKind {
    Dockerfile,
    Compose,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildFile {
    pub kind: BuildFileKind,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Service {
    pub name: String,
    pub ports: Vec<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitInspection {
    pub revision: String,
    pub git_ref: String,
    pub build_files: Vec<BuildFile>,
    pub ports: Vec<u16>,
    pub services: Vec<Service>,
}

/// Uses the same isolated Git transport and checkout rules as a build. The
/// response contains only commit, paths and port numbers, never manifest values.
pub async fn inspect(root: &Path, source: &GitSource) -> Result<GitInspection, SourceError> {
    super::validate(source)?;
    super::private_directory(root)?;
    let workspace = Workspace(root.join(format!("work-{:032x}", rand::random::<u128>())));
    super::private_directory(&workspace.0)?;
    let (checkout, revision) = super::checkout(&workspace.0, source, None, root).await?;
    let mut result = inspect_checkout(&checkout, source)?;
    result.revision = revision;
    Ok(result)
}

fn regular_file(checkout: &Path, path: &str) -> Result<Option<PathBuf>, SourceError> {
    // The checkout has already rejected every symlink. A missing optional
    // default is different from a configured path that cannot be read.
    match std::fs::symlink_metadata(checkout.join(path)) {
        Ok(_) => {
            let path = contained_path(checkout, path)?;
            if !path.is_file() || std::fs::metadata(&path)?.len() > 1048576 {
                return Err(invalid("build definition must be a file below 1 MiB"));
            }
            Ok(Some(path))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn inspect_checkout(checkout: &Path, source: &GitSource) -> Result<GitInspection, SourceError> {
    let canonical_checkout = checkout.canonicalize()?;
    let checkout = canonical_checkout.as_path();
    let mut files = Vec::new();
    let mut ports = BTreeSet::new();
    let mut services = Vec::new();
    if let Some(path) = regular_file(checkout, &source.dockerfile)? {
        if source.compose_path.is_none() {
            ports.extend(dockerfile_ports(&std::fs::read_to_string(path)?)?);
        }
        files.push(BuildFile {
            kind: BuildFileKind::Dockerfile,
            path: source.dockerfile.clone(),
        });
    } else if source.dockerfile != "Dockerfile" {
        return Err(invalid("configured Dockerfile does not exist"));
    }
    let candidates: Vec<&str> = match source.compose_path.as_deref() {
        Some(path) => vec![path],
        None => vec![
            "compose.yaml",
            "compose.yml",
            "docker-compose.yaml",
            "docker-compose.yml",
        ],
    };
    for candidate in candidates {
        if let Some(path) = regular_file(checkout, candidate)? {
            let found = compose_services(checkout, &path, source.compose_path.is_some())?;
            if source.compose_path.is_some() {
                ports.extend(
                    found
                        .iter()
                        .flat_map(|service| service.ports.iter().copied()),
                );
            }
            services.extend(found);
            files.push(BuildFile {
                kind: BuildFileKind::Compose,
                path: candidate.into(),
            });
        } else if source.compose_path.is_some() {
            return Err(invalid("configured Compose file does not exist"));
        }
    }
    services.sort_by(|left, right| left.name.cmp(&right.name));
    services.dedup();
    Ok(GitInspection {
        revision: String::new(),
        git_ref: source.git_ref.clone(),
        build_files: files,
        ports: ports.into_iter().collect(),
        services,
    })
}

fn tcp_port(value: &str) -> Option<u16> {
    let (port, protocol) = value.split_once('/').unwrap_or((value, "tcp"));
    if !protocol.eq_ignore_ascii_case("tcp") || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    port.parse::<u16>().ok().filter(|port| *port != 0)
}

fn dockerfile_ports(text: &str) -> Result<Vec<u16>, SourceError> {
    let mut ports = BTreeSet::new();
    for instruction in super::dockerfile_instructions(text)? {
        let (name, body) = instruction
            .split_once(char::is_whitespace)
            .unwrap_or((&instruction, ""));
        match name.to_ascii_uppercase().as_str() {
            // Only the final stage is the Application image. Earlier build
            // stages do not provide a Web Target.
            "FROM" => ports.clear(),
            "EXPOSE" => ports.extend(body.split_whitespace().filter_map(tcp_port)),
            _ => {}
        }
    }
    Ok(ports.into_iter().collect())
}

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn declared_port(value: &Value, published: bool) -> Option<u16> {
    if let Some(value) = scalar(value) {
        return tcp_port(if published {
            value.rsplit(':').next()?
        } else {
            &value
        });
    }
    let protocol = value
        .get("protocol")
        .and_then(Value::as_str)
        .unwrap_or("tcp");
    if !protocol.eq_ignore_ascii_case("tcp") {
        return None;
    }
    tcp_port(&scalar(value.get("target")?)?)
}

fn relative_to(checkout: &Path, directory: &Path, value: &str) -> Result<PathBuf, SourceError> {
    super::relative_path(value)?;
    let path = directory.join(value);
    let relative = path
        .strip_prefix(checkout)
        .map_err(|_| invalid("Compose build path escapes checkout"))?
        .to_string_lossy();
    contained_path(checkout, if relative.is_empty() { "." } else { &relative })
}

fn compose_services(
    checkout: &Path,
    path: &Path,
    resolve_build: bool,
) -> Result<Vec<Service>, SourceError> {
    let mut document: Value = serde_yaml::from_str(&std::fs::read_to_string(path)?)
        .map_err(|_| invalid("Compose file is not valid YAML"))?;
    document
        .apply_merge()
        .map_err(|_| invalid("Compose merge is invalid"))?;
    let services = document
        .get("services")
        .and_then(Value::as_mapping)
        .ok_or_else(|| invalid("Compose file needs services"))?;
    let directory = path
        .parent()
        .ok_or_else(|| invalid("Compose file has no directory"))?;
    let mut result = Vec::new();
    for (name, service) in services {
        let name = name
            .as_str()
            .filter(|name| {
                !name.is_empty()
                    && name.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'-' | b'_')
                    })
            })
            .ok_or_else(|| invalid("Compose service name is invalid"))?;
        if !service.is_mapping() {
            return Err(invalid("Compose service must be a mapping"));
        }
        let mut ports = BTreeSet::new();
        for (key, published) in [("ports", true), ("expose", false)] {
            if let Some(values) = service.get(key).and_then(Value::as_sequence) {
                ports.extend(
                    values
                        .iter()
                        .filter_map(|value| declared_port(value, published)),
                );
            }
        }
        if let Some(build) = service.get("build").filter(|_| resolve_build) {
            let (context, dockerfile) = match build {
                Value::String(context) => (context.clone(), "Dockerfile".into()),
                Value::Mapping(_) => (
                    build
                        .get("context")
                        .map(|value| {
                            scalar(value)
                                .ok_or_else(|| invalid("Compose build context must be a path"))
                        })
                        .transpose()?
                        .unwrap_or_else(|| ".".into()),
                    build
                        .get("dockerfile")
                        .map(|value| {
                            scalar(value)
                                .ok_or_else(|| invalid("Compose Dockerfile must be a path"))
                        })
                        .transpose()?
                        .unwrap_or_else(|| "Dockerfile".into()),
                ),
                _ => return Err(invalid("Compose build must be a path or mapping")),
            };
            let context = relative_to(checkout, directory, &context)?;
            if !context.is_dir() {
                return Err(invalid("Compose build context must be a directory"));
            }
            let dockerfile = relative_to(checkout, &context, &dockerfile)?;
            if !dockerfile.is_file() || std::fs::metadata(&dockerfile)?.len() > 1048576 {
                return Err(invalid("Dockerfile must be a file below 1 MiB"));
            }
            if ports.is_empty() {
                ports.extend(dockerfile_ports(&std::fs::read_to_string(dockerfile)?)?);
            }
        }
        result.push(Service {
            name: name.into(),
            ports: ports.into_iter().collect(),
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_only_literal_tcp_ports_in_the_final_stage() {
        assert_eq!(dockerfile_ports("FROM alpine AS build\nEXPOSE 9000\nFROM alpine\nEXPOSE 8080 8443/tcp 5353/udp $PORT 0 70000\nUSER 1001\n").unwrap(), vec![8080, 8443]);
        assert_eq!(
            declared_port(&Value::String("127.0.0.1:80:8080/tcp".into()), true),
            Some(8080)
        );
        assert_eq!(
            declared_port(&Value::String("53:5353/udp".into()), true),
            None
        );
        let long: Value =
            serde_yaml::from_str("target: 8443\npublished: 443\nprotocol: tcp\n").unwrap();
        assert_eq!(declared_port(&long, true), Some(8443));
    }
}
