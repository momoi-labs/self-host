//! Persistent certificates for every public Hostname, through HTTP-01 or, for
//! names inside a configured deSEC zone, DNS-01. Applications never receive keys.
//!
//! The Operator configures the ACME account once, in `public.json`, through
//! the Settings screen or by hand. Which names get a certificate follows the
//! Applications: every Hostname outside the DNS Suffix is public, plus any
//! name the file lists. The daemon reads both again every minute, so a new
//! Hostname or a changed account needs no restart.

mod desec;

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
const DNS_WAIT: Duration = Duration::from_secs(600);
const DNS_ORDER_POLL: Duration = Duration::from_secs(600);
const DNS_POLL: Duration = Duration::from_secs(1);
const CHALLENGE_LABEL: &str = "_acme-challenge";
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns: Option<DnsConfig>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DnsProvider {
    Desec,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DnsConfig {
    pub provider: DnsProvider,
    pub zone: String,
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nameservers: Option<Vec<std::net::SocketAddr>>,
}

impl DnsConfig {
    fn covers(&self, hostname: &str) -> bool {
        hostname == self.zone || hostname.ends_with(&format!(".{}", self.zone))
    }
}

#[derive(Debug)]
enum DnsFailure {
    Refused,
    NotReached,
}

impl DnsFailure {
    fn message(&self) -> &'static str {
        match self {
            Self::Refused => "DNS provider refused the challenge record",
            Self::NotReached => "challenge record did not reach the nameservers",
        }
    }
}

impl std::fmt::Display for DnsFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for DnsFailure {}

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

    fn covers(&self, name: &str) -> bool {
        self.names.iter().any(|covered| {
            covered == name
                || covered.strip_prefix("*.").is_some_and(|base| {
                    name.split_once('.')
                        .is_some_and(|(label, parent)| !label.is_empty() && parent == base)
                })
        })
    }

    fn valid_at(&self, now: i64) -> bool {
        self.not_before <= now && now < self.expires_at
    }
    fn renew_at(&self) -> i64 {
        self.expires_at - ((self.expires_at - self.not_before) / 3).min(MAX_RENEWAL_SECONDS)
    }
}

pub struct CertificateResolver {
    local: CertificateEntry,
    managed: RwLock<BTreeSet<String>>,
    public: RwLock<HashMap<String, CertificateEntry>>,
    tailnet: RwLock<Option<(String, CertificateEntry)>>,
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
        if let Some((tailnet, entry)) = &*self.tailnet.read().unwrap()
            && *tailnet == name
            && entry.valid_at(now)
        {
            return Some(entry.key.clone());
        }
        if self.managed.read().unwrap().contains(&name) {
            return self
                .public
                .read()
                .unwrap()
                .get(&name)
                .filter(|entry| entry.valid_at(now))
                .map(|entry| entry.key.clone())
                .or_else(|| self.wildcard_for(&name, now));
        }
        if let Some(key) = self.wildcard_for(&name, now) {
            return Some(key);
        }
        // Any other name gets the Platform's own certificate, whether or not
        // its names cover it. A browser rejects the mismatch either way; an
        // edge proxy that trusts the origin on its own terms, such as
        // Cloudflare in "Full" mode, completes the handshake and serves the
        // Application without a public certificate.
        self.local.valid_at(now).then(|| self.local.key.clone())
    }
}

impl CertificateResolver {
    fn wildcard_for(&self, name: &str, now: i64) -> Option<Arc<CertifiedKey>> {
        let parent = name.split_once('.').map(|(_, parent)| parent);
        let managed = self.managed.read().unwrap();
        let public = self.public.read().unwrap();
        [Some(name), parent]
            .into_iter()
            .flatten()
            .map(|base| format!("*.{base}"))
            .filter(|wildcard| managed.contains(wildcard))
            .filter_map(|wildcard| public.get(&wildcard))
            .find(|entry| entry.valid_at(now) && entry.covers(name))
            .map(|entry| entry.key.clone())
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
    dns_wait: std::sync::atomic::AtomicU64,
    suffix: RwLock<Option<String>>,
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
                    tailnet: RwLock::new(None),
                }),
                challenges: Arc::new(RwLock::new(HashMap::new())),
                statuses: RwLock::new(statuses),
                reconcile: Mutex::new(()),
                dns_wait: std::sync::atomic::AtomicU64::new(DNS_WAIT.as_millis() as u64),
                suffix: RwLock::new(None),
            }),
        })
    }

    pub fn set_suffix(&self, suffix: &str) {
        *self.inner.suffix.write().unwrap() = Some(suffix.to_ascii_lowercase());
    }

    pub fn resolver(&self) -> Arc<CertificateResolver> {
        self.inner.resolver.clone()
    }

    pub fn install_tailnet(&self, name: &str, chain: &str, key: &str) -> anyhow::Result<()> {
        let entry = CertificateEntry::from_pem(chain, key)?;
        let name = name.to_ascii_lowercase();
        if !entry.names.contains(&name) {
            bail!("the tailnet certificate does not name {name}");
        }
        *self.inner.resolver.tailnet.write().unwrap() = Some((name, entry));
        Ok(())
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
        let suffix = self.inner.suffix.read().unwrap().clone();
        let desired: BTreeSet<String> = match &config {
            Some(config) => config
                .hostnames
                .iter()
                .cloned()
                .chain(hostnames.into_iter().filter(|name| valid_hostname(name)))
                .chain(
                    suffix
                        .filter(|suffix| {
                            valid_hostname(suffix)
                                && config.dns.as_ref().is_some_and(|dns| dns.covers(suffix))
                        })
                        .map(|suffix| format!("*.{suffix}")),
                )
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
            let dns = config.dns.as_ref().filter(|dns| dns.covers(&hostname));
            let limit = match dns {
                Some(_) => self.dns_wait() + DNS_ORDER_POLL + Duration::from_secs(120),
                None => Duration::from_secs(120),
            };
            let result = match &account {
                Ok(account) => {
                    match tokio::time::timeout(limit, self.issue(account, &hostname, &config)).await
                    {
                        Ok(result) => result,
                        Err(_) => Err(anyhow::anyhow!("ACME operation timed out")),
                    }
                }
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
                Err(error) => {
                    tracing::warn!(%hostname, cause = %format!("{error:#}"), "public certificate order failed");
                    mark_failure(
                        status,
                        now,
                        match error.downcast_ref::<DnsFailure>() {
                            Some(failure) => failure.message(),
                            None if account.is_err() => "ACME account unavailable",
                            None => "ACME issuance or renewal failed",
                        },
                    )
                }
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

    fn dns_wait(&self) -> Duration {
        Duration::from_millis(
            self.inner
                .dns_wait
                .load(std::sync::atomic::Ordering::SeqCst),
        )
    }

    #[cfg(test)]
    fn set_dns_wait(&self, wait: Duration) {
        self.inner
            .dns_wait
            .store(wait.as_millis() as u64, std::sync::atomic::Ordering::SeqCst);
    }

    async fn issue(
        &self,
        account: &Account,
        hostname: &str,
        config: &CertificateConfig,
    ) -> anyhow::Result<CertificateBundle> {
        let dns = config.dns.as_ref().filter(|dns| dns.covers(hostname));
        let mut published = Vec::new();
        let result = self.order(account, hostname, dns, &mut published).await;
        if let Some(dns) = dns
            && let Ok(client) = desec::Desec::new(dns)
        {
            for subname in published {
                if client.delete_txt(&subname).await.is_err() {
                    tracing::warn!(%hostname, "could not remove the DNS challenge record");
                }
            }
        }
        result
    }

    async fn order(
        &self,
        account: &Account,
        hostname: &str,
        dns: Option<&DnsConfig>,
        published: &mut Vec<String>,
    ) -> anyhow::Result<CertificateBundle> {
        let mut cleanup = ChallengeCleanup {
            challenges: self.inner.challenges.clone(),
            keys: Vec::new(),
        };
        let names: Vec<String> = match hostname.strip_prefix("*.") {
            Some(base) => {
                anyhow::ensure!(dns.is_some(), "a wildcard needs a DNS zone");
                vec![hostname.to_string(), base.to_string()]
            }
            None => vec![hostname.to_string()],
        };
        let identifiers: Vec<Identifier> = names.iter().cloned().map(Identifier::Dns).collect();
        let mut order = account.new_order(&NewOrder::new(&identifiers)).await?;
        let mut records: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut authorizations = order.authorizations();
        while let Some(authorization) = authorizations.next().await {
            let mut authorization = authorization?;
            match authorization.status {
                AuthorizationStatus::Valid => continue,
                AuthorizationStatus::Pending => {}
                _ => bail!("authorization unavailable"),
            }
            let mut challenge = match dns {
                Some(_) => authorization
                    .challenge(ChallengeType::Dns01)
                    .context("ACME service has no DNS-01 challenge")?,
                None => authorization
                    .challenge(ChallengeType::Http01)
                    .context("ACME service has no HTTP-01 challenge")?,
            };
            let identifier = challenge.identifier().to_string();
            anyhow::ensure!(names.contains(&identifier), "unexpected authorization name");
            anyhow::ensure!(valid_token(&challenge.token), "invalid challenge token");
            if dns.is_some() {
                records
                    .entry(identifier.trim_start_matches("*.").to_string())
                    .or_default()
                    .push(challenge.key_authorization().dns_value());
            } else {
                let key = (identifier, challenge.token.clone());
                self.inner.challenges.write().unwrap().insert(
                    key.clone(),
                    challenge.key_authorization().as_str().to_string(),
                );
                cleanup.keys.push(key);
                challenge.set_ready().await?;
            }
        }
        if let Some(dns) = dns {
            for (name, values) in &records {
                self.publish_txt(dns, name, values, published).await?;
            }
            let mut authorizations = order.authorizations();
            while let Some(authorization) = authorizations.next().await {
                let mut authorization = authorization?;
                if authorization.status != AuthorizationStatus::Pending {
                    continue;
                }
                authorization
                    .challenge(ChallengeType::Dns01)
                    .context("ACME service has no DNS-01 challenge")?
                    .set_ready()
                    .await?;
            }
        }
        let retry = RetryPolicy::new().timeout(match dns {
            Some(_) => DNS_ORDER_POLL,
            None => Duration::from_secs(45),
        });
        let status = order.poll_ready(&retry).await?;
        if status != OrderStatus::Ready {
            let mut problems = Vec::new();
            let mut authorizations = order.authorizations();
            while let Some(Ok(authorization)) = authorizations.next().await {
                for challenge in &authorization.challenges {
                    if let Some(problem) = &challenge.error {
                        problems.push(format!("{problem:?}"));
                    }
                }
            }
            bail!("ACME order is {status:?}: {}", problems.join("; "));
        }
        let key = rcgen::KeyPair::generate()?;
        let mut params = rcgen::CertificateParams::new(names)?;
        params.distinguished_name = rcgen::DistinguishedName::new();
        let request = params.serialize_request(&key)?;
        order.finalize_csr(request.der()).await?;
        let chain_pem = order.poll_certificate(&retry).await?;
        Ok(CertificateBundle {
            hostname: hostname.to_string(),
            chain_pem,
            key_pem: key.serialize_pem(),
        })
    }
}

impl CertificateManager {
    async fn publish_txt(
        &self,
        dns: &DnsConfig,
        name: &str,
        values: &[String],
        published: &mut Vec<String>,
    ) -> anyhow::Result<()> {
        let client = desec::Desec::new(dns)?;
        let subname = match name.strip_suffix(&format!(".{}", dns.zone)) {
            Some(label) => format!("{CHALLENGE_LABEL}.{label}"),
            None => CHALLENGE_LABEL.to_string(),
        };
        let ttl = client.minimum_ttl().await;
        published.push(subname.clone());
        client
            .put_txt(&subname, values, ttl)
            .await
            .map_err(|_| anyhow::Error::new(DnsFailure::Refused))?;
        let servers = match &dns.nameservers {
            Some(servers) => servers.clone(),
            None => client
                .nameserver_addresses()
                .await
                .map_err(|_| anyhow::Error::new(DnsFailure::NotReached))?,
        };
        desec::wait_for_txt(
            &servers,
            &format!("{CHALLENGE_LABEL}.{name}"),
            values,
            self.dns_wait(),
            DNS_POLL,
        )
        .await
        .map_err(|_| anyhow::Error::new(DnsFailure::NotReached))
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

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AcmeSettings {
    pub directory_url: String,
    pub email: String,
    #[serde(default, deserialize_with = "present")]
    pub dns: Option<Option<DnsSettings>>,
}

fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DnsSettings {
    pub provider: DnsProvider,
    pub zone: String,
    #[serde(default)]
    pub token: Option<String>,
}

impl std::fmt::Debug for DnsSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DnsSettings")
            .field("zone", &self.zone)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct AcmeView {
    pub directory_url: String,
    pub email: String,
    pub dns: Option<DnsView>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DnsView {
    pub provider: DnsProvider,
    pub zone: String,
    pub token_set: bool,
}

pub const LETS_ENCRYPT_DIRECTORY: &str = "https://acme-v02.api.letsencrypt.org/directory";

/// What `public.json` says about the account, or `None` while issuance is off.
pub fn read_acme(dir: &Path) -> anyhow::Result<Option<AcmeView>> {
    reject_symlink(dir)?;
    Ok(read_config(dir)?.map(|config| AcmeView {
        dns: config.dns.as_ref().map(|dns| DnsView {
            provider: dns.provider,
            zone: dns.zone.clone(),
            token_set: !dns.token.is_empty(),
        }),
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
    let saved_dns = previous.as_ref().and_then(|config| config.dns.clone());
    let dns = match &settings.dns {
        None => saved_dns,
        Some(None) => None,
        Some(Some(requested)) => {
            let token = match requested.token.as_deref().map(str::trim) {
                Some(token) if !token.is_empty() => token.to_string(),
                _ => saved_dns
                    .as_ref()
                    .map(|saved| saved.token.clone())
                    .context("the DNS token is required")?,
            };
            Some(DnsConfig {
                provider: requested.provider,
                zone: requested.zone.trim().to_ascii_lowercase(),
                token,
                api_url: saved_dns.as_ref().and_then(|saved| saved.api_url.clone()),
                nameservers: saved_dns.and_then(|saved| saved.nameservers),
            })
        }
    };
    let config = validate_config(CertificateConfig {
        dns,
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
    if let Some(dns) = &mut config.dns {
        anyhow::ensure!(valid_hostname(&dns.zone), "invalid DNS zone");
        anyhow::ensure!(
            !dns.token.is_empty() && dns.token.bytes().all(|b| b.is_ascii_graphic()),
            "invalid DNS token"
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
