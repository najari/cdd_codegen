//! Generates code for the two CDD files in `fixtures/` (written by `fixtures/make_fixtures.py`).

use cdd_codegen::{Config, Role};
use std::path::PathBuf;

const FIXTURES: &[(&str, &str)] = &[("mini_uds", "mini-uds.cdd"), ("mini_kwp", "mini-kwp.cdd")];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let manifest = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"),
    );
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let mut modules = String::new();
    for (module, file) in FIXTURES {
        let path = manifest.join("fixtures").join(file);
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes =
            std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let config = Config {
            role: Role::Both,
            ..Config::default()
        };
        let artifacts = config
            .generate(file, &bytes)
            .unwrap_or_else(|e| panic!("generating `{module}` failed: {e}"));
        assert!(
            artifacts.report.skipped.is_empty(),
            "`{module}` must be generated whole: {:?}",
            artifacts.report.skipped
        );
        std::fs::write(out.join(format!("{module}.rs")), artifacts.code)
            .expect("write generated code");
        modules.push_str(&format!("pub mod {module} {{\n    include!(concat!(env!(\"OUT_DIR\"), \"/{module}.rs\"));\n}}\n"));
    }
    std::fs::write(out.join("modules.rs"), modules).expect("write module list");
}
