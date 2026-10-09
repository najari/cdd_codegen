//! Bytes computed by hand from the layout of the synthetic CDD files, one feature at a time.
//!
//! The differential test shows the generated code reads a description as the interpreter does;
//! these show what the bytes on the wire are. Bit containers follow CANdela's order: the first
//! member of a container takes the least significant bits.

use cdd_rt::{Encode, List};
use synthetic::mini_kwp;
use synthetic::mini_uds::{self as uds, service as s, types as t, AnyRequest, AnyResponse};

fn bytes<M: Encode>(m: &M) -> Vec<u8> {
    let mut buf = [0u8; 256];
    let n = m.encode(&mut buf).expect("encodes");
    buf[..n].to_vec()
}

fn request(payload: &[u8]) -> AnyRequest<'_> {
    AnyRequest::decode(payload)
        .unwrap_or_else(|| panic!("{payload:02X?} is a request of no service"))
        .unwrap_or_else(|e| panic!("{payload:02X?}: {e}"))
}

// ---- the suppress-positive-response bit ----------------------------------------------------

#[test]
fn the_suppress_bit_is_a_flag_and_the_rest_of_the_byte_selects_the_service() {
    let AnyRequest::DefaultSessionStart(r) = request(&[0x10, 0x81]) else {
        panic!("10 81 is the default session")
    };
    assert!(r.suppress_positive_response);
    let AnyRequest::ProgrammingSessionStart(r) = request(&[0x10, 0x02]) else {
        panic!("10 02 is the programming session")
    };
    assert!(!r.suppress_positive_response);
    let AnyRequest::ExtendedSessionStart(r) = request(&[0x10, 0x83]) else {
        panic!("10 83 is the extended session")
    };
    assert!(r.suppress_positive_response);
    assert!(
        AnyRequest::decode(&[0x10, 0x04]).is_none(),
        "no service has session 4"
    );

    assert_eq!(
        bytes(&s::default_session_start::Request {
            suppress_positive_response: true
        }),
        [0x10, 0x81]
    );
    assert_eq!(
        bytes(&s::extended_session_start::Request {
            suppress_positive_response: false
        }),
        [0x10, 0x03]
    );
}

#[test]
fn a_response_repeats_the_session_and_gives_the_timing() {
    let r = s::programming_session_start::Response {
        p2: 50,
        p2_star: 5000,
    };
    assert_eq!(bytes(&r), [0x50, 0x02, 0x00, 0x32, 0x13, 0x88]);
    let AnyResponse::ProgrammingSessionStart(back) =
        AnyResponse::decode(&[0x50, 0x02, 0x00, 0x32, 0x13, 0x88])
            .expect("a response")
            .expect("decodes")
    else {
        panic!("wrong service")
    };
    assert_eq!(back, r);
    // The echoed session is part of the response: the answer of another session is not this one's.
    assert!(matches!(
        AnyResponse::decode(&[0x50, 0x01, 0x00, 0x32, 0x13, 0x88]),
        Some(Ok(AnyResponse::DefaultSessionStart(_)))
    ));
}

#[test]
fn a_constant_sub_function_with_the_suppress_bit_takes_both_forms() {
    assert!(
        matches!(request(&[0x3E, 0x00]), AnyRequest::TesterPresentSend(r) if !r.suppress_positive_response)
    );
    assert!(
        matches!(request(&[0x3E, 0x80]), AnyRequest::TesterPresentSend(r) if r.suppress_positive_response)
    );
    assert!(AnyRequest::decode(&[0x3E, 0x01]).is_none());
    // A message too short to show which service it is, is no service's: what to answer is up to the ECU.
    assert!(AnyRequest::decode(&[0x3E]).is_none());
    assert!(matches!(
        AnyRequest::decode(&[0x3E, 0x00, 0x00]),
        Some(Err(_))
    ));
    assert_eq!(bytes(&s::tester_present_send::Response), [0x7E, 0x00]);
}

// ---- numbers, bit containers, text ---------------------------------------------------------

#[test]
fn a_bit_container_packs_its_members_from_the_least_significant_bit() {
    let battery = |alarm, mode, delta| s::battery_read::Response {
        voltage: t::Voltage2byte::from_raw(0x3138),
        temperature: t::Temperature1byte::from_raw(100),
        charging: t::OffOn1byte::from_raw(1),
        status: s::battery_read::Status {
            alarm: t::Flag1bit::from_raw(alarm),
            mode: t::Mode3bit::from_raw(mode),
            delta,
        },
    };
    // 62, the identifier 0100, the voltage, the temperature, the charging flag, then the container.
    let head = [0x62, 0x01, 0x00, 0x31, 0x38, 0x64, 0x01];
    for (alarm, mode, delta, container) in [
        (1, 0, 0, 0x01),
        (0, 1, 0, 0x02),
        (0, 7, 0, 0x0E),
        (0, 0, -1, 0xF0),
        (0, 0, 7, 0x70),
        (0, 0, -8, 0x80),
        (1, 1, -1, 0xF3),
    ] {
        let wire = [&head[..], &[container]].concat();
        assert_eq!(
            bytes(&battery(alarm, mode, delta)),
            wire,
            "alarm {alarm} mode {mode} delta {delta}"
        );
        let Some(Ok(AnyResponse::BatteryRead(back))) = AnyResponse::decode(&wire) else {
            panic!("{wire:02X?} is not read back")
        };
        assert_eq!(back, battery(alarm, mode, delta));
    }
}

#[test]
fn a_linear_conversion_goes_both_ways() {
    let v = t::Voltage2byte::from_raw(12_600);
    assert!((v.physical().unwrap() - 12.6).abs() < 1e-9);
    assert_eq!(t::Voltage2byte::UNIT, Some("V"));
    assert_eq!(t::Voltage2byte::from_physical(12.6), Some(v));
    assert_eq!(
        t::Voltage2byte::from_physical(65.535),
        Some(t::Voltage2byte::from_raw(u16::MAX))
    );
    assert_eq!(
        t::Voltage2byte::from_physical(65.54),
        None,
        "beyond what two bytes hold"
    );
    assert_eq!(t::Voltage2byte::from_physical(-1.0), None);
    assert_eq!(t::Voltage2byte::from_physical(f64::NAN), None);

    // An offset: -40 degrees at raw 0.
    assert_eq!(t::Temperature1byte::from_raw(0).physical(), Some(-40.0));
    assert_eq!(t::Temperature1byte::from_raw(100).physical(), Some(60.0));
    assert_eq!(
        t::Temperature1byte::from_physical(60.0),
        Some(t::Temperature1byte::from_raw(100))
    );
}

#[test]
fn text_tables_name_values_and_keep_the_ones_they_do_not() {
    assert_eq!(t::OffOn1byte::from_raw(0).label(), Some("off"));
    assert_eq!(t::OffOn1byte::from_raw(1).label(), Some("on"));
    assert_eq!(
        t::OffOn1byte::from_raw(200).label(),
        Some("on"),
        "a range of values has one name"
    );
    assert_eq!(t::Mode3bit::from_raw(1), t::Mode3bit::Run);
    assert_eq!(t::Mode3bit::from_raw(7).raw(), 7);
    assert_eq!(t::Mode3bit::from_raw(5), t::Mode3bit::Other(5));
    assert_eq!(t::Mode3bit::Other(5).raw(), 5);
}

#[test]
fn little_endian_signed_bcd_and_float_fields() {
    let counters = s::counters_write::Request {
        total: 0x0102_0304,
        offset: -2,
        serial: 1234,
        gain: 1.5,
    };
    let wire = [
        0x2E, 0x02, 0x00, 0x04, 0x03, 0x02, 0x01, 0xFF, 0xFE, 0x12, 0x34, 0x3F, 0xC0, 0x00, 0x00,
    ];
    assert_eq!(bytes(&counters), wire);
    assert!(matches!(request(&wire), AnyRequest::CountersWrite(r) if r == counters));

    // A BCD number has no nibble above 9.
    let mut bad = wire;
    bad[9] = 0x1A;
    assert!(matches!(AnyRequest::decode(&bad), Some(Err(_))));
    // And one that does not fit its digits cannot be written.
    let mut out = [0u8; 32];
    assert!(s::counters_write::Request {
        serial: 10_000,
        ..counters
    }
    .encode(&mut out)
    .is_err());
}

#[test]
fn fixed_text_and_variable_byte_fields_keep_their_lengths() {
    let vin = *b"WVWZZZ1JZXW000001";
    assert_eq!(
        bytes(&s::vin_read::Response { vin }),
        [&[0x62, 0xF1, 0x90][..], &vin].concat()
    );
    assert!(
        matches!(
            AnyResponse::decode(&[&[0x62, 0xF1, 0x90][..], &vin[..16]].concat()),
            Some(Err(_))
        ),
        "one character short"
    );

    let data = [1u8, 2, 3];
    assert_eq!(
        bytes(&s::blob_read::Response { bytes: &data }),
        [0x62, 0x03, 0x00, 1, 2, 3]
    );
    assert_eq!(
        bytes(&s::blob_read::Response { bytes: &[] }),
        [0x62, 0x03, 0x00]
    );
    let long = [0u8; 17];
    let mut out = [0u8; 64];
    assert!(
        s::blob_read::Response { bytes: &long }
            .encode(&mut out)
            .is_err(),
        "at most 16 bytes"
    );
    assert!(matches!(
        AnyResponse::decode(&[&[0x62, 0x03, 0x00][..], &long].concat()),
        Some(Err(_))
    ));
}

#[test]
fn a_structure_is_its_members_one_after_the_other() {
    let r = s::coding_read::Response {
        coding: s::coding_read::Coding {
            country: t::OffOn1byte::from_raw(1),
            limit: t::Voltage2byte::from_raw(0x0100),
        },
    };
    assert_eq!(bytes(&r), [0x62, 0x04, 0x00, 0x01, 0x01, 0x00]);
}

// ---- lists ---------------------------------------------------------------------------------

#[test]
fn a_list_to_the_end_of_the_message() {
    use s::fault_memory_read_by_status_mask::{ListOfDtcElement as Dtc, Response};
    let items = [
        Dtc {
            dtc: 0x010203,
            status_of_dtc: 0x08,
        },
        Dtc {
            dtc: 0xABCDEF,
            status_of_dtc: 0x2F,
        },
    ];
    let wire = [
        0x59, 0x02, 0xFF, 0x01, 0x02, 0x03, 0x08, 0xAB, 0xCD, 0xEF, 0x2F,
    ];
    assert_eq!(
        bytes(&Response {
            availability_mask: 0xFF,
            list_of_dtc: List::Items(&items)
        }),
        wire
    );

    let Some(Ok(AnyResponse::FaultMemoryReadByStatusMask(r))) = AnyResponse::decode(&wire) else {
        panic!("not read back")
    };
    assert_eq!(r.availability_mask, 0xFF);
    assert_eq!(r.list_of_dtc.len(), 2);
    assert_eq!(
        r.list_of_dtc.iter().collect::<Result<Vec<_>, _>>().unwrap(),
        items
    );

    // No trouble code is a valid answer; half of one is not.
    let Some(Ok(AnyResponse::FaultMemoryReadByStatusMask(none))) =
        AnyResponse::decode(&[0x59, 0x02, 0xFF])
    else {
        panic!("empty list")
    };
    assert!(none.list_of_dtc.is_empty());
    assert!(matches!(
        AnyResponse::decode(&[0x59, 0x02, 0xFF, 0x01, 0x02]),
        Some(Err(_))
    ));
    assert!(matches!(
        AnyResponse::decode(&[0x59, 0x02, 0xFF, 0x01, 0x02, 0x03, 0x08, 0x01]),
        Some(Err(_))
    ));

    assert!(
        matches!(request(&[0x19, 0x82, 0x08]), AnyRequest::FaultMemoryReadByStatusMask(r) if r.suppress_positive_response && r.status_mask == 0x08)
    );
}

#[test]
fn a_multiplexer_picks_its_layout_from_an_earlier_field() {
    use s::memory_read::{AddressAndSize as Mux, AddressAndSizeCase12, AddressAndSizeCase22};
    // Format 22: two bytes of address, two of size.
    let AnyRequest::MemoryRead(r) = request(&[0x23, 0x22, 0x12, 0x34, 0x00, 0x10]) else {
        panic!("23 22 ..")
    };
    assert_eq!(r.format, 0x22);
    assert_eq!(
        r.address_and_size,
        Mux::Case22(AddressAndSizeCase22 {
            address: 0x1234,
            size: 0x0010
        })
    );
    // Format 12: two bytes of address, one of size.
    let AnyRequest::MemoryRead(r) = request(&[0x23, 0x12, 0x12, 0x34, 0x10]) else {
        panic!("23 12 ..")
    };
    assert_eq!(
        r.address_and_size,
        Mux::Case12(AddressAndSizeCase12 {
            address: 0x1234,
            size: 0x10
        })
    );
    // A format the multiplexer names no case for uses its default structure.
    let AnyRequest::MemoryRead(r) = request(&[0x23, 0x55, 0xAA, 0x04]) else {
        panic!("23 55 ..")
    };
    assert!(matches!(r.address_and_size, Mux::Default(d) if d.address == 0xAA && d.size == 4));
    // The bytes of another case do not fit this one.
    assert!(matches!(
        AnyRequest::decode(&[0x23, 0x22, 0x12, 0x34, 0x10]),
        Some(Err(_))
    ));

    let r = s::memory_read::Request {
        format: 0x22,
        address_and_size: Mux::Case22(AddressAndSizeCase22 {
            address: 0xBEEF,
            size: 2,
        }),
    };
    assert_eq!(bytes(&r), [0x23, 0x22, 0xBE, 0xEF, 0x00, 0x02]);
}

// ---- KWP2000 -------------------------------------------------------------------------------

#[test]
fn kwp_identification_with_bcd_numbers() {
    use mini_kwp::{service::ecu_identification_read as read, AnyRequest, AnyResponse};
    assert_eq!(mini_kwp::PROTOCOL, cdd_rt::Protocol::Kwp2000);
    assert_eq!(bytes(&read::Request), [0x1A, 0x90]);
    assert!(matches!(
        AnyRequest::decode(&[0x1A, 0x90]),
        Some(Ok(AnyRequest::EcuIdentificationRead(_)))
    ));
    assert!(
        AnyRequest::decode(&[0x1A, 0x91]).is_none(),
        "another local identifier is another service"
    );

    let r = read::Response {
        ident_number_high: 9877,
        ident_number_low: 5433,
        identification: 42,
    };
    let wire = [0x5A, 0x90, 0x98, 0x77, 0x54, 0x33, 0x00, 0x2A];
    assert_eq!(bytes(&r), wire);
    assert!(
        matches!(AnyResponse::decode(&wire), Some(Ok(AnyResponse::EcuIdentificationRead(back))) if back == r)
    );
}

#[test]
fn kwp_a_counted_list_whose_count_comes_first() {
    use mini_kwp::{
        service::fault_memory_read_all_identified as dtcs, types, AnyRequest, AnyResponse,
    };
    assert_eq!(
        bytes(&dtcs::Request {
            group_of_dtc: 0xFF00
        }),
        [0x18, 0x02, 0xFF, 0x00]
    );
    assert!(
        matches!(AnyRequest::decode(&[0x18, 0x02, 0xFF, 0x00]), Some(Ok(AnyRequest::FaultMemoryReadAllIdentified(r))) if r.group_of_dtc == 0xFF00)
    );

    let status = |confirmed, not_completed| dtcs::DtcStatusByte {
        bits: dtcs::Bits {
            confirmed: types::Flag1bit::from_raw(confirmed),
            not_completed: types::Flag1bit::from_raw(not_completed),
        },
    };
    let items = [
        dtcs::ListOfDtcElement {
            dtc: 0x0123,
            dtc_status_byte: status(1, 0),
        },
        dtcs::ListOfDtcElement {
            dtc: 0x0456,
            dtc_status_byte: status(1, 1),
        },
    ];
    // The count is a field like the others, and has to agree with the list that is written.
    let wire = [0x58, 0x02, 0x01, 0x23, 0x01, 0x04, 0x56, 0x03];
    assert_eq!(
        bytes(&dtcs::Response {
            number_of_dtc: [2],
            list_of_dtc: List::Items(&items)
        }),
        wire
    );
    let mut out = [0u8; 32];
    assert!(
        dtcs::Response {
            number_of_dtc: [3],
            list_of_dtc: List::Items(&items)
        }
        .encode(&mut out)
        .is_err(),
        "a count the list does not have"
    );

    let Some(Ok(AnyResponse::FaultMemoryReadAllIdentified(r))) = AnyResponse::decode(&wire) else {
        panic!("not read back")
    };
    assert_eq!(
        r.list_of_dtc.iter().collect::<Result<Vec<_>, _>>().unwrap(),
        items
    );

    // A count that does not match what follows is not a message.
    assert!(matches!(
        AnyResponse::decode(&[0x58, 0x03, 0x01, 0x23, 0x01, 0x04, 0x56, 0x03]),
        Some(Err(_))
    ));
    assert!(matches!(
        AnyResponse::decode(&[0x58, 0x01, 0x01, 0x23, 0x01, 0x04, 0x56, 0x03]),
        Some(Err(_))
    ));
    assert!(matches!(AnyResponse::decode(&[0x58, 0x00]), Some(Ok(_))));
}

#[test]
fn the_documents_say_where_the_code_came_from() {
    assert_eq!(
        (uds::DOCUMENT, uds::ECU, uds::VARIANT),
        ("mini-uds.cdd", "Mini", "Base")
    );
    assert_eq!(uds::PROTOCOL, cdd_rt::Protocol::Uds);
    assert_eq!(uds::SOURCE_SHA256.len(), 64);
    assert!(uds::SERVICES
        .iter()
        .any(|s| s.name == "Memory_Read" && s.sid == Some(0x23) && s.nrcs == [0x13, 0x31, 0x33]));
    assert_eq!(
        uds::nrc::name(0x78),
        Some("Request correctly received, response pending")
    );
}
