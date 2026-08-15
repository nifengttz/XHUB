use crate::{
    Bytes32, CanonicalDecode, CanonicalEncode, Decoder, ProtocolError, Result, put_u32,
    sha256_parts,
};

pub const LEDGER_LEAF_DOMAIN: &[u8] = b"XHUB_LEDGER_LEAF_V3_6";
pub const LEDGER_NODE_DOMAIN: &[u8] = b"XHUB_LEDGER_NODE_V3_6";
pub const LEDGER_EMPTY_DOMAIN: &[u8] = b"XHUB_LEDGER_EMPTY_V3_6";

pub fn empty_root() -> Bytes32 {
    sha256_parts(&[LEDGER_EMPTY_DOMAIN])
}

pub fn leaf_hash(entry_hash: &Bytes32) -> Bytes32 {
    sha256_parts(&[LEDGER_LEAF_DOMAIN, entry_hash])
}

pub fn node_hash(left: &Bytes32, right: &Bytes32) -> Bytes32 {
    sha256_parts(&[LEDGER_NODE_DOMAIN, left, right])
}

pub fn merkle_root(leaves: &[Bytes32]) -> Bytes32 {
    if leaves.is_empty() {
        return empty_root();
    }

    let mut level = leaves.to_vec();
    while level.len() > 1 {
        if level.len() % 2 == 1 {
            level.push(*level.last().expect("non-empty level"));
        }
        level = level
            .chunks_exact(2)
            .map(|pair| node_hash(&pair[0], &pair[1]))
            .collect();
    }
    level[0]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiblingSide {
    Right = 0,
    Left = 1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerkleStep {
    pub side: SiblingSide,
    pub sibling: Bytes32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerkleProof {
    pub leaf_index: u32,
    pub leaf_count: u32,
    pub steps: Vec<MerkleStep>,
}

impl MerkleProof {
    pub fn build(leaves: &[Bytes32], index: usize) -> Result<Self> {
        if leaves.is_empty() || index >= leaves.len() {
            return Err(ProtocolError::MerkleIndex);
        }

        let leaf_index = u32::try_from(index).map_err(|_| ProtocolError::MerkleIndex)?;
        let leaf_count = u32::try_from(leaves.len()).map_err(|_| ProtocolError::MerkleIndex)?;
        let mut cursor = index;
        let mut level = leaves.to_vec();
        let mut steps = Vec::new();

        while level.len() > 1 {
            if level.len() % 2 == 1 {
                level.push(*level.last().expect("non-empty level"));
            }
            let is_right = !cursor.is_multiple_of(2);
            let sibling_index = if is_right { cursor - 1 } else { cursor + 1 };
            steps.push(MerkleStep {
                side: if is_right {
                    SiblingSide::Left
                } else {
                    SiblingSide::Right
                },
                sibling: level[sibling_index],
            });
            level = level
                .chunks_exact(2)
                .map(|pair| node_hash(&pair[0], &pair[1]))
                .collect();
            cursor /= 2;
        }

        Ok(Self {
            leaf_index,
            leaf_count,
            steps,
        })
    }

    pub fn calculate_root(&self, leaf: Bytes32) -> Result<Bytes32> {
        if self.leaf_count == 0 || self.leaf_index >= self.leaf_count {
            return Err(ProtocolError::MerkleIndex);
        }

        let mut cursor = self.leaf_index as usize;
        let mut width = self.leaf_count as usize;
        let mut current = leaf;

        for step in &self.steps {
            if width <= 1 {
                return Err(ProtocolError::MerkleDirection);
            }
            let expected = if cursor.is_multiple_of(2) {
                SiblingSide::Right
            } else {
                SiblingSide::Left
            };
            if step.side != expected {
                return Err(ProtocolError::MerkleDirection);
            }
            current = match step.side {
                SiblingSide::Left => node_hash(&step.sibling, &current),
                SiblingSide::Right => node_hash(&current, &step.sibling),
            };
            cursor /= 2;
            width = width.div_ceil(2);
        }

        if width != 1 {
            return Err(ProtocolError::MerkleDirection);
        }
        Ok(current)
    }

    pub fn verify(&self, leaf: Bytes32, expected_root: Bytes32) -> Result<()> {
        if self.calculate_root(leaf)? == expected_root {
            Ok(())
        } else {
            Err(ProtocolError::MerkleRoot)
        }
    }
}

impl CanonicalEncode for MerkleProof {
    fn encode_to(&self, output: &mut Vec<u8>) {
        put_u32(output, self.leaf_index);
        put_u32(output, self.leaf_count);
        put_u32(
            output,
            u32::try_from(self.steps.len()).expect("Merkle proof length exceeds u32"),
        );
        for step in &self.steps {
            output.push(step.side as u8);
            output.extend_from_slice(&step.sibling);
        }
    }
}

impl CanonicalDecode for MerkleProof {
    fn decode_from(decoder: &mut Decoder<'_>) -> Result<Self> {
        let leaf_index = decoder.u32()?;
        let leaf_count = decoder.u32()?;
        let step_count = decoder.u32()? as usize;
        if step_count > 64 {
            return Err(ProtocolError::LengthLimit {
                actual: step_count,
                limit: 64,
            });
        }
        let mut steps = Vec::with_capacity(step_count);
        for _ in 0..step_count {
            let side = match decoder.u8()? {
                0 => SiblingSide::Right,
                1 => SiblingSide::Left,
                value => return Err(ProtocolError::InvalidBool(value)),
            };
            steps.push(MerkleStep {
                side,
                sibling: decoder.take()?,
            });
        }
        Ok(Self {
            leaf_index,
            leaf_count,
            steps,
        })
    }
}
