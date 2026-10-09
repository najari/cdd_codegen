//! Runtime support for code generated from a CDD: cursors, errors and the message traits.
#![no_std]
#![forbid(unsafe_code)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The payload ends before the field does.
    Truncated { needed: usize, got: usize },
    /// Bytes remain after the last field of a fixed-size message.
    TrailingBytes { extra: usize },
    /// A constant byte (SID, subfunction, DID) does not match.
    Prefix { offset: usize },
    /// The rest of the payload is not a whole number of elements, or the count is out of range.
    Length { field: &'static str },
    /// A value the document does not allow for the field.
    Value { field: &'static str },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeError {
    BufferTooSmall { needed: usize },
    Range { field: &'static str },
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub const fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).filter(|end| *end <= self.buf.len());
        match end {
            Some(end) => {
                let out = &self.buf[self.pos..end];
                self.pos = end;
                Ok(out)
            }
            None => Err(DecodeError::Truncated {
                needed: self.pos.saturating_add(n),
                got: self.buf.len(),
            }),
        }
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.bytes(N)?);
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.bytes(1)?[0])
    }

    /// An unsigned integer of `n` bytes (1..=8), most significant byte first (`bo='21'`).
    pub fn uint_be(&mut self, n: usize) -> Result<u64, DecodeError> {
        Ok(self.bytes(n)?.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b)))
    }

    /// The constant bytes of a message: SID, subfunction, DID.
    pub fn expect(&mut self, constant: &[u8]) -> Result<(), DecodeError> {
        let start = self.pos;
        let got = self.bytes(constant.len())?;
        match got.iter().zip(constant).position(|(a, b)| a != b) {
            Some(i) => Err(DecodeError::Prefix { offset: start + i }),
            None => Ok(()),
        }
    }

    pub fn rest(&mut self) -> &'a [u8] {
        let out = &self.buf[self.pos..];
        self.pos = self.buf.len();
        out
    }

    pub fn finish(self) -> Result<(), DecodeError> {
        match self.buf.len() - self.pos {
            0 => Ok(()),
            extra => Err(DecodeError::TrailingBytes { extra }),
        }
    }
}

pub struct Writer<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> Writer<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Writer { buf, pos: 0 }
    }

    pub fn put(&mut self, bytes: &[u8]) -> Result<(), EncodeError> {
        let end = self.pos + bytes.len();
        if end > self.buf.len() {
            return Err(EncodeError::BufferTooSmall { needed: end });
        }
        self.buf[self.pos..end].copy_from_slice(bytes);
        self.pos = end;
        Ok(())
    }

    pub fn uint_be(&mut self, value: u64, n: usize) -> Result<(), EncodeError> {
        self.put(&value.to_be_bytes()[8 - n..])
    }

    pub fn finish(self) -> usize {
        self.pos
    }
}

/// One request or response of one service of the document.
pub trait Message<'a>: Sized {
    /// The stable key of the service in the document, with `/REQ` or `/POS`.
    const KEY: &'static str;
    const MIN_LEN: usize;
    const MAX_LEN: usize;

    fn decode(payload: &'a [u8]) -> Result<Self, DecodeError>;
    /// Writes the payload to `out` and returns its length.
    fn encode(&self, out: &mut [u8]) -> Result<usize, EncodeError>;
}

/// A request and the positive response that answers it.
pub trait Request<'a>: Message<'a> {
    const SID: u8;
    type Positive<'r>: Message<'r>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NegativeResponse {
    pub sid: u8,
    pub nrc: u8,
}

/// Decodes the answer to a request of type `R`: its positive response or `7F <SID> <NRC>`.
pub fn decode_response<'r, R: Request<'r>>(
    payload: &'r [u8],
) -> Result<Result<R::Positive<'r>, NegativeResponse>, DecodeError> {
    match payload {
        [0x7F, sid, nrc] if *sid == R::SID => Ok(Err(NegativeResponse { sid: *sid, nrc: *nrc })),
        _ => R::Positive::decode(payload).map(Ok),
    }
}
