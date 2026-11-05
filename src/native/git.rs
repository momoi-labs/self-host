//! Git candidates cross the privilege boundary as checked files, without .git
//! or checkout credentials. Materialization and repository code run as the app.

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

// The transport ceiling bounds reads before authenticated payloads are parsed.
pub const TRANSPORT_MAX: u64 = 1024 * 1024 * 1024;

fn limits_path() -> PathBuf {
    crate::paths::platform_config_dir().join("native-build-limits.json")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BuildLimits {
    pub max_source_bytes: usize,
    pub max_files: usize,
    pub max_input_bytes: u64,
}

impl Default for BuildLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: 16 * 1024 * 1024,
            max_files: 10_000,
            max_input_bytes: 32 * 1024 * 1024,
        }
    }
}

impl BuildLimits {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let limits: Self = serde_json::from_slice(bytes)?;
        if limits.max_source_bytes == 0
            || limits.max_files == 0
            || limits.max_input_bytes == 0
            || limits.max_input_bytes > TRANSPORT_MAX
        {
            bail!(
                "native build limits must be positive and the payload cannot exceed the 1 GiB transport ceiling"
            );
        }
        Ok(limits)
    }
}

pub fn build_limits() -> Result<BuildLimits> {
    match std::fs::read(limits_path()) {
        Ok(bytes) => {
            BuildLimits::parse(&bytes).context("invalid native build limits configuration")
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BuildLimits::default()),
        Err(error) => Err(error).context("cannot read native build limits configuration"),
    }
}

/// Save atomically so concurrent build readers see a complete configuration.
pub fn save_build_limits(limits: &BuildLimits) -> Result<()> {
    write_build_limits(&limits_path(), limits)
}

fn write_build_limits(path: &Path, limits: &BuildLimits) -> Result<()> {
    use std::io::Write;
    let bytes = serde_json::to_vec_pretty(limits)?;
    BuildLimits::parse(&bytes)?;
    let parent = path.parent().context("missing build limits directory")?;
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".native-build-limits-{:032x}",
        rand::random::<u128>()
    ));
    let result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o644))?;
        }
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.context("could not save native build limits in the Platform data directory")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerDefinition {
    /// Wrangler configuration, relative to the selected working directory.
    pub config: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeFile {
    pub path: String,
    pub content: String,
    pub executable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeSource {
    pub revision: String,
    pub files: Vec<NativeFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildInput {
    #[serde(default)]
    pub limits: BuildLimits,
    pub source: NativeSource,
    pub release: String,
    pub definition: crate::store::NativeDefinition,
}

pub fn validate_source(
    source: &crate::source::GitSource,
) -> Result<(), crate::source::SourceError> {
    crate::source::validate(source)?;
    if source.compose_path.is_some()
        || source.dockerfile != "Dockerfile"
        || !source.build_args.is_empty()
        || !source.build_secrets.is_empty()
        || source.registry_credential_id.is_some()
        || source.strict_build
    {
        return Err(crate::source::SourceError::Invalid(
            "native Git sources accept repository credentials and a context, not container build settings".into(),
        ));
    }
    Ok(())
}

pub fn relative(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 4096
        || path.contains(['\0', '\\'])
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || Path::new(path)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        bail!("native source paths must be relative without empty, '.' or '..' components");
    }
    Ok(())
}

pub fn build_id(id: &str) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "build-{}",
        &format!("{:x}", Sha256::digest(id.as_bytes()))[..16]
    )
}

pub fn validate_release(release: &str) -> Result<()> {
    if release.len() != 32 || !release.bytes().all(|c| c.is_ascii_hexdigit()) {
        bail!("invalid native release identity");
    }
    Ok(())
}

pub fn validate_definition(definition: &crate::store::NativeDefinition) -> Result<()> {
    if definition.build.len() > 16
        || definition
            .build
            .iter()
            .any(|c| c.is_empty() || c.len() > 4096 || c.contains(['\0', '\n', '\r']))
    {
        bail!("use at most 16 build commands, each a single command of 1 to 4096 bytes");
    }
    if let Some(worker) = &definition.worker {
        relative(&worker.config)?;
        if !definition.command.is_empty() {
            bail!("Workers use a managed workerd command; leave the process command empty");
        }
        if definition.port.is_none() {
            bail!("Workers require a loopback Web Target port");
        }
        for tool in ["node", "npm:wrangler", "github:cloudflare/workerd"] {
            if !definition
                .recipe
                .dependencies
                .iter()
                .any(|d| d.tool == tool)
            {
                bail!("Workers require {tool} in Dependencies");
            }
        }
    }
    Ok(())
}

pub fn source_files(root: &Path) -> Result<Vec<NativeFile>> {
    fn walk(
        root: &Path,
        directory: &Path,
        files: &mut Vec<NativeFile>,
        bytes: &mut usize,
        limits: &BuildLimits,
    ) -> Result<()> {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.is_dir() {
                walk(root, &path, files, bytes, limits)?;
            } else if metadata.is_file() {
                let name = path
                    .strip_prefix(root)?
                    .to_str()
                    .context("source paths must be UTF-8")?
                    .to_owned();
                relative(&name)?;
                *bytes = bytes
                    .checked_add(usize::try_from(metadata.len())?)
                    .context("source too large")?;
                if *bytes > limits.max_source_bytes || files.len() >= limits.max_files {
                    bail!(
                        "native Git source exceeds configured limit ({} bytes or {} files); change Settings > Applications or select a smaller context",
                        limits.max_source_bytes,
                        limits.max_files
                    );
                }
                #[cfg(unix)]
                let executable = {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let executable = false;
                files.push(NativeFile {
                    path: name,
                    content: STANDARD.encode(std::fs::read(path)?),
                    executable,
                });
            } else {
                bail!("native source accepts only regular files and directories");
            }
        }
        Ok(())
    }
    if !root.is_dir() {
        bail!("native Git context must be a directory");
    }
    let mut files = Vec::new();
    walk(root, root, &mut files, &mut 0, &build_limits()?)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

impl BuildInput {
    pub fn validate(&self) -> Result<()> {
        let limits = &self.limits;
        BuildLimits::parse(&serde_json::to_vec(limits)?)?;
        validate_release(&self.release)?;
        validate_definition(&self.definition)?;
        self.definition
            .recipe
            .validate()
            .map_err(anyhow::Error::msg)?;
        if ![40, 64].contains(&self.source.revision.len())
            || !self.source.revision.bytes().all(|b| b.is_ascii_hexdigit())
        {
            bail!("native source requires a full Git commit");
        }
        if self.source.files.len() > limits.max_files {
            bail!("native source has too many files");
        }
        let mut names = BTreeSet::new();
        let mut size = 0usize;
        for file in &self.source.files {
            relative(&file.path)?;
            if !names.insert(&file.path) {
                bail!("duplicate source path");
            }
            // Check encoded length before decoding, including a malicious helper request.
            size = size
                .checked_add(file.content.len())
                .context("source too large")?;
            if size
                > limits
                    .max_source_bytes
                    .div_ceil(3)
                    .saturating_mul(4)
                    .saturating_add(limits.max_files.saturating_mul(4))
            {
                bail!("native source exceeds configured byte limit");
            }
        }
        let mut decoded = 0usize;
        for file in &self.source.files {
            decoded += STANDARD.decode(&file.content)?.len();
            if decoded > limits.max_source_bytes {
                bail!("native source exceeds configured byte limit");
            }
        }
        if let Some(directory) = &self.definition.working_dir {
            relative(directory)?;
        }
        Ok(())
    }
}

pub fn working_dir(home: &Path, record: &crate::store::ApplicationRecord) -> Result<PathBuf> {
    let crate::store::Runtime::Native(definition) = &record.runtime else {
        bail!("expected native Runtime");
    };
    let base = if record.git.is_some() {
        let release = record
            .git_build
            .as_ref()
            .and_then(|b| b.native_release.as_deref())
            .context("native Git Application has no successful release")?;
        validate_release(release)?;
        home.join(".self-host/releases").join(release)
    } else {
        home.to_owned()
    };
    Ok(definition
        .working_dir
        .as_deref()
        .map(|d| base.join(d))
        .unwrap_or(base))
}

/// Internal CLI entry point. Refuse execution before N1 drops privileges.
#[cfg(unix)]
pub fn build() -> Result<()> {
    use std::io::Read;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if unsafe { libc::geteuid() } == 0 {
        bail!("native source builds must run as the Application Account, never root");
    }
    let max_input = TRANSPORT_MAX;
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(max_input + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_input {
        bail!("native build input exceeds configured payload limit");
    }
    let input: BuildInput = serde_json::from_slice(&bytes)?;
    input.validate()?;
    if bytes.len() as u64 > input.limits.max_input_bytes {
        bail!("native build input exceeds configured payload limit");
    }
    let home = PathBuf::from(std::env::var_os("HOME").context("missing Application home")?);
    let releases = home.join(".self-host/releases");
    std::fs::create_dir_all(&releases)?;
    let release = releases.join(&input.release);
    std::fs::create_dir(&release).context("candidate release already exists")?;
    std::fs::set_permissions(&release, std::fs::Permissions::from_mode(0o700))?;
    let result = (|| -> Result<()> {
        for file in &input.source.files {
            let path = release.join(&file.path);
            std::fs::create_dir_all(path.parent().unwrap())?;
            use std::io::Write;
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .custom_flags(libc::O_NOFOLLOW)
                .mode(if file.executable { 0o700 } else { 0o600 })
                .open(path)?;
            output.write_all(&STANDARD.decode(&file.content)?)?;
        }
        let directory = input
            .definition
            .working_dir
            .as_deref()
            .map(|d| release.join(d))
            .unwrap_or(release.clone());
        let canonical = directory
            .canonicalize()
            .context("native working directory does not exist in the checkout")?;
        if !canonical.starts_with(release.canonicalize()?) {
            bail!("build working directory escapes release");
        }
        for command in &input.definition.build {
            println!("Running native Git build command.");
            run(
                &home,
                &input.definition.recipe,
                &directory,
                &["/bin/sh", "-ec", command],
            )?;
        }
        if let Some(worker) = &input.definition.worker {
            build_worker(&home, &input.definition.recipe, &directory, worker)?;
        }
        println!("Native Git candidate built from {}.", input.source.revision);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&release);
    }
    result
}

fn run(
    home: &Path,
    recipe: &super::mise::NativeRecipe,
    directory: &Path,
    argv: &[&str],
) -> Result<()> {
    let argv = super::mise::command_at(
        home,
        recipe,
        directory,
        argv.iter().map(|v| (*v).into()).collect(),
    );
    let status = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(directory)
        .stdin(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        bail!("native Git build command failed; inspect Application logs");
    }
    Ok(())
}

fn build_worker(
    home: &Path,
    recipe: &super::mise::NativeRecipe,
    directory: &Path,
    worker: &WorkerDefinition,
) -> Result<()> {
    // Resolve Wrangler outside the repository's module-resolution paths.
    let binary = std::process::Command::new(home.join(".self-host/mise/bin/mise"))
        .args(["--cd"])
        .arg(home)
        .args(["which", "wrangler"])
        .output()?;
    if !binary.status.success() {
        bail!("Wrangler is not installed in Dependencies");
    }
    let binary = String::from_utf8(binary.stdout)?.trim().to_owned();
    let config =
        crate::source::contained_path(directory, &worker.config).map_err(anyhow::Error::new)?;
    let output = directory.join(".self-host-worker");
    std::fs::create_dir(&output)
        .context(".self-host-worker is reserved for generated Worker artifacts")?;
    let argv = [
        "node",
        "-e",
        include_str!("worker-build.cjs"),
        "--",
        &binary,
        config.to_str().context("non-UTF8 config path")?,
        output.to_str().context("non-UTF8 output path")?,
    ];
    run(home, recipe, directory, &argv)
}

/// Generate bindings as the app, then replace this process with workerd. Node
/// and Wrangler do not remain in the Runtime. Values never enter argv.
#[cfg(unix)]
pub fn run_worker(port: u16, variables: &[String]) -> Result<()> {
    use std::io::{Read, Write};
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::os::unix::process::CommandExt;
    if unsafe { libc::geteuid() } == 0 {
        bail!("workerd must run as the Application Account");
    }
    let root = Path::new(".self-host-worker");
    let mut bytes = Vec::new();
    std::fs::File::open(root.join("worker.json"))?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        bail!("Worker metadata exceeds 1 MiB");
    }
    #[derive(Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum Metadata {
        Module {
            compatibility_date: String,
            compatibility_flags: Vec<String>,
            vars: std::collections::BTreeMap<String, String>,
        },
        Site {
            directory: String,
            not_found_handling: String,
            compatibility_date: String,
            compatibility_flags: Vec<String>,
        },
    }
    let quote = |s: &str| serde_json::to_string(s).expect("string JSON");
    let config = match serde_json::from_slice(&bytes)? {
        Metadata::Module {
            compatibility_date,
            compatibility_flags,
            mut vars,
        } => {
            for key in variables {
                if let Ok(value) = std::env::var(key) {
                    vars.insert(key.clone(), value);
                }
            }
            let bindings = vars
                .iter()
                .map(|(k, v)| format!("(name = {}, text = {})", quote(k), quote(v)))
                .collect::<Vec<_>>()
                .join(",");
            let flags = compatibility_flags
                .iter()
                .map(|s| quote(s))
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "using Workerd = import \"/workerd/workerd.capnp\";\nconst app :Workerd.Config = (services = [(name = \"main\", worker = (modules = [(name = \"worker.mjs\", esModule = embed \"bundle.mjs\")], compatibilityDate = {}, compatibilityFlags = [{}], bindings = [{}]))], sockets = [(name = \"http\", address = \"127.0.0.1:{port}\", http = (), service = \"main\")]);\n",
                quote(&compatibility_date),
                flags,
                bindings
            )
        }
        Metadata::Site {
            directory,
            not_found_handling,
            compatibility_date,
            compatibility_flags,
        } => {
            if !Path::new(&directory).is_absolute() {
                bail!("static site directory must be absolute");
            }
            if !["spa", "none"].contains(&not_found_handling.as_str()) {
                bail!("static site fallback must be spa or none");
            }
            let wrapper = root.join("static-worker.mjs");
            std::fs::write(&wrapper, include_str!("static-worker.mjs"))?;
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o600))?;
            let flags = compatibility_flags
                .iter()
                .map(|s| quote(s))
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "using Workerd = import \"/workerd/workerd.capnp\";\nconst app :Workerd.Config = (services = [(name = \"assets\", disk = (path = {}, writable = false, allowDotfiles = false)), (name = \"main\", worker = (modules = [(name = \"worker.mjs\", esModule = embed \"static-worker.mjs\")], compatibilityDate = {}, compatibilityFlags = [{}], bindings = [(name = \"ASSETS\", service = \"assets\"), (name = \"NOT_FOUND_HANDLING\", text = {})]))], sockets = [(name = \"http\", address = \"127.0.0.1:{port}\", http = (), service = \"main\")]);\n",
                quote(&directory),
                quote(&compatibility_date),
                flags,
                quote(&not_found_handling)
            )
        }
    };
    let path = root.join("config.capnp");
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(&path)?;
    output.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    output.write_all(config.as_bytes())?;
    drop(output);
    Err(std::process::Command::new("workerd")
        .arg("serve")
        .arg(path)
        .exec()
        .into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn build_limits_persist_atomically_and_invalid_values_preserve_saved_file() {
        let directory =
            std::env::temp_dir().join(format!("native-limits-{:032x}", rand::random::<u128>()));
        let path = directory.join("limits.json");
        let mut limits = BuildLimits::default();
        limits.max_source_bytes = 64 * 1024 * 1024;
        write_build_limits(&path, &limits).unwrap();
        assert_eq!(
            BuildLimits::parse(&std::fs::read(&path).unwrap()).unwrap(),
            limits
        );
        limits.max_files = 0;
        assert!(write_build_limits(&path, &limits).is_err());
        assert_eq!(
            BuildLimits::parse(&std::fs::read(&path).unwrap())
                .unwrap()
                .max_files,
            10_000
        );
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn payload_carries_limits_across_process_boundaries() {
        let custom = BuildLimits {
            max_source_bytes: 64 * 1024 * 1024,
            max_files: 20000,
            max_input_bytes: 128 * 1024 * 1024,
        };
        let wire = serde_json::json!({"limits":custom,"source":{"revision":"a".repeat(40),"files":[]},"release":"b".repeat(32),"definition":{"account":"","command":["true"]}});
        let input: BuildInput = serde_json::from_value(wire).unwrap();
        input.validate().unwrap();
        assert_eq!(input.limits, custom);
        let excessive = BuildLimits {
            max_input_bytes: TRANSPORT_MAX + 1,
            ..custom
        };
        assert!(BuildLimits::parse(&serde_json::to_vec(&excessive).unwrap()).is_err());
    }

    #[test]
    fn build_limits_allow_overrides_and_reject_invalid_configuration() {
        let defaults = BuildLimits::parse(b"{}").unwrap();
        assert_eq!(defaults.max_source_bytes, 16 * 1024 * 1024);
        let custom = BuildLimits::parse(
            br#"{"max_source_bytes":67108864,"max_files":20000,"max_input_bytes":134217728}"#,
        )
        .unwrap();
        assert_eq!(custom.max_source_bytes, 64 * 1024 * 1024);
        assert_eq!(custom.max_files, 20_000);
        assert_eq!(custom.max_input_bytes, 128 * 1024 * 1024);
        for invalid in [
            br#"{"max_files":0}"#.as_slice(),
            br#"{"max_source_bytes":-1}"#,
            br#"{"typo":1}"#,
            b"invalid",
        ] {
            assert!(BuildLimits::parse(invalid).is_err());
        }
    }

    #[test]
    fn rejects_traversal_and_untrusted_release_names() {
        for path in ["", "/tmp/x", "../x", "a/../b", "a//b", "a/./b", "a\\b"] {
            assert!(relative(path).is_err(), "{path}");
        }
        assert!(relative("src/index.ts").is_ok());
        assert!(validate_release(&"a".repeat(32)).is_ok());
        assert!(validate_release("../../current").is_err());
    }
}
