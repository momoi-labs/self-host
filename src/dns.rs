//! Host-native DNS, independent of Docker and the state store.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow};
use hickory_server::Server;
use hickory_server::proto::rr::{
    LowerName, Name, RData, Record, RecordType, RrKey, TSigResponseContext,
    rdata::{A, NS, SOA},
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

use crate::host_addresses::{self, AddressPolicy};

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
    pub fn new(dns_suffix: &str, include: Vec<Ipv4Addr>) -> anyhow::Result<Self> {
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
                let address = match legacy.host_ip {
                    IpAddr::V4(ip) => ip,
                    IpAddr::V6(_) => {
                        anyhow::bail!("dns.json names an IPv6 address; IPv6 is not supported yet")
                    }
                };
                Self::new(&legacy.dns_suffix, vec![address])
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

fn cloudflare() -> ForwardConfig {
    let mut options = ResolverOpts::default();
    options.timeout = Duration::from_secs(2);
    options.attempts = 2;
    options.edns0 = true;
    ForwardConfig {
        name_servers: [Ipv4Addr::new(1, 1, 1, 1), Ipv4Addr::new(1, 0, 0, 1)]
            .into_iter()
            .map(|ip| NameServerConfig::udp_and_tcp(ip.into()))
            .collect(),
        options: Some(options),
    }
}

fn catalog(
    config: &Config,
    addresses: &[Ipv4Addr],
    upstream: ForwardConfig,
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
        records.push(Record::from_rdata(
            wildcard.clone(),
            60,
            RData::A(A(*address)),
        ));
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
    let zone = Arc::new(WildcardZone {
        origin: origin.clone().into(),
        handler: Mutex::new(local),
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
// The handler sits behind a mutex because the address records change under
// it: the 30-second scan rewrites the wildcard RRset in place, and a lookup
// that lands mid-rewrite must see the old set or the new one, never neither.
struct WildcardZone {
    origin: LowerName,
    handler: Mutex<InMemoryZoneHandler>,
}

impl WildcardZone {
    /// Replaces the wildcard A records with `addresses`. The whole RRset goes
    /// first, so a withdrawn interface stops being answered within one scan,
    /// inside the 60-second TTL a client may cache.
    async fn publish_addresses(&self, wildcard: &Name, addresses: &[Ipv4Addr]) {
        let mut handler = self.handler.lock().await;
        let key = RrKey::new(wildcard.clone().into(), RecordType::A);
        handler.records_get_mut().remove(&key);
        for address in addresses {
            handler.upsert_mut(
                Record::from_rdata(wildcard.clone(), 60, RData::A(A(*address))),
                1,
            );
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
        self.handler
            .lock()
            .await
            .lookup(name, rtype, request_info, options)
            .await
            .map_err(wildcard_error)
    }

    async fn search(
        &self,
        request: &Request,
        options: LookupOptions,
    ) -> (LookupControlFlow<AuthLookup>, Option<TSigResponseContext>) {
        let (lookup, signature) = self.handler.lock().await.search(request, options).await;
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

async fn bind(
    config: &Config,
    addresses: &[Ipv4Addr],
    address: SocketAddr,
    upstream: ForwardConfig,
) -> anyhow::Result<(Server<Catalog>, SocketAddr)> {
    let (catalog, zone, wildcard) = catalog(config, addresses, upstream)?;
    // Bind both transports before spawning either listener.
    let udp = UdpSocket::bind(address)
        .await
        .context("bind DNS UDP listener")?;
    let tcp = TcpListener::bind(udp.local_addr()?)
        .await
        .context("bind DNS TCP listener")?;
    let bound = udp.local_addr()?;
    let mut server = Server::new(catalog);
    server.register_socket(udp);
    server.register_listener(tcp, Duration::from_secs(5), 16);

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
        let mut published: Vec<Ipv4Addr> = initial;
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

    Ok((server, bound))
}

/// What the interfaces have right now: the address the default route leaves
/// through, and everything the policy publishes.
fn scan_addresses(config: &Config) -> anyhow::Result<(Ipv4Addr, Vec<Ipv4Addr>)> {
    let source = host_addresses::default_source()?;
    let interfaces = host_addresses::interfaces()?;
    let addresses = host_addresses::select(&config.host_addresses, source, &interfaces);
    Ok((source, addresses))
}

/// Where DNS listens, which is not the same question on both Hosts.
///
/// Linux binds the LAN address alone and leaves systemd-resolved's loopback
/// listener free; `CAP_NET_BIND_SERVICE` covers the privilege. macOS has no
/// capabilities and its LaunchDaemon runs as the Operator (ADR-0013), so a
/// named address below port 1024 is refused to anyone but root — `in_pcbbind`
/// applies that check only when the address is not the unspecified one. The
/// Mac therefore answers on every interface. Records still come from the
/// Host IP either way.
fn listen_address(host_ip: IpAddr) -> SocketAddr {
    #[cfg(target_os = "macos")]
    let ip = match host_ip {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(_) => IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
    };
    #[cfg(not(target_os = "macos"))]
    let ip = host_ip;
    SocketAddr::new(ip, 53)
}

#[cfg(target_os = "macos")]
const BIND_HINT: &str = "Find what already holds port 53 with 'lsof -nP -iTCP:53 -iUDP:53'";
#[cfg(not(target_os = "macos"))]
const BIND_HINT: &str = "Check port 53 conflicts; grant CAP_NET_BIND_SERVICE to the Platform";

pub async fn start(config: &Config) -> anyhow::Result<Server<Catalog>> {
    // An unattended boot may reach DNS before any interface is up, so the
    // scan is retried until there is something to serve and something to
    // listen on. Publishing an address that is not there is what #61 forbids.
    let (primary, addresses) = loop {
        match scan_addresses(config) {
            Ok((primary, addresses)) if !addresses.is_empty() => break (primary, addresses),
            Ok(_) | Err(_) => {
                tracing::warn!("waiting for the Host LAN address before starting DNS");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    };
    loop {
        match bind(
            config,
            &addresses,
            listen_address(IpAddr::from(primary)),
            cloudflare(),
        )
        .await
        {
            Ok((server, address)) => {
                tracing::info!(%address, ?addresses, suffix = %config.dns_suffix, "DNS listening on UDP and TCP");
                return Ok(server);
            }
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
                    "cannot start DNS on {}: {error:#}. {BIND_HINT}",
                    listen_address(IpAddr::from(primary))
                ));
            }
        }
    }
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
                let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
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

    async fn local_server(upstream: ForwardConfig) -> (Server<Catalog>, SocketAddr) {
        bind(
            &Config::new("home.lan", vec![]).unwrap(),
            &[Ipv4Addr::new(192, 0, 2, 10)],
            "127.0.0.1:0".parse().unwrap(),
            upstream,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn local_zone_answers_without_docker_database_or_upstream() {
        // Bound but never read: any accidental forwarding times out.
        let unavailable = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (mut server, address) =
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
        let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
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
        let (mut server, address) = local_server(upstream(&[upstream_address])).await;
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
        let (mut server, address) = local_server(upstream(&[upstream_address])).await;
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
        let (mut server, address) =
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
        let (mut server, address) = local_server(upstream(&[
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
        let (mut server, address) = bind(
            &Config::new("home.lan", vec![]).unwrap(),
            &[Ipv4Addr::new(192, 0, 2, 10), Ipv4Addr::new(192, 0, 2, 11)],
            "127.0.0.1:0".parse().unwrap(),
            upstream(&[unavailable.local_addr().unwrap()]),
        )
        .await
        .unwrap();
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

    async fn catalog_with(addresses: &[Ipv4Addr]) -> (Arc<WildcardZone>, Name) {
        let config = Config::new("home.lan", vec![]).unwrap();
        let (_, zone, wildcard) = catalog(
            &config,
            addresses,
            upstream(&[("127.0.0.1:1".parse().unwrap())]),
        )
        .unwrap();
        (zone, wildcard)
    }

    async fn wildcard_answers(zone: &WildcardZone, wildcard: &Name) -> usize {
        match ZoneHandler::lookup(
            zone,
            &wildcard.clone().into(),
            RecordType::A,
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
        let (zone, wildcard) = catalog_with(&[Ipv4Addr::new(192, 0, 2, 10)]).await;
        assert_eq!(wildcard_answers(&zone, &wildcard).await, 1);

        zone.publish_addresses(
            &wildcard,
            &[Ipv4Addr::new(192, 0, 2, 11), Ipv4Addr::new(192, 0, 2, 12)],
        )
        .await;
        assert_eq!(wildcard_answers(&zone, &wildcard).await, 2);

        zone.publish_addresses(&wildcard, &[]).await;
        assert_eq!(wildcard_answers(&zone, &wildcard).await, 0);
    }

    #[tokio::test]
    async fn bind_conflicts_fail_without_leaving_a_partial_listener() {
        let config = Config::new("home.lan", vec![]).unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = udp.local_addr().unwrap();
        let error = bind(
            &config,
            &[Ipv4Addr::new(192, 0, 2, 10)],
            address,
            cloudflare(),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::AddrInUse
        );
        drop(udp);

        let tcp = TcpListener::bind(address).await.unwrap();
        let error = bind(
            &config,
            &[Ipv4Addr::new(192, 0, 2, 10)],
            address,
            cloudflare(),
        )
        .await
        .err()
        .unwrap();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::AddrInUse
        );
        // The UDP socket bound before the TCP conflict was released.
        let _udp = UdpSocket::bind(address).await.unwrap();
        drop(tcp);
    }

    /// macOS refuses port 53 on a named address to the Operator the
    /// LaunchDaemon runs as, and allows it on the unspecified one. The
    /// address family has to survive the swap so an IPv6 Host still gets an
    /// IPv6 listener.
    #[test]
    fn the_listen_address_follows_the_hosts_privileged_bind_rules() {
        let v4 = listen_address("192.0.2.10".parse().unwrap());
        let v6 = listen_address("2001:db8::10".parse().unwrap());

        assert_eq!((v4.port(), v6.port()), (53, 53));
        assert!(v4.is_ipv4() && v6.is_ipv6());

        if cfg!(target_os = "macos") {
            assert!(v4.ip().is_unspecified() && v6.ip().is_unspecified());
        } else {
            assert_eq!(v4.ip(), "192.0.2.10".parse::<IpAddr>().unwrap());
            assert_eq!(v6.ip(), "2001:db8::10".parse::<IpAddr>().unwrap());
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
        for ip in ["0.0.0.0", "224.0.0.1"] {
            let address: Ipv4Addr = ip.parse().unwrap();
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
            vec![Ipv4Addr::new(192, 168, 1, 101)]
        );
    }

    #[test]
    fn a_legacy_ipv6_address_is_refused_until_ipv6_is_supported() {
        assert!(
            Config::from_json(br#"{"dns_suffix": "home.lan", "host_ip": "2001:db8::1"}"#).is_err()
        );
    }

    #[test]
    fn the_policy_survives_a_save_and_load() {
        let config = Config::new("home.lan", vec![Ipv4Addr::new(192, 168, 1, 101)]).unwrap();
        let json = serde_json::to_vec(&config).unwrap();
        let loaded = Config::from_json(&json).unwrap();
        assert_eq!(
            loaded.host_addresses.include,
            vec![Ipv4Addr::new(192, 168, 1, 101)]
        );
    }
}
