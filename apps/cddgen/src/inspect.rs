//! `cddgen inspect` and `cddgen decode`: reading a CDD without generating anything.

use anyhow::{bail, Context, Result};
use cdd_model::interp::{Decoded, Interp, Raw};
use cdd_model::ir::{BitChild, Item, MsgKind, Repeat, RepeatCount, Shape};
use cdd_model::{Cdd, Ecu, Service, Slot, Variant};
use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct InspectArgs {
    /// The `.cdd` file.
    cdd: PathBuf,
    #[arg(long)]
    ecu: Option<String>,
    #[arg(long)]
    variant: Option<String>,
    /// Show the layout of this service (its CAPL name) instead of the list.
    #[arg(long)]
    service: Option<String>,
    /// Language for names and labels.
    #[arg(long = "language")]
    languages: Vec<String>,
}

#[derive(Args)]
pub struct DecodeArgs {
    /// The `.cdd` file.
    cdd: PathBuf,
    /// The payload in hex: `22 F1 90`.
    payload: Vec<String>,
    #[arg(long)]
    ecu: Option<String>,
    #[arg(long)]
    variant: Option<String>,
    /// The payload is a response (default: a request).
    #[arg(long)]
    response: bool,
    #[arg(long = "language")]
    languages: Vec<String>,
}

pub fn load(path: &PathBuf) -> Result<Cdd> {
    Cdd::load(path).with_context(|| format!("cannot load `{}`", path.display()))
}

pub fn select<'c>(
    cdd: &'c Cdd,
    ecu: Option<&str>,
    variant: Option<&str>,
) -> Result<(&'c Ecu, &'c Variant)> {
    let e = match ecu {
        Some(q) => cdd.ecu(q).with_context(|| format!("no ECU `{q}`"))?,
        None => cdd.ecus.first().context("the document has no ECU")?,
    };
    let v = match variant {
        Some(q) => e
            .variant(q)
            .with_context(|| format!("no variant `{q}` in ECU `{}`", e.qual))?,
        None => e.default_variant().context("the ECU has no variant")?,
    };
    Ok((e, v))
}

fn languages(cdd: &Cdd, configured: &[String]) -> Vec<String> {
    cdd_codegen::preferred_languages(cdd, configured)
}

fn prefix_text(cdd: &Cdd, svc: &Service, kind: MsgKind) -> String {
    match svc.slot(kind) {
        Slot::Ok(m) => {
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
                "{} ({lo}..{})",
                p.join(" "),
                hi.map_or("*".to_string(), |h| h.to_string())
            )
        }
        Slot::Absent => "-".into(),
        Slot::Unsupported(i) => format!("skipped {}", i.code),
    }
}

pub fn inspect(a: InspectArgs) -> Result<()> {
    let cdd = load(&a.cdd)?;
    let langs = languages(&cdd, &a.languages);
    if a.service.is_none() {
        println!(
            "{}: {} dtd {} encoding {} languages {:?}",
            a.cdd.display(),
            cdd.protocol.label(),
            cdd.dtd_version,
            cdd.encoding,
            cdd.languages
        );
        for e in &cdd.ecus {
            let vs: Vec<String> = e
                .variants
                .iter()
                .map(|v| format!("{}{}", v.qual, if v.base { " [base]" } else { "" }))
                .collect();
            println!("ECU {} variants: {}", e.qual, vs.join(", "));
        }
    }
    let (ecu, var) = select(&cdd, a.ecu.as_deref(), a.variant.as_deref())?;
    match &a.service {
        None => {
            println!(
                "\n{}/{}: {} services",
                ecu.qual,
                var.qual,
                var.services.len()
            );
            for s in &var.services {
                println!(
                    "{:<56} REQ {:<34} POS {}",
                    s.shortcut,
                    prefix_text(&cdd, s, MsgKind::Request),
                    prefix_text(&cdd, s, MsgKind::Positive)
                );
            }
            let skipped: usize = var
                .services
                .iter()
                .map(|s| {
                    usize::from(s.request.issue().is_some())
                        + usize::from(s.positive.issue().is_some())
                })
                .sum();
            if skipped > 0 {
                println!("\n{skipped} message(s) cannot be generated yet; `inspect --service NAME` shows why.");
            }
        }
        Some(name) => {
            let s = var
                .services
                .iter()
                .find(|s| s.shortcut == *name)
                .with_context(|| format!("no service `{name}` in {}/{}", ecu.qual, var.qual))?;
            println!("{} ({})", s.shortcut, s.key);
            if let Some(n) = s.name.pick(&langs) {
                println!("  {n}");
            }
            println!(
                "  protocol service {}, SID {}",
                s.protocol_service,
                s.sid.map_or("?".into(), |b| format!("{b:02X}"))
            );
            println!(
                "  NRCs: {}",
                s.nrcs
                    .iter()
                    .map(|n| format!("{n:02X}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            for kind in [MsgKind::Request, MsgKind::Positive] {
                println!("\n{}:", kind.label());
                match s.slot(kind) {
                    Slot::Ok(m) => show_items(&cdd, &m.items, 1),
                    Slot::Absent => println!("  (none)"),
                    Slot::Unsupported(i) => println!("  cannot be generated: {i}"),
                }
            }
        }
    }
    Ok(())
}

fn show_items(cdd: &Cdd, items: &[Item], depth: usize) {
    let pad = "  ".repeat(depth);
    let dt = |id: usize| cdd.datatypes.get(id).qual.clone();
    for item in items {
        match item {
            Item::Const(c) => println!(
                "{pad}const {} = {:#04x} ({} bits{})",
                c.name,
                c.value,
                c.bits,
                if c.suppress_bit { ", suppress bit" } else { "" }
            ),
            Item::Field(f) => {
                let shape = match f.shape {
                    Shape::Atom => String::new(),
                    Shape::Array { count } => format!(" x{count}"),
                    Shape::Rest { min, max } => format!(" x{min}..={max} to the end"),
                };
                println!(
                    "{pad}field {}: {}{shape}",
                    f.name,
                    f.dt.map_or("raw bytes".to_string(), dt)
                );
            }
            Item::Group(g) => {
                println!("{pad}record {}", g.name);
                show_items(cdd, &g.items, depth + 1);
            }
            Item::Bits(b) => {
                println!("{pad}bits {} ({} bits)", b.name, b.bits);
                for c in &b.children {
                    match c {
                        BitChild::Field {
                            name,
                            dt: d,
                            offset,
                            width,
                            ..
                        } => println!("{pad}  bit {offset}..+{width} {name}: {}", dt(*d)),
                        BitChild::Gap { offset, width } => {
                            println!("{pad}  bit {offset}..+{width} (reserved)")
                        }
                    }
                }
            }
            Item::Repeat(Repeat {
                name,
                element,
                count,
                ..
            }) => {
                match count {
                    RepeatCount::ToEnd { min, max } => println!(
                        "{pad}repeat {name} to the end ({min}..={})",
                        max.map_or("any".to_string(), |m| m.to_string())
                    ),
                    RepeatCount::Counted { field, mask } => println!(
                        "{pad}repeat {name} as often as {} says (mask {mask:#x})",
                        field.join(".")
                    ),
                }
                show_items(cdd, element, depth + 1);
            }
            Item::Mux(m) => println!("{pad}multiplexer {}", m.name),
            Item::Gap(n) => println!("{pad}reserved {n} byte(s)"),
            Item::Nrc(n) => println!("{pad}negative response code {}", n.name),
        }
    }
}

pub fn parse_hex(parts: &[String]) -> Result<Vec<u8>> {
    let joined: String = parts.join(" ");
    let cleaned: String = joined
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ',')
        .collect();
    let cleaned = cleaned.strip_prefix("0x").unwrap_or(&cleaned);
    if !cleaned.len().is_multiple_of(2) {
        bail!("`{joined}` has an odd number of hex digits");
    }
    (0..cleaned.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&cleaned[i..i + 2], 16)
                .with_context(|| format!("`{}` is not a hex byte", &cleaned[i..i + 2]))
        })
        .collect()
}

pub fn show_decoded(d: &Decoded) {
    for i in &d.items {
        let raw = match &i.raw {
            Raw::Int(v) => format!("{v} (0x{v:X})"),
            Raw::Float(v) => format!("{v}"),
            Raw::Bytes(b) => b
                .iter()
                .map(|x| format!("{x:02X}"))
                .collect::<Vec<_>>()
                .join(" "),
        };
        let text = i
            .text
            .as_ref()
            .map(|t| format!(" '{t}'"))
            .unwrap_or_default();
        let phys = i
            .phys
            .map(|p| {
                format!(
                    " = {p}{}",
                    i.unit.as_ref().map(|u| format!(" {u}")).unwrap_or_default()
                )
            })
            .unwrap_or_default();
        let note = i
            .note
            .as_ref()
            .map(|n| format!("  ! {n}"))
            .unwrap_or_default();
        println!("    {:<44} {raw}{text}{phys}{note}", i.path);
    }
    if d.suppress_response {
        println!("    (positive response suppressed)");
    }
}

pub fn decode(a: DecodeArgs) -> Result<()> {
    let cdd = load(&a.cdd)?;
    let (ecu, var) = select(&cdd, a.ecu.as_deref(), a.variant.as_deref())?;
    let payload = parse_hex(&a.payload)?;
    let kind = if a.response {
        MsgKind::Positive
    } else {
        MsgKind::Request
    };
    let interp = Interp::new(&cdd).with_languages(languages(&cdd, &a.languages));
    let hits = interp.identify(&var.services, kind, &payload);
    if hits.is_empty() {
        println!(
            "no service of {}/{} has a {} like this",
            ecu.qual,
            var.qual,
            kind.label()
        );
        if let Some(first) = payload.first() {
            let mut near: Vec<&str> = var
                .services
                .iter()
                .filter(|s| {
                    s.sid
                        == Some(if a.response {
                            first.wrapping_sub(0x40)
                        } else {
                            *first
                        })
                })
                .map(|s| s.shortcut.as_str())
                .collect();
            near.truncate(6);
            if !near.is_empty() {
                println!("services with the same SID: {}", near.join(", "));
            }
        }
        return Ok(());
    }
    for (svc, decoded) in hits {
        println!("{} ({})", svc.shortcut, svc.key);
        show_decoded(&decoded);
    }
    Ok(())
}
