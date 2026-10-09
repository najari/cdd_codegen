//! Runtime of code generated from CDD diagnostic descriptions by `cddgen`.
//!
//! * [`io`]: the cursors generated code reads and writes payloads with;
//! * [`msg`]: the traits every generated message implements, [`msg::Reply`] and [`msg::List`];
//! * [`describe`]: every value of a message with its path, raw, symbolic and physical meaning;
//! * [`tester`]: sending a request and understanding the answer, over a [`tester::Transport`] you provide;
//! * [`ecu`]: the simulated ECU's side, a [`ecu::Responder`];
//! * [`sim`] (feature `std`): tester and simulated ECU in one process.
//!
//! The crate is `no_std` and has no dependencies. Without the `alloc` feature nothing allocates.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "std")]
extern crate std;

pub mod describe;
pub mod ecu;
pub mod error;
pub mod io;
pub mod math;
pub mod msg;
pub mod nrc;
pub mod path;
#[cfg(feature = "std")]
pub mod sim;
pub mod tester;

pub use describe::{Describe, FieldValue, RawValue, Visitor};
pub use error::{DecodeError, EncodeError};
pub use io::{Order, Reader, Writer};
pub use msg::{
    Decode, Element, Encode, List, Message, NegativeResponse, PrefixByte, Protocol, Reply, Request,
    ServiceInfo,
};
pub use path::Path;
