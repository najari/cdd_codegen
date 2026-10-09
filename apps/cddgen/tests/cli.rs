//! The command line, run as a user would, on the synthetic CDD files of `testing/synthetic`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testing/synthetic/fixtures")
        .join(name)
}

fn cddgen(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cddgen"))
        .args(args)
        .output()
        .expect("cddgen runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// An empty directory of its own.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cddgen-cli-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch directory");
    dir
}

#[test]
fn generate_writes_the_code_and_check_notices_a_change() {
    let out = scratch("generate");
    let cdd = fixture("mini-uds.cdd");
    let run = cddgen(&["generate", cdd.to_str().unwrap(), out.to_str().unwrap()]);
    assert!(run.status.success(), "{}", text(&run.stderr));
    assert!(
        text(&run.stdout).contains("16 of 16 services"),
        "{}",
        text(&run.stdout)
    );

    let code = std::fs::read_to_string(out.join("diag.rs")).expect("diag.rs is written");
    assert!(code.contains("pub mod default_session_start"));
    assert!(code.contains("pub mod server"), "both roles by default");
    let manifest =
        std::fs::read_to_string(out.join("manifest.json")).expect("manifest.json is written");
    assert!(manifest.contains("mini-uds.cdd"));

    let check = cddgen(&[
        "generate",
        cdd.to_str().unwrap(),
        out.to_str().unwrap(),
        "--check",
    ]);
    assert!(
        check.status.success(),
        "the output is up to date: {}",
        text(&check.stderr)
    );

    std::fs::write(out.join("diag.rs"), format!("{code}// edited by hand\n")).unwrap();
    let check = cddgen(&[
        "generate",
        cdd.to_str().unwrap(),
        out.to_str().unwrap(),
        "--check",
    ]);
    assert!(!check.status.success(), "an edited file is not up to date");
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn the_role_decides_what_is_generated() {
    let cdd = fixture("mini-uds.cdd");
    let generated = |role: &str| {
        let out = scratch(role);
        let run = cddgen(&[
            "generate",
            cdd.to_str().unwrap(),
            out.to_str().unwrap(),
            "--role",
            role,
        ]);
        assert!(run.status.success(), "{role}: {}", text(&run.stderr));
        let code = std::fs::read_to_string(out.join("diag.rs")).unwrap();
        let _ = std::fs::remove_dir_all(out);
        code
    };
    let tester = generated("tester");
    assert!(
        !tester.contains("pub mod server"),
        "a tester has no simulated ECU"
    );
    assert!(
        tester.contains("::cdd_rt::Request for"),
        "a tester can send requests"
    );
    let ecu = generated("ecu");
    assert!(ecu.contains("pub mod server"));
    assert!(
        !ecu.contains("::cdd_rt::Request for"),
        "a simulated ECU does not send requests"
    );
}

#[test]
fn decode_names_the_service_and_its_values() {
    let cdd = fixture("mini-uds.cdd");
    let run = cddgen(&["decode", cdd.to_str().unwrap(), "10", "81"]);
    assert!(run.status.success(), "{}", text(&run.stderr));
    let shown = text(&run.stdout);
    assert!(shown.contains("DefaultSession_Start"), "{shown}");
    assert!(shown.contains("positive response suppressed"), "{shown}");

    let run = cddgen(&[
        "decode",
        cdd.to_str().unwrap(),
        "--response",
        "62",
        "01",
        "00",
        "31",
        "38",
        "64",
        "01",
        "F3",
    ]);
    assert!(run.status.success(), "{}", text(&run.stderr));
    let shown = text(&run.stdout);
    assert!(shown.contains("Battery_Read"), "{shown}");
    assert!(shown.contains("12.6"), "the voltage in volts: {shown}");
}

#[test]
fn inspect_lists_the_services() {
    let cdd = fixture("mini-kwp.cdd");
    let run = cddgen(&["inspect", cdd.to_str().unwrap()]);
    assert!(run.status.success(), "{}", text(&run.stderr));
    let shown = text(&run.stdout);
    assert!(
        shown.contains("EcuIdentification_Read") && shown.contains("FaultMemory_ReadAllIdentified"),
        "{shown}"
    );

    let run = cddgen(&[
        "inspect",
        cdd.to_str().unwrap(),
        "--service",
        "EcuIdentification_Read",
    ]);
    assert!(run.status.success());
    assert!(
        text(&run.stdout).contains("SID 1A"),
        "{}",
        text(&run.stdout)
    );
}

#[test]
fn mistakes_are_reported_not_panicked() {
    let missing = cddgen(&["inspect", "no-such-file.cdd"]);
    assert!(!missing.status.success());
    assert!(
        text(&missing.stderr).contains("no-such-file.cdd"),
        "{}",
        text(&missing.stderr)
    );

    let cdd = fixture("mini-uds.cdd");
    let out = scratch("mistakes");
    let bad_variant = cddgen(&[
        "generate",
        cdd.to_str().unwrap(),
        out.to_str().unwrap(),
        "--variant",
        "NoSuchVariant",
    ]);
    assert!(!bad_variant.status.success());
    assert!(
        !text(&bad_variant.stderr).contains("panicked"),
        "{}",
        text(&bad_variant.stderr)
    );
    let _ = std::fs::remove_dir_all(out);

    let bad_hex = cddgen(&["decode", cdd.to_str().unwrap(), "1"]);
    assert!(!bad_hex.status.success());
    assert!(
        text(&bad_hex.stderr).contains("odd number of hex digits"),
        "{}",
        text(&bad_hex.stderr)
    );
}
