use std::{path::Path, time::Duration};

use chia_protocol::{Bytes32, Coin};
use chia_traits::Streamable;
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use thiserror::Error;

use crate::{
    ChannelArgs, FUNDING_AMOUNT, HubSigner, MerchantInvoice, PaymentIntent, PaymentVoucher,
    ProtocolError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum ChannelState {
    Funded = 1,
    IntentSigned = 2,
    VoucherIssued = 3,
    ClaimSubmitted = 4,
    Settled = 5,
    Refundable = 6,
    RefundSubmitted = 7,
    Refunded = 8,
}

impl ChannelState {
    fn from_i64(value: i64) -> Result<Self, StateStoreError> {
        match value {
            1 => Ok(Self::Funded),
            2 => Ok(Self::IntentSigned),
            3 => Ok(Self::VoucherIssued),
            4 => Ok(Self::ClaimSubmitted),
            5 => Ok(Self::Settled),
            6 => Ok(Self::Refundable),
            7 => Ok(Self::RefundSubmitted),
            8 => Ok(Self::Refunded),
            _ => Err(StateStoreError::CorruptData("channel_state")),
        }
    }
}

#[derive(Debug, Error)]
pub enum StateStoreError {
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error("channel already exists")]
    ChannelAlreadyExists,
    #[error("channel not found")]
    ChannelNotFound,
    #[error("duplicate order id")]
    DuplicateOrder,
    #[error("duplicate nonce")]
    DuplicateNonce,
    #[error("voucher does not match the persisted intent")]
    VoucherMismatch,
    #[error("illegal state transition from {from:?} to {to:?}")]
    IllegalStateTransition {
        from: ChannelState,
        to: ChannelState,
    },
    #[error("corrupt persisted data: {0}")]
    CorruptData(&'static str),
    #[error("idempotency key already has a different operation")]
    IdempotencyConflict,
    #[error("serialized spend bundle could not be stored")]
    SpendBundleEncoding,
}

impl StateStoreError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Database(_) => "DATABASE_ERROR",
            Self::Protocol(error) => error.code(),
            Self::ChannelAlreadyExists => "CHANNEL_ALREADY_EXISTS",
            Self::ChannelNotFound => "CHANNEL_NOT_FOUND",
            Self::DuplicateOrder => "DUPLICATE_ORDER",
            Self::DuplicateNonce => "DUPLICATE_NONCE",
            Self::VoucherMismatch => "VOUCHER_MISMATCH",
            Self::IllegalStateTransition { .. } => "ILLEGAL_STATE_TRANSITION",
            Self::CorruptData(_) => "CORRUPT_DATA",
            Self::IdempotencyConflict => "IDEMPOTENCY_CONFLICT",
            Self::SpendBundleEncoding => "SPEND_BUNDLE_ENCODING",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BroadcastKind {
    Claim,
    Refund,
}

impl BroadcastKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Claim => "CLAIM",
            Self::Refund => "REFUND",
        }
    }

    fn from_str(value: &str) -> Result<Self, StateStoreError> {
        match value {
            "CLAIM" => Ok(Self::Claim),
            "REFUND" => Ok(Self::Refund),
            _ => Err(StateStoreError::CorruptData("broadcast_kind")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BroadcastState {
    Prepared,
    Submitted,
    Pending,
    Rejected,
    Confirmed,
    Reorged,
    Expired,
}

impl BroadcastState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "PREPARED",
            Self::Submitted => "SUBMITTED",
            Self::Pending => "PENDING",
            Self::Rejected => "REJECTED",
            Self::Confirmed => "CONFIRMED",
            Self::Reorged => "REORGED",
            Self::Expired => "EXPIRED",
        }
    }

    fn from_str(value: &str) -> Result<Self, StateStoreError> {
        match value {
            "PREPARED" => Ok(Self::Prepared),
            "SUBMITTED" => Ok(Self::Submitted),
            "PENDING" => Ok(Self::Pending),
            "REJECTED" => Ok(Self::Rejected),
            "CONFIRMED" => Ok(Self::Confirmed),
            "REORGED" => Ok(Self::Reorged),
            "EXPIRED" => Ok(Self::Expired),
            _ => Err(StateStoreError::CorruptData("broadcast_state")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BroadcastJob {
    pub idempotency_key: String,
    pub channel_id: Bytes32,
    pub kind: BroadcastKind,
    pub spend_bundle_id: Bytes32,
    pub spend_bundle: Vec<u8>,
    pub funding_coin_id: Bytes32,
    pub fee: Option<u64>,
    pub fee_coin_id: Option<Bytes32>,
    pub state: BroadcastState,
    pub attempts: u32,
    pub last_error: Option<String>,
}

pub struct BroadcastRequest<'a> {
    pub idempotency_key: &'a str,
    pub channel_id: Bytes32,
    pub kind: BroadcastKind,
    pub bundle: &'a chia_protocol::SpendBundle,
    pub funding_coin_id: Bytes32,
    pub fee: Option<u64>,
    pub fee_coin_id: Option<Bytes32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelRecord {
    pub channel_id: Bytes32,
    pub state: ChannelState,
    pub order_id: Option<Bytes32>,
    pub nonce: Option<Bytes32>,
    pub intent: Option<PaymentIntent>,
    pub voucher: Option<PaymentVoucher>,
    pub merchant_amount: u64,
    pub user_remaining_amount: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredChainObservation {
    pub channel_id: Bytes32,
    pub tx_id: Bytes32,
    pub funding_coin_id: Bytes32,
    pub observed_height: u32,
    pub peak_hash: Bytes32,
    pub funding_coin_json: Option<String>,
    pub confirmed_height: Option<u32>,
    pub fee: Option<u64>,
    pub mempool_status: String,
    pub children: Vec<Coin>,
    pub reorged: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct StoreMetrics {
    pub channels: u64,
    pub broadcast_jobs: u64,
    pub recoverable_broadcasts: u64,
    pub broadcast_attempts: u64,
    pub confirmed_broadcasts: u64,
    pub reorg_observations: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAuditEvent {
    pub id: u64,
    pub channel_id: Option<Bytes32>,
    pub event_type: String,
    pub idempotency_key: Option<String>,
    pub details_json: String,
}

pub struct ChannelStore {
    connection: Connection,
}

impl ChannelStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StateStoreError> {
        let connection = Connection::open(path)?;
        Self::initialize(connection)
    }

    pub fn open_in_memory() -> Result<Self, StateStoreError> {
        let connection = Connection::open_in_memory()?;
        Self::initialize(connection)
    }

    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self, StateStoreError> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(Self { connection })
    }

    fn initialize(connection: Connection) -> Result<Self, StateStoreError> {
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS channels (
                 channel_id BLOB PRIMARY KEY CHECK(length(channel_id) = 32),
                 state INTEGER NOT NULL,
                 order_id BLOB CHECK(order_id IS NULL OR length(order_id) = 32),
                 nonce BLOB CHECK(nonce IS NULL OR length(nonce) = 32),
                 intent BLOB,
                 voucher BLOB,
                 merchant_amount INTEGER NOT NULL,
                 user_remaining_amount INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS orders (
                 channel_id BLOB NOT NULL,
                 order_id BLOB NOT NULL CHECK(length(order_id) = 32),
                 PRIMARY KEY(channel_id, order_id),
                 FOREIGN KEY(channel_id) REFERENCES channels(channel_id)
             );
             CREATE TABLE IF NOT EXISTS nonces (
                 channel_id BLOB NOT NULL,
                 nonce BLOB NOT NULL CHECK(length(nonce) = 32),
                 PRIMARY KEY(channel_id, nonce),
                 FOREIGN KEY(channel_id) REFERENCES channels(channel_id)
             );
             CREATE TABLE IF NOT EXISTS chain_observations (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 channel_id BLOB NOT NULL CHECK(length(channel_id) = 32),
                 tx_id BLOB NOT NULL CHECK(length(tx_id) = 32),
                 funding_coin_id BLOB NOT NULL CHECK(length(funding_coin_id) = 32),
                 observed_height INTEGER NOT NULL,
                 peak_hash BLOB NOT NULL CHECK(length(peak_hash) = 32),
                 funding_coin_json TEXT,
                 confirmed_height INTEGER,
                 fee INTEGER,
                 mempool_status TEXT NOT NULL,
                 children_json TEXT NOT NULL,
                 reorged INTEGER NOT NULL DEFAULT 0,
                 observed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                 FOREIGN KEY(channel_id) REFERENCES channels(channel_id)
             );
             CREATE TABLE IF NOT EXISTS broadcast_jobs (
                 idempotency_key TEXT PRIMARY KEY,
                 channel_id BLOB NOT NULL CHECK(length(channel_id) = 32),
                 kind TEXT NOT NULL,
                 spend_bundle_id BLOB NOT NULL CHECK(length(spend_bundle_id) = 32),
                 spend_bundle BLOB NOT NULL,
                 funding_coin_id BLOB NOT NULL CHECK(length(funding_coin_id) = 32),
                 fee INTEGER,
                 fee_coin_id BLOB CHECK(fee_coin_id IS NULL OR length(fee_coin_id) = 32),
                 state TEXT NOT NULL,
                 attempts INTEGER NOT NULL DEFAULT 0,
                 last_error TEXT,
                 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                 updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                 UNIQUE(spend_bundle_id),
                 FOREIGN KEY(channel_id) REFERENCES channels(channel_id)
             );
             CREATE TABLE IF NOT EXISTS broadcast_attempts (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 idempotency_key TEXT NOT NULL,
                 attempt_no INTEGER NOT NULL,
                 spend_bundle_id BLOB NOT NULL CHECK(length(spend_bundle_id) = 32),
                 state TEXT NOT NULL,
                 error TEXT,
                 observed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                 FOREIGN KEY(idempotency_key) REFERENCES broadcast_jobs(idempotency_key)
             );
             CREATE TABLE IF NOT EXISTS audit_events (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 channel_id BLOB CHECK(channel_id IS NULL OR length(channel_id) = 32),
                 event_type TEXT NOT NULL,
                 idempotency_key TEXT,
                 details_json TEXT NOT NULL,
                 created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
             );",
        )?;
        let has_funding_coin_json = connection
            .prepare("PRAGMA table_info(chain_observations)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .any(|name| name == "funding_coin_json");
        if !has_funding_coin_json {
            connection.execute(
                "ALTER TABLE chain_observations ADD COLUMN funding_coin_json TEXT",
                [],
            )?;
        }
        Ok(Self { connection })
    }

    pub fn create_channel(&mut self, channel_id: Bytes32) -> Result<(), StateStoreError> {
        self.insert_channel(channel_id, FUNDING_AMOUNT)
    }

    pub fn create_channel_with_funding_amount(
        &mut self,
        channel_id: Bytes32,
        funding_amount: u64,
    ) -> Result<(), StateStoreError> {
        self.insert_channel(channel_id, funding_amount)
    }

    fn insert_channel(
        &mut self,
        channel_id: Bytes32,
        funding_amount: u64,
    ) -> Result<(), StateStoreError> {
        if self.channel_exists(channel_id)? {
            return Err(StateStoreError::ChannelAlreadyExists);
        }
        self.connection.execute(
            "INSERT INTO channels (
                 channel_id, state, merchant_amount, user_remaining_amount
             ) VALUES (?1, ?2, 0, ?3)",
            params![
                channel_id.as_ref(),
                ChannelState::Funded as i64,
                u64_to_sql(funding_amount)?
            ],
        )?;
        Ok(())
    }

    pub fn prepare_broadcast(
        &mut self,
        request: &BroadcastRequest<'_>,
    ) -> Result<BroadcastJob, StateStoreError> {
        let spend_bundle = request
            .bundle
            .to_bytes()
            .map_err(|_| StateStoreError::SpendBundleEncoding)?;
        let spend_bundle_id = request.bundle.name();
        let existing = self.load_broadcast(request.idempotency_key)?;
        if let Some(existing) = existing {
            if existing.channel_id != request.channel_id || existing.kind != request.kind {
                return Err(StateStoreError::IdempotencyConflict);
            }
            if existing.funding_coin_id != request.funding_coin_id {
                return Err(StateStoreError::IdempotencyConflict);
            }
            if existing.spend_bundle_id == spend_bundle_id {
                return Ok(existing);
            }
            if matches!(
                existing.state,
                BroadcastState::Confirmed | BroadcastState::Expired
            ) {
                return Err(StateStoreError::IdempotencyConflict);
            }
            self.connection.execute(
                "UPDATE broadcast_jobs SET spend_bundle_id = ?2, spend_bundle = ?3,
                    fee = ?4, fee_coin_id = ?5, state = ?6, updated_at = CURRENT_TIMESTAMP
                 WHERE idempotency_key = ?1",
                params![
                    request.idempotency_key,
                    spend_bundle_id.as_ref(),
                    spend_bundle,
                    request.fee.map(u64_to_sql).transpose()?,
                    request.fee_coin_id.as_ref().map(|id| id.as_ref()),
                    BroadcastState::Prepared.as_str(),
                ],
            )?;
        } else {
            self.connection.execute(
                "INSERT INTO broadcast_jobs (
                    idempotency_key, channel_id, kind, spend_bundle_id, spend_bundle,
                    funding_coin_id, fee, fee_coin_id, state
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    request.idempotency_key,
                    request.channel_id.as_ref(),
                    request.kind.as_str(),
                    spend_bundle_id.as_ref(),
                    spend_bundle,
                    request.funding_coin_id.as_ref(),
                    request.fee.map(u64_to_sql).transpose()?,
                    request.fee_coin_id.as_ref().map(|id| id.as_ref()),
                    BroadcastState::Prepared.as_str(),
                ],
            )?;
        }
        self.record_audit(
            Some(request.channel_id),
            "BROADCAST_PREPARED",
            Some(request.idempotency_key),
            serde_json::json!({"spend_bundle_id": hex::encode(spend_bundle_id)}),
        )?;
        self.load_broadcast(request.idempotency_key)?
            .ok_or(StateStoreError::CorruptData("broadcast_job"))
    }

    pub fn record_broadcast_attempt(
        &mut self,
        idempotency_key: &str,
        state: BroadcastState,
        error: Option<&str>,
    ) -> Result<BroadcastJob, StateStoreError> {
        let job = self
            .load_broadcast(idempotency_key)?
            .ok_or(StateStoreError::CorruptData("broadcast_job"))?;
        let next_attempt = job
            .attempts
            .checked_add(1)
            .ok_or(StateStoreError::CorruptData("attempt_overflow"))?;
        self.connection.execute(
            "UPDATE broadcast_jobs SET state = ?2, attempts = ?3, last_error = ?4,
                updated_at = CURRENT_TIMESTAMP WHERE idempotency_key = ?1",
            params![idempotency_key, state.as_str(), next_attempt, error],
        )?;
        self.connection.execute(
            "INSERT INTO broadcast_attempts (
                idempotency_key, attempt_no, spend_bundle_id, state, error
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                idempotency_key,
                next_attempt,
                job.spend_bundle_id.as_ref(),
                state.as_str(),
                error,
            ],
        )?;
        self.record_audit(
            Some(job.channel_id),
            "BROADCAST_ATTEMPT",
            Some(idempotency_key),
            serde_json::json!({
                "attempt": next_attempt,
                "state": state.as_str(),
                "error": error,
            }),
        )?;
        self.load_broadcast(idempotency_key)?
            .ok_or(StateStoreError::CorruptData("broadcast_job"))
    }

    pub fn update_broadcast_state(
        &mut self,
        idempotency_key: &str,
        state: BroadcastState,
        error: Option<&str>,
    ) -> Result<BroadcastJob, StateStoreError> {
        let job = self
            .load_broadcast(idempotency_key)?
            .ok_or(StateStoreError::CorruptData("broadcast_job"))?;
        self.connection.execute(
            "UPDATE broadcast_jobs SET state = ?2, last_error = ?3,
                updated_at = CURRENT_TIMESTAMP WHERE idempotency_key = ?1",
            params![idempotency_key, state.as_str(), error],
        )?;
        self.record_audit(
            Some(job.channel_id),
            "BROADCAST_STATE",
            Some(idempotency_key),
            serde_json::json!({"state": state.as_str(), "error": error}),
        )?;
        self.load_broadcast(idempotency_key)?
            .ok_or(StateStoreError::CorruptData("broadcast_job"))
    }

    pub fn load_broadcast(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<BroadcastJob>, StateStoreError> {
        let row = self
            .connection
            .query_row(
                "SELECT channel_id, kind, spend_bundle_id, spend_bundle, funding_coin_id,
                        fee, fee_coin_id, state, attempts, last_error
                 FROM broadcast_jobs WHERE idempotency_key = ?1",
                [idempotency_key],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Option<Vec<u8>>>(6)?,
                        row.get::<_, String>(7)?,
                        row.get::<_, i64>(8)?,
                        row.get::<_, Option<String>>(9)?,
                    ))
                },
            )
            .optional()?;
        row.map(
            |(
                channel_id,
                kind,
                spend_bundle_id,
                spend_bundle,
                funding_coin_id,
                fee,
                fee_coin_id,
                state,
                attempts,
                last_error,
            )| {
                Ok(BroadcastJob {
                    idempotency_key: idempotency_key.to_string(),
                    channel_id: bytes32_from_vec(channel_id, "channel_id")?,
                    kind: BroadcastKind::from_str(&kind)?,
                    spend_bundle_id: bytes32_from_vec(spend_bundle_id, "spend_bundle_id")?,
                    spend_bundle,
                    funding_coin_id: bytes32_from_vec(funding_coin_id, "funding_coin_id")?,
                    fee: fee.map(|value| sql_to_u64(value, "fee")).transpose()?,
                    fee_coin_id: fee_coin_id
                        .map(|value| bytes32_from_vec(value, "fee_coin_id"))
                        .transpose()?,
                    state: BroadcastState::from_str(&state)?,
                    attempts: i64_to_u32(attempts, "attempts")?,
                    last_error,
                })
            },
        )
        .transpose()
    }

    pub fn recoverable_broadcasts(&self) -> Result<Vec<BroadcastJob>, StateStoreError> {
        let mut statement = self.connection.prepare(
            "SELECT idempotency_key FROM broadcast_jobs
             WHERE state IN ('PREPARED', 'SUBMITTED', 'PENDING', 'REORGED')
             ORDER BY created_at",
        )?;
        let keys = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        keys.into_iter()
            .map(|key| {
                self.load_broadcast(&key)?
                    .ok_or(StateStoreError::CorruptData("broadcast_job"))
            })
            .collect()
    }

    pub fn metrics(&self) -> Result<StoreMetrics, StateStoreError> {
        let count = |sql: &str| -> Result<u64, StateStoreError> {
            let value: i64 = self.connection.query_row(sql, [], |row| row.get(0))?;
            i64_to_u64(value, "metric")
        };
        Ok(StoreMetrics {
            channels: count("SELECT COUNT(*) FROM channels")?,
            broadcast_jobs: count("SELECT COUNT(*) FROM broadcast_jobs")?,
            recoverable_broadcasts: count(
                "SELECT COUNT(*) FROM broadcast_jobs
                 WHERE state IN ('PREPARED', 'SUBMITTED', 'PENDING', 'REORGED')",
            )?,
            broadcast_attempts: count("SELECT COUNT(*) FROM broadcast_attempts")?,
            confirmed_broadcasts: count(
                "SELECT COUNT(*) FROM broadcast_jobs WHERE state = 'CONFIRMED'",
            )?,
            reorg_observations: count("SELECT COUNT(*) FROM chain_observations WHERE reorged = 1")?,
        })
    }

    pub fn list_audit_events(&self) -> Result<Vec<StoredAuditEvent>, StateStoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, channel_id, event_type, idempotency_key, details_json
             FROM audit_events ORDER BY id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<Vec<u8>>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        rows.map(|row| {
            let (id, channel_id, event_type, idempotency_key, details_json) = row?;
            Ok(StoredAuditEvent {
                id: i64_to_u64(id, "audit_id")?,
                channel_id: channel_id
                    .map(|value| bytes32_from_vec(value, "audit_channel_id"))
                    .transpose()?,
                event_type,
                idempotency_key,
                details_json,
            })
        })
        .collect()
    }

    fn record_audit(
        &self,
        channel_id: Option<Bytes32>,
        event_type: &str,
        idempotency_key: Option<&str>,
        details: serde_json::Value,
    ) -> Result<(), StateStoreError> {
        self.connection.execute(
            "INSERT INTO audit_events (
                channel_id, event_type, idempotency_key, details_json
             ) VALUES (?1, ?2, ?3, ?4)",
            params![
                channel_id.as_ref().map(|id| id.as_ref()),
                event_type,
                idempotency_key,
                serde_json::to_string(&details)
                    .map_err(|_| StateStoreError::CorruptData("audit_json"))?,
            ],
        )?;
        Ok(())
    }

    pub fn record_intent(
        &mut self,
        channel_id: Bytes32,
        intent: &PaymentIntent,
        invoice: &MerchantInvoice,
        args: &ChannelArgs,
        agg_sig_me_additional_data: Bytes32,
        current_height: u64,
    ) -> Result<(), StateStoreError> {
        intent.verify(invoice, args, agg_sig_me_additional_data, current_height)?;
        ensure_channel_binding(channel_id, intent)?;

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_unique_order_and_nonce(
            &transaction,
            channel_id,
            intent.commitment.order_id,
            intent.commitment.nonce,
        )?;
        require_state(&transaction, channel_id, ChannelState::Funded)?;
        insert_order_and_nonce(
            &transaction,
            channel_id,
            intent.commitment.order_id,
            intent.commitment.nonce,
        )?;
        transaction.execute(
            "UPDATE channels
             SET state = ?2, order_id = ?3, nonce = ?4, intent = ?5
             WHERE channel_id = ?1",
            params![
                channel_id.as_ref(),
                ChannelState::IntentSigned as i64,
                intent.commitment.order_id.as_ref(),
                intent.commitment.nonce.as_ref(),
                intent.to_bytes()
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn issue_voucher(
        &mut self,
        channel_id: Bytes32,
        invoice: &MerchantInvoice,
        args: &ChannelArgs,
        hub_signer: &dyn HubSigner,
        agg_sig_me_additional_data: Bytes32,
        current_height: u64,
    ) -> Result<PaymentVoucher, StateStoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        require_state(&transaction, channel_id, ChannelState::IntentSigned)?;
        let persisted_intent = load_intent_blob(&transaction, channel_id)?;
        let voucher = PaymentVoucher::issue_with_signer(
            persisted_intent,
            invoice,
            args,
            hub_signer,
            agg_sig_me_additional_data,
            current_height,
        )?;
        persist_voucher(&transaction, channel_id, &voucher)?;
        transaction.commit()?;
        Ok(voucher)
    }

    pub fn accept_intent_and_issue_voucher_atomic(
        &mut self,
        intent: &PaymentIntent,
        invoice: &MerchantInvoice,
        args: &ChannelArgs,
        hub_signer: &dyn HubSigner,
        agg_sig_me_additional_data: Bytes32,
        current_height: u64,
    ) -> Result<PaymentVoucher, StateStoreError> {
        let channel_id = intent.commitment.channel_id;
        intent.verify(invoice, args, agg_sig_me_additional_data, current_height)?;

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_unique_order_and_nonce(
            &transaction,
            channel_id,
            intent.commitment.order_id,
            intent.commitment.nonce,
        )?;
        require_state(&transaction, channel_id, ChannelState::Funded)?;
        let voucher = PaymentVoucher::issue_with_signer(
            intent.clone(),
            invoice,
            args,
            hub_signer,
            agg_sig_me_additional_data,
            current_height,
        )?;
        insert_order_and_nonce(
            &transaction,
            channel_id,
            intent.commitment.order_id,
            intent.commitment.nonce,
        )?;
        persist_voucher(&transaction, channel_id, &voucher)?;
        transaction.commit()?;
        Ok(voucher)
    }

    pub fn mark_claim_submitted(&mut self, channel_id: Bytes32) -> Result<(), StateStoreError> {
        self.transition(
            channel_id,
            &[ChannelState::VoucherIssued],
            ChannelState::ClaimSubmitted,
        )
    }

    pub fn mark_settled(&mut self, channel_id: Bytes32) -> Result<(), StateStoreError> {
        self.transition(
            channel_id,
            &[ChannelState::ClaimSubmitted],
            ChannelState::Settled,
        )
    }

    pub fn mark_refundable(&mut self, channel_id: Bytes32) -> Result<(), StateStoreError> {
        self.transition(
            channel_id,
            &[
                ChannelState::Funded,
                ChannelState::IntentSigned,
                ChannelState::VoucherIssued,
                ChannelState::ClaimSubmitted,
            ],
            ChannelState::Refundable,
        )
    }

    pub fn mark_refund_submitted(&mut self, channel_id: Bytes32) -> Result<(), StateStoreError> {
        self.transition(
            channel_id,
            &[ChannelState::Refundable],
            ChannelState::RefundSubmitted,
        )
    }

    pub fn mark_refunded(&mut self, channel_id: Bytes32) -> Result<(), StateStoreError> {
        self.transition(
            channel_id,
            &[ChannelState::RefundSubmitted],
            ChannelState::Refunded,
        )
    }

    pub fn rollback_claim_after_reorg(
        &mut self,
        channel_id: Bytes32,
    ) -> Result<(), StateStoreError> {
        self.transition(
            channel_id,
            &[ChannelState::Settled],
            ChannelState::ClaimSubmitted,
        )
    }

    pub fn rollback_refund_after_reorg(
        &mut self,
        channel_id: Bytes32,
    ) -> Result<(), StateStoreError> {
        self.transition(
            channel_id,
            &[ChannelState::Refunded],
            ChannelState::RefundSubmitted,
        )
    }

    pub fn record_chain_observation(
        &mut self,
        channel_id: Bytes32,
        observation: &crate::ChainObservation,
        fee: Option<u64>,
        reorged: bool,
    ) -> Result<(), StateStoreError> {
        let children = observation
            .children
            .iter()
            .map(|record| {
                serde_json::json!({
                    "coin_id": format!("0x{}", hex::encode(record.coin.coin_id().to_bytes())),
                    "parent_coin_info": format!("0x{}", hex::encode(record.coin.parent_coin_info.to_bytes())),
                    "puzzle_hash": format!("0x{}", hex::encode(record.coin.puzzle_hash.to_bytes())),
                    "amount": record.coin.amount,
                    "confirmed_block_index": record.confirmed_block_index,
                    "spent": record.spent,
                    "spent_block_index": record.spent_block_index,
                })
            })
            .collect::<Vec<_>>();
        let mempool_status = match observation.mempool {
            crate::MempoolStatus::Pending { .. } => "PENDING",
            crate::MempoolStatus::NotFound => "NOT_FOUND",
        };
        self.connection.execute(
            "INSERT INTO chain_observations (
                 channel_id, tx_id, funding_coin_id, observed_height, peak_hash,
                 funding_coin_json, confirmed_height, fee, mempool_status, children_json, reorged
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                channel_id.as_ref(),
                observation.tx_id.as_ref(),
                observation.funding_coin_id.as_ref(),
                observation.peak_height,
                observation.peak_hash.as_ref(),
                observation.funding_coin.as_ref().map(coin_record_json),
                observation.confirmed_height,
                fee.map(u64_to_sql).transpose()?,
                mempool_status,
                serde_json::to_string(&children)
                    .map_err(|_| StateStoreError::CorruptData("children_json"))?,
                if reorged { 1_i64 } else { 0_i64 },
            ],
        )?;
        Ok(())
    }

    pub fn list_chain_observations(
        &self,
        channel_id: Bytes32,
    ) -> Result<Vec<StoredChainObservation>, StateStoreError> {
        let mut statement = self.connection.prepare(
            "SELECT tx_id, funding_coin_id, observed_height, peak_hash,
                    funding_coin_json, confirmed_height, fee, mempool_status,
                    children_json, reorged
             FROM chain_observations WHERE channel_id = ?1 ORDER BY id",
        )?;
        let rows = statement.query_map([channel_id.as_ref()], |row| {
            let tx_id: Vec<u8> = row.get(0)?;
            let funding_coin_id: Vec<u8> = row.get(1)?;
            let peak_hash: Vec<u8> = row.get(3)?;
            let funding_coin_json: Option<String> = row.get(4)?;
            let children_json: String = row.get(8)?;
            Ok((
                tx_id,
                funding_coin_id,
                row.get::<_, i64>(2)?,
                peak_hash,
                funding_coin_json,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, String>(7)?,
                children_json,
                row.get::<_, i64>(9)?,
            ))
        })?;
        rows.map(|row| {
            let (
                tx_id,
                funding_coin_id,
                observed_height,
                peak_hash,
                funding_coin_json,
                confirmed_height,
                fee,
                mempool_status,
                children_json,
                reorged,
            ) = row?;
            let children = serde_json::from_str::<Vec<serde_json::Value>>(&children_json)
                .map_err(|_| StateStoreError::CorruptData("children_json"))?
                .into_iter()
                .map(|value| coin_from_observation_json(&value))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(StoredChainObservation {
                channel_id,
                tx_id: bytes32_from_vec(tx_id, "tx_id")?,
                funding_coin_id: bytes32_from_vec(funding_coin_id, "funding_coin_id")?,
                observed_height: i64_to_u32(observed_height, "observed_height")?,
                peak_hash: bytes32_from_vec(peak_hash, "peak_hash")?,
                funding_coin_json,
                confirmed_height: confirmed_height
                    .map(|value| i64_to_u32(value, "confirmed_height"))
                    .transpose()?,
                fee: fee.map(|value| sql_to_u64(value, "fee")).transpose()?,
                mempool_status,
                children,
                reorged: reorged != 0,
            })
        })
        .collect()
    }

    pub fn record_chain_observation_with_reorg(
        &mut self,
        channel_id: Bytes32,
        observation: &crate::ChainObservation,
        fee: Option<u64>,
    ) -> Result<bool, StateStoreError> {
        let previous = self.list_chain_observations(channel_id)?.into_iter().last();
        let reorged = previous.is_some_and(|previous| {
            previous.confirmed_height.is_some()
                && (observation.confirmed_height.is_none()
                    || observation.children.is_empty()
                    || observation.peak_height < previous.observed_height
                    || previous.children.iter().any(|previous_child| {
                        !observation.children.iter().any(|current_child| {
                            current_child.coin.coin_id() == previous_child.coin_id()
                        })
                    }))
        });
        if reorged {
            match self.load_channel(channel_id)?.state {
                ChannelState::Settled => self.rollback_claim_after_reorg(channel_id)?,
                ChannelState::Refunded => self.rollback_refund_after_reorg(channel_id)?,
                _ => {}
            }
        }
        self.record_chain_observation(channel_id, observation, fee, reorged)?;
        Ok(reorged)
    }

    pub fn load_channel(&self, channel_id: Bytes32) -> Result<ChannelRecord, StateStoreError> {
        type Row = (
            i64,
            Option<Vec<u8>>,
            Option<Vec<u8>>,
            Option<Vec<u8>>,
            Option<Vec<u8>>,
            i64,
            i64,
        );
        let row: Row = self
            .connection
            .query_row(
                "SELECT state, order_id, nonce, intent, voucher,
                        merchant_amount, user_remaining_amount
                 FROM channels WHERE channel_id = ?1",
                [channel_id.as_ref()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .optional()?
            .ok_or(StateStoreError::ChannelNotFound)?;

        Ok(ChannelRecord {
            channel_id,
            state: ChannelState::from_i64(row.0)?,
            order_id: row
                .1
                .map(|bytes| bytes32_from_vec(bytes, "order_id"))
                .transpose()?,
            nonce: row
                .2
                .map(|bytes| bytes32_from_vec(bytes, "nonce"))
                .transpose()?,
            intent: row
                .3
                .map(|bytes| PaymentIntent::from_bytes(&bytes))
                .transpose()?,
            voucher: row
                .4
                .map(|bytes| PaymentVoucher::from_bytes(&bytes))
                .transpose()?,
            merchant_amount: sql_to_u64(row.5, "merchant_amount")?,
            user_remaining_amount: sql_to_u64(row.6, "user_remaining_amount")?,
        })
    }

    fn channel_exists(&self, channel_id: Bytes32) -> Result<bool, StateStoreError> {
        Ok(self
            .connection
            .query_row(
                "SELECT 1 FROM channels WHERE channel_id = ?1",
                [channel_id.as_ref()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    fn transition(
        &mut self,
        channel_id: Bytes32,
        allowed: &[ChannelState],
        target: ChannelState,
    ) -> Result<(), StateStoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = load_state(&transaction, channel_id)?;
        if !allowed.contains(&current) {
            return Err(StateStoreError::IllegalStateTransition {
                from: current,
                to: target,
            });
        }
        transaction.execute(
            "UPDATE channels SET state = ?2 WHERE channel_id = ?1",
            params![channel_id.as_ref(), target as i64],
        )?;
        transaction.commit()?;
        Ok(())
    }
}

fn ensure_channel_binding(
    channel_id: Bytes32,
    intent: &PaymentIntent,
) -> Result<(), StateStoreError> {
    if intent.commitment.channel_id != channel_id {
        return Err(StateStoreError::VoucherMismatch);
    }
    Ok(())
}

fn ensure_unique_order_and_nonce(
    transaction: &Transaction<'_>,
    channel_id: Bytes32,
    order_id: Bytes32,
    nonce: Bytes32,
) -> Result<(), StateStoreError> {
    let duplicate_order = transaction
        .query_row(
            "SELECT 1 FROM orders WHERE channel_id = ?1 AND order_id = ?2",
            params![channel_id.as_ref(), order_id.as_ref()],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if duplicate_order {
        return Err(StateStoreError::DuplicateOrder);
    }
    let duplicate_nonce = transaction
        .query_row(
            "SELECT 1 FROM nonces WHERE channel_id = ?1 AND nonce = ?2",
            params![channel_id.as_ref(), nonce.as_ref()],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if duplicate_nonce {
        return Err(StateStoreError::DuplicateNonce);
    }
    Ok(())
}

fn insert_order_and_nonce(
    transaction: &Transaction<'_>,
    channel_id: Bytes32,
    order_id: Bytes32,
    nonce: Bytes32,
) -> Result<(), StateStoreError> {
    transaction.execute(
        "INSERT INTO orders (channel_id, order_id) VALUES (?1, ?2)",
        params![channel_id.as_ref(), order_id.as_ref()],
    )?;
    transaction.execute(
        "INSERT INTO nonces (channel_id, nonce) VALUES (?1, ?2)",
        params![channel_id.as_ref(), nonce.as_ref()],
    )?;
    Ok(())
}

fn persist_voucher(
    transaction: &Transaction<'_>,
    channel_id: Bytes32,
    voucher: &PaymentVoucher,
) -> Result<(), StateStoreError> {
    transaction.execute(
        "UPDATE channels
         SET state = ?2, voucher = ?3, merchant_amount = ?4,
             user_remaining_amount = ?5
         WHERE channel_id = ?1",
        params![
            channel_id.as_ref(),
            ChannelState::VoucherIssued as i64,
            voucher.to_bytes(),
            u64_to_sql(voucher.intent.commitment.merchant_amount)?,
            u64_to_sql(voucher.intent.commitment.user_remaining_amount)?
        ],
    )?;
    Ok(())
}

fn require_state(
    transaction: &Transaction<'_>,
    channel_id: Bytes32,
    expected: ChannelState,
) -> Result<(), StateStoreError> {
    let current = load_state(transaction, channel_id)?;
    if current != expected {
        return Err(StateStoreError::IllegalStateTransition {
            from: current,
            to: expected,
        });
    }
    Ok(())
}

fn load_state(
    transaction: &Transaction<'_>,
    channel_id: Bytes32,
) -> Result<ChannelState, StateStoreError> {
    let value = transaction
        .query_row(
            "SELECT state FROM channels WHERE channel_id = ?1",
            [channel_id.as_ref()],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(StateStoreError::ChannelNotFound)?;
    ChannelState::from_i64(value)
}

fn load_intent_blob(
    transaction: &Transaction<'_>,
    channel_id: Bytes32,
) -> Result<PaymentIntent, StateStoreError> {
    let bytes: Vec<u8> = transaction
        .query_row(
            "SELECT intent FROM channels WHERE channel_id = ?1",
            [channel_id.as_ref()],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(StateStoreError::ChannelNotFound)?;
    Ok(PaymentIntent::from_bytes(&bytes)?)
}

fn bytes32_from_vec(bytes: Vec<u8>, field: &'static str) -> Result<Bytes32, StateStoreError> {
    let value: [u8; 32] = bytes
        .try_into()
        .map_err(|_| StateStoreError::CorruptData(field))?;
    Ok(Bytes32::from(value))
}

fn u64_to_sql(value: u64) -> Result<i64, StateStoreError> {
    i64::try_from(value).map_err(|_| StateStoreError::CorruptData("u64_overflow"))
}

fn sql_to_u64(value: i64, field: &'static str) -> Result<u64, StateStoreError> {
    u64::try_from(value).map_err(|_| StateStoreError::CorruptData(field))
}

fn i64_to_u64(value: i64, field: &'static str) -> Result<u64, StateStoreError> {
    u64::try_from(value).map_err(|_| StateStoreError::CorruptData(field))
}

fn i64_to_u32(value: i64, field: &'static str) -> Result<u32, StateStoreError> {
    u32::try_from(value).map_err(|_| StateStoreError::CorruptData(field))
}

fn coin_from_observation_json(value: &serde_json::Value) -> Result<Coin, StateStoreError> {
    let parent = json_bytes32(value, "parent_coin_info")?;
    let puzzle_hash = json_bytes32(value, "puzzle_hash")?;
    let amount = value
        .get("amount")
        .and_then(serde_json::Value::as_u64)
        .ok_or(StateStoreError::CorruptData("children_json.amount"))?;
    Ok(Coin::new(parent, puzzle_hash, amount))
}

fn coin_record_json(record: &chia_sdk_coinset::CoinRecord) -> String {
    serde_json::json!({
        "coin_id": format!("0x{}", hex::encode(record.coin.coin_id().to_bytes())),
        "parent_coin_info": format!("0x{}", hex::encode(record.coin.parent_coin_info.to_bytes())),
        "puzzle_hash": format!("0x{}", hex::encode(record.coin.puzzle_hash.to_bytes())),
        "amount": record.coin.amount,
        "confirmed_block_index": record.confirmed_block_index,
        "spent": record.spent,
        "spent_block_index": record.spent_block_index,
        "timestamp": record.timestamp,
    })
    .to_string()
}

fn json_bytes32(value: &serde_json::Value, field: &str) -> Result<Bytes32, StateStoreError> {
    let raw = value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or(StateStoreError::CorruptData("children_json.bytes32"))?;
    let raw = raw.strip_prefix("0x").unwrap_or(raw);
    let bytes =
        hex::decode(raw).map_err(|_| StateStoreError::CorruptData("children_json.bytes32"))?;
    bytes32_from_vec(bytes, "children_json.bytes32")
}
