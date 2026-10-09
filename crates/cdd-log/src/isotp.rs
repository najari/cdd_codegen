//! ISO-TP (ISO 15765-2) reassembly: the CAN frames of a log back into diagnostic payloads.
//!
//! One reassembler follows one stream of frames. Each CAN identifier has its own state, so the
//! requests of a tester and the answers of an ECU can be fed through the same one. Normal
//! addressing is the default; with `address_bytes = 1` the first data byte is an address
//! (extended or mixed addressing) and is left out of the payload.

use crate::CanFrame;
use std::collections::HashMap;

/// What the reassembler found.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A whole message.
    Message(Message),
    /// Frames that do not form a message.
    Error(Fault),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub id: u32,
    pub channel: u16,
    /// The time of the first frame (the single frame or the first frame).
    pub start: f64,
    /// The time of the last frame.
    pub end: f64,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fault {
    pub id: u32,
    pub time: f64,
    pub kind: FaultKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FaultKind {
    /// A consecutive frame with the wrong sequence number.
    WrongSequence { expected: u8, got: u8 },
    /// A consecutive frame with no first frame before it.
    UnexpectedConsecutive,
    /// A first frame while an earlier message of the same identifier was unfinished.
    Interrupted { missing: usize },
    /// A frame whose type nibble is not 0..=3, or that is too short for its type.
    Malformed,
}

struct Pending {
    channel: u16,
    start: f64,
    expected_len: usize,
    next_sn: u8,
    payload: Vec<u8>,
}

#[derive(Default)]
pub struct Reassembler {
    address_bytes: usize,
    pending: HashMap<(u16, u32), Pending>,
}

impl Reassembler {
    pub fn new() -> Self {
        Reassembler::default()
    }

    /// For extended or mixed addressing: how many leading data bytes are an address.
    pub fn with_address_bytes(mut self, n: usize) -> Self {
        self.address_bytes = n;
        self
    }

    /// Feeds one frame. Flow-control frames are consumed without an event.
    pub fn push(&mut self, f: &CanFrame) -> Option<Event> {
        let key = (f.channel, f.id);
        let d = f.data.get(self.address_bytes..)?;
        let pci = *d.first()?;
        let fault = |kind| {
            Some(Event::Error(Fault {
                id: f.id,
                time: f.time,
                kind,
            }))
        };
        match pci >> 4 {
            0 => {
                // Single frame: a length in the low nibble, or (CAN FD) in the next byte.
                let (len, start) = if pci & 0x0F == 0 && f.fd {
                    (usize::from(*d.get(1)?), 2)
                } else {
                    (usize::from(pci & 0x0F), 1)
                };
                if len == 0 || d.len() < start + len {
                    return fault(FaultKind::Malformed);
                }
                self.pending.remove(&key);
                Some(Event::Message(Message {
                    id: f.id,
                    channel: f.channel,
                    start: f.time,
                    end: f.time,
                    payload: d[start..start + len].to_vec(),
                }))
            }
            1 => {
                if d.len() < 2 {
                    return fault(FaultKind::Malformed);
                }
                let (len, start) = match (usize::from(pci & 0x0F) << 8) | usize::from(d[1]) {
                    0 => {
                        if d.len() < 6 {
                            return fault(FaultKind::Malformed);
                        }
                        (u32::from_be_bytes([d[2], d[3], d[4], d[5]]) as usize, 6)
                    }
                    n => (n, 2),
                };
                let first = &d[start..];
                let interrupted = self
                    .pending
                    .remove(&key)
                    .map(|p| p.expected_len - p.payload.len());
                if first.len() >= len {
                    // A "first frame" that already holds everything.
                    return Some(Event::Message(Message {
                        id: f.id,
                        channel: f.channel,
                        start: f.time,
                        end: f.time,
                        payload: first[..len].to_vec(),
                    }));
                }
                self.pending.insert(
                    key,
                    Pending {
                        channel: f.channel,
                        start: f.time,
                        expected_len: len,
                        next_sn: 1,
                        payload: first.to_vec(),
                    },
                );
                match interrupted {
                    Some(missing) => fault(FaultKind::Interrupted { missing }),
                    None => None,
                }
            }
            2 => {
                let sn = pci & 0x0F;
                let Some(p) = self.pending.get_mut(&key) else {
                    return fault(FaultKind::UnexpectedConsecutive);
                };
                if sn != p.next_sn {
                    let expected = p.next_sn;
                    self.pending.remove(&key);
                    return fault(FaultKind::WrongSequence { expected, got: sn });
                }
                p.next_sn = (p.next_sn + 1) & 0x0F;
                let want = p.expected_len - p.payload.len();
                let chunk = &d[1..];
                p.payload.extend_from_slice(&chunk[..chunk.len().min(want)]);
                if p.payload.len() >= p.expected_len {
                    let p = self.pending.remove(&key)?;
                    return Some(Event::Message(Message {
                        id: f.id,
                        channel: p.channel,
                        start: p.start,
                        end: f.time,
                        payload: p.payload,
                    }));
                }
                None
            }
            3 => None,
            _ => fault(FaultKind::Malformed),
        }
    }
}

/// Every message and fault of a stream of frames.
pub fn reassemble(frames: &[CanFrame]) -> Vec<Event> {
    let mut r = Reassembler::new();
    frames.iter().filter_map(|f| r.push(f)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(id: u32, time: f64, data: &[u8]) -> CanFrame {
        CanFrame {
            time,
            channel: 1,
            id,
            extended: false,
            tx: false,
            data: data.to_vec(),
            fd: false,
        }
    }

    fn payloads(events: &[Event]) -> Vec<Vec<u8>> {
        events
            .iter()
            .filter_map(|e| {
                if let Event::Message(m) = e {
                    Some(m.payload.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn single_frames() {
        let ev = reassemble(&[
            frame(0x700, 1.0, &[0x02, 0x10, 0x00, 0, 0, 0, 0, 0]),
            frame(0x600, 1.1, &[0x01, 0x7E, 0, 0, 0, 0, 0, 0]),
        ]);
        assert_eq!(payloads(&ev), vec![vec![0x10, 0x00], vec![0x7E]]);
    }

    #[test]
    fn a_segmented_message_from_the_canoe_sample_log() {
        // `1A 90` answered with 12 bytes: first frame, flow control from the tester, one consecutive frame.
        let ev = reassemble(&[
            frame(
                0x600,
                8.892781,
                &[0x10, 0x0C, 0x5A, 0x90, 0x98, 0x77, 0x54, 0x33],
            ),
            frame(0x700, 8.895829, &[0x30, 0x00, 0x14, 0, 0, 0, 0, 0]),
            frame(
                0x600,
                8.917305,
                &[0x21, 0x20, 0x00, 0x88, 0x88, 0x00, 0x02, 0x00],
            ),
        ]);
        assert_eq!(
            payloads(&ev),
            vec![vec![
                0x5A, 0x90, 0x98, 0x77, 0x54, 0x33, 0x20, 0x00, 0x88, 0x88, 0x00, 0x02
            ]]
        );
        let Event::Message(m) = &ev[0] else { panic!() };
        assert_eq!((m.start, m.end), (8.892781, 8.917305));
    }

    #[test]
    fn sequence_numbers_wrap_after_fifteen() {
        let mut frames = vec![frame(0x600, 0.0, &[0x10, 0x64, 1, 2, 3, 4, 5, 6])];
        for i in 0..14u8 {
            // The first consecutive frame is number 1; after 15 the numbers start again at 0.
            let sn = (i + 1) & 0x0F;
            frames.push(frame(
                0x600,
                0.1 + f64::from(i),
                &[0x20 | sn, 1, 2, 3, 4, 5, 6, 7],
            ));
        }
        let ev = reassemble(&frames);
        assert_eq!(payloads(&ev)[0].len(), 100);
    }

    #[test]
    fn errors_are_reported_not_hidden() {
        let ev = reassemble(&[
            frame(0x600, 0.0, &[0x10, 0x0C, 1, 2, 3, 4, 5, 6]),
            frame(0x600, 0.1, &[0x23, 1, 2, 3, 4, 5, 6, 7]),
        ]);
        assert_eq!(
            ev,
            vec![Event::Error(Fault {
                id: 0x600,
                time: 0.1,
                kind: FaultKind::WrongSequence {
                    expected: 1,
                    got: 3
                }
            })]
        );
        let ev = reassemble(&[frame(0x600, 0.0, &[0x21, 1, 2, 3, 4, 5, 6, 7])]);
        assert!(matches!(&ev[0], Event::Error(f) if f.kind == FaultKind::UnexpectedConsecutive));
    }

    #[test]
    fn streams_of_different_identifiers_do_not_mix() {
        let ev = reassemble(&[
            frame(0x600, 0.0, &[0x10, 0x09, 1, 2, 3, 4, 5, 6]),
            frame(0x200, 0.1, &[0x02, 0x10, 0x81, 0, 0, 0, 0, 0]),
            frame(0x600, 0.2, &[0x21, 7, 8, 9, 0, 0, 0, 0]),
        ]);
        assert_eq!(
            payloads(&ev),
            vec![vec![0x10, 0x81], vec![1, 2, 3, 4, 5, 6, 7, 8, 9]]
        );
    }

    #[test]
    fn extended_addressing_skips_the_address_byte() {
        let mut r = Reassembler::new().with_address_bytes(1);
        let Some(Event::Message(m)) =
            r.push(&frame(0x7E0, 0.0, &[0xF1, 0x03, 0x22, 0xF1, 0x90, 0, 0, 0]))
        else {
            panic!()
        };
        assert_eq!(m.payload, vec![0x22, 0xF1, 0x90]);
    }
}
