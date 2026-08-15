use rusqlite::{OptionalExtension, params};
use xhub_protocol_v3_6::{Bytes32, PROTOCOL_VERSION, sha256_parts};

use crate::{
    WatchtowerError, WatchtowerStore,
    approval::DUAL_APPROVED_RECHECK_REQUIRED,
    audit::append_execution_audit_event_in_transaction,
    authorization::{EXECUTION_AUTHORIZATION_INVALIDATED, EXECUTION_AUTHORIZATION_SUPERSEDED},
    final_recheck::{FINAL_RECHECK_VERIFIED_NO_BROADCAST, FinalChainRecheck},
};

pub const EXECUTION_MANIFEST_DOMAIN: &[u8] = b"XHUB_EXECUTION_MANIFEST_V3_6";
pub const EXECUTION_MANIFEST_VERIFIED_NO_BROADCAST: &str = "MANIFEST_VERIFIED_NO_BROADCAST";
pub const EXECUTION_MANIFEST_EXPIRED: &str = "MANIFEST_EXPIRED";
pub const EXECUTION_MANIFEST_INVALIDATED_CHAIN_CHANGE: &str = "MANIFEST_INVALIDATED_CHAIN_CHANGE";
pub const EXECUTION_MANIFEST_SUPERSEDED: &str = "MANIFEST_SUPERSEDED";
pub const EXECUTION_MANIFEST_TTL_SECONDS: u64 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionManifest {
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
    pub broadcast_enabled: bool,
    pub broadcast_ready: bool,
    pub chain_broadcast: bool,
}

impl WatchtowerStore {
    pub fn issue_execution_manifest(
        &self,
        recheck_id: Bytes32,
        now: u64,
    ) -> crate::Result<ExecutionManifest> {
        let recheck = self
            .final_chain_recheck(recheck_id, now)?
            .ok_or_else(|| WatchtowerError::Invalid("final chain recheck was not found".into()))?;
        if recheck.status != FINAL_RECHECK_VERIFIED_NO_BROADCAST {
            return Err(WatchtowerError::Invalid(format!(
                "final chain recheck is not active: {}",
                recheck.status
            )));
        }
        let preparation = self
            .offline_preparation(recheck.closing_coin_id)?
            .ok_or_else(|| WatchtowerError::Corrupt("manifest preparation was not found".into()))?;
        let approval = self.approval_status(recheck.closing_coin_id, now)?;
        let (approval_set_hash, _) = self.verified_approval_set(recheck.preparation_id, now)?;
        if approval.status != DUAL_APPROVED_RECHECK_REQUIRED
            || approval.preparation_id != recheck.preparation_id
            || approval_set_hash != recheck.approval_set_hash
            || preparation.funding_coin_id != recheck.funding_coin_id
            || preparation.fee_coin_id != recheck.fee_coin_id
            || preparation.report_hash != recheck.report_hash
            || preparation.bundle_commitment != recheck.bundle_commitment
            || preparation.snapshot.peak_height != recheck.peak_height
            || preparation.snapshot.peak_header_hash != recheck.peak_header_hash
            || preparation.challenge_deadline_height != recheck.challenge_deadline_height
        {
            return Err(WatchtowerError::Corrupt(
                "final recheck differs from the current preparation or approval set".into(),
            ));
        }
        let expires_at = now
            .checked_add(EXECUTION_MANIFEST_TTL_SECONDS)
            .ok_or_else(|| WatchtowerError::Invalid("execution manifest expiry overflow".into()))?
            .min(recheck.expires_at);
        if expires_at <= now {
            return Err(WatchtowerError::Invalid(
                "final chain recheck expires before an execution manifest can be issued".into(),
            ));
        }
        let manifest_id = manifest_id(&recheck, now, expires_at);
        if let Some(existing) = self.execution_manifest(manifest_id, now)?
            && existing.status == EXECUTION_MANIFEST_VERIFIED_NO_BROADCAST
        {
            return Ok(existing);
        }
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            "UPDATE v36_execution_manifests SET status=?2, invalidated_at=?3,
               invalidation_reason='superseded by a newer execution manifest'
             WHERE recheck_id=?1 AND status=?4",
            params![
                recheck_id.as_slice(),
                EXECUTION_MANIFEST_SUPERSEDED,
                super::to_i64(now)?,
                EXECUTION_MANIFEST_VERIFIED_NO_BROADCAST,
            ],
        )?;
        self.invalidate_execution_authorizations(
            "recheck_id",
            recheck_id,
            EXECUTION_AUTHORIZATION_SUPERSEDED,
            "superseded by a newer execution manifest",
            now,
        )?;
        transaction.execute(
            "INSERT INTO v36_execution_manifests (
               manifest_id, recheck_id, preparation_id, closing_coin_id,
               funding_coin_id, fee_coin_id, report_hash, bundle_commitment,
               approval_set_hash, peak_height, peak_header_hash,
               challenge_deadline_height, issued_at, expires_at, status,
               broadcast_enabled, broadcast_ready, chain_broadcast
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, 0, 0, 0)",
            params![
                manifest_id.as_slice(),
                recheck.recheck_id.as_slice(),
                recheck.preparation_id.as_slice(),
                recheck.closing_coin_id.as_slice(),
                recheck.funding_coin_id.as_slice(),
                recheck.fee_coin_id.as_slice(),
                recheck.report_hash.as_slice(),
                recheck.bundle_commitment.as_slice(),
                recheck.approval_set_hash.as_slice(),
                super::to_i64(recheck.peak_height)?,
                recheck.peak_header_hash.as_slice(),
                super::to_i64(recheck.challenge_deadline_height)?,
                super::to_i64(now)?,
                super::to_i64(expires_at)?,
                EXECUTION_MANIFEST_VERIFIED_NO_BROADCAST,
            ],
        )?;
        append_execution_audit_event_in_transaction(
            &transaction,
            "EXECUTION_MANIFEST_ISSUED",
            manifest_id,
            recheck.bundle_commitment,
            EXECUTION_MANIFEST_VERIFIED_NO_BROADCAST,
            now,
        )?;
        transaction.commit()?;
        self.execution_manifest(manifest_id, now)?
            .ok_or_else(|| WatchtowerError::Corrupt("new execution manifest was not found".into()))
    }

    pub fn execution_manifest(
        &self,
        manifest_id: Bytes32,
        now: u64,
    ) -> crate::Result<Option<ExecutionManifest>> {
        self.connection.execute(
            "UPDATE v36_execution_manifests SET status=?2, invalidated_at=?3,
               invalidation_reason='execution manifest validity window expired'
             WHERE manifest_id=?1 AND status=?4 AND expires_at<=?3",
            params![
                manifest_id.as_slice(),
                EXECUTION_MANIFEST_EXPIRED,
                super::to_i64(now)?,
                EXECUTION_MANIFEST_VERIFIED_NO_BROADCAST,
            ],
        )?;
        let row = self
            .connection
            .query_row(
                "SELECT recheck_id, preparation_id, closing_coin_id, funding_coin_id,
                    fee_coin_id, report_hash, bundle_commitment, approval_set_hash,
                    peak_height, peak_header_hash, challenge_deadline_height,
                    issued_at, expires_at, status, invalidation_reason,
                    broadcast_enabled, broadcast_ready, chain_broadcast
                 FROM v36_execution_manifests WHERE manifest_id=?1",
                [manifest_id.as_slice()],
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
                        row.get::<_, i64>(8)?,
                        row.get::<_, Vec<u8>>(9)?,
                        row.get::<_, i64>(10)?,
                        row.get::<_, i64>(11)?,
                        row.get::<_, i64>(12)?,
                        row.get::<_, String>(13)?,
                        row.get::<_, Option<String>>(14)?,
                        row.get::<_, bool>(15)?,
                        row.get::<_, bool>(16)?,
                        row.get::<_, bool>(17)?,
                    ))
                },
            )
            .optional()?;
        row.map(|row| {
            Ok(ExecutionManifest {
                manifest_id,
                recheck_id: super::bytes32(row.0, "manifest recheck ID")?,
                preparation_id: super::bytes32(row.1, "manifest preparation ID")?,
                closing_coin_id: super::bytes32(row.2, "manifest Closing Coin ID")?,
                funding_coin_id: super::bytes32(row.3, "manifest Funding Coin ID")?,
                fee_coin_id: super::bytes32(row.4, "manifest fee Coin ID")?,
                report_hash: super::bytes32(row.5, "manifest report hash")?,
                bundle_commitment: super::bytes32(row.6, "manifest bundle commitment")?,
                approval_set_hash: super::bytes32(row.7, "manifest approval set hash")?,
                peak_height: super::from_i64(row.8, "manifest peak height")?,
                peak_header_hash: super::bytes32(row.9, "manifest peak header hash")?,
                challenge_deadline_height: super::from_i64(row.10, "manifest deadline")?,
                issued_at: super::from_i64(row.11, "manifest issued time")?,
                expires_at: super::from_i64(row.12, "manifest expiry")?,
                status: row.13,
                invalidation_reason: row.14,
                broadcast_enabled: row.15,
                broadcast_ready: row.16,
                chain_broadcast: row.17,
            })
        })
        .transpose()
    }

    pub(crate) fn invalidate_execution_manifests_for_funding(
        &self,
        funding_coin_id: Bytes32,
        reason: &str,
        now: u64,
    ) -> crate::Result<()> {
        self.invalidate_execution_manifests("funding_coin_id", funding_coin_id, reason, now)
    }

    pub(crate) fn invalidate_execution_manifests_for_closing(
        &self,
        closing_coin_id: Bytes32,
        reason: &str,
        now: u64,
    ) -> crate::Result<()> {
        self.invalidate_execution_manifests("closing_coin_id", closing_coin_id, reason, now)
    }

    pub(crate) fn supersede_execution_manifests_for_preparation(
        &self,
        preparation_id: Bytes32,
        reason: &str,
        now: u64,
    ) -> crate::Result<()> {
        let sql = "UPDATE v36_execution_manifests SET status=?2, invalidated_at=?3,
             invalidation_reason=?4 WHERE preparation_id=?1 AND status=?5";
        self.connection.execute(
            sql,
            params![
                preparation_id.as_slice(),
                EXECUTION_MANIFEST_SUPERSEDED,
                super::to_i64(now)?,
                reason,
                EXECUTION_MANIFEST_VERIFIED_NO_BROADCAST,
            ],
        )?;
        self.invalidate_execution_authorizations(
            "preparation_id",
            preparation_id,
            EXECUTION_AUTHORIZATION_SUPERSEDED,
            reason,
            now,
        )?;
        Ok(())
    }

    fn invalidate_execution_manifests(
        &self,
        column: &str,
        coin_id: Bytes32,
        reason: &str,
        now: u64,
    ) -> crate::Result<()> {
        let sql = format!(
            "UPDATE v36_execution_manifests SET status=?2, invalidated_at=?3,
               invalidation_reason=?4 WHERE {column}=?1 AND status=?5"
        );
        self.connection.execute(
            &sql,
            params![
                coin_id.as_slice(),
                EXECUTION_MANIFEST_INVALIDATED_CHAIN_CHANGE,
                super::to_i64(now)?,
                reason,
                EXECUTION_MANIFEST_VERIFIED_NO_BROADCAST,
            ],
        )?;
        self.invalidate_execution_authorizations(
            column,
            coin_id,
            EXECUTION_AUTHORIZATION_INVALIDATED,
            reason,
            now,
        )?;
        Ok(())
    }
}

fn manifest_id(recheck: &FinalChainRecheck, issued_at: u64, expires_at: u64) -> Bytes32 {
    sha256_parts(&[
        EXECUTION_MANIFEST_DOMAIN,
        &PROTOCOL_VERSION.to_be_bytes(),
        &recheck.recheck_id,
        &recheck.preparation_id,
        &recheck.bundle_commitment,
        &recheck.approval_set_hash,
        &recheck.peak_height.to_be_bytes(),
        &recheck.peak_header_hash,
        &recheck.challenge_deadline_height.to_be_bytes(),
        &issued_at.to_be_bytes(),
        &expires_at.to_be_bytes(),
    ])
}
