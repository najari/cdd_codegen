//! Vector's ASCII log (`.asc`).
//!
//! ```text
//! date Don Nov 24 15:27:14 2005
//! base hex  timestamps absolute
//! no internal events logged
//!    1.047413 1  700             Tx   d 8 02 10 00 00 00 00 00 00
//! ```
//!
//! A line is `time channel id direction d length data`; an extended identifier ends in `x`.
//! `base dec` writes identifiers and data in decimal, `timestamps relative` gives the time since
//! the previous line. Lines that are not classic data frames with a numeric identifier (remote\n//! frames, error frames, signal values, frames named instead of numbered, CAN FD) are skipped, and\n//! counted by [`parse_with_stats`].

use crate::{CanFrame, LogError};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub frames: usize,
    /// Lines that look like frames but are not classic data frames.
    pub skipped: usize,
}

pub fn parse(text: &str) -> Result<Vec<CanFrame>, LogError> {
    parse_with_stats(text).map(|(f, _)| f)
}

pub fn parse_with_stats(text: &str) -> Result<(Vec<CanFrame>, Stats), LogError> {
    let mut hex = true;
    let mut relative = false;
    let mut now = 0.0f64;
    let mut frames = Vec::new();
    let mut stats = Stats::default();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        match tokens[0] {
            "date" | "no" | "internal" | "Begin" | "End" => continue,
            "base" => {
                hex = tokens.get(1).is_none_or(|b| *b != "dec");
                relative = tokens
                    .windows(2)
                    .any(|w| w[0] == "timestamps" && w[1] == "relative");
                continue;
            }
            _ => {}
        }
        let Ok(t) = tokens[0].parse::<f64>() else {
            continue;
        };
        let time = if relative {
            now += t;
            now
        } else {
            t
        };
        // The first token that is a time and then a channel number marks a frame line.
        if tokens.len() < 6 {
            continue;
        }
        let Ok(channel) = tokens[1].parse::<u16>() else {
            // `1.0 CANFD ...`, `1.0 Statistic: ...`
            stats.skipped += 1;
            continue;
        };
        let (id_token, dir, kind, len_token) = (tokens[2], tokens[3], tokens[4], tokens[5]);
        if kind != "d" && kind != "D" {
            stats.skipped += 1;
            continue;
        }
        let (id_text, extended) = match id_token
            .strip_suffix('x')
            .or_else(|| id_token.strip_suffix('X'))
        {
            Some(s) => (s, true),
            None => (id_token, false),
        };
        let radix = if hex { 16 } else { 10 };
        // A log written with a database may name the message instead of numbering it: there is no identifier to use.
        let Ok(id) = u32::from_str_radix(id_text, radix) else {
            stats.skipped += 1;
            continue;
        };
        let len: usize = len_token.parse().map_err(|_| {
            LogError::Format(format!(
                "line {}: `{len_token}` is not a data length",
                n + 1
            ))
        })?;
        let data: Vec<u8> = tokens
            .iter()
            .skip(6)
            .take(len)
            .map(|b| {
                u8::from_str_radix(b, radix).map_err(|_| {
                    LogError::Format(format!("line {}: `{b}` is not a data byte", n + 1))
                })
            })
            .collect::<Result<_, _>>()?;
        if data.len() != len {
            return Err(LogError::Format(format!(
                "line {}: the length says {len} data bytes, the line has {}",
                n + 1,
                data.len()
            )));
        }
        frames.push(CanFrame {
            time,
            channel,
            id,
            extended,
            tx: dir.eq_ignore_ascii_case("tx"),
            data,
            fd: false,
        });
        stats.frames += 1;
    }
    Ok((frames, stats))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "date Don Nov 24 15:27:14 2005\nbase hex  timestamps absolute\nno internal events logged\n   1.047413 1  700             Tx   d 8 02 10 00 00 00 00 00 00\n   1.048937 1  600             Tx   d 8 02 50 00 00 00 00 00 00\n   2.5 2  18FEF100x       Rx   d 3 01 02 03\n   2.6 1  600  Rx  r\n";

    #[test]
    fn reads_classic_data_frames() {
        let (f, stats) = parse_with_stats(SAMPLE).unwrap();
        assert_eq!(f.len(), 3);
        assert_eq!(
            f[0],
            CanFrame {
                time: 1.047413,
                channel: 1,
                id: 0x700,
                extended: false,
                tx: true,
                data: vec![2, 0x10, 0, 0, 0, 0, 0, 0],
                fd: false
            }
        );
        assert!(f[2].extended && f[2].id == 0x18FE_F100 && !f[2].tx && f[2].data == [1, 2, 3]);
        assert_eq!(stats.frames, 3);
    }

    #[test]
    fn decimal_base_and_relative_time() {
        let text =
            "base dec  timestamps relative\n 1.0 1 1792 Rx d 2 16 0\n 0.5 1 1792 Rx d 1 255\n";
        let f = parse(text).unwrap();
        assert_eq!(f[0].id, 1792);
        assert_eq!(f[0].data, [16, 0]);
        assert!((f[1].time - 1.5).abs() < 1e-12);
    }

    #[test]
    fn a_damaged_line_is_an_error_with_its_number() {
        let err = parse("base hex\n 1.0 1 700 Tx d 4 01 02\n").unwrap_err();
        assert!(err.to_string().contains("line 2"), "{err}");
    }
}
