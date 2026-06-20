use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::AppState;
use crate::error::AppError;
use crate::keeper::tx::{CallArg, KeeperCtx, KeeperSigner};

#[derive(Debug, Deserialize)]
pub struct CreatePoolRequest {
    /// Number of price legs in this pool (default: 1).
    pub leg_count: Option<u64>,
    /// Entry fee in base units (default: 500_000_000 = 0.5 USDC with 9 decimals).
    pub entry_fee_amount: Option<u64>,
    /// Oracle object IDs to attach. Falls back to POOL_ORACLE_IDS env var.
    pub oracle_ids: Option<Vec<String>>,
    /// Milliseconds from now until commit deadline (default: 9_000_000 = 2.5 h).
    pub commit_window_ms: Option<u64>,
    /// Milliseconds from now until reveal deadline (default: 12_600_000 = 3.5 h).
    pub reveal_window_ms: Option<u64>,
}

fn check_bearer(headers: &HeaderMap, secret: &str) -> bool {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == format!("Bearer {secret}"))
        .unwrap_or(false)
}

pub async fn create_pool(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreatePoolRequest>,
) -> Result<Json<Value>, AppError> {
    let Some(admin_secret) = &state.cfg.admin_secret else {
        return Err(AppError::Unauthorized);
    };
    if !check_bearer(&headers, admin_secret) {
        return Err(AppError::Unauthorized);
    }

    let (Some(package_id), Some(pool_config_id), Some(keeper_key)) = (
        state.cfg.apex_package_id.as_ref(),
        state.cfg.pool_config_id.as_ref(),
        state.cfg.sui_keeper_key.as_ref(),
    ) else {
        return Err(AppError::BadRequest(
            "keeper not configured (requires APEX_PACKAGE_ID, POOL_CONFIG_ID, SUI_KEEPER_KEY)".into(),
        ));
    };

    let oracle_ids = body
        .oracle_ids
        .or_else(|| state.cfg.pool_oracle_ids.clone())
        .unwrap_or_default();

    let now_ms = chrono::Utc::now().timestamp_millis() as u64;
    let commit_deadline_ms = now_ms + body.commit_window_ms.unwrap_or(9_000_000);
    let reveal_deadline_ms = now_ms + body.reveal_window_ms.unwrap_or(12_600_000);
    let leg_count = body.leg_count.unwrap_or(1);
    let entry_fee_amount = body.entry_fee_amount.unwrap_or(500_000_000);

    let signer = KeeperSigner::from_bech32(keeper_key)
        .map_err(|e| AppError::Internal(format!("bad keeper key: {e}")))?;

    let config_obj = state
        .sui
        .get_object(pool_config_id)
        .await
        .map_err(|e| AppError::Internal(format!("fetch pool config: {e}")))?;
    let config_version = config_obj
        .initial_shared_version
        .ok_or_else(|| AppError::Internal("PoolConfig is not a shared object".into()))?;

    let ctx = KeeperCtx {
        sui: &state.sui,
        signer: &signer,
        package_id,
        keeper_cap_id: "",
    };

    let args = vec![
        CallArg::SharedMut {
            id: pool_config_id.clone(),
            initial_version: config_version,
        },
        CallArg::PureU64(leg_count),
        CallArg::PureU64(commit_deadline_ms),
        CallArg::PureU64(reveal_deadline_ms),
        CallArg::PureU64(entry_fee_amount),
        CallArg::PureObjectIdVec(oracle_ids),
        CallArg::ClockRef,
    ];

    let (digest, success, error) = crate::keeper::tx::execute_call(&ctx, "create_pool", args)
        .await
        .map_err(|e| AppError::Internal(format!("execute_call: {e}")))?;

    if success {
        tracing::info!(digest, commit_deadline_ms, reveal_deadline_ms, "pool created");
        Ok(Json(json!({
            "status": "ok",
            "digest": digest,
            "commit_deadline_ms": commit_deadline_ms,
            "reveal_deadline_ms": reveal_deadline_ms,
        })))
    } else {
        let msg = error.unwrap_or_default();
        tracing::warn!(digest, error = msg, "create_pool tx failed on-chain");
        Err(AppError::Internal(format!("create_pool failed on-chain: {msg}")))
    }
}
