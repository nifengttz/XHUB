use chia_bls::{PublicKey, SecretKey, Signature, aggregate, sign, verify};
use chia_protocol::Bytes32;
use thiserror::Error;

use super::{
    ChannelArgs, ChannelSolution, FEE_POLICY, INVOICE_DOMAIN, MAX_PROTOCOL_U64, MERCHANT_AMOUNT,
    MIN_CLAIM_WINDOW_BLOCKS, PROTOCOL_VERSION, SETTLEMENT_DOMAIN, STATE_NUMBER, channel_id,
    hash_parts,
};

const VERSION: u16 = u16::from_be_bytes(PROTOCOL_VERSION);
const STATE: u64 = u64::from_be_bytes(STATE_NUMBER);
const FEE: u8 = FEE_POLICY[0];

/// Signing is deliberately expressed as a narrow capability so the state store
/// does not need to know how or where the Hub private key is held.
pub trait HubSigner {
    fn public_key(&self) -> PublicKey;
    fn sign_invoice(&self, invoice_hash: Bytes32) -> Signature;
    fn sign_claim(
        &self,
        commitment: &SettlementCommitment,
        agg_sig_me_additional_data: Bytes32,
    ) -> Signature;
}

impl HubSigner for SecretKey {
    fn public_key(&self) -> PublicKey {
        SecretKey::public_key(self)
    }

    fn sign_invoice(&self, invoice_hash: Bytes32) -> Signature {
        sign(self, invoice_hash.as_ref())
    }

    fn sign_claim(
        &self,
        commitment: &SettlementCommitment,
        agg_sig_me_additional_data: Bytes32,
    ) -> Signature {
        sign(
            self,
            commitment.claim_signature_message(agg_sig_me_additional_data),
        )
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("unsupported protocol version")]
    UnsupportedVersion,
    #[error("wrong network")]
    WrongNetwork,
    #[error("wrong funding coin")]
    WrongFundingCoin,
    #[error("wrong channel")]
    WrongChannel,
    #[error("wrong public key: {0}")]
    WrongPublicKey(&'static str),
    #[error("invalid field: {0}")]
    InvalidField(&'static str),
    #[error("invalid signature: {0}")]
    InvalidSignature(&'static str),
    #[error("payment expired")]
    PaymentExpired,
    #[error("claim window is shorter than the protocol minimum")]
    ClaimWindowTooShort,
    #[error("invalid binary encoding: {0}")]
    InvalidEncoding(&'static str),
}

impl ProtocolError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedVersion => "UNSUPPORTED_VERSION",
            Self::WrongNetwork => "WRONG_NETWORK",
            Self::WrongFundingCoin => "WRONG_FUNDING_COIN",
            Self::WrongChannel => "WRONG_CHANNEL",
            Self::WrongPublicKey(_) => "WRONG_PUBLIC_KEY",
            Self::InvalidField(_) => "INVALID_FIELD",
            Self::InvalidSignature(_) => "INVALID_SIGNATURE",
            Self::PaymentExpired => "PAYMENT_EXPIRED",
            Self::ClaimWindowTooShort => "CLAIM_WINDOW_TOO_SHORT",
            Self::InvalidEncoding(_) => "INVALID_ENCODING",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoiceFields {
    pub protocol_version: u16,
    pub genesis_challenge: Bytes32,
    pub funding_coin_id: Bytes32,
    pub channel_id: Bytes32,
    pub order_id: Bytes32,
    pub merchant_puzzle_hash: Bytes32,
    pub merchant_amount: u64,
    pub payment_expiry_height: u64,
    pub invoice_nonce: Bytes32,
}

impl InvoiceFields {
    pub fn new(
        genesis_challenge: Bytes32,
        funding_coin_id: Bytes32,
        order_id: Bytes32,
        merchant_puzzle_hash: Bytes32,
        payment_expiry_height: u64,
        invoice_nonce: Bytes32,
    ) -> Self {
        Self {
            protocol_version: VERSION,
            genesis_challenge,
            funding_coin_id,
            channel_id: channel_id(genesis_challenge, funding_coin_id),
            order_id,
            merchant_puzzle_hash,
            merchant_amount: MERCHANT_AMOUNT,
            payment_expiry_height,
            invoice_nonce,
        }
    }

    pub fn hash(&self) -> Bytes32 {
        hash_parts(&[
            INVOICE_DOMAIN,
            &self.protocol_version.to_be_bytes(),
            self.genesis_challenge.as_ref(),
            self.funding_coin_id.as_ref(),
            self.channel_id.as_ref(),
            self.order_id.as_ref(),
            self.merchant_puzzle_hash.as_ref(),
            &self.merchant_amount.to_be_bytes(),
            &self.payment_expiry_height.to_be_bytes(),
            self.invoice_nonce.as_ref(),
        ])
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerchantInvoice {
    pub fields: InvoiceFields,
    pub invoice_hash: Bytes32,
    pub hub_invoice_signature: Signature,
}

impl MerchantInvoice {
    pub(crate) const ENCODED_LENGTH: usize = 310;

    pub fn issue(fields: InvoiceFields, hub_secret_key: &SecretKey) -> Self {
        Self::issue_with_signer(fields, hub_secret_key)
    }

    pub fn issue_with_signer<S: HubSigner + ?Sized>(fields: InvoiceFields, signer: &S) -> Self {
        let invoice_hash = fields.hash();
        Self {
            fields,
            invoice_hash,
            hub_invoice_signature: signer.sign_invoice(invoice_hash),
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(4 + 210 + 96);
        bytes.extend_from_slice(b"WHN1");
        bytes.extend_from_slice(&self.fields.protocol_version.to_be_bytes());
        bytes.extend_from_slice(self.fields.genesis_challenge.as_ref());
        bytes.extend_from_slice(self.fields.funding_coin_id.as_ref());
        bytes.extend_from_slice(self.fields.channel_id.as_ref());
        bytes.extend_from_slice(self.fields.order_id.as_ref());
        bytes.extend_from_slice(self.fields.merchant_puzzle_hash.as_ref());
        bytes.extend_from_slice(&self.fields.merchant_amount.to_be_bytes());
        bytes.extend_from_slice(&self.fields.payment_expiry_height.to_be_bytes());
        bytes.extend_from_slice(self.fields.invoice_nonce.as_ref());
        bytes.extend_from_slice(&self.hub_invoice_signature.to_bytes());
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() != Self::ENCODED_LENGTH || &bytes[..4] != b"WHN1" {
            return Err(ProtocolError::InvalidEncoding("merchant_invoice"));
        }
        let mut cursor = ByteCursor::new(&bytes[4..]);
        let fields = InvoiceFields {
            protocol_version: u16::from_be_bytes(cursor.take()?),
            genesis_challenge: Bytes32::from(cursor.take::<32>()?),
            funding_coin_id: Bytes32::from(cursor.take::<32>()?),
            channel_id: Bytes32::from(cursor.take::<32>()?),
            order_id: Bytes32::from(cursor.take::<32>()?),
            merchant_puzzle_hash: Bytes32::from(cursor.take::<32>()?),
            merchant_amount: u64::from_be_bytes(cursor.take()?),
            payment_expiry_height: u64::from_be_bytes(cursor.take()?),
            invoice_nonce: Bytes32::from(cursor.take::<32>()?),
        };
        let hub_invoice_signature = Signature::from_bytes(&cursor.take::<96>()?)
            .map_err(|_| ProtocolError::InvalidEncoding("hub_invoice_signature"))?;
        cursor.finish()?;
        let invoice = Self {
            invoice_hash: fields.hash(),
            fields,
            hub_invoice_signature,
        };
        if invoice.invoice_hash != invoice.fields.hash() {
            return Err(ProtocolError::InvalidEncoding("invoice_hash"));
        }
        Ok(invoice)
    }

    pub fn verify(
        &self,
        args: &ChannelArgs,
        expected_funding_coin_id: Bytes32,
        current_height: u64,
    ) -> Result<(), ProtocolError> {
        if self.fields.protocol_version != VERSION {
            return Err(ProtocolError::UnsupportedVersion);
        }
        if self.fields.genesis_challenge != args.genesis_challenge {
            return Err(ProtocolError::WrongNetwork);
        }
        if self.fields.funding_coin_id != expected_funding_coin_id {
            return Err(ProtocolError::WrongFundingCoin);
        }
        if self.fields.channel_id
            != channel_id(self.fields.genesis_challenge, self.fields.funding_coin_id)
        {
            return Err(ProtocolError::WrongChannel);
        }
        if self.fields.merchant_amount != MERCHANT_AMOUNT {
            return Err(ProtocolError::InvalidField("merchant_amount"));
        }
        if self.fields.payment_expiry_height > MAX_PROTOCOL_U64 {
            return Err(ProtocolError::InvalidField("payment_expiry_height"));
        }
        if current_height > self.fields.payment_expiry_height {
            return Err(ProtocolError::PaymentExpired);
        }
        if self.invoice_hash != self.fields.hash()
            || !verify(
                &self.hub_invoice_signature,
                &args.hub_public_key,
                self.invoice_hash.as_ref(),
            )
        {
            return Err(ProtocolError::InvalidSignature("hub_invoice"));
        }
        Ok(())
    }

    pub fn merchant_status(&self, current_height: u64) -> MerchantPaymentStatus {
        if current_height > self.fields.payment_expiry_height {
            MerchantPaymentStatus::Expired
        } else {
            MerchantPaymentStatus::Pending
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementCommitment {
    pub protocol_version: u16,
    pub genesis_challenge: Bytes32,
    pub funding_coin_id: Bytes32,
    pub channel_id: Bytes32,
    pub state_number: u64,
    pub invoice_hash: Bytes32,
    pub order_id: Bytes32,
    pub merchant_puzzle_hash: Bytes32,
    pub merchant_amount: u64,
    pub user_puzzle_hash: Bytes32,
    pub user_remaining_amount: u64,
    pub nonce: Bytes32,
    pub payment_expiry_height: u64,
    pub claim_before_height: u64,
    pub refund_height: u64,
    pub fee_policy: u8,
}

impl SettlementCommitment {
    pub fn from_channel(
        args: &ChannelArgs,
        solution: &ChannelSolution,
    ) -> Result<Self, ProtocolError> {
        if solution.branch != 1 {
            return Err(ProtocolError::InvalidField("branch"));
        }
        Ok(Self {
            protocol_version: VERSION,
            genesis_challenge: args.genesis_challenge,
            funding_coin_id: solution.funding_coin_id,
            channel_id: channel_id(args.genesis_challenge, solution.funding_coin_id),
            state_number: STATE,
            invoice_hash: solution.invoice_hash,
            order_id: solution.order_id,
            merchant_puzzle_hash: solution.merchant_puzzle_hash,
            merchant_amount: MERCHANT_AMOUNT,
            user_puzzle_hash: args.user_puzzle_hash,
            user_remaining_amount: decode_u64(&solution.user_remaining_amount)?,
            nonce: solution.nonce,
            payment_expiry_height: decode_u64(&solution.payment_expiry_height)?,
            claim_before_height: decode_u64(&args.claim_before_height)?,
            refund_height: decode_u64(&args.refund_height)?,
            fee_policy: FEE,
        })
    }

    pub fn hash(&self) -> Bytes32 {
        hash_parts(&[
            SETTLEMENT_DOMAIN,
            &self.protocol_version.to_be_bytes(),
            self.genesis_challenge.as_ref(),
            self.funding_coin_id.as_ref(),
            self.channel_id.as_ref(),
            &self.state_number.to_be_bytes(),
            self.invoice_hash.as_ref(),
            self.order_id.as_ref(),
            self.merchant_puzzle_hash.as_ref(),
            &self.merchant_amount.to_be_bytes(),
            self.user_puzzle_hash.as_ref(),
            &self.user_remaining_amount.to_be_bytes(),
            self.nonce.as_ref(),
            &self.payment_expiry_height.to_be_bytes(),
            &self.claim_before_height.to_be_bytes(),
            &self.refund_height.to_be_bytes(),
            &[self.fee_policy],
        ])
    }

    pub fn claim_signature_message(&self, agg_sig_me_additional_data: Bytes32) -> Vec<u8> {
        [
            self.hash().as_ref(),
            self.funding_coin_id.as_ref(),
            agg_sig_me_additional_data.as_ref(),
        ]
        .concat()
    }

    pub fn validate(
        &self,
        args: &ChannelArgs,
        invoice: &MerchantInvoice,
    ) -> Result<(), ProtocolError> {
        if self.protocol_version != VERSION {
            return Err(ProtocolError::UnsupportedVersion);
        }
        if self.genesis_challenge != args.genesis_challenge {
            return Err(ProtocolError::WrongNetwork);
        }
        if self.funding_coin_id != invoice.fields.funding_coin_id {
            return Err(ProtocolError::WrongFundingCoin);
        }
        if self.channel_id != channel_id(self.genesis_challenge, self.funding_coin_id)
            || self.channel_id != invoice.fields.channel_id
        {
            return Err(ProtocolError::WrongChannel);
        }
        if self.state_number != STATE {
            return Err(ProtocolError::InvalidField("state_number"));
        }
        if self.invoice_hash != invoice.invoice_hash {
            return Err(ProtocolError::InvalidField("invoice_hash"));
        }
        if self.order_id != invoice.fields.order_id {
            return Err(ProtocolError::InvalidField("order_id"));
        }
        if self.merchant_puzzle_hash != invoice.fields.merchant_puzzle_hash {
            return Err(ProtocolError::InvalidField("merchant_puzzle_hash"));
        }
        if self.merchant_amount != MERCHANT_AMOUNT
            || self.merchant_amount != invoice.fields.merchant_amount
        {
            return Err(ProtocolError::InvalidField("merchant_amount"));
        }
        if self.user_puzzle_hash != args.user_puzzle_hash {
            return Err(ProtocolError::InvalidField("user_puzzle_hash"));
        }
        if self
                .merchant_amount
                .checked_add(self.user_remaining_amount)
                .is_none_or(|amount| amount > MAX_PROTOCOL_U64)
        {
            return Err(ProtocolError::InvalidField("user_remaining_amount"));
        }
        if self.payment_expiry_height != invoice.fields.payment_expiry_height {
            return Err(ProtocolError::InvalidField("payment_expiry_height"));
        }
        if self.user_remaining_amount > MAX_PROTOCOL_U64
            || self.payment_expiry_height > MAX_PROTOCOL_U64
            || self.claim_before_height > MAX_PROTOCOL_U64
            || self.refund_height > MAX_PROTOCOL_U64
        {
            return Err(ProtocolError::InvalidField("u64_range"));
        }
        if self.claim_before_height != decode_u64(&args.claim_before_height)?
            || self.refund_height != decode_u64(&args.refund_height)?
        {
            return Err(ProtocolError::InvalidField("settlement_height"));
        }
        if self
            .claim_before_height
            .checked_add(1)
            .is_none_or(|height| height != self.refund_height)
        {
            return Err(ProtocolError::InvalidField("refund_height"));
        }
        if self
            .payment_expiry_height
            .saturating_add(MIN_CLAIM_WINDOW_BLOCKS)
            > self.claim_before_height
        {
            return Err(ProtocolError::ClaimWindowTooShort);
        }
        if self.fee_policy != FEE {
            return Err(ProtocolError::InvalidField("fee_policy"));
        }
        Ok(())
    }

    fn encode_into(&self, bytes: &mut Vec<u8>) {
        bytes.extend_from_slice(&self.protocol_version.to_be_bytes());
        bytes.extend_from_slice(self.genesis_challenge.as_ref());
        bytes.extend_from_slice(self.funding_coin_id.as_ref());
        bytes.extend_from_slice(self.channel_id.as_ref());
        bytes.extend_from_slice(&self.state_number.to_be_bytes());
        bytes.extend_from_slice(self.invoice_hash.as_ref());
        bytes.extend_from_slice(self.order_id.as_ref());
        bytes.extend_from_slice(self.merchant_puzzle_hash.as_ref());
        bytes.extend_from_slice(&self.merchant_amount.to_be_bytes());
        bytes.extend_from_slice(self.user_puzzle_hash.as_ref());
        bytes.extend_from_slice(&self.user_remaining_amount.to_be_bytes());
        bytes.extend_from_slice(self.nonce.as_ref());
        bytes.extend_from_slice(&self.payment_expiry_height.to_be_bytes());
        bytes.extend_from_slice(&self.claim_before_height.to_be_bytes());
        bytes.extend_from_slice(&self.refund_height.to_be_bytes());
        bytes.push(self.fee_policy);
    }

    fn decode(cursor: &mut ByteCursor<'_>) -> Result<Self, ProtocolError> {
        Ok(Self {
            protocol_version: u16::from_be_bytes(cursor.take()?),
            genesis_challenge: Bytes32::from(cursor.take::<32>()?),
            funding_coin_id: Bytes32::from(cursor.take::<32>()?),
            channel_id: Bytes32::from(cursor.take::<32>()?),
            state_number: u64::from_be_bytes(cursor.take()?),
            invoice_hash: Bytes32::from(cursor.take::<32>()?),
            order_id: Bytes32::from(cursor.take::<32>()?),
            merchant_puzzle_hash: Bytes32::from(cursor.take::<32>()?),
            merchant_amount: u64::from_be_bytes(cursor.take()?),
            user_puzzle_hash: Bytes32::from(cursor.take::<32>()?),
            user_remaining_amount: u64::from_be_bytes(cursor.take()?),
            nonce: Bytes32::from(cursor.take::<32>()?),
            payment_expiry_height: u64::from_be_bytes(cursor.take()?),
            claim_before_height: u64::from_be_bytes(cursor.take()?),
            refund_height: u64::from_be_bytes(cursor.take()?),
            fee_policy: cursor.take::<1>()?[0],
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentIntent {
    pub commitment: SettlementCommitment,
    pub user_public_key: PublicKey,
    pub user_claim_signature: Signature,
}

impl PaymentIntent {
    const ENCODED_LENGTH: usize = 455;

    pub fn sign(
        commitment: SettlementCommitment,
        invoice: &MerchantInvoice,
        args: &ChannelArgs,
        user_secret_key: &SecretKey,
        agg_sig_me_additional_data: Bytes32,
        current_height: u64,
    ) -> Result<Self, ProtocolError> {
        invoice.verify(args, commitment.funding_coin_id, current_height)?;
        commitment.validate(args, invoice)?;
        if current_height > commitment.payment_expiry_height {
            return Err(ProtocolError::PaymentExpired);
        }
        let user_public_key = user_secret_key.public_key();
        if user_public_key != args.user_public_key {
            return Err(ProtocolError::WrongPublicKey("user"));
        }
        let message = commitment.claim_signature_message(agg_sig_me_additional_data);
        Ok(Self {
            commitment,
            user_public_key,
            user_claim_signature: sign(user_secret_key, message),
        })
    }

    pub fn verify(
        &self,
        invoice: &MerchantInvoice,
        args: &ChannelArgs,
        agg_sig_me_additional_data: Bytes32,
        current_height: u64,
    ) -> Result<(), ProtocolError> {
        invoice.verify(args, self.commitment.funding_coin_id, current_height)?;
        self.commitment.validate(args, invoice)?;
        if current_height > self.commitment.payment_expiry_height {
            return Err(ProtocolError::PaymentExpired);
        }
        if self.user_public_key != args.user_public_key {
            return Err(ProtocolError::WrongPublicKey("user"));
        }
        let message = self
            .commitment
            .claim_signature_message(agg_sig_me_additional_data);
        if !verify(&self.user_claim_signature, &self.user_public_key, message) {
            return Err(ProtocolError::InvalidSignature("user_claim"));
        }
        Ok(())
    }

    pub fn merchant_status(&self, current_height: u64) -> MerchantPaymentStatus {
        if current_height > self.commitment.payment_expiry_height {
            MerchantPaymentStatus::Expired
        } else {
            MerchantPaymentStatus::PendingHub
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::ENCODED_LENGTH);
        bytes.extend_from_slice(b"WHI1");
        self.commitment.encode_into(&mut bytes);
        bytes.extend_from_slice(&self.user_public_key.to_bytes());
        bytes.extend_from_slice(&self.user_claim_signature.to_bytes());
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() != Self::ENCODED_LENGTH || &bytes[..4] != b"WHI1" {
            return Err(ProtocolError::InvalidEncoding("payment_intent"));
        }
        let mut cursor = ByteCursor::new(&bytes[4..]);
        let commitment = SettlementCommitment::decode(&mut cursor)?;
        let user_public_key = PublicKey::from_bytes(&cursor.take::<48>()?)
            .map_err(|_| ProtocolError::InvalidEncoding("user_public_key"))?;
        let user_claim_signature = Signature::from_bytes(&cursor.take::<96>()?)
            .map_err(|_| ProtocolError::InvalidEncoding("user_claim_signature"))?;
        cursor.finish()?;
        Ok(Self {
            commitment,
            user_public_key,
            user_claim_signature,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentVoucher {
    pub intent: PaymentIntent,
    pub hub_public_key: PublicKey,
    pub hub_claim_signature: Signature,
}

impl PaymentVoucher {
    const ENCODED_LENGTH: usize = 603;

    pub fn issue(
        intent: PaymentIntent,
        invoice: &MerchantInvoice,
        args: &ChannelArgs,
        hub_secret_key: &SecretKey,
        agg_sig_me_additional_data: Bytes32,
        current_height: u64,
    ) -> Result<Self, ProtocolError> {
        Self::issue_with_signer(
            intent,
            invoice,
            args,
            hub_secret_key,
            agg_sig_me_additional_data,
            current_height,
        )
    }

    pub fn issue_with_signer<S: HubSigner + ?Sized>(
        intent: PaymentIntent,
        invoice: &MerchantInvoice,
        args: &ChannelArgs,
        signer: &S,
        agg_sig_me_additional_data: Bytes32,
        current_height: u64,
    ) -> Result<Self, ProtocolError> {
        intent.verify(invoice, args, agg_sig_me_additional_data, current_height)?;
        let hub_public_key = signer.public_key();
        if hub_public_key != args.hub_public_key {
            return Err(ProtocolError::WrongPublicKey("hub"));
        }
        let hub_claim_signature = signer.sign_claim(&intent.commitment, agg_sig_me_additional_data);
        Ok(Self {
            intent,
            hub_public_key,
            hub_claim_signature,
        })
    }

    pub fn verify(
        &self,
        invoice: &MerchantInvoice,
        args: &ChannelArgs,
        agg_sig_me_additional_data: Bytes32,
        current_height: u64,
    ) -> Result<(), ProtocolError> {
        self.intent
            .verify(invoice, args, agg_sig_me_additional_data, current_height)?;
        if self.hub_public_key != args.hub_public_key {
            return Err(ProtocolError::WrongPublicKey("hub"));
        }
        let message = self
            .intent
            .commitment
            .claim_signature_message(agg_sig_me_additional_data);
        if !verify(&self.hub_claim_signature, &self.hub_public_key, message) {
            return Err(ProtocolError::InvalidSignature("hub_claim"));
        }
        Ok(())
    }

    pub fn aggregated_signature(&self) -> Signature {
        aggregate([&self.intent.user_claim_signature, &self.hub_claim_signature])
    }

    pub fn merchant_status(
        &self,
        invoice: &MerchantInvoice,
        args: &ChannelArgs,
        agg_sig_me_additional_data: Bytes32,
        current_height: u64,
    ) -> Result<MerchantPaymentStatus, ProtocolError> {
        if current_height > self.intent.commitment.claim_before_height {
            return Ok(MerchantPaymentStatus::ClaimExpired);
        }
        // Payment expiry limits signing, not redemption of an already-issued Voucher.
        self.verify(
            invoice,
            args,
            agg_sig_me_additional_data,
            self.intent.commitment.payment_expiry_height,
        )?;
        Ok(MerchantPaymentStatus::PaidOffchain)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::ENCODED_LENGTH);
        bytes.extend_from_slice(b"WHV1");
        bytes.extend_from_slice(&self.intent.to_bytes());
        bytes.extend_from_slice(&self.hub_public_key.to_bytes());
        bytes.extend_from_slice(&self.hub_claim_signature.to_bytes());
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() != Self::ENCODED_LENGTH || &bytes[..4] != b"WHV1" {
            return Err(ProtocolError::InvalidEncoding("payment_voucher"));
        }
        let intent_end = 4 + PaymentIntent::ENCODED_LENGTH;
        let intent = PaymentIntent::from_bytes(&bytes[4..intent_end])?;
        let mut cursor = ByteCursor::new(&bytes[intent_end..]);
        let hub_public_key = PublicKey::from_bytes(&cursor.take::<48>()?)
            .map_err(|_| ProtocolError::InvalidEncoding("hub_public_key"))?;
        let hub_claim_signature = Signature::from_bytes(&cursor.take::<96>()?)
            .map_err(|_| ProtocolError::InvalidEncoding("hub_claim_signature"))?;
        cursor.finish()?;
        Ok(Self {
            intent,
            hub_public_key,
            hub_claim_signature,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MerchantPaymentStatus {
    Pending,
    PendingHub,
    PaidOffchain,
    Expired,
    ClaimExpired,
}

fn decode_u64(bytes: &[u8]) -> Result<u64, ProtocolError> {
    let value: [u8; 8] = bytes
        .try_into()
        .map_err(|_| ProtocolError::InvalidField("fixed_u64"))?;
    Ok(u64::from_be_bytes(value))
}

struct ByteCursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> ByteCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], ProtocolError> {
        let end = self
            .position
            .checked_add(N)
            .ok_or(ProtocolError::InvalidEncoding("length_overflow"))?;
        let slice = self
            .bytes
            .get(self.position..end)
            .ok_or(ProtocolError::InvalidEncoding("unexpected_end"))?;
        self.position = end;
        slice
            .try_into()
            .map_err(|_| ProtocolError::InvalidEncoding("field_length"))
    }

    fn finish(self) -> Result<(), ProtocolError> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(ProtocolError::InvalidEncoding("trailing_bytes"))
        }
    }
}
