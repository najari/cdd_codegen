//! The code examples of the user manual (README.md), kept here so they keep compiling and running.
//! `diag` is what a user gets from `cddgen generate` and `mod diag;`.

use synthetic::mini_uds as diag;

// ---- "Test an ECU" -------------------------------------------------------------------------

mod tester_example {
    use super::diag;
    use cdd_rt::tester::{Tester, Transport};
    use cdd_rt::Reply;

    /// Your own channel: ISO-TP over a CAN adapter, DoIP, ...
    #[allow(dead_code)]
    struct MyBus;

    impl Transport for MyBus {
        type Error = std::io::Error;

        fn send(&mut self, _payload: &[u8]) -> Result<(), Self::Error> {
            Err(std::io::ErrorKind::Unsupported.into()) // send one whole diagnostic message, e.g. over ISO-TP
        }

        fn recv(&mut self, _buf: &mut [u8]) -> Result<usize, Self::Error> {
            Err(std::io::ErrorKind::Unsupported.into()) // wait for the next whole message and return its length
        }
    }

    #[allow(dead_code)]
    fn read_battery(bus: MyBus) -> Result<(), Box<dyn std::error::Error>> {
        let mut tester = Tester::new(bus);
        let mut rx = [0u8; 4095]; // the answer is read out of this buffer

        match tester.call(&diag::service::battery_read::Request, &mut rx)? {
            Reply::Positive(r) => {
                println!("voltage {:?} V", r.voltage.physical());
                println!("charging: {:?}", r.charging.label());
                println!("mode: {:?}", r.status.mode);
            }
            Reply::Negative(n) => {
                println!("refused with {:#04x} {:?}", n.nrc, diag::nrc::name(n.nrc));
            }
        }
        Ok(())
    }
}

// ---- "Simulate an ECU" ---------------------------------------------------------------------

mod ecu_example {
    use super::diag;
    use cdd_rt::sim::Loopback;
    use cdd_rt::tester::Tester;
    use cdd_rt::Reply;
    use diag::server::{Handler, Server};
    use diag::service::{battery_read, vin_write};

    #[derive(Default)]
    pub struct MyEcu {
        pub vin: [u8; 17],
    }

    impl Handler for MyEcu {
        fn on_battery_read(
            &mut self,
            _request: &battery_read::Request,
        ) -> Reply<battery_read::Response> {
            Reply::Positive(battery_read::Response {
                voltage: diag::types::Voltage2byte::from_physical(12.6).unwrap(),
                temperature: diag::types::Temperature1byte::from_physical(21.0).unwrap(),
                charging: diag::types::OffOn1byte::from_raw(0),
                status: battery_read::Status {
                    alarm: diag::types::Flag1bit::from_raw(0),
                    mode: diag::types::Mode3bit::Run,
                    delta: 0,
                },
            })
        }

        fn on_vin_write(&mut self, request: &vin_write::Request) -> Reply<vin_write::Response> {
            if request.vin.iter().any(|b| !b.is_ascii_alphanumeric()) {
                return Reply::nrc(0x2E, diag::nrc::REQUEST_OUT_OF_RANGE);
            }
            self.vin = request.vin;
            Reply::Positive(vin_write::Response)
        }
        // Every other service answers with Self::DEFAULT_NRC (0x12).
    }

    #[test]
    fn a_tester_and_an_ecu_in_one_process() {
        let mut tester = Tester::new(Loopback::new(Server::new(MyEcu::default())));
        let mut rx = [0u8; 4095];

        let battery = tester
            .call(&battery_read::Request, &mut rx)
            .unwrap()
            .expect_positive();
        assert_eq!(
            battery.voltage.physical().map(|v| (v * 10.0).round()),
            Some(126.0)
        );

        tester
            .call(
                &vin_write::Request {
                    vin: *b"WVWZZZ1JZXW000001",
                },
                &mut rx,
            )
            .unwrap()
            .expect_positive();
        assert_eq!(&tester.transport.ecu.handler.vin, b"WVWZZZ1JZXW000001");
        tester
            .call(
                &vin_write::Request {
                    vin: *b"not a valid vin!!",
                },
                &mut rx,
            )
            .unwrap()
            .expect_nrc(0x31);
        // Services the handler does not implement are refused.
        tester
            .call(&diag::service::blob_read::Request, &mut rx)
            .unwrap()
            .expect_nrc(0x12);
    }
}

// ---- "Read captured messages" --------------------------------------------------------------

mod decode_example {
    use super::diag;
    use cdd_rt::{FieldValue, Path, RawValue, Visitor};

    /// Prints every value of a message: its path, raw value, name or physical value.
    struct Print(Vec<String>);

    impl Visitor for Print {
        fn field(&mut self, path: &Path<'_>, v: &FieldValue<'_>) {
            let raw = match v.raw {
                RawValue::Uint(x) => x.to_string(),
                RawValue::Int(x) => x.to_string(),
                RawValue::Float(x) => x.to_string(),
                RawValue::Bytes(b) => format!("{b:02X?}"),
            };
            let meaning = match (v.text, v.physical) {
                (Some(text), _) => format!(" '{text}'"),
                (None, Some(p)) => format!(" = {p} {}", v.unit.unwrap_or("")),
                (None, None) => String::new(),
            };
            self.0.push(format!("{path} = {raw}{meaning}"));
        }
    }

    #[test]
    fn what_a_captured_message_says() {
        let payload = [0x62, 0x01, 0x00, 0x31, 0x38, 0x64, 0x01, 0xF3];
        let message = diag::AnyResponse::decode(&payload)
            .expect("a response of the description")
            .expect("well formed");
        assert_eq!(message.name(), "Battery_Read");

        let mut print = Print(Vec::new());
        message.describe(&mut print);
        assert!(
            print.0.contains(&"Voltage = 12600 = 12.6 V".to_string()),
            "{:?}",
            print.0
        );
        assert!(
            print.0.contains(&"Status.Mode = 1 'run'".to_string()),
            "{:?}",
            print.0
        );
    }
}

// ---- "Generate from build.rs" -------------------------------------------------------------

#[allow(dead_code)]
fn build_rs() {
    use cdd_codegen::{Config, Role};

    let bytes = std::fs::read("diag/ecu.cdd").unwrap();
    let config = Config {
        role: Role::Tester,
        variant: Some("Base".into()),
        ..Config::default()
    };
    let artifacts = config.generate("ecu.cdd", &bytes).unwrap();
    for skipped in &artifacts.report.skipped {
        println!(
            "cargo:warning=left out {} {}: {}",
            skipped.service, skipped.message, skipped.reason
        );
    }
    std::fs::write("out/diag.rs", artifacts.code).unwrap();
}
