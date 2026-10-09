//! Reads Vector CANdela diagnostic descriptions (`*.cdd`) into a resolved model.
//!
//! The model is built for generating test and simulation code: for every service of an ECU
//! variant it holds the request, the positive response and the negative response as a
//! [layout IR](ir) with fixed byte positions. [`interp`] decodes and encodes by interpreting
//! that IR, which gives the code generator an independent reference.

pub mod datatype;
pub mod error;
pub mod interp;
pub mod ir;
pub mod model;
mod resolve;
pub mod source;
pub mod xml;

pub use error::{codes, Error, Issue, Severity};
pub use model::{Addressing, Cdd, Ecu, Nrc, Protocol, Service, Slot, Variant};

use std::path::Path;

impl Cdd {
    /// Decodes and resolves a CDD held in memory.
    pub fn from_bytes(bytes: &[u8]) -> Result<Cdd, Error> {
        let src = source::decode(bytes)?;
        resolve::build(&src.text, src.encoding)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Cdd, Error> {
        Cdd::from_bytes(&std::fs::read(path)?)
    }
}
