use std::{
    collections::HashSet,
    io::{self, Read},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use bech32::{FromBase32, Variant};
use chia_bls::{SecretKey, Signature, sign};
use chia_consensus::{flags::MEMPOOL_MODE, spendbundle_validation::validate_clvm_and_signature};
use chia_protocol::{Bytes32, Coin, SpendBundle};
use chia_puzzle_types::{DeriveSynthetic, Memos};
use chia_sdk_driver::{SpendContext, StandardLayer};
use chia_sdk_signer::{AggSigConstants, RequiredSignature};
use chia_sdk_types::{
    Conditions, MAINNET_CONSTANTS,
    conditions::{CreateCoin, ReserveFee},
};
use clvmr::{Allocator, NodePtr};
use flate2::read::GzDecoder;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const DEFAULT_RPC_URL: &str = "https://api.coinset.org";
const MAX_INPUT_COINS: usize = 100;
const MAX_RPC_BODY_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SyncRequest {
    #[serde(default = "default_rpc_url")]
    rpc_url: String,
    puzzle_hash: String,
}

#[derive(Debug, Clone, Serialize)]
struct HistoryEntry {
    coin_id: String,
    amount_mojo: u64,
    status: &'static str,
    confirmed_height: u64,
    spent_height: Option<u64>,
    timestamp: u64,
    coinbase: bool,
}

#[derive(Debug, Serialize)]
struct SyncOutput {
    schema: &'static str,
    network: &'static str,
    rpc_url: String,
    puzzle_hash: String,
    peak_height: u64,
    confirmed_balance_mojo: u64,
    unspent_coin_count: usize,
    total_coin_count: usize,
    synced_at_unix: u64,
    history: Vec<HistoryEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareSendRequest {
    #[serde(default = "default_rpc_url")]
    rpc_url: String,
    wallet_private_key_index0: String,
    expected_puzzle_hash: String,
    destination_address: String,
    amount_mojo: u64,
    fee_mojo: u64,
    purpose: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectedCoin {
    coin_id: String,
    parent_coin_info: String,
    puzzle_hash: String,
    amount_mojo: u64,
    confirmed_height: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparedSend {
    schema: String,
    protocol_version: String,
    network: String,
    rpc_url: String,
    source_puzzle_hash: String,
    destination_address: String,
    destination_puzzle_hash: String,
    amount_mojo: u64,
    fee_mojo: u64,
    input_total_mojo: u64,
    change_mojo: u64,
    purpose: String,
    selected_coins: Vec<SelectedCoin>,
    spend_bundle_id: String,
    spend_bundle: SpendBundle,
    consensus_conditions_verified: bool,
    aggregate_signature_verified: bool,
    broadcast_performed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BroadcastRequest {
    prepared: PreparedSend,
}

#[derive(Debug, Serialize)]
struct BroadcastOutput {
    schema: &'static str,
    network: &'static str,
    rpc_url: String,
    spend_bundle_id: String,
    status: String,
    success: bool,
    chain_broadcast: bool,
    submitted_at_unix: u64,
}

#[derive(Debug, Clone)]
struct CoinRecord {
    coin: Coin,
    confirmed_height: u64,
    spent_height: Option<u64>,
    timestamp: u64,
    coinbase: bool,
}

struct RpcClient {
    http: Client,
    base_url: String,
}

impl RpcClient {
    fn new(base_url: &str) -> Result<Self, String> {
        let parsed =
            reqwest::Url::parse(base_url).map_err(|error| format!("invalid RPC URL: {error}"))?;
        if parsed.scheme() != "https" || parsed.host_str().is_none() {
            return Err("RPC URL must be an HTTPS URL with a host".to_string());
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err("RPC URL must not contain embedded credentials".to_string());
        }
        let http = Client::builder()
            .timeout(Duration::from_secs(20))
            .default_headers({
                let mut headers = reqwest::header::HeaderMap::new();
                headers.insert(
                    reqwest::header::ACCEPT_ENCODING,
                    reqwest::header::HeaderValue::from_static("identity"),
                );
                headers
            })
            .build()
            .map_err(|error| format!("cannot build RPC client: {error}"))?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    fn call(&self, method: &str, body: Value) -> Result<Value, String> {
        let response = self
            .http
            .post(format!("{}/{method}", self.base_url))
            .json(&body)
            .send()
            .map_err(|error| format!("RPC {method} unavailable: {error}"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("RPC {method} returned HTTP {status}"));
        }
        let content_encoding = response
            .headers()
            .get(reqwest::header::CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let mut bytes = Vec::new();
        response
            .take(MAX_RPC_BODY_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("RPC {method} response body failed: {error}"))?;
        if bytes.len() as u64 > MAX_RPC_BODY_BYTES {
            return Err(format!("RPC {method} response exceeded 64 MiB"));
        }
        let decoded = if content_encoding == "gzip" {
            let decoder = GzDecoder::new(bytes.as_slice());
            let mut output = Vec::new();
            decoder
                .take(MAX_RPC_BODY_BYTES + 1)
                .read_to_end(&mut output)
                .map_err(|error| format!("RPC {method} gzip decode failed: {error}"))?;
            if output.len() as u64 > MAX_RPC_BODY_BYTES {
                return Err(format!("RPC {method} decoded response exceeded 64 MiB"));
            }
            output
        } else if content_encoding.is_empty() || content_encoding == "identity" {
            bytes
        } else {
            return Err(format!(
                "RPC {method} used unsupported content encoding {content_encoding}"
            ));
        };
        let value = serde_json::from_slice::<Value>(&decoded)
            .map_err(|error| format!("RPC {method} returned invalid JSON: {error}"))?;
        if value.get("success").and_then(Value::as_bool) == Some(false) {
            return Err(format!(
                "RPC {method} rejected the request: {}",
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
            ));
        }
        Ok(value)
    }

    fn require_mainnet(&self) -> Result<(), String> {
        let value = self.call("get_network_info", json!({}))?;
        let name_ok = value.get("network_name").and_then(Value::as_str) == Some("mainnet");
        let genesis_ok = value
            .get("genesis_challenge")
            .and_then(Value::as_str)
            .and_then(|text| decode_bytes32(text).ok())
            == Some(MAINNET_CONSTANTS.genesis_challenge);
        if !name_ok && !genesis_ok {
            return Err("RPC endpoint did not prove it is Chia mainnet".to_string());
        }
        Ok(())
    }

    fn peak_height(&self) -> Result<u64, String> {
        let value = self.call("get_blockchain_state", json!({}))?;
        parse_u64(
            value
                .pointer("/blockchain_state/peak/height")
                .ok_or("RPC omitted blockchain_state.peak.height")?,
            "peak.height",
        )
    }

    fn coin_records(&self, puzzle_hash: Bytes32) -> Result<Vec<CoinRecord>, String> {
        let response = self.call(
            "get_coin_records_by_puzzle_hash",
            json!({
                "puzzle_hash": format!("0x{}", hex::encode(puzzle_hash)),
                "include_spent_coins": true
            }),
        )?;
        let records = response
            .get("coin_records")
            .and_then(Value::as_array)
            .ok_or("RPC omitted coin_records")?;
        let mut result = Vec::with_capacity(records.len());
        let mut ids = HashSet::new();
        for record in records {
            let parsed = parse_coin_record(record)?;
            if parsed.coin.puzzle_hash != puzzle_hash {
                return Err("RPC returned a coin for a different puzzle hash".to_string());
            }
            if !ids.insert(parsed.coin.coin_id()) {
                return Err("RPC returned duplicate coin records".to_string());
            }
            result.push(parsed);
        }
        Ok(result)
    }

    fn require_unspent(&self, expected: &SelectedCoin) -> Result<(), String> {
        let response = self.call(
            "get_coin_record_by_name",
            json!({"name": format!("0x{}", expected.coin_id)}),
        )?;
        let value = response
            .get("coin_record")
            .ok_or_else(|| format!("selected coin {} is no longer available", expected.coin_id))?;
        if value.is_null() {
            return Err(format!(
                "selected coin {} no longer exists",
                expected.coin_id
            ));
        }
        let current = parse_coin_record(value)?;
        if current.spent_height.is_some()
            || hex::encode(current.coin.coin_id()) != expected.coin_id
            || hex::encode(current.coin.parent_coin_info) != expected.parent_coin_info
            || hex::encode(current.coin.puzzle_hash) != expected.puzzle_hash
            || current.coin.amount != expected.amount_mojo
        {
            return Err(format!(
                "selected coin {} changed or was already spent",
                expected.coin_id
            ));
        }
        Ok(())
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }
    match command.as_str() {
        "sync" => sync_wallet(),
        "prepare-send" => prepare_send(),
        "broadcast" => broadcast(),
        _ => Err(usage()),
    }
}

fn usage() -> String {
    "usage: xhub-wallet-chain-v3-6 <sync|prepare-send|broadcast>; JSON input is read from stdin"
        .to_string()
}

fn read_json<T: for<'de> Deserialize<'de>>() -> Result<T, String> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .map_err(|error| format!("cannot read stdin: {error}"))?;
    serde_json::from_str(&input).map_err(|error| format!("invalid request JSON: {error}"))
}

fn print_json<T: Serialize>(value: &T) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value)
            .map_err(|error| format!("cannot encode response JSON: {error}"))?
    );
    Ok(())
}

fn sync_wallet() -> Result<(), String> {
    let request: SyncRequest = read_json()?;
    let puzzle_hash = decode_bytes32(&request.puzzle_hash)?;
    let rpc = RpcClient::new(&request.rpc_url)?;
    rpc.require_mainnet()?;
    let peak_height = rpc.peak_height()?;
    let records = rpc.coin_records(puzzle_hash)?;

    let mut balance = 0_u64;
    let mut unspent_count = 0_usize;
    let mut history = Vec::with_capacity(records.len());
    for record in &records {
        if record.spent_height.is_none() {
            balance = balance
                .checked_add(record.coin.amount)
                .ok_or("balance exceeds u64")?;
            unspent_count += 1;
        }
        history.push(HistoryEntry {
            coin_id: hex::encode(record.coin.coin_id()),
            amount_mojo: record.coin.amount,
            status: if record.spent_height.is_some() {
                "SPENT"
            } else {
                "UNSPENT"
            },
            confirmed_height: record.confirmed_height,
            spent_height: record.spent_height,
            timestamp: record.timestamp,
            coinbase: record.coinbase,
        });
    }
    history.sort_by(|a, b| {
        b.confirmed_height
            .cmp(&a.confirmed_height)
            .then_with(|| b.coin_id.cmp(&a.coin_id))
    });
    print_json(&SyncOutput {
        schema: "xhub.wallet.v3_6.chain_sync.v1",
        network: "chia-mainnet",
        rpc_url: rpc.base_url,
        puzzle_hash: hex::encode(puzzle_hash),
        peak_height,
        confirmed_balance_mojo: balance,
        unspent_coin_count: unspent_count,
        total_coin_count: records.len(),
        synced_at_unix: now_unix()?,
        history,
    })
}

fn prepare_send() -> Result<(), String> {
    let request: PrepareSendRequest = read_json()?;
    if request.amount_mojo == 0 {
        return Err("amount_mojo must be greater than zero".to_string());
    }
    if request.purpose.trim().is_empty() {
        return Err("purpose must not be empty".to_string());
    }
    let required = request
        .amount_mojo
        .checked_add(request.fee_mojo)
        .ok_or("amount plus fee exceeds u64")?;
    let source_puzzle_hash = decode_bytes32(&request.expected_puzzle_hash)?;
    let destination_puzzle_hash = decode_mainnet_address(&request.destination_address)?;
    if source_puzzle_hash == destination_puzzle_hash {
        return Err("sending to the same strict index-0 address is not allowed".to_string());
    }
    let wallet_secret = parse_secret(&request.wallet_private_key_index0)?;
    let synthetic_secret = wallet_secret.derive_synthetic();
    let derived_puzzle_hash: Bytes32 =
        chia_puzzle_types::standard::StandardArgs::curry_tree_hash(synthetic_secret.public_key())
            .into();
    if derived_puzzle_hash != source_puzzle_hash {
        return Err("index-0 private key does not match the wallet puzzle hash".to_string());
    }

    let rpc = RpcClient::new(&request.rpc_url)?;
    rpc.require_mainnet()?;
    let records = rpc.coin_records(source_puzzle_hash)?;
    let selected = select_coins(&records, required)?;
    let input_total = selected.iter().try_fold(0_u64, |sum, record| {
        sum.checked_add(record.coin.amount)
            .ok_or("selected input total exceeds u64")
    })?;
    let change = input_total - required;
    let bundle = build_standard_bundle(
        &selected,
        &synthetic_secret,
        destination_puzzle_hash,
        request.amount_mojo,
        request.fee_mojo,
        change,
    )?;
    let spend_bundle_id = hex::encode(bundle.name());
    let selected_coins = selected
        .iter()
        .map(|record| SelectedCoin {
            coin_id: hex::encode(record.coin.coin_id()),
            parent_coin_info: hex::encode(record.coin.parent_coin_info),
            puzzle_hash: hex::encode(record.coin.puzzle_hash),
            amount_mojo: record.coin.amount,
            confirmed_height: record.confirmed_height,
        })
        .collect();
    print_json(&PreparedSend {
        schema: "xhub.wallet.v3_6.prepared_send.v1".to_string(),
        protocol_version: "3.6".to_string(),
        network: "chia-mainnet".to_string(),
        rpc_url: rpc.base_url,
        source_puzzle_hash: hex::encode(source_puzzle_hash),
        destination_address: request.destination_address,
        destination_puzzle_hash: hex::encode(destination_puzzle_hash),
        amount_mojo: request.amount_mojo,
        fee_mojo: request.fee_mojo,
        input_total_mojo: input_total,
        change_mojo: change,
        purpose: request.purpose.trim().to_string(),
        selected_coins,
        spend_bundle_id,
        spend_bundle: bundle,
        consensus_conditions_verified: true,
        aggregate_signature_verified: true,
        broadcast_performed: false,
    })
}

fn broadcast() -> Result<(), String> {
    let request: BroadcastRequest = read_json()?;
    let prepared = request.prepared;
    validate_prepared(&prepared)?;
    let rpc = RpcClient::new(&prepared.rpc_url)?;
    rpc.require_mainnet()?;
    for coin in &prepared.selected_coins {
        rpc.require_unspent(coin)?;
    }
    let response = rpc.call("push_tx", json!({"spend_bundle": &prepared.spend_bundle}))?;
    let status = response
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("SUCCESS")
        .to_string();
    print_json(&BroadcastOutput {
        schema: "xhub.wallet.v3_6.broadcast_result.v1",
        network: "chia-mainnet",
        rpc_url: rpc.base_url,
        spend_bundle_id: prepared.spend_bundle_id,
        status,
        success: true,
        chain_broadcast: true,
        submitted_at_unix: now_unix()?,
    })
}

fn select_coins(records: &[CoinRecord], required: u64) -> Result<Vec<CoinRecord>, String> {
    let mut available = records
        .iter()
        .filter(|record| record.spent_height.is_none())
        .cloned()
        .collect::<Vec<_>>();
    available.sort_by(|a, b| {
        b.coin
            .amount
            .cmp(&a.coin.amount)
            .then_with(|| a.coin.coin_id().cmp(&b.coin.coin_id()))
    });
    let mut selected = Vec::new();
    let mut total = 0_u64;
    for record in available {
        if selected.len() == MAX_INPUT_COINS {
            break;
        }
        total = total
            .checked_add(record.coin.amount)
            .ok_or("available balance exceeds u64")?;
        selected.push(record);
        if total >= required {
            return Ok(selected);
        }
    }
    Err(format!(
        "insufficient confirmed balance: need {required} mojo, selected only {total} mojo"
    ))
}

fn build_standard_bundle(
    records: &[CoinRecord],
    synthetic_secret: &SecretKey,
    destination_puzzle_hash: Bytes32,
    amount: u64,
    fee: u64,
    change: u64,
) -> Result<SpendBundle, String> {
    if records.is_empty() {
        return Err("at least one input coin is required".to_string());
    }
    let source_puzzle_hash = records[0].coin.puzzle_hash;
    if records
        .iter()
        .any(|record| record.coin.puzzle_hash != source_puzzle_hash)
    {
        return Err("all input coins must belong to the strict index-0 address".to_string());
    }
    let expected_source: Bytes32 =
        chia_puzzle_types::standard::StandardArgs::curry_tree_hash(synthetic_secret.public_key())
            .into();
    if expected_source != source_puzzle_hash {
        return Err("input coins do not match the signing key".to_string());
    }

    let mut ctx = SpendContext::new();
    let standard = StandardLayer::new(synthetic_secret.public_key());
    let mut first_conditions = Conditions::new();
    first_conditions.push(CreateCoin::<NodePtr>::new(
        destination_puzzle_hash,
        amount,
        Memos::None,
    ));
    if change > 0 {
        first_conditions.push(CreateCoin::<NodePtr>::new(
            source_puzzle_hash,
            change,
            Memos::None,
        ));
    }
    if fee > 0 {
        first_conditions.push(ReserveFee::new(fee));
    }
    standard
        .spend(&mut ctx, records[0].coin, first_conditions)
        .map_err(|error| format!("cannot construct first standard spend: {error}"))?;
    for record in &records[1..] {
        standard
            .spend(&mut ctx, record.coin, Conditions::new())
            .map_err(|error| format!("cannot construct standard spend: {error}"))?;
    }
    let coin_spends = ctx.take();
    let required_signatures = RequiredSignature::from_coin_spends(
        &mut Allocator::new(),
        &coin_spends,
        &AggSigConstants::from(&*MAINNET_CONSTANTS),
    )
    .map_err(|error| format!("cannot derive required signatures: {error}"))?;
    let mut aggregated_signature = Signature::default();
    for required in required_signatures {
        match required {
            RequiredSignature::Bls(required) => {
                if required.public_key != synthetic_secret.public_key() {
                    return Err("spend unexpectedly requires a different BLS key".to_string());
                }
                aggregated_signature += &sign(synthetic_secret, required.message());
            }
            RequiredSignature::Secp(_) => {
                return Err("standard XCH spend unexpectedly requires a secp signature".to_string());
            }
        }
    }
    let bundle = SpendBundle {
        coin_spends,
        aggregated_signature,
    };
    validate_bundle(&bundle, records, amount, fee, change)?;
    Ok(bundle)
}

fn validate_prepared(prepared: &PreparedSend) -> Result<(), String> {
    if prepared.schema != "xhub.wallet.v3_6.prepared_send.v1"
        || prepared.protocol_version != "3.6"
        || prepared.network != "chia-mainnet"
        || prepared.broadcast_performed
        || !prepared.consensus_conditions_verified
        || !prepared.aggregate_signature_verified
    {
        return Err("prepared transaction guard fields are invalid".to_string());
    }
    if hex::encode(prepared.spend_bundle.name()) != prepared.spend_bundle_id {
        return Err("prepared SpendBundle ID does not match its contents".to_string());
    }
    let records = prepared
        .selected_coins
        .iter()
        .map(|selected| {
            Ok(CoinRecord {
                coin: Coin::new(
                    decode_bytes32(&selected.parent_coin_info)?,
                    decode_bytes32(&selected.puzzle_hash)?,
                    selected.amount_mojo,
                ),
                confirmed_height: selected.confirmed_height,
                spent_height: None,
                timestamp: 0,
                coinbase: false,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    validate_bundle(
        &prepared.spend_bundle,
        &records,
        prepared.amount_mojo,
        prepared.fee_mojo,
        prepared.change_mojo,
    )
}

fn validate_bundle(
    bundle: &SpendBundle,
    records: &[CoinRecord],
    amount: u64,
    fee: u64,
    change: u64,
) -> Result<(), String> {
    let (conditions, _) = validate_clvm_and_signature(
        bundle,
        MAINNET_CONSTANTS.max_block_cost_clvm,
        &MAINNET_CONSTANTS,
        MEMPOOL_MODE,
    )
    .map_err(|error| format!("mainnet consensus/signature validation failed: {error:?}"))?;
    let input_total = records.iter().try_fold(0_u128, |sum, record| {
        sum.checked_add(u128::from(record.coin.amount))
            .ok_or("input total overflow")
    })?;
    let expected_output = u128::from(amount) + u128::from(change);
    if conditions.removal_amount != input_total
        || conditions.addition_amount != expected_output
        || conditions.reserve_fee != fee
        || input_total != expected_output + u128::from(fee)
    {
        return Err("SpendBundle amount conservation or fee validation failed".to_string());
    }
    let unique_removals = bundle
        .coin_spends
        .iter()
        .map(|spend| spend.coin.coin_id())
        .collect::<HashSet<_>>();
    if unique_removals.len() != bundle.coin_spends.len()
        || bundle.coin_spends.len() != records.len()
    {
        return Err("SpendBundle input set is duplicate or incomplete".to_string());
    }
    Ok(())
}

fn parse_coin_record(value: &Value) -> Result<CoinRecord, String> {
    let coin = value.get("coin").ok_or("coin record omitted coin")?;
    let parent = decode_bytes32(
        coin.get("parent_coin_info")
            .and_then(Value::as_str)
            .ok_or("coin omitted parent_coin_info")?,
    )?;
    let puzzle_hash = decode_bytes32(
        coin.get("puzzle_hash")
            .and_then(Value::as_str)
            .ok_or("coin omitted puzzle_hash")?,
    )?;
    let amount = parse_u64(
        coin.get("amount").ok_or("coin omitted amount")?,
        "coin.amount",
    )?;
    let spent = value
        .get("spent")
        .and_then(Value::as_bool)
        .ok_or("coin record omitted spent")?;
    let spent_height = if spent {
        Some(parse_u64(
            value
                .get("spent_block_index")
                .ok_or("coin record omitted spent_block_index")?,
            "spent_block_index",
        )?)
    } else {
        None
    };
    Ok(CoinRecord {
        coin: Coin::new(parent, puzzle_hash, amount),
        confirmed_height: parse_u64(
            value
                .get("confirmed_block_index")
                .ok_or("coin record omitted confirmed_block_index")?,
            "confirmed_block_index",
        )?,
        spent_height,
        timestamp: value
            .get("timestamp")
            .map(|value| parse_u64(value, "timestamp"))
            .transpose()?
            .unwrap_or(0),
        coinbase: value
            .get("coinbase")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn decode_mainnet_address(address: &str) -> Result<Bytes32, String> {
    let (prefix, data, variant) =
        bech32::decode(address).map_err(|error| format!("invalid destination address: {error}"))?;
    let bytes = Vec::<u8>::from_base32(&data)
        .map_err(|error| format!("invalid destination address data: {error}"))?;
    if prefix != "xch" || variant != Variant::Bech32m || bytes.len() != 32 {
        return Err("destination must be a Chia mainnet xch Bech32m address".to_string());
    }
    Ok(Bytes32::new(bytes.try_into().expect("length checked")))
}

fn decode_bytes32(value: &str) -> Result<Bytes32, String> {
    let bytes = hex::decode(value.strip_prefix("0x").unwrap_or(value))
        .map_err(|error| format!("invalid 32-byte hex: {error}"))?;
    Ok(Bytes32::new(bytes.try_into().map_err(|_| {
        "value must be exactly 32 bytes".to_string()
    })?))
}

fn parse_secret(value: &str) -> Result<SecretKey, String> {
    let bytes = hex::decode(value).map_err(|error| format!("invalid private key hex: {error}"))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "private key must be exactly 32 bytes".to_string())?;
    SecretKey::from_bytes(&bytes).map_err(|error| format!("invalid BLS private key: {error}"))
}

fn parse_u64(value: &Value, field: &str) -> Result<u64, String> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .ok_or_else(|| format!("{field} is not a u64"))
}

fn default_rpc_url() -> String {
    DEFAULT_RPC_URL.to_string()
}

fn now_unix() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| format!("system clock is before Unix epoch: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(coin: Coin) -> CoinRecord {
        CoinRecord {
            coin,
            confirmed_height: 100,
            spent_height: None,
            timestamp: 1,
            coinbase: false,
        }
    }

    #[test]
    fn builds_and_validates_mainnet_standard_bundle() {
        let wallet = SecretKey::from_seed(&[7; 32]);
        let synthetic = wallet.derive_synthetic();
        let source: Bytes32 =
            chia_puzzle_types::standard::StandardArgs::curry_tree_hash(synthetic.public_key())
                .into();
        let records = vec![
            record(Coin::new(Bytes32::new([1; 32]), source, 4)),
            record(Coin::new(Bytes32::new([2; 32]), source, 8)),
        ];
        let bundle =
            build_standard_bundle(&records, &synthetic, Bytes32::new([9; 32]), 7, 1, 4).unwrap();
        assert_eq!(bundle.coin_spends.len(), 2);
        assert_ne!(bundle.aggregated_signature, Signature::default());

        let prepared = PreparedSend {
            schema: "xhub.wallet.v3_6.prepared_send.v1".to_string(),
            protocol_version: "3.6".to_string(),
            network: "chia-mainnet".to_string(),
            rpc_url: DEFAULT_RPC_URL.to_string(),
            source_puzzle_hash: hex::encode(source),
            destination_address: "test-only".to_string(),
            destination_puzzle_hash: hex::encode([9; 32]),
            amount_mojo: 7,
            fee_mojo: 1,
            input_total_mojo: 12,
            change_mojo: 4,
            purpose: "serde round trip".to_string(),
            selected_coins: records
                .iter()
                .map(|record| SelectedCoin {
                    coin_id: hex::encode(record.coin.coin_id()),
                    parent_coin_info: hex::encode(record.coin.parent_coin_info),
                    puzzle_hash: hex::encode(record.coin.puzzle_hash),
                    amount_mojo: record.coin.amount,
                    confirmed_height: record.confirmed_height,
                })
                .collect(),
            spend_bundle_id: hex::encode(bundle.name()),
            spend_bundle: bundle,
            consensus_conditions_verified: true,
            aggregate_signature_verified: true,
            broadcast_performed: false,
        };
        let json = serde_json::to_string(&prepared).unwrap();
        let decoded: PreparedSend = serde_json::from_str(&json).unwrap();
        validate_prepared(&decoded).unwrap();
    }

    #[test]
    fn selects_largest_coins_and_rejects_insufficient_balance() {
        let puzzle_hash = Bytes32::new([3; 32]);
        let records = vec![
            record(Coin::new(Bytes32::new([1; 32]), puzzle_hash, 2)),
            record(Coin::new(Bytes32::new([2; 32]), puzzle_hash, 7)),
            record(Coin::new(Bytes32::new([3; 32]), puzzle_hash, 4)),
        ];
        let selected = select_coins(&records, 9).unwrap();
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].coin.amount, 7);
        assert!(select_coins(&records, 14).is_err());
    }

    #[test]
    fn rejects_non_https_rpc_and_non_mainnet_address() {
        assert!(RpcClient::new("http://api.coinset.org").is_err());
        assert!(decode_mainnet_address("txch1invalid").is_err());
    }
}
