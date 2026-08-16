use rusqlite::{OptionalExtension, params};
use xhub_protocol_v3_6::{
    Bytes32, PROTOCOL_VERSION, PublicKeyBytes, SignatureBytes, sha256_parts, verify_hash,
};

use crate::{
    WatchtowerError, WatchtowerStore,
    approval::{ApprovalDecision, ApprovalStatement, DUAL_APPROVED_RECHECK_REQUIRED},
    monitor::{ClosingObservation, MonitorAction},
    rpc::WatchtowerChainProvider,
};

pub const FINAL_RECHECK_DOMAIN: &[u8] = b"XHUB_FINAL_CHAIN_RECHECK_V3_6";
pub const APPROVAL_SET_DOMAIN: &[u8] = b"XHUB_APPROVAL_SET_V3_6";
pub const FINAL_RECHECK_VERIFIED_NO_BROADCAST: &str = "FINAL_RECHECK_VERIFIED_NO_BROADCAST";
pub const FINAL_RECHECK_EXPIRED: &str = "FINAL_RECHECK_EXPIRED";
pub const FINAL_RECHECK_INVALIDATED_CHAIN_CHANGE: &str = "FINAL_RECHECK_INVALIDATED_CHAIN_CHANGE";
pub const FINAL_RECHECK_TTL_SECONDS: u64 = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalChainRecheck {
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
    pub performed_at: u64,
    pub expires_at: u64,
    pub status: String,
    pub invalidation_reason: Option<String>,
    pub broadcast_enabled: bool,
    pub broadcast_ready: bool,
    pub chain_broadcast: bool,
}

impl WatchtowerStore {
    pub fn poll_final_chain_recheck<P: WatchtowerChainProvider>(
        &self,
        provider: &P,
        funding_coin_id: Bytes32,
        now: u64,
    ) -> crate::Result<FinalChainRecheck> {
        let observation = match self.build_observation(provider, funding_coin_id) {
            Ok(observation) => observation,
            Err(error) => {
                self.mark_offline_recheck_required(
                    funding_coin_id,
                    &format!("final RPC recheck failed: {error}"),
                    now,
                )?;
                return Err(WatchtowerError::Invalid(error.to_string()));
            }
        };
        match self.perform_final_chain_recheck(&observation, now) {
            Ok(recheck) => Ok(recheck),
            Err(error) => {
                self.mark_offline_recheck_required(
                    funding_coin_id,
                    &format!("final chain snapshot failed recheck: {error}"),
                    now,
                )?;
                Err(error)
            }
        }
    }

    #[doc(hidden)]
    pub fn perform_final_chain_recheck(
        &self,
        observation: &ClosingObservation,
        now: u64,
    ) -> crate::Result<FinalChainRecheck> {
        let closing = observation.closing_coin.as_ref().ok_or_else(|| {
            WatchtowerError::Invalid("final recheck requires a Closing Coin".into())
        })?;
        let preparation = self
            .offline_preparation(closing.coin_id)?
            .ok_or_else(|| WatchtowerError::Invalid("offline preparation was not found".into()))?;
        let approval = self.approval_status(closing.coin_id, now)?;
        if approval.status != DUAL_APPROVED_RECHECK_REQUIRED
            || approval.approver_count != 2
            || approval.failure_domain_count != 2
        {
            return Err(WatchtowerError::Invalid(
                "final recheck requires two current approvals from two failure domains".into(),
            ));
        }
        if !observation.synced || closing.spent_height.is_some() {
            return Err(WatchtowerError::Invalid(
                "final recheck requires a synced, unspent Closing Coin".into(),
            ));
        }
        if observation.peak.height >= preparation.challenge_deadline_height {
            return Err(WatchtowerError::Invalid(
                "final recheck cannot pass at or after the challenge deadline".into(),
            ));
        }
        if observation.peak.height != preparation.snapshot.peak_height
            || observation.peak.header_hash != preparation.snapshot.peak_header_hash
            || closing.coin_id != preparation.closing_coin_id
            || closing.parent_coin_id
                != preparation
                    .snapshot
                    .closing_coin
                    .parent_coin_info
                    .to_bytes()
            || closing.puzzle_hash != preparation.snapshot.closing_coin.puzzle_hash.to_bytes()
            || closing.amount != preparation.snapshot.closing_coin.amount
            || closing.birth_height != preparation.snapshot.closing_birth_height
            || observation.challenge_deadline_height != Some(preparation.challenge_deadline_height)
        {
            return Err(WatchtowerError::Invalid(
                "final recheck chain snapshot differs from the approved preparation".into(),
            ));
        }
        let decision = self.evaluate_observation(observation)?;
        if !matches!(
            decision.action,
            MonitorAction::ChallengeAlreadyPlanned | MonitorAction::ChallengePlanned
        ) {
            return Err(WatchtowerError::Invalid(format!(
                "final recheck no longer permits a challenge: {:?}",
                decision.action
            )));
        }

        let (approval_set_hash, earliest_approval_expiry) =
            self.verified_approval_set(approval.preparation_id, now)?;
        let ttl_expiry = now
            .checked_add(FINAL_RECHECK_TTL_SECONDS)
            .ok_or_else(|| WatchtowerError::Invalid("final recheck expiry overflow".into()))?;
        let expires_at = ttl_expiry.min(earliest_approval_expiry);
        if expires_at <= now {
            return Err(WatchtowerError::Invalid(
                "approvals expire before a final recheck record can be issued".into(),
            ));
        }
        let recheck_id = sha256_parts(&[
            FINAL_RECHECK_DOMAIN,
            &PROTOCOL_VERSION.to_be_bytes(),
            &approval.preparation_id,
            &approval_set_hash,
            &observation.peak.height.to_be_bytes(),
            &observation.peak.header_hash,
            &now.to_be_bytes(),
            &expires_at.to_be_bytes(),
        ]);
        if let Some(existing) = self.final_chain_recheck(recheck_id, now)?
            && existing.status == FINAL_RECHECK_VERIFIED_NO_BROADCAST
        {
            return Ok(existing);
        }
        self.supersede_execution_manifests_for_preparation(
            approval.preparation_id,
            "final chain recheck superseded",
            now,
        )?;
        self.connection.execute(
            "UPDATE v36_final_chain_rechecks SET status=?2, invalidated_at=?3,
               invalidation_reason='superseded by a newer final recheck'
             WHERE preparation_id=?1 AND status=?4",
            params![
                approval.preparation_id.as_slice(),
                FINAL_RECHECK_INVALIDATED_CHAIN_CHANGE,
                super::to_i64(now)?,
                FINAL_RECHECK_VERIFIED_NO_BROADCAST,
            ],
        )?;
        self.connection.execute(
            "INSERT INTO v36_final_chain_rechecks (
               recheck_id, preparation_id, closing_coin_id, funding_coin_id,
               fee_coin_id, report_hash, bundle_commitment, approval_set_hash, peak_height,
               peak_header_hash, challenge_deadline_height, performed_at,
               expires_at, status, broadcast_enabled, broadcast_ready, chain_broadcast
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 0, 0, 0)",
            params![
                recheck_id.as_slice(),
                approval.preparation_id.as_slice(),
                closing.coin_id.as_slice(),
                preparation.funding_coin_id.as_slice(),
                preparation.fee_coin_id.as_slice(),
                preparation.report_hash.as_slice(),
                preparation.bundle_commitment.as_slice(),
                approval_set_hash.as_slice(),
                super::to_i64(observation.peak.height)?,
                observation.peak.header_hash.as_slice(),
                super::to_i64(preparation.challenge_deadline_height)?,
                super::to_i64(now)?,
                super::to_i64(expires_at)?,
                FINAL_RECHECK_VERIFIED_NO_BROADCAST,
            ],
        )?;
        self.final_chain_recheck(recheck_id, now)?.ok_or_else(|| {
            WatchtowerError::Corrupt("new final recheck record was not found".into())
        })
    }

    pub fn final_chain_recheck(
        &self,
        recheck_id: Bytes32,
        now: u64,
    ) -> crate::Result<Option<FinalChainRecheck>> {
        self.connection.execute(
            "UPDATE v36_final_chain_rechecks SET status=?2, invalidated_at=?3,
               invalidation_reason='final recheck validity window expired'
             WHERE recheck_id=?1 AND status=?4 AND expires_at<=?3",
            params![
                recheck_id.as_slice(),
                FINAL_RECHECK_EXPIRED,
                super::to_i64(now)?,
                FINAL_RECHECK_VERIFIED_NO_BROADCAST,
            ],
        )?;
        let row = self
            .connection
            .query_row(
                "SELECT preparation_id, closing_coin_id, funding_coin_id, fee_coin_id,
                    report_hash, bundle_commitment, approval_set_hash, peak_height, peak_header_hash,
                    challenge_deadline_height, performed_at, expires_at, status,
                    invalidation_reason, broadcast_enabled, broadcast_ready, chain_broadcast
             FROM v36_final_chain_rechecks WHERE recheck_id=?1",
                [recheck_id.as_slice()],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                        row.get::<_, Option<Vec<u8>>>(5)?,
                        row.get::<_, Vec<u8>>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, Vec<u8>>(8)?,
                        row.get::<_, i64>(9)?,
                        row.get::<_, i64>(10)?,
                        row.get::<_, i64>(11)?,
                        row.get::<_, String>(12)?,
                        row.get::<_, Option<String>>(13)?,
                        row.get::<_, bool>(14)?,
                        row.get::<_, bool>(15)?,
                        row.get::<_, bool>(16)?,
                    ))
                },
            )
            .optional()?;
        row.map(|row| {
            Ok(FinalChainRecheck {
                recheck_id,
                preparation_id: super::bytes32(row.0, "recheck preparation ID")?,
                closing_coin_id: super::bytes32(row.1, "recheck Closing Coin ID")?,
                funding_coin_id: super::bytes32(row.2, "recheck Funding Coin ID")?,
                fee_coin_id: super::bytes32(row.3, "recheck fee Coin ID")?,
                report_hash: super::bytes32(row.4, "recheck report hash")?,
                bundle_commitment: super::bytes32(
                    row.5.ok_or_else(|| {
                        WatchtowerError::Corrupt(
                            "legacy final recheck omitted SpendBundle commitment".into(),
                        )
                    })?,
                    "recheck SpendBundle commitment",
                )?,
                approval_set_hash: super::bytes32(row.6, "recheck approval set hash")?,
                peak_height: super::from_i64(row.7, "recheck peak height")?,
                peak_header_hash: super::bytes32(row.8, "recheck peak header hash")?,
                challenge_deadline_height: super::from_i64(row.9, "recheck deadline")?,
                performed_at: super::from_i64(row.10, "recheck time")?,
                expires_at: super::from_i64(row.11, "recheck expiry")?,
                status: row.12,
                invalidation_reason: row.13,
                broadcast_enabled: row.14,
                broadcast_ready: row.15,
                chain_broadcast: row.16,
            })
        })
        .transpose()
    }

    pub(crate) fn verified_approval_set(
        &self,
        preparation_id: Bytes32,
        now: u64,
    ) -> crate::Result<(Bytes32, u64)> {
        let mut statement = self.connection.prepare(
            "SELECT approver_id, failure_domain, approver_public_key, statement_blob,
                    signature, expires_at
             FROM v36_challenge_approvals
             WHERE preparation_id=?1 AND status='ACTIVE' AND expires_at>?2
             ORDER BY approver_id",
        )?;
        let rows = statement.query_map(
            params![preparation_id.as_slice(), super::to_i64(now)?],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )?;
        let mut material = Vec::new();
        let mut domains = std::collections::HashSet::new();
        let mut earliest_expiry = u64::MAX;
        let mut count = 0_u16;
        for row in rows {
            let (approver_id, domain, public_key, blob, signature, expiry) = row?;
            let public_key: PublicKeyBytes = public_key
                .try_into()
                .map_err(|_| WatchtowerError::Corrupt("approval public key length".into()))?;
            let signature: SignatureBytes = signature
                .try_into()
                .map_err(|_| WatchtowerError::Corrupt("approval signature length".into()))?;
            let decoded = ApprovalStatement::from_canonical_bytes(&blob)?;
            if decoded.protocol_version != PROTOCOL_VERSION
                || decoded.preparation_id != preparation_id
                || decoded.approver_id != approver_id
                || decoded.failure_domain != domain
                || decoded.approver_public_key != public_key
                || decoded.decision != ApprovalDecision::Approve
                || decoded.expires_at != super::from_i64(expiry, "persisted approval expiry")?
            {
                return Err(WatchtowerError::Corrupt(
                    "persisted approval index columns differ from the signed statement".into(),
                ));
            }
            let signing_hash = decoded.signing_hash();
            verify_hash(&public_key, &signing_hash, &signature).map_err(|_| {
                WatchtowerError::Corrupt("persisted approval signature is invalid".into())
            })?;
            material.extend_from_slice(&(approver_id.len() as u32).to_be_bytes());
            material.extend_from_slice(approver_id.as_bytes());
            material.extend_from_slice(&(domain.len() as u32).to_be_bytes());
            material.extend_from_slice(domain.as_bytes());
            material.extend_from_slice(&public_key);
            material.extend_from_slice(&signing_hash);
            material.extend_from_slice(&signature);
            domains.insert(domain);
            earliest_expiry = earliest_expiry.min(super::from_i64(expiry, "approval expiry")?);
            count = count
                .checked_add(1)
                .ok_or_else(|| WatchtowerError::Corrupt("approval count overflow".into()))?;
        }
        if count != 2 || domains.len() != 2 {
            return Err(WatchtowerError::Invalid(
                "final recheck requires exactly two verified approvals in two failure domains"
                    .into(),
            ));
        }
        Ok((
            sha256_parts(&[APPROVAL_SET_DOMAIN, &material]),
            earliest_expiry,
        ))
    }

    pub(crate) fn invalidate_final_rechecks_for_funding(
        &self,
        funding_coin_id: Bytes32,
        reason: &str,
        now: u64,
    ) -> crate::Result<()> {
        self.invalidate_final_rechecks("funding_coin_id", funding_coin_id, reason, now)
    }

    pub(crate) fn invalidate_final_rechecks_for_closing(
        &self,
        closing_coin_id: Bytes32,
        reason: &str,
        now: u64,
    ) -> crate::Result<()> {
        self.invalidate_final_rechecks("closing_coin_id", closing_coin_id, reason, now)
    }

    fn invalidate_final_rechecks(
        &self,
        column: &str,
        coin_id: Bytes32,
        reason: &str,
        now: u64,
    ) -> crate::Result<()> {
        let sql = format!(
            "UPDATE v36_final_chain_rechecks SET status=?2, invalidated_at=?3,
               invalidation_reason=?4 WHERE {column}=?1 AND status=?5"
        );
        self.connection.execute(
            &sql,
            params![
                coin_id.as_slice(),
                FINAL_RECHECK_INVALIDATED_CHAIN_CHANGE,
                super::to_i64(now)?,
                reason,
                FINAL_RECHECK_VERIFIED_NO_BROADCAST,
            ],
        )?;
        Ok(())
    }
}
