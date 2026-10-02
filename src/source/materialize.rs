use super::{GitSource, SourceError, contained_path, dockerfile_policy, invalid, nonroot_user};
use crate::docker::{DockerRuntime, SourceBuild};
use serde_yaml::{Mapping, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn relative_to(checkout: &Path, directory: &Path, value: &str) -> Result<PathBuf, SourceError> {
    super::relative_path(value)?;
    let path = directory
        .join(value)
        .strip_prefix(checkout)
        .map_err(|_| invalid("Compose path escapes checkout"))?
        .to_string_lossy()
        .into_owned();
    contained_path(checkout, if path.is_empty() { "." } else { &path })
}

fn string(value: &Value, what: &str) -> Result<String, SourceError> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        _ => Err(invalid(format!("{what} must be a string"))),
    }
}

fn env_file(path: &Path) -> Result<Mapping, SourceError> {
    if !path.is_file() || std::fs::metadata(path)?.len() > 1048576 {
        return Err(invalid("env_file must be a file below 1 MiB"));
    }
    let text = std::fs::read_to_string(path)?;
    let mut result = Mapping::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, value) = line
            .split_once('=')
            .ok_or_else(|| invalid("env_file supports NAME=value lines only"))?;
        let name = name.trim();
        if name.is_empty()
            || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
            || name.as_bytes()[0].is_ascii_digit()
        {
            return Err(invalid("env_file Variable name is invalid"));
        }
        let value = value.trim();
        let value = if value.starts_with(['\'', '"']) {
            if value.len() < 2 || value.chars().last() != value.chars().next() {
                return Err(invalid("env_file has an unfinished quoted value"));
            }
            &value[1..value.len() - 1]
        } else {
            value
        };
        // Repository files never read Variables from the daemon environment.
        result.insert(
            Value::String(name.into()),
            Value::String(value.replace('$', "$$")),
        );
    }
    Ok(result)
}

fn environment(value: Option<Value>) -> Result<Mapping, SourceError> {
    match value {
        None | Some(Value::Null) => Ok(Mapping::new()),
        Some(Value::Mapping(mapping)) => Ok(mapping),
        Some(Value::Sequence(values)) => {
            let mut result = Mapping::new();
            for value in values {
                let value = string(&value, "environment entry")?;
                let (name, value) = value
                    .split_once('=')
                    .ok_or_else(|| invalid("Git Compose environment entries need NAME=value"))?;
                result.insert(Value::String(name.into()), Value::String(value.into()));
            }
            Ok(result)
        }
        _ => Err(invalid("Compose environment must be a mapping or list")),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn compose(
    checkout: &Path,
    compose_path: &str,
    source: &GitSource,
    app_id: &str,
    revision: &str,
    secrets: &BTreeMap<String, PathBuf>,
    registry: Option<&Path>,
    docker: &(impl DockerRuntime + ?Sized),
    images: &mut BTreeMap<String, String>,
) -> Result<String, SourceError> {
    let canonical_checkout = checkout.canonicalize()?;
    let checkout = canonical_checkout.as_path();
    let path = contained_path(checkout, compose_path)?;
    if !path.is_file() || std::fs::metadata(&path)?.len() > 1048576 {
        return Err(invalid("Compose definition must be a file below 1 MiB"));
    }
    let directory = path
        .parent()
        .ok_or_else(|| invalid("Compose file has no directory"))?;
    let mut document: Value = serde_yaml::from_str(&std::fs::read_to_string(&path)?)
        .map_err(|_| invalid("Compose file is not valid YAML"))?;
    document
        .apply_merge()
        .map_err(|_| invalid("Compose merge is invalid"))?;
    let services = document
        .get_mut("services")
        .and_then(Value::as_mapping_mut)
        .ok_or_else(|| invalid("Compose file needs services"))?;
    for (name, service) in services {
        let name = string(name, "service name")?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_')
        {
            return Err(invalid("Compose service name is invalid"));
        }
        let service = service
            .as_mapping_mut()
            .ok_or_else(|| invalid("Compose service must be a mapping"))?;
        let build = service.remove("build");
        if let Some(user) = service.get("user") {
            if !nonroot_user(&string(user, "Compose user")?) {
                return Err(invalid(
                    "Git Compose user must be a numeric nonzero uid and optional nonzero gid",
                ));
            }
        } else if build.is_none() {
            return Err(invalid(
                "registry-only Git Compose services need an explicit numeric nonzero user",
            ));
        }
        if let Some(files) = service.remove("env_file") {
            let files = match files {
                Value::String(value) => vec![Value::String(value)],
                Value::Sequence(values) => values,
                _ => return Err(invalid("env_file supports a path or list of paths")),
            };
            let mut values = Mapping::new();
            for file in files {
                values.extend(env_file(&relative_to(
                    checkout,
                    directory,
                    &string(&file, "env_file path")?,
                )?)?);
            }
            values.extend(environment(service.remove("environment"))?);
            service.insert(Value::String("environment".into()), Value::Mapping(values));
        }
        if let Some(build) = build {
            let (context, dockerfile, mut args) = match build {
                Value::String(context) => (context, "Dockerfile".into(), BTreeMap::new()),
                Value::Mapping(mut build) => {
                    for key in build.keys() {
                        if !matches!(key.as_str(), Some("context" | "dockerfile" | "args")) {
                            return Err(invalid(
                                "Compose build supports context, dockerfile and args only",
                            ));
                        }
                    }
                    let context = build
                        .remove("context")
                        .map(|value| string(&value, "build context"))
                        .transpose()?
                        .unwrap_or_else(|| ".".into());
                    let dockerfile = build
                        .remove("dockerfile")
                        .map(|value| string(&value, "Dockerfile path"))
                        .transpose()?
                        .unwrap_or_else(|| "Dockerfile".into());
                    let mut args = BTreeMap::new();
                    if let Some(value) = build.remove("args") {
                        match value {
                            Value::Mapping(values) => {
                                for (name, value) in values {
                                    args.insert(
                                        string(&name, "build argument")?,
                                        string(&value, "build argument value")?,
                                    );
                                }
                            }
                            Value::Sequence(values) => {
                                for value in values {
                                    let value = string(&value, "build argument")?;
                                    let (name, value) = value.split_once('=').ok_or_else(|| {
                                        invalid("build args need explicit NAME=value")
                                    })?;
                                    args.insert(name.into(), value.into());
                                }
                            }
                            _ => return Err(invalid("build args must be a mapping or list")),
                        }
                    }
                    (context, dockerfile, args)
                }
                _ => return Err(invalid("build must be a path or mapping")),
            };
            let context = relative_to(checkout, directory, &context)?;
            let dockerfile = relative_to(checkout, &context, &dockerfile)?;
            if !context.is_dir()
                || !dockerfile.is_file()
                || std::fs::metadata(&dockerfile)?.len() > 1048576
            {
                return Err(invalid(
                    "Compose build context must be a directory and Dockerfile a file below 1 MiB",
                ));
            }
            args.extend(source.build_args.clone());
            if args.len() > 128
                || args
                    .values()
                    .any(|value| value.len() > 16384 || value.contains('\0'))
            {
                return Err(invalid("Compose build arguments exceed the supported size"));
            }
            if args.contains_key("BUILDKIT_SYNTAX") {
                return Err(invalid(
                    "BUILDKIT_SYNTAX cannot replace the checked Dockerfile frontend",
                ));
            }
            if args.keys().any(|name| !super::valid_input_name(name)) {
                return Err(invalid("build argument name is invalid"));
            }
            let base_images = dockerfile_policy(&std::fs::read_to_string(&dockerfile)?)?;
            let request = SourceBuild {
                context,
                dockerfile,
                tag: format!(
                    "sf-source-{app_id}:{}-{:016x}",
                    &revision[..12],
                    rand::random::<u64>()
                ),
                build_args: args,
                secret_files: secrets.clone(),
                registry_config: registry.map(Path::to_path_buf),
                base_images,
            };
            let image = docker.build_source(&request).await?;
            service.insert(Value::String("image".into()), Value::String(image.clone()));
            images.insert(name, image);
        } else {
            let image = service
                .get("image")
                .ok_or_else(|| invalid("Compose service needs an image or build"))?;
            let image = docker
                .pin_source_image(&string(image, "service image")?, registry)
                .await?;
            service.insert(Value::String("image".into()), Value::String(image.clone()));
            images.insert(name, image);
        }
    }
    let text = serde_yaml::to_string(&document)
        .map_err(|_| invalid("Compose file cannot be materialized"))?;
    let definition = crate::compose_app::ComposeDefinition::parse(&text)
        .map_err(|error| invalid(error.to_string()))?;
    if definition
        .services
        .iter()
        .flat_map(|service| &service.mounts)
        .any(|mount| mount.kind == crate::compose_app::MountKind::Host)
    {
        return Err(invalid("Git Compose cannot bind arbitrary Host paths"));
    }
    Ok(text)
}
