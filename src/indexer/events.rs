//! Typed decoding of `apex::apex_pool` and `apex::reputation` events from
//! the `parsedJson` payload of `suix_queryEvents`.

use serde_json::Value;

use crate::sui::types::{parse_u64, SuiEvent};

fn pj_str(v: &Value, name: &str) -> Option<String> {
    v.get(name).and_then(|x| x.as_str()).map(|s| s.to_string())
}

fn pj_u64(v: &Value, name: &str) -> Option<u64> {
    v.get(name).and_then(parse_u64)
}

/// u128 fields arrive as decimal strings; keep them as strings end to end.
fn pj_u128_string(v: &Value, name: &str) -> Option<String> {
    match v.get(name)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub enum ApexEvent {
    PoolCreated {
        pool_id: String,
        leg_count: u64,
        commit_deadline: u64,
        reveal_deadline: u64,
        entry_fee_amount: u64,
    },
    CommitSubmitted {
        pool_id: String,
        participant: String,
        entry_fee: u64,
        timestamp: u64,
    },
    RevealSubmitted {
        pool_id: String,
        participant: String,
        crowd_median_estimate: u64,
    },
    PoolLocked {
        pool_id: String,
        reveal_count: u64,
        forfeited_count: u64,
    },
    PoolResolved {
        pool_id: String,
        total_weight: String,
    },
    ParticipantPaid {
        pool_id: String,
        participant: String,
        apex_payout: u64,
        leg_hits: u64,
        composite_weight: String,
    },
    PrizeDistributed {
        pool_id: String,
        total_prize: u64,
    },
    FrsCreated {
        participant: String,
        pool_id: Option<String>,
    },
    FrsSettled {
        participant: String,
        pool_id: String,
        new_tier: u64,
        lifetime_accuracy_bps: u64,
        pool_count: u64,
        tx_digest: String,
    },
    TierUpgraded {
        participant: String,
        new_tier: u64,
    },
}

/// Returns None for unrecognized or malformed events (logged by the caller).
pub fn decode(event: &SuiEvent) -> Option<ApexEvent> {
    let name = event.event_type.rsplit("::").next()?;
    let p = &event.parsed_json;
    let ev = match name {
        "PoolCreated" => ApexEvent::PoolCreated {
            pool_id: pj_str(p, "pool_id")?,
            leg_count: pj_u64(p, "leg_count")?,
            commit_deadline: pj_u64(p, "commit_deadline")?,
            reveal_deadline: pj_u64(p, "reveal_deadline")?,
            entry_fee_amount: pj_u64(p, "entry_fee_amount")?,
        },
        "CommitSubmitted" => ApexEvent::CommitSubmitted {
            pool_id: pj_str(p, "pool_id")?,
            participant: pj_str(p, "participant")?,
            entry_fee: pj_u64(p, "entry_fee")?,
            timestamp: pj_u64(p, "timestamp")?,
        },
        "RevealSubmitted" => ApexEvent::RevealSubmitted {
            pool_id: pj_str(p, "pool_id")?,
            participant: pj_str(p, "participant")?,
            crowd_median_estimate: pj_u64(p, "crowd_median_estimate")?,
        },
        "PoolLocked" => ApexEvent::PoolLocked {
            pool_id: pj_str(p, "pool_id")?,
            reveal_count: pj_u64(p, "reveal_count")?,
            forfeited_count: pj_u64(p, "forfeited_count")?,
        },
        "PoolResolved" => ApexEvent::PoolResolved {
            pool_id: pj_str(p, "pool_id")?,
            total_weight: pj_u128_string(p, "total_weight")?,
        },
        "ParticipantPaid" => ApexEvent::ParticipantPaid {
            pool_id: pj_str(p, "pool_id")?,
            participant: pj_str(p, "participant")?,
            apex_payout: pj_u64(p, "apex_payout")?,
            leg_hits: pj_u64(p, "leg_hits")?,
            composite_weight: pj_u128_string(p, "composite_weight")?,
        },
        "PrizeDistributed" => ApexEvent::PrizeDistributed {
            pool_id: pj_str(p, "pool_id")?,
            total_prize: pj_u64(p, "total_prize")?,
        },
        "FRSCreated" => ApexEvent::FrsCreated {
            participant: pj_str(p, "participant")?,
            pool_id: pj_str(p, "pool_id"),
        },
        "FRSSettled" => ApexEvent::FrsSettled {
            participant: pj_str(p, "participant")?,
            pool_id: pj_str(p, "pool_id")?,
            new_tier: pj_u64(p, "new_tier")?,
            lifetime_accuracy_bps: pj_u64(p, "lifetime_accuracy_bps")?,
            pool_count: pj_u64(p, "pool_count")?,
            tx_digest: event.id.tx_digest.clone(),
        },
        "TierUpgraded" => ApexEvent::TierUpgraded {
            participant: pj_str(p, "participant")?,
            new_tier: pj_u64(p, "new_tier")?,
        },
        _ => return None,
    };
    Some(ev)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sui::types::EventId;
    use serde_json::json;

    fn ev(type_suffix: &str, parsed: serde_json::Value) -> SuiEvent {
        SuiEvent {
            id: EventId {
                tx_digest: "Dig1".into(),
                event_seq: "0".into(),
            },
            event_type: format!("0xabc::apex_pool::{type_suffix}"),
            parsed_json: parsed,
            timestamp_ms: Some("1760000000000".into()),
        }
    }

    #[test]
    fn decodes_pool_created() {
        let e = ev(
            "PoolCreated",
            json!({
                "pool_id": "0xp",
                "leg_count": "2",
                "commit_deadline": "100",
                "reveal_deadline": "200",
                "entry_fee_amount": "500000000"
            }),
        );
        match decode(&e).unwrap() {
            ApexEvent::PoolCreated {
                pool_id,
                leg_count,
                entry_fee_amount,
                ..
            } => {
                assert_eq!(pool_id, "0xp");
                assert_eq!(leg_count, 2);
                assert_eq!(entry_fee_amount, 500_000_000);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn decodes_participant_paid_with_u128_weight() {
        let e = ev(
            "ParticipantPaid",
            json!({
                "pool_id": "0xp",
                "participant": "0xa",
                "apex_payout": "487500000",
                "leg_hits": 2,
                "composite_weight": "340282366920938463463374607431768211455"
            }),
        );
        match decode(&e).unwrap() {
            ApexEvent::ParticipantPaid {
                composite_weight, leg_hits, ..
            } => {
                assert_eq!(composite_weight, "340282366920938463463374607431768211455");
                assert_eq!(leg_hits, 2);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn unknown_event_is_none() {
        let e = ev("SomethingElse", json!({}));
        assert!(decode(&e).is_none());
    }
}
