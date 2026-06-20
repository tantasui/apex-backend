//! PTB construction, signing and execution for keeper calls into
//! `{package}::apex_pool`. Built offline with sui-transaction-builder,
//! executed over JSON-RPC.

use std::str::FromStr;

use anyhow::{anyhow, Context};
use base64::Engine;
use bech32::primitives::decode::CheckedHrpstring;
use bech32::{Bech32, Hrp};
use sui_crypto::ed25519::Ed25519PrivateKey;
use sui_crypto::SuiSigner;
use sui_sdk_types::{Address, Digest, Identifier};
use sui_transaction_builder::{Function, ObjectInput, TransactionBuilder};

use crate::sui::SuiClient;

const GAS_BUDGET: u64 = 100_000_000; // 0.1 SUI
const CLOCK_OBJECT_ID: &str = "0x6";
const CLOCK_INITIAL_VERSION: u64 = 1;

/// Decode a `suiprivkey1...` bech32 string into an Ed25519 private key.
/// Payload is `flag(1) || key(32)`, flag 0x00 = Ed25519.
fn decode_suiprivkey(s: &str) -> anyhow::Result<Ed25519PrivateKey> {
    let hrp = Hrp::parse("suiprivkey").expect("valid hrp");
    let parsed = CheckedHrpstring::new::<Bech32>(s)
        .map_err(|e| anyhow!("invalid suiprivkey bech32: {e}"))?;
    if parsed.hrp() != hrp {
        return Err(anyhow!("expected 'suiprivkey' HRP"));
    }
    let bytes: Vec<u8> = parsed.byte_iter().collect();
    let (flag, key) = bytes
        .split_first()
        .ok_or_else(|| anyhow!("suiprivkey payload is empty"))?;
    if *flag != 0x00 {
        return Err(anyhow!("expected Ed25519 flag 0x00, got {flag:#x}"));
    }
    let arr: [u8; 32] = key
        .try_into()
        .map_err(|_| anyhow!("Ed25519 key must be 32 bytes, got {}", key.len()))?;
    Ok(Ed25519PrivateKey::new(arr))
}

pub struct KeeperSigner {
    key: Ed25519PrivateKey,
    pub address: Address,
}

impl KeeperSigner {
    pub fn from_bech32(suiprivkey: &str) -> anyhow::Result<Self> {
        let key = decode_suiprivkey(suiprivkey)
            .context("failed to decode SUI_KEEPER_KEY")?;
        let address = key.public_key().derive_address();
        Ok(Self { key, address })
    }
}

/// Argument kinds for one keeper move call, in declaration order.
pub enum CallArg {
    /// Shared object passed `&mut` (the Pool).
    SharedMut { id: String, initial_version: u64 },
    /// Shared object passed `&` (the PoolConfig).
    SharedRef { id: String, initial_version: u64 },
    /// `&Clock` at 0x6.
    ClockRef,
    /// The owned KeeperCap; a fresh object ref is fetched per call.
    OwnedKeeperCap,
    /// BCS-encoded `vector<u64>` (oracle results).
    PureU64Vec(Vec<u64>),
    /// BCS-encoded single `u64`.
    PureU64(u64),
    /// BCS-encoded `vector<address>` / `vector<ID>` (oracle object IDs).
    PureObjectIdVec(Vec<String>),
}

pub struct KeeperCtx<'a> {
    pub sui: &'a SuiClient,
    pub signer: &'a KeeperSigner,
    pub package_id: &'a str,
    pub keeper_cap_id: &'a str,
}

/// Build, sign and execute `{package}::apex_pool::{function}(args)`.
/// Returns (digest, success, error).
pub async fn execute_call(
    ctx: &KeeperCtx<'_>,
    function: &str,
    args: Vec<CallArg>,
) -> anyhow::Result<(String, bool, Option<String>)> {
    let mut tx = TransactionBuilder::new();
    let package = Address::from_str(ctx.package_id).context("bad package id")?;

    let mut built_args = Vec::with_capacity(args.len());
    for arg in args {
        let built = match arg {
            CallArg::SharedMut { id, initial_version } => tx.object(ObjectInput::shared(
                Address::from_str(&id).context("bad shared object id")?,
                initial_version,
                true,
            )),
            CallArg::SharedRef { id, initial_version } => tx.object(ObjectInput::shared(
                Address::from_str(&id).context("bad shared object id")?,
                initial_version,
                false,
            )),
            CallArg::ClockRef => tx.object(ObjectInput::shared(
                Address::from_str(CLOCK_OBJECT_ID).expect("clock id"),
                CLOCK_INITIAL_VERSION,
                false,
            )),
            CallArg::OwnedKeeperCap => {
                let cap = ctx.sui.get_object(ctx.keeper_cap_id).await?;
                tx.object(ObjectInput::owned(
                    Address::from_str(&cap.object_id).context("bad keeper cap id")?,
                    cap.version,
                    Digest::from_str(&cap.digest).context("bad keeper cap digest")?,
                ))
            }
            CallArg::PureU64Vec(values) => tx.pure(&values),
            CallArg::PureU64(value) => tx.pure(&value),
            CallArg::PureObjectIdVec(ids) => {
                let bytes: anyhow::Result<Vec<[u8; 32]>> = ids
                    .iter()
                    .map(|id| {
                        let addr = Address::from_str(id).context("bad oracle id")?;
                        let arr: [u8; 32] = addr
                            .as_bytes()
                            .try_into()
                            .map_err(|_| anyhow!("Address must be 32 bytes"))?;
                        Ok(arr)
                    })
                    .collect();
                tx.pure(&bytes?)
            }
        };
        built_args.push(built);
    }

    let func = Function::new(
        package,
        Identifier::new("apex_pool").expect("valid module name"),
        Identifier::new(function).context("bad function name")?,
    );
    tx.move_call(func, built_args);

    // Gas: largest SUI coin owned by the keeper.
    let coins = ctx.sui.get_sui_coins(&ctx.signer.address.to_string()).await?;
    let (coin_id, coin_version, coin_digest, _) = coins
        .into_iter()
        .max_by_key(|(_, _, _, balance)| *balance)
        .ok_or_else(|| anyhow!("keeper address {} has no SUI gas coins", ctx.signer.address))?;
    tx.add_gas_objects(vec![ObjectInput::owned(
        Address::from_str(&coin_id)?,
        coin_version,
        Digest::from_str(&coin_digest)?,
    )]);
    tx.set_gas_budget(GAS_BUDGET);
    tx.set_gas_price(ctx.sui.reference_gas_price().await?);
    tx.set_sender(ctx.signer.address);

    let transaction = tx.try_build().map_err(|e| anyhow!("PTB build failed: {e}"))?;
    let tx_bytes = bcs::to_bytes(&transaction).context("bcs serialize tx")?;
    let tx_b64 = base64::engine::general_purpose::STANDARD.encode(&tx_bytes);

    let signature = ctx
        .signer
        .key
        .sign_transaction(&transaction)
        .map_err(|e| anyhow!("signing failed: {e}"))?;
    let sig_b64 = signature.to_base64();

    ctx.sui.execute_transaction(&tx_b64, &[sig_b64]).await
}
