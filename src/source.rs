//! Git inputs are copied into a private checkout before Docker sees them.

pub mod connections;
pub mod guard;
pub mod inspection;
mod materialize;
#[cfg(test)]
mod tests;

use crate::docker::{DockerError, DockerRuntime, SourceBuild};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

fn default_ref() -> String {
    "HEAD".into()
}
fn default_context() -> String {
    ".".into()
}
fn default_dockerfile() -> String {
    "Dockerfile".into()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitSource {
    pub repository: String,
    #[serde(default = "default_ref")]
    pub git_ref: String,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default = "default_context")]
    pub context: String,
    #[serde(default = "default_dockerfile")]
    pub dockerfile: String,
    #[serde(default)]
    pub compose_path: Option<String>,
    #[serde(default)]
    pub build_args: BTreeMap<String, String>,
    /// BuildKit mount names mapped to private credential ids.
    #[serde(default)]
    pub build_secrets: BTreeMap<String, String>,
    #[serde(default)]
    pub credential_id: Option<String>,
    #[serde(default)]
    pub registry_credential_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBuild {
    pub revision: String,
    pub images: BTreeMap<String, String>,
    pub status: String,
}

#[derive(Clone, Debug)]
pub struct BuiltGitRelease {
    pub build: GitBuild,
    pub image: String,
    pub compose: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CredentialKind {
    Git,
    Registry,
    BuildSecret,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialRequest {
    pub kind: CredentialKind,
    #[serde(default)]
    pub username: Option<String>,
    pub value: String,
    #[serde(default)]
    pub server: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CredentialMetadata {
    pub id: String,
    pub kind: CredentialKind,
}

#[derive(Serialize, Deserialize)]
struct Credential {
    metadata: CredentialMetadata,
    username: Option<String>,
    value: String,
    server: Option<String>,
}

#[derive(Debug)]
pub enum SourceError {
    Invalid(String),
    Io(std::io::Error),
    Checkout,
    Build(DockerError),
}
impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "invalid Git source: {message}"),
            Self::Io(_) => f.write_str("could not access private source storage"),
            Self::Checkout => {
                f.write_str("Git checkout failed; check the repository, ref and credential")
            }
            Self::Build(_) => f.write_str("Git image build failed"),
        }
    }
}
impl std::error::Error for SourceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // Host paths and repository output can contain credentials.
        match self {
            Self::Build(error) => Some(error),
            _ => None,
        }
    }
}
impl From<std::io::Error> for SourceError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<DockerError> for SourceError {
    fn from(error: DockerError) -> Self {
        Self::Build(error)
    }
}
fn invalid(message: impl Into<String>) -> SourceError {
    SourceError::Invalid(message.into())
}

pub fn private_root() -> PathBuf {
    crate::file_store::state_dir().join("source")
}

fn private_directory(path: &Path) -> Result<(), SourceError> {
    std::fs::create_dir_all(path)?;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid("private storage must be a directory"));
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn private_file(path: &Path, contents: &[u8]) -> Result<(), SourceError> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}
fn valid_input_name(id: &str) -> bool {
    valid_id(id) && !id.starts_with('-')
}

pub fn save_credential(
    root: &Path,
    request: CredentialRequest,
) -> Result<CredentialMetadata, SourceError> {
    if request.value.is_empty() || request.value.len() > 65536 || request.value.contains('\0') {
        return Err(invalid("credential value is empty or invalid"));
    }
    match request.kind {
        CredentialKind::Git => {
            if request
                .username
                .as_deref()
                .is_none_or(|name| name.is_empty() || name.contains(['\n', '\r', '\0']))
                || request.server.is_some()
                || request.value.contains(['\n', '\r'])
            {
                return Err(invalid(
                    "Git credentials need a username and a single-line token",
                ));
            }
        }
        CredentialKind::Registry => {
            if request
                .username
                .as_deref()
                .is_none_or(|name| name.is_empty() || name.contains([':', '\n', '\r', '\0']))
                || request.server.as_deref().is_none_or(|server| {
                    server.is_empty() || server.contains(['/', '\n', '\r', '\0'])
                })
            {
                return Err(invalid(
                    "registry credentials need a username and registry host",
                ));
            }
        }
        CredentialKind::BuildSecret => {
            if request.username.is_some() || request.server.is_some() {
                return Err(invalid("build secrets accept only their value"));
            }
        }
    }
    private_directory(root)?;
    let directory = root.join("credentials");
    private_directory(&directory)?;
    let metadata = CredentialMetadata {
        id: format!("cred-{:032x}", rand::random::<u128>()),
        kind: request.kind,
    };
    let credential = Credential {
        metadata: metadata.clone(),
        username: request.username,
        value: request.value,
        server: request.server,
    };
    private_file(
        &directory.join(&metadata.id),
        &serde_json::to_vec(&credential).map_err(|_| invalid("credential cannot be recorded"))?,
    )?;
    Ok(metadata)
}

fn load_credential(root: &Path, id: &str) -> Result<Credential, SourceError> {
    if !valid_id(id) {
        return Err(invalid("credential id is invalid"));
    }
    let path = root.join("credentials").join(id);
    let metadata = std::fs::symlink_metadata(&path)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(invalid("credential storage permissions are unsafe"));
    }
    serde_json::from_slice(&std::fs::read(path)?).map_err(|_| invalid("credential cannot be read"))
}

pub fn list_credentials(root: &Path) -> Result<Vec<CredentialMetadata>, SourceError> {
    let directory = root.join("credentials");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        result.push(load_credential(root, &id)?.metadata);
    }
    result.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(result)
}

pub fn validate(source: &GitSource) -> Result<(), SourceError> {
    let url = reqwest::Url::parse(&source.repository)
        .map_err(|_| invalid("repository must be an HTTP or HTTPS URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid(
            "repository must be HTTP(S), without embedded credentials, query or fragment",
        ));
    }
    if source.credential_id.is_some() && url.scheme() != "https" {
        return Err(invalid("private Git credentials require HTTPS"));
    }
    if source.git_ref.is_empty()
        || source.git_ref.starts_with('-')
        || source
            .git_ref
            .bytes()
            .any(|c| c.is_ascii_control() || c.is_ascii_whitespace() || b"~^:?*[\\".contains(&c))
        || source.git_ref.contains("..")
        || source.git_ref.contains("@{")
    {
        return Err(invalid("Git ref is invalid"));
    }
    if source
        .revision
        .as_deref()
        .is_some_and(|revision| !valid_revision(revision))
    {
        return Err(invalid("revision must be a full Git commit id"));
    }
    for path in [&source.context, &source.dockerfile]
        .into_iter()
        .chain(source.compose_path.iter())
    {
        relative_path(path)?;
    }
    for name in source.build_args.keys().chain(source.build_secrets.keys()) {
        if !valid_input_name(name) {
            return Err(invalid("build input name is invalid"));
        }
    }
    if source.build_args.contains_key("BUILDKIT_SYNTAX") {
        return Err(invalid(
            "BUILDKIT_SYNTAX cannot replace the checked Dockerfile frontend",
        ));
    }
    if source.build_args.len() > 128
        || source.build_secrets.len() > 32
        || source
            .build_args
            .values()
            .any(|value| value.len() > 16384 || value.contains('\0'))
    {
        return Err(invalid("build inputs exceed the supported size"));
    }
    for id in source
        .build_secrets
        .values()
        .chain(source.credential_id.iter())
        .chain(source.registry_credential_id.iter())
    {
        if !valid_id(id) {
            return Err(invalid("credential id is invalid"));
        }
    }
    Ok(())
}
fn valid_revision(revision: &str) -> bool {
    matches!(revision.len(), 40 | 64) && revision.bytes().all(|c| c.is_ascii_hexdigit())
}
fn relative_path(path: &str) -> Result<(), SourceError> {
    if path.is_empty()
        || path.contains(['\0', '\\'])
        || Path::new(path).components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(invalid("paths must stay inside the checkout"));
    }
    Ok(())
}

/// Reject symlinks even when they currently point inside: Docker must only see
/// regular checkout files, never a path that can redirect a Host read.
pub fn contained_path(root: &Path, path: &str) -> Result<PathBuf, SourceError> {
    relative_path(path)?;
    let root = root.canonicalize()?;
    let mut current = root.clone();
    for component in Path::new(path).components() {
        if let Component::Normal(name) = component {
            current.push(name);
            if std::fs::symlink_metadata(&current)?
                .file_type()
                .is_symlink()
            {
                return Err(invalid("checkout symlinks are not supported"));
            }
        }
    }
    let current = current.canonicalize()?;
    if !current.starts_with(root) {
        return Err(invalid("path escapes the checkout"));
    }
    Ok(current)
}

fn check_tree(root: &Path) -> Result<(), SourceError> {
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() || (!metadata.is_file() && !metadata.is_dir()) {
            return Err(invalid(
                "checkout must contain only regular files and directories",
            ));
        }
        if metadata.is_dir() {
            check_tree(&entry.path())?;
        }
    }
    Ok(())
}

struct Workspace(PathBuf);
impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git_command(workspace: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new("git");
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", workspace)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "protocol.file.allow=never",
            "-c",
            "protocol.ext.allow=never",
            "-c",
            "credential.helper=",
            "-c",
            "http.followRedirects=false",
            "-c",
            "protocol.allow=never",
            "-c",
            "protocol.http.allow=always",
            "-c",
            "protocol.https.allow=always",
        ])
        .kill_on_drop(true);
    command
}

async fn checkout(
    workspace: &Path,
    source: &GitSource,
    revision: Option<&str>,
    root: &Path,
) -> Result<(PathBuf, String), SourceError> {
    let directory = workspace.join("checkout");
    let mut authentication = Vec::new();
    if let Some(id) = &source.credential_id {
        let credential = connections::auth::resolve_credential(root, id).await?;
        if !matches!(credential.metadata.kind, CredentialKind::Git) {
            return Err(invalid("checkout credential has the wrong kind"));
        }
        connections::validate_bound_repository(credential.server.as_deref(), &source.repository)?;
        private_file(
            &workspace.join("username"),
            credential.username.unwrap_or_default().as_bytes(),
        )?;
        private_file(&workspace.join("password"), credential.value.as_bytes())?;
        let helper = workspace.join("askpass");
        private_file(&helper, b"#!/bin/sh\ncase \"$1\" in *Username*) cat \"$SOURCE_AUTH_DIR/username\";; *) cat \"$SOURCE_AUTH_DIR/password\";; esac\n")?;
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700))?;
        authentication.push(("GIT_ASKPASS", helper));
        authentication.push(("SOURCE_AUTH_DIR", workspace.to_path_buf()));
    }
    async fn run(
        workspace: &Path,
        auth: &[(&str, PathBuf)],
        args: &[&str],
    ) -> Result<Vec<u8>, SourceError> {
        use std::process::Stdio;
        use tokio::io::AsyncReadExt;
        let mut command = git_command(workspace);
        for (name, path) in auth {
            command.env(name, path);
        }
        let mut child = command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| SourceError::Checkout)?;
        tokio::time::timeout(std::time::Duration::from_secs(180), async {
            let mut output = Vec::new();
            child
                .stdout
                .take()
                .ok_or(SourceError::Checkout)?
                .take(65537)
                .read_to_end(&mut output)
                .await
                .map_err(|_| SourceError::Checkout)?;
            if output.len() > 65536
                || !child
                    .wait()
                    .await
                    .map_err(|_| SourceError::Checkout)?
                    .success()
            {
                return Err(SourceError::Checkout);
            }
            Ok(output)
        })
        .await
        .map_err(|_| SourceError::Checkout)?
    }
    let path = directory
        .to_str()
        .ok_or_else(|| invalid("checkout path is invalid"))?;
    run(
        workspace,
        &authentication,
        &["init", "--template=/dev/null", path],
    )
    .await?;
    run(
        workspace,
        &authentication,
        &["-C", path, "remote", "add", "origin", &source.repository],
    )
    .await?;
    let selected = source
        .revision
        .as_deref()
        .or(revision)
        .unwrap_or(&source.git_ref);
    let fetch = run(
        workspace,
        &authentication,
        &[
            "-C",
            path,
            "fetch",
            "--no-tags",
            "--depth=1",
            "origin",
            selected,
        ],
    )
    .await;
    if fetch.is_err() {
        if !valid_revision(selected) {
            return Err(SourceError::Checkout);
        }
        run(
            workspace,
            &authentication,
            &["-C", path, "fetch", "--no-tags", "origin", &source.git_ref],
        )
        .await?;
    }
    let selector = if valid_revision(selected) {
        format!("{selected}^{{commit}}")
    } else {
        "FETCH_HEAD^{commit}".into()
    };
    let pinned = String::from_utf8(
        run(
            workspace,
            &authentication,
            &["-C", path, "rev-parse", &selector],
        )
        .await?,
    )
    .map_err(|_| SourceError::Checkout)?
    .trim()
    .to_owned();
    if !valid_revision(&pinned) {
        return Err(SourceError::Checkout);
    }
    run(
        workspace,
        &authentication,
        &["-C", path, "checkout", "--detach", "--force", &pinned],
    )
    .await?;
    std::fs::remove_dir_all(directory.join(".git"))?;
    check_tree(&directory)?;
    Ok((directory, pinned))
}

pub async fn build(
    root: &Path,
    app_id: &str,
    source: &GitSource,
    pinned_revision: Option<&str>,
    docker: &(impl DockerRuntime + ?Sized),
) -> Result<BuiltGitRelease, SourceError> {
    validate(source)?;
    if !valid_id(app_id) || pinned_revision.is_some_and(|revision| !valid_revision(revision)) {
        return Err(invalid(
            "Application identity or pinned revision is invalid",
        ));
    }
    private_directory(root)?;
    let workspace = Workspace(root.join(format!("work-{:032x}", rand::random::<u128>())));
    private_directory(&workspace.0)?;
    let (checkout, revision) = checkout(&workspace.0, source, pinned_revision, root).await?;
    let mut secret_files = BTreeMap::new();
    for (name, id) in &source.build_secrets {
        let credential = load_credential(root, id)?;
        if !matches!(credential.metadata.kind, CredentialKind::BuildSecret) {
            return Err(invalid("build secret credential has the wrong kind"));
        }
        let path = workspace.0.join(format!("secret-{name}"));
        private_file(&path, credential.value.as_bytes())?;
        secret_files.insert(name.clone(), path);
    }
    let registry_directory = workspace.0.join("registry");
    private_directory(&registry_directory)?;
    let registry_config = if let Some(id) = &source.registry_credential_id {
        let credential = load_credential(root, id)?;
        if !matches!(credential.metadata.kind, CredentialKind::Registry) {
            return Err(invalid("registry credential has the wrong kind"));
        }
        let auth = base64(
            format!(
                "{}:{}",
                credential.username.unwrap_or_default(),
                credential.value
            )
            .as_bytes(),
        );
        let config =
            serde_json::json!({"auths": {credential.server.unwrap_or_default(): {"auth": auth}}});
        private_file(
            &registry_directory.join("config.json"),
            &serde_json::to_vec(&config)
                .map_err(|_| invalid("registry credential cannot be prepared"))?,
        )?;
        Some(registry_directory)
    } else {
        private_file(&registry_directory.join("config.json"), b"{\"auths\":{}}")?;
        Some(registry_directory)
    };
    let mut images = BTreeMap::new();
    let compose = if let Some(compose_path) = &source.compose_path {
        Some(
            materialize::compose(
                &checkout,
                compose_path,
                source,
                app_id,
                &revision,
                &secret_files,
                registry_config.as_deref(),
                docker,
                &mut images,
            )
            .await?,
        )
    } else {
        let context = contained_path(&checkout, &source.context)?;
        let dockerfile = contained_path(&checkout, &source.dockerfile)?;
        if !context.is_dir()
            || !dockerfile.is_file()
            || std::fs::metadata(&dockerfile)?.len() > 1048576
        {
            return Err(invalid(
                "build context must be a directory and Dockerfile a file below 1 MiB",
            ));
        }
        let text = std::fs::read_to_string(&dockerfile)?;
        let base_images = dockerfile_policy(&text)?;
        let request = SourceBuild {
            context,
            dockerfile,
            tag: format!(
                "sf-source-{app_id}:{}-{:016x}",
                &revision[..12],
                rand::random::<u64>()
            ),
            build_args: source.build_args.clone(),
            secret_files,
            registry_config,
            base_images,
        };
        images.insert("app".into(), docker.build_source(&request).await?);
        None
    };
    let image = images.values().next().cloned().unwrap_or_default();
    Ok(BuiltGitRelease {
        build: GitBuild {
            revision,
            images,
            status: "completed".into(),
        },
        image,
        compose,
    })
}

fn base64(value: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    for chunk in value.chunks(3) {
        let bits = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or_default() as u32) << 8)
            | chunk.get(2).copied().unwrap_or_default() as u32;
        result.push(ALPHABET[((bits >> 18) & 63) as usize] as char);
        result.push(ALPHABET[((bits >> 12) & 63) as usize] as char);
        result.push(if chunk.len() > 1 {
            ALPHABET[((bits >> 6) & 63) as usize] as char
        } else {
            '='
        });
        result.push(if chunk.len() > 2 {
            ALPHABET[(bits & 63) as usize] as char
        } else {
            '='
        });
    }
    result
}

pub fn nonroot_user(value: &str) -> bool {
    let mut ids = value.split(':');
    let Some(uid) = ids.next() else {
        return false;
    };
    if uid.is_empty()
        || !uid.bytes().all(|c| c.is_ascii_digit())
        || uid.parse::<u32>().is_ok_and(|uid| uid == 0)
        || uid.parse::<u32>().is_err()
    {
        return false;
    }
    let group_valid = ids.next().is_none_or(|gid| {
        !gid.is_empty()
            && gid.bytes().all(|c| c.is_ascii_digit())
            && gid.parse::<u32>().is_ok_and(|gid| gid != 0)
    });
    group_valid && ids.next().is_none()
}

/// The supported Dockerfile subset refuses implicit root execution and any
/// frontend that could change these instructions' meaning.
pub fn dockerfile_instructions(text: &str) -> Result<Vec<String>, SourceError> {
    let mut instructions = Vec::new();
    let mut line = String::new();
    for raw in text.lines() {
        let raw = raw.trim();
        if raw
            .split_whitespace()
            .next()
            .is_some_and(|word| word.eq_ignore_ascii_case("FROM"))
            && raw.ends_with('\\')
        {
            return Err(invalid("multiline FROM instructions are not supported"));
        }
        if let Some(comment) = raw.strip_prefix('#') {
            let comment = comment.trim_start().to_ascii_lowercase();
            if comment
                .split_once('=')
                .is_some_and(|(key, _)| matches!(key.trim(), "syntax" | "escape"))
            {
                return Err(invalid("Dockerfile parser directives are not supported"));
            }
            continue;
        }
        if raw.ends_with('\\') {
            line.push_str(raw.trim_end_matches('\\'));
            line.push(' ');
            continue;
        }
        line.push_str(raw);
        if !line.trim().is_empty() {
            instructions.push(line.trim().to_owned());
        }
        line.clear();
    }
    if !line.is_empty() {
        return Err(invalid("Dockerfile has an unfinished continuation"));
    }
    Ok(instructions)
}

pub fn dockerfile_policy(text: &str) -> Result<Vec<String>, SourceError> {
    let instructions = dockerfile_instructions(text)?;
    let mut stage = false;
    let mut user = false;
    let mut aliases = Vec::new();
    let mut images = Vec::new();
    for line in instructions {
        let (instruction, body) = line.split_once(char::is_whitespace).unwrap_or((&line, ""));
        match instruction.to_ascii_uppercase().as_str() {
            "FROM" => {
                let words: Vec<_> = body.split_whitespace().collect();
                if words.is_empty()
                    || words[0].starts_with('-')
                    || words[0].contains('$')
                    || !(words.len() == 1
                        || words.len() == 3 && words[1].eq_ignore_ascii_case("as"))
                {
                    return Err(invalid(
                        "FROM must name a literal image, optionally AS a stage",
                    ));
                }
                if words[0] != "scratch" && !aliases.contains(&words[0].to_ascii_lowercase()) {
                    images.push(words[0].into());
                }
                if words.len() == 3 {
                    aliases.push(words[2].to_ascii_lowercase());
                }
                stage = true;
                user = false;
            }
            "USER" => {
                if !stage || !nonroot_user(body.trim()) {
                    return Err(invalid(
                        "every USER must be a numeric nonzero uid and optional nonzero gid",
                    ));
                }
                user = true;
            }
            "RUN" => {
                if !stage || !user || body.contains("--security=insecure") || body.contains("<<") {
                    return Err(invalid(
                        "every RUN needs an explicit nonzero numeric USER in its stage; insecure runs and heredocs are refused",
                    ));
                }
            }
            "ONBUILD" => return Err(invalid("ONBUILD is not supported")),
            "ARG" => {}
            "COPY" | "ADD" | "WORKDIR" | "ENV" | "LABEL" | "EXPOSE" | "ENTRYPOINT" | "CMD"
            | "HEALTHCHECK" | "VOLUME" | "STOPSIGNAL" | "SHELL"
                if stage => {}
            _ => return Err(invalid("Dockerfile instruction is not supported")),
        }
    }
    if !stage || !user {
        return Err(invalid(
            "the final Dockerfile stage needs an explicit numeric nonzero USER",
        ));
    }
    images.sort();
    images.dedup();
    guard::dockerfile(text, "sf_guard_policy", &BTreeMap::new())?;
    Ok(images)
}
