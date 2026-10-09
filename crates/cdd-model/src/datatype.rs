//! Data types: how a value looks on the wire (`CVALUETYPE`) and what it means (`PVALUETYPE`,
//! text tables, linear conversions, records).

use crate::error::{codes, Issue};
use crate::xml::{
    attr_f64, attr_int, kid, kids, kids_named, local_text, qual, tag, text_of, tuv_list, LocalText,
};
use roxmltree::Node;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ByteOrder {
    /// `bo='21'`: the most significant byte comes first.
    Big,
    /// `bo='12'`: the least significant byte comes first.
    Little,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Enc {
    Unsigned,
    Signed,
    /// Packed BCD, most significant nibble first.
    Bcd,
    Ascii,
    Utf,
    Float,
    Double,
}

impl Enc {
    fn from_attr(s: &str) -> Option<Enc> {
        Some(match s {
            "uns" => Enc::Unsigned,
            "sgn" => Enc::Signed,
            "bcd" => Enc::Bcd,
            "asc" => Enc::Ascii,
            "utf" => Enc::Utf,
            "flt" => Enc::Float,
            "dbl" => Enc::Double,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Qty {
    /// One value of `bits` bits.
    Atom,
    /// An array of `min..=max` elements of `bits` bits each.
    Field,
}

/// The wire form of a value (`CVALUETYPE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Coded {
    /// Bits of one element.
    pub bits: u32,
    pub order: ByteOrder,
    pub enc: Enc,
    pub qty: Qty,
    pub min: u32,
    pub max: u32,
}

/// The physical form of a value (`PVALUETYPE`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Phys {
    pub enc: Enc,
    pub bits: u32,
    pub unit: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextEntry {
    pub lo: i128,
    pub hi: i128,
    pub text: LocalText,
}

/// One `COMP`: physical = f / div * raw + o, for raw in `s..=e` when those are given.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Comp {
    pub s: Option<i128>,
    pub e: Option<i128>,
    pub f: f64,
    pub div: f64,
    pub o: f64,
}

impl Comp {
    fn covers(&self, raw: i128) -> bool {
        self.s.is_none_or(|s| raw >= s) && self.e.is_none_or(|e| raw <= e)
    }

    fn factor(&self) -> f64 {
        self.f / self.div
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Linear {
    pub comps: Vec<Comp>,
}

/// A data object of a record, a DID or a message part.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjMember {
    pub name: String,
    /// `id`: lets another object name this one in `dataObjectRef`.
    pub id: Option<String>,
    /// `dtref`: the id of the data type.
    pub dtref: Option<String>,
    /// `def`: the default raw value.
    pub default: Option<i128>,
    /// `dataObjectRef`: another object whose value selects this one's layout.
    pub object_ref: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BitChildMember {
    Obj(ObjMember),
    Gap { bits: Option<u32> },
    Other(String),
}

/// A `STRUCT`: an integer container whose children are bit fields.
#[derive(Clone, Debug, PartialEq)]
pub struct BitsMember {
    pub name: String,
    pub dtref: Option<String>,
    pub children: Vec<BitChildMember>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Member {
    Obj(ObjMember),
    Bits(BitsMember),
    /// `GAPDATAOBJ`: reserved space.
    Gap {
        bits: Option<u32>,
    },
    /// A DID reference inside a structure (`DIDREF`): the id of the DID.
    DidRef {
        name: String,
        did: Option<String>,
    },
    /// Anything the model does not read: its tag.
    Other {
        tag: String,
        name: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct MuxCase {
    pub lo: i128,
    pub hi: i128,
    pub members: Vec<Member>,
}

/// A `MUXDT`: a structure chosen by the value of an earlier object. The first `STRUCTURE` is the
/// default, used when no `CASE` holds the value (ODX calls it `DEFAULT-CASE`): in the memory
/// services every case is a whole layout of its own and the default equals one of them.
#[derive(Clone, Debug, PartialEq)]
pub struct MuxDef {
    /// `bm`: the bits of the selector that choose the case.
    pub mask: u64,
    pub default: Vec<Member>,
    pub cases: Vec<MuxCase>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DtKind {
    Ident,
    Text(Vec<TextEntry>),
    Linear(Linear),
    /// `STRUCTDT`.
    Record(Vec<Member>),
    Mux(MuxDef),
    Unsupported(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct DataType {
    pub id: String,
    pub qual: String,
    pub name: LocalText,
    pub coded: Coded,
    pub phys: Phys,
    pub kind: DtKind,
}

impl DataType {
    /// The text of the entry that holds `raw`.
    pub fn text_for(&self, raw: i128) -> Option<&LocalText> {
        match &self.kind {
            DtKind::Text(entries) => entries
                .iter()
                .find(|e| raw >= e.lo && raw <= e.hi)
                .map(|e| &e.text),
            _ => None,
        }
    }

    /// The physical value of `raw` for a linear conversion.
    pub fn physical(&self, raw: i128) -> Option<f64> {
        let DtKind::Linear(lin) = &self.kind else {
            return None;
        };
        let comp = lin.comps.iter().find(|c| c.covers(raw))?;
        Some(comp.factor() * raw as f64 + comp.o)
    }

    /// The raw value nearest to the physical value `p`. `None` when no conversion reaches it.
    pub fn raw_from_physical(&self, p: f64) -> Option<i128> {
        let DtKind::Linear(lin) = &self.kind else {
            return None;
        };
        if !p.is_finite() {
            return None;
        }
        for comp in &lin.comps {
            let factor = comp.factor();
            if factor == 0.0 || !factor.is_finite() {
                continue;
            }
            let raw = ((p - comp.o) / factor).round();
            if raw.abs() > 1e30 {
                continue;
            }
            let raw = raw as i128;
            if comp.covers(raw) {
                return Some(raw);
            }
        }
        None
    }

    pub fn is_text_table(&self) -> bool {
        matches!(self.kind, DtKind::Text(_))
    }
}

/// All data types of a document, addressable by `id` or by index.
#[derive(Clone, Debug, Default)]
pub struct DataTypes {
    pub items: Vec<DataType>,
    by_id: HashMap<String, usize>,
}

impl DataTypes {
    pub fn index(&self, id: &str) -> Option<usize> {
        self.by_id.get(id).copied()
    }

    pub fn get(&self, index: usize) -> &DataType {
        &self.items[index]
    }

    pub fn by_id(&self, id: &str) -> Option<&DataType> {
        self.index(id).map(|i| &self.items[i])
    }

    fn push(&mut self, dt: DataType) {
        self.by_id.insert(dt.id.clone(), self.items.len());
        self.items.push(dt);
    }

    /// Registers the text tables that `GODTCDATAOBJ` and `RECORDDATAOBJ` define inline. They come
    /// after the document's own data types, so no index moves.
    pub fn add_inline(&mut self, ecudoc: Node<'_, '_>, issues: &mut Vec<Issue>) {
        for owner in ecudoc
            .descendants()
            .filter(|n| matches!(tag(*n), "GODTCDATAOBJ" | "RECORDDATAOBJ"))
        {
            let inline = if tag(owner) == "GODTCDATAOBJ" {
                "TEXTTBL"
            } else {
                "RECORDDT"
            };
            let Some(n) = kid(owner, inline) else {
                continue;
            };
            let Some(id) = n.attribute("id") else {
                continue;
            };
            if self.by_id.contains_key(id) {
                continue;
            }
            let q = qual(n).unwrap_or_else(|| id.to_string());
            match parse_datatype(n, &q, id) {
                Ok(dt) => self.push(dt),
                Err(issue) => issues.push(issue.at(format!("inline data type {q}"))),
            }
        }
    }

    /// Reads every child of `ECUDOC/DATATYPES`.
    pub fn parse(datatypes: Node<'_, '_>, issues: &mut Vec<Issue>) -> DataTypes {
        let mut out = DataTypes::default();
        for n in kids(datatypes) {
            let Some(id) = n.attribute("id") else {
                continue;
            };
            let q = qual(n).unwrap_or_else(|| id.to_string());
            match parse_datatype(n, &q, id) {
                Ok(dt) => out.push(dt),
                Err(issue) => {
                    issues.push(issue.at(format!("data type {q}")));
                    // Keep a placeholder so references still resolve to something.
                    out.push(DataType {
                        id: id.to_string(),
                        qual: q,
                        name: local_text(n, "NAME"),
                        coded: Coded {
                            bits: 8,
                            order: ByteOrder::Big,
                            enc: Enc::Unsigned,
                            qty: Qty::Atom,
                            min: 1,
                            max: 1,
                        },
                        phys: Phys {
                            enc: Enc::Unsigned,
                            bits: 8,
                            unit: None,
                        },
                        kind: DtKind::Unsupported(tag(n).to_string()),
                    });
                }
            }
        }
        out
    }
}

fn parse_coded(v: Node<'_, '_>) -> Result<Coded, Issue> {
    let bits = attr_int(v, "bl")
        .filter(|b| *b > 0 && *b <= u32::MAX as i128)
        .ok_or_else(|| {
            Issue::error(
                codes::BAD_NUMBER,
                format!(
                    "`bl` is missing or not a positive number: {:?}",
                    v.attribute("bl")
                ),
            )
        })? as u32;
    let order = match v.attribute("bo") {
        Some("21") | None => ByteOrder::Big,
        Some("12") => ByteOrder::Little,
        Some(other) => {
            return Err(Issue::error(
                codes::BAD_BYTE_ORDER,
                format!("byte order `{other}` is neither 21 nor 12"),
            ));
        }
    };
    let enc = match v.attribute("enc") {
        None => Enc::Unsigned,
        Some(e) => Enc::from_attr(e).ok_or_else(|| {
            Issue::error(
                codes::UNSUPPORTED_DATATYPE,
                format!("encoding `{e}` is not known"),
            )
        })?,
    };
    let qty = if v.attribute("qty") == Some("field") {
        Qty::Field
    } else {
        Qty::Atom
    };
    let min = attr_int(v, "minsz").unwrap_or(1).clamp(0, u32::MAX as i128) as u32;
    let max = attr_int(v, "maxsz")
        .unwrap_or(min as i128)
        .clamp(0, u32::MAX as i128) as u32;
    Ok(Coded {
        bits,
        order,
        enc,
        qty,
        min,
        max,
    })
}

fn parse_phys(n: Node<'_, '_>, coded: &Coded) -> Phys {
    let Some(p) = kid(n, "PVALUETYPE") else {
        return Phys {
            enc: coded.enc,
            bits: coded.bits,
            unit: None,
        };
    };
    let enc = p
        .attribute("enc")
        .and_then(Enc::from_attr)
        .unwrap_or(coded.enc);
    let bits = attr_int(p, "bl")
        .unwrap_or(coded.bits as i128)
        .clamp(1, u32::MAX as i128) as u32;
    let unit = kid(p, "UNIT").map(text_of).filter(|u| !u.is_empty());
    Phys { enc, bits, unit }
}

pub(crate) fn parse_datatype(n: Node<'_, '_>, q: &str, id: &str) -> Result<DataType, Issue> {
    let cv = kid(n, "CVALUETYPE").ok_or_else(|| {
        Issue::error(
            codes::UNSUPPORTED_DATATYPE,
            "the data type has no CVALUETYPE",
        )
    })?;
    let coded = parse_coded(cv)?;
    let phys = parse_phys(n, &coded);
    let kind = match tag(n) {
        "IDENT" => DtKind::Ident,
        "TEXTTBL" => DtKind::Text(
            kids_named(n, "TEXTMAP")
                .filter_map(|m| {
                    let lo = attr_int(m, "s")?;
                    let hi = attr_int(m, "e").unwrap_or(lo);
                    let text = kid(m, "TEXT").map(tuv_list).unwrap_or_default();
                    Some(TextEntry { lo, hi, text })
                })
                .collect(),
        ),
        // A code table: every `RECORD v` names one value (the DTC numbers of a fault memory).
        "RECORDDT" => DtKind::Text(
            kids_named(n, "RECORD")
                .filter_map(|r| {
                    let v = attr_int(r, "v")?;
                    Some(TextEntry {
                        lo: v,
                        hi: v,
                        text: kid(r, "TEXT").map(tuv_list).unwrap_or_default(),
                    })
                })
                .collect(),
        ),
        "LINCOMP" => DtKind::Linear(Linear {
            comps: kids_named(n, "COMP")
                .map(|c| Comp {
                    s: attr_int(c, "s"),
                    e: attr_int(c, "e"),
                    f: attr_f64(c, "f").unwrap_or(1.0),
                    div: attr_f64(c, "div").unwrap_or(1.0),
                    o: attr_f64(c, "o").unwrap_or(0.0),
                })
                .collect(),
        }),
        "STRUCTDT" => DtKind::Record(parse_members(n)),
        "MUXDT" => {
            let default = kid(n, "STRUCTURE").map(parse_members).unwrap_or_default();
            let cases = kids_named(n, "CASE")
                .filter_map(|c| {
                    let lo = attr_int(c, "s")?;
                    let hi = attr_int(c, "e").unwrap_or(lo);
                    let members = kid(c, "STRUCTURE").map(parse_members).unwrap_or_default();
                    Some(MuxCase { lo, hi, members })
                })
                .collect();
            let mask = attr_int(n, "bm")
                .filter(|m| *m > 0 && *m <= i128::from(u64::MAX))
                .map_or(u64::MAX, |m| m as u64);
            DtKind::Mux(MuxDef {
                mask,
                default,
                cases,
            })
        }
        other => DtKind::Unsupported(other.to_string()),
    };
    Ok(DataType {
        id: id.to_string(),
        qual: q.to_string(),
        name: local_text(n, "NAME"),
        coded,
        phys,
        kind,
    })
}

fn obj_member(n: Node<'_, '_>) -> ObjMember {
    ObjMember {
        name: qual(n).unwrap_or_default(),
        id: n.attribute("id").map(str::to_string),
        dtref: n.attribute("dtref").map(str::to_string),
        default: attr_int(n, "def"),
        object_ref: n.attribute("dataObjectRef").map(str::to_string),
    }
}

/// The data members below `container`, in document order. Metadata children are skipped.
pub fn parse_members(container: Node<'_, '_>) -> Vec<Member> {
    let mut out = Vec::new();
    for c in kids(container) {
        match tag(c) {
            "DATAOBJ" => out.push(Member::Obj(obj_member(c))),
            "GAPDATAOBJ" => out.push(Member::Gap {
                bits: attr_int(c, "bl").map(|b| b.clamp(0, u32::MAX as i128) as u32),
            }),
            "STRUCT" => out.push(Member::Bits(BitsMember {
                name: qual(c).unwrap_or_default(),
                dtref: c.attribute("dtref").map(str::to_string),
                children: kids(c)
                    .filter_map(|k| match tag(k) {
                        "DATAOBJ" => Some(BitChildMember::Obj(obj_member(k))),
                        "GAPDATAOBJ" => Some(BitChildMember::Gap {
                            bits: attr_int(k, "bl").map(|b| b.clamp(0, u32::MAX as i128) as u32),
                        }),
                        "NAME" | "DESC" | "QUAL" => None,
                        other => Some(BitChildMember::Other(other.to_string())),
                    })
                    .collect(),
            })),
            "DIDREF" | "DIDDATAREF" => out.push(Member::DidRef {
                name: qual(c).unwrap_or_default(),
                did: c.attribute("didRef").map(str::to_string),
            }),
            // These carry their own text table (the DTC groups, the DTC numbers): a data object whose
            // type is defined inline.
            "GODTCDATAOBJ" | "RECORDDATAOBJ" => {
                let inline = if tag(c) == "GODTCDATAOBJ" {
                    "TEXTTBL"
                } else {
                    "RECORDDT"
                };
                out.push(Member::Obj(ObjMember {
                    name: qual(c).unwrap_or_default(),
                    id: None,
                    dtref: kid(c, inline)
                        .and_then(|t| t.attribute("id"))
                        .map(str::to_string),
                    default: None,
                    object_ref: None,
                }))
            }
            "SPECDATAOBJ" => out.push(Member::Other {
                tag: tag(c).to_string(),
                name: qual(c).unwrap_or_default(),
            }),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_one(xml: &str) -> Result<DataType, Issue> {
        let doc = roxmltree::Document::parse(xml).unwrap();
        let root = doc.root_element();
        parse_datatype(root, "T", "_1")
    }

    #[test]
    fn linear_conversion_both_ways() {
        let dt = parse_one(
            "<LINCOMP id='_1'><CVALUETYPE bl='8' bo='21' enc='uns' qty='atom'/>\
             <PVALUETYPE bl='64' enc='dbl'><UNIT>V</UNIT></PVALUETYPE><COMP f='0.1' o='0'/></LINCOMP>",
        )
        .unwrap();
        assert_eq!(dt.phys.unit.as_deref(), Some("V"));
        assert!((dt.physical(125).unwrap() - 12.5).abs() < 1e-12);
        assert_eq!(dt.raw_from_physical(12.5), Some(125));
        assert_eq!(dt.raw_from_physical(f64::NAN), None);
    }

    #[test]
    fn linear_conversion_with_divisor_offset_and_ranges() {
        let dt = parse_one(
            "<LINCOMP id='_1'><CVALUETYPE bl='16' bo='21' enc='uns'/>\
             <COMP s='0' e='99' f='1' div='10' o='-5'/><COMP s='100' e='65535' f='2' o='0'/></LINCOMP>",
        )
        .unwrap();
        assert!((dt.physical(50).unwrap() - 0.0).abs() < 1e-12);
        assert!((dt.physical(100).unwrap() - 200.0).abs() < 1e-12);
        assert_eq!(dt.raw_from_physical(0.0), Some(50));
        assert_eq!(dt.raw_from_physical(200.0), Some(100));
    }

    #[test]
    fn text_table_ranges() {
        let dt = parse_one(
            "<TEXTTBL id='_1'><CVALUETYPE bl='8'/><TEXTMAP s='0' e='0'><TEXT><TUV xml:lang='en-US'>off</TUV></TEXT></TEXTMAP>\
             <TEXTMAP s='1' e='255'><TEXT><TUV xml:lang='en-US'>on</TUV></TEXT></TEXTMAP></TEXTTBL>",
        )
        .unwrap();
        assert_eq!(dt.text_for(0).unwrap().pick(&[]), Some("off"));
        assert_eq!(dt.text_for(77).unwrap().pick(&[]), Some("on"));
        assert!(dt.text_for(256).is_none());
    }

    #[test]
    fn a_bad_byte_order_is_a_diagnostic_not_a_panic() {
        let err = parse_one("<IDENT id='_1'><CVALUETYPE bl='8' bo='4321'/></IDENT>").unwrap_err();
        assert_eq!(err.code, codes::BAD_BYTE_ORDER);
    }

    #[test]
    fn little_endian_and_arrays() {
        let dt = parse_one("<IDENT id='_1'><CVALUETYPE bl='8' bo='12' enc='uns' qty='field' minsz='2' maxsz='9'/></IDENT>").unwrap();
        assert_eq!(dt.coded.order, ByteOrder::Little);
        assert_eq!(
            (dt.coded.qty, dt.coded.min, dt.coded.max),
            (Qty::Field, 2, 9)
        );
    }
}
