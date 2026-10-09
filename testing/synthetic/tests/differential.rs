//! The generated code against the interpreter over every service of the two synthetic CDD files.

use harness::{check_subject, subject, Subject};
use std::path::Path;

#[test]
fn generated_code_agrees_with_the_interpreter() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let subjects: [Subject; 2] = [
        subject!(synthetic, mini_uds, "mini-uds.cdd", None),
        subject!(synthetic, mini_kwp, "mini-kwp.cdd", None),
    ];
    for s in &subjects {
        let stats = check_subject(&fixtures, s);
        assert_eq!(
            stats.not_generated, 0,
            "{}: every message of the fixtures is generated",
            s.module
        );
        assert!(
            stats.payloads >= 90 && stats.mutated >= 1000,
            "{}: {} payloads, {} variations",
            s.module,
            stats.payloads,
            stats.mutated
        );
    }
}
