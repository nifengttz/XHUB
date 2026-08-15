use chia_bls::SecretKey;
use rusqlite::{OptionalExtension, params};
use xhub_protocol_v3_6::{
    Bytes32, PROTOCOL_VERSION, PublicKeyBytes, SignatureBytes, public_key_bytes, sha256_parts,
    sign_hash, verify_hash,
};

use crate::{WatchtowerError, WatchtowerStore, preparation::PersistedOfflinePreparation};

pub const APPROVAL_STATEMENT_DOMAIN: &[u8] = b"XHUB_CHALLENGE_APPROVAL_V3_6";
pub const APPROVAL_PREPARATION_DOMAIN: &[u8] = b"XHUB_CHALLENGE_PREPARATION_V3_6";
pub const APPROVAL_REPORT_DOMAIN: &[u8] = b"XHUB_CHALLENGE_REPORT_V3_6";

pub const AWAITING_APPROVAL: &str = "AWAITING_APPROVAL";
pub const PARTIALLY_APPROVED: &str = "PARTIALLY_APPROVED";
pub const DUAL_APPROVED_RECHECK_REQUIRED: &str = "DUAL_APPROVED_RECHECK_REQUIRED";
pub const APPROVAL_REVOKED_CHAIN_CHANGE: &str = "APPROVAL_REVOKED_CHAIN_CHANGE";
const ACTIVE_APPROVAL: &str = "ACTIVE";
const EXPIRED_APPROVAL: &str = "EXPIRED";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ApprovalDecision {
    Approve = 1,
    Reject = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalStatement {
    pub protocol_version: u16,
    pub preparation_id: Bytes32,
    pub closing_coin_id: Bytes32,
    pub funding_coin_id: Bytes32,
    pub fee_coin_id: Bytes32,
    pub report_hash: Bytes32,
    pub bundle_commitment: Bytes32,
    pub peak_height: u64,
    pub peak_header_hash: Bytes32,
    pub challenge_deadline_height: u64,
    pub approver_id: String,
    pub failure_domain: String,
    pub approver_public_key: PublicKeyBytes,
    pub decision: ApprovalDecision,
    pub issued_at: u64,
    pub expires_at: u64,
    pub nonce: Bytes32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedApproval {
    pub statement: ApprovalStatement,
    pub signature: SignatureBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalStatus {
    pub preparation_id: Bytes32,
    pub closing_coin_id: Bytes32,
    pub status: String,
    pub approver_count: u16,
    pub failure_domain_count: u16,
    pub broadcast_enabled: bool,
    pub broadcast_ready: bool,
    pub chain_broadcast: bool,
}

impl ApprovalStatement {
    pub fn for_preparation(
        preparation: &PersistedOfflinePreparation,
        approver_id: impl Into<String>,
        failure_domain: impl Into<String>,
        approver_public_key: PublicKeyBytes,
        issued_at: u64,
        expires_at: u64,
        nonce: Bytes32,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            preparation_id: preparation_id(preparation),
            closing_coin_id: preparation.closing_coin_id,
            funding_coin_id: preparation.funding_coin_id,
            fee_coin_id: preparation.fee_coin_id,
            report_hash: preparation.report_hash,
            bundle_commitment: preparation.bundle_commitment,
            peak_height: preparation.snapshot.peak_height,
            peak_header_hash: preparation.snapshot.peak_header_hash,
            challenge_deadline_height: preparation.challenge_deadline_height,
            approver_id: approver_id.into(),
            failure_domain: failure_domain.into(),
            approver_public_key,
            decision: ApprovalDecision::Approve,
            issued_at,
            expires_at,
            nonce,
        }
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(320);
        output.extend_from_slice(&self.protocol_version.to_be_bytes());
        output.extend_from_slice(&self.preparation_id);
        output.extend_from_slice(&self.closing_coin_id);
        output.extend_from_slice(&self.funding_coin_id);
        output.extend_from_slice(&self.fee_coin_id);
        output.extend_from_slice(&self.report_hash);
        output.extend_from_slice(&self.bundle_commitment);
        output.extend_from_slice(&self.peak_height.to_be_bytes());
        output.extend_from_slice(&self.peak_header_hash);
        output.extend_from_slice(&self.challenge_deadline_height.to_be_bytes());
        put_text(&mut output, &self.approver_id);
        put_text(&mut output, &self.failure_domain);
        output.extend_from_slice(&self.approver_public_key);
        output.push(self.decision as u8);
        output.extend_from_slice(&self.issued_at.to_be_bytes());
        output.extend_from_slice(&self.expires_at.to_be_bytes());
        output.extend_from_slice(&self.nonce);
        output
    }

    pub fn signing_hash(&self) -> Bytes32 {
        sha256_parts(&[APPROVAL_STATEMENT_DOMAIN, &self.canonical_bytes()])
    }

    pub(crate) fn from_canonical_bytes(input: &[u8]) -> crate::Result<Self> {
        let mut decoder = ApprovalDecoder::new(input);
        let protocol_version = decoder.u16()?;
        let preparation_id = decoder.fixed()?;
        let closing_coin_id = decoder.fixed()?;
        let funding_coin_id = decoder.fixed()?;
        let fee_coin_id = decoder.fixed()?;
        let report_hash = decoder.fixed()?;
        let bundle_commitment = decoder.fixed()?;
        let peak_height = decoder.u64()?;
        let peak_header_hash = decoder.fixed()?;
        let challenge_deadline_height = decoder.u64()?;
        let approver_id = decoder.text()?;
        let failure_domain = decoder.text()?;
        let approver_public_key = decoder.fixed()?;
        let decision = match decoder.u8()? {
            1 => ApprovalDecision::Approve,
            2 => ApprovalDecision::Reject,
            _ => {
                return Err(WatchtowerError::Corrupt(
                    "persisted approval decision is invalid".into(),
                ));
            }
        };
        let issued_at = decoder.u64()?;
        let expires_at = decoder.u64()?;
        let nonce = decoder.fixed()?;
        decoder.finish()?;
        Ok(Self {
            protocol_version,
            preparation_id,
            closing_coin_id,
            funding_coin_id,
            fee_coin_id,
            report_hash,
            bundle_commitment,
            peak_height,
            peak_header_hash,
            challenge_deadline_height,
            approver_id,
            failure_domain,
            approver_public_key,
            decision,
            issued_at,
            expires_at,
            nonce,
        })
    }
}

impl SignedApproval {
    pub fn sign(statement: ApprovalStatement, secret_key: &SecretKey) -> crate::Result<Self> {
        if public_key_bytes(secret_key) != statement.approver_public_key {
            return Err(WatchtowerError::Invalid(
                "approval secret key does not match the declared public key".into(),
            ));
        }
        let signature = sign_hash(secret_key, &statement.signing_hash());
        Ok(Self {
            statement,
            signature,
        })
    }
}

pub fn preparation_id(preparation: &PersistedOfflinePreparation) -> Bytes32 {
    sha256_parts(&[
        APPROVAL_PREPARATION_DOMAIN,
        &PROTOCOL_VERSION.to_be_bytes(),
        &preparation.closing_coin_id,
        &preparation.funding_coin_id,
        &preparation.fee_coin_id,
        &preparation.report_hash,
        &preparation.bundle_commitment,
        &preparation.snapshot.peak_height.to_be_bytes(),
        &preparation.snapshot.peak_header_hash,
        &preparation.challenge_deadline_height.to_be_bytes(),
        &preparation.prepared_at.to_be_bytes(),
    ])
}

impl WatchtowerStore {
    pub fn submit_challenge_approval(
        &self,
        approval: &SignedApproval,
        now: u64,
    ) -> crate::Result<ApprovalStatus> {
        validate_name("approver_id", &approval.statement.approver_id)?;
        validate_name("failure_domain", &approval.statement.failure_domain)?;
        if approval.statement.protocol_version != PROTOCOL_VERSION {
            return Err(WatchtowerError::Invalid(
                "approval protocol version mismatch".into(),
            ));
        }
        if approval.statement.decision != ApprovalDecision::Approve {
            return Err(WatchtowerError::Invalid(
                "a rejection cannot contribute to dual approval".into(),
            ));
        }
        if approval.statement.issued_at > now || approval.statement.expires_at <= now {
            return Err(WatchtowerError::Invalid(
                "approval credential is not currently valid".into(),
            ));
        }
        if approval.statement.expires_at <= approval.statement.issued_at {
            return Err(WatchtowerError::Invalid(
                "approval expiry must follow issuance".into(),
            ));
        }
        verify_hash(
            &approval.statement.approver_public_key,
            &approval.statement.signing_hash(),
            &approval.signature,
        )
        .map_err(|_| WatchtowerError::Invalid("approval signature is invalid".into()))?;

        let preparation = self
            .offline_preparation(approval.statement.closing_coin_id)?
            .ok_or_else(|| WatchtowerError::Invalid("offline preparation was not found".into()))?;
        if matches!(
            preparation.status.as_str(),
            crate::preparation::CHAIN_RECHECK_REQUIRED
                | crate::preparation::INVALIDATED_CHAIN_CHANGE
        ) {
            return Err(WatchtowerError::Invalid(format!(
                "offline preparation is not approvable in status {}",
                preparation.status
            )));
        }
        validate_binding(&approval.statement, &preparation)?;
        self.expire_approvals(approval.statement.preparation_id, now)?;

        let statement_blob = approval.statement.canonical_bytes();
        let existing = self
            .connection
            .query_row(
                "SELECT statement_blob, signature FROM v36_challenge_approvals
                 WHERE preparation_id=?1 AND approver_id=?2",
                params![
                    approval.statement.preparation_id.as_slice(),
                    approval.statement.approver_id,
                ],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()?;
        if let Some((persisted_statement, persisted_signature)) = existing {
            if persisted_statement == statement_blob && persisted_signature == approval.signature {
                return self.approval_status(approval.statement.closing_coin_id, now);
            }
            return Err(WatchtowerError::Invalid(
                "approver already submitted a different credential for this preparation".into(),
            ));
        }

        let current = self.approval_status(approval.statement.closing_coin_id, now)?;
        if current.status == DUAL_APPROVED_RECHECK_REQUIRED {
            return Err(WatchtowerError::Invalid(
                "offline preparation already has two active approvals".into(),
            ));
        }
        if !matches!(
            current.status.as_str(),
            AWAITING_APPROVAL | PARTIALLY_APPROVED
        ) {
            return Err(WatchtowerError::Invalid(format!(
                "offline preparation is not approvable in status {}",
                current.status
            )));
        }

        if self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM v36_challenge_approvals
             WHERE preparation_id=?1 AND failure_domain=?2 AND status=?3)",
            params![
                approval.statement.preparation_id.as_slice(),
                approval.statement.failure_domain,
                ACTIVE_APPROVAL
            ],
            |row| row.get::<_, bool>(0),
        )? {
            return Err(WatchtowerError::Invalid(
                "a failure domain may only contribute one active approval".into(),
            ));
        }

        self.connection
            .execute(
                "INSERT INTO v36_challenge_approvals (
               preparation_id, closing_coin_id, approver_id, failure_domain,
               approver_public_key, decision, issued_at, expires_at, nonce,
               statement_blob, signature, status, received_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    approval.statement.preparation_id.as_slice(),
                    approval.statement.closing_coin_id.as_slice(),
                    approval.statement.approver_id,
                    approval.statement.failure_domain,
                    approval.statement.approver_public_key.as_slice(),
                    approval.statement.decision as u8,
                    super::to_i64(approval.statement.issued_at)?,
                    super::to_i64(approval.statement.expires_at)?,
                    approval.statement.nonce.as_slice(),
                    statement_blob,
                    approval.signature.as_slice(),
                    ACTIVE_APPROVAL,
                    super::to_i64(now)?,
                ],
            )
            .map_err(|error| {
                if matches!(error, rusqlite::Error::SqliteFailure(_, _)) {
                    WatchtowerError::Invalid("duplicate approval public key or nonce".into())
                } else {
                    WatchtowerError::Database(error)
                }
            })?;

        let status = self.approval_status(approval.statement.closing_coin_id, now)?;
        self.connection.execute(
            "UPDATE v36_offline_challenge_preparations SET status=?2, updated_at=?3
             WHERE closing_coin_id=?1",
            params![
                approval.statement.closing_coin_id.as_slice(),
                status.status,
                super::to_i64(now)?,
            ],
        )?;
        Ok(status)
    }

    pub fn approval_status(
        &self,
        closing_coin_id: Bytes32,
        now: u64,
    ) -> crate::Result<ApprovalStatus> {
        let preparation = self
            .offline_preparation(closing_coin_id)?
            .ok_or_else(|| WatchtowerError::Invalid("offline preparation was not found".into()))?;
        let id = preparation_id(&preparation);
        self.expire_approvals(id, now)?;
        let (approvers, domains) = self.connection.query_row(
            "SELECT COUNT(DISTINCT approver_id), COUNT(DISTINCT failure_domain)
             FROM v36_challenge_approvals
             WHERE preparation_id=?1 AND status=?2 AND expires_at>?3",
            params![id.as_slice(), ACTIVE_APPROVAL, super::to_i64(now)?],
            |row| Ok((row.get::<_, u16>(0)?, row.get::<_, u16>(1)?)),
        )?;
        let status = if matches!(
            preparation.status.as_str(),
            crate::preparation::CHAIN_RECHECK_REQUIRED
                | crate::preparation::INVALIDATED_CHAIN_CHANGE
        ) {
            APPROVAL_REVOKED_CHAIN_CHANGE
        } else if approvers >= 2 && domains >= 2 {
            DUAL_APPROVED_RECHECK_REQUIRED
        } else if approvers == 1 && domains == 1 {
            PARTIALLY_APPROVED
        } else {
            AWAITING_APPROVAL
        };
        if !matches!(
            preparation.status.as_str(),
            crate::preparation::CHAIN_RECHECK_REQUIRED
                | crate::preparation::INVALIDATED_CHAIN_CHANGE
        ) && preparation.status != status
        {
            self.connection.execute(
                "UPDATE v36_offline_challenge_preparations SET status=?2, updated_at=?3
                 WHERE closing_coin_id=?1",
                params![closing_coin_id.as_slice(), status, super::to_i64(now)?,],
            )?;
        }
        Ok(ApprovalStatus {
            preparation_id: id,
            closing_coin_id,
            status: status.into(),
            approver_count: approvers,
            failure_domain_count: domains,
            broadcast_enabled: false,
            broadcast_ready: false,
            chain_broadcast: false,
        })
    }

    fn expire_approvals(&self, preparation_id: Bytes32, now: u64) -> crate::Result<()> {
        self.connection.execute(
            "UPDATE v36_challenge_approvals SET status=?2, revoked_at=?3,
               revocation_reason='approval credential expired'
             WHERE preparation_id=?1 AND status=?4 AND expires_at<=?3",
            params![
                preparation_id.as_slice(),
                EXPIRED_APPROVAL,
                super::to_i64(now)?,
                ACTIVE_APPROVAL,
            ],
        )?;
        Ok(())
    }

    pub(crate) fn revoke_approvals_for_funding(
        &self,
        funding_coin_id: Bytes32,
        reason: &str,
        now: u64,
    ) -> crate::Result<()> {
        self.connection.execute(
            "UPDATE v36_challenge_approvals SET status=?2, revoked_at=?3, revocation_reason=?4
             WHERE closing_coin_id IN (
               SELECT closing_coin_id FROM v36_offline_challenge_preparations
               WHERE funding_coin_id=?1
             ) AND status=?5",
            params![
                funding_coin_id.as_slice(),
                APPROVAL_REVOKED_CHAIN_CHANGE,
                super::to_i64(now)?,
                reason,
                ACTIVE_APPROVAL
            ],
        )?;
        Ok(())
    }

    pub(crate) fn revoke_approvals_for_closing(
        &self,
        closing_coin_id: Bytes32,
        reason: &str,
        now: u64,
    ) -> crate::Result<()> {
        self.connection.execute(
            "UPDATE v36_challenge_approvals SET status=?2, revoked_at=?3, revocation_reason=?4
             WHERE closing_coin_id=?1 AND status=?5",
            params![
                closing_coin_id.as_slice(),
                APPROVAL_REVOKED_CHAIN_CHANGE,
                super::to_i64(now)?,
                reason,
                ACTIVE_APPROVAL
            ],
        )?;
        Ok(())
    }
}

fn validate_binding(
    statement: &ApprovalStatement,
    preparation: &PersistedOfflinePreparation,
) -> crate::Result<()> {
    if statement.preparation_id != preparation_id(preparation)
        || statement.closing_coin_id != preparation.closing_coin_id
        || statement.funding_coin_id != preparation.funding_coin_id
        || statement.fee_coin_id != preparation.fee_coin_id
        || statement.report_hash != preparation.report_hash
        || statement.bundle_commitment != preparation.bundle_commitment
        || statement.peak_height != preparation.snapshot.peak_height
        || statement.peak_header_hash != preparation.snapshot.peak_header_hash
        || statement.challenge_deadline_height != preparation.challenge_deadline_height
    {
        return Err(WatchtowerError::Invalid(
            "approval is not bound to the current offline preparation".into(),
        ));
    }
    Ok(())
}

fn validate_name(field: &str, value: &str) -> crate::Result<()> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        Err(WatchtowerError::Invalid(format!(
            "{field} must contain 1..=256 non-control bytes"
        )))
    } else {
        Ok(())
    }
}

fn put_text(output: &mut Vec<u8>, value: &str) {
    let length = u16::try_from(value.len()).expect("validated approval text exceeds u16");
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(value.as_bytes());
}

struct ApprovalDecoder<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> ApprovalDecoder<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self { input, offset: 0 }
    }

    fn take(&mut self, length: usize) -> crate::Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| WatchtowerError::Corrupt("approval length overflow".into()))?;
        let value = self.input.get(self.offset..end).ok_or_else(|| {
            WatchtowerError::Corrupt("persisted approval statement is truncated".into())
        })?;
        self.offset = end;
        Ok(value)
    }

    fn fixed<const N: usize>(&mut self) -> crate::Result<[u8; N]> {
        self.take(N)?
            .try_into()
            .map_err(|_| WatchtowerError::Corrupt("persisted approval fixed field length".into()))
    }

    fn u8(&mut self) -> crate::Result<u8> {
        Ok(self.fixed::<1>()?[0])
    }

    fn u16(&mut self) -> crate::Result<u16> {
        Ok(u16::from_be_bytes(self.fixed()?))
    }

    fn u64(&mut self) -> crate::Result<u64> {
        Ok(u64::from_be_bytes(self.fixed()?))
    }

    fn text(&mut self) -> crate::Result<String> {
        let length = self.u16()? as usize;
        let bytes = self.take(length)?;
        let value = std::str::from_utf8(bytes)
            .map_err(|_| WatchtowerError::Corrupt("persisted approval text is not UTF-8".into()))?;
        validate_name("persisted approval text", value)?;
        Ok(value.into())
    }

    fn finish(self) -> crate::Result<()> {
        if self.offset == self.input.len() {
            Ok(())
        } else {
            Err(WatchtowerError::Corrupt(
                "persisted approval statement has trailing bytes".into(),
            ))
        }
    }
}
