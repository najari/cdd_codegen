//! The layout IR: one message as an ordered list of items whose byte positions are fixed.
//! The interpreter and the code generator both read it, so they cannot disagree about where
//! a constant or a field sits.

use crate::datatype::{ByteOrder, DataTypes};

/// Index into [`DataTypes::items`].
pub type DtId = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MsgKind {
    Request,
    Positive,
    Negative,
}

impl MsgKind {
    pub fn label(self) -> &'static str {
        match self {
            MsgKind::Request => "REQ",
            MsgKind::Positive => "POS",
            MsgKind::Negative => "NEG",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub kind: MsgKind,
    pub qual: String,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Const(Const),
    Field(Field),
    /// A record (`STRUCTDT`, a DID's structure): named items laid out one after another.
    Group(Group),
    /// A `STRUCT`: an integer container of bit fields.
    Bits(Bits),
    Repeat(Repeat),
    Mux(Mux),
    /// Reserved bytes.
    Gap(u32),
    /// The negative response code of a negative response.
    Nrc(NrcSlot),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Sid,
    Sub,
    /// A data, local or routine identifier.
    Id,
    Other,
}

/// A value fixed by the protocol or by the service instance: the SID, a sub-function, a DID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Const {
    pub name: String,
    /// Unique path in the message (see [`assign_paths`]).
    pub path: String,
    pub role: Role,
    pub value: u64,
    /// A multiple of 8.
    pub bits: u32,
    pub order: ByteOrder,
    /// Which bits of a received value must equal `value` (all ones, or `0x7F` for a
    /// sub-function that carries the suppress-positive-response bit).
    pub mask: u64,
    /// The sub-function has the suppress-positive-response bit (`respsupbit`).
    pub suppress_bit: bool,
}

impl Const {
    pub fn bytes(&self) -> usize {
        (self.bits / 8) as usize
    }

    /// The value as it goes on the wire.
    pub fn wire_bytes(&self) -> Vec<u8> {
        let n = self.bytes();
        let be = self.value.to_be_bytes();
        let mut v = be[8 - n.min(8)..].to_vec();
        if self.order == ByteOrder::Little {
            v.reverse();
        }
        v
    }

    /// The mask as it applies to each wire byte.
    pub fn wire_mask(&self) -> Vec<u8> {
        let n = self.bytes();
        let be = self.mask.to_be_bytes();
        let mut v = be[8 - n.min(8)..].to_vec();
        if self.order == ByteOrder::Little {
            v.reverse();
        }
        v
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// One value.
    Atom,
    /// A fixed number of elements.
    Array { count: u32 },
    /// As many elements as the rest of the message holds, `min..=max` of them.
    Rest { min: u32, max: u32 },
}

/// A parameter: a slot the caller fills (request) or reads (response).
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: String,
    pub path: String,
    /// `None`: raw bytes without a data type.
    pub dt: Option<DtId>,
    pub shape: Shape,
    pub default: Option<i128>,
}

impl Field {
    pub fn elem_bits(&self, dts: &DataTypes) -> u32 {
        self.dt.map_or(8, |d| dts.get(d).coded.bits)
    }

    pub fn elem_bytes(&self, dts: &DataTypes) -> usize {
        (self.elem_bits(dts) / 8) as usize
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub name: String,
    pub path: String,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BitChild {
    Field {
        name: String,
        path: String,
        dt: DtId,
        offset: u32,
        width: u32,
        default: Option<i128>,
    },
    Gap {
        offset: u32,
        width: u32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bits {
    pub name: String,
    pub path: String,
    /// Bits of the container integer; a multiple of 8, at most 64.
    pub bits: u32,
    pub order: ByteOrder,
    /// From the least significant bit up. Bits above the last child are an implicit gap.
    pub children: Vec<BitChild>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RepeatCount {
    /// To the end of the message.
    ToEnd { min: u32, max: Option<u32> },
    /// As often as an earlier field says (after masking).
    Counted { field: Vec<String>, mask: u64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Repeat {
    pub name: String,
    pub path: String,
    /// Paths of the element's items are relative to the element: the item `DTC` of element 2 of
    /// the repeat `List` is `List[2].DTC`.
    pub element: Vec<Item>,
    pub count: RepeatCount,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MuxCase {
    pub lo: i128,
    pub hi: i128,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mux {
    pub name: String,
    pub path: String,
    /// Path of the earlier field whose value selects the case.
    pub selector: Vec<String>,
    pub mask: u64,
    /// The structure when no case holds the selector value (possibly empty).
    pub default: Vec<Item>,
    pub cases: Vec<MuxCase>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NrcSlot {
    pub name: String,
    pub path: String,
}

/// Hands out unique paths: the second item called `Data` of a message is `Data~2`.
#[derive(Clone, Debug, Default)]
pub struct PathNamer {
    seen: std::collections::HashMap<String, usize>,
    /// The path most recently given to each plain name: a count or a selector refers to an
    /// earlier item by name.
    latest: std::collections::HashMap<String, String>,
}

impl PathNamer {
    pub fn unique(&mut self, path: String) -> String {
        let n = self.seen.entry(path.clone()).or_insert(0);
        *n += 1;
        if *n == 1 {
            path
        } else {
            format!("{path}~{n}")
        }
    }

    fn named(&mut self, name: &str, path: String) -> String {
        self.latest.insert(name.to_string(), path.clone());
        path
    }
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}.{name}")
    }
}

/// Gives every item of a message its unique path. Both the interpreter and the code generator use
/// these strings, so they name a value the same way.
pub fn assign_paths(items: &mut [Item]) {
    assign(items, "", &mut PathNamer::default());
}

fn assign(items: &mut [Item], prefix: &str, namer: &mut PathNamer) {
    for item in items {
        match item {
            Item::Const(c) => {
                let p = namer.unique(join(prefix, &c.name));
                c.path = namer.named(&c.name, p);
            }
            Item::Field(f) => {
                let p = namer.unique(join(prefix, &f.name));
                f.path = namer.named(&f.name, p);
            }
            Item::Nrc(n) => {
                let p = namer.unique(join(prefix, &n.name));
                n.path = namer.named(&n.name, p);
            }
            Item::Group(g) => {
                g.path = join(prefix, &g.name);
                let p = g.path.clone();
                assign(&mut g.items, &p, namer);
            }
            Item::Bits(b) => {
                b.path = join(prefix, &b.name);
                for ch in &mut b.children {
                    if let BitChild::Field { name, path, .. } = ch {
                        let p = namer.unique(join(&b.path, name));
                        *path = namer.named(name, p);
                    }
                }
            }
            Item::Repeat(r) => {
                r.path = namer.unique(join(prefix, &r.name));
                if let RepeatCount::Counted { field, .. } = &mut r.count {
                    if let Some(p) = field.first().and_then(|n| namer.latest.get(n)) {
                        *field = vec![p.clone()];
                    }
                }
                // The element is its own scope: its paths are relative.
                assign(&mut r.element, "", &mut PathNamer::default());
            }
            Item::Mux(m) => {
                m.path = join(prefix, &m.name);
                let p = m.path.clone();
                if let Some(path) = m.selector.first().and_then(|n| namer.latest.get(n)) {
                    m.selector = vec![path.clone()];
                }
                // The default and the cases are alternatives: each names its items as if the others were
                // not there.
                assign(&mut m.default, &p, &mut namer.clone());
                for case in &mut m.cases {
                    assign(&mut case.items, &p, &mut namer.clone());
                }
            }
            Item::Gap(_) => {}
        }
    }
}

/// A byte every message of a kind starts with at a known position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrefixByte {
    pub offset: usize,
    pub value: u8,
    pub mask: u8,
}

impl Item {
    /// Bytes on the wire when the item never varies.
    pub fn fixed_bytes(&self, dts: &DataTypes) -> Option<usize> {
        match self {
            Item::Const(c) => Some(c.bytes()),
            Item::Field(f) => match f.shape {
                Shape::Atom => Some(f.elem_bytes(dts)),
                Shape::Array { count } => Some(f.elem_bytes(dts) * count as usize),
                Shape::Rest { .. } => None,
            },
            Item::Group(g) => items_fixed_bytes(&g.items, dts),
            Item::Bits(b) => Some((b.bits / 8) as usize),
            Item::Gap(n) => Some(*n as usize),
            Item::Nrc(_) => Some(1),
            Item::Repeat(_) | Item::Mux(_) => None,
        }
    }

    /// Smallest and largest size on the wire. `None` for the largest: unbounded.
    pub fn size_range(&self, dts: &DataTypes) -> (usize, Option<usize>) {
        if let Some(n) = self.fixed_bytes(dts) {
            return (n, Some(n));
        }
        match self {
            Item::Field(f) => {
                let e = f.elem_bytes(dts);
                match f.shape {
                    Shape::Rest { min, max } => (min as usize * e, Some(max as usize * e)),
                    _ => (0, None),
                }
            }
            Item::Group(g) => items_size_range(&g.items, dts),
            Item::Repeat(r) => {
                let (emin, emax) = items_size_range(&r.element, dts);
                match &r.count {
                    RepeatCount::ToEnd { min, max } => (
                        *min as usize * emin,
                        max.and_then(|m| emax.map(|e| m as usize * e)),
                    ),
                    RepeatCount::Counted { mask, .. } => {
                        (0, emax.map(|e| (*mask).min(65_535) as usize * e))
                    }
                }
            }
            Item::Mux(m) => {
                // The smallest and largest of the default and every case.
                let (mut lo, mut hi) = items_size_range(&m.default, dts);
                for case in &m.cases {
                    let (a, b) = items_size_range(&case.items, dts);
                    lo = lo.min(a);
                    hi = match (hi, b) {
                        (Some(h), Some(b)) => Some(h.max(b)),
                        _ => None,
                    };
                }
                (lo, hi)
            }
            _ => (0, None),
        }
    }
}

pub fn items_fixed_bytes(items: &[Item], dts: &DataTypes) -> Option<usize> {
    items
        .iter()
        .try_fold(0usize, |acc, i| i.fixed_bytes(dts).map(|n| acc + n))
}

pub fn items_size_range(items: &[Item], dts: &DataTypes) -> (usize, Option<usize>) {
    let mut lo = 0usize;
    let mut hi = Some(0usize);
    for i in items {
        let (a, b) = i.size_range(dts);
        lo += a;
        hi = match (hi, b) {
            (Some(h), Some(b)) => Some(h + b),
            _ => None,
        };
    }
    (lo, hi)
}

impl Message {
    pub fn len_range(&self, dts: &DataTypes) -> (usize, Option<usize>) {
        items_size_range(&self.items, dts)
    }

    /// The constant bytes at the front of the message, up to the first item whose size varies.
    /// Constants after a fixed-size field are included at their offset.
    pub fn prefix(&self, dts: &DataTypes) -> Vec<PrefixByte> {
        let mut out = Vec::new();
        let mut offset = 0usize;
        for item in &self.items {
            match item {
                Item::Const(c) => {
                    for (i, (b, m)) in c.wire_bytes().into_iter().zip(c.wire_mask()).enumerate() {
                        out.push(PrefixByte {
                            offset: offset + i,
                            value: b & m,
                            mask: m,
                        });
                    }
                    offset += c.bytes();
                }
                other => match other.fixed_bytes(dts) {
                    Some(n) => offset += n,
                    None => break,
                },
            }
        }
        out
    }

    /// The SID: the first constant byte of a request or a negative response's second byte.
    pub fn first_const(&self) -> Option<&Const> {
        self.items.iter().find_map(|i| {
            if let Item::Const(c) = i {
                Some(c)
            } else {
                None
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str, role: Role, value: u64, bits: u32) -> Item {
        Item::Const(Const {
            name: name.into(),
            path: String::new(),
            role,
            value,
            bits,
            order: ByteOrder::Big,
            mask: u64::MAX >> (64 - bits),
            suppress_bit: false,
        })
    }

    #[test]
    fn prefix_covers_constants_up_to_the_first_variable_item() {
        let dts = DataTypes::default();
        let msg = Message {
            kind: MsgKind::Request,
            qual: "X".into(),
            items: vec![
                c("sid", Role::Sid, 0x22, 8),
                c("did", Role::Id, 0xF190, 16),
                Item::Field(Field {
                    name: "data".into(),
                    path: String::new(),
                    dt: None,
                    shape: Shape::Rest { min: 0, max: 255 },
                    default: None,
                }),
                c("late", Role::Other, 0x55, 8),
            ],
        };
        let p = msg.prefix(&dts);
        assert_eq!(
            p.iter().map(|b| (b.offset, b.value)).collect::<Vec<_>>(),
            vec![(0, 0x22), (1, 0xF1), (2, 0x90)]
        );
        // 3 constant bytes, 0..=255 raw bytes, 1 late constant byte.
        assert_eq!(msg.len_range(&dts), (4, Some(259)));
    }

    #[test]
    fn little_endian_constants_and_masks_follow_the_wire_order() {
        let k = Const {
            name: "x".into(),
            path: String::new(),
            role: Role::Id,
            value: 0x1234,
            bits: 16,
            order: ByteOrder::Little,
            mask: 0xFF00,
            suppress_bit: false,
        };
        assert_eq!(k.wire_bytes(), vec![0x34, 0x12]);
        assert_eq!(k.wire_mask(), vec![0x00, 0xFF]);
    }

    #[test]
    fn suppress_bit_sub_function_masks_the_top_bit() {
        let dts = DataTypes::default();
        let msg = Message {
            kind: MsgKind::Request,
            qual: "X".into(),
            items: vec![
                c("sid", Role::Sid, 0x19, 8),
                Item::Const(Const {
                    name: "sub".into(),
                    path: String::new(),
                    role: Role::Sub,
                    value: 0x02,
                    bits: 8,
                    order: ByteOrder::Big,
                    mask: 0x7F,
                    suppress_bit: true,
                }),
            ],
        };
        let p = msg.prefix(&dts);
        assert_eq!(
            p[1],
            PrefixByte {
                offset: 1,
                value: 0x02,
                mask: 0x7F
            }
        );
    }
}
