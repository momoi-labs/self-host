use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, bail};
use hickory_server::proto::op::{Message, MessageType, OpCode, Query};
use hickory_server::proto::rr::{Name, RData, RecordType};
use serde_json::{Value, json};
use tokio::net::UdpSocket;

use super::DnsConfig;

const DESEC_API: &str = "https://desec.io";
const FALLBACK_TTL: u64 = 3600;

pub struct Desec {
    client: reqwest::Client,
    api: String,
    zone: String,
    token: String,
}

impl Desec {
    pub fn new(config: &DnsConfig) -> anyhow::Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()?,
            api: config
                .api_url
                .clone()
                .unwrap_or_else(|| DESEC_API.into())
                .trim_end_matches('/')
                .to_string(),
            zone: config.zone.clone(),
            token: config.token.clone(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/v1/domains/{}/{path}", self.api, self.zone)
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, self.url(path))
            .header("Authorization", format!("Token {}", self.token))
    }

    pub async fn minimum_ttl(&self) -> u64 {
        let Ok(response) = self.request(reqwest::Method::GET, "").send().await else {
            return FALLBACK_TTL;
        };
        let Ok(domain) = response.json::<Value>().await else {
            return FALLBACK_TTL;
        };
        domain["minimum_ttl"].as_u64().unwrap_or(FALLBACK_TTL)
    }

    pub async fn nameserver_addresses(&self) -> anyhow::Result<Vec<SocketAddr>> {
        let response = self
            .request(reqwest::Method::GET, "rrsets/@/NS/")
            .send()
            .await?;
        anyhow::ensure!(response.status().is_success(), "NS lookup refused");
        let rrset: Value = response.json().await?;
        let mut addresses = Vec::new();
        for name in rrset["records"].as_array().context("no NS records")? {
            let name = name.as_str().context("invalid NS record")?;
            let address = tokio::net::lookup_host((name.trim_end_matches('.'), 53))
                .await?
                .find(SocketAddr::is_ipv4)
                .with_context(|| format!("no IPv4 address for nameserver {name}"))?;
            addresses.push(address);
        }
        anyhow::ensure!(!addresses.is_empty(), "the zone has no nameservers");
        Ok(addresses)
    }

    pub async fn put_txt(&self, subname: &str, values: &[String], ttl: u64) -> anyhow::Result<()> {
        let records: Vec<String> = values.iter().map(|value| format!("\"{value}\"")).collect();
        let response = self
            .request(reqwest::Method::PUT, "rrsets/")
            .json(&json!([{
                "subname": subname,
                "type": "TXT",
                "ttl": ttl,
                "records": records,
            }]))
            .send()
            .await?;
        anyhow::ensure!(response.status().is_success(), "TXT write refused");
        Ok(())
    }

    pub async fn delete_txt(&self, subname: &str) -> anyhow::Result<()> {
        let response = self
            .request(reqwest::Method::DELETE, &format!("rrsets/{subname}/TXT/"))
            .send()
            .await?;
        anyhow::ensure!(
            response.status().is_success() || response.status() == reqwest::StatusCode::NOT_FOUND,
            "TXT delete refused"
        );
        Ok(())
    }
}

async fn serves_txt(server: SocketAddr, name: &Name, values: &[String]) -> bool {
    let attempt = async {
        let mut query = Message::new(rand::random(), MessageType::Query, OpCode::Query);
        query.metadata.recursion_desired = false;
        query.add_query(Query::query(name.clone(), RecordType::TXT));
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        socket.connect(server).await?;
        socket.send(&query.to_vec()?).await?;
        let mut buffer = vec![0; 4096];
        let length = socket.recv(&mut buffer).await?;
        let response = Message::from_vec(&buffer[..length])?;
        anyhow::ensure!(response.id == query.id, "answer to another query");
        let served: Vec<Vec<u8>> = response
            .answers
            .iter()
            .filter_map(|record| match &record.data {
                RData::TXT(txt) => Some(
                    txt.txt_data
                        .iter()
                        .map(|part| part.as_ref())
                        .collect::<Vec<_>>()
                        .concat(),
                ),
                _ => None,
            })
            .collect();
        Ok::<bool, anyhow::Error>(
            values
                .iter()
                .all(|value| served.iter().any(|txt| txt == value.as_bytes())),
        )
    };
    matches!(
        tokio::time::timeout(Duration::from_secs(3), attempt).await,
        Ok(Ok(true))
    )
}

pub async fn wait_for_txt(
    servers: &[SocketAddr],
    name: &str,
    values: &[String],
    limit: Duration,
    poll: Duration,
) -> anyhow::Result<()> {
    let name = Name::from_ascii(format!("{name}."))?;
    let deadline = tokio::time::Instant::now() + limit;
    let mut missing: Vec<SocketAddr> = servers.to_vec();
    loop {
        let mut still = Vec::new();
        for server in missing {
            if !serves_txt(server, &name, values).await {
                still.push(server);
            }
        }
        missing = still;
        if missing.is_empty() {
            return Ok(());
        }
        if tokio::time::Instant::now() + poll > deadline {
            bail!("{} nameservers never served the record", missing.len());
        }
        tokio::time::sleep(poll).await;
    }
}
