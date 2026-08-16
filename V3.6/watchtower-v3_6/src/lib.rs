use std::{path::Path, time::Duration};

use chia_bls::SecretKey;
use clvmr::{Allocator, serde::node_from_bytes};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use thiserror::Error;
use xhub_protocol_v3_6::{
    Bytes32, CanonicalDecode, CanonicalEncode, DeliveryConfirmation, ProtocolError, PublicKeyBytes,
    RecoveryPackage, SignatureBytes, parse_public_key, parse_signature, public_key_bytes,
    sign_hash, verify_hash,
};

pub mod api;
pub mod approval;
pub mod audit;
pub mod authorization;
pub mod backup;
pub mod bundle;
pub mod custody;
pub mod final_recheck;
pub mod manifest;
pub mod monitor;
pub mod preparation;
pub mod rpc;

pub use custody::{
    CustodyAttestation, ProductionGreenlightStatus, SignedCustodyAttestation,
    SingleVpsTestGreenlightStatus,
};

#[derive(Debug, Error)]
pub enum WatchtowerError {
    #[error(transparent)]
    Database(#[from] rusqlite::Error),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error("invalid watchtower input: {0}")]
    Invalid(String),
    #[error("RecoveryPackage conflicts with the accepted state at the same sequence")]
    StateConflict,
    #[error("a lower RecoveryPackage sequence cannot replace the latest accepted state")]
    StalePackage,
    #[error("RecoveryPackage was not found")]
    PackageNotFound,
    #[error("the requested ledger entry was not found in the RecoveryPackage")]
    EntryNotFound,
    #[error("DeliveryConfirmation is not bound to the accepted RecoveryPackage")]
    ConfirmationMismatch,
    #[error("DeliveryConfirmation signature is invalid")]
    InvalidConfirmationSignature,
    #[error("confirmer registration conflicts with persisted data")]
    ConfirmerConflict,
    #[error("custody attester registration conflicts with persisted data")]
    AttesterConflict,
    #[error("a merchant DeliveryConfirmation is required before custody attestation")]
    MerchantConfirmationRequired,
    #[error("custody attestation is not bound to the accepted delivery")]
    CustodyAttestationMismatch,
    #[error("custody attestation signature is invalid")]
    InvalidCustodyAttestationSignature,
    #[error("execution audit anchor conflicts with persisted data")]
    AuditAnchorConflict,
    #[error("a signer may only contribute one confirmation per delivery binding")]
    DuplicateSigner,
    #[error("an attester may only contribute one custody attestation per delivery binding")]
    DuplicateAttester,
    #[error("persisted watchtower data is corrupt: {0}")]
    Corrupt(String),
}

pub type Result<T> = std::result::Result<T, WatchtowerError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedPackage {
    pub funding_coin_id: Bytes32,
    pub state_sequence: u64,
    pub checkpoint_hash: Bytes32,
    pub recovery_package_content_hash: Bytes32,
    pub entry_count: u64,
    pub received_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantinedPackage {
    pub content_hash: Bytes32,
    pub funding_coin_id: Option<Bytes32>,
    pub state_sequence: Option<u64>,
    pub reason_code: String,
    pub reason: String,
    pub received_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedDeliveryConfirmation {
    pub confirmation: DeliveryConfirmation,
    pub signer_id: String,
    pub failure_domain: String,
    pub signer_public_key: PublicKeyBytes,
    pub signature: SignatureBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreenlightStatus {
    pub funding_coin_id: Bytes32,
    pub state_sequence: u64,
    pub checkpoint_hash: Bytes32,
    pub recovery_package_content_hash: Bytes32,
    pub entry_index: u64,
    pub authorization_hash: Bytes32,
    pub threshold: u16,
    pub signer_count: u16,
    pub failure_domain_count: u16,
    pub delivered: bool,
}

pub struct WatchtowerStore {
    pub(crate) connection: Connection,
}

impl WatchtowerStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::initialize(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(mut connection: Connection) -> Result<Self> {
        connection.busy_timeout(Duration::from_secs(10))?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             CREATE TABLE IF NOT EXISTS v36_watchtower_packages (
                funding_coin_id BLOB NOT NULL CHECK(length(funding_coin_id) = 32),
                state_sequence INTEGER NOT NULL CHECK(state_sequence > 0),
                checkpoint_hash BLOB NOT NULL CHECK(length(checkpoint_hash) = 32),
                recovery_package_content_hash BLOB NOT NULL UNIQUE
                  CHECK(length(recovery_package_content_hash) = 32),
                package_blob BLOB NOT NULL,
                entry_count INTEGER NOT NULL CHECK(entry_count > 0),
                received_at INTEGER NOT NULL,
                PRIMARY KEY(funding_coin_id, state_sequence),
                UNIQUE(funding_coin_id, checkpoint_hash)
             );
             CREATE TABLE IF NOT EXISTS v36_watchtower_heads (
                funding_coin_id BLOB PRIMARY KEY CHECK(length(funding_coin_id) = 32),
                latest_valid_sequence INTEGER NOT NULL CHECK(latest_valid_sequence > 0),
                latest_checkpoint_hash BLOB NOT NULL CHECK(length(latest_checkpoint_hash) = 32),
                recovery_package_content_hash BLOB NOT NULL CHECK(length(recovery_package_content_hash) = 32),
                updated_at INTEGER NOT NULL,
                FOREIGN KEY(funding_coin_id, latest_valid_sequence)
                  REFERENCES v36_watchtower_packages(funding_coin_id, state_sequence)
             );
             CREATE TABLE IF NOT EXISTS v36_watchtower_quarantine (
                content_hash BLOB PRIMARY KEY CHECK(length(content_hash) = 32),
                funding_coin_id BLOB CHECK(funding_coin_id IS NULL OR length(funding_coin_id) = 32),
                state_sequence INTEGER,
                package_blob BLOB NOT NULL,
                reason_code TEXT NOT NULL,
                reason TEXT NOT NULL,
                received_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS v36_watchtower_confirmers (
                signer_id TEXT PRIMARY KEY,
                failure_domain TEXT NOT NULL,
                signer_public_key BLOB NOT NULL CHECK(length(signer_public_key) = 48),
                active INTEGER NOT NULL CHECK(active IN (0, 1)),
                created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS v36_delivery_confirmations (
                funding_coin_id BLOB NOT NULL CHECK(length(funding_coin_id) = 32),
                state_sequence INTEGER NOT NULL CHECK(state_sequence > 0),
                checkpoint_hash BLOB NOT NULL CHECK(length(checkpoint_hash) = 32),
                recovery_package_content_hash BLOB NOT NULL CHECK(length(recovery_package_content_hash) = 32),
                entry_index INTEGER NOT NULL CHECK(entry_index >= 0),
                authorization_hash BLOB NOT NULL CHECK(length(authorization_hash) = 32),
                signer_id TEXT NOT NULL,
                failure_domain TEXT NOT NULL,
                signer_public_key BLOB NOT NULL CHECK(length(signer_public_key) = 48),
                confirmation_blob BLOB NOT NULL,
                signature BLOB NOT NULL CHECK(length(signature) = 96),
                received_at INTEGER NOT NULL,
                PRIMARY KEY(funding_coin_id, state_sequence, entry_index, signer_id),
                FOREIGN KEY(funding_coin_id, state_sequence)
                  REFERENCES v36_watchtower_packages(funding_coin_id, state_sequence),
                FOREIGN KEY(signer_id) REFERENCES v36_watchtower_confirmers(signer_id)
             );
             CREATE TABLE IF NOT EXISTS v36_chain_monitor_state (
                funding_coin_id BLOB PRIMARY KEY CHECK(length(funding_coin_id) = 32),
                peak_height INTEGER,
                peak_header_hash BLOB CHECK(peak_header_hash IS NULL OR length(peak_header_hash) = 32),
                action TEXT NOT NULL,
                detail TEXT NOT NULL,
                updated_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS v36_challenge_plans (
                closing_coin_id BLOB PRIMARY KEY CHECK(length(closing_coin_id) = 32),
                funding_coin_id BLOB NOT NULL CHECK(length(funding_coin_id) = 32),
                current_state_sequence INTEGER NOT NULL CHECK(current_state_sequence >= 0),
                latest_state_sequence INTEGER NOT NULL CHECK(latest_state_sequence > current_state_sequence),
                challenge_deadline_height INTEGER NOT NULL CHECK(challenge_deadline_height > 0),
                simulation_json TEXT NOT NULL,
                status TEXT NOT NULL,
                attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count >= 0),
                next_retry_height INTEGER,
                last_error TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                FOREIGN KEY(funding_coin_id, latest_state_sequence)
                  REFERENCES v36_watchtower_packages(funding_coin_id, state_sequence)
             );
             CREATE TABLE IF NOT EXISTS v36_offline_challenge_preparations (
                closing_coin_id BLOB PRIMARY KEY CHECK(length(closing_coin_id) = 32),
                funding_coin_id BLOB NOT NULL CHECK(length(funding_coin_id) = 32),
                peak_height INTEGER NOT NULL CHECK(peak_height > 0),
                peak_header_hash BLOB NOT NULL CHECK(length(peak_header_hash) = 32),
                closing_parent_coin_id BLOB NOT NULL CHECK(length(closing_parent_coin_id) = 32),
                closing_puzzle_hash BLOB NOT NULL CHECK(length(closing_puzzle_hash) = 32),
                closing_amount INTEGER NOT NULL CHECK(closing_amount > 0),
                closing_birth_height INTEGER NOT NULL CHECK(closing_birth_height > 0),
                challenge_deadline_height INTEGER NOT NULL CHECK(challenge_deadline_height > 0),
                fee_coin_id BLOB NOT NULL CHECK(length(fee_coin_id) = 32),
                fee_mojo INTEGER NOT NULL CHECK(fee_mojo > 0),
                report_json TEXT NOT NULL,
                bundle_commitment BLOB CHECK(bundle_commitment IS NULL OR length(bundle_commitment) = 32),
                status TEXT NOT NULL,
                invalidation_reason TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                FOREIGN KEY(closing_coin_id) REFERENCES v36_challenge_plans(closing_coin_id)
             );
             CREATE TABLE IF NOT EXISTS v36_challenge_approvals (
                preparation_id BLOB NOT NULL CHECK(length(preparation_id) = 32),
                closing_coin_id BLOB NOT NULL CHECK(length(closing_coin_id) = 32),
                approver_id TEXT NOT NULL,
                failure_domain TEXT NOT NULL,
                approver_public_key BLOB NOT NULL CHECK(length(approver_public_key) = 48),
                decision INTEGER NOT NULL CHECK(decision IN (1, 2)),
                issued_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL CHECK(expires_at > issued_at),
                nonce BLOB NOT NULL CHECK(length(nonce) = 32),
                statement_blob BLOB NOT NULL,
                signature BLOB NOT NULL CHECK(length(signature) = 96),
                status TEXT NOT NULL,
                received_at INTEGER NOT NULL,
                revoked_at INTEGER,
                revocation_reason TEXT,
                PRIMARY KEY(preparation_id, approver_id),
                UNIQUE(preparation_id, approver_public_key),
                UNIQUE(preparation_id, nonce),
                FOREIGN KEY(closing_coin_id)
                  REFERENCES v36_offline_challenge_preparations(closing_coin_id)
             );
             CREATE TABLE IF NOT EXISTS v36_final_chain_rechecks (
                recheck_id BLOB PRIMARY KEY CHECK(length(recheck_id) = 32),
                preparation_id BLOB NOT NULL CHECK(length(preparation_id) = 32),
                closing_coin_id BLOB NOT NULL CHECK(length(closing_coin_id) = 32),
                funding_coin_id BLOB NOT NULL CHECK(length(funding_coin_id) = 32),
                fee_coin_id BLOB NOT NULL CHECK(length(fee_coin_id) = 32),
                report_hash BLOB NOT NULL CHECK(length(report_hash) = 32),
                bundle_commitment BLOB CHECK(bundle_commitment IS NULL OR length(bundle_commitment) = 32),
                approval_set_hash BLOB NOT NULL CHECK(length(approval_set_hash) = 32),
                peak_height INTEGER NOT NULL CHECK(peak_height > 0),
                peak_header_hash BLOB NOT NULL CHECK(length(peak_header_hash) = 32),
                challenge_deadline_height INTEGER NOT NULL CHECK(challenge_deadline_height > peak_height),
                performed_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL CHECK(expires_at > performed_at),
                status TEXT NOT NULL,
                invalidated_at INTEGER,
                invalidation_reason TEXT,
                broadcast_enabled INTEGER NOT NULL CHECK(broadcast_enabled = 0),
                broadcast_ready INTEGER NOT NULL CHECK(broadcast_ready = 0),
                chain_broadcast INTEGER NOT NULL CHECK(chain_broadcast = 0),
                FOREIGN KEY(closing_coin_id)
                  REFERENCES v36_offline_challenge_preparations(closing_coin_id)
             );
             CREATE TABLE IF NOT EXISTS v36_execution_manifests (
                manifest_id BLOB PRIMARY KEY CHECK(length(manifest_id) = 32),
                recheck_id BLOB NOT NULL CHECK(length(recheck_id) = 32),
                preparation_id BLOB NOT NULL CHECK(length(preparation_id) = 32),
                closing_coin_id BLOB NOT NULL CHECK(length(closing_coin_id) = 32),
                funding_coin_id BLOB NOT NULL CHECK(length(funding_coin_id) = 32),
                fee_coin_id BLOB NOT NULL CHECK(length(fee_coin_id) = 32),
                report_hash BLOB NOT NULL CHECK(length(report_hash) = 32),
                bundle_commitment BLOB NOT NULL CHECK(length(bundle_commitment) = 32),
                approval_set_hash BLOB NOT NULL CHECK(length(approval_set_hash) = 32),
                peak_height INTEGER NOT NULL CHECK(peak_height > 0),
                peak_header_hash BLOB NOT NULL CHECK(length(peak_header_hash) = 32),
                challenge_deadline_height INTEGER NOT NULL CHECK(challenge_deadline_height > peak_height),
                issued_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL CHECK(expires_at > issued_at),
                status TEXT NOT NULL,
                invalidated_at INTEGER,
                invalidation_reason TEXT,
                broadcast_enabled INTEGER NOT NULL CHECK(broadcast_enabled = 0),
                broadcast_ready INTEGER NOT NULL CHECK(broadcast_ready = 0),
                chain_broadcast INTEGER NOT NULL CHECK(chain_broadcast = 0),
                FOREIGN KEY(recheck_id) REFERENCES v36_final_chain_rechecks(recheck_id)
             );
             CREATE TABLE IF NOT EXISTS v36_execution_authorizations (
                authorization_id BLOB PRIMARY KEY CHECK(length(authorization_id) = 32),
                manifest_id BLOB NOT NULL CHECK(length(manifest_id) = 32),
                recheck_id BLOB NOT NULL CHECK(length(recheck_id) = 32),
                preparation_id BLOB NOT NULL CHECK(length(preparation_id) = 32),
                closing_coin_id BLOB NOT NULL CHECK(length(closing_coin_id) = 32),
                funding_coin_id BLOB NOT NULL CHECK(length(funding_coin_id) = 32),
                fee_coin_id BLOB NOT NULL CHECK(length(fee_coin_id) = 32),
                report_hash BLOB NOT NULL CHECK(length(report_hash) = 32),
                bundle_commitment BLOB NOT NULL CHECK(length(bundle_commitment) = 32),
                approval_set_hash BLOB NOT NULL CHECK(length(approval_set_hash) = 32),
                peak_height INTEGER NOT NULL CHECK(peak_height > 0),
                peak_header_hash BLOB NOT NULL CHECK(length(peak_header_hash) = 32),
                challenge_deadline_height INTEGER NOT NULL CHECK(challenge_deadline_height > peak_height),
                issued_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL CHECK(expires_at > issued_at),
                status TEXT NOT NULL,
                invalidated_at INTEGER,
                invalidation_reason TEXT,
                simulated_submission_count INTEGER NOT NULL DEFAULT 0 CHECK(simulated_submission_count >= 0),
                last_simulated_at INTEGER,
                broadcast_enabled INTEGER NOT NULL CHECK(broadcast_enabled = 0),
                broadcast_ready INTEGER NOT NULL CHECK(broadcast_ready = 0),
                chain_broadcast INTEGER NOT NULL CHECK(chain_broadcast = 0),
                FOREIGN KEY(manifest_id) REFERENCES v36_execution_manifests(manifest_id)
             );
             CREATE TABLE IF NOT EXISTS v36_simulated_submission_receipts (
                receipt_id BLOB PRIMARY KEY CHECK(length(receipt_id) = 32),
                authorization_id BLOB NOT NULL UNIQUE CHECK(length(authorization_id) = 32),
                manifest_id BLOB NOT NULL UNIQUE CHECK(length(manifest_id) = 32),
                bundle_commitment BLOB NOT NULL CHECK(length(bundle_commitment) = 32),
                submission_nonce BLOB NOT NULL UNIQUE CHECK(length(submission_nonce) = 32),
                consumed_at INTEGER NOT NULL,
                status TEXT NOT NULL,
                broadcast_enabled INTEGER NOT NULL CHECK(broadcast_enabled = 0),
                broadcast_ready INTEGER NOT NULL CHECK(broadcast_ready = 0),
                chain_broadcast INTEGER NOT NULL CHECK(chain_broadcast = 0),
                FOREIGN KEY(authorization_id) REFERENCES v36_execution_authorizations(authorization_id)
             );
             CREATE TABLE IF NOT EXISTS v36_execution_audit_heads (
                singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                event_count INTEGER NOT NULL CHECK(event_count >= 0),
                head_hash BLOB NOT NULL CHECK(length(head_hash) = 32)
             );
             CREATE TABLE IF NOT EXISTS v36_execution_audit_events (
                event_index INTEGER PRIMARY KEY CHECK(event_index > 0),
                event_hash BLOB NOT NULL UNIQUE CHECK(length(event_hash) = 32),
                previous_hash BLOB NOT NULL CHECK(length(previous_hash) = 32),
                event_type TEXT NOT NULL,
                subject_id BLOB NOT NULL CHECK(length(subject_id) = 32),
                binding_hash BLOB NOT NULL CHECK(length(binding_hash) = 32),
                status TEXT NOT NULL,
                occurred_at INTEGER NOT NULL,
                broadcast_enabled INTEGER NOT NULL CHECK(broadcast_enabled = 0),
                broadcast_ready INTEGER NOT NULL CHECK(broadcast_ready = 0),
                chain_broadcast INTEGER NOT NULL CHECK(chain_broadcast = 0)
             );
             CREATE TABLE IF NOT EXISTS v36_execution_audit_anchors (
                anchor_id BLOB PRIMARY KEY CHECK(length(anchor_id) = 32),
                event_count INTEGER NOT NULL CHECK(event_count >= 0),
                head_hash BLOB NOT NULL CHECK(length(head_hash) = 32),
                anchored_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS v36_backup_artifact_handoffs (
                artifact_hash BLOB PRIMARY KEY CHECK(length(artifact_hash) = 32),
                backup_id BLOB NOT NULL UNIQUE CHECK(length(backup_id) = 32),
                envelope_hash BLOB NOT NULL CHECK(length(envelope_hash) = 32),
                key_id BLOB NOT NULL CHECK(length(key_id) = 32),
                manifest_bytes_hash BLOB NOT NULL CHECK(length(manifest_bytes_hash) = 32),
                received_at INTEGER NOT NULL,
                verified_at INTEGER,
                status TEXT NOT NULL CHECK(status IN ('RECEIVED', 'VERIFIED', 'REJECTED')),
                rejection_reason TEXT
             );
             CREATE TABLE IF NOT EXISTS v36_backup_restore_drills (
                drill_id BLOB PRIMARY KEY CHECK(length(drill_id) = 32),
                artifact_hash BLOB NOT NULL CHECK(length(artifact_hash) = 32),
                backup_id BLOB NOT NULL CHECK(length(backup_id) = 32),
                started_at INTEGER NOT NULL,
                completed_at INTEGER NOT NULL CHECK(completed_at >= started_at),
                duration_seconds INTEGER NOT NULL CHECK(duration_seconds >= 0),
                hash_matches INTEGER NOT NULL CHECK(hash_matches IN (0, 1)),
                size_matches INTEGER NOT NULL CHECK(size_matches IN (0, 1)),
                audit_valid INTEGER NOT NULL CHECK(audit_valid IN (0, 1)),
                anchor_valid INTEGER CHECK(anchor_valid IS NULL OR anchor_valid IN (0, 1)),
                status TEXT NOT NULL CHECK(status IN ('PASSED', 'FAILED')),
                failure_reason TEXT,
                FOREIGN KEY(artifact_hash) REFERENCES v36_backup_artifact_handoffs(artifact_hash)
             );",
        )?;
        connection.execute(
            "INSERT OR IGNORE INTO v36_execution_audit_heads (singleton, event_count, head_hash)
             VALUES (1, 0, ?1)",
            [audit::execution_audit_genesis_hash().as_slice()],
        )?;
        migrate_state_zero_challenge_plans(&mut connection)?;
        migrate_bundle_commitment_columns(&connection)?;
        custody::initialize_schema(&connection)?;
        Ok(Self { connection })
    }

    pub fn accept_package(&mut self, bytes: &[u8], now: u64) -> Result<AcceptedPackage> {
        let package = match RecoveryPackage::from_canonical_bytes(bytes) {
            Ok(package) => package,
            Err(error) => {
                self.quarantine(
                    bytes,
                    None,
                    None,
                    "INVALID_ENCODING",
                    &error.to_string(),
                    now,
                )?;
                return Err(error.into());
            }
        };
        let funding_coin_id = package.funding_coin_id;
        let state_sequence = package.official_state.checkpoint.state_sequence;
        if let Err(error) = package.validate() {
            self.quarantine(
                bytes,
                Some(funding_coin_id),
                Some(state_sequence),
                "INVALID_PACKAGE",
                &error.to_string(),
                now,
            )?;
            return Err(error.into());
        }
        if let Err(error) = validate_funding_puzzle_reveal(&package.funding_puzzle_reveal) {
            self.quarantine(
                bytes,
                Some(funding_coin_id),
                Some(state_sequence),
                "INVALID_FUNDING_PUZZLE",
                &error.to_string(),
                now,
            )?;
            return Err(error);
        }
        let checkpoint_hash = package
            .official_state
            .checkpoint
            .hash(&package.channel_terms)?;
        let content_hash = package.content_hash()?;
        let accepted = AcceptedPackage {
            funding_coin_id,
            state_sequence,
            checkpoint_hash,
            recovery_package_content_hash: content_hash,
            entry_count: package.entries.len() as u64,
            received_at: now,
        };

        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let same_sequence = tx
            .query_row(
                "SELECT checkpoint_hash, recovery_package_content_hash
                 FROM v36_watchtower_packages
                 WHERE funding_coin_id = ?1 AND state_sequence = ?2",
                params![funding_coin_id.as_slice(), to_i64(state_sequence)?],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()?;
        if let Some((existing_checkpoint, existing_content)) = same_sequence {
            if bytes32(existing_checkpoint, "checkpoint hash")? == checkpoint_hash
                && bytes32(existing_content, "content hash")? == content_hash
            {
                tx.commit()?;
                return Ok(accepted);
            }
            drop(tx);
            self.quarantine(
                bytes,
                Some(funding_coin_id),
                Some(state_sequence),
                "STATE_CONFLICT",
                "same sequence has different checkpoint or content hash",
                now,
            )?;
            return Err(WatchtowerError::StateConflict);
        }
        let head = tx
            .query_row(
                "SELECT latest_valid_sequence FROM v36_watchtower_heads WHERE funding_coin_id = ?1",
                [funding_coin_id.as_slice()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if head.is_some_and(|latest| latest >= to_i64(state_sequence).unwrap_or(i64::MAX)) {
            drop(tx);
            self.quarantine(
                bytes,
                Some(funding_coin_id),
                Some(state_sequence),
                "STALE_PACKAGE",
                "package sequence is lower than the latest accepted state",
                now,
            )?;
            return Err(WatchtowerError::StalePackage);
        }
        if let Some(latest) = head {
            let previous_bytes: Vec<u8> = tx.query_row(
                "SELECT package_blob FROM v36_watchtower_packages
                 WHERE funding_coin_id = ?1 AND state_sequence = ?2",
                params![funding_coin_id.as_slice(), latest],
                |row| row.get(0),
            )?;
            let previous = RecoveryPackage::from_canonical_bytes(&previous_bytes)
                .map_err(|error| WatchtowerError::Corrupt(error.to_string()))?;
            let old_len = previous.entries.len();
            let append_only = package.entries.get(..old_len) == Some(previous.entries.as_slice())
                && package.user_authorization_signatures.get(..old_len)
                    == Some(previous.user_authorization_signatures.as_slice());
            let latest_sequence = from_i64(latest, "latest sequence")?;
            let adjacent_previous_matches = state_sequence != latest_sequence + 1
                || package.official_state.checkpoint.previous_checkpoint_hash
                    == previous
                        .official_state
                        .checkpoint
                        .hash(&previous.channel_terms)?;
            if !append_only || !adjacent_previous_matches {
                drop(tx);
                self.quarantine(
                    bytes,
                    Some(funding_coin_id),
                    Some(state_sequence),
                    "APPEND_ONLY_VIOLATION",
                    "higher package modified prior ledger content or broke adjacent linkage",
                    now,
                )?;
                return Err(WatchtowerError::StateConflict);
            }
        }
        tx.execute(
            "INSERT INTO v36_watchtower_packages (
               funding_coin_id, state_sequence, checkpoint_hash,
               recovery_package_content_hash, package_blob, entry_count, received_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                funding_coin_id.as_slice(),
                to_i64(state_sequence)?,
                checkpoint_hash.as_slice(),
                content_hash.as_slice(),
                bytes,
                to_i64(accepted.entry_count)?,
                to_i64(now)?,
            ],
        )?;
        tx.execute(
            "INSERT INTO v36_watchtower_heads (
               funding_coin_id, latest_valid_sequence, latest_checkpoint_hash,
               recovery_package_content_hash, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(funding_coin_id) DO UPDATE SET
               latest_valid_sequence = excluded.latest_valid_sequence,
               latest_checkpoint_hash = excluded.latest_checkpoint_hash,
               recovery_package_content_hash = excluded.recovery_package_content_hash,
               updated_at = excluded.updated_at",
            params![
                funding_coin_id.as_slice(),
                to_i64(state_sequence)?,
                checkpoint_hash.as_slice(),
                content_hash.as_slice(),
                to_i64(now)?,
            ],
        )?;
        tx.commit()?;
        Ok(accepted)
    }

    pub fn package(
        &self,
        funding_coin_id: Bytes32,
        state_sequence: u64,
    ) -> Result<RecoveryPackage> {
        let bytes = self
            .connection
            .query_row(
                "SELECT package_blob FROM v36_watchtower_packages
                 WHERE funding_coin_id = ?1 AND state_sequence = ?2",
                params![funding_coin_id.as_slice(), to_i64(state_sequence)?],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?
            .ok_or(WatchtowerError::PackageNotFound)?;
        let package = RecoveryPackage::from_canonical_bytes(&bytes)
            .map_err(|error| WatchtowerError::Corrupt(error.to_string()))?;
        package
            .validate()
            .map_err(|error| WatchtowerError::Corrupt(error.to_string()))?;
        Ok(package)
    }

    pub fn latest_package(&self, funding_coin_id: Bytes32) -> Result<RecoveryPackage> {
        let sequence = self
            .connection
            .query_row(
                "SELECT latest_valid_sequence FROM v36_watchtower_heads WHERE funding_coin_id = ?1",
                [funding_coin_id.as_slice()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .ok_or(WatchtowerError::PackageNotFound)?;
        self.package(funding_coin_id, from_i64(sequence, "latest sequence")?)
    }

    pub fn quarantined(&self) -> Result<Vec<QuarantinedPackage>> {
        let mut statement = self.connection.prepare(
            "SELECT content_hash, funding_coin_id, state_sequence, reason_code, reason, received_at
             FROM v36_watchtower_quarantine ORDER BY received_at, content_hash",
        )?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Option<Vec<u8>>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })?
            .map(|row| {
                let row = row?;
                Ok(QuarantinedPackage {
                    content_hash: bytes32(row.0, "quarantine content hash")?,
                    funding_coin_id: row
                        .1
                        .map(|value| bytes32(value, "quarantine funding coin id"))
                        .transpose()?,
                    state_sequence: row
                        .2
                        .map(|value| from_i64(value, "quarantine state sequence"))
                        .transpose()?,
                    reason_code: row.3,
                    reason: row.4,
                    received_at: from_i64(row.5, "quarantine received_at")?,
                })
            })
            .collect()
    }

    pub fn register_confirmer(
        &mut self,
        signer_id: &str,
        failure_domain: &str,
        signer_public_key: PublicKeyBytes,
        now: u64,
    ) -> Result<()> {
        validate_name("signer_id", signer_id)?;
        validate_name("failure_domain", failure_domain)?;
        parse_public_key(&signer_public_key)?;
        let changed = self.connection.execute(
            "INSERT OR IGNORE INTO v36_watchtower_confirmers (
               signer_id, failure_domain, signer_public_key, active, created_at
             ) VALUES (?1, ?2, ?3, 1, ?4)",
            params![
                signer_id,
                failure_domain,
                signer_public_key.as_slice(),
                to_i64(now)?
            ],
        )?;
        if changed == 0 {
            let existing = self.connection.query_row(
                "SELECT failure_domain, signer_public_key FROM v36_watchtower_confirmers
                 WHERE signer_id = ?1",
                [signer_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )?;
            if existing.0 != failure_domain
                || public_key(existing.1, "confirmer public key")? != signer_public_key
            {
                return Err(WatchtowerError::ConfirmerConflict);
            }
        }
        Ok(())
    }

    pub fn sign_confirmation(
        &self,
        funding_coin_id: Bytes32,
        state_sequence: u64,
        entry_index: u64,
        signer_id: &str,
        signer_secret_key: &SecretKey,
    ) -> Result<SignedDeliveryConfirmation> {
        let (failure_domain, signer_public_key) = self.load_confirmer(signer_id)?;
        if public_key_bytes(signer_secret_key) != signer_public_key {
            return Err(WatchtowerError::ConfirmerConflict);
        }
        let package = self.package(funding_coin_id, state_sequence)?;
        let expected_signer_key = receipt_public_key(&package, entry_index)?;
        if signer_public_key != expected_signer_key {
            return Err(WatchtowerError::ConfirmerConflict);
        }
        let confirmation = confirmation_for(&package, entry_index)?;
        let signature = sign_hash(signer_secret_key, &confirmation.hash()?);
        Ok(SignedDeliveryConfirmation {
            confirmation,
            signer_id: signer_id.to_string(),
            failure_domain,
            signer_public_key,
            signature,
        })
    }

    pub fn record_confirmation(
        &mut self,
        signed: &SignedDeliveryConfirmation,
        now: u64,
    ) -> Result<()> {
        validate_name("signer_id", &signed.signer_id)?;
        parse_signature(&signed.signature)?;
        let (failure_domain, signer_public_key) = self.load_confirmer(&signed.signer_id)?;
        if failure_domain != signed.failure_domain || signer_public_key != signed.signer_public_key
        {
            return Err(WatchtowerError::ConfirmerConflict);
        }
        let package = self.package(
            signed.confirmation.funding_coin_id,
            signed.confirmation.state_sequence,
        )?;
        if signed.signer_public_key
            != receipt_public_key(&package, signed.confirmation.entry_index)?
        {
            return Err(WatchtowerError::ConfirmerConflict);
        }
        if signed.confirmation != confirmation_for(&package, signed.confirmation.entry_index)? {
            return Err(WatchtowerError::ConfirmationMismatch);
        }
        verify_hash(
            &signed.signer_public_key,
            &signed.confirmation.hash()?,
            &signed.signature,
        )
        .map_err(|_| WatchtowerError::InvalidConfirmationSignature)?;
        let changed = self.connection.execute(
            "INSERT OR IGNORE INTO v36_delivery_confirmations (
               funding_coin_id, state_sequence, checkpoint_hash,
               recovery_package_content_hash, entry_index, authorization_hash,
               signer_id, failure_domain, signer_public_key,
               confirmation_blob, signature, received_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                signed.confirmation.funding_coin_id.as_slice(),
                to_i64(signed.confirmation.state_sequence)?,
                signed.confirmation.checkpoint_hash.as_slice(),
                signed.confirmation.recovery_package_content_hash.as_slice(),
                to_i64(signed.confirmation.entry_index)?,
                signed.confirmation.authorization_hash.as_slice(),
                signed.signer_id,
                signed.failure_domain,
                signed.signer_public_key.as_slice(),
                signed.confirmation.canonical_bytes(),
                signed.signature.as_slice(),
                to_i64(now)?,
            ],
        )?;
        if changed == 0 {
            let existing = self.connection.query_row(
                "SELECT confirmation_blob, signature FROM v36_delivery_confirmations
                 WHERE funding_coin_id = ?1 AND state_sequence = ?2
                   AND entry_index = ?3 AND signer_id = ?4",
                params![
                    signed.confirmation.funding_coin_id.as_slice(),
                    to_i64(signed.confirmation.state_sequence)?,
                    to_i64(signed.confirmation.entry_index)?,
                    signed.signer_id,
                ],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )?;
            if existing.0 != signed.confirmation.canonical_bytes()
                || signature(existing.1, "confirmation signature")? != signed.signature
            {
                return Err(WatchtowerError::DuplicateSigner);
            }
        }
        Ok(())
    }

    pub fn greenlight_status(
        &self,
        funding_coin_id: Bytes32,
        state_sequence: u64,
        entry_index: u64,
        threshold: u16,
    ) -> Result<GreenlightStatus> {
        if !(1..=3).contains(&threshold) {
            return Err(WatchtowerError::Invalid(
                "greenlight threshold must be 1, 2, or 3".into(),
            ));
        }
        let package = self.package(funding_coin_id, state_sequence)?;
        let expected = confirmation_for(&package, entry_index)?;
        let (signers, domains): (i64, i64) = self.connection.query_row(
            "SELECT COUNT(*), COUNT(DISTINCT failure_domain) FROM (
               SELECT signer_public_key, MIN(failure_domain) AS failure_domain
               FROM v36_delivery_confirmations
               WHERE funding_coin_id = ?1 AND state_sequence = ?2
                 AND checkpoint_hash = ?3 AND recovery_package_content_hash = ?4
                 AND entry_index = ?5 AND authorization_hash = ?6
               GROUP BY signer_public_key
             )",
            params![
                funding_coin_id.as_slice(),
                to_i64(state_sequence)?,
                expected.checkpoint_hash.as_slice(),
                expected.recovery_package_content_hash.as_slice(),
                to_i64(entry_index)?,
                expected.authorization_hash.as_slice(),
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let signer_count =
            u16::try_from(signers).map_err(|_| WatchtowerError::Corrupt("signer count".into()))?;
        let failure_domain_count = u16::try_from(domains)
            .map_err(|_| WatchtowerError::Corrupt("failure domain count".into()))?;
        Ok(GreenlightStatus {
            funding_coin_id,
            state_sequence,
            checkpoint_hash: expected.checkpoint_hash,
            recovery_package_content_hash: expected.recovery_package_content_hash,
            entry_index,
            authorization_hash: expected.authorization_hash,
            threshold,
            signer_count,
            failure_domain_count,
            delivered: signer_count >= threshold && failure_domain_count >= threshold,
        })
    }

    pub fn durability_mode(&self) -> Result<(String, i64)> {
        Ok((
            self.connection
                .query_row("PRAGMA journal_mode", [], |row| row.get(0))?,
            self.connection
                .query_row("PRAGMA synchronous", [], |row| row.get(0))?,
        ))
    }

    fn load_confirmer(&self, signer_id: &str) -> Result<(String, PublicKeyBytes)> {
        let value = self
            .connection
            .query_row(
                "SELECT failure_domain, signer_public_key FROM v36_watchtower_confirmers
                 WHERE signer_id = ?1 AND active = 1",
                [signer_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()?
            .ok_or_else(|| WatchtowerError::Invalid("unknown or inactive signer".into()))?;
        Ok((value.0, public_key(value.1, "confirmer public key")?))
    }

    fn quarantine(
        &mut self,
        bytes: &[u8],
        funding_coin_id: Option<Bytes32>,
        state_sequence: Option<u64>,
        reason_code: &str,
        reason: &str,
        now: u64,
    ) -> Result<()> {
        let content_hash =
            xhub_protocol_v3_6::sha256_parts(&[b"XHUB_QUARANTINED_PACKAGE_V3_6", bytes]);
        self.connection.execute(
            "INSERT OR IGNORE INTO v36_watchtower_quarantine (
               content_hash, funding_coin_id, state_sequence, package_blob,
               reason_code, reason, received_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                content_hash.as_slice(),
                funding_coin_id.map(|value| value.to_vec()),
                state_sequence.map(to_i64).transpose()?,
                bytes,
                reason_code,
                reason,
                to_i64(now)?,
            ],
        )?;
        Ok(())
    }
}

fn migrate_state_zero_challenge_plans(connection: &mut Connection) -> Result<()> {
    let schema = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'v36_challenge_plans'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if !schema.is_some_and(|sql| sql.contains("current_state_sequence > 0")) {
        return Ok(());
    }
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch(
        "CREATE TABLE v36_challenge_plans_v2 (
           closing_coin_id BLOB PRIMARY KEY CHECK(length(closing_coin_id) = 32),
           funding_coin_id BLOB NOT NULL CHECK(length(funding_coin_id) = 32),
           current_state_sequence INTEGER NOT NULL CHECK(current_state_sequence >= 0),
           latest_state_sequence INTEGER NOT NULL CHECK(latest_state_sequence > current_state_sequence),
           challenge_deadline_height INTEGER NOT NULL CHECK(challenge_deadline_height > 0),
           simulation_json TEXT NOT NULL,
           status TEXT NOT NULL,
           attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count >= 0),
           next_retry_height INTEGER,
           last_error TEXT,
           created_at INTEGER NOT NULL,
           updated_at INTEGER NOT NULL,
           FOREIGN KEY(funding_coin_id, latest_state_sequence)
             REFERENCES v36_watchtower_packages(funding_coin_id, state_sequence)
         );
         INSERT INTO v36_challenge_plans_v2
         SELECT * FROM v36_challenge_plans;
         DROP TABLE v36_challenge_plans;
         ALTER TABLE v36_challenge_plans_v2 RENAME TO v36_challenge_plans;",
    )?;
    tx.commit()?;
    Ok(())
}

fn migrate_bundle_commitment_columns(connection: &Connection) -> Result<()> {
    for (table, definition) in [
        (
            "v36_offline_challenge_preparations",
            "bundle_commitment BLOB CHECK(bundle_commitment IS NULL OR length(bundle_commitment) = 32)",
        ),
        (
            "v36_final_chain_rechecks",
            "bundle_commitment BLOB CHECK(bundle_commitment IS NULL OR length(bundle_commitment) = 32)",
        ),
    ] {
        let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if !columns.iter().any(|column| column == "bundle_commitment") {
            connection.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {definition}"))?;
        }
    }
    Ok(())
}

fn confirmation_for(package: &RecoveryPackage, entry_index: u64) -> Result<DeliveryConfirmation> {
    package.validate()?;
    let index = usize::try_from(entry_index)
        .map_err(|_| WatchtowerError::Invalid("entry_index is out of range".into()))?;
    let entry = package
        .entries
        .get(index)
        .ok_or(WatchtowerError::EntryNotFound)?;
    Ok(DeliveryConfirmation {
        network_id: package.channel_terms.network_id,
        funding_coin_id: package.funding_coin_id,
        channel_terms_hash: package.channel_terms.hash()?,
        state_sequence: package.official_state.checkpoint.state_sequence,
        checkpoint_hash: package
            .official_state
            .checkpoint
            .hash(&package.channel_terms)?,
        entry_index,
        authorization_hash: entry
            .authorization_hash(&package.channel_terms, &package.funding_coin_id)?,
        recovery_package_content_hash: package.content_hash()?,
    })
}

fn validate_funding_puzzle_reveal(bytes: &[u8]) -> Result<()> {
    let mut allocator = Allocator::new();
    node_from_bytes(&mut allocator, bytes)
        .map(|_| ())
        .map_err(|error| {
            WatchtowerError::Invalid(format!("invalid funding puzzle reveal: {error:?}"))
        })
}

fn receipt_public_key(package: &RecoveryPackage, entry_index: u64) -> Result<PublicKeyBytes> {
    let index = usize::try_from(entry_index)
        .map_err(|_| WatchtowerError::Invalid("entry_index is out of range".into()))?;
    package
        .entries
        .get(index)
        .map(|entry| entry.merchant_receipt_public_key)
        .ok_or(WatchtowerError::EntryNotFound)
}

fn validate_name(field: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        Err(WatchtowerError::Invalid(format!(
            "{field} must contain 1..=256 non-control bytes"
        )))
    } else {
        Ok(())
    }
}

fn to_i64(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| WatchtowerError::Invalid("u64 exceeds SQLite range".into()))
}

fn from_i64(value: i64, field: &str) -> Result<u64> {
    u64::try_from(value).map_err(|_| WatchtowerError::Corrupt(field.into()))
}

fn bytes32(value: Vec<u8>, field: &str) -> Result<Bytes32> {
    value
        .try_into()
        .map_err(|_| WatchtowerError::Corrupt(field.into()))
}

fn public_key(value: Vec<u8>, field: &str) -> Result<PublicKeyBytes> {
    value
        .try_into()
        .map_err(|_| WatchtowerError::Corrupt(field.into()))
}

fn signature(value: Vec<u8>, field: &str) -> Result<SignatureBytes> {
    value
        .try_into()
        .map_err(|_| WatchtowerError::Corrupt(field.into()))
}
