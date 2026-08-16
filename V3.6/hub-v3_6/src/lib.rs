use std::{path::Path, time::Duration};

use chia_bls::SecretKey;
use clvm_utils::tree_hash;
use clvmr::{Allocator, serde::node_from_bytes};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use thiserror::Error;
use xhub_protocol_v3_6::{
    Bytes32, CanonicalDecode, CanonicalEncode, ChannelTerms, Ledger, LedgerCheckpoint, LedgerEntry,
    MAX_CANONICAL_BLOB_BYTES, MAX_PROTOCOL_U64, OfficialState, ProtocolError, RecoveryPackage,
    ReservationResult, ReservationStatus, SignatureBytes, SignedReservationResult, StateZero,
    parse_signature, public_key_bytes, sha256_parts, sign_hash, verify_hash,
};

pub mod api;
mod chain;
mod transport;
mod vectors;
pub use chain::*;
pub use transport::*;
pub use vectors::*;

const RESERVATION_REQUEST_DOMAIN: &[u8] = b"XHUB_RESERVATION_REQUEST_V3_6";

#[derive(Debug, Error)]
pub enum HubError {
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error(transparent)]
    Chain(#[from] ChainProviderError),
    #[error("V3.6 channel was not found")]
    ChannelNotFound,
    #[error("V3.6 channel registration conflicts with persisted data")]
    ChannelConflict,
    #[error("HUB state signing key does not match channel terms")]
    HubKeyMismatch,
    #[error("reservation was not found")]
    ReservationNotFound,
    #[error("reservation nonce is already bound to different authorization content")]
    NonceConflict,
    #[error("a durable state transition is awaiting recovery")]
    PendingTransition,
    #[error("candidate state is not the unique adjacent append-only transition")]
    StateConflict,
    #[error("invalid HUB input: {0}")]
    Invalid(String),
    #[error("persisted HUB data is corrupt: {0}")]
    Corrupt(String),
    #[error("injected crash at {0:?}")]
    InjectedFailure(FailurePoint),
}

pub type Result<T> = std::result::Result<T, HubError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailurePoint {
    None,
    AfterPreparationCommit,
    AfterStateSignature,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelRegistration {
    pub funding_coin_id: Bytes32,
    pub funding_puzzle_reveal: Vec<u8>,
    pub funding_birth_height: u64,
    pub channel_terms: ChannelTerms,
}

pub const FUNDING_CONFIRMATION_BLOCKS_TEST: u64 = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainChannelRegistration {
    pub funding_coin_id: Bytes32,
    pub funding_puzzle_reveal: Vec<u8>,
    pub channel_terms: ChannelTerms,
}

impl ChainChannelRegistration {
    pub fn validate(&self) -> Result<()> {
        self.channel_terms.validate()?;
        if self.funding_puzzle_reveal.len() > MAX_CANONICAL_BLOB_BYTES {
            return Err(ProtocolError::LengthLimit {
                actual: self.funding_puzzle_reveal.len(),
                limit: MAX_CANONICAL_BLOB_BYTES,
            }
            .into());
        }
        program_tree_hash(&self.funding_puzzle_reveal)?;
        Ok(())
    }
}

impl ChannelRegistration {
    pub fn validate(&self) -> Result<()> {
        self.channel_terms.validate()?;
        if self.funding_puzzle_reveal.len() > MAX_CANONICAL_BLOB_BYTES {
            return Err(ProtocolError::LengthLimit {
                actual: self.funding_puzzle_reveal.len(),
                limit: MAX_CANONICAL_BLOB_BYTES,
            }
            .into());
        }
        checked_height("funding_birth_height", self.funding_birth_height)?;
        self.funding_puzzle_hash()?;
        self.acceptance_cutoff_height()?;
        self.scheduled_close_height()?;
        Ok(())
    }

    pub fn acceptance_cutoff_height(&self) -> Result<u64> {
        checked_add_height(
            self.funding_birth_height,
            self.channel_terms.acceptance_blocks,
            "acceptance_cutoff_height",
        )
    }

    pub fn scheduled_close_height(&self) -> Result<u64> {
        checked_add_height(
            self.funding_birth_height,
            self.channel_terms.close_delay_blocks,
            "scheduled_close_height",
        )
    }

    pub fn funding_puzzle_hash(&self) -> Result<Bytes32> {
        program_tree_hash(&self.funding_puzzle_reveal)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationRequest {
    pub request_id: Bytes32,
    pub funding_coin_id: Bytes32,
    pub ledger_entry: LedgerEntry,
    pub user_authorization_signature: SignatureBytes,
}

impl ReservationRequest {
    pub fn authorization_hash(&self, terms: &ChannelTerms) -> Result<Bytes32> {
        Ok(self
            .ledger_entry
            .authorization_hash(terms, &self.funding_coin_id)?)
    }

    pub fn fingerprint(&self) -> Result<Bytes32> {
        self.ledger_entry.validate()?;
        parse_signature(&self.user_authorization_signature)?;
        Ok(sha256_parts(&[
            RESERVATION_REQUEST_DOMAIN,
            &self.ledger_entry.canonical_bytes(),
            &self.user_authorization_signature,
        ]))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationOutcome {
    pub signed_result: SignedReservationResult,
    pub recovery_package: Option<RecoveryPackage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReservationLookup {
    Pending,
    Completed(Box<ReservationOutcome>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelSnapshot {
    pub funding_coin_id: Bytes32,
    pub latest_sequence: u64,
    pub latest_checkpoint_hash: Bytes32,
    pub entry_count: u64,
    pub funding_birth_height: u64,
    pub acceptance_cutoff_height: u64,
    pub scheduled_close_height: u64,
    pub chain_state: ChannelChainState,
    pub last_peak_height: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryDeliveryStatus {
    Pending,
    Delivered,
    FailedRetryable,
    FailedFinal,
}

impl RecoveryDeliveryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Delivered => "DELIVERED",
            Self::FailedRetryable => "FAILED_RETRYABLE",
            Self::FailedFinal => "FAILED_FINAL",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "PENDING" => Ok(Self::Pending),
            "DELIVERED" => Ok(Self::Delivered),
            "FAILED_RETRYABLE" => Ok(Self::FailedRetryable),
            "FAILED_FINAL" => Ok(Self::FailedFinal),
            _ => Err(HubError::Corrupt("recovery delivery status".into())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryDelivery {
    pub funding_coin_id: Bytes32,
    pub state_sequence: u64,
    pub checkpoint_hash: Bytes32,
    pub recovery_package_content_hash: Bytes32,
    pub recipient_id: String,
    pub recipient_kind: String,
    pub idempotency_key: String,
    pub status: RecoveryDeliveryStatus,
    pub attempt_count: u64,
    pub last_error: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelChainState {
    Unconfirmed,
    Active,
    NodeNotSynced,
    RpcUnavailable,
    ChainStateUncertain,
    ReorgPending,
    Closing,
}

impl ChannelChainState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Unconfirmed => "UNCONFIRMED",
            Self::Active => "ACTIVE",
            Self::NodeNotSynced => "NODE_NOT_SYNCED",
            Self::RpcUnavailable => "RPC_UNAVAILABLE",
            Self::ChainStateUncertain => "CHAIN_STATE_UNCERTAIN",
            Self::ReorgPending => "REORG_PENDING",
            Self::Closing => "CLOSING",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "UNCONFIRMED" => Ok(Self::Unconfirmed),
            "ACTIVE" => Ok(Self::Active),
            "NODE_NOT_SYNCED" => Ok(Self::NodeNotSynced),
            "RPC_UNAVAILABLE" => Ok(Self::RpcUnavailable),
            "CHAIN_STATE_UNCERTAIN" => Ok(Self::ChainStateUncertain),
            "REORG_PENDING" => Ok(Self::ReorgPending),
            "CLOSING" => Ok(Self::Closing),
            _ => Err(HubError::Corrupt("channel chain state".into())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateCandidate {
    pub checkpoint: LedgerCheckpoint,
    pub entries: Vec<LedgerEntry>,
    pub user_authorization_signatures: Vec<SignatureBytes>,
}

pub struct HubStore {
    connection: Connection,
}

impl HubStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::initialize(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(connection: Connection) -> Result<Self> {
        connection.busy_timeout(Duration::from_secs(10))?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             CREATE TABLE IF NOT EXISTS v36_channels (
                funding_coin_id BLOB PRIMARY KEY CHECK(length(funding_coin_id) = 32),
                channel_terms_blob BLOB NOT NULL,
                channel_terms_hash BLOB NOT NULL CHECK(length(channel_terms_hash) = 32),
                funding_puzzle_reveal BLOB NOT NULL,
                funding_puzzle_hash BLOB NOT NULL CHECK(length(funding_puzzle_hash) = 32),
                funding_birth_height INTEGER NOT NULL CHECK(funding_birth_height >= 0),
                acceptance_cutoff_height INTEGER NOT NULL CHECK(acceptance_cutoff_height > 0),
                scheduled_close_height INTEGER NOT NULL CHECK(scheduled_close_height > 0),
                confirmation_blocks INTEGER NOT NULL CHECK(confirmation_blocks >= 0),
                chain_state TEXT NOT NULL CHECK(chain_state IN
                  ('UNCONFIRMED', 'ACTIVE', 'NODE_NOT_SYNCED', 'RPC_UNAVAILABLE',
                   'CHAIN_STATE_UNCERTAIN', 'REORG_PENDING', 'CLOSING')),
                activated INTEGER NOT NULL CHECK(activated IN (0, 1)),
                last_peak_height INTEGER CHECK(last_peak_height IS NULL OR last_peak_height >= 0),
                last_peak_hash BLOB CHECK(last_peak_hash IS NULL OR length(last_peak_hash) = 32),
                latest_sequence INTEGER NOT NULL CHECK(latest_sequence >= 0),
                latest_checkpoint_hash BLOB NOT NULL CHECK(length(latest_checkpoint_hash) = 32),
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS v36_state_intents (
                funding_coin_id BLOB NOT NULL CHECK(length(funding_coin_id) = 32),
                state_sequence INTEGER NOT NULL CHECK(state_sequence > 0),
                previous_checkpoint_hash BLOB NOT NULL CHECK(length(previous_checkpoint_hash) = 32),
                checkpoint_hash BLOB NOT NULL CHECK(length(checkpoint_hash) = 32),
                checkpoint_blob BLOB NOT NULL,
                commit_height INTEGER NOT NULL CHECK(commit_height >= 0),
                reservation_nonce BLOB NOT NULL CHECK(length(reservation_nonce) = 32),
                stage TEXT NOT NULL CHECK(stage IN ('PREPARED', 'SIGNED')),
                hub_state_signature BLOB CHECK(hub_state_signature IS NULL OR length(hub_state_signature) = 96),
                recovery_package_blob BLOB,
                created_at INTEGER NOT NULL,
                signed_at INTEGER,
                PRIMARY KEY(funding_coin_id, state_sequence),
                UNIQUE(funding_coin_id, checkpoint_hash),
                FOREIGN KEY(funding_coin_id) REFERENCES v36_channels(funding_coin_id)
             );
             CREATE UNIQUE INDEX IF NOT EXISTS v36_one_prepared_intent
               ON v36_state_intents(funding_coin_id) WHERE stage = 'PREPARED';
             CREATE TABLE IF NOT EXISTS v36_states (
                funding_coin_id BLOB NOT NULL CHECK(length(funding_coin_id) = 32),
                state_sequence INTEGER NOT NULL CHECK(state_sequence > 0),
                checkpoint_hash BLOB NOT NULL CHECK(length(checkpoint_hash) = 32),
                official_state_blob BLOB NOT NULL,
                recovery_package_blob BLOB NOT NULL,
                commit_height INTEGER NOT NULL CHECK(commit_height >= 0),
                signed_at INTEGER NOT NULL,
                PRIMARY KEY(funding_coin_id, state_sequence),
                UNIQUE(funding_coin_id, checkpoint_hash),
                FOREIGN KEY(funding_coin_id, state_sequence)
                  REFERENCES v36_state_intents(funding_coin_id, state_sequence)
             );
             CREATE TABLE IF NOT EXISTS v36_ledger_entries (
                funding_coin_id BLOB NOT NULL CHECK(length(funding_coin_id) = 32),
                entry_index INTEGER NOT NULL CHECK(entry_index >= 0),
                entry_blob BLOB NOT NULL,
                user_authorization_signature BLOB NOT NULL CHECK(length(user_authorization_signature) = 96),
                reservation_nonce BLOB NOT NULL CHECK(length(reservation_nonce) = 32),
                state_sequence INTEGER NOT NULL CHECK(state_sequence > 0),
                PRIMARY KEY(funding_coin_id, entry_index),
                UNIQUE(funding_coin_id, reservation_nonce),
                FOREIGN KEY(funding_coin_id, state_sequence)
                  REFERENCES v36_states(funding_coin_id, state_sequence)
             );
             CREATE TABLE IF NOT EXISTS v36_reservations (
                funding_coin_id BLOB NOT NULL CHECK(length(funding_coin_id) = 32),
                reservation_nonce BLOB NOT NULL CHECK(length(reservation_nonce) = 32),
                request_fingerprint BLOB NOT NULL CHECK(length(request_fingerprint) = 32),
                request_id BLOB NOT NULL CHECK(length(request_id) = 32),
                authorization_hash BLOB NOT NULL CHECK(length(authorization_hash) = 32),
                entry_blob BLOB NOT NULL,
                user_authorization_signature BLOB NOT NULL CHECK(length(user_authorization_signature) = 96),
                observed_peak_height INTEGER NOT NULL CHECK(observed_peak_height >= 0),
                acceptance_cutoff_height INTEGER NOT NULL CHECK(acceptance_cutoff_height > 0),
                scheduled_close_height INTEGER NOT NULL CHECK(scheduled_close_height > 0),
                target_status INTEGER NOT NULL,
                target_state_sequence INTEGER,
                target_checkpoint_hash BLOB CHECK(target_checkpoint_hash IS NULL OR length(target_checkpoint_hash) = 32),
                entry_index INTEGER,
                ledger_written INTEGER NOT NULL CHECK(ledger_written IN (0, 1)),
                stage TEXT NOT NULL CHECK(stage IN ('PREPARED', 'SIGNED')),
                signed_result_blob BLOB,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY(funding_coin_id, reservation_nonce),
                FOREIGN KEY(funding_coin_id) REFERENCES v36_channels(funding_coin_id)
             );
             CREATE TABLE IF NOT EXISTS v36_recovery_deliveries (
                funding_coin_id BLOB NOT NULL CHECK(length(funding_coin_id) = 32),
                state_sequence INTEGER NOT NULL CHECK(state_sequence > 0),
                checkpoint_hash BLOB NOT NULL CHECK(length(checkpoint_hash) = 32),
                recovery_package_content_hash BLOB NOT NULL CHECK(length(recovery_package_content_hash) = 32),
                recipient_id TEXT NOT NULL CHECK(length(recipient_id) BETWEEN 1 AND 256),
                recipient_kind TEXT NOT NULL CHECK(recipient_kind IN ('MERCHANT', 'WATCHTOWER')),
                idempotency_key TEXT NOT NULL CHECK(length(idempotency_key) BETWEEN 1 AND 256),
                status TEXT NOT NULL CHECK(status IN
                  ('PENDING', 'DELIVERED', 'FAILED_RETRYABLE', 'FAILED_FINAL')),
                attempt_count INTEGER NOT NULL CHECK(attempt_count > 0),
                last_error TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY(funding_coin_id, state_sequence, recipient_id, idempotency_key),
                FOREIGN KEY(funding_coin_id, state_sequence)
                  REFERENCES v36_states(funding_coin_id, state_sequence)
             );",
        )?;
        Ok(Self { connection })
    }

    pub fn register_channel(
        &mut self,
        registration: &ChannelRegistration,
        now: u64,
    ) -> Result<ChannelSnapshot> {
        self.register_channel_record(registration, now, 0, ChannelChainState::Active, None)
    }

    pub fn register_channel_from_chain<P: ChainStateProvider + ?Sized>(
        &mut self,
        registration: &ChainChannelRegistration,
        provider: &P,
        confirmation_blocks: u64,
        now: u64,
    ) -> Result<ChannelSnapshot> {
        registration.validate()?;
        checked_height("confirmation_blocks", confirmation_blocks)?;
        let snapshot = provider.snapshot(registration.funding_coin_id)?;
        if snapshot.network_id != registration.channel_terms.network_id {
            return Err(HubError::Invalid("chain network_id mismatch".into()));
        }
        if !snapshot.synced {
            return Err(HubError::Invalid("chain node is not synced".into()));
        }
        let peak = snapshot
            .peak
            .ok_or_else(|| HubError::Invalid("chain peak is missing".into()))?;
        let (birth_height, puzzle_hash, amount) = match snapshot.funding_coin {
            FundingCoinState::Confirmed {
                birth_height,
                puzzle_hash,
                amount,
            } => (birth_height, puzzle_hash, amount),
            FundingCoinState::Missing => {
                return Err(HubError::Invalid("Funding Coin is not confirmed".into()));
            }
            FundingCoinState::Spent { .. } => {
                return Err(HubError::Invalid("Funding Coin is already spent".into()));
            }
        };
        let expected_puzzle_hash = program_tree_hash(&registration.funding_puzzle_reveal)?;
        if puzzle_hash != expected_puzzle_hash
            || amount != registration.channel_terms.funding_amount
        {
            return Err(HubError::Invalid(
                "Funding Coin fields do not match channel".into(),
            ));
        }
        let confirmations = confirmation_depth(peak.height, birth_height);
        let chain_state = if confirmations >= confirmation_blocks {
            ChannelChainState::Active
        } else {
            ChannelChainState::Unconfirmed
        };
        let trusted = ChannelRegistration {
            funding_coin_id: registration.funding_coin_id,
            funding_puzzle_reveal: registration.funding_puzzle_reveal.clone(),
            funding_birth_height: birth_height,
            channel_terms: registration.channel_terms.clone(),
        };
        self.register_channel_record(&trusted, now, confirmation_blocks, chain_state, Some(peak))
    }

    fn register_channel_record(
        &mut self,
        registration: &ChannelRegistration,
        now: u64,
        confirmation_blocks: u64,
        chain_state: ChannelChainState,
        last_peak: Option<ChainPeak>,
    ) -> Result<ChannelSnapshot> {
        registration.validate()?;
        checked_height("now", now)?;
        let terms_hash = registration.channel_terms.hash()?;
        let funding_puzzle_hash = registration.funding_puzzle_hash()?;
        let state_zero_hash = StateZero::new(&registration.channel_terms)?
            .hash(&registration.channel_terms, &registration.funding_coin_id)?;
        let cutoff = registration.acceptance_cutoff_height()?;
        let scheduled_close = registration.scheduled_close_height()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO v36_channels
             (funding_coin_id, channel_terms_blob, channel_terms_hash, funding_puzzle_reveal,
              funding_puzzle_hash, funding_birth_height, acceptance_cutoff_height,
              scheduled_close_height, confirmation_blocks, chain_state,
              activated, last_peak_height, last_peak_hash,
              latest_sequence, latest_checkpoint_hash, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                     0, ?14, ?15, ?15)
             ON CONFLICT(funding_coin_id) DO NOTHING",
            params![
                registration.funding_coin_id.as_slice(),
                registration.channel_terms.canonical_bytes(),
                terms_hash.as_slice(),
                registration.funding_puzzle_reveal,
                funding_puzzle_hash.as_slice(),
                to_i64(registration.funding_birth_height)?,
                to_i64(cutoff)?,
                to_i64(scheduled_close)?,
                to_i64(confirmation_blocks)?,
                chain_state.as_str(),
                i64::from(chain_state == ChannelChainState::Active),
                last_peak
                    .as_ref()
                    .map(|peak| to_i64(peak.height))
                    .transpose()?,
                last_peak.as_ref().map(|peak| peak.header_hash.to_vec()),
                state_zero_hash.as_slice(),
                to_i64(now)?,
            ],
        )?;
        let stored = load_channel(&tx, &registration.funding_coin_id)?;
        if stored.terms != registration.channel_terms
            || stored.funding_puzzle_reveal != registration.funding_puzzle_reveal
            || stored.funding_puzzle_hash != funding_puzzle_hash
            || stored.funding_birth_height != registration.funding_birth_height
            || stored.acceptance_cutoff_height != cutoff
            || stored.scheduled_close_height != scheduled_close
            || stored.confirmation_blocks != confirmation_blocks
            || (stored.latest_sequence == 0 && stored.latest_checkpoint_hash != state_zero_hash)
        {
            return Err(HubError::ChannelConflict);
        }
        tx.commit()?;
        self.channel_snapshot(registration.funding_coin_id)
    }

    pub fn reserve(
        &mut self,
        request: &ReservationRequest,
        observed_peak_height: u64,
        hub_secret_key: &SecretKey,
        now: u64,
    ) -> Result<ReservationOutcome> {
        self.reserve_internal(
            request,
            ReservationGate::Manual(observed_peak_height),
            hub_secret_key,
            now,
            FailurePoint::None,
        )
    }

    pub fn reserve_with_failure(
        &mut self,
        request: &ReservationRequest,
        observed_peak_height: u64,
        hub_secret_key: &SecretKey,
        now: u64,
        failure_point: FailurePoint,
    ) -> Result<ReservationOutcome> {
        self.reserve_internal(
            request,
            ReservationGate::Manual(observed_peak_height),
            hub_secret_key,
            now,
            failure_point,
        )
    }

    pub fn reserve_with_chain(
        &mut self,
        request: &ReservationRequest,
        provider: &dyn ChainStateProvider,
        hub_secret_key: &SecretKey,
        now: u64,
    ) -> Result<ReservationOutcome> {
        self.reserve_with_chain_failure(request, provider, hub_secret_key, now, FailurePoint::None)
    }

    pub fn reserve_with_chain_failure(
        &mut self,
        request: &ReservationRequest,
        provider: &dyn ChainStateProvider,
        hub_secret_key: &SecretKey,
        now: u64,
        failure_point: FailurePoint,
    ) -> Result<ReservationOutcome> {
        let initial = provider.snapshot(request.funding_coin_id);
        self.reserve_internal(
            request,
            ReservationGate::Chain { provider, initial },
            hub_secret_key,
            now,
            failure_point,
        )
    }

    fn reserve_internal(
        &mut self,
        request: &ReservationRequest,
        gate: ReservationGate<'_>,
        hub_secret_key: &SecretKey,
        now: u64,
        failure_point: FailurePoint,
    ) -> Result<ReservationOutcome> {
        checked_height("now", now)?;
        let fingerprint = request.fingerprint()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut channel = load_channel(&tx, &request.funding_coin_id)?;
        require_hub_key(&channel.terms, hub_secret_key)?;
        let authorization_hash = request.authorization_hash(&channel.terms)?;

        if let Some(existing) = load_reservation(
            &tx,
            &request.funding_coin_id,
            &request.ledger_entry.reservation_nonce,
        )? {
            validate_reservation_binding(
                &existing,
                &channel,
                &request.funding_coin_id,
                &request.ledger_entry.reservation_nonce,
            )?;
            if existing.request_fingerprint != fingerprint {
                return Err(HubError::NonceConflict);
            }
            let stage = existing.stage;
            tx.commit()?;
            if stage == ReservationStage::Signed {
                return self.completed_reservation(
                    request.funding_coin_id,
                    request.ledger_entry.reservation_nonce,
                );
            }
            return self.finalize_prepared(
                request.funding_coin_id,
                request.ledger_entry.reservation_nonce,
                hub_secret_key,
                now,
                failure_point,
            );
        }

        let gate =
            evaluate_reservation_gate(&tx, &mut channel, request.funding_coin_id, gate, now)?;
        let observed_peak_height = gate.observed_peak_height;
        let authorization_valid = verify_hash(
            &channel.terms.user_public_key,
            &authorization_hash,
            &request.user_authorization_signature,
        )
        .is_ok();

        let mut target_status = if !authorization_valid {
            ReservationStatus::InvalidAuthorization
        } else if let Some(status) = gate.blocking_status {
            status
        } else {
            ReservationStatus::Signed
        };
        let mut target_sequence = None;
        let mut target_checkpoint_hash = None;
        let mut entry_index = None;
        let mut checkpoint = None;

        if target_status == ReservationStatus::Signed {
            if has_prepared_intent(&tx, &request.funding_coin_id)? {
                return Err(HubError::PendingTransition);
            }
            let (mut entries, mut signatures) =
                load_committed_ledger(&tx, &request.funding_coin_id)?;
            entry_index = Some(entries.len() as u64);
            entries.push(request.ledger_entry.clone());
            signatures.push(request.user_authorization_signature);
            let ledger = Ledger {
                entries: entries.clone(),
            };
            match ledger.validate(&channel.terms) {
                Ok(_) => {
                    let sequence = channel
                        .latest_sequence
                        .checked_add(1)
                        .ok_or(ProtocolError::ArithmeticOverflow("state_sequence"))?;
                    let candidate_checkpoint = ledger.checkpoint(
                        &channel.terms,
                        request.funding_coin_id,
                        sequence,
                        channel.latest_checkpoint_hash,
                    )?;
                    let candidate = StateCandidate {
                        checkpoint: candidate_checkpoint.clone(),
                        entries,
                        user_authorization_signatures: signatures,
                    };
                    validate_candidate_against(&tx, &channel, &candidate)?;
                    let hash = candidate_checkpoint.hash(&channel.terms)?;
                    target_sequence = Some(sequence);
                    target_checkpoint_hash = Some(hash);
                    checkpoint = Some(candidate_checkpoint);
                }
                Err(ProtocolError::LedgerFull) => target_status = ReservationStatus::LedgerFull,
                Err(ProtocolError::InsufficientRemainder) => {
                    target_status = ReservationStatus::InsufficientRemainder
                }
                Err(error) => return Err(error.into()),
            }
        }

        tx.execute(
            "INSERT INTO v36_reservations
             (funding_coin_id, reservation_nonce, request_fingerprint, request_id,
              authorization_hash, entry_blob, user_authorization_signature,
              observed_peak_height, acceptance_cutoff_height, scheduled_close_height,
              target_status, target_state_sequence,
              target_checkpoint_hash, entry_index, ledger_written, stage,
              signed_result_blob, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                     ?13, ?14, ?15, 'PREPARED', NULL, ?16, ?16)",
            params![
                request.funding_coin_id.as_slice(),
                request.ledger_entry.reservation_nonce.as_slice(),
                fingerprint.as_slice(),
                request.request_id.as_slice(),
                authorization_hash.as_slice(),
                request.ledger_entry.canonical_bytes(),
                request.user_authorization_signature.as_slice(),
                to_i64(observed_peak_height)?,
                to_i64(channel.acceptance_cutoff_height)?,
                to_i64(channel.scheduled_close_height)?,
                target_status as u16,
                target_sequence.map(to_i64).transpose()?,
                target_checkpoint_hash.map(|value| value.to_vec()),
                entry_index.map(to_i64).transpose()?,
                i64::from(target_status == ReservationStatus::Signed),
                to_i64(now)?,
            ],
        )?;

        if let (Some(sequence), Some(hash), Some(checkpoint)) =
            (target_sequence, target_checkpoint_hash, checkpoint)
        {
            tx.execute(
                "INSERT INTO v36_state_intents
                 (funding_coin_id, state_sequence, previous_checkpoint_hash,
                  checkpoint_hash, checkpoint_blob, commit_height, reservation_nonce,
                  stage, hub_state_signature, recovery_package_blob, created_at, signed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'PREPARED', NULL, NULL, ?8, NULL)",
                params![
                    request.funding_coin_id.as_slice(),
                    to_i64(sequence)?,
                    checkpoint.previous_checkpoint_hash.as_slice(),
                    hash.as_slice(),
                    checkpoint.canonical_bytes(),
                    to_i64(observed_peak_height)?,
                    request.ledger_entry.reservation_nonce.as_slice(),
                    to_i64(now)?,
                ],
            )?;
        }
        tx.commit()?;

        if failure_point == FailurePoint::AfterPreparationCommit {
            return Err(HubError::InjectedFailure(failure_point));
        }
        self.finalize_prepared(
            request.funding_coin_id,
            request.ledger_entry.reservation_nonce,
            hub_secret_key,
            now,
            failure_point,
        )
    }

    pub fn recover_pending(
        &mut self,
        hub_secret_key: &SecretKey,
        now: u64,
    ) -> Result<Vec<ReservationOutcome>> {
        checked_height("now", now)?;
        let pending = {
            let mut statement = self.connection.prepare(
                "SELECT funding_coin_id, reservation_nonce FROM v36_reservations
                 WHERE stage = 'PREPARED' ORDER BY created_at, funding_coin_id",
            )?;
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut recovered = Vec::with_capacity(pending.len());
        for (coin_id, nonce) in pending {
            recovered.push(self.finalize_prepared(
                bytes32(coin_id, "pending funding coin id")?,
                bytes32(nonce, "pending reservation nonce")?,
                hub_secret_key,
                now,
                FailurePoint::None,
            )?);
        }
        Ok(recovered)
    }

    pub fn reservation_status(
        &self,
        funding_coin_id: Bytes32,
        reservation_nonce: Bytes32,
    ) -> Result<ReservationLookup> {
        let channel = load_channel(&self.connection, &funding_coin_id)?;
        let row = load_reservation(&self.connection, &funding_coin_id, &reservation_nonce)?
            .ok_or(HubError::ReservationNotFound)?;
        validate_reservation_binding(&row, &channel, &funding_coin_id, &reservation_nonce)?;
        if row.stage == ReservationStage::Prepared {
            Ok(ReservationLookup::Pending)
        } else {
            Ok(ReservationLookup::Completed(Box::new(
                self.completed_reservation(funding_coin_id, reservation_nonce)?,
            )))
        }
    }

    pub fn channel_snapshot(&self, funding_coin_id: Bytes32) -> Result<ChannelSnapshot> {
        let channel = load_channel(&self.connection, &funding_coin_id)?;
        let entry_count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM v36_ledger_entries WHERE funding_coin_id = ?1",
            [funding_coin_id.as_slice()],
            |row| row.get(0),
        )?;
        Ok(ChannelSnapshot {
            funding_coin_id,
            latest_sequence: channel.latest_sequence,
            latest_checkpoint_hash: channel.latest_checkpoint_hash,
            entry_count: from_i64(entry_count, "entry count")?,
            funding_birth_height: channel.funding_birth_height,
            acceptance_cutoff_height: channel.acceptance_cutoff_height,
            scheduled_close_height: channel.scheduled_close_height,
            chain_state: channel.chain_state,
            last_peak_height: channel.last_peak_height,
        })
    }

    pub fn latest_recovery_package(
        &self,
        funding_coin_id: Bytes32,
    ) -> Result<Option<RecoveryPackage>> {
        let channel = load_channel(&self.connection, &funding_coin_id)?;
        if channel.latest_sequence == 0 {
            return Ok(None);
        }
        self.recovery_package(funding_coin_id, channel.latest_sequence)
            .map(Some)
    }

    pub fn recovery_package(
        &self,
        funding_coin_id: Bytes32,
        state_sequence: u64,
    ) -> Result<RecoveryPackage> {
        checked_height("state_sequence", state_sequence)?;
        if state_sequence == 0 {
            return Err(HubError::Invalid("state_sequence must be positive".into()));
        }
        let channel = load_channel(&self.connection, &funding_coin_id)?;
        let bytes: Vec<u8> = self
            .connection
            .query_row(
                "SELECT recovery_package_blob FROM v36_states
             WHERE funding_coin_id = ?1 AND state_sequence = ?2",
                params![funding_coin_id.as_slice(), to_i64(state_sequence)?],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(HubError::ReservationNotFound)?;
        let package = RecoveryPackage::from_canonical_bytes(&bytes)
            .map_err(|error| HubError::Corrupt(format!("recovery package: {error}")))?;
        package.validate()?;
        if package.official_state.checkpoint.state_sequence != state_sequence {
            return Err(HubError::Corrupt("recovery package sequence".into()));
        }
        let checkpoint_hash: Vec<u8> = self.connection.query_row(
            "SELECT checkpoint_hash FROM v36_states
             WHERE funding_coin_id = ?1 AND state_sequence = ?2",
            params![funding_coin_id.as_slice(), to_i64(state_sequence)?],
            |row| row.get(0),
        )?;
        if package.official_state.checkpoint.hash(&channel.terms)?
            != bytes32(checkpoint_hash, "state checkpoint hash")?
        {
            return Err(HubError::Corrupt(
                "recovery package checkpoint binding".into(),
            ));
        }
        Ok(package)
    }

    pub fn begin_recovery_delivery(
        &mut self,
        funding_coin_id: Bytes32,
        state_sequence: u64,
        recipient_id: &str,
        recipient_kind: &str,
        idempotency_key: &str,
        now: u64,
    ) -> Result<(RecoveryDelivery, RecoveryPackage)> {
        validate_delivery_text("recipient_id", recipient_id)?;
        validate_delivery_text("idempotency_key", idempotency_key)?;
        if !matches!(recipient_kind, "MERCHANT" | "WATCHTOWER") {
            return Err(HubError::Invalid("invalid recipient_kind".into()));
        }
        checked_height("now", now)?;
        let package = self.recovery_package(funding_coin_id, state_sequence)?;
        let checkpoint_hash = package
            .official_state
            .checkpoint
            .hash(&package.channel_terms)?;
        let content_hash = package.content_hash()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = load_recovery_delivery(
            &tx,
            &funding_coin_id,
            state_sequence,
            recipient_id,
            idempotency_key,
        )?;
        if let Some(existing) = existing {
            if existing.checkpoint_hash != checkpoint_hash
                || existing.recovery_package_content_hash != content_hash
                || existing.recipient_kind != recipient_kind
            {
                return Err(HubError::NonceConflict);
            }
            if matches!(
                existing.status,
                RecoveryDeliveryStatus::Delivered | RecoveryDeliveryStatus::FailedFinal
            ) {
                tx.commit()?;
                return Ok((existing, package));
            }
            tx.execute(
                "UPDATE v36_recovery_deliveries
                 SET status = 'PENDING', attempt_count = attempt_count + 1,
                     last_error = NULL, updated_at = ?5
                 WHERE funding_coin_id = ?1 AND state_sequence = ?2
                   AND recipient_id = ?3 AND idempotency_key = ?4",
                params![
                    funding_coin_id.as_slice(),
                    to_i64(state_sequence)?,
                    recipient_id,
                    idempotency_key,
                    to_i64(now)?,
                ],
            )?;
        } else {
            tx.execute(
                "INSERT INTO v36_recovery_deliveries (
                   funding_coin_id, state_sequence, checkpoint_hash,
                   recovery_package_content_hash, recipient_id, recipient_kind,
                   idempotency_key, status, attempt_count, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'PENDING', 1, ?8, ?8)",
                params![
                    funding_coin_id.as_slice(),
                    to_i64(state_sequence)?,
                    checkpoint_hash.as_slice(),
                    content_hash.as_slice(),
                    recipient_id,
                    recipient_kind,
                    idempotency_key,
                    to_i64(now)?,
                ],
            )?;
        }
        tx.commit()?;
        let delivery = self.recovery_delivery(
            funding_coin_id,
            state_sequence,
            recipient_id,
            idempotency_key,
        )?;
        Ok((delivery, package))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn finish_recovery_delivery(
        &mut self,
        funding_coin_id: Bytes32,
        state_sequence: u64,
        recipient_id: &str,
        idempotency_key: &str,
        status: RecoveryDeliveryStatus,
        last_error: Option<&str>,
        now: u64,
    ) -> Result<RecoveryDelivery> {
        if status == RecoveryDeliveryStatus::Pending {
            return Err(HubError::Invalid(
                "delivery result cannot be PENDING".into(),
            ));
        }
        if last_error.is_some_and(|error| error.len() > 1024) {
            return Err(HubError::Invalid("last_error exceeds 1024 bytes".into()));
        }
        let changed = self.connection.execute(
            "UPDATE v36_recovery_deliveries SET status = ?5, last_error = ?6, updated_at = ?7
             WHERE funding_coin_id = ?1 AND state_sequence = ?2
               AND recipient_id = ?3 AND idempotency_key = ?4 AND status = 'PENDING'",
            params![
                funding_coin_id.as_slice(),
                to_i64(state_sequence)?,
                recipient_id,
                idempotency_key,
                status.as_str(),
                last_error,
                to_i64(now)?,
            ],
        )?;
        if changed != 1 {
            let existing = self.recovery_delivery(
                funding_coin_id,
                state_sequence,
                recipient_id,
                idempotency_key,
            )?;
            if existing.status == RecoveryDeliveryStatus::Delivered {
                return Ok(existing);
            }
            return Err(HubError::StateConflict);
        }
        self.recovery_delivery(
            funding_coin_id,
            state_sequence,
            recipient_id,
            idempotency_key,
        )
    }

    pub fn recovery_delivery(
        &self,
        funding_coin_id: Bytes32,
        state_sequence: u64,
        recipient_id: &str,
        idempotency_key: &str,
    ) -> Result<RecoveryDelivery> {
        load_recovery_delivery(
            &self.connection,
            &funding_coin_id,
            state_sequence,
            recipient_id,
            idempotency_key,
        )?
        .ok_or(HubError::ReservationNotFound)
    }

    pub fn recovery_deliveries(
        &self,
        funding_coin_id: Bytes32,
        state_sequence: u64,
    ) -> Result<Vec<RecoveryDelivery>> {
        self.recovery_package(funding_coin_id, state_sequence)?;
        let mut statement = self.connection.prepare(
            "SELECT checkpoint_hash, recovery_package_content_hash, recipient_id,
                    recipient_kind, idempotency_key, status, attempt_count,
                    last_error, created_at, updated_at
             FROM v36_recovery_deliveries
             WHERE funding_coin_id = ?1 AND state_sequence = ?2
             ORDER BY recipient_kind, recipient_id, idempotency_key",
        )?;
        let rows = statement.query_map(
            params![funding_coin_id.as_slice(), to_i64(state_sequence)?],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            },
        )?;
        rows.map(|row| delivery_from_raw(funding_coin_id, state_sequence, row?))
            .collect()
    }

    pub fn validate_next_state(
        &self,
        funding_coin_id: Bytes32,
        candidate: &StateCandidate,
    ) -> Result<()> {
        let channel = load_channel(&self.connection, &funding_coin_id)?;
        validate_candidate_against(&self.connection, &channel, candidate)
    }

    pub fn intent_count(&self, funding_coin_id: Bytes32) -> Result<u64> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM v36_state_intents WHERE funding_coin_id = ?1",
            [funding_coin_id.as_slice()],
            |row| row.get(0),
        )?;
        from_i64(count, "intent count")
    }

    pub fn durability_mode(&self) -> Result<(String, i64)> {
        let journal_mode = self
            .connection
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
        let synchronous = self
            .connection
            .query_row("PRAGMA synchronous", [], |row| row.get(0))?;
        Ok((journal_mode, synchronous))
    }

    fn finalize_prepared(
        &mut self,
        funding_coin_id: Bytes32,
        reservation_nonce: Bytes32,
        hub_secret_key: &SecretKey,
        now: u64,
        failure_point: FailurePoint,
    ) -> Result<ReservationOutcome> {
        let channel = load_channel(&self.connection, &funding_coin_id)?;
        require_hub_key(&channel.terms, hub_secret_key)?;
        let reservation = load_reservation(&self.connection, &funding_coin_id, &reservation_nonce)?
            .ok_or(HubError::ReservationNotFound)?;
        validate_reservation_binding(&reservation, &channel, &funding_coin_id, &reservation_nonce)?;
        if reservation.stage == ReservationStage::Signed {
            return self.completed_reservation(funding_coin_id, reservation_nonce);
        }

        let result =
            reservation_result_from(&reservation, &channel, funding_coin_id, reservation_nonce);
        let signed_result = SignedReservationResult {
            hub_result_signature: sign_hash(hub_secret_key, &result.hash()?),
            result,
        };
        signed_result.verify(&channel.terms)?;

        let mut official_state = None;
        let mut recovery_package = None;
        if reservation.target_status == ReservationStatus::Signed {
            let checkpoint = load_intent_checkpoint(
                &self.connection,
                &funding_coin_id,
                reservation
                    .target_state_sequence
                    .ok_or_else(|| HubError::Corrupt("signed reservation sequence".into()))?,
            )?;
            let state_signature =
                sign_hash(hub_secret_key, &checkpoint.hub_state_hash(&channel.terms)?);
            let state = OfficialState {
                checkpoint,
                hub_state_signature: state_signature,
            };
            state.verify(&channel.terms)?;
            let (mut entries, mut signatures) =
                load_committed_ledger(&self.connection, &funding_coin_id)?;
            entries.push(reservation.entry.clone());
            signatures.push(reservation.user_authorization_signature);
            let package = RecoveryPackage {
                funding_coin_id,
                funding_puzzle_reveal: channel.funding_puzzle_reveal.clone(),
                funding_amount: channel.terms.funding_amount,
                channel_terms: channel.terms.clone(),
                official_state: state.clone(),
                entries,
                user_authorization_signatures: signatures,
            };
            package.validate()?;
            official_state = Some(state);
            recovery_package = Some(package);
        }

        if failure_point == FailurePoint::AfterStateSignature {
            return Err(HubError::InjectedFailure(failure_point));
        }

        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let persisted = load_reservation(&tx, &funding_coin_id, &reservation_nonce)?
            .ok_or(HubError::ReservationNotFound)?;
        validate_reservation_binding(&persisted, &channel, &funding_coin_id, &reservation_nonce)?;
        if persisted.stage == ReservationStage::Signed {
            tx.commit()?;
            return self.completed_reservation(funding_coin_id, reservation_nonce);
        }
        if persisted.request_fingerprint != reservation.request_fingerprint
            || persisted.target_status != reservation.target_status
            || persisted.target_checkpoint_hash != reservation.target_checkpoint_hash
        {
            return Err(HubError::StateConflict);
        }

        if let (Some(state), Some(package)) = (&official_state, &recovery_package) {
            let current = load_channel(&tx, &funding_coin_id)?;
            let sequence = state.checkpoint.state_sequence;
            let checkpoint_hash = state.checkpoint.hash(&current.terms)?;
            if sequence != current.latest_sequence + 1
                || state.checkpoint.previous_checkpoint_hash != current.latest_checkpoint_hash
                || Some(checkpoint_hash) != reservation.target_checkpoint_hash
            {
                return Err(HubError::StateConflict);
            }
            let intent_stage: String = tx.query_row(
                "SELECT stage FROM v36_state_intents
                 WHERE funding_coin_id = ?1 AND state_sequence = ?2 AND checkpoint_hash = ?3",
                params![
                    funding_coin_id.as_slice(),
                    to_i64(sequence)?,
                    checkpoint_hash.as_slice()
                ],
                |row| row.get(0),
            )?;
            if intent_stage != "PREPARED" {
                return Err(HubError::StateConflict);
            }
            let entry_index = reservation
                .entry_index
                .ok_or_else(|| HubError::Corrupt("signed reservation entry index".into()))?;
            tx.execute(
                "INSERT INTO v36_states
                 (funding_coin_id, state_sequence, checkpoint_hash, official_state_blob,
                  recovery_package_blob, commit_height, signed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    funding_coin_id.as_slice(),
                    to_i64(sequence)?,
                    checkpoint_hash.as_slice(),
                    state.canonical_bytes(),
                    package.canonical_bytes(),
                    to_i64(reservation.observed_peak_height)?,
                    to_i64(now)?,
                ],
            )?;
            tx.execute(
                "INSERT INTO v36_ledger_entries
                 (funding_coin_id, entry_index, entry_blob, user_authorization_signature,
                  reservation_nonce, state_sequence)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    funding_coin_id.as_slice(),
                    to_i64(entry_index)?,
                    reservation.entry.canonical_bytes(),
                    reservation.user_authorization_signature.as_slice(),
                    reservation_nonce.as_slice(),
                    to_i64(sequence)?,
                ],
            )?;
            let changed = tx.execute(
                "UPDATE v36_channels
                 SET latest_sequence = ?2, latest_checkpoint_hash = ?3, updated_at = ?4
                 WHERE funding_coin_id = ?1 AND latest_sequence = ?5
                   AND latest_checkpoint_hash = ?6",
                params![
                    funding_coin_id.as_slice(),
                    to_i64(sequence)?,
                    checkpoint_hash.as_slice(),
                    to_i64(now)?,
                    to_i64(current.latest_sequence)?,
                    current.latest_checkpoint_hash.as_slice(),
                ],
            )?;
            if changed != 1 {
                return Err(HubError::StateConflict);
            }
            let changed = tx.execute(
                "UPDATE v36_state_intents
                 SET stage = 'SIGNED', hub_state_signature = ?3,
                     recovery_package_blob = ?4, signed_at = ?5
                 WHERE funding_coin_id = ?1 AND state_sequence = ?2 AND stage = 'PREPARED'",
                params![
                    funding_coin_id.as_slice(),
                    to_i64(sequence)?,
                    state.hub_state_signature.as_slice(),
                    package.canonical_bytes(),
                    to_i64(now)?,
                ],
            )?;
            if changed != 1 {
                return Err(HubError::StateConflict);
            }
        }

        let changed = tx.execute(
            "UPDATE v36_reservations
             SET stage = 'SIGNED', signed_result_blob = ?3, updated_at = ?4
             WHERE funding_coin_id = ?1 AND reservation_nonce = ?2 AND stage = 'PREPARED'",
            params![
                funding_coin_id.as_slice(),
                reservation_nonce.as_slice(),
                signed_result.canonical_bytes(),
                to_i64(now)?,
            ],
        )?;
        if changed != 1 {
            return Err(HubError::StateConflict);
        }
        tx.commit()?;
        Ok(ReservationOutcome {
            signed_result,
            recovery_package,
        })
    }

    fn completed_reservation(
        &self,
        funding_coin_id: Bytes32,
        reservation_nonce: Bytes32,
    ) -> Result<ReservationOutcome> {
        let channel = load_channel(&self.connection, &funding_coin_id)?;
        let row = load_reservation(&self.connection, &funding_coin_id, &reservation_nonce)?
            .ok_or(HubError::ReservationNotFound)?;
        validate_reservation_binding(&row, &channel, &funding_coin_id, &reservation_nonce)?;
        if row.stage != ReservationStage::Signed {
            return Err(HubError::PendingTransition);
        }
        let bytes = row
            .signed_result_blob
            .as_ref()
            .ok_or_else(|| HubError::Corrupt("missing signed reservation result".into()))?;
        let signed_result = SignedReservationResult::from_canonical_bytes(bytes)
            .map_err(|error| HubError::Corrupt(format!("signed reservation result: {error}")))?;
        signed_result.verify(&channel.terms)?;
        if signed_result.result
            != reservation_result_from(&row, &channel, funding_coin_id, reservation_nonce)
        {
            return Err(HubError::Corrupt(
                "signed reservation result binding".into(),
            ));
        }
        let recovery_package = match row.target_state_sequence {
            Some(sequence) if row.ledger_written => {
                let bytes: Vec<u8> = self.connection.query_row(
                    "SELECT recovery_package_blob FROM v36_states
                     WHERE funding_coin_id = ?1 AND state_sequence = ?2",
                    params![funding_coin_id.as_slice(), to_i64(sequence)?],
                    |sql_row| sql_row.get(0),
                )?;
                let package = RecoveryPackage::from_canonical_bytes(&bytes)
                    .map_err(|error| HubError::Corrupt(format!("recovery package: {error}")))?;
                package.validate()?;
                Some(package)
            }
            _ => None,
        };
        Ok(ReservationOutcome {
            signed_result,
            recovery_package,
        })
    }
}

#[derive(Debug, Clone)]
struct ChannelRecord {
    funding_coin_id: Bytes32,
    terms: ChannelTerms,
    funding_puzzle_reveal: Vec<u8>,
    funding_puzzle_hash: Bytes32,
    funding_birth_height: u64,
    acceptance_cutoff_height: u64,
    scheduled_close_height: u64,
    confirmation_blocks: u64,
    chain_state: ChannelChainState,
    activated: bool,
    last_peak_height: Option<u64>,
    last_peak_hash: Option<Bytes32>,
    latest_sequence: u64,
    latest_checkpoint_hash: Bytes32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReservationStage {
    Prepared,
    Signed,
}

#[derive(Debug, Clone)]
struct ReservationRow {
    request_fingerprint: Bytes32,
    request_id: Bytes32,
    authorization_hash: Bytes32,
    entry: LedgerEntry,
    user_authorization_signature: SignatureBytes,
    observed_peak_height: u64,
    acceptance_cutoff_height: u64,
    scheduled_close_height: u64,
    target_status: ReservationStatus,
    target_state_sequence: Option<u64>,
    target_checkpoint_hash: Option<Bytes32>,
    entry_index: Option<u64>,
    ledger_written: bool,
    stage: ReservationStage,
    signed_result_blob: Option<Vec<u8>>,
}

enum ReservationGate<'a> {
    Manual(u64),
    Chain {
        provider: &'a dyn ChainStateProvider,
        initial: ChainProviderResult<ChainSnapshot>,
    },
}

struct GateEvaluation {
    observed_peak_height: u64,
    blocking_status: Option<ReservationStatus>,
}

fn evaluate_reservation_gate(
    tx: &Transaction<'_>,
    channel: &mut ChannelRecord,
    funding_coin_id: Bytes32,
    gate: ReservationGate<'_>,
    now: u64,
) -> Result<GateEvaluation> {
    match gate {
        ReservationGate::Manual(height) => {
            checked_height("observed_peak_height", height)?;
            Ok(GateEvaluation {
                observed_peak_height: height,
                blocking_status: (height >= channel.acceptance_cutoff_height)
                    .then_some(ReservationStatus::RejectedFreezing),
            })
        }
        ReservationGate::Chain { provider, initial } => {
            let committed = provider.snapshot(funding_coin_id);
            evaluate_chain_snapshots(tx, channel, initial, committed, now)
        }
    }
}

fn evaluate_chain_snapshots(
    tx: &Transaction<'_>,
    channel: &mut ChannelRecord,
    initial: ChainProviderResult<ChainSnapshot>,
    committed: ChainProviderResult<ChainSnapshot>,
    now: u64,
) -> Result<GateEvaluation> {
    let (initial, committed) =
        match (initial, committed) {
            (Ok(initial), Ok(committed)) => (initial, committed),
            (initial, committed) => {
                let observed_peak = committed
                    .as_ref()
                    .ok()
                    .and_then(|snapshot| snapshot.peak.clone())
                    .or_else(|| {
                        initial
                            .as_ref()
                            .ok()
                            .and_then(|snapshot| snapshot.peak.clone())
                    });
                let rpc_unavailable =
                    initial.as_ref().err().is_some_and(|error| {
                        matches!(error, ChainProviderError::RpcUnavailable(_))
                    }) || committed.as_ref().err().is_some_and(|error| {
                        matches!(error, ChainProviderError::RpcUnavailable(_))
                    });
                let (chain_state, status) = if rpc_unavailable {
                    (
                        ChannelChainState::RpcUnavailable,
                        ReservationStatus::RpcUnavailable,
                    )
                } else {
                    (
                        ChannelChainState::ChainStateUncertain,
                        ReservationStatus::ChainStateUncertain,
                    )
                };
                persist_chain_observation(
                    tx,
                    channel,
                    chain_state,
                    channel.activated,
                    channel.funding_birth_height,
                    channel.acceptance_cutoff_height,
                    channel.scheduled_close_height,
                    observed_peak.clone(),
                    now,
                )?;
                return Ok(GateEvaluation {
                    observed_peak_height: observed_peak
                        .map(|peak| peak.height)
                        .or(channel.last_peak_height)
                        .unwrap_or(0),
                    blocking_status: Some(status),
                });
            }
        };

    let committed_peak = match committed.peak.clone() {
        Some(peak) => peak,
        None => {
            return persist_blocked_snapshot(
                tx,
                channel,
                ChannelChainState::ChainStateUncertain,
                ReservationStatus::ChainStateUncertain,
                None,
                now,
            );
        }
    };
    let initial_peak = match initial.peak.as_ref() {
        Some(peak) => peak,
        None => {
            return persist_blocked_snapshot(
                tx,
                channel,
                ChannelChainState::ChainStateUncertain,
                ReservationStatus::ChainStateUncertain,
                Some(committed_peak),
                now,
            );
        }
    };
    if initial.network_id != channel.terms.network_id
        || committed.network_id != channel.terms.network_id
    {
        return persist_blocked_snapshot(
            tx,
            channel,
            ChannelChainState::ChainStateUncertain,
            ReservationStatus::ChainStateUncertain,
            Some(committed_peak),
            now,
        );
    }
    if !initial.synced || !committed.synced {
        return persist_blocked_snapshot(
            tx,
            channel,
            ChannelChainState::NodeNotSynced,
            ReservationStatus::NodeNotSynced,
            Some(committed_peak),
            now,
        );
    }

    let mut reorged = committed_peak.height < initial_peak.height
        || (committed_peak.height == initial_peak.height
            && committed_peak.header_hash != initial_peak.header_hash)
        || channel.last_peak_height.is_some_and(|height| {
            committed_peak.height < height
                || (committed_peak.height == height
                    && channel.last_peak_hash != Some(committed_peak.header_hash))
        });

    let (birth_height, puzzle_hash, amount) = match committed.funding_coin {
        FundingCoinState::Missing => {
            let (state, status) = if channel.activated {
                (
                    ChannelChainState::ReorgPending,
                    ReservationStatus::ChannelReorgPending,
                )
            } else {
                (
                    ChannelChainState::ChainStateUncertain,
                    ReservationStatus::ChainStateUncertain,
                )
            };
            return persist_blocked_snapshot(tx, channel, state, status, Some(committed_peak), now);
        }
        FundingCoinState::Spent {
            birth_height,
            puzzle_hash,
            amount,
            ..
        } => {
            if puzzle_hash != channel.funding_puzzle_hash || amount != channel.terms.funding_amount
            {
                return persist_blocked_snapshot(
                    tx,
                    channel,
                    ChannelChainState::ChainStateUncertain,
                    ReservationStatus::ChainStateUncertain,
                    Some(committed_peak),
                    now,
                );
            }
            persist_chain_observation(
                tx,
                channel,
                ChannelChainState::Closing,
                channel.activated,
                birth_height,
                channel.acceptance_cutoff_height,
                checked_add_height(
                    birth_height,
                    channel.terms.close_delay_blocks,
                    "scheduled_close_height",
                )?,
                Some(committed_peak.clone()),
                now,
            )?;
            return Ok(GateEvaluation {
                observed_peak_height: committed_peak.height,
                blocking_status: Some(ReservationStatus::ChannelClosing),
            });
        }
        FundingCoinState::Confirmed {
            birth_height,
            puzzle_hash,
            amount,
        } => (birth_height, puzzle_hash, amount),
    };
    if puzzle_hash != channel.funding_puzzle_hash || amount != channel.terms.funding_amount {
        return persist_blocked_snapshot(
            tx,
            channel,
            ChannelChainState::ChainStateUncertain,
            ReservationStatus::ChainStateUncertain,
            Some(committed_peak),
            now,
        );
    }
    if let FundingCoinState::Confirmed {
        birth_height: initial_birth,
        puzzle_hash: initial_puzzle,
        amount: initial_amount,
    } = initial.funding_coin
    {
        if initial_puzzle != channel.funding_puzzle_hash
            || initial_amount != channel.terms.funding_amount
            || initial_birth != birth_height
        {
            reorged = true;
        }
    } else {
        reorged = true;
    }
    if birth_height != channel.funding_birth_height {
        reorged = true;
    }

    let new_cutoff = checked_add_height(
        birth_height,
        channel.terms.acceptance_blocks,
        "acceptance_cutoff_height",
    )?;
    let effective_cutoff = if channel.activated {
        channel.acceptance_cutoff_height.min(new_cutoff)
    } else {
        new_cutoff
    };
    let scheduled_close = checked_add_height(
        birth_height,
        channel.terms.close_delay_blocks,
        "scheduled_close_height",
    )?;
    let confirmed =
        confirmation_depth(committed_peak.height, birth_height) >= channel.confirmation_blocks;
    let activated = channel.activated || confirmed;
    let (chain_state, blocking_status) = if !activated {
        (
            ChannelChainState::Unconfirmed,
            Some(ReservationStatus::ChainStateUncertain),
        )
    } else if reorged {
        (
            ChannelChainState::ReorgPending,
            Some(ReservationStatus::ChannelReorgPending),
        )
    } else if committed_peak.height >= effective_cutoff {
        (
            ChannelChainState::Active,
            Some(ReservationStatus::RejectedFreezing),
        )
    } else {
        (ChannelChainState::Active, None)
    };
    persist_chain_observation(
        tx,
        channel,
        chain_state,
        activated,
        birth_height,
        effective_cutoff,
        scheduled_close,
        Some(committed_peak.clone()),
        now,
    )?;
    Ok(GateEvaluation {
        observed_peak_height: committed_peak.height,
        blocking_status,
    })
}

fn persist_blocked_snapshot(
    tx: &Transaction<'_>,
    channel: &mut ChannelRecord,
    chain_state: ChannelChainState,
    status: ReservationStatus,
    peak: Option<ChainPeak>,
    now: u64,
) -> Result<GateEvaluation> {
    persist_chain_observation(
        tx,
        channel,
        chain_state,
        channel.activated,
        channel.funding_birth_height,
        channel.acceptance_cutoff_height,
        channel.scheduled_close_height,
        peak.clone(),
        now,
    )?;
    Ok(GateEvaluation {
        observed_peak_height: peak
            .map(|peak| peak.height)
            .or(channel.last_peak_height)
            .unwrap_or(0),
        blocking_status: Some(status),
    })
}

#[allow(clippy::too_many_arguments)]
fn persist_chain_observation(
    tx: &Transaction<'_>,
    channel: &mut ChannelRecord,
    chain_state: ChannelChainState,
    activated: bool,
    funding_birth_height: u64,
    acceptance_cutoff_height: u64,
    scheduled_close_height: u64,
    peak: Option<ChainPeak>,
    now: u64,
) -> Result<()> {
    let last_peak_height = peak
        .as_ref()
        .map(|peak| peak.height)
        .or(channel.last_peak_height);
    let last_peak_hash = peak
        .as_ref()
        .map(|peak| peak.header_hash)
        .or(channel.last_peak_hash);
    let changed = tx.execute(
        "UPDATE v36_channels
         SET funding_birth_height = ?2, acceptance_cutoff_height = ?3,
             scheduled_close_height = ?4, chain_state = ?5, activated = ?6,
             last_peak_height = ?7, last_peak_hash = ?8, updated_at = ?9
         WHERE funding_coin_id = ?1",
        params![
            channel.funding_coin_id.as_slice(),
            to_i64(funding_birth_height)?,
            to_i64(acceptance_cutoff_height)?,
            to_i64(scheduled_close_height)?,
            chain_state.as_str(),
            i64::from(activated),
            last_peak_height.map(to_i64).transpose()?,
            last_peak_hash.map(|hash| hash.to_vec()),
            to_i64(now)?,
        ],
    )?;
    if changed != 1 {
        return Err(HubError::ChannelNotFound);
    }
    channel.funding_birth_height = funding_birth_height;
    channel.acceptance_cutoff_height = acceptance_cutoff_height;
    channel.scheduled_close_height = scheduled_close_height;
    channel.chain_state = chain_state;
    channel.activated = activated;
    channel.last_peak_height = last_peak_height;
    channel.last_peak_hash = last_peak_hash;
    Ok(())
}

fn load_channel(connection: &Connection, funding_coin_id: &Bytes32) -> Result<ChannelRecord> {
    let raw = connection
        .query_row(
            "SELECT channel_terms_blob, channel_terms_hash, funding_puzzle_reveal,
                    funding_puzzle_hash, funding_birth_height,
                    acceptance_cutoff_height, scheduled_close_height, confirmation_blocks,
                    chain_state, activated, last_peak_height, last_peak_hash, latest_sequence,
                    latest_checkpoint_hash
             FROM v36_channels WHERE funding_coin_id = ?1",
            [funding_coin_id.as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, Option<i64>>(10)?,
                    row.get::<_, Option<Vec<u8>>>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, Vec<u8>>(13)?,
                ))
            },
        )
        .optional()?
        .ok_or(HubError::ChannelNotFound)?;
    let terms = ChannelTerms::from_canonical_bytes(&raw.0)
        .map_err(|error| HubError::Corrupt(format!("channel terms: {error}")))?;
    let terms_hash = bytes32(raw.1, "channel terms hash")?;
    let funding_puzzle_hash = bytes32(raw.3, "funding puzzle hash")?;
    let funding_birth_height = from_i64(raw.4, "funding birth height")?;
    let acceptance_cutoff_height = from_i64(raw.5, "acceptance cutoff height")?;
    let scheduled_close_height = from_i64(raw.6, "scheduled close height")?;
    let confirmation_blocks = from_i64(raw.7, "confirmation blocks")?;
    let chain_state = ChannelChainState::parse(&raw.8)?;
    let activated = match raw.9 {
        0 => false,
        1 => true,
        _ => return Err(HubError::Corrupt("channel activated flag".into())),
    };
    let last_peak_height = raw
        .10
        .map(|value| from_i64(value, "last peak height"))
        .transpose()?;
    let last_peak_hash = raw
        .11
        .map(|value| bytes32(value, "last peak hash"))
        .transpose()?;
    let latest_sequence = from_i64(raw.12, "latest sequence")?;
    let latest_checkpoint_hash = bytes32(raw.13, "latest checkpoint hash")?;
    let calculated_cutoff = checked_add_height(
        funding_birth_height,
        terms.acceptance_blocks,
        "acceptance_cutoff_height",
    )?;
    if terms.hash()? != terms_hash
        || program_tree_hash(&raw.2)? != funding_puzzle_hash
        || (!activated && calculated_cutoff != acceptance_cutoff_height)
        || (activated && acceptance_cutoff_height > calculated_cutoff)
        || checked_add_height(
            funding_birth_height,
            terms.close_delay_blocks,
            "scheduled_close_height",
        )? != scheduled_close_height
    {
        return Err(HubError::Corrupt("channel terms binding".into()));
    }
    if latest_sequence == 0 {
        let expected = StateZero::new(&terms)?.hash(&terms, funding_coin_id)?;
        if latest_checkpoint_hash != expected {
            return Err(HubError::Corrupt("state zero pointer".into()));
        }
    } else {
        let persisted_hash = connection
            .query_row(
                "SELECT checkpoint_hash FROM v36_states
                 WHERE funding_coin_id = ?1 AND state_sequence = ?2",
                params![funding_coin_id.as_slice(), to_i64(latest_sequence)?],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?
            .ok_or_else(|| HubError::Corrupt("latest state row".into()))?;
        if bytes32(persisted_hash, "latest state checkpoint hash")? != latest_checkpoint_hash {
            return Err(HubError::Corrupt("latest state pointer".into()));
        }
    }
    Ok(ChannelRecord {
        funding_coin_id: *funding_coin_id,
        terms,
        funding_puzzle_reveal: raw.2,
        funding_puzzle_hash,
        funding_birth_height,
        acceptance_cutoff_height,
        scheduled_close_height,
        confirmation_blocks,
        chain_state,
        activated,
        last_peak_height,
        last_peak_hash,
        latest_sequence,
        latest_checkpoint_hash,
    })
}

fn load_reservation(
    connection: &Connection,
    funding_coin_id: &Bytes32,
    reservation_nonce: &Bytes32,
) -> Result<Option<ReservationRow>> {
    #[allow(clippy::type_complexity)]
    let raw = connection
        .query_row(
            "SELECT request_fingerprint, request_id, authorization_hash, entry_blob,
                    user_authorization_signature, observed_peak_height,
                    acceptance_cutoff_height, scheduled_close_height, target_status,
                    target_state_sequence, target_checkpoint_hash, entry_index,
                    ledger_written, stage, signed_result_blob
             FROM v36_reservations
             WHERE funding_coin_id = ?1 AND reservation_nonce = ?2",
            params![funding_coin_id.as_slice(), reservation_nonce.as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, Option<Vec<u8>>>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                    row.get::<_, i64>(12)?,
                    row.get::<_, String>(13)?,
                    row.get::<_, Option<Vec<u8>>>(14)?,
                ))
            },
        )
        .optional()?;
    raw.map(|raw| {
        let entry = LedgerEntry::from_canonical_bytes(&raw.3)
            .map_err(|error| HubError::Corrupt(format!("ledger entry: {error}")))?;
        let signature = signature96(raw.4, "user authorization signature")?;
        parse_signature(&signature)
            .map_err(|error| HubError::Corrupt(format!("user signature: {error}")))?;
        let status_code =
            u16::try_from(raw.8).map_err(|_| HubError::Corrupt("reservation status".into()))?;
        let row = ReservationRow {
            request_fingerprint: bytes32(raw.0, "request fingerprint")?,
            request_id: bytes32(raw.1, "request id")?,
            authorization_hash: bytes32(raw.2, "authorization hash")?,
            entry,
            user_authorization_signature: signature,
            observed_peak_height: from_i64(raw.5, "observed peak height")?,
            acceptance_cutoff_height: from_i64(raw.6, "reservation acceptance cutoff")?,
            scheduled_close_height: from_i64(raw.7, "reservation scheduled close")?,
            target_status: ReservationStatus::from_code(status_code)
                .map_err(|error| HubError::Corrupt(format!("reservation status: {error}")))?,
            target_state_sequence: raw
                .9
                .map(|value| from_i64(value, "target state sequence"))
                .transpose()?,
            target_checkpoint_hash: raw
                .10
                .map(|value| bytes32(value, "target checkpoint hash"))
                .transpose()?,
            entry_index: raw
                .11
                .map(|value| from_i64(value, "entry index"))
                .transpose()?,
            ledger_written: match raw.12 {
                0 => false,
                1 => true,
                _ => return Err(HubError::Corrupt("ledger_written".into())),
            },
            stage: match raw.13.as_str() {
                "PREPARED" => ReservationStage::Prepared,
                "SIGNED" => ReservationStage::Signed,
                _ => return Err(HubError::Corrupt("reservation stage".into())),
            },
            signed_result_blob: raw.14,
        };
        let expected_fingerprint = sha256_parts(&[
            RESERVATION_REQUEST_DOMAIN,
            &row.entry.canonical_bytes(),
            &row.user_authorization_signature,
        ]);
        if row.entry.reservation_nonce != *reservation_nonce
            || row.request_fingerprint != expected_fingerprint
        {
            return Err(HubError::Corrupt("reservation request binding".into()));
        }
        Ok(row)
    })
    .transpose()
}

fn validate_reservation_binding(
    row: &ReservationRow,
    channel: &ChannelRecord,
    funding_coin_id: &Bytes32,
    reservation_nonce: &Bytes32,
) -> Result<()> {
    if row.entry.reservation_nonce != *reservation_nonce
        || row.authorization_hash
            != row
                .entry
                .authorization_hash(&channel.terms, funding_coin_id)?
    {
        return Err(HubError::Corrupt(
            "reservation authorization binding".into(),
        ));
    }
    let signed_target = row.target_status == ReservationStatus::Signed;
    if signed_target
        != (row.target_state_sequence.is_some()
            && row.target_checkpoint_hash.is_some()
            && row.entry_index.is_some()
            && row.ledger_written)
        || (!signed_target
            && (row.target_state_sequence.is_some()
                || row.target_checkpoint_hash.is_some()
                || row.entry_index.is_some()
                || row.ledger_written))
        || (row.stage == ReservationStage::Prepared && row.signed_result_blob.is_some())
        || (row.stage == ReservationStage::Signed && row.signed_result_blob.is_none())
    {
        return Err(HubError::Corrupt("reservation state binding".into()));
    }
    Ok(())
}

fn reservation_result_from(
    row: &ReservationRow,
    channel: &ChannelRecord,
    funding_coin_id: Bytes32,
    reservation_nonce: Bytes32,
) -> ReservationResult {
    ReservationResult {
        network_id: channel.terms.network_id,
        request_id: row.request_id,
        funding_coin_id,
        reservation_nonce,
        authorization_hash: row.authorization_hash,
        status: row.target_status,
        state_sequence: row.target_state_sequence,
        checkpoint_hash: row.target_checkpoint_hash,
        observed_peak_height: row.observed_peak_height,
        acceptance_cutoff_height: row.acceptance_cutoff_height,
        scheduled_close_height: row.scheduled_close_height,
        ledger_written: row.ledger_written,
    }
}

fn load_committed_ledger(
    connection: &Connection,
    funding_coin_id: &Bytes32,
) -> Result<(Vec<LedgerEntry>, Vec<SignatureBytes>)> {
    let mut statement = connection.prepare(
        "SELECT entry_blob, user_authorization_signature FROM v36_ledger_entries
         WHERE funding_coin_id = ?1 ORDER BY entry_index",
    )?;
    let raw = statement
        .query_map([funding_coin_id.as_slice()], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut entries = Vec::with_capacity(raw.len());
    let mut signatures = Vec::with_capacity(raw.len());
    for (entry, signature) in raw {
        entries.push(
            LedgerEntry::from_canonical_bytes(&entry)
                .map_err(|error| HubError::Corrupt(format!("ledger entry: {error}")))?,
        );
        let signature = signature96(signature, "user authorization signature")?;
        parse_signature(&signature)
            .map_err(|error| HubError::Corrupt(format!("user signature: {error}")))?;
        signatures.push(signature);
    }
    Ok((entries, signatures))
}

fn validate_candidate_against(
    connection: &Connection,
    channel: &ChannelRecord,
    candidate: &StateCandidate,
) -> Result<()> {
    if candidate.entries.len() != candidate.user_authorization_signatures.len()
        || candidate.checkpoint.funding_coin_id != channel.funding_coin_id
        || candidate.checkpoint.state_sequence != channel.latest_sequence + 1
        || candidate.checkpoint.previous_checkpoint_hash != channel.latest_checkpoint_hash
        || candidate.checkpoint.channel_terms_hash != channel.terms.hash()?
    {
        return Err(HubError::StateConflict);
    }
    let (stored_entries, stored_signatures) =
        load_committed_ledger(connection, &candidate.checkpoint.funding_coin_id)?;
    if candidate.entries.len() <= stored_entries.len()
        || candidate.entries[..stored_entries.len()] != stored_entries
        || candidate.user_authorization_signatures[..stored_signatures.len()] != stored_signatures
    {
        return Err(HubError::StateConflict);
    }
    for (entry, signature) in candidate
        .entries
        .iter()
        .zip(&candidate.user_authorization_signatures)
    {
        verify_hash(
            &channel.terms.user_public_key,
            &entry.authorization_hash(&channel.terms, &candidate.checkpoint.funding_coin_id)?,
            signature,
        )?;
    }
    let expected = Ledger {
        entries: candidate.entries.clone(),
    }
    .checkpoint(
        &channel.terms,
        candidate.checkpoint.funding_coin_id,
        candidate.checkpoint.state_sequence,
        candidate.checkpoint.previous_checkpoint_hash,
    )?;
    if expected != candidate.checkpoint {
        return Err(HubError::StateConflict);
    }
    Ok(())
}

fn load_intent_checkpoint(
    connection: &Connection,
    funding_coin_id: &Bytes32,
    state_sequence: u64,
) -> Result<LedgerCheckpoint> {
    let bytes: Vec<u8> = connection.query_row(
        "SELECT checkpoint_blob FROM v36_state_intents
         WHERE funding_coin_id = ?1 AND state_sequence = ?2 AND stage = 'PREPARED'",
        params![funding_coin_id.as_slice(), to_i64(state_sequence)?],
        |row| row.get(0),
    )?;
    LedgerCheckpoint::from_canonical_bytes(&bytes)
        .map_err(|error| HubError::Corrupt(format!("checkpoint intent: {error}")))
}

fn has_prepared_intent(connection: &Connection, funding_coin_id: &Bytes32) -> Result<bool> {
    Ok(connection
        .query_row(
            "SELECT 1 FROM v36_state_intents
             WHERE funding_coin_id = ?1 AND stage = 'PREPARED'",
            [funding_coin_id.as_slice()],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

type RawRecoveryDelivery = (
    Vec<u8>,
    Vec<u8>,
    String,
    String,
    String,
    String,
    i64,
    Option<String>,
    i64,
    i64,
);

fn load_recovery_delivery(
    connection: &Connection,
    funding_coin_id: &Bytes32,
    state_sequence: u64,
    recipient_id: &str,
    idempotency_key: &str,
) -> Result<Option<RecoveryDelivery>> {
    let raw = connection
        .query_row(
            "SELECT checkpoint_hash, recovery_package_content_hash, recipient_id,
                    recipient_kind, idempotency_key, status, attempt_count,
                    last_error, created_at, updated_at
             FROM v36_recovery_deliveries
             WHERE funding_coin_id = ?1 AND state_sequence = ?2
               AND recipient_id = ?3 AND idempotency_key = ?4",
            params![
                funding_coin_id.as_slice(),
                to_i64(state_sequence)?,
                recipient_id,
                idempotency_key,
            ],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                ))
            },
        )
        .optional()?;
    raw.map(|raw| delivery_from_raw(*funding_coin_id, state_sequence, raw))
        .transpose()
}

fn delivery_from_raw(
    funding_coin_id: Bytes32,
    state_sequence: u64,
    raw: RawRecoveryDelivery,
) -> Result<RecoveryDelivery> {
    Ok(RecoveryDelivery {
        funding_coin_id,
        state_sequence,
        checkpoint_hash: bytes32(raw.0, "delivery checkpoint hash")?,
        recovery_package_content_hash: bytes32(raw.1, "delivery package content hash")?,
        recipient_id: raw.2,
        recipient_kind: raw.3,
        idempotency_key: raw.4,
        status: RecoveryDeliveryStatus::parse(&raw.5)?,
        attempt_count: from_i64(raw.6, "delivery attempt count")?,
        last_error: raw.7,
        created_at: from_i64(raw.8, "delivery created_at")?,
        updated_at: from_i64(raw.9, "delivery updated_at")?,
    })
}

fn validate_delivery_text(field: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        Err(HubError::Invalid(format!(
            "{field} must contain 1..=256 non-control bytes"
        )))
    } else {
        Ok(())
    }
}

fn require_hub_key(terms: &ChannelTerms, secret_key: &SecretKey) -> Result<()> {
    if public_key_bytes(secret_key) == terms.hub_state_public_key_a {
        Ok(())
    } else {
        Err(HubError::HubKeyMismatch)
    }
}

fn checked_height(field: &'static str, value: u64) -> Result<u64> {
    if value <= MAX_PROTOCOL_U64 {
        Ok(value)
    } else {
        Err(ProtocolError::IntegerRange { field }.into())
    }
}

fn confirmation_depth(peak_height: u64, birth_height: u64) -> u64 {
    peak_height
        .checked_sub(birth_height)
        .and_then(|depth| depth.checked_add(1))
        .unwrap_or(0)
}

fn checked_add_height(left: u64, right: u64, field: &'static str) -> Result<u64> {
    let value = left
        .checked_add(right)
        .ok_or(ProtocolError::ArithmeticOverflow(field))?;
    checked_height(field, value)
}

fn to_i64(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| {
        ProtocolError::IntegerRange {
            field: "sqlite u64",
        }
        .into()
    })
}

fn from_i64(value: i64, field: &'static str) -> Result<u64> {
    u64::try_from(value).map_err(|_| HubError::Corrupt(field.into()))
}

fn bytes32(value: Vec<u8>, field: &'static str) -> Result<Bytes32> {
    value
        .try_into()
        .map_err(|_| HubError::Corrupt(field.into()))
}

fn signature96(value: Vec<u8>, field: &'static str) -> Result<SignatureBytes> {
    value
        .try_into()
        .map_err(|_| HubError::Corrupt(field.into()))
}

fn program_tree_hash(program: &[u8]) -> Result<Bytes32> {
    let mut allocator = Allocator::new();
    let node = node_from_bytes(&mut allocator, program)
        .map_err(|error| HubError::Invalid(format!("funding puzzle reveal: {error:?}")))?;
    Ok(tree_hash(&allocator, node).to_bytes())
}

#[allow(dead_code)]
fn _transaction_type_check(_: &Transaction<'_>) {}
