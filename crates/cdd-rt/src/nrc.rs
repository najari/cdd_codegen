//! The negative response codes the runtime itself needs. UDS (ISO 14229) and KWP2000 agree on
//! these. Every other code comes from the document's own table, in the generated `nrc` module.

pub const SERVICE_NOT_SUPPORTED: u8 = 0x11;
pub const SUBFUNCTION_NOT_SUPPORTED: u8 = 0x12;
pub const INCORRECT_MESSAGE_LENGTH_OR_INVALID_FORMAT: u8 = 0x13;
pub const CONDITIONS_NOT_CORRECT: u8 = 0x22;
pub const REQUEST_OUT_OF_RANGE: u8 = 0x31;
/// "Request correctly received, response pending": the real answer follows.
pub const RESPONSE_PENDING: u8 = 0x78;
