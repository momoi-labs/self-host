//! Backup storage (ADR-0035): the one S3-compatible bucket a Host's database
//! backups go to. The secret key is private Platform State, like a database
//! password: it reaches the S3 client that tests the storage and the
//! `pgbackrest.conf` a database container reads, and nothing else.

use std::{path::PathBuf, sync::Arc, time::Duration};

use object_store::{
    ClientOptions, ObjectStore, ObjectStoreExt, RetryConfig, aws::AmazonS3Builder, path::Path,
};
use serde::{Deserialize, Serialize};

use super::Error;
use crate::collection::{Collection, Keyed};
use crate::store::StateStore;

pub(crate) static STORAGE: Collection<BackupStorage> = Collection::new("backup-storage");
/// The Host has one backup storage.
pub(crate) const STORAGE_ID: &str = "default";

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct BackupStorage {
    pub id: String,
    /// Empty for AWS itself; a URL for any other S3-compatible service.
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    /// The folder inside the bucket every backup goes under; may be empty.
    pub prefix: String,
    /// `https://endpoint/bucket/key` instead of `https://bucket.endpoint/key`.
    pub path_style: bool,
    /// Off for a bucket with a self-signed certificate.
    #[serde(default = "verify_tls_default")]
    pub verify_tls: bool,
    pub access_key_id: String,
    pub secret_access_key: String,
}

pub(crate) fn verify_tls_default() -> bool {
    true
}

impl Keyed for BackupStorage {
    fn key(&self) -> String {
        self.id.clone()
    }
}

/// The storage the Operator configured, or why a backup cannot run.
pub(crate) async fn configured(store: &impl StateStore) -> Result<BackupStorage, Error> {
    STORAGE.get(store, STORAGE_ID).await?.ok_or_else(|| {
        Error::Conflict("Configure the backup storage in Settings before using backups".into())
    })
}

/// Checks what the Operator typed, before it is saved.
pub(crate) fn validate(storage: &BackupStorage) -> Result<(), Error> {
    let bucket = &storage.bucket;
    if bucket.is_empty()
        || bucket.len() > 63
        || !bucket
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
    {
        return Err(Error::Invalid(
            "Name the bucket with lowercase letters, digits, hyphens and dots, in at most 63 characters".into(),
        ));
    }
    if !storage.endpoint.is_empty() && host_and_port(&storage.endpoint).is_none() {
        return Err(Error::Invalid(
            "The endpoint is https:// and a host, with a port when it needs one. pgBackRest does not use plain HTTP. Leave it empty for AWS"
                .into(),
        ));
    }
    if storage.region.is_empty() {
        return Err(Error::Invalid(
            "Name the region, for example us-east-1".into(),
        ));
    }
    if storage.access_key_id.is_empty() || storage.secret_access_key.is_empty() {
        return Err(Error::Invalid(
            "Give the access key id and the secret access key".into(),
        ));
    }
    if storage
        .prefix
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
        && !storage.prefix.is_empty()
    {
        return Err(Error::Invalid(
            "The prefix is a folder path such as backups/home, with no empty, '.' or '..' part"
                .into(),
        ));
    }
    Ok(())
}

/// The host and port of an `https://host[:port]` endpoint.
fn host_and_port(endpoint: &str) -> Option<(&str, Option<u16>)> {
    let rest = endpoint.strip_prefix("https://")?;
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) if !port.contains(']') => (host, Some(port.parse().ok()?)),
        _ => (rest, None),
    };
    let plain = !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-.:[]".contains(&b));
    plain.then_some((host, port))
}

/// Where a database's `pgbackrest.conf` is on the Host. The container's
/// `postgres` user reads it through a read-only mount, so the file is
/// world-readable inside a parent directory only the Platform's user enters.
pub(crate) fn conf_dir(id: &str) -> PathBuf {
    crate::file_store::state_dir()
        .join("postgres-backups")
        .join(id)
}

/// Where the container sees `conf_dir`.
pub(crate) const CONF_MOUNT: &str = "/etc/pgbackrest";
/// Every server is the one stanza of its own repository.
pub(crate) const STANZA: &str = "main";

/// The cluster's directory inside the container.
pub(crate) fn pg_path(major: u16) -> String {
    if major >= 18 {
        format!("/var/lib/postgresql/{major}/docker")
    } else {
        "/var/lib/postgresql/data".into()
    }
}

/// The `pgbackrest.conf` of one server. `repository` is the id of the
/// database whose repository it reads and writes: its own, or its source's
/// while a restore replays WAL.
pub(crate) fn conf(
    storage: &BackupStorage,
    repository: &str,
    major: u16,
    keep_days: u32,
) -> String {
    let (host, port) = match host_and_port(&storage.endpoint) {
        Some((host, port)) => (host.to_owned(), port),
        None => (format!("s3.{}.amazonaws.com", storage.region), None),
    };
    let path = if storage.prefix.is_empty() {
        format!("/{repository}")
    } else {
        format!("/{}/{repository}", storage.prefix)
    };
    let mut text = format!(
        "[global]\nrepo1-type=s3\nrepo1-path={path}\nrepo1-s3-bucket={}\nrepo1-s3-endpoint={host}\nrepo1-s3-region={}\nrepo1-s3-uri-style={}\nrepo1-storage-verify-tls={}\nrepo1-s3-key={}\nrepo1-s3-key-secret={}\nrepo1-retention-full-type=time\nrepo1-retention-full={keep_days}\nstart-fast=y\nlog-level-console=info\nlog-level-file=off\n",
        storage.bucket,
        storage.region,
        if storage.path_style { "path" } else { "host" },
        if storage.verify_tls { "y" } else { "n" },
        storage.access_key_id,
        storage.secret_access_key,
    );
    if let Some(port) = port {
        text.push_str(&format!("repo1-storage-port={port}\n"));
    }
    text.push_str(&format!(
        "\n[{STANZA}]\npg1-path={}\npg1-user={}\npg1-database=postgres\n",
        pg_path(major),
        super::ADMIN
    ));
    text
}

/// Writes `text` as the `pgbackrest.conf` in `dir`, replacing the old one in
/// one step so a running `archive-push` never reads half a file.
pub(crate) fn write_conf(dir: &std::path::Path, text: &str) -> Result<(), Error> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let failed = |error: std::io::Error| {
        Error::Unavailable(format!(
            "Could not write the backup configuration on the Host: {error}"
        ))
    };
    let parent = dir.parent().unwrap_or(dir);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)
        .map_err(failed)?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).map_err(failed)?;
    if !dir.exists() {
        std::fs::DirBuilder::new()
            .mode(0o755)
            .create(dir)
            .map_err(failed)?;
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).map_err(failed)?;
    let draft = dir.join(".pgbackrest.conf.new");
    std::fs::write(&draft, text).map_err(failed)?;
    std::fs::set_permissions(&draft, std::fs::Permissions::from_mode(0o644)).map_err(failed)?;
    std::fs::rename(&draft, dir.join("pgbackrest.conf")).map_err(failed)
}

/// The prefix as it is kept: no slash at either end.
pub(crate) fn clean_prefix(prefix: &str) -> String {
    prefix.trim().trim_matches('/').to_owned()
}

/// An open bucket, for the storage test. Keys are relative to the storage's
/// prefix. Backups themselves go through pgBackRest, not through this.
#[derive(Clone)]
pub(crate) struct Bucket {
    store: Arc<dyn ObjectStore>,
    prefix: String,
}

pub(crate) fn refused(what: &str, error: impl std::fmt::Display) -> Error {
    Error::Unavailable(format!("The backup storage {what}: {error}"))
}

impl Bucket {
    fn open_with(storage: &BackupStorage, retry: RetryConfig) -> Result<Self, Error> {
        // The binary's TLS provider; another module may have installed it.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut builder = AmazonS3Builder::new()
            .with_region(&storage.region)
            .with_bucket_name(&storage.bucket)
            .with_access_key_id(&storage.access_key_id)
            .with_secret_access_key(&storage.secret_access_key)
            .with_virtual_hosted_style_request(!storage.path_style)
            .with_retry(retry)
            .with_client_options(
                ClientOptions::new()
                    .with_connect_timeout(Duration::from_secs(15))
                    .with_timeout(Duration::from_secs(60))
                    .with_allow_invalid_certificates(!storage.verify_tls),
            );
        if !storage.endpoint.is_empty() {
            builder = builder.with_endpoint(&storage.endpoint);
        }
        let store = builder
            .build()
            .map_err(|error| refused("could not be opened", error))?;
        Ok(Self {
            store: Arc::new(store),
            prefix: storage.prefix.clone(),
        })
    }

    fn path(&self, key: &str) -> Path {
        if self.prefix.is_empty() {
            Path::from(key)
        } else {
            Path::from(format!("{}/{key}", self.prefix))
        }
    }

    pub(crate) async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<(), Error> {
        self.store
            .put(&self.path(key), bytes.into())
            .await
            .map(|_| ())
            .map_err(|error| refused("did not take the object", error))
    }

    pub(crate) async fn get(&self, key: &str) -> Result<Vec<u8>, Error> {
        let result = self
            .store
            .get(&self.path(key))
            .await
            .map_err(|error| refused("did not return the object", error))?;
        result
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|error| refused("did not return the object", error))
    }

    pub(crate) async fn remove(&self, key: &str) -> Result<(), Error> {
        self.store
            .delete(&self.path(key))
            .await
            .map_err(|error| refused("did not delete the object", error))
    }
}

/// Writes, reads and deletes a small object, so a wrong key, bucket or
/// endpoint shows before the first backup depends on it.
pub(crate) async fn test(storage: &BackupStorage) -> Result<(), Error> {
    let bucket = Bucket::open_with(
        storage,
        RetryConfig {
            max_retries: 1,
            retry_timeout: Duration::from_secs(20),
            ..RetryConfig::default()
        },
    )?;
    let key = format!(".self-host-probe-{:016x}", rand::random::<u64>());
    let body = b"self-host backup storage probe".to_vec();
    bucket.put(&key, body.clone()).await?;
    let read = bucket.get(&key).await;
    let removed = bucket.remove(&key).await;
    if read? != body {
        return Err(Error::Unavailable(
            "The backup storage returned different bytes than it was given".into(),
        ));
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage() -> BackupStorage {
        BackupStorage {
            id: STORAGE_ID.into(),
            endpoint: "https://s3.example.com".into(),
            region: "us-east-1".into(),
            bucket: "backups".into(),
            prefix: "home".into(),
            path_style: true,
            verify_tls: true,
            access_key_id: "key".into(),
            secret_access_key: "secret".into(),
        }
    }

    #[test]
    fn storage_refuses_what_s3_would_not_take() {
        assert!(validate(&storage()).is_ok());
        for change in [
            |s: &mut BackupStorage| s.bucket = "Has Spaces".into(),
            |s: &mut BackupStorage| s.endpoint = "s3.example.com".into(),
            |s: &mut BackupStorage| s.endpoint = "http://s3.example.com".into(),
            |s: &mut BackupStorage| s.endpoint = "https://s3.example.com/path".into(),
            |s: &mut BackupStorage| s.region.clear(),
            |s: &mut BackupStorage| s.secret_access_key.clear(),
            |s: &mut BackupStorage| s.prefix = "a/../b".into(),
        ] {
            let mut changed = storage();
            change(&mut changed);
            assert!(validate(&changed).is_err());
        }
        let mut aws = storage();
        aws.endpoint.clear();
        aws.prefix.clear();
        assert!(validate(&aws).is_ok());
    }

    #[test]
    fn the_configuration_names_the_repository_by_the_database_id() {
        let mut storage = storage();
        storage.endpoint = "https://s3.example.com:9443".into();
        storage.verify_tls = false;
        let text = conf(&storage, "db1", 17, 14);
        for line in [
            "repo1-path=/home/db1",
            "repo1-s3-endpoint=s3.example.com",
            "repo1-storage-port=9443",
            "repo1-storage-verify-tls=n",
            "repo1-s3-uri-style=path",
            "repo1-retention-full=14",
            "pg1-path=/var/lib/postgresql/data",
        ] {
            assert!(text.lines().any(|l| l == line), "{line} is missing");
        }
        storage.endpoint.clear();
        storage.prefix.clear();
        let aws = conf(&storage, "db1", 18, 7);
        assert!(aws.contains("repo1-s3-endpoint=s3.us-east-1.amazonaws.com\n"));
        assert!(aws.contains("repo1-path=/db1\n"));
        assert!(aws.contains("pg1-path=/var/lib/postgresql/18/docker\n"));
    }

    #[test]
    fn a_prefix_is_kept_without_slashes_at_its_ends() {
        assert_eq!(clean_prefix(" /backups/home/ "), "backups/home");
        assert_eq!(clean_prefix("/"), "");
    }
}
