mod api;
mod config;
mod db;
mod error;
mod indexer;
mod keeper;
mod math;
mod models;
mod sui;

use std::sync::Arc;

use config::Config;
use sui::SuiClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "apex_backend=info,tower_http=info".into()),
        )
        .init();

    let cfg = Arc::new(Config::from_env()?);
    let pool = db::connect_and_migrate(&cfg.database_url).await?;
    tracing::info!("database connected, migrations applied");

    let sui = SuiClient::new(&cfg.sui_jsonrpc_url);

    match &cfg.apex_package_id {
        Some(package_id) => {
            indexer::spawn(pool.clone(), sui.clone(), package_id.clone());
        }
        None => tracing::warn!("indexer disabled: APEX_PACKAGE_ID not set"),
    }

    if cfg.keeper_enabled() {
        keeper::spawn(pool.clone(), sui.clone(), &cfg)?;
    } else {
        tracing::warn!(
            "keeper disabled: requires APEX_PACKAGE_ID, POOL_CONFIG_ID, KEEPER_CAP_ID and SUI_KEEPER_KEY"
        );
    }

    let state = api::AppState::new(pool, sui, cfg.clone());
    let app = api::router(state);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], cfg.port));
    tracing::info!(%addr, "API listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
