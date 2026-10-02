use super::{SourceError, dockerfile_instructions, invalid};
use std::collections::BTreeMap;

fn check_mount(flag: &str, guard: &str) -> Result<(), SourceError> {
    if flag.contains('$') {
        return Err(invalid("RUN mount inputs must be literal"));
    }
    for field in flag.trim_start_matches("--mount=").split(',') {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        if matches!(
            key.trim().to_ascii_lowercase().as_str(),
            "target" | "dst" | "destination"
        ) {
            let path = std::path::Path::new(value);
            if !path.is_absolute()
                || path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                return Err(invalid(
                    "RUN mount targets must be absolute and cannot contain '..'",
                ));
            }
            let target: std::path::PathBuf = path.components().collect();
            if std::path::Path::new(guard).starts_with(target) {
                return Err(invalid("RUN mounts cannot cover the trusted build guard"));
            }
        }
    }
    Ok(())
}

fn argv(text: &str) -> Result<Vec<String>, SourceError> {
    let values: Vec<String> = serde_json::from_str(text)
        .map_err(|_| invalid("SHELL and exec RUN need a JSON string array"))?;
    if values.is_empty() || values[0].is_empty() || values.iter().any(|value| value.contains('\0'))
    {
        return Err(invalid("build command argv is empty or invalid"));
    }
    Ok(values)
}

pub fn dockerfile(
    text: &str,
    helper_name: &str,
    base_shells: &BTreeMap<String, Vec<String>>,
) -> Result<String, SourceError> {
    if !super::valid_id(helper_name) {
        return Err(invalid("build guard filename is invalid"));
    }
    let destination = format!("/.__{helper_name}");
    let mut shell = vec!["/bin/sh".into(), "-c".into()];
    let mut aliases: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut alias = None;
    let mut output = Vec::new();
    for line in dockerfile_instructions(text)? {
        let (instruction, body) = line.split_once(char::is_whitespace).unwrap_or((&line, ""));
        match instruction.to_ascii_uppercase().as_str() {
            "FROM" => {
                if let Some(alias) = alias.take() {
                    aliases.insert(alias, shell.clone());
                }
                let words: Vec<_> = body.split_whitespace().collect();
                let image = words.first().ok_or_else(|| invalid("FROM has no image"))?;
                shell = aliases
                    .get(&image.to_ascii_lowercase())
                    .or_else(|| base_shells.get(*image))
                    .cloned()
                    .unwrap_or_else(|| vec!["/bin/sh".into(), "-c".into()]);
                alias = if words.len() == 3 {
                    Some(words[2].to_ascii_lowercase())
                } else {
                    None
                };
                if alias.as_deref() == Some(helper_name) {
                    return Err(invalid(
                        "build guard context collides with a Dockerfile stage",
                    ));
                }
                output.push(line);
            }
            "SHELL" => {
                shell = argv(body.trim())?;
                output.push(line);
            }
            "RUN" => {
                let mut body = body.trim_start();
                let mut flags = Vec::new();
                while body.starts_with("--") {
                    let (flag, rest) = body
                        .split_once(char::is_whitespace)
                        .ok_or_else(|| invalid("RUN has flags but no command"))?;
                    if !flag.starts_with("--mount=")
                        || flag.len() <= 8
                        || flag.contains(['\"', '\'', '\\'])
                    {
                        return Err(invalid("RUN supports --mount flags only"));
                    }
                    check_mount(flag, &destination)?;
                    flags.push(flag.to_owned());
                    body = rest.trim_start();
                }
                if body.is_empty() {
                    return Err(invalid("RUN needs a command"));
                }
                let command = if body.starts_with('[') {
                    argv(body)?
                } else {
                    let mut command = shell.clone();
                    command.push(body.to_string());
                    command
                };
                flags.push(format!("--mount=type=bind,from={helper_name},source=/{helper_name},target={destination},readonly"));
                let mut guarded = vec![destination.clone()];
                guarded.extend(command);
                let guarded = serde_json::to_string(&guarded)
                    .map_err(|_| invalid("build command cannot be rendered"))?;
                let flags = if flags.is_empty() {
                    String::new()
                } else {
                    format!("{} ", flags.join(" "))
                };
                output.push(format!("RUN {flags}{guarded}"));
            }
            _ => output.push(line),
        }
    }
    Ok(output.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guards_shell_exec_mounts_and_stage_shells() {
        let source = "FROM alpine AS first\nUSER 1001\nRUN echo first\nSHELL [\"/bin/ash\",\"-ec\"]\nRUN --mount=type=secret,id=fixture cat /run/secrets/fixture\nRUN [\"/bin/true\"]\nFROM first AS second\nUSER 1001\nRUN echo inherited\nFROM alpine\nUSER 1001\nRUN echo default\n";
        let guarded = dockerfile(source, "sf_guard_fixture", &BTreeMap::new()).unwrap();
        assert!(guarded.contains("[\"/.__sf_guard_fixture\",\"/bin/sh\",\"-c\",\"echo first\"]"));
        assert!(guarded.contains("RUN --mount=type=secret,id=fixture --mount=type=bind,from=sf_guard_fixture,source=/sf_guard_fixture,target=/.__sf_guard_fixture,readonly [\"/.__sf_guard_fixture\",\"/bin/ash\",\"-ec\",\"cat /run/secrets/fixture\"]"));
        assert!(guarded.contains("[\"/.__sf_guard_fixture\",\"/bin/true\"]"));
        assert!(
            guarded.contains("[\"/.__sf_guard_fixture\",\"/bin/ash\",\"-ec\",\"echo inherited\"]")
        );
        assert!(guarded.contains("[\"/.__sf_guard_fixture\",\"/bin/sh\",\"-c\",\"echo default\"]"));
        assert_eq!(guarded.matches("--mount=type=bind,from=sf_guard_fixture,source=/sf_guard_fixture,target=/.__sf_guard_fixture,readonly").count(), 5);
        assert!(!guarded.contains("COPY"));
        for command in [
            "RUN --security=insecure true",
            "RUN --network=host true",
            "RUN []",
            "RUN --mount=type=secret",
            "RUN --mount=type=bind,target=/ true",
            "RUN --mount=type=bind,target=/./ true",
            "RUN --mount=type=bind,dst=/.__sf_guard_fixture true",
            "RUN --mount=type=bind,TARGET=/ true",
            "RUN --mount=type=bind,Dst=/.__sf_guard_fixture true",
            "RUN --mount=type=bind,Destination=/./ true",
            "RUN --mount=type=cache,target=$TARGET true",
            "RUN --mount=type=cache,target=. true",
            "SHELL []",
            "FROM alpine AS SF_GUARD_FIXTURE\nUSER 1001\nRUN true",
        ] {
            assert!(dockerfile(command, "sf_guard_fixture", &BTreeMap::new()).is_err());
        }
        let mut shells = BTreeMap::new();
        shells.insert("base".into(), vec!["/bin/bash".into(), "-c".into()]);
        assert!(
            dockerfile("FROM base\nRUN true", "sf_guard_fixture", &shells)
                .unwrap()
                .contains("\"/bin/bash\"")
        );
    }
}
