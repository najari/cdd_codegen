//! Generates code for the CDD files of CANoe's sample configurations into `OUT_DIR`.
//!
//! The samples are Vector's and are not part of this repository. Where they are comes from the
//! environment variable `CANOE_SAMPLES` (the folder `Sample Configurations .../CAN`), else the
//! default install location. When they are not there every module is empty and the tests skip.

use cdd_codegen::{Config, Role};
use std::path::{Path, PathBuf};

const DEFAULT_ROOT: &str =
    r"C:\Users\Public\Documents\Vector\CANoe\Sample Configurations 13.0.172\CAN";

struct Sample {
    module: &'static str,
    file: &'static str,
    variant: Option<&'static str>,
}

const SAMPLES: &[Sample] = &[
    Sample {
        module: "can_system",
        file: r"CANSystemDemo\CDD\CANSystem.cdd",
        variant: None,
    },
    Sample {
        module: "can_system_door",
        file: r"CANSystemDemo\CDD\CANSystemDoor.cdd",
        variant: None,
    },
    Sample {
        module: "kwp_seat",
        file: r"TestFeatureSet\SeatTest\DBs\kwp2000-seat.cdd",
        variant: None,
    },
    Sample {
        module: "sample_uds",
        file: r"TestFeatureSet\CentralLockingSystem\DBs\SampleUDS.cdd",
        variant: None,
    },
    Sample {
        module: "uds_basic",
        file: r"Diagnostics\UDSBasic\Cdd\UDS-ExampleEcu-5.0.4.cdd",
        variant: None,
    },
    Sample {
        module: "uds_base",
        file: r"Diagnostics\UDSSystem\CDD\UDS-ExampleEcu-5.1.0.cdd",
        variant: Some("CommonDiagnostics"),
    },
    Sample {
        module: "uds_dev",
        file: r"Diagnostics\UDSSystem\CDD\UDS-ExampleEcu-5.1.0.cdd",
        variant: Some("DevSample"),
    },
];

fn main() {
    println!("cargo:rerun-if-env-changed=CANOE_SAMPLES");
    println!("cargo:rerun-if-changed=build.rs");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let root = std::env::var_os("CANOE_SAMPLES")
        .map_or_else(|| PathBuf::from(DEFAULT_ROOT), PathBuf::from);
    println!("cargo:rustc-check-cfg=cfg(have_samples)");
    let mut modules = String::new();
    let mut generated = 0;
    for s in SAMPLES {
        let path = root.join(s.file);
        let code_file = out.join(format!("{}.rs", s.module));
        println!("cargo:rerun-if-changed={}", path.display());
        match generate(&path, s) {
            Some(code) => {
                std::fs::write(&code_file, code).expect("write generated code");
                modules.push_str(&format!(
                    "pub mod {} {{\n    include!(concat!(env!(\"OUT_DIR\"), \"/{}.rs\"));\n}}\n",
                    s.module, s.module
                ));
                generated += 1;
            }
            None => println!(
                "cargo:warning=sample `{}` not generated: {} is missing or not accepted",
                s.module,
                path.display()
            ),
        }
    }
    if generated > 0 {
        println!("cargo:rustc-cfg=have_samples");
    }
    std::fs::write(out.join("modules.rs"), modules).expect("write module list");
}

fn generate(path: &Path, s: &Sample) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let config = Config {
        variant: s.variant.map(str::to_string),
        role: Role::Both,
        ..Config::default()
    };
    let name = path.file_name()?.to_string_lossy().into_owned();
    match config.generate(&name, &bytes) {
        Ok(a) => Some(a.code),
        Err(e) => panic!("generating `{}` failed: {e}", s.module),
    }
}
