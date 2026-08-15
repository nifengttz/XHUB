use crate::{MAX_CANONICAL_BLOB_BYTES, ProtocolError, Result};

pub trait CanonicalEncode {
    fn encode_to(&self, output: &mut Vec<u8>);

    fn canonical_bytes(&self) -> Vec<u8> {
        let mut output = Vec::new();
        self.encode_to(&mut output);
        output
    }
}

pub trait CanonicalDecode: Sized {
    fn decode_from(decoder: &mut Decoder<'_>) -> Result<Self>;

    fn from_canonical_bytes(input: &[u8]) -> Result<Self> {
        let mut decoder = Decoder::new(input);
        let value = Self::decode_from(&mut decoder)?;
        decoder.finish()?;
        Ok(value)
    }
}

#[derive(Debug, Clone)]
pub struct Decoder<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    pub fn new(input: &'a [u8]) -> Self {
        Self { input, offset: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.input.len().saturating_sub(self.offset)
    }

    pub fn finish(self) -> Result<()> {
        if self.remaining() == 0 {
            Ok(())
        } else {
            Err(ProtocolError::TrailingBytes(self.remaining()))
        }
    }

    pub fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let end = self
            .offset
            .checked_add(N)
            .ok_or(ProtocolError::UnexpectedEnd)?;
        let bytes = self
            .input
            .get(self.offset..end)
            .ok_or(ProtocolError::UnexpectedEnd)?;
        self.offset = end;
        let mut output = [0_u8; N];
        output.copy_from_slice(bytes);
        Ok(output)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take::<1>()?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take()?))
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take()?))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.take()?))
    }

    pub fn bool(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(ProtocolError::InvalidBool(value)),
        }
    }

    pub fn bytes(&mut self, limit: usize) -> Result<Vec<u8>> {
        let length = self.u32()? as usize;
        if length > limit {
            return Err(ProtocolError::LengthLimit {
                actual: length,
                limit,
            });
        }
        let end = self
            .offset
            .checked_add(length)
            .ok_or(ProtocolError::UnexpectedEnd)?;
        let bytes = self
            .input
            .get(self.offset..end)
            .ok_or(ProtocolError::UnexpectedEnd)?;
        self.offset = end;
        Ok(bytes.to_vec())
    }
}

pub fn put_u16(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_be_bytes());
}

pub fn put_u32(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_be_bytes());
}

pub fn put_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_be_bytes());
}

pub fn put_bool(output: &mut Vec<u8>, value: bool) {
    output.push(u8::from(value));
}

pub fn put_bytes(output: &mut Vec<u8>, bytes: &[u8]) {
    let length = u32::try_from(bytes.len()).expect("canonical blob length exceeds u32");
    put_u32(output, length);
    output.extend_from_slice(bytes);
}

pub fn decode_bounded_blob(decoder: &mut Decoder<'_>) -> Result<Vec<u8>> {
    decoder.bytes(MAX_CANONICAL_BLOB_BYTES)
}
