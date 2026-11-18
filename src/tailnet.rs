use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct Arrival {
    pub name: String,
    pub peer: IpAddr,
    pub nodes: Arc<dyn NodeLookup>,
}

#[async_trait]
pub trait NodeLookup: Send + Sync {
    async fn node_of(&self, peer: IpAddr) -> Option<String>;
}

const WHOIS_TTL: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct WhoIs {
    cache: Mutex<HashMap<IpAddr, (Instant, Option<String>)>>,
}

#[async_trait]
impl NodeLookup for WhoIs {
    async fn node_of(&self, peer: IpAddr) -> Option<String> {
        if let Some((at, node)) = self.cache.lock().unwrap().get(&peer)
            && at.elapsed() < WHOIS_TTL
        {
            return node.clone();
        }
        let node = whois(peer).await;
        self.cache
            .lock()
            .unwrap()
            .insert(peer, (Instant::now(), node.clone()));
        node
    }
}

async fn whois(peer: IpAddr) -> Option<String> {
    #[derive(Deserialize)]
    struct Answer {
        #[serde(rename = "Node")]
        node: AnswerNode,
    }
    #[derive(Deserialize)]
    struct AnswerNode {
        #[serde(rename = "StableID")]
        stable_id: String,
    }
    let output = tailscale(&["whois", "--json", &peer.to_string()])
        .await
        .ok()?;
    serde_json::from_slice::<Answer>(&output)
        .ok()
        .map(|answer| answer.node.stable_id)
}

#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub name: Option<String>,
    pub nodes: Vec<Node>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Node {
    pub id: String,
    pub name: String,
}

#[derive(Deserialize)]
struct RawStatus {
    #[serde(rename = "Self")]
    me: Option<RawNode>,
    #[serde(rename = "Peer", default)]
    peers: HashMap<String, RawNode>,
    #[serde(rename = "CertDomains", default)]
    cert_domains: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct RawNode {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "HostName", default)]
    host_name: String,
    #[serde(rename = "DNSName", default)]
    dns_name: String,
}

pub async fn status() -> Option<Status> {
    let output = tailscale(&["status", "--json"]).await.ok()?;
    parse_status(&output)
}

fn parse_status(output: &[u8]) -> Option<Status> {
    let raw: RawStatus = serde_json::from_slice(output).ok()?;
    let me = raw.me?;
    let name = raw
        .cert_domains
        .unwrap_or_default()
        .into_iter()
        .find(|domain| domain.eq_ignore_ascii_case(me.dns_name.trim_end_matches('.')))
        .map(|domain| domain.to_ascii_lowercase());
    let node = |raw: RawNode| Node {
        name: match raw.dns_name.trim_end_matches('.').split('.').next() {
            Some(short) if !short.is_empty() => short.to_owned(),
            _ => raw.host_name,
        },
        id: raw.id,
    };
    let mut nodes: Vec<Node> = std::iter::once(me)
        .chain(raw.peers.into_values())
        .map(node)
        .collect();
    nodes.sort_by(|a, b| a.name.cmp(&b.name));
    Some(Status { name, nodes })
}

pub async fn issue_certificate(name: &str, dir: &Path) -> anyhow::Result<(String, String)> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let cert = dir.join("tailnet.crt");
    let key = dir.join("tailnet.key");
    tailscale(&[
        "cert",
        "--cert-file",
        &cert.display().to_string(),
        "--key-file",
        &key.display().to_string(),
        name,
    ])
    .await?;
    Ok((
        std::fs::read_to_string(&cert).with_context(|| format!("read {}", cert.display()))?,
        std::fs::read_to_string(&key).with_context(|| format!("read {}", key.display()))?,
    ))
}

async fn tailscale(args: &[&str]) -> anyhow::Result<Vec<u8>> {
    let output = tokio::time::timeout(
        Duration::from_secs(120),
        tokio::process::Command::new("tailscale")
            .args(args)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("tailscale did not answer in 2 minutes")?
    .context("run tailscale")?;
    if !output.status.success() {
        bail!(
            "tailscale {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_is_the_cert_domain_of_this_node() {
        let status = parse_status(
            br#"{
                "Self": {"ID": "nSelf", "HostName": "Venusaur", "DNSName": "venusaur.tail1.ts.net."},
                "Peer": {"k": {"ID": "nPeer", "HostName": "phone", "DNSName": "phone.tail1.ts.net."}},
                "CertDomains": ["venusaur.tail1.ts.net"]
            }"#,
        )
        .unwrap();
        assert_eq!(status.name.as_deref(), Some("venusaur.tail1.ts.net"));
        let names: Vec<_> = status.nodes.iter().map(|n| (&*n.id, &*n.name)).collect();
        assert_eq!(names, [("nPeer", "phone"), ("nSelf", "venusaur")]);
    }

    #[test]
    fn a_tailnet_without_https_has_no_name() {
        let status = parse_status(
            br#"{"Self": {"ID": "n", "HostName": "h", "DNSName": "h.t.ts.net."}, "CertDomains": null}"#,
        )
        .unwrap();
        assert_eq!(status.name, None);
    }
}
