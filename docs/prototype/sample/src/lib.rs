//! Hand-written model of what the generator would emit for a few services.
#![no_std]
#![forbid(unsafe_code)]

use cdd_rt::{DecodeError, EncodeError, Message, Reader, Request, Writer};

pub const PROFILE_MATURITY: &str = "Experimental";

// ---- value types -------------------------------------------------------------------------

/// `TEXTTBL` whose entries are single values: an enum that keeps unknown raw values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlType {
    EnableRxAndTx,
    EnableRxAndDisableTx,
    DisableRxAndEnableTx,
    DisableRxAndTx,
    Other(u8),
}

impl ControlType {
    pub const fn from_raw(raw: u8) -> Self {
        match raw {
            0 => ControlType::EnableRxAndTx,
            1 => ControlType::EnableRxAndDisableTx,
            2 => ControlType::DisableRxAndEnableTx,
            3 => ControlType::DisableRxAndTx,
            other => ControlType::Other(other),
        }
    }

    pub const fn raw(self) -> u8 {
        match self {
            ControlType::EnableRxAndTx => 0,
            ControlType::EnableRxAndDisableTx => 1,
            ControlType::DisableRxAndEnableTx => 2,
            ControlType::DisableRxAndTx => 3,
            ControlType::Other(raw) => raw,
        }
    }
}

/// `TEXTTBL` with a range entry (`0` = off, `1..=255` = on): the raw value is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OffOn(pub u8);

impl OffOn {
    pub const fn label(self) -> &'static str {
        match self.0 {
            0 => "off",
            1..=255 => "on",
        }
    }
}

/// `LINCOMP` "Voltage": physical = 0.1 * raw + 0, unit V, 8 bits unsigned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Voltage(pub u8);

impl Voltage {
    pub const UNIT: &'static str = "V";
    pub const FACTOR: f64 = 0.1;
    pub const OFFSET: f64 = 0.0;

    pub fn physical(self) -> f64 {
        f64::from(self.0) * Self::FACTOR + Self::OFFSET
    }

    /// The nearest raw value; `None` when it is not finite or outside the 8-bit range.
    pub fn from_physical(value: f64) -> Option<Self> {
        let raw = (value - Self::OFFSET) / Self::FACTOR;
        // `f64::round` needs std; adding 0.5 and truncating rounds a non-negative value. A NaN is
        // in no range.
        if (-0.5..255.5).contains(&raw) {
            Some(Voltage((raw + 0.5) as u8))
        } else {
            None
        }
    }
}

/// `STRUCT` bit container of 8 bits: children are laid from the least significant bit upwards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommunicationType(u8);

impl CommunicationType {
    /// Bits 2..=3 are reserved: ignored on decode, zero on encode.
    const USED: u8 = 0b1111_0011;

    pub const fn from_raw(raw: u8) -> Self {
        CommunicationType(raw & Self::USED)
    }

    pub const fn raw(self) -> u8 {
        self.0
    }

    /// Offset 0, 2 bits.
    pub const fn message_type(self) -> u8 {
        self.0 & 0b11
    }

    /// Offset 4, 4 bits.
    pub const fn network(self) -> u8 {
        (self.0 >> 4) & 0b1111
    }

    pub fn set_message_type(&mut self, value: u8) -> Result<(), EncodeError> {
        if value > 0b11 {
            return Err(EncodeError::Range { field: "message_type" });
        }
        self.0 = (self.0 & !0b11) | value;
        Ok(())
    }

    pub fn set_network(&mut self, value: u8) -> Result<(), EncodeError> {
        if value > 0b1111 {
            return Err(EncodeError::Range { field: "network" });
        }
        self.0 = (self.0 & !(0b1111 << 4)) | (value << 4);
        Ok(())
    }
}

/// `IDENT`, `enc='asc'`, 17 elements of 8 bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vin(pub [u8; 17]);

impl Vin {
    pub fn as_str(&self) -> Result<&str, DecodeError> {
        match core::str::from_utf8(&self.0) {
            Ok(text) if text.is_ascii() => Ok(text),
            _ => Err(DecodeError::Value { field: "vin" }),
        }
    }
}

// ---- 0x22 F190: fixed layout --------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadVinRequest;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadVinResponse {
    pub vin: Vin,
}

impl<'a> Message<'a> for ReadVinRequest {
    const KEY: &'static str = "Ecu/Base/Identification/Vin/Read/REQ";
    const MIN_LEN: usize = 3;
    const MAX_LEN: usize = 3;

    fn decode(payload: &'a [u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(payload);
        r.expect(&[0x22, 0xF1, 0x90])?;
        r.finish()?;
        Ok(ReadVinRequest)
    }

    fn encode(&self, out: &mut [u8]) -> Result<usize, EncodeError> {
        let mut w = Writer::new(out);
        w.put(&[0x22, 0xF1, 0x90])?;
        Ok(w.finish())
    }
}

impl<'a> Message<'a> for ReadVinResponse {
    const KEY: &'static str = "Ecu/Base/Identification/Vin/Read/POS";
    const MIN_LEN: usize = 20;
    const MAX_LEN: usize = 20;

    fn decode(payload: &'a [u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(payload);
        r.expect(&[0x62, 0xF1, 0x90])?;
        let vin = Vin(r.array()?);
        r.finish()?;
        Ok(ReadVinResponse { vin })
    }

    fn encode(&self, out: &mut [u8]) -> Result<usize, EncodeError> {
        let mut w = Writer::new(out);
        w.put(&[0x62, 0xF1, 0x90])?;
        w.put(&self.vin.0)?;
        Ok(w.finish())
    }
}

impl<'a> Request<'a> for ReadVinRequest {
    const SID: u8 = 0x22;
    type Positive<'r> = ReadVinResponse;
}

// ---- 0x28: a field and a bit container ----------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommunicationControlRequest {
    pub control_type: ControlType,
    pub communication_type: CommunicationType,
}

impl<'a> Message<'a> for CommunicationControlRequest {
    const KEY: &'static str = "Ecu/Base/Communication/Control/Set/REQ";
    const MIN_LEN: usize = 3;
    const MAX_LEN: usize = 3;

    fn decode(payload: &'a [u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(payload);
        r.expect(&[0x28])?;
        let control_type = ControlType::from_raw(r.u8()?);
        let communication_type = CommunicationType::from_raw(r.u8()?);
        r.finish()?;
        Ok(CommunicationControlRequest { control_type, communication_type })
    }

    fn encode(&self, out: &mut [u8]) -> Result<usize, EncodeError> {
        let mut w = Writer::new(out);
        w.put(&[0x28, self.control_type.raw(), self.communication_type.raw()])?;
        Ok(w.finish())
    }
}

// ---- 0x59 02: an element repeated to the end of the payload -------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DtcAndStatus {
    /// 24 bits, most significant byte first.
    pub dtc: u32,
    pub status: u8,
}

/// The repeated elements of a response, decoded one at a time from the borrowed payload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DtcRecords<'a>(&'a [u8]);

impl<'a> DtcRecords<'a> {
    pub const ELEMENT_BYTES: usize = 4;

    pub fn len(&self) -> usize {
        self.0.len() / Self::ELEMENT_BYTES
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = DtcAndStatus> + 'a {
        self.0.as_chunks::<4>().0.iter().map(|e| DtcAndStatus {
            dtc: u32::from_be_bytes([0, e[0], e[1], e[2]]),
            status: e[3],
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadDtcByStatusMaskResponse<'a> {
    pub availability_mask: u8,
    pub records: DtcRecords<'a>,
}

impl<'a> Message<'a> for ReadDtcByStatusMaskResponse<'a> {
    const KEY: &'static str = "Ecu/Base/FaultMemory/FaultMemory/ReadByStatusMask/POS";
    const MIN_LEN: usize = 3;
    const MAX_LEN: usize = 4095;

    fn decode(payload: &'a [u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(payload);
        r.expect(&[0x59, 0x02])?;
        let availability_mask = r.u8()?;
        let rest = r.rest();
        if !rest.len().is_multiple_of(DtcRecords::ELEMENT_BYTES) {
            return Err(DecodeError::Length { field: "records" });
        }
        Ok(ReadDtcByStatusMaskResponse { availability_mask, records: DtcRecords(rest) })
    }

    fn encode(&self, out: &mut [u8]) -> Result<usize, EncodeError> {
        let mut w = Writer::new(out);
        w.put(&[0x59, 0x02, self.availability_mask])?;
        w.put(self.records.0)?;
        Ok(w.finish())
    }
}

// ---- dispatch: constant prefixes as slice patterns ----------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnyRequest {
    ReadVin(ReadVinRequest),
    CommunicationControl(CommunicationControlRequest),
}

impl AnyRequest {
    /// `None` when no request of the document starts with these bytes.
    pub fn decode(payload: &[u8]) -> Option<Result<Self, DecodeError>> {
        Some(match payload {
            [0x22, 0xF1, 0x90, ..] => ReadVinRequest::decode(payload).map(AnyRequest::ReadVin),
            [0x28, ..] => {
                CommunicationControlRequest::decode(payload).map(AnyRequest::CommunicationControl)
            }
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use cdd_rt::{decode_response, NegativeResponse};

    #[test]
    fn vin_request_and_response() {
        let mut buf = [0u8; 32];
        let n = ReadVinRequest.encode(&mut buf).unwrap();
        assert_eq!(&buf[..n], &[0x22, 0xF1, 0x90]);

        let mut wire = std::vec![0x62, 0xF1, 0x90];
        wire.extend_from_slice(b"WVWZZZ1JZXW000001");
        let response = decode_response::<ReadVinRequest>(&wire).unwrap().unwrap();
        assert_eq!(response.vin.as_str().unwrap(), "WVWZZZ1JZXW000001");
        let n = response.encode(&mut buf).unwrap();
        assert_eq!(&buf[..n], wire.as_slice());
    }

    #[test]
    fn negative_response_and_errors() {
        let negative = decode_response::<ReadVinRequest>(&[0x7F, 0x22, 0x31]).unwrap();
        assert_eq!(negative, Err(NegativeResponse { sid: 0x22, nrc: 0x31 }));
        assert_eq!(
            ReadVinResponse::decode(&[0x62, 0xF1, 0x90, 0x41]),
            Err(DecodeError::Truncated { needed: 20, got: 4 })
        );
        assert_eq!(ReadVinRequest::decode(&[0x22, 0xF1, 0x91]), Err(DecodeError::Prefix { offset: 2 }));
        assert_eq!(
            ReadVinRequest::decode(&[0x22, 0xF1, 0x90, 0x00]),
            Err(DecodeError::TrailingBytes { extra: 1 })
        );
        assert_eq!(ReadVinRequest.encode(&mut [0u8; 2]), Err(EncodeError::BufferTooSmall { needed: 3 }));
    }

    #[test]
    fn bit_container() {
        let request = CommunicationControlRequest::decode(&[0x28, 0x03, 0x1D]).unwrap();
        assert_eq!(request.control_type, ControlType::DisableRxAndTx);
        assert_eq!(request.communication_type.message_type(), 0b01);
        assert_eq!(request.communication_type.network(), 0b0001);
        // The reserved bits 2..=3 were set on the wire and are written back as zero.
        let mut buf = [0u8; 3];
        request.encode(&mut buf).unwrap();
        assert_eq!(buf, [0x28, 0x03, 0x11]);

        let mut value = CommunicationType::default();
        value.set_network(0xF).unwrap();
        value.set_message_type(0b10).unwrap();
        assert_eq!(value.raw(), 0xF2);
        assert_eq!(value.set_message_type(4), Err(EncodeError::Range { field: "message_type" }));
        assert_eq!(value.raw(), 0xF2);
    }

    #[test]
    fn value_types() {
        assert_eq!(ControlType::from_raw(0x42), ControlType::Other(0x42));
        assert_eq!(ControlType::from_raw(0x42).raw(), 0x42);
        assert_eq!(OffOn(0).label(), "off");
        assert_eq!(OffOn(7).label(), "on");
        assert!((Voltage(125).physical() - 12.5).abs() < 1e-9);
        assert_eq!(Voltage::from_physical(12.5), Some(Voltage(125)));
        assert_eq!(Voltage::from_physical(25.6), None);
        assert_eq!(Voltage::from_physical(f64::NAN), None);
    }

    #[test]
    fn repeat_to_end() {
        let wire = [0x59, 0x02, 0xFF, 0x01, 0x23, 0x45, 0x08, 0xC0, 0x10, 0x00, 0x2F];
        let response = ReadDtcByStatusMaskResponse::decode(&wire).unwrap();
        assert_eq!(response.records.len(), 2);
        let records: std::vec::Vec<_> = response.records.iter().collect();
        assert_eq!(records[0], DtcAndStatus { dtc: 0x012345, status: 0x08 });
        assert_eq!(records[1], DtcAndStatus { dtc: 0xC01000, status: 0x2F });
        let mut buf = [0u8; 16];
        let n = response.encode(&mut buf).unwrap();
        assert_eq!(&buf[..n], &wire);
        assert_eq!(
            ReadDtcByStatusMaskResponse::decode(&wire[..10]),
            Err(DecodeError::Length { field: "records" })
        );
    }

    #[test]
    fn dispatch() {
        assert_eq!(
            AnyRequest::decode(&[0x22, 0xF1, 0x90]),
            Some(Ok(AnyRequest::ReadVin(ReadVinRequest)))
        );
        assert!(matches!(
            AnyRequest::decode(&[0x28, 0x00, 0x01]),
            Some(Ok(AnyRequest::CommunicationControl(_)))
        ));
        assert_eq!(AnyRequest::decode(&[0x3E, 0x00]), None);
    }
}
