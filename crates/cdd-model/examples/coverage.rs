//! Prints how much of each given CDD the resolver understands, and why it skips the rest.
//! `cargo run -p cdd-model --example coverage -- file.cdd...`

use cdd_model::ir::MsgKind;
use cdd_model::{Cdd, Slot};
use std::collections::BTreeMap;

fn main() {
    let verbose = std::env::args().any(|a| a == "-v");
    for path in std::env::args().skip(1).filter(|a| a != "-v") {
        let cdd = match Cdd::load(&path) {
            Ok(c) => c,
            Err(e) => {
                println!("{path}: {e}");
                continue;
            }
        };
        println!("== {path}");
        println!(
            "   {} dtd {} encoding {} languages {:?} data types {} NRCs {}",
            cdd.protocol.label(),
            cdd.dtd_version,
            cdd.encoding,
            cdd.languages,
            cdd.datatypes.items.len(),
            cdd.nrcs.len()
        );
        for ecu in &cdd.ecus {
            for var in &ecu.variants {
                let mut ok = [0usize; 3];
                let mut absent = [0usize; 3];
                let mut why: BTreeMap<String, usize> = BTreeMap::new();
                for s in &var.services {
                    for (i, kind) in [MsgKind::Request, MsgKind::Positive, MsgKind::Negative]
                        .into_iter()
                        .enumerate()
                    {
                        match s.slot(kind) {
                            Slot::Ok(_) => ok[i] += 1,
                            Slot::Absent => absent[i] += 1,
                            Slot::Unsupported(issue) => {
                                if kind != MsgKind::Negative {
                                    // Group by the part of the message before the first backtick: the cause.
                                    let msg = issue
                                        .message
                                        .split('`')
                                        .next()
                                        .unwrap_or("")
                                        .trim()
                                        .to_string();
                                    let full = if verbose { issue.message.clone() } else { msg };
                                    *why.entry(format!("{} {}", issue.code, full)).or_default() +=
                                        1;
                                }
                            }
                        }
                    }
                }
                println!(
                    "   {}/{}{}: {} services, REQ ok {} POS ok {} NEG ok {} (absent REQ {} POS {})",
                    ecu.qual,
                    var.qual,
                    if var.base { " [base]" } else { "" },
                    var.services.len(),
                    ok[0],
                    ok[1],
                    ok[2],
                    absent[0],
                    absent[1]
                );
                for (reason, n) in &why {
                    println!("      {n:4} x {reason}");
                }
            }
        }
        let unresolved = cdd
            .issues
            .iter()
            .filter(|i| i.context.starts_with("data type"))
            .count();
        if unresolved > 0 {
            println!("   data type issues: {unresolved}");
        }
    }
}
