//! `self-host db backup ...`: the Host's backup storage and maintenance
//! window, and each database's backups (ADR-0035).

use std::path::PathBuf;

use clap::{Subcommand, ValueEnum};
use reqwest::Method;
use serde_json::{Value, json};

use super::{Api, databases::database_id, segment, str};

/// The environment variable that carries the secret access key.
const SECRET_ENV: &str = "SELF_HOST_S3_SECRET_ACCESS_KEY";

#[derive(Clone, Copy, ValueEnum)]
pub enum Type {
    Full,
    Diff,
    Incr,
}

#[derive(Subcommand)]
pub enum BackupCommand {
    /// The S3 bucket every backup repository lives in
    Storage {
        #[command(subcommand)]
        command: StorageCommand,
    },
    /// The hours scheduled backups may start in, in the Host's local time
    Window {
        #[command(subcommand)]
        command: WindowCommand,
    },
    /// Turn a server's backups on and take the first full backup
    ///
    /// The container is recreated with an image that has pgBackRest, and
    /// PostgreSQL restarts once to turn WAL archiving on.
    Enable {
        /// Server name
        name: String,
        /// How many days back a restore can go
        #[arg(long)]
        keep_days: Option<u32>,
    },
    /// Turn a server's backups off; its repository stays in the bucket
    Disable {
        /// Server name
        name: String,
    },
    /// Back a server up now and wait until the backup is in the bucket
    Now {
        /// Server name
        name: String,
        /// full, diff or incr
        #[arg(long = "type", value_enum, default_value = "incr")]
        kind: Type,
    },
    /// Show a server's backups, newest first, and whether WAL reaches the bucket
    List {
        /// Server name
        name: String,
    },
    /// Restore a server into a new server, at the latest state or at a time
    Restore {
        /// The server to restore from
        name: String,
        /// The new server's name; it must be free
        #[arg(long = "as")]
        new_name: String,
        /// The time to restore to, as RFC 3339 (2026-11-12T14:30:00Z).
        /// Left out, the latest state in the repository
        #[arg(long)]
        time: Option<String>,
    },
    /// Test the latest backup: restore it into a throwaway PostgreSQL
    Verify {
        /// Server name
        name: String,
    },
}

#[derive(Subcommand)]
pub enum StorageCommand {
    /// Show the backup storage, without its secret
    Show,
    /// Save the backup storage, after testing it
    ///
    /// The secret access key comes from --secret-access-key-file, from
    /// SELF_HOST_S3_SECRET_ACCESS_KEY, or from a prompt. It is never an
    /// argument. Left out when a secret is saved, the saved one stays.
    Set {
        /// The S3 endpoint, as https://host[:port]; leave out for AWS
        #[arg(long, default_value = "")]
        endpoint: String,
        #[arg(long, default_value = "us-east-1")]
        region: String,
        #[arg(long)]
        bucket: String,
        /// A folder inside the bucket for every repository
        #[arg(long, default_value = "")]
        prefix: String,
        /// Address the bucket as a host name (bucket.endpoint), as AWS does
        #[arg(long)]
        virtual_hosted: bool,
        /// Accept a TLS certificate that cannot be verified, such as a
        /// self-signed one
        #[arg(long)]
        no_verify_tls: bool,
        #[arg(long)]
        access_key_id: String,
        /// A file that holds the secret access key
        #[arg(long)]
        secret_access_key_file: Option<PathBuf>,
        /// Save without testing the bucket first
        #[arg(long)]
        no_test: bool,
    },
    /// Write, read and delete a small object in the saved storage
    Test,
    /// Forget the backup storage; the bucket keeps its repositories
    Remove,
}

#[derive(Subcommand)]
pub enum WindowCommand {
    /// Show the maintenance window
    Show,
    /// Change the maintenance window
    Set {
        /// The hour it starts, 0 to 23, in the Host's local time
        #[arg(long)]
        start: u8,
        /// How many hours it lasts, 1 to 24
        #[arg(long)]
        hours: u8,
    },
}

pub async fn run(api: &Api, command: BackupCommand) -> anyhow::Result<()> {
    match command {
        BackupCommand::Storage { command } => storage(api, command).await,
        BackupCommand::Window { command } => window(api, command).await,
        BackupCommand::Enable { name, keep_days } => {
            let mut change = json!({"enabled": true});
            if let Some(keep_days) = keep_days {
                change["keep_days"] = json!(keep_days);
            }
            println!("Turning backups on for {name}…");
            let settings = configure(api, &name, change).await?;
            print_settings(&name, &settings);
            Ok(())
        }
        BackupCommand::Disable { name } => {
            let settings = configure(api, &name, json!({"enabled": false})).await?;
            print_settings(&name, &settings);
            println!("The repository stays in the bucket.");
            Ok(())
        }
        BackupCommand::Now { name, kind } => {
            let id = database_id(api, &name).await?;
            let kind = match kind {
                Type::Full => "full",
                Type::Diff => "diff",
                Type::Incr => "incr",
            };
            let accepted = api
                .send(
                    Method::POST,
                    &format!("/databases/{}/backups", segment(&id)),
                    Some(json!({"kind": kind})),
                    "Backup failed",
                )
                .await?;
            println!("Backing up {name}…");
            super::wait_for_task(api, &accepted, "Backup failed").await?;
            match backups(api, &id).await?.first() {
                Some(backup) => println!(
                    "Backed up '{name}': {} backup {}, {} added to the bucket",
                    str(backup, "kind"),
                    str(backup, "label"),
                    size(backup["repository_bytes"].as_u64().unwrap_or(0)),
                ),
                None => println!("Backed up '{name}'"),
            }
            Ok(())
        }
        BackupCommand::List { name } => {
            let id = database_id(api, &name).await?;
            let settings = api
                .get(
                    &format!("/databases/{}/backups/settings", segment(&id)),
                    "List failed",
                )
                .await?;
            print_settings(&name, &settings);
            let backups = backups(api, &id).await?;
            if backups.is_empty() {
                println!("No backups.");
                return Ok(());
            }
            println!();
            println!(
                "{:<36} {:<5} {:<21} {:<10} IN BUCKET",
                "BACKUP", "TYPE", "ENDED", "SIZE"
            );
            for backup in &backups {
                println!(
                    "{:<36} {:<5} {:<21} {:<10} {}",
                    str(backup, "label"),
                    str(backup, "kind"),
                    str(backup, "finished_at"),
                    size(backup["bytes"].as_u64().unwrap_or(0)),
                    size(backup["repository_bytes"].as_u64().unwrap_or(0)),
                );
            }
            Ok(())
        }
        BackupCommand::Restore {
            name,
            new_name,
            time,
        } => {
            let id = database_id(api, &name).await?;
            let mut body = json!({"name": new_name});
            if let Some(time) = &time {
                body["time"] = json!(time);
            }
            let accepted = api
                .send(
                    Method::POST,
                    &format!("/databases/{}/backups/restore", segment(&id)),
                    Some(body),
                    "Restore failed",
                )
                .await?;
            println!("Restoring {name} into the new server {new_name}…");
            super::wait_for_task(api, &accepted, "Restore failed").await?;
            println!(
                "Restored '{name}' into the new server '{new_name}', at {}. Its backups are off.",
                time.as_deref().unwrap_or("the latest state")
            );
            Ok(())
        }
        BackupCommand::Verify { name } => {
            let id = database_id(api, &name).await?;
            let accepted = api
                .send(
                    Method::POST,
                    &format!("/databases/{}/backups/verify", segment(&id)),
                    None,
                    "Verification failed",
                )
                .await?;
            println!("Restoring the latest backup into a throwaway PostgreSQL…");
            super::wait_for_task(api, &accepted, "Verification failed").await?;
            println!("The backup restores. Every database answered a query.");
            Ok(())
        }
    }
}

/// Changes a server's backup settings and waits for the Task, if one ran.
async fn configure(api: &Api, name: &str, change: Value) -> anyhow::Result<Value> {
    let id = database_id(api, name).await?;
    let path = format!("/databases/{}/backups/settings", segment(&id));
    let settings = api
        .send(Method::PUT, &path, Some(change), "Change failed")
        .await?;
    if settings["task_id"].is_string() {
        super::wait_for_task(api, &settings, "Change failed").await?;
        return api.get(&path, "Change failed").await;
    }
    Ok(settings)
}

async fn backups(api: &Api, id: &str) -> anyhow::Result<Vec<Value>> {
    Ok(api
        .get(
            &format!("/databases/{}/backups", segment(id)),
            "List failed",
        )
        .await?
        .as_array()
        .cloned()
        .unwrap_or_default())
}

fn print_settings(name: &str, settings: &Value) {
    if settings["enabled"].as_bool() == Some(true) {
        println!(
            "Backups of {name}: on, restores go back {} days, next {} backup {}",
            settings["keep_days"],
            str(settings, "next_kind"),
            str(settings, "next_run_at")
        );
    } else {
        println!("Backups of {name}: off");
    }
    if let Some(archived) = settings["archiving"]["last_archived_at"].as_str() {
        println!("Last WAL segment archived: {archived}");
    }
    if let Some(problem) = settings["problem"].as_str() {
        println!("Problem: {problem}");
    }
}

fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

async fn storage(api: &Api, command: StorageCommand) -> anyhow::Result<()> {
    match command {
        StorageCommand::Show => {
            let storage = api.get("/backups/storage", "Show failed").await?;
            if storage["configured"].as_bool() != Some(true) {
                println!("No backup storage. Set one with 'self-host db backup storage set'.");
                return Ok(());
            }
            let endpoint = str(&storage, "endpoint");
            println!(
                "Endpoint:   {}",
                if endpoint.is_empty() { "AWS" } else { endpoint }
            );
            println!("Region:     {}", str(&storage, "region"));
            println!("Bucket:     {}", str(&storage, "bucket"));
            println!("Prefix:     {}", str(&storage, "prefix"));
            println!(
                "Addressing: {}",
                if storage["path_style"].as_bool() == Some(true) {
                    "path"
                } else {
                    "virtual-hosted"
                }
            );
            println!(
                "TLS:        {}",
                if storage["verify_tls"].as_bool() == Some(false) {
                    "certificate not verified"
                } else {
                    "certificate verified"
                }
            );
            println!("Access key: {}", str(&storage, "access_key_id"));
            println!("Secret:     saved, never shown");
        }
        StorageCommand::Set {
            endpoint,
            region,
            bucket,
            prefix,
            virtual_hosted,
            no_verify_tls,
            access_key_id,
            secret_access_key_file,
            no_test,
        } => {
            let saved = api.get("/backups/storage", "Show failed").await?;
            let mut body = json!({
                "endpoint": endpoint,
                "region": region,
                "bucket": bucket,
                "prefix": prefix,
                "path_style": !virtual_hosted,
                "verify_tls": !no_verify_tls,
                "access_key_id": access_key_id,
            });
            if let Some(secret) = secret(
                secret_access_key_file,
                saved["has_secret"].as_bool() == Some(true),
            )? {
                body["secret_access_key"] = json!(secret);
            }
            if !no_test {
                api.send(
                    Method::POST,
                    "/backups/storage/test",
                    Some(body.clone()),
                    "The backup storage did not pass its test; nothing was saved",
                )
                .await?;
            }
            api.send(Method::PUT, "/backups/storage", Some(body), "Save failed")
                .await?;
            println!(
                "Saved the backup storage{}",
                if no_test {
                    ""
                } else {
                    "; a test object was written, read and deleted"
                }
            );
        }
        StorageCommand::Test => {
            api.send(
                Method::POST,
                "/backups/storage/test",
                None,
                "The backup storage did not pass its test",
            )
            .await?;
            println!("A test object was written, read and deleted");
        }
        StorageCommand::Remove => {
            api.send(Method::DELETE, "/backups/storage", None, "Remove failed")
                .await?;
            println!("Removed the backup storage. The bucket keeps its repositories.");
        }
    }
    Ok(())
}

/// The secret access key: from the file, the environment, or a prompt. None
/// keeps the saved one, when a terminal answers the prompt with nothing.
fn secret(file: Option<PathBuf>, saved: bool) -> anyhow::Result<Option<String>> {
    if let Some(file) = file {
        let secret = std::fs::read_to_string(&file)
            .map_err(|error| anyhow::anyhow!("Could not read {}: {error}", file.display()))?;
        let secret = secret.trim();
        anyhow::ensure!(!secret.is_empty(), "{} is empty", file.display());
        return Ok(Some(secret.to_owned()));
    }
    if let Ok(secret) = std::env::var(SECRET_ENV)
        && !secret.is_empty()
    {
        return Ok(Some(secret));
    }
    let terminal = std::io::IsTerminal::is_terminal(&std::io::stdin());
    if !terminal {
        anyhow::ensure!(
            saved,
            "Give the secret access key with --secret-access-key-file or {SECRET_ENV}"
        );
        return Ok(None);
    }
    eprint!(
        "Secret access key{}: ",
        if saved {
            " (empty keeps the saved one)"
        } else {
            ""
        }
    );
    let typed = read_hidden()?;
    eprintln!();
    let typed = typed.trim();
    if typed.is_empty() {
        anyhow::ensure!(saved, "The secret access key is empty");
        return Ok(None);
    }
    Ok(Some(typed.to_owned()))
}

/// Reads one line from the terminal without showing it.
fn read_hidden() -> anyhow::Result<String> {
    let fd = libc::STDIN_FILENO;
    // SAFETY: `tcgetattr` and `tcsetattr` read and write only the `termios`
    // they are given, for a descriptor this process owns.
    let mut settings: libc::termios = unsafe { std::mem::zeroed() };
    let known = unsafe { libc::tcgetattr(fd, &mut settings) } == 0;
    if known {
        let mut hidden = settings;
        hidden.c_lflag &= !libc::ECHO;
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &hidden) };
    }
    let mut input = String::new();
    let read = std::io::stdin().read_line(&mut input);
    if known {
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &settings) };
    }
    read?;
    Ok(input)
}

async fn window(api: &Api, command: WindowCommand) -> anyhow::Result<()> {
    let window = match command {
        WindowCommand::Show => api.get("/maintenance-window", "Show failed").await?,
        WindowCommand::Set { start, hours } => {
            api.send(
                Method::PUT,
                "/maintenance-window",
                Some(json!({"start_hour": start, "hours": hours})),
                "Save failed",
            )
            .await?
        }
    };
    let start = window["start_hour"].as_u64().unwrap_or(0);
    let hours = window["hours"].as_u64().unwrap_or(0);
    println!(
        "Maintenance window: {start:02}:00 to {:02}:00, Host time (UTC{})",
        (start + hours) % 24,
        str(&window, "utc_offset")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_reads_in_the_unit_that_fits() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(1536), "1.5 KiB");
        assert_eq!(size(5 * 1024 * 1024), "5.0 MiB");
    }
}
