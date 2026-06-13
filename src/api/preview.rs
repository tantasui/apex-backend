//! Live previews backing the commit form: the projected DeepBook range for a
//! prediction value (mirrors `apex::bridge::build_range_params` against the
//! live oracle), and the time-bonus / weight projection (mirrors
//! `apex::scorer`).

use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::AppState;
use crate::error::AppError;
use crate::math::{self, I64Fp};
use crate::sui::types::{OracleSvi, PoolConfigData};

const ORACLE_TTL: Duration = Duration::from_secs(10);
const CONFIG_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Deserialize)]
pub struct ValueQuery {
    /// Predicted price in oracle units.
    pub value: u64,
    /// Leg index for multi-leg pools; defaults to 0.
    pub leg: Option<usize>,
}

async fn pool_config(state: &AppState) -> Result<PoolConfigData, AppError> {
    let config_id = state
        .cfg
        .pool_config_id
        .as_ref()
        .ok_or(AppError::ChainNotConfigured)?;

    let mut cache = state.pool_config_cache.lock().await;
    if let Some((at, cfg)) = cache.as_ref() {
        if at.elapsed() < CONFIG_TTL {
            return Ok(*cfg);
        }
    }
    let obj = state.sui.get_object(config_id).await?;
    let cfg = PoolConfigData::from_object(&obj)
        .ok_or_else(|| AppError::Upstream(anyhow::anyhow!("cannot parse PoolConfig {config_id}")))?;
    *cache = Some((Instant::now(), cfg));
    Ok(cfg)
}

async fn oracle(state: &AppState, oracle_id: &str) -> Result<OracleSvi, AppError> {
    let mut cache = state.oracle_cache.lock().await;
    if let Some((at, svi)) = cache.get(oracle_id) {
        if at.elapsed() < ORACLE_TTL {
            return Ok(svi.clone());
        }
    }
    let obj = state.sui.get_object(oracle_id).await?;
    let svi = OracleSvi::from_fields(&obj.fields)
        .ok_or_else(|| AppError::Upstream(anyhow::anyhow!("cannot parse OracleSVI {oracle_id}")))?;
    cache.insert(oracle_id.to_string(), (Instant::now(), svi.clone()));
    Ok(svi)
}

/// GET /pools/:id/range-preview?value=V[&leg=N]
pub async fn range_preview(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ValueQuery>,
) -> Result<Json<Value>, AppError> {
    let pool = super::pools::fetch_pool(&state, &id).await?;
    let cfg = pool_config(&state).await?;

    let oracle_ids: Vec<String> = pool
        .oracle_ids
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let leg = q.leg.unwrap_or(0);
    let oracle_id = oracle_ids.get(leg).ok_or_else(|| {
        AppError::BadRequest(format!(
            "pool has no oracle for leg {leg} (oracle ids not yet indexed?)"
        ))
    })?;

    let svi = oracle(&state, oracle_id).await?;
    let iv = math::svi_atm_iv(
        svi.svi_a,
        svi.svi_b,
        I64Fp::new(svi.svi_rho_mag, svi.svi_rho_neg),
        I64Fp::new(svi.svi_m_mag, svi.svi_m_neg),
        svi.svi_sigma,
    )?;

    let now_ms = chrono::Utc::now().timestamp_millis() as u64;
    let t_secs = svi.expiry_ms.saturating_sub(now_ms) / 1_000;
    let delta = math::compute_delta(iv, t_secs, q.value, cfg.k_scaling);
    let params = math::snap_to_grid(q.value, delta, cfg.min_strike, cfg.tick_size, cfg.max_strike)?;

    Ok(Json(json!({
        "value": q.value.to_string(),
        "leg": leg,
        "oracle_id": oracle_id,
        "underlying_asset": svi.underlying_asset,
        "spot": svi.spot.map(|v| v.to_string()),
        "atm_iv": iv.to_string(),
        "delta": delta.to_string(),
        "lower_snapped": params.lower_snapped.to_string(),
        "upper_snapped": params.upper_snapped.to_string(),
        "t_secs_to_expiry": t_secs,
    })))
}

/// GET /pools/:id/scoring-preview?value=V
///
/// Time bonus for committing right now, plus the projected composite weight
/// at hypothetical perfect accuracy. Works without any chain config; adds an
/// accuracy estimate vs the live oracle spot when one is available.
pub async fn scoring_preview(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ValueQuery>,
) -> Result<Json<Value>, AppError> {
    let pool = super::pools::fetch_pool(&state, &id).await?;
    let now_ms = chrono::Utc::now().timestamp_millis();

    let t_open = pool.start_time_ms.unwrap_or(now_ms);
    let tb = math::time_bonus(now_ms as u64, t_open as u64, pool.commit_deadline_ms as u64);
    let max_weight =
        math::composite_weight(pool.entry_fee_amount as u64, math::FLOAT_SCALING, tb);

    // Best-effort accuracy hint against the live spot (epsilon from PoolConfig
    // when available, otherwise just the raw score curve).
    let mut acc_vs_spot: Option<String> = None;
    let mut spot: Option<String> = None;
    if let Ok(cfg) = pool_config(&state).await {
        let oracle_ids: Vec<String> = pool
            .oracle_ids
            .as_ref()
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        if let Some(oracle_id) = oracle_ids.get(q.leg.unwrap_or(0)) {
            if let Ok(svi) = oracle(&state, oracle_id).await {
                if let Some(s) = svi.spot {
                    spot = Some(s.to_string());
                    acc_vs_spot =
                        Some(math::acc_score(q.value, s, cfg.epsilon_bps).to_string());
                }
            }
        }
    }

    Ok(Json(json!({
        "value": q.value.to_string(),
        "commit_now_time_bonus": tb.to_string(),
        "max_composite_weight": max_weight.to_string(),
        "entry_fee": pool.entry_fee_amount.to_string(),
        "commit_deadline_ms": pool.commit_deadline_ms,
        "spot": spot,
        "acc_score_vs_spot": acc_vs_spot,
        "note": "final score depends on the oracle settlement price",
    })))
}
