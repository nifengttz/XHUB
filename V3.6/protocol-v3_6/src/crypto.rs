use chia_bls::{PublicKey, SecretKey, Signature, sign, verify};
use sha2::{Digest, Sha256};

use crate::{Bytes32, ProtocolError, PublicKeyBytes, Result, SignatureBytes};

pub const BLS_CIPHERSUITE: &str = "BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_AUG_";

pub fn sha256_parts(parts: &[&[u8]]) -> Bytes32 {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

pub fn parse_public_key(bytes: &PublicKeyBytes) -> Result<PublicKey> {
    if bytes[0] == 0xc0 && bytes[1..].iter().all(|byte| *byte == 0) {
        return Err(ProtocolError::PublicKeyInfinity);
    }
    PublicKey::from_bytes(bytes).map_err(|_| ProtocolError::InvalidPublicKey)
}

pub fn parse_signature(bytes: &SignatureBytes) -> Result<Signature> {
    if bytes[0] == 0xc0 && bytes[1..].iter().all(|byte| *byte == 0) {
        return Err(ProtocolError::SignatureInfinity);
    }
    Signature::from_bytes(bytes).map_err(|_| ProtocolError::InvalidSignature)
}

pub fn sign_hash(secret_key: &SecretKey, message_hash: &Bytes32) -> SignatureBytes {
    sign(secret_key, message_hash).to_bytes()
}

pub fn verify_hash(
    public_key: &PublicKeyBytes,
    message_hash: &Bytes32,
    signature: &SignatureBytes,
) -> Result<()> {
    let public_key = parse_public_key(public_key)?;
    let signature = parse_signature(signature)?;
    if verify(&signature, &public_key, message_hash) {
        Ok(())
    } else {
        Err(ProtocolError::SignatureVerification)
    }
}

pub fn public_key_bytes(secret_key: &SecretKey) -> PublicKeyBytes {
    secret_key.public_key().to_bytes()
}
