//! Keeper: drives pool state transitions on a 10s loop.
//!
//!   Open   --past commit deadline-->  close_pool   (permissionless)
//!   Closed --past reveal deadline-->  lock_pool    (permissionless)
//!   Locked --all oracles settled-->   resolve      (KeeperCap)
//!   Scored ------------------------>  distribute   (KeeperCap)
//!
//! The keeper never writes pool state to the DB — the indexer/reconciler is
//! the single source of truth. Before sending it re-checks the on-chain
//! state so a lagging DB can't trigger EWrongState aborts.

pub mod tx;

use std::time::Duration;

use sqlx::PgPool;

use crate::config::Config;
use crate::sui::types::{field_enum_variant, OracleSvi};
use crate::sui::SuiClient;
use tx::{CallArg, KeeperCtx, KeeperSigner};

const LOOP_INTERVAL: Duration = Duration::from_secs(10);
/// Don't re-attempt an action on the same pool within this window.
const BACKOFF_SECONDS: i64 = 60;

type PoolRow = (String, String, i64, i64, Option<i64>, Option<serde_json::Value>);

pub fn spawn(db: PgPool, sui: SuiClient, cfg: &Config) -> anyhow::Result<()> {
    let signer = KeeperSigner::from_bech32(
        cfg.sui_keeper_key.as_ref().expect("keeper_enabled checked"),
    )?;
    tracing::info!(address = %signer.address, "keeper started");

    let package_id = cfg.apex_package_id.clone().expect("checked");
    let pool_config_id = cfg.pool_config_id.clone().expect("checked");
    let keeper_cap_id = cfg.keeper_cap_id.clone().expect("checked");

    tokio::spawn(async move {
        loop {
            if let Err(e) = tick(
                &db,
                &sui,
                &signer,
                &package_id,
                &pool_config_id,
                &keeper_cap_id,
            )
            .await
            {
                tracing::warn!(error = %e, "keeper tick failed");
            }
            tokio::time::sleep(LOOP_INTERVAL).await;
        }
    });
    Ok(())
}

async fn recently_attempted(db: &PgPool, pool_id: &str, action: &str) -> sqlx::Result<bool> {
    let (count,): (i64,) = sqlx::query_as(
        r#"
        SELECT count(*) FROM keeper_actions
        WHERE pool_id = $1 AND action = $2
          AND created_at > now() - make_interval(secs => $3)
        "#,
    )
    .bind(pool_id)
    .bind(action)
    .bind(BACKOFF_SECONDS as f64)
    .fetch_one(db)
    .await?;
    Ok(count > 0)
}

async fn record_action(
    db: &PgPool,
    pool_id: &str,
    action: &str,
    result: &anyhow::Result<(String, bool, Option<String>)>,
) {
    let (digest, status, error) = match result {
        Ok((digest, true, _)) => (Some(digest.clone()), "success", None),
        Ok((digest, false, err)) => (Some(digest.clone()), "failed", err.clone()),
        Err(e) => (None, "failed", Some(e.to_string())),
    };
    if let Err(e) = sqlx::query(
        "INSERT INTO keeper_actions (pool_id, action, tx_digest, status, error) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(pool_id)
    .bind(action)
    .bind(&digest)
    .bind(status)
    .bind(&error)
    .execute(db)
    .await
    {
        tracing::error!(error = %e, "failed to record keeper action");
    }

    match status {
        "success" => tracing::info!(pool_id, action, digest = digest.as_deref().unwrap_or(""), "keeper action succeeded"),
        _ => tracing::warn!(pool_id, action, error = error.as_deref().unwrap_or(""), "keeper action failed"),
    }
}

#[allow(clippy::too_many_arguments)]
async fn tick(
    db: &PgPool,
    sui: &SuiClient,
    signer: &KeeperSigner,
    package_id: &str,
    pool_config_id: &str,
    keeper_cap_id: &str,
) -> anyhow::Result<()> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let pools: Vec<PoolRow> = sqlx::query_as(
            r#"
            SELECT pool_id, state, commit_deadline_ms, reveal_deadline_ms,
                   initial_shared_version, oracle_ids
            FROM pools WHERE state <> 'Distributed'
            "#,
        )
        .fetch_all(db)
        .await?;

    let ctx = KeeperCtx {
        sui,
        signer,
        package_id,
        keeper_cap_id,
    };

    for (pool_id, db_state, commit_deadline, reveal_deadline, shared_version, oracle_ids) in pools
    {
        let Some(initial_version) = shared_version else {
            // Reconciler hasn't fetched the object yet.
            continue;
        };
        let initial_version = initial_version as u64;

        let action: Option<(&str, Vec<CallArg>)> = match db_state.as_str() {
            "Open" if now_ms > commit_deadline => Some((
                "close_pool",
                vec![
                    CallArg::SharedMut {
                        id: pool_id.clone(),
                        initial_version,
                    },
                    CallArg::ClockRef,
                ],
            )),
            "Closed" if now_ms > reveal_deadline => Some((
                "lock_pool",
                vec![
                    CallArg::SharedMut {
                        id: pool_id.clone(),
                        initial_version,
                    },
                    CallArg::ClockRef,
                ],
            )),
            "Locked" => {
                match settlement_prices(sui, oracle_ids.as_ref()).await? {
                    Some(results) => {
                        let config_version = config_initial_version(sui, pool_config_id).await?;
                        Some((
                            "resolve",
                            vec![
                                CallArg::SharedMut {
                                    id: pool_id.clone(),
                                    initial_version,
                                },
                                CallArg::PureU64Vec(results),
                                CallArg::SharedRef {
                                    id: pool_config_id.to_string(),
                                    initial_version: config_version,
                                },
                                CallArg::OwnedKeeperCap,
                                CallArg::ClockRef,
                            ],
                        ))
                    }
                    None => None, // some oracle not settled yet
                }
            }
            "Scored" => {
                let config_version = config_initial_version(sui, pool_config_id).await?;
                Some((
                    "distribute",
                    vec![
                        CallArg::SharedMut {
                            id: pool_id.clone(),
                            initial_version,
                        },
                        CallArg::SharedRef {
                            id: pool_config_id.to_string(),
                            initial_version: config_version,
                        },
                        CallArg::OwnedKeeperCap,
                    ],
                ))
            }
            _ => None,
        };

        let Some((function, args)) = action else {
            continue;
        };

        if recently_attempted(db, &pool_id, function).await? {
            continue;
        }

        // The DB may lag the chain; confirm the on-chain state still warrants
        // this action before spending gas.
        let on_chain = sui.get_object(&pool_id).await?;
        let chain_state = field_enum_variant(&on_chain.fields, "state").unwrap_or_default();
        let expected = match function {
            "close_pool" => "Open",
            "lock_pool" => "Closed",
            "resolve" => "Locked",
            "distribute" => "Scored",
            _ => unreachable!(),
        };
        if chain_state != expected {
            tracing::debug!(pool_id, chain_state, expected, "skipping; chain state moved on");
            continue;
        }

        let result = tx::execute_call(&ctx, function, args).await;
        record_action(db, &pool_id, function, &result).await;
    }
    Ok(())
}

/// One settlement price per leg, or None if any oracle hasn't settled.
async fn settlement_prices(
    sui: &SuiClient,
    oracle_ids: Option<&serde_json::Value>,
) -> anyhow::Result<Option<Vec<u64>>> {
    let ids: Vec<String> = match oracle_ids {
        Some(v) => serde_json::from_value(v.clone()).unwrap_or_default(),
        None => vec![],
    };
    if ids.is_empty() {
        return Ok(None);
    }
    let objects = sui.multi_get_objects(&ids).await?;
    if objects.len() != ids.len() {
        return Ok(None);
    }
    let mut results = Vec::with_capacity(ids.len());
    // multi_get preserves request order on the fullnode; map by id anyway.
    for id in &ids {
        let Some(obj) = objects.iter().find(|o| {
            o.object_id == *id
                || o.object_id.trim_start_matches("0x") == id.trim_start_matches("0x")
        }) else {
            return Ok(None);
        };
        let Some(svi) = OracleSvi::from_fields(&obj.fields) else {
            return Ok(None);
        };
        match svi.settlement_price {
            Some(price) => results.push(price),
            None => return Ok(None),
        }
    }
    Ok(Some(results))
}

async fn config_initial_version(sui: &SuiClient, config_id: &str) -> anyhow::Result<u64> {
    let obj = sui.get_object(config_id).await?;
    obj.initial_shared_version
        .ok_or_else(|| anyhow::anyhow!("PoolConfig {config_id} is not a shared object"))
}
