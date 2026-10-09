//! The generated code against the interpreter, over every service of a description.
//!
//! For each message of each service a few hundred random values are encoded by the interpreter,
//! and the generated code must (1) accept the bytes, (2) describe every value exactly as the
//! interpreter decodes it, and (3) write the same bytes again. Truncated, extended and random
//! payloads must get the same verdict from both: accepted by one if and only if by the other.
//!
//! The two read the same layout, but through different code, so this finds mistakes of the
//! generator. It cannot find a mistake in how the layout was read from the CDD: that is what the
//! logs of CANoe's own demo, and the hand-computed bytes of the synthetic tests, are for.

use cdd_model::datatype::{DtKind, Enc};
use cdd_model::interp::{Decoded as Interpreted, EncodeOptions, Input, Interp, Raw as IRaw, Value};
use cdd_model::ir::{BitChild, Field, Item, MsgKind, Repeat, RepeatCount, Shape};
use cdd_model::{Cdd, Service};
use cdd_rt::{FieldValue, Path, RawValue, Visitor};
use std::collections::HashSet;
use std::path::Path as FsPath;

/// A value of a message, owned, so two descriptions can be compared.
#[derive(Clone, Debug, PartialEq)]
pub enum Raw {
    Int(i128),
    Float(f64),
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct Described {
    pub path: String,
    pub raw: Raw,
    pub text: Option<String>,
    pub phys: Option<f64>,
    pub unit: Option<String>,
}

/// Collects what a message describes.
#[derive(Default)]
pub struct Collect(pub Vec<Described>);

impl Visitor for Collect {
    fn field(&mut self, path: &Path<'_>, v: &FieldValue<'_>) {
        let raw = match v.raw {
            RawValue::Uint(x) => Raw::Int(i128::from(x)),
            RawValue::Int(x) => Raw::Int(i128::from(x)),
            RawValue::Float(x) => Raw::Float(x),
            RawValue::Bytes(b) => Raw::Bytes(b.to_vec()),
        };
        self.0.push(Described {
            path: path.to_string(),
            raw,
            text: v.text.map(str::to_string),
            phys: v.physical,
            unit: v.unit.map(str::to_string),
        });
    }
}

/// A small deterministic generator, so a failure can be reproduced.
pub struct Rng(pub u64);

impl Rng {
    pub fn next_u64(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }

    pub fn bool(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }
}

/// What the generated code made of a payload.
pub struct Outcome {
    pub name: String,
    pub items: Vec<Described>,
    pub reencoded: Result<Vec<u8>, String>,
}

/// `None`: no service matches; `Some(Err)`: one matches but the payload does not decode.
pub type Decoded = Option<Result<Outcome, String>>;

/// One generated module, seen through the same two functions.
pub struct Subject {
    pub module: &'static str,
    /// The CDD, below the folder given to [`check_subject`].
    pub file: &'static str,
    pub variant: Option<&'static str>,
    pub request: fn(&[u8]) -> Decoded,
    pub response: fn(&[u8]) -> Decoded,
}

/// Decodes `$payload` with the generated enum `$any` and describes and re-encodes the result.
#[macro_export]
macro_rules! decode_with {
    ($any:path, $payload:expr) => {
        match <$any>::decode($payload) {
            None => None,
            Some(Err(e)) => Some(Err(e.to_string())),
            Some(Ok(m)) => {
                let mut c = $crate::Collect::default();
                m.describe(&mut c);
                let mut out = [0u8; 8192];
                let reencoded = m
                    .encode(&mut out)
                    .map(|n| out[..n].to_vec())
                    .map_err(|e| e.to_string());
                Some(Ok($crate::Outcome {
                    name: m.name().to_string(),
                    items: c.0,
                    reencoded,
                }))
            }
        }
    };
}

/// A [`Subject`] for the generated module `$module` of the crate `$gen`.
#[macro_export]
macro_rules! subject {
    ($gen:ident, $module:ident, $file:expr, $variant:expr) => {
        $crate::Subject {
            module: stringify!($module),
            file: $file,
            variant: $variant,
            request: |p| $crate::decode_with!($gen::$module::AnyRequest, p),
            response: |p| $crate::decode_with!($gen::$module::AnyResponse, p),
        }
    };
}

fn full(prefix: &str, rel: &str) -> String {
    if prefix.is_empty() {
        rel.to_string()
    } else {
        format!("{prefix}.{rel}")
    }
}

/// Random values for every parameter of a message.
fn fill(cdd: &Cdd, items: &[Item], prefix: &str, rng: &mut Rng, input: &mut Input) {
    // The count of a repeated part is left out: encoding sets it from the elements given.
    let counted: HashSet<&str> = items
        .iter()
        .filter_map(|i| match i {
            Item::Repeat(Repeat {
                count: RepeatCount::Counted { field, .. },
                ..
            }) => field.first().map(String::as_str),
            _ => None,
        })
        .collect();
    for item in items {
        match item {
            Item::Const(_) | Item::Gap(_) | Item::Nrc(_) => {}
            Item::Mux(m) => {
                // A case of the multiplexer, or none: then the default structure applies.
                let pick = rng.below(m.cases.len() as u64 + 1) as usize;
                let selector_path = full(prefix, m.selector.first().map_or("", String::as_str));
                let (items, value): (&[Item], i128) = if pick < m.cases.len() {
                    let c = &m.cases[pick];
                    (
                        &c.items,
                        c.lo + rng.below((c.hi - c.lo + 1).min(100) as u64) as i128,
                    )
                } else {
                    let mut v = 0i128;
                    for _ in 0..200 {
                        v = (rng.next_u64() & m.mask) as i128;
                        if !m.cases.iter().any(|c| v >= c.lo && v <= c.hi) {
                            break;
                        }
                    }
                    (&m.default, v)
                };
                fill(cdd, items, prefix, rng, input);
                // The selector field was filled before: it now says which structure follows.
                let chosen = match input.values.get(&selector_path) {
                    Some(Value::Bytes(b)) => Value::Bytes(
                        (0..b.len())
                            .map(|i| (value >> (8 * (b.len() - 1 - i))) as u8)
                            .collect(),
                    ),
                    _ => Value::Raw(value),
                };
                input.set(selector_path, chosen);
            }
            Item::Field(f) => {
                if counted.contains(f.path.as_str()) {
                    continue;
                }
                input.set(full(prefix, &f.path), field_value(cdd, f, rng));
            }
            Item::Group(g) => fill(cdd, &g.items, prefix, rng, input),
            Item::Bits(b) => {
                for ch in &b.children {
                    if let BitChild::Field {
                        path, dt, width, ..
                    } = ch
                    {
                        let signed = cdd.datatypes.get(*dt).coded.enc == Enc::Signed;
                        input.set(
                            full(prefix, path),
                            Value::Raw(random_int(rng, *width, signed)),
                        );
                    }
                }
            }
            Item::Repeat(r) => {
                let (lo, hi) = match r.count {
                    RepeatCount::ToEnd { min, max } => {
                        (u64::from(min), max.map_or(4, u64::from).min(4))
                    }
                    RepeatCount::Counted { .. } => (0, 4),
                };
                let n = lo + rng.below(hi.saturating_sub(lo) + 1);
                for i in 0..n {
                    fill(
                        cdd,
                        &r.element,
                        &format!("{}[{i}]", full(prefix, &r.path)),
                        rng,
                        input,
                    );
                }
            }
        }
    }
}

fn random_int(rng: &mut Rng, bits: u32, signed: bool) -> i128 {
    let raw = u128::from(rng.next_u64()) | (u128::from(rng.next_u64()) << 64);
    let masked = raw & ((1u128 << bits.min(127)) - 1);
    if signed {
        masked as i128 - (1i128 << (bits - 1))
    } else {
        masked as i128
    }
}

fn field_value(cdd: &Cdd, f: &Field, rng: &mut Rng) -> Value {
    let elem_bytes = f.elem_bytes(&cdd.datatypes);
    let bytes =
        |rng: &mut Rng, n: usize| Value::Bytes((0..n).map(|_| rng.next_u64() as u8).collect());
    match (f.dt, f.shape) {
        (_, Shape::Array { count }) => bytes(rng, elem_bytes * count as usize),
        (_, Shape::Rest { min, max }) => {
            let n = u64::from(min) + rng.below(u64::from(max.min(min + 3) - min) + 1);
            bytes(rng, n as usize * elem_bytes)
        }
        (Some(dt), Shape::Atom) => {
            let d = cdd.datatypes.get(dt);
            let bits = d.coded.bits;
            match d.coded.enc {
                Enc::Float | Enc::Double => Value::Physical(if bits == 32 {
                    f64::from(((rng.below(2000) as f64) - 1000.0) as f32)
                } else {
                    (rng.below(2_000_000) as f64) / 100.0 - 10_000.0
                }),
                Enc::Bcd => Value::Raw(i128::from(
                    rng.next_u64() % 10u64.pow((bits / 8 * 2).min(18)),
                )),
                Enc::Signed => Value::Raw(random_int(rng, bits, true)),
                _ => {
                    // Half the time a value the description names, so the text tables are exercised.
                    if let DtKind::Text(entries) = &d.kind {
                        if !entries.is_empty() && rng.bool() {
                            return Value::Raw(
                                entries[rng.below(entries.len() as u64) as usize].lo,
                            );
                        }
                    }
                    Value::Raw(random_int(rng, bits, false))
                }
            }
        }
        (None, Shape::Atom) => unreachable!("an atom has a data type"),
    }
}

/// The interpreter shows a BCD number with a nibble above 9 as raw bytes with a note, so a log can
/// still be read; the generated types hold a number and refuse it. That is the one place they differ.
fn lenient_only(interpreted: &[(&Service, Interpreted)]) -> bool {
    interpreted
        .iter()
        .all(|(_, d)| d.items.iter().any(|i| i.note.is_some()))
}

fn same_float(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

/// The generated description of a message must be the interpreter's, item by item.
fn compare(context: &str, generated: &[Described], interpreted: &Interpreted) {
    assert_eq!(
        generated.len(),
        interpreted.items.len(),
        "{context}: {} items generated, {} interpreted\n generated: {:?}\n interpreted: {:?}",
        generated.len(),
        interpreted.items.len(),
        generated.iter().map(|i| &i.path).collect::<Vec<_>>(),
        interpreted
            .items
            .iter()
            .map(|i| &i.path)
            .collect::<Vec<_>>()
    );
    for (g, i) in generated.iter().zip(&interpreted.items) {
        assert_eq!(g.path, i.path, "{context}: path");
        let raw_ok = match (&g.raw, &i.raw) {
            (Raw::Int(a), IRaw::Int(b)) => a == b,
            (Raw::Float(a), IRaw::Float(b)) => same_float(*a, *b),
            (Raw::Bytes(a), IRaw::Bytes(b)) => a == b,
            _ => false,
        };
        assert!(
            raw_ok,
            "{context}: `{}` raw {:?} generated, {:?} interpreted",
            g.path, g.raw, i.raw
        );
        // The characters of a byte field are read leniently by both; the symbolic and physical
        // values of the others are exact.
        if !matches!(g.raw, Raw::Bytes(_)) {
            assert_eq!(g.text, i.text, "{context}: `{}` text", g.path);
            match (g.phys, i.phys) {
                (None, None) => {}
                (Some(a), Some(b)) => assert!(
                    same_float(a, b),
                    "{context}: `{}` physical {a} generated, {b} interpreted",
                    g.path
                ),
                (a, b) => panic!(
                    "{context}: `{}` physical {a:?} generated, {b:?} interpreted",
                    g.path
                ),
            }
        }
        assert_eq!(g.unit, i.unit, "{context}: `{}` unit", g.path);
    }
}

pub struct Stats {
    pub payloads: usize,
    pub mutated: usize,
    /// Messages of the description that are not generated, and so not checked.
    pub not_generated: usize,
}

/// Checks `s` against the CDD `root/s.file`. Panics, with the message that failed, on a difference.
pub fn check_subject(root: &FsPath, s: &Subject) -> Stats {
    let cdd = Cdd::load(root.join(s.file)).expect("the description loads");
    let ecu = &cdd.ecus[0];
    let variant = match s.variant {
        Some(v) => ecu.variant(v).expect("the variant exists"),
        None => ecu.default_variant().expect("the ECU has a variant"),
    };
    let interp = Interp::new(&cdd).with_languages(cdd_codegen::preferred_languages(&cdd, &[]));
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ s.module.len() as u64);
    let mut stats = Stats {
        payloads: 0,
        mutated: 0,
        not_generated: 0,
    };

    for svc in &variant.services {
        for (kind, which) in [
            (MsgKind::Request, s.request),
            (MsgKind::Positive, s.response),
        ] {
            let Some(msg) = svc.slot(kind).message() else {
                continue;
            };
            for round in 0..24 {
                let mut input = Input::default();
                fill(&cdd, &msg.items, "", &mut rng, &mut input);
                let opts = EncodeOptions {
                    suppress_response: kind == MsgKind::Request && rng.bool(),
                    fill: false,
                };
                let ctx = format!(
                    "{} {} {} round {round}",
                    s.module,
                    svc.shortcut,
                    kind.label()
                );
                let payload = interp
                    .encode(msg, &input, opts)
                    .unwrap_or_else(|e| panic!("{ctx}: the interpreter cannot encode: {e}"));
                stats.payloads += 1;

                let candidates = interp.identify(&variant.services, kind, &payload);
                assert!(
                    candidates.iter().any(|(c, _)| c.key == svc.key),
                    "{ctx}: the interpreter does not read back what it wrote ({payload:02X?})"
                );

                let Some(outcome) = which(&payload) else {
                    // A message the generator left out (and said so in its report).
                    stats.not_generated += 1;
                    break;
                };
                let generated = outcome.unwrap_or_else(|e| {
                    panic!("{ctx}: the generated code refuses {payload:02X?}: {e}")
                });
                let (_, expected) = candidates
                    .iter()
                    .find(|(c, _)| c.shortcut == generated.name)
                    .unwrap_or_else(|| {
                        panic!(
                            "{ctx}: the generated code picked `{}`, the interpreter only {:?}",
                            generated.name,
                            candidates
                                .iter()
                                .map(|(c, _)| &c.shortcut)
                                .collect::<Vec<_>>()
                        )
                    });
                compare(&ctx, &generated.items, expected);
                assert_eq!(
                    generated.reencoded.as_deref(),
                    Ok(payload.as_slice()),
                    "{ctx}: writing the decoded message gives other bytes"
                );

                // The same verdict on payloads that are almost right.
                let mut variants: Vec<Vec<u8>> = (0..payload.len())
                    .map(|cut| payload[..cut].to_vec())
                    .collect();
                let mut longer = payload.clone();
                longer.push(rng.next_u64() as u8);
                variants.push(longer);
                if !payload.is_empty() {
                    let mut flipped = payload.clone();
                    let at = rng.below(flipped.len() as u64) as usize;
                    flipped[at] ^= 1 << rng.below(8);
                    variants.push(flipped);
                }
                for v in variants {
                    stats.mutated += 1;
                    let interpreted = interp.identify(&variant.services, kind, &v);
                    match (which(&v), interpreted.is_empty()) {
                        (Some(Ok(g)), false) => assert!(interpreted.iter().any(|(c, _)| c.shortcut == g.name), "{ctx}: {v:02X?} is `{}` for the generated code, not for the interpreter", g.name),
                        (Some(Ok(g)), true) => panic!("{ctx}: the generated code reads {v:02X?} as `{}`, the interpreter reads nothing", g.name),
                        (None | Some(Err(_)), false) => assert!(lenient_only(&interpreted), "{ctx}: the interpreter reads {v:02X?} as `{}`, the generated code refuses it", interpreted[0].0.shortcut),
                        (None | Some(Err(_)), true) => {}
                    }
                }
            }
        }
    }

    // Random bytes: never a crash, and the same verdict.
    for _ in 0..3000 {
        let len = rng.below(12) as usize;
        let mut v: Vec<u8> = (0..len).map(|_| rng.next_u64() as u8).collect();
        // Half of them start like a real service.
        if rng.bool() {
            if let Some(m) = variant
                .services
                .get(rng.below(variant.services.len() as u64) as usize)
                .and_then(|svc| svc.request.message())
            {
                for p in m.prefix(&cdd.datatypes) {
                    if p.offset < v.len() {
                        v[p.offset] = p.value;
                    }
                }
            }
        }
        stats.mutated += 1;
        let interpreted = interp.identify(&variant.services, MsgKind::Request, &v);
        match ((s.request)(&v), interpreted.is_empty()) {
            (Some(Ok(g)), false) => assert!(
                interpreted.iter().any(|(c, _)| c.shortcut == g.name),
                "{} random {v:02X?}",
                s.module
            ),
            (Some(Ok(g)), true) => panic!(
                "{}: random {v:02X?} is `{}` for the generated code only",
                s.module, g.name
            ),
            (None | Some(Err(_)), false) => {
                // A request of a service whose request was left out is not the generated code's to read.
                let left_out = interpreted
                    .iter()
                    .all(|(svc, _)| svc.request.issue().is_some());
                assert!(
                    left_out || lenient_only(&interpreted),
                    "{}: random {v:02X?} is `{}` for the interpreter only",
                    s.module,
                    interpreted[0].0.shortcut
                )
            }
            (None | Some(Err(_)), true) => {}
        }
    }
    stats
}
