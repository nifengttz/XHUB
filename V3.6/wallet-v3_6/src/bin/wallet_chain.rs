use std::{
    collections::HashSet,
    io::{self, Read},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(test)]
use bech32::ToBase32;
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
use xhub_protocol_v3_6::{CanonicalDecode, ChannelTerms};
use xhub_wallet_v3_6::{
    FUNDING_CONFIRMATION_BLOCKS_TEST, FundingDraft, FundingTermsInput, MAINNET_NETWORK_ID,
};

const DEFAULT_RPC_URL: &str = "https://api.coinset.org";
const DEFAULT_WALLET_SERVICE_URL: &str = "https://wallet.chiagame.top";
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareFundingTermsRequest {
    #[serde(default = "default_wallet_service_url")]
    wallet_service_url: String,
    wallet_public_key_index0: String,
    expected_remainder_puzzle_hash: String,
    funding_amount_mojo: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmFundingTermsRequest {
    #[serde(default = "default_wallet_service_url")]
    wallet_service_url: String,
    wallet_public_key_index0: String,
    expected_remainder_puzzle_hash: String,
    funding_amount_mojo: u64,
    draft: FundingDraft,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareFundingRequest {
    #[serde(default = "default_rpc_url")]
    rpc_url: String,
    #[serde(default = "default_wallet_service_url")]
    wallet_service_url: String,
    wallet_private_key_index0: String,
    wallet_public_key_index0: String,
    expected_puzzle_hash: String,
    fee_mojo: u64,
    confirmed_draft: FundingDraft,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FundingMetadata {
    wallet_service_url: String,
    draft: FundingDraft,
    predicted_funding_coin_id: String,
    required_confirmations: u64,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    funding: Option<FundingMetadata>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    funding_coin_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FundingStatusRequest {
    #[serde(default = "default_rpc_url")]
    rpc_url: String,
    funding_coin_id: String,
    funding_puzzle_hash: String,
    funding_amount_mojo: u64,
    #[serde(default = "default_funding_confirmations")]
    required_confirmations: u64,
}

#[derive(Debug, Clone, Serialize)]
struct FundingStatusOutput {
    schema: &'static str,
    network: &'static str,
    rpc_url: String,
    funding_coin_id: String,
    funding_puzzle_hash: String,
    funding_amount_mojo: u64,
    peak_height: u64,
    status: &'static str,
    confirmed_height: Option<u64>,
    spent_height: Option<u64>,
    confirmations: u64,
    required_confirmations: u64,
    registration_ready: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterFundingRequest {
    #[serde(default = "default_rpc_url")]
    rpc_url: String,
    #[serde(default = "default_wallet_service_url")]
    wallet_service_url: String,
    wallet_public_key_index0: String,
    expected_remainder_puzzle_hash: String,
    funding_coin_id: String,
    confirmed_draft: FundingDraft,
}

#[derive(Debug, Serialize)]
struct RegisterFundingOutput {
    schema: &'static str,
    chain: FundingStatusOutput,
    hub_response: Value,
}

#[derive(Debug, Deserialize)]
struct RemoteProfile {
    network_id: String,
    acceptance_blocks: u64,
    freeze_blocks: u64,
    challenge_blocks: u64,
    funding_confirmation_blocks: u64,
    hub_state_public_key_a: String,
    state_rules_hash: String,
    hub_gateway_enabled: bool,
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

struct WalletServiceClient {
    http: Client,
    base_url: String,
}

impl WalletServiceClient {
    fn new(base_url: &str) -> Result<Self, String> {
        let parsed = reqwest::Url::parse(base_url)
            .map_err(|error| format!("invalid wallet service URL: {error}"))?;
        if parsed.scheme() != "https" || parsed.host_str().is_none() {
            return Err("wallet service URL must be an HTTPS URL with a host".to_string());
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err("wallet service URL must not contain embedded credentials".to_string());
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err("wallet service URL must not contain a query or fragment".to_string());
        }
        let http = Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|error| format!("cannot build wallet service client: {error}"))?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
        })
    }

    fn get<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T, String> {
        self.decode(
            "GET",
            path,
            self.http.get(format!("{}{}", self.base_url, path)).send(),
        )
    }

    fn post<T: for<'de> Deserialize<'de>>(&self, path: &str, body: &Value) -> Result<T, String> {
        self.decode(
            "POST",
            path,
            self.http
                .post(format!("{}{}", self.base_url, path))
                .header("x-xhub-protocol-version", "0x0360")
                .json(body)
                .send(),
        )
    }

    fn decode<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        path: &str,
        response: Result<reqwest::blocking::Response, reqwest::Error>,
    ) -> Result<T, String> {
        let response = response
            .map_err(|error| format!("wallet service {method} {path} unavailable: {error}"))?;
        let status = response.status();
        let value: Value = response.json().map_err(|error| {
            format!("wallet service {method} {path} returned invalid JSON: {error}")
        })?;
        if !status.is_success() {
            return Err(format!(
                "wallet service {method} {path} returned HTTP {status}: {}",
                value
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("request rejected")
            ));
        }
        serde_json::from_value(value)
            .map_err(|error| format!("wallet service {method} {path} response mismatch: {error}"))
    }
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

    fn coin_record_by_id(&self, coin_id: Bytes32) -> Result<Option<CoinRecord>, String> {
        let response = self.call(
            "get_coin_record_by_name",
            json!({"name": format!("0x{}", hex::encode(coin_id))}),
        )?;
        match response.get("coin_record") {
            Some(Value::Null) | None => Ok(None),
            Some(value) => parse_coin_record(value).map(Some),
        }
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
        "prepare-funding-terms" => prepare_funding_terms(),
        "confirm-funding-terms" => confirm_funding_terms(),
        "prepare-funding" => prepare_funding(),
        "funding-status" => funding_status(),
        "register-funding" => register_funding(),
        "prepare-send" => prepare_send(),
        "broadcast" => broadcast(),
        _ => Err(usage()),
    }
}

fn usage() -> String {
    "usage: xhub-wallet-chain-v3-6 <sync|prepare-funding-terms|confirm-funding-terms|prepare-funding|funding-status|register-funding|prepare-send|broadcast>; JSON input is read from stdin".to_string()
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

fn prepare_funding_terms() -> Result<(), String> {
    let request: PrepareFundingTermsRequest = read_json()?;
    if request.funding_amount_mojo == 0 {
        return Err("funding_amount_mojo must be greater than zero".to_string());
    }
    let service = WalletServiceClient::new(&request.wallet_service_url)?;
    let profile: RemoteProfile = service.get("/api/v3.6/config")?;
    validate_remote_profile(&profile)?;
    let terms = FundingTermsInput {
        network_id: profile.network_id,
        acceptance_blocks: profile.acceptance_blocks.to_string(),
        freeze_blocks: profile.freeze_blocks.to_string(),
        challenge_blocks: profile.challenge_blocks.to_string(),
        user_public_key: normalize_fixed_hex::<48>(
            &request.wallet_public_key_index0,
            "wallet_public_key_index0",
        )?,
        hub_state_public_key_a: profile.hub_state_public_key_a,
        state_rules_hash: profile.state_rules_hash,
        funding_amount: request.funding_amount_mojo.to_string(),
        user_remainder_puzzle_hash: normalize_fixed_hex::<32>(
            &request.expected_remainder_puzzle_hash,
            "expected_remainder_puzzle_hash",
        )?,
    };
    let body = funding_terms_body(&terms);
    let draft: FundingDraft = service.post("/api/v3.6/funding-drafts", &body)?;
    validate_funding_draft(
        &draft,
        &request.wallet_public_key_index0,
        &request.expected_remainder_puzzle_hash,
        request.funding_amount_mojo,
        false,
    )?;
    print_json(&draft)
}

fn confirm_funding_terms() -> Result<(), String> {
    let request: ConfirmFundingTermsRequest = read_json()?;
    validate_funding_draft(
        &request.draft,
        &request.wallet_public_key_index0,
        &request.expected_remainder_puzzle_hash,
        request.funding_amount_mojo,
        false,
    )?;
    let terms = decode_draft_terms(&request.draft)?;
    let service = WalletServiceClient::new(&request.wallet_service_url)?;
    let prepared: FundingDraft = service.post(
        "/api/v3.6/funding-drafts",
        &funding_terms_body_from_channel(&terms),
    )?;
    if prepared.preview != request.draft.preview || prepared.draft_id != request.draft.draft_id {
        return Err("wallet service returned different immutable Funding terms".to_string());
    }
    let confirmed = if prepared.confirmed {
        prepared
    } else {
        service.post(
            &format!(
                "/api/v3.6/funding-drafts/{}/confirm",
                request.draft.draft_id
            ),
            &json!({
                "protocol_version": "0x0360",
                "channel_terms_hash": request.draft.preview.channel_terms_hash,
                "user_confirmed": true
            }),
        )?
    };
    validate_funding_draft(
        &confirmed,
        &request.wallet_public_key_index0,
        &request.expected_remainder_puzzle_hash,
        request.funding_amount_mojo,
        true,
    )?;
    print_json(&confirmed)
}

fn prepare_funding() -> Result<(), String> {
    let request: PrepareFundingRequest = read_json()?;
    let funding_amount = decode_draft_terms(&request.confirmed_draft)?.funding_amount;
    let terms = validate_funding_draft(
        &request.confirmed_draft,
        &request.wallet_public_key_index0,
        &request.expected_puzzle_hash,
        funding_amount,
        true,
    )?;
    let wallet_secret = parse_secret(&request.wallet_private_key_index0)?;
    if hex::encode(wallet_secret.public_key().to_bytes())
        != normalize_fixed_hex::<48>(
            &request.wallet_public_key_index0,
            "wallet_public_key_index0",
        )?
    {
        return Err("index-0 private key does not match the index-0 public key".to_string());
    }
    let service = WalletServiceClient::new(&request.wallet_service_url)?;
    let profile: RemoteProfile = service.get("/api/v3.6/config")?;
    validate_remote_profile(&profile)?;
    if profile.hub_state_public_key_a != hex::encode(terms.hub_state_public_key_a)
        || profile.state_rules_hash != hex::encode(terms.state_rules_hash)
        || profile.acceptance_blocks != terms.acceptance_blocks
        || profile.freeze_blocks != terms.freeze_blocks
        || profile.challenge_blocks != terms.challenge_blocks
    {
        return Err("confirmed Funding terms no longer match the deployed HUB profile".to_string());
    }
    let send = PrepareSendRequest {
        rpc_url: request.rpc_url,
        wallet_private_key_index0: request.wallet_private_key_index0,
        expected_puzzle_hash: request.expected_puzzle_hash,
        destination_address: request.confirmed_draft.preview.funding_address.clone(),
        amount_mojo: terms.funding_amount,
        fee_mojo: request.fee_mojo,
        purpose: format!(
            "XHUB V3.6 Funding Coin {}",
            request.confirmed_draft.preview.channel_terms_hash
        ),
    };
    let mut prepared = prepare_send_request(send)?;
    let first_parent = decode_bytes32(&prepared.selected_coins[0].coin_id)?;
    let funding_coin = Coin::new(
        first_parent,
        decode_bytes32(&prepared.destination_puzzle_hash)?,
        prepared.amount_mojo,
    );
    prepared.funding = Some(FundingMetadata {
        wallet_service_url: service.base_url,
        draft: request.confirmed_draft,
        predicted_funding_coin_id: hex::encode(funding_coin.coin_id()),
        required_confirmations: profile.funding_confirmation_blocks,
    });
    validate_prepared(&prepared)?;
    print_json(&prepared)
}

fn funding_status() -> Result<(), String> {
    let request: FundingStatusRequest = read_json()?;
    let status = funding_status_for(
        &request.rpc_url,
        &request.funding_coin_id,
        &request.funding_puzzle_hash,
        request.funding_amount_mojo,
        request.required_confirmations,
    )?;
    print_json(&status)
}

fn register_funding() -> Result<(), String> {
    let request: RegisterFundingRequest = read_json()?;
    let amount = decode_draft_terms(&request.confirmed_draft)?.funding_amount;
    validate_funding_draft(
        &request.confirmed_draft,
        &request.wallet_public_key_index0,
        &request.expected_remainder_puzzle_hash,
        amount,
        true,
    )?;
    let status = funding_status_for(
        &request.rpc_url,
        &request.funding_coin_id,
        &request.confirmed_draft.preview.funding_puzzle_hash,
        amount,
        request.confirmed_draft.preview.funding_confirmation_blocks,
    )?;
    if !status.registration_ready {
        return Err(format!(
            "Funding Coin is not ready for HUB registration: status={}, confirmations={}/{}",
            status.status, status.confirmations, status.required_confirmations
        ));
    }
    let service = WalletServiceClient::new(&request.wallet_service_url)?;
    let hub_response: Value = service.post(
        "/api/v3.6/hub/funding-coins",
        &json!({
            "protocol_version": "0x0360",
            "funding_coin_id": request.funding_coin_id,
            "funding_puzzle_reveal_hex": request.confirmed_draft.preview.funding_puzzle_reveal,
            "channel_terms_canonical_hex": request.confirmed_draft.preview.channel_terms_canonical_hex
        }),
    )?;
    let returned_coin = hub_response
        .get("funding_coin_id")
        .and_then(Value::as_str)
        .ok_or("HUB registration response omitted funding_coin_id")?;
    if returned_coin != status.funding_coin_id {
        return Err("HUB registration response returned a different Funding Coin ID".to_string());
    }
    print_json(&RegisterFundingOutput {
        schema: "xhub.wallet.v3_6.funding_registration.v1",
        chain: status,
        hub_response,
    })
}

fn funding_status_for(
    rpc_url: &str,
    funding_coin_id: &str,
    funding_puzzle_hash: &str,
    funding_amount_mojo: u64,
    required_confirmations: u64,
) -> Result<FundingStatusOutput, String> {
    if funding_amount_mojo == 0 || required_confirmations == 0 {
        return Err(
            "Funding amount and required confirmations must be greater than zero".to_string(),
        );
    }
    let coin_id = decode_bytes32(funding_coin_id)?;
    let expected_puzzle_hash = decode_bytes32(funding_puzzle_hash)?;
    let rpc = RpcClient::new(rpc_url)?;
    rpc.require_mainnet()?;
    let peak_height = rpc.peak_height()?;
    let record = rpc.coin_record_by_id(coin_id)?;
    let (status, confirmed_height, spent_height, confirmations) = match record {
        None => ("MISSING", None, None, 0),
        Some(record) => {
            if record.coin.coin_id() != coin_id
                || record.coin.puzzle_hash != expected_puzzle_hash
                || record.coin.amount != funding_amount_mojo
            {
                return Err("chain returned a Funding Coin with mismatched identity, puzzle hash, or amount".to_string());
            }
            let confirmations =
                if record.confirmed_height == 0 || peak_height < record.confirmed_height {
                    0
                } else {
                    peak_height - record.confirmed_height + 1
                };
            let status = if record.spent_height.is_some() {
                "SPENT"
            } else if confirmations >= required_confirmations {
                "CONFIRMED"
            } else {
                "CONFIRMING"
            };
            (
                status,
                Some(record.confirmed_height),
                record.spent_height,
                confirmations,
            )
        }
    };
    Ok(FundingStatusOutput {
        schema: "xhub.wallet.v3_6.funding_status.v1",
        network: "chia-mainnet",
        rpc_url: rpc.base_url,
        funding_coin_id: hex::encode(coin_id),
        funding_puzzle_hash: hex::encode(expected_puzzle_hash),
        funding_amount_mojo,
        peak_height,
        status,
        confirmed_height,
        spent_height,
        confirmations,
        required_confirmations,
        registration_ready: status == "CONFIRMED" && spent_height.is_none(),
    })
}

fn validate_remote_profile(profile: &RemoteProfile) -> Result<(), String> {
    if profile.network_id.to_ascii_lowercase() != MAINNET_NETWORK_ID
        || profile.funding_confirmation_blocks != FUNDING_CONFIRMATION_BLOCKS_TEST
        || !profile.hub_gateway_enabled
    {
        return Err(
            "wallet service is not the expected V3.6 Chia mainnet canary profile".to_string(),
        );
    }
    normalize_fixed_hex::<48>(&profile.hub_state_public_key_a, "hub_state_public_key_a")?;
    normalize_fixed_hex::<32>(&profile.state_rules_hash, "state_rules_hash")?;
    Ok(())
}

fn funding_terms_body(input: &FundingTermsInput) -> Value {
    json!({
        "protocol_version": "0x0360",
        "network_id": input.network_id,
        "acceptance_blocks": input.acceptance_blocks,
        "freeze_blocks": input.freeze_blocks,
        "challenge_blocks": input.challenge_blocks,
        "user_public_key": input.user_public_key,
        "hub_state_public_key_a": input.hub_state_public_key_a,
        "state_rules_hash": input.state_rules_hash,
        "funding_amount": input.funding_amount,
        "user_remainder_puzzle_hash": input.user_remainder_puzzle_hash
    })
}

fn funding_terms_body_from_channel(terms: &ChannelTerms) -> Value {
    funding_terms_body(&FundingTermsInput {
        network_id: hex::encode(terms.network_id),
        acceptance_blocks: terms.acceptance_blocks.to_string(),
        freeze_blocks: terms.freeze_blocks.to_string(),
        challenge_blocks: terms.challenge_blocks.to_string(),
        user_public_key: hex::encode(terms.user_public_key),
        hub_state_public_key_a: hex::encode(terms.hub_state_public_key_a),
        state_rules_hash: hex::encode(terms.state_rules_hash),
        funding_amount: terms.funding_amount.to_string(),
        user_remainder_puzzle_hash: hex::encode(terms.user_remainder_puzzle_hash),
    })
}

fn decode_draft_terms(draft: &FundingDraft) -> Result<ChannelTerms, String> {
    let bytes = hex::decode(&draft.preview.channel_terms_canonical_hex)
        .map_err(|error| format!("invalid channel terms canonical hex: {error}"))?;
    ChannelTerms::from_canonical_bytes(&bytes)
        .map_err(|error| format!("invalid canonical Channel Terms: {error}"))
}

fn validate_funding_draft(
    draft: &FundingDraft,
    expected_public_key: &str,
    expected_remainder_puzzle_hash: &str,
    expected_amount: u64,
    require_confirmed: bool,
) -> Result<ChannelTerms, String> {
    if require_confirmed && !draft.confirmed {
        return Err("Funding terms must be explicitly confirmed and locked".to_string());
    }
    let terms = decode_draft_terms(draft)?;
    let local_preview = xhub_wallet_v3_6::preview(&terms).map_err(|error| error.to_string())?;
    if local_preview != draft.preview
        || draft.draft_id != draft.preview.channel_terms_hash
        || hex::encode(terms.hash().map_err(|error| error.to_string())?)
            != draft.preview.channel_terms_hash
    {
        return Err(
            "Funding draft failed local canonical hash and puzzle verification".to_string(),
        );
    }
    if hex::encode(terms.network_id) != MAINNET_NETWORK_ID
        || hex::encode(terms.user_public_key)
            != normalize_fixed_hex::<48>(expected_public_key, "wallet_public_key_index0")?
        || hex::encode(terms.user_remainder_puzzle_hash)
            != normalize_fixed_hex::<32>(
                expected_remainder_puzzle_hash,
                "expected_remainder_puzzle_hash",
            )?
        || terms.funding_amount != expected_amount
    {
        return Err(
            "Funding terms do not belong to this strict index-0 wallet or requested amount"
                .to_string(),
        );
    }
    Ok(terms)
}

fn prepare_send() -> Result<(), String> {
    let request: PrepareSendRequest = read_json()?;
    print_json(&prepare_send_request(request)?)
}

fn prepare_send_request(request: PrepareSendRequest) -> Result<PreparedSend, String> {
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
    Ok(PreparedSend {
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
        funding: None,
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
    let response = rpc.call("push_tx", push_tx_payload(&prepared.spend_bundle))?;
    let status = response
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("SUCCESS")
        .to_string();
    let funding_coin_id = prepared
        .funding
        .as_ref()
        .map(|funding| funding.predicted_funding_coin_id.clone());
    print_json(&BroadcastOutput {
        schema: "xhub.wallet.v3_6.broadcast_result.v1",
        network: "chia-mainnet",
        rpc_url: rpc.base_url,
        spend_bundle_id: prepared.spend_bundle_id,
        status,
        success: true,
        chain_broadcast: true,
        submitted_at_unix: now_unix()?,
        funding_coin_id,
    })
}

fn push_tx_payload(bundle: &SpendBundle) -> Value {
    let coin_spends = bundle
        .coin_spends
        .iter()
        .map(|spend| {
            json!({
                "coin": {
                    "parent_coin_info": rpc_hex(spend.coin.parent_coin_info.as_ref()),
                    "puzzle_hash": rpc_hex(spend.coin.puzzle_hash.as_ref()),
                    "amount": spend.coin.amount,
                },
                "puzzle_reveal": rpc_hex(spend.puzzle_reveal.as_slice()),
                "solution": rpc_hex(spend.solution.as_slice()),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "spend_bundle": {
            "coin_spends": coin_spends,
            "aggregated_signature": rpc_hex(bundle.aggregated_signature.to_bytes()),
        }
    })
}

fn rpc_hex(bytes: impl AsRef<[u8]>) -> String {
    format!("0x{}", hex::encode(bytes))
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
    validate_bundle(
        &bundle,
        records,
        destination_puzzle_hash,
        amount,
        fee,
        change,
    )?;
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
    if records.is_empty()
        || records
            .iter()
            .zip(&prepared.selected_coins)
            .any(|(record, selected)| hex::encode(record.coin.coin_id()) != selected.coin_id)
        || records
            .iter()
            .any(|record| hex::encode(record.coin.puzzle_hash) != prepared.source_puzzle_hash)
        || records.iter().try_fold(0_u64, |sum, record| {
            sum.checked_add(record.coin.amount)
                .ok_or("prepared input total overflow")
        })? != prepared.input_total_mojo
        || decode_mainnet_address(&prepared.destination_address)?
            != decode_bytes32(&prepared.destination_puzzle_hash)?
    {
        return Err(
            "prepared transaction identity fields do not match its Coin inputs and outputs"
                .to_string(),
        );
    }
    if let Some(funding) = &prepared.funding {
        let terms = decode_draft_terms(&funding.draft)?;
        validate_funding_draft(
            &funding.draft,
            &hex::encode(terms.user_public_key),
            &hex::encode(terms.user_remainder_puzzle_hash),
            terms.funding_amount,
            true,
        )?;
        WalletServiceClient::new(&funding.wallet_service_url)?;
        let predicted = Coin::new(
            records[0].coin.coin_id(),
            decode_bytes32(&prepared.destination_puzzle_hash)?,
            prepared.amount_mojo,
        );
        if prepared.destination_address != funding.draft.preview.funding_address
            || prepared.destination_puzzle_hash != funding.draft.preview.funding_puzzle_hash
            || prepared.amount_mojo != terms.funding_amount
            || funding.required_confirmations != FUNDING_CONFIRMATION_BLOCKS_TEST
            || funding.predicted_funding_coin_id != hex::encode(predicted.coin_id())
        {
            return Err(
                "prepared Funding transaction does not match its locked Channel Terms".to_string(),
            );
        }
    }
    validate_bundle(
        &prepared.spend_bundle,
        &records,
        decode_bytes32(&prepared.destination_puzzle_hash)?,
        prepared.amount_mojo,
        prepared.fee_mojo,
        prepared.change_mojo,
    )
}

fn validate_bundle(
    bundle: &SpendBundle,
    records: &[CoinRecord],
    destination_puzzle_hash: Bytes32,
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
    let created = conditions
        .spends
        .iter()
        .flat_map(|spend| spend.create_coin.iter())
        .map(|coin| (coin.0, coin.1))
        .collect::<Vec<_>>();
    let source_puzzle_hash = records
        .first()
        .ok_or("SpendBundle must contain at least one input")?
        .coin
        .puzzle_hash;
    let expected_count = if change > 0 { 2 } else { 1 };
    let destination_count = created
        .iter()
        .filter(|(puzzle_hash, created_amount)| {
            *puzzle_hash == destination_puzzle_hash && *created_amount == amount
        })
        .count();
    let destination_on_first_parent = conditions
        .spends
        .iter()
        .filter(|spend| spend.coin_id.as_ref() == records[0].coin.coin_id().as_ref())
        .flat_map(|spend| spend.create_coin.iter())
        .filter(|coin| coin.0 == destination_puzzle_hash && coin.1 == amount)
        .count();
    let change_count = created
        .iter()
        .filter(|(puzzle_hash, created_amount)| {
            *puzzle_hash == source_puzzle_hash && *created_amount == change
        })
        .count();
    if created.len() != expected_count
        || destination_count != 1
        || destination_on_first_parent != 1
        || (change > 0 && change_count != 1)
    {
        return Err(
            "SpendBundle outputs do not exactly match destination and index-0 change".to_string(),
        );
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

fn default_wallet_service_url() -> String {
    DEFAULT_WALLET_SERVICE_URL.to_string()
}

fn default_funding_confirmations() -> u64 {
    FUNDING_CONFIRMATION_BLOCKS_TEST
}

fn normalize_fixed_hex<const N: usize>(value: &str, field: &str) -> Result<String, String> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    if value.len() != N * 2 || value.bytes().any(|byte| !byte.is_ascii_hexdigit()) {
        return Err(format!("{field} must encode exactly {N} bytes as hex"));
    }
    Ok(value.to_ascii_lowercase())
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
            destination_address: bech32::encode("xch", [9_u8; 32].to_base32(), Variant::Bech32m)
                .unwrap(),
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
            funding: None,
        };
        let json = serde_json::to_string(&prepared).unwrap();
        let decoded: PreparedSend = serde_json::from_str(&json).unwrap();
        validate_prepared(&decoded).unwrap();
    }

    #[test]
    fn push_tx_payload_prefixes_every_binary_field() {
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
        let payload = push_tx_payload(&bundle);
        let coin_spends = payload
            .pointer("/spend_bundle/coin_spends")
            .and_then(Value::as_array)
            .unwrap();

        assert_eq!(coin_spends.len(), 2);
        for spend in coin_spends {
            for pointer in [
                "/coin/parent_coin_info",
                "/coin/puzzle_hash",
                "/puzzle_reveal",
                "/solution",
            ] {
                assert_rpc_hex(spend.pointer(pointer).unwrap());
            }
        }
        assert_rpc_hex(
            payload
                .pointer("/spend_bundle/aggregated_signature")
                .unwrap(),
        );
    }

    fn assert_rpc_hex(value: &Value) {
        let value = value.as_str().expect("RPC byte field must be hex text");
        let encoded = value
            .strip_prefix("0x")
            .expect("RPC byte field must start with 0x");
        assert!(!encoded.is_empty());
        hex::decode(encoded).expect("RPC byte field must contain valid hex");
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
    fn funding_preview_is_bound_to_locked_terms_and_predicted_coin_id() {
        let wallet = SecretKey::from_seed(&[7; 32]);
        let hub = SecretKey::from_seed(&[8; 32]);
        let synthetic = wallet.derive_synthetic();
        let source: Bytes32 =
            chia_puzzle_types::standard::StandardArgs::curry_tree_hash(synthetic.public_key())
                .into();
        let modules = xhub_puzzles_v3_6::module_hashes();
        let network_id: [u8; 32] = hex::decode(MAINNET_NETWORK_ID).unwrap().try_into().unwrap();
        let terms = ChannelTerms::new(
            network_id,
            12_288,
            200,
            6_000,
            wallet.public_key().to_bytes(),
            hub.public_key().to_bytes(),
            xhub_protocol_v3_6::state_rules_hash(
                &modules.initial_closing,
                &modules.subsequent_closing,
                &modules.merchant_payment,
            ),
            7,
            source.into(),
        )
        .unwrap();
        let preview = xhub_wallet_v3_6::preview(&terms).unwrap();
        let draft = FundingDraft {
            draft_id: preview.channel_terms_hash.clone(),
            confirmed: true,
            preview,
        };
        validate_funding_draft(
            &draft,
            &hex::encode(wallet.public_key().to_bytes()),
            &hex::encode(source),
            7,
            true,
        )
        .unwrap();

        let records = vec![record(Coin::new(Bytes32::new([1; 32]), source, 12))];
        let destination = decode_bytes32(&draft.preview.funding_puzzle_hash).unwrap();
        let bundle = build_standard_bundle(&records, &synthetic, destination, 7, 1, 4).unwrap();
        let predicted = Coin::new(records[0].coin.coin_id(), destination, 7);
        let selected = vec![SelectedCoin {
            coin_id: hex::encode(records[0].coin.coin_id()),
            parent_coin_info: hex::encode(records[0].coin.parent_coin_info),
            puzzle_hash: hex::encode(source),
            amount_mojo: 12,
            confirmed_height: 100,
        }];
        let mut prepared = PreparedSend {
            schema: "xhub.wallet.v3_6.prepared_send.v1".to_string(),
            protocol_version: "3.6".to_string(),
            network: "chia-mainnet".to_string(),
            rpc_url: DEFAULT_RPC_URL.to_string(),
            source_puzzle_hash: hex::encode(source),
            destination_address: draft.preview.funding_address.clone(),
            destination_puzzle_hash: draft.preview.funding_puzzle_hash.clone(),
            amount_mojo: 7,
            fee_mojo: 1,
            input_total_mojo: 12,
            change_mojo: 4,
            purpose: "Funding test".to_string(),
            selected_coins: selected,
            spend_bundle_id: hex::encode(bundle.name()),
            spend_bundle: bundle,
            consensus_conditions_verified: true,
            aggregate_signature_verified: true,
            broadcast_performed: false,
            funding: Some(FundingMetadata {
                wallet_service_url: DEFAULT_WALLET_SERVICE_URL.to_string(),
                draft,
                predicted_funding_coin_id: hex::encode(predicted.coin_id()),
                required_confirmations: FUNDING_CONFIRMATION_BLOCKS_TEST,
            }),
        };
        validate_prepared(&prepared).unwrap();
        prepared.funding.as_mut().unwrap().predicted_funding_coin_id = "00".repeat(32);
        assert!(validate_prepared(&prepared).is_err());
    }

    #[test]
    fn rejects_non_https_rpc_and_non_mainnet_address() {
        assert!(RpcClient::new("http://api.coinset.org").is_err());
        assert!(decode_mainnet_address("txch1invalid").is_err());
    }
}
