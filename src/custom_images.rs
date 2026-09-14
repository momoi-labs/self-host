//! Custom image recipes and their latest build, kept in Platform State.

use std::{collections::BTreeSet, path::PathBuf};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, MutexGuard};

use crate::{AppState, error::ErrorReport, store::StateStore};

const STATE_KEY: &str = "custom_images_v1";

fn valid_tool(tool: &str) -> bool {
    !tool.is_empty()
        && tool.len() <= 160
        && tool.starts_with(|c: char| c.is_ascii_alphanumeric())
        && tool
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-:/@".contains(&b))
}

const ENTRYPOINT: &str = include_str!("custom_images/entrypoint.sh");
const PROFILE: &str = include_str!("custom_images/profile.sh");
const BUILD_CHECKS: &str = include_str!("custom_images/check.py");

// The two holes the recipe fills. They sit on their own line, so the value
// replaces the line and brings its own trailing newline.
const MISE_HOLE: &str = "{mise_config}\n";
const SETUP_HOLE: &str = "{setup}\n";

// The image carries tools only. Personal state lives in the optional /data
// volume, and the entrypoint starts the configured Application command as dev.
pub const DOCKERFILE: &str = r#"FROM debian:13-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl git gosu procps build-essential python3 unzip xz-utils \
    && rm -rf /var/lib/apt/lists/*
ARG USERNAME=dev
ARG USER_UID=1000
RUN useradd --uid "${USER_UID}" --user-group --no-create-home \
    --home-dir /data/home --shell /bin/bash "${USERNAME}"
ENV MISE_INSTALL_PATH=/usr/local/bin/mise \
    MISE_DATA_DIR=/opt/mise \
    MISE_CONFIG_DIR=/opt/mise/config \
    MISE_CACHE_DIR=/opt/mise/cache \
    MISE_TRUSTED_CONFIG_PATHS=/opt/mise/config \
    HOME=/data/home \
    T3CODE_HOME=/data/t3home \
    CARGO_HOME=/data/home/.cargo \
    RUSTUP_HOME=/opt/mise/rustup \
    PATH=/data/home/.cargo/bin:/opt/mise/cargo/bin:/opt/mise/shims:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin
RUN curl --proto '=https' --proto-redir '=https' -fsSL https://mise.run -o /tmp/install-mise.sh \
    && sh /tmp/install-mise.sh && rm /tmp/install-mise.sh
# --- dependencies (mise) ---
COPY <<'MISE_CONFIG_EOF' /opt/mise/config/config.toml
{mise_config}
MISE_CONFIG_EOF
# Keep rustup's toolchains and CLI in the image. The runtime CARGO_HOME stays
# under /data for a developer's own cache and cargo-installed programs.
RUN mkdir -p /data/home /tmp/mise-home /opt/mise/cargo \
    && HOME=/tmp/mise-home CARGO_HOME=/opt/mise/cargo mise install --yes \
    && HOME=/tmp/mise-home CARGO_HOME=/opt/mise/cargo mise reshim \
    && rm -rf /tmp/mise-home /opt/mise/cache \
    && chown -R "${USERNAME}:${USERNAME}" /opt/mise
# --- setup (root, with network) ---
{setup}
COPY runtime-profile.sh /etc/profile.d/self-host-custom-image.sh
COPY runtime-entrypoint.sh /usr/local/bin/self-host-custom-image-entrypoint
RUN chmod 0755 /usr/local/bin/self-host-custom-image-entrypoint
WORKDIR /data/repos
COPY build-checks.py /usr/local/lib/self-host-build-checks.py
RUN --mount=type=tmpfs,target=/data --network=none \
    /usr/local/bin/self-host-custom-image-entrypoint python3 /usr/local/lib/self-host-build-checks.py
ENTRYPOINT ["/usr/local/bin/self-host-custom-image-entrypoint"]
CMD ["sleep", "infinity"]
"#;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Dependency {
    pub tool: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_builds: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Recipe {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_id: Option<String>,
    pub dependencies: Vec<Dependency>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub setup: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build_checks: Vec<String>,
    /// The file an Operator took over. `Some` switches the builder off: the
    /// Host builds this text as is (ADR-0022).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dockerfile: Option<String>,
}

/// A list of shell commands the build runs one per line: at most 16, each a
/// single command of 1 to 4096 bytes.
fn valid_commands(commands: &[String]) -> bool {
    commands.len() <= 16
        && !commands.iter().any(|command| {
            command.trim().is_empty()
                || command.len() > 4096
                || command.chars().any(char::is_control)
        })
}

impl Recipe {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self
            .template_id
            .as_deref()
            .is_some_and(|id| id != "t3-code")
        {
            return Err("Unknown custom image template.".into());
        }
        if self.name.trim().is_empty() || self.name.chars().count() > 128 {
            return Err("Use an image name of 1 to 128 characters.".into());
        }
        if let Some(dockerfile) = &self.dockerfile {
            return self.validate_manual(dockerfile);
        }
        self.validate_builder(true)
    }

    /// The same recipe read as a machine's. A machine may install no mise
    /// tools at all: plain Ubuntu with a couple of custom commands is a
    /// workspace an Operator may well have asked for, while an image with
    /// nothing to install has nothing to build.
    pub(crate) fn validate_machine(&self) -> Result<(), String> {
        if self
            .template_id
            .as_deref()
            .is_some_and(|id| id != "t3-code")
        {
            return Err("Unknown custom image template.".into());
        }
        self.validate_builder(false)
    }

    /// A file the Operator owns. Nothing is generated for it, so the builder's
    /// fields must arrive empty rather than be silently dropped.
    fn validate_manual(&self, dockerfile: &str) -> Result<(), String> {
        if dockerfile.is_empty()
            || dockerfile.len() > 65536
            || dockerfile
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
        {
            return Err("Write a Dockerfile of 1 to 65536 bytes.".into());
        }
        if !dockerfile
            .lines()
            .any(|line| line.trim_start().to_uppercase().starts_with("FROM "))
        {
            return Err("A Dockerfile needs a FROM instruction.".into());
        }
        if !self.dependencies.is_empty() || !self.setup.is_empty() || !self.build_checks.is_empty()
        {
            return Err(
                "An edited Dockerfile replaces the dependencies, the setup and the build checks."
                    .into(),
            );
        }
        Ok(())
    }

    fn validate_builder(&self, require_dependencies: bool) -> Result<(), String> {
        if !valid_commands(&self.build_checks) {
            return Err(
                "Use at most 16 build checks, each a single command of 1 to 4096 bytes.".into(),
            );
        }
        if !valid_commands(&self.setup) {
            return Err(
                "Use at most 16 setup commands, each a single command of 1 to 4096 bytes.".into(),
            );
        }
        if (require_dependencies && self.dependencies.is_empty()) || self.dependencies.len() > 64 {
            return Err("Select at least one dependency, with one version per tool.".into());
        }
        let mut seen = BTreeSet::new();
        for dep in &self.dependencies {
            if !dep.allow_builds.is_empty()
                && (!dep.tool.starts_with("npm:")
                    || dep.allow_builds.len() > 64
                    || dep.allow_builds.iter().any(|name| {
                        name.len() > 160
                            || name.is_empty()
                            || !name
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b"@/._-".contains(&b))
                    }))
            {
                return Err("allow_builds must list npm package names on an npm tool.".into());
            }
            if !valid_tool(&dep.tool) || !seen.insert(&dep.tool) {
                return Err(format!("Invalid or repeated mise key: {}", dep.tool));
            }
            if dep.version.is_empty()
                || dep.version.len() > 64
                || !dep.version.starts_with(|c: char| c.is_ascii_alphanumeric())
                || !dep
                    .version
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
            {
                return Err(format!(
                    "Enter a version or alias such as latest for {}. Only letters, digits, dots, hyphens and underscores are supported.",
                    dep.tool
                ));
            }
        }
        Ok(())
    }

    pub fn mise_toml(&self) -> String {
        let mut config = String::from("[tools]\n");
        for dep in &self.dependencies {
            config.push_str(&format!(
                "{} = {}\n",
                serde_json::to_string(&dep.tool).unwrap(),
                if dep.allow_builds.is_empty() {
                    serde_json::to_string(&dep.version).unwrap()
                } else {
                    format!(
                        "{{ version = {}, allow_builds = {} }}",
                        serde_json::to_string(&dep.version).unwrap(),
                        serde_json::to_string(&dep.allow_builds).unwrap()
                    )
                }
            ));
        }
        if !self.build_checks.is_empty() {
            config.push_str(&format!(
                "\n[tasks.check]\nrun = {}\n",
                serde_json::to_string(&self.build_checks).unwrap()
            ));
        }
        config
    }

    /// The file the Host builds. The mise config travels inside it as a
    /// heredoc so the text an Operator reads carries its own dependencies.
    pub fn dockerfile_text(&self) -> String {
        if let Some(dockerfile) = &self.dockerfile {
            return dockerfile.clone();
        }
        let (head, rest) = DOCKERFILE
            .split_once(MISE_HOLE)
            .expect("the Dockerfile template keeps its mise hole");
        let (body, tail) = rest
            .split_once(SETUP_HOLE)
            .expect("the Dockerfile template keeps its setup hole");
        let mut setup = String::new();
        for command in &self.setup {
            setup.push_str(&format!("RUN {command}\n"));
        }
        // mise install leaves /opt/mise owned by dev; a setup command running
        // as root can undo that, so the section restores it when it ran.
        if !setup.is_empty() {
            setup.push_str("RUN chown -R \"${USERNAME}:${USERNAME}\" /opt/mise\n");
        }
        format!("{head}{}{body}{setup}{tail}", self.mise_toml())
    }

    fn image_tag(&self, id: &str) -> String {
        format!(
            "sf-img-{}:{:x}",
            id,
            Md5::digest(self.dockerfile_text().as_bytes())
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Image {
    pub id: String,
    #[serde(flatten)]
    pub recipe: Recipe,
    pub image: String,
    pub status: String,
    pub last_error: Option<ErrorReport>,
    #[serde(default)]
    pub log: String,
}

#[derive(Default)]
pub(crate) struct Builds {
    records: Mutex<Option<Vec<Image>>>,
}

impl Builds {
    async fn records<S: StateStore>(
        &self,
        store: &S,
    ) -> anyhow::Result<MutexGuard<'_, Option<Vec<Image>>>> {
        let mut records = self.records.lock().await;
        if records.is_none() {
            let mut loaded: Vec<Image> = match store.get_state(STATE_KEY).await? {
                Some(json) => serde_json::from_str(&json)?,
                None => Vec::new(),
            };
            let mut interrupted = false;
            for image in &mut loaded {
                if image.status == "building" {
                    image.status = "failed".into();
                    image.last_error = Some(ErrorReport::plain(
                        "The Platform restarted before this build finished. Build the image again.",
                    ));
                    interrupted = true;
                }
            }
            if interrupted {
                save(store, &loaded).await?;
            }
            *records = Some(loaded);
        }
        Ok(records)
    }
}

async fn save<S: StateStore>(store: &S, images: &[Image]) -> anyhow::Result<()> {
    store
        .store_state(STATE_KEY, &serde_json::to_string(images)?)
        .await?;
    Ok(())
}

#[derive(Deserialize)]
pub(crate) struct ToolSearch {
    #[serde(default)]
    q: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Tool {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    backends: Vec<String>,
}

#[derive(Deserialize)]
struct ToolResults {
    tools: Vec<Tool>,
    total_pages: usize,
}

// Tool names are stable enough to cache for this server's lifetime. Failed
// loads are not cached, and concurrent callers share the first successful load.
static TOOL_CATALOG: tokio::sync::OnceCell<Vec<Tool>> = tokio::sync::OnceCell::const_new();

async fn cached_tool_catalog<'a>(
    cache: &'a tokio::sync::OnceCell<Vec<Tool>>,
    url: &str,
) -> anyhow::Result<&'a Vec<Tool>> {
    cache
        .get_or_try_init(|| async {
            tokio::time::timeout(std::time::Duration::from_secs(15), load_tool_catalog(url)).await?
        })
        .await
}

async fn load_tool_catalog(url: &str) -> anyhow::Result<Vec<Tool>> {
    use futures_util::{StreamExt, TryStreamExt};

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()?;
    let page = |page: usize| {
        let client = &client;
        async move {
            Ok::<_, anyhow::Error>(
                client
                    .get(url)
                    .query(&[
                        ("page", page.to_string()),
                        ("limit", "100".into()),
                        ("sort", "name".into()),
                    ])
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<ToolResults>()
                    .await?,
            )
        }
    };
    let first = page(1).await?;
    anyhow::ensure!(
        (1..=100).contains(&first.total_pages),
        "Invalid tool catalog page count"
    );
    let remaining: Vec<ToolResults> = futures_util::stream::iter(2..=first.total_pages)
        .map(page)
        .buffer_unordered(4)
        .try_collect()
        .await?;
    let mut tools = first.tools;
    tools.extend(remaining.into_iter().flat_map(|page| page.tools));
    tools.retain(|tool| valid_tool(&tool.name));
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    tools.dedup_by(|a, b| a.name == b.name);
    Ok(tools)
}

pub(crate) async fn catalog(Query(query): Query<ToolSearch>) -> Response {
    if query.q.len() > 160 {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorReport::plain("Search is too long.")),
        )
            .into_response();
    }
    let result =
        cached_tool_catalog(&TOOL_CATALOG, "https://mise-versions.jdx.dev/api/tools").await;
    match result {
        Ok(tools) => {
            let term = query.q.trim().to_lowercase();
            let matches: Vec<&Tool> = tools
                .iter()
                .filter(|tool| {
                    tool.name.to_lowercase().contains(&term)
                        || tool
                            .backends
                            .iter()
                            .any(|key| key.to_lowercase().contains(&term))
                })
                .collect();
            Json(matches).into_response()
        }
        Err(error) => crate::error_response(StatusCode::BAD_GATEWAY, error.as_ref()),
    }
}

pub(crate) async fn list<S: StateStore>(State(state): State<AppState<S>>) -> Response {
    let references = referenced_images(&state).await;
    match state.custom_images.records(&state.store).await {
        Ok(records) => Json(
            records
                .as_ref()
                .unwrap()
                .iter()
                .map(|image| ListedImage {
                    image: image.clone(),
                    in_use: references
                        .as_ref()
                        .ok()
                        .map(|references| is_in_use(image, references)),
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => crate::error_response(StatusCode::INTERNAL_SERVER_ERROR, error.as_ref()),
    }
}

#[derive(Serialize)]
struct ListedImage {
    #[serde(flatten)]
    image: Image,
    in_use: Option<bool>,
}

fn repository(tag: &str) -> &str {
    tag.split([':', '@']).next().unwrap_or(tag)
}

fn is_in_use(image: &Image, references: &[String]) -> bool {
    references
        .iter()
        .any(|tag| repository(tag) == repository(&image.image))
}

async fn referenced_images<S: StateStore>(state: &AppState<S>) -> anyhow::Result<Vec<String>> {
    let mut references = state.docker.container_images().await?;
    for app in state.store.list_applications().await? {
        references.push(app.image);
        if let Some(compose) = app.compose {
            let definition = crate::compose_app::ComposeDefinition::parse(&compose)?;
            references.extend(definition.services.into_iter().map(|service| service.image));
        }
    }
    Ok(references)
}

/// Returns a ready custom image when its stable ID still names the exact
/// requested tag. Callers keep already deployed tags independently, so an
/// image edit cannot invalidate an Application that still uses an older tag.
pub(crate) async fn ready_image<S: StateStore>(
    state: &AppState<S>,
    id: &str,
    tag: &str,
) -> anyhow::Result<Option<Image>> {
    let records = state.custom_images.records(&state.store).await?;
    Ok(records
        .as_ref()
        .unwrap()
        .iter()
        .find(|image| image.id == id && image.image == tag && image.status == "ready")
        .cloned())
}

pub(crate) async fn remove<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
) -> Response {
    let mut records = match state.custom_images.records(&state.store).await {
        Ok(records) => records,
        Err(error) => {
            return crate::error_response(StatusCode::INTERNAL_SERVER_ERROR, error.as_ref());
        }
    };
    let mut next = records.as_ref().unwrap().clone();
    let Some(image) = next.iter().find(|image| image.id == id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(ErrorReport::plain("Image not found.")),
        )
            .into_response();
    };
    if image.status == "building" {
        return (
            StatusCode::CONFLICT,
            Json(ErrorReport::plain(
                "Wait for the build to finish before deleting this image.",
            )),
        )
            .into_response();
    }
    let references = match referenced_images(&state).await {
        Ok(references) => references,
        Err(error) => {
            return crate::error_response(StatusCode::SERVICE_UNAVAILABLE, error.as_ref());
        }
    };
    if is_in_use(image, &references) {
        return (
            StatusCode::CONFLICT,
            Json(ErrorReport::plain(
                "This image is in use by an application or container.",
            )),
        )
            .into_response();
    }
    if let Err(error) = state
        .docker
        .remove_image_repository(repository(&image.image))
        .await
    {
        return crate::error_response(StatusCode::CONFLICT, &error);
    }
    next.retain(|image| image.id != id);
    if let Err(error) = save(&state.store, &next).await {
        return crate::error_response(StatusCode::INTERNAL_SERVER_ERROR, error.as_ref());
    }
    *records = Some(next);
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
pub(crate) struct SaveAndBuild {
    id: Option<String>,
    #[serde(flatten)]
    recipe: Recipe,
}

pub(crate) async fn create<S: StateStore>(
    State(state): State<AppState<S>>,
    Json(request): Json<SaveAndBuild>,
) -> Response {
    let mut recipe = request.recipe;
    recipe.name = recipe.name.trim().to_owned();
    if let Err(error) = recipe.validate() {
        return (StatusCode::BAD_REQUEST, Json(ErrorReport::plain(error))).into_response();
    }
    let mut records = match state.custom_images.records(&state.store).await {
        Ok(records) => records,
        Err(error) => {
            return crate::error_response(StatusCode::INTERNAL_SERVER_ERROR, error.as_ref());
        }
    };
    let mut next = records.as_ref().unwrap().clone();
    if next.iter().any(|image| image.status == "building") {
        return (
            StatusCode::CONFLICT,
            Json(ErrorReport::plain(
                "An image is already building. Wait for it to finish.",
            )),
        )
            .into_response();
    }
    let audit_action = if request.id.is_some() {
        "configure"
    } else {
        "create"
    };
    let id = match request.id {
        Some(id) if next.iter().any(|image| image.id == id) => id,
        Some(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorReport::plain("Image not found.")),
            )
                .into_response();
        }
        None => crate::apps::generate_app_id(),
    };
    if next
        .iter()
        .any(|image| image.id != id && image.recipe.name == recipe.name)
    {
        return (
            StatusCode::CONFLICT,
            Json(ErrorReport::plain(
                "An image with this name already exists. Open it to edit.",
            )),
        )
            .into_response();
    }
    let image = Image {
        image: recipe.image_tag(&id),
        id,
        recipe,
        status: "building".into(),
        last_error: None,
        log: String::from("Starting Docker build...\n"),
    };
    // Changed configuration gets a new tag; identical configuration reuses it.
    next.retain(|old| old.id != image.id);
    next.insert(0, image.clone());
    if let Err(error) = save(&state.store, &next).await {
        return crate::error_response(StatusCode::INTERNAL_SERVER_ERROR, error.as_ref());
    }
    *records = Some(next);
    drop(records);
    let accepted = image.clone();
    let actor = crate::audit::actor();
    tokio::spawn(
        crate::audit::EVENT_ID.scope(crate::audit::event_id(), async move {
            let result = build(&state, &image).await;
            let mut outcome = if result.is_ok() {
                "completed"
            } else {
                "failed"
            };
            let mut records = state.custom_images.records.lock().await;
            let images = records.as_mut().unwrap();
            let record = images
                .iter_mut()
                .find(|record| record.id == image.id)
                .unwrap();
            match result {
                Ok(()) => {
                    record.status = "ready".into();
                    append_log(
                        &mut record.log,
                        &format!("\nImage built: {}\n", record.image),
                    );
                }
                Err(error) => {
                    record.status = "failed".into();
                    append_log(&mut record.log, &format!("\nBuild failed: {error:#}\n"));
                    record.last_error = Some(ErrorReport::new(error.as_ref()));
                }
            }
            if let Err(error) = save(&state.store, images).await {
                outcome = "failed";
                tracing::error!(%error, "Could not save custom image build result");
                let record = images
                    .iter_mut()
                    .find(|record| record.id == image.id)
                    .unwrap();
                record.status = "failed".into();
                record.last_error = Some(ErrorReport::plain(format!(
                    "Could not save the build result: {error}"
                )));
            }
            drop(records);
            crate::audit::record(
                &state,
                crate::audit::event(
                    audit_action,
                    crate::audit::Subject {
                        kind: "custom-image".into(),
                        id: image.id.clone(),
                        name: image.recipe.name.clone(),
                        available: None,
                    },
                    outcome,
                    actor,
                    format!("Custom image build {outcome}."),
                ),
            )
            .await;
        }),
    );
    (StatusCode::ACCEPTED, Json(accepted)).into_response()
}

/// Renders the file a builder recipe would build. The console calls this when
/// the Operator takes the file over, so the template stays on the Host and
/// nothing is stored here.
pub(crate) async fn render_dockerfile(Json(recipe): Json<Recipe>) -> Response {
    if recipe.dockerfile.is_some() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorReport::plain(
                "This recipe already carries its own Dockerfile.",
            )),
        )
            .into_response();
    }
    // The name plays no part in the render, so an unnamed recipe still has a
    // file to show.
    if let Err(error) = recipe.validate_builder(true) {
        return (StatusCode::BAD_REQUEST, Json(ErrorReport::plain(error))).into_response();
    }
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        recipe.dockerfile_text(),
    )
        .into_response()
}

struct BuildContext(PathBuf);

impl Drop for BuildContext {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn build<S: StateStore>(state: &AppState<S>, image: &Image) -> anyhow::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let path = std::env::temp_dir().join(format!("self-host-custom-image-{}", image.id));
    std::fs::DirBuilder::new().mode(0o700).create(&path)?;
    let context = BuildContext(path);
    write_build_context(&context.0, image)?;
    let path = context
        .0
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Build path is not UTF-8"))?;
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<String>(64);
    let collect = async {
        while let Some(chunk) = receiver.recv().await {
            let mut records = state.custom_images.records.lock().await;
            if let Some(record) = records
                .as_mut()
                .and_then(|images| images.iter_mut().find(|record| record.id == image.id))
            {
                append_log(&mut record.log, &chunk);
            }
        }
    };
    let (result, ()) = tokio::join!(
        state
            .docker
            .build_image_with_logs(path, &image.image, sender),
        collect
    );
    result?;
    Ok(())
}

fn write_build_context(path: &std::path::Path, image: &Image) -> std::io::Result<()> {
    std::fs::write(path.join("Dockerfile"), image.recipe.dockerfile_text())?;
    std::fs::write(path.join("runtime-entrypoint.sh"), ENTRYPOINT)?;
    std::fs::write(path.join("runtime-profile.sh"), PROFILE)?;
    std::fs::write(path.join("build-checks.py"), BUILD_CHECKS)?;
    Ok(())
}

// Keep the tail in memory while building; save it with the final result.
fn append_log(log: &mut String, chunk: &str) {
    const MAX_BYTES: usize = 256 * 1024;
    log.push_str(chunk);
    if log.len() > MAX_BYTES {
        let mut start = log.len() - MAX_BYTES;
        while !log.is_char_boundary(start) {
            start += 1;
        }
        log.drain(..start);
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn catalog_retries_failures_loads_every_page_and_survives_upstream_loss() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let router = axum::Router::new().route(
            "/tools",
            axum::routing::get(
                move |axum::extract::Query(query): axum::extract::Query<
                    std::collections::HashMap<String, String>,
                >| {
                    let calls = observed.clone();
                    async move {
                        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                            return (
                                axum::http::StatusCode::BAD_GATEWAY,
                                axum::Json(serde_json::json!({})),
                            )
                                .into_response();
                        }
                        assert_eq!(query["limit"], "100");
                        assert_eq!(query["sort"], "name");
                        let name = match query["page"].as_str() {
                            "1" => "claude-code",
                            "2" => "just",
                            "3" => "node",
                            _ => panic!("unexpected page"),
                        };
                        axum::Json(serde_json::json!({"total_pages": 3, "tools": [{"name": name}]}))
                            .into_response()
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/tools", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let cache = tokio::sync::OnceCell::new();
        assert!(super::cached_tool_catalog(&cache, &url).await.is_err());
        let (a, b) = tokio::join!(
            super::cached_tool_catalog(&cache, &url),
            super::cached_tool_catalog(&cache, &url)
        );
        assert_eq!(
            a.unwrap()
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["claude-code", "just", "node"]
        );
        assert_eq!(b.unwrap().len(), 3);
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        server.abort();
        assert_eq!(
            super::cached_tool_catalog(&cache, &url)
                .await
                .unwrap()
                .len(),
            3
        );
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    use super::*;

    #[test]
    fn image_tag_uses_the_md5_of_the_exact_dockerfile() {
        let mut recipe = Recipe {
            build_checks: vec![],
            setup: vec![],
            dockerfile: None,
            template_id: None,
            name: "Minha Imagem de Ação".into(),
            dependencies: vec![Dependency {
                tool: "node".into(),
                version: "24".into(),
                allow_builds: vec![],
            }],
        };
        let id = crate::apps::generate_app_id();
        let tag = recipe.image_tag(&id);
        assert_eq!(tag, format!("sf-img-{id}:e40f5fd794b982f958b5c581efd83dc1"));
        recipe.name = "A different display name".repeat(4);
        assert!(recipe.validate().is_ok());
        assert_eq!(recipe.image_tag(&id), tag);
        recipe.dependencies[0].version = "22".into();
        assert_ne!(recipe.image_tag(&id), tag);
    }

    #[test]
    fn npm_recipe_is_passed_to_mise_without_adding_a_runtime() {
        let recipe = Recipe {
            build_checks: vec![],
            setup: vec![],
            dockerfile: None,
            template_id: None,
            name: "coding".into(),
            dependencies: vec![Dependency {
                tool: "npm:@openai/codex".into(),
                version: "latest".into(),
                allow_builds: vec![],
            }],
        };
        assert!(recipe.validate().is_ok());
        assert_eq!(
            recipe.mise_toml(),
            "[tools]\n\"npm:@openai/codex\" = \"latest\"\n"
        );
    }

    #[test]
    fn explicit_npm_build_permissions_are_persisted_and_change_the_tag() {
        let mut recipe: Recipe = serde_json::from_str(
            r#"{"name":"t3","dependencies":[{"tool":"npm:t3","version":"latest"}]}"#,
        )
        .unwrap();
        let original = recipe.image_tag("test");
        recipe.dependencies[0].allow_builds = vec!["node-pty".into()];
        assert!(recipe.validate().is_ok());
        assert_eq!(
            recipe.mise_toml(),
            "[tools]\n\"npm:t3\" = { version = \"latest\", allow_builds = [\"node-pty\"] }\n"
        );
        assert_ne!(recipe.image_tag("test"), original);
        let restored: Recipe =
            serde_json::from_str(&serde_json::to_string(&recipe).unwrap()).unwrap();
        assert_eq!(restored.mise_toml(), recipe.mise_toml());
        recipe.dependencies[0].tool = "node".into();
        assert!(recipe.validate().is_err());
    }

    #[test]
    fn validates_recipes_before_rendering_config() {
        let mut recipe = Recipe {
            build_checks: vec![],
            setup: vec![],
            dockerfile: None,
            template_id: None,
            name: "web-dev".into(),
            dependencies: vec![Dependency {
                tool: "node".into(),
                version: "24".into(),
                allow_builds: vec![],
            }],
        };
        assert!(recipe.validate().is_ok());
        assert_eq!(recipe.mise_toml(), "[tools]\n\"node\" = \"24\"\n");
        for version in [
            "",
            "$(id)",
            "{{exec(command='id')}}",
            "24\"\n[hooks]",
            "path:/tmp/tool",
            "--help",
        ] {
            recipe.dependencies[0].version = version.into();
            assert!(recipe.validate().is_err(), "{version}");
        }
        recipe.dependencies[0].version = "latest".into();
        recipe.dependencies.push(recipe.dependencies[0].clone());
        assert!(recipe.validate().is_err());
        recipe.dependencies.pop();
        recipe.dependencies[0].tool = "node\"\n[hooks]".into();
        assert!(recipe.validate().is_err());
        recipe.dependencies[0].tool = "node".into();
        for name in ["", "   ", &"x".repeat(129)] {
            recipe.name = name.into();
            assert!(recipe.validate().is_err(), "{name}");
        }
    }

    #[test]
    fn templates_do_not_change_recipe_contents_or_the_image_tag() {
        let mut recipe: Recipe = serde_json::from_str(
            r#"{"name":"custom","dependencies":[{"tool":"node","version":"24"}]}"#,
        )
        .unwrap();
        let original = recipe.image_tag("test");
        recipe.template_id = Some("t3-code".into());
        assert!(recipe.validate().is_ok());
        assert_eq!(recipe.image_tag("test"), original);
        assert_eq!(recipe.dependencies.len(), 1);
        assert!(recipe.build_checks.is_empty());
        recipe.template_id = Some("missing-template".into());
        assert!(recipe.validate().is_err());
    }

    #[test]
    fn build_checks_preserve_shell_commands_and_change_the_mise_digest() {
        let mut recipe: Recipe = serde_json::from_str(
            r#"{"name":"checks","dependencies":[{"tool":"node","version":"24"}]}"#,
        )
        .unwrap();
        let original = recipe.image_tag("test");
        recipe.build_checks = vec![r#"node -e 'console.log("installed")'"#.into()];
        assert!(recipe.validate().is_ok());
        assert!(
            recipe.mise_toml().ends_with(
                "\n[tasks.check]\nrun = [\"node -e 'console.log(\\\"installed\\\")'\"]\n"
            )
        );
        assert_ne!(recipe.image_tag("test"), original);
        let restored: Recipe =
            serde_json::from_str(&serde_json::to_string(&recipe).unwrap()).unwrap();
        assert_eq!(restored.mise_toml(), recipe.mise_toml());
        for invalid in ["", " \t", "true\nfalse", "echo\0bad"] {
            recipe.build_checks = vec![invalid.into()];
            assert!(recipe.validate().is_err());
        }
        recipe.build_checks = vec!["true".into(); 17];
        assert!(recipe.validate().is_err());
    }

    #[test]
    fn setup_commands_run_as_their_own_layers_between_mise_and_the_checks() {
        let mut recipe: Recipe = serde_json::from_str(
            r#"{"name":"hermes","dependencies":[{"tool":"node","version":"24"}]}"#,
        )
        .unwrap();
        let plain = recipe.dockerfile_text();
        assert!(plain.contains("# --- setup (root, with network) ---\nCOPY runtime-profile.sh"));
        assert!(!plain.contains("RUN chown -R \"${USERNAME}:${USERNAME}\" /opt/mise\nCOPY"));

        let original = recipe.image_tag("test");
        recipe.setup = vec![
            "curl -fsSL https://example.test/install.sh | bash".into(),
            "uv tool install ruff".into(),
        ];
        assert!(recipe.validate().is_ok());
        let rendered = recipe.dockerfile_text();
        let setup = rendered
            .split_once("# --- setup (root, with network) ---\n")
            .unwrap()
            .1;
        assert_eq!(
            setup.lines().take(3).collect::<Vec<_>>(),
            [
                "RUN curl -fsSL https://example.test/install.sh | bash",
                "RUN uv tool install ruff",
                "RUN chown -R \"${USERNAME}:${USERNAME}\" /opt/mise",
            ]
        );
        assert!(setup.starts_with("RUN curl"));
        assert!(setup.contains("COPY runtime-profile.sh"));
        // The checks still run after setup, and still without a network.
        assert!(
            rendered.find("RUN uv tool install ruff").unwrap()
                < rendered.find("--network=none").unwrap()
        );
        assert_ne!(recipe.image_tag("test"), original);

        let restored: Recipe =
            serde_json::from_str(&serde_json::to_string(&recipe).unwrap()).unwrap();
        assert_eq!(restored.dockerfile_text(), rendered);
        for invalid in ["", " \t", "true\nfalse", "echo\0bad"] {
            recipe.setup = vec![invalid.into()];
            assert!(recipe.validate().is_err(), "{invalid}");
        }
        recipe.setup = vec!["true".into(); 17];
        assert!(recipe.validate().is_err());
    }

    #[test]
    fn the_mise_config_travels_inside_the_dockerfile_as_a_heredoc() {
        let recipe: Recipe = serde_json::from_str(
            r#"{"name":"heredoc","dependencies":[{"tool":"npm:t3","version":"latest"}],
                "build_checks":["t3 --help"]}"#,
        )
        .unwrap();
        let rendered = recipe.dockerfile_text();
        let body = rendered
            .split_once("COPY <<'MISE_CONFIG_EOF' /opt/mise/config/config.toml\n")
            .unwrap()
            .1
            .split_once("MISE_CONFIG_EOF\n")
            .unwrap()
            .0;
        assert_eq!(body, recipe.mise_toml());
        assert!(body.contains("[tasks.check]"));
        // A quoted delimiter, so a $ in a version is not expanded by the build.
        assert!(rendered.contains("<<'MISE_CONFIG_EOF'"));
    }

    #[test]
    fn an_edited_dockerfile_is_built_as_is_and_owns_the_whole_recipe() {
        let mut recipe: Recipe = serde_json::from_str(
            r#"{"name":"hand written","dependencies":[],"dockerfile":"FROM debian:13-slim\nRUN true\n"}"#,
        )
        .unwrap();
        assert!(recipe.validate().is_ok());
        assert_eq!(recipe.dockerfile_text(), "FROM debian:13-slim\nRUN true\n");
        let tag = recipe.image_tag("test");

        recipe.name = "renamed".into();
        assert_eq!(recipe.image_tag("test"), tag);
        recipe.dockerfile = Some("FROM debian:13-slim\nRUN false\n".into());
        assert_ne!(recipe.image_tag("test"), tag);

        // Indentation and case are the Dockerfile's, not ours.
        recipe.dockerfile = Some("# a comment\n  from debian:13-slim\n".into());
        assert!(recipe.validate().is_ok());

        for invalid in ["", "RUN true\n", &"#\n".repeat(40000), "FROM debian\u{7}"] {
            recipe.dockerfile = Some(invalid.into());
            assert!(recipe.validate().is_err(), "{invalid}");
        }
        recipe.dockerfile = Some("FROM debian:13-slim\n".into());
        for leftover in [
            r#"{"name":"x","dependencies":[{"tool":"node","version":"24"}],"dockerfile":"FROM x\n"}"#,
            r#"{"name":"x","dependencies":[],"setup":["true"],"dockerfile":"FROM x\n"}"#,
            r#"{"name":"x","dependencies":[],"build_checks":["true"],"dockerfile":"FROM x\n"}"#,
        ] {
            let recipe: Recipe = serde_json::from_str(leftover).unwrap();
            assert!(recipe.validate().is_err(), "{leftover}");
        }
        // A builder recipe still needs its dependencies.
        recipe.dockerfile = None;
        assert!(recipe.validate().is_err());
    }

    #[test]
    fn generated_context_includes_every_runtime_asset_referenced_by_dockerfile() {
        let path = std::env::temp_dir().join(format!(
            "self-host-custom-image-context-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir(&path).unwrap();
        let image = Image {
            id: "test".into(),
            recipe: Recipe {
                build_checks: vec![],
                setup: vec![],
                dockerfile: None,
                template_id: None,
                name: "test".into(),
                dependencies: vec![Dependency {
                    tool: "node".into(),
                    version: "24".into(),
                    allow_builds: vec![],
                }],
            },
            image: "sf-img-test:latest".into(),
            status: "ready".into(),
            last_error: None,
            log: String::new(),
        };

        write_build_context(&path, &image).unwrap();

        let dockerfile = std::fs::read_to_string(path.join("Dockerfile")).unwrap();
        assert_eq!(dockerfile, image.recipe.dockerfile_text());
        for source in dockerfile.lines().filter_map(|line| {
            line.strip_prefix("COPY ")
                .and_then(|line| line.split_whitespace().next())
                .filter(|source| !source.starts_with("<<"))
        }) {
            assert!(path.join(source).is_file(), "missing COPY source: {source}");
        }
        assert!(!path.join("mise.toml").exists());
        std::fs::remove_dir_all(path).unwrap();
    }
}
