use chia_protocol::Coin;
use rusqlite::{OptionalExtension, params};
use serde_json::Value;
use xhub_protocol_v3_6::Bytes32;

use crate::{
    WatchtowerError, WatchtowerStore,
    bundle::{
        ChainSnapshot, OfflineChallengeBundle, TestFeeSponsor, build_offline_challenge_bundle,
    },
    monitor::{ClosingObservation, MonitorAction, MonitorDecision},
};

pub const OFFLINE_VERIFIED_AWAITING_APPROVAL: &str = "OFFLINE_VERIFIED_AWAITING_APPROVAL";
pub const CHAIN_RECHECK_REQUIRED: &str = "CHAIN_RECHECK_REQUIRED";
pub const INVALIDATED_CHAIN_CHANGE: &str = "INVALIDATED_CHAIN_CHANGE";

#[derive(Debug, Clone, PartialEq)]
pub struct PersistedOfflinePreparation {
    pub closing_coin_id: Bytes32,
    pub funding_coin_id: Bytes32,
    pub snapshot: ChainSnapshot,
    pub challenge_deadline_height: u64,
    pub fee_coin_id: Bytes32,
    pub fee_mojo: u64,
    pub report: Value,
    pub report_hash: Bytes32,
    pub bundle_commitment: Bytes32,
    pub prepared_at: u64,
    pub status: String,
    pub invalidation_reason: Option<String>,
}

impl WatchtowerStore {
    pub fn prepare_offline_challenge(
        &self,
        observation: &ClosingObservation,
        fee: &TestFeeSponsor,
        now: u64,
    ) -> crate::Result<OfflineChallengeBundle> {
        let closing = observation.closing_coin.as_ref().ok_or_else(|| {
            WatchtowerError::Invalid("offline preparation requires an unspent Closing Coin".into())
        })?;
        if !observation.synced || closing.spent_height.is_some() {
            return Err(WatchtowerError::Invalid(
                "offline preparation requires a synced, unspent Closing Coin".into(),
            ));
        }
        let plan = self
            .challenge_plan(closing.coin_id)?
            .ok_or_else(|| WatchtowerError::Invalid("challenge plan was not found".into()))?;
        let kind = observation.closing_coin_kind.ok_or_else(|| {
            WatchtowerError::Invalid("Closing observation omitted coin kind".into())
        })?;
        let current_sequence = observation.current_state_sequence.ok_or_else(|| {
            WatchtowerError::Invalid("Closing observation omitted current state sequence".into())
        })?;
        let initial_birth_height = observation.initial_birth_height.ok_or_else(|| {
            WatchtowerError::Invalid("Closing observation omitted initial birth height".into())
        })?;
        let deadline = observation.challenge_deadline_height.ok_or_else(|| {
            WatchtowerError::Invalid("Closing observation omitted challenge deadline".into())
        })?;
        if plan.funding_coin_id != observation.funding_coin.coin_id
            || plan.current_state_sequence != current_sequence
            || plan.challenge_deadline_height != deadline
            || plan.simulation.closing_coin_kind != kind
            || plan.simulation.initial_birth_height != initial_birth_height
            || plan.simulation.current_checkpoint_hash
                != hex::encode(observation.current_checkpoint_hash.ok_or_else(|| {
                    WatchtowerError::Invalid(
                        "Closing observation omitted current checkpoint hash".into(),
                    )
                })?)
        {
            return Err(WatchtowerError::Invalid(
                "challenge plan no longer matches the current chain observation".into(),
            ));
        }
        let latest = self.latest_package(plan.funding_coin_id)?;
        if latest.official_state.checkpoint.state_sequence != plan.latest_state_sequence {
            return Err(WatchtowerError::Invalid(
                "a newer RecoveryPackage requires a new challenge plan".into(),
            ));
        }
        let current = (current_sequence > 0)
            .then(|| self.package(plan.funding_coin_id, current_sequence))
            .transpose()?;
        let coin = Coin::new(
            closing.parent_coin_id.into(),
            closing.puzzle_hash.into(),
            closing.amount,
        );
        if coin.coin_id().to_bytes() != closing.coin_id {
            return Err(WatchtowerError::Invalid(
                "Closing Coin ID does not match its parent, puzzle hash, and amount".into(),
            ));
        }
        let snapshot = ChainSnapshot {
            peak_height: observation.peak.height,
            peak_header_hash: observation.peak.header_hash,
            closing_coin_id: closing.coin_id,
            closing_coin: coin,
            closing_birth_height: closing.birth_height,
            closing_spent_height: closing.spent_height,
        };
        let bundle = build_offline_challenge_bundle(
            current.as_ref(),
            &latest,
            kind,
            initial_birth_height,
            deadline,
            snapshot.clone(),
            fee,
        )
        .map_err(WatchtowerError::Invalid)?;
        let report_json = serde_json::to_string(bundle.report())
            .map_err(|error| WatchtowerError::Corrupt(error.to_string()))?;
        let previous_epoch = self
            .connection
            .query_row(
                "SELECT created_at FROM v36_offline_challenge_preparations
                 WHERE closing_coin_id=?1",
                [closing.coin_id.as_slice()],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .map(|value| super::from_i64(value, "preparation creation time"))
            .transpose()?;
        let prepared_at = match previous_epoch {
            Some(previous) => now.max(previous.checked_add(1).ok_or_else(|| {
                WatchtowerError::Invalid("offline preparation epoch overflow".into())
            })?),
            None => now,
        };
        self.revoke_approvals_for_closing(
            closing.coin_id,
            "offline preparation was rebuilt from a fresh snapshot",
            now,
        )?;
        self.invalidate_final_rechecks_for_closing(
            closing.coin_id,
            "offline preparation was rebuilt from a fresh snapshot",
            now,
        )?;
        self.invalidate_execution_manifests_for_closing(
            closing.coin_id,
            "offline preparation was rebuilt from a fresh snapshot",
            now,
        )?;
        self.connection.execute(
            "INSERT INTO v36_offline_challenge_preparations (
               closing_coin_id, funding_coin_id, peak_height, peak_header_hash,
               closing_parent_coin_id, closing_puzzle_hash, closing_amount,
               closing_birth_height, challenge_deadline_height, fee_coin_id,
               fee_mojo, report_json, bundle_commitment, status, invalidation_reason, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, NULL, ?15, ?15)
             ON CONFLICT(closing_coin_id) DO UPDATE SET
               funding_coin_id=excluded.funding_coin_id,
               peak_height=excluded.peak_height,
               peak_header_hash=excluded.peak_header_hash,
               closing_parent_coin_id=excluded.closing_parent_coin_id,
               closing_puzzle_hash=excluded.closing_puzzle_hash,
               closing_amount=excluded.closing_amount,
               closing_birth_height=excluded.closing_birth_height,
               challenge_deadline_height=excluded.challenge_deadline_height,
               fee_coin_id=excluded.fee_coin_id,
               fee_mojo=excluded.fee_mojo,
               report_json=excluded.report_json,
               bundle_commitment=excluded.bundle_commitment,
               status=excluded.status,
               invalidation_reason=NULL,
               created_at=excluded.created_at,
               updated_at=excluded.updated_at",
            params![
                closing.coin_id.as_slice(),
                plan.funding_coin_id.as_slice(),
                super::to_i64(snapshot.peak_height)?,
                snapshot.peak_header_hash.as_slice(),
                closing.parent_coin_id.as_slice(),
                closing.puzzle_hash.as_slice(),
                super::to_i64(closing.amount)?,
                super::to_i64(closing.birth_height)?,
                super::to_i64(deadline)?,
                fee.coin.coin_id().as_slice(),
                super::to_i64(fee.fee_mojo)?,
                report_json,
                bundle.commitment().as_slice(),
                OFFLINE_VERIFIED_AWAITING_APPROVAL,
                super::to_i64(prepared_at)?,
            ],
        )?;
        Ok(bundle)
    }

    pub fn offline_preparation(
        &self,
        closing_coin_id: Bytes32,
    ) -> crate::Result<Option<PersistedOfflinePreparation>> {
        let row = self
            .connection
            .query_row(
                "SELECT funding_coin_id, peak_height, peak_header_hash,
                    closing_parent_coin_id, closing_puzzle_hash, closing_amount,
                    closing_birth_height, challenge_deadline_height, fee_coin_id,
                    fee_mojo, report_json, bundle_commitment, status, invalidation_reason, created_at
             FROM v36_offline_challenge_preparations WHERE closing_coin_id=?1",
                [closing_coin_id.as_slice()],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, Vec<u8>>(8)?,
                        row.get::<_, i64>(9)?,
                        row.get::<_, String>(10)?,
                        row.get::<_, Option<Vec<u8>>>(11)?,
                        row.get::<_, String>(12)?,
                        row.get::<_, Option<String>>(13)?,
                        row.get::<_, i64>(14)?,
                    ))
                },
            )
            .optional()?;
        row.map(|row| {
            let parent = super::bytes32(row.3, "preparation Closing parent")?;
            let puzzle_hash = super::bytes32(row.4, "preparation Closing puzzle hash")?;
            let amount = super::from_i64(row.5, "preparation Closing amount")?;
            let coin = Coin::new(parent.into(), puzzle_hash.into(), amount);
            if coin.coin_id().to_bytes() != closing_coin_id {
                return Err(WatchtowerError::Corrupt(
                    "persisted preparation Closing Coin ID is inconsistent".into(),
                ));
            }
            Ok(PersistedOfflinePreparation {
                closing_coin_id,
                funding_coin_id: super::bytes32(row.0, "preparation funding Coin ID")?,
                snapshot: ChainSnapshot {
                    peak_height: super::from_i64(row.1, "preparation peak height")?,
                    peak_header_hash: super::bytes32(row.2, "preparation peak hash")?,
                    closing_coin_id,
                    closing_coin: coin,
                    closing_birth_height: super::from_i64(row.6, "preparation birth height")?,
                    closing_spent_height: None,
                },
                challenge_deadline_height: super::from_i64(row.7, "preparation deadline")?,
                fee_coin_id: super::bytes32(row.8, "preparation fee Coin ID")?,
                fee_mojo: super::from_i64(row.9, "preparation fee")?,
                report_hash: xhub_protocol_v3_6::sha256_parts(&[
                    crate::approval::APPROVAL_REPORT_DOMAIN,
                    row.10.as_bytes(),
                ]),
                bundle_commitment: super::bytes32(
                    row.11.ok_or_else(|| WatchtowerError::Corrupt(
                        "legacy offline preparation omitted SpendBundle commitment; rebuild required".into()
                    ))?,
                    "preparation SpendBundle commitment",
                )?,
                prepared_at: super::from_i64(row.14, "preparation creation time")?,
                report: serde_json::from_str(&row.10)
                    .map_err(|error| WatchtowerError::Corrupt(error.to_string()))?,
                status: row.12,
                invalidation_reason: row.13,
            })
        })
        .transpose()
    }

    pub(crate) fn mark_offline_recheck_required(
        &self,
        funding_coin_id: Bytes32,
        reason: &str,
        now: u64,
    ) -> crate::Result<()> {
        self.update_preparations_for_funding(funding_coin_id, CHAIN_RECHECK_REQUIRED, reason, now)
    }

    pub(crate) fn reconcile_offline_preparation(
        &self,
        observation: &ClosingObservation,
        decision: &MonitorDecision,
        now: u64,
    ) -> crate::Result<()> {
        let Some(closing) = &observation.closing_coin else {
            return self.update_preparations_for_funding(
                observation.funding_coin.coin_id,
                INVALIDATED_CHAIN_CHANGE,
                "the prepared Closing Coin disappeared from the current chain view",
                now,
            );
        };
        let Some(prepared) = self.offline_preparation(closing.coin_id)? else {
            return Ok(());
        };
        let unchanged = prepared.snapshot.peak_height == observation.peak.height
            && prepared.snapshot.peak_header_hash == observation.peak.header_hash
            && prepared.snapshot.closing_coin_id == closing.coin_id
            && prepared.snapshot.closing_coin.parent_coin_info.to_bytes() == closing.parent_coin_id
            && prepared.snapshot.closing_coin.puzzle_hash.to_bytes() == closing.puzzle_hash
            && prepared.snapshot.closing_coin.amount == closing.amount
            && prepared.snapshot.closing_birth_height == closing.birth_height
            && closing.spent_height.is_none()
            && observation.peak.height < prepared.challenge_deadline_height
            && matches!(
                decision.action,
                MonitorAction::ChallengeAlreadyPlanned | MonitorAction::ChallengePlanned
            );
        if unchanged {
            return Ok(());
        }
        self.connection.execute(
            "UPDATE v36_offline_challenge_preparations SET
               status=?2, invalidation_reason=?3, updated_at=?4
             WHERE closing_coin_id=?1",
            params![
                closing.coin_id.as_slice(),
                INVALIDATED_CHAIN_CHANGE,
                format!("chain observation changed: {:?}", decision.action),
                super::to_i64(now)?,
            ],
        )?;
        self.revoke_approvals_for_closing(
            closing.coin_id,
            "chain observation changed after approval",
            now,
        )?;
        self.invalidate_final_rechecks_for_closing(
            closing.coin_id,
            "chain observation changed after final recheck",
            now,
        )?;
        self.invalidate_execution_manifests_for_closing(
            closing.coin_id,
            "chain observation changed after final recheck",
            now,
        )?;
        Ok(())
    }

    fn update_preparations_for_funding(
        &self,
        funding_coin_id: Bytes32,
        status: &str,
        reason: &str,
        now: u64,
    ) -> crate::Result<()> {
        self.connection.execute(
            "UPDATE v36_offline_challenge_preparations SET
               status=?2, invalidation_reason=?3, updated_at=?4
             WHERE funding_coin_id=?1 AND status IN (?5, ?6, ?7)",
            params![
                funding_coin_id.as_slice(),
                status,
                reason,
                super::to_i64(now)?,
                OFFLINE_VERIFIED_AWAITING_APPROVAL,
                crate::approval::PARTIALLY_APPROVED,
                crate::approval::DUAL_APPROVED_RECHECK_REQUIRED,
            ],
        )?;
        self.revoke_approvals_for_funding(funding_coin_id, reason, now)?;
        self.invalidate_final_rechecks_for_funding(funding_coin_id, reason, now)?;
        self.invalidate_execution_manifests_for_funding(funding_coin_id, reason, now)?;
        Ok(())
    }
}
