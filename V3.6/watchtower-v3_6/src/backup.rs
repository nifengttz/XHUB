use std::{
    fs,
    path::{Path, PathBuf},
};

use chacha20poly1305::{AeadInPlace, KeyInit, XChaCha20Poly1305, XNonce};
use rand::RngCore;
use xhub_protocol_v3_6::{
    Bytes32, CanonicalDecode, CanonicalEncode, Decoder, PROTOCOL_VERSION, put_bool, put_bytes,
    put_u16, put_u64, sha256_parts,
};
use zeroize::{Zeroize, Zeroizing};

use crate::{
    WatchtowerError, WatchtowerStore,
    audit::{ExecutionAuditAnchor, append_execution_audit_event_in_transaction},
};

pub const BACKUP_MANIFEST_DOMAIN: &[u8] = b"XHUB_WATCHTOWER_BACKUP_MANIFEST_V3_6";
pub const ENCRYPTED_BACKUP_DOMAIN: &[u8] = b"XHUB_WATCHTOWER_ENCRYPTED_BACKUP_V1";
pub const BACKUP_ARTIFACT_HANDOFF_DOMAIN: &[u8] = b"XHUB_WATCHTOWER_BACKUP_ARTIFACT_HANDOFF_V3_6";
pub const BACKUP_HANDOFF_RECEIVED: &str = "RECEIVED";
pub const BACKUP_HANDOFF_VERIFIED: &str = "VERIFIED";
pub const BACKUP_HANDOFF_REJECTED: &str = "REJECTED";
pub const BACKUP_RESTORE_DRILL_DOMAIN: &[u8] = b"XHUB_WATCHTOWER_BACKUP_RESTORE_DRILL_V3_6";
pub const BACKUP_RESTORE_DRILL_PASSED: &str = "PASSED";
pub const BACKUP_RESTORE_DRILL_FAILED: &str = "FAILED";
const ENCRYPTED_BACKUP_MAGIC: &[u8; 8] = b"XHUBBK01";
const ARTIFACT_MANIFEST_MAGIC: &[u8; 8] = b"XHUBAM01";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseBackupManifest {
    pub backup_id: Bytes32,
    pub file_hash: Bytes32,
    pub size_bytes: u64,
    pub audit_event_count: u64,
    pub audit_head_hash: Bytes32,
    pub anchor_id: Option<Bytes32>,
    pub created_at: u64,
}

impl DatabaseBackupManifest {
    pub fn validate(&self) -> crate::Result<()> {
        let expected = database_backup_id(
            self.file_hash,
            self.size_bytes,
            self.audit_event_count,
            self.audit_head_hash,
            self.anchor_id,
            self.created_at,
        );
        if expected != self.backup_id {
            return Err(WatchtowerError::Invalid(
                "backup manifest identity does not match its fields".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupVerification {
    pub manifest: DatabaseBackupManifest,
    pub file_exists: bool,
    pub hash_matches: bool,
    pub size_matches: bool,
    pub audit_valid: bool,
    pub anchor_valid: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedBackupArtifact {
    pub manifest: DatabaseBackupManifest,
    pub envelope_hash: Bytes32,
    pub key_id: Bytes32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupArtifactHandoff {
    pub artifact_hash: Bytes32,
    pub backup_id: Bytes32,
    pub envelope_hash: Bytes32,
    pub key_id: Bytes32,
    pub manifest_bytes_hash: Bytes32,
    pub received_at: u64,
    pub verified_at: Option<u64>,
    pub status: String,
    pub rejection_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupRestoreDrill {
    pub drill_id: Bytes32,
    pub artifact_hash: Bytes32,
    pub backup_id: Bytes32,
    pub started_at: u64,
    pub completed_at: u64,
    pub duration_seconds: u64,
    pub hash_matches: bool,
    pub size_matches: bool,
    pub audit_valid: bool,
    pub anchor_valid: Option<bool>,
    pub status: String,
    pub failure_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupRetentionPolicy {
    pub keep_latest: usize,
    pub minimum_age_seconds: u64,
}

impl CanonicalEncode for EncryptedBackupArtifact {
    fn encode_to(&self, output: &mut Vec<u8>) {
        output.extend_from_slice(ARTIFACT_MANIFEST_MAGIC);
        put_bytes(output, ENCRYPTED_BACKUP_DOMAIN);
        put_u16(output, PROTOCOL_VERSION);
        output.extend_from_slice(&self.manifest.backup_id);
        output.extend_from_slice(&self.manifest.file_hash);
        put_u64(output, self.manifest.size_bytes);
        put_u64(output, self.manifest.audit_event_count);
        output.extend_from_slice(&self.manifest.audit_head_hash);
        put_bool(output, self.manifest.anchor_id.is_some());
        if let Some(anchor_id) = self.manifest.anchor_id {
            output.extend_from_slice(&anchor_id);
        }
        put_u64(output, self.manifest.created_at);
        output.extend_from_slice(&self.envelope_hash);
        output.extend_from_slice(&self.key_id);
    }
}

impl CanonicalDecode for EncryptedBackupArtifact {
    fn decode_from(decoder: &mut Decoder<'_>) -> xhub_protocol_v3_6::Result<Self> {
        if decoder.take::<8>()? != *ARTIFACT_MANIFEST_MAGIC {
            return Err(xhub_protocol_v3_6::ProtocolError::InvalidStatus(0));
        }
        if decoder.bytes(128)? != ENCRYPTED_BACKUP_DOMAIN {
            return Err(xhub_protocol_v3_6::ProtocolError::EvidenceContext);
        }
        let version = decoder.u16()?;
        if version != PROTOCOL_VERSION {
            return Err(xhub_protocol_v3_6::ProtocolError::InvalidStatus(version));
        }
        let backup_id = decoder.take::<32>()?;
        let file_hash = decoder.take::<32>()?;
        let size_bytes = decoder.u64()?;
        let audit_event_count = decoder.u64()?;
        let audit_head_hash = decoder.take::<32>()?;
        let anchor_id = if decoder.bool()? {
            Some(decoder.take::<32>()?)
        } else {
            None
        };
        let created_at = decoder.u64()?;
        let envelope_hash = decoder.take::<32>()?;
        let key_id = decoder.take::<32>()?;
        Ok(Self {
            manifest: DatabaseBackupManifest {
                backup_id,
                file_hash,
                size_bytes,
                audit_event_count,
                audit_head_hash,
                anchor_id,
                created_at,
            },
            envelope_hash,
            key_id,
        })
    }
}

pub fn encode_encrypted_backup_artifact(artifact: &EncryptedBackupArtifact) -> Vec<u8> {
    artifact.canonical_bytes()
}

pub fn decode_encrypted_backup_artifact(bytes: &[u8]) -> crate::Result<EncryptedBackupArtifact> {
    let artifact = EncryptedBackupArtifact::from_canonical_bytes(bytes)?;
    artifact.manifest.validate()?;
    Ok(artifact)
}

pub fn encrypted_backup_artifact_hash(bytes: &[u8]) -> Bytes32 {
    sha256_parts(&[BACKUP_ARTIFACT_HANDOFF_DOMAIN, bytes])
}

impl WatchtowerStore {
    pub fn record_backup_artifact_handoff(
        &self,
        artifact_bytes: &[u8],
        received_at: u64,
    ) -> crate::Result<BackupArtifactHandoff> {
        let artifact = decode_encrypted_backup_artifact(artifact_bytes)?;
        let artifact_hash = encrypted_backup_artifact_hash(artifact_bytes);
        let manifest_bytes_hash = sha256_parts(&[BACKUP_ARTIFACT_HANDOFF_DOMAIN, artifact_bytes]);
        let tx = self.connection.unchecked_transaction()?;
        if let Some(existing) = load_handoff(&tx, artifact_hash)? {
            if existing.backup_id != artifact.manifest.backup_id
                || existing.envelope_hash != artifact.envelope_hash
                || existing.key_id != artifact.key_id
                || existing.manifest_bytes_hash != manifest_bytes_hash
            {
                return Err(WatchtowerError::Invalid(
                    "backup artifact handoff conflicts with persisted data".into(),
                ));
            }
            tx.commit()?;
            return Ok(existing);
        }
        let inserted = tx.execute(
            "INSERT INTO v36_backup_artifact_handoffs (
               artifact_hash, backup_id, envelope_hash, key_id, manifest_bytes_hash,
               received_at, status
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                artifact_hash.as_slice(),
                artifact.manifest.backup_id.as_slice(),
                artifact.envelope_hash.as_slice(),
                artifact.key_id.as_slice(),
                manifest_bytes_hash.as_slice(),
                super::to_i64(received_at)?,
                BACKUP_HANDOFF_RECEIVED,
            ],
        )?;
        if inserted != 1 {
            return Err(WatchtowerError::Corrupt(
                "backup handoff insert failed".into(),
            ));
        }
        append_execution_audit_event_in_transaction(
            &tx,
            "BACKUP_ARTIFACT_RECEIVED",
            artifact.manifest.backup_id,
            artifact_hash,
            BACKUP_HANDOFF_RECEIVED,
            received_at,
        )?;
        tx.commit()?;
        self.backup_artifact_handoff(artifact_hash)?
            .ok_or_else(|| WatchtowerError::Corrupt("backup handoff was not persisted".into()))
    }

    pub fn verify_backup_artifact_handoff<P: BackupKeyProvider>(
        &self,
        artifact_bytes: &[u8],
        encrypted_path: impl AsRef<Path>,
        anchor: Option<&ExecutionAuditAnchor>,
        key_provider: &P,
        verified_at: u64,
    ) -> crate::Result<BackupArtifactHandoff> {
        let artifact = decode_encrypted_backup_artifact(artifact_bytes)?;
        let artifact_hash = encrypted_backup_artifact_hash(artifact_bytes);
        let existing = self
            .backup_artifact_handoff(artifact_hash)?
            .ok_or_else(|| WatchtowerError::Invalid("backup artifact was not received".into()))?;
        if existing.backup_id != artifact.manifest.backup_id
            || existing.envelope_hash != artifact.envelope_hash
            || existing.key_id != artifact.key_id
        {
            return Err(WatchtowerError::Invalid(
                "backup artifact handoff does not match the supplied artifact".into(),
            ));
        }
        if existing.status == BACKUP_HANDOFF_VERIFIED {
            return Ok(existing);
        }
        if existing.status == BACKUP_HANDOFF_REJECTED {
            return Err(WatchtowerError::Invalid(
                "backup artifact handoff was previously rejected".into(),
            ));
        }
        let encrypted_path = encrypted_path.as_ref();
        let envelope = fs::read(encrypted_path).map_err(|error| {
            WatchtowerError::Invalid(format!("encrypted backup cannot be read: {error}"))
        })?;
        let envelope_hash = sha256_parts(&[ENCRYPTED_BACKUP_DOMAIN, &envelope]);
        if envelope_hash != artifact.envelope_hash {
            let reason = "encrypted backup envelope hash does not match the artifact";
            self.reject_backup_artifact_handoff(artifact_hash, reason, verified_at)?;
            return Err(WatchtowerError::Invalid(reason.into()));
        }
        let plaintext_temp = temporary_sibling(encrypted_path, "handoff")?;
        let result = (|| {
            let key = key_provider.load_backup_key(artifact.key_id)?;
            Self::decrypt_database_backup(encrypted_path, &plaintext_temp, artifact.key_id, &key)?;
            let verification =
                Self::verify_database_backup_state(&plaintext_temp, &artifact.manifest, anchor)?;
            if !verification.hash_matches
                || !verification.size_matches
                || !verification.audit_valid
                || verification.anchor_valid == Some(false)
            {
                return Err(WatchtowerError::Invalid(
                    "backup artifact failed verification".into(),
                ));
            }
            Ok(())
        })();
        remove_if_exists(&plaintext_temp);
        if let Err(error) = result {
            let reason = error.to_string();
            let _ = self.reject_backup_artifact_handoff(artifact_hash, &reason, verified_at);
            return Err(error);
        }
        let tx = self.connection.unchecked_transaction()?;
        tx.execute(
            "UPDATE v36_backup_artifact_handoffs SET status=?2, verified_at=?3,
               rejection_reason=NULL WHERE artifact_hash=?1 AND status=?4",
            rusqlite::params![
                artifact_hash.as_slice(),
                BACKUP_HANDOFF_VERIFIED,
                super::to_i64(verified_at)?,
                BACKUP_HANDOFF_RECEIVED,
            ],
        )?;
        append_execution_audit_event_in_transaction(
            &tx,
            "BACKUP_ARTIFACT_VERIFIED",
            artifact.manifest.backup_id,
            artifact_hash,
            BACKUP_HANDOFF_VERIFIED,
            verified_at,
        )?;
        tx.commit()?;
        self.backup_artifact_handoff(artifact_hash)?
            .ok_or_else(|| WatchtowerError::Corrupt("verified backup handoff disappeared".into()))
    }

    fn reject_backup_artifact_handoff(
        &self,
        artifact_hash: Bytes32,
        reason: &str,
        rejected_at: u64,
    ) -> crate::Result<BackupArtifactHandoff> {
        let existing = self
            .backup_artifact_handoff(artifact_hash)?
            .ok_or_else(|| WatchtowerError::Invalid("backup artifact was not received".into()))?;
        if existing.status == BACKUP_HANDOFF_VERIFIED {
            return Err(WatchtowerError::Invalid(
                "verified backup artifact cannot be rejected".into(),
            ));
        }
        let tx = self.connection.unchecked_transaction()?;
        tx.execute(
            "UPDATE v36_backup_artifact_handoffs SET status=?2, rejection_reason=?3
             WHERE artifact_hash=?1 AND status=?4",
            rusqlite::params![
                artifact_hash.as_slice(),
                BACKUP_HANDOFF_REJECTED,
                reason,
                BACKUP_HANDOFF_RECEIVED,
            ],
        )?;
        append_execution_audit_event_in_transaction(
            &tx,
            "BACKUP_ARTIFACT_REJECTED",
            existing.backup_id,
            artifact_hash,
            BACKUP_HANDOFF_REJECTED,
            rejected_at,
        )?;
        tx.commit()?;
        self.backup_artifact_handoff(artifact_hash)?
            .ok_or_else(|| WatchtowerError::Corrupt("rejected backup handoff disappeared".into()))
    }

    pub fn backup_artifact_handoff(
        &self,
        artifact_hash: Bytes32,
    ) -> crate::Result<Option<BackupArtifactHandoff>> {
        load_handoff(&self.connection, artifact_hash)
    }

    pub fn run_backup_restore_drill<P: BackupKeyProvider>(
        &self,
        artifact_bytes: &[u8],
        encrypted_path: impl AsRef<Path>,
        anchor: Option<&ExecutionAuditAnchor>,
        key_provider: &P,
        started_at: u64,
        completed_at: u64,
    ) -> crate::Result<BackupRestoreDrill> {
        if completed_at < started_at {
            return Err(WatchtowerError::Invalid(
                "backup restore drill completion precedes its start".into(),
            ));
        }
        let artifact = decode_encrypted_backup_artifact(artifact_bytes)?;
        let artifact_hash = encrypted_backup_artifact_hash(artifact_bytes);
        let handoff = self
            .backup_artifact_handoff(artifact_hash)?
            .ok_or_else(|| WatchtowerError::Invalid("backup artifact was not received".into()))?;
        if handoff.status != BACKUP_HANDOFF_VERIFIED {
            return Err(WatchtowerError::Invalid(
                "backup restore drill requires a verified handoff".into(),
            ));
        }
        let drill_id = sha256_parts(&[
            BACKUP_RESTORE_DRILL_DOMAIN,
            &PROTOCOL_VERSION.to_be_bytes(),
            &artifact_hash,
            &started_at.to_be_bytes(),
            &completed_at.to_be_bytes(),
        ]);
        if let Some(existing) = self.backup_restore_drill(drill_id)? {
            return Ok(existing);
        }
        let encrypted_path = encrypted_path.as_ref();
        let plaintext_temp = temporary_sibling(encrypted_path, "drill")?;
        let verification = (|| {
            let key = key_provider.load_backup_key(artifact.key_id)?;
            Self::decrypt_database_backup(encrypted_path, &plaintext_temp, artifact.key_id, &key)?;
            Self::verify_database_backup_state(&plaintext_temp, &artifact.manifest, anchor)
        })();
        remove_if_exists(&plaintext_temp);
        let (hash_matches, size_matches, audit_valid, anchor_valid, status, failure_reason) =
            match verification {
                Ok(value)
                    if value.hash_matches
                        && value.size_matches
                        && value.audit_valid
                        && value.anchor_valid != Some(false) =>
                {
                    (
                        true,
                        true,
                        true,
                        value.anchor_valid,
                        BACKUP_RESTORE_DRILL_PASSED,
                        None,
                    )
                }
                Ok(value) => (
                    value.hash_matches,
                    value.size_matches,
                    value.audit_valid,
                    value.anchor_valid,
                    BACKUP_RESTORE_DRILL_FAILED,
                    Some("restored database verification failed".to_string()),
                ),
                Err(error) => (
                    false,
                    false,
                    false,
                    None,
                    BACKUP_RESTORE_DRILL_FAILED,
                    Some(error.to_string()),
                ),
            };
        let drill = BackupRestoreDrill {
            drill_id,
            artifact_hash,
            backup_id: artifact.manifest.backup_id,
            started_at,
            completed_at,
            duration_seconds: completed_at - started_at,
            hash_matches,
            size_matches,
            audit_valid,
            anchor_valid,
            status: status.to_string(),
            failure_reason,
        };
        let tx = self.connection.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO v36_backup_restore_drills (
               drill_id, artifact_hash, backup_id, started_at, completed_at,
               duration_seconds, hash_matches, size_matches, audit_valid,
               anchor_valid, status, failure_reason
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            rusqlite::params![
                drill.drill_id.as_slice(),
                drill.artifact_hash.as_slice(),
                drill.backup_id.as_slice(),
                super::to_i64(drill.started_at)?,
                super::to_i64(drill.completed_at)?,
                super::to_i64(drill.duration_seconds)?,
                drill.hash_matches,
                drill.size_matches,
                drill.audit_valid,
                drill.anchor_valid,
                drill.status,
                drill.failure_reason,
            ],
        )?;
        append_execution_audit_event_in_transaction(
            &tx,
            "BACKUP_RESTORE_DRILL_COMPLETED",
            drill.backup_id,
            drill.drill_id,
            &drill.status,
            completed_at,
        )?;
        tx.commit()?;
        self.backup_restore_drill(drill_id)?
            .ok_or_else(|| WatchtowerError::Corrupt("backup restore drill disappeared".into()))
    }

    pub fn backup_restore_drill(
        &self,
        drill_id: Bytes32,
    ) -> crate::Result<Option<BackupRestoreDrill>> {
        load_restore_drill(&self.connection, drill_id)
    }

    pub fn backup_retention_candidates(
        &self,
        manifests: &[DatabaseBackupManifest],
        policy: BackupRetentionPolicy,
        now: u64,
    ) -> crate::Result<Vec<Bytes32>> {
        if policy.keep_latest == 0 {
            return Err(WatchtowerError::Invalid(
                "backup retention must keep at least one backup".into(),
            ));
        }
        for manifest in manifests {
            manifest.validate()?;
        }
        let mut ordered = manifests.to_vec();
        ordered.sort_by_key(|manifest| std::cmp::Reverse(manifest.created_at));
        let mut candidates = Vec::new();
        for manifest in ordered.into_iter().skip(policy.keep_latest) {
            let age = now.checked_sub(manifest.created_at).ok_or_else(|| {
                WatchtowerError::Invalid("backup creation time is in the future".into())
            })?;
            if age < policy.minimum_age_seconds {
                continue;
            }
            let passed: bool = self.connection.query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM v36_backup_restore_drills
                   WHERE backup_id=?1 AND status=?2
                 )",
                rusqlite::params![manifest.backup_id.as_slice(), BACKUP_RESTORE_DRILL_PASSED],
                |row| row.get(0),
            )?;
            if passed {
                candidates.push(manifest.backup_id);
            }
        }
        Ok(candidates)
    }
}

pub trait BackupKeyProvider {
    fn load_backup_key(&self, key_id: Bytes32) -> crate::Result<Zeroizing<[u8; 32]>>;
}

pub fn backup_replicas_are_consistent(manifests: &[DatabaseBackupManifest]) -> bool {
    let Some(first) = manifests.first() else {
        return false;
    };
    manifests.iter().all(|manifest| {
        manifest.file_hash == first.file_hash
            && manifest.size_bytes == first.size_bytes
            && manifest.audit_event_count == first.audit_event_count
            && manifest.audit_head_hash == first.audit_head_hash
            && manifest.anchor_id == first.anchor_id
    })
}

/// Compares replica metadata after decryption. Random envelope nonces and
/// independently rotated key IDs are intentionally excluded from the match.
pub fn encrypted_backup_replicas_are_consistent(artifacts: &[EncryptedBackupArtifact]) -> bool {
    let Some(first) = artifacts.first() else {
        return false;
    };
    artifacts
        .iter()
        .all(|artifact| artifact.manifest == first.manifest && artifact.manifest.validate().is_ok())
}

pub fn verified_backup_handoffs_are_consistent(handoffs: &[BackupArtifactHandoff]) -> bool {
    let Some(first) = handoffs.first() else {
        return false;
    };
    first.status == BACKUP_HANDOFF_VERIFIED
        && handoffs.iter().all(|handoff| {
            handoff.status == BACKUP_HANDOFF_VERIFIED
                && handoff.backup_id == first.backup_id
                && handoff.envelope_hash == first.envelope_hash
                && handoff.manifest_bytes_hash == first.manifest_bytes_hash
        })
}

impl WatchtowerStore {
    pub fn create_encrypted_database_backup<P: BackupKeyProvider>(
        &self,
        destination: impl AsRef<Path>,
        created_at: u64,
        anchor: Option<&ExecutionAuditAnchor>,
        key_id: Bytes32,
        key_provider: &P,
    ) -> crate::Result<EncryptedBackupArtifact> {
        let destination = destination.as_ref();
        if destination.exists() {
            return Err(WatchtowerError::Invalid(
                "encrypted backup destination already exists".into(),
            ));
        }
        let key = key_provider.load_backup_key(key_id)?;
        let plaintext_temp = temporary_sibling(destination, "plaintext")?;
        let encrypted_temp = temporary_sibling(destination, "encrypted")?;
        let result = (|| {
            let manifest = self.create_database_backup(&plaintext_temp, created_at, anchor)?;
            let envelope_hash =
                Self::encrypt_database_backup(&plaintext_temp, &encrypted_temp, key_id, &key)?;
            fs::rename(&encrypted_temp, destination).map_err(|error| {
                WatchtowerError::Invalid(format!("encrypted backup cannot be published: {error}"))
            })?;
            Ok(EncryptedBackupArtifact {
                manifest,
                envelope_hash,
                key_id,
            })
        })();
        remove_if_exists(&plaintext_temp);
        remove_if_exists(&encrypted_temp);
        result
    }

    pub fn restore_encrypted_database_backup<P: BackupKeyProvider>(
        encrypted_path: impl AsRef<Path>,
        destination: impl AsRef<Path>,
        artifact: &EncryptedBackupArtifact,
        anchor: Option<&ExecutionAuditAnchor>,
        key_provider: &P,
    ) -> crate::Result<BackupVerification> {
        let encrypted_path = encrypted_path.as_ref();
        let destination = destination.as_ref();
        artifact.manifest.validate()?;
        if destination.exists() {
            return Err(WatchtowerError::Invalid(
                "restore destination already exists".into(),
            ));
        }
        let envelope = fs::read(encrypted_path).map_err(|error| {
            WatchtowerError::Invalid(format!("encrypted backup cannot be read: {error}"))
        })?;
        let envelope_hash = sha256_parts(&[ENCRYPTED_BACKUP_DOMAIN, &envelope]);
        if envelope_hash != artifact.envelope_hash {
            return Err(WatchtowerError::Invalid(
                "encrypted backup envelope hash does not match the artifact".into(),
            ));
        }
        let key = key_provider.load_backup_key(artifact.key_id)?;
        let plaintext_temp = temporary_sibling(destination, "restore")?;
        let result = (|| {
            Self::decrypt_database_backup(encrypted_path, &plaintext_temp, artifact.key_id, &key)?;
            let verification =
                Self::verify_database_backup_state(&plaintext_temp, &artifact.manifest, anchor)?;
            if !verification.hash_matches
                || !verification.size_matches
                || !verification.audit_valid
                || verification.anchor_valid == Some(false)
            {
                return Err(WatchtowerError::Invalid(
                    "decrypted backup failed restore verification".into(),
                ));
            }
            fs::rename(&plaintext_temp, destination).map_err(|error| {
                WatchtowerError::Invalid(format!("restored database cannot be published: {error}"))
            })?;
            Ok(verification)
        })();
        remove_if_exists(&plaintext_temp);
        result
    }

    pub fn create_database_backup(
        &self,
        destination: impl AsRef<Path>,
        created_at: u64,
        anchor: Option<&ExecutionAuditAnchor>,
    ) -> crate::Result<DatabaseBackupManifest> {
        let destination = destination.as_ref();
        if destination.exists() {
            return Err(WatchtowerError::Invalid(
                "backup destination already exists".into(),
            ));
        }
        let verification = self.verify_execution_audit_chain()?;
        if !verification.valid {
            return Err(WatchtowerError::Corrupt(
                "cannot back up an invalid execution audit chain".into(),
            ));
        }
        if let Some(anchor) = anchor {
            let checked = self.verify_execution_audit_anchor(anchor)?;
            if !checked.valid {
                return Err(WatchtowerError::Invalid(
                    "backup anchor does not match the current audit chain".into(),
                ));
            }
        }
        let destination_text = destination.to_string_lossy().into_owned();
        self.connection
            .execute("VACUUM INTO ?1", [&destination_text])?;
        let bytes = fs::read(destination).map_err(|error| {
            WatchtowerError::Invalid(format!("backup output cannot be read: {error}"))
        })?;
        let file_hash = sha256_parts(&[BACKUP_MANIFEST_DOMAIN, &bytes]);
        let size_bytes = bytes.len() as u64;
        let anchor_id = anchor.map(|value| value.anchor_id);
        let backup_id = database_backup_id(
            file_hash,
            size_bytes,
            verification.head.event_count,
            verification.head.head_hash,
            anchor_id,
            created_at,
        );
        Ok(DatabaseBackupManifest {
            backup_id,
            file_hash,
            size_bytes,
            audit_event_count: verification.head.event_count,
            audit_head_hash: verification.head.head_hash,
            anchor_id,
            created_at,
        })
    }

    pub fn verify_database_backup(
        path: impl AsRef<Path>,
        manifest: &DatabaseBackupManifest,
    ) -> crate::Result<BackupVerification> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(BackupVerification {
                manifest: manifest.clone(),
                file_exists: false,
                hash_matches: false,
                size_matches: false,
                audit_valid: false,
                anchor_valid: None,
            });
        }
        let bytes = fs::read(path)
            .map_err(|error| WatchtowerError::Invalid(format!("backup cannot be read: {error}")))?;
        let file_hash = sha256_parts(&[BACKUP_MANIFEST_DOMAIN, &bytes]);
        let size_bytes = bytes.len() as u64;
        Ok(BackupVerification {
            manifest: manifest.clone(),
            file_exists: true,
            hash_matches: file_hash == manifest.file_hash,
            size_matches: size_bytes == manifest.size_bytes,
            audit_valid: false,
            anchor_valid: None,
        })
    }

    pub fn verify_database_backup_state(
        path: impl AsRef<Path>,
        manifest: &DatabaseBackupManifest,
        anchor: Option<&ExecutionAuditAnchor>,
    ) -> crate::Result<BackupVerification> {
        let file_check = Self::verify_database_backup(&path, manifest)?;
        if !file_check.file_exists || !file_check.hash_matches || !file_check.size_matches {
            return Ok(file_check);
        }
        let store = WatchtowerStore::open(path)?;
        let audit_valid = store.verify_execution_audit_chain()?.valid;
        let anchor_valid = anchor.map(|value| {
            store
                .verify_execution_audit_anchor(value)
                .map(|check| check.valid)
        });
        let anchor_valid = anchor_valid.transpose()?;
        Ok(BackupVerification {
            audit_valid,
            anchor_valid,
            ..file_check
        })
    }

    pub fn encrypt_database_backup(
        plaintext_path: impl AsRef<Path>,
        encrypted_path: impl AsRef<Path>,
        key_id: Bytes32,
        key: &[u8; 32],
    ) -> crate::Result<Bytes32> {
        let plaintext_path = plaintext_path.as_ref();
        let encrypted_path = encrypted_path.as_ref();
        if encrypted_path.exists() {
            return Err(WatchtowerError::Invalid(
                "encrypted backup destination already exists".into(),
            ));
        }
        let mut plaintext = fs::read(plaintext_path).map_err(|error| {
            WatchtowerError::Invalid(format!("backup plaintext cannot be read: {error}"))
        })?;
        let mut nonce = [0_u8; 24];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let cipher = XChaCha20Poly1305::new_from_slice(key)
            .map_err(|_| WatchtowerError::Invalid("backup key must be 32 bytes".into()))?;
        let aad = backup_aad(key_id);
        let tag = cipher
            .encrypt_in_place_detached(XNonce::from_slice(&nonce), &aad, &mut plaintext)
            .map_err(|_| WatchtowerError::Invalid("backup encryption failed".into()))?;
        let mut envelope = Vec::with_capacity(8 + 2 + 32 + 24 + 8 + plaintext.len() + 16);
        envelope.extend_from_slice(ENCRYPTED_BACKUP_MAGIC);
        envelope.extend_from_slice(&PROTOCOL_VERSION.to_be_bytes());
        envelope.extend_from_slice(&key_id);
        envelope.extend_from_slice(&nonce);
        envelope.extend_from_slice(&(plaintext.len() as u64).to_be_bytes());
        envelope.extend_from_slice(&plaintext);
        envelope.extend_from_slice(&tag);
        fs::write(encrypted_path, &envelope).map_err(|error| {
            WatchtowerError::Invalid(format!("encrypted backup cannot be written: {error}"))
        })?;
        Ok(sha256_parts(&[ENCRYPTED_BACKUP_DOMAIN, &envelope]))
    }

    pub fn decrypt_database_backup(
        encrypted_path: impl AsRef<Path>,
        plaintext_path: impl AsRef<Path>,
        expected_key_id: Bytes32,
        key: &[u8; 32],
    ) -> crate::Result<Bytes32> {
        let encrypted_path = encrypted_path.as_ref();
        let plaintext_path = plaintext_path.as_ref();
        if plaintext_path.exists() {
            return Err(WatchtowerError::Invalid(
                "decrypted backup destination already exists".into(),
            ));
        }
        let envelope = fs::read(encrypted_path).map_err(|error| {
            WatchtowerError::Invalid(format!("encrypted backup cannot be read: {error}"))
        })?;
        if envelope.len() < 8 + 2 + 32 + 24 + 8 + 16 || &envelope[..8] != ENCRYPTED_BACKUP_MAGIC {
            return Err(WatchtowerError::Invalid(
                "encrypted backup header is invalid".into(),
            ));
        }
        let version = u16::from_be_bytes(envelope[8..10].try_into().unwrap());
        if version != PROTOCOL_VERSION {
            return Err(WatchtowerError::Invalid(
                "encrypted backup protocol version is unsupported".into(),
            ));
        }
        let key_id: Bytes32 = envelope[10..42]
            .try_into()
            .map_err(|_| WatchtowerError::Invalid("encrypted backup key ID is invalid".into()))?;
        if key_id != expected_key_id {
            return Err(WatchtowerError::Invalid(
                "encrypted backup key ID does not match the supplied key".into(),
            ));
        }
        let nonce: [u8; 24] = envelope[42..66]
            .try_into()
            .map_err(|_| WatchtowerError::Invalid("encrypted backup nonce is invalid".into()))?;
        let plaintext_len = u64::from_be_bytes(envelope[66..74].try_into().unwrap()) as usize;
        let expected_len = 74usize
            .checked_add(plaintext_len)
            .and_then(|value| value.checked_add(16))
            .ok_or_else(|| WatchtowerError::Invalid("encrypted backup length overflow".into()))?;
        if envelope.len() != expected_len {
            return Err(WatchtowerError::Invalid(
                "encrypted backup length does not match its header".into(),
            ));
        }
        let tag_offset = 74 + plaintext_len;
        let mut plaintext = envelope[74..tag_offset].to_vec();
        let tag = chacha20poly1305::Tag::from_slice(&envelope[tag_offset..]);
        let cipher = XChaCha20Poly1305::new_from_slice(key)
            .map_err(|_| WatchtowerError::Invalid("backup key must be 32 bytes".into()))?;
        cipher
            .decrypt_in_place_detached(
                XNonce::from_slice(&nonce),
                &backup_aad(key_id),
                &mut plaintext,
                tag,
            )
            .map_err(|_| {
                WatchtowerError::Invalid("encrypted backup authentication failed".into())
            })?;
        fs::write(plaintext_path, &plaintext).map_err(|error| {
            WatchtowerError::Invalid(format!("decrypted backup cannot be written: {error}"))
        })?;
        plaintext.zeroize();
        Ok(sha256_parts(&[ENCRYPTED_BACKUP_DOMAIN, &envelope]))
    }
}

fn temporary_sibling(destination: &Path, role: &str) -> crate::Result<PathBuf> {
    let parent = destination.parent().ok_or_else(|| {
        WatchtowerError::Invalid("backup destination must have a parent directory".into())
    })?;
    for _ in 0..16 {
        let mut random = [0_u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut random);
        let file_name = destination
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("watchtower-backup");
        let candidate = parent.join(format!(".{file_name}.{role}.{}.tmp", hex::encode(random)));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(WatchtowerError::Invalid(
        "could not allocate a unique backup temporary path".into(),
    ))
}

fn remove_if_exists(path: &Path) {
    if path.exists() {
        let _ = fs::remove_file(path);
    }
}

fn backup_aad(key_id: Bytes32) -> Vec<u8> {
    let mut aad = Vec::with_capacity(ENCRYPTED_BACKUP_DOMAIN.len() + 2 + 32);
    aad.extend_from_slice(ENCRYPTED_BACKUP_DOMAIN);
    aad.extend_from_slice(&PROTOCOL_VERSION.to_be_bytes());
    aad.extend_from_slice(&key_id);
    aad
}

pub fn database_backup_id(
    file_hash: Bytes32,
    size_bytes: u64,
    audit_event_count: u64,
    audit_head_hash: Bytes32,
    anchor_id: Option<Bytes32>,
    created_at: u64,
) -> Bytes32 {
    sha256_parts(&[
        BACKUP_MANIFEST_DOMAIN,
        &PROTOCOL_VERSION.to_be_bytes(),
        &file_hash,
        &size_bytes.to_be_bytes(),
        &audit_event_count.to_be_bytes(),
        &audit_head_hash,
        anchor_id.as_ref().map_or(&[0_u8; 32], |value| value),
        &created_at.to_be_bytes(),
    ])
}

fn load_handoff(
    connection: &rusqlite::Connection,
    artifact_hash: Bytes32,
) -> crate::Result<Option<BackupArtifactHandoff>> {
    use rusqlite::OptionalExtension;
    connection
        .query_row(
            "SELECT backup_id, envelope_hash, key_id, manifest_bytes_hash,
                    received_at, verified_at, status, rejection_reason
             FROM v36_backup_artifact_handoffs WHERE artifact_hash=?1",
            [artifact_hash.as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()?
        .map(|row| {
            Ok(BackupArtifactHandoff {
                artifact_hash,
                backup_id: super::bytes32(row.0, "backup handoff backup ID")?,
                envelope_hash: super::bytes32(row.1, "backup handoff envelope hash")?,
                key_id: super::bytes32(row.2, "backup handoff key ID")?,
                manifest_bytes_hash: super::bytes32(row.3, "backup handoff manifest hash")?,
                received_at: super::from_i64(row.4, "backup handoff received_at")?,
                verified_at: row
                    .5
                    .map(|value| super::from_i64(value, "backup handoff verified_at"))
                    .transpose()?,
                status: row.6,
                rejection_reason: row.7,
            })
        })
        .transpose()
}

fn load_restore_drill(
    connection: &rusqlite::Connection,
    drill_id: Bytes32,
) -> crate::Result<Option<BackupRestoreDrill>> {
    use rusqlite::OptionalExtension;
    connection
        .query_row(
            "SELECT artifact_hash, backup_id, started_at, completed_at, duration_seconds,
                    hash_matches, size_matches, audit_valid, anchor_valid, status, failure_reason
             FROM v36_backup_restore_drills WHERE drill_id=?1",
            [drill_id.as_slice()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, bool>(5)?,
                    row.get::<_, bool>(6)?,
                    row.get::<_, bool>(7)?,
                    row.get::<_, Option<bool>>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, Option<String>>(10)?,
                ))
            },
        )
        .optional()?
        .map(|row| {
            Ok(BackupRestoreDrill {
                drill_id,
                artifact_hash: super::bytes32(row.0, "restore drill artifact hash")?,
                backup_id: super::bytes32(row.1, "restore drill backup ID")?,
                started_at: super::from_i64(row.2, "restore drill started_at")?,
                completed_at: super::from_i64(row.3, "restore drill completed_at")?,
                duration_seconds: super::from_i64(row.4, "restore drill duration")?,
                hash_matches: row.5,
                size_matches: row.6,
                audit_valid: row.7,
                anchor_valid: row.8,
                status: row.9,
                failure_reason: row.10,
            })
        })
        .transpose()
}
