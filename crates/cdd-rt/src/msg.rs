//! What every generated message implements, and the shapes messages are exchanged in.

use crate::describe::Describe;
use crate::describe::Visitor;
use crate::error::{DecodeError, EncodeError};
use crate::io::{Reader, Writer};
use crate::path::Path;

/// A constant byte of a message at a known place: what tells one service from another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrefixByte {
    pub offset: usize,
    pub value: u8,
    /// Only these bits of the received byte are compared.
    pub mask: u8,
}

/// Whether `payload` has every constant of `prefix`.
pub fn prefix_matches(prefix: &[PrefixByte], payload: &[u8]) -> bool {
    prefix
        .iter()
        .all(|p| payload.get(p.offset).is_some_and(|b| b & p.mask == p.value))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Uds,
    Kwp2000,
    Unknown,
}

/// What the description says about one service, without its types: for tools that list services.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServiceInfo {
    /// The name CANoe's CAPL gives the service.
    pub name: &'static str,
    pub key: &'static str,
    pub sid: Option<u8>,
    pub has_request: bool,
    pub has_response: bool,
    /// The negative response codes the service lists.
    pub nrcs: &'static [u8],
    pub functional: Option<bool>,
    pub physical: Option<bool>,
}

pub trait Encode {
    /// Writes the payload to `out` and returns its length.
    fn encode(&self, out: &mut [u8]) -> Result<usize, EncodeError>;
}

pub trait Decode<'a>: Sized {
    fn decode(payload: &'a [u8]) -> Result<Self, DecodeError>;
}

/// A request or a positive response of one service.
pub trait Message: Describe {
    /// The name CANoe's CAPL gives the service, with `.Request` or `.Response`.
    const NAME: &'static str;
    /// The stable key of the service in the document.
    const KEY: &'static str;
    const MIN_LEN: usize;
    const MAX_LEN: usize;
    const PREFIX: &'static [PrefixByte];
}

/// A request, and the response that answers it.
pub trait Request: Message + Encode {
    const SID: u8;
    type Response<'r>: Decode<'r> + Message;
}

/// `7F <SID> <NRC>`: a service refusing a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NegativeResponse {
    pub sid: u8,
    pub nrc: u8,
}

impl NegativeResponse {
    pub const SID: u8 = 0x7F;

    pub fn parse(payload: &[u8]) -> Option<NegativeResponse> {
        match payload {
            [0x7F, sid, nrc] => Some(NegativeResponse {
                sid: *sid,
                nrc: *nrc,
            }),
            _ => None,
        }
    }

    pub fn encode(&self, out: &mut [u8]) -> Result<usize, EncodeError> {
        let mut w = Writer::new(out);
        w.put(&[Self::SID, self.sid, self.nrc])?;
        Ok(w.finish())
    }
}

impl Describe for NegativeResponse {
    fn describe(&self, path: &Path<'_>, v: &mut dyn Visitor) {
        use crate::describe::FieldValue;
        v.field(&path.name("SID_NR"), &FieldValue::uint(0x7F));
        v.field(
            &path.name("SID_RQ_NR"),
            &FieldValue::uint(u64::from(self.sid)),
        );
        v.field(&path.name("RC"), &FieldValue::uint(u64::from(self.nrc)));
    }
}

/// What a service answered, or what a simulated ECU decides to answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply<R> {
    Positive(R),
    Negative(NegativeResponse),
}

impl<R> Reply<R> {
    /// A negative response to the request with service id `sid`.
    pub const fn nrc(sid: u8, nrc: u8) -> Self {
        Reply::Negative(NegativeResponse { sid, nrc })
    }

    pub fn is_positive(&self) -> bool {
        matches!(self, Reply::Positive(_))
    }

    pub fn positive(self) -> Option<R> {
        match self {
            Reply::Positive(r) => Some(r),
            Reply::Negative(_) => None,
        }
    }

    /// The negative response code, if the reply is negative.
    pub fn response_code(&self) -> Option<u8> {
        match self {
            Reply::Negative(n) => Some(n.nrc),
            Reply::Positive(_) => None,
        }
    }

    /// For tests: the positive response, or a panic that says what came back instead.
    #[track_caller]
    pub fn expect_positive(self) -> R {
        match self {
            Reply::Positive(r) => r,
            Reply::Negative(n) => panic!("expected a positive response, got negative response code {:#04x} for service {:#04x}", n.nrc, n.sid),
        }
    }

    /// For tests: the negative response code, or a panic.
    #[track_caller]
    pub fn expect_nrc(self, nrc: u8) {
        match self {
            Reply::Negative(n) if n.nrc == nrc => {}
            Reply::Negative(n) => panic!(
                "expected negative response code {nrc:#04x}, got {:#04x}",
                n.nrc
            ),
            Reply::Positive(_) => {
                panic!("expected negative response code {nrc:#04x}, got a positive response")
            }
        }
    }
}

/// One fixed-size element of a repeated part of a message.
pub trait Element: Sized + Clone + core::fmt::Debug + PartialEq {
    const SIZE: usize;
    fn read(r: &mut Reader<'_>) -> Result<Self, DecodeError>;
    fn write(&self, w: &mut Writer<'_>) -> Result<(), EncodeError>;
    fn describe(&self, path: &Path<'_>, v: &mut dyn Visitor);
}

/// A repeated part of a message: elements still in their payload (what decoding gives), or elements
/// the caller holds (what encoding takes).
#[derive(Debug, PartialEq)]
pub enum List<'a, E: Element> {
    Encoded(&'a [u8]),
    Items(&'a [E]),
}

impl<E: Element> Clone for List<'_, E> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<E: Element> Copy for List<'_, E> {}

impl<'a, E: Element> List<'a, E> {
    /// The elements in `bytes`, which must hold a whole number of them.
    pub fn from_encoded(bytes: &'a [u8], field: &'static str) -> Result<Self, DecodeError> {
        if !bytes.len().is_multiple_of(E::SIZE) {
            return Err(DecodeError::Length { field });
        }
        Ok(List::Encoded(bytes))
    }

    pub fn len(&self) -> usize {
        match self {
            List::Encoded(b) => b.len() / E::SIZE,
            List::Items(i) => i.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn iter(&self) -> ListIter<'a, E> {
        ListIter {
            list: *self,
            pos: 0,
        }
    }

    pub fn write(&self, w: &mut Writer<'_>) -> Result<(), EncodeError> {
        match self {
            List::Encoded(b) => w.put(b),
            List::Items(items) => items.iter().try_for_each(|e| e.write(w)),
        }
    }

    pub fn describe(&self, path: &Path<'_>, v: &mut dyn Visitor) {
        for (i, e) in self.iter().enumerate() {
            if let Ok(e) = e {
                e.describe(&path.index(i), v);
            }
        }
    }
}

pub struct ListIter<'a, E: Element> {
    list: List<'a, E>,
    pos: usize,
}

impl<E: Element> Iterator for ListIter<'_, E> {
    type Item = Result<E, DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.list.len() {
            return None;
        }
        let i = self.pos;
        self.pos += 1;
        Some(match self.list {
            List::Items(items) => Ok(items[i].clone()),
            List::Encoded(b) => E::read(&mut Reader::new(&b[i * E::SIZE..(i + 1) * E::SIZE])),
        })
    }
}
