use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

use crate::compose;

#[derive(Debug)]
pub enum TlsError {
    OpenSslNotFound,
    CertGenerationFailed(String),
    ConfigWrite(String),
}

impl std::fmt::Display for TlsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TlsError::OpenSslNotFound => {
                write!(f, "openssl not found; install openssl to enable HTTPS")
            }
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

    let ca_key = dir.join("ca-key.pem");
    let ca_cert = ca_cert_path();
    let key = key_path();
    let cert = cert_path();

    generate_ca(&ca_key, &ca_cert)?;
    generate_wildcard_cert(&ca_key, &ca_cert, &key, &cert, dns_suffix)?;

    Ok(())
}

fn find_openssl() -> Option<String> {
    // Prefer system openssl on macOS (has config file)
    if std::path::Path::new("/usr/bin/openssl").exists() {
        return Some("/usr/bin/openssl".to_string());
    }
    // Fall back to PATH
    if Command::new("openssl").arg("version").output().is_ok() {
        return Some("openssl".to_string());
    }
    None
}

fn generate_ca(ca_key: &PathBuf, ca_cert: &PathBuf) -> Result<(), TlsError> {
    let openssl = find_openssl().ok_or(TlsError::OpenSslNotFound)?;
    
    let output = Command::new(&openssl)
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:4096",
            "-nodes",
            "-sha256",
            "-days",
            "3650",
            "-keyout",
            ca_key.to_str().unwrap(),
            "-out",
            ca_cert.to_str().unwrap(),
            "-subj",
            "/CN=Self-Host LAN CA",
        ])
        .output()
        .map_err(|_| TlsError::OpenSslNotFound)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TlsError::CertGenerationFailed(format!(
            "CA generation failed: {stderr}"
        )));
    }

    Ok(())
}

fn generate_wildcard_cert(
    ca_key: &PathBuf,
    ca_cert: &PathBuf,
    key: &PathBuf,
    cert: &PathBuf,
    dns_suffix: &str,
) -> Result<(), TlsError> {
    let openssl = find_openssl().ok_or(TlsError::OpenSslNotFound)?;
    let csr = certs_dir().join("cert.csr");
    let ext_file = certs_dir().join("ext.cnf");

    let ext_content = format!(
        "authorityKeyIdentifier=keyid,issuer\n\
         basicConstraints=CA:FALSE\n\
         keyUsage=digitalSignature,keyEncipherment\n\
         extendedKeyUsage=serverAuth\n\
         subjectAltName=DNS:*.{},DNS:{}\n",
        dns_suffix, dns_suffix
    );

    let mut file = std::fs::File::create(&ext_file)
        .map_err(|e| TlsError::ConfigWrite(format!("create ext file: {e}")))?;
    file.write_all(ext_content.as_bytes())
        .map_err(|e| TlsError::ConfigWrite(format!("write ext file: {e}")))?;

    let output = Command::new(&openssl)
        .args([
            "req",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-sha256",
            "-keyout",
            key.to_str().unwrap(),
            "-out",
            csr.to_str().unwrap(),
            "-subj",
            &format!("/CN=*.{}", dns_suffix),
        ])
        .output()
        .map_err(|_| TlsError::OpenSslNotFound)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TlsError::CertGenerationFailed(format!(
            "CSR generation failed: {stderr}"
        )));
    }

    let output = Command::new(&openssl)
        .args([
            "x509",
            "-req",
            "-sha256",
            "-days",
            "825",
            "-in",
            csr.to_str().unwrap(),
            "-CA",
            ca_cert.to_str().unwrap(),
            "-CAkey",
            ca_key.to_str().unwrap(),
            "-CAcreateserial",
            "-out",
            cert.to_str().unwrap(),
            "-extfile",
            ext_file.to_str().unwrap(),
        ])
        .output()
        .map_err(|_| TlsError::OpenSslNotFound)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TlsError::CertGenerationFailed(format!(
            "cert signing failed: {stderr}"
        )));
    }

    let _ = std::fs::remove_file(&csr);
    let _ = std::fs::remove_file(&ext_file);

    Ok(())
}

pub fn traefik_tls_args() -> Vec<String> {
    vec![
        "--providers.docker=true".into(),
        "--providers.docker.exposedbydefault=false".into(),
        "--entrypoints.web.address=:80".into(),
        "--entrypoints.web.http.redirections.entrypoint.to=websecure".into(),
        "--entrypoints.web.http.redirections.entrypoint.scheme=https".into(),
        "--entrypoints.websecure.address=:443".into(),
        "--entrypoints.websecure.http.tls=true".into(),
        "--certificatesresolvers.default.file.certificates[0].certfile=/certs/cert.pem".into(),
        "--certificatesresolvers.default.file.certificates[0].keyfile=/certs/key.pem".into(),
        "--entrypoints.websecure.http.tls.certresolver=default".into(),
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
    fn traefik_tls_args_include_redirection() {
        let args = traefik_tls_args();
        assert!(args.iter().any(|a| a.contains("redirections")));
        assert!(args.iter().any(|a| a.contains("websecure")));
    }
}