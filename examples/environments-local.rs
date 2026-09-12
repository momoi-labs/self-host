//! Local console/API harness with its own state and no DNS or HTTP proxy.
//!
//! Run with `cargo run --example environments-local`, then open
//! http://127.0.0.1:13721/console and sign in with `local-environments`.
//! Environment actions use the real runner. Application actions use FakeDocker
//! so this harness cannot operate the Host's production Docker workloads.

use std::sync::Arc;

use self_host::{
    build_app, docker::FakeDocker, file_store::FileStateStore, metrics::Metrics,
    routes::FakeRoutes, store::StateStore,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let state_dir = std::env::var_os("SELF_HOST_ENVIRONMENTS_STATE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("self-host-environments-local"));
    let store = FileStateStore::open(&state_dir)?;
    store.initialize().await?;
    store.store_state("dns_suffix", "environments.test").await?;
    store.store_state("api_key", "local-environments").await?;
    let app = build_app(
        store,
        Arc::new(FakeDocker::new()),
        Arc::new(FakeRoutes::new()),
        Metrics::new(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:13721").await?;
    println!("Console: http://127.0.0.1:13721/console");
    println!("Development key: local-environments");
    println!("State: {}", state_dir.display());
    axum::serve(listener, app).await?;
    Ok(())
}
