use std::env;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use self_host::bootstrap::{self, BootstrapResult, OPERATOR_API_PORT};
use self_host::build_app;
use self_host::config::CliConfig;
use self_host::db::{PgStateStore, StateStore};
use self_host::docker::{ComposeDocker, DockerRuntime};
use self_host::error::ErrorReport;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "self-host", about = "LAN self-host PaaS platform")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Bootstrap the platform on this Host
    Init {
        /// DNS suffix for Application Hostnames (default: home.lan)
        #[arg(long, default_value = bootstrap::DEFAULT_DNS_SUFFIX)]
        dns: String,
        /// The LAN address Consumers reach this Host on (default: detected)
        #[arg(long)]
        host_ip: Option<String>,
    },
    /// Start the platform daemon
    Serve,
    /// Manage Applications
    Apps {
        #[command(subcommand)]
        command: AppsCommand,
    },
    /// Stream Application logs
    Logs {
        /// Application name
        app: String,
    },
    /// Reset the platform (remove all containers, volumes, and config)
    Reset {
        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },
    /// Install a Platform CA in this machine's trust store
    TrustCa {
        /// Remote Hostname or HTTPS base URL to download the public CA from
        #[arg(long)]
        from: Option<String>,
        /// Expected SHA-256 fingerprint, obtained from the Host by a trusted channel
        #[arg(long, requires = "from")]
        fingerprint: Option<String>,
    },
}

#[derive(Subcommand)]
enum AppsCommand {
    /// Deploy an Application from a Docker image or local build path
    Add {
        /// Application name (used in the default Hostname)
        #[arg(long)]
        name: String,
        /// Docker image reference
        #[arg(long)]
        image: Option<String>,
        /// Local build path (Dockerfile/context directory)
        #[arg(long)]
        path: Option<String>,
        /// A Compose file to run as the Application
        #[arg(long)]
        compose_file: Option<String>,
        /// The Compose service the Hostname routes to (default: the first
        /// one that publishes a port)
        #[arg(long)]
        web_service: Option<String>,
        /// The container port the Hostname routes to (default: the container
        /// side of the web service's first published port)
        #[arg(long)]
        web_port: Option<u16>,
        /// Override the default Application Hostname
        #[arg(long)]
        hostname: Option<String>,
        /// Extra Hostname the Application also answers on (repeatable)
        #[arg(long = "alias")]
        aliases: Vec<String>,
    },
    /// List Applications
    List,
    /// Start a stopped Application
    Start {
        /// Application name
        name: String,
    },
    /// Stop an Application; it stays stopped until started again
    Stop {
        /// Application name
        name: String,
    },
    /// Restart an Application's containers
    Restart {
        /// Application name
        name: String,
    },
    /// Remove an Application and clean Docker resources
    Remove {
        /// Application name
        name: String,
    },
    /// Manage Application environment variables
    Env {
        #[command(subcommand)]
        command: EnvCommand,
    },
}

#[derive(Subcommand)]
enum EnvCommand {
    /// Set an environment variable
    Set {
        /// Application name
        app: String,
        /// KEY=value pair
        key_value: String,
    },
    /// Get environment variables
    Get {
        /// Application name
        app: String,
        /// Optional: specific key to get
        key: Option<String>,
    },
    /// Remove an environment variable
    Unset {
        /// Application name
        app: String,
        /// Environment variable key
        key: String,
    },
}

/// A failure the platform reported over HTTP, back as a chain.
///
/// The API answers `{"error": ..., "caused_by": [...]}`. Rebuilding the layers
/// here means a failure on the Host prints exactly like one raised in the CLI,
/// instead of arriving as one line of JSON.
fn api_error(what: &str, status: reqwest::StatusCode, body: &str) -> anyhow::Error {
    let report = serde_json::from_str::<ErrorReport>(body)
        .unwrap_or_else(|_| ErrorReport::plain(body.trim()));

    chain_from(report).context(format!("{what} ({status})"))
}

/// Stacks a report back into an `anyhow` chain, innermost cause first, so
/// `{:?}` renders the layers the platform kept apart.
fn chain_from(report: ErrorReport) -> anyhow::Error {
    let mut causes = report.caused_by.into_iter().rev();

    let Some(innermost) = causes.next() else {
        return anyhow::Error::msg(report.error);
    };

    let mut err = anyhow::Error::msg(innermost);
    for cause in causes {
        err = err.context(cause);
    }
    err.context(report.error)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Command::Init { dns, host_ip }) => {
            if let Err(e) = run_init_command(&dns, host_ip.as_deref()).await {
                eprintln!("Error: {e:?}");
                std::process::exit(1);
            }
        }
        Some(Command::Apps { command }) => {
            if let Err(e) = run_apps_command(command).await {
                eprintln!("Error: {e:?}");
                std::process::exit(1);
            }
        }
        Some(Command::Logs { app }) => {
            if let Err(e) = run_logs_command(&app).await {
                eprintln!("Error: {e:?}");
                std::process::exit(1);
            }
        }
        Some(Command::Reset { force }) => {
            if let Err(e) = run_reset_command(force).await {
                eprintln!("Error: {e:?}");
                std::process::exit(1);
            }
        }
        Some(Command::TrustCa { from, fingerprint }) => {
            if let Err(e) = run_trust_ca_command(from.as_deref(), fingerprint.as_deref()).await {
                eprintln!("Error: {e:?}");
                std::process::exit(1);
            }
        }
        Some(Command::Serve) | None => {
            run_server().await;
        }
    }
}

async fn run_init_command(dns_suffix: &str, host_ip: Option<&str>) -> anyhow::Result<()> {
    info!("bootstrapping with DNS suffix: {dns_suffix}");

    let docker = ComposeDocker::new()?;

    docker.ping().await?;
    let db_name = self_host::apps::system_container_name("db");
    if docker.container_running(&db_name).await? {
        wait_for_postgres().await?;
        let store = PgStateStore::connect(bootstrap::PG_DB_URL).await?;
        if bootstrap::preflight_initialized(&store, dns_suffix).await? {
            let api_key = store
                .get_api_key()
                .await?
                .ok_or_else(|| anyhow::anyhow!("initialized Platform has no API key"))?;
            save_cli_config_values(dns_suffix, &api_key)?;
            println!("Platform is already initialized with DNS Suffix '{dns_suffix}'.");
            println!("Existing DNS, TLS and routing configuration is consistent.");
            println!("CLI config points to https://admin.{dns_suffix}.");
            return Ok(());
        }
    } else if bootstrap::platform_configuration_exists() {
        return Err(bootstrap::BootstrapError::ExistingConfiguration.into());
    }

    let result = bootstrap::run_bootstrap(&docker, dns_suffix, host_ip).await?;

    wait_for_postgres().await?;

    let store = PgStateStore::connect(bootstrap::PG_DB_URL).await?;

    match bootstrap::persist_bootstrap_state(&store, &result).await {
        Ok(()) => {}
        Err(bootstrap::BootstrapError::AlreadyInitialized) => {
            eprintln!("Platform is already initialized.");
            eprintln!("Start the daemon with: self-host serve");
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    }

    save_cli_config(&result)?;

    bootstrap::print_bootstrap_instructions(&result);

    Ok(())
}

fn run_checked(command: &mut std::process::Command, description: &str) -> anyhow::Result<()> {
    let status = command.status()?;
    if !status.success() {
        return Err(anyhow::anyhow!("{description} failed with {status}"));
    }
    Ok(())
}

fn remote_ca_url(source: &str) -> anyhow::Result<String> {
    let base = if source.contains("://") {
        source.to_string()
    } else {
        format!("https://{source}")
    };
    let url = reqwest::Url::parse(&base)?;
    if url.scheme() != "https" {
        return Err(anyhow::anyhow!(
            "the remote CA must be downloaded over HTTPS"
        ));
    }
    Ok(url.join("/ca.pem")?.to_string())
}

fn normalized_fingerprint(value: &str) -> Option<String> {
    let mut normalized = String::new();
    for character in value.chars() {
        if character.is_ascii_hexdigit() {
            normalized.push(character.to_ascii_uppercase());
        } else if character == ':' || character == '-' || character.is_ascii_whitespace() {
            continue;
        } else {
            return None;
        }
    }
    Some(normalized)
}

fn fingerprints_match(expected: &str, actual: &str) -> bool {
    match (
        normalized_fingerprint(expected),
        normalized_fingerprint(actual),
    ) {
        (Some(expected), Some(actual)) => expected == actual,
        _ => false,
    }
}

struct TemporaryCa(std::path::PathBuf);

impl Drop for TemporaryCa {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn write_temporary_ca(ca: &[u8]) -> anyhow::Result<TemporaryCa> {
    use std::io::Write;

    let path = std::env::temp_dir().join(format!(
        "self-host-ca-{}-{}.pem",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(ca)?;
    Ok(TemporaryCa(path))
}

async fn download_remote_ca(source: &str, expected_fingerprint: &str) -> anyhow::Result<Vec<u8>> {
    use futures_util::StreamExt;

    let normalized = normalized_fingerprint(expected_fingerprint)
        .filter(|fingerprint| fingerprint.len() == 64)
        .ok_or_else(|| {
            anyhow::anyhow!("expected fingerprint must contain 64 hexadecimal digits")
        })?;
    let url = remote_ca_url(source)?;
    let client = reqwest::Client::builder()
        // The fingerprint is the trust anchor for this first download.
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = client.get(&url).send().await?;
    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "failed to download {url}: HTTP {}",
            response.status()
        ));
    }

    const MAX_CA_SIZE: usize = 1024 * 1024;
    let mut ca = Vec::new();
    let mut body = response.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        if ca.len() + chunk.len() > MAX_CA_SIZE {
            return Err(anyhow::anyhow!("downloaded CA exceeds 1 MiB"));
        }
        ca.extend_from_slice(&chunk);
    }

    self_host::tls::validate_public_ca(&ca)?;
    let actual = self_host::tls::ca_sha256_fingerprint_from_pem(&ca)?;
    if !fingerprints_match(&normalized, &actual) {
        return Err(anyhow::anyhow!(
            "CA fingerprint mismatch: expected {expected_fingerprint}, received {actual}"
        ));
    }
    Ok(ca)
}

fn install_ca(ca_path: &std::path::Path) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    run_checked(
        std::process::Command::new("sudo")
            .args([
                "security",
                "add-trusted-cert",
                "-d",
                "-r",
                "trustRoot",
                "-k",
            ])
            .arg("/Library/Keychains/System.keychain")
            .arg(ca_path),
        "installing the CA in the System Keychain",
    )?;

    #[cfg(target_os = "linux")]
    {
        if std::path::Path::new("/etc/arch-release").exists() {
            run_checked(
                std::process::Command::new("sudo")
                    .arg("trust")
                    .arg("anchor")
                    .arg("--store")
                    .arg(ca_path),
                "installing the CA with p11-kit",
            )?;
        } else {
            let destination = "/usr/local/share/ca-certificates/self-host-ca.crt";
            run_checked(
                std::process::Command::new("sudo")
                    .arg("cp")
                    .arg(ca_path)
                    .arg(destination),
                "copying the CA into the system trust store",
            )?;
            run_checked(
                std::process::Command::new("sudo").arg("update-ca-certificates"),
                "updating the system trust store",
            )?;
        }

        if std::process::Command::new("certutil")
            .arg("-H")
            .output()
            .is_ok()
        {
            let nss_db = dirs::home_dir()
                .ok_or_else(|| anyhow::anyhow!("cannot locate the home directory"))?
                .join(".pki/nssdb");
            std::fs::create_dir_all(&nss_db)?;
            let database = format!("sql:{}", nss_db.display());
            if !nss_db.join("cert9.db").exists() {
                run_checked(
                    std::process::Command::new("certutil")
                        .args(["-N", "--empty-password", "-d"])
                        .arg(&database),
                    "creating the browser certificate database",
                )?;
            }
            run_checked(
                std::process::Command::new("certutil")
                    .args(["-A", "-n", "Self-Host LAN CA", "-t", "C,,", "-i"])
                    .arg(ca_path)
                    .arg("-d")
                    .arg(&database),
                "installing the CA for Chromium-based browsers",
            )?;
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    return Err(anyhow::anyhow!(
        "automatic Host trust is supported only on Linux and macOS"
    ));

    #[cfg(target_os = "linux")]
    println!("Fully restart open browsers before testing HTTPS.");
    Ok(())
}

async fn run_trust_ca_command(
    from: Option<&str>,
    expected_fingerprint: Option<&str>,
) -> anyhow::Result<()> {
    if let Some(source) = from {
        let expected = expected_fingerprint.ok_or_else(|| {
            anyhow::anyhow!(
                "--fingerprint is required with --from; obtain it from the Host by a trusted channel"
            )
        })?;
        let ca = download_remote_ca(source, expected).await?;
        let fingerprint = self_host::tls::ca_sha256_fingerprint_from_pem(&ca)?;
        let temporary = write_temporary_ca(&ca)?;
        install_ca(&temporary.0)?;
        println!("Trusted CA downloaded from {source}");
        println!("SHA-256: {fingerprint}");
        return Ok(());
    }

    let ca_path = self_host::tls::ca_cert_path();
    if !ca_path.exists() {
        return Err(anyhow::anyhow!(
            "CA certificate not found; run 'self-host init' or use 'self-host trust-ca --from <host> --fingerprint <sha256>'"
        ));
    }
    install_ca(&ca_path)?;
    println!("Trusted CA: {}", ca_path.display());
    println!("SHA-256: {}", self_host::tls::ca_sha256_fingerprint()?);
    Ok(())
}

/// Polls an Application until its deploy settles. There is no upper bound on
/// how long a `docker pull` takes, so there is none here either — Ctrl-C is
/// the way out, and the Application keeps deploying without us.
async fn wait_for_deploy(
    client: &reqwest::Client,
    config: &CliConfig,
    id: &str,
) -> anyhow::Result<serde_json::Value> {
    let url = format!(
        "{}/apps/id/{}",
        config.api_base_url.trim_end_matches('/'),
        id
    );

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let response = client.get(&url).bearer_auth(&config.api_key).send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(api_error("Deploy status unavailable", status, &body));
        }

        let parsed: serde_json::Value = serde_json::from_str(&body)?;
        if parsed["status"].as_str() != Some("pending") {
            return Ok(parsed);
        }
    }
}

async fn run_apps_command(command: AppsCommand) -> anyhow::Result<()> {
    let config = CliConfig::load()?.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "CLI config not found; run 'self-host init' first",
        )
    })?;

    let client = reqwest::Client::new();

    match command {
        AppsCommand::Add {
            name,
            image,
            path,
            compose_file,
            web_service,
            web_port,
            hostname,
            aliases,
        } => {
            let image = image.unwrap_or_default();
            let path = path.unwrap_or_default();
            let compose = match &compose_file {
                Some(file) => std::fs::read_to_string(file)
                    .map_err(|e| anyhow::anyhow!("failed to read {file}: {e}"))?,
                None => String::new(),
            };

            if image.is_empty() && path.is_empty() && compose.is_empty() {
                return Err(anyhow::anyhow!(
                    "One of --image, --path or --compose-file is required for deploy."
                ));
            }

            let mut body = serde_json::json!({
                "name": name, "image": image, "path": path, "compose": compose
            });
            if let Some(service) = &web_service {
                body["web_service"] = serde_json::json!(service);
            }
            if let Some(port) = web_port {
                body["web_port"] = serde_json::json!(port);
            }
            if let Some(h) = &hostname {
                body["hostname"] = serde_json::json!(h);
            }
            if !aliases.is_empty() {
                body["aliases"] = serde_json::json!(aliases);
            }

            let url = format!("{}/apps", config.api_base_url.trim_end_matches('/'));
            let response = client
                .post(&url)
                .bearer_auth(&config.api_key)
                .json(&body)
                .send()
                .await?;

            let status = response.status();
            let body = response.text().await?;
            if !status.is_success() {
                return Err(api_error("Deploy failed", status, &body));
            }

            let parsed: serde_json::Value = serde_json::from_str(&body)?;

            // The API answers as soon as the Application is on record and
            // pulls in the background. An operator at a terminal is waiting
            // for the outcome, so wait for it here.
            let deployed = if parsed["status"].as_str() == Some("pending") {
                println!("Deploying {name}…");
                wait_for_deploy(&client, &config, parsed["id"].as_str().unwrap_or_default()).await?
            } else {
                parsed
            };

            if deployed["status"].as_str() == Some("failed") {
                let report = serde_json::from_value(deployed["last_error"].clone())
                    .unwrap_or_else(|_| ErrorReport::plain("unknown error"));
                return Err(chain_from(report).context("Deploy failed"));
            }

            println!(
                "Deployed {} → https://{} ({})",
                deployed["name"].as_str().unwrap_or(&name),
                deployed["hostname"].as_str().unwrap_or("?"),
                deployed["status"].as_str().unwrap_or("?")
            );
            for alias in deployed["aliases"].as_array().unwrap_or(&vec![]) {
                println!("  also at https://{}", alias.as_str().unwrap_or("?"));
            }
        }
        AppsCommand::List => {
            let url = format!("{}/apps", config.api_base_url.trim_end_matches('/'));
            let response = client.get(&url).bearer_auth(&config.api_key).send().await?;

            let status = response.status();
            let body = response.text().await?;
            if !status.is_success() {
                return Err(api_error("List failed", status, &body));
            }

            let apps: Vec<serde_json::Value> = serde_json::from_str(&body)?;
            if apps.is_empty() {
                println!("No Applications deployed.");
                return Ok(());
            }

            println!("{:<20} {:<40} {:<10} STATUS", "NAME", "HOSTNAME", "SOURCE");
            for app in apps {
                println!(
                    "{:<20} {:<40} {:<10} {}",
                    app["name"].as_str().unwrap_or("?"),
                    app["hostname"].as_str().unwrap_or("?"),
                    app["source"].as_str().unwrap_or("?"),
                    app["status"].as_str().unwrap_or("?")
                );
            }
        }
        AppsCommand::Start { name } => run_lifecycle(&config, &client, &name, "start").await?,
        AppsCommand::Stop { name } => run_lifecycle(&config, &client, &name, "stop").await?,
        AppsCommand::Restart { name } => run_lifecycle(&config, &client, &name, "restart").await?,
        AppsCommand::Remove { name } => {
            let url = format!(
                "{}/apps/{}",
                config.api_base_url.trim_end_matches('/'),
                name
            );
            let response = client
                .delete(&url)
                .bearer_auth(&config.api_key)
                .send()
                .await?;

            let status = response.status();
            if !status.is_success() {
                let body = response.text().await?;
                return Err(api_error("Remove failed", status, &body));
            }

            println!("Removed Application '{name}'");
        }
        AppsCommand::Env { command } => {
            run_env_command(&config, &client, command).await?;
        }
    }

    Ok(())
}

/// Start, stop and restart are addressed by id on the API; the CLI speaks in
/// names, so it looks the id up first.
async fn run_lifecycle(
    config: &CliConfig,
    client: &reqwest::Client,
    name: &str,
    verb: &str,
) -> anyhow::Result<()> {
    let base = config.api_base_url.trim_end_matches('/');

    let response = client
        .get(format!("{base}/apps"))
        .bearer_auth(&config.api_key)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(api_error("List failed", status, &body));
    }
    let apps: Vec<serde_json::Value> = serde_json::from_str(&body)?;
    let id = apps
        .iter()
        .find(|a| a["name"].as_str() == Some(name))
        .and_then(|a| a["id"].as_str())
        .ok_or_else(|| anyhow::anyhow!("Application '{name}' not found"))?;

    let response = client
        .post(format!("{base}/apps/id/{id}/{verb}"))
        .bearer_auth(&config.api_key)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(api_error(&format!("{verb} failed"), status, &body));
    }

    let app: serde_json::Value = serde_json::from_str(&body)?;
    println!("{name}: {}", app["status"].as_str().unwrap_or("?"));
    for service in app["services"].as_array().unwrap_or(&vec![]) {
        println!(
            "  {:<20} {}",
            service["service"].as_str().unwrap_or("?"),
            service["state"].as_str().unwrap_or("?")
        );
    }
    Ok(())
}

async fn run_env_command(
    config: &CliConfig,
    client: &reqwest::Client,
    command: EnvCommand,
) -> anyhow::Result<()> {
    let base = config.api_base_url.trim_end_matches('/');

    match command {
        EnvCommand::Set { app, key_value } => {
            let (key, value) = key_value
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("env must be in KEY=value format"))?;
            let url = format!("{base}/apps/{app}/env");
            let response = client
                .post(&url)
                .bearer_auth(&config.api_key)
                .json(&serde_json::json!({"key": key, "value": value}))
                .send()
                .await?;

            let status = response.status();
            if !status.is_success() {
                let body = response.text().await?;
                return Err(api_error("env set failed", status, &body));
            }
            println!("Set {key}={value} on '{app}'");
        }
        EnvCommand::Get { app, key } => {
            if let Some(key) = key {
                let url = format!("{base}/apps/{app}/env");
                let response = client.get(&url).bearer_auth(&config.api_key).send().await?;
                let status = response.status();
                if !status.is_success() {
                    let body = response.text().await?;
                    return Err(api_error("env get failed", status, &body));
                }
                let vars: Vec<(String, String)> = response.json().await?;
                if let Some((_, value)) = vars.iter().find(|(k, _)| k == &key) {
                    println!("{value}");
                } else {
                    eprintln!("env key '{key}' not found on '{app}'");
                }
            } else {
                let url = format!("{base}/apps/{app}/env");
                let response = client.get(&url).bearer_auth(&config.api_key).send().await?;
                let status = response.status();
                if !status.is_success() {
                    let body = response.text().await?;
                    return Err(api_error("env get failed", status, &body));
                }
                let vars: Vec<(String, String)> = response.json().await?;
                if vars.is_empty() {
                    println!("No env vars set on '{app}'");
                } else {
                    for (k, v) in &vars {
                        println!("{k}={v}");
                    }
                }
            }
        }
        EnvCommand::Unset { app, key } => {
            let url = format!("{base}/apps/{app}/env/{key}");
            let response = client
                .delete(&url)
                .bearer_auth(&config.api_key)
                .send()
                .await?;

            let status = response.status();
            if !status.is_success() {
                let body = response.text().await?;
                return Err(api_error("env unset failed", status, &body));
            }
            println!("Removed env '{key}' from '{app}'");
        }
    }

    Ok(())
}

async fn run_logs_command(app_name: &str) -> anyhow::Result<()> {
    let config = CliConfig::load()?.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "CLI config not found; run 'self-host init' first",
        )
    })?;

    let url = format!(
        "{}/apps/{}/logs",
        config.api_base_url.trim_end_matches('/'),
        app_name
    );

    let response = reqwest::Client::new()
        .get(&url)
        .bearer_auth(&config.api_key)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await?;
        return Err(api_error("Logs failed", status, &body));
    }

    use futures_util::StreamExt;
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let text = String::from_utf8_lossy(&chunk);

        for line in text.lines() {
            if let Some(data) = line.strip_prefix("data: ")
                && data != "keepalive"
            {
                println!("{data}");
            }
        }
    }

    Ok(())
}

async fn run_reset_command(force: bool) -> anyhow::Result<()> {
    if !force {
        println!("This will remove all self-host containers, volumes, and configuration.");
        println!("All deployed applications will be lost.");
        print!("Continue? [y/N] ");
        std::io::Write::flush(&mut std::io::stdout())?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Reset cancelled.");
            return Ok(());
        }
    }

    println!("Stopping and removing containers...");
    let mut container_ids = Vec::new();
    for name in ["self-host", "sf-"] {
        let output = std::process::Command::new("docker")
            .args(["ps", "-aq", "--filter", &format!("name={name}")])
            .output()?;
        container_ids.extend(
            String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .map(str::to_string),
        );
    }
    container_ids.sort();
    container_ids.dedup();
    if !container_ids.is_empty() {
        let mut remove_cmd = std::process::Command::new("docker");
        remove_cmd.arg("rm").arg("-f");
        for id in container_ids {
            remove_cmd.arg(id);
        }
        remove_cmd.output()?;
        println!("Containers removed.");
    } else {
        println!("No containers found.");
    }

    // A Compose Application's named volumes are created under its project,
    // so they are sf-app-<id>_<volume> and the self-host prefix alone leaves
    // them behind. Repeated name filters are an OR.
    println!("Removing volumes...");
    let output = std::process::Command::new("docker")
        .args([
            "volume",
            "ls",
            "-q",
            "--filter",
            "name=self-host",
            "--filter",
            "name=^sf-app-",
        ])
        .output()?;

    let volume_names = String::from_utf8_lossy(&output.stdout);
    if !volume_names.trim().is_empty() {
        let mut remove_cmd = std::process::Command::new("docker");
        remove_cmd.arg("volume").arg("rm");
        for name in volume_names.split_whitespace() {
            remove_cmd.arg(name);
        }
        remove_cmd.output()?;
        println!("Volumes removed.");
    } else {
        println!("No volumes found.");
    }

    println!("Removing networks...");
    let output = std::process::Command::new("docker")
        .args(["network", "ls", "-q", "--filter", "name=sf-"])
        .output()?;
    let mut network_ids: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    network_ids.push("self-host".into());
    let mut remove_cmd = std::process::Command::new("docker");
    remove_cmd.arg("network").arg("rm");
    for id in network_ids {
        remove_cmd.arg(id);
    }
    let _ = remove_cmd.output();
    println!("Networks removed.");

    let config_dir = self_host::compose::platform_config_dir();
    if config_dir.exists() {
        println!("Removing configuration directory...");
        std::fs::remove_dir_all(&config_dir)?;
        println!("Configuration removed.");
    } else {
        println!("No configuration directory found.");
    }

    println!("\nReset complete. You can run 'self-host init' to start fresh.");
    Ok(())
}

/// Polls Docker until it answers. The first minute after boot is the usual
/// wait; a runtime that never comes is logged once a minute, forever.
async fn wait_for_docker(docker: &ComposeDocker) {
    let mut attempt: u32 = 0;
    loop {
        match docker.ping().await {
            Ok(()) => {
                if attempt > 0 {
                    info!("Docker is available");
                }
                return;
            }
            Err(e) => {
                attempt += 1;
                if attempt == 1 || attempt.is_multiple_of(30) {
                    tracing::warn!("waiting for Docker: {e}");
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            }
        }
    }
}

/// Polls the state store until it answers. Without one there is nothing to
/// serve: every Application and the API key live in it.
async fn wait_for_state_store() -> PgStateStore {
    let mut attempt: u32 = 0;
    loop {
        match PgStateStore::connect(bootstrap::PG_DB_URL).await {
            Ok(store) => {
                if attempt > 0 {
                    info!("PostgreSQL is available");
                }
                return store;
            }
            Err(e) => {
                attempt += 1;
                if attempt == 1 || attempt.is_multiple_of(30) {
                    tracing::warn!(
                        "waiting for PostgreSQL (run 'self-host init' if this Host was never bootstrapped): {e}"
                    );
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            }
        }
    }
}

async fn wait_for_postgres() -> anyhow::Result<()> {
    let max_attempts = 30;
    for attempt in 1..=max_attempts {
        match PgStateStore::connect(bootstrap::PG_DB_URL).await {
            Ok(_) => {
                info!("PostgreSQL is ready");
                return Ok(());
            }
            Err(_) if attempt < max_attempts => {
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
            Err(e) => {
                return Err(e.into());
            }
        }
    }

    Err(anyhow::anyhow!("PostgreSQL did not become ready in time"))
}

fn cli_config_from_bootstrap(result: &BootstrapResult) -> CliConfig {
    cli_config(&result.dns_suffix, &result.api_key)
}

fn cli_config(dns_suffix: &str, api_key: &str) -> CliConfig {
    CliConfig {
        api_base_url: format!("https://admin.{dns_suffix}"),
        api_key: api_key.to_string(),
    }
}

fn save_cli_config_values(dns_suffix: &str, api_key: &str) -> anyhow::Result<()> {
    save_cli_config_object(cli_config(dns_suffix, api_key))
}

fn save_cli_config_object(config: CliConfig) -> anyhow::Result<()> {
    config.save()?;

    info!("CLI config saved to {}", CliConfig::config_path().display());

    Ok(())
}

fn save_cli_config(result: &BootstrapResult) -> anyhow::Result<()> {
    save_cli_config_object(cli_config_from_bootstrap(result))
}

/// The daemon is started by launchd at boot (ADR-0013), when the Docker
/// runtime is still coming up and the Platform Infra with it. Nothing here
/// gives up: each dependency is waited for, out loud, for as long as it
/// takes. Exiting would only make launchd start us again with less context.
async fn run_server() {
    let (api_key, listen_addr) = resolve_server_config().await;

    let compose_docker = match ComposeDocker::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("Docker unavailable: {e}");
            std::process::exit(1);
        }
    };

    wait_for_docker(&compose_docker).await;

    // Docker's restart policy brings the Infra back after a reboot; this
    // covers the rest, and is a no-op when everything is already up.
    if let Err(e) = compose_docker.infra_up() {
        tracing::warn!(
            "failed to bring the Platform Infra up: {}",
            ErrorReport::new(&e)
        );
    }

    let docker: Arc<dyn DockerRuntime> = Arc::new(compose_docker);

    let store = wait_for_state_store().await;

    // Ensure schema includes applications table for existing installs.
    if let Err(e) = store.initialize_schema().await {
        tracing::warn!("failed to initialize schema: {e}");
    }

    // If we have a DB but no stored API key, use env var
    if store.get_api_key().await.ok().flatten().is_none() && !api_key.is_empty() {
        let _ = store.store_state("api_key", &api_key).await;
    }

    // A row left `pending` by a restart is nobody's deploy any more; settle it
    // against what Docker is actually running before serving.
    let routes: Arc<dyn self_host::routes::RouteStore> =
        Arc::new(self_host::routes::FileRoutes::new());

    if let Err(e) = self_host::apps::reconcile(&store, docker.as_ref(), routes.as_ref()).await {
        tracing::warn!("failed to reconcile Applications: {e}");
    }

    // Log the admin dashboard URL if we know the DNS suffix.
    if let Ok(Some(dns_suffix)) = store.get_state("dns_suffix").await {
        info!("admin dashboard: https://admin.{dns_suffix}");
    }

    let app = build_app(store, docker, routes);

    let listener = tokio::net::TcpListener::bind(&listen_addr)
        .await
        .expect("failed to bind to listen address");

    info!("listening on {listen_addr}");

    axum::serve(listener, app).await.expect("server error");
}

async fn resolve_server_config() -> (String, String) {
    let api_key = env::var("SELF_HOST_API_KEY").unwrap_or_default();
    let listen_addr =
        env::var("SELF_HOST_LISTEN").unwrap_or_else(|_| format!("0.0.0.0:{OPERATOR_API_PORT}"));

    (api_key, listen_addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The layers, not the rendering: `{:?}` on an anyhow error also carries a
    /// backtrace wherever `RUST_BACKTRACE` is set, and CI sets it.
    #[test]
    fn an_api_failure_comes_back_as_a_chain() {
        let body = r#"{"error":"failed to deploy the Application",
                       "caused_by":["failed to pull image 'b'",
                                    "Error response from daemon"]}"#;

        let err = api_error(
            "Deploy failed",
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body,
        );

        let layers: Vec<String> = err.chain().map(|layer| layer.to_string()).collect();

        assert_eq!(
            layers,
            [
                "Deploy failed (500 Internal Server Error)",
                "failed to deploy the Application",
                "failed to pull image 'b'",
                "Error response from daemon",
            ]
        );
    }

    #[test]
    fn a_body_that_is_not_a_report_is_still_the_failure() {
        let err = api_error(
            "List failed",
            reqwest::StatusCode::BAD_GATEWAY,
            "  gateway down  ",
        );

        assert_eq!(err.to_string(), "List failed (502 Bad Gateway)");
        assert_eq!(err.source().unwrap().to_string(), "gateway down");
    }

    #[test]
    fn remote_ca_requires_the_expected_fingerprint() {
        let actual = "AA:BB:CC";

        assert!(fingerprints_match("aa bb cc", actual));
        assert!(!fingerprints_match("AA:BB:CD", actual));
    }

    #[test]
    fn remote_ca_url_accepts_a_hostname_or_base_url() {
        assert_eq!(
            remote_ca_url("admin.home.lan").unwrap(),
            "https://admin.home.lan/ca.pem"
        );
        assert_eq!(
            remote_ca_url("https://admin.home.lan/").unwrap(),
            "https://admin.home.lan/ca.pem"
        );
        assert!(remote_ca_url("http://admin.home.lan").is_err());
    }

    #[test]
    fn bootstrap_cli_config_uses_the_public_https_endpoint() {
        let result = BootstrapResult {
            dns_suffix: "home.lan".into(),
            api_key: "secret".into(),
            api_listen_addr: "0.0.0.0:3721".into(),
            host_ip: "192.168.1.10".into(),
        };

        let config = cli_config_from_bootstrap(&result);

        assert_eq!(config.api_base_url, "https://admin.home.lan");
        assert_eq!(config.api_key, "secret");
    }
}
