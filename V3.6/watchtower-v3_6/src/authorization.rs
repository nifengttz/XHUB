use rusqlite::{OptionalExtension, params};
use xhub_protocol_v3_6::{Bytes32, PROTOCOL_VERSION, sha256_parts};

use crate::{
    WatchtowerError, WatchtowerStore,
    approval::DUAL_APPROVED_RECHECK_REQUIRED,
    audit::append_execution_audit_event_in_transaction,
    final_recheck::FINAL_RECHECK_VERIFIED_NO_BROADCAST,
    manifest::{EXECUTION_MANIFEST_VERIFIED_NO_BROADCAST, ExecutionManifest},
};

pub const EXECUTION_AUTHORIZATION_DOMAIN: &[u8] = b"XHUB_EXECUTION_AUTHORIZATION_V3_6";
pub const EXECUTION_AUTHORIZED_SIMULATED_ONLY: &str = "EXECUTION_AUTHORIZED_SIMULATED_ONLY";
pub const EXECUTION_AUTHORIZATION_EXPIRED: &str = "EXECUTION_AUTHORIZATION_EXPIRED";
pub const EXECUTION_AUTHORIZATION_INVALIDATED: &str = "EXECUTION_AUTHORIZATION_INVALIDATED";
pub const EXECUTION_AUTHORIZATION_SUPERSEDED: &str = "EXECUTION_AUTHORIZATION_SUPERSEDED";
pub const EXECUTION_AUTHORIZATION_CONSUMED_SIMULATED_ONLY: &str =
    "EXECUTION_AUTHORIZATION_CONSUMED_SIMULATED_ONLY";
pub const EXECUTION_AUTHORIZATION_TTL_SECONDS: u64 = 5;
pub const SIMULATED_SUBMISSION_RECEIPT_DOMAIN: &[u8] = b"XHUB_SIMULATED_SUBMISSION_RECEIPT_V3_6";
pub const SIMULATED_SUBMISSION_RECORDED: &str = "SIMULATED_SUBMISSION_RECORDED";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionAuthorization {
    pub authorization_id: Bytes32,
    pub manifest_id: Bytes32,
    pub recheck_id: Bytes32,
    pub preparation_id: Bytes32,
    pub closing_coin_id: Bytes32,
    pub funding_coin_id: Bytes32,
    pub fee_coin_id: Bytes32,
    pub report_hash: Bytes32,
    pub bundle_commitment: Bytes32,
    pub approval_set_hash: Bytes32,
    pub peak_height: u64,
    pub peak_header_hash: Bytes32,
    pub challenge_deadline_height: u64,
    pub issued_at: u64,
    pub expires_at: u64,
    pub status: String,
    pub invalidation_reason: Option<String>,
    pub simulated_submission_count: u64,
    pub last_simulated_at: Option<u64>,
    pub broadcast_enabled: bool,
    pub broadcast_ready: bool,
    pub chain_broadcast: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulatedSubmissionReceipt {
    pub receipt_id: Bytes32,
    pub authorization_id: Bytes32,
    pub manifest_id: Bytes32,
    pub bundle_commitment: Bytes32,
    pub submission_nonce: Bytes32,
    pub consumed_at: u64,
    pub status: String,
    pub broadcast_enabled: bool,
    pub broadcast_ready: bool,
    pub chain_broadcast: bool,
}

impl WatchtowerStore {
    pub fn issue_execution_authorization(
        &self,
        manifest_id: Bytes32,
        now: u64,
    ) -> crate::Result<ExecutionAuthorization> {
        let manifest = self
            .execution_manifest(manifest_id, now)?
            .ok_or_else(|| WatchtowerError::Invalid("execution manifest was not found".into()))?;
        if manifest.status != EXECUTION_MANIFEST_VERIFIED_NO_BROADCAST {
            return Err(WatchtowerError::Invalid(format!(
                "execution manifest is not active: {}",
                manifest.status
            )));
        }
        if self
            .simulated_submission_receipt_for_manifest(manifest.manifest_id)?
            .is_some()
        {
            return Err(WatchtowerError::Invalid(
                "execution manifest was already consumed by a simulated submission".into(),
            ));
        }
        let recheck = self
            .final_chain_recheck(manifest.recheck_id, now)?
            .ok_or_else(|| {
                WatchtowerError::Corrupt("authorization recheck was not found".into())
            })?;
        let preparation = self
            .offline_preparation(manifest.closing_coin_id)?
            .ok_or_else(|| {
                WatchtowerError::Corrupt("authorization preparation was not found".into())
            })?;
        let approval = self.approval_status(manifest.closing_coin_id, now)?;
        let (approval_set_hash, _) = self.verified_approval_set(manifest.preparation_id, now)?;
        if recheck.status != FINAL_RECHECK_VERIFIED_NO_BROADCAST
            || recheck.preparation_id != manifest.preparation_id
            || recheck.closing_coin_id != manifest.closing_coin_id
            || recheck.funding_coin_id != manifest.funding_coin_id
            || recheck.fee_coin_id != manifest.fee_coin_id
            || recheck.report_hash != manifest.report_hash
            || recheck.bundle_commitment != manifest.bundle_commitment
            || recheck.approval_set_hash != manifest.approval_set_hash
            || recheck.peak_height != manifest.peak_height
            || recheck.peak_header_hash != manifest.peak_header_hash
            || recheck.challenge_deadline_height != manifest.challenge_deadline_height
            || approval.status != DUAL_APPROVED_RECHECK_REQUIRED
            || approval.preparation_id != manifest.preparation_id
            || approval_set_hash != manifest.approval_set_hash
            || preparation.funding_coin_id != manifest.funding_coin_id
            || preparation.fee_coin_id != manifest.fee_coin_id
            || preparation.report_hash != manifest.report_hash
            || preparation.bundle_commitment != manifest.bundle_commitment
            || preparation.snapshot.peak_height != manifest.peak_height
            || preparation.snapshot.peak_header_hash != manifest.peak_header_hash
            || preparation.challenge_deadline_height != manifest.challenge_deadline_height
        {
            return Err(WatchtowerError::Corrupt(
                "execution manifest no longer matches the final recheck".into(),
            ));
        }
        let expires_at = now
            .checked_add(EXECUTION_AUTHORIZATION_TTL_SECONDS)
            .ok_or_else(|| {
                WatchtowerError::Invalid("execution authorization expiry overflow".into())
            })?
            .min(manifest.expires_at);
        if expires_at <= now {
            return Err(WatchtowerError::Invalid(
                "execution manifest expires before authorization can be issued".into(),
            ));
        }
        let authorization_id = authorization_id(&manifest, now, expires_at);
        if let Some(existing) = self.execution_authorization(authorization_id, now)?
            && existing.status == EXECUTION_AUTHORIZED_SIMULATED_ONLY
        {
            return Ok(existing);
        }
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            "UPDATE v36_execution_authorizations SET status=?2, invalidated_at=?3,
               invalidation_reason='superseded by a newer execution authorization'
             WHERE manifest_id=?1 AND status=?4",
            params![
                manifest_id.as_slice(),
                EXECUTION_AUTHORIZATION_SUPERSEDED,
                super::to_i64(now)?,
                EXECUTION_AUTHORIZED_SIMULATED_ONLY
            ],
        )?;
        transaction.execute(
            "INSERT INTO v36_execution_authorizations (
               authorization_id, manifest_id, recheck_id, preparation_id, closing_coin_id,
               funding_coin_id, fee_coin_id, report_hash, bundle_commitment, approval_set_hash,
               peak_height, peak_header_hash, challenge_deadline_height, issued_at, expires_at,
               status, broadcast_enabled, broadcast_ready, chain_broadcast
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 0, 0, 0)",
            params![authorization_id.as_slice(), manifest.manifest_id.as_slice(), manifest.recheck_id.as_slice(),
                manifest.preparation_id.as_slice(), manifest.closing_coin_id.as_slice(), manifest.funding_coin_id.as_slice(),
                manifest.fee_coin_id.as_slice(), manifest.report_hash.as_slice(), manifest.bundle_commitment.as_slice(),
                manifest.approval_set_hash.as_slice(), super::to_i64(manifest.peak_height)?, manifest.peak_header_hash.as_slice(),
                super::to_i64(manifest.challenge_deadline_height)?, super::to_i64(now)?, super::to_i64(expires_at)?,
                EXECUTION_AUTHORIZED_SIMULATED_ONLY],
        )?;
        append_execution_audit_event_in_transaction(
            &transaction,
            "EXECUTION_AUTHORIZATION_ISSUED",
            authorization_id,
            manifest.bundle_commitment,
            EXECUTION_AUTHORIZED_SIMULATED_ONLY,
            now,
        )?;
        transaction.commit()?;
        self.execution_authorization(authorization_id, now)?
            .ok_or_else(|| {
                WatchtowerError::Corrupt("new execution authorization was not found".into())
            })
    }

    pub fn execution_authorization(
        &self,
        authorization_id: Bytes32,
        now: u64,
    ) -> crate::Result<Option<ExecutionAuthorization>> {
        self.connection.execute(
            "UPDATE v36_execution_authorizations SET status=?2, invalidated_at=?3,
               invalidation_reason='execution authorization validity window expired'
             WHERE authorization_id=?1 AND status=?4 AND expires_at<=?3",
            params![
                authorization_id.as_slice(),
                EXECUTION_AUTHORIZATION_EXPIRED,
                super::to_i64(now)?,
                EXECUTION_AUTHORIZED_SIMULATED_ONLY
            ],
        )?;
        let row = self
            .connection
            .query_row(
                "SELECT manifest_id, recheck_id, preparation_id, closing_coin_id, funding_coin_id,
                    fee_coin_id, report_hash, bundle_commitment, approval_set_hash, peak_height,
                    peak_header_hash, challenge_deadline_height, issued_at, expires_at, status,
                    invalidation_reason, simulated_submission_count, last_simulated_at,
                    broadcast_enabled, broadcast_ready, chain_broadcast
             FROM v36_execution_authorizations WHERE authorization_id=?1",
                [authorization_id.as_slice()],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                        row.get::<_, Vec<u8>>(5)?,
                        row.get::<_, Vec<u8>>(6)?,
                        row.get::<_, Vec<u8>>(7)?,
                        row.get::<_, Vec<u8>>(8)?,
                        row.get::<_, i64>(9)?,
                        row.get::<_, Vec<u8>>(10)?,
                        row.get::<_, i64>(11)?,
                        row.get::<_, i64>(12)?,
                        row.get::<_, i64>(13)?,
                        row.get::<_, String>(14)?,
                        row.get::<_, Option<String>>(15)?,
                        row.get::<_, i64>(16)?,
                        row.get::<_, Option<i64>>(17)?,
                        row.get::<_, bool>(18)?,
                        row.get::<_, bool>(19)?,
                        row.get::<_, bool>(20)?,
                    ))
                },
            )
            .optional()?;
        row.map(|r| {
            Ok(ExecutionAuthorization {
                authorization_id,
                manifest_id: super::bytes32(r.0, "authorization manifest ID")?,
                recheck_id: super::bytes32(r.1, "authorization recheck ID")?,
                preparation_id: super::bytes32(r.2, "authorization preparation ID")?,
                closing_coin_id: super::bytes32(r.3, "authorization Closing Coin ID")?,
                funding_coin_id: super::bytes32(r.4, "authorization Funding Coin ID")?,
                fee_coin_id: super::bytes32(r.5, "authorization fee Coin ID")?,
                report_hash: super::bytes32(r.6, "authorization report hash")?,
                bundle_commitment: super::bytes32(r.7, "authorization bundle commitment")?,
                approval_set_hash: super::bytes32(r.8, "authorization approval set hash")?,
                peak_height: super::from_i64(r.9, "authorization peak height")?,
                peak_header_hash: super::bytes32(r.10, "authorization peak header hash")?,
                challenge_deadline_height: super::from_i64(r.11, "authorization deadline")?,
                issued_at: super::from_i64(r.12, "authorization issued time")?,
                expires_at: super::from_i64(r.13, "authorization expiry")?,
                status: r.14,
                invalidation_reason: r.15,
                simulated_submission_count: super::from_i64(r.16, "simulation count")?,
                last_simulated_at: r
                    .17
                    .map(|v| super::from_i64(v, "last simulated time"))
                    .transpose()?,
                broadcast_enabled: r.18,
                broadcast_ready: r.19,
                chain_broadcast: r.20,
            })
        })
        .transpose()
    }

    pub fn simulate_execution_submission(
        &self,
        authorization_id: Bytes32,
        submission_nonce: Bytes32,
        now: u64,
    ) -> crate::Result<SimulatedSubmissionReceipt> {
        let authorization = self
            .execution_authorization(authorization_id, now)?
            .ok_or_else(|| {
                WatchtowerError::Invalid("execution authorization was not found".into())
            })?;
        if let Some(existing) =
            self.simulated_submission_receipt_for_manifest(authorization.manifest_id)?
        {
            if existing.authorization_id == authorization_id
                && existing.submission_nonce == submission_nonce
            {
                return Ok(existing);
            }
            return Err(WatchtowerError::Invalid(
                "execution manifest was already consumed by another simulated submission".into(),
            ));
        }
        if self.submission_nonce_is_used(submission_nonce)? {
            return Err(WatchtowerError::Invalid(
                "submission nonce was already used by another simulated submission".into(),
            ));
        }
        if authorization.status != EXECUTION_AUTHORIZED_SIMULATED_ONLY {
            return Err(WatchtowerError::Invalid(format!(
                "execution authorization is not active: {}",
                authorization.status
            )));
        }
        let manifest = self
            .execution_manifest(authorization.manifest_id, now)?
            .ok_or_else(|| {
                WatchtowerError::Corrupt("authorization manifest was not found".into())
            })?;
        if manifest.status != EXECUTION_MANIFEST_VERIFIED_NO_BROADCAST
            || manifest.bundle_commitment != authorization.bundle_commitment
        {
            self.connection.execute(
                "UPDATE v36_execution_authorizations SET status=?2, invalidated_at=?3,
                   invalidation_reason='execution manifest changed before simulated submission'
                 WHERE authorization_id=?1 AND status=?4",
                params![
                    authorization_id.as_slice(),
                    EXECUTION_AUTHORIZATION_INVALIDATED,
                    super::to_i64(now)?,
                    EXECUTION_AUTHORIZED_SIMULATED_ONLY
                ],
            )?;
            return Err(WatchtowerError::Invalid(
                "execution authorization was invalidated".into(),
            ));
        }
        let transaction = self.connection.unchecked_transaction()?;
        let updated = transaction.execute(
            "UPDATE v36_execution_authorizations SET simulated_submission_count=simulated_submission_count+1,
               last_simulated_at=?2, status=?3 WHERE authorization_id=?1 AND status=?4
               AND simulated_submission_count=0",
            params![authorization_id.as_slice(), super::to_i64(now)?,
                EXECUTION_AUTHORIZATION_CONSUMED_SIMULATED_ONLY,
                EXECUTION_AUTHORIZED_SIMULATED_ONLY],
        )?;
        if updated != 1 {
            return Err(WatchtowerError::Invalid(
                "execution authorization was already consumed".into(),
            ));
        }
        let receipt_id = sha256_parts(&[
            SIMULATED_SUBMISSION_RECEIPT_DOMAIN,
            &PROTOCOL_VERSION.to_be_bytes(),
            &authorization.authorization_id,
            &authorization.manifest_id,
            &authorization.bundle_commitment,
            &submission_nonce,
            &now.to_be_bytes(),
        ]);
        transaction.execute(
            "INSERT INTO v36_simulated_submission_receipts (
               receipt_id, authorization_id, manifest_id, bundle_commitment,
               submission_nonce, consumed_at, status,
               broadcast_enabled, broadcast_ready, chain_broadcast
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 0, 0)",
            params![
                receipt_id.as_slice(),
                authorization.authorization_id.as_slice(),
                authorization.manifest_id.as_slice(),
                authorization.bundle_commitment.as_slice(),
                submission_nonce.as_slice(),
                super::to_i64(now)?,
                SIMULATED_SUBMISSION_RECORDED
            ],
        )?;
        append_execution_audit_event_in_transaction(
            &transaction,
            "SIMULATED_SUBMISSION_RECORDED",
            receipt_id,
            authorization.bundle_commitment,
            SIMULATED_SUBMISSION_RECORDED,
            now,
        )?;
        transaction.commit()?;
        self.simulated_submission_receipt(authorization_id)?
            .ok_or_else(|| {
                WatchtowerError::Corrupt("simulated submission receipt disappeared".into())
            })
    }

    pub fn simulated_submission_receipt(
        &self,
        authorization_id: Bytes32,
    ) -> crate::Result<Option<SimulatedSubmissionReceipt>> {
        let row = self
            .connection
            .query_row(
                "SELECT receipt_id, manifest_id, bundle_commitment, submission_nonce,
                    consumed_at, status, broadcast_enabled, broadcast_ready, chain_broadcast
             FROM v36_simulated_submission_receipts WHERE authorization_id=?1",
                [authorization_id.as_slice()],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, bool>(6)?,
                        row.get::<_, bool>(7)?,
                        row.get::<_, bool>(8)?,
                    ))
                },
            )
            .optional()?;
        row.map(|row| {
            Ok(SimulatedSubmissionReceipt {
                receipt_id: super::bytes32(row.0, "simulated receipt ID")?,
                authorization_id,
                manifest_id: super::bytes32(row.1, "simulated receipt manifest ID")?,
                bundle_commitment: super::bytes32(row.2, "simulated receipt bundle commitment")?,
                submission_nonce: super::bytes32(row.3, "simulated receipt nonce")?,
                consumed_at: super::from_i64(row.4, "simulated receipt consumed time")?,
                status: row.5,
                broadcast_enabled: row.6,
                broadcast_ready: row.7,
                chain_broadcast: row.8,
            })
        })
        .transpose()
    }

    fn simulated_submission_receipt_for_manifest(
        &self,
        manifest_id: Bytes32,
    ) -> crate::Result<Option<SimulatedSubmissionReceipt>> {
        let authorization_id = self.connection.query_row(
            "SELECT authorization_id FROM v36_simulated_submission_receipts WHERE manifest_id=?1",
            [manifest_id.as_slice()],
            |row| row.get::<_, Vec<u8>>(0),
        ).optional()?;
        authorization_id
            .map(|value| {
                self.simulated_submission_receipt(super::bytes32(
                    value,
                    "receipt authorization ID",
                )?)
            })
            .transpose()
            .map(Option::flatten)
    }

    fn submission_nonce_is_used(&self, submission_nonce: Bytes32) -> crate::Result<bool> {
        self.connection
            .query_row(
                "SELECT 1 FROM v36_simulated_submission_receipts WHERE submission_nonce=?1",
                [submission_nonce.as_slice()],
                |_| Ok(()),
            )
            .optional()
            .map(|value| value.is_some())
            .map_err(Into::into)
    }

    pub(crate) fn invalidate_execution_authorizations(
        &self,
        column: &str,
        value: Bytes32,
        status: &str,
        reason: &str,
        now: u64,
    ) -> crate::Result<()> {
        let sql = format!(
            "UPDATE v36_execution_authorizations SET status=?2, invalidated_at=?3,
               invalidation_reason=?4 WHERE {column}=?1 AND status=?5"
        );
        self.connection.execute(
            &sql,
            params![
                value.as_slice(),
                status,
                super::to_i64(now)?,
                reason,
                EXECUTION_AUTHORIZED_SIMULATED_ONLY
            ],
        )?;
        Ok(())
    }
}

fn authorization_id(manifest: &ExecutionManifest, issued_at: u64, expires_at: u64) -> Bytes32 {
    sha256_parts(&[
        EXECUTION_AUTHORIZATION_DOMAIN,
        &PROTOCOL_VERSION.to_be_bytes(),
        &manifest.manifest_id,
        &manifest.recheck_id,
        &manifest.bundle_commitment,
        &manifest.approval_set_hash,
        &issued_at.to_be_bytes(),
        &expires_at.to_be_bytes(),
    ])
}
