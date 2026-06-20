pub mod admin;
pub mod leaderboard;
pub mod pools;
pub mod preview;
pub mod stats;
pub mod users;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::routing::{get, post};
use axum::Router;
use sqlx::PgPool;
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::config::Config;
use crate::sui::types::{OracleSvi, PoolConfigData};
use crate::sui::SuiClient;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub sui: SuiClient,
    pub cfg: Arc<Config>,
    /// oracle_id -> (fetched_at, parsed oracle). 10s TTL.
    pub oracle_cache: Arc<Mutex<HashMap<String, (Instant, OracleSvi)>>>,
    /// (fetched_at, PoolConfig). 60s TTL.
    pub pool_config_cache: Arc<Mutex<Option<(Instant, PoolConfigData)>>>,
}

impl AppState {
    pub fn new(db: PgPool, sui: SuiClient, cfg: Arc<Config>) -> Self {
        Self {
            db,
            sui,
            cfg,
            oracle_cache: Arc::new(Mutex::new(HashMap::new())),
            pool_config_cache: Arc::new(Mutex::new(None)),
        }
    }
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/stats/hero", get(stats::hero))
        .route("/pools", get(pools::list))
        .route("/pools/{id}", get(pools::detail))
        .route("/pools/{id}/participants", get(pools::participants))
        .route("/pools/{id}/results", get(pools::results))
        .route("/pools/{id}/range-preview", get(preview::range_preview))
        .route("/pools/{id}/scoring-preview", get(preview::scoring_preview))
        .route("/pools/{id}/users/{address}", get(users::pool_status))
        .route("/users/{address}/frs", get(users::frs_profile))
        .route("/users/{address}/accuracy-history", get(users::accuracy_history))
        .route("/users/{address}/pools", get(users::pool_history))
        .route("/users/{address}/rank", get(users::rank))
        .route("/leaderboard", get(leaderboard::leaderboard));

    let admin = Router::new()
        .route("/pools", post(admin::create_pool));

    Router::new()
        .route("/health", get(stats::health))
        .nest("/api/v1", api)
        .nest("/admin", admin)
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
