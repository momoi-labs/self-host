use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use axum::{
    Json,
    extract::{ConnectInfo, Path, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use webauthn_rs::prelude::{
    CreationChallengeResponse, Passkey, PasskeyAuthentication, PasskeyRegistration,
    PublicKeyCredential, RegisterPublicKeyCredential, RequestChallengeResponse, Url, Uuid,
    Webauthn, WebauthnBuilder,
};

use crate::{
    AppState,
    auth::{AuthorizedBy, Kind, Principal, Scope, SessionRef},
    collection::{Collection, Keyed},
    error::ErrorReport,
    store::StateStore,
};

pub const SESSION_COOKIE: &str = "__Host-self-host-session";
pub const RECOVERY_HEADER: &str = "x-self-host-recovery";
pub const RECOVERY_SECRET_STATE: &str = "account_recovery_secret";
pub const OPERATOR: &str = "operator";
pub const ROOT_RETIRED_STATE: &str = "api_key_retired_at";
pub const CONSOLE_ORIGIN_ENV: &str = "SELF_HOST_CONSOLE_ORIGIN";
const ACCOUNT_STATE: &str = "operator_account";
const SESSION_HOURS: i64 = 12;
const CEREMONY_TTL: Duration = Duration::from_secs(5 * 60);
const STEP_UP_TTL: Duration = Duration::from_secs(2 * 60);
const SETUP_KEY_TTL: Duration = Duration::from_secs(15 * 60);
const SETUP_KEY_PREFIX: &str = "setup-";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub created_at: String,
    pub rp_id: String,
    pub passkeys: Vec<StoredPasskey>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredPasskey {
    pub id: String,
    pub name: String,
    pub created_at: String,
    #[serde(default)]
    pub last_used_at: Option<String>,
    pub credential: Passkey,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SessionRecord {
    id: String,
    account: String,
    passkey: String,
    created_at: String,
    expires_at: String,
}

impl Keyed for SessionRecord {
    fn key(&self) -> String {
        self.id.clone()
    }
}

static SESSIONS: Collection<SessionRecord> = Collection::new("operator-session");

#[derive(Clone, PartialEq, Eq)]
enum Authority {
    Root,
    SetupKey(String),
    Session(String),
}

struct Registration {
    state: PasskeyRegistration,
    authority: Authority,
    account: String,
    name: String,
    rp_id: String,
    expires: Instant,
}

struct Login {
    state: PasskeyAuthentication,
    rp_id: String,
    expires: Instant,
}

struct StepUp {
    state: PasskeyAuthentication,
    session: String,
    fingerprint: String,
    rp_id: String,
    expires: Instant,
}

#[derive(Default)]
struct Pending {
    registrations: HashMap<String, Registration>,
    logins: HashMap<String, Login>,
    step_ups: HashMap<String, StepUp>,
    setup_key: Option<(String, Instant)>,
}

impl Pending {
    fn sweep(&mut self) {
        let now = Instant::now();
        self.registrations.retain(|_, r| r.expires > now);
        self.logins.retain(|_, l| l.expires > now);
        self.step_ups.retain(|_, s| s.expires > now);
        if self.setup_key.as_ref().is_some_and(|(_, at)| *at <= now) {
            self.setup_key = None;
        }
    }
}

#[derive(Default)]
pub struct Ceremonies {
    write: Mutex<()>,
    pending: std::sync::Mutex<Pending>,
}

impl Ceremonies {
    fn pending(&self) -> std::sync::MutexGuard<'_, Pending> {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.sweep();
        pending
    }
}

#[derive(Clone)]
pub struct Canonical {
    pub host: String,
    pub origin: Url,
    pub rp_id: String,
}

impl Canonical {
    pub fn origin_text(&self) -> String {
        self.origin.origin().ascii_serialization()
    }

    pub fn console_url(&self) -> String {
        format!("{}/console", self.origin_text())
    }

    fn webauthn(&self) -> Result<Webauthn, Refusal> {
        WebauthnBuilder::new(&self.rp_id, &self.origin)
            .and_then(|builder| builder.rp_name("self-host").build())
            .map_err(|error| {
                Refusal::internal(format!(
                    "Passkeys are unavailable for {}: {error}",
                    self.host
                ))
            })
    }
}

pub fn canonical_for(dns_suffix: &str, https_port: u16) -> Option<Canonical> {
    let host = format!("console.{dns_suffix}");
    let origin = if https_port == 443 {
        format!("https://{host}")
    } else {
        format!("https://{host}:{https_port}")
    };
    Some(Canonical {
        origin: Url::parse(&origin).ok()?,
        rp_id: host.clone(),
        host,
    })
}

fn canonical_at(origin: &str) -> Option<Canonical> {
    let origin = Url::parse(origin).ok()?;
    let host = origin.host_str()?.to_ascii_lowercase();
    (origin.scheme() == "https").then(|| Canonical {
        rp_id: host.clone(),
        host,
        origin,
    })
}

pub async fn canonical<S: StateStore>(store: &S) -> Option<Canonical> {
    if let Ok(origin) = std::env::var(CONSOLE_ORIGIN_ENV)
        && !origin.is_empty()
    {
        return canonical_at(&origin);
    }
    let suffix = store.get_state("dns_suffix").await.ok().flatten()?;
    let port = std::env::var("SELF_HOST_PROXY_HTTPS_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(443);
    canonical_for(&suffix, port)
}

pub(crate) struct Refusal {
    status: StatusCode,
    message: String,
}

impl Refusal {
    pub(crate) fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Refusal {
            status,
            message: message.into(),
        }
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Refusal::new(StatusCode::FORBIDDEN, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Refusal::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        (self.status, Json(ErrorReport::plain(self.message))).into_response()
    }
}

type Answer = Result<Response, Refusal>;

fn random_hex() -> String {
    let bytes: [u8; 32] = rand::rng().random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn digest(secret: &str) -> String {
    format!("{:x}", Sha256::digest(secret.as_bytes()))
}

pub fn fingerprint(purpose: &str, request: &impl Serialize) -> String {
    let body = serde_json::to_string(&(purpose, request)).unwrap_or_default();
    digest(&body)
}

async fn load<S: StateStore>(store: &S) -> Result<Option<Account>, Refusal> {
    let body = store
        .get_state(ACCOUNT_STATE)
        .await
        .map_err(|e| Refusal::internal(e.to_string()))?;
    match body.filter(|body| !body.is_empty()) {
        None => Ok(None),
        Some(body) => serde_json::from_str(&body)
            .map(Some)
            .map_err(|e| Refusal::internal(format!("could not read the Operator account: {e}"))),
    }
}

async fn save<S: StateStore>(store: &S, account: &Account) -> Result<(), Refusal> {
    let body = serde_json::to_string(account).map_err(|e| Refusal::internal(e.to_string()))?;
    store
        .store_state(ACCOUNT_STATE, &body)
        .await
        .map_err(|e| Refusal::internal(e.to_string()))
}

fn host_of(headers: &HeaderMap, uri: &Uri) -> Option<String> {
    let raw = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| uri.host())?;
    Some(raw.split(':').next().unwrap_or(raw).to_ascii_lowercase())
}

fn origin_of(headers: &HeaderMap) -> Option<&str> {
    headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
}

fn on_canonical(headers: &HeaderMap, uri: &Uri, canonical: &Canonical) -> Result<(), Refusal> {
    if host_of(headers, uri).as_deref() != Some(canonical.host.as_str()) {
        return Err(Refusal::forbidden(format!(
            "Passkeys work only at {}. Open that address and try again.",
            canonical.console_url()
        )));
    }
    if origin_of(headers).is_some_and(|origin| origin != canonical.origin_text()) {
        return Err(Refusal::forbidden(format!(
            "This request came from another site. Passkeys work only at {}.",
            canonical.console_url()
        )));
    }
    Ok(())
}

async fn canonical_or_fail<S: StateStore>(store: &S) -> Result<Canonical, Refusal> {
    canonical(store).await.ok_or_else(|| {
        Refusal::new(
            StatusCode::PRECONDITION_FAILED,
            "The Platform has no DNS Suffix yet. Run 'self-host init' first.",
        )
    })
}

fn void_message(account: &Account, canonical: &Canonical) -> String {
    format!(
        "The passkeys were registered for {}, and the console is now {}. A DNS Suffix change voids every passkey: run 'self-host account recover' on the Host.",
        account.rp_id, canonical.host
    )
}

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value.to_owned())
        .filter(|value| !value.is_empty())
}

async fn find_session<S: StateStore>(store: &S, token: &str) -> Option<SessionRecord> {
    let record = SESSIONS.get(store, &digest(token)).await.ok().flatten()?;
    (record.expires_at.as_str() > crate::auth::now().as_str()).then_some(record)
}

fn session_principal(record: &SessionRecord) -> Principal {
    Principal {
        label: OPERATOR.into(),
        kind: Kind::Human,
        scopes: Scope::ALL.into_iter().collect(),
        key_id: None,
        expires_at: Some(record.expires_at.clone()),
        node: None,
        via: crate::auth::Via::Api,
        session: Some(SessionRef {
            id: record.id.clone(),
            account: record.account.clone(),
        }),
    }
}

fn state_changing(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

pub(crate) async fn authenticate_session<S: StateStore>(
    store: &S,
    headers: &HeaderMap,
    method: &Method,
) -> Result<Option<Principal>, Refusal> {
    let Some(token) = cookie_token(headers) else {
        return Ok(None);
    };
    check_origin(store, headers, state_changing(method)).await?;
    match find_session(store, &token).await {
        Some(record) => Ok(Some(session_principal(&record))),
        None => Err(Refusal::new(
            StatusCode::UNAUTHORIZED,
            "This session expired. Log in again with your passkey.",
        )),
    }
}

async fn check_origin<S: StateStore>(
    store: &S,
    headers: &HeaderMap,
    required: bool,
) -> Result<(), Refusal> {
    let expected = canonical(store).await.map(|c| c.origin_text());
    match (origin_of(headers), expected) {
        (Some(origin), Some(expected)) if origin == expected => Ok(()),
        (None, _) if !required => Ok(()),
        _ => Err(Refusal::forbidden(
            "A session works only from the console's own address. This request came from another origin.",
        )),
    }
}

pub(crate) async fn upgrade_session<S: StateStore>(
    store: &S,
    headers: &HeaderMap,
) -> Result<bool, Refusal> {
    let Some(token) = cookie_token(headers) else {
        return Ok(false);
    };
    check_origin(store, headers, true).await?;
    Ok(find_session(store, &token).await.is_some())
}

fn session_cookie(token: &str, max_age: i64) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={token}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age={max_age}"
    ))
    .expect("a hex token is a valid header value")
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|key| !key.is_empty())
}

#[derive(Serialize)]
struct PasskeySummary {
    id: String,
    name: String,
    created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_used_at: Option<String>,
}

impl From<&StoredPasskey> for PasskeySummary {
    fn from(passkey: &StoredPasskey) -> Self {
        PasskeySummary {
            id: passkey.id.clone(),
            name: passkey.name.clone(),
            created_at: passkey.created_at.clone(),
            last_used_at: passkey.last_used_at.clone(),
        }
    }
}

#[derive(Serialize)]
struct AccountResponse {
    exists: bool,
    root_key_retired: bool,
    rp_id: String,
    console_url: String,
    passkeys_void: bool,
    passkeys: Vec<PasskeySummary>,
}

pub(crate) async fn get<S: StateStore>(State(state): State<AppState<S>>) -> Answer {
    let canonical = canonical_or_fail(&state.store).await?;
    let account = load(&state.store).await?;
    Ok(Json(AccountResponse {
        exists: account.is_some(),
        root_key_retired: root_retired(&state.store).await,
        passkeys_void: account
            .as_ref()
            .is_some_and(|account| account.rp_id != canonical.rp_id),
        rp_id: account
            .as_ref()
            .map_or(canonical.rp_id.clone(), |account| account.rp_id.clone()),
        console_url: canonical.console_url(),
        passkeys: account
            .iter()
            .flat_map(|account| account.passkeys.iter().map(PasskeySummary::from))
            .collect(),
    })
    .into_response())
}

pub async fn root_retired<S: StateStore>(store: &S) -> bool {
    store
        .get_state(ROOT_RETIRED_STATE)
        .await
        .ok()
        .flatten()
        .is_some_and(|at| !at.is_empty())
}

pub async fn seed_root_key<S: StateStore>(store: &S, key: &str) {
    if key.is_empty() || root_retired(store).await {
        return;
    }
    if store.get_api_key().await.ok().flatten().is_none()
        && let Err(error) = store.store_state("api_key", key).await
    {
        tracing::warn!(%error, "could not store the Platform key");
    }
}

async fn retire_root<S: StateStore>(state: &AppState<S>) -> Result<bool, Refusal> {
    if root_retired(&state.store).await {
        return Ok(false);
    }
    let failed = |e: crate::store::StoreError| Refusal::internal(e.to_string());
    state
        .store
        .store_state(ROOT_RETIRED_STATE, &crate::auth::now())
        .await
        .map_err(failed)?;
    state
        .store
        .store_state("api_key", "")
        .await
        .map_err(failed)?;
    crate::audit::record(
        state,
        crate::audit::event(
            "retire",
            crate::audit::Subject::new("api_key", crate::audit::ADMIN, crate::audit::ADMIN),
            "completed",
            None,
            "The Platform key was retired after the first passkey login.",
        ),
    )
    .await;
    Ok(true)
}

#[derive(Serialize)]
struct StatusResponse {
    exists: bool,
    usable: bool,
    canonical: bool,
    console_url: String,
    root_key_retired: bool,
}

pub(crate) async fn status<S: StateStore>(
    State(state): State<AppState<S>>,
    headers: HeaderMap,
    uri: Uri,
) -> Answer {
    let canonical = canonical_or_fail(&state.store).await?;
    let account = load(&state.store).await?;
    Ok(Json(StatusResponse {
        exists: account.is_some(),
        usable: account
            .as_ref()
            .is_some_and(|a| a.rp_id == canonical.rp_id && !a.passkeys.is_empty()),
        canonical: host_of(&headers, &uri).as_deref() == Some(canonical.host.as_str()),
        console_url: canonical.console_url(),
        root_key_retired: root_retired(&state.store).await,
    })
    .into_response())
}

#[derive(Deserialize)]
pub(crate) struct NameRequest {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
pub struct StepUpProof {
    pub id: String,
    pub credential: PublicKeyCredential,
}

#[derive(Serialize)]
struct RegistrationStarted {
    registration: String,
    options: CreationChallengeResponse,
}

#[derive(Deserialize)]
pub(crate) struct RegistrationFinish {
    registration: String,
    credential: RegisterPublicKeyCredential,
}

fn passkey_name(name: &str) -> Result<String, Refusal> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Refusal::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Name the passkey, for example the device it lives on",
        ));
    }
    Ok(name.chars().take(64).collect())
}

async fn start_registration<S: StateStore>(
    state: &AppState<S>,
    canonical: &Canonical,
    authority: Authority,
    name: String,
) -> Answer {
    let account = load(&state.store).await?;
    let webauthn = canonical.webauthn()?;
    let (account_id, exclude) = match &account {
        Some(account) if account.rp_id == canonical.rp_id => (
            account.id.clone(),
            Some(
                account
                    .passkeys
                    .iter()
                    .map(|p| p.credential.cred_id().clone())
                    .collect(),
            ),
        ),
        Some(account) => (account.id.clone(), None),
        None => (Uuid::from_bytes(rand::rng().random()).to_string(), None),
    };
    let user = Uuid::parse_str(&account_id)
        .map_err(|_| Refusal::internal("The Operator account id is not a UUID"))?;
    let (options, registration_state) = webauthn
        .start_passkey_registration(user, OPERATOR, "Operator", exclude)
        .map_err(|error| {
            Refusal::internal(format!("Could not start the passkey registration: {error}"))
        })?;
    let id = random_hex();
    state.account.pending().registrations.insert(
        id.clone(),
        Registration {
            state: registration_state,
            authority,
            account: account_id,
            name,
            rp_id: canonical.rp_id.clone(),
            expires: Instant::now() + CEREMONY_TTL,
        },
    );
    Ok(Json(RegistrationStarted {
        registration: id,
        options,
    })
    .into_response())
}

async fn finish_registration<S: StateStore>(
    state: &AppState<S>,
    canonical: &Canonical,
    authority: Authority,
    body: RegistrationFinish,
) -> Answer {
    let registration = state
        .account
        .pending()
        .registrations
        .remove(&body.registration)
        .ok_or_else(|| {
            Refusal::forbidden("This registration expired or was already used. Start again.")
        })?;
    if registration.authority != authority || registration.rp_id != canonical.rp_id {
        return Err(Refusal::forbidden(
            "This registration belongs to another authorization. Start again.",
        ));
    }
    let credential = canonical
        .webauthn()?
        .finish_passkey_registration(&body.credential, &registration.state)
        .map_err(|error| Refusal::forbidden(format!("The passkey was not accepted: {error}")))?;
    let _write = state.account.write.lock().await;
    if let Authority::SetupKey(key) = &authority {
        let mut pending = state.account.pending();
        if pending.setup_key.as_ref().map(|(k, _)| k) != Some(key) {
            return Err(Refusal::forbidden(
                "This setup key expired or was already used.",
            ));
        }
        pending.setup_key = None;
    }
    let existing = load(&state.store).await?;
    if authority == Authority::Root && existing.is_some() {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            "The Operator account exists. Log in with a passkey to add another.",
        ));
    }
    let mut account = existing
        .filter(|account| account.id == registration.account)
        .unwrap_or_else(|| Account {
            id: registration.account.clone(),
            created_at: crate::auth::now(),
            rp_id: canonical.rp_id.clone(),
            passkeys: Vec::new(),
        });
    if account.rp_id != canonical.rp_id {
        account.rp_id = canonical.rp_id.clone();
        account.passkeys.clear();
    }
    let stored = StoredPasskey {
        id: credential
            .cred_id()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        name: registration.name,
        created_at: crate::auth::now(),
        last_used_at: None,
        credential,
    };
    let summary = PasskeySummary::from(&stored);
    account.passkeys.retain(|p| p.id != stored.id);
    account.passkeys.push(stored);
    save(&state.store, &account).await?;
    Ok((StatusCode::CREATED, Json(summary)).into_response())
}

async fn setup_authority<S: StateStore>(
    state: &AppState<S>,
    headers: &HeaderMap,
) -> Result<Authority, Refusal> {
    const REFUSED: &str = "Setting up a passkey needs the Platform key, before the first passkey, or a setup key from 'self-host account recover'";
    let key = bearer(headers).ok_or_else(|| Refusal::new(StatusCode::UNAUTHORIZED, REFUSED))?;
    if key.starts_with(SETUP_KEY_PREFIX) {
        let key = digest(key);
        let known = state
            .account
            .pending()
            .setup_key
            .as_ref()
            .is_some_and(|(stored, _)| crate::constant_time_eq(stored, &key));
        return if known {
            Ok(Authority::SetupKey(key))
        } else {
            Err(Refusal::new(
                StatusCode::UNAUTHORIZED,
                "This setup key expired or was already used. Run 'self-host account recover' again.",
            ))
        };
    }
    let root = state
        .store
        .get_api_key()
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    if root.is_empty() || !crate::constant_time_eq(key, &root) {
        return Err(Refusal::new(StatusCode::UNAUTHORIZED, REFUSED));
    }
    if load(&state.store).await?.is_some() {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            "The Operator account exists. Log in with a passkey to add another, or run 'self-host account recover' on the Host.",
        ));
    }
    Ok(Authority::Root)
}

pub(crate) async fn setup_start<S: StateStore>(
    State(state): State<AppState<S>>,
    headers: HeaderMap,
    uri: Uri,
    Json(body): Json<NameRequest>,
) -> Answer {
    let canonical = canonical_or_fail(&state.store).await?;
    on_canonical(&headers, &uri, &canonical)?;
    let authority = setup_authority(&state, &headers).await?;
    let name = passkey_name(&body.name)?;
    start_registration(&state, &canonical, authority, name).await
}

pub(crate) async fn setup_finish<S: StateStore>(
    State(state): State<AppState<S>>,
    headers: HeaderMap,
    uri: Uri,
    Json(body): Json<RegistrationFinish>,
) -> Answer {
    let canonical = canonical_or_fail(&state.store).await?;
    on_canonical(&headers, &uri, &canonical)?;
    let authority = setup_authority(&state, &headers).await?;
    finish_registration(&state, &canonical, authority, body).await
}

#[derive(Serialize)]
struct LoginStarted {
    login: String,
    options: RequestChallengeResponse,
}

#[derive(Deserialize)]
pub(crate) struct LoginFinish {
    login: String,
    credential: PublicKeyCredential,
}

#[derive(Serialize)]
struct LoggedIn {
    expires_at: String,
    retired_root_key: bool,
}

async fn usable_account<S: StateStore>(
    store: &S,
    canonical: &Canonical,
) -> Result<Account, Refusal> {
    match load(store).await? {
        None => Err(Refusal::new(
            StatusCode::CONFLICT,
            "No passkey is set up yet. Set one up with the Platform key first.",
        )),
        Some(account) if account.rp_id != canonical.rp_id || account.passkeys.is_empty() => Err(
            Refusal::new(StatusCode::CONFLICT, void_message(&account, canonical)),
        ),
        Some(account) => Ok(account),
    }
}

async fn start_assertion<S: StateStore>(
    store: &S,
    canonical: &Canonical,
) -> Result<(RequestChallengeResponse, PasskeyAuthentication), Refusal> {
    let account = usable_account(store, canonical).await?;
    let credentials: Vec<Passkey> = account
        .passkeys
        .iter()
        .map(|p| p.credential.clone())
        .collect();
    canonical
        .webauthn()?
        .start_passkey_authentication(&credentials)
        .map_err(|error| Refusal::internal(format!("Could not start the passkey check: {error}")))
}

async fn verify_assertion<S: StateStore>(
    state: &AppState<S>,
    canonical: &Canonical,
    credential: &PublicKeyCredential,
    challenge: &PasskeyAuthentication,
) -> Result<(Account, StoredPasskey), Refusal> {
    let result = canonical
        .webauthn()?
        .finish_passkey_authentication(credential, challenge)
        .map_err(|error| Refusal::forbidden(format!("The passkey was not accepted: {error}")))?;
    if !result.user_verified() {
        return Err(Refusal::forbidden(
            "The passkey did not verify you. Use a passkey that asks for your PIN, fingerprint or face.",
        ));
    }
    let _write = state.account.write.lock().await;
    let mut account = usable_account(&state.store, canonical).await?;
    let passkey = account
        .passkeys
        .iter_mut()
        .find(|p| p.credential.cred_id() == result.cred_id())
        .ok_or_else(|| Refusal::forbidden("This passkey was removed from the account."))?;
    passkey.credential.update_credential(&result);
    passkey.last_used_at = Some(crate::auth::now());
    let used = passkey.clone();
    save(&state.store, &account).await?;
    Ok((account, used))
}

pub(crate) async fn login_start<S: StateStore>(
    State(state): State<AppState<S>>,
    headers: HeaderMap,
    uri: Uri,
) -> Answer {
    let canonical = canonical_or_fail(&state.store).await?;
    on_canonical(&headers, &uri, &canonical)?;
    let (options, challenge) = start_assertion(&state.store, &canonical).await?;
    let id = random_hex();
    state.account.pending().logins.insert(
        id.clone(),
        Login {
            state: challenge,
            rp_id: canonical.rp_id.clone(),
            expires: Instant::now() + CEREMONY_TTL,
        },
    );
    Ok(Json(LoginStarted { login: id, options }).into_response())
}

pub(crate) async fn login_finish<S: StateStore>(
    State(state): State<AppState<S>>,
    headers: HeaderMap,
    uri: Uri,
    Json(body): Json<LoginFinish>,
) -> Answer {
    let canonical = canonical_or_fail(&state.store).await?;
    on_canonical(&headers, &uri, &canonical)?;
    let login = state
        .account
        .pending()
        .logins
        .remove(&body.login)
        .ok_or_else(|| {
            Refusal::forbidden("This login expired or was already used. Start again.")
        })?;
    if login.rp_id != canonical.rp_id {
        return Err(Refusal::forbidden(
            "This login was started for another address. Start again.",
        ));
    }
    let (account, passkey) =
        verify_assertion(&state, &canonical, &body.credential, &login.state).await?;
    let token = random_hex();
    let record = SessionRecord {
        id: digest(&token),
        account: account.id,
        passkey: passkey.id,
        created_at: crate::auth::now(),
        expires_at: crate::auth::hours_from_now(SESSION_HOURS),
    };
    if let Ok(sessions) = SESSIONS.list(&state.store).await {
        let now = crate::auth::now();
        for expired in sessions
            .iter()
            .filter(|s| s.expires_at.as_str() <= now.as_str())
        {
            let _ = SESSIONS.remove(&state.store, &expired.id).await;
        }
    }
    SESSIONS
        .upsert(&state.store, &record)
        .await
        .map_err(|error| Refusal::internal(error.to_string()))?;
    let retired_root_key = retire_root(&state).await?;
    let mut response = Json(LoggedIn {
        expires_at: record.expires_at,
        retired_root_key,
    })
    .into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        session_cookie(&token, SESSION_HOURS * 3600),
    );
    Ok(response)
}

pub(crate) async fn logout<S: StateStore>(
    State(state): State<AppState<S>>,
    headers: HeaderMap,
) -> Answer {
    check_origin(&state.store, &headers, true).await?;
    if let Some(token) = cookie_token(&headers) {
        SESSIONS
            .remove(&state.store, &digest(&token))
            .await
            .map_err(|error| Refusal::internal(error.to_string()))?;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, session_cookie("", 0));
    Ok(response)
}

fn session_of(principal: Option<&Principal>) -> Result<SessionRef, Refusal> {
    principal
        .and_then(|principal| principal.session.clone())
        .ok_or_else(|| {
            Refusal::forbidden(
                "This needs a passkey: log in to the console with your passkey and confirm it there. An API key is not enough.",
            )
        })
}

#[derive(Serialize)]
struct StepUpStarted {
    step_up: String,
    options: RequestChallengeResponse,
}

async fn begin_step_up<S: StateStore>(state: &AppState<S>, fingerprint: String) -> Answer {
    let session = session_of(crate::audit::principal().as_ref())?;
    let canonical = canonical_or_fail(&state.store).await?;
    let (options, challenge) = start_assertion(&state.store, &canonical).await?;
    let id = random_hex();
    state.account.pending().step_ups.insert(
        id.clone(),
        StepUp {
            state: challenge,
            session: session.id,
            fingerprint,
            rp_id: canonical.rp_id,
            expires: Instant::now() + STEP_UP_TTL,
        },
    );
    Ok(Json(StepUpStarted {
        step_up: id,
        options,
    })
    .into_response())
}

pub(crate) async fn confirm_step_up<S: StateStore>(
    state: &AppState<S>,
    proof: Option<&StepUpProof>,
    fingerprint: &str,
) -> Result<AuthorizedBy, Refusal> {
    let session = session_of(crate::audit::principal().as_ref())?;
    let proof = proof.ok_or_else(|| {
        Refusal::forbidden(
            "This needs a fresh passkey confirmation for this exact request. Start a step-up first.",
        )
    })?;
    let step_up = state
        .account
        .pending()
        .step_ups
        .remove(&proof.id)
        .ok_or_else(|| {
            Refusal::forbidden(
                "This passkey confirmation expired or was already used. Confirm again.",
            )
        })?;
    let canonical = canonical_or_fail(&state.store).await?;
    if step_up.session != session.id || step_up.rp_id != canonical.rp_id {
        return Err(Refusal::forbidden(
            "This passkey confirmation belongs to another session.",
        ));
    }
    if step_up.fingerprint != fingerprint {
        return Err(Refusal::forbidden(
            "This passkey confirmation was for a different request. Confirm again.",
        ));
    }
    let (account, passkey) =
        verify_assertion(state, &canonical, &proof.credential, &step_up.state).await?;
    Ok(AuthorizedBy {
        account: account.id,
        passkey: passkey.id,
        passkey_name: passkey.name,
    })
}

#[derive(Deserialize)]
pub(crate) struct StepUpRequest {
    purpose: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    key: Option<serde_json::Value>,
}

pub(crate) async fn step_up<S: StateStore>(
    State(state): State<AppState<S>>,
    Json(body): Json<StepUpRequest>,
) -> Answer {
    let fingerprint = match body.purpose.as_str() {
        "add_passkey" => fingerprint("add_passkey", &passkey_name(&body.name)?),
        "create_key" => {
            let key = body.key.ok_or_else(|| {
                Refusal::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Send the key request this confirmation is for",
                )
            })?;
            crate::key_plan_fingerprint(key)?
        }
        other => {
            return Err(Refusal::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("'{other}' is not something a passkey confirms"),
            ));
        }
    };
    begin_step_up(&state, fingerprint).await
}

#[derive(Deserialize)]
pub(crate) struct AddPasskey {
    #[serde(default)]
    name: String,
    #[serde(default)]
    step_up: Option<StepUpProof>,
}

pub(crate) async fn add_passkey_start<S: StateStore>(
    State(state): State<AppState<S>>,
    Json(body): Json<AddPasskey>,
) -> Answer {
    let name = passkey_name(&body.name)?;
    let session = session_of(crate::audit::principal().as_ref())?;
    confirm_step_up(
        &state,
        body.step_up.as_ref(),
        &fingerprint("add_passkey", &name),
    )
    .await?;
    let canonical = canonical_or_fail(&state.store).await?;
    start_registration(&state, &canonical, Authority::Session(session.id), name).await
}

pub(crate) async fn add_passkey_finish<S: StateStore>(
    State(state): State<AppState<S>>,
    Json(body): Json<RegistrationFinish>,
) -> Answer {
    let session = session_of(crate::audit::principal().as_ref())?;
    let canonical = canonical_or_fail(&state.store).await?;
    finish_registration(&state, &canonical, Authority::Session(session.id), body).await
}

pub(crate) async fn remove_passkey<S: StateStore>(
    State(state): State<AppState<S>>,
    Path(id): Path<String>,
) -> Answer {
    session_of(crate::audit::principal().as_ref())?;
    let _write = state.account.write.lock().await;
    let mut account = load(&state.store)
        .await?
        .ok_or_else(|| Refusal::new(StatusCode::NOT_FOUND, "There is no Operator account"))?;
    if !account.passkeys.iter().any(|p| p.id == id) {
        return Err(Refusal::new(
            StatusCode::NOT_FOUND,
            "No passkey has that id",
        ));
    }
    if account.passkeys.len() == 1 {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            "This is the account's last passkey. Add another before you remove it.",
        ));
    }
    account.passkeys.retain(|p| p.id != id);
    save(&state.store, &account).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Serialize, Deserialize)]
pub struct SetupKey {
    pub setup_key: String,
    pub expires_in_seconds: u64,
    pub console_url: String,
}

pub(crate) async fn recover<S: StateStore>(
    State(state): State<AppState<S>>,
    request: Request,
) -> Answer {
    let loopback = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .is_some_and(|ConnectInfo(peer)| peer.ip().to_canonical().is_loopback());
    if !loopback {
        return Err(Refusal::forbidden(
            "Recovery works only from the Host itself, on its loopback address.",
        ));
    }
    let expected = state
        .store
        .get_state(RECOVERY_SECRET_STATE)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let given = request
        .headers()
        .get(RECOVERY_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if expected.is_empty() || !crate::constant_time_eq(given, &expected) {
        return Err(Refusal::forbidden(
            "Recovery needs the secret in the Platform state. Run 'self-host account recover' on the Host as the account the Platform runs under.",
        ));
    }
    let canonical = canonical_or_fail(&state.store).await?;
    let key = format!("{SETUP_KEY_PREFIX}{}", random_hex());
    state.account.pending().setup_key = Some((digest(&key), Instant::now() + SETUP_KEY_TTL));
    Ok(Json(SetupKey {
        setup_key: key,
        expires_in_seconds: SETUP_KEY_TTL.as_secs(),
        console_url: canonical.console_url(),
    })
    .into_response())
}

pub async fn ensure_recovery_secret<S: StateStore>(store: &S) {
    let existing = store
        .get_state(RECOVERY_SECRET_STATE)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    if existing.is_empty()
        && let Err(error) = store
            .store_state(RECOVERY_SECRET_STATE, &random_hex())
            .await
    {
        tracing::warn!(%error, "could not prepare account recovery");
    }
}

#[cfg(test)]
pub(crate) fn expire_ceremonies(ceremonies: &Ceremonies) {
    let mut pending = ceremonies.pending.lock().unwrap();
    let past = Instant::now() - Duration::from_secs(1);
    for registration in pending.registrations.values_mut() {
        registration.expires = past;
    }
    for step_up in pending.step_ups.values_mut() {
        step_up.expires = past;
    }
    if let Some((_, at)) = pending.setup_key.as_mut() {
        *at = past;
    }
}
