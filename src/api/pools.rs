use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::AppState;
use crate::error::AppError;
use crate::models::{ParticipantEntry, ParticipantRow, PoolDetail, PoolRow, PoolSummary, ResultEntry};

const POOL_COLUMNS: &str = r#"
    pool_id, leg_count, state, start_time_ms, commit_deadline_ms, reveal_deadline_ms,
    oracle_expiry_ms, entry_fee_amount, oracle_ids, oracle_results, participant_count,
    reveal_count, forfeited_count, prize_pool, total_weight, total_prize,
    initial_shared_version, predictions_table_id
"#;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// Open | Closed | Locked | Scored | Distributed (case-insensitive).
    pub state: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

fn normalize_state(s: &str) -> Result<String, AppError> {
    let canonical = match s.to_ascii_lowercase().as_str() {
        "open" => "Open",
        "closed" | "reveal" => "Closed",
        "locked" => "Locked",
        "scored" => "Scored",
        "distributed" | "settled" => "Distributed",
        other => {
            return Err(AppError::BadRequest(format!(
                "unknown pool state filter: {other}"
            )))
        }
    };
    Ok(canonical.to_string())
}

pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).max(0);

    let rows: Vec<PoolRow> = match &q.state {
        Some(s) => {
            let canonical = normalize_state(s)?;
            sqlx::query_as(&format!(
                "SELECT {POOL_COLUMNS} FROM pools WHERE state = $1
                 ORDER BY commit_deadline_ms DESC LIMIT $2 OFFSET $3"
            ))
            .bind(canonical)
            .bind(limit)
            .bind(offset)
            .fetch_all(&state.db)
            .await?
        }
        None => {
            sqlx::query_as(&format!(
                "SELECT {POOL_COLUMNS} FROM pools
                 ORDER BY commit_deadline_ms DESC LIMIT $1 OFFSET $2"
            ))
            .bind(limit)
            .bind(offset)
            .fetch_all(&state.db)
            .await?
        }
    };

    let pools: Vec<PoolSummary> = rows.iter().map(PoolSummary::from).collect();
    Ok(Json(json!({ "pools": pools })))
}

pub async fn fetch_pool(state: &AppState, pool_id: &str) -> Result<PoolRow, AppError> {
    let row: Option<PoolRow> =
        sqlx::query_as(&format!("SELECT {POOL_COLUMNS} FROM pools WHERE pool_id = $1"))
            .bind(pool_id)
            .fetch_optional(&state.db)
            .await?;
    row.ok_or_else(|| AppError::NotFound(format!("pool {pool_id} not found")))
}

pub async fn detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PoolDetail>, AppError> {
    let row = fetch_pool(&state, &id).await?;
    Ok(Json(PoolDetail::from(&row)))
}

pub async fn participants(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    fetch_pool(&state, &id).await?;
    let rows: Vec<ParticipantRow> = sqlx::query_as(
        r#"
        SELECT pool_id, participant, entry_fee, commit_timestamp_ms, revealed,
               crowd_median_estimate, predicted_prices, leg_hits, acc_score,
               time_bonus, composite_weight, apex_payout, paid
        FROM participants WHERE pool_id = $1 ORDER BY commit_timestamp_ms ASC
        "#,
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;

    let entries: Vec<ParticipantEntry> = rows
        .iter()
        .map(|r| ParticipantEntry {
            participant: r.participant.clone(),
            entry_fee: r.entry_fee.to_string(),
            commit_timestamp_ms: r.commit_timestamp_ms,
            revealed: r.revealed,
        })
        .collect();
    Ok(Json(json!({ "participants": entries })))
}

/// Full results, ranked by composite weight descending. Prediction values are
/// populated by the reconciler once revealed; scores once the pool is Scored.
pub async fn results(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let pool = fetch_pool(&state, &id).await?;
    let rows: Vec<ParticipantRow> = sqlx::query_as(
        r#"
        SELECT pool_id, participant, entry_fee, commit_timestamp_ms, revealed,
               crowd_median_estimate, predicted_prices, leg_hits, acc_score,
               time_bonus, composite_weight, apex_payout, paid
        FROM participants
        WHERE pool_id = $1 AND revealed
        ORDER BY COALESCE(composite_weight, '0')::NUMERIC DESC, commit_timestamp_ms ASC
        "#,
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;

    let entries: Vec<ResultEntry> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| ResultEntry {
            rank: i as i64 + 1,
            participant: r.participant.clone(),
            predicted_prices: r.predicted_prices.clone(),
            leg_hits: r.leg_hits,
            acc_score: r.acc_score.map(|v| v.to_string()),
            time_bonus: r.time_bonus.map(|v| v.to_string()),
            composite_weight: r.composite_weight.clone(),
            apex_payout: r.apex_payout.map(|v| v.to_string()),
        })
        .collect();

    Ok(Json(json!({
        "pool": PoolDetail::from(&pool),
        "results": entries,
    })))
}
