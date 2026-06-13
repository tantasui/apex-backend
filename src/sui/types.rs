//! Serde models for the Sui fullnode JSON-RPC surface we touch, plus lenient
//! extractors for Move object field JSON (fullnode renders u64/u128 as strings,
//! Move enums as either a plain string or `{"variant": ...}` depending on
//! node version — we accept both).

use serde::Deserialize;
use serde_json::Value;

// === Events ===

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct EventId {
    #[serde(rename = "txDigest")]
    pub tx_digest: String,
    #[serde(rename = "eventSeq")]
    pub event_seq: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct SuiEvent {
    pub id: EventId,
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(rename = "parsedJson", default)]
    pub parsed_json: Value,
    #[serde(rename = "timestampMs", default)]
    pub timestamp_ms: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventPage {
    pub data: Vec<SuiEvent>,
    #[serde(rename = "nextCursor")]
    pub next_cursor: Option<EventId>,
    #[serde(rename = "hasNextPage", default)]
    pub has_next_page: bool,
}

// === Objects ===

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ObjectData {
    pub object_id: String,
    pub version: u64,
    pub digest: String,
    /// `Shared { initial_shared_version }` if the object is shared.
    pub initial_shared_version: Option<u64>,
    /// `content.fields` of a Move object.
    pub fields: Value,
    /// Full Move type string, e.g. `0xabc::apex_pool::Pool`.
    pub type_: Option<String>,
}

impl ObjectData {
    /// Parse a `data` payload from `sui_getObject` / `sui_multiGetObjects`.
    pub fn from_rpc(data: &Value) -> Option<Self> {
        let object_id = data.get("objectId")?.as_str()?.to_string();
        let version = parse_u64(data.get("version")?)?;
        let digest = data.get("digest")?.as_str()?.to_string();
        let initial_shared_version = data
            .get("owner")
            .and_then(|o| o.get("Shared"))
            .and_then(|s| s.get("initial_shared_version"))
            .and_then(parse_u64);
        let content = data.get("content");
        let fields = content
            .and_then(|c| c.get("fields"))
            .cloned()
            .unwrap_or(Value::Null);
        let type_ = content
            .and_then(|c| c.get("type"))
            .and_then(|t| t.as_str())
            .map(|s| s.to_string());
        Some(Self {
            object_id,
            version,
            digest,
            initial_shared_version,
            fields,
            type_,
        })
    }
}

// === Lenient field extractors ===

/// u64 from either a JSON string or a JSON number.
pub fn parse_u64(v: &Value) -> Option<u64> {
    match v {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_u64(),
        _ => None,
    }
}

pub fn field_u64(fields: &Value, name: &str) -> Option<u64> {
    fields.get(name).and_then(parse_u64)
}

pub fn field_str<'a>(fields: &'a Value, name: &str) -> Option<&'a str> {
    fields.get(name).and_then(|v| v.as_str())
}

/// `Option<u64>` rendered by the fullnode as null or the value (older nodes
/// wrap it as {"fields": {"vec": [...]}} — accept both).
pub fn field_opt_u64(fields: &Value, name: &str) -> Option<u64> {
    let v = fields.get(name)?;
    if let Some(n) = parse_u64(v) {
        return Some(n);
    }
    v.get("fields")
        .and_then(|f| f.get("vec"))
        .and_then(|vec| vec.as_array())
        .and_then(|arr| arr.first())
        .and_then(parse_u64)
}

/// Move enum field: `"Open"` or `{"variant": "Open", ...}`.
pub fn field_enum_variant(fields: &Value, name: &str) -> Option<String> {
    let v = fields.get(name)?;
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    v.get("variant")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

/// `vector<u64>` of JSON strings/numbers.
pub fn field_u64_vec(fields: &Value, name: &str) -> Option<Vec<u64>> {
    fields
        .get(name)?
        .as_array()?
        .iter()
        .map(parse_u64)
        .collect()
}

/// `vector<ID>` — IDs render as plain hex strings.
pub fn field_id_vec(fields: &Value, name: &str) -> Option<Vec<String>> {
    fields
        .get(name)?
        .as_array()?
        .iter()
        .map(|v| v.as_str().map(|s| s.to_string()))
        .collect()
}

/// `Table` field — we only need its object id: fields.<name>.fields.id.id
pub fn field_table_id(fields: &Value, name: &str) -> Option<String> {
    fields
        .get(name)?
        .get("fields")?
        .get("id")?
        .get("id")?
        .as_str()
        .map(|s| s.to_string())
}

// === OracleSVI ===

#[derive(Debug, Clone)]
pub struct OracleSvi {
    pub underlying_asset: Option<String>,
    pub expiry_ms: u64,
    pub spot: Option<u64>,
    pub svi_a: u64,
    pub svi_b: u64,
    pub svi_rho_mag: u64,
    pub svi_rho_neg: bool,
    pub svi_m_mag: u64,
    pub svi_m_neg: bool,
    pub svi_sigma: u64,
    pub settlement_price: Option<u64>,
}

impl OracleSvi {
    pub fn from_fields(fields: &Value) -> Option<Self> {
        let expiry_ms = field_u64(fields, "expiry")?;
        let prices = fields.get("prices").and_then(|p| p.get("fields"));
        let spot = prices.and_then(|p| field_u64(p, "spot"));
        let svi = fields.get("svi")?.get("fields")?;
        let (rho_mag, rho_neg) = parse_i64_fp(svi.get("rho")?)?;
        let (m_mag, m_neg) = parse_i64_fp(svi.get("m")?)?;
        Some(Self {
            underlying_asset: field_str(fields, "underlying_asset").map(|s| s.to_string()),
            expiry_ms,
            spot,
            svi_a: field_u64(svi, "a")?,
            svi_b: field_u64(svi, "b")?,
            svi_rho_mag: rho_mag,
            svi_rho_neg: rho_neg,
            svi_m_mag: m_mag,
            svi_m_neg: m_neg,
            svi_sigma: field_u64(svi, "sigma")?,
            settlement_price: field_opt_u64(fields, "settlement_price"),
        })
    }
}

/// deepbook_predict::i64::I64 renders as {"fields": {"magnitude": "...", "is_negative": bool}}.
fn parse_i64_fp(v: &Value) -> Option<(u64, bool)> {
    let f = v.get("fields").unwrap_or(v);
    let mag = field_u64(f, "magnitude")?;
    let neg = f.get("is_negative").and_then(|b| b.as_bool()).unwrap_or(false);
    Some((mag, neg))
}

// === PoolConfig (apex::bridge::PoolConfig shared object) ===

#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct PoolConfigData {
    pub epsilon_bps: u64,
    pub k_scaling: u64,
    pub protocol_fee_bps: u64,
    pub keeper_tip_bps: u64,
    pub min_strike: u64,
    pub tick_size: u64,
    pub max_strike: u64,
    pub initial_shared_version: u64,
}

impl PoolConfigData {
    pub fn from_object(obj: &ObjectData) -> Option<Self> {
        let f = &obj.fields;
        Some(Self {
            epsilon_bps: field_u64(f, "epsilon_bps")?,
            k_scaling: field_u64(f, "k_scaling")?,
            protocol_fee_bps: field_u64(f, "protocol_fee_bps")?,
            keeper_tip_bps: field_u64(f, "keeper_tip_bps")?,
            min_strike: field_u64(f, "min_strike")?,
            tick_size: field_u64(f, "tick_size")?,
            max_strike: field_u64(f, "max_strike")?,
            initial_shared_version: obj.initial_shared_version?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_event_page() {
        let raw = json!({
            "data": [{
                "id": {"txDigest": "8tGi", "eventSeq": "0"},
                "packageId": "0xabc",
                "transactionModule": "apex_pool",
                "sender": "0x1",
                "type": "0xabc::apex_pool::CommitSubmitted",
                "parsedJson": {
                    "pool_id": "0xdef",
                    "participant": "0x1",
                    "entry_fee": "500000000",
                    "timestamp": "1760000000000"
                },
                "timestampMs": "1760000000000"
            }],
            "nextCursor": {"txDigest": "8tGi", "eventSeq": "0"},
            "hasNextPage": false
        });
        let page: EventPage = serde_json::from_value(raw).unwrap();
        assert_eq!(page.data.len(), 1);
        let ev = &page.data[0];
        assert!(ev.event_type.ends_with("::apex_pool::CommitSubmitted"));
        assert_eq!(field_u64(&ev.parsed_json, "entry_fee"), Some(500_000_000));
        assert_eq!(page.next_cursor.as_ref().unwrap().event_seq, "0");
    }

    #[test]
    fn decodes_shared_object_with_enum_state() {
        let raw = json!({
            "objectId": "0xpool",
            "version": "42",
            "digest": "9pXy",
            "owner": {"Shared": {"initial_shared_version": "7"}},
            "content": {
                "dataType": "moveObject",
                "type": "0xabc::apex_pool::Pool",
                "fields": {
                    "state": {"variant": "Open", "fields": {}},
                    "commit_deadline": "1760000000000",
                    "oracle_ids": ["0x1", "0x2"],
                    "oracle_results": [],
                    "predictions": {"fields": {"id": {"id": "0xtable"}, "size": "3"}},
                    "total_weight": "123456789012345678901"
                }
            }
        });
        let obj = ObjectData::from_rpc(&raw).unwrap();
        assert_eq!(obj.version, 42);
        assert_eq!(obj.initial_shared_version, Some(7));
        assert_eq!(
            field_enum_variant(&obj.fields, "state").as_deref(),
            Some("Open")
        );
        assert_eq!(field_u64(&obj.fields, "commit_deadline"), Some(1_760_000_000_000));
        assert_eq!(
            field_id_vec(&obj.fields, "oracle_ids").unwrap(),
            vec!["0x1", "0x2"]
        );
        assert_eq!(field_table_id(&obj.fields, "predictions").as_deref(), Some("0xtable"));
    }

    #[test]
    fn decodes_oracle_svi() {
        let raw = json!({
            "underlying_asset": "BTC",
            "expiry": "1760000000000",
            "active": true,
            "prices": {"fields": {"spot": "61200000000000", "forward": "61500000000000"}},
            "svi": {"fields": {
                "a": "40000000",
                "b": "100000000",
                "rho": {"fields": {"magnitude": "300000000", "is_negative": true}},
                "m": {"fields": {"magnitude": "100000000", "is_negative": false}},
                "sigma": "200000000"
            }},
            "timestamp": "1759990000000",
            "settlement_price": null
        });
        let svi = OracleSvi::from_fields(&raw).unwrap();
        assert_eq!(svi.expiry_ms, 1_760_000_000_000);
        assert_eq!(svi.spot, Some(61_200_000_000_000));
        assert!(svi.svi_rho_neg);
        assert_eq!(svi.svi_m_mag, 100_000_000);
        assert_eq!(svi.settlement_price, None);

        let settled = json!({
            "expiry": "1760000000000",
            "svi": raw["svi"].clone(),
            "settlement_price": "62450000000000"
        });
        let svi = OracleSvi::from_fields(&settled).unwrap();
        assert_eq!(svi.settlement_price, Some(62_450_000_000_000));
    }
}
