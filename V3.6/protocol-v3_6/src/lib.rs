mod crypto;
mod encoding;
mod error;
mod merkle;
mod puzzle;
mod types;
mod vectors;

pub use crypto::*;
pub use encoding::*;
pub use error::*;
pub use merkle::*;
pub use puzzle::*;
pub use types::*;
pub use vectors::*;

pub const PROTOCOL_VERSION: u16 = 0x0360;
pub const MAX_PROTOCOL_U64: u64 = i64::MAX as u64;
pub const MAX_LEDGER_ENTRIES: u64 = 64;
pub const MAX_CANONICAL_BLOB_BYTES: usize = 1_048_576;

pub type Bytes32 = [u8; 32];
pub type PublicKeyBytes = [u8; 48];
pub type SignatureBytes = [u8; 96];
