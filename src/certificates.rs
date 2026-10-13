//! Persistent HTTP-01 certificates for every public Hostname. Applications never receive keys.
//!
//! The Operator configures the ACME account once, in `public.json`, through
//! the Settings screen or by hand. Which names get a certificate follows the
//! Applications: every Hostname outside the DNS Suffix is public, plus any
//! name the file lists. The daemon reads both again every minute, so a new
//! Hostname or a changed account needs no restart.

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

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CertificateConfig {
    pub directory_url: String,
    #[serde(default)]
    pub contact: Vec<String>,
    /// Names issued besides the Applications' public Hostnames.
    #[serde(default)]
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
    managed: RwLock<BTreeSet<String>>,
    public: RwLock<HashMap<String, CertificateEntry>>,
}

impl std::fmt::Debug for CertificateResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertificateResolver")
            .field("managed", &*self.managed.read().unwrap())
            .finish_non_exhaustive()
    }
}

impl CertificateResolver {
    fn for_name(&self, name: Option<&str>, now: i64) -> Option<Arc<CertifiedKey>> {
        let Some(name) = name else {
            return self.local.valid_at(now).then(|| self.local.key.clone());
        };
        let name = name.to_ascii_lowercase();
        if self.managed.read().unwrap().contains(&name) {
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
    /// What `public.json` said when it was last read. `None` disables issuance.
    config: RwLock<Option<CertificateConfig>>,
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
        let config = read_config(dir)?;
        // The names the file lists are known now; the Applications' public
        // Hostnames arrive with the first reconcile. A status the previous run
        // persisted for a name since removed is dropped then, not here, so a
        // restart never loses the record of an Application's certificate.
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
        let managed: BTreeSet<String> = managed
            .into_iter()
            .chain(statuses.keys().cloned())
            .collect();
        let mut public = HashMap::new();
        for hostname in &managed {
            if let Some(entry) = load_entry(dir, hostname, &mut statuses)? {
                public.insert(hostname.clone(), entry);
            }
        }
        if config.is_some() {
            atomic_json(&dir.join("status.json"), &statuses)?;
        }
        Ok(Self {
            inner: Arc::new(ManagerInner {
                dir: dir.into(),
                config: RwLock::new(config),
                resolver: Arc::new(CertificateResolver {
                    local,
                    managed: RwLock::new(managed),
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
    /// Issues the names `public.json` lists, and nothing for the Applications:
    /// `reconcile_for` is what the daemon runs.
    pub async fn reconcile(&self) -> anyhow::Result<()> {
        self.reconcile_for_at(BTreeSet::new(), now()).await
    }

    /// `reconcile` for the file's names and the Applications' public
    /// Hostnames together. Reads `public.json` again first, so a changed
    /// account applies without a restart; a name no longer wanted stops being
    /// served, and its certificate stays on disk for when it comes back.
    pub async fn reconcile_for(&self, hostnames: BTreeSet<String>) -> anyhow::Result<()> {
        self.reconcile_for_at(hostnames, now()).await
    }

    #[cfg(test)]
    async fn reconcile_at(&self, now: i64) -> anyhow::Result<()> {
        self.reconcile_for_at(BTreeSet::new(), now).await
    }

    async fn reconcile_for_at(&self, hostnames: BTreeSet<String>, now: i64) -> anyhow::Result<()> {
        let _guard = self.inner.reconcile.lock().await;
        let config = read_config(&self.inner.dir)?;
        let desired: BTreeSet<String> = match &config {
            Some(config) => config
                .hostnames
                .iter()
                .cloned()
                .chain(hostnames.into_iter().filter(|name| valid_hostname(name)))
                .collect(),
            None => BTreeSet::new(),
        };
        self.sync(desired, config.is_some())?;
        let changed = *self.inner.config.read().unwrap() != config;
        if changed {
            *self.inner.config.write().unwrap() = config.clone();
        }
        let Some(config) = config else {
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
            match tokio::time::timeout(Duration::from_secs(30), self.account(&config)).await {
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

    /// Makes the managed set `desired`: a new name starts pending, or serves
    /// the certificate a previous run left on disk; a dropped name stops being
    /// served. The statuses file follows when anything moved.
    fn sync(&self, desired: BTreeSet<String>, persist: bool) -> anyhow::Result<()> {
        let current = self.inner.resolver.managed.read().unwrap().clone();
        if current == desired {
            return Ok(());
        }
        let mut statuses = self.inner.statuses.write().unwrap();
        let mut public = self.inner.resolver.public.write().unwrap();
        for hostname in current.difference(&desired) {
            statuses.remove(hostname);
            public.remove(hostname);
        }
        for hostname in desired.difference(&current) {
            if let Some(entry) = load_entry(&self.inner.dir, hostname, &mut statuses)? {
                public.insert(hostname.clone(), entry);
            }
        }
        *self.inner.resolver.managed.write().unwrap() = desired;
        if persist {
            atomic_json(&self.inner.dir.join("status.json"), &*statuses)?;
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
/// Pass the authoritative `state/certificates` directory. The names are the
/// ones the daemon manages, as it last wrote them: the file's and the
/// Applications' public Hostnames.
pub fn read_status(dir: &Path) -> anyhow::Result<Vec<CertificateStatus>> {
    reject_symlink(dir)?;
    if read_config(dir)?.is_none() {
        return Ok(Vec::new());
    }
    let statuses = read_optional::<BTreeMap<String, CertificateStatus>>(&dir.join("status.json"))?
        .unwrap_or_default();
    let now = now();
    Ok(statuses
        .into_iter()
        .map(|(hostname, mut status)| {
            status.hostname = hostname;
            refresh_status(&mut status, now);
            status
        })
        .collect())
}

/// The ACME account as the Operator sets it: the CA and the contact address.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AcmeSettings {
    pub directory_url: String,
    pub email: String,
}

pub const LETS_ENCRYPT_DIRECTORY: &str = "https://acme-v02.api.letsencrypt.org/directory";

/// What `public.json` says about the account, or `None` while issuance is off.
pub fn read_acme(dir: &Path) -> anyhow::Result<Option<AcmeSettings>> {
    reject_symlink(dir)?;
    Ok(read_config(dir)?.map(|config| AcmeSettings {
        directory_url: config.directory_url,
        email: config
            .contact
            .first()
            .map(|contact| contact.trim_start_matches("mailto:").to_string())
            .unwrap_or_default(),
    }))
}

/// Saves the account and turns issuance on. The file's other fields, extra
/// names and a private CA's trust anchor, are kept. A different CA starts a
/// new account: the old credentials are removed, and the certificates the
/// old CA issued keep serving until they are renewed by the new one.
pub fn write_acme(dir: &Path, settings: &AcmeSettings) -> anyhow::Result<()> {
    let email = settings.email.trim();
    anyhow::ensure!(
        email.contains('@') && !email.contains(['/', '?', '#', ' ']),
        "contact must be an email address"
    );
    let previous = read_config(dir)?;
    let config = validate_config(CertificateConfig {
        directory_url: settings.directory_url.trim().to_string(),
        contact: vec![format!("mailto:{email}")],
        hostnames: previous
            .as_ref()
            .map(|config| config.hostnames.clone())
            .unwrap_or_default(),
        ca_cert_path: previous
            .as_ref()
            .and_then(|config| config.ca_cert_path.clone()),
    })?;
    secure_dir(dir)?;
    if previous.is_some_and(|previous| previous.directory_url != config.directory_url) {
        let account = dir.join("account.json");
        reject_symlink(&account)?;
        if let Err(error) = std::fs::remove_file(&account)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(error.into());
        }
    }
    atomic_json(&dir.join("public.json"), &config)
}

/// Turns issuance off. Certificates and the account stay on disk.
pub fn remove_acme(dir: &Path) -> anyhow::Result<()> {
    let path = dir.join("public.json");
    reject_symlink(&path)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Every Hostname the Applications answer on outside the DNS Suffix: the
/// names a public CA can vouch for, since the LAN certificate cannot.
pub fn public_hostnames(
    apps: &[crate::store::ApplicationRecord],
    dns_suffix: &str,
) -> BTreeSet<String> {
    let zone = format!(".{}", dns_suffix.to_ascii_lowercase());
    apps.iter()
        .flat_map(crate::routes::hostnames)
        .map(str::to_ascii_lowercase)
        .filter(|hostname| {
            !hostname.ends_with(&zone) && *hostname != zone[1..] && valid_hostname(hostname)
        })
        .collect()
}

fn read_config(dir: &Path) -> anyhow::Result<Option<CertificateConfig>> {
    read_optional::<CertificateConfig>(&dir.join("public.json"))?
        .map(validate_config)
        .transpose()
}

/// The certificate a previous run saved for the name, when it is still
/// readable and matches; the status reflects what was found either way.
fn load_entry(
    dir: &Path,
    hostname: &str,
    statuses: &mut BTreeMap<String, CertificateStatus>,
) -> anyhow::Result<Option<CertificateEntry>> {
    let status = statuses
        .entry(hostname.to_string())
        .or_insert_with(|| CertificateStatus::pending(hostname.to_string()));
    status.hostname = hostname.to_string();
    match read_optional::<CertificateBundle>(&bundle_path(dir, hostname))? {
        Some(bundle) => match validated_bundle(hostname, &bundle) {
            Ok(entry) => {
                apply_entry_status(status, &entry);
                Ok(Some(entry))
            }
            Err(_) => {
                *status = CertificateStatus::pending(hostname.to_string());
                status.last_error = Some("persisted certificate is invalid".into());
                Ok(None)
            }
        },
        None => {
            status.not_before = None;
            status.expires_at = None;
            status.renew_at = None;
            Ok(None)
        }
    }
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
        anyhow::ensure!(valid_hostname(hostname), "invalid certificate hostname");
        anyhow::ensure!(
            names.insert(hostname.clone()),
            "duplicate certificate hostname"
        );
    }
    for contact in &config.contact {
        anyhow::ensure!(
            contact.starts_with("mailto:") && !contact.contains(['\n', '\r']),
            "ACME contact must be a mailto address"
        );
    }
    Ok(config)
}

/// A name a CA can be asked for: a lowercase DNS name with a dot, not an
/// address and not a wildcard.
fn valid_hostname(hostname: &str) -> bool {
    hostname.len() <= 253
        && hostname.contains('.')
        && hostname.parse::<std::net::IpAddr>().is_err()
        && hostname.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
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
