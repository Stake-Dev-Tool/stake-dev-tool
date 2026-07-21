use std::net::SocketAddr;

use server::config::Config;
use server::{AppState, db, http, storage};
use tracing_subscriber::{EnvFilter, fmt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::from_env()?;
    tracing::info!(bind = %config.bind_addr, "starting server");

    let pool = db::connect_lazy(&config.database_url)?;
    let store = storage::build_object_store(&config)?;
    let state = AppState::new(config, pool.clone(), store);

    // Migrations run in the background so a cold database never blocks the
    // server from binding and answering /healthz.
    tokio::spawn(db::run_migrations(pool));

    let bind_addr = state.config.bind_addr.clone();
    let app = http::build_router(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(addr = %listener.local_addr()?, "server listening");
    // `into_make_service_with_connect_info` surfaces the peer address so the
    // login rate limiter can key on it (behind a proxy, X-Forwarded-For wins).
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    Ok(())
}

async fn shutdown_signal() {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::error!(error = %e, "failed to listen for ctrl_c; shutting down");
    }
}
