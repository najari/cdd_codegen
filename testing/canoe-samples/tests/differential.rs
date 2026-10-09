//! The generated code against the interpreter, over every service of every sample of CANoe's
//! (see `harness` for what is checked).

mod common;

use harness::{check_subject, subject, Subject};

fn subjects() -> Vec<Subject> {
    vec![
        subject!(
            canoe_samples,
            can_system,
            r"CANSystemDemo\CDD\CANSystem.cdd",
            None
        ),
        subject!(
            canoe_samples,
            can_system_door,
            r"CANSystemDemo\CDD\CANSystemDoor.cdd",
            None
        ),
        subject!(
            canoe_samples,
            kwp_seat,
            r"TestFeatureSet\SeatTest\DBs\kwp2000-seat.cdd",
            None
        ),
        subject!(
            canoe_samples,
            sample_uds,
            r"TestFeatureSet\CentralLockingSystem\DBs\SampleUDS.cdd",
            None
        ),
        subject!(
            canoe_samples,
            uds_basic,
            r"Diagnostics\UDSBasic\Cdd\UDS-ExampleEcu-5.0.4.cdd",
            None
        ),
        subject!(
            canoe_samples,
            uds_base,
            r"Diagnostics\UDSSystem\CDD\UDS-ExampleEcu-5.1.0.cdd",
            Some("CommonDiagnostics")
        ),
        subject!(
            canoe_samples,
            uds_dev,
            r"Diagnostics\UDSSystem\CDD\UDS-ExampleEcu-5.1.0.cdd",
            Some("DevSample")
        ),
    ]
}

#[test]
fn generated_code_agrees_with_the_interpreter_on_every_sample() {
    let root = require_samples!();
    for s in subjects() {
        let stats = check_subject(&root, &s);
        eprintln!("{}: {} payloads, {} variations, {} messages not generated: generated code and interpreter agree", s.module, stats.payloads, stats.mutated, stats.not_generated);
    }
}
