//! Per-Application tools. Every home write and setup command runs through N1.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::custom_images::{Dependency, Recipe};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRecipe {
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    #[serde(default)]
    pub setup: Vec<String>,
}

impl NativeRecipe {
    fn shared(&self) -> Recipe {
        Recipe {
            name: String::new(),
            template_id: None,
            dependencies: self.dependencies.clone(),
            setup: self.setup.clone(),
            build_checks: Vec::new(),
            dockerfile: None,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        self.shared().validate_machine()
    }

    pub fn is_empty(&self) -> bool {
        self.dependencies.is_empty() && self.setup.is_empty()
    }

    pub(crate) fn fingerprint(&self) -> String {
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(self).expect("recipe JSON"))
        )
    }

    fn toml(&self) -> String {
        self.shared().mise_toml()
    }
}

pub(crate) fn reserved_variable(key: &str) -> bool {
    key.starts_with("MISE_")
        || [
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "XDG_STATE_HOME",
            "CARGO_HOME",
            "RUSTUP_HOME",
        ]
        .contains(&key)
}

/// Fixed paths take precedence over Application Variables. PATH keeps the
/// Operator's suffix, with this Application's tools first.
pub(crate) fn environment(home: &Path, values: &[(String, String)]) -> Vec<(String, String)> {
    let root = home.join(".self-host/mise");
    let path = values
        .iter()
        .find(|(key, _)| key == "PATH")
        .map(|(_, value)| value.as_str())
        .unwrap_or("/usr/local/bin:/usr/bin:/bin");
    let mut result: Vec<_> = values
        .iter()
        .filter(|(key, _)| key != "PATH" && !reserved_variable(key))
        .cloned()
        .collect();
    for (key, relative) in [
        ("MISE_CONFIG_DIR", "config"),
        ("MISE_GLOBAL_CONFIG_FILE", "config/config.toml"),
        ("MISE_DATA_DIR", "data"),
        ("MISE_CACHE_DIR", "cache"),
        ("MISE_STATE_DIR", "state"),
        ("MISE_TMP_DIR", "tmp"),
        ("MISE_SYSTEM_CONFIG_DIR", "system"),
        ("MISE_SYSTEM_DIR", "system"),
        ("MISE_INSTALL_PATH", "bin/mise"),
        ("MISE_TRUSTED_CONFIG_PATHS", "config"),
        ("XDG_CONFIG_HOME", "xdg/config"),
        ("XDG_DATA_HOME", "xdg/data"),
        ("XDG_CACHE_HOME", "xdg/cache"),
        ("XDG_STATE_HOME", "xdg/state"),
        ("CARGO_HOME", "cargo"),
        ("RUSTUP_HOME", "rustup"),
    ] {
        result.push((key.into(), root.join(relative).display().to_string()));
    }
    result.extend([
        ("MISE_CEILING_PATHS".into(), home.display().to_string()),
        ("MISE_AUTO_INSTALL".into(), "0".into()),
        ("MISE_AUTO_UPDATE".into(), "0".into()),
        ("MISE_YES".into(), "1".into()),
        ("MISE_JOBS".into(), "1".into()),
        (
            "PATH".into(),
            format!(
                "{}/bin:{}/data/shims:{}/cargo/bin:{path}",
                root.display(),
                root.display(),
                root.display()
            ),
        ),
    ]);
    result
}

pub(crate) fn command(home: &Path, recipe: &NativeRecipe, argv: Vec<String>) -> Vec<String> {
    if recipe.is_empty() {
        return argv;
    }
    let mut command = vec![
        home.join(".self-host/mise/bin/mise").display().to_string(),
        "exec".into(),
        "--".into(),
    ];
    command.extend(argv);
    command
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn apply(
    #[cfg(target_os = "linux")] root: &super::CgroupRoot,
    service: &super::supervision::ServiceDefinition,
    mut log: std::fs::File,
) -> anyhow::Result<()> {
    use anyhow::{Context, bail};
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};
    let account = super::AccountName::parse(&service.account)?;
    let home = super::resolve(&account)?.home;
    let mut command = vec![
        "/bin/sh".into(),
        "-ec".into(),
        include_str!("mise-setup.sh").into(),
        "native-recipe".into(),
        service.recipe.toml(),
        if service.recipe.is_empty() { "0" } else { "1" }.into(),
    ];
    command.extend(service.recipe.setup.clone());
    log.write_all(b"Preparing native environment.\n")?;
    let request = super::LaunchRequest {
        application_id: service.application_id.clone(),
        account,
        command,
        working_dir: home.clone(),
        environment: environment(&home, &service.environment),
        limits: service.limits.clone(),
        purpose: super::Purpose::Build,
    };
    #[cfg(target_os = "linux")]
    let launched = super::launch(root, &request);
    #[cfg(target_os = "macos")]
    let launched = super::launch(&request);
    let mut process =
        launched.context("native environment preparation was refused by the non-root launcher")?;
    let (sender, chunks) = std::sync::mpsc::sync_channel(16);
    let mut readers = Vec::new();
    let pipes: Vec<Box<dyn Read + Send>> = vec![
        Box::new(process.child.stdout.take().unwrap()),
        Box::new(process.child.stderr.take().unwrap()),
    ];
    for mut pipe in pipes {
        let sender = sender.clone();
        readers.push(std::thread::spawn(move || {
            let mut buffer = [0; 8192];
            loop {
                match pipe.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        if sender.send(buffer[..count].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        }));
    }
    drop(sender);
    let redactor = super::redaction::Redactor::new(&service.environment);
    let logger = std::thread::spawn(move || {
        redactor.forward(
            chunks,
            LimitedLog {
                log,
                remaining: 256 * 1024,
                truncated: false,
            },
        )
    });
    let deadline = Instant::now() + Duration::from_secs(900);
    let result = loop {
        match process.child.try_wait() {
            Ok(Some(status)) => break Ok(Some(status)),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            Ok(None) => break Ok(None),
            Err(error) => break Err(error),
        }
    };
    let cleanup = process.stop(Duration::from_secs(1));
    #[cfg(target_os = "linux")]
    for reader in readers {
        let _ = reader.join();
    }
    #[cfg(target_os = "macos")]
    super::macos::drain_readers(readers);
    // A foreground command's inherited pipes should close on exit. s6's
    // finish hook cleans any same-group stragglers on macOS.
    if cfg!(target_os = "linux") || logger.is_finished() {
        logger
            .join()
            .map_err(|_| anyhow::anyhow!("native environment log worker failed"))??;
    }
    cleanup.context("could not clean the native environment preparation process tree")?;
    let Some(status) = result.context("could not observe native environment preparation")? else {
        bail!("native environment preparation exceeded 15 minutes; inspect Application logs");
    };
    if !status.success() {
        bail!("native environment preparation failed ({status}); inspect Application logs");
    }
    Ok(())
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct LimitedLog<W> {
    log: W,
    remaining: usize,
    truncated: bool,
}

impl<W: std::io::Write> std::io::Write for LimitedLog<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let count = self.remaining.min(bytes.len());
        self.log.write_all(&bytes[..count])?;
        self.remaining -= count;
        if count < bytes.len() && !self.truncated {
            self.log
                .write_all(b"\nNative environment output truncated at 256 KiB.\n")?;
            self.truncated = true;
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.log.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_recipe_reuses_dependencies_but_rejects_image_only_fields() {
        let recipe: NativeRecipe = serde_json::from_str(
            r#"{"dependencies":[{"tool":"node","version":"24"}],"setup":["node --version"]}"#,
        )
        .unwrap();
        recipe.validate().unwrap();
        assert_eq!(recipe.toml(), "[tools]\n\"node\" = \"24\"\n");
        assert!(serde_json::from_str::<NativeRecipe>(r#"{"dockerfile":"FROM debian"}"#).is_err());
        assert!(
            NativeRecipe {
                setup: vec!["echo first\necho second".into()],
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(NativeRecipe::default().validate().is_ok());
    }
    #[test]
    fn recipes_and_only_recipes_determine_the_preparation_fingerprint() {
        let original = NativeRecipe::default();
        let changed = NativeRecipe {
            setup: vec!["touch configured".into()],
            ..Default::default()
        };
        assert_ne!(original.fingerprint(), changed.fingerprint());
        assert_eq!(
            original.fingerprint(),
            serde_json::from_str::<NativeRecipe>("{}")
                .unwrap()
                .fingerprint()
        );
    }
    #[test]
    fn each_home_owns_its_environment_and_automatic_installs_stay_disabled() {
        let home = Path::new("/fixture/one");
        let values: std::collections::BTreeMap<_, _> = environment(
            home,
            &[
                ("PATH".into(), "/custom/bin".into()),
                ("MISE_DATA_DIR".into(), "/root/shared".into()),
                ("TOKEN".into(), "private".into()),
            ],
        )
        .into_iter()
        .collect();
        assert_eq!(values["MISE_DATA_DIR"], "/fixture/one/.self-host/mise/data");
        assert_eq!(values["MISE_AUTO_INSTALL"], "0");
        assert_eq!(values["TOKEN"], "private");
        assert!(values["PATH"].ends_with(":/custom/bin"));
        assert_ne!(
            environment(home, &[]),
            environment(Path::new("/fixture/two"), &[])
        );
        let argv = vec![
            "node".into(),
            "-e".into(),
            "first\nsecond".into(),
            "".into(),
        ];
        assert_eq!(command(home, &NativeRecipe::default(), argv.clone()), argv);
        let recipe = NativeRecipe {
            setup: vec!["true".into()],
            ..Default::default()
        };
        assert_eq!(&command(home, &recipe, argv.clone())[3..], argv);
    }
    #[test]
    fn preparation_output_is_bounded_and_still_drains_the_process() {
        use std::io::Write;
        let mut bytes = Vec::new();
        let mut log = LimitedLog {
            log: &mut bytes,
            remaining: 4,
            truncated: false,
        };
        log.write_all(b"123456789").unwrap();
        log.write_all(&[b'x'; 4096]).unwrap();
        assert!(bytes.starts_with(b"1234\nNative environment output truncated"));
        assert!(bytes.len() < 80);
    }
}
