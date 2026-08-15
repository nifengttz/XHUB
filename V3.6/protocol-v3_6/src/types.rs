use std::collections::HashSet;

use crate::{
    Bytes32, CanonicalDecode, CanonicalEncode, Decoder, MAX_CANONICAL_BLOB_BYTES,
    MAX_LEDGER_ENTRIES, MAX_PROTOCOL_U64, PROTOCOL_VERSION, ProtocolError, PublicKeyBytes, Result,
    SignatureBytes, decode_bounded_blob, empty_root, leaf_hash, merkle_root, parse_public_key,
    parse_signature, put_bool, put_bytes, put_u16, put_u32, put_u64, sha256_parts, verify_hash,
};

pub const CHANNEL_TERMS_DOMAIN: &[u8] = b"XHUB_CHANNEL_TERMS_V3_6";
pub const USER_AUTH_DOMAIN: &[u8] = b"XHUB_USER_AUTH_V3_6";
pub const LEDGER_ENTRY_DOMAIN: &[u8] = b"XHUB_LEDGER_ENTRY_V3_6";
pub const CHECKPOINT_DOMAIN: &[u8] = b"XHUB_LEDGER_CHECKPOINT_V3_6";
pub const HUB_STATE_DOMAIN: &[u8] = b"XHUB_HUB_STATE_V3_6";
pub const STATE_ZERO_DOMAIN: &[u8] = b"XHUB_STATE_ZERO_V3_6";
pub const RECOVERY_PACKAGE_DOMAIN: &[u8] = b"XHUB_RECOVERY_PACKAGE_V3_6";
pub const DELIVERY_CONFIRMATION_DOMAIN: &[u8] = b"XHUB_DELIVERY_CONFIRMATION_V3_6";
pub const RESERVATION_RESULT_DOMAIN: &[u8] = b"XHUB_RESERVATION_RESULT_V3_6";
pub const DOUBLE_SIGN_EVIDENCE_DOMAIN: &[u8] = b"XHUB_DOUBLE_SIGN_EVIDENCE_V3_6";
pub const CONFLICTING_RESULT_EVIDENCE_DOMAIN: &[u8] = b"XHUB_CONFLICTING_RESULT_EVIDENCE_V3_6";

fn validate_u64(field: &'static str, value: u64) -> Result<()> {
    if value <= MAX_PROTOCOL_U64 {
        Ok(())
    } else {
        Err(ProtocolError::IntegerRange { field })
    }
}

fn validate_positive(field: &'static str, value: u64) -> Result<()> {
    validate_u64(field, value)?;
    if value == 0 {
        Err(ProtocolError::ZeroValue { field })
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelTerms {
    pub network_id: Bytes32,
    pub acceptance_blocks: u64,
    pub freeze_blocks: u64,
    pub close_delay_blocks: u64,
    pub challenge_blocks: u64,
    pub user_public_key: PublicKeyBytes,
    pub hub_state_public_key_a: PublicKeyBytes,
    pub state_rules_hash: Bytes32,
    pub funding_amount: u64,
    pub user_remainder_puzzle_hash: Bytes32,
    pub max_ledger_entries: u64,
}

impl ChannelTerms {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        network_id: Bytes32,
        acceptance_blocks: u64,
        freeze_blocks: u64,
        challenge_blocks: u64,
        user_public_key: PublicKeyBytes,
        hub_state_public_key_a: PublicKeyBytes,
        state_rules_hash: Bytes32,
        funding_amount: u64,
        user_remainder_puzzle_hash: Bytes32,
    ) -> Result<Self> {
        let close_delay_blocks = acceptance_blocks
            .checked_add(freeze_blocks)
            .ok_or(ProtocolError::ArithmeticOverflow("close_delay_blocks"))?;
        let terms = Self {
            network_id,
            acceptance_blocks,
            freeze_blocks,
            close_delay_blocks,
            challenge_blocks,
            user_public_key,
            hub_state_public_key_a,
            state_rules_hash,
            funding_amount,
            user_remainder_puzzle_hash,
            max_ledger_entries: MAX_LEDGER_ENTRIES,
        };
        terms.validate()?;
        Ok(terms)
    }

    pub fn validate(&self) -> Result<()> {
        validate_positive("acceptance_blocks", self.acceptance_blocks)?;
        validate_positive("freeze_blocks", self.freeze_blocks)?;
        validate_positive("close_delay_blocks", self.close_delay_blocks)?;
        validate_positive("challenge_blocks", self.challenge_blocks)?;
        validate_positive("funding_amount", self.funding_amount)?;
        let expected = self
            .acceptance_blocks
            .checked_add(self.freeze_blocks)
            .ok_or(ProtocolError::ArithmeticOverflow("close_delay_blocks"))?;
        if self.close_delay_blocks != expected {
            return Err(ProtocolError::CloseDelayMismatch);
        }
        if self.max_ledger_entries != MAX_LEDGER_ENTRIES {
            return Err(ProtocolError::InvalidMaxEntries);
        }
        parse_public_key(&self.user_public_key)?;
        parse_public_key(&self.hub_state_public_key_a)?;
        Ok(())
    }

    pub fn hash(&self) -> Result<Bytes32> {
        self.validate()?;
        Ok(sha256_parts(&[
            CHANNEL_TERMS_DOMAIN,
            &self.canonical_bytes(),
        ]))
    }
}

impl CanonicalEncode for ChannelTerms {
    fn encode_to(&self, output: &mut Vec<u8>) {
        put_u16(output, PROTOCOL_VERSION);
        output.extend_from_slice(&self.network_id);
        put_u64(output, self.acceptance_blocks);
        put_u64(output, self.freeze_blocks);
        put_u64(output, self.close_delay_blocks);
        put_u64(output, self.challenge_blocks);
        output.extend_from_slice(&self.user_public_key);
        output.extend_from_slice(&self.hub_state_public_key_a);
        output.extend_from_slice(&self.state_rules_hash);
        put_u64(output, self.funding_amount);
        output.extend_from_slice(&self.user_remainder_puzzle_hash);
        put_u64(output, self.max_ledger_entries);
    }
}

impl CanonicalDecode for ChannelTerms {
    fn decode_from(decoder: &mut Decoder<'_>) -> Result<Self> {
        if decoder.u16()? != PROTOCOL_VERSION {
            return Err(ProtocolError::IntegerRange {
                field: "protocol_version",
            });
        }
        let terms = Self {
            network_id: decoder.take()?,
            acceptance_blocks: decoder.u64()?,
            freeze_blocks: decoder.u64()?,
            close_delay_blocks: decoder.u64()?,
            challenge_blocks: decoder.u64()?,
            user_public_key: decoder.take()?,
            hub_state_public_key_a: decoder.take()?,
            state_rules_hash: decoder.take()?,
            funding_amount: decoder.u64()?,
            user_remainder_puzzle_hash: decoder.take()?,
            max_ledger_entries: decoder.u64()?,
        };
        terms.validate()?;
        Ok(terms)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerEntry {
    pub merchant_puzzle_hash: Bytes32,
    pub merchant_receipt_public_key: PublicKeyBytes,
    pub amount: u64,
    pub reservation_nonce: Bytes32,
}

impl LedgerEntry {
    pub fn validate(&self) -> Result<()> {
        validate_positive("amount", self.amount).map_err(|error| match error {
            ProtocolError::ZeroValue { .. } => ProtocolError::ZeroAmount,
            other => other,
        })?;
        parse_public_key(&self.merchant_receipt_public_key)?;
        Ok(())
    }

    pub fn authorization_hash(
        &self,
        terms: &ChannelTerms,
        funding_coin_id: &Bytes32,
    ) -> Result<Bytes32> {
        self.validate()?;
        let channel_terms_hash = terms.hash()?;
        Ok(sha256_parts(&[
            USER_AUTH_DOMAIN,
            &PROTOCOL_VERSION.to_be_bytes(),
            &terms.network_id,
            funding_coin_id,
            &channel_terms_hash,
            &self.merchant_puzzle_hash,
            &self.merchant_receipt_public_key,
            &self.amount.to_be_bytes(),
            &self.reservation_nonce,
        ]))
    }

    pub fn entry_hash(
        &self,
        terms: &ChannelTerms,
        funding_coin_id: &Bytes32,
        entry_index: u64,
    ) -> Result<Bytes32> {
        validate_u64("entry_index", entry_index)?;
        let authorization_hash = self.authorization_hash(terms, funding_coin_id)?;
        Ok(sha256_parts(&[
            LEDGER_ENTRY_DOMAIN,
            &entry_index.to_be_bytes(),
            &self.merchant_puzzle_hash,
            &self.merchant_receipt_public_key,
            &self.amount.to_be_bytes(),
            &self.reservation_nonce,
            &authorization_hash,
        ]))
    }
}

impl CanonicalEncode for LedgerEntry {
    fn encode_to(&self, output: &mut Vec<u8>) {
        output.extend_from_slice(&self.merchant_puzzle_hash);
        output.extend_from_slice(&self.merchant_receipt_public_key);
        put_u64(output, self.amount);
        output.extend_from_slice(&self.reservation_nonce);
    }
}

impl CanonicalDecode for LedgerEntry {
    fn decode_from(decoder: &mut Decoder<'_>) -> Result<Self> {
        let entry = Self {
            merchant_puzzle_hash: decoder.take()?,
            merchant_receipt_public_key: decoder.take()?,
            amount: decoder.u64()?,
            reservation_nonce: decoder.take()?,
        };
        entry.validate()?;
        Ok(entry)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerCheckpoint {
    pub funding_coin_id: Bytes32,
    pub channel_terms_hash: Bytes32,
    pub state_sequence: u64,
    pub previous_checkpoint_hash: Bytes32,
    pub manifest_root: Bytes32,
    pub entry_count: u64,
    pub reserved_total: u64,
    pub user_remainder: u64,
}

impl LedgerCheckpoint {
    pub fn validate(&self, terms: &ChannelTerms) -> Result<()> {
        validate_u64("state_sequence", self.state_sequence)?;
        validate_u64("entry_count", self.entry_count)?;
        validate_u64("reserved_total", self.reserved_total)?;
        validate_u64("user_remainder", self.user_remainder)?;
        if self.channel_terms_hash != terms.hash()? || self.entry_count > MAX_LEDGER_ENTRIES {
            return Err(ProtocolError::CheckpointMismatch);
        }
        let total = self
            .reserved_total
            .checked_add(self.user_remainder)
            .ok_or(ProtocolError::ArithmeticOverflow("checkpoint total"))?;
        if total != terms.funding_amount {
            return Err(ProtocolError::CheckpointMismatch);
        }
        Ok(())
    }

    pub fn hash(&self, terms: &ChannelTerms) -> Result<Bytes32> {
        self.validate(terms)?;
        Ok(sha256_parts(&[
            CHECKPOINT_DOMAIN,
            &PROTOCOL_VERSION.to_be_bytes(),
            &terms.network_id,
            &self.canonical_bytes(),
        ]))
    }

    pub fn hub_state_hash(&self, terms: &ChannelTerms) -> Result<Bytes32> {
        Ok(sha256_parts(&[HUB_STATE_DOMAIN, &self.hash(terms)?]))
    }
}

impl CanonicalEncode for LedgerCheckpoint {
    fn encode_to(&self, output: &mut Vec<u8>) {
        output.extend_from_slice(&self.funding_coin_id);
        output.extend_from_slice(&self.channel_terms_hash);
        put_u64(output, self.state_sequence);
        output.extend_from_slice(&self.previous_checkpoint_hash);
        output.extend_from_slice(&self.manifest_root);
        put_u64(output, self.entry_count);
        put_u64(output, self.reserved_total);
        put_u64(output, self.user_remainder);
    }
}

impl CanonicalDecode for LedgerCheckpoint {
    fn decode_from(decoder: &mut Decoder<'_>) -> Result<Self> {
        Ok(Self {
            funding_coin_id: decoder.take()?,
            channel_terms_hash: decoder.take()?,
            state_sequence: decoder.u64()?,
            previous_checkpoint_hash: decoder.take()?,
            manifest_root: decoder.take()?,
            entry_count: decoder.u64()?,
            reserved_total: decoder.u64()?,
            user_remainder: decoder.u64()?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfficialState {
    pub checkpoint: LedgerCheckpoint,
    pub hub_state_signature: SignatureBytes,
}

impl OfficialState {
    pub fn verify(&self, terms: &ChannelTerms) -> Result<()> {
        let message = self.checkpoint.hub_state_hash(terms)?;
        verify_hash(
            &terms.hub_state_public_key_a,
            &message,
            &self.hub_state_signature,
        )
    }
}

impl CanonicalEncode for OfficialState {
    fn encode_to(&self, output: &mut Vec<u8>) {
        self.checkpoint.encode_to(output);
        output.extend_from_slice(&self.hub_state_signature);
    }
}

impl CanonicalDecode for OfficialState {
    fn decode_from(decoder: &mut Decoder<'_>) -> Result<Self> {
        let state = Self {
            checkpoint: LedgerCheckpoint::decode_from(decoder)?,
            hub_state_signature: decoder.take()?,
        };
        parse_signature(&state.hub_state_signature)?;
        Ok(state)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateZero {
    pub state_sequence: u64,
    pub manifest_root: Bytes32,
    pub entry_count: u64,
    pub reserved_total: u64,
    pub user_remainder: u64,
}

impl StateZero {
    pub fn new(terms: &ChannelTerms) -> Result<Self> {
        terms.validate()?;
        Ok(Self {
            state_sequence: 0,
            manifest_root: empty_root(),
            entry_count: 0,
            reserved_total: 0,
            user_remainder: terms.funding_amount,
        })
    }

    pub fn hash(&self, terms: &ChannelTerms, funding_coin_id: &Bytes32) -> Result<Bytes32> {
        if self != &Self::new(terms)? {
            return Err(ProtocolError::CheckpointMismatch);
        }
        Ok(sha256_parts(&[
            STATE_ZERO_DOMAIN,
            &PROTOCOL_VERSION.to_be_bytes(),
            &terms.network_id,
            funding_coin_id,
            &terms.hash()?,
            &terms.funding_amount.to_be_bytes(),
            &terms.user_remainder_puzzle_hash,
        ]))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ledger {
    pub entries: Vec<LedgerEntry>,
}

impl Ledger {
    pub fn validate(&self, terms: &ChannelTerms) -> Result<(u64, u64)> {
        if self.entries.len() > MAX_LEDGER_ENTRIES as usize {
            return Err(ProtocolError::LedgerFull);
        }
        let mut nonces = HashSet::with_capacity(self.entries.len());
        let mut reserved_total = 0_u64;
        for entry in &self.entries {
            entry.validate()?;
            if !nonces.insert(entry.reservation_nonce) {
                return Err(ProtocolError::DuplicateNonce);
            }
            reserved_total = reserved_total
                .checked_add(entry.amount)
                .ok_or(ProtocolError::ArithmeticOverflow("reserved_total"))?;
            validate_u64("reserved_total", reserved_total)?;
        }
        let user_remainder = terms
            .funding_amount
            .checked_sub(reserved_total)
            .ok_or(ProtocolError::InsufficientRemainder)?;
        Ok((reserved_total, user_remainder))
    }

    pub fn leaf_hashes(
        &self,
        terms: &ChannelTerms,
        funding_coin_id: &Bytes32,
    ) -> Result<Vec<Bytes32>> {
        self.entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let index = u64::try_from(index).expect("ledger index exceeds u64");
                Ok(leaf_hash(&entry.entry_hash(
                    terms,
                    funding_coin_id,
                    index,
                )?))
            })
            .collect()
    }

    pub fn manifest_root(
        &self,
        terms: &ChannelTerms,
        funding_coin_id: &Bytes32,
    ) -> Result<Bytes32> {
        Ok(merkle_root(&self.leaf_hashes(terms, funding_coin_id)?))
    }

    pub fn checkpoint(
        &self,
        terms: &ChannelTerms,
        funding_coin_id: Bytes32,
        state_sequence: u64,
        previous_checkpoint_hash: Bytes32,
    ) -> Result<LedgerCheckpoint> {
        let (reserved_total, user_remainder) = self.validate(terms)?;
        let checkpoint = LedgerCheckpoint {
            funding_coin_id,
            channel_terms_hash: terms.hash()?,
            state_sequence,
            previous_checkpoint_hash,
            manifest_root: self.manifest_root(terms, &funding_coin_id)?,
            entry_count: self.entries.len() as u64,
            reserved_total,
            user_remainder,
        };
        checkpoint.validate(terms)?;
        Ok(checkpoint)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryPackage {
    pub funding_coin_id: Bytes32,
    pub funding_puzzle_reveal: Vec<u8>,
    pub funding_amount: u64,
    pub channel_terms: ChannelTerms,
    pub official_state: OfficialState,
    pub entries: Vec<LedgerEntry>,
    pub user_authorization_signatures: Vec<SignatureBytes>,
}

impl RecoveryPackage {
    pub fn validate(&self) -> Result<()> {
        self.channel_terms.validate()?;
        if self.funding_puzzle_reveal.len() > MAX_CANONICAL_BLOB_BYTES {
            return Err(ProtocolError::LengthLimit {
                actual: self.funding_puzzle_reveal.len(),
                limit: MAX_CANONICAL_BLOB_BYTES,
            });
        }
        if self.funding_coin_id != self.official_state.checkpoint.funding_coin_id
            || self.funding_amount != self.channel_terms.funding_amount
            || self.entries.len() != self.user_authorization_signatures.len()
        {
            return Err(ProtocolError::RecoveryCount);
        }

        let ledger = Ledger {
            entries: self.entries.clone(),
        };
        let (reserved_total, user_remainder) = ledger.validate(&self.channel_terms)?;
        let checkpoint = &self.official_state.checkpoint;
        if checkpoint.entry_count != self.entries.len() as u64
            || checkpoint.reserved_total != reserved_total
            || checkpoint.user_remainder != user_remainder
            || checkpoint.manifest_root
                != ledger.manifest_root(&self.channel_terms, &self.funding_coin_id)?
        {
            return Err(ProtocolError::CheckpointMismatch);
        }

        self.official_state.verify(&self.channel_terms)?;
        for (entry, signature) in self.entries.iter().zip(&self.user_authorization_signatures) {
            verify_hash(
                &self.channel_terms.user_public_key,
                &entry.authorization_hash(&self.channel_terms, &self.funding_coin_id)?,
                signature,
            )?;
        }
        Ok(())
    }

    pub fn content_hash(&self) -> Result<Bytes32> {
        self.validate()?;
        Ok(sha256_parts(&[
            RECOVERY_PACKAGE_DOMAIN,
            &self.canonical_bytes(),
        ]))
    }
}

impl CanonicalEncode for RecoveryPackage {
    fn encode_to(&self, output: &mut Vec<u8>) {
        output.extend_from_slice(&self.funding_coin_id);
        put_bytes(output, &self.funding_puzzle_reveal);
        put_u64(output, self.funding_amount);
        self.channel_terms.encode_to(output);
        self.official_state.encode_to(output);
        put_u32(
            output,
            u32::try_from(self.entries.len()).expect("entry count exceeds u32"),
        );
        for entry in &self.entries {
            entry.encode_to(output);
        }
        put_u32(
            output,
            u32::try_from(self.user_authorization_signatures.len())
                .expect("signature count exceeds u32"),
        );
        for signature in &self.user_authorization_signatures {
            output.extend_from_slice(signature);
        }
    }
}

impl CanonicalDecode for RecoveryPackage {
    fn decode_from(decoder: &mut Decoder<'_>) -> Result<Self> {
        let funding_coin_id = decoder.take()?;
        let funding_puzzle_reveal = decode_bounded_blob(decoder)?;
        let funding_amount = decoder.u64()?;
        let channel_terms = ChannelTerms::decode_from(decoder)?;
        let official_state = OfficialState::decode_from(decoder)?;
        let entry_count = decoder.u32()? as usize;
        if entry_count > MAX_LEDGER_ENTRIES as usize {
            return Err(ProtocolError::LedgerFull);
        }
        let mut entries = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            entries.push(LedgerEntry::decode_from(decoder)?);
        }
        let signature_count = decoder.u32()? as usize;
        if signature_count > MAX_LEDGER_ENTRIES as usize {
            return Err(ProtocolError::LedgerFull);
        }
        let mut user_authorization_signatures = Vec::with_capacity(signature_count);
        for _ in 0..signature_count {
            let signature = decoder.take()?;
            parse_signature(&signature)?;
            user_authorization_signatures.push(signature);
        }
        Ok(Self {
            funding_coin_id,
            funding_puzzle_reveal,
            funding_amount,
            channel_terms,
            official_state,
            entries,
            user_authorization_signatures,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryConfirmation {
    pub network_id: Bytes32,
    pub funding_coin_id: Bytes32,
    pub channel_terms_hash: Bytes32,
    pub state_sequence: u64,
    pub checkpoint_hash: Bytes32,
    pub entry_index: u64,
    pub authorization_hash: Bytes32,
    pub recovery_package_content_hash: Bytes32,
}

impl DeliveryConfirmation {
    pub fn validate(&self) -> Result<()> {
        validate_u64("state_sequence", self.state_sequence)?;
        validate_u64("entry_index", self.entry_index)
    }

    pub fn hash(&self) -> Result<Bytes32> {
        self.validate()?;
        Ok(sha256_parts(&[
            DELIVERY_CONFIRMATION_DOMAIN,
            &self.canonical_bytes(),
        ]))
    }
}

impl CanonicalEncode for DeliveryConfirmation {
    fn encode_to(&self, output: &mut Vec<u8>) {
        put_u16(output, PROTOCOL_VERSION);
        output.extend_from_slice(&self.network_id);
        output.extend_from_slice(&self.funding_coin_id);
        output.extend_from_slice(&self.channel_terms_hash);
        put_u64(output, self.state_sequence);
        output.extend_from_slice(&self.checkpoint_hash);
        put_u64(output, self.entry_index);
        output.extend_from_slice(&self.authorization_hash);
        output.extend_from_slice(&self.recovery_package_content_hash);
    }
}

impl CanonicalDecode for DeliveryConfirmation {
    fn decode_from(decoder: &mut Decoder<'_>) -> Result<Self> {
        if decoder.u16()? != PROTOCOL_VERSION {
            return Err(ProtocolError::IntegerRange {
                field: "protocol_version",
            });
        }
        let confirmation = Self {
            network_id: decoder.take()?,
            funding_coin_id: decoder.take()?,
            channel_terms_hash: decoder.take()?,
            state_sequence: decoder.u64()?,
            checkpoint_hash: decoder.take()?,
            entry_index: decoder.u64()?,
            authorization_hash: decoder.take()?,
            recovery_package_content_hash: decoder.take()?,
        };
        confirmation.validate()?;
        Ok(confirmation)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ReservationStatus {
    Signed = 1,
    Delivered = 2,
    Pending = 3,
    Unknown = 4,
    RejectedFreezing = 100,
    RejectedCloseable = 101,
    InvalidAuthorization = 102,
    InsufficientRemainder = 103,
    NonceConflict = 104,
    LedgerFull = 105,
    ChannelClosing = 106,
    ChannelFinalized = 107,
    NodeNotSynced = 200,
    RpcUnavailable = 201,
    ChainStateUncertain = 202,
    ChannelReorgPending = 203,
    InternalError = 204,
}

impl ReservationStatus {
    pub fn from_code(code: u16) -> Result<Self> {
        match code {
            1 => Ok(Self::Signed),
            2 => Ok(Self::Delivered),
            3 => Ok(Self::Pending),
            4 => Ok(Self::Unknown),
            100 => Ok(Self::RejectedFreezing),
            101 => Ok(Self::RejectedCloseable),
            102 => Ok(Self::InvalidAuthorization),
            103 => Ok(Self::InsufficientRemainder),
            104 => Ok(Self::NonceConflict),
            105 => Ok(Self::LedgerFull),
            106 => Ok(Self::ChannelClosing),
            107 => Ok(Self::ChannelFinalized),
            200 => Ok(Self::NodeNotSynced),
            201 => Ok(Self::RpcUnavailable),
            202 => Ok(Self::ChainStateUncertain),
            203 => Ok(Self::ChannelReorgPending),
            204 => Ok(Self::InternalError),
            value => Err(ProtocolError::InvalidStatus(value)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservationResult {
    pub network_id: Bytes32,
    pub request_id: Bytes32,
    pub funding_coin_id: Bytes32,
    pub reservation_nonce: Bytes32,
    pub authorization_hash: Bytes32,
    pub status: ReservationStatus,
    pub state_sequence: Option<u64>,
    pub checkpoint_hash: Option<Bytes32>,
    pub observed_peak_height: u64,
    pub acceptance_cutoff_height: u64,
    pub scheduled_close_height: u64,
    pub ledger_written: bool,
}

impl ReservationResult {
    pub fn validate(&self) -> Result<()> {
        validate_u64("observed_peak_height", self.observed_peak_height)?;
        validate_u64("acceptance_cutoff_height", self.acceptance_cutoff_height)?;
        validate_u64("scheduled_close_height", self.scheduled_close_height)?;
        if let Some(sequence) = self.state_sequence {
            validate_u64("state_sequence", sequence)?;
        }
        if self.state_sequence.is_some() != self.checkpoint_hash.is_some() {
            return Err(ProtocolError::CheckpointMismatch);
        }
        Ok(())
    }

    pub fn hash(&self) -> Result<Bytes32> {
        self.validate()?;
        Ok(sha256_parts(&[
            RESERVATION_RESULT_DOMAIN,
            &self.canonical_bytes(),
        ]))
    }
}

impl CanonicalEncode for ReservationResult {
    fn encode_to(&self, output: &mut Vec<u8>) {
        put_u16(output, PROTOCOL_VERSION);
        output.extend_from_slice(&self.network_id);
        output.extend_from_slice(&self.request_id);
        output.extend_from_slice(&self.funding_coin_id);
        output.extend_from_slice(&self.reservation_nonce);
        output.extend_from_slice(&self.authorization_hash);
        put_u16(output, self.status as u16);
        match self.state_sequence {
            Some(value) => {
                output.push(1);
                put_u64(output, value);
            }
            None => output.push(0),
        }
        match self.checkpoint_hash {
            Some(value) => {
                output.push(1);
                output.extend_from_slice(&value);
            }
            None => output.push(0),
        }
        put_u64(output, self.observed_peak_height);
        put_u64(output, self.acceptance_cutoff_height);
        put_u64(output, self.scheduled_close_height);
        put_bool(output, self.ledger_written);
    }
}

impl CanonicalDecode for ReservationResult {
    fn decode_from(decoder: &mut Decoder<'_>) -> Result<Self> {
        if decoder.u16()? != PROTOCOL_VERSION {
            return Err(ProtocolError::IntegerRange {
                field: "protocol_version",
            });
        }
        let network_id = decoder.take()?;
        let request_id = decoder.take()?;
        let funding_coin_id = decoder.take()?;
        let reservation_nonce = decoder.take()?;
        let authorization_hash = decoder.take()?;
        let status = ReservationStatus::from_code(decoder.u16()?)?;
        let state_sequence = match decoder.u8()? {
            0 => None,
            1 => Some(decoder.u64()?),
            value => return Err(ProtocolError::InvalidOption(value)),
        };
        let checkpoint_hash = match decoder.u8()? {
            0 => None,
            1 => Some(decoder.take()?),
            value => return Err(ProtocolError::InvalidOption(value)),
        };
        let result = Self {
            network_id,
            request_id,
            funding_coin_id,
            reservation_nonce,
            authorization_hash,
            status,
            state_sequence,
            checkpoint_hash,
            observed_peak_height: decoder.u64()?,
            acceptance_cutoff_height: decoder.u64()?,
            scheduled_close_height: decoder.u64()?,
            ledger_written: decoder.bool()?,
        };
        result.validate()?;
        Ok(result)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedReservationResult {
    pub result: ReservationResult,
    pub hub_result_signature: SignatureBytes,
}

impl SignedReservationResult {
    pub fn verify(&self, terms: &ChannelTerms) -> Result<()> {
        if self.result.network_id != terms.network_id {
            return Err(ProtocolError::EvidenceContext);
        }
        verify_hash(
            &terms.hub_state_public_key_a,
            &self.result.hash()?,
            &self.hub_result_signature,
        )
    }
}

impl CanonicalEncode for SignedReservationResult {
    fn encode_to(&self, output: &mut Vec<u8>) {
        self.result.encode_to(output);
        output.extend_from_slice(&self.hub_result_signature);
    }
}

impl CanonicalDecode for SignedReservationResult {
    fn decode_from(decoder: &mut Decoder<'_>) -> Result<Self> {
        let signed = Self {
            result: ReservationResult::decode_from(decoder)?,
            hub_result_signature: decoder.take()?,
        };
        parse_signature(&signed.hub_result_signature)?;
        Ok(signed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoubleSignEvidence {
    pub first: OfficialState,
    pub second: OfficialState,
}

impl DoubleSignEvidence {
    pub fn new(
        terms: &ChannelTerms,
        mut first: OfficialState,
        mut second: OfficialState,
    ) -> Result<Self> {
        if first.checkpoint.hash(terms)? > second.checkpoint.hash(terms)? {
            std::mem::swap(&mut first, &mut second);
        }
        let evidence = Self { first, second };
        evidence.validate(terms)?;
        Ok(evidence)
    }

    pub fn validate(&self, terms: &ChannelTerms) -> Result<()> {
        self.first.verify(terms)?;
        self.second.verify(terms)?;
        let first_checkpoint = &self.first.checkpoint;
        let second_checkpoint = &self.second.checkpoint;
        if first_checkpoint.funding_coin_id != second_checkpoint.funding_coin_id
            || first_checkpoint.channel_terms_hash != second_checkpoint.channel_terms_hash
            || first_checkpoint.state_sequence != second_checkpoint.state_sequence
        {
            return Err(ProtocolError::EvidenceContext);
        }
        let first_hash = first_checkpoint.hash(terms)?;
        let second_hash = second_checkpoint.hash(terms)?;
        if first_hash == second_hash {
            return Err(ProtocolError::EvidenceNotConflicting);
        }
        if first_hash > second_hash {
            return Err(ProtocolError::EvidenceOrder);
        }
        Ok(())
    }

    pub fn hash(&self, terms: &ChannelTerms) -> Result<Bytes32> {
        self.validate(terms)?;
        Ok(sha256_parts(&[
            DOUBLE_SIGN_EVIDENCE_DOMAIN,
            &self.canonical_bytes(),
        ]))
    }
}

impl CanonicalEncode for DoubleSignEvidence {
    fn encode_to(&self, output: &mut Vec<u8>) {
        self.first.encode_to(output);
        self.second.encode_to(output);
    }
}

impl CanonicalDecode for DoubleSignEvidence {
    fn decode_from(decoder: &mut Decoder<'_>) -> Result<Self> {
        Ok(Self {
            first: OfficialState::decode_from(decoder)?,
            second: OfficialState::decode_from(decoder)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictingResultEvidence {
    pub first: SignedReservationResult,
    pub second: SignedReservationResult,
}

impl ConflictingResultEvidence {
    pub fn new(
        terms: &ChannelTerms,
        mut first: SignedReservationResult,
        mut second: SignedReservationResult,
    ) -> Result<Self> {
        if first.result.hash()? > second.result.hash()? {
            std::mem::swap(&mut first, &mut second);
        }
        let evidence = Self { first, second };
        evidence.validate(terms)?;
        Ok(evidence)
    }

    pub fn validate(&self, terms: &ChannelTerms) -> Result<()> {
        self.first.verify(terms)?;
        self.second.verify(terms)?;
        let first_result = &self.first.result;
        let second_result = &self.second.result;
        if first_result.network_id != second_result.network_id
            || first_result.funding_coin_id != second_result.funding_coin_id
            || first_result.reservation_nonce != second_result.reservation_nonce
        {
            return Err(ProtocolError::EvidenceContext);
        }
        let first_hash = first_result.hash()?;
        let second_hash = second_result.hash()?;
        if first_hash == second_hash {
            return Err(ProtocolError::EvidenceNotConflicting);
        }
        if first_hash > second_hash {
            return Err(ProtocolError::EvidenceOrder);
        }
        Ok(())
    }

    pub fn hash(&self, terms: &ChannelTerms) -> Result<Bytes32> {
        self.validate(terms)?;
        Ok(sha256_parts(&[
            CONFLICTING_RESULT_EVIDENCE_DOMAIN,
            &self.canonical_bytes(),
        ]))
    }
}

impl CanonicalEncode for ConflictingResultEvidence {
    fn encode_to(&self, output: &mut Vec<u8>) {
        self.first.encode_to(output);
        self.second.encode_to(output);
    }
}

impl CanonicalDecode for ConflictingResultEvidence {
    fn decode_from(decoder: &mut Decoder<'_>) -> Result<Self> {
        Ok(Self {
            first: SignedReservationResult::decode_from(decoder)?,
            second: SignedReservationResult::decode_from(decoder)?,
        })
    }
}
