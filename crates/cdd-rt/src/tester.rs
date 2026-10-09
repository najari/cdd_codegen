//! The tester side: send a request, wait for the answer, understand it. What carries the bytes is
//! yours (a CAN adapter with ISO-TP, DoIP, the [`Loopback`](crate::sim::Loopback) of the simulation).

use crate::error::{DecodeError, EncodeError};
use crate::msg::{Decode, Encode, NegativeResponse, Reply, Request};
use crate::nrc;
use core::fmt;

/// The largest payload an ISO-TP message of the classic kind carries.
pub const MAX_PAYLOAD: usize = 4095;

/// Moves payloads. A payload is a whole diagnostic message after ISO-TP reassembly: `22 F1 90`,
/// never a CAN frame.
pub trait Transport {
    type Error: fmt::Debug;

    fn send(&mut self, payload: &[u8]) -> Result<(), Self::Error>;

    /// Waits for the next payload and returns its length. Time-outs are the transport's business.
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum CallError<E> {
    Transport(E),
    Encode(EncodeError),
    Decode(DecodeError),
    /// A negative response to another service, or a payload that is neither.
    Unexpected {
        first_byte: Option<u8>,
    },
    /// The ECU kept answering "response pending" (0x78).
    TooManyPending,
}

impl<E: fmt::Debug> fmt::Display for CallError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::Transport(e) => write!(f, "transport failed: {e:?}"),
            CallError::Encode(e) => write!(f, "cannot encode the request: {e}"),
            CallError::Decode(e) => write!(f, "cannot decode the response: {e}"),
            CallError::Unexpected {
                first_byte: Some(b),
            } => write!(f, "an unexpected response starting with {b:#04x}"),
            CallError::Unexpected { first_byte: None } => write!(f, "an empty response"),
            CallError::TooManyPending => write!(f, "the ECU kept the request pending"),
        }
    }
}

impl<E: fmt::Debug> core::error::Error for CallError<E> {}

pub struct Tester<T: Transport> {
    pub transport: T,
    /// How many "response pending" answers are waited out before giving up.
    pub max_pending: u32,
}

impl<T: Transport> Tester<T> {
    pub fn new(transport: T) -> Self {
        Tester {
            transport,
            max_pending: 16,
        }
    }

    /// Sends a request and understands what comes back. The positive response borrows `rx`.
    pub fn call<'r, R: Request>(
        &mut self,
        request: &R,
        rx: &'r mut [u8],
    ) -> Result<Reply<R::Response<'r>>, CallError<T::Error>> {
        let mut tx = [0u8; MAX_PAYLOAD];
        let n = request.encode(&mut tx).map_err(CallError::Encode)?;
        self.transport
            .send(&tx[..n])
            .map_err(CallError::Transport)?;
        let mut pending = 0u32;
        let len = loop {
            let len = self.transport.recv(rx).map_err(CallError::Transport)?;
            match NegativeResponse::parse(&rx[..len]) {
                Some(neg) if neg.sid == R::SID && neg.nrc == nrc::RESPONSE_PENDING => {
                    pending += 1;
                    if pending > self.max_pending {
                        return Err(CallError::TooManyPending);
                    }
                }
                _ => break len,
            }
        };
        let rx: &'r [u8] = rx;
        let got = &rx[..len];
        if let Some(neg) = NegativeResponse::parse(got) {
            return if neg.sid == R::SID {
                Ok(Reply::Negative(neg))
            } else {
                Err(CallError::Unexpected {
                    first_byte: Some(neg.sid),
                })
            };
        }
        <R::Response<'r> as Decode<'r>>::decode(got)
            .map(Reply::Positive)
            .map_err(CallError::Decode)
    }

    /// Sends a message that no response answers (the "no response" tester-present).
    pub fn send<M: Encode>(&mut self, message: &M) -> Result<(), CallError<T::Error>> {
        let mut tx = [0u8; MAX_PAYLOAD];
        let n = message.encode(&mut tx).map_err(CallError::Encode)?;
        self.transport.send(&tx[..n]).map_err(CallError::Transport)
    }
}
