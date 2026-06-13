//! Per-event database writes. Every statement is an upsert so replaying a
//! page after a crash-before-cursor-commit is harmless.

use sqlx::{PgConnection, Postgres, Transaction};

use super::events::ApexEvent;

/// Pool aggregates are recomputed from the participants table instead of
/// incremented, so reprocessing an event cannot double-count.
async fn recompute_pool_aggregates(
    conn: &mut PgConnection,
    pool_id: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        UPDATE pools SET
            participant_count = (SELECT count(*) FROM participants WHERE pool_id = $1),
            prize_pool        = COALESCE((SELECT sum(entry_fee) FROM participants WHERE pool_id = $1), 0),
            reveal_count      = (SELECT count(*) FROM participants WHERE pool_id = $1 AND revealed),
            updated_at        = now()
        WHERE pool_id = $1
        "#,
    )
    .bind(pool_id)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn apply(tx: &mut Transaction<'_, Postgres>, event: ApexEvent) -> sqlx::Result<()> {
    match event {
        ApexEvent::PoolCreated {
            pool_id,
            leg_count,
            commit_deadline,
            reveal_deadline,
            entry_fee_amount,
        } => {
            sqlx::query(
                r#"
                INSERT INTO pools (pool_id, leg_count, commit_deadline_ms, reveal_deadline_ms, entry_fee_amount)
                VALUES ($1, $2, $3, $4, $5)
                ON CONFLICT (pool_id) DO UPDATE SET
                    leg_count = EXCLUDED.leg_count,
                    commit_deadline_ms = EXCLUDED.commit_deadline_ms,
                    reveal_deadline_ms = EXCLUDED.reveal_deadline_ms,
                    entry_fee_amount = EXCLUDED.entry_fee_amount,
                    updated_at = now()
                "#,
            )
            .bind(&pool_id)
            .bind(leg_count as i64)
            .bind(commit_deadline as i64)
            .bind(reveal_deadline as i64)
            .bind(entry_fee_amount as i64)
            .execute(&mut **tx)
            .await?;
        }

        ApexEvent::CommitSubmitted {
            pool_id,
            participant,
            entry_fee,
            timestamp,
        } => {
            sqlx::query(
                r#"
                INSERT INTO participants (pool_id, participant, entry_fee, commit_timestamp_ms)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (pool_id, participant) DO UPDATE SET
                    entry_fee = EXCLUDED.entry_fee,
                    commit_timestamp_ms = EXCLUDED.commit_timestamp_ms
                "#,
            )
            .bind(&pool_id)
            .bind(&participant)
            .bind(entry_fee as i64)
            .bind(timestamp as i64)
            .execute(&mut **tx)
            .await?;
            recompute_pool_aggregates(tx, &pool_id).await?;
        }

        ApexEvent::RevealSubmitted {
            pool_id,
            participant,
            crowd_median_estimate,
        } => {
            sqlx::query(
                r#"
                UPDATE participants
                SET revealed = TRUE, crowd_median_estimate = $3
                WHERE pool_id = $1 AND participant = $2
                "#,
            )
            .bind(&pool_id)
            .bind(&participant)
            .bind(crowd_median_estimate as i64)
            .execute(&mut **tx)
            .await?;
            recompute_pool_aggregates(tx, &pool_id).await?;
        }

        ApexEvent::PoolLocked {
            pool_id,
            reveal_count,
            forfeited_count,
        } => {
            sqlx::query(
                r#"
                UPDATE pools
                SET state = 'Locked', reveal_count = $2, forfeited_count = $3, updated_at = now()
                WHERE pool_id = $1 AND state NOT IN ('Scored', 'Distributed')
                "#,
            )
            .bind(&pool_id)
            .bind(reveal_count as i64)
            .bind(forfeited_count as i64)
            .execute(&mut **tx)
            .await?;
        }

        ApexEvent::PoolResolved {
            pool_id,
            total_weight,
        } => {
            sqlx::query(
                r#"
                UPDATE pools
                SET state = 'Scored', total_weight = $2, updated_at = now()
                WHERE pool_id = $1 AND state <> 'Distributed'
                "#,
            )
            .bind(&pool_id)
            .bind(&total_weight)
            .execute(&mut **tx)
            .await?;
        }

        ApexEvent::ParticipantPaid {
            pool_id,
            participant,
            apex_payout,
            leg_hits,
            composite_weight,
        } => {
            sqlx::query(
                r#"
                UPDATE participants
                SET apex_payout = $3, leg_hits = $4, composite_weight = $5, paid = TRUE
                WHERE pool_id = $1 AND participant = $2
                "#,
            )
            .bind(&pool_id)
            .bind(&participant)
            .bind(apex_payout as i64)
            .bind(leg_hits as i16)
            .bind(&composite_weight)
            .execute(&mut **tx)
            .await?;
        }

        ApexEvent::PrizeDistributed {
            pool_id,
            total_prize,
        } => {
            sqlx::query(
                r#"
                UPDATE pools
                SET state = 'Distributed', total_prize = $2, updated_at = now()
                WHERE pool_id = $1
                "#,
            )
            .bind(&pool_id)
            .bind(total_prize as i64)
            .execute(&mut **tx)
            .await?;
        }

        ApexEvent::FrsCreated {
            participant,
            pool_id,
        } => {
            sqlx::query(
                r#"
                INSERT INTO frs_profiles (participant, frs_pool_id)
                VALUES ($1, $2)
                ON CONFLICT (participant) DO NOTHING
                "#,
            )
            .bind(&participant)
            .bind(&pool_id)
            .execute(&mut **tx)
            .await?;
        }

        ApexEvent::FrsSettled {
            participant,
            pool_id,
            new_tier,
            lifetime_accuracy_bps,
            pool_count,
            tx_digest,
        } => {
            sqlx::query(
                r#"
                INSERT INTO frs_profiles (participant, tier, lifetime_accuracy_bps, pool_count, updated_at)
                VALUES ($1, $2, $3, $4, now())
                ON CONFLICT (participant) DO UPDATE SET
                    tier = EXCLUDED.tier,
                    lifetime_accuracy_bps = EXCLUDED.lifetime_accuracy_bps,
                    pool_count = GREATEST(frs_profiles.pool_count, EXCLUDED.pool_count),
                    updated_at = now()
                "#,
            )
            .bind(&participant)
            .bind(new_tier as i16)
            .bind(lifetime_accuracy_bps as i64)
            .bind(pool_count as i64)
            .execute(&mut **tx)
            .await?;

            sqlx::query(
                r#"
                INSERT INTO frs_settlements
                    (participant, pool_id, new_tier, lifetime_accuracy_bps, pool_count, tx_digest)
                VALUES ($1, $2, $3, $4, $5, $6)
                ON CONFLICT (participant, pool_id) DO NOTHING
                "#,
            )
            .bind(&participant)
            .bind(&pool_id)
            .bind(new_tier as i16)
            .bind(lifetime_accuracy_bps as i64)
            .bind(pool_count as i64)
            .bind(&tx_digest)
            .execute(&mut **tx)
            .await?;
        }

        ApexEvent::TierUpgraded {
            participant,
            new_tier,
        } => {
            sqlx::query(
                r#"
                UPDATE frs_profiles SET tier = $2, updated_at = now() WHERE participant = $1
                "#,
            )
            .bind(&participant)
            .bind(new_tier as i16)
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(())
}
