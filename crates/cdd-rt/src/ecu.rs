//! The ECU side of a simulation: a [`Responder`] takes a request payload and decides the answer.
//! The generated `Server` implements it by calling one handler method per service.

use crate::error::EncodeError;
use crate::msg::{Encode, NegativeResponse, Reply};

pub trait Responder {
    /// Answers `request` into `out` and returns the length of the answer. 0 means: say nothing
    /// (the request asked for its positive response to be suppressed).
    fn respond(&mut self, request: &[u8], out: &mut [u8]) -> Result<usize, EncodeError>;
}

/// Writes `7F <sid> <nrc>`.
pub fn negative(out: &mut [u8], sid: u8, nrc: u8) -> Result<usize, EncodeError> {
    NegativeResponse { sid, nrc }.encode(out)
}

/// Writes what a handler decided: the positive response, unless the request asked for it to be
/// suppressed, or the negative response.
pub fn finish<R: Encode>(
    reply: Reply<R>,
    suppress_positive: bool,
    out: &mut [u8],
) -> Result<usize, EncodeError> {
    match reply {
        Reply::Positive(_) if suppress_positive => Ok(0),
        Reply::Positive(r) => r.encode(out),
        Reply::Negative(n) => n.encode(out),
    }
}
