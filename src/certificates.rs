//! Persistent, explicitly configured HTTP-01 certificates. Applications never receive keys.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, bail};
use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::get,
};
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, Identifier, NewAccount,
    NewOrder, OrderStatus, RetryPolicy,
};
use rustls::sign::CertifiedKey;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use x509_parser::{extensions::GeneralName, parse_x509_certificate};

type Challenges = Arc<RwLock<HashMap<(String, String), String>>>;

const RETRY_SECONDS: i64 = 3600;
const MAX_RENEWAL_SECONDS: i64 = 30 * 24 * 3600;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CertificateConfig {
    pub directory_url: String,
    #[serde(default)]
    pub contact: Vec<String>,
    pub hostnames: Vec<String>,
    /// Trust a private ACME service explicitly. Normal public CAs use system roots.
    #[serde(default)]
    pub ca_cert_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CertificateState {
    Pending,
    Valid,
    RenewalFailed,
    Expired,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CertificateStatus {
    pub hostname: String,
    pub state: CertificateState,
    pub not_before: Option<i64>,
    pub expires_at: Option<i64>,
    pub renew_at: Option<i64>,
    pub last_attempt: Option<i64>,
    pub retry_at: Option<i64>,
    pub last_error: Option<String>,
}

impl CertificateStatus {
    fn pending(hostname: String) -> Self {
        Self {
            hostname,
            state: CertificateState::Pending,
            not_before: None,
            expires_at: None,
            renew_at: None,
            last_attempt: None,
            retry_at: None,
            last_error: None,
        }
    }
}

#[derive(Deserialize, Serialize)]
struct AccountFile {
    directory_url: String,
    credentials: AccountCredentials,
}

// One atomic file keeps a chain and its private key together across a crash.
#[derive(Deserialize, Serialize)]
struct CertificateBundle {
    hostname: String,
    chain_pem: String,
    key_pem: String,
}

struct CertificateEntry {
    key: Arc<CertifiedKey>,
    names: Vec<String>,
    not_before: i64,
    expires_at: i64,
}

impl CertificateEntry {
    fn from_pem(chain: &str, key: &str) -> anyhow::Result<Self> {
        let certs = rustls_pemfile::certs(&mut chain.as_bytes()).collect::<Result<Vec<_>, _>>()?;
        let leaf = certs.first().context("empty certificate chain")?;
        let (_, certificate) = parse_x509_certificate(leaf.as_ref())
            .map_err(|_| anyhow::anyhow!("invalid leaf certificate"))?;
        let names = certificate
            .subject_alternative_name()
            .map_err(|_| anyhow::anyhow!("invalid subject alternative names"))?
            .context("missing subject alternative names")?
            .value
            .general_names
            .iter()
            .filter_map(|name| match name {
                GeneralName::DNSName(name) => Some(name.to_ascii_lowercase()),
                _ => None,
            })
            .collect();
        let not_before = certificate.validity().not_before.timestamp();
        let expires_at = certificate.validity().not_after.timestamp();
        let key =
            rustls_pemfile::private_key(&mut key.as_bytes())?.context("missing private key")?;
        let key = rustls::crypto::ring::sign::any_supported_type(&key)
            .context("unsupported private key")?;
        let certified = CertifiedKey::new(certs, key);
        certified
            .keys_match()
            .context("certificate and private key differ")?;
        Ok(Self {
            key: Arc::new(certified),
            names,
            not_before,
            expires_at,
        })
    }

    fn valid_at(&self, now: i64) -> bool {
        self.not_before <= now && now < self.expires_at
    }
    fn renew_at(&self) -> i64 {
        self.expires_at - ((self.expires_at - self.not_before) / 3).min(MAX_RENEWAL_SECONDS)
    }
    fn covers(&self, name: &str) -> bool {
        self.names.iter().any(|san| {
            san == name
                || san.strip_prefix("*.").is_some_and(|suffix| {
                    name.strip_suffix(suffix).is_some_and(|prefix| {
                        prefix.ends_with('.')
                            && !prefix[..prefix.len() - 1].contains('.')
                            && prefix.len() > 1
                    })
                })
        })
    }
}

pub struct CertificateResolver {
    local: CertificateEntry,
    managed: BTreeSet<String>,
    public: RwLock<HashMap<String, CertificateEntry>>,
}

impl std::fmt::Debug for CertificateResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertificateResolver")
            .field("managed", &self.managed)
            .finish_non_exhaustive()
    }
}

impl CertificateResolver {
    fn for_name(&self, name: Option<&str>, now: i64) -> Option<Arc<CertifiedKey>> {
        let Some(name) = name else {
            return self.local.valid_at(now).then(|| self.local.key.clone());
        };
        let name = name.to_ascii_lowercase();
        if self.managed.contains(&name) {
            return self
                .public
                .read()
                .unwrap()
                .get(&name)
                .filter(|entry| entry.valid_at(now))
                .map(|entry| entry.key.clone());
        }
        (self.local.valid_at(now) && self.local.covers(&name)).then(|| self.local.key.clone())
    }
}

impl rustls::server::ResolvesServerCert for CertificateResolver {
    fn resolve(&self, hello: rustls::server::ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.for_name(hello.server_name(), now())
    }
}

#[derive(Clone)]
pub struct CertificateManager {
    inner: Arc<ManagerInner>,
}

struct ManagerInner {
    dir: PathBuf,
    config: Option<CertificateConfig>,
    resolver: Arc<CertificateResolver>,
    challenges: Challenges,
    statuses: RwLock<BTreeMap<String, CertificateStatus>>,
    reconcile: Mutex<()>,
}

impl CertificateManager {
    /// `dir` is authoritative Platform State, distinct from generated LAN CA files.
    /// Absent public.json means public issuance is disabled.
    pub fn open(dir: &Path, local_cert: &Path, local_key: &Path) -> anyhow::Result<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let local = CertificateEntry::from_pem(
            &std::fs::read_to_string(local_cert)?,
            &std::fs::read_to_string(local_key)?,
        )?;
        reject_symlink(dir)?;
        if dir.exists() {
            secure_dir(dir)?;
        }
        let config = read_optional::<CertificateConfig>(&dir.join("public.json"))?;
        let config = config.map(validate_config).transpose()?;
        let managed: BTreeSet<String> = config
            .as_ref()
            .map(|config| config.hostnames.iter().cloned().collect())
            .unwrap_or_default();
        let mut statuses = if config.is_some() {
            read_optional::<BTreeMap<String, CertificateStatus>>(&dir.join("status.json"))?
                .unwrap_or_default()
        } else {
            BTreeMap::new()
        };
        statuses.retain(|hostname, _| managed.contains(hostname));
        let mut public = HashMap::new();
        for hostname in &managed {
            let status = statuses
                .entry(hostname.clone())
                .or_insert_with(|| CertificateStatus::pending(hostname.clone()));
            status.hostname = hostname.clone();
            match read_optional::<CertificateBundle>(&bundle_path(dir, hostname)) {
                Ok(Some(bundle)) => match validated_bundle(hostname, &bundle) {
                    Ok(entry) => {
                        apply_entry_status(status, &entry);
                        public.insert(hostname.clone(), entry);
                    }
                    Err(_) => {
                        *status = CertificateStatus::pending(hostname.clone());
                        status.last_error = Some("persisted certificate is invalid".into());
                    }
                },
                Ok(None) => {
                    status.not_before = None;
                    status.expires_at = None;
                    status.renew_at = None;
                }
                Err(error) => return Err(error),
            }
        }
        if config.is_some() {
            atomic_json(&dir.join("status.json"), &statuses)?;
        }
        Ok(Self {
            inner: Arc::new(ManagerInner {
                dir: dir.into(),
                config,
                resolver: Arc::new(CertificateResolver {
                    local,
                    managed,
                    public: RwLock::new(public),
                }),
                challenges: Arc::new(RwLock::new(HashMap::new())),
                statuses: RwLock::new(statuses),
                reconcile: Mutex::new(()),
            }),
        })
    }

    pub fn resolver(&self) -> Arc<CertificateResolver> {
        self.inner.resolver.clone()
    }

    pub fn status(&self) -> Vec<CertificateStatus> {
        let now = now();
        self.inner
            .statuses
            .read()
            .unwrap()
            .values()
            .cloned()
            .map(|mut status| {
                refresh_status(&mut status, now);
                status
            })
            .collect()
    }

    #[cfg(test)]
    pub fn status_router(&self) -> Router {
        Router::new()
            .route(
                "/certificates",
                get(|State(manager): State<Self>| async move { axum::Json(manager.status()) }),
            )
            .with_state(self.clone())
    }

    pub fn challenge_router(&self) -> Router {
        Router::new()
            .route("/.well-known/acme-challenge/{token}", get(challenge))
            .with_state(self.inner.challenges.clone())
    }

    /// Call after the challenge listeners are serving. Per-name errors are persisted
    /// and retried in an hour; one failed alias does not prevent another order.
    pub async fn reconcile(&self) -> anyhow::Result<()> {
        self.reconcile_at(now()).await
    }

    async fn reconcile_at(&self, now: i64) -> anyhow::Result<()> {
        let _guard = self.inner.reconcile.lock().await;
        let Some(config) = &self.inner.config else {
            return Ok(());
        };
        let due: Vec<String> = self
            .inner
            .statuses
            .read()
            .unwrap()
            .values()
            .filter(|status| {
                status.retry_at.is_none_or(|retry| now >= retry)
                    && status.renew_at.is_none_or(|renew| now >= renew)
            })
            .map(|status| status.hostname.clone())
            .collect();
        if due.is_empty() {
            return Ok(());
        }
        secure_dir(&self.inner.dir)?;
        let account =
            match tokio::time::timeout(Duration::from_secs(30), self.account(config)).await {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!("ACME account request timed out")),
            };
        for hostname in due {
            let result = match &account {
                Ok(account) => match tokio::time::timeout(
                    Duration::from_secs(120),
                    self.issue(account, &hostname),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err(anyhow::anyhow!("ACME operation timed out")),
                },
                Err(_) => Err(anyhow::anyhow!("ACME account unavailable")),
            };
            let mut statuses = self.inner.statuses.write().unwrap();
            let status = statuses.get_mut(&hostname).unwrap();
            status.last_attempt = Some(now);
            match result {
                Ok(bundle) => {
                    // Validate and persist before replacing the live key.
                    match validated_bundle(&hostname, &bundle).and_then(|entry| {
                        anyhow::ensure!(
                            entry.valid_at(now),
                            "issued certificate is not currently valid"
                        );
                        atomic_json(&bundle_path(&self.inner.dir, &hostname), &bundle)?;
                        Ok(entry)
                    }) {
                        Ok(entry) => {
                            apply_entry_status(status, &entry);
                            status.last_error = None;
                            status.retry_at = None;
                            self.inner
                                .resolver
                                .public
                                .write()
                                .unwrap()
                                .insert(hostname.clone(), entry);
                        }
                        Err(_) => mark_failure(
                            status,
                            now,
                            "certificate validation or persistence failed",
                        ),
                    }
                }
                Err(_) => mark_failure(
                    status,
                    now,
                    if account.is_err() {
                        "ACME account unavailable"
                    } else {
                        "ACME issuance or renewal failed"
                    },
                ),
            }
            atomic_json(&self.inner.dir.join("status.json"), &*statuses)?;
            if status_failed(&statuses, &hostname) {
                tracing::warn!(%hostname, "public certificate operation failed; inspect /certificates");
            }
        }
        Ok(())
    }

    async fn account(&self, config: &CertificateConfig) -> anyhow::Result<Account> {
        let builder = match &config.ca_cert_path {
            Some(path) => Account::builder_with_root(path)?,
            None => Account::builder()?,
        };
        if let Some(saved) = read_optional::<AccountFile>(&self.inner.dir.join("account.json"))? {
            anyhow::ensure!(
                saved.directory_url == config.directory_url,
                "ACME directory changed; use separate account state"
            );
            return Ok(builder.from_credentials(saved.credentials).await?);
        }
        let contacts: Vec<&str> = config.contact.iter().map(String::as_str).collect();
        let (account, credentials) = builder
            .create(
                &NewAccount {
                    contact: &contacts,
                    terms_of_service_agreed: true,
                    only_return_existing: false,
                },
                config.directory_url.clone(),
                None,
            )
            .await?;
        atomic_json(
            &self.inner.dir.join("account.json"),
            &AccountFile {
                directory_url: config.directory_url.clone(),
                credentials,
            },
        )?;
        Ok(account)
    }

    async fn issue(&self, account: &Account, hostname: &str) -> anyhow::Result<CertificateBundle> {
        let mut cleanup = ChallengeCleanup {
            challenges: self.inner.challenges.clone(),
            keys: Vec::new(),
        };
        let identifiers = [Identifier::Dns(hostname.to_string())];
        let mut order = account.new_order(&NewOrder::new(&identifiers)).await?;
        let mut authorizations = order.authorizations();
        while let Some(authorization) = authorizations.next().await {
            let mut authorization = authorization?;
            match authorization.status {
                AuthorizationStatus::Valid => continue,
                AuthorizationStatus::Pending => {}
                _ => bail!("authorization unavailable"),
            }
            let mut challenge = authorization
                .challenge(ChallengeType::Http01)
                .context("ACME service has no HTTP-01 challenge")?;
            anyhow::ensure!(
                challenge.identifier().to_string() == hostname,
                "unexpected authorization name"
            );
            anyhow::ensure!(valid_token(&challenge.token), "invalid challenge token");
            let key = (hostname.to_string(), challenge.token.clone());
            self.inner.challenges.write().unwrap().insert(
                key.clone(),
                challenge.key_authorization().as_str().to_string(),
            );
            cleanup.keys.push(key);
            challenge.set_ready().await?;
        }
        let retry = RetryPolicy::new().timeout(Duration::from_secs(45));
        anyhow::ensure!(
            order.poll_ready(&retry).await? == OrderStatus::Ready,
            "ACME order is not ready"
        );
        let key = rcgen::KeyPair::generate()?;
        let request =
            rcgen::CertificateParams::new(vec![hostname.to_string()])?.serialize_request(&key)?;
        order.finalize_csr(request.der()).await?;
        let chain_pem = order.poll_certificate(&retry).await?;
        Ok(CertificateBundle {
            hostname: hostname.to_string(),
            chain_pem,
            key_pem: key.serialize_pem(),
        })
    }
}

/// Metadata-only view for the existing authenticated Operator API.
/// Pass the authoritative `state/certificates` directory.
pub fn read_status(dir: &Path) -> anyhow::Result<Vec<CertificateStatus>> {
    reject_symlink(dir)?;
    let Some(config) = read_optional::<CertificateConfig>(&dir.join("public.json"))? else {
        return Ok(Vec::new());
    };
    let config = validate_config(config)?;
    let mut statuses =
        read_optional::<BTreeMap<String, CertificateStatus>>(&dir.join("status.json"))?
            .unwrap_or_default();
    let now = now();
    Ok(config
        .hostnames
        .into_iter()
        .map(|hostname| {
            let mut status = statuses
                .remove(&hostname)
                .unwrap_or_else(|| CertificateStatus::pending(hostname.clone()));
            status.hostname = hostname;
            refresh_status(&mut status, now);
            status
        })
        .collect())
}

fn refresh_status(status: &mut CertificateStatus, now: i64) {
    status.state = match (status.not_before, status.expires_at) {
        (Some(before), _) if now < before => CertificateState::Pending,
        (_, Some(expires)) if now >= expires => CertificateState::Expired,
        (_, Some(_)) if status.last_error.is_some() => CertificateState::RenewalFailed,
        (_, Some(_)) => CertificateState::Valid,
        (_, None) => CertificateState::Pending,
    };
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
fn status_failed(statuses: &BTreeMap<String, CertificateStatus>, hostname: &str) -> bool {
    statuses[hostname].last_error.is_some()
}
fn mark_failure(status: &mut CertificateStatus, now: i64, error: &str) {
    status.last_error = Some(error.into());
    status.retry_at = Some(now + RETRY_SECONDS);
}
fn apply_entry_status(status: &mut CertificateStatus, entry: &CertificateEntry) {
    status.not_before = Some(entry.not_before);
    status.expires_at = Some(entry.expires_at);
    status.renew_at = Some(entry.renew_at());
}
fn validated_bundle(
    hostname: &str,
    bundle: &CertificateBundle,
) -> anyhow::Result<CertificateEntry> {
    anyhow::ensure!(
        bundle.hostname == hostname,
        "certificate bundle has a different name"
    );
    let entry = CertificateEntry::from_pem(&bundle.chain_pem, &bundle.key_pem)?;
    anyhow::ensure!(
        entry.names.iter().any(|name| name == hostname),
        "certificate does not cover requested name"
    );
    Ok(entry)
}
fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn validate_config(mut config: CertificateConfig) -> anyhow::Result<CertificateConfig> {
    let directory = reqwest::Url::parse(&config.directory_url)?;
    anyhow::ensure!(
        directory.scheme() == "https"
            && directory.username().is_empty()
            && directory.password().is_none()
            && directory.fragment().is_none(),
        "ACME directory must use HTTPS without credentials or fragment"
    );
    let mut names = BTreeSet::new();
    for hostname in &mut config.hostnames {
        *hostname = hostname.to_ascii_lowercase();
        anyhow::ensure!(
            hostname.len() <= 253
                && hostname.contains('.')
                && hostname.parse::<std::net::IpAddr>().is_err()
                && hostname.split('.').all(|label| !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')),
            "invalid certificate hostname"
        );
        anyhow::ensure!(
            names.insert(hostname.clone()),
            "duplicate certificate hostname"
        );
    }
    anyhow::ensure!(
        !names.is_empty(),
        "certificate configuration needs at least one hostname"
    );
    for contact in &config.contact {
        anyhow::ensure!(
            contact.starts_with("mailto:") && !contact.contains(['\n', '\r']),
            "ACME contact must be a mailto address"
        );
    }
    Ok(config)
}

async fn challenge(
    State(challenges): State<Challenges>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Result<String, StatusCode> {
    let token = uri
        .path()
        .strip_prefix("/.well-known/acme-challenge/")
        .filter(|token| valid_token(token))
        .ok_or(StatusCode::NOT_FOUND)?;
    let hostname = headers
        .get("host")
        .and_then(|host| host.to_str().ok())
        .and_then(|host| host.parse::<axum::http::uri::Authority>().ok())
        .map(|host| host.host().to_ascii_lowercase())
        .ok_or(StatusCode::NOT_FOUND)?;
    challenges
        .read()
        .unwrap()
        .get(&(hostname, token.to_string()))
        .cloned()
        .ok_or(StatusCode::NOT_FOUND)
}

struct ChallengeCleanup {
    challenges: Challenges,
    keys: Vec<(String, String)>,
}
impl Drop for ChallengeCleanup {
    fn drop(&mut self) {
        let mut challenges = self.challenges.write().unwrap();
        for key in &self.keys {
            challenges.remove(key);
        }
    }
}

fn bundle_path(dir: &Path, hostname: &str) -> PathBuf {
    dir.join(format!("{hostname}.json"))
}
fn reject_symlink(path: &Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("certificate state cannot be a symlink")
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
fn secure_dir(dir: &Path) -> anyhow::Result<()> {
    reject_symlink(dir)?;
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn read_optional<T: serde::de::DeserializeOwned>(path: &Path) -> anyhow::Result<Option<T>> {
    reject_symlink(path)?;
    match std::fs::read(path) {
        Ok(bytes) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
            }
            Ok(Some(
                serde_json::from_slice(&bytes).context("invalid certificate state JSON")?,
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn atomic_json(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    secure_dir(path.parent().context("certificate state has no parent")?)?;
    reject_symlink(path)?;
    let temporary = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec(value)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        std::fs::File::open(path.parent().unwrap())?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
#[path = "certificates_tests.rs"]
mod tests;
