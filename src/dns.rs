//! Host-native DNS, independent of Docker and the state store.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow};
use hickory_server::Server;
use hickory_server::proto::rr::{
    LowerName, Name, RData, Record, RecordType, TSigResponseContext,
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

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub dns_suffix: String,
    pub host_ip: IpAddr,
}

impl Config {
    pub fn new(dns_suffix: &str, host_ip: &str) -> anyhow::Result<Self> {
        let config = Self {
            dns_suffix: dns_suffix.into(),
            host_ip: host_ip.parse()?,
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
        anyhow::ensure!(
            !self.host_ip.is_unspecified() && !self.host_ip.is_multicast(),
            "Host IP must be a unicast address"
        );
        Ok(())
    }

    pub fn load() -> anyhow::Result<Self> {
        let bytes =
            std::fs::read(config_path()).context("read dns.json; run 'self-host init' first")?;
        let config: Self = serde_json::from_slice(&bytes).context("parse dns.json")?;
        config.validate()?;
        Ok(config)
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
    crate::compose::platform_config_dir().join("dns.json")
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

fn catalog(config: &Config, upstream: ForwardConfig) -> anyhow::Result<Catalog> {
    config.validate()?;
    let origin = Name::from_ascii(format!("{}.", config.dns_suffix))?;
    let nameserver = Name::from_ascii(format!("ns.{}.", config.dns_suffix))?;
    let mailbox = Name::from_ascii(format!("hostmaster.{}.", config.dns_suffix))?;
    let mut local: InMemoryZoneHandler =
        InMemoryZoneHandler::empty(origin.clone(), ZoneType::Primary, AxfrPolicy::Deny);
    let records = [
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
        Record::from_rdata(
            Name::from_ascii(format!("*.{}.", config.dns_suffix))?,
            60,
            match config.host_ip {
                IpAddr::V4(ip) => RData::A(A(ip)),
                IpAddr::V6(ip) => RData::AAAA(AAAA(ip)),
            },
        ),
    ];
    for record in records {
        anyhow::ensure!(
            local.upsert_mut(record, 1),
            "could not build local DNS zone"
        );
    }
    let forward = ForwardZoneHandler::builder_tokio(upstream)
        .build()
        .map_err(anyhow::Error::msg)?;
    let mut catalog = Catalog::new();
    catalog.upsert(origin.into(), vec![Arc::new(WildcardZone(local))]);
    catalog.upsert(Name::root().into(), vec![Arc::new(forward)]);
    Ok(catalog)
}

// Hickory's in-memory lookup reports NXDOMAIN when only a different type
// exists at a wildcard. Every name in our wildcard zone exists: a missing
// type must be NODATA, or clients may negatively cache the working A/AAAA too.
struct WildcardZone(InMemoryZoneHandler);

#[async_trait::async_trait]
impl ZoneHandler for WildcardZone {
    fn zone_type(&self) -> ZoneType {
        self.0.zone_type()
    }
    fn axfr_policy(&self) -> AxfrPolicy {
        AxfrPolicy::Deny
    }
    fn origin(&self) -> &LowerName {
        self.0.origin()
    }

    async fn lookup(
        &self,
        name: &LowerName,
        rtype: RecordType,
        request_info: Option<&RequestInfo<'_>>,
        options: LookupOptions,
    ) -> LookupControlFlow<AuthLookup> {
        self.0
            .lookup(name, rtype, request_info, options)
            .await
            .map_err(wildcard_error)
    }

    async fn search(
        &self,
        request: &Request,
        options: LookupOptions,
    ) -> (LookupControlFlow<AuthLookup>, Option<TSigResponseContext>) {
        let (lookup, signature) = self.0.search(request, options).await;
        (lookup.map_err(wildcard_error), signature)
    }

    async fn nsec_records(
        &self,
        name: &LowerName,
        options: LookupOptions,
    ) -> LookupControlFlow<AuthLookup> {
        self.0.nsec_records(name, options).await
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
    address: SocketAddr,
    upstream: ForwardConfig,
) -> anyhow::Result<(Server<Catalog>, SocketAddr)> {
    let catalog = catalog(config, upstream)?;
    // Bind both transports before spawning either listener.
    let udp = UdpSocket::bind(address)
        .await
        .context("bind DNS UDP listener")?;
    let tcp = TcpListener::bind(udp.local_addr()?)
        .await
        .context("bind DNS TCP listener")?;
    let address = udp.local_addr()?;
    let mut server = Server::new(catalog);
    server.register_socket(udp);
    server.register_listener(tcp, Duration::from_secs(5), 16);
    Ok((server, address))
}

pub async fn start(config: &Config) -> anyhow::Result<Server<Catalog>> {
    let address = SocketAddr::new(config.host_ip, 53);
    loop {
        match bind(config, address, cloudflare()).await {
            Ok((server, _)) => {
                tracing::info!(%address, suffix = %config.dns_suffix, "DNS listening on UDP and TCP");
                return Ok(server);
            }
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::AddrNotAvailable) =>
            {
                tracing::warn!(%address, "waiting for the Host LAN address before starting DNS");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(error) => {
                return Err(anyhow!(
                    "cannot start DNS on {address}: {error:#}. Check port 53 conflicts; on Linux grant CAP_NET_BIND_SERVICE to the Platform"
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
            &Config::new("home.lan", "192.0.2.10").unwrap(),
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
                    .any(|r| &r.data == &RData::A(A(Ipv4Addr::new(203, 0, 113, 7))))
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
    async fn ipv6_host_answers_aaaa_instead_of_a() {
        let (mut server, address) = bind(
            &Config::new("home.lan", "2001:db8::1").unwrap(),
            "127.0.0.1:0".parse().unwrap(),
            cloudflare(),
        )
        .await
        .unwrap();
        let answer = exchange(
            address,
            &query("app.home.lan.", RecordType::AAAA, None),
            false,
        )
        .await;
        assert_eq!(
            &answer.answers[0].data,
            &RData::AAAA(AAAA("2001:db8::1".parse().unwrap()))
        );
        let empty = exchange(address, &query("app.home.lan.", RecordType::A, None), false).await;
        assert_eq!(empty.response_code, ResponseCode::NoError);
        assert!(empty.answers.is_empty());
        server.shutdown_gracefully().await.unwrap();
    }

    #[tokio::test]
    async fn bind_conflicts_fail_without_leaving_a_partial_listener() {
        let config = Config::new("home.lan", "192.0.2.10").unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = udp.local_addr().unwrap();
        let error = bind(&config, address, cloudflare()).await.err().unwrap();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::AddrInUse
        );
        drop(udp);

        let tcp = TcpListener::bind(address).await.unwrap();
        let error = bind(&config, address, cloudflare()).await.err().unwrap();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::AddrInUse
        );
        // The UDP socket bound before the TCP conflict was released.
        let _udp = UdpSocket::bind(address).await.unwrap();
        drop(tcp);
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
            assert!(Config::new(suffix, "192.0.2.1").is_err());
        }
        for ip in ["hostname", "0.0.0.0", "::", "224.0.0.1"] {
            assert!(Config::new("home.lan", ip).is_err());
        }
    }
}
