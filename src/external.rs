use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::host_addresses::LocalNetwork;
use crate::proxy::{Scheme, Upstream, parse_fingerprint};
use crate::store::ExternalDefinition;

const DOCKER_DEFAULT_BRIDGE: (Ipv4Addr, u8) = (Ipv4Addr::new(172, 17, 0, 0), 16);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidUpstream(pub String);

impl std::fmt::Display for InvalidUpstream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidUpstream {}

fn invalid(message: impl Into<String>) -> InvalidUpstream {
    InvalidUpstream(message.into())
}

pub fn is_private_lan(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(ip) => ip.is_private(),
        IpAddr::V6(ip) => (ip.segments()[0] & 0xfe00) == 0xfc00,
    }
}

pub fn parse(definition: &ExternalDefinition) -> Result<Upstream, InvalidUpstream> {
    let text = definition.upstream.trim();
    let (scheme, rest) = if let Some(rest) = text.strip_prefix("https://") {
        (Scheme::Https, rest)
    } else if let Some(rest) = text.strip_prefix("http://") {
        (Scheme::Http, rest)
    } else {
        return Err(invalid("the upstream must start with http:// or https://"));
    };
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    if authority.is_empty() || authority.contains(['/', '?', '#', '@']) {
        return Err(invalid(
            "the upstream is a scheme, an address and an optional port, with no path, query or credentials",
        ));
    }
    let default_port = match scheme {
        Scheme::Http => 80,
        Scheme::Https => 443,
    };
    let address = if let Ok(address) = authority.parse::<SocketAddr>() {
        address
    } else if let Ok(ip) = authority
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .map_err(drop)
        .and_then(|ip| {
            if ip.is_ipv6() && !authority.starts_with('[') {
                Err(())
            } else {
                Ok(ip)
            }
        })
    {
        SocketAddr::new(ip, default_port)
    } else {
        return Err(invalid(
            "the upstream host must be an IP address; names are not supported",
        ));
    };
    if address.port() == 0 {
        return Err(invalid("the upstream port must not be 0"));
    }
    let address = SocketAddr::new(address.ip().to_canonical(), address.port());
    check_class(address.ip())?;
    let pin = match definition.pin.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(_) if scheme == Scheme::Http => {
            return Err(invalid(
                "a certificate pin applies only to an https:// upstream",
            ));
        }
        Some(text) => Some(parse_fingerprint(text).ok_or_else(|| {
            invalid("the pin must be a SHA-256 fingerprint: 64 hex digits, colons allowed")
        })?),
    };
    Ok(Upstream {
        scheme,
        address,
        pin,
    })
}

fn check_class(ip: IpAddr) -> Result<(), InvalidUpstream> {
    let rule = "an External Application must be on a private LAN address (RFC 1918 or IPv6 ULA)";
    if ip.is_loopback() {
        return Err(invalid(format!("{ip} is a loopback address; {rule}")));
    }
    let link_local = match ip {
        IpAddr::V4(ip) => ip.is_link_local(),
        IpAddr::V6(ip) => (ip.segments()[0] & 0xffc0) == 0xfe80,
    };
    if link_local {
        return Err(invalid(format!("{ip} is a link-local address; {rule}")));
    }
    if ip.is_unspecified() || ip.is_multicast() || ip == IpAddr::V4(Ipv4Addr::BROADCAST) {
        return Err(invalid(format!("{ip} is not a unicast address; {rule}")));
    }
    if !is_private_lan(ip) {
        return Err(invalid(format!("{ip} is a public address; {rule}")));
    }
    Ok(())
}

fn contains(network: IpAddr, prefix: u8, ip: IpAddr) -> bool {
    match (network, ip) {
        (IpAddr::V4(network), IpAddr::V4(ip)) => {
            let mask = u32::MAX
                .checked_shl(32 - u32::from(prefix.min(32)))
                .unwrap_or(0);
            u32::from(network) & mask == u32::from(ip) & mask
        }
        (IpAddr::V6(network), IpAddr::V6(ip)) => {
            let mask = u128::MAX
                .checked_shl(128 - u32::from(prefix.min(128)))
                .unwrap_or(0);
            u128::from(network) & mask == u128::from(ip) & mask
        }
        _ => false,
    }
}

fn is_docker_bridge(interface: &str) -> bool {
    interface.starts_with("docker") || interface.starts_with("br-")
}

pub fn check_local(ip: IpAddr, networks: &[LocalNetwork]) -> Result<(), InvalidUpstream> {
    if networks
        .iter()
        .any(|network| network.address.to_canonical() == ip)
    {
        return Err(invalid(format!(
            "{ip} is an address of this Host; the proxy would send requests back to itself"
        )));
    }
    let (default_bridge, default_prefix) = DOCKER_DEFAULT_BRIDGE;
    if contains(IpAddr::V4(default_bridge), default_prefix, ip)
        || networks.iter().any(|network| {
            is_docker_bridge(&network.interface) && contains(network.address, network.prefix, ip)
        })
    {
        return Err(invalid(format!(
            "{ip} is on a Docker bridge network, where Applications run, not on the LAN"
        )));
    }
    Ok(())
}

pub fn validate(definition: &ExternalDefinition) -> Result<ExternalDefinition, InvalidUpstream> {
    let networks = crate::host_addresses::local_networks()
        .map_err(|error| invalid(format!("could not read this Host's addresses: {error}")))?;
    validate_with(definition, &networks)
}

pub fn validate_with(
    definition: &ExternalDefinition,
    networks: &[LocalNetwork],
) -> Result<ExternalDefinition, InvalidUpstream> {
    let upstream = parse(definition)?;
    check_local(upstream.address.ip(), networks)?;
    Ok(ExternalDefinition {
        upstream: upstream.to_string(),
        pin: upstream.pin.as_ref().map(crate::proxy::format_fingerprint),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PresentedCertificate {
    pub fingerprint: String,
    pub subject: Option<String>,
    pub not_after: Option<String>,
}

#[derive(Debug)]
pub enum FetchError {
    Refused(InvalidUpstream),
    Unreachable(String),
}

const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

pub async fn fetch_certificate(upstream: &str) -> Result<PresentedCertificate, FetchError> {
    let networks = crate::host_addresses::local_networks().map_err(|error| {
        FetchError::Refused(invalid(format!(
            "could not read this Host's addresses: {error}"
        )))
    })?;
    let address = fetch_target(upstream, &networks).map_err(FetchError::Refused)?;
    let certificate = crate::proxy::presented_certificate(address, FETCH_TIMEOUT)
        .await
        .map_err(FetchError::Unreachable)?;
    Ok(describe(certificate.as_ref()))
}

fn fetch_target(upstream: &str, networks: &[LocalNetwork]) -> Result<SocketAddr, InvalidUpstream> {
    let definition = ExternalDefinition {
        upstream: upstream.into(),
        pin: None,
    };
    let parsed = parse(&definition)?;
    if parsed.scheme != Scheme::Https {
        return Err(invalid(
            "a fingerprint is fetched only from an https:// upstream",
        ));
    }
    check_local(parsed.address.ip(), networks)?;
    Ok(parsed.address)
}

fn describe(der: &[u8]) -> PresentedCertificate {
    let fingerprint = crate::proxy::format_fingerprint(&crate::proxy::fingerprint_of(der));
    let Ok((_, certificate)) = x509_parser::parse_x509_certificate(der) else {
        return PresentedCertificate {
            fingerprint,
            subject: None,
            not_after: None,
        };
    };
    let subject = certificate.subject().to_string();
    PresentedCertificate {
        fingerprint,
        subject: (!subject.is_empty()).then_some(subject),
        not_after: certificate
            .validity()
            .not_after
            .to_datetime()
            .format(&time::format_description::well_known::Rfc3339)
            .ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition(upstream: &str, pin: Option<&str>) -> ExternalDefinition {
        ExternalDefinition {
            upstream: upstream.into(),
            pin: pin.map(str::to_string),
        }
    }

    fn host() -> Vec<LocalNetwork> {
        vec![
            LocalNetwork {
                interface: "eth0".into(),
                address: "192.168.1.20".parse().unwrap(),
                prefix: 24,
            },
            LocalNetwork {
                interface: "br-3f2a".into(),
                address: "172.20.0.1".parse().unwrap(),
                prefix: 16,
            },
            LocalNetwork {
                interface: "docker0".into(),
                address: "10.200.0.1".parse().unwrap(),
                prefix: 24,
            },
        ]
    }

    fn refusal(upstream: &str) -> String {
        validate_with(&definition(upstream, None), &host())
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn a_private_lan_address_is_accepted_and_normalized() {
        let pin = "ab".repeat(32);
        let accepted =
            validate_with(&definition("https://192.168.1.1/", Some(&pin)), &host()).unwrap();
        assert_eq!(accepted.upstream, "https://192.168.1.1:443");
        assert_eq!(accepted.pin.as_deref(), Some(["AB"; 32].join(":").as_str()));
        let accepted = validate_with(&definition("http://[fd00::5]:8080", None), &host()).unwrap();
        assert_eq!(accepted.upstream, "http://[fd00::5]:8080");
        assert!(validate_with(&definition("http://10.0.0.5:8080", None), &host()).is_ok());
        assert!(validate_with(&definition("http://172.16.4.2", None), &host()).is_ok());
    }

    #[test]
    fn each_forbidden_upstream_class_is_refused_by_name() {
        assert!(refusal("http://127.0.0.1:8080").contains("loopback"));
        assert!(refusal("http://[::1]:8080").contains("loopback"));
        assert!(refusal("http://[::ffff:127.0.0.1]:8080").contains("loopback"));
        assert!(refusal("http://169.254.169.254").contains("link-local"));
        assert!(refusal("http://[fe80::1]").contains("link-local"));
        assert!(refusal("https://8.8.8.8").contains("public"));
        assert!(refusal("https://100.64.0.1").contains("public"));
        assert!(refusal("https://[2001:db8::1]").contains("public"));
        assert!(refusal("http://0.0.0.0:80").contains("unicast"));
        assert!(refusal("https://router.lan").contains("names are not supported"));
        assert!(refusal("http://192.168.1.20:8080").contains("address of this Host"));
        assert!(refusal("http://172.17.0.2").contains("Docker bridge"));
        assert!(refusal("http://172.20.3.4").contains("Docker bridge"));
        assert!(refusal("http://10.200.0.9").contains("Docker bridge"));
        assert!(refusal("ftp://192.168.1.1").contains("http://"));
        assert!(refusal("https://192.168.1.1/admin").contains("no path"));
        assert!(refusal("https://user@192.168.1.1").contains("credentials"));
        assert!(refusal("http://192.168.1.1:0").contains("port"));
    }

    #[test]
    fn a_pin_must_be_a_sha256_fingerprint_on_an_https_upstream() {
        let pin = "ab".repeat(32);
        assert!(
            validate_with(&definition("http://192.168.1.1", Some(&pin)), &host())
                .unwrap_err()
                .to_string()
                .contains("https://")
        );
        assert!(
            validate_with(&definition("https://192.168.1.1", Some("abcd")), &host())
                .unwrap_err()
                .to_string()
                .contains("SHA-256")
        );
    }

    #[test]
    fn a_fingerprint_is_fetched_only_from_an_https_lan_upstream() {
        assert_eq!(
            fetch_target("https://192.168.1.1", &host()).unwrap(),
            "192.168.1.1:443".parse().unwrap()
        );
        let refusal = |upstream: &str| fetch_target(upstream, &host()).unwrap_err().to_string();
        assert!(refusal("http://192.168.1.1").contains("only from an https://"));
        assert!(refusal("https://8.8.8.8").contains("public"));
        assert!(refusal("https://192.168.1.20").contains("address of this Host"));
        assert!(refusal("https://172.17.0.2").contains("Docker bridge"));
    }

    #[test]
    fn a_presented_certificate_names_its_subject_and_expiry() {
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(vec!["unifi.local".into()]).unwrap();
        params.distinguished_name = rcgen::DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "unifi.local");
        params.not_after = rcgen::date_time_ymd(2027, 3, 2);
        let certificate = params.self_signed(&key).unwrap();
        let described = describe(certificate.der());
        assert_eq!(
            described.fingerprint,
            crate::proxy::format_fingerprint(&crate::proxy::fingerprint_of(certificate.der()))
        );
        assert_eq!(described.subject.as_deref(), Some("CN=unifi.local"));
        assert_eq!(described.not_after.as_deref(), Some("2027-03-02T00:00:00Z"));
    }
}
