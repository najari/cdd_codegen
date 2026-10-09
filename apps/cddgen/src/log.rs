//! `cddgen decode-log`: a CAN log read as diagnostics, the way CANoe's trace window shows it.
//!
//! The frames are put back together with ISO-TP, each message is matched to the services of the
//! description, and a response is read in the light of the request before it.

use crate::inspect::{load, select, show_decoded};
use anyhow::{bail, Context, Result};
use cdd_log::isotp::{Event, FaultKind, Message, Reassembler};
use cdd_model::interp::{Decoded, Interp};
use cdd_model::ir::MsgKind;
use cdd_model::{Cdd, Service};
use clap::Args;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Args)]
pub struct LogArgs {
    /// The `.cdd` file.
    cdd: PathBuf,
    /// The `.asc` or `.blf` log.
    log: PathBuf,
    /// A tester and its ECU: `REQUEST:RESPONSE`, the CAN identifiers in hex (`700:600`). Repeat for
    /// several ECUs. Without it the identifiers are found from the traffic.
    #[arg(long = "id", value_parser = parse_pair)]
    ids: Vec<(u32, u32)>,
    #[arg(long)]
    ecu: Option<String>,
    #[arg(long)]
    variant: Option<String>,
    /// Only frames of this channel.
    #[arg(long)]
    channel: Option<u16>,
    #[arg(long = "language")]
    languages: Vec<String>,
    /// One line per message, without the values.
    #[arg(long)]
    brief: bool,
}

fn parse_hex(s: &str) -> Result<u32, String> {
    let s = s.trim().trim_start_matches("0x").trim_start_matches("0X");
    u32::from_str_radix(s, 16).map_err(|_| format!("`{s}` is not a hexadecimal CAN identifier"))
}

fn parse_pair(s: &str) -> Result<(u32, u32), String> {
    let (a, b) = s
        .split_once(':')
        .ok_or("write REQUEST:RESPONSE, for example 700:600")?;
    Ok((parse_hex(a)?, parse_hex(b)?))
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// How many of the messages of a CAN identifier read as requests, and as responses.
fn tally(interp: &Interp<'_>, services: &[Service], messages: &[&Message]) -> (usize, usize) {
    let mut req = 0;
    let mut resp = 0;
    for m in messages {
        if !interp
            .identify(services, MsgKind::Request, &m.payload)
            .is_empty()
        {
            req += 1;
        }
        let negative = m.payload.len() == 3 && m.payload[0] == 0x7F;
        if negative
            || !interp
                .identify(services, MsgKind::Positive, &m.payload)
                .is_empty()
        {
            resp += 1;
        }
    }
    (req, resp)
}

/// Finds which identifiers carry requests, and which answer them.
fn find_pairs(
    interp: &Interp<'_>,
    services: &[Service],
    messages: &[Message],
) -> Result<Vec<(u32, u32)>> {
    let mut by_id: HashMap<u32, Vec<&Message>> = HashMap::new();
    for m in messages {
        by_id.entry(m.id).or_default().push(m);
    }
    let mut testers: Vec<u32> = by_id
        .iter()
        .filter(|(_, ms)| {
            let (req, resp) = tally(interp, services, ms);
            req > 0 && req >= resp
        })
        .map(|(id, _)| *id)
        .collect();
    testers.sort_unstable();
    if testers.is_empty() {
        bail!("no CAN identifier of the log carries requests of this description; name them with --id REQUEST:RESPONSE");
    }
    let mut pairs = Vec::new();
    for t in testers {
        // The identifier that most often speaks right after the tester does.
        let mut next: HashMap<u32, usize> = HashMap::new();
        for w in messages.windows(2) {
            if w[0].id == t && w[1].id != t && w[1].start - w[0].end < 1.0 {
                *next.entry(w[1].id).or_default() += 1;
            }
        }
        match next
            .into_iter()
            .max_by_key(|(id, n)| (*n, std::cmp::Reverse(*id)))
        {
            Some((resp, _)) => pairs.push((t, resp)),
            None => bail!(
                "nothing answers requests on 0x{t:X}; name the answering identifier with --id"
            ),
        }
    }
    Ok(pairs)
}

pub fn decode_log(a: LogArgs) -> Result<()> {
    let cdd: Cdd = load(&a.cdd)?;
    let (ecu, var) = select(&cdd, a.ecu.as_deref(), a.variant.as_deref())?;
    let interp =
        Interp::new(&cdd).with_languages(cdd_codegen::preferred_languages(&cdd, &a.languages));
    let mut frames = cdd_log::read_frames(&a.log)
        .with_context(|| format!("cannot read `{}`", a.log.display()))?;
    if let Some(c) = a.channel {
        frames.retain(|f| f.channel == c);
    }

    let mut reassembler = Reassembler::new();
    let mut events = Vec::new();
    for f in &frames {
        if let Some(e) = reassembler.push(f) {
            events.push(e);
        }
    }
    let messages: Vec<Message> = events
        .iter()
        .filter_map(|e| {
            if let Event::Message(m) = e {
                Some(m.clone())
            } else {
                None
            }
        })
        .collect();
    let pairs = if a.ids.is_empty() {
        find_pairs(&interp, &var.services, &messages)?
    } else {
        a.ids.clone()
    };
    println!(
        "{} frames, {} diagnostic messages; {}/{} {}",
        frames.len(),
        messages.len(),
        ecu.qual,
        var.qual,
        pairs
            .iter()
            .map(|(q, r)| format!("request 0x{q:X} / response 0x{r:X}"))
            .collect::<Vec<_>>()
            .join(", ")
    );

    // The service the last request of each pair was, so its response can be read with it.
    let mut last_request: HashMap<usize, Option<&Service>> = HashMap::new();
    let (mut requests, mut responses, mut unknown, mut faults) = (0, 0, 0, 0);
    for e in &events {
        let m = match e {
            Event::Error(f) => {
                if pairs.iter().any(|(q, r)| f.id == *q || f.id == *r) {
                    faults += 1;
                    let what = match &f.kind {
                        FaultKind::WrongSequence { expected, got } => {
                            format!("consecutive frame {got} where {expected} was expected")
                        }
                        FaultKind::UnexpectedConsecutive => {
                            "a consecutive frame with no first frame".to_string()
                        }
                        FaultKind::Interrupted { missing } => format!(
                            "a new message began while {missing} bytes of the last were missing"
                        ),
                        FaultKind::Malformed => "a malformed frame".to_string(),
                    };
                    println!("{:>12.6}  0x{:<4X}  ISO-TP fault: {what}", f.time, f.id);
                }
                continue;
            }
            Event::Message(m) => m,
        };
        let Some(pair_index) = pairs.iter().position(|(q, r)| m.id == *q || m.id == *r) else {
            continue;
        };
        let is_request = m.id == pairs[pair_index].0;
        let arrow = if is_request { "->" } else { "<-" };
        let head = format!("{:>12.6}  0x{:<4X} {arrow}", m.start, m.id);

        let (label, decoded): (String, Option<(&Service, Decoded)>) = if is_request {
            requests += 1;
            let mut hits = interp.identify(&var.services, MsgKind::Request, &m.payload);
            match hits.is_empty() {
                true => {
                    last_request.insert(pair_index, None);
                    (String::new(), None)
                }
                false => {
                    let (svc, d) = hits.swap_remove(0);
                    last_request.insert(pair_index, Some(svc));
                    (format!("REQ  {}", svc.shortcut), Some((svc, d)))
                }
            }
        } else if m.payload.len() == 3 && m.payload[0] == 0x7F {
            responses += 1;
            let (sid, nrc) = (m.payload[1], m.payload[2]);
            let name = cdd
                .nrc_name(nrc)
                .and_then(|n| {
                    n.name
                        .pick(&cdd_codegen::preferred_languages(&cdd, &a.languages))
                })
                .unwrap_or("?");
            let service = last_request
                .get(&pair_index)
                .copied()
                .flatten()
                .filter(|s| s.sid == Some(sid))
                .map_or_else(|| format!("service {sid:02X}"), |s| s.shortcut.clone());
            (format!("NEG  {service}: code {nrc:02X} {name}"), None)
        } else {
            responses += 1;
            let mut hits = interp.identify(&var.services, MsgKind::Positive, &m.payload);
            // Prefer the answer of the service that was asked.
            let asked = last_request.get(&pair_index).copied().flatten();
            let pick = asked
                .and_then(|q| hits.iter().position(|(s, _)| s.key == q.key))
                .unwrap_or(0);
            if hits.is_empty() {
                (String::new(), None)
            } else {
                let (svc, d) = hits.swap_remove(pick);
                (format!("POS  {}", svc.shortcut), Some((svc, d)))
            }
        };

        if label.is_empty() {
            unknown += 1;
            println!("{head} ?    not in the description  {}", hex(&m.payload));
            continue;
        }
        println!("{head} {label}  {}", hex(&m.payload));
        if let (false, Some((_, d))) = (a.brief, &decoded) {
            show_decoded(d);
        }
    }
    println!("\n{requests} requests, {responses} responses, {unknown} not in the description, {faults} ISO-TP faults");
    Ok(())
}
