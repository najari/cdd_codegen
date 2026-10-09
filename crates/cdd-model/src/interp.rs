//! Decodes and encodes payloads by interpreting the layout IR.
//!
//! This is the reference the generated code is compared against, and what the command line uses
//! to show a payload without generating anything first.

use crate::datatype::{ByteOrder, DataType, DtKind, Enc};
use crate::ir::{
    items_size_range, BitChild, Bits, Const, DtId, Field, Item, Message, MsgKind, Mux, Repeat,
    RepeatCount, Role, Shape,
};
use crate::model::{Cdd, Service};
use std::collections::{BTreeMap, HashMap};
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Raw {
    Int(i128),
    Float(f64),
    Bytes(Vec<u8>),
}

/// One value found in a payload.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedItem {
    /// `Group.Field`, `List[2].Field`; a repeated name has `~2`, `~3`.
    pub path: String,
    pub raw: Raw,
    /// The physical value of a linear conversion.
    pub phys: Option<f64>,
    /// The symbolic value of a text table, or the characters of a text.
    pub text: Option<String>,
    pub unit: Option<String>,
    /// Where it starts in the payload, and how many bytes (the container's, for a bit field).
    pub offset: usize,
    pub len: usize,
    /// Why the value deserves a second look.
    pub note: Option<String>,
    pub role: Option<Role>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Decoded {
    pub items: Vec<DecodedItem>,
    /// The request had the suppress-positive-response bit set.
    pub suppress_response: bool,
}

impl Decoded {
    pub fn get(&self, path: &str) -> Option<&DecodedItem> {
        self.items.iter().find(|i| i.path == path)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum DecodeError {
    Truncated {
        at: String,
        needed: usize,
        got: usize,
    },
    TrailingBytes {
        extra: usize,
    },
    /// A constant byte (SID, sub-function, DID) differs.
    Mismatch {
        at: String,
        offset: usize,
        expected: u8,
        got: u8,
    },
    Length {
        at: String,
        detail: String,
    },
    Unsupported(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Truncated { at, needed, got } => write!(
                f,
                "payload ends inside `{at}`: needs {needed} bytes, has {got}"
            ),
            DecodeError::TrailingBytes { extra } => {
                write!(f, "{extra} bytes remain after the last item")
            }
            DecodeError::Mismatch {
                at,
                offset,
                expected,
                got,
            } => {
                write!(
                    f,
                    "byte {offset} of `{at}` is {got:#04x}, expected {expected:#04x}"
                )
            }
            DecodeError::Length { at, detail } => write!(f, "`{at}`: {detail}"),
            DecodeError::Unsupported(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// What to encode a field from.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Raw(i128),
    Physical(f64),
    Text(String),
    Bytes(Vec<u8>),
}

/// Values by path. Missing fields take their default when `EncodeOptions::fill` says so.
#[derive(Clone, Debug, Default)]
pub struct Input {
    pub values: BTreeMap<String, Value>,
}

impl Input {
    pub fn set(&mut self, path: impl Into<String>, v: Value) -> &mut Self {
        self.values.insert(path.into(), v);
        self
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EncodeOptions {
    /// Set the suppress-positive-response bit of the request's sub-function.
    pub suppress_response: bool,
    /// A field without a value takes its default (else 0). Without this it is an error.
    pub fill: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EncodeError {
    Missing { path: String },
    Range { path: String, detail: String },
    Unsupported(String),
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncodeError::Missing { path } => write!(f, "no value for `{path}`"),
            EncodeError::Range { path, detail } => write!(f, "`{path}`: {detail}"),
            EncodeError::Unsupported(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for EncodeError {}

/// `List[2]` + `DTC` is `List[2].DTC`; with no prefix the path stands alone.
fn full(prefix: &str, rel: &str) -> String {
    if prefix.is_empty() {
        rel.to_string()
    } else {
        format!("{prefix}.{rel}")
    }
}

fn mask_bits(bits: u32) -> u128 {
    if bits >= 128 {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    }
}

fn read_uint(bytes: &[u8], order: ByteOrder) -> u128 {
    let fold = |acc: u128, b: &u8| (acc << 8) | u128::from(*b);
    match order {
        ByteOrder::Big => bytes.iter().fold(0, fold),
        ByteOrder::Little => bytes.iter().rev().fold(0, fold),
    }
}

fn write_uint(v: u128, n: usize, order: ByteOrder) -> Vec<u8> {
    let be = v.to_be_bytes();
    let mut out = be[16 - n.min(16)..].to_vec();
    if order == ByteOrder::Little {
        out.reverse();
    }
    out
}

fn sign_extend(v: u128, bits: u32) -> i128 {
    if bits == 0 || bits >= 128 {
        return v as i128;
    }
    let shift = 128 - bits;
    ((v << shift) as i128) >> shift
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|b| *b as char).collect()
}

struct Cursor<'p> {
    payload: &'p [u8],
    pos: usize,
}

impl<'p> Cursor<'p> {
    fn remaining(&self) -> usize {
        self.payload.len() - self.pos
    }

    fn take(&mut self, n: usize, at: &str) -> Result<&'p [u8], DecodeError> {
        if self.remaining() < n {
            return Err(DecodeError::Truncated {
                at: at.to_string(),
                needed: self.pos + n,
                got: self.payload.len(),
            });
        }
        let s = &self.payload[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
}

/// Values decoded so far, by path: what a count refers to.
type Scope = HashMap<String, i128>;

pub struct Interp<'c> {
    cdd: &'c Cdd,
    langs: Vec<String>,
}

impl<'c> Interp<'c> {
    pub fn new(cdd: &'c Cdd) -> Self {
        Interp {
            cdd,
            langs: cdd.languages.clone(),
        }
    }

    /// Languages to take symbolic values from, best first.
    pub fn with_languages(mut self, langs: Vec<String>) -> Self {
        self.langs = langs;
        self
    }

    fn dt(&self, id: DtId) -> &'c DataType {
        self.cdd.datatypes.get(id)
    }

    fn text_of(&self, dt: &DataType, raw: i128) -> Option<String> {
        // An entry with an empty text says nothing; it still ends the search, as the first entry that holds the value.
        dt.text_for(raw)
            .and_then(|t| t.pick(&self.langs))
            .filter(|t| !t.is_empty())
            .map(str::to_string)
    }

    // ---- decoding --------------------------------------------------------------------------

    pub fn decode(&self, msg: &Message, payload: &[u8]) -> Result<Decoded, DecodeError> {
        let mut out = Decoded::default();
        let mut cur = Cursor { payload, pos: 0 };
        self.decode_items(&msg.items, &mut cur, "", &mut Scope::new(), &mut out)?;
        if cur.remaining() > 0 {
            return Err(DecodeError::TrailingBytes {
                extra: cur.remaining(),
            });
        }
        Ok(out)
    }

    fn decode_items(
        &self,
        items: &[Item],
        cur: &mut Cursor<'_>,
        prefix: &str,
        scope: &mut Scope,
        out: &mut Decoded,
    ) -> Result<(), DecodeError> {
        for item in items {
            match item {
                Item::Const(c) => self.decode_const(c, cur, prefix, out)?,
                Item::Field(f) => self.decode_field(f, cur, prefix, scope, out)?,
                Item::Group(g) => self.decode_items(&g.items, cur, prefix, scope, out)?,
                Item::Bits(b) => self.decode_bits(b, cur, prefix, scope, out)?,
                Item::Gap(n) => {
                    cur.take(*n as usize, "gap")?;
                }
                Item::Nrc(n) => {
                    let path = full(prefix, &n.path);
                    let offset = cur.pos;
                    let b = cur.take(1, &path)?[0];
                    let text = self
                        .cdd
                        .nrc_name(b)
                        .and_then(|nrc| nrc.name.pick(&self.langs).map(str::to_string));
                    scope.insert(path.clone(), i128::from(b));
                    out.items.push(DecodedItem {
                        path,
                        raw: Raw::Int(i128::from(b)),
                        phys: None,
                        text,
                        unit: None,
                        offset,
                        len: 1,
                        note: None,
                        role: None,
                    });
                }
                Item::Repeat(r) => self.decode_repeat(r, cur, prefix, scope, out)?,
                Item::Mux(m) => self.decode_mux(m, cur, prefix, scope, out)?,
            }
        }
        Ok(())
    }

    fn decode_const(
        &self,
        c: &Const,
        cur: &mut Cursor<'_>,
        prefix: &str,
        out: &mut Decoded,
    ) -> Result<(), DecodeError> {
        let path = full(prefix, &c.path);
        let offset = cur.pos;
        let got = cur.take(c.bytes(), &path)?;
        for (i, ((g, want), m)) in got
            .iter()
            .zip(c.wire_bytes())
            .zip(c.wire_mask())
            .enumerate()
        {
            if g & m != want & m {
                return Err(DecodeError::Mismatch {
                    at: path,
                    offset: offset + i,
                    expected: want,
                    got: *g,
                });
            }
        }
        let raw = read_uint(got, c.order) & u128::from(u64::MAX);
        if c.suppress_bit && raw & (1u128 << (c.bits - 1)) != 0 {
            out.suppress_response = true;
        }
        out.items.push(DecodedItem {
            path,
            raw: Raw::Int((raw & u128::from(c.mask)) as i128),
            phys: None,
            text: None,
            unit: None,
            offset,
            len: c.bytes(),
            note: None,
            role: Some(c.role),
        });
        Ok(())
    }

    /// The physical, symbolic and unit meaning of one integer.
    fn describe(
        &self,
        dt: Option<&DataType>,
        raw: i128,
    ) -> (Option<f64>, Option<String>, Option<String>) {
        match dt {
            Some(dt) => (
                dt.physical(raw),
                self.text_of(dt, raw),
                dt.phys.unit.clone(),
            ),
            None => (None, None, None),
        }
    }

    fn decode_value(&self, dt: &DataType, bytes: &[u8], bits: u32) -> (Raw, Option<String>) {
        let order = dt.coded.order;
        match dt.coded.enc {
            Enc::Signed => (Raw::Int(sign_extend(read_uint(bytes, order), bits)), None),
            Enc::Float | Enc::Double => {
                let v = read_uint(bytes, order);
                match bits {
                    32 => (Raw::Float(f64::from(f32::from_bits(v as u32))), None),
                    64 => (Raw::Float(f64::from_bits(v as u64)), None),
                    _ => (
                        Raw::Bytes(bytes.to_vec()),
                        Some(format!("a {bits}-bit float is not defined")),
                    ),
                }
            }
            Enc::Bcd => {
                let wire: Vec<u8> = if order == ByteOrder::Little {
                    bytes.iter().rev().copied().collect()
                } else {
                    bytes.to_vec()
                };
                let mut v: i128 = 0;
                for b in &wire {
                    for nib in [b >> 4, b & 0x0F] {
                        if nib > 9 {
                            return (
                                Raw::Bytes(bytes.to_vec()),
                                Some("not a BCD number (a nibble is above 9)".into()),
                            );
                        }
                        v = v * 10 + i128::from(nib);
                    }
                }
                (Raw::Int(v), None)
            }
            Enc::Unsigned | Enc::Ascii | Enc::Utf => {
                (Raw::Int(read_uint(bytes, order) as i128), None)
            }
        }
    }

    fn decode_field(
        &self,
        f: &Field,
        cur: &mut Cursor<'_>,
        prefix: &str,
        scope: &mut Scope,
        out: &mut Decoded,
    ) -> Result<(), DecodeError> {
        let path = full(prefix, &f.path);
        let offset = cur.pos;
        let dt = f.dt.map(|d| self.dt(d));
        let elem_bytes = f.dt.map_or(1, |d| (self.dt(d).coded.bits / 8) as usize);
        match f.shape {
            Shape::Atom => {
                let dt = dt.expect("an atom has a data type");
                let bytes = cur.take(elem_bytes, &path)?;
                let (raw, note) = self.decode_value(dt, bytes, dt.coded.bits);
                let (phys, text, unit) = match &raw {
                    Raw::Int(v) => {
                        scope.insert(path.clone(), *v);
                        self.describe(Some(dt), *v)
                    }
                    _ => (None, None, dt.phys.unit.clone()),
                };
                out.items.push(DecodedItem {
                    path,
                    raw,
                    phys,
                    text,
                    unit,
                    offset,
                    len: elem_bytes,
                    note,
                    role: None,
                });
            }
            Shape::Array { count } => {
                let bytes = cur.take(elem_bytes * count as usize, &path)?;
                self.push_array(dt, bytes, path, offset, scope, out);
            }
            Shape::Rest { min, max } => {
                let rest = cur.remaining();
                if !rest.is_multiple_of(elem_bytes) {
                    return Err(DecodeError::Length {
                        at: path,
                        detail: format!(
                            "{rest} bytes are not a whole number of {elem_bytes}-byte elements"
                        ),
                    });
                }
                let n = rest / elem_bytes;
                if n < min as usize || n > max as usize {
                    return Err(DecodeError::Length {
                        at: path,
                        detail: format!("{n} elements, expected {min}..={max}"),
                    });
                }
                let bytes = cur.take(rest, &path)?;
                self.push_array(dt, bytes, path, offset, scope, out);
            }
        }
        Ok(())
    }

    fn push_array(
        &self,
        dt: Option<&DataType>,
        bytes: &[u8],
        path: String,
        offset: usize,
        scope: &mut Scope,
        out: &mut Decoded,
    ) {
        // A raw slot of up to eight bytes is a number too: iterations count with it.
        if dt.is_none() && (1..=8).contains(&bytes.len()) {
            scope.insert(path.clone(), read_uint(bytes, ByteOrder::Big) as i128);
        }
        let text = dt.and_then(|d| match d.coded.enc {
            Enc::Ascii => Some(latin1(bytes)),
            Enc::Utf => Some(String::from_utf8_lossy(bytes).into_owned()),
            _ => None,
        });
        out.items.push(DecodedItem {
            path,
            raw: Raw::Bytes(bytes.to_vec()),
            phys: None,
            text,
            unit: dt.and_then(|d| d.phys.unit.clone()),
            offset,
            len: bytes.len(),
            note: None,
            role: None,
        });
    }

    fn decode_bits(
        &self,
        b: &Bits,
        cur: &mut Cursor<'_>,
        prefix: &str,
        scope: &mut Scope,
        out: &mut Decoded,
    ) -> Result<(), DecodeError> {
        let offset = cur.pos;
        let n = (b.bits / 8) as usize;
        let value = read_uint(cur.take(n, &full(prefix, &b.path))?, b.order);
        for ch in &b.children {
            if let BitChild::Field {
                path,
                dt,
                offset: o,
                width,
                ..
            } = ch
            {
                let dtype = self.dt(*dt);
                let field = (value >> o) & mask_bits(*width);
                let raw = if dtype.coded.enc == Enc::Signed {
                    sign_extend(field, *width)
                } else {
                    field as i128
                };
                let path = full(prefix, path);
                scope.insert(path.clone(), raw);
                let (phys, text, unit) = self.describe(Some(dtype), raw);
                out.items.push(DecodedItem {
                    path,
                    raw: Raw::Int(raw),
                    phys,
                    text,
                    unit,
                    offset,
                    len: n,
                    note: None,
                    role: None,
                });
            }
        }
        Ok(())
    }

    /// The case of a multiplexer a selector value chooses.
    fn mux_items<'m>(&self, m: &'m Mux, selector: i128) -> &'m [Item] {
        let v = (selector as u128 & u128::from(m.mask)) as i128;
        m.cases
            .iter()
            .find(|c| v >= c.lo && v <= c.hi)
            .map_or(m.default.as_slice(), |c| c.items.as_slice())
    }

    fn decode_mux(
        &self,
        m: &Mux,
        cur: &mut Cursor<'_>,
        prefix: &str,
        scope: &mut Scope,
        out: &mut Decoded,
    ) -> Result<(), DecodeError> {
        let key = full(prefix, m.selector.first().map_or("", String::as_str));
        let selector = scope.get(&key).copied().ok_or_else(|| {
            DecodeError::Unsupported(format!(
                "the selector `{key}` of `{}` was not decoded before it",
                m.name
            ))
        })?;
        self.decode_items(self.mux_items(m, selector), cur, prefix, scope, out)
    }

    fn decode_repeat(
        &self,
        r: &Repeat,
        cur: &mut Cursor<'_>,
        prefix: &str,
        scope: &mut Scope,
        out: &mut Decoded,
    ) -> Result<(), DecodeError> {
        let rpath = full(prefix, &r.path);
        let (ebytes, _) = items_size_range(&r.element, &self.cdd.datatypes);
        let count = match &r.count {
            RepeatCount::ToEnd { min, max } => {
                let rest = cur.remaining();
                if ebytes == 0 || !rest.is_multiple_of(ebytes) {
                    return Err(DecodeError::Length {
                        at: rpath,
                        detail: format!(
                            "{rest} bytes are not a whole number of {ebytes}-byte elements"
                        ),
                    });
                }
                let n = rest / ebytes;
                if n < *min as usize || max.is_some_and(|m| n > m as usize) {
                    return Err(DecodeError::Length {
                        at: rpath,
                        detail: format!(
                            "{n} elements, expected {min}..={}",
                            max.map_or("any".into(), |m| m.to_string())
                        ),
                    });
                }
                n
            }
            RepeatCount::Counted { field, mask } => {
                let key = full(prefix, field.first().map_or("", String::as_str));
                let v = scope.get(&key).copied().ok_or_else(|| {
                    DecodeError::Unsupported(format!(
                        "the count `{key}` of `{rpath}` was not decoded before it"
                    ))
                })?;
                let n = (v as u128 & u128::from(*mask)) as usize;
                if n.saturating_mul(ebytes) > cur.remaining() {
                    return Err(DecodeError::Truncated {
                        at: rpath,
                        needed: cur.pos + n * ebytes,
                        got: cur.payload.len(),
                    });
                }
                n
            }
        };
        for i in 0..count {
            self.decode_items(&r.element, cur, &format!("{rpath}[{i}]"), scope, out)?;
        }
        Ok(())
    }

    // ---- encoding --------------------------------------------------------------------------

    pub fn encode(
        &self,
        msg: &Message,
        input: &Input,
        opts: EncodeOptions,
    ) -> Result<Vec<u8>, EncodeError> {
        let mut out = Vec::new();
        self.encode_items(&msg.items, input, opts, "", &mut HashMap::new(), &mut out)?;
        Ok(out)
    }

    fn encode_items(
        &self,
        items: &[Item],
        input: &Input,
        opts: EncodeOptions,
        prefix: &str,
        forced: &mut HashMap<String, i128>,
        out: &mut Vec<u8>,
    ) -> Result<(), EncodeError> {
        // A count the caller does not give is the number of elements it did give.
        for item in items {
            if let Item::Repeat(Repeat {
                path,
                count: RepeatCount::Counted { field, .. },
                ..
            }) = item
            {
                let n = repeat_len(input, &full(prefix, path));
                if let Some(key) = field.first() {
                    forced.entry(key.clone()).or_insert(n as i128);
                }
            }
        }
        for item in items {
            match item {
                Item::Const(c) => {
                    let mut v = u128::from(c.value);
                    if c.suppress_bit && opts.suppress_response {
                        v |= 1u128 << (c.bits - 1);
                    }
                    out.extend(write_uint(v, c.bytes(), c.order));
                }
                Item::Field(f) => self.encode_field(f, input, opts, prefix, forced, out)?,
                Item::Group(g) => self.encode_items(&g.items, input, opts, prefix, forced, out)?,
                Item::Bits(b) => self.encode_bits(b, input, opts, prefix, out)?,
                Item::Gap(n) => out.extend(std::iter::repeat_n(0u8, *n as usize)),
                Item::Nrc(n) => {
                    let path = full(prefix, &n.path);
                    let v = self.scalar(input.values.get(&path), &path, None, opts, None)?;
                    out.push(v as u8);
                }
                Item::Repeat(r) => {
                    let rpath = full(prefix, &r.path);
                    for i in 0..repeat_len(input, &rpath) {
                        self.encode_items(
                            &r.element,
                            input,
                            opts,
                            &format!("{rpath}[{i}]"),
                            forced,
                            out,
                        )?;
                    }
                }
                Item::Mux(m) => {
                    let key = full(prefix, m.selector.first().map_or("", String::as_str));
                    let selector = match input.values.get(&key) {
                        // A raw slot of a few bytes is a big-endian number.
                        Some(Value::Bytes(b)) if b.len() <= 8 => {
                            b.iter().fold(0i128, |acc, x| (acc << 8) | i128::from(*x))
                        }
                        given => self.scalar(given, &key, None, opts, None)?,
                    };
                    let items = self.mux_items(m, selector);
                    self.encode_items(items, input, opts, prefix, forced, out)?;
                }
            }
        }
        Ok(())
    }

    /// An integer from the caller's value, the field's default or zero.
    fn scalar(
        &self,
        v: Option<&Value>,
        path: &str,
        dt: Option<&DataType>,
        opts: EncodeOptions,
        default: Option<i128>,
    ) -> Result<i128, EncodeError> {
        match v {
            Some(Value::Raw(r)) => Ok(*r),
            Some(Value::Physical(p)) => {
                dt.and_then(|d| d.raw_from_physical(*p))
                    .ok_or_else(|| EncodeError::Range {
                        path: path.into(),
                        detail: format!("no raw value gives the physical value {p}"),
                    })
            }
            Some(Value::Text(t)) => dt
                .and_then(|d| match &d.kind {
                    DtKind::Text(entries) => entries
                        .iter()
                        .find(|e| e.text.0.iter().any(|(_, s)| s == t))
                        .map(|e| e.lo),
                    _ => None,
                })
                .ok_or_else(|| EncodeError::Range {
                    path: path.into(),
                    detail: format!("`{t}` is not an entry of the text table"),
                }),
            Some(Value::Bytes(_)) => Err(EncodeError::Range {
                path: path.into(),
                detail: "bytes given for a single value".into(),
            }),
            None if opts.fill => Ok(default.unwrap_or(0)),
            None => Err(EncodeError::Missing { path: path.into() }),
        }
    }

    fn check_range(
        &self,
        path: &str,
        v: i128,
        bits: u32,
        signed: bool,
    ) -> Result<u128, EncodeError> {
        let (lo, hi) = if signed {
            (
                -(1i128 << (bits - 1).min(126)),
                (1i128 << (bits - 1).min(126)) - 1,
            )
        } else {
            (0, (1i128 << bits.min(126)) - 1)
        };
        if bits >= 127 || (v >= lo && v <= hi) {
            Ok(v as u128 & mask_bits(bits))
        } else {
            Err(EncodeError::Range {
                path: path.into(),
                detail: format!("{v} does not fit in {bits} bits"),
            })
        }
    }

    fn encode_field(
        &self,
        f: &Field,
        input: &Input,
        opts: EncodeOptions,
        prefix: &str,
        forced: &HashMap<String, i128>,
        out: &mut Vec<u8>,
    ) -> Result<(), EncodeError> {
        let path = full(prefix, &f.path);
        let dt = f.dt.map(|d| self.dt(d));
        let ebytes = f.dt.map_or(1, |d| (self.dt(d).coded.bits / 8) as usize);
        let given = input.values.get(&path);
        match f.shape {
            Shape::Atom => {
                let dt = dt.expect("an atom has a data type");
                if matches!(dt.coded.enc, Enc::Float | Enc::Double) {
                    return self.encode_float(dt, given, &path, opts, out);
                }
                // The caller's value wins; a count the caller left out is the number of elements.
                let raw = match (given, forced.get(&path)) {
                    (None, Some(n)) => *n,
                    _ => self.scalar(given, &path, Some(dt), opts, f.default)?,
                };
                if dt.coded.enc == Enc::Bcd {
                    let digits = raw.to_string();
                    if raw < 0 || digits.len() > ebytes * 2 {
                        return Err(EncodeError::Range {
                            path,
                            detail: format!("{raw} does not fit in {ebytes} BCD bytes"),
                        });
                    }
                    let padded = format!("{digits:0>width$}", width = ebytes * 2);
                    let mut bytes: Vec<u8> = padded
                        .as_bytes()
                        .chunks(2)
                        .map(|p| ((p[0] - b'0') << 4) | (p[1] - b'0'))
                        .collect();
                    if dt.coded.order == ByteOrder::Little {
                        bytes.reverse();
                    }
                    out.extend(bytes);
                    return Ok(());
                }
                let v = self.check_range(&path, raw, dt.coded.bits, dt.coded.enc == Enc::Signed)?;
                out.extend(write_uint(v, ebytes, dt.coded.order));
            }
            Shape::Array { count } => match (given, forced.get(&path)) {
                // A raw slot that counts elements.
                (None, Some(n)) if ebytes * count as usize <= 8 => out.extend(write_uint(
                    *n as u128,
                    ebytes * count as usize,
                    ByteOrder::Big,
                )),
                _ => self.encode_array(
                    &path,
                    given,
                    ebytes * count as usize,
                    ebytes,
                    count,
                    count,
                    opts,
                    out,
                )?,
            },
            Shape::Rest { min, max } => {
                let n = match given {
                    Some(Value::Bytes(b)) => b.len(),
                    _ => 0,
                };
                self.encode_array(&path, given, n, ebytes, min, max, opts, out)?
            }
        }
        Ok(())
    }

    fn encode_float(
        &self,
        dt: &DataType,
        v: Option<&Value>,
        path: &str,
        opts: EncodeOptions,
        out: &mut Vec<u8>,
    ) -> Result<(), EncodeError> {
        let x = match v {
            Some(Value::Physical(p)) => *p,
            Some(Value::Raw(r)) => *r as f64,
            None if opts.fill => 0.0,
            None => return Err(EncodeError::Missing { path: path.into() }),
            _ => {
                return Err(EncodeError::Range {
                    path: path.into(),
                    detail: "a float needs a number".into(),
                })
            }
        };
        let bits = match dt.coded.bits {
            32 => u128::from((x as f32).to_bits()),
            64 => u128::from(x.to_bits()),
            b => {
                return Err(EncodeError::Range {
                    path: path.into(),
                    detail: format!("a {b}-bit float is not defined"),
                })
            }
        };
        out.extend(write_uint(
            bits,
            (dt.coded.bits / 8) as usize,
            dt.coded.order,
        ));
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_array(
        &self,
        path: &str,
        v: Option<&Value>,
        total: usize,
        ebytes: usize,
        min: u32,
        max: u32,
        opts: EncodeOptions,
        out: &mut Vec<u8>,
    ) -> Result<(), EncodeError> {
        match v {
            Some(Value::Bytes(b)) => {
                if !b.len().is_multiple_of(ebytes) {
                    return Err(EncodeError::Range {
                        path: path.into(),
                        detail: format!(
                            "{} bytes are not a whole number of {ebytes}-byte elements",
                            b.len()
                        ),
                    });
                }
                let n = b.len() / ebytes;
                if n < min as usize || n > max as usize {
                    return Err(EncodeError::Range {
                        path: path.into(),
                        detail: format!("{n} elements, expected {min}..={max}"),
                    });
                }
                out.extend_from_slice(b);
                Ok(())
            }
            Some(Value::Text(t)) => {
                let b = t.as_bytes();
                if b.len() > total.max(max as usize * ebytes) {
                    return Err(EncodeError::Range {
                        path: path.into(),
                        detail: "the text is too long".into(),
                    });
                }
                let mut bytes = b.to_vec();
                bytes.resize(total.max(bytes.len()), 0);
                out.extend(bytes);
                Ok(())
            }
            Some(_) => Err(EncodeError::Range {
                path: path.into(),
                detail: "an array needs bytes".into(),
            }),
            None if opts.fill => {
                out.extend(std::iter::repeat_n(0u8, total.max(min as usize * ebytes)));
                Ok(())
            }
            None => Err(EncodeError::Missing { path: path.into() }),
        }
    }

    fn encode_bits(
        &self,
        b: &Bits,
        input: &Input,
        opts: EncodeOptions,
        prefix: &str,
        out: &mut Vec<u8>,
    ) -> Result<(), EncodeError> {
        let mut value: u128 = 0;
        for ch in &b.children {
            if let BitChild::Field {
                path,
                dt,
                offset,
                width,
                default,
                ..
            } = ch
            {
                let dtype = self.dt(*dt);
                let path = full(prefix, path);
                let raw =
                    self.scalar(input.values.get(&path), &path, Some(dtype), opts, *default)?;
                let v = self.check_range(&path, raw, *width, dtype.coded.enc == Enc::Signed)?;
                value |= v << offset;
            }
        }
        out.extend(write_uint(value, (b.bits / 8) as usize, b.order));
        Ok(())
    }

    // ---- finding the service a payload belongs to ------------------------------------------

    /// Services whose message of `kind` accepts `payload`, with the decoded values.
    pub fn identify<'s>(
        &self,
        services: &'s [Service],
        kind: MsgKind,
        payload: &[u8],
    ) -> Vec<(&'s Service, Decoded)> {
        let mut found = Vec::new();
        for s in services {
            let Some(msg) = s.slot(kind).message() else {
                continue;
            };
            let prefix = msg.prefix(&self.cdd.datatypes);
            if !prefix
                .iter()
                .all(|p| payload.get(p.offset).is_some_and(|b| b & p.mask == p.value))
            {
                continue;
            }
            if let Ok(d) = self.decode(msg, payload) {
                found.push((s, d));
            }
        }
        found
    }
}

/// How many elements of the repeat at `path` the input names: one more than the highest index.
fn repeat_len(input: &Input, path: &str) -> usize {
    let head = format!("{path}[");
    input
        .values
        .keys()
        .filter_map(|k| k.strip_prefix(&head))
        .filter_map(|rest| rest.split(']').next()?.parse::<usize>().ok())
        .max()
        .map_or(0, |m| m + 1)
}
