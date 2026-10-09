//! A tester talking to a simulated ECU in one process, both made from the CDD: the "simulation
//! mode" of CANoe. KWP2000 with the CANSystem demo's description, UDS with the example ECU's.

mod common;

use canoe_samples::{can_system as kwp, uds_base as uds};
use cdd_rt::sim::{Direction, Loopback, LoopbackError};
use cdd_rt::tester::{CallError, Tester, Transport};
use cdd_rt::Reply;
use std::collections::VecDeque;

const RX: usize = 4095;

// ---- KWP2000 -------------------------------------------------------------------------------

#[derive(Default)]
struct KwpEcu {
    /// What the ECU was asked, in order.
    seen: Vec<&'static str>,
}

impl kwp::server::Handler for KwpEcu {
    fn on_ecu_identification_read(
        &mut self,
        _request: &kwp::service::ecu_identification_read::Request,
    ) -> Reply<kwp::service::ecu_identification_read::Response> {
        self.seen.push("ecu identification");
        // The values CANoe's demo ECU answers with in ComfortDiagData.asc.
        Reply::Positive(kwp::service::ecu_identification_read::Response {
            ident_number_7_6: 9877,
            ident_number_5_4: 5433,
            ident_number_3_2: 2000,
            ident_number_1_0: 8888,
            diagnostic_identification: 2,
        })
    }

    fn on_tester_present_send_no_response(
        &mut self,
        _request: &kwp::service::tester_present_send_no_response::Request,
    ) {
        self.seen.push("tester present, no response");
    }
}

fn kwp_tester() -> Tester<Loopback<kwp::server::Server<KwpEcu>>> {
    Tester::new(Loopback::new(kwp::server::Server::new(KwpEcu::default())))
}

#[test]
fn a_kwp_tester_reads_the_identification_of_a_simulated_ecu() {
    let mut tester = kwp_tester();
    let mut rx = [0u8; RX];
    let r = tester
        .call(&kwp::service::ecu_identification_read::Request, &mut rx)
        .unwrap()
        .expect_positive();
    assert_eq!(
        (
            r.ident_number_7_6,
            r.ident_number_5_4,
            r.ident_number_3_2,
            r.ident_number_1_0
        ),
        (9877, 5433, 2000, 8888)
    );

    // The conversation is the one in CANoe's own log: `1A 90`, then 12 bytes.
    let traffic = &tester.transport.traffic;
    assert_eq!(traffic[0], (Direction::TesterToEcu, vec![0x1A, 0x90]));
    assert_eq!(
        traffic[1],
        (
            Direction::EcuToTester,
            vec![0x5A, 0x90, 0x98, 0x77, 0x54, 0x33, 0x20, 0x00, 0x88, 0x88, 0x00, 0x02]
        )
    );
    assert_eq!(tester.transport.ecu.handler.seen, ["ecu identification"]);
}

#[test]
fn a_service_the_ecu_does_not_implement_is_refused_with_the_default_code() {
    let mut tester = kwp_tester();
    let mut rx = [0u8; RX];
    // "Subfunction not supported", what CANoe's example ECU answers for the rest.
    tester
        .call(&kwp::service::coding_read::Request, &mut rx)
        .unwrap()
        .expect_nrc(0x12);
    assert_eq!(tester.transport.traffic[1].1, vec![0x7F, 0x21, 0x12]);
}

#[test]
fn a_request_no_service_matches_is_a_service_not_supported() {
    let mut tester = kwp_tester();
    tester.transport.send(&[0x10, 0x00]).unwrap();
    let mut rx = [0u8; RX];
    let n = tester.transport.recv(&mut rx).unwrap();
    assert_eq!(&rx[..n], &[0x7F, 0x10, 0x11]);
    // A request that starts like a service but does not decode is "incorrect length".
    tester.transport.send(&[0x1A, 0x90, 0x00]).unwrap();
    let n = tester.transport.recv(&mut rx).unwrap();
    assert_eq!(&rx[..n], &[0x7F, 0x1A, 0x13]);
}

#[test]
fn a_service_without_a_response_is_sent_and_nothing_comes_back() {
    let mut tester = kwp_tester();
    tester
        .send(&kwp::service::tester_present_send_no_response::Request)
        .unwrap();
    assert_eq!(
        tester.transport.ecu.handler.seen,
        ["tester present, no response"]
    );
    let mut rx = [0u8; RX];
    assert_eq!(
        tester.transport.recv(&mut rx),
        Err(LoopbackError::NoResponse)
    );
}

// ---- UDS -----------------------------------------------------------------------------------

#[derive(Default)]
struct UdsEcu {
    vin: [u8; 17],
}

impl uds::server::Handler for UdsEcu {
    fn on_tester_present_send(
        &mut self,
        _request: &uds::service::tester_present_send::Request,
    ) -> Reply<uds::service::tester_present_send::Response> {
        Reply::Positive(uds::service::tester_present_send::Response)
    }

    fn on_vehicle_identification_read(
        &mut self,
        _request: &uds::service::vehicle_identification_read::Request,
    ) -> Reply<uds::service::vehicle_identification_read::Response> {
        Reply::Positive(uds::service::vehicle_identification_read::Response { vin: self.vin })
    }

    fn on_vehicle_identification_write(
        &mut self,
        request: &uds::service::vehicle_identification_write::Request,
    ) -> Reply<uds::service::vehicle_identification_write::Response> {
        if request.vin.iter().any(|b| !b.is_ascii_alphanumeric()) {
            // Request out of range.
            return Reply::nrc(0x2E, 0x31);
        }
        self.vin = request.vin;
        Reply::Positive(uds::service::vehicle_identification_write::Response)
    }
}

fn uds_tester() -> Tester<Loopback<uds::server::Server<UdsEcu>>> {
    Tester::new(Loopback::new(uds::server::Server::new(UdsEcu {
        vin: *b"VF1ABCDEFGH123456",
    })))
}

#[test]
fn a_uds_tester_writes_and_reads_back_a_vin() {
    let mut tester = uds_tester();
    let mut rx = [0u8; RX];

    let new_vin = *b"WVWZZZ1JZXW000001";
    tester
        .call(
            &uds::service::vehicle_identification_write::Request { vin: new_vin },
            &mut rx,
        )
        .unwrap()
        .expect_positive();
    let read = tester
        .call(&uds::service::vehicle_identification_read::Request, &mut rx)
        .unwrap()
        .expect_positive();
    assert_eq!(read.vin, new_vin);

    let wire: Vec<&[u8]> = tester
        .transport
        .traffic
        .iter()
        .map(|(_, p)| p.as_slice())
        .collect();
    assert_eq!(wire[0], [&[0x2E, 0xF1, 0x90][..], &new_vin].concat());
    assert_eq!(wire[1], [0x6E, 0xF1, 0x90]);
    assert_eq!(wire[2], [0x22, 0xF1, 0x90]);
    assert_eq!(wire[3], [&[0x62, 0xF1, 0x90][..], &new_vin].concat());
}

#[test]
fn the_ecu_can_refuse_a_value_with_a_code_of_the_description() {
    let mut tester = uds_tester();
    let mut rx = [0u8; RX];
    let reply = tester
        .call(
            &uds::service::vehicle_identification_write::Request {
                vin: *b"not a valid vin!!",
            },
            &mut rx,
        )
        .unwrap();
    assert_eq!(reply.response_code(), Some(0x31));
    // The description names the code.
    assert_eq!(uds::nrc::name(0x31), Some("Request out of range"));
    assert_eq!(uds::nrc::REQUEST_OUT_OF_RANGE, 0x31);
}

#[test]
fn the_suppress_positive_response_bit_silences_a_positive_answer() {
    let mut tester = uds_tester();
    let mut rx = [0u8; RX];

    tester
        .send(&uds::service::tester_present_send::Request {
            suppress_positive_response: true,
        })
        .unwrap();
    assert_eq!(
        tester.transport.traffic.last().unwrap(),
        &(Direction::TesterToEcu, vec![0x3E, 0x80])
    );
    assert_eq!(
        tester.transport.recv(&mut rx),
        Err(LoopbackError::NoResponse),
        "a suppressed answer is not sent"
    );

    let reply = tester
        .call(
            &uds::service::tester_present_send::Request {
                suppress_positive_response: false,
            },
            &mut rx,
        )
        .unwrap();
    assert!(reply.is_positive());
    let sent: Vec<_> = tester
        .transport
        .traffic
        .iter()
        .map(|(d, p)| (*d, p.clone()))
        .collect();
    assert_eq!(sent[1], (Direction::TesterToEcu, vec![0x3E, 0x00]));
    assert_eq!(sent[2], (Direction::EcuToTester, vec![0x7E, 0x00]));
}

// ---- "response pending" --------------------------------------------------------------------

/// A transport that answers from a script.
struct Scripted {
    answers: VecDeque<Vec<u8>>,
    sent: Vec<Vec<u8>>,
}

impl Transport for Scripted {
    type Error = &'static str;

    fn send(&mut self, payload: &[u8]) -> Result<(), Self::Error> {
        self.sent.push(payload.to_vec());
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        let a = self
            .answers
            .pop_front()
            .ok_or("the script has no more answers")?;
        buf[..a.len()].copy_from_slice(&a);
        Ok(a.len())
    }
}

#[test]
fn a_tester_waits_out_response_pending_and_then_gives_up() {
    let vin = *b"WVWZZZ1JZXW000001";
    let positive = [&[0x62, 0xF1, 0x90][..], &vin].concat();
    let script = |n: usize| {
        let mut answers: VecDeque<Vec<u8>> =
            std::iter::repeat_n(vec![0x7F, 0x22, 0x78], n).collect();
        answers.push_back(positive.clone());
        Scripted {
            answers,
            sent: Vec::new(),
        }
    };
    let mut rx = [0u8; RX];

    let mut tester = Tester::new(script(3));
    let r = tester
        .call(&uds::service::vehicle_identification_read::Request, &mut rx)
        .unwrap()
        .expect_positive();
    assert_eq!(r.vin, vin);
    assert_eq!(
        tester.transport.sent,
        vec![vec![0x22, 0xF1, 0x90]],
        "the request is sent once"
    );

    let mut tester = Tester::new(script(5));
    tester.max_pending = 2;
    let err = tester
        .call(&uds::service::vehicle_identification_read::Request, &mut rx)
        .unwrap_err();
    assert_eq!(err, CallError::TooManyPending);
}

#[test]
fn a_garbled_answer_is_an_error_and_a_foreign_refusal_is_not_taken_for_ours() {
    let mut rx = [0u8; RX];
    let mut tester = Tester::new(Scripted {
        answers: VecDeque::from([vec![0x62, 0xF1, 0x90, 0x41]]),
        sent: Vec::new(),
    });
    let err = tester
        .call(&uds::service::vehicle_identification_read::Request, &mut rx)
        .unwrap_err();
    assert!(
        matches!(
            err,
            CallError::Decode(cdd_rt::DecodeError::Truncated { .. })
        ),
        "{err:?}"
    );

    let mut tester = Tester::new(Scripted {
        answers: VecDeque::from([vec![0x7F, 0x2E, 0x31]]),
        sent: Vec::new(),
    });
    let err = tester
        .call(&uds::service::vehicle_identification_read::Request, &mut rx)
        .unwrap_err();
    assert_eq!(
        err,
        CallError::Unexpected {
            first_byte: Some(0x2E)
        }
    );
}
