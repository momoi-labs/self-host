use std::env;

use self_host::build_app;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let api_key = env::var("SELF_HOST_API_KEY").unwrap_or_else(|_| "dev-key".into());
    let listen_addr = env::var("SELF_HOST_LISTEN").unwrap_or_else(|_| "127.0.0.1:3000".into());

    let app = build_app(api_key);

    let listener = tokio::net::TcpListener::bind(&listen_addr)
        .await
        .expect("failed to bind to listen address");

    info!("platform listening on {listen_addr}");

    axum::serve(listener, app).await.expect("server error");
}
