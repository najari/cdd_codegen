//! `cargo run -p cdd-model --example identify -- file.cdd REQ|POS "1A 90"`
//! Shows which services of the default variant accept the payload, with every decoded value.

use cdd_model::interp::{Interp, Raw};
use cdd_model::ir::MsgKind;
use cdd_model::Cdd;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [path, kind, hex] = args.as_slice() else {
        eprintln!("usage: identify FILE.cdd REQ|POS HEX");
        std::process::exit(2);
    };
    let kind = match kind.as_str() {
        "REQ" => MsgKind::Request,
        "POS" => MsgKind::Positive,
        _ => MsgKind::Negative,
    };
    let payload: Vec<u8> = hex
        .split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).expect("hex byte"))
        .collect();
    let cdd = Cdd::load(path).expect("load");
    let ecu = &cdd.ecus[0];
    let var = ecu.default_variant().expect("a variant");
    let interp = Interp::new(&cdd).with_languages(vec!["en-US".into()]);
    let hits = interp.identify(&var.services, kind, &payload);
    println!("{} candidate(s) in {}/{}", hits.len(), ecu.qual, var.qual);
    for (svc, decoded) in hits {
        println!("-- {} ({})", svc.shortcut, svc.key);
        for i in &decoded.items {
            let raw = match &i.raw {
                Raw::Int(v) => format!("{v} (0x{v:X})"),
                Raw::Float(v) => format!("{v}"),
                Raw::Bytes(b) => b
                    .iter()
                    .map(|x| format!("{x:02X}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            };
            println!(
                "   {:<40} {raw}{}{}{}{}",
                i.path,
                i.text
                    .as_ref()
                    .map(|t| format!("  '{t}'"))
                    .unwrap_or_default(),
                i.phys.map(|p| format!("  = {p}")).unwrap_or_default(),
                i.unit.as_ref().map(|u| format!(" {u}")).unwrap_or_default(),
                i.note
                    .as_ref()
                    .map(|n| format!("  !{n}"))
                    .unwrap_or_default()
            );
        }
    }
}
