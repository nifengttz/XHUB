use rusqlite::{OptionalExtension, Transaction, params};
use xhub_protocol_v3_6::{Bytes32, PROTOCOL_VERSION, sha256_parts};

use crate::{WatchtowerError, WatchtowerStore};

pub const EXECUTION_AUDIT_DOMAIN: &[u8] = b"XHUB_EXECUTION_AUDIT_V3_6";
pub const EXECUTION_AUDIT_GENESIS_DOMAIN: &[u8] = b"XHUB_EXECUTION_AUDIT_GENESIS_V3_6";
pub const EXECUTION_AUDIT_ANCHOR_DOMAIN: &[u8] = b"XHUB_EXECUTION_AUDIT_ANCHOR_V3_6";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionAuditHead {
    pub event_count: u64,
    pub head_hash: Bytes32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionAuditVerification {
    pub head: ExecutionAuditHead,
    pub valid: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionAuditAnchor {
    pub anchor_id: Bytes32,
    pub event_count: u64,
    pub head_hash: Bytes32,
    pub anchored_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionAuditAnchorCheck {
    pub anchor: ExecutionAuditAnchor,
    pub current: ExecutionAuditHead,
    pub valid: bool,
    pub rollback_detected: bool,
}

pub fn execution_audit_genesis_hash() -> Bytes32 {
    sha256_parts(&[
        EXECUTION_AUDIT_GENESIS_DOMAIN,
        &PROTOCOL_VERSION.to_be_bytes(),
    ])
}

impl WatchtowerStore {
    pub fn execution_audit_head(&self) -> crate::Result<ExecutionAuditHead> {
        let row = self
            .connection
            .query_row(
                "SELECT event_count, head_hash FROM v36_execution_audit_heads WHERE singleton=1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()?
            .ok_or_else(|| WatchtowerError::Corrupt("execution audit head was not found".into()))?;
        Ok(ExecutionAuditHead {
            event_count: super::from_i64(row.0, "execution audit count")?,
            head_hash: super::bytes32(row.1, "execution audit head hash")?,
        })
    }

    pub fn verify_execution_audit_chain(&self) -> crate::Result<ExecutionAuditVerification> {
        let head = self.execution_audit_head()?;
        let mut statement = self.connection.prepare(
            "SELECT event_index, event_hash, previous_hash, event_type, subject_id, binding_hash,
                    status, occurred_at
             FROM v36_execution_audit_events ORDER BY event_index ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Vec<u8>>(4)?,
                row.get::<_, Vec<u8>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })?;
        let mut previous_hash = execution_audit_genesis_hash();
        let mut expected_index = 1_u64;
        for row in rows {
            let row = row?;
            let event_index = super::from_i64(row.0, "execution audit event index")?;
            let event_hash = super::bytes32(row.1, "execution audit event hash")?;
            let stored_previous = super::bytes32(row.2, "execution audit previous hash")?;
            let subject_id = super::bytes32(row.4, "execution audit subject ID")?;
            let binding_hash = super::bytes32(row.5, "execution audit binding hash")?;
            let occurred_at = super::from_i64(row.7, "execution audit occurred time")?;
            let expected_hash = execution_audit_event_hash(
                event_index,
                previous_hash,
                &row.3,
                subject_id,
                binding_hash,
                &row.6,
                occurred_at,
            );
            if event_index != expected_index
                || stored_previous != previous_hash
                || event_hash != expected_hash
            {
                return Ok(ExecutionAuditVerification { head, valid: false });
            }
            previous_hash = event_hash;
            expected_index = expected_index.checked_add(1).ok_or_else(|| {
                WatchtowerError::Corrupt("execution audit event index overflow".into())
            })?;
        }
        Ok(ExecutionAuditVerification {
            valid: head.event_count == expected_index - 1 && head.head_hash == previous_hash,
            head,
        })
    }

    pub fn create_execution_audit_anchor(
        &self,
        anchored_at: u64,
    ) -> crate::Result<ExecutionAuditAnchor> {
        let verification = self.verify_execution_audit_chain()?;
        if !verification.valid {
            return Err(WatchtowerError::Corrupt(
                "cannot anchor an invalid execution audit chain".into(),
            ));
        }
        let anchor_id = sha256_parts(&[
            EXECUTION_AUDIT_ANCHOR_DOMAIN,
            &PROTOCOL_VERSION.to_be_bytes(),
            &verification.head.event_count.to_be_bytes(),
            &verification.head.head_hash,
            &anchored_at.to_be_bytes(),
        ]);
        let anchor = ExecutionAuditAnchor {
            anchor_id,
            event_count: verification.head.event_count,
            head_hash: verification.head.head_hash,
            anchored_at,
        };
        let changed = self.connection.execute(
            "INSERT OR IGNORE INTO v36_execution_audit_anchors
             (anchor_id, event_count, head_hash, anchored_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                anchor.anchor_id.as_slice(),
                super::to_i64(anchor.event_count)?,
                anchor.head_hash.as_slice(),
                super::to_i64(anchor.anchored_at)?
            ],
        )?;
        if changed == 0 {
            let existing = self
                .execution_audit_anchor(anchor.anchor_id)?
                .ok_or_else(|| WatchtowerError::Corrupt("audit anchor disappeared".into()))?;
            if existing != anchor {
                return Err(WatchtowerError::AuditAnchorConflict);
            }
        }
        Ok(anchor)
    }

    pub fn execution_audit_anchor(
        &self,
        anchor_id: Bytes32,
    ) -> crate::Result<Option<ExecutionAuditAnchor>> {
        let row = self
            .connection
            .query_row(
                "SELECT event_count, head_hash, anchored_at
                 FROM v36_execution_audit_anchors WHERE anchor_id=?1",
                [anchor_id.as_slice()],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(event_count, head_hash, anchored_at)| {
            Ok(ExecutionAuditAnchor {
                anchor_id,
                event_count: super::from_i64(event_count, "audit anchor event count")?,
                head_hash: super::bytes32(head_hash, "audit anchor head hash")?,
                anchored_at: super::from_i64(anchored_at, "audit anchor time")?,
            })
        })
        .transpose()
    }

    pub fn verify_execution_audit_anchor(
        &self,
        anchor: &ExecutionAuditAnchor,
    ) -> crate::Result<ExecutionAuditAnchorCheck> {
        let verification = self.verify_execution_audit_chain()?;
        let rollback_detected = verification.head.event_count < anchor.event_count;
        let prefix_matches = if rollback_detected {
            false
        } else if verification.head.event_count == anchor.event_count {
            verification.head.head_hash == anchor.head_hash
        } else {
            self.connection
                .query_row(
                    "SELECT event_hash FROM v36_execution_audit_events WHERE event_index=?1",
                    [super::to_i64(anchor.event_count)?],
                    |row| row.get::<_, Vec<u8>>(0),
                )
                .optional()?
                .map(|value| super::bytes32(value, "audit anchor prefix hash"))
                .transpose()?
                .is_some_and(|hash| hash == anchor.head_hash)
        };
        Ok(ExecutionAuditAnchorCheck {
            anchor: anchor.clone(),
            current: verification.head,
            valid: verification.valid && prefix_matches,
            rollback_detected,
        })
    }
}

pub(crate) fn append_execution_audit_event_in_transaction(
    transaction: &Transaction<'_>,
    event_type: &str,
    subject_id: Bytes32,
    binding_hash: Bytes32,
    status: &str,
    occurred_at: u64,
) -> crate::Result<ExecutionAuditHead> {
    let current = transaction.query_row(
        "SELECT event_count, head_hash FROM v36_execution_audit_heads WHERE singleton=1",
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
    )?;
    let event_count = super::from_i64(current.0, "execution audit count")?;
    let previous_hash = super::bytes32(current.1, "execution audit head hash")?;
    let event_index = event_count
        .checked_add(1)
        .ok_or_else(|| WatchtowerError::Invalid("execution audit event count overflow".into()))?;
    let event_hash = execution_audit_event_hash(
        event_index,
        previous_hash,
        event_type,
        subject_id,
        binding_hash,
        status,
        occurred_at,
    );
    transaction.execute(
        "INSERT INTO v36_execution_audit_events (
               event_index, event_hash, previous_hash, event_type, subject_id, binding_hash,
               status, occurred_at, broadcast_enabled, broadcast_ready, chain_broadcast
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, 0, 0)",
        params![
            super::to_i64(event_index)?,
            event_hash.as_slice(),
            previous_hash.as_slice(),
            event_type,
            subject_id.as_slice(),
            binding_hash.as_slice(),
            status,
            super::to_i64(occurred_at)?
        ],
    )?;
    let updated = transaction.execute(
        "UPDATE v36_execution_audit_heads SET event_count=?2, head_hash=?3
             WHERE singleton=1 AND event_count=?1 AND head_hash=?4",
        params![
            super::to_i64(event_count)?,
            super::to_i64(event_index)?,
            event_hash.as_slice(),
            previous_hash.as_slice()
        ],
    )?;
    if updated != 1 {
        return Err(WatchtowerError::Corrupt(
            "execution audit head changed during append".into(),
        ));
    }
    Ok(ExecutionAuditHead {
        event_count: event_index,
        head_hash: event_hash,
    })
}

fn execution_audit_event_hash(
    event_index: u64,
    previous_hash: Bytes32,
    event_type: &str,
    subject_id: Bytes32,
    binding_hash: Bytes32,
    status: &str,
    occurred_at: u64,
) -> Bytes32 {
    sha256_parts(&[
        EXECUTION_AUDIT_DOMAIN,
        &PROTOCOL_VERSION.to_be_bytes(),
        &event_index.to_be_bytes(),
        &previous_hash,
        &(event_type.len() as u64).to_be_bytes(),
        event_type.as_bytes(),
        &subject_id,
        &binding_hash,
        &(status.len() as u64).to_be_bytes(),
        status.as_bytes(),
        &occurred_at.to_be_bytes(),
    ])
}
