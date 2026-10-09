//! Simulation without a bus: a tester talks to a simulated ECU in the same process, like the
//! built-in simulated ECU of CANoe. Every payload that crosses is kept, so a test can look at the
//! conversation afterwards.

use crate::ecu::Responder;
use crate::error::EncodeError;
use crate::tester::{Transport, MAX_PAYLOAD};
use std::collections::VecDeque;
use std::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    TesterToEcu,
    EcuToTester,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LoopbackError {
    Encode(EncodeError),
    /// The tester waited for an answer the ECU never gave.
    NoResponse,
    /// The caller's buffer cannot hold the answer.
    BufferTooSmall {
        needed: usize,
    },
}

pub struct Loopback<R: Responder> {
    pub ecu: R,
    answers: VecDeque<Vec<u8>>,
    /// Every payload so far, in order.
    pub traffic: Vec<(Direction, Vec<u8>)>,
}

impl<R: Responder> Loopback<R> {
    pub fn new(ecu: R) -> Self {
        Loopback {
            ecu,
            answers: VecDeque::new(),
            traffic: Vec::new(),
        }
    }
}

impl<R: Responder> Transport for Loopback<R> {
    type Error = LoopbackError;

    fn send(&mut self, payload: &[u8]) -> Result<(), LoopbackError> {
        self.traffic
            .push((Direction::TesterToEcu, payload.to_vec()));
        let mut out = [0u8; MAX_PAYLOAD];
        let n = self
            .ecu
            .respond(payload, &mut out)
            .map_err(LoopbackError::Encode)?;
        if n > 0 {
            self.traffic
                .push((Direction::EcuToTester, out[..n].to_vec()));
            self.answers.push_back(out[..n].to_vec());
        }
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, LoopbackError> {
        let a = self.answers.pop_front().ok_or(LoopbackError::NoResponse)?;
        if a.len() > buf.len() {
            return Err(LoopbackError::BufferTooSmall { needed: a.len() });
        }
        buf[..a.len()].copy_from_slice(&a);
        Ok(a.len())
    }
}
