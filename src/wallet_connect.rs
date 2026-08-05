use std::{path::Path, time::{SystemTime, UNIX_EPOCH}};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chia_bls::{PublicKey, SecretKey, Signature, sign, verify};
use chia_protocol::Bytes32;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use thiserror::Error;
use uuid::Uuid;

use crate::hash_parts;

const URI_DOMAIN: &[u8] = b"XHUB_CONNECT_URI_V1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalletConnectState {
    Created, Paired, FundingAuthorized, WalletConfirming, Broadcast,
    PendingConfirmation, Active, Cancelled, Expired, Rejected, Failed, Reorged,
}

impl WalletConnectState {
    pub fn as_str(self) -> &'static str { match self {
        Self::Created => "CREATED", Self::Paired => "PAIRED", Self::FundingAuthorized => "FUNDING_AUTHORIZED",
        Self::WalletConfirming => "WALLET_CONFIRMING", Self::Broadcast => "BROADCAST",
        Self::PendingConfirmation => "PENDING_CONFIRMATION", Self::Active => "ACTIVE",
        Self::Cancelled => "CANCELLED", Self::Expired => "EXPIRED", Self::Rejected => "REJECTED",
        Self::Failed => "FAILED", Self::Reorged => "REORGED",
    }}
    fn parse(value: &str) -> Result<Self, WalletConnectError> { match value {
        "CREATED" => Ok(Self::Created), "PAIRED" => Ok(Self::Paired), "FUNDING_AUTHORIZED" => Ok(Self::FundingAuthorized),
        "WALLET_CONFIRMING" => Ok(Self::WalletConfirming), "BROADCAST" => Ok(Self::Broadcast),
        "PENDING_CONFIRMATION" => Ok(Self::PendingConfirmation), "ACTIVE" => Ok(Self::Active), "CANCELLED" => Ok(Self::Cancelled),
        "EXPIRED" => Ok(Self::Expired), "REJECTED" => Ok(Self::Rejected), "FAILED" => Ok(Self::Failed), "REORGED" => Ok(Self::Reorged),
        _ => Err(WalletConnectError::Corrupt("state")),
    }}
    fn terminal(self) -> bool { matches!(self, Self::Active | Self::Cancelled | Self::Expired | Self::Rejected | Self::Failed) }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletConnectRequest {
    pub request_id: String,
    pub amount_mojos: u64,
    pub refund_delay_blocks: u64,
    pub network: String,
    pub state: WalletConnectState,
    pub expires_at: u64,
    pub session_id: Option<String>,
    pub user_public_key: Option<[u8; 48]>,
    pub user_puzzle_hash: Option<Bytes32>,
    pub funding_puzzle_hash: Option<Bytes32>,
    pub funding_coin_id: Option<Bytes32>,
    pub transaction_id: Option<Bytes32>,
}

#[derive(Debug, Error)]
pub enum WalletConnectError {
    #[error(transparent)] Database(#[from] rusqlite::Error),
    #[error("wallet connect request not found")] NotFound,
    #[error("wallet connect request is expired")] Expired,
    #[error("wallet connect request has already been consumed")] Consumed,
    #[error("illegal wallet connect transition from {from:?} to {to:?}")] Transition { from: WalletConnectState, to: WalletConnectState },
    #[error("wallet connect request conflicts with existing session")] SessionConflict,
    #[error("invalid persisted wallet-connect data: {0}")] Corrupt(&'static str),
    #[error("invalid wallet connect input: {0}")] Invalid(&'static str),
}

pub struct WalletConnectStore { connection: Connection }

impl WalletConnectStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WalletConnectError> { Self::initialize(Connection::open(path)?) }
    pub fn open_in_memory() -> Result<Self, WalletConnectError> { Self::initialize(Connection::open_in_memory()?) }
    fn initialize(connection: Connection) -> Result<Self, WalletConnectError> {
        connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL;
            CREATE TABLE IF NOT EXISTS wallet_connect_requests (
                request_id TEXT PRIMARY KEY, amount_mojos INTEGER NOT NULL, refund_delay_blocks INTEGER NOT NULL,
                network TEXT NOT NULL, state TEXT NOT NULL, expires_at INTEGER NOT NULL, session_id TEXT UNIQUE,
                user_public_key BLOB CHECK(user_public_key IS NULL OR length(user_public_key) = 48),
                user_puzzle_hash BLOB CHECK(user_puzzle_hash IS NULL OR length(user_puzzle_hash) = 32),
                funding_puzzle_hash BLOB CHECK(funding_puzzle_hash IS NULL OR length(funding_puzzle_hash) = 32),
                funding_coin_id BLOB CHECK(funding_coin_id IS NULL OR length(funding_coin_id) = 32),
                transaction_id BLOB CHECK(transaction_id IS NULL OR length(transaction_id) = 32),
                created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS wallet_connect_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT, request_id TEXT NOT NULL, from_state TEXT, to_state TEXT NOT NULL,
                details_json TEXT NOT NULL, created_at INTEGER NOT NULL,
                FOREIGN KEY(request_id) REFERENCES wallet_connect_requests(request_id)
            );")?;
        let columns = connection.prepare("PRAGMA table_info(wallet_connect_requests)")?.query_map([], |row| row.get::<_, String>(1))?.collect::<Result<Vec<_>, _>>()?;
        if !columns.iter().any(|column| column == "transaction_id") { connection.execute("ALTER TABLE wallet_connect_requests ADD COLUMN transaction_id BLOB", [])?; }
        Ok(Self { connection })
    }

    pub fn create(&mut self, amount_mojos: u64, refund_delay_blocks: u64, network: &str, expires_at: u64) -> Result<WalletConnectRequest, WalletConnectError> {
        if amount_mojos == 0 || amount_mojos > i64::MAX as u64 { return Err(WalletConnectError::Invalid("amount_mojos")); }
        if refund_delay_blocks == 0 || refund_delay_blocks > i64::MAX as u64 { return Err(WalletConnectError::Invalid("refund_delay_blocks")); }
        if network.is_empty() || expires_at <= now_unix() { return Err(WalletConnectError::Invalid("expiry or network")); }
        let request_id = Uuid::new_v4().to_string(); let now = now_unix();
        self.connection.execute("INSERT INTO wallet_connect_requests (request_id, amount_mojos, refund_delay_blocks, network, state, expires_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'CREATED', ?5, ?6, ?6)", params![request_id, amount_mojos as i64, refund_delay_blocks as i64, network, expires_at as i64, now as i64])?;
        self.event(&request_id, None, WalletConnectState::Created, serde_json::json!({}))?;
        self.load(&request_id)?.ok_or(WalletConnectError::Corrupt("created request"))
    }

    pub fn load(&self, request_id: &str) -> Result<Option<WalletConnectRequest>, WalletConnectError> {
        self.connection.query_row("SELECT amount_mojos, refund_delay_blocks, network, state, expires_at, session_id, user_public_key, user_puzzle_hash, funding_puzzle_hash, funding_coin_id, transaction_id FROM wallet_connect_requests WHERE request_id = ?1", [request_id], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, i64>(4)?, r.get::<_, Option<String>>(5)?, r.get::<_, Option<Vec<u8>>>(6)?, r.get::<_, Option<Vec<u8>>>(7)?, r.get::<_, Option<Vec<u8>>>(8)?, r.get::<_, Option<Vec<u8>>>(9)?, r.get::<_, Option<Vec<u8>>>(10)?))
        }).optional()?.map(|(amount, delay, network, state, expires, session, user_pk, user_ph, funding_ph, funding_coin, transaction_id)| Ok(WalletConnectRequest {
            request_id: request_id.to_string(), amount_mojos: u64::try_from(amount).map_err(|_| WalletConnectError::Corrupt("amount"))?, refund_delay_blocks: u64::try_from(delay).map_err(|_| WalletConnectError::Corrupt("delay"))?, network, state: WalletConnectState::parse(&state)?, expires_at: u64::try_from(expires).map_err(|_| WalletConnectError::Corrupt("expires"))?, session_id: session,
            user_public_key: user_pk.map(|v| bytes48(v, "user_public_key")).transpose()?,
            user_puzzle_hash: user_ph.map(|v| bytes32(v, "user_puzzle_hash")).transpose()?, funding_puzzle_hash: funding_ph.map(|v| bytes32(v, "funding_puzzle_hash")).transpose()?, funding_coin_id: funding_coin.map(|v| bytes32(v, "funding_coin_id")).transpose()?, transaction_id: transaction_id.map(|v| bytes32(v, "transaction_id")).transpose()?,
        })).transpose()
    }

    pub fn pair(&mut self, request_id: &str, session_id: &str, user_public_key: [u8; 48], user_puzzle_hash: Bytes32) -> Result<WalletConnectRequest, WalletConnectError> {
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = request_state(&tx, request_id)?; let expires_at = request_expiry(&tx, request_id)?;
        if expires_at <= now_unix() { tx.execute("UPDATE wallet_connect_requests SET state = 'EXPIRED', updated_at = ?2 WHERE request_id = ?1", params![request_id, now_unix() as i64])?; tx.commit()?; return Err(WalletConnectError::Expired); }
        if current == WalletConnectState::Paired {
            let existing: Option<String> = tx.query_row("SELECT session_id FROM wallet_connect_requests WHERE request_id = ?1", [request_id], |r| r.get(0)).optional()?;
            if existing.as_deref() == Some(session_id) { tx.commit()?; return self.load(request_id)?.ok_or(WalletConnectError::NotFound); }
            return Err(WalletConnectError::SessionConflict);
        }
        if current != WalletConnectState::Created { return Err(if current.terminal() { WalletConnectError::Consumed } else { WalletConnectError::Transition { from: current, to: WalletConnectState::Paired } }); }
        tx.execute("UPDATE wallet_connect_requests SET state = 'PAIRED', session_id = ?2, user_public_key = ?3, user_puzzle_hash = ?4, updated_at = ?5 WHERE request_id = ?1", params![request_id, session_id, user_public_key, user_puzzle_hash.as_ref(), now_unix() as i64])?;
        tx.execute("INSERT INTO wallet_connect_events (request_id, from_state, to_state, details_json, created_at) VALUES (?1, ?2, 'PAIRED', ?3, ?4)", params![request_id, current.as_str(), serde_json::json!({"session_id": session_id}).to_string(), now_unix() as i64])?;
        tx.commit()?; self.load(request_id)?.ok_or(WalletConnectError::NotFound)
    }

    pub fn transition(&mut self, request_id: &str, target: WalletConnectState) -> Result<WalletConnectRequest, WalletConnectError> {
        let current = self.load(request_id)?.ok_or(WalletConnectError::NotFound)?;
        if current.expires_at <= now_unix() && !current.state.terminal() { return self.expire(request_id); }
        if !allowed(current.state, target) { return Err(WalletConnectError::Transition { from: current.state, to: target }); }
        self.connection.execute("UPDATE wallet_connect_requests SET state = ?2, updated_at = ?3 WHERE request_id = ?1", params![request_id, target.as_str(), now_unix() as i64])?;
        self.event(request_id, Some(current.state), target, serde_json::json!({}))?;
        self.load(request_id)?.ok_or(WalletConnectError::NotFound)
    }

    pub fn authorize_funding(&mut self, request_id: &str, funding_puzzle_hash: Bytes32) -> Result<WalletConnectRequest, WalletConnectError> {
        let current = self.load(request_id)?.ok_or(WalletConnectError::NotFound)?;
        if current.expires_at <= now_unix() { return self.expire(request_id); }
        if current.state == WalletConnectState::FundingAuthorized {
            if current.funding_puzzle_hash == Some(funding_puzzle_hash) { return Ok(current); }
            return Err(WalletConnectError::SessionConflict);
        }
        if current.state != WalletConnectState::Paired { return Err(WalletConnectError::Transition { from: current.state, to: WalletConnectState::FundingAuthorized }); }
        self.connection.execute("UPDATE wallet_connect_requests SET state = 'FUNDING_AUTHORIZED', funding_puzzle_hash = ?2, updated_at = ?3 WHERE request_id = ?1", params![request_id, funding_puzzle_hash.as_ref(), now_unix() as i64])?;
        self.event(request_id, Some(WalletConnectState::Paired), WalletConnectState::FundingAuthorized, serde_json::json!({"funding_puzzle_hash": hex::encode(funding_puzzle_hash)}))?;
        self.load(request_id)?.ok_or(WalletConnectError::NotFound)
    }

    pub fn record_broadcast(&mut self, request_id: &str, transaction_id: Bytes32) -> Result<WalletConnectRequest, WalletConnectError> {
        let current = self.load(request_id)?.ok_or(WalletConnectError::NotFound)?;
        if !matches!(current.state, WalletConnectState::FundingAuthorized | WalletConnectState::WalletConfirming | WalletConnectState::Broadcast | WalletConnectState::PendingConfirmation) { return Err(WalletConnectError::Transition { from: current.state, to: WalletConnectState::Broadcast }); }
        self.connection.execute("UPDATE wallet_connect_requests SET state = 'BROADCAST', transaction_id = ?2, updated_at = ?3 WHERE request_id = ?1", params![request_id, transaction_id.as_ref(), now_unix() as i64])?;
        self.event(request_id, Some(current.state), WalletConnectState::Broadcast, serde_json::json!({"transaction_id": hex::encode(transaction_id)}))?;
        self.load(request_id)?.ok_or(WalletConnectError::NotFound)
    }

    pub fn observe_funding(&mut self, request_id: &str, funding_coin_id: Bytes32, active: bool, reorged: bool) -> Result<WalletConnectRequest, WalletConnectError> {
        let current = self.load(request_id)?.ok_or(WalletConnectError::NotFound)?;
        let target = if reorged { WalletConnectState::Reorged } else if active { WalletConnectState::Active } else { WalletConnectState::PendingConfirmation };
        if !matches!(current.state, WalletConnectState::Broadcast | WalletConnectState::PendingConfirmation | WalletConnectState::Active | WalletConnectState::Reorged) { return Err(WalletConnectError::Transition { from: current.state, to: target }); }
        self.connection.execute("UPDATE wallet_connect_requests SET state = ?2, funding_coin_id = ?3, updated_at = ?4 WHERE request_id = ?1", params![request_id, target.as_str(), funding_coin_id.as_ref(), now_unix() as i64])?;
        self.event(request_id, Some(current.state), target, serde_json::json!({"funding_coin_id": hex::encode(funding_coin_id)}))?;
        self.load(request_id)?.ok_or(WalletConnectError::NotFound)
    }

    fn expire(&mut self, request_id: &str) -> Result<WalletConnectRequest, WalletConnectError> { self.connection.execute("UPDATE wallet_connect_requests SET state = 'EXPIRED', updated_at = ?2 WHERE request_id = ?1", params![request_id, now_unix() as i64])?; self.event(request_id, None, WalletConnectState::Expired, serde_json::json!({}))?; self.load(request_id)?.ok_or(WalletConnectError::NotFound) }
    fn event(&self, request_id: &str, from: Option<WalletConnectState>, to: WalletConnectState, details: serde_json::Value) -> Result<(), WalletConnectError> { self.connection.execute("INSERT INTO wallet_connect_events (request_id, from_state, to_state, details_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5)", params![request_id, from.map(WalletConnectState::as_str), to.as_str(), details.to_string(), now_unix() as i64])?; Ok(()) }
}

pub fn canonical_uri_signature_hash(request_uri: &str, request_id: &str, expires_at: u64, hub_key_id: &str) -> Result<Bytes32, WalletConnectError> {
    let request_id = Uuid::parse_str(request_id).map_err(|_| WalletConnectError::Invalid("request_id"))?;
    let parts = vec![URI_DOMAIN.to_vec(), 1u16.to_be_bytes().to_vec(), encoded_text(request_uri)?, request_id.as_bytes().to_vec(), expires_at.to_be_bytes().to_vec(), encoded_text(hub_key_id)?];
    Ok(hash_parts(&parts.iter().map(Vec::as_slice).collect::<Vec<_>>()))
}
pub fn sign_connect_uri(key: &SecretKey, request_uri: &str, request_id: &str, expires_at: u64, hub_key_id: &str) -> Result<String, WalletConnectError> { Ok(URL_SAFE_NO_PAD.encode(sign(key, canonical_uri_signature_hash(request_uri, request_id, expires_at, hub_key_id)?.as_ref()).to_bytes())) }
pub fn verify_connect_uri(key: &PublicKey, request_uri: &str, request_id: &str, expires_at: u64, hub_key_id: &str, sig: &str) -> Result<(), WalletConnectError> { let raw: [u8; 96] = URL_SAFE_NO_PAD.decode(sig).map_err(|_| WalletConnectError::Invalid("signature"))?.try_into().map_err(|_| WalletConnectError::Invalid("signature"))?; let signature = Signature::from_bytes(&raw).map_err(|_| WalletConnectError::Invalid("signature"))?; if verify(&signature, key, canonical_uri_signature_hash(request_uri, request_id, expires_at, hub_key_id)?.as_ref()) { Ok(()) } else { Err(WalletConnectError::Invalid("signature")) } }

fn encoded_text(value: &str) -> Result<Vec<u8>, WalletConnectError> { let len = u32::try_from(value.len()).map_err(|_| WalletConnectError::Invalid("text length"))?; let mut out = len.to_be_bytes().to_vec(); out.extend_from_slice(value.as_bytes()); Ok(out) }
fn bytes32(value: Vec<u8>, field: &'static str) -> Result<Bytes32, WalletConnectError> { let value: [u8; 32] = value.try_into().map_err(|_| WalletConnectError::Corrupt(field))?; Ok(Bytes32::from(value)) }
fn bytes48(value: Vec<u8>, field: &'static str) -> Result<[u8; 48], WalletConnectError> { value.try_into().map_err(|_| WalletConnectError::Corrupt(field)) }
fn request_state(tx: &rusqlite::Transaction<'_>, request_id: &str) -> Result<WalletConnectState, WalletConnectError> { let state: String = tx.query_row("SELECT state FROM wallet_connect_requests WHERE request_id = ?1", [request_id], |r| r.get(0)).optional()?.ok_or(WalletConnectError::NotFound)?; WalletConnectState::parse(&state) }
fn request_expiry(tx: &rusqlite::Transaction<'_>, request_id: &str) -> Result<u64, WalletConnectError> { let value: i64 = tx.query_row("SELECT expires_at FROM wallet_connect_requests WHERE request_id = ?1", [request_id], |r| r.get(0))?; u64::try_from(value).map_err(|_| WalletConnectError::Corrupt("expires")) }
fn allowed(from: WalletConnectState, to: WalletConnectState) -> bool { matches!((from, to), (WalletConnectState::Created, WalletConnectState::Paired) | (WalletConnectState::Paired, WalletConnectState::FundingAuthorized) | (WalletConnectState::FundingAuthorized, WalletConnectState::WalletConfirming) | (WalletConnectState::WalletConfirming, WalletConnectState::Broadcast) | (WalletConnectState::Broadcast, WalletConnectState::PendingConfirmation) | (WalletConnectState::PendingConfirmation, WalletConnectState::Active) | (WalletConnectState::PendingConfirmation, WalletConnectState::Reorged) | (WalletConnectState::Active, WalletConnectState::Reorged) | (WalletConnectState::Reorged, WalletConnectState::PendingConfirmation) | (_, WalletConnectState::Cancelled | WalletConnectState::Expired | WalletConnectState::Rejected | WalletConnectState::Failed)) }
fn now_unix() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }

#[cfg(test)] mod tests { use super::*;
    #[test] fn request_is_single_session_and_transitions_in_order() { let mut store = WalletConnectStore::open_in_memory().unwrap(); let request = store.create(10, 20, "testnet11", now_unix() + 60).unwrap(); let paired = store.pair(&request.request_id, "session-a", [7;48], Bytes32::from([8;32])).unwrap(); assert_eq!(paired.state, WalletConnectState::Paired); assert_eq!(store.pair(&request.request_id, "session-a", [9;48], Bytes32::from([1;32])).unwrap().state, WalletConnectState::Paired); assert!(matches!(store.pair(&request.request_id, "session-b", [7;48], Bytes32::from([8;32])), Err(WalletConnectError::SessionConflict))); assert_eq!(store.transition(&request.request_id, WalletConnectState::FundingAuthorized).unwrap().state, WalletConnectState::FundingAuthorized); assert!(store.transition(&request.request_id, WalletConnectState::Active).is_err()); }
    #[test] fn uri_signature_is_canonical_and_verifiable() { let key = SecretKey::from_seed(&[3; 32]); let id = "4c55c3fd-8470-44a6-a98e-53bd0e11aa38"; let sig = sign_connect_uri(&key, "https://api.xhub.example/v1/wallet-connect/requests/4c55c3fd-8470-44a6-a98e-53bd0e11aa38", id, 2_000_000_000, "xhub-main-2026").unwrap(); verify_connect_uri(&key.public_key(), "https://api.xhub.example/v1/wallet-connect/requests/4c55c3fd-8470-44a6-a98e-53bd0e11aa38", id, 2_000_000_000, "xhub-main-2026", &sig).unwrap(); assert!(verify_connect_uri(&key.public_key(), "https://api.xhub.example/v1/wallet-connect/requests/4c55c3fd-8470-44a6-a98e-53bd0e11aa38", id, 2_000_000_001, "xhub-main-2026", &sig).is_err()); }
}
