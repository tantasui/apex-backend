use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use super::AppState;
use crate::error::AppError;

pub async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "status": "ok",
        "indexer": state.cfg.indexer_enabled(),
        "keeper": state.cfg.keeper_enabled(),
    }))
}

/// Hero stats bar: active pools, dUSDC in escrow, distinct forecasters,
/// highest tier currently held.
pub async fn hero(State(state): State<AppState>) -> Result<Json<Value>, AppError> {
    let (active_pools, escrow): (i64, i64) = sqlx::query_as(
        r#"
        SELECT count(*), COALESCE(sum(prize_pool), 0)::BIGINT
        FROM pools WHERE state <> 'Distributed'
        "#,
    )
    .fetch_one(&state.db)
    .await?;

    let (forecasters,): (i64,) =
        sqlx::query_as("SELECT count(DISTINCT participant) FROM participants")
            .fetch_one(&state.db)
            .await?;

    let (top_tier,): (i16,) = sqlx::query_as(
        "SELECT COALESCE(max(tier), 0)::SMALLINT FROM frs_profiles WHERE pool_count > 0",
    )
    .fetch_one(&state.db)
    .await?;

    Ok(Json(json!({
        "active_pools": active_pools,
        "total_escrow": escrow.to_string(),
        "forecasters": forecasters,
        "top_tier": crate::models::tier_label(top_tier),
    })))
}
