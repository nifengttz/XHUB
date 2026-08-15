use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use xhub_protocol_v3_6::{
    Bytes32, CanonicalEncode, ChannelTerms, MAX_PROTOCOL_U64, PublicKeyBytes,
};

pub mod api;

pub const PROFILE_ID: &str = "v3.6-testnet-vector-1";
pub const DEFAULT_ACCEPTANCE_BLOCKS: u64 = 12_288;
pub const DEFAULT_FREEZE_BLOCKS: u64 = 200;
pub const DEFAULT_CHALLENGE_BLOCKS: u64 = 6_000;
pub const FUNDING_CONFIRMATION_BLOCKS_TEST: u64 = 32;
pub const TEST_DELIVERY_THRESHOLD: u16 = 1;
pub const TEST_DELIVERY_PARTICIPANTS: u16 = 3;

#[derive(Debug, Error)]
pub enum WalletError {
    #[error(transparent)]
    Protocol(#[from] xhub_protocol_v3_6::ProtocolError),
    #[error("invalid wallet input: {0}")]
    Invalid(String),
    #[error("Funding draft was not found")]
    DraftNotFound,
    #[error("Funding draft is already confirmed and immutable")]
    DraftImmutable,
    #[error("Funding draft confirmation hash does not match")]
    ConfirmationMismatch,
}

pub type Result<T> = std::result::Result<T, WalletError>;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FundingTermsInput {
    pub network_id: String,
    pub acceptance_blocks: String,
    pub freeze_blocks: String,
    pub challenge_blocks: String,
    pub user_public_key: String,
    pub hub_state_public_key_a: String,
    pub state_rules_hash: String,
    pub funding_amount: String,
    pub user_remainder_puzzle_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FundingTermsPreview {
    pub protocol_version: &'static str,
    pub profile_id: &'static str,
    pub network_id: String,
    pub acceptance_blocks: u64,
    pub freeze_blocks: u64,
    pub close_delay_blocks: u64,
    pub challenge_blocks: u64,
    pub funding_confirmation_blocks: u64,
    pub max_ledger_entries: u64,
    pub channel_terms_hash: String,
    pub channel_terms_canonical_hex: String,
    pub funding_puzzle_hash: String,
    pub funding_puzzle_reveal: String,
    pub funding_module_hash: String,
    pub initial_closing_module_hash: String,
    pub subsequent_closing_module_hash: String,
    pub merchant_payment_module_hash: String,
    pub mainnet_approved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FundingDraft {
    pub draft_id: String,
    pub confirmed: bool,
    pub preview: FundingTermsPreview,
}

#[derive(Default)]
pub struct FundingDraftStore {
    drafts: HashMap<String, FundingDraft>,
}

impl FundingDraftStore {
    pub fn prepare(&mut self, input: &FundingTermsInput) -> Result<FundingDraft> {
        let terms = input.to_channel_terms()?;
        let preview = preview(&terms)?;
        let draft_id = preview.channel_terms_hash.clone();
        if let Some(existing) = self.drafts.get(&draft_id) {
            return Ok(existing.clone());
        }
        let draft = FundingDraft {
            draft_id: draft_id.clone(),
            confirmed: false,
            preview,
        };
        self.drafts.insert(draft_id, draft.clone());
        Ok(draft)
    }

    pub fn confirm(&mut self, draft_id: &str, channel_terms_hash: &str) -> Result<FundingDraft> {
        let draft = self
            .drafts
            .get_mut(draft_id)
            .ok_or(WalletError::DraftNotFound)?;
        if draft.preview.channel_terms_hash != channel_terms_hash {
            return Err(WalletError::ConfirmationMismatch);
        }
        draft.confirmed = true;
        Ok(draft.clone())
    }

    pub fn get(&self, draft_id: &str) -> Result<FundingDraft> {
        self.drafts
            .get(draft_id)
            .cloned()
            .ok_or(WalletError::DraftNotFound)
    }
}

impl FundingTermsInput {
    pub fn to_channel_terms(&self) -> Result<ChannelTerms> {
        let acceptance_blocks = parse_u64(&self.acceptance_blocks, "acceptance_blocks")?;
        let freeze_blocks = parse_u64(&self.freeze_blocks, "freeze_blocks")?;
        let challenge_blocks = parse_u64(&self.challenge_blocks, "challenge_blocks")?;
        let funding_amount = parse_u64(&self.funding_amount, "funding_amount")?;
        ChannelTerms::new(
            parse_hex(&self.network_id, "network_id")?,
            acceptance_blocks,
            freeze_blocks,
            challenge_blocks,
            parse_hex(&self.user_public_key, "user_public_key")?,
            parse_hex(&self.hub_state_public_key_a, "hub_state_public_key_a")?,
            parse_hex(&self.state_rules_hash, "state_rules_hash")?,
            funding_amount,
            parse_hex(
                &self.user_remainder_puzzle_hash,
                "user_remainder_puzzle_hash",
            )?,
        )
        .map_err(Into::into)
    }
}

pub fn preview(terms: &ChannelTerms) -> Result<FundingTermsPreview> {
    terms.validate()?;
    let (funding_puzzle_hash, funding_puzzle_reveal) =
        xhub_puzzles_v3_6::funding_puzzle_reveal(terms).map_err(WalletError::Invalid)?;
    let module_hashes = xhub_puzzles_v3_6::module_hashes();
    Ok(FundingTermsPreview {
        protocol_version: "0x0360",
        profile_id: PROFILE_ID,
        network_id: hex::encode(terms.network_id),
        acceptance_blocks: terms.acceptance_blocks,
        freeze_blocks: terms.freeze_blocks,
        close_delay_blocks: terms.close_delay_blocks,
        challenge_blocks: terms.challenge_blocks,
        funding_confirmation_blocks: FUNDING_CONFIRMATION_BLOCKS_TEST,
        max_ledger_entries: terms.max_ledger_entries,
        channel_terms_hash: hex::encode(terms.hash()?),
        channel_terms_canonical_hex: hex::encode(terms.canonical_bytes()),
        funding_puzzle_hash: hex::encode(funding_puzzle_hash),
        funding_puzzle_reveal: hex::encode(funding_puzzle_reveal.to_vec()),
        funding_module_hash: hex::encode(module_hashes.funding),
        initial_closing_module_hash: hex::encode(module_hashes.initial_closing),
        subsequent_closing_module_hash: hex::encode(module_hashes.subsequent_closing),
        merchant_payment_module_hash: hex::encode(module_hashes.merchant_payment),
        mainnet_approved: false,
    })
}

fn parse_u64(value: &str, field: &str) -> Result<u64> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || value.bytes().any(|byte| !byte.is_ascii_digit())
    {
        return Err(WalletError::Invalid(format!(
            "{field} must be a canonical unsigned decimal integer"
        )));
    }
    let parsed = value
        .parse::<u64>()
        .map_err(|_| WalletError::Invalid(format!("{field} is out of range")))?;
    if parsed == 0 || parsed > MAX_PROTOCOL_U64 {
        return Err(WalletError::Invalid(format!(
            "{field} must be within 1..={MAX_PROTOCOL_U64}"
        )));
    }
    Ok(parsed)
}

fn parse_hex<const N: usize>(value: &str, field: &str) -> Result<[u8; N]> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    if value.len() != N * 2 || value.bytes().any(|byte| !byte.is_ascii_hexdigit()) {
        return Err(WalletError::Invalid(format!(
            "{field} must encode exactly {N} bytes as hex"
        )));
    }
    let bytes = hex::decode(value)
        .map_err(|error| WalletError::Invalid(format!("invalid {field}: {error}")))?;
    bytes
        .try_into()
        .map_err(|_| WalletError::Invalid(format!("{field} has an invalid length")))
}

#[allow(dead_code)]
fn _type_checks(_: Bytes32, _: PublicKeyBytes) {}
