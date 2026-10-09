//! `cargo run -p cdd-model --example services -- file.cdd [variant]`
//! Lists the services of a variant with the constant prefix of their request.

use cdd_model::ir::MsgKind;
use cdd_model::Cdd;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cdd = Cdd::load(&args[0]).expect("load");
    let ecu = &cdd.ecus[0];
    let var = match args.get(1) {
        Some(v) => ecu.variant(v).expect("variant"),
        None => ecu.default_variant().expect("a variant"),
    };
    println!("{}/{}: {} services", ecu.qual, var.qual, var.services.len());
    for s in &var.services {
        let describe = |kind: MsgKind| match s.slot(kind) {
            cdd_model::Slot::Ok(m) => {
                let p: Vec<String> = m
                    .prefix(&cdd.datatypes)
                    .iter()
                    .map(|b| {
                        if b.mask == 0xFF {
                            format!("{:02X}", b.value)
                        } else {
                            format!("{:02X}&{:02X}", b.value, b.mask)
                        }
                    })
                    .collect();
                let (lo, hi) = m.len_range(&cdd.datatypes);
                format!(
                    "{} len {lo}..{}",
                    p.join(" "),
                    hi.map_or("*".into(), |h| h.to_string())
                )
            }
            cdd_model::Slot::Absent => "-".into(),
            cdd_model::Slot::Unsupported(i) => format!("UNSUPPORTED {}", i.code),
        };
        println!(
            "{:<58} REQ {:<34} POS {}",
            s.shortcut,
            describe(MsgKind::Request),
            describe(MsgKind::Positive)
        );
    }
}
