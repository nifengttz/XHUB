use chia_bls::SecretKey;
use rusqlite::{OptionalExtension, params};
use xhub_protocol_v3_6::{
    Bytes32, CanonicalEncode, PROTOCOL_VERSION, PublicKeyBytes, SignatureBytes, parse_public_key,
    parse_signature, public_key_bytes, put_u16, put_u64, sha256_parts, sign_hash, verify_hash,
};

use crate::{
    Result, WatchtowerError, WatchtowerStore, confirmation_for, public_key, signature, to_i64,
    validate_name,
};

pub const CUSTODY_ATTESTATION_DOMAIN: &[u8] = b"XHUB_WATCHTOWER_CUSTODY_ATTESTATION_V3_6";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyAttestation {
    pub funding_coin_id: Bytes32,
    pub state_sequence: u64,
    pub checkpoint_hash: Bytes32,
    pub recovery_package_content_hash: Bytes32,
    pub entry_index: u64,
    pub authorization_hash: Bytes32,
    pub delivery_confirmation_hash: Bytes32,
}

impl CustodyAttestation {
    pub fn hash(&self) -> Bytes32 {
        sha256_parts(&[CUSTODY_ATTESTATION_DOMAIN, &self.canonical_bytes()])
    }
}

impl CanonicalEncode for CustodyAttestation {
    fn encode_to(&self, output: &mut Vec<u8>) {
        put_u16(output, PROTOCOL_VERSION);
        output.extend_from_slice(&self.funding_coin_id);
        put_u64(output, self.state_sequence);
        output.extend_from_slice(&self.checkpoint_hash);
        output.extend_from_slice(&self.recovery_package_content_hash);
        put_u64(output, self.entry_index);
        output.extend_from_slice(&self.authorization_hash);
        output.extend_from_slice(&self.delivery_confirmation_hash);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedCustodyAttestation {
    pub attestation: CustodyAttestation,
    pub attester_id: String,
    pub failure_domain: String,
    pub attester_public_key: PublicKeyBytes,
    pub signature: SignatureBytes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductionGreenlightStatus {
    pub funding_coin_id: Bytes32,
    pub state_sequence: u64,
    pub checkpoint_hash: Bytes32,
    pub recovery_package_content_hash: Bytes32,
    pub entry_index: u64,
    pub authorization_hash: Bytes32,
    pub delivery_confirmation_hash: Bytes32,
    pub merchant_delivered: bool,
    pub custody_threshold: u16,
    pub custody_attester_count: u16,
    pub custody_failure_domain_count: u16,
    pub production_ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SingleVpsTestGreenlightStatus {
    pub funding_coin_id: Bytes32,
    pub state_sequence: u64,
    pub checkpoint_hash: Bytes32,
    pub recovery_package_content_hash: Bytes32,
    pub entry_index: u64,
    pub authorization_hash: Bytes32,
    pub delivery_confirmation_hash: Bytes32,
    pub merchant_delivered: bool,
    pub custody_threshold: u16,
    pub custody_attester_count: u16,
    pub observed_failure_domain_count: u16,
    pub test_ready: bool,
}

pub(crate) fn initialize_schema(store: &rusqlite::Connection) -> Result<()> {
    store.execute_batch(
        "CREATE TABLE IF NOT EXISTS v36_custody_attesters (
            attester_id TEXT PRIMARY KEY,
            failure_domain TEXT NOT NULL,
            attester_public_key BLOB NOT NULL UNIQUE CHECK(length(attester_public_key) = 48),
            active INTEGER NOT NULL CHECK(active IN (0, 1)),
            created_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS v36_custody_attestations (
            funding_coin_id BLOB NOT NULL CHECK(length(funding_coin_id) = 32),
            state_sequence INTEGER NOT NULL CHECK(state_sequence > 0),
            checkpoint_hash BLOB NOT NULL CHECK(length(checkpoint_hash) = 32),
            recovery_package_content_hash BLOB NOT NULL CHECK(length(recovery_package_content_hash) = 32),
            entry_index INTEGER NOT NULL CHECK(entry_index >= 0),
            authorization_hash BLOB NOT NULL CHECK(length(authorization_hash) = 32),
            delivery_confirmation_hash BLOB NOT NULL CHECK(length(delivery_confirmation_hash) = 32),
            attester_id TEXT NOT NULL,
            failure_domain TEXT NOT NULL,
            attester_public_key BLOB NOT NULL CHECK(length(attester_public_key) = 48),
            attestation_blob BLOB NOT NULL,
            signature BLOB NOT NULL CHECK(length(signature) = 96),
            received_at INTEGER NOT NULL,
            PRIMARY KEY(funding_coin_id, state_sequence, entry_index, attester_id),
            UNIQUE(funding_coin_id, state_sequence, entry_index, attester_public_key),
            FOREIGN KEY(funding_coin_id, state_sequence)
              REFERENCES v36_watchtower_packages(funding_coin_id, state_sequence),
            FOREIGN KEY(attester_id) REFERENCES v36_custody_attesters(attester_id)
         );",
    )?;
    Ok(())
}

impl WatchtowerStore {
    pub fn register_custody_attester(
        &mut self,
        attester_id: &str,
        failure_domain: &str,
        attester_public_key: PublicKeyBytes,
        now: u64,
    ) -> Result<()> {
        validate_name("attester_id", attester_id)?;
        validate_name("failure_domain", failure_domain)?;
        parse_public_key(&attester_public_key)?;
        let duplicate_identity = self
            .connection
            .query_row(
                "SELECT attester_id FROM v36_custody_attesters
             WHERE attester_public_key=?1 AND attester_id<>?2",
                params![attester_public_key.as_slice(), attester_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if duplicate_identity.is_some() {
            return Err(WatchtowerError::AttesterConflict);
        }
        let changed = self.connection.execute(
            "INSERT OR IGNORE INTO v36_custody_attesters (
               attester_id, failure_domain, attester_public_key, active, created_at
             ) VALUES (?1, ?2, ?3, 1, ?4)",
            params![
                attester_id,
                failure_domain,
                attester_public_key.as_slice(),
                to_i64(now)?
            ],
        )?;
        if changed == 0 {
            let existing = self.load_custody_attester(attester_id)?;
            if existing != (failure_domain.to_string(), attester_public_key) {
                return Err(WatchtowerError::AttesterConflict);
            }
        }
        Ok(())
    }

    pub fn custody_attestation(
        &self,
        funding_coin_id: Bytes32,
        state_sequence: u64,
        entry_index: u64,
    ) -> Result<CustodyAttestation> {
        let package = self.package(funding_coin_id, state_sequence)?;
        let confirmation = confirmation_for(&package, entry_index)?;
        if !self.has_merchant_confirmation(&confirmation)? {
            return Err(WatchtowerError::MerchantConfirmationRequired);
        }
        Ok(CustodyAttestation {
            funding_coin_id,
            state_sequence,
            checkpoint_hash: confirmation.checkpoint_hash,
            recovery_package_content_hash: confirmation.recovery_package_content_hash,
            entry_index,
            authorization_hash: confirmation.authorization_hash,
            delivery_confirmation_hash: confirmation.hash()?,
        })
    }

    pub fn sign_custody_attestation(
        &self,
        funding_coin_id: Bytes32,
        state_sequence: u64,
        entry_index: u64,
        attester_id: &str,
        attester_secret_key: &SecretKey,
    ) -> Result<SignedCustodyAttestation> {
        let (failure_domain, attester_public_key) = self.load_custody_attester(attester_id)?;
        if public_key_bytes(attester_secret_key) != attester_public_key {
            return Err(WatchtowerError::AttesterConflict);
        }
        let attestation = self.custody_attestation(funding_coin_id, state_sequence, entry_index)?;
        let signature = sign_hash(attester_secret_key, &attestation.hash());
        Ok(SignedCustodyAttestation {
            attestation,
            attester_id: attester_id.to_string(),
            failure_domain,
            attester_public_key,
            signature,
        })
    }

    pub fn record_custody_attestation(
        &mut self,
        signed: &SignedCustodyAttestation,
        now: u64,
    ) -> Result<()> {
        validate_name("attester_id", &signed.attester_id)?;
        parse_signature(&signed.signature)?;
        let identity = self.load_custody_attester(&signed.attester_id)?;
        if identity != (signed.failure_domain.clone(), signed.attester_public_key) {
            return Err(WatchtowerError::AttesterConflict);
        }
        let expected = self.custody_attestation(
            signed.attestation.funding_coin_id,
            signed.attestation.state_sequence,
            signed.attestation.entry_index,
        )?;
        if signed.attestation != expected {
            return Err(WatchtowerError::CustodyAttestationMismatch);
        }
        verify_hash(
            &signed.attester_public_key,
            &signed.attestation.hash(),
            &signed.signature,
        )
        .map_err(|_| WatchtowerError::InvalidCustodyAttestationSignature)?;
        let changed = self.connection.execute(
            "INSERT OR IGNORE INTO v36_custody_attestations (
               funding_coin_id, state_sequence, checkpoint_hash,
               recovery_package_content_hash, entry_index, authorization_hash,
               delivery_confirmation_hash, attester_id, failure_domain,
               attester_public_key, attestation_blob, signature, received_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                signed.attestation.funding_coin_id.as_slice(),
                to_i64(signed.attestation.state_sequence)?,
                signed.attestation.checkpoint_hash.as_slice(),
                signed.attestation.recovery_package_content_hash.as_slice(),
                to_i64(signed.attestation.entry_index)?,
                signed.attestation.authorization_hash.as_slice(),
                signed.attestation.delivery_confirmation_hash.as_slice(),
                signed.attester_id,
                signed.failure_domain,
                signed.attester_public_key.as_slice(),
                signed.attestation.canonical_bytes(),
                signed.signature.as_slice(),
                to_i64(now)?,
            ],
        )?;
        if changed == 0 {
            let existing = self.connection.query_row(
                "SELECT attestation_blob, signature FROM v36_custody_attestations
                 WHERE funding_coin_id=?1 AND state_sequence=?2 AND entry_index=?3
                   AND attester_id=?4",
                params![
                    signed.attestation.funding_coin_id.as_slice(),
                    to_i64(signed.attestation.state_sequence)?,
                    to_i64(signed.attestation.entry_index)?,
                    signed.attester_id,
                ],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )?;
            if existing.0 != signed.attestation.canonical_bytes()
                || signature(existing.1, "custody attestation signature")? != signed.signature
            {
                return Err(WatchtowerError::DuplicateAttester);
            }
        }
        Ok(())
    }

    pub fn production_greenlight_status(
        &self,
        funding_coin_id: Bytes32,
        state_sequence: u64,
        entry_index: u64,
        custody_threshold: u16,
    ) -> Result<ProductionGreenlightStatus> {
        if !(1..=3).contains(&custody_threshold) {
            return Err(WatchtowerError::Invalid(
                "custody threshold must be 1, 2, or 3".into(),
            ));
        }
        let package = self.package(funding_coin_id, state_sequence)?;
        let confirmation = confirmation_for(&package, entry_index)?;
        let merchant_delivered = self.has_merchant_confirmation(&confirmation)?;
        let delivery_confirmation_hash = confirmation.hash()?;
        let (attesters, domains): (i64, i64) = self.connection.query_row(
            "SELECT COUNT(DISTINCT attester_public_key), COUNT(DISTINCT failure_domain)
             FROM v36_custody_attestations
             WHERE funding_coin_id=?1 AND state_sequence=?2 AND checkpoint_hash=?3
               AND recovery_package_content_hash=?4 AND entry_index=?5
               AND authorization_hash=?6 AND delivery_confirmation_hash=?7",
            params![
                funding_coin_id.as_slice(),
                to_i64(state_sequence)?,
                confirmation.checkpoint_hash.as_slice(),
                confirmation.recovery_package_content_hash.as_slice(),
                to_i64(entry_index)?,
                confirmation.authorization_hash.as_slice(),
                delivery_confirmation_hash.as_slice(),
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let custody_attester_count = u16::try_from(attesters)
            .map_err(|_| WatchtowerError::Corrupt("custody attester count".into()))?;
        let custody_failure_domain_count = u16::try_from(domains)
            .map_err(|_| WatchtowerError::Corrupt("custody failure domain count".into()))?;
        Ok(ProductionGreenlightStatus {
            funding_coin_id,
            state_sequence,
            checkpoint_hash: confirmation.checkpoint_hash,
            recovery_package_content_hash: confirmation.recovery_package_content_hash,
            entry_index,
            authorization_hash: confirmation.authorization_hash,
            delivery_confirmation_hash,
            merchant_delivered,
            custody_threshold,
            custody_attester_count,
            custody_failure_domain_count,
            production_ready: merchant_delivered
                && custody_attester_count >= custody_threshold
                && custody_failure_domain_count >= custody_threshold,
        })
    }

    pub fn single_vps_test_greenlight_status(
        &self,
        funding_coin_id: Bytes32,
        state_sequence: u64,
        entry_index: u64,
        custody_threshold: u16,
    ) -> Result<SingleVpsTestGreenlightStatus> {
        if !(1..=3).contains(&custody_threshold) {
            return Err(WatchtowerError::Invalid(
                "custody threshold must be 1, 2, or 3".into(),
            ));
        }
        let package = self.package(funding_coin_id, state_sequence)?;
        let confirmation = confirmation_for(&package, entry_index)?;
        let merchant_delivered = self.has_merchant_confirmation(&confirmation)?;
        let delivery_confirmation_hash = confirmation.hash()?;
        let (attesters, domains): (i64, i64) = self.connection.query_row(
            "SELECT COUNT(DISTINCT attester_public_key), COUNT(DISTINCT failure_domain)
             FROM v36_custody_attestations
             WHERE funding_coin_id=?1 AND state_sequence=?2 AND checkpoint_hash=?3
               AND recovery_package_content_hash=?4 AND entry_index=?5
               AND authorization_hash=?6 AND delivery_confirmation_hash=?7",
            params![
                funding_coin_id.as_slice(),
                to_i64(state_sequence)?,
                confirmation.checkpoint_hash.as_slice(),
                confirmation.recovery_package_content_hash.as_slice(),
                to_i64(entry_index)?,
                confirmation.authorization_hash.as_slice(),
                delivery_confirmation_hash.as_slice(),
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let custody_attester_count = u16::try_from(attesters)
            .map_err(|_| WatchtowerError::Corrupt("custody attester count".into()))?;
        let observed_failure_domain_count = u16::try_from(domains)
            .map_err(|_| WatchtowerError::Corrupt("custody failure domain count".into()))?;
        Ok(SingleVpsTestGreenlightStatus {
            funding_coin_id,
            state_sequence,
            checkpoint_hash: confirmation.checkpoint_hash,
            recovery_package_content_hash: confirmation.recovery_package_content_hash,
            entry_index,
            authorization_hash: confirmation.authorization_hash,
            delivery_confirmation_hash,
            merchant_delivered,
            custody_threshold,
            custody_attester_count,
            observed_failure_domain_count,
            test_ready: merchant_delivered && custody_attester_count >= custody_threshold,
        })
    }

    fn has_merchant_confirmation(
        &self,
        confirmation: &xhub_protocol_v3_6::DeliveryConfirmation,
    ) -> Result<bool> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(DISTINCT signer_public_key) FROM v36_delivery_confirmations
             WHERE funding_coin_id=?1 AND state_sequence=?2 AND checkpoint_hash=?3
               AND recovery_package_content_hash=?4 AND entry_index=?5
               AND authorization_hash=?6",
            params![
                confirmation.funding_coin_id.as_slice(),
                to_i64(confirmation.state_sequence)?,
                confirmation.checkpoint_hash.as_slice(),
                confirmation.recovery_package_content_hash.as_slice(),
                to_i64(confirmation.entry_index)?,
                confirmation.authorization_hash.as_slice(),
            ],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    fn load_custody_attester(&self, attester_id: &str) -> Result<(String, PublicKeyBytes)> {
        let value = self
            .connection
            .query_row(
                "SELECT failure_domain, attester_public_key FROM v36_custody_attesters
             WHERE attester_id=?1 AND active=1",
                [attester_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()?
            .ok_or_else(|| {
                WatchtowerError::Invalid("unknown or inactive custody attester".into())
            })?;
        Ok((value.0, public_key(value.1, "custody attester public key")?))
    }
}
