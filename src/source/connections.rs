//! Saved provider access uses the same private credentials as Git checkouts.

pub mod auth;

use super::{Credential, CredentialKind, CredentialMetadata, SourceError, invalid};
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

static RECORDS: Mutex<()> = Mutex::new(());
const PAGE_SIZE: u32 = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Github,
    Gitlab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionStatus {
    Connected,
    Expired,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Authentication {
    #[default]
    Token,
    GithubApp,
    Oauth,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,
    pub name: String,
    pub provider: Provider,
    pub credential_id: String,
    pub status: ConnectionStatus,
    #[serde(default)]
    pub authentication: Authentication,
    #[serde(skip)]
    generation: u64,
    #[serde(skip)]
    authorization_generation: u64,
}

#[derive(Serialize, Deserialize)]
struct StoredConnection {
    #[serde(flatten)]
    connection: Connection,
    #[serde(default)]
    generation: u64,
    #[serde(default)]
    authorization_generation: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionRequest {
    pub name: String,
    pub provider: Provider,
    #[serde(default)]
    pub username: Option<String>,
    pub token: String,
}

#[derive(Debug, Serialize)]
pub struct Repository {
    pub full_name: String,
    pub clone_url: String,
    pub default_branch: Option<String>,
    pub private: bool,
}

#[derive(Debug, Serialize)]
pub struct RepositoryPage {
    pub repositories: Vec<Repository>,
    pub next_page: Option<u32>,
}

fn record_path(root: &Path, id: &str) -> Result<std::path::PathBuf, SourceError> {
    if !super::valid_id(id) {
        return Err(invalid("Git connection id is invalid"));
    }
    Ok(root.join("connections").join(id))
}

fn read_connection(root: &Path, id: &str) -> Result<Connection, SourceError> {
    use std::os::unix::fs::PermissionsExt;
    let path = record_path(root, id)?;
    let metadata = std::fs::symlink_metadata(&path)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(invalid("Git connection storage permissions are unsafe"));
    }
    let mut record: StoredConnection = serde_json::from_slice(&std::fs::read(path)?)
        .map_err(|_| invalid("Git connection cannot be read"))?;
    if record.connection.id != id || !super::valid_id(&record.connection.credential_id) {
        return Err(invalid("Git connection cannot be read"));
    }
    record.connection.generation = record.generation;
    record.connection.authorization_generation = record.authorization_generation;
    Ok(record.connection)
}

fn write_connection(root: &Path, connection: &Connection) -> Result<(), SourceError> {
    super::private_directory(root)?;
    super::private_directory(&root.join("connections"))?;
    let contents = serde_json::to_vec(&StoredConnection {
        connection: connection.clone(),
        generation: connection.generation,
        authorization_generation: connection.authorization_generation,
    })
    .map_err(|_| invalid("Git connection cannot be recorded"))?;
    replace_private_file(&record_path(root, &connection.id)?, &contents)
}

fn replace_private_file(path: &Path, contents: &[u8]) -> Result<(), SourceError> {
    // Keep the temporary file outside record directories so concurrent lists
    // never read a half-published credential or count the replacement twice.
    let temporary = path
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| invalid("private storage path is invalid"))?
        .join(format!("pending-{:032x}", rand::random::<u128>()));
    super::private_file(&temporary, contents)?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(temporary);
        return Err(error.into());
    }
    Ok(())
}

pub fn list_connections(root: &Path) -> Result<Vec<Connection>, SourceError> {
    let directory = root.join("connections");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let metadata = std::fs::symlink_metadata(&directory)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid("Git connection storage must be a directory"));
    }
    let mut result = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let id = entry?.file_name().to_string_lossy().into_owned();
        if id.starts_with("pending-") {
            continue;
        }
        result.push(read_connection(root, &id)?);
    }
    result.sort_by_key(|connection| connection.name.to_lowercase());
    Ok(result)
}

/// Provider access cannot be reused as a credential for an arbitrary Git host.
pub fn validate_credential_repository(
    root: &Path,
    credential_id: &str,
    repository: &str,
) -> Result<(), SourceError> {
    let credential = super::load_credential(root, credential_id)?;
    validate_bound_repository(credential.server.as_deref(), repository)
}

/// Check the credential snapshot that Git will receive, even during disconnect.
pub fn validate_bound_repository(
    server: Option<&str>,
    repository: &str,
) -> Result<(), SourceError> {
    let Some(host) = server else {
        return Ok(());
    };
    if !matches!(host, "github.com" | "gitlab.com") {
        return Err(invalid("Git connection provider is invalid"));
    }
    let url =
        reqwest::Url::parse(repository).map_err(|_| invalid("Git repository URL is invalid"))?;
    if url.scheme() != "https"
        || url.host_str() != Some(host)
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(invalid(
            "Git connection can only access repositories on its provider",
        ));
    }
    Ok(())
}

fn validate_request(request: &ConnectionRequest) -> Result<(), SourceError> {
    if request.name.trim().is_empty()
        || request.name.len() > 120
        || request.name.chars().any(char::is_control)
    {
        return Err(invalid(
            "Git connection needs a name of at most 120 characters",
        ));
    }
    if request.token.is_empty()
        || request.token.len() > 65536
        || request.token.chars().any(char::is_control)
    {
        return Err(invalid("Git connection needs a single-line access token"));
    }
    if request.username.as_deref().is_some_and(|username| {
        username.is_empty() || username.len() > 256 || username.chars().any(char::is_control)
    }) {
        return Err(invalid("Git username is invalid"));
    }
    Ok(())
}

struct ProviderClient {
    client: Client,
    github_base: String,
    gitlab_base: String,
}

impl ProviderClient {
    fn new() -> Result<Self, SourceError> {
        Ok(Self {
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(15))
                .user_agent("self-host")
                .build()
                .map_err(|_| invalid("Git provider client could not start"))?,
            github_base: "https://api.github.com".into(),
            gitlab_base: "https://gitlab.com/api/v4".into(),
        })
    }

    async fn get(
        &self,
        provider: Provider,
        path: &str,
        token: &str,
        query: &[(&str, String)],
    ) -> Result<Response, SourceError> {
        self.get_authenticated(provider, Authentication::Token, path, token, query)
            .await
    }

    async fn get_authenticated(
        &self,
        provider: Provider,
        authentication: Authentication,
        path: &str,
        token: &str,
        query: &[(&str, String)],
    ) -> Result<Response, SourceError> {
        let base = match provider {
            Provider::Github => &self.github_base,
            Provider::Gitlab => &self.gitlab_base,
        };
        let request = self.client.get(format!("{base}{path}")).query(query);
        let mut sensitive_token = reqwest::header::HeaderValue::from_str(token)
            .map_err(|_| invalid("Git access token is invalid"))?;
        sensitive_token.set_sensitive(true);
        let request = match provider {
            Provider::Github => {
                let mut authorization =
                    reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
                        .map_err(|_| invalid("Git access token is invalid"))?;
                authorization.set_sensitive(true);
                request
                    .header(reqwest::header::AUTHORIZATION, authorization)
                    .header(reqwest::header::ACCEPT, "application/vnd.github+json")
                    .header("X-GitHub-Api-Version", "2022-11-28")
            }
            Provider::Gitlab if authentication == Authentication::Token => {
                request.header("PRIVATE-TOKEN", sensitive_token)
            }
            Provider::Gitlab => {
                let mut authorization =
                    reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
                        .map_err(|_| invalid("Git access token is invalid"))?;
                authorization.set_sensitive(true);
                request.header(reqwest::header::AUTHORIZATION, authorization)
            }
        };
        request
            .send()
            .await
            .map_err(|_| invalid("Git provider could not be reached. Try again."))
    }

    async fn username(&self, request: &ConnectionRequest) -> Result<String, SourceError> {
        let response = self
            .get(request.provider, "/user", &request.token, &[])
            .await?;
        check_status(response.status())?;
        #[derive(Deserialize)]
        struct Identity {
            login: Option<String>,
            username: Option<String>,
        }
        let identity: Identity = read_json(response).await?;
        let username = request.username.clone().or(match request.provider {
            Provider::Github => identity.login,
            Provider::Gitlab => identity.username,
        });
        username
            .filter(|name| !name.is_empty() && !name.chars().any(char::is_control))
            .ok_or_else(|| invalid("Git provider did not return an account name"))
    }
}

fn check_status(status: StatusCode) -> Result<(), SourceError> {
    if status.is_success() {
        return Ok(());
    }
    Err(invalid(match status {
        StatusCode::UNAUTHORIZED => "Git access token was rejected. Reconnect with a valid token.",
        StatusCode::FORBIDDEN => {
            "Git provider denied access. Check token permissions and rate limits."
        }
        StatusCode::TOO_MANY_REQUESTS => "Git provider rate limit reached. Try again later.",
        _ => "Git provider request failed. Try again.",
    }))
}

async fn read_json<T: serde::de::DeserializeOwned>(
    mut response: Response,
) -> Result<T, SourceError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| invalid("Git provider response could not be read"))?
    {
        if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
            return Err(invalid("Git provider response is too large"));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid("Git provider returned an invalid response"))
}

pub async fn save_connection(
    root: &Path,
    request: ConnectionRequest,
) -> Result<Connection, SourceError> {
    save_with(root, request, &ProviderClient::new()?).await
}

async fn save_with(
    root: &Path,
    request: ConnectionRequest,
    client: &ProviderClient,
) -> Result<Connection, SourceError> {
    validate_request(&request)?;
    let username = client.username(&request).await?;
    let _guard = RECORDS
        .lock()
        .map_err(|_| invalid("Git connection storage is busy"))?;
    super::private_directory(root)?;
    super::private_directory(&root.join("credentials"))?;
    let credential = Credential {
        metadata: CredentialMetadata {
            id: format!("cred-{:032x}", rand::random::<u128>()),
            kind: CredentialKind::Git,
        },
        username: Some(username),
        value: request.token,
        server: Some(provider_host(request.provider).into()),
    };
    super::private_file(
        &root.join("credentials").join(&credential.metadata.id),
        &serde_json::to_vec(&credential).map_err(|_| invalid("credential cannot be recorded"))?,
    )?;
    let connection = Connection {
        id: format!("git-{:032x}", rand::random::<u128>()),
        name: request.name.trim().into(),
        provider: request.provider,
        credential_id: credential.metadata.id,
        status: ConnectionStatus::Connected,
        authentication: Authentication::Token,
        generation: rand::random(),
        authorization_generation: rand::random(),
    };
    if let Err(error) = write_connection(root, &connection) {
        let _ = std::fs::remove_file(root.join("credentials").join(&connection.credential_id));
        return Err(error);
    }
    Ok(connection)
}

pub async fn reconnect_connection(
    root: &Path,
    id: &str,
    request: ConnectionRequest,
) -> Result<Connection, SourceError> {
    reconnect_with(root, id, request, &ProviderClient::new()?).await
}

async fn reconnect_with(
    root: &Path,
    id: &str,
    request: ConnectionRequest,
    client: &ProviderClient,
) -> Result<Connection, SourceError> {
    validate_request(&request)?;
    let previous = read_connection(root, id)?;
    if previous.provider != request.provider {
        return Err(invalid("reconnect with the same Git provider"));
    }
    if previous.authentication != Authentication::Token {
        return Err(invalid(
            "reconnect this access through its provider authorization",
        ));
    }
    let username = client.username(&request).await?;
    let _guard = RECORDS
        .lock()
        .map_err(|_| invalid("Git connection storage is busy"))?;
    let mut connection = read_connection(root, id)?;
    let old_credential = super::load_credential(root, &connection.credential_id)?;
    if !matches!(old_credential.metadata.kind, CredentialKind::Git)
        || old_credential.server.as_deref() != Some(provider_host(connection.provider))
    {
        return Err(invalid("Git connection credential is invalid"));
    }
    let credential = Credential {
        metadata: old_credential.metadata,
        username: Some(username),
        value: request.token,
        server: old_credential.server,
    };
    replace_private_file(
        &root.join("credentials").join(&connection.credential_id),
        &serde_json::to_vec(&credential).map_err(|_| invalid("credential cannot be recorded"))?,
    )?;
    connection.name = request.name.trim().into();
    connection.status = ConnectionStatus::Connected;
    connection.generation = rand::random();
    connection.authorization_generation = rand::random();
    write_connection(root, &connection)?;
    Ok(connection)
}

pub fn delete_connection(root: &Path, id: &str) -> Result<(), SourceError> {
    let _guard = RECORDS
        .lock()
        .map_err(|_| invalid("Git connection storage is busy"))?;
    let connection = read_connection(root, id)?;
    std::fs::remove_file(record_path(root, id)?)?;
    // Application records remain intact. A future private fetch needs new access.
    std::fs::remove_file(root.join("credentials").join(connection.credential_id))?;
    auth::remove_grant(root, id)?;
    Ok(())
}

pub async fn list_repositories(
    root: &Path,
    id: &str,
    page: u32,
) -> Result<RepositoryPage, SourceError> {
    repositories_with(root, id, page, &ProviderClient::new()?).await
}

async fn repositories_with(
    root: &Path,
    id: &str,
    page: u32,
    client: &ProviderClient,
) -> Result<RepositoryPage, SourceError> {
    if page == 0 || page > 10000 {
        return Err(invalid("repository page must be between 1 and 10000"));
    }
    let selected = read_connection(root, id)?;
    let resolved = auth::resolve_credential(root, &selected.credential_id).await?;
    let (connection, mut credential) = repository_snapshot(root, id)?;
    if selected.authorization_generation != connection.authorization_generation {
        return Err(invalid("Git connection changed; reload its repositories"));
    }
    credential.value = resolved.value;
    credential.username = resolved.username;
    let common = [
        ("per_page", PAGE_SIZE.to_string()),
        ("page", page.to_string()),
    ];
    let (path, extras) = match connection.provider {
        Provider::Github => (
            if connection.authentication == Authentication::GithubApp {
                "/installation/repositories"
            } else {
                "/user/repos"
            },
            if connection.authentication == Authentication::GithubApp {
                vec![]
            } else {
                vec![("sort", "updated"), ("direction", "desc")]
            },
        ),
        Provider::Gitlab => (
            "/projects",
            vec![
                ("membership", "true"),
                ("order_by", "last_activity_at"),
                ("sort", "desc"),
            ],
        ),
    };
    let mut query = common.to_vec();
    query.extend(
        extras
            .into_iter()
            .map(|(key, value)| (key, value.to_string())),
    );
    let response = client
        .get_authenticated(
            connection.provider,
            connection.authentication,
            path,
            &credential.value,
            &query,
        )
        .await?;
    if response.status() == StatusCode::UNAUTHORIZED {
        set_status(root, id, ConnectionStatus::Expired, connection.generation)?;
    }
    check_status(response.status())?;
    let has_next = match connection.provider {
        Provider::Github => response
            .headers()
            .get("link")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.split(',').any(|link| link.contains("rel=\"next\""))),
        Provider::Gitlab => response
            .headers()
            .get("x-next-page")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u32>().ok())
            .is_some_and(|next| next == page + 1),
    };
    let repositories = match connection.provider {
        Provider::Github => {
            #[derive(Deserialize)]
            struct Item {
                full_name: String,
                clone_url: String,
                default_branch: Option<String>,
                private: bool,
            }
            let items = if connection.authentication == Authentication::GithubApp {
                #[derive(Deserialize)]
                struct InstallationRepositories {
                    repositories: Vec<Item>,
                }
                read_json::<InstallationRepositories>(response)
                    .await?
                    .repositories
            } else {
                read_json::<Vec<Item>>(response).await?
            };
            items
                .into_iter()
                .map(|item| Repository {
                    full_name: item.full_name,
                    clone_url: item.clone_url,
                    default_branch: item.default_branch,
                    private: item.private,
                })
                .collect::<Vec<_>>()
        }
        Provider::Gitlab => {
            #[derive(Deserialize)]
            struct Item {
                path_with_namespace: String,
                http_url_to_repo: String,
                default_branch: Option<String>,
                visibility: String,
            }
            read_json::<Vec<Item>>(response)
                .await?
                .into_iter()
                .map(|item| Repository {
                    full_name: item.path_with_namespace,
                    clone_url: item.http_url_to_repo,
                    default_branch: item.default_branch,
                    private: item.visibility != "public",
                })
                .collect::<Vec<_>>()
        }
    };
    if repositories.len() > PAGE_SIZE as usize
        || repositories
            .iter()
            .any(|repository| !valid_repository(connection.provider, repository))
    {
        return Err(invalid("Git provider returned an invalid repository"));
    }
    set_status(root, id, ConnectionStatus::Connected, connection.generation)?;
    Ok(RepositoryPage {
        repositories,
        next_page: (has_next && page < 10000).then_some(page + 1),
    })
}

fn valid_repository(provider: Provider, repository: &Repository) -> bool {
    let Ok(url) = reqwest::Url::parse(&repository.clone_url) else {
        return false;
    };
    let host = provider_host(provider);
    url.scheme() == "https"
        && url.host_str() == Some(host)
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && !repository.full_name.is_empty()
        && repository.full_name.len() <= 512
        && !repository.full_name.chars().any(char::is_control)
        && repository
            .default_branch
            .as_deref()
            .is_none_or(|branch| branch.len() <= 256 && !branch.chars().any(char::is_control))
}

fn provider_host(provider: Provider) -> &'static str {
    match provider {
        Provider::Github => "github.com",
        Provider::Gitlab => "gitlab.com",
    }
}

fn repository_snapshot(root: &Path, id: &str) -> Result<(Connection, Credential), SourceError> {
    let _guard = RECORDS
        .lock()
        .map_err(|_| invalid("Git connection storage is busy"))?;
    let mut connection = read_connection(root, id)?;
    let credential = super::load_credential(root, &connection.credential_id)?;
    if !matches!(credential.metadata.kind, CredentialKind::Git)
        || credential.server.as_deref() != Some(provider_host(connection.provider))
    {
        return Err(invalid("Git connection credential is invalid"));
    }
    connection.generation = rand::random();
    write_connection(root, &connection)?;
    Ok((connection, credential))
}

fn set_status(
    root: &Path,
    id: &str,
    status: ConnectionStatus,
    generation: u64,
) -> Result<(), SourceError> {
    let _guard = RECORDS
        .lock()
        .map_err(|_| invalid("Git connection storage is busy"))?;
    let mut connection = match read_connection(root, id) {
        Ok(connection) => connection,
        Err(SourceError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    if connection.generation != generation {
        return Ok(());
    }
    if connection.status != status {
        connection.status = status;
        write_connection(root, &connection)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
