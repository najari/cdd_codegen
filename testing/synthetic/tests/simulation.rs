//! A tester talking to a simulated ECU, both made from the synthetic description: a handler that
//! owns state, answers with borrowed data, reads a multiplexed request and refuses what it should.

use cdd_rt::sim::{Direction, Loopback, LoopbackError};
use cdd_rt::tester::{Tester, Transport};
use cdd_rt::{List, Reply};
use synthetic::mini_uds::service::{
    blob_read, default_session_start as session, extended_session_start,
    fault_memory_read_by_status_mask as dtcs, memory_read, tester_present_send, vin_write,
};
use synthetic::mini_uds::{self as uds, server::Handler, server::Server};

const RX: usize = 4095;

struct Ecu {
    memory: Vec<u8>,
    dtcs: Vec<dtcs::ListOfDtcElement>,
    keep_alive: u32,
}

impl Handler for Ecu {
    fn on_default_session_start(&mut self, _: &session::Request) -> Reply<session::Response> {
        Reply::Positive(session::Response {
            p2: 50,
            p2_star: 5000,
        })
    }

    fn on_tester_present_send(
        &mut self,
        _: &tester_present_send::Request,
    ) -> Reply<tester_present_send::Response> {
        self.keep_alive += 1;
        Reply::Positive(tester_present_send::Response)
    }

    fn on_fault_memory_read_by_status_mask(
        &mut self,
        request: &dtcs::Request,
    ) -> Reply<dtcs::Response<'_>> {
        // The answer borrows the ECU's own list.
        let wanted: Vec<_> = self
            .dtcs
            .iter()
            .filter(|d| d.status_of_dtc & request.status_mask != 0)
            .copied()
            .collect();
        if wanted.len() == self.dtcs.len() {
            Reply::Positive(dtcs::Response {
                availability_mask: 0xFF,
                list_of_dtc: List::Items(&self.dtcs),
            })
        } else {
            self.dtcs = wanted;
            Reply::Positive(dtcs::Response {
                availability_mask: 0xFF,
                list_of_dtc: List::Items(&self.dtcs),
            })
        }
    }

    fn on_memory_read(
        &mut self,
        request: &memory_read::Request,
    ) -> Reply<memory_read::Response<'_>> {
        use memory_read::AddressAndSize as Layout;
        let (address, size) = match request.address_and_size {
            Layout::Case11(c) => (usize::from(c.address), usize::from(c.size)),
            Layout::Case12(c) => (usize::from(c.address), usize::from(c.size)),
            Layout::Case22(c) => (usize::from(c.address), usize::from(c.size)),
            // A format byte no case names: the ECU does not know it.
            Layout::Default(_) => return Reply::nrc(0x23, uds::nrc::REQUEST_OUT_OF_RANGE),
        };
        match self.memory.get(address..address + size) {
            Some(bytes) if size <= 16 => Reply::Positive(memory_read::Response { memory: bytes }),
            _ => Reply::nrc(0x23, uds::nrc::REQUEST_OUT_OF_RANGE),
        }
    }
}

fn tester() -> Tester<Loopback<Server<Ecu>>> {
    let ecu = Ecu {
        memory: (0..=255).collect(),
        dtcs: vec![
            dtcs::ListOfDtcElement {
                dtc: 0x0A0B0C,
                status_of_dtc: 0x01,
            },
            dtcs::ListOfDtcElement {
                dtc: 0x112233,
                status_of_dtc: 0x08,
            },
        ],
        keep_alive: 0,
    };
    Tester::new(Loopback::new(Server::new(ecu)))
}

#[test]
fn a_multiplexed_request_reaches_the_handler_in_the_layout_it_named() {
    use memory_read::{AddressAndSize as Layout, AddressAndSizeCase11, AddressAndSizeCase22};
    let mut tester = tester();
    let mut rx = [0u8; RX];

    let one_byte = memory_read::Request {
        format: 0x11,
        address_and_size: Layout::Case11(AddressAndSizeCase11 {
            address: 0x10,
            size: 3,
        }),
    };
    assert_eq!(
        tester
            .call(&one_byte, &mut rx)
            .unwrap()
            .expect_positive()
            .memory,
        [0x10, 0x11, 0x12]
    );

    let two_bytes = memory_read::Request {
        format: 0x22,
        address_and_size: Layout::Case22(AddressAndSizeCase22 {
            address: 0x0020,
            size: 0x0002,
        }),
    };
    assert_eq!(
        tester
            .call(&two_bytes, &mut rx)
            .unwrap()
            .expect_positive()
            .memory,
        [0x20, 0x21]
    );

    let wire: Vec<_> = tester
        .transport
        .traffic
        .iter()
        .map(|(d, p)| (*d, p.clone()))
        .collect();
    assert_eq!(
        wire[0],
        (Direction::TesterToEcu, vec![0x23, 0x11, 0x10, 0x03])
    );
    assert_eq!(
        wire[2],
        (
            Direction::TesterToEcu,
            vec![0x23, 0x22, 0x00, 0x20, 0x00, 0x02]
        )
    );

    // The handler refuses what lies outside the memory, and a size over what a response may hold.
    let outside = memory_read::Request {
        format: 0x11,
        address_and_size: Layout::Case11(AddressAndSizeCase11 {
            address: 0xFF,
            size: 2,
        }),
    };
    tester
        .call(&outside, &mut rx)
        .unwrap()
        .expect_nrc(uds::nrc::REQUEST_OUT_OF_RANGE);
    let too_big = memory_read::Request {
        format: 0x11,
        address_and_size: Layout::Case11(AddressAndSizeCase11 {
            address: 0,
            size: 17,
        }),
    };
    tester
        .call(&too_big, &mut rx)
        .unwrap()
        .expect_nrc(uds::nrc::REQUEST_OUT_OF_RANGE);
}

#[test]
fn an_answer_may_borrow_the_state_of_the_ecu() {
    let mut tester = tester();
    let mut rx = [0u8; RX];
    let reply = tester
        .call(
            &dtcs::Request {
                suppress_positive_response: false,
                status_mask: 0x08,
            },
            &mut rx,
        )
        .unwrap()
        .expect_positive();
    let found: Vec<_> = reply.list_of_dtc.iter().collect::<Result<_, _>>().unwrap();
    assert_eq!(
        found,
        [dtcs::ListOfDtcElement {
            dtc: 0x112233,
            status_of_dtc: 0x08
        }]
    );
    assert_eq!(
        tester.transport.traffic[1].1,
        [0x59, 0x02, 0xFF, 0x11, 0x22, 0x33, 0x08]
    );
}

#[test]
fn the_suppress_bit_silences_the_answer_but_not_the_handler() {
    let mut tester = tester();
    let mut rx = [0u8; RX];
    tester
        .send(&tester_present_send::Request {
            suppress_positive_response: true,
        })
        .unwrap();
    assert_eq!(
        tester.transport.recv(&mut rx),
        Err(LoopbackError::NoResponse)
    );
    tester
        .call(
            &tester_present_send::Request {
                suppress_positive_response: false,
            },
            &mut rx,
        )
        .unwrap()
        .expect_positive();
    assert_eq!(tester.transport.ecu.handler.keep_alive, 2);

    // A refusal is not a positive answer: the bit does not silence it.
    tester.transport.send(&[0x10, 0x82]).unwrap();
    let n = tester.transport.recv(&mut rx).unwrap();
    assert_eq!(
        &rx[..n],
        [0x7F, 0x10, 0x12],
        "the handler does not implement the programming session"
    );
}

#[test]
fn services_the_handler_leaves_alone_answer_with_the_default_code() {
    let mut tester = tester();
    let mut rx = [0u8; RX];
    tester
        .call(&blob_read::Request, &mut rx)
        .unwrap()
        .expect_nrc(0x12);
    tester
        .call(
            &extended_session_start::Request {
                suppress_positive_response: false,
            },
            &mut rx,
        )
        .unwrap()
        .expect_nrc(0x12);
    tester
        .call(
            &vin_write::Request {
                vin: *b"WVWZZZ1JZXW000001",
            },
            &mut rx,
        )
        .unwrap()
        .expect_nrc(0x12);
    // Not a service of the description at all.
    tester.transport.send(&[0x85, 0x01]).unwrap();
    let n = tester.transport.recv(&mut rx).unwrap();
    assert_eq!(&rx[..n], [0x7F, 0x85, 0x11]);
    // The right service with a message that does not fit it.
    tester.transport.send(&[0x23, 0x22, 0x00, 0x20]).unwrap();
    let n = tester.transport.recv(&mut rx).unwrap();
    assert_eq!(&rx[..n], [0x7F, 0x23, 0x13]);
}
