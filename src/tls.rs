use std::path::{Path, PathBuf};

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair, KeyUsagePurpose,
    SanType,
};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use x509_parser::{extensions::ParsedExtension, parse_x509_certificate};

use crate::compose;

#[derive(Debug)]
pub enum TlsError {
    CertGenerationFailed(String),
    ConfigWrite(String),
    ExistingCertificates(String),
}

impl std::fmt::Display for TlsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TlsError::CertGenerationFailed(msg) => {
                write!(f, "certificate generation failed: {msg}")
            }
            TlsError::ConfigWrite(msg) => write!(f, "failed to write TLS config: {msg}"),
            TlsError::ExistingCertificates(msg) => {
                write!(f, "existing TLS certificates are inconsistent: {msg}")
            }
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

pub fn ca_key_path() -> PathBuf {
    certs_dir().join("ca-key.pem")
}

fn write_private_key(path: &Path, pem: &str) -> Result<(), TlsError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| TlsError::ConfigWrite(format!("create {}: {e}", path.display())))?;
    std::io::Write::write_all(&mut file, pem.as_bytes())
        .map_err(|e| TlsError::ConfigWrite(format!("write {}: {e}", path.display())))
}

fn parse_certificate(path: &Path) -> Result<CertificateParams, TlsError> {
    let pem = std::fs::read_to_string(path).map_err(|e| {
        TlsError::ExistingCertificates(format!("cannot read {}: {e}", path.display()))
    })?;
    CertificateParams::from_ca_cert_pem(&pem).map_err(|e| {
        TlsError::ExistingCertificates(format!("cannot parse {}: {e}", path.display()))
    })
}

fn certificate_der(path: &Path) -> Result<Vec<u8>, TlsError> {
    let pem = std::fs::read(path).map_err(|e| {
        TlsError::ExistingCertificates(format!("cannot read {}: {e}", path.display()))
    })?;
    pem::parse(pem).map(|pem| pem.into_contents()).map_err(|e| {
        TlsError::ExistingCertificates(format!("cannot parse {}: {e}", path.display()))
    })
}

fn certificate_has_authority_key_identifier(path: &Path) -> Result<bool, TlsError> {
    let der = certificate_der(path)?;
    let (_, certificate) = parse_x509_certificate(&der).map_err(|e| {
        TlsError::ExistingCertificates(format!("cannot inspect {}: {e}", path.display()))
    })?;
    Ok(certificate.iter_extensions().any(|extension| {
        matches!(
            extension.parsed_extension(),
            ParsedExtension::AuthorityKeyIdentifier(_)
        )
    }))
}

fn validate_key_matches_certificate(key: &Path, certificate_spki: &[u8]) -> Result<(), TlsError> {
    let pem = std::fs::read_to_string(key).map_err(|e| {
        TlsError::ExistingCertificates(format!("cannot read {}: {e}", key.display()))
    })?;
    let key_pair = KeyPair::from_pem(&pem).map_err(|e| {
        TlsError::ExistingCertificates(format!("cannot parse {}: {e}", key.display()))
    })?;
    if key_pair.public_key_der() != certificate_spki {
        return Err(TlsError::ExistingCertificates(format!(
            "{} does not match its certificate; run 'self-host reset' to generate a consistent certificate set",
            key.display()
        )));
    }
    Ok(())
}

fn validate_certificates_in(dir: &Path, dns_suffix: &str) -> Result<(), TlsError> {
    let ca_cert = dir.join("ca.pem");
    let ca_key = dir.join("ca-key.pem");
    let cert = dir.join("cert.pem");
    let key = dir.join("key.pem");
    let paths = [&ca_cert, &ca_key, &cert, &key];
    let existing = paths.iter().filter(|path| path.exists()).count();
    if existing != paths.len() {
        return Err(TlsError::ExistingCertificates(format!(
            "found {existing} of 4 required files; run 'self-host reset' before creating a new CA"
        )));
    }

    let _ = parse_certificate(&ca_cert)?;
    let leaf = parse_certificate(&cert)?;
    if !certificate_has_authority_key_identifier(&cert)? {
        return Err(TlsError::ExistingCertificates(
            "cert.pem has no Authority Key Identifier; run 'self-host reset' to prevent ambiguity with an older CA"
                .into(),
        ));
    }
    let expected = [dns_suffix.to_string(), format!("*.{dns_suffix}")];
    for expected_name in expected {
        let present = leaf.subject_alt_names.iter().any(|name| match name {
            SanType::DnsName(name) => name.as_str() == expected_name,
            _ => false,
        });
        if !present {
            return Err(TlsError::ExistingCertificates(format!(
                "cert.pem does not cover '{expected_name}'; use the original DNS Suffix or run 'self-host reset'"
            )));
        }
    }

    let ca_der = certificate_der(&ca_cert)?;
    let leaf_der = certificate_der(&cert)?;
    let (_, ca_x509) = parse_x509_certificate(&ca_der)
        .map_err(|e| TlsError::ExistingCertificates(format!("cannot verify ca.pem: {e}")))?;
    let (_, leaf_x509) = parse_x509_certificate(&leaf_der)
        .map_err(|e| TlsError::ExistingCertificates(format!("cannot verify cert.pem: {e}")))?;
    ca_x509.verify_signature(None).map_err(|e| {
        TlsError::ExistingCertificates(format!("ca.pem is not self-signed correctly: {e}"))
    })?;
    leaf_x509
        .verify_signature(Some(ca_x509.public_key()))
        .map_err(|e| {
            TlsError::ExistingCertificates(format!("cert.pem was not signed by ca.pem: {e}"))
        })?;
    validate_key_matches_certificate(&ca_key, ca_x509.public_key().raw)?;
    validate_key_matches_certificate(&key, leaf_x509.public_key().raw)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for path in [&ca_key, &key] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| TlsError::ConfigWrite(format!("secure {}: {e}", path.display())))?;
        }
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| TlsError::ConfigWrite(format!("secure {}: {e}", dir.display())))?;
    }
    Ok(())
}

pub fn validate_certificates(dns_suffix: &str) -> Result<(), TlsError> {
    validate_certificates_in(&certs_dir(), dns_suffix)
}

pub fn generate_certificates(dns_suffix: &str) -> Result<(), TlsError> {
    generate_certificates_in(&certs_dir(), dns_suffix)
}

fn generate_certificates_in(dir: &Path, dns_suffix: &str) -> Result<(), TlsError> {
    std::fs::create_dir_all(dir)
        .map_err(|e| TlsError::ConfigWrite(format!("create certs directory: {e}")))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| TlsError::ConfigWrite(format!("secure certs directory: {e}")))?;
    }

    let ca_key_path = dir.join("ca-key.pem");
    let ca_cert_file = dir.join("ca.pem");
    let key = dir.join("key.pem");
    let cert = dir.join("cert.pem");

    if [&ca_key_path, &ca_cert_file, &key, &cert]
        .iter()
        .any(|path| path.exists())
    {
        return validate_certificates_in(dir, dns_suffix);
    }

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
    write_private_key(&ca_key_path, &ca_key.serialize_pem())?;

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
    cert_params.use_authority_key_identifier_extension = true;

    let cert_key = KeyPair::generate()
        .map_err(|e| TlsError::CertGenerationFailed(format!("generate cert key: {e}")))?;
    let cert_obj = cert_params
        .signed_by(&cert_key, &ca_cert, &ca_key)
        .map_err(|e| TlsError::CertGenerationFailed(format!("sign cert: {e}")))?;

    std::fs::write(&cert, cert_obj.pem())
        .map_err(|e| TlsError::ConfigWrite(format!("write cert: {e}")))?;
    write_private_key(&key, &cert_key.serialize_pem())?;

    Ok(())
}

pub fn ca_sha256_fingerprint() -> Result<String, TlsError> {
    let ca = std::fs::read(ca_cert_path())
        .map_err(|e| TlsError::ExistingCertificates(format!("cannot read ca.pem: {e}")))?;
    ca_sha256_fingerprint_from_pem(&ca)
}

pub fn ca_sha256_fingerprint_from_pem(ca: &[u8]) -> Result<String, TlsError> {
    let ca = pem::parse(ca)
        .map_err(|e| TlsError::ExistingCertificates(format!("cannot parse CA certificate: {e}")))?;
    Ok(Sha256::digest(ca.contents())
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":"))
}

pub fn validate_public_ca(ca: &[u8]) -> Result<(), TlsError> {
    let pem = pem::parse(ca)
        .map_err(|e| TlsError::ExistingCertificates(format!("cannot parse CA certificate: {e}")))?;
    let (_, certificate) = parse_x509_certificate(pem.contents()).map_err(|e| {
        TlsError::ExistingCertificates(format!("cannot inspect CA certificate: {e}"))
    })?;
    let is_ca = certificate
        .basic_constraints()
        .map_err(|e| TlsError::ExistingCertificates(format!("invalid CA constraints: {e}")))?
        .is_some_and(|constraints| constraints.value.ca);
    if !is_ca {
        return Err(TlsError::ExistingCertificates(
            "downloaded certificate is not a CA".into(),
        ));
    }
    certificate.verify_signature(None).map_err(|e| {
        TlsError::ExistingCertificates(format!("CA certificate is not self-signed correctly: {e}"))
    })
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
    fn repeated_generation_keeps_the_ca() {
        let dir =
            std::env::temp_dir().join(format!("self-host-tls-stability-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        generate_certificates_in(&dir, "home.lan").unwrap();
        let first_ca = std::fs::read(dir.join("ca.pem")).unwrap();
        generate_certificates_in(&dir, "home.lan").unwrap();
        let second_ca = std::fs::read(dir.join("ca.pem")).unwrap();

        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(first_ca, second_ca, "a repeated init replaced the CA");
    }

    #[test]
    fn generated_leaf_covers_the_suffix_and_its_wildcard() {
        let dir = std::env::temp_dir().join(format!("self-host-tls-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        generate_certificates_in(&dir, "home.lan").unwrap();
        validate_certificates_in(&dir, "home.lan").unwrap();
        let has_authority_key_identifier =
            certificate_has_authority_key_identifier(&dir.join("cert.pem")).unwrap();
        let mismatch = validate_certificates_in(&dir, "test.lan").unwrap_err();

        std::fs::remove_dir_all(&dir).unwrap();
        assert!(has_authority_key_identifier);
        assert!(mismatch.to_string().contains("does not cover"));
    }

    #[test]
    fn validation_rejects_a_ca_that_did_not_sign_the_leaf() {
        let first = std::env::temp_dir().join(format!("self-host-tls-ca-a-{}", std::process::id()));
        let second =
            std::env::temp_dir().join(format!("self-host-tls-ca-b-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&first);
        let _ = std::fs::remove_dir_all(&second);
        generate_certificates_in(&first, "home.lan").unwrap();
        generate_certificates_in(&second, "home.lan").unwrap();
        std::fs::copy(second.join("ca.pem"), first.join("ca.pem")).unwrap();

        let error = validate_certificates_in(&first, "home.lan").unwrap_err();

        std::fs::remove_dir_all(&first).unwrap();
        std::fs::remove_dir_all(&second).unwrap();
        assert!(error.to_string().contains("not signed by ca.pem"));
    }

    #[cfg(unix)]
    #[test]
    fn generated_private_keys_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("self-host-tls-mode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        generate_certificates_in(&dir, "home.lan").unwrap();

        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for name in ["ca-key.pem", "key.pem"] {
            assert_eq!(
                std::fs::metadata(dir.join(name))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn certs_dir_is_under_platform_config() {
        let dir = certs_dir();
        assert!(dir.ends_with("certs"));
    }

    #[test]
    fn public_ca_fingerprint_is_computed_from_certificate_der() {
        let dir =
            std::env::temp_dir().join(format!("self-host-tls-public-ca-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        generate_certificates_in(&dir, "home.lan").unwrap();

        let ca = std::fs::read(dir.join("ca.pem")).unwrap();
        let fingerprint = ca_sha256_fingerprint_from_pem(&ca).unwrap();
        validate_public_ca(&ca).unwrap();

        let leaf = std::fs::read(dir.join("cert.pem")).unwrap();
        let error = validate_public_ca(&leaf).unwrap_err();

        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(fingerprint.len(), 95);
        assert!(error.to_string().contains("not a CA"));
    }

    #[test]
    fn traefik_args_include_configfile() {
        let args = traefik_args();
        assert!(args.iter().any(|a| a.contains("configfile")));
    }
}
