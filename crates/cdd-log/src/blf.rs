//! Vector's binary log (`.blf`).
//!
//! A file is a header (`LOGG`) followed by objects (`LOBJ`). Almost all of them are
//! `LOG_CONTAINER`s that hold, zlib-compressed, the stream of the real objects; an object may
//! begin in one container and end in the next. Of the real objects the CAN frames are read:
//! `CAN_MESSAGE` (1), `CAN_MESSAGE2` (86), `CAN_FD_MESSAGE` (100) and `CAN_FD_MESSAGE_64` (101).
//! Everything else (events, statistics, LIN, FlexRay, ...) is skipped.

use crate::{CanFrame, LogError};
use flate2::read::ZlibDecoder;
use std::io::Read;

const LOG_CONTAINER: u32 = 10;
const CAN_MESSAGE: u32 = 1;
const CAN_MESSAGE2: u32 = 86;
const CAN_FD_MESSAGE: u32 = 100;
const CAN_FD_MESSAGE_64: u32 = 101;

/// `LOBJ`, header size, header version, object size, object type.
const BASE_HEADER: usize = 16;
const EXTENDED_ID: u32 = 0x8000_0000;

fn u16le(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32le(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64le(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

pub fn parse(bytes: &[u8]) -> Result<Vec<CanFrame>, LogError> {
    let bad = |m: &str| LogError::Format(format!("BLF: {m}"));
    if bytes.get(..4) != Some(b"LOGG") {
        return Err(bad("the file does not start with LOGG"));
    }
    let header_size = u32le(bytes, 4).ok_or_else(|| bad("the file header is cut off"))? as usize;
    let mut pos = header_size;
    let mut stream: Vec<u8> = Vec::new();
    let mut frames = Vec::new();
    while pos + BASE_HEADER <= bytes.len() {
        if &bytes[pos..pos + 4] != b"LOBJ" {
            return Err(bad(&format!(
                "an object at byte {pos} does not start with LOBJ"
            )));
        }
        let hsize = u16le(bytes, pos + 4).unwrap_or(0) as usize;
        let size = u32le(bytes, pos + 8).unwrap_or(0) as usize;
        let kind = u32le(bytes, pos + 12).unwrap_or(0);
        if size < BASE_HEADER || pos + size > bytes.len() {
            // A log that was cut off: take what is whole.
            break;
        }
        if kind == LOG_CONTAINER {
            let body = &bytes[pos + hsize.max(BASE_HEADER)..pos + size];
            // Compression method, 6 reserved bytes, uncompressed size, 4 reserved bytes.
            let method = u16le(body, 0).ok_or_else(|| bad("a container is cut off"))?;
            let data = body
                .get(16..)
                .ok_or_else(|| bad("a container is cut off"))?;
            match method {
                0 => stream.extend_from_slice(data),
                2 => {
                    ZlibDecoder::new(data)
                        .read_to_end(&mut stream)
                        .map_err(|e| bad(&format!("a container does not inflate: {e}")))?;
                }
                m => return Err(bad(&format!("compression method {m} is not known"))),
            }
            let used = drain(&mut stream, &mut frames)?;
            stream.drain(..used);
        }
        pos += size + size % 4;
    }
    Ok(frames)
}

/// Reads every whole object of `stream`; returns how many bytes that used.
fn drain(stream: &mut [u8], frames: &mut Vec<CanFrame>) -> Result<usize, LogError> {
    let mut pos = 0;
    while pos + BASE_HEADER <= stream.len() {
        if &stream[pos..pos + 4] != b"LOBJ" {
            return Err(LogError::Format(format!(
                "BLF: an inner object does not start with LOBJ (at {pos})"
            )));
        }
        let hsize = u16le(stream, pos + 4).unwrap_or(0) as usize;
        let size = u32le(stream, pos + 8).unwrap_or(0) as usize;
        let kind = u32le(stream, pos + 12).unwrap_or(0);
        if size < BASE_HEADER {
            return Err(LogError::Format(
                "BLF: an inner object has a size below its header".into(),
            ));
        }
        if pos + size > stream.len() {
            break; // the rest comes with the next container
        }
        let obj = &stream[pos..pos + size];
        if let Some(f) = read_object(obj, hsize, kind) {
            frames.push(f);
        }
        pos += size + size % 4;
    }
    Ok(pos.min(stream.len()))
}

fn read_object(obj: &[u8], hsize: usize, kind: u32) -> Option<CanFrame> {
    if !matches!(
        kind,
        CAN_MESSAGE | CAN_MESSAGE2 | CAN_FD_MESSAGE | CAN_FD_MESSAGE_64
    ) {
        return None;
    }
    // After the base header: flags, (client index | timestamp status), object version, timestamp.
    let flags = u32le(obj, BASE_HEADER)?;
    let stamp = u64le(obj, BASE_HEADER + 8)?;
    let time = if flags & 2 != 0 {
        stamp as f64 * 1e-9
    } else {
        stamp as f64 * 1e-5
    };
    let b = obj.get(hsize.max(BASE_HEADER + 16)..)?;
    match kind {
        CAN_MESSAGE | CAN_MESSAGE2 => {
            // Channel, flags, data length code, identifier, 8 data bytes.
            let channel = u16le(b, 0)?;
            let fl = *b.get(2)?;
            let dlc = usize::from(*b.get(3)?).min(8);
            let id = u32le(b, 4)?;
            let data = b.get(8..8 + dlc)?.to_vec();
            if fl & 0x80 != 0 {
                return None; // a remote frame
            }
            Some(CanFrame {
                time,
                channel,
                id: id & !EXTENDED_ID,
                extended: id & EXTENDED_ID != 0,
                tx: fl & 1 != 0,
                data,
                fd: false,
            })
        }
        CAN_FD_MESSAGE => {
            // Channel, flags, dlc, id, frame length, bit count, FD flags, valid data bytes, 5 reserved, 64 data.
            let channel = u16le(b, 0)?;
            let fl = *b.get(2)?;
            let id = u32le(b, 4)?;
            let valid = usize::from(*b.get(15)?).min(64);
            let data = b.get(20..20 + valid)?.to_vec();
            Some(CanFrame {
                time,
                channel,
                id: id & !EXTENDED_ID,
                extended: id & EXTENDED_ID != 0,
                tx: fl & 1 != 0,
                data,
                fd: true,
            })
        }
        CAN_FD_MESSAGE_64 => {
            // Channel, dlc, valid data bytes, tx count, id, frame length, flags, 3 bit timing words,
            // 2 time offsets, bit count, direction, extended data offset, crc, then the data.
            let channel = u16::from(*b.first()?);
            let valid = usize::from(*b.get(2)?).min(64);
            let id = u32le(b, 4)?;
            let dir = *b.get(33)?;
            let data = b.get(40..40 + valid)?.to_vec();
            Some(CanFrame {
                time,
                channel,
                id: id & !EXTENDED_ID,
                extended: id & EXTENDED_ID != 0,
                tx: dir != 0,
                data,
                fd: true,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A one-container log with the given inner objects.
    fn log(inner: &[Vec<u8>], compress: bool) -> Vec<u8> {
        let mut stream = Vec::new();
        for o in inner {
            stream.extend_from_slice(o);
            stream.extend(std::iter::repeat_n(0u8, o.len() % 4));
        }
        let data = if compress {
            let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(&stream).unwrap();
            e.finish().unwrap()
        } else {
            stream.clone()
        };
        let mut file = b"LOGG".to_vec();
        file.extend_from_slice(&144u32.to_le_bytes());
        file.resize(144, 0);
        let size = 16 + 16 + data.len();
        file.extend_from_slice(b"LOBJ");
        file.extend_from_slice(&16u16.to_le_bytes());
        file.extend_from_slice(&1u16.to_le_bytes());
        file.extend_from_slice(&(size as u32).to_le_bytes());
        file.extend_from_slice(&LOG_CONTAINER.to_le_bytes());
        file.extend_from_slice(&(if compress { 2u16 } else { 0 }).to_le_bytes());
        file.extend_from_slice(&[0; 6]);
        file.extend_from_slice(&(stream.len() as u32).to_le_bytes());
        file.extend_from_slice(&[0; 4]);
        file.extend_from_slice(&data);
        file
    }

    fn can_message(channel: u16, id: u32, data: &[u8], stamp: u64, tx: bool) -> Vec<u8> {
        let mut o = b"LOBJ".to_vec();
        o.extend_from_slice(&32u16.to_le_bytes());
        o.extend_from_slice(&1u16.to_le_bytes());
        o.extend_from_slice(&(48u32).to_le_bytes());
        o.extend_from_slice(&CAN_MESSAGE.to_le_bytes());
        o.extend_from_slice(&2u32.to_le_bytes()); // timestamps in nanoseconds
        o.extend_from_slice(&[0; 4]);
        o.extend_from_slice(&stamp.to_le_bytes());
        o.extend_from_slice(&channel.to_le_bytes());
        o.push(u8::from(tx));
        o.push(data.len() as u8);
        o.extend_from_slice(&id.to_le_bytes());
        let mut d = data.to_vec();
        d.resize(8, 0);
        o.extend_from_slice(&d);
        o
    }

    #[test]
    fn reads_frames_from_a_compressed_container() {
        let bytes = log(
            &[
                can_message(1, 0x700, &[2, 0x10, 0], 1_500_000_000, true),
                can_message(2, 0x8000_0123, &[9], 2_000_000_000, false),
            ],
            true,
        );
        let f = parse(&bytes).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(
            (
                f[0].channel,
                f[0].id,
                f[0].extended,
                f[0].tx,
                f[0].data.clone()
            ),
            (1, 0x700, false, true, vec![2, 0x10, 0])
        );
        assert!((f[0].time - 1.5).abs() < 1e-9);
        assert!(f[1].extended && f[1].id == 0x123 && !f[1].tx);
    }

    #[test]
    fn an_uncompressed_container_and_a_cut_off_file() {
        let bytes = log(&[can_message(1, 0x100, &[1, 2, 3, 4], 10, false)], false);
        assert_eq!(parse(&bytes).unwrap().len(), 1);
        // A log that was cut in the middle of its container yields what was whole (here: nothing).
        assert_eq!(parse(&bytes[..bytes.len() - 5]).unwrap().len(), 0);
    }

    #[test]
    fn not_a_blf_file() {
        assert!(parse(b"nope").is_err());
    }
}
