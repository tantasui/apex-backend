//! Minimal Sui fullnode JSON-RPC client. One reqwest client serves the
//! indexer (events), the API previews (object reads) and the keeper
//! (object reads + transaction execution).

use anyhow::{anyhow, Context};
use serde_json::{json, Value};

use super::types::{EventId, EventPage, ObjectData};

#[derive(Debug, Clone)]
pub struct SuiClient {
    http: reqwest::Client,
    url: String,
}

impl SuiClient {
    pub fn new(url: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            url: url.to_string(),
        }
    }

    pub async fn rpc(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let resp: Value = self
            .http
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("{method}: request failed"))?
            .error_for_status()
            .with_context(|| format!("{method}: http error"))?
            .json()
            .await
            .with_context(|| format!("{method}: invalid json"))?;
        if let Some(err) = resp.get("error") {
            return Err(anyhow!("{method}: rpc error: {err}"));
        }
        resp.get("result")
            .cloned()
            .ok_or_else(|| anyhow!("{method}: missing result"))
    }

    /// `suix_queryEvents` with a MoveEventModule filter, ascending order.
    pub async fn query_module_events(
        &self,
        package: &str,
        module: &str,
        cursor: Option<&EventId>,
        limit: u64,
    ) -> anyhow::Result<EventPage> {
        let cursor_json = match cursor {
            Some(c) => json!({ "txDigest": c.tx_digest, "eventSeq": c.event_seq }),
            None => Value::Null,
        };
        let result = self
            .rpc(
                "suix_queryEvents",
                json!([
                    { "MoveEventModule": { "package": package, "module": module } },
                    cursor_json,
                    limit,
                    false
                ]),
            )
            .await?;
        serde_json::from_value(result).context("suix_queryEvents: unexpected response shape")
    }

    pub async fn get_object(&self, object_id: &str) -> anyhow::Result<ObjectData> {
        let result = self
            .rpc(
                "sui_getObject",
                json!([object_id, { "showContent": true, "showOwner": true }]),
            )
            .await?;
        let data = result
            .get("data")
            .ok_or_else(|| anyhow!("sui_getObject: object {object_id} not found: {result}"))?;
        ObjectData::from_rpc(data)
            .ok_or_else(|| anyhow!("sui_getObject: cannot parse object {object_id}"))
    }

    /// Best-effort batch read; objects that fail to parse are skipped.
    pub async fn multi_get_objects(&self, ids: &[String]) -> anyhow::Result<Vec<ObjectData>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let result = self
            .rpc(
                "sui_multiGetObjects",
                json!([ids, { "showContent": true, "showOwner": true }]),
            )
            .await?;
        let arr = result
            .as_array()
            .ok_or_else(|| anyhow!("sui_multiGetObjects: expected array"))?;
        Ok(arr
            .iter()
            .filter_map(|entry| entry.get("data").and_then(ObjectData::from_rpc))
            .collect())
    }

    /// Dynamic field lookup on a Table<address, V> by address key.
    pub async fn get_table_entry_by_address(
        &self,
        table_id: &str,
        address: &str,
    ) -> anyhow::Result<Option<ObjectData>> {
        let result = self
            .rpc(
                "suix_getDynamicFieldObject",
                json!([table_id, { "type": "address", "value": address }]),
            )
            .await?;
        Ok(result.get("data").and_then(ObjectData::from_rpc))
    }

    /// All SUI gas coins owned by an address (first page, plenty for a keeper).
    pub async fn get_sui_coins(&self, owner: &str) -> anyhow::Result<Vec<(String, u64, String, u64)>> {
        let result = self
            .rpc("suix_getCoins", json!([owner, "0x2::sui::SUI", null, 50]))
            .await?;
        let mut coins = vec![];
        if let Some(arr) = result.get("data").and_then(|d| d.as_array()) {
            for c in arr {
                let id = c.get("coinObjectId").and_then(|v| v.as_str());
                let version = c.get("version").and_then(super::types::parse_u64);
                let digest = c.get("digest").and_then(|v| v.as_str());
                let balance = c.get("balance").and_then(super::types::parse_u64);
                if let (Some(id), Some(version), Some(digest), Some(balance)) =
                    (id, version, digest, balance)
                {
                    coins.push((id.to_string(), version, digest.to_string(), balance));
                }
            }
        }
        Ok(coins)
    }

    pub async fn reference_gas_price(&self) -> anyhow::Result<u64> {
        let result = self.rpc("suix_getReferenceGasPrice", json!([])).await?;
        super::types::parse_u64(&result).ok_or_else(|| anyhow!("bad gas price: {result}"))
    }

    /// Execute a signed transaction; returns (digest, success, error_text).
    pub async fn execute_transaction(
        &self,
        tx_bytes_b64: &str,
        signatures_b64: &[String],
    ) -> anyhow::Result<(String, bool, Option<String>)> {
        let result = self
            .rpc(
                "sui_executeTransactionBlock",
                json!([
                    tx_bytes_b64,
                    signatures_b64,
                    { "showEffects": true },
                    "WaitForLocalExecution"
                ]),
            )
            .await?;
        let digest = result
            .get("digest")
            .and_then(|d| d.as_str())
            .unwrap_or_default()
            .to_string();
        let status = result
            .get("effects")
            .and_then(|e| e.get("status"))
            .cloned()
            .unwrap_or(Value::Null);
        let success = status.get("status").and_then(|s| s.as_str()) == Some("success");
        let error = status
            .get("error")
            .and_then(|e| e.as_str())
            .map(|s| s.to_string());
        Ok((digest, success, error))
    }
}
