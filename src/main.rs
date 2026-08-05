use std::env;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use self_host::bootstrap::{self, BootstrapResult, OPERATOR_API_PORT};
use self_host::build_app;
use self_host::config::CliConfig;
use self_host::db::{PgStateStore, StateStore};
use self_host::docker::{ComposeDocker, DockerRuntime};
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
    },
    /// List Applications
    List,
    /// Remove an Application and clean Docker resources
    Remove {
        /// Application name
        name: String,
    },
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
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        Some(Command::Apps { command }) => {
            if let Err(e) = run_apps_command(command).await {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        }
        Some(Command::Serve) | None => {
            run_server().await;
        }
    }
}

async fn run_init_command(dns_suffix: &str) -> Result<(), Box<dyn std::error::Error>> {
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
        Err(e) => return Err(Box::new(e)),
    }

    save_cli_config(&result)?;

    bootstrap::print_bootstrap_instructions(&result);

    Ok(())
}

async fn run_apps_command(command: AppsCommand) -> Result<(), Box<dyn std::error::Error>> {
    let config = CliConfig::load()?.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "CLI config not found; run 'self-host init' first",
        )
    })?;

    let client = reqwest::Client::new();

    match command {
        AppsCommand::Add { name, image, path } => {
            let image = image.unwrap_or_default();
            let path = path.unwrap_or_default();

            if image.is_empty() && path.is_empty() {
                return Err(
                    "Either --image or --path is required for deploy. Use --image <ref> or --path <dir>."
                        .into(),
                );
            }

            let url = format!("{}/apps", config.api_base_url.trim_end_matches('/'));
            let response = client
                .post(&url)
                .bearer_auth(&config.api_key)
                .json(&serde_json::json!({ "name": name, "image": image, "path": path }))
                .send()
                .await?;

            let status = response.status();
            let body = response.text().await?;
            if !status.is_success() {
                return Err(format!("Deploy failed ({status}): {body}").into());
            }

            let parsed: serde_json::Value = serde_json::from_str(&body)?;
            println!(
                "Deployed {} → http://{} ({})",
                parsed["name"].as_str().unwrap_or(&name),
                parsed["hostname"].as_str().unwrap_or("?"),
                parsed["status"].as_str().unwrap_or("?")
            );
        }
        AppsCommand::List => {
            let url = format!("{}/apps", config.api_base_url.trim_end_matches('/'));
            let response = client.get(&url).bearer_auth(&config.api_key).send().await?;

            let status = response.status();
            let body = response.text().await?;
            if !status.is_success() {
                return Err(format!("List failed ({status}): {body}").into());
            }

            let apps: Vec<serde_json::Value> = serde_json::from_str(&body)?;
            if apps.is_empty() {
                println!("No Applications deployed.");
                return Ok(());
            }

            println!("{:<20} {:<40} STATUS", "NAME", "HOSTNAME");
            for app in apps {
                println!(
                    "{:<20} {:<40} {}",
                    app["name"].as_str().unwrap_or("?"),
                    app["hostname"].as_str().unwrap_or("?"),
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
                return Err(format!("Remove failed ({status}): {body}").into());
            }

            println!("Removed Application '{name}'");
        }
    }

    Ok(())
}

async fn wait_for_postgres() -> Result<(), Box<dyn std::error::Error>> {
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
                return Err(Box::new(e));
            }
        }
    }

    Err("PostgreSQL did not become ready in time".into())
}

fn save_cli_config(result: &BootstrapResult) -> Result<(), Box<dyn std::error::Error>> {
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

    let app = build_app(store, docker);

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
