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
        /// Override the default Application Hostname
        #[arg(long)]
        hostname: Option<String>,
        /// Extra Hostname the Application also answers on (repeatable)
        #[arg(long = "alias")]
        aliases: Vec<String>,
    },
    /// List Applications
    List,
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
        Some(Command::Init { dns }) => {
            if let Err(e) = run_init_command(&dns).await {
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
        Some(Command::Serve) | None => {
            run_server().await;
        }
    }
}

async fn run_init_command(dns_suffix: &str) -> anyhow::Result<()> {
    info!("bootstrapping with DNS suffix: {dns_suffix}");

    let docker = ComposeDocker::new()?;

    let result = bootstrap::run_bootstrap(&docker, dns_suffix).await?;

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
            hostname,
            aliases,
        } => {
            let image = image.unwrap_or_default();
            let path = path.unwrap_or_default();

            if image.is_empty() && path.is_empty() {
                return Err(anyhow::anyhow!(
                    "Either --image or --path is required for deploy. Use --image <ref> or --path <dir>."
                ));
            }

            let mut body = serde_json::json!({ "name": name, "image": image, "path": path });
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
    let output = std::process::Command::new("docker")
        .args(["ps", "-aq", "--filter", "name=self-host"])
        .output()?;

    let container_ids = String::from_utf8_lossy(&output.stdout);
    if !container_ids.trim().is_empty() {
        let mut remove_cmd = std::process::Command::new("docker");
        remove_cmd.arg("rm").arg("-f");
        for id in container_ids.split_whitespace() {
            remove_cmd.arg(id);
        }
        remove_cmd.output()?;
        println!("Containers removed.");
    } else {
        println!("No containers found.");
    }

    println!("Removing volumes...");
    let output = std::process::Command::new("docker")
        .args(["volume", "ls", "-q", "--filter", "name=self-host"])
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

    println!("Removing network...");
    let _ = std::process::Command::new("docker")
        .args(["network", "rm", "self-host"])
        .output();
    println!("Network removed.");

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

fn save_cli_config(result: &BootstrapResult) -> anyhow::Result<()> {
    let config = CliConfig {
        api_base_url: format!("http://{}", result.api_listen_addr),
        api_key: result.api_key.clone(),
    };

    config.save()?;

    info!("CLI config saved to {}", CliConfig::config_path().display());

    Ok(())
}

async fn run_server() {
    let (api_key, listen_addr) = resolve_server_config().await;

    let docker: Arc<dyn DockerRuntime> = match ComposeDocker::new() {
        Ok(d) => Arc::new(d),
        Err(e) => {
            tracing::error!("Docker unavailable: {e}");
            std::process::exit(1);
        }
    };

    let store = match PgStateStore::connect(bootstrap::PG_DB_URL).await {
        Ok(s) => s,
        Err(_) => {
            tracing::warn!("PostgreSQL not available, running without persistent state");
            let app = build_app_with_key(api_key);
            let listener = tokio::net::TcpListener::bind(&listen_addr)
                .await
                .expect("failed to bind to listen address");
            info!("listening on {listen_addr} (no DB)");
            axum::serve(listener, app).await.expect("server error");
            return;
        }
    };

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

fn build_app_with_key(api_key: String) -> axum::Router {
    use axum::{
        Json, Router,
        extract::Request,
        http::StatusCode,
        middleware::{self, Next},
        response::{IntoResponse, Response},
        routing::get,
    };
    use serde::Serialize;

    #[derive(Clone)]
    struct SimpleState {
        api_key: String,
    }

    #[derive(Serialize)]
    struct HealthResponse {
        status: String,
    }

    async fn health() -> Json<HealthResponse> {
        Json(HealthResponse {
            status: "ok".into(),
        })
    }

    async fn require_key(
        state: axum::extract::State<SimpleState>,
        req: Request,
        next: Next,
    ) -> Response {
        let auth_header = req
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));

        let expected = state.api_key.as_bytes();
        let found = auth_header.unwrap_or("").as_bytes();

        let matches = expected.len() == found.len()
            && expected
                .iter()
                .zip(found.iter())
                .fold(0, |acc, (x, y)| acc | (x ^ y))
                == 0;

        if matches {
            next.run(req).await
        } else {
            (StatusCode::UNAUTHORIZED, "invalid api key").into_response()
        }
    }

    let state = SimpleState { api_key };

    Router::new()
        .route("/health", get(health))
        .layer(middleware::from_fn_with_state(state.clone(), require_key))
        .with_state(state)
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

    #[test]
    fn an_api_failure_prints_as_a_chain() {
        let body = r#"{"error":"failed to start the Application container",
                       "caused_by":["Docker unavailable: pull access denied for a"]}"#;

        let err = api_error(
            "Deploy failed",
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body,
        );

        assert_eq!(
            format!("{err:?}"),
            "Deploy failed (500 Internal Server Error)\n\n\
             Caused by:\n\
             \x20   0: failed to start the Application container\n\
             \x20   1: Docker unavailable: pull access denied for a"
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
}
