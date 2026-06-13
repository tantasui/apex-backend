use axum::extract::{Path, State};
use axum::Json;
use serde_json::{json, Value};

use super::AppState;
use crate::error::AppError;
use crate::models::{tier_label, FrsProfileRow, ParticipantRow};

type PoolHistoryRow = (
    String,
    String,
    i64,
    i64,
    bool,
    Option<i64>,
    Option<i64>,
    Option<Value>,
    Option<Value>,
);

/// Status of one user inside one pool, driving the action panel state.
pub async fn pool_status(
    State(state): State<AppState>,
    Path((pool_id, address)): Path<(String, String)>,
) -> Result<Json<Value>, AppError> {
    let pool = super::pools::fetch_pool(&state, &pool_id).await?;
    let row: Option<ParticipantRow> = sqlx::query_as(
        r#"
        SELECT pool_id, participant, entry_fee, commit_timestamp_ms, revealed,
               crowd_median_estimate, predicted_prices, leg_hits, acc_score,
               time_bonus, composite_weight, apex_payout, paid
        FROM participants WHERE pool_id = $1 AND participant = $2
        "#,
    )
    .bind(&pool_id)
    .bind(&address)
    .fetch_optional(&state.db)
    .await?;

    let Some(r) = row else {
        return Ok(Json(json!({ "status": "not_entered" })));
    };

    let status = if r.paid {
        "paid"
    } else if !r.revealed && matches!(pool.state.as_str(), "Locked" | "Scored" | "Distributed") {
        "forfeited"
    } else if r.revealed && r.composite_weight.is_some() {
        "scored"
    } else if r.revealed {
        "revealed"
    } else {
        "committed"
    };

    Ok(Json(json!({
        "status": status,
        "entry_fee": r.entry_fee.to_string(),
        "commit_timestamp_ms": r.commit_timestamp_ms,
        "revealed": r.revealed,
        "predicted_prices": r.predicted_prices,
        "leg_hits": r.leg_hits,
        "acc_score": r.acc_score.map(|v| v.to_string()),
        "time_bonus": r.time_bonus.map(|v| v.to_string()),
        "composite_weight": r.composite_weight,
        "apex_payout": r.apex_payout.map(|v| v.to_string()),
    })))
}

/// Longest run of pools (ordered by reveal deadline) where the participant's
/// accuracy was >= 7500 bps — mirrors the on-chain streak rule.
async fn best_streak(state: &AppState, address: &str) -> Result<(i64, i64), AppError> {
    let rows: Vec<(Option<i64>,)> = sqlx::query_as(
        r#"
        SELECT p.acc_score
        FROM participants p
        JOIN pools pl ON pl.pool_id = p.pool_id
        WHERE p.participant = $1 AND pl.state = 'Distributed' AND p.revealed
        ORDER BY pl.reveal_deadline_ms ASC
        "#,
    )
    .bind(address)
    .fetch_all(&state.db)
    .await?;

    let mut best = 0i64;
    let mut current = 0i64;
    for (acc,) in rows {
        // acc_score is 1e9-scaled; 7500 bps = 0.75e9
        if acc.unwrap_or(0) >= 750_000_000 {
            current += 1;
            best = best.max(current);
        } else {
            current = 0;
        }
    }
    Ok((best, current))
}

pub async fn frs_profile(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Json<Value>, AppError> {
    let profile: Option<FrsProfileRow> = sqlx::query_as(
        "SELECT participant, tier, lifetime_accuracy_bps, pool_count FROM frs_profiles WHERE participant = $1",
    )
    .bind(&address)
    .fetch_optional(&state.db)
    .await?;

    let (total_earned, pools_entered): (i64, i64) = sqlx::query_as(
        r#"
        SELECT COALESCE(sum(apex_payout), 0)::BIGINT, count(*)
        FROM participants WHERE participant = $1
        "#,
    )
    .bind(&address)
    .fetch_one(&state.db)
    .await?;

    let (best, current) = best_streak(&state, &address).await?;

    let (tier, accuracy_bps, pool_count) = profile
        .map(|p| (p.tier, p.lifetime_accuracy_bps, p.pool_count))
        .unwrap_or((0, 0, 0));

    Ok(Json(json!({
        "participant": address,
        "tier": tier,
        "tier_label": tier_label(tier),
        "lifetime_accuracy_bps": accuracy_bps,
        "pool_count": pool_count,
        "pools_entered": pools_entered,
        "best_streak": best,
        "current_streak": current,
        "total_earned": total_earned.to_string(),
    })))
}

/// Chart data: lifetime accuracy after each settled pool, in settlement order.
pub async fn accuracy_history(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Json<Value>, AppError> {
    let rows: Vec<(String, i16, i64, i64)> = sqlx::query_as(
        r#"
        SELECT pool_id, new_tier, lifetime_accuracy_bps, pool_count
        FROM frs_settlements WHERE participant = $1 ORDER BY pool_count ASC
        "#,
    )
    .bind(&address)
    .fetch_all(&state.db)
    .await?;

    let history: Vec<Value> = rows
        .iter()
        .map(|(pool_id, tier, bps, count)| {
            json!({
                "pool_id": pool_id,
                "tier": tier,
                "lifetime_accuracy_bps": bps,
                "pool_count": count,
            })
        })
        .collect();
    Ok(Json(json!({ "history": history })))
}

/// Per-pool history rows for the profile page table.
pub async fn pool_history(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Json<Value>, AppError> {
    let rows: Vec<PoolHistoryRow> = sqlx::query_as(
        r#"
        SELECT p.pool_id, pl.state, p.entry_fee, p.commit_timestamp_ms, p.revealed,
               p.acc_score, p.apex_payout, p.predicted_prices, pl.oracle_results
        FROM participants p
        JOIN pools pl ON pl.pool_id = p.pool_id
        WHERE p.participant = $1
        ORDER BY pl.commit_deadline_ms DESC
        "#,
    )
    .bind(&address)
    .fetch_all(&state.db)
    .await?;

    let history: Vec<Value> = rows
        .iter()
        .map(
            |(pool_id, pool_state, fee, ts, revealed, acc, payout, prices, results)| {
                json!({
                    "pool_id": pool_id,
                    "pool_state": pool_state,
                    "entry_fee": fee.to_string(),
                    "commit_timestamp_ms": ts,
                    "revealed": revealed,
                    "acc_score": acc.map(|v| v.to_string()),
                    "apex_payout": payout.map(|v| v.to_string()),
                    "predicted_prices": prices,
                    "oracle_results": results,
                })
            },
        )
        .collect();
    Ok(Json(json!({ "pools": history })))
}

pub async fn rank(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Json<Value>, AppError> {
    let me: Option<FrsProfileRow> = sqlx::query_as(
        "SELECT participant, tier, lifetime_accuracy_bps, pool_count FROM frs_profiles WHERE participant = $1",
    )
    .bind(&address)
    .fetch_optional(&state.db)
    .await?;

    let (total,): (i64,) = sqlx::query_as("SELECT count(*) FROM frs_profiles WHERE pool_count > 0")
        .fetch_one(&state.db)
        .await?;

    let Some(me) = me else {
        return Ok(Json(json!({ "ranked": false, "total": total })));
    };

    let (ahead,): (i64,) = sqlx::query_as(
        r#"
        SELECT count(*) FROM frs_profiles
        WHERE pool_count > 0
          AND (lifetime_accuracy_bps, pool_count, participant)
            > ($1, $2, $3)
        "#,
    )
    .bind(me.lifetime_accuracy_bps)
    .bind(me.pool_count)
    .bind(&me.participant)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(json!({
        "ranked": me.pool_count > 0,
        "rank": ahead + 1,
        "total": total,
        "tier": me.tier,
        "tier_label": tier_label(me.tier),
        "lifetime_accuracy_bps": me.lifetime_accuracy_bps,
        "pool_count": me.pool_count,
    })))
}
