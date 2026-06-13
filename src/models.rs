//! Database row types and JSON response DTOs.
//!
//! Convention: anything that is a Move u64 amount/score or u128 weight is
//! serialized to JSON as a string to avoid JS Number precision loss.
//! Millisecond timestamps stay numeric (they fit in 2^53).

use serde::Serialize;
use sqlx::FromRow;

// === Row types ===

#[allow(dead_code)]
#[derive(Debug, Clone, FromRow)]
pub struct PoolRow {
    pub pool_id: String,
    pub leg_count: i64,
    pub state: String,
    pub start_time_ms: Option<i64>,
    pub commit_deadline_ms: i64,
    pub reveal_deadline_ms: i64,
    pub oracle_expiry_ms: Option<i64>,
    pub entry_fee_amount: i64,
    pub oracle_ids: Option<serde_json::Value>,
    pub oracle_results: Option<serde_json::Value>,
    pub participant_count: i64,
    pub reveal_count: i64,
    pub forfeited_count: i64,
    pub prize_pool: i64,
    pub total_weight: String,
    pub total_prize: Option<i64>,
    pub initial_shared_version: Option<i64>,
    pub predictions_table_id: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, FromRow)]
pub struct ParticipantRow {
    pub pool_id: String,
    pub participant: String,
    pub entry_fee: i64,
    pub commit_timestamp_ms: i64,
    pub revealed: bool,
    pub crowd_median_estimate: Option<i64>,
    pub predicted_prices: Option<serde_json::Value>,
    pub leg_hits: Option<i16>,
    pub acc_score: Option<i64>,
    pub time_bonus: Option<i64>,
    pub composite_weight: Option<String>,
    pub apex_payout: Option<i64>,
    pub paid: bool,
}

#[derive(Debug, Clone, FromRow)]
pub struct FrsProfileRow {
    pub participant: String,
    pub tier: i16,
    pub lifetime_accuracy_bps: i64,
    pub pool_count: i64,
}

// === Response DTOs ===

pub fn tier_label(tier: i16) -> &'static str {
    match tier {
        1 => "ANALYST",
        2 => "EXPERT",
        3 => "APEX",
        _ => "OBSERVER",
    }
}

#[derive(Debug, Serialize)]
pub struct PoolSummary {
    pub pool_id: String,
    pub state: String,
    pub leg_count: i64,
    pub start_time_ms: Option<i64>,
    pub commit_deadline_ms: i64,
    pub reveal_deadline_ms: i64,
    pub oracle_expiry_ms: Option<i64>,
    pub entry_fee_amount: String,
    pub prize_pool: String,
    pub participant_count: i64,
    pub reveal_count: i64,
}

impl From<&PoolRow> for PoolSummary {
    fn from(r: &PoolRow) -> Self {
        Self {
            pool_id: r.pool_id.clone(),
            state: r.state.clone(),
            leg_count: r.leg_count,
            start_time_ms: r.start_time_ms,
            commit_deadline_ms: r.commit_deadline_ms,
            reveal_deadline_ms: r.reveal_deadline_ms,
            oracle_expiry_ms: r.oracle_expiry_ms,
            entry_fee_amount: r.entry_fee_amount.to_string(),
            prize_pool: r.prize_pool.to_string(),
            participant_count: r.participant_count,
            reveal_count: r.reveal_count,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct PoolDetail {
    #[serde(flatten)]
    pub summary: PoolSummary,
    pub forfeited_count: i64,
    pub total_weight: String,
    pub total_prize: Option<String>,
    pub oracle_ids: Option<serde_json::Value>,
    pub oracle_results: Option<serde_json::Value>,
}

impl From<&PoolRow> for PoolDetail {
    fn from(r: &PoolRow) -> Self {
        Self {
            summary: PoolSummary::from(r),
            forfeited_count: r.forfeited_count,
            total_weight: r.total_weight.clone(),
            total_prize: r.total_prize.map(|v| v.to_string()),
            oracle_ids: r.oracle_ids.clone(),
            oracle_results: r.oracle_results.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ParticipantEntry {
    pub participant: String,
    pub entry_fee: String,
    pub commit_timestamp_ms: i64,
    pub revealed: bool,
}

#[derive(Debug, Serialize)]
pub struct ResultEntry {
    pub rank: i64,
    pub participant: String,
    pub predicted_prices: Option<serde_json::Value>,
    pub leg_hits: Option<i16>,
    pub acc_score: Option<String>,
    pub time_bonus: Option<String>,
    pub composite_weight: Option<String>,
    pub apex_payout: Option<String>,
}
