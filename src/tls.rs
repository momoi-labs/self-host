use std::path::PathBuf;

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair, KeyUsagePurpose,
    SanType,
};
use time::{Duration, OffsetDateTime};

use crate::compose;

#[derive(Debug)]
pub enum TlsError {
    CertGenerationFailed(String),
    ConfigWrite(String),
}

impl std::fmt::Display for TlsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TlsError::CertGenerationFailed(msg) => {
                write!(f, "certificate generation failed: {msg}")
            }
            TlsError::ConfigWrite(msg) => write!(f, "failed to write TLS config: {msg}"),
        }
    }
}

impl std::error::Error for TlsError {}

pub fn certs_dir() -> PathBuf {
    compose::platform_config_dir().join("certs")
}

pub fn cert_path() -> PathBuf {
    certs_dir().join("cert.pem")
}

pub fn key_path() -> PathBuf {
    certs_dir().join("key.pem")
}

pub fn ca_cert_path() -> PathBuf {
    certs_dir().join("ca.pem")
}

pub fn generate_certificates(dns_suffix: &str) -> Result<(), TlsError> {
    let dir = certs_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| TlsError::ConfigWrite(format!("create certs directory: {e}")))?;

    let ca_key_path = dir.join("ca-key.pem");
    let ca_cert_file = ca_cert_path();
    let key = key_path();
    let cert = cert_path();

    let now = OffsetDateTime::now_utc();
    let ten_years = Duration::days(365 * 10);

    let ca_key = KeyPair::generate()
        .map_err(|e| TlsError::CertGenerationFailed(format!("generate CA key: {e}")))?;

    let mut ca_params = CertificateParams::default();
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(DnType::CommonName, "Self-Host LAN CA");
    ca_params.distinguished_name = ca_dn;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    ca_params.not_before = now;
    ca_params.not_after = now + ten_years;

    let ca_cert = ca_params
        .self_signed(&ca_key)
        .map_err(|e| TlsError::CertGenerationFailed(format!("generate CA cert: {e}")))?;

    std::fs::write(&ca_cert_file, ca_cert.pem())
        .map_err(|e| TlsError::ConfigWrite(format!("write CA cert: {e}")))?;
    std::fs::write(&ca_key_path, ca_key.serialize_pem())
        .map_err(|e| TlsError::ConfigWrite(format!("write CA key: {e}")))?;

    let mut cert_params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, format!("*.{}", dns_suffix));
    cert_params.distinguished_name = dn;

    let wildcard_name = format!("*.{}", dns_suffix);
    cert_params.subject_alt_names = vec![
        SanType::DnsName(wildcard_name.try_into().unwrap()),
        SanType::DnsName(dns_suffix.to_string().try_into().unwrap()),
    ];
    cert_params.not_before = now;
    cert_params.not_after = now + ten_years;

    let cert_key = KeyPair::generate()
        .map_err(|e| TlsError::CertGenerationFailed(format!("generate cert key: {e}")))?;
    let cert_obj = cert_params
        .signed_by(&cert_key, &ca_cert, &ca_key)
        .map_err(|e| TlsError::CertGenerationFailed(format!("sign cert: {e}")))?;

    std::fs::write(&cert, cert_obj.pem())
        .map_err(|e| TlsError::ConfigWrite(format!("write cert: {e}")))?;
    std::fs::write(&key, cert_key.serialize_pem())
        .map_err(|e| TlsError::ConfigWrite(format!("write key: {e}")))?;

    Ok(())
}

pub fn traefik_config_path() -> PathBuf {
    compose::platform_config_dir().join("traefik.yml")
}

pub fn traefik_dynamic_dir() -> std::path::PathBuf {
    compose::platform_config_dir().join("traefik-dynamic")
}

pub fn write_traefik_config() -> Result<(), TlsError> {
    let config = r#"entryPoints:
  web:
    address: ":80"
    http:
      redirections:
        entryPoint:
          to: websecure
          scheme: https
  websecure:
    address: ":443"

providers:
  docker:
    exposedByDefault: false
  file:
    directory: /etc/traefik/dynamic
    watch: true
"#;

    let path = traefik_config_path();
    std::fs::write(&path, config)
        .map_err(|e| TlsError::ConfigWrite(format!("write traefik config: {e}")))?;

    let dynamic_dir = traefik_dynamic_dir();
    std::fs::create_dir_all(&dynamic_dir)
        .map_err(|e| TlsError::ConfigWrite(format!("create dynamic dir: {e}")))?;

    let tls_config = r#"tls:
  certificates:
    - certFile: /certs/cert.pem
      keyFile: /certs/key.pem
"#;
    std::fs::write(dynamic_dir.join("tls.yml"), tls_config)
        .map_err(|e| TlsError::ConfigWrite(format!("write tls.yml: {e}")))?;

    Ok(())
}

pub fn write_admin_route(dns_suffix: &str) -> Result<(), TlsError> {
    let dynamic_dir = traefik_dynamic_dir();
    std::fs::create_dir_all(&dynamic_dir)
        .map_err(|e| TlsError::ConfigWrite(format!("create dynamic dir: {e}")))?;

    let admin_config = format!(
        r#"http:
  routers:
    admin:
      rule: "Host(`admin.{dns_suffix}`)"
      entryPoints:
        - websecure
      tls: {{}}
      service: admin-api
  services:
    admin-api:
      loadBalancer:
        servers:
          - url: "http://host.docker.internal:{OPERATOR_API_PORT}"
"#,
        dns_suffix = dns_suffix,
        OPERATOR_API_PORT = crate::bootstrap::OPERATOR_API_PORT,
    );

    std::fs::write(dynamic_dir.join("admin.yml"), admin_config)
        .map_err(|e| TlsError::ConfigWrite(format!("write admin.yml: {e}")))?;

    Ok(())
}

pub fn traefik_args() -> Vec<String> {
    vec![
        "--providers.docker=true".into(),
        "--providers.docker.exposedbydefault=false".into(),
        "--configfile=/etc/traefik/traefik.yml".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certs_dir_is_under_platform_config() {
        let dir = certs_dir();
        assert!(dir.ends_with("certs"));
    }

    #[test]
    fn traefik_args_include_configfile() {
        let args = traefik_args();
        assert!(args.iter().any(|a| a.contains("configfile")));
    }
}
