//! The diagnostic logs of CANoe's CANSystem demo (KWP2000 on CAN), read with the code generated
//! from the demo's own `CANSystem.cdd`.

mod common;

use canoe_samples::can_system as sys;
use cdd_log::isotp::{self, Event};
use cdd_rt::{Decode, NegativeResponse};

/// What the generated code makes of a payload one side sent.
fn name_of(request: bool, payload: &[u8]) -> String {
    if let Some(n) = NegativeResponse::parse(payload) {
        return format!("negative response {:#04x} to {:#04x}", n.nrc, n.sid);
    }
    let found = if request {
        sys::AnyRequest::decode(payload).map(|r| r.map(|m| m.name()))
    } else {
        sys::AnyResponse::decode(payload).map(|r| r.map(|m| m.name()))
    };
    match found {
        Some(Ok(name)) => name.to_string(),
        Some(Err(e)) => format!("does not decode: {e}"),
        None => "not in the description".to_string(),
    }
}

/// The conversation of a log: `(id, name)` for every reassembled message, in order.
fn conversation(root: &std::path::Path, file: &str, tester_id: u32) -> Vec<(u32, String)> {
    let frames = cdd_log::read_frames(root.join(file)).expect("the log reads");
    isotp::reassemble(&frames)
        .into_iter()
        .map(|e| match e {
            Event::Message(m) => (m.id, name_of(m.id == tester_id, &m.payload)),
            Event::Error(f) => panic!("the sample log should have no ISO-TP faults: {f:?}"),
        })
        .collect()
}

fn s(id: u32, name: &str) -> (u32, String) {
    (id, name.to_string())
}

#[test]
fn the_comfort_log_is_understood_message_by_message() {
    let root = require_samples!();
    let got = conversation(&root, r"CANSystemDemo\Logging\ComfortDiagData.asc", 0x700);
    // The tester asks on 0x700, the comfort ECU answers on 0x600. `10 00`, `11 00` and their
    // answers are not services of CANSystem.cdd, whose sessions are `10 81` and `10 85`.
    let expected = vec![
        s(0x700, "not in the description"),
        s(0x600, "not in the description"),
        s(0x700, "Tester_Present_Send_Response"),
        s(0x600, "Tester_Present_Send_Response"),
        s(0x700, "not in the description"),
        s(0x600, "not in the description"),
        s(0x700, "Tester_Present_Send_Response"),
        s(0x600, "Tester_Present_Send_Response"),
        s(0x700, "FAULT_MEMORY_DeleteAll"),
        s(0x600, "FAULT_MEMORY_DeleteAll"),
        s(0x700, "ECU_Identification_Read"),
        s(0x600, "ECU_Identification_Read"),
        s(0x700, "Tester_Present_Send_Response"),
        s(0x600, "Tester_Present_Send_Response"),
        s(0x700, "STOP_SESSION_Stop"),
        s(0x600, "STOP_SESSION_Stop"),
        s(0x700, "Tester_Present_Send_Response"),
        s(0x600, "Tester_Present_Send_Response"),
    ];
    assert_eq!(got, expected);
}

#[test]
fn the_engine_log_has_a_dtc_answer_with_two_entries() {
    let root = require_samples!();
    let frames =
        cdd_log::read_frames(root.join(r"CANSystemDemo\Logging\EngineDiagData.asc")).unwrap();
    let messages: Vec<isotp::Message> = isotp::reassemble(&frames)
        .into_iter()
        .filter_map(|e| {
            if let Event::Message(m) = e {
                Some(m)
            } else {
                None
            }
        })
        .collect();

    // The engine ECU answers `18 02 FF 00` (read all identified trouble codes) on 0x400.
    let reply = messages
        .iter()
        .find(|m| m.id == 0x400 && m.payload.first() == Some(&0x58) && m.payload.len() > 2)
        .expect("a DTC answer in the log");
    let r = sys::service::fault_memory_read_all_identified_trouble_codes::Response::decode(
        &reply.payload,
    )
    .expect("it decodes");
    assert_eq!(r.number_of_dtc, [2]);
    assert_eq!(r.list_of_dtc_and_status.len(), 2);
    let entries: Vec<_> = r
        .list_of_dtc_and_status
        .iter()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        (entries[0].dtc.raw(), entries[0].dtc.label()),
        (0x9002, Some("Voltage too high"))
    );
    assert_eq!(
        (entries[1].dtc.raw(), entries[1].dtc.label()),
        (0x9012, Some("overtemperature"))
    );
    assert_eq!(
        entries[0].dtc_status_byte.confirmed_dtc.label(),
        Some("true")
    );
    assert_eq!(
        entries[1]
            .dtc_status_byte
            .test_not_completed_this_monitoring_cycle
            .label(),
        Some("true")
    );

    // Writing it back gives the payload that was in the log.
    let mut out = [0u8; 64];
    let n = cdd_rt::Encode::encode(&r, &mut out).unwrap();
    assert_eq!(&out[..n], reply.payload.as_slice());
}

#[test]
fn the_ecu_identification_is_four_bcd_numbers() {
    let root = require_samples!();
    let frames =
        cdd_log::read_frames(root.join(r"CANSystemDemo\Logging\ComfortDiagData.asc")).unwrap();
    let reply = isotp::reassemble(&frames)
        .into_iter()
        .find_map(|e| match e {
            Event::Message(m) if m.id == 0x600 && m.payload.first() == Some(&0x5A) => Some(m),
            _ => None,
        })
        .expect("an identification answer");
    let r = sys::service::ecu_identification_read::Response::decode(&reply.payload).unwrap();
    assert_eq!(
        (
            r.ident_number_7_6,
            r.ident_number_5_4,
            r.ident_number_3_2,
            r.ident_number_1_0
        ),
        (9877, 5433, 2000, 8888)
    );
    assert_eq!(r.diagnostic_identification, 2);
}
