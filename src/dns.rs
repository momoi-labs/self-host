//! Host-native DNS, independent of Docker. The Zone it serves is the wildcard
//! plus the Records in Platform State (ADR-0025), which [`crate::dns_records`]
//! publishes into it through [`ServedZone`].

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow};
use hickory_server::Server;
use hickory_server::proto::rr::{
    LowerName, Name, RData, Record, RecordType, RrKey, TSigResponseContext,
    rdata::{A, AAAA, NS, SOA},
};
use hickory_server::resolver::config::{NameServerConfig, ResolverOpts};
use hickory_server::server::{Request, RequestInfo};
use hickory_server::store::forwarder::{ForwardConfig, ForwardZoneHandler};
use hickory_server::store::in_memory::InMemoryZoneHandler;
use hickory_server::zone_handler::{
    AuthLookup, AxfrPolicy, Catalog, LookupControlFlow, LookupError, LookupOptions, ZoneHandler,
    ZoneType,
};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::Mutex;

use crate::dns_records;
use crate::host_addresses::{self, AddressPolicy};
use crate::metrics::Metrics;

/// What DNS serves for the local zone, written by `init`. The addresses are a
/// policy, not a list: the Platform publishes what the interfaces actually
/// have, refreshed every 30 seconds (#61). A `host_ip` left by an older
/// Platform is migrated on load into `include`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub dns_suffix: String,
    #[serde(default)]
    pub host_addresses: AddressPolicy,
}

impl Config {
    pub fn new(dns_suffix: &str, include: Vec<IpAddr>) -> anyhow::Result<Self> {
        let config = Self {
            dns_suffix: dns_suffix.into(),
            host_addresses: AddressPolicy {
                include,
                exclude: Vec::new(),
            },
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        crate::bootstrap::validate_dns_suffix(&self.dns_suffix.to_ascii_lowercase())
            .map_err(anyhow::Error::msg)?;
        anyhow::ensure!(
            self.dns_suffix.split('.').all(|label| !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')),
            "invalid DNS Suffix"
        );
        Name::from_ascii(format!("*.{}.", self.dns_suffix))?;
        for address in self
            .host_addresses
            .include
            .iter()
            .chain(self.host_addresses.exclude.iter())
        {
            anyhow::ensure!(
                !address.is_unspecified() && !address.is_multicast(),
                "host address {address} is not a unicast address"
            );
        }
        Ok(())
    }

    pub fn load() -> anyhow::Result<Self> {
        let bytes =
            std::fs::read(config_path()).context("read dns.json; run 'self-host init' first")?;
        Self::from_json(&bytes)
    }

    fn from_json(bytes: &[u8]) -> anyhow::Result<Self> {
        match serde_json::from_slice::<Self>(bytes) {
            Ok(config) => {
                config.validate()?;
                Ok(config)
            }
            // An older Platform named one address in the file. That address
            // was what an unattended boot served even when the interface was
            // down, so it becomes an `include` candidate: published only when
            // it is actually up.
            Err(_) => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct SingleAddress {
                    dns_suffix: String,
                    host_ip: IpAddr,
                }
                let legacy: SingleAddress =
                    serde_json::from_slice(bytes).context("parse dns.json")?;
                Self::new(&legacy.dns_suffix, vec![legacy.host_ip])
            }
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.validate()?;
        let path = config_path();
        std::fs::create_dir_all(path.parent().expect("configuration directory"))?;
        // serve may start while init is writing. It must see a complete file.
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(temporary, path)?;
        Ok(())
    }
}

pub fn config_path() -> PathBuf {
    crate::paths::platform_config_dir().join("dns.json")
}

/// Cloudflare on both families. An IPv6-only Host has no route to the IPv4
/// pair, and the forwarder moves on to the servers it can reach.
pub(crate) const FORWARDERS: [IpAddr; 4] = [
    IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
    IpAddr::V4(Ipv4Addr::new(1, 0, 0, 1)),
    IpAddr::V6(Ipv6Addr::new(0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111)),
    IpAddr::V6(Ipv6Addr::new(0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1001)),
];

fn cloudflare() -> ForwardConfig {
    let mut options = ResolverOpts::default();
    options.timeout = Duration::from_secs(2);
    options.attempts = 2;
    options.edns0 = true;
    ForwardConfig {
        name_servers: FORWARDERS
            .into_iter()
            .map(NameServerConfig::udp_and_tcp)
            .collect(),
        options: Some(options),
    }
}

/// An A or AAAA record for `address`, whichever its family calls for.
fn address_record(name: &Name, address: IpAddr) -> Record {
    let data = match address {
        IpAddr::V4(ip) => RData::A(A(ip)),
        IpAddr::V6(ip) => RData::AAAA(AAAA(ip)),
    };
    Record::from_rdata(name.clone(), 60, data)
}

fn catalog(
    config: &Config,
    addresses: &[IpAddr],
    upstream: ForwardConfig,
    metrics: &Metrics,
) -> anyhow::Result<(Catalog, Arc<WildcardZone>, Name)> {
    config.validate()?;
    anyhow::ensure!(
        !addresses.is_empty(),
        "no LAN address to publish; connect the Host to the LAN"
    );
    let origin = Name::from_ascii(format!("{}.", config.dns_suffix))?;
    let nameserver = Name::from_ascii(format!("ns.{}.", config.dns_suffix))?;
    let mailbox = Name::from_ascii(format!("hostmaster.{}.", config.dns_suffix))?;
    let wildcard = Name::from_ascii(format!("*.{}.", config.dns_suffix))?;
    let mut local: InMemoryZoneHandler =
        InMemoryZoneHandler::empty(origin.clone(), ZoneType::Primary, AxfrPolicy::Deny);
    let mut records = vec![
        Record::from_rdata(
            origin.clone(),
            60,
            RData::SOA(SOA::new(
                nameserver.clone(),
                mailbox,
                1,
                3600,
                600,
                86400,
                60,
            )),
        ),
        Record::from_rdata(origin.clone(), 60, RData::NS(NS(nameserver))),
    ];
    for address in addresses {
        records.push(address_record(&wildcard, *address));
    }
    for record in records {
        anyhow::ensure!(
            local.upsert_mut(record, 1),
            "could not build local DNS zone"
        );
    }
    let forward = ForwardZoneHandler::builder_tokio(upstream)
        .build()
        .map_err(anyhow::Error::msg)?;
    let forward = CountingForward {
        origin: Name::root().into(),
        inner: forward,
        metrics: metrics.clone(),
    };
    let zone = Arc::new(WildcardZone {
        admin: Name::from_ascii(format!("admin.{}.", config.dns_suffix))?.into(),
        origin: origin.clone().into(),
        handler: Mutex::new(local),
        metrics: metrics.clone(),
    });
    let mut catalog = Catalog::new();
    catalog.upsert(origin.into(), vec![zone.clone()]);
    catalog.upsert(Name::root().into(), vec![Arc::new(forward)]);
    Ok((catalog, zone, wildcard))
}

// Hickory's in-memory lookup reports NXDOMAIN when only a different type
// exists at a wildcard. Every name in our wildcard zone exists: a missing
// type must be NODATA, or clients may negatively cache the working A/AAAA too.
//
// The handler sits behind a mutex because the records change under it: the
// 30-second scan rewrites the wildcard RRset in place, the Operator's Records
// come and go through the API, and a lookup that lands mid-rewrite must see
// the old set or the new one, never neither. An explicit Record at a name
// answers ahead of the wildcard, as DNS already says it should.
struct WildcardZone {
    origin: LowerName,
    /// `admin.<suffix>`, the one name with a Record that is still the Host.
    admin: LowerName,
    handler: Mutex<InMemoryZoneHandler>,
    /// Local-zone queries counted where they are answered (ADR-0020).
    metrics: Metrics,
}

impl WildcardZone {
    /// Replaces the A and AAAA records at `name` with `addresses`. Both whole
    /// RRsets go first, so a withdrawn interface stops being answered within
    /// one scan, inside the 60-second TTL a client may cache.
    async fn publish_addresses(&self, name: &Name, addresses: &[IpAddr]) {
        let mut handler = self.handler.lock().await;
        for record_type in [RecordType::A, RecordType::AAAA] {
            let key = RrKey::new(name.clone().into(), record_type);
            handler.records_get_mut().remove(&key);
        }
        for address in addresses {
            handler.upsert_mut(address_record(name, *address), 1);
        }
    }

    /// Whether `name` has Records but none of `record_type`, so the answer is
    /// NODATA and not the wildcard's. Hickory falls back to the wildcard one
    /// type at a time, which would answer AAAA for `nas`, whose A Record
    /// points at the NAS, with the Host's own IPv6 address, and send a
    /// dual-stack client to the Host. A name that exists answers for itself
    /// (RFC 4592). `admin` is the exception: its Record is the Host's, so it
    /// takes the wildcard's AAAA and follows the Host's IPv6 addresses.
    fn hides_wildcard(
        &self,
        handler: &mut InMemoryZoneHandler,
        name: &LowerName,
        record_type: RecordType,
    ) -> bool {
        if *name == self.admin {
            return false;
        }
        let mut types = handler
            .records_get_mut()
            .keys()
            .filter(|key| key.name == *name)
            .map(|key| key.record_type)
            .peekable();
        types.peek().is_some()
            && types.all(|found| found != record_type && found != RecordType::CNAME)
    }

    /// Removes every record of `record_type` at `name`; the wildcard answers
    /// for it again.
    async fn withdraw(&self, name: &Name, record_type: RecordType) {
        let key = RrKey::new(name.clone().into(), record_type);
        self.handler.lock().await.records_get_mut().remove(&key);
    }
}

/// The Zone this process serves, as the Operator's Records reach it.
pub struct ServedZone {
    origin: Name,
    zone: Arc<WildcardZone>,
}

impl ServedZone {
    fn fqdn(&self, name: &str) -> Option<Name> {
        match Name::from_ascii(format!("{name}.{}", self.origin)) {
            Ok(fqdn) => Some(fqdn),
            Err(error) => {
                tracing::warn!(name, "not a DNS name, so not served: {error}");
                None
            }
        }
    }
}

#[async_trait::async_trait]
impl dns_records::Zone for ServedZone {
    async fn addresses(&self) -> Vec<IpAddr> {
        let wildcard = self.fqdn("*").expect("the Zone origin is a DNS name");
        let mut handler = self.zone.handler.lock().await;
        let records = handler.records_get_mut();
        [RecordType::A, RecordType::AAAA]
            .into_iter()
            .filter_map(|record_type| {
                records.get(&RrKey::new(wildcard.clone().into(), record_type))
            })
            .flat_map(|set| set.records_without_rrsigs())
            .filter_map(|record| match &record.data {
                RData::A(A(address)) => Some(IpAddr::V4(*address)),
                RData::AAAA(AAAA(address)) => Some(IpAddr::V6(*address)),
                _ => None,
            })
            .collect()
    }

    async fn publish(&self, name: &str, address: Ipv4Addr) {
        if let Some(fqdn) = self.fqdn(name) {
            self.zone.publish_addresses(&fqdn, &[address.into()]).await;
        }
    }

    async fn withdraw(&self, name: &str, record_type: dns_records::RecordType) {
        if let Some(fqdn) = self.fqdn(name) {
            self.zone.withdraw(&fqdn, record_type.into()).await;
        }
    }
}

#[async_trait::async_trait]
impl ZoneHandler for WildcardZone {
    fn zone_type(&self) -> ZoneType {
        ZoneType::Primary
    }
    fn axfr_policy(&self) -> AxfrPolicy {
        AxfrPolicy::Deny
    }
    fn origin(&self) -> &LowerName {
        &self.origin
    }

    async fn lookup(
        &self,
        name: &LowerName,
        rtype: RecordType,
        request_info: Option<&RequestInfo<'_>>,
        options: LookupOptions,
    ) -> LookupControlFlow<AuthLookup> {
        let mut handler = self.handler.lock().await;
        if self.hides_wildcard(&mut handler, name, rtype) {
            return LookupControlFlow::Continue(Err(LookupError::NameExists));
        }
        handler
            .lookup(name, rtype, request_info, options)
            .await
            .map_err(wildcard_error)
    }

    async fn search(
        &self,
        request: &Request,
        options: LookupOptions,
    ) -> (LookupControlFlow<AuthLookup>, Option<TSigResponseContext>) {
        // The name the client asked for, without the wire format's trailing
        // dot: what the console would print, not what DNS encodes.
        let mut handler = self.handler.lock().await;
        if let Ok(info) = request.request_info() {
            self.metrics
                .count_dns_query(info.query.name().to_string().trim_end_matches('.'));
            if self.hides_wildcard(&mut handler, info.query.name(), info.query.query_type()) {
                return (
                    LookupControlFlow::Continue(Err(LookupError::NameExists)),
                    None,
                );
            }
        }
        let (lookup, signature) = handler.search(request, options).await;
        (lookup.map_err(wildcard_error), signature)
    }

    async fn nsec_records(
        &self,
        name: &LowerName,
        options: LookupOptions,
    ) -> LookupControlFlow<AuthLookup> {
        self.handler.lock().await.nsec_records(name, options).await
    }
}

fn wildcard_error(error: LookupError) -> LookupError {
    if error.is_nx_domain() {
        LookupError::NameExists
    } else {
        error
    }
}

/// The forwarder with a counter on it. Every query the local zone does not
/// own lands here, and the Platform counts its own DNS traffic (ADR-0020)
/// without knowing, or keeping, which outside names the LAN asked for.
struct CountingForward {
    origin: LowerName,
    inner: ForwardZoneHandler,
    metrics: Metrics,
}

#[async_trait::async_trait]
impl ZoneHandler for CountingForward {
    fn zone_type(&self) -> ZoneType {
        self.inner.zone_type()
    }

    fn axfr_policy(&self) -> AxfrPolicy {
        self.inner.axfr_policy()
    }

    fn origin(&self) -> &LowerName {
        &self.origin
    }

    async fn lookup(
        &self,
        name: &LowerName,
        rtype: RecordType,
        request_info: Option<&RequestInfo<'_>>,
        options: LookupOptions,
    ) -> LookupControlFlow<AuthLookup> {
        self.inner.lookup(name, rtype, request_info, options).await
    }

    async fn search(
        &self,
        request: &Request,
        options: LookupOptions,
    ) -> (LookupControlFlow<AuthLookup>, Option<TSigResponseContext>) {
        self.metrics.count_dns_forwarded();
        self.inner.search(request, options).await
    }

    async fn nsec_records(
        &self,
        name: &LowerName,
        options: LookupOptions,
    ) -> LookupControlFlow<AuthLookup> {
        self.inner.nsec_records(name, options).await
    }
}

/// One UDP socket and one TCP listener on the same address. Both bind
/// before either serves, so a conflict on one leaves no half listener.
async fn listen(address: SocketAddr) -> anyhow::Result<(UdpSocket, TcpListener)> {
    let udp = crate::listeners::udp(address).context("bind DNS UDP listener")?;
    let tcp = crate::listeners::tcp(udp.local_addr()?).context("bind DNS TCP listener")?;
    Ok((udp, tcp))
}

async fn bind(
    config: &Config,
    addresses: &[IpAddr],
    listeners: Vec<(UdpSocket, TcpListener)>,
    upstream: ForwardConfig,
    metrics: &Metrics,
) -> anyhow::Result<(Server<Catalog>, ServedZone)> {
    let (catalog, zone, wildcard) = catalog(config, addresses, upstream, metrics)?;
    let served = ServedZone {
        origin: wildcard.base_name(),
        zone: zone.clone(),
    };
    let mut server = Server::new(catalog);
    for (udp, tcp) in listeners {
        server.register_socket(udp);
        server.register_listener(tcp, Duration::from_secs(5), 16);
    }

    // The 30-second scan from #61's decision: local records carry a
    // 60-second TTL, so a withdrawn address stops being served inside the
    // window a client may hold it. Polling keeps one code path for every
    // Host, with no per-platform interface watcher.
    let policy = config.clone();
    let initial = addresses.to_vec();
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(30));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // `interval` ticks immediately; the scan that built the zone is
        // seconds old, so the first real check is one interval out.
        timer.tick().await;
        let mut published: Vec<IpAddr> = initial;
        loop {
            timer.tick().await;
            let scanned = scan_addresses(&policy);
            match scanned {
                Ok((_, current)) if current != published => {
                    zone.publish_addresses(&wildcard, &current).await;
                    tracing::info!(?current, "DNS published addresses changed");
                    published = current;
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!("could not scan the Host addresses: {error:#}");
                }
            }
        }
    });

    Ok((server, served))
}

/// What the interfaces have right now: the address each family's default
/// route leaves through, and everything the policy publishes.
fn scan_addresses(config: &Config) -> anyhow::Result<(Vec<IpAddr>, Vec<IpAddr>)> {
    let sources = host_addresses::default_sources();
    let interfaces = host_addresses::interfaces()?;
    let addresses = host_addresses::select(&config.host_addresses, &sources, &interfaces);
    Ok((sources, addresses))
}

/// Where DNS listens, which is not the same question on both Hosts. The
/// first address is required; the rest are best effort.
///
/// Linux binds the IPv4 LAN address alone and leaves systemd-resolved's
/// loopback listener free; `CAP_NET_BIND_SERVICE` covers the privilege.
/// macOS has no capabilities and its LaunchDaemon runs as the Operator
/// (ADR-0013), so a named address below port 1024 is refused to anyone but
/// root — `in_pcbbind` applies that check only when the address is not the
/// unspecified one. The Mac therefore answers on every interface, and its
/// resolver asks on loopback whether or not the LAN has IPv4.
///
/// IPv6 takes the unspecified address on both (#82): `in6_pcbbind` has the
/// same rule on macOS, and on Linux the IPv6 addresses a provider hands out
/// change with its prefix, where a named bind would go stale. Records still
/// come from the Host's addresses either way.
fn listen_addresses(sources: &[IpAddr]) -> Vec<SocketAddr> {
    #[cfg(target_os = "macos")]
    let ipv4 = {
        let _ = sources;
        Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
    };
    #[cfg(not(target_os = "macos"))]
    let ipv4 = sources.iter().copied().find(IpAddr::is_ipv4);
    ipv4.into_iter()
        .chain([IpAddr::V6(Ipv6Addr::UNSPECIFIED)])
        .map(|ip| SocketAddr::new(ip, 53))
        .collect()
}

#[cfg(target_os = "macos")]
const BIND_HINT: &str = "Find what already holds port 53 with 'lsof -nP -iTCP:53 -iUDP:53'";
#[cfg(not(target_os = "macos"))]
const BIND_HINT: &str = "Check port 53 conflicts; grant CAP_NET_BIND_SERVICE to the Platform";

/// Serves the Zone, and hands back the handle the Operator's Records are
/// published through once the state is open.
pub async fn start(
    config: &Config,
    metrics: &Metrics,
) -> anyhow::Result<(Server<Catalog>, ServedZone)> {
    // An unattended boot may reach DNS before any interface is up, so the
    // scan is retried until there is something to serve and something to
    // listen on. Publishing an address that is not there is what #61 forbids.
    let (sources, addresses) = loop {
        match scan_addresses(config) {
            Ok((sources, addresses)) if !addresses.is_empty() => break (sources, addresses),
            Ok(_) | Err(_) => {
                tracing::warn!("waiting for the Host LAN address before starting DNS");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    };
    let mut wanted = listen_addresses(&sources).into_iter();
    let required = wanted.next().expect("IPv6 is always a listen address");
    let first = loop {
        match listen(required).await {
            Ok(sockets) => break sockets,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::AddrNotAvailable) =>
            {
                tracing::warn!("waiting for the Host LAN address before starting DNS");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(error) => {
                return Err(anyhow!(
                    "cannot start DNS on {required}: {error:#}. {BIND_HINT}"
                ));
            }
        }
    };
    let mut listeners = vec![first];
    // IPv6 next to IPv4 is best effort: a Host whose IPv6 port 53 is taken
    // keeps the DNS it had before IPv6 was served.
    for address in wanted {
        match listen(address).await {
            Ok(sockets) => listeners.push(sockets),
            Err(error) => {
                tracing::warn!("DNS is not listening on {address}: {error:#}. {BIND_HINT}");
            }
        }
    }
    let bound: Vec<SocketAddr> = listeners
        .iter()
        .filter_map(|(udp, _)| udp.local_addr().ok())
        .collect();
    let (server, zone) = bind(config, &addresses, listeners, cloudflare(), metrics).await?;
    tracing::info!(?bound, ?addresses, suffix = %config.dns_suffix, "DNS listening on UDP and TCP");
    Ok((server, zone))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_server::proto::op::{Edns, Message, MessageType, OpCode, Query, ResponseCode};
    use hickory_server::proto::rr::{
        RecordType,
        rdata::{CNAME, TXT},
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    fn upstream(addresses: &[SocketAddr]) -> ForwardConfig {
        let name_servers = addresses
            .iter()
            .map(|address| {
                let mut config = NameServerConfig::udp_and_tcp(address.ip());
                for connection in &mut config.connections {
                    connection.port = address.port();
                }
                config
            })
            .collect();
        let mut options = ResolverOpts::default();
        options.timeout = Duration::from_millis(100);
        options.attempts = 1;
        options.edns0 = true;
        ForwardConfig {
            name_servers,
            options: Some(options),
        }
    }

    fn query(name: &str, record_type: RecordType, edns: Option<u16>) -> Message {
        let mut query = Message::new(1234, MessageType::Query, OpCode::Query);
        query.metadata.recursion_desired = true;
        query.add_query(Query::query(Name::from_ascii(name).unwrap(), record_type));
        if let Some(payload) = edns {
            let mut extension = Edns::new();
            extension.set_max_payload(payload);
            query.edns = Some(extension);
        }
        query
    }

    async fn exchange(address: SocketAddr, query: &Message, tcp: bool) -> Message {
        tokio::time::timeout(Duration::from_secs(5), async {
            let bytes = query.to_vec().unwrap();
            let response = if tcp {
                let mut stream = TcpStream::connect(address).await.unwrap();
                stream.write_u16(bytes.len() as u16).await.unwrap();
                stream.write_all(&bytes).await.unwrap();
                let size = stream.read_u16().await.unwrap();
                let mut response = vec![0; size as usize];
                stream.read_exact(&mut response).await.unwrap();
                response
            } else {
                let local: SocketAddr = if address.is_ipv6() {
                    "[::1]:0".parse().unwrap()
                } else {
                    "127.0.0.1:0".parse().unwrap()
                };
                let socket = UdpSocket::bind(local).await.unwrap();
                socket.connect(address).await.unwrap();
                socket.send(&bytes).await.unwrap();
                let mut response = vec![0; 65535];
                let length = socket.recv(&mut response).await.unwrap();
                response.truncate(length);
                response
            };
            let response = Message::from_vec(&response).unwrap();
            assert_eq!(response.id, query.id);
            response
        })
        .await
        .expect("DNS response within five seconds")
    }

    /// A server on an ephemeral loopback port of `listen`'s family.
    async fn server_on(
        listen: &str,
        addresses: &[IpAddr],
        upstream: ForwardConfig,
        metrics: &Metrics,
    ) -> (Server<Catalog>, SocketAddr, ServedZone) {
        let sockets = super::listen(listen.parse().unwrap()).await.unwrap();
        let address = sockets.0.local_addr().unwrap();
        let (server, zone) = bind(
            &Config::new("home.lan", vec![]).unwrap(),
            addresses,
            vec![sockets],
            upstream,
            metrics,
        )
        .await
        .unwrap();
        (server, address, zone)
    }

    async fn local_server(upstream: ForwardConfig) -> (Server<Catalog>, SocketAddr, ServedZone) {
        server_on(
            "127.0.0.1:0",
            &[Ipv4Addr::new(192, 0, 2, 10).into()],
            upstream,
            &Metrics::new(),
        )
        .await
    }

    /// The A answers for `name`, in address order.
    async fn a_answers(address: SocketAddr, name: &str) -> (Message, Vec<Ipv4Addr>) {
        let response = exchange(address, &query(name, RecordType::A, None), false).await;
        let mut answers: Vec<Ipv4Addr> = response
            .answers
            .iter()
            .filter_map(|record| match &record.data {
                RData::A(A(ip)) => Some(*ip),
                _ => None,
            })
            .collect();
        answers.sort();
        (response, answers)
    }

    #[tokio::test]
    async fn a_record_answers_ahead_of_the_wildcard_and_the_wildcard_takes_over_when_withdrawn() {
        use crate::dns_records::{RecordType as Type, Zone};
        let unavailable = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (mut server, address, zone) =
            local_server(upstream(&[unavailable.local_addr().unwrap()])).await;
        let nas = Ipv4Addr::new(192, 0, 2, 30);
        let host = Ipv4Addr::new(192, 0, 2, 10);

        zone.publish("nas", nas).await;

        let (response, answers) = a_answers(address, "nas.home.lan.").await;
        assert_eq!(answers, vec![nas]);
        assert!(response.authoritative);
        assert_eq!(response.answers[0].ttl, 60);
        // Case does not matter, and neither does the trailing dot a client adds.
        assert_eq!(a_answers(address, "NAS.Home.LAN.").await.1, vec![nas]);
        // Every other name is still the wildcard's, and the apex is untouched.
        assert_eq!(a_answers(address, "other.home.lan.").await.1, vec![host]);
        assert_eq!(
            a_answers(address, "media.nas.home.lan.").await.1,
            vec![host]
        );
        let apex = exchange(address, &query("home.lan.", RecordType::SOA, None), false).await;
        assert_eq!(apex.answers[0].record_type(), RecordType::SOA);
        // A type the Record does not carry falls through to the wildcard's
        // answer for it: nothing, as NODATA, never NXDOMAIN.
        let aaaa = exchange(
            address,
            &query("nas.home.lan.", RecordType::AAAA, None),
            false,
        )
        .await;
        assert_eq!(aaaa.response_code, ResponseCode::NoError);
        assert!(aaaa.answers.is_empty());

        // Publishing again replaces the value rather than adding a second one.
        let moved = Ipv4Addr::new(192, 0, 2, 31);
        zone.publish("nas", moved).await;
        assert_eq!(a_answers(address, "nas.home.lan.").await.1, vec![moved]);

        zone.withdraw("nas", Type::A).await;
        assert_eq!(a_answers(address, "nas.home.lan.").await.1, vec![host]);
        // Withdrawing what is not there is not an error.
        zone.withdraw("nas", Type::A).await;
        assert_eq!(a_answers(address, "nas.home.lan.").await.1, vec![host]);
        server.shutdown_gracefully().await.unwrap();
    }

    #[tokio::test]
    async fn bootstrap_admin_answers_explicitly_after_the_host_address_changes() {
        use crate::bootstrap::{BootstrapResult, persist_bootstrap_state};
        use crate::dns_records::Zone;
        use crate::store::FakeStateStore;
        let store = FakeStateStore::new();
        persist_bootstrap_state(
            &store,
            &BootstrapResult {
                dns_suffix: "home.lan".into(),
                api_key: "test-key".into(),
                api_listen_addr: "0.0.0.0:3721".into(),
                host_ip: "192.0.2.30".into(),
                host_addresses: vec![IpAddr::from([192, 0, 2, 30])],
                execution_unavailable: None,
            },
        )
        .await
        .unwrap();
        let unavailable = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (mut server, address, zone) =
            local_server(upstream(&[unavailable.local_addr().unwrap()])).await;
        crate::dns_records::rebuild(&store, &zone).await.unwrap();
        let (response, answers) = a_answers(address, "admin.home.lan.").await;
        assert_eq!(answers, vec![Ipv4Addr::new(192, 0, 2, 30)]);
        assert_eq!(response.answers[0].ttl, 60);
        assert_eq!(
            a_answers(address, "other.home.lan.").await.1,
            vec![Ipv4Addr::new(192, 0, 2, 10)]
        );
        assert_eq!(zone.addresses().await, vec![IpAddr::from([192, 0, 2, 10])]);
        zone.publish("*", Ipv4Addr::new(192, 0, 2, 40)).await;
        assert_eq!(zone.addresses().await, vec![IpAddr::from([192, 0, 2, 40])]);
        assert_eq!(
            a_answers(address, "admin.home.lan.").await.1,
            vec![Ipv4Addr::new(192, 0, 2, 30)]
        );
        server.shutdown_gracefully().await.unwrap();
    }

    #[tokio::test]
    async fn the_records_in_state_are_served_again_after_a_restart() {
        use crate::dns_records::{Owner, Record, RecordType as Type, TTL};
        use crate::store::FakeStateStore;
        let store = FakeStateStore::new();
        let records = vec![
            Record {
                name: "nas".into(),
                record_type: Type::A,
                value: Ipv4Addr::new(192, 0, 2, 30),
                ttl: TTL,
                description: None,
                owner: Owner::Operator,
            },
            Record {
                name: "media.nas".into(),
                record_type: Type::A,
                value: Ipv4Addr::new(192, 0, 2, 31),
                ttl: TTL,
                description: Some("the media share".into()),
                owner: Owner::Operator,
            },
        ];
        crate::collection::RECORDS
            .replace_all(&store, &records)
            .await
            .unwrap();
        let unavailable = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        // A fresh Zone, as after a restart: memory is empty, the state is not.
        let (mut server, address, zone) =
            local_server(upstream(&[unavailable.local_addr().unwrap()])).await;
        assert_eq!(
            a_answers(address, "nas.home.lan.").await.1,
            vec![Ipv4Addr::new(192, 0, 2, 10)]
        );

        assert_eq!(crate::dns_records::rebuild(&store, &zone).await.unwrap(), 2);

        assert_eq!(
            a_answers(address, "nas.home.lan.").await.1,
            vec![Ipv4Addr::new(192, 0, 2, 30)]
        );
        assert_eq!(
            a_answers(address, "media.nas.home.lan.").await.1,
            vec![Ipv4Addr::new(192, 0, 2, 31)]
        );
        assert_eq!(
            a_answers(address, "other.home.lan.").await.1,
            vec![Ipv4Addr::new(192, 0, 2, 10)]
        );
        server.shutdown_gracefully().await.unwrap();
    }

    #[tokio::test]
    async fn queries_are_counted_local_named_and_forwarded() {
        // Bound but never read: any accidental forwarding times out.
        let unavailable = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let metrics = Metrics::new();
        let (mut server, address, _) = server_on(
            "127.0.0.1:0",
            &[Ipv4Addr::new(192, 0, 2, 10).into()],
            upstream(&[unavailable.local_addr().unwrap()]),
            &metrics,
        )
        .await;

        for _ in 0..2 {
            let response =
                exchange(address, &query("app.home.lan.", RecordType::A, None), false).await;
            assert_eq!(response.response_code, ResponseCode::NoError);
        }
        // Outside the local zone: ServFail counts the query anyway.
        let failed = exchange(address, &query("example.", RecordType::A, None), false).await;
        assert_eq!(failed.response_code, ResponseCode::ServFail);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.dns.queries_total, 3);
        assert_eq!(
            snapshot.dns.by_name.len(),
            1,
            "forwarded names are not kept"
        );
        assert_eq!(snapshot.dns.by_name[0].name, "app.home.lan");
        assert_eq!(snapshot.dns.by_name[0].queries, 2);
        server.shutdown_gracefully().await.unwrap();
    }

    #[tokio::test]
    async fn local_zone_answers_without_docker_database_or_upstream() {
        // Bound but never read: any accidental forwarding times out.
        let unavailable = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (mut server, address, _) =
            local_server(upstream(&[unavailable.local_addr().unwrap()])).await;
        for tcp in [false, true] {
            for name in ["admin.home.lan.", "nested.app.home.lan.", "APP.HOME.LAN."] {
                let response =
                    exchange(address, &query(name, RecordType::A, Some(1232)), tcp).await;
                assert_eq!(response.response_code, ResponseCode::NoError);
                assert!(response.authoritative);
                assert_eq!(response.answers.len(), 1);
                assert_eq!(
                    &response.answers[0].data,
                    &RData::A(A(Ipv4Addr::new(192, 0, 2, 10)))
                );
                assert_eq!(response.answers[0].ttl, 60);
                assert!(response.edns.is_some());
            }
            for kind in [RecordType::AAAA, RecordType::TXT, RecordType::MX] {
                let response = exchange(address, &query("app.home.lan.", kind, None), tcp).await;
                assert_eq!(response.response_code, ResponseCode::NoError);
                assert!(response.answers.is_empty());
            }
            let apex = exchange(address, &query("home.lan.", RecordType::SOA, None), tcp).await;
            assert_eq!(apex.answers[0].record_type(), RecordType::SOA);
        }
        server.shutdown_gracefully().await.unwrap();
    }

    async fn external_server() -> (Server<Catalog>, SocketAddr) {
        external_server_on("127.0.0.1:0").await
    }

    async fn external_server_on(listen: &str) -> (Server<Catalog>, SocketAddr) {
        let origin = Name::from_ascii("example.").unwrap();
        let mut zone: InMemoryZoneHandler =
            InMemoryZoneHandler::empty(origin.clone(), ZoneType::Primary, AxfrPolicy::Deny);
        zone.upsert_mut(
            Record::from_rdata(
                origin.clone(),
                60,
                RData::SOA(SOA::new(
                    origin.clone(),
                    origin.clone(),
                    1,
                    3600,
                    600,
                    86400,
                    60,
                )),
            ),
            1,
        );
        zone.upsert_mut(
            Record::from_rdata(
                Name::from_ascii("www.example.").unwrap(),
                60,
                RData::A(A(Ipv4Addr::new(203, 0, 113, 7))),
            ),
            1,
        );
        zone.upsert_mut(
            Record::from_rdata(
                Name::from_ascii("alias.example.").unwrap(),
                60,
                RData::CNAME(CNAME(Name::from_ascii("www.example.").unwrap())),
            ),
            1,
        );
        for i in 0..40 {
            zone.upsert_mut(
                Record::from_rdata(
                    Name::from_ascii("large.example.").unwrap(),
                    60,
                    RData::TXT(TXT::new(vec![format!("{i:02}{}", "x".repeat(198))])),
                ),
                1,
            );
        }
        let mut catalog = Catalog::new();
        catalog.upsert(origin.into(), vec![Arc::new(zone)]);
        let udp = UdpSocket::bind(listen).await.unwrap();
        let address = udp.local_addr().unwrap();
        let tcp = TcpListener::bind(address).await.unwrap();
        let mut server = Server::new(catalog);
        server.register_socket(udp);
        server.register_listener(tcp, Duration::from_secs(1), 16);
        (server, address)
    }

    #[tokio::test]
    async fn forwards_external_names_preserving_aliases_and_negative_answers() {
        let (mut external, upstream_address) = external_server().await;
        let (mut server, address, _) = local_server(upstream(&[upstream_address])).await;
        for tcp in [false, true] {
            let answer = exchange(
                address,
                &query("alias.example.", RecordType::A, Some(1232)),
                tcp,
            )
            .await;
            assert_eq!(answer.response_code, ResponseCode::NoError);
            assert!(!answer.authoritative);
            assert!(answer.recursion_available);
            assert!(
                answer
                    .answers
                    .iter()
                    .any(|r| r.record_type() == RecordType::CNAME)
            );
            assert!(
                answer
                    .answers
                    .iter()
                    .any(|r| r.data == RData::A(A(Ipv4Addr::new(203, 0, 113, 7))))
            );
            let missing = exchange(
                address,
                &query("missing.example.", RecordType::A, None),
                tcp,
            )
            .await;
            assert_eq!(missing.response_code, ResponseCode::NXDomain);
            let empty =
                exchange(address, &query("www.example.", RecordType::AAAA, None), tcp).await;
            assert_eq!(empty.response_code, ResponseCode::NoError);
            assert!(empty.answers.is_empty());
        }
        server.shutdown_gracefully().await.unwrap();
        external.shutdown_gracefully().await.unwrap();
    }

    #[tokio::test]
    async fn retries_truncated_upstream_over_tcp_and_respects_client_udp_limits() {
        let (mut external, upstream_address) = external_server().await;
        let (mut server, address, _) = local_server(upstream(&[upstream_address])).await;
        for edns in [None, Some(1232)] {
            let query = query("large.example.", RecordType::TXT, edns);
            let udp = exchange(address, &query, false).await;
            assert!(udp.truncation);
            assert!(udp.to_vec().unwrap().len() <= edns.unwrap_or(512) as usize);
            let tcp = exchange(address, &query, true).await;
            assert!(!tcp.truncation);
            assert_eq!(tcp.answers.len(), 40);
        }
        server.shutdown_gracefully().await.unwrap();
        external.shutdown_gracefully().await.unwrap();
    }

    #[tokio::test]
    async fn upstream_failure_returns_servfail_but_local_names_still_work() {
        let unavailable = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (mut server, address, _) =
            local_server(upstream(&[unavailable.local_addr().unwrap()])).await;
        for name in ["example.", "nothome.lan.", "home.lan.example."] {
            let failed = exchange(address, &query(name, RecordType::A, None), false).await;
            assert_eq!(failed.response_code, ResponseCode::ServFail);
        }
        let local = exchange(address, &query("app.home.lan.", RecordType::A, None), false).await;
        assert_eq!(local.answers.len(), 1);
        server.shutdown_gracefully().await.unwrap();
    }

    #[tokio::test]
    async fn uses_another_upstream_when_one_is_unavailable() {
        let unavailable = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (mut external, upstream_address) = external_server().await;
        let (mut server, address, _) = local_server(upstream(&[
            unavailable.local_addr().unwrap(),
            upstream_address,
        ]))
        .await;
        let answer = exchange(address, &query("www.example.", RecordType::A, None), false).await;
        assert_eq!(answer.response_code, ResponseCode::NoError);
        assert_eq!(answer.answers.len(), 1);
        server.shutdown_gracefully().await.unwrap();
        external.shutdown_gracefully().await.unwrap();
    }

    #[tokio::test]
    async fn the_wildcard_answers_with_every_published_address() {
        let unavailable = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (mut server, address, _) = server_on(
            "127.0.0.1:0",
            &[
                Ipv4Addr::new(192, 0, 2, 10).into(),
                Ipv4Addr::new(192, 0, 2, 11).into(),
            ],
            upstream(&[unavailable.local_addr().unwrap()]),
            &Metrics::new(),
        )
        .await;
        let response = exchange(address, &query("app.home.lan.", RecordType::A, None), false).await;
        assert_eq!(response.response_code, ResponseCode::NoError);
        let mut answers: Vec<Ipv4Addr> = response
            .answers
            .iter()
            .filter_map(|record| match &record.data {
                RData::A(A(ip)) => Some(*ip),
                _ => None,
            })
            .collect();
        answers.sort();
        assert_eq!(
            answers,
            vec![Ipv4Addr::new(192, 0, 2, 10), Ipv4Addr::new(192, 0, 2, 11)]
        );
        server.shutdown_gracefully().await.unwrap();
    }

    const HOST_V4: Ipv4Addr = Ipv4Addr::new(192, 0, 2, 10);
    const HOST_V6: Ipv6Addr = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0x10);

    /// The AAAA answers for `name`.
    async fn aaaa_answers(address: SocketAddr, name: &str, tcp: bool) -> (Message, Vec<Ipv6Addr>) {
        let response = exchange(address, &query(name, RecordType::AAAA, None), tcp).await;
        let answers = response
            .answers
            .iter()
            .filter_map(|record| match &record.data {
                RData::AAAA(AAAA(ip)) => Some(*ip),
                _ => None,
            })
            .collect();
        (response, answers)
    }

    /// A dual-stack Host answers A and AAAA together, and on each family's
    /// listener, over UDP and TCP.
    #[tokio::test]
    async fn a_dual_stack_host_answers_both_families_on_both_families() {
        let unavailable = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let v4 = listen("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let v6 = listen("[::1]:0".parse().unwrap()).await.unwrap();
        let listeners = [v4.0.local_addr().unwrap(), v6.0.local_addr().unwrap()];
        let (mut server, _) = bind(
            &Config::new("home.lan", vec![]).unwrap(),
            &[HOST_V4.into(), HOST_V6.into()],
            vec![v4, v6],
            upstream(&[unavailable.local_addr().unwrap()]),
            &Metrics::new(),
        )
        .await
        .unwrap();
        for address in listeners {
            for tcp in [false, true] {
                let a = exchange(address, &query("app.home.lan.", RecordType::A, None), tcp).await;
                assert_eq!(a.answers.len(), 1);
                assert_eq!(a.answers[0].data, RData::A(A(HOST_V4)));
                let (aaaa, answers) = aaaa_answers(address, "app.home.lan.", tcp).await;
                assert!(aaaa.authoritative);
                assert_eq!(aaaa.answers[0].ttl, 60);
                assert_eq!(answers, vec![HOST_V6]);
            }
        }
        server.shutdown_gracefully().await.unwrap();
    }

    /// A Record's name answers for itself: an A Record for the NAS must not
    /// pick up the Host's AAAA from the wildcard, or a dual-stack client
    /// would reach the Host instead. `admin` is the Host, so it does.
    #[tokio::test]
    async fn a_record_hides_the_wildcards_aaaa_but_admin_follows_the_host() {
        use crate::dns_records::{RecordType as Type, Zone};
        let unavailable = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (mut server, address, zone) = server_on(
            "127.0.0.1:0",
            &[HOST_V4.into(), HOST_V6.into()],
            upstream(&[unavailable.local_addr().unwrap()]),
            &Metrics::new(),
        )
        .await;
        zone.publish("nas", Ipv4Addr::new(192, 0, 2, 30)).await;
        zone.publish("admin", HOST_V4).await;

        for tcp in [false, true] {
            let (nas, answers) = aaaa_answers(address, "nas.home.lan.", tcp).await;
            assert_eq!(nas.response_code, ResponseCode::NoError);
            assert!(answers.is_empty());
            assert_eq!(
                aaaa_answers(address, "admin.home.lan.", tcp).await.1,
                vec![HOST_V6]
            );
            assert_eq!(
                aaaa_answers(address, "other.home.lan.", tcp).await.1,
                vec![HOST_V6]
            );
        }
        assert_eq!(
            a_answers(address, "nas.home.lan.").await.1,
            vec![Ipv4Addr::new(192, 0, 2, 30)]
        );
        assert_eq!(a_answers(address, "admin.home.lan.").await.1, vec![HOST_V4]);

        // Without its Record the name is the wildcard's again, AAAA included.
        zone.withdraw("nas", Type::A).await;
        assert_eq!(
            aaaa_answers(address, "nas.home.lan.", false).await.1,
            vec![HOST_V6]
        );
        server.shutdown_gracefully().await.unwrap();
    }

    /// An IPv6-only Host reaches no IPv4 forwarder; external names still
    /// resolve through the IPv6 one, asked over IPv6.
    #[tokio::test]
    async fn external_names_resolve_through_an_ipv6_forwarder() {
        let unavailable = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (mut external, upstream_address) = external_server_on("[::1]:0").await;
        let (mut server, address, _) = server_on(
            "[::1]:0",
            &[HOST_V6.into()],
            upstream(&[unavailable.local_addr().unwrap(), upstream_address]),
            &Metrics::new(),
        )
        .await;
        for tcp in [false, true] {
            let answer = exchange(address, &query("www.example.", RecordType::A, None), tcp).await;
            assert_eq!(answer.response_code, ResponseCode::NoError);
            assert_eq!(answer.answers.len(), 1);
            assert_eq!(
                aaaa_answers(address, "app.home.lan.", tcp).await.1,
                vec![HOST_V6]
            );
        }
        server.shutdown_gracefully().await.unwrap();
        external.shutdown_gracefully().await.unwrap();
    }

    #[test]
    fn the_forwarders_cover_both_families() {
        assert!(FORWARDERS.iter().any(IpAddr::is_ipv4));
        assert!(FORWARDERS.iter().any(IpAddr::is_ipv6));
        let config = cloudflare();
        assert_eq!(config.name_servers.len(), FORWARDERS.len());
    }

    async fn catalog_with(addresses: &[IpAddr]) -> (Arc<WildcardZone>, Name) {
        let config = Config::new("home.lan", vec![]).unwrap();
        let (_, zone, wildcard) = catalog(
            &config,
            addresses,
            upstream(&[("127.0.0.1:1".parse().unwrap())]),
            &Metrics::new(),
        )
        .unwrap();
        (zone, wildcard)
    }

    async fn wildcard_answers(
        zone: &WildcardZone,
        wildcard: &Name,
        record_type: RecordType,
    ) -> usize {
        match ZoneHandler::lookup(
            zone,
            &wildcard.clone().into(),
            record_type,
            None,
            LookupOptions::default(),
        )
        .await
        {
            LookupControlFlow::Continue(Ok(answers)) => answers.iter().count(),
            LookupControlFlow::Continue(Err(_)) => 0,
            LookupControlFlow::Break(_) | LookupControlFlow::Skip => {
                panic!("unexpected lookup result")
            }
        }
    }

    #[tokio::test]
    async fn publishing_replaces_the_whole_address_set() {
        let (zone, wildcard) = catalog_with(&[Ipv4Addr::new(192, 0, 2, 10).into()]).await;
        assert_eq!(wildcard_answers(&zone, &wildcard, RecordType::A).await, 1);

        zone.publish_addresses(
            &wildcard,
            &[
                Ipv4Addr::new(192, 0, 2, 11).into(),
                Ipv4Addr::new(192, 0, 2, 12).into(),
                "2001:db8::11".parse().unwrap(),
            ],
        )
        .await;
        assert_eq!(wildcard_answers(&zone, &wildcard, RecordType::A).await, 2);
        assert_eq!(
            wildcard_answers(&zone, &wildcard, RecordType::AAAA).await,
            1
        );

        zone.publish_addresses(&wildcard, &[]).await;
        assert_eq!(wildcard_answers(&zone, &wildcard, RecordType::A).await, 0);
        assert_eq!(
            wildcard_answers(&zone, &wildcard, RecordType::AAAA).await,
            0
        );
    }

    #[tokio::test]
    async fn bind_conflicts_fail_without_leaving_a_partial_listener() {
        let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = udp.local_addr().unwrap();
        let error = listen(address).await.err().unwrap();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::AddrInUse
        );
        drop(udp);

        let tcp = TcpListener::bind(address).await.unwrap();
        let error = listen(address).await.err().unwrap();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::AddrInUse
        );
        // The UDP socket bound before the TCP conflict was released.
        let _udp = UdpSocket::bind(address).await.unwrap();
        drop(tcp);
    }

    /// macOS refuses port 53 on a named address to the Operator the
    /// LaunchDaemon runs as, and allows it on the unspecified one. IPv6 takes
    /// the unspecified address on both Hosts, after IPv4, which an
    /// IPv6-only Linux Host has none of.
    #[test]
    fn the_listen_addresses_follow_the_hosts_privileged_bind_rules() {
        let dual = listen_addresses(&[
            "192.0.2.10".parse().unwrap(),
            "2001:db8::10".parse().unwrap(),
        ]);
        let ipv6_only = listen_addresses(&["2001:db8::10".parse().unwrap()]);
        let v6: SocketAddr = "[::]:53".parse().unwrap();

        if cfg!(target_os = "macos") {
            let v4: SocketAddr = "0.0.0.0:53".parse().unwrap();
            assert_eq!(dual, vec![v4, v6]);
            assert_eq!(ipv6_only, vec![v4, v6]);
        } else {
            assert_eq!(dual, vec!["192.0.2.10:53".parse().unwrap(), v6]);
            assert_eq!(ipv6_only, vec![v6]);
        }
    }

    #[test]
    fn rejects_invalid_configuration_before_startup() {
        for suffix in [
            "home..lan",
            "*.home.lan",
            "-home.lan",
            "home.lan.",
            "home.local",
            "home.LOCAL",
        ] {
            assert!(Config::new(suffix, vec![]).is_err());
        }
        for ip in ["0.0.0.0", "224.0.0.1", "::", "ff02::1"] {
            let address: IpAddr = ip.parse().unwrap();
            assert!(Config::new("home.lan", vec![address]).is_err());
        }
    }

    #[test]
    fn a_legacy_single_address_file_migrates_into_an_include() {
        let config =
            Config::from_json(br#"{"dns_suffix": "home.lan", "host_ip": "192.168.1.101"}"#)
                .unwrap();
        assert_eq!(config.dns_suffix, "home.lan");
        assert_eq!(
            config.host_addresses.include,
            vec![IpAddr::from([192, 168, 1, 101])]
        );
    }

    #[test]
    fn a_legacy_ipv6_address_migrates_like_an_ipv4_one() {
        let config =
            Config::from_json(br#"{"dns_suffix": "home.lan", "host_ip": "2001:db8::1"}"#).unwrap();
        assert_eq!(
            config.host_addresses.include,
            vec!["2001:db8::1".parse::<IpAddr>().unwrap()]
        );
    }

    #[test]
    fn the_policy_survives_a_save_and_load() {
        let include: Vec<IpAddr> = vec![
            IpAddr::from([192, 168, 1, 101]),
            "2001:db8::101".parse().unwrap(),
        ];
        let config = Config::new("home.lan", include.clone()).unwrap();
        let json = serde_json::to_vec(&config).unwrap();
        let loaded = Config::from_json(&json).unwrap();
        assert_eq!(loaded.host_addresses.include, include);
    }
}
