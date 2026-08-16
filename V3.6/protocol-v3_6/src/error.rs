use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("unexpected end of canonical input")]
    UnexpectedEnd,
    #[error("canonical input has {0} trailing bytes")]
    TrailingBytes(usize),
    #[error("invalid canonical boolean tag {0}")]
    InvalidBool(u8),
    #[error("invalid canonical option tag {0}")]
    InvalidOption(u8),
    #[error("invalid reservation status code {0}")]
    InvalidStatus(u16),
    #[error("canonical length {actual} exceeds limit {limit}")]
    LengthLimit { actual: usize, limit: usize },
    #[error("protocol integer {field} is outside 0..=2^63-1")]
    IntegerRange { field: &'static str },
    #[error("protocol value {field} must be greater than zero")]
    ZeroValue { field: &'static str },
    #[error("close_delay_blocks must equal acceptance_blocks + freeze_blocks")]
    CloseDelayMismatch,
    #[error("protocol arithmetic overflow in {0}")]
    ArithmeticOverflow(&'static str),
    #[error("max_ledger_entries must equal 64")]
    InvalidMaxEntries,
    #[error("ledger contains too many entries")]
    LedgerFull,
    #[error("ledger entry amount must be greater than zero")]
    ZeroAmount,
    #[error("reservation nonce is duplicated")]
    DuplicateNonce,
    #[error("ledger amount exceeds funding amount")]
    InsufficientRemainder,
    #[error("ledger checkpoint is inconsistent with the supplied entries")]
    CheckpointMismatch,
    #[error("invalid BLS public key encoding")]
    InvalidPublicKey,
    #[error("BLS public key at infinity is forbidden")]
    PublicKeyInfinity,
    #[error("invalid BLS signature encoding")]
    InvalidSignature,
    #[error("BLS signature at infinity is forbidden")]
    SignatureInfinity,
    #[error("BLS signature verification failed")]
    SignatureVerification,
    #[error("Merkle proof index is out of bounds")]
    MerkleIndex,
    #[error("Merkle proof direction is inconsistent with its index")]
    MerkleDirection,
    #[error("Merkle proof does not match the expected root")]
    MerkleRoot,
    #[error("recovery package field counts do not match")]
    RecoveryCount,
    #[error("evidence objects do not refer to the same protocol context")]
    EvidenceContext,
    #[error("evidence objects are not conflicting")]
    EvidenceNotConflicting,
    #[error("evidence objects are not in canonical hash order")]
    EvidenceOrder,
}

pub type Result<T> = std::result::Result<T, ProtocolError>;
