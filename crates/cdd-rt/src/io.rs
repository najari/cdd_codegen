//! The cursors generated code reads and writes payloads with. Nothing here indexes a slice
//! without checking, so a short or garbled payload is an error and never a panic.

use crate::error::{DecodeError, EncodeError};

/// Which byte of a multi-byte number comes first on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// `bo='21'`
    Big,
    /// `bo='12'`
    Little,
}

fn fold(bytes: &[u8], order: Order) -> u64 {
    let step = |acc: u64, b: &u8| (acc << 8) | u64::from(*b);
    match order {
        Order::Big => bytes.iter().fold(0, step),
        Order::Little => bytes.iter().rev().fold(0, step),
    }
}

fn mask(bits: u32) -> u64 {
    if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    }
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub const fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    pub const fn position(&self) -> usize {
        self.pos
    }

    pub const fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.remaining() < n {
            return Err(DecodeError::Truncated {
                needed: self.pos.saturating_add(n),
                got: self.buf.len(),
            });
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.bytes(N)?);
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.bytes(1)?[0])
    }

    /// An unsigned integer of `n` bytes (at most 8).
    pub fn uint(&mut self, n: usize, order: Order) -> Result<u64, DecodeError> {
        Ok(fold(self.bytes(n)?, order))
    }

    /// A signed integer of `n` bytes whose `bits` low bits count (sign-extended from bit `bits - 1`).
    pub fn int(&mut self, n: usize, order: Order, bits: u32) -> Result<i64, DecodeError> {
        let v = self.uint(n, order)?;
        Ok(sign_extend(v, bits))
    }

    /// A packed BCD number of `n` bytes, most significant nibble first once the byte order is applied.
    pub fn bcd(&mut self, n: usize, order: Order, field: &'static str) -> Result<u64, DecodeError> {
        let bytes = self.bytes(n)?;
        let mut v: u64 = 0;
        let mut feed = |b: u8| -> Result<(), DecodeError> {
            for nib in [b >> 4, b & 0x0F] {
                if nib > 9 {
                    return Err(DecodeError::Value { field });
                }
                v = v
                    .checked_mul(10)
                    .and_then(|x| x.checked_add(u64::from(nib)))
                    .ok_or(DecodeError::Value { field })?;
            }
            Ok(())
        };
        match order {
            Order::Big => bytes.iter().try_for_each(|b| feed(*b))?,
            Order::Little => bytes.iter().rev().try_for_each(|b| feed(*b))?,
        }
        Ok(v)
    }

    /// Checks `value.len()` bytes against the constants a service requires. A bit that is not in
    /// `mask` may be anything (the suppress-positive-response bit of a sub-function).
    pub fn expect(&mut self, value: &[u8], mask: &[u8]) -> Result<(), DecodeError> {
        let start = self.pos;
        let got = self.bytes(value.len())?;
        for (i, ((g, v), m)) in got.iter().zip(value).zip(mask).enumerate() {
            if g & m != v & m {
                return Err(DecodeError::Prefix { offset: start + i });
            }
        }
        Ok(())
    }

    /// Everything that is left.
    pub fn rest(&mut self) -> &'a [u8] {
        let s = &self.buf[self.pos..];
        self.pos = self.buf.len();
        s
    }

    pub fn finish(self) -> Result<(), DecodeError> {
        match self.remaining() {
            0 => Ok(()),
            extra => Err(DecodeError::TrailingBytes { extra }),
        }
    }
}

/// Up to eight bytes as one big-endian number (a raw slot that counts elements).
pub fn be_uint(bytes: &[u8]) -> u64 {
    fold(bytes, Order::Big)
}

pub fn sign_extend(v: u64, bits: u32) -> i64 {
    if bits == 0 || bits >= 64 {
        return v as i64;
    }
    let shift = 64 - bits;
    ((v << shift) as i64) >> shift
}

/// `v` as an unsigned number of `bits` bits.
pub fn check_uint(v: u64, bits: u32, field: &'static str) -> Result<u64, EncodeError> {
    if v > mask(bits) {
        Err(EncodeError::Range { field })
    } else {
        Ok(v)
    }
}

/// `v` as a two's complement number of `bits` bits.
pub fn check_int(v: i64, bits: u32, field: &'static str) -> Result<u64, EncodeError> {
    if bits >= 64 {
        return Ok(v as u64);
    }
    let (lo, hi) = (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1);
    if v < lo || v > hi {
        Err(EncodeError::Range { field })
    } else {
        Ok(v as u64 & mask(bits))
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

    pub fn position(&self) -> usize {
        self.pos
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

    pub fn u8(&mut self, v: u8) -> Result<(), EncodeError> {
        self.put(&[v])
    }

    /// The low `n` bytes of `v`.
    pub fn uint(&mut self, v: u64, n: usize, order: Order) -> Result<(), EncodeError> {
        let be = v.to_be_bytes();
        let bytes = &be[8 - n.min(8)..];
        match order {
            Order::Big => self.put(bytes),
            Order::Little => {
                let end = self.pos + bytes.len();
                if end > self.buf.len() {
                    return Err(EncodeError::BufferTooSmall { needed: end });
                }
                for (i, b) in bytes.iter().rev().enumerate() {
                    self.buf[self.pos + i] = *b;
                }
                self.pos = end;
                Ok(())
            }
        }
    }

    /// `v` as packed BCD in `n` bytes.
    pub fn bcd(
        &mut self,
        v: u64,
        n: usize,
        order: Order,
        field: &'static str,
    ) -> Result<(), EncodeError> {
        // Digits, least significant first.
        let mut digits = [0u8; 20];
        let mut x = v;
        for d in digits.iter_mut() {
            *d = (x % 10) as u8;
            x /= 10;
        }
        if x != 0 || digits[(n * 2).min(20)..].iter().any(|d| *d != 0) || n * 2 > 20 {
            return Err(EncodeError::Range { field });
        }
        let end = self.pos + n;
        if end > self.buf.len() {
            return Err(EncodeError::BufferTooSmall { needed: end });
        }
        for i in 0..n {
            // Byte `i` counted from the most significant end.
            let hi = digits[n * 2 - 1 - i * 2];
            let lo = digits[n * 2 - 2 - i * 2];
            let at = match order {
                Order::Big => self.pos + i,
                Order::Little => self.pos + n - 1 - i,
            };
            self.buf[at] = (hi << 4) | lo;
        }
        self.pos = end;
        Ok(())
    }

    pub fn zeros(&mut self, n: usize) -> Result<(), EncodeError> {
        let end = self.pos + n;
        if end > self.buf.len() {
            return Err(EncodeError::BufferTooSmall { needed: end });
        }
        self.buf[self.pos..end].fill(0);
        self.pos = end;
        Ok(())
    }

    /// The number of bytes written.
    pub fn finish(self) -> usize {
        self.pos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_byte_orders_and_never_overruns() {
        let mut r = Reader::new(&[0x12, 0x34, 0x56]);
        assert_eq!(r.uint(2, Order::Big), Ok(0x1234));
        assert_eq!(
            r.uint(2, Order::Little),
            Err(DecodeError::Truncated { needed: 4, got: 3 })
        );
        assert_eq!(r.u8(), Ok(0x56));
        assert_eq!(r.finish(), Ok(()));
        let mut r = Reader::new(&[0x34, 0x12]);
        assert_eq!(r.uint(2, Order::Little), Ok(0x1234));
    }

    #[test]
    fn signed_values_extend_from_their_width() {
        let mut r = Reader::new(&[0xFF]);
        assert_eq!(r.int(1, Order::Big, 8), Ok(-1));
        assert_eq!(sign_extend(0x7F, 7), -1);
        assert_eq!(sign_extend(0x3F, 7), 63);
        assert_eq!(check_int(-64, 7, "x"), Ok(0x40));
        assert_eq!(
            check_int(64, 7, "x"),
            Err(EncodeError::Range { field: "x" })
        );
        assert_eq!(check_uint(255, 8, "x"), Ok(255));
        assert_eq!(
            check_uint(256, 8, "x"),
            Err(EncodeError::Range { field: "x" })
        );
    }

    #[test]
    fn expect_honours_the_mask() {
        // 0x82 is sub-function 0x02 with the suppress bit set.
        let mut r = Reader::new(&[0x19, 0x82]);
        assert_eq!(r.expect(&[0x19, 0x02], &[0xFF, 0x7F]), Ok(()));
        let mut r = Reader::new(&[0x19, 0x83]);
        assert_eq!(
            r.expect(&[0x19, 0x02], &[0xFF, 0x7F]),
            Err(DecodeError::Prefix { offset: 1 })
        );
    }

    #[test]
    fn bcd_round_trips_and_rejects_bad_nibbles() {
        let mut buf = [0u8; 4];
        let mut w = Writer::new(&mut buf);
        w.bcd(9877, 2, Order::Big, "x").unwrap();
        w.bcd(1234, 2, Order::Little, "y").unwrap();
        assert_eq!(w.finish(), 4);
        assert_eq!(buf, [0x98, 0x77, 0x34, 0x12]);
        let mut r = Reader::new(&buf);
        assert_eq!(r.bcd(2, Order::Big, "x"), Ok(9877));
        assert_eq!(r.bcd(2, Order::Little, "y"), Ok(1234));
        assert_eq!(
            Reader::new(&[0x1A]).bcd(1, Order::Big, "z"),
            Err(DecodeError::Value { field: "z" })
        );
        let mut w = Writer::new(&mut buf);
        assert_eq!(
            w.bcd(10_000, 2, Order::Big, "x"),
            Err(EncodeError::Range { field: "x" })
        );
    }

    #[test]
    fn writer_reports_the_size_it_needed() {
        let mut buf = [0u8; 2];
        let mut w = Writer::new(&mut buf);
        w.uint(0x1234, 2, Order::Little).unwrap();
        assert_eq!(w.put(&[1]), Err(EncodeError::BufferTooSmall { needed: 3 }));
        assert_eq!(buf, [0x34, 0x12]);
    }
}
