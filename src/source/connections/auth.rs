//! Provider authorization and renewable access stay in private Host storage.

use super::{
    Authentication, Connection, ConnectionStatus, Credential, CredentialKind, CredentialMetadata,
    Provider, RECORDS, invalid,
};
use crate::source::SourceError;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

const TTL: u64 = 600;
const MAX_PENDING: usize = 256;
const RENEW_BEFORE: i64 = 180;
const REJECTED: &str = "provider authorization was rejected; reconnect access";
fn is_rejected(error: &SourceError) -> bool {
    matches!(error, SourceError::Invalid(message) if message == REJECTED)
}
const GITLAB_SCOPES: [&str; 3] = ["read_user", "read_api", "read_repository"];

#[derive(Clone, Debug, Serialize)]
pub struct GithubSettings {
    pub configured: bool,
    pub app_slug: Option<String>,
    pub console_url: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct GitlabSettings {
    pub configured: bool,
    pub client_id: Option<String>,
    pub console_url: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct IntegrationSettings {
    pub github: GithubSettings,
    pub gitlab: GitlabSettings,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitlabIntegrationRequest {
    pub console_url: String,
    pub client_id: String,
    pub client_secret: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubRegistrationRequest {
    pub console_url: String,
    #[serde(default)]
    pub organization: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationRequest {
    pub provider: Provider,
    pub name: String,
    #[serde(default)]
    pub connection_id: Option<String>,
    #[serde(default)]
    pub use_existing_installation: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationCompletion {
    pub state: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub installation_id: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
}
#[derive(Serialize)]
pub struct AuthorizationForm {
    pub action: String,
    pub manifest: String,
}
#[derive(Serialize)]
pub struct AuthorizationStart {
    pub state: String,
    pub url: Option<String>,
    pub form: Option<AuthorizationForm>,
    pub expires_in: u64,
}
#[derive(Serialize)]
pub struct AuthorizationResult {
    pub connection: Option<Connection>,
    pub integrations: Option<IntegrationSettings>,
    pub authorization: Option<AuthorizationStart>,
}

#[derive(Clone, Serialize, Deserialize)]
struct GithubConfig {
    generation: String,
    console_url: String,
    app_id: u64,
    app_slug: String,
    client_id: String,
    client_secret: String,
    private_key: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct GitlabConfig {
    generation: String,
    console_url: String,
    client_id: String,
    client_secret: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "lowercase")]
enum Grant {
    Github {
        config: String,
        installation: u64,
        account: u64,
        access_token: String,
        expires_at: i64,
    },
    Gitlab {
        config: String,
        account: u64,
        access_token: String,
        refresh_token: String,
        expires_at: i64,
    },
}
impl Grant {
    fn token(&self) -> &str {
        match self {
            Self::Github { access_token, .. } | Self::Gitlab { access_token, .. } => access_token,
        }
    }
    fn expires_at(&self) -> i64 {
        match self {
            Self::Github { expires_at, .. } | Self::Gitlab { expires_at, .. } => *expires_at,
        }
    }
    fn same_identity(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Github {
                    config: a,
                    installation: ai,
                    account: aa,
                    ..
                },
                Self::Github {
                    config: b,
                    installation: bi,
                    account: ba,
                    ..
                },
            ) => a == b && ai == bi && aa == ba,
            (
                Self::Gitlab {
                    config: a,
                    account: aa,
                    ..
                },
                Self::Gitlab {
                    config: b,
                    account: ba,
                    ..
                },
            ) => a == b && aa == ba,
            _ => false,
        }
    }
}
#[derive(Clone)]
struct Reconnect {
    id: String,
    generation: u64,
    credential_id: String,
}
#[derive(Clone)]
enum PendingKind {
    Registration {
        console_url: String,
        previous: Option<String>,
    },
    GithubInstallation {
        config: String,
        name: String,
        reconnect: Option<Reconnect>,
    },
    Github {
        config: String,
        name: String,
        verifier: String,
        installation: Option<u64>,
        reconnect: Option<Reconnect>,
    },
    Gitlab {
        config: String,
        name: String,
        verifier: String,
        reconnect: Option<Reconnect>,
    },
}
#[derive(Clone)]
struct Pending {
    deadline: Instant,
    origin: String,
    kind: PendingKind,
}
static SESSIONS: LazyLock<Mutex<BTreeMap<(PathBuf, String), Pending>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
type RenewalLocks = BTreeMap<(PathBuf, String), Arc<tokio::sync::Mutex<()>>>;
static RENEWALS: LazyLock<Mutex<RenewalLocks>> = LazyLock::new(|| Mutex::new(BTreeMap::new()));

fn random() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}
fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
fn configuration(root: &Path, name: &str) -> PathBuf {
    root.join("integrations").join(name)
}
fn grant_path(root: &Path, id: &str) -> Result<PathBuf, SourceError> {
    if !super::super::valid_id(id) {
        return Err(invalid("Git connection id is invalid"));
    }
    Ok(root.join("provider-grants").join(id))
}
fn read_private<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, SourceError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.len() > 1024 * 1024
    {
        return Err(invalid(
            "provider authorization storage permissions are unsafe",
        ));
    }
    serde_json::from_slice(&std::fs::read(path)?)
        .map(Some)
        .map_err(|_| invalid("provider authorization cannot be read"))
}
fn write_private<T: Serialize>(path: &Path, value: &T) -> Result<(), SourceError> {
    let directory = path
        .parent()
        .ok_or_else(|| invalid("provider storage path is invalid"))?;
    let root = directory
        .parent()
        .ok_or_else(|| invalid("provider storage path is invalid"))?;
    super::super::private_directory(root)?;
    super::super::private_directory(directory)?;
    let value = serde_json::to_vec(value)
        .map_err(|_| invalid("provider authorization cannot be recorded"))?;
    super::replace_private_file(path, &value)
}
fn console_url(value: &str) -> Result<(String, String), SourceError> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| invalid("console URL must be HTTPS or HTTP loopback"))?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if !(url.scheme() == "https" || url.scheme() == "http" && loopback)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/console/"
    {
        return Err(invalid(
            "console URL must use /console/ on HTTPS or HTTP loopback, without credentials, query or fragment",
        ));
    }
    Ok((url.as_str().into(), url.origin().ascii_serialization()))
}
fn callback(console: &str) -> String {
    let mut url = reqwest::Url::parse(console).expect("validated console URL");
    url.set_path("/source/authorization/callback");
    url.to_string()
}
fn safe_value(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn github(root: &Path) -> Result<Option<GithubConfig>, SourceError> {
    read_private(&configuration(root, "github"))
}
fn gitlab(root: &Path) -> Result<Option<GitlabConfig>, SourceError> {
    read_private(&configuration(root, "gitlab"))
}
fn active(root: &Path, authentication: Authentication) -> Result<bool, SourceError> {
    Ok(super::list_connections(root)?
        .iter()
        .any(|connection| connection.authentication == authentication))
}

pub fn integrations(root: &Path) -> Result<IntegrationSettings, SourceError> {
    let gh = github(root)?;
    let gl = gitlab(root)?;
    Ok(IntegrationSettings {
        github: GithubSettings {
            configured: gh.is_some(),
            app_slug: gh.as_ref().map(|config| config.app_slug.clone()),
            console_url: gh.map(|config| config.console_url),
        },
        gitlab: GitlabSettings {
            configured: gl.is_some(),
            client_id: gl.as_ref().map(|config| config.client_id.clone()),
            console_url: gl.map(|config| config.console_url),
        },
    })
}
pub fn save_gitlab(
    root: &Path,
    request: GitlabIntegrationRequest,
) -> Result<IntegrationSettings, SourceError> {
    let (console, _) = console_url(&request.console_url)?;
    if !safe_value(&request.client_id, 256) || !safe_value(&request.client_secret, 4096) {
        return Err(invalid("GitLab integration needs a client ID and secret"));
    }
    let _records = RECORDS
        .lock()
        .map_err(|_| invalid("Git connection storage is busy"))?;
    if active(root, Authentication::Oauth)? {
        return Err(invalid(
            "disconnect active GitLab OAuth connections before replacing the integration",
        ));
    }
    write_private(
        &configuration(root, "gitlab"),
        &GitlabConfig {
            generation: random(),
            console_url: console,
            client_id: request.client_id,
            client_secret: request.client_secret,
        },
    )?;
    integrations(root)
}
fn insert_session(root: &Path, pending: Pending) -> Result<String, SourceError> {
    super::super::private_directory(root)?;
    let root = root.canonicalize()?;
    let mut sessions = SESSIONS
        .lock()
        .map_err(|_| invalid("provider authorization is busy"))?;
    sessions.retain(|_, pending| pending.deadline > Instant::now());
    if sessions.len() >= MAX_PENDING {
        return Err(invalid(
            "too many pending authorizations; close an earlier popup and retry",
        ));
    }
    let state = random();
    sessions.insert((root, state.clone()), pending);
    Ok(state)
}
fn pending(root: &Path, state: &str, consume: bool) -> Result<Pending, SourceError> {
    if !safe_value(state, 256) {
        return Err(invalid("authorization state is invalid or expired"));
    }
    let root = root
        .canonicalize()
        .map_err(|_| invalid("authorization state is invalid or expired"))?;
    let mut sessions = SESSIONS
        .lock()
        .map_err(|_| invalid("provider authorization is busy"))?;
    let key = (root, state.into());
    let value = if consume {
        sessions.remove(&key)
    } else {
        sessions.get(&key).cloned()
    };
    value
        .filter(|pending| pending.deadline > Instant::now())
        .ok_or_else(|| invalid("authorization state is invalid or expired"))
}
pub fn callback_origin(root: &Path, state: &str) -> Result<String, SourceError> {
    Ok(pending(root, state, false)?.origin)
}
pub fn cancel(root: &Path, state: &str) -> Result<(), SourceError> {
    let _ = pending(root, state, true)?;
    Ok(())
}

pub fn register_github(
    root: &Path,
    request: GithubRegistrationRequest,
) -> Result<AuthorizationStart, SourceError> {
    let (console, origin) = console_url(&request.console_url)?;
    let organization = request
        .organization
        .filter(|organization| !organization.is_empty());
    if organization.as_deref().is_some_and(|value| {
        value.len() > 100
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    }) {
        return Err(invalid("GitHub organization name is invalid"));
    }
    let _records = RECORDS
        .lock()
        .map_err(|_| invalid("Git connection storage is busy"))?;
    if active(root, Authentication::GithubApp)? {
        return Err(invalid(
            "disconnect active GitHub App connections before replacing the integration",
        ));
    }
    let previous = github(root)?.map(|config| config.generation);
    let state = insert_session(
        root,
        Pending {
            deadline: Instant::now() + Duration::from_secs(TTL),
            origin,
            kind: PendingKind::Registration {
                console_url: console.clone(),
                previous,
            },
        },
    )?;
    let action = match organization {
        Some(organization) => format!(
            "https://github.com/organizations/{organization}/settings/apps/new?state={state}"
        ),
        None => format!("https://github.com/settings/apps/new?state={state}"),
    };
    let manifest = serde_json::json!({ "name": "self-host", "url": console, "redirect_url": callback(&console), "callback_urls": [callback(&console)], "public": true, "default_permissions": {"contents":"read", "metadata":"read"}, "default_events": [], "request_oauth_on_install": false, "setup_url": callback(&console) });
    Ok(AuthorizationStart {
        state,
        url: None,
        form: Some(AuthorizationForm {
            action,
            manifest: manifest.to_string(),
        }),
        expires_in: TTL,
    })
}
pub fn start(
    root: &Path,
    request: AuthorizationRequest,
) -> Result<AuthorizationStart, SourceError> {
    if !safe_value(request.name.trim(), 120) {
        return Err(invalid(
            "Git connection needs a name of at most 120 characters",
        ));
    }
    if request.use_existing_installation && request.provider != Provider::Github {
        return Err(invalid(
            "existing installations are only supported by GitHub",
        ));
    }
    let authentication = match request.provider {
        Provider::Github => Authentication::GithubApp,
        Provider::Gitlab => Authentication::Oauth,
    };
    let reconnect = match request.connection_id {
        Some(id) => {
            let _records = RECORDS
                .lock()
                .map_err(|_| invalid("Git connection storage is busy"))?;
            let connection = super::read_connection(root, &id)?;
            if connection.provider != request.provider
                || connection.authentication != authentication
            {
                return Err(invalid(
                    "reconnect using the connection's existing authentication method",
                ));
            }
            Some(Reconnect {
                id,
                generation: connection.authorization_generation,
                credential_id: connection.credential_id,
            })
        }
        None => None,
    };
    let (console, kind, base) = match request.provider {
        Provider::Github => {
            let config = github(root)?
                .ok_or_else(|| invalid("configure the GitHub App before connecting access"))?;
            if request.use_existing_installation {
                (
                    config.console_url.clone(),
                    PendingKind::Github {
                        config: config.generation,
                        name: request.name.trim().into(),
                        verifier: random(),
                        installation: None,
                        reconnect,
                    },
                    "https://github.com/login/oauth/authorize".into(),
                )
            } else {
                (
                    config.console_url.clone(),
                    PendingKind::GithubInstallation {
                        config: config.generation,
                        name: request.name.trim().into(),
                        reconnect,
                    },
                    format!(
                        "https://github.com/apps/{}/installations/new",
                        config.app_slug
                    ),
                )
            }
        }
        Provider::Gitlab => {
            let config = gitlab(root)?
                .ok_or_else(|| invalid("configure GitLab OAuth before connecting access"))?;
            (
                config.console_url.clone(),
                PendingKind::Gitlab {
                    config: config.generation,
                    name: request.name.trim().into(),
                    verifier: random(),
                    reconnect,
                },
                "https://gitlab.com/oauth/authorize".into(),
            )
        }
    };
    let (_, origin) = console_url(&console)?;
    let state = insert_session(
        root,
        Pending {
            deadline: Instant::now() + Duration::from_secs(TTL),
            origin,
            kind: kind.clone(),
        },
    )?;
    let mut url =
        reqwest::Url::parse(&base).map_err(|_| invalid("provider authorization URL is invalid"))?;
    url.query_pairs_mut().append_pair("state", &state);
    if let PendingKind::Github { verifier, .. } = &kind {
        let config =
            github(root)?.ok_or_else(|| invalid("GitHub integration changed; reconnect"))?;
        url.query_pairs_mut()
            .append_pair("client_id", &config.client_id)
            .append_pair("redirect_uri", &callback(&console))
            .append_pair(
                "code_challenge",
                &URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
            )
            .append_pair("code_challenge_method", "S256");
    }
    if let PendingKind::Gitlab { verifier, .. } = kind {
        let config =
            gitlab(root)?.ok_or_else(|| invalid("GitLab integration changed; reconnect"))?;
        url.query_pairs_mut()
            .append_pair("client_id", &config.client_id)
            .append_pair("redirect_uri", &callback(&console))
            .append_pair("response_type", "code")
            .append_pair("scope", &GITLAB_SCOPES.join(" "))
            .append_pair(
                "code_challenge",
                &URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
            )
            .append_pair("code_challenge_method", "S256");
    }
    Ok(AuthorizationStart {
        state,
        url: Some(url.to_string()),
        form: None,
        expires_in: TTL,
    })
}

struct Endpoints {
    github_api: String,
    github_token: String,
    gitlab_api: String,
    gitlab_token: String,
    gitlab_info: String,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            github_api: "https://api.github.com".into(),
            github_token: "https://github.com/login/oauth/access_token".into(),
            gitlab_api: "https://gitlab.com/api/v4".into(),
            gitlab_token: "https://gitlab.com/oauth/token".into(),
            gitlab_info: "https://gitlab.com/oauth/token/info".into(),
        }
    }
}
struct Client {
    http: reqwest::Client,
    endpoints: Endpoints,
}
impl Client {
    fn new() -> Result<Self, SourceError> {
        Ok(Self {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(15))
                .user_agent("self-host")
                .build()
                .map_err(|_| invalid("provider authorization client could not start"))?,
            endpoints: Endpoints::default(),
        })
    }
    fn bearer(
        &self,
        request: reqwest::RequestBuilder,
        token: &str,
    ) -> Result<reqwest::RequestBuilder, SourceError> {
        let mut header = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| invalid("provider credential is invalid"))?;
        header.set_sensitive(true);
        Ok(request.header(reqwest::header::AUTHORIZATION, header))
    }
    async fn send<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, SourceError> {
        self.send_with_status(request, false).await
    }
    async fn send_with_status<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
        missing_is_revoked: bool,
    ) -> Result<T, SourceError> {
        let response = request
            .send()
            .await
            .map_err(|_| invalid("provider authorization could not reach the provider"))?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED
            || missing_is_revoked && response.status() == reqwest::StatusCode::NOT_FOUND
        {
            return Err(invalid(REJECTED));
        }
        if response.status() == reqwest::StatusCode::BAD_REQUEST {
            #[derive(Deserialize)]
            struct ProviderError {
                error: String,
            }
            if let Ok(error) = super::read_json::<ProviderError>(response).await
                && matches!(error.error.as_str(), "invalid_grant" | "invalid_token")
            {
                return Err(invalid(REJECTED));
            }
            return Err(invalid("provider authorization request failed; try again"));
        }
        super::check_status(response.status())?;
        super::read_json(response).await
    }
    fn github(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
    }
}
#[derive(Deserialize)]
struct GithubUserToken {
    access_token: String,
    token_type: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    expires_in: Option<u64>,
}
#[derive(Deserialize)]
struct OAuthToken {
    access_token: String,
    token_type: String,
    refresh_token: String,
    expires_in: u64,
    #[serde(default)]
    scope: Option<String>,
}
#[derive(Deserialize)]
struct TokenInfo {
    resource_owner_id: u64,
    scope: Vec<String>,
    expires_in: Option<u64>,
    application: TokenApplication,
}
#[derive(Deserialize)]
struct TokenApplication {
    uid: String,
}
#[derive(Deserialize)]
struct Identity {
    id: u64,
}
fn validate_token(token: &str, kind: &str) -> Result<(), SourceError> {
    if !safe_value(token, 65536) || !kind.eq_ignore_ascii_case("bearer") {
        return Err(invalid("provider returned an invalid access token"));
    }
    Ok(())
}
fn expiration(seconds: u64) -> Result<i64, SourceError> {
    if !(60..=86400).contains(&seconds) {
        return Err(invalid("provider returned an invalid token expiry"));
    }
    now()
        .checked_add(seconds as i64)
        .ok_or_else(|| invalid("provider returned an invalid token expiry"))
}
fn scopes(scopes: &[String]) -> Result<(), SourceError> {
    if scopes.len() != GITLAB_SCOPES.len()
        || !GITLAB_SCOPES
            .iter()
            .all(|required| scopes.iter().any(|scope| scope == required))
    {
        return Err(invalid(
            "GitLab authorization needs only read_user, read_api and read_repository access",
        ));
    }
    Ok(())
}
async fn gitlab_grant(
    client: &Client,
    config: &GitlabConfig,
    fields: &[(&str, String)],
) -> Result<Grant, SourceError> {
    let mut form = vec![
        ("client_id", config.client_id.clone()),
        ("client_secret", config.client_secret.clone()),
        ("redirect_uri", callback(&config.console_url)),
    ];
    form.extend_from_slice(fields);
    let token: OAuthToken = client
        .send(client.http.post(&client.endpoints.gitlab_token).form(&form))
        .await?;
    validate_token(&token.access_token, &token.token_type)?;
    if !safe_value(&token.refresh_token, 65536) {
        return Err(invalid("GitLab returned an invalid refresh token"));
    }
    if let Some(scope) = &token.scope {
        scopes(
            &scope
                .split_whitespace()
                .map(String::from)
                .collect::<Vec<_>>(),
        )?;
    }
    let expires_at = expiration(token.expires_in)?;
    let info: TokenInfo = client
        .send(client.bearer(
            client.http.get(&client.endpoints.gitlab_info),
            &token.access_token,
        )?)
        .await?;
    scopes(&info.scope)?;
    if info.application.uid != config.client_id
        || info.resource_owner_id == 0
        || info
            .expires_in
            .is_none_or(|expiry| expiry == 0 || expiry > token.expires_in + 60)
    {
        return Err(invalid(
            "GitLab token does not match this integration or expiry",
        ));
    }
    let identity: Identity = client
        .send(
            client.bearer(
                client
                    .http
                    .get(format!("{}/user", client.endpoints.gitlab_api)),
                &token.access_token,
            )?,
        )
        .await?;
    if identity.id != info.resource_owner_id {
        return Err(invalid("GitLab authorization account did not match"));
    }
    Ok(Grant::Gitlab {
        config: config.generation.clone(),
        account: identity.id,
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_at,
    })
}

fn check_reconnect(
    root: &Path,
    reconnect: Option<&Reconnect>,
    provider: Provider,
    authentication: Authentication,
) -> Result<(), SourceError> {
    let Some(reconnect) = reconnect else {
        return Ok(());
    };
    let _records = RECORDS
        .lock()
        .map_err(|_| invalid("Git connection storage is busy"))?;
    let current = super::read_connection(root, &reconnect.id)
        .map_err(|_| invalid("Git connection changed; start authorization again"))?;
    if current.authorization_generation != reconnect.generation
        || current.credential_id != reconnect.credential_id
        || current.provider != provider
        || current.authentication != authentication
    {
        return Err(invalid("Git connection changed; start authorization again"));
    }
    Ok(())
}

fn jwt(config: &GithubConfig) -> Result<String, SourceError> {
    let key = pem::parse(&config.private_key)
        .map_err(|_| invalid("GitHub App signing key is invalid"))?;
    let pair = match key.tag() {
        "RSA PRIVATE KEY" => ring::signature::RsaKeyPair::from_der(key.contents()),
        "PRIVATE KEY" => ring::signature::RsaKeyPair::from_pkcs8(key.contents()),
        _ => return Err(invalid("GitHub App signing key is invalid")),
    }
    .map_err(|_| invalid("GitHub App signing key is invalid"))?;
    let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
    let claims = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(
            &serde_json::json!({"iat": now() - 60, "exp": now() + 540, "iss": config.client_id}),
        )
        .map_err(|_| invalid("GitHub App signing failed"))?,
    );
    let input = format!("{header}.{claims}");
    let mut signature = vec![0; pair.public().modulus_len()];
    pair.sign(
        &ring::signature::RSA_PKCS1_SHA256,
        &ring::rand::SystemRandom::new(),
        input.as_bytes(),
        &mut signature,
    )
    .map_err(|_| invalid("GitHub App signing failed"))?;
    Ok(format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature)))
}
#[derive(Deserialize)]
struct InstallationToken {
    token: String,
    expires_at: String,
    permissions: BTreeMap<String, String>,
}
async fn mint_github(
    client: &Client,
    config: &GithubConfig,
    installation: u64,
    account: u64,
) -> Result<Grant, SourceError> {
    let token: InstallationToken = client.send_with_status(client.github(client.bearer(client.http.post(format!("{}/app/installations/{installation}/access_tokens", client.endpoints.github_api)).json(&serde_json::json!({"permissions":{"contents":"read","metadata":"read"}})), &jwt(config)?)?), true).await?;
    if !safe_value(&token.token, 65536)
        || token.permissions.len() != 2
        || token.permissions.get("contents").map(String::as_str) != Some("read")
        || token.permissions.get("metadata").map(String::as_str) != Some("read")
    {
        return Err(invalid(
            "GitHub installation token needs only Contents and metadata read access",
        ));
    }
    let expires_at = time::OffsetDateTime::parse(
        &token.expires_at,
        &time::format_description::well_known::Rfc3339,
    )
    .map_err(|_| invalid("GitHub installation token expiry is invalid"))?
    .unix_timestamp();
    if expires_at <= now() + RENEW_BEFORE || expires_at > now() + 7200 {
        return Err(invalid("GitHub installation token expiry is invalid"));
    }
    Ok(Grant::Github {
        config: config.generation.clone(),
        installation,
        account,
        access_token: token.token,
        expires_at,
    })
}
#[derive(Deserialize)]
struct Installation {
    id: u64,
    app_id: u64,
}
#[derive(Deserialize)]
struct Installations {
    total_count: u64,
    installations: Vec<Installation>,
}
async fn verified_installation(
    client: &Client,
    config: &GithubConfig,
    code: &str,
    verifier: &str,
    requested: Option<u64>,
    reconnect: Option<&Grant>,
) -> Result<(u64, u64), SourceError> {
    let token: GithubUserToken = client
        .send(
            client
                .http
                .post(&client.endpoints.github_token)
                .header(reqwest::header::ACCEPT, "application/json")
                .form(&[
                    ("client_id", config.client_id.clone()),
                    ("client_secret", config.client_secret.clone()),
                    ("code", code.into()),
                    ("redirect_uri", callback(&config.console_url)),
                    ("code_verifier", verifier.into()),
                ]),
        )
        .await?;
    validate_token(&token.access_token, &token.token_type)?;
    if !token.scope.is_empty()
        || token
            .expires_in
            .is_some_and(|seconds| seconds == 0 || seconds > 86400)
    {
        return Err(invalid("GitHub returned an invalid App user authorization"));
    }
    let identity: Identity = client
        .send(
            client.github(
                client.bearer(
                    client
                        .http
                        .get(format!("{}/user", client.endpoints.github_api)),
                    &token.access_token,
                )?,
            ),
        )
        .await?;
    if identity.id == 0 {
        return Err(invalid("GitHub authorization account is invalid"));
    }
    let mut allowed = Vec::new();
    for page in 1..=20 {
        let response: Installations = client
            .send(
                client.github(
                    client.bearer(
                        client
                            .http
                            .get(format!(
                                "{}/user/installations",
                                client.endpoints.github_api
                            ))
                            .query(&[("per_page", "100".into()), ("page", page.to_string())]),
                        &token.access_token,
                    )?,
                ),
            )
            .await?;
        if response.installations.len() > 100 || response.total_count > 2000 {
            return Err(invalid("GitHub returned too many installations"));
        }
        allowed.extend(
            response
                .installations
                .into_iter()
                .filter(|installation| installation.app_id == config.app_id)
                .map(|installation| installation.id),
        );
        if page * 100 >= response.total_count {
            break;
        }
    }
    allowed.sort();
    allowed.dedup();
    let selected = requested
        .or(match reconnect {
            Some(Grant::Github { installation, .. }) => Some(*installation),
            _ => None,
        })
        .or_else(|| {
            if allowed.len() == 1 {
                Some(allowed[0])
            } else {
                None
            }
        });
    let selected = selected.filter(|id| *id != 0 && allowed.contains(id)).ok_or_else(|| invalid("GitHub installation is not authorized for this user and App, or more than one installation needs selection"))?;
    Ok((selected, identity.id))
}

pub async fn complete(
    root: &Path,
    request: AuthorizationCompletion,
) -> Result<AuthorizationResult, SourceError> {
    complete_with(root, request, &Client::new()?).await
}
async fn complete_with(
    root: &Path,
    request: AuthorizationCompletion,
    client: &Client,
) -> Result<AuthorizationResult, SourceError> {
    let pending = pending(root, &request.state, true)?;
    if request.error.is_some() {
        return Err(invalid(
            "provider authorization was denied; reconnect to try again",
        ));
    }
    if let PendingKind::GithubInstallation {
        config: generation,
        name,
        reconnect,
    } = &pending.kind
    {
        check_reconnect(
            root,
            reconnect.as_ref(),
            Provider::Github,
            Authentication::GithubApp,
        )?;
        let config = github(root)?
            .filter(|config| &config.generation == generation)
            .ok_or_else(|| invalid("GitHub integration changed; start authorization again"))?;
        let installation = request
            .installation_id
            .filter(|id| *id != 0)
            .ok_or_else(|| invalid("GitHub installation callback needs an installation ID"))?;
        let verifier = random();
        let state = insert_session(
            root,
            Pending {
                deadline: Instant::now() + Duration::from_secs(TTL),
                origin: pending.origin.clone(),
                kind: PendingKind::Github {
                    config: generation.clone(),
                    name: name.clone(),
                    verifier: verifier.clone(),
                    installation: Some(installation),
                    reconnect: reconnect.clone(),
                },
            },
        )?;
        let mut url = reqwest::Url::parse("https://github.com/login/oauth/authorize")
            .expect("fixed provider URL");
        url.query_pairs_mut()
            .append_pair("client_id", &config.client_id)
            .append_pair("redirect_uri", &callback(&config.console_url))
            .append_pair("state", &state)
            .append_pair(
                "code_challenge",
                &URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
            )
            .append_pair("code_challenge_method", "S256");
        return Ok(AuthorizationResult {
            connection: None,
            integrations: None,
            authorization: Some(AuthorizationStart {
                state,
                url: Some(url.to_string()),
                form: None,
                expires_in: TTL,
            }),
        });
    }
    let code = request
        .code
        .filter(|code| safe_value(code, 4096))
        .ok_or_else(|| invalid("provider authorization did not return a valid code"))?;
    match pending.kind {
        PendingKind::Registration {
            console_url,
            previous,
        } => {
            if !code
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return Err(invalid("GitHub manifest code is invalid"));
            }
            #[derive(Deserialize)]
            struct Manifest {
                id: u64,
                slug: String,
                client_id: String,
                client_secret: String,
                pem: String,
                permissions: BTreeMap<String, String>,
            }
            let manifest: Manifest = client
                .send(client.github(client.http.post(format!(
                    "{}/app-manifests/{code}/conversions",
                    client.endpoints.github_api
                ))))
                .await?;
            if manifest.id == 0
                || !safe_value(&manifest.client_id, 256)
                || !safe_value(&manifest.client_secret, 4096)
                || manifest.slug.is_empty()
                || manifest.slug.len() > 120
                || !manifest
                    .slug
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                || manifest.permissions.len() != 2
                || manifest.permissions.get("contents").map(String::as_str) != Some("read")
                || manifest.permissions.get("metadata").map(String::as_str) != Some("read")
            {
                return Err(invalid(
                    "GitHub manifest returned an invalid App or permissions",
                ));
            }
            let config = GithubConfig {
                generation: random(),
                console_url,
                app_id: manifest.id,
                app_slug: manifest.slug,
                client_id: manifest.client_id,
                client_secret: manifest.client_secret,
                private_key: manifest.pem,
            };
            jwt(&config)?;
            let _records = RECORDS
                .lock()
                .map_err(|_| invalid("Git connection storage is busy"))?;
            if active(root, Authentication::GithubApp)?
                || github(root)?.map(|config| config.generation) != previous
            {
                return Err(invalid(
                    "GitHub integration changed; close the popup and start again",
                ));
            }
            write_private(&configuration(root, "github"), &config)?;
            Ok(AuthorizationResult {
                connection: None,
                integrations: Some(integrations(root)?),
                authorization: None,
            })
        }
        PendingKind::Github {
            config: generation,
            name,
            verifier,
            installation,
            reconnect,
        } => {
            check_reconnect(
                root,
                reconnect.as_ref(),
                Provider::Github,
                Authentication::GithubApp,
            )?;
            let config = github(root)?
                .filter(|config| config.generation == generation)
                .ok_or_else(|| invalid("GitHub integration changed; start authorization again"))?;
            let old = reconnect
                .as_ref()
                .map(|connection| read_private::<Grant>(&grant_path(root, &connection.id)?))
                .transpose()?
                .flatten();
            let (installation, account) = verified_installation(
                client,
                &config,
                &code,
                &verifier,
                installation,
                old.as_ref(),
            )
            .await?;
            let grant = mint_github(client, &config, installation, account).await?;
            let connection = publish(
                root,
                Provider::Github,
                Authentication::GithubApp,
                name,
                reconnect,
                grant,
            )?;
            Ok(AuthorizationResult {
                connection: Some(connection),
                integrations: None,
                authorization: None,
            })
        }
        PendingKind::GithubInstallation { .. } => {
            unreachable!("installation callback returned its OAuth continuation")
        }
        PendingKind::Gitlab {
            config: generation,
            name,
            verifier,
            reconnect,
        } => {
            check_reconnect(
                root,
                reconnect.as_ref(),
                Provider::Gitlab,
                Authentication::Oauth,
            )?;
            let config = gitlab(root)?
                .filter(|config| config.generation == generation)
                .ok_or_else(|| invalid("GitLab integration changed; start authorization again"))?;
            let grant = gitlab_grant(
                client,
                &config,
                &[
                    ("grant_type", "authorization_code".into()),
                    ("code", code),
                    ("code_verifier", verifier),
                ],
            )
            .await?;
            let connection = publish(
                root,
                Provider::Gitlab,
                Authentication::Oauth,
                name,
                reconnect,
                grant,
            )?;
            Ok(AuthorizationResult {
                connection: Some(connection),
                integrations: None,
                authorization: None,
            })
        }
    }
}
fn publish(
    root: &Path,
    provider: Provider,
    authentication: Authentication,
    name: String,
    reconnect: Option<Reconnect>,
    grant: Grant,
) -> Result<Connection, SourceError> {
    let _records = RECORDS
        .lock()
        .map_err(|_| invalid("Git connection storage is busy"))?;
    let generation = match &grant {
        Grant::Github { .. } => github(root)?.map(|config| config.generation),
        Grant::Gitlab { .. } => gitlab(root)?.map(|config| config.generation),
    };
    let expected = match &grant {
        Grant::Github { config, .. } | Grant::Gitlab { config, .. } => config,
    };
    if generation.as_ref() != Some(expected) {
        return Err(invalid("provider integration changed; authorize again"));
    }
    let mut connection = if let Some(reconnect) = reconnect {
        let current = super::read_connection(root, &reconnect.id)?;
        if current.authorization_generation != reconnect.generation
            || current.credential_id != reconnect.credential_id
            || current.provider != provider
            || current.authentication != authentication
        {
            return Err(invalid("Git connection changed; authorize again"));
        }
        let previous: Grant = read_private(&grant_path(root, &current.id)?)?
            .ok_or_else(|| invalid("Git authorization is missing; disconnect and reconnect"))?;
        if !previous.same_identity(&grant) {
            return Err(invalid(
                "reconnect with the same authorized account and installation",
            ));
        }
        current
    } else {
        Connection {
            id: format!("git-{:032x}", rand::random::<u128>()),
            name: name.clone(),
            provider,
            credential_id: format!("cred-{:032x}", rand::random::<u128>()),
            status: ConnectionStatus::Connected,
            authentication,
            generation: rand::random(),
            authorization_generation: rand::random(),
        }
    };
    let credential = Credential {
        metadata: CredentialMetadata {
            id: connection.credential_id.clone(),
            kind: CredentialKind::Git,
        },
        username: Some(
            match provider {
                Provider::Github => "x-access-token",
                Provider::Gitlab => "oauth2",
            }
            .into(),
        ),
        value: String::new(),
        server: Some(super::provider_host(provider).into()),
    };
    super::super::private_directory(root)?;
    super::super::private_directory(&root.join("credentials"))?;
    let new_connection = !super::record_path(root, &connection.id)?.exists();
    if new_connection {
        super::super::private_file(
            &root.join("credentials").join(&connection.credential_id),
            &serde_json::to_vec(&credential)
                .map_err(|_| invalid("credential cannot be recorded"))?,
        )?;
    }
    if let Err(error) = write_private(&grant_path(root, &connection.id)?, &grant) {
        if new_connection {
            let _ = std::fs::remove_file(root.join("credentials").join(&connection.credential_id));
        }
        return Err(error);
    }
    connection.name = name;
    connection.status = ConnectionStatus::Connected;
    connection.generation = rand::random();
    connection.authorization_generation = rand::random();
    if let Err(error) = super::write_connection(root, &connection) {
        if new_connection {
            let _ = std::fs::remove_file(grant_path(root, &connection.id)?);
            let _ = std::fs::remove_file(root.join("credentials").join(&connection.credential_id));
        }
        return Err(error);
    }
    Ok(connection)
}
pub(super) fn remove_grant(root: &Path, id: &str) -> Result<(), SourceError> {
    match std::fs::remove_file(grant_path(root, id)?) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(in crate::source) async fn resolve_credential(
    root: &Path,
    credential_id: &str,
) -> Result<Credential, SourceError> {
    resolve_with(root, credential_id, &Client::new()?).await
}
async fn resolve_with(
    root: &Path,
    credential_id: &str,
    client: &Client,
) -> Result<Credential, SourceError> {
    if !super::super::valid_id(credential_id) {
        return Err(invalid("Git credential id is invalid"));
    }
    let root = root.canonicalize()?;
    let key = (root.clone(), credential_id.to_owned());
    let renewal = {
        let mut renewals = RENEWALS
            .lock()
            .map_err(|_| invalid("provider token renewal is busy"))?;
        renewals.retain(|_, lock| Arc::strong_count(lock) > 1);
        renewals.entry(key).or_default().clone()
    };
    let _renewal = renewal.lock().await;
    let (connection, mut credential, mut grant) = {
        let _records = RECORDS
            .lock()
            .map_err(|_| invalid("Git connection storage is busy"))?;
        let credential = super::super::load_credential(&root, credential_id)?;
        let connection = super::list_connections(&root)?
            .into_iter()
            .find(|connection| connection.credential_id == credential_id);
        let Some(connection) =
            connection.filter(|connection| connection.authentication != Authentication::Token)
        else {
            return Ok(credential);
        };
        let grant: Grant = read_private(&grant_path(&root, &connection.id)?)?
            .ok_or_else(|| invalid("provider authorization is missing; reconnect access"))?;
        (connection, credential, grant)
    };
    if !matches!(credential.metadata.kind, CredentialKind::Git)
        || credential.server.as_deref() != Some(super::provider_host(connection.provider))
        || !safe_value(grant.token(), 65536)
    {
        return Err(invalid("provider credential is invalid"));
    }
    match &grant {
        Grant::Github {
            config,
            installation,
            account,
            ..
        } if connection.provider == Provider::Github
            && connection.authentication == Authentication::GithubApp
            && *installation != 0
            && *account != 0 =>
        {
            if github(&root)?.is_none_or(|current| current.generation != *config) {
                return Err(invalid("GitHub integration changed; reconnect access"));
            }
        }
        Grant::Gitlab {
            config,
            account,
            refresh_token,
            ..
        } if connection.provider == Provider::Gitlab
            && connection.authentication == Authentication::Oauth
            && *account != 0
            && safe_value(refresh_token, 65536) =>
        {
            if gitlab(&root)?.is_none_or(|current| current.generation != *config) {
                return Err(invalid("GitLab integration changed; reconnect access"));
            }
        }
        _ => {
            return Err(invalid(
                "provider authorization does not match this connection",
            ));
        }
    }
    if grant.expires_at() <= now() + RENEW_BEFORE {
        let renewed = match &grant {
            Grant::Github {
                config: generation,
                installation,
                account,
                ..
            } => {
                let config = github(&root)?
                    .filter(|config| &config.generation == generation)
                    .ok_or_else(|| invalid("GitHub integration changed; reconnect access"))?;
                mint_github(client, &config, *installation, *account).await
            }
            Grant::Gitlab {
                config: generation,
                refresh_token,
                ..
            } => {
                let config = gitlab(&root)?
                    .filter(|config| &config.generation == generation)
                    .ok_or_else(|| invalid("GitLab integration changed; reconnect access"))?;
                gitlab_grant(
                    client,
                    &config,
                    &[
                        ("grant_type", "refresh_token".into()),
                        ("refresh_token", refresh_token.clone()),
                    ],
                )
                .await
            }
        };
        let renewed = match renewed {
            Ok(renewed) => renewed,
            Err(error) => {
                if is_rejected(&error) {
                    let _ = super::set_status(
                        &root,
                        &connection.id,
                        ConnectionStatus::Expired,
                        connection.generation,
                    );
                }
                return Err(error);
            }
        };
        if !grant.same_identity(&renewed) {
            return Err(invalid(
                "provider token renewal changed its authorized account",
            ));
        }
        let _records = RECORDS
            .lock()
            .map_err(|_| invalid("Git connection storage is busy"))?;
        let current = super::read_connection(&root, &connection.id).map_err(|_| {
            invalid("Git connection changed during token renewal; reconnect access")
        })?;
        if current.authorization_generation != connection.authorization_generation {
            return Err(invalid(
                "Git connection changed during token renewal; retry",
            ));
        }
        write_private(&grant_path(&root, &connection.id)?, &renewed)?;
        grant = renewed;
    }
    // A disconnect or reconnect after the network response must not publish or
    // return a token from the obsolete authorization snapshot.
    {
        let _records = RECORDS
            .lock()
            .map_err(|_| invalid("Git connection storage is busy"))?;
        let current = super::read_connection(&root, &connection.id)
            .map_err(|_| invalid("Git connection changed; reconnect access"))?;
        if current.authorization_generation != connection.authorization_generation {
            return Err(invalid("Git connection changed; retry"));
        }
    }
    credential.value = grant.token().into();
    Ok(credential)
}

#[cfg(test)]
mod tests;
