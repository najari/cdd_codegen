//! Reading CAN logs and putting diagnostic messages back together.
//!
//! * [`asc`]: Vector's ASCII log format;
//! * [`blf`]: Vector's binary log format;
//! * [`isotp`]: reassembling ISO-TP (ISO 15765-2) messages from the CAN frames of a log.
//!
//! The readers give [`CanFrame`]s; the reassembler turns frames of one CAN identifier into the
//! diagnostic payloads (`22 F1 90`) the generated code and the interpreter understand.

pub mod asc;
pub mod blf;
pub mod isotp;

use std::path::Path;

/// One frame of a log.
#[derive(Clone, Debug, PartialEq)]
pub struct CanFrame {
    /// Seconds since the start of the measurement.
    pub time: f64,
    pub channel: u16,
    /// The 11-bit or 29-bit identifier.
    pub id: u32,
    pub extended: bool,
    /// The log says the frame was sent by the logging node (`Tx`) rather than received.
    pub tx: bool,
    pub data: Vec<u8>,
    /// A CAN FD frame (up to 64 data bytes).
    pub fd: bool,
}

#[derive(Debug)]
pub enum LogError {
    Io(std::io::Error),
    /// The file is not a log of the format its extension names, or is damaged.
    Format(String),
}

impl std::fmt::Display for LogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogError::Io(e) => write!(f, "cannot read the log: {e}"),
            LogError::Format(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for LogError {}

impl From<std::io::Error> for LogError {
    fn from(e: std::io::Error) -> Self {
        LogError::Io(e)
    }
}

/// Reads a `.asc` or `.blf` log, by its extension.
pub fn read_frames(path: impl AsRef<Path>) -> Result<Vec<CanFrame>, LogError> {
    let path = path.as_ref();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("asc") => asc::parse(&String::from_utf8_lossy(&std::fs::read(path)?)),
        Some("blf") => blf::parse(&std::fs::read(path)?),
        _ => Err(LogError::Format(format!(
            "`{}`: only .asc and .blf logs are read",
            path.display()
        ))),
    }
}
