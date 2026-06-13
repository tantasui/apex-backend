use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::AppState;
use crate::error::AppError;
use crate::models::tier_label;

#[derive(Debug, Deserialize)]
pub struct LeaderboardQuery {
    /// all | 30d | 7d — period filters the earned/pool aggregates; the
    /// accuracy ranking itself is lifetime (it lives on chain as a lifetime
    /// average).
    pub period: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

pub async fn leaderboard(
    State(state): State<AppState>,
    Query(q): Query<LeaderboardQuery>,
) -> Result<Json<Value>, AppError> {
    let limit = q.limit.unwrap_or(25).clamp(1, 100);
    let offset = q.offset.unwrap_or(0).max(0);

    let window_ms: i64 = match q.period.as_deref().unwrap_or("all") {
        "all" => i64::MAX,
        "30d" => 30 * 24 * 3600 * 1000,
        "7d" => 7 * 24 * 3600 * 1000,
        other => {
            return Err(AppError::BadRequest(format!(
                "unknown period: {other} (expected all|30d|7d)"
            )))
        }
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    let since_ms = if window_ms == i64::MAX { 0 } else { now_ms - window_ms };

    let rows: Vec<(String, i16, i64, i64, i64, i64)> = sqlx::query_as(
        r#"
        SELECT f.participant, f.tier, f.lifetime_accuracy_bps, f.pool_count,
               COALESCE(agg.earned, 0)::BIGINT AS earned,
               COALESCE(agg.pools_in_period, 0)::BIGINT AS pools_in_period
        FROM frs_profiles f
        LEFT JOIN (
            SELECT p.participant,
                   sum(p.apex_payout) AS earned,
                   count(*) AS pools_in_period
            FROM participants p
            JOIN pools pl ON pl.pool_id = p.pool_id
            WHERE pl.state = 'Distributed' AND pl.reveal_deadline_ms >= $1
            GROUP BY p.participant
        ) agg ON agg.participant = f.participant
        WHERE f.pool_count > 0
        ORDER BY f.lifetime_accuracy_bps DESC, f.pool_count DESC, f.participant ASC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(since_ms)
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let (total,): (i64,) = sqlx::query_as("SELECT count(*) FROM frs_profiles WHERE pool_count > 0")
        .fetch_one(&state.db)
        .await?;

    let entries: Vec<Value> = rows
        .iter()
        .enumerate()
        .map(|(i, (addr, tier, bps, pools, earned, period_pools))| {
            json!({
                "rank": offset + i as i64 + 1,
                "participant": addr,
                "tier": tier,
                "tier_label": tier_label(*tier),
                "lifetime_accuracy_bps": bps,
                "pool_count": pools,
                "earned": earned.to_string(),
                "pools_in_period": period_pools,
            })
        })
        .collect();

    // Tier breakdown banner counts.
    let tier_rows: Vec<(i16, i64)> = sqlx::query_as(
        "SELECT tier, count(*) FROM frs_profiles WHERE pool_count > 0 GROUP BY tier",
    )
    .fetch_all(&state.db)
    .await?;
    let mut tiers = [0i64; 4];
    for (tier, count) in tier_rows {
        if (0..4).contains(&(tier as usize as i32)) {
            tiers[tier as usize] = count;
        }
    }

    Ok(Json(json!({
        "leaderboard": entries,
        "total": total,
        "tiers": {
            "observer": tiers[0],
            "analyst": tiers[1],
            "expert": tiers[2],
            "apex": tiers[3],
        },
    })))
}
