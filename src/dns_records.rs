//! The Records the Operator keeps in the Zone (ADR-0025).
//!
//! A Record is one answer under the DNS Suffix: `nas` with value
//! `192.168.1.30` makes `nas.<suffix>` resolve there, ahead of the wildcard.
//! Records live in Platform State under `dns_records_v1`, keyed by name and
//! Record Type, and the served Zone is rebuilt from them on every start.
//! Everything the API changes goes to the state first and to the Zone second,
//! so a restart never serves a Record that was not saved.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::{AppState, error::ErrorReport, store::StateStore, store::StoreError};

pub const STATE_KEY: &str = "dns_records_v1";

/// Every Record the Platform serves carries this TTL, so an address change
/// reaches Consumers within a minute (CONTEXT.md: TTL).
pub const TTL: u32 = 60;

/// The DNS type of a Record. `A` is the only one the Operator can create
/// today; the Zone is keyed by name and type so others slot in later (#82).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RecordType {
    A,
}

impl RecordType {
    fn parse(input: &str) -> Result<Self, String> {
        match input.trim().to_ascii_uppercase().as_str() {
            "A" => Ok(RecordType::A),
            "" => Err("a Record Type is required; only 'A' is supported".into()),
            other => Err(format!(
                "Record Type '{other}' is not supported; only 'A' is"
            )),
        }
    }
}

impl std::fmt::Display for RecordType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecordType::A => write!(f, "A"),
        }
    }
}

impl From<RecordType> for hickory_server::proto::rr::RecordType {
    fn from(record_type: RecordType) -> Self {
        match record_type {
            RecordType::A => hickory_server::proto::rr::RecordType::A,
        }
    }
}

/// Who a Record belongs to in the Zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Owner {
    Operator,
    Platform,
    Application,
    VirtualMachine,
}

/// One answer in the Zone, as it is stored and as the API returns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// Under the DNS Suffix: `nas` answers on `nas.<suffix>`.
    pub name: String,
    #[serde(rename = "type")]
    pub record_type: RecordType,
    pub value: Ipv4Addr,
    pub ttl: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub owner: Owner,
}

impl Record {
    fn is(&self, name: &str, record_type: RecordType) -> bool {
        self.name == name && self.record_type == record_type
    }

    /// How the audit journal names a Record, which has no id of its own:
    /// `nas/A`.
    pub fn key(&self) -> String {
        key(&self.name, &self.record_type.to_string())
    }
}

/// The key `nas/A`, from parts that may not have been validated yet.
pub fn key(name: &str, record_type: &str) -> String {
    format!("{name}/{record_type}")
}

/// The name the Operator typed, made relative to the DNS Suffix.
///
/// `nas` and `nas.home.lan` name the same Record. A dotted name is read in
/// full, so `nas.example.com` is refused rather than answered as
/// `nas.example.com.home.lan`; a nested name is typed in full.
pub fn parse_name(input: &str, suffix: &str) -> Result<String, String> {
    let typed = input.trim().to_ascii_lowercase();
    let name = typed.strip_suffix('.').unwrap_or(&typed);
    if name.is_empty() {
        return Err("a Record needs a name, such as 'nas'".into());
    }
    if name == suffix {
        return Err(format!(
            "'{suffix}' is the apex of the Zone; a Record needs a name under it, such as 'nas'"
        ));
    }
    let relative = if name.contains('.') {
        name.strip_suffix(&format!(".{suffix}")).ok_or_else(|| {
            format!(
                "'{name}' is outside the DNS Suffix '{suffix}'; \
                 type a single label such as 'nas', or a full name ending in '.{suffix}'"
            )
        })?
    } else {
        name
    };
    let label_ok = |label: &str| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    if !relative.split('.').all(label_ok) {
        return Err(format!(
            "'{name}' is not a DNS name; use letters, digits and hyphens, such as 'nas'"
        ));
    }
    if relative.len() + 1 + suffix.len() > 253 {
        return Err(format!("'{name}' is too long for a DNS name"));
    }
    Ok(relative.to_owned())
}

/// The address an `A` Record answers with.
pub fn parse_value(input: &str) -> Result<Ipv4Addr, String> {
    let value = input.trim();
    let address: Ipv4Addr = value.parse().map_err(|_| {
        format!("'{value}' is not an IPv4 address; use dotted decimal, such as 192.168.1.30")
    })?;
    if address.is_unspecified() || address.is_multicast() || address.is_broadcast() {
        return Err(format!("'{value}' is not a unicast address"));
    }
    Ok(address)
}

fn parse_description(input: Option<String>) -> Result<Option<String>, String> {
    let description = input.map(|text| text.trim().to_owned()).unwrap_or_default();
    if description.chars().count() > 256 {
        return Err("a description is at most 256 characters".into());
    }
    Ok(Some(description).filter(|text| !text.is_empty()))
}

/// Publishing a Record into the Zone the Platform serves, and withdrawing
/// it again.
///
/// A trait, like [`crate::routes::RouteStore`], so the API can be tested
/// without binding port 53. Names are relative to the DNS Suffix; the Zone
/// knows its own origin.
#[async_trait]
pub trait Zone: Send + Sync + 'static {
    /// The addresses currently published at the wildcard.
    async fn addresses(&self) -> Vec<Ipv4Addr>;
    /// Answers `<name>.<suffix>` with `address` from now on, ahead of the
    /// wildcard, replacing any `A` Record already there.
    async fn publish(&self, name: &str, address: Ipv4Addr);
    /// Stops answering `<name>.<suffix>` for `record_type`; the wildcard
    /// takes the name back.
    async fn withdraw(&self, name: &str, record_type: RecordType);
}

/// A Zone kept in memory, for tests.
#[derive(Default)]
pub struct FakeZone {
    published: std::sync::Mutex<BTreeMap<(String, RecordType), Ipv4Addr>>,
}

impl FakeZone {
    pub fn new() -> Self {
        FakeZone::default()
    }

    /// What `<name>.<suffix>` answers with, or `None` when only the wildcard
    /// would.
    pub fn answer(&self, name: &str) -> Option<Ipv4Addr> {
        self.published
            .lock()
            .unwrap()
            .get(&(name.to_owned(), RecordType::A))
            .copied()
    }
}

#[async_trait]
impl Zone for FakeZone {
    async fn addresses(&self) -> Vec<Ipv4Addr> {
        self.answer("*").into_iter().collect()
    }

    async fn publish(&self, name: &str, address: Ipv4Addr) {
        self.published
            .lock()
            .unwrap()
            .insert((name.to_owned(), RecordType::A), address);
    }

    async fn withdraw(&self, name: &str, record_type: RecordType) {
        self.published
            .lock()
            .unwrap()
            .remove(&(name.to_owned(), record_type));
    }
}

/// Where Records go on a Host that serves no DNS (`serve --no-dns`): nowhere.
/// They are still kept, and served again once DNS is back.
pub struct UnservedZone;

#[async_trait]
impl Zone for UnservedZone {
    async fn addresses(&self) -> Vec<Ipv4Addr> {
        Vec::new()
    }
    async fn publish(&self, _name: &str, _address: Ipv4Addr) {}
    async fn withdraw(&self, _name: &str, _record_type: RecordType) {}
}

#[derive(Debug)]
pub enum RecordError {
    NotInitialized,
    Invalid(String),
    AlreadyExists(String, RecordType),
    ApplicationOwned {
        name: String,
        application: String,
        id: String,
    },
    MachineOwned {
        name: String,
        machine: String,
        id: String,
    },
    NotFound(String, RecordType),
    Store(StoreError),
}

impl std::fmt::Display for RecordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecordError::NotInitialized => {
                write!(f, "platform is not initialized; run 'self-host init' first")
            }
            RecordError::Invalid(message) => write!(f, "invalid Record: {message}"),
            RecordError::AlreadyExists(name, record_type) => {
                write!(f, "'{name}' already has an {record_type} Record")
            }
            RecordError::ApplicationOwned {
                name,
                application,
                id,
            } => {
                write!(
                    f,
                    "'{name}' is owned by Application '{application}' ({id}); edit the Application instead"
                )
            }
            RecordError::MachineOwned { name, machine, id } => {
                write!(
                    f,
                    "'{name}' is owned by Virtual machine '{machine}' ({id}); it goes when the machine does"
                )
            }
            RecordError::NotFound(name, record_type) => {
                write!(f, "'{name}' has no {record_type} Record")
            }
            RecordError::Store(_) => write!(f, "failed to access the Records"),
        }
    }
}

impl std::error::Error for RecordError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RecordError::Store(error) => Some(error),
            _ => None,
        }
    }
}

impl From<StoreError> for RecordError {
    fn from(error: StoreError) -> Self {
        RecordError::Store(error)
    }
}

pub async fn load<S: StateStore>(store: &S) -> Result<Vec<Record>, StoreError> {
    match store.get_state(STATE_KEY).await? {
        None => Ok(Vec::new()),
        Some(json) => serde_json::from_str(&json)
            .map_err(|e| StoreError::Serialize(format!("could not read the DNS Records: {e}"))),
    }
}

async fn save<S: StateStore>(store: &S, records: &[Record]) -> Result<(), StoreError> {
    let json = serde_json::to_string(records).map_err(|e| StoreError::Serialize(e.to_string()))?;
    store.store_state(STATE_KEY, &json).await
}

pub(crate) async fn initialize_admin(
    store: &impl StateStore,
    address: Ipv4Addr,
) -> Result<(), StoreError> {
    let mut records = load(store).await?;
    if !records
        .iter()
        .any(|record| record.is("admin", RecordType::A))
    {
        records.push(Record {
            name: "admin".into(),
            record_type: RecordType::A,
            value: address,
            ttl: TTL,
            description: None,
            owner: Owner::Platform,
        });
        save(store, &records).await?;
    }
    Ok(())
}

/// Publishes every stored Record into the Zone, and every Virtual machine's
/// last lease. Run on start, after the state is open: the Zone is memory,
/// and only the state survives a restart.
pub async fn rebuild<S: StateStore>(store: &S, zone: &dyn Zone) -> Result<usize, StoreError> {
    let records = load(store).await?;
    for record in &records {
        zone.publish(&record.name, record.value).await;
    }
    let mut published = records.len();
    // A machine's Record is the machine's hostname and lease; it lives on
    // the machine, and the next inspect corrects it if the lease moved.
    if let Some(suffix) = store.get_state("dns_suffix").await? {
        for machine in crate::environments::load(store).await? {
            if let Some(name) = machine_name(&machine.hostname, &suffix)
                && let Some(lease) = machine.lease()
            {
                zone.publish(name, lease).await;
                published += 1;
            }
        }
    }
    Ok(published)
}

/// A Virtual machine's hostname made relative to the DNS Suffix, or `None`
/// when it is not under the Suffix and so not an answer in this Zone.
fn machine_name<'a>(hostname: &'a str, suffix: &str) -> Option<&'a str> {
    hostname.strip_suffix(&format!(".{suffix}"))
}

#[derive(Deserialize)]
pub struct CreateRequest {
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "type")]
    pub record_type: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// `PUT` addresses the current key in the path. An optional name moves the
/// Record to a new key; omitting it keeps the current name.
#[derive(Deserialize)]
pub struct UpdateRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// The Operator's Records: one writer at a time, so two requests cannot both
/// read the list, each add a Record, and each write back a list missing the
/// other's.
pub(crate) struct Records {
    zone: Arc<dyn Zone>,
    write: Mutex<()>,
}

impl Records {
    /// Shares the Record writer with Application and Virtual machine name
    /// claims until their row is saved.
    pub(crate) async fn lock_namespace(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.write.lock().await
    }

    pub(crate) async fn addresses(&self) -> Vec<String> {
        self.zone
            .addresses()
            .await
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    async fn require_unowned<S: StateStore>(
        &self,
        store: &S,
        name: &str,
    ) -> Result<(), RecordError> {
        let hostname = format!("{name}.{}", self.suffix(store).await?);
        for application in store.list_applications().await? {
            if crate::routes::hostnames(&application).contains(&hostname.as_str()) {
                return Err(RecordError::ApplicationOwned {
                    name: name.into(),
                    application: application.name,
                    id: application.id,
                });
            }
        }
        for machine in crate::environments::load(store).await? {
            if machine.hostname == hostname {
                return Err(RecordError::MachineOwned {
                    name: name.into(),
                    machine: machine.config.name,
                    id: machine.id,
                });
            }
        }
        Ok(())
    }

    /// Keeps a Virtual machine's name answering with its lease, and lets the
    /// wildcard take the name back while it has none (ADR-0025). Called once
    /// the lease is saved on the machine, so a restart republishes the same.
    pub(crate) async fn follow_lease<S: StateStore>(
        &self,
        store: &S,
        hostname: &str,
        lease: Option<Ipv4Addr>,
    ) {
        let Ok(suffix) = self.suffix(store).await else {
            return;
        };
        let Some(name) = machine_name(hostname, &suffix) else {
            return;
        };
        match lease {
            Some(address) => self.zone.publish(name, address).await,
            None => self.zone.withdraw(name, RecordType::A).await,
        }
    }

    pub(crate) fn new(zone: Arc<dyn Zone>) -> Self {
        Records {
            zone,
            write: Mutex::new(()),
        }
    }

    async fn suffix<S: StateStore>(&self, store: &S) -> Result<String, RecordError> {
        store
            .get_state("dns_suffix")
            .await?
            .ok_or(RecordError::NotInitialized)
    }

    /// The key a request addresses, as the path or body typed it.
    async fn key<S: StateStore>(
        &self,
        store: &S,
        name: &str,
        record_type: &str,
    ) -> Result<(String, RecordType), RecordError> {
        let suffix = self.suffix(store).await?;
        Ok((
            parse_name(name, &suffix).map_err(RecordError::Invalid)?,
            RecordType::parse(record_type).map_err(RecordError::Invalid)?,
        ))
    }

    pub(crate) async fn create<S: StateStore>(
        &self,
        store: &S,
        request: CreateRequest,
    ) -> Result<Record, RecordError> {
        let (name, record_type) = self.key(store, &request.name, &request.record_type).await?;
        let record = Record {
            name,
            record_type,
            value: parse_value(&request.value).map_err(RecordError::Invalid)?,
            ttl: TTL,
            description: parse_description(request.description).map_err(RecordError::Invalid)?,
            owner: Owner::Operator,
        };
        let _guard = self.write.lock().await;
        self.require_unowned(store, &record.name).await?;
        let mut records = load(store).await?;
        if records
            .iter()
            .any(|existing| existing.is(&record.name, record.record_type))
        {
            return Err(RecordError::AlreadyExists(record.name, record.record_type));
        }
        records.push(record.clone());
        save(store, &records).await?;
        self.zone.publish(&record.name, record.value).await;
        Ok(record)
    }

    pub(crate) async fn update<S: StateStore>(
        &self,
        store: &S,
        name: &str,
        record_type: &str,
        request: UpdateRequest,
    ) -> Result<Record, RecordError> {
        let (name, record_type) = self.key(store, name, record_type).await?;
        let new_name = match request.name {
            Some(typed) => {
                parse_name(&typed, &self.suffix(store).await?).map_err(RecordError::Invalid)?
            }
            None => name.clone(),
        };
        let value = parse_value(&request.value).map_err(RecordError::Invalid)?;
        let description = parse_description(request.description).map_err(RecordError::Invalid)?;
        let _guard = self.write.lock().await;
        self.require_unowned(store, &name).await?;
        let mut records = load(store).await?;
        let index = records
            .iter()
            .position(|existing| existing.is(&name, record_type))
            .ok_or(RecordError::NotFound(name.clone(), record_type))?;
        if new_name != name {
            self.require_unowned(store, &new_name).await?;
            if records
                .iter()
                .any(|existing| existing.is(&new_name, record_type))
            {
                return Err(RecordError::AlreadyExists(new_name, record_type));
            }
        }
        let record = &mut records[index];
        record.name = new_name;
        record.value = value;
        record.description = description;
        let record = record.clone();
        save(store, &records).await?;
        self.zone.publish(&record.name, record.value).await;
        if name != record.name {
            self.zone.withdraw(&name, record_type).await;
        }
        Ok(record)
    }

    pub(crate) async fn delete<S: StateStore>(
        &self,
        store: &S,
        name: &str,
        record_type: &str,
    ) -> Result<(), RecordError> {
        let (name, record_type) = self.key(store, name, record_type).await?;
        let _guard = self.write.lock().await;
        self.require_unowned(store, &name).await?;
        let mut records = load(store).await?;
        let before = records.len();
        records.retain(|existing| !existing.is(&name, record_type));
        if records.len() == before {
            return Err(RecordError::NotFound(name, record_type));
        }
        save(store, &records).await?;
        self.zone.withdraw(&name, record_type).await;
        Ok(())
    }
}

#[derive(Serialize)]
struct InventoryRecord {
    #[serde(flatten)]
    record: Record,
    #[serde(skip_serializing_if = "Option::is_none")]
    application_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    virtual_machine_id: Option<String>,
}

impl Records {
    async fn inventory<S: StateStore>(
        &self,
        store: &S,
    ) -> Result<Vec<InventoryRecord>, RecordError> {
        let suffix = self.suffix(store).await?;
        let mut records: Vec<_> = load(store)
            .await?
            .into_iter()
            .map(|record| InventoryRecord {
                record,
                application_id: None,
                virtual_machine_id: None,
            })
            .collect();
        let addresses = self.zone.addresses().await;
        for value in &addresses {
            records.push(InventoryRecord {
                record: Record {
                    name: "*".into(),
                    record_type: RecordType::A,
                    value: *value,
                    ttl: TTL,
                    description: None,
                    owner: Owner::Platform,
                },
                application_id: None,
                virtual_machine_id: None,
            });
        }
        for application in store.list_applications().await? {
            for hostname in crate::routes::hostnames(&application) {
                // External routing hostnames are not answers in our Zone.
                let Some(name) = hostname.strip_suffix(&format!(".{suffix}")) else {
                    continue;
                };
                for value in &addresses {
                    records.push(InventoryRecord {
                        record: Record {
                            name: name.into(),
                            record_type: RecordType::A,
                            value: *value,
                            ttl: TTL,
                            description: None,
                            owner: Owner::Application,
                        },
                        application_id: Some(application.id.clone()),
                        virtual_machine_id: None,
                    });
                }
            }
        }
        // A machine without a lease has no Record; the wildcard answers.
        for machine in crate::environments::load(store).await? {
            if let Some(name) = machine_name(&machine.hostname, &suffix)
                && let Some(lease) = machine.lease()
            {
                records.push(InventoryRecord {
                    record: Record {
                        name: name.into(),
                        record_type: RecordType::A,
                        value: lease,
                        ttl: TTL,
                        description: None,
                        owner: Owner::VirtualMachine,
                    },
                    application_id: None,
                    virtual_machine_id: Some(machine.id),
                });
            }
        }
        records.sort_by(|a, b| {
            (&a.record.name, a.record.value).cmp(&(&b.record.name, b.record.value))
        });
        Ok(records)
    }
}

pub(crate) async fn list<S: StateStore>(State(state): State<AppState<S>>) -> Response {
    match state.dns_records.inventory(&state.store).await {
        Ok(records) => Json(records).into_response(),
        Err(error) => error_response(error),
    }
}

fn error_response(error: RecordError) -> Response {
    let status = match &error {
        RecordError::NotInitialized => StatusCode::PRECONDITION_FAILED,
        RecordError::Invalid(_) => StatusCode::BAD_REQUEST,
        RecordError::AlreadyExists(..)
        | RecordError::ApplicationOwned { .. }
        | RecordError::MachineOwned { .. } => StatusCode::CONFLICT,
        RecordError::NotFound(..) => StatusCode::NOT_FOUND,
        RecordError::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(ErrorReport::new(&error))).into_response()
}

pub(crate) async fn create<S: StateStore>(
    State(state): State<AppState<S>>,
    Json(body): Json<CreateRequest>,
) -> Response {
    match state.dns_records.create(&state.store, body).await {
        Ok(record) => (StatusCode::CREATED, Json(record)).into_response(),
        Err(error) => error_response(error),
    }
}

pub(crate) async fn update<S: StateStore>(
    State(state): State<AppState<S>>,
    Path((name, record_type)): Path<(String, String)>,
    Json(body): Json<UpdateRequest>,
) -> Response {
    match state
        .dns_records
        .update(&state.store, &name, &record_type, body)
        .await
    {
        Ok(record) => (StatusCode::OK, Json(record)).into_response(),
        Err(error) => error_response(error),
    }
}

pub(crate) async fn remove<S: StateStore>(
    State(state): State<AppState<S>>,
    Path((name, record_type)): Path<(String, String)>,
) -> Response {
    match state
        .dns_records
        .delete(&state.store, &name, &record_type)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => error_response(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        build_app_with_vm_runtime, docker::FakeDocker, environments::FakeVmRuntime,
        metrics::Metrics, routes::FakeRoutes, store::FakeStateStore,
    };
    use axum::{
        Router,
        body::{Body, to_bytes},
        http::Request,
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    #[test]
    fn a_name_is_taken_relative_to_the_suffix_or_in_full() {
        for (typed, relative) in [
            ("nas", "nas"),
            (" NAS ", "nas"),
            ("nas.home.lan", "nas"),
            ("nas.home.lan.", "nas"),
            ("media.nas.home.lan", "media.nas"),
            ("admin", "admin"),
            ("a-1", "a-1"),
        ] {
            assert_eq!(
                parse_name(typed, "home.lan").as_deref(),
                Ok(relative),
                "{typed}"
            );
        }
    }

    #[test]
    fn the_apex_a_name_outside_the_suffix_and_a_malformed_name_are_refused_by_name() {
        let apex = parse_name("home.lan", "home.lan").unwrap_err();
        assert!(apex.contains("apex"), "{apex}");
        let outside = parse_name("nas.example.com", "home.lan").unwrap_err();
        assert!(
            outside.contains("outside the DNS Suffix 'home.lan'"),
            "{outside}"
        );
        let nested = parse_name("media.nas", "home.lan").unwrap_err();
        assert!(nested.contains("ending in '.home.lan'"), "{nested}");
        for malformed in ["", "-nas", "nas-", "na s", "*", "nas_1", "nas..home.lan"] {
            let error = parse_name(malformed, "home.lan").unwrap_err();
            assert!(
                error.contains("name") || error.contains("DNS"),
                "{malformed:?}: {error}"
            );
        }
        assert!(parse_name("*.home.lan", "home.lan").is_err());
        assert!(parse_name(&"a".repeat(64), "home.lan").is_err());
        assert!(parse_name(&"a".repeat(63), "home.lan").is_ok());
        let long = format!(
            "{}.{}.{}.{}",
            "a".repeat(63),
            "b".repeat(63),
            "c".repeat(63),
            "d".repeat(60)
        );
        assert!(parse_name(&long, "home.lan").is_err());
    }

    #[test]
    fn a_value_is_an_ipv4_unicast_address() {
        assert_eq!(
            parse_value(" 192.168.1.30 "),
            Ok(Ipv4Addr::new(192, 168, 1, 30))
        );
        for bad in ["", "nas", "192.168.1", "2001:db8::1", "192.168.1.300"] {
            let error = parse_value(bad).unwrap_err();
            assert!(error.contains("IPv4"), "{bad:?}: {error}");
        }
        for bad in ["0.0.0.0", "224.0.0.1", "255.255.255.255"] {
            assert!(parse_value(bad).unwrap_err().contains("unicast"), "{bad}");
        }
    }

    #[test]
    fn only_the_a_type_is_accepted() {
        assert_eq!(RecordType::parse("A"), Ok(RecordType::A));
        assert_eq!(RecordType::parse(" a "), Ok(RecordType::A));
        assert!(RecordType::parse("AAAA").unwrap_err().contains("AAAA"));
        assert!(RecordType::parse("").unwrap_err().contains("'A'"));
    }

    #[test]
    fn a_record_is_stored_with_its_key_value_ttl_description_and_owner() {
        let record = Record {
            name: "nas".into(),
            record_type: RecordType::A,
            value: Ipv4Addr::new(192, 168, 1, 30),
            ttl: TTL,
            description: Some("the NAS".into()),
            owner: Owner::Operator,
        };
        assert_eq!(
            serde_json::to_value(&record).unwrap(),
            json!({"name": "nas", "type": "A", "value": "192.168.1.30", "ttl": 60,
                   "description": "the NAS", "owner": "operator"})
        );
        let bare: Record =
            serde_json::from_value(json!({"name": "nas", "type": "A", "value": "192.168.1.30",
                                          "ttl": 60, "owner": "operator"}))
            .unwrap();
        assert_eq!(bare.description, None);
    }

    fn stored(name: &str, value: Ipv4Addr) -> Record {
        Record {
            name: name.into(),
            record_type: RecordType::A,
            value,
            ttl: TTL,
            description: None,
            owner: Owner::Operator,
        }
    }

    #[tokio::test]
    async fn every_stored_record_is_published_again_on_start() {
        let store = FakeStateStore::new();
        let records = vec![
            stored("nas", Ipv4Addr::new(192, 168, 1, 30)),
            stored("printer", Ipv4Addr::new(192, 168, 1, 31)),
        ];
        save(&store, &records).await.unwrap();
        let zone = FakeZone::new();

        assert_eq!(rebuild(&store, &zone).await.unwrap(), 2);

        assert_eq!(zone.answer("nas"), Some(Ipv4Addr::new(192, 168, 1, 30)));
        assert_eq!(zone.answer("printer"), Some(Ipv4Addr::new(192, 168, 1, 31)));
        // A Platform that never had a Record starts with nothing to publish.
        assert_eq!(rebuild(&FakeStateStore::new(), &zone).await.unwrap(), 0);
    }

    const KEY: &str = "test-key";

    async fn setup() -> (Router, FakeStateStore, Arc<FakeZone>) {
        let store = FakeStateStore::new();
        store.store_state("api_key", KEY).await.unwrap();
        store.store_state("dns_suffix", "home.lan").await.unwrap();
        let zone = Arc::new(FakeZone::new());
        let app = build_app_with_vm_runtime(
            store.clone(),
            Arc::new(FakeDocker::new()),
            Arc::new(FakeRoutes::new()),
            Metrics::new(),
            Arc::new(FakeVmRuntime),
            zone.clone(),
        );
        (app, store, zone)
    }

    async fn call(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("authorization", format!("Bearer {KEY}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn inventory_lists_stored_records_with_their_owner() {
        let (app, _, _) = setup().await;
        let (status, created) = call(
            &app,
            "POST",
            "/dns/records",
            json!({"name": "nas", "type": "A", "value": "192.168.1.30"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let (status, inventory) = call(&app, "GET", "/dns/records", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert!(inventory.as_array().unwrap().contains(&created));
    }

    #[tokio::test]
    async fn inventory_derives_application_names_and_wildcard_from_the_served_zone() {
        let (app, _, zone) = setup().await;
        zone.publish("*", "192.168.1.10".parse().unwrap()).await;
        let (status, application) = call(
            &app,
            "POST",
            "/apps",
            json!({
                "name": "blog", "image": "nginx:alpine",
                "aliases": ["news.home.lan", "blog.home.lan", "external.example.com"]
            }),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{application}");
        let (status, inventory) = call(&app, "GET", "/dns/records", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            inventory,
            json!([
                {"name": "*", "type": "A", "value": "192.168.1.10", "ttl": 60, "owner": "platform"},
                {"name": "blog", "type": "A", "value": "192.168.1.10", "ttl": 60, "owner": "application", "application_id": application["id"]},
                {"name": "news", "type": "A", "value": "192.168.1.10", "ttl": 60, "owner": "application", "application_id": application["id"]}
            ])
        );
    }

    #[tokio::test]
    async fn application_names_cannot_be_created_edited_or_deleted_as_operator_records() {
        let (app, _, _) = setup().await;
        let (_, application) = call(
            &app,
            "POST",
            "/apps",
            json!({
                "name": "blog", "image": "nginx:alpine", "aliases": ["news.home.lan"]
            }),
        )
        .await;
        for name in ["blog", "news"] {
            for (method, path, body) in [
                (
                    "POST",
                    "/dns/records".to_string(),
                    json!({"name": format!("{name}.home.lan"), "type": "A", "value": "192.168.1.30"}),
                ),
                (
                    "PUT",
                    format!("/dns/records/{name}/A"),
                    json!({"value": "192.168.1.30"}),
                ),
                ("DELETE", format!("/dns/records/{name}/A"), Value::Null),
            ] {
                let (status, error) = call(&app, method, &path, body).await;
                assert_eq!(status, StatusCode::CONFLICT, "{method} {error}");
                let message = error["error"].as_str().unwrap();
                assert!(message.contains("Application 'blog'"), "{message}");
                assert!(
                    message.contains(application["id"].as_str().unwrap()),
                    "{message}"
                );
            }
        }
    }

    /// The namespace runs the other way too: a Virtual machine's name is
    /// refused to the Operator's Records and to Applications, naming the
    /// machine that holds it.
    #[tokio::test]
    async fn a_machine_name_cannot_be_taken_by_a_record_or_an_application() {
        let (app, _, _) = setup().await;
        let (status, machine) = call(
            &app,
            "POST",
            "/environments",
            json!({"request_id": "m-1", "config": {
                "name": "foo", "cpus": 2, "memory_gib": 4, "disk_gib": 20,
                "ssh_public_key": "", "command": "", "web_port": 0,
                "recipe": {"name": "", "dependencies": []}
            }}),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED, "{machine}");
        let id = machine["id"].as_str().unwrap();
        for (method, path, body) in [
            (
                "POST",
                "/dns/records".to_string(),
                json!({"name": "foo", "type": "A", "value": "192.168.1.30"}),
            ),
            (
                "PUT",
                "/dns/records/foo/A".to_string(),
                json!({"value": "192.168.1.30"}),
            ),
            ("DELETE", "/dns/records/foo/A".to_string(), Value::Null),
        ] {
            let (status, error) = call(&app, method, &path, body).await;
            assert_eq!(status, StatusCode::CONFLICT, "{method} {error}");
            let message = error["error"].as_str().unwrap();
            assert!(message.contains("Virtual machine 'foo'"), "{message}");
            assert!(message.contains(id), "{message}");
        }
        for body in [
            json!({"name": "foo", "image": "nginx:alpine"}),
            json!({"name": "other", "image": "nginx:alpine", "aliases": ["foo.home.lan"]}),
        ] {
            let (status, error) = call(&app, "POST", "/apps", body).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
            assert!(
                error["error"]
                    .as_str()
                    .unwrap()
                    .contains("Virtual machine 'foo'"),
                "{error}"
            );
        }
    }

    #[tokio::test]
    async fn applications_cannot_take_a_record_name_on_deploy_or_update() {
        let (app, _, _) = setup().await;
        call(
            &app,
            "POST",
            "/dns/records",
            json!({"name": "nas", "type": "A", "value": "192.168.1.30"}),
        )
        .await;
        for body in [
            json!({"name": "nas", "image": "nginx:alpine"}),
            json!({"name": "other", "image": "nginx:alpine", "hostname": "nas.home.lan"}),
            json!({"name": "other", "image": "nginx:alpine", "aliases": ["nas.home.lan"]}),
        ] {
            let (status, error) = call(&app, "POST", "/apps", body).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
            assert!(
                error["error"].as_str().unwrap().contains("Record 'nas/A'"),
                "{error}"
            );
        }
        let (_, application) = call(
            &app,
            "POST",
            "/apps",
            json!({"name": "blog", "image": "nginx:alpine"}),
        )
        .await;
        let path = format!("/apps/id/{}", application["id"].as_str().unwrap());
        for body in [
            json!({"hostname": "nas.home.lan"}),
            json!({"aliases": ["nas.home.lan"]}),
        ] {
            let (status, error) = call(&app, "PUT", &path, body).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
            assert!(
                error["error"].as_str().unwrap().contains("Record 'nas/A'"),
                "{error}"
            );
        }
        let (_, unchanged) = call(&app, "GET", &path, Value::Null).await;
        assert_eq!(unchanged["hostname"], "blog.home.lan");
        assert_eq!(unchanged["aliases"], json!([]));
        for hostname in ["home.lan", "*.home.lan"] {
            let (status, _) = call(&app, "PUT", &path, json!({"hostname": hostname})).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn init_creates_an_editable_admin_that_keeps_its_address_and_can_be_deleted() {
        let store = FakeStateStore::new();
        let result = crate::bootstrap::BootstrapResult {
            dns_suffix: "home.lan".into(),
            api_key: KEY.into(),
            api_listen_addr: "0.0.0.0:3721".into(),
            host_ip: "192.168.1.10".into(),
            host_addresses: vec!["192.168.1.10".parse().unwrap()],
            execution_unavailable: None,
        };
        crate::bootstrap::persist_bootstrap_state(&store, &result)
            .await
            .unwrap();
        let zone = Arc::new(FakeZone::new());
        zone.publish("*", "192.168.1.20".parse().unwrap()).await;
        rebuild(&store, zone.as_ref()).await.unwrap();
        let app = build_app_with_vm_runtime(
            store.clone(),
            Arc::new(FakeDocker::new()),
            Arc::new(FakeRoutes::new()),
            Metrics::new(),
            Arc::new(FakeVmRuntime),
            zone,
        );
        let (_, inventory) = call(&app, "GET", "/dns/records", Value::Null).await;
        assert!(inventory.as_array().unwrap().contains(&json!({
            "name": "admin", "type": "A", "value": "192.168.1.10", "ttl": 60, "owner": "platform"
        })));
        let (status, error) = call(
            &app,
            "POST",
            "/apps",
            json!({"name": "admin", "image": "nginx"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
        let (status, updated) = call(
            &app,
            "PUT",
            "/dns/records/admin/A",
            json!({"value": "192.168.1.30"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(updated["owner"], "platform");
        let restarted = FakeZone::new();
        rebuild(&store, &restarted).await.unwrap();
        assert_eq!(
            restarted.answer("admin"),
            Some("192.168.1.30".parse().unwrap())
        );
        let (status, _) = call(&app, "DELETE", "/dns/records/admin/A", Value::Null).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(
            crate::bootstrap::persist_bootstrap_state(&store, &result)
                .await
                .is_err()
        );
        let (_, inventory) = call(&app, "GET", "/dns/records", Value::Null).await;
        assert!(
            inventory
                .as_array()
                .unwrap()
                .iter()
                .all(|record| record["name"] != "admin")
        );
    }

    #[tokio::test]
    async fn a_late_deploy_cannot_reclaim_a_name_released_by_an_edit() {
        let (app, store, _) = setup().await;
        let pending = crate::apps::prepare_deploy_from_image(&store, "blog", "nginx", None, None)
            .await
            .unwrap();
        let path = format!("/apps/id/{}", pending.record.id);
        let (status, _) = call(&app, "PUT", &path, json!({"hostname": "news.home.lan"})).await;
        assert!(status.is_success());
        let (status, _) = call(
            &app,
            "POST",
            "/dns/records",
            json!({"name": "blog", "type": "A", "value": "192.168.1.30"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        crate::apps::finish_deploy(&store, &FakeDocker::new(), &FakeRoutes::new(), pending)
            .await
            .unwrap();
        let (_, application) = call(&app, "GET", &path, Value::Null).await;
        assert_eq!(application["hostname"], "news.home.lan");
    }

    #[tokio::test]
    async fn status_exposes_the_upstream_forwarders() {
        let (app, _, _) = setup().await;
        let (status, body) = call(&app, "GET", "/bootstrap/status", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["dns_suffix"], "home.lan");
        assert_eq!(body["forwarders"], json!(["1.1.1.1", "1.0.0.1"]));
    }

    #[tokio::test]
    async fn the_operator_creates_edits_and_deletes_a_record_and_the_zone_follows() {
        let (app, store, zone) = setup().await;

        let (status, created) = call(
            &app,
            "POST",
            "/dns/records",
            json!({"name": "nas", "type": "A", "value": "192.168.1.30", "description": "the NAS"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        assert_eq!(
            created,
            json!({"name": "nas", "type": "A", "value": "192.168.1.30", "ttl": 60,
                   "description": "the NAS", "owner": "operator"})
        );
        assert_eq!(zone.answer("nas"), Some(Ipv4Addr::new(192, 168, 1, 30)));
        assert_eq!(
            load(&store).await.unwrap(),
            vec![Record {
                description: Some("the NAS".into()),
                ..stored("nas", Ipv4Addr::new(192, 168, 1, 30))
            }]
        );

        let (status, updated) = call(
            &app,
            "PUT",
            "/dns/records/nas.home.lan/a",
            json!({"value": "192.168.1.40"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{updated}");
        assert_eq!(updated["value"], "192.168.1.40");
        assert_eq!(updated.get("description"), None);
        assert_eq!(zone.answer("nas"), Some(Ipv4Addr::new(192, 168, 1, 40)));
        assert_eq!(
            load(&store).await.unwrap()[0].value,
            Ipv4Addr::new(192, 168, 1, 40)
        );

        let (status, _) = call(&app, "DELETE", "/dns/records/nas/A", Value::Null).await;
        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(zone.answer("nas"), None);
        assert!(load(&store).await.unwrap().is_empty());

        let (status, _) = call(&app, "DELETE", "/dns/records/nas/A", Value::Null).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = call(
            &app,
            "PUT",
            "/dns/records/nas/A",
            json!({"value": "192.168.1.40"}),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn renaming_a_record_moves_its_answer_and_survives_a_rebuild() {
        let (app, store, zone) = setup().await;
        call(
            &app,
            "POST",
            "/dns/records",
            json!({"name": "nas", "type": "A", "value": "192.168.1.30"}),
        )
        .await;

        let (status, updated) = call(&app, "PUT", "/dns/records/nas/A",
            json!({"name": " STORAGE.home.lan. ", "value": "192.168.1.40", "description": "storage"})).await;
        assert_eq!(status, StatusCode::OK, "{updated}");
        assert_eq!(updated["name"], "storage");
        assert_eq!(updated["owner"], "operator");
        assert_eq!(updated["ttl"], 60);
        assert_eq!(updated["description"], "storage");
        let (_, inventory) = call(&app, "GET", "/dns/records", Value::Null).await;
        assert_eq!(inventory, json!([updated]));
        assert_eq!(zone.answer("nas"), None);
        assert_eq!(zone.answer("storage"), Some(Ipv4Addr::new(192, 168, 1, 40)));
        let restarted = FakeZone::new();
        rebuild(&store, &restarted).await.unwrap();
        assert_eq!(restarted.answer("nas"), None);
        assert_eq!(
            restarted.answer("storage"),
            Some(Ipv4Addr::new(192, 168, 1, 40))
        );
    }

    #[tokio::test]
    async fn a_rename_collision_preserves_both_records_and_application_names() {
        let (app, _, zone) = setup().await;
        for (name, value) in [("nas", "192.168.1.30"), ("storage", "192.168.1.40")] {
            call(
                &app,
                "POST",
                "/dns/records",
                json!({"name": name, "type": "A", "value": value}),
            )
            .await;
        }
        let (status, _) = call(
            &app,
            "POST",
            "/apps",
            json!({"name": "blog", "image": "nginx:alpine", "aliases": ["news.home.lan"]}),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        let (_, before) = call(&app, "GET", "/dns/records", Value::Null).await;
        for (target, reason) in [
            ("STORAGE.home.lan", "already has"),
            ("blog", "owned by Application"),
            ("news", "owned by Application"),
        ] {
            let (status, error) = call(&app, "PUT", "/dns/records/nas/A",
                json!({"name": target, "value": "192.168.1.99", "description": "must not be saved"})).await;
            assert_eq!(status, StatusCode::CONFLICT, "{target}: {error}");
            assert!(error["error"].as_str().unwrap().contains(reason), "{error}");
            assert_eq!(
                call(&app, "GET", "/dns/records", Value::Null).await.1,
                before
            );
            assert_eq!(zone.answer("nas"), Some(Ipv4Addr::new(192, 168, 1, 30)));
            assert_eq!(zone.answer("storage"), Some(Ipv4Addr::new(192, 168, 1, 40)));
        }
    }

    #[tokio::test]
    async fn a_rename_event_identifies_the_record_at_its_new_name() {
        let (app, _, _) = setup().await;
        call(
            &app,
            "POST",
            "/dns/records",
            json!({"name": "nas", "type": "A", "value": "192.168.1.30"}),
        )
        .await;
        call(
            &app,
            "PUT",
            "/dns/records/nas/A",
            json!({"name": "storage", "value": "192.168.1.30"}),
        )
        .await;
        let (_, events) = call(&app, "GET", "/events", Value::Null).await;
        let event = events
            .as_array()
            .unwrap()
            .iter()
            .find(|event| event["action"] == "configure")
            .unwrap();
        assert_eq!(event["subject"]["id"], "storage/A");
        assert_eq!(event["subject"]["name"], "storage");
        assert_eq!(event["subject"]["available"], true);
    }

    #[tokio::test]
    async fn invalid_rename_names_preserve_the_original_record() {
        let (app, _, zone) = setup().await;
        let (_, original) = call(
            &app,
            "POST",
            "/dns/records",
            json!({"name": "nas", "type": "A", "value": "192.168.1.30"}),
        )
        .await;
        for target in ["", "home.lan", "outside.example", "*", "bad name"] {
            let (status, error) = call(
                &app,
                "PUT",
                "/dns/records/nas/A",
                json!({"name": target, "value": "192.168.1.99"}),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{target}: {error}");
            assert_eq!(
                call(&app, "GET", "/dns/records", Value::Null).await.1,
                json!([original])
            );
            assert_eq!(zone.answer("nas"), Some(Ipv4Addr::new(192, 168, 1, 30)));
        }
    }

    #[tokio::test]
    async fn keeping_the_same_normalized_name_does_not_collide_with_itself() {
        let (app, _, zone) = setup().await;
        call(
            &app,
            "POST",
            "/dns/records",
            json!({"name": "nas", "type": "A", "value": "192.168.1.30"}),
        )
        .await;
        let (status, updated) = call(
            &app,
            "PUT",
            "/dns/records/nas/A",
            json!({"name": " NAS.home.lan. ", "value": "192.168.1.40"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{updated}");
        assert_eq!(updated["name"], "nas");
        assert_eq!(zone.answer("nas"), Some(Ipv4Addr::new(192, 168, 1, 40)));
        assert_eq!(
            call(&app, "GET", "/dns/records", Value::Null).await.1,
            json!([updated])
        );
    }

    #[tokio::test]
    async fn a_record_that_cannot_be_served_is_refused_with_the_reason() {
        let (app, store, zone) = setup().await;
        for (body, reason) in [
            (
                json!({"name": "home.lan", "type": "A", "value": "192.168.1.30"}),
                "apex",
            ),
            (
                json!({"name": "nas.example.com", "type": "A", "value": "192.168.1.30"}),
                "outside",
            ),
            (
                json!({"name": "na s", "type": "A", "value": "192.168.1.30"}),
                "not a DNS name",
            ),
            (
                json!({"name": "nas", "type": "AAAA", "value": "2001:db8::1"}),
                "AAAA",
            ),
            (
                json!({"name": "nas", "type": "A", "value": "2001:db8::1"}),
                "IPv4",
            ),
            (
                json!({"name": "nas", "type": "A", "value": "nowhere"}),
                "IPv4",
            ),
            (json!({"name": "nas", "type": "A"}), "IPv4"),
            (json!({"type": "A", "value": "192.168.1.30"}), "name"),
        ] {
            let (status, error) = call(&app, "POST", "/dns/records", body.clone()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {error}");
            let message = error["error"].as_str().unwrap();
            assert!(message.contains(reason), "{body}: {message}");
        }
        assert!(load(&store).await.unwrap().is_empty());
        assert_eq!(zone.answer("nas"), None);

        let (status, _) = call(
            &app,
            "POST",
            "/dns/records",
            json!({"name": "nas", "type": "A", "value": "192.168.1.30"}),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let (status, error) = call(
            &app,
            "POST",
            "/dns/records",
            json!({"name": "NAS.home.lan", "type": "a", "value": "192.168.1.31"}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(
            error["error"]
                .as_str()
                .unwrap()
                .contains("'nas' already has an A Record")
        );
        assert_eq!(zone.answer("nas"), Some(Ipv4Addr::new(192, 168, 1, 30)));
    }

    #[tokio::test]
    async fn a_platform_that_is_not_initialized_has_no_zone_to_put_a_record_in() {
        let store = FakeStateStore::new();
        store.store_state("api_key", KEY).await.unwrap();
        let app = build_app_with_vm_runtime(
            store.clone(),
            Arc::new(FakeDocker::new()),
            Arc::new(FakeRoutes::new()),
            Metrics::new(),
            Arc::new(FakeVmRuntime),
            Arc::new(FakeZone::new()),
        );
        let (status, _) = call(
            &app,
            "POST",
            "/dns/records",
            json!({"name": "nas", "type": "A", "value": "192.168.1.30"}),
        )
        .await;
        assert_eq!(status, StatusCode::PRECONDITION_FAILED);
    }

    #[tokio::test]
    async fn each_change_is_journalled_as_a_dns_record_event() {
        let (app, _, _) = setup().await;
        call(
            &app,
            "POST",
            "/dns/records",
            json!({"name": "nas", "type": "A", "value": "192.168.1.30"}),
        )
        .await;
        call(
            &app,
            "PUT",
            "/dns/records/nas/A",
            json!({"value": "192.168.1.40"}),
        )
        .await;
        call(&app, "DELETE", "/dns/records/nas/A", Value::Null).await;
        call(&app, "DELETE", "/dns/records/nas/A", Value::Null).await;

        let (status, events) = call(&app, "GET", "/events", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        let mut seen: Vec<(String, String, String, String, Option<bool>)> = events
            .as_array()
            .unwrap()
            .iter()
            .map(|event| {
                (
                    event["action"].as_str().unwrap().to_owned(),
                    event["subject"]["kind"].as_str().unwrap().to_owned(),
                    event["subject"]["name"].as_str().unwrap().to_owned(),
                    event["status"].as_str().unwrap().to_owned(),
                    event["subject"]["available"].as_bool(),
                )
            })
            .collect();
        seen.sort();
        assert_eq!(
            seen,
            vec![
                (
                    "configure".into(),
                    "dns-record".into(),
                    "nas".into(),
                    "completed".into(),
                    Some(false)
                ),
                (
                    "create".into(),
                    "dns-record".into(),
                    "nas".into(),
                    "completed".into(),
                    Some(false)
                ),
                (
                    "delete".into(),
                    "dns-record".into(),
                    "nas".into(),
                    "completed".into(),
                    Some(false)
                ),
                (
                    "delete".into(),
                    "dns-record".into(),
                    "nas".into(),
                    "failed".into(),
                    Some(false)
                ),
            ]
        );
    }

    #[tokio::test]
    async fn a_record_that_still_exists_is_available_to_its_events() {
        let (app, _, _) = setup().await;
        call(
            &app,
            "POST",
            "/dns/records",
            json!({"name": "nas.home.lan", "type": "a", "value": "192.168.1.30"}),
        )
        .await;
        let (_, events) = call(&app, "GET", "/events", Value::Null).await;
        let event = &events.as_array().unwrap()[0];
        assert_eq!(event["subject"]["id"], "nas/A");
        assert_eq!(event["subject"]["name"], "nas");
        assert_eq!(event["subject"]["available"], true);
    }
}
