//! Errors of decoding a payload and of encoding one.

use core::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The payload ends before the item does.
    Truncated { needed: usize, got: usize },
    /// Bytes remain after the last item of a message.
    TrailingBytes { extra: usize },
    /// A constant (SID, sub-function, DID) differs; `offset` is the byte.
    Prefix { offset: usize },
    /// The rest of the payload is not a whole number of elements, or there are too few or too many.
    Length { field: &'static str },
    /// A value the document does not allow: a BCD nibble above 9, a selector no case holds.
    Value { field: &'static str },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Truncated { needed, got } => {
                write!(f, "payload ends early: needs {needed} bytes, has {got}")
            }
            DecodeError::TrailingBytes { extra } => {
                write!(f, "{extra} bytes remain after the last item")
            }
            DecodeError::Prefix { offset } => {
                write!(f, "byte {offset} is not the constant the service requires")
            }
            DecodeError::Length { field } => write!(f, "`{field}` has a wrong length"),
            DecodeError::Value { field } => {
                write!(f, "`{field}` holds a value the description does not allow")
            }
        }
    }
}

impl core::error::Error for DecodeError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncodeError {
    /// The output buffer is too small; `needed` is the size that would have been enough.
    BufferTooSmall { needed: usize },
    /// A value does not fit the bits of its field.
    Range { field: &'static str },
    /// A list has too few or too many elements, or disagrees with the count that precedes it.
    Length { field: &'static str },
    /// The case written is not the one the selector value chooses (or the default was written for a
    /// value some case holds).
    Selector { field: &'static str },
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncodeError::BufferTooSmall { needed } => write!(
                f,
                "the output buffer is too small: {needed} bytes are needed"
            ),
            EncodeError::Range { field } => write!(f, "`{field}` does not fit its field"),
            EncodeError::Length { field } => write!(f, "`{field}` has a wrong number of elements"),
            EncodeError::Selector { field } => {
                write!(f, "`{field}` is not the case its selector value chooses")
            }
        }
    }
}

impl core::error::Error for EncodeError {}
