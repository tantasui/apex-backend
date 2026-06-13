//! Event indexer: polls `suix_queryEvents` for the apex package's two event
//! modules, applying each page and its cursor in a single DB transaction.
//! A separate reconciler task syncs pool objects, because `close_pool` emits
//! no event and several Pool fields never appear in events at all.

pub mod events;
pub mod handlers;

use std::time::Duration;

use sqlx::PgPool;

use crate::sui::types::{
    field_enum_variant, field_id_vec, field_table_id, field_u64, field_u64_vec, EventId,
};
use crate::sui::SuiClient;

const POLL_INTERVAL: Duration = Duration::from_secs(2);
const RECONCILE_INTERVAL: Duration = Duration::from_secs(15);
const PAGE_SIZE: u64 = 100;
const EVENT_MODULES: [&str; 2] = ["apex_pool", "reputation"];

pub fn spawn(db: PgPool, sui: SuiClient, package_id: String) {
    {
        let (db, sui, package_id) = (db.clone(), sui.clone(), package_id.clone());
        tokio::spawn(async move {
            tracing::info!(package_id, "indexer started");
            loop {
                for module in EVENT_MODULES {
                    if let Err(e) = poll_module(&db, &sui, &package_id, module).await {
                        tracing::warn!(module, error = %e, "indexer poll failed");
                    }
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        });
    }
    tokio::spawn(async move {
        tracing::info!("pool reconciler started");
        loop {
            if let Err(e) = reconcile_pools(&db, &sui).await {
                tracing::warn!(error = %e, "pool reconciliation failed");
            }
            tokio::time::sleep(RECONCILE_INTERVAL).await;
        }
    });
}

async fn load_cursor(db: &PgPool, filter_key: &str) -> sqlx::Result<Option<EventId>> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT tx_digest, event_seq FROM indexer_cursors WHERE filter_key = $1",
    )
    .bind(filter_key)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(tx_digest, event_seq)| EventId {
        tx_digest,
        event_seq,
    }))
}

async fn poll_module(
    db: &PgPool,
    sui: &SuiClient,
    package_id: &str,
    module: &str,
) -> anyhow::Result<()> {
    let mut cursor = load_cursor(db, module).await?;
    loop {
        let page = sui
            .query_module_events(package_id, module, cursor.as_ref(), PAGE_SIZE)
            .await?;
        if page.data.is_empty() {
            return Ok(());
        }

        let mut tx = db.begin().await?;
        for raw in &page.data {
            match events::decode(raw) {
                Some(event) => handlers::apply(&mut tx, event).await?,
                None => tracing::debug!(event_type = %raw.event_type, "skipping unrecognized event"),
            }
        }
        if let Some(next) = &page.next_cursor {
            sqlx::query(
                r#"
                INSERT INTO indexer_cursors (filter_key, tx_digest, event_seq, updated_at)
                VALUES ($1, $2, $3, now())
                ON CONFLICT (filter_key) DO UPDATE SET
                    tx_digest = EXCLUDED.tx_digest,
                    event_seq = EXCLUDED.event_seq,
                    updated_at = now()
                "#,
            )
            .bind(module)
            .bind(&next.tx_digest)
            .bind(&next.event_seq)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        tracing::info!(module, count = page.data.len(), "indexed events");

        cursor = page.next_cursor;
        if !page.has_next_page {
            return Ok(());
        }
    }
}

/// Sync non-final pools from their on-chain objects: state (close_pool emits
/// no event), start_time, oracle ids/results/expiry, shared version, and the
/// predictions Table id. Then backfill revealed predictions per participant.
async fn reconcile_pools(db: &PgPool, sui: &SuiClient) -> anyhow::Result<()> {
    let pool_ids: Vec<(String,)> = sqlx::query_as(
        "SELECT pool_id FROM pools WHERE state <> 'Distributed' ORDER BY created_at DESC LIMIT 200",
    )
    .fetch_all(db)
    .await?;
    if pool_ids.is_empty() {
        return Ok(());
    }

    for chunk in pool_ids.chunks(50) {
        let ids: Vec<String> = chunk.iter().map(|(id,)| id.clone()).collect();
        let objects = sui.multi_get_objects(&ids).await?;
        for obj in objects {
            let f = &obj.fields;
            let state = field_enum_variant(f, "state");
            let oracle_ids = field_id_vec(f, "oracle_ids");
            let oracle_results = field_u64_vec(f, "oracle_results")
                .filter(|v| !v.is_empty())
                .map(|v| serde_json::json!(v.iter().map(|n| n.to_string()).collect::<Vec<_>>()));

            sqlx::query(
                r#"
                UPDATE pools SET
                    state = COALESCE($2, state),
                    start_time_ms = COALESCE($3, start_time_ms),
                    oracle_expiry_ms = COALESCE($4, oracle_expiry_ms),
                    oracle_ids = COALESCE($5, oracle_ids),
                    oracle_results = COALESCE($6, oracle_results),
                    initial_shared_version = COALESCE($7, initial_shared_version),
                    predictions_table_id = COALESCE($8, predictions_table_id),
                    total_weight = COALESCE($9, total_weight),
                    updated_at = now()
                WHERE pool_id = $1
                "#,
            )
            .bind(&obj.object_id)
            .bind(&state)
            .bind(field_u64(f, "start_time").map(|v| v as i64))
            .bind(field_u64(f, "oracle_expiry").map(|v| v as i64))
            .bind(oracle_ids.map(|v| serde_json::json!(v)))
            .bind(oracle_results)
            .bind(obj.initial_shared_version.map(|v| v as i64))
            .bind(field_table_id(f, "predictions"))
            .bind(f.get("total_weight").and_then(|v| v.as_str().map(|s| s.to_string())))
            .execute(db)
            .await?;
        }
    }

    backfill_predictions(db, sui).await
}

/// For pools past the commit phase, fetch revealed participants' Prediction
/// entries from the pool's dynamic-field table to expose predicted prices and
/// final scores in the results API. Bounded per tick to stay polite to the
/// fullnode.
async fn backfill_predictions(db: &PgPool, sui: &SuiClient) -> anyhow::Result<()> {
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        r#"
        SELECT p.pool_id, p.participant, pl.predictions_table_id, pl.state
        FROM participants p
        JOIN pools pl ON pl.pool_id = p.pool_id
        WHERE pl.predictions_table_id IS NOT NULL
          AND pl.state IN ('Closed', 'Locked', 'Scored', 'Distributed')
          AND p.revealed
          AND (
            p.predicted_prices IS NULL
            OR (pl.state IN ('Scored', 'Distributed') AND p.acc_score IS NULL)
          )
        LIMIT 25
        "#,
    )
    .fetch_all(db)
    .await?;

    for (pool_id, participant, table_id, pool_state) in rows {
        let Some(entry) = sui
            .get_table_entry_by_address(&table_id, &participant)
            .await?
        else {
            continue;
        };
        // Dynamic field object: fields.value.fields is the Prediction struct.
        let Some(pred) = entry
            .fields
            .get("value")
            .and_then(|v| v.get("fields"))
        else {
            continue;
        };
        let prices = field_u64_vec(pred, "predicted_prices")
            .map(|v| serde_json::json!(v.iter().map(|n| n.to_string()).collect::<Vec<_>>()));
        // Score fields default to 0 on chain until resolve runs, so only
        // trust them once the pool has actually been scored.
        let scored = matches!(pool_state.as_str(), "Scored" | "Distributed");
        sqlx::query(
            r#"
            UPDATE participants SET
                predicted_prices = COALESCE($3, predicted_prices),
                acc_score = COALESCE($4, acc_score),
                time_bonus = COALESCE($5, time_bonus),
                leg_hits = COALESCE($6, leg_hits),
                composite_weight = COALESCE($7, composite_weight),
                apex_payout = COALESCE($8, apex_payout)
            WHERE pool_id = $1 AND participant = $2
            "#,
        )
        .bind(&pool_id)
        .bind(&participant)
        .bind(prices)
        .bind(if scored { field_u64(pred, "acc_score").map(|v| v as i64) } else { None })
        .bind(if scored { field_u64(pred, "time_bonus").map(|v| v as i64) } else { None })
        .bind(if scored { field_u64(pred, "leg_hits").map(|v| v as i16) } else { None })
        .bind(if scored {
            pred.get("composite_weight").and_then(|v| v.as_str().map(|s| s.to_string()))
        } else {
            None
        })
        .bind(if scored { field_u64(pred, "apex_payout").map(|v| v as i64) } else { None })
        .execute(db)
        .await?;
    }
    Ok(())
}
