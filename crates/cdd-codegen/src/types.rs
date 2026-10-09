//! Value types: what a field of a data type is in Rust.
//!
//! A plain `IDENT` is just an integer. A text table (`TEXTTBL`) becomes an enum when every entry
//! is one value, and a newtype with a `label()` when an entry covers a range (so a value that was
//! read can be written back unchanged). A linear conversion (`LINCOMP`) becomes a newtype with
//! `physical()` and `from_physical()`.

use crate::names::{self, Scope};
use cdd_model::datatype::{DataType, DtKind, Enc, TextEntry};
use cdd_model::ir::DtId;
use cdd_model::Cdd;
use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};
use std::collections::{HashMap, HashSet};
use syn::Ident;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prim {
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
}

impl Prim {
    pub fn tokens(self) -> TokenStream {
        match self {
            Prim::U8 => quote!(u8),
            Prim::U16 => quote!(u16),
            Prim::U32 => quote!(u32),
            Prim::U64 => quote!(u64),
            Prim::I8 => quote!(i8),
            Prim::I16 => quote!(i16),
            Prim::I32 => quote!(i32),
            Prim::I64 => quote!(i64),
            Prim::F32 => quote!(f32),
            Prim::F64 => quote!(f64),
        }
    }

    pub fn bits(self) -> u32 {
        match self {
            Prim::U8 | Prim::I8 => 8,
            Prim::U16 | Prim::I16 => 16,
            Prim::U32 | Prim::I32 | Prim::F32 => 32,
            Prim::U64 | Prim::I64 | Prim::F64 => 64,
        }
    }

    pub fn is_signed(self) -> bool {
        matches!(self, Prim::I8 | Prim::I16 | Prim::I32 | Prim::I64)
    }

    pub fn is_float(self) -> bool {
        matches!(self, Prim::F32 | Prim::F64)
    }

    pub fn unsigned_for(bits: u32) -> Prim {
        match bits {
            0..=8 => Prim::U8,
            9..=16 => Prim::U16,
            17..=32 => Prim::U32,
            _ => Prim::U64,
        }
    }

    pub fn signed_for(bits: u32) -> Prim {
        match bits {
            0..=8 => Prim::I8,
            9..=16 => Prim::I16,
            17..=32 => Prim::I32,
            _ => Prim::I64,
        }
    }

    /// The range of raw values the type holds.
    fn range(self) -> (i128, i128) {
        match self {
            Prim::U8 => (0, i128::from(u8::MAX)),
            Prim::U16 => (0, i128::from(u16::MAX)),
            Prim::U32 => (0, i128::from(u32::MAX)),
            Prim::U64 => (0, i128::from(u64::MAX)),
            Prim::I8 => (i128::from(i8::MIN), i128::from(i8::MAX)),
            Prim::I16 => (i128::from(i16::MIN), i128::from(i16::MAX)),
            Prim::I32 => (i128::from(i32::MIN), i128::from(i32::MAX)),
            Prim::I64 => (i128::from(i64::MIN), i128::from(i64::MAX)),
            Prim::F32 | Prim::F64 => (0, 0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedKind {
    Enum,
    Range,
    Linear,
}

#[derive(Clone, Debug)]
pub struct Named {
    /// How generated code names the type: `types::Name`.
    pub path: TokenStream,
    pub kind: NamedKind,
}

/// How a field of one data type is held in Rust.
#[derive(Clone, Debug)]
pub struct ValueTy {
    /// The type of the struct field.
    pub ty: TokenStream,
    /// The primitive the wire value is read into (and the raw value of a named type).
    pub prim: Prim,
    /// The value is packed BCD (the primitive holds the decimal number).
    pub bcd: bool,
    pub named: Option<Named>,
}

/// The primitive a data type's raw value is held in.
pub fn prim_of(dt: &DataType) -> Result<(Prim, bool), String> {
    let width = dt.coded.bits;
    match dt.coded.enc {
        Enc::Unsigned | Enc::Ascii | Enc::Utf => {
            if width > 64 {
                return Err(format!("{width}-bit integers are not supported"));
            }
            Ok((Prim::unsigned_for(width), false))
        }
        Enc::Signed => {
            if width > 64 {
                return Err(format!("{width}-bit integers are not supported"));
            }
            Ok((Prim::signed_for(width), false))
        }
        Enc::Float | Enc::Double => match width {
            32 => Ok((Prim::F32, false)),
            64 => Ok((Prim::F64, false)),
            w => Err(format!("a {w}-bit float is not defined")),
        },
        Enc::Bcd => {
            let digits = width / 8 * 2;
            if digits > 19 {
                return Err(format!(
                    "a BCD number of {digits} digits does not fit 64 bits"
                ));
            }
            Ok((
                match digits {
                    0..=2 => Prim::U8,
                    3..=4 => Prim::U16,
                    5..=9 => Prim::U32,
                    _ => Prim::U64,
                },
                true,
            ))
        }
    }
}

fn lit(v: i128) -> Literal {
    Literal::i128_unsuffixed(v)
}

fn opt_str(s: Option<&str>) -> TokenStream {
    match s {
        Some(s) => quote!(Some(#s)),
        None => quote!(None),
    }
}

pub struct Types<'c> {
    cdd: &'c Cdd,
    langs: Vec<String>,
    rt: TokenStream,
    scope: Scope,
    by_dt: HashMap<DtId, ValueTy>,
    /// Definitions of the named types, in the order they were first needed.
    pub defs: Vec<TokenStream>,
}

impl<'c> Types<'c> {
    pub fn new(cdd: &'c Cdd, langs: Vec<String>, rt: TokenStream) -> Self {
        Types {
            cdd,
            langs,
            rt,
            scope: Scope::camel(),
            by_dt: HashMap::new(),
            defs: Vec::new(),
        }
    }

    pub fn langs(&self) -> &[String] {
        &self.langs
    }

    fn label<'a>(&'a self, e: &'a TextEntry) -> Option<&'a str> {
        e.text.pick(&self.langs).filter(|s| !s.is_empty())
    }

    /// The Rust type for values of data type `dt`, defining it the first time.
    pub fn value_type(&mut self, dt: DtId) -> Result<ValueTy, String> {
        if let Some(v) = self.by_dt.get(&dt) {
            return Ok(v.clone());
        }
        let d = self.cdd.datatypes.get(dt);
        let (prim, bcd) = prim_of(d)?;
        let raw = prim.tokens();
        let vt = match &d.kind {
            DtKind::Ident => ValueTy {
                ty: raw,
                prim,
                bcd,
                named: None,
            },
            DtKind::Text(entries) => {
                let ident = format_ident!("{}", self.scope.take(names::camel(&d.qual)));
                let entries: Vec<&TextEntry> = {
                    let (lo, hi) = prim.range();
                    // An entry outside what the raw type holds can never match; leave it out.
                    entries
                        .iter()
                        .filter(|e| e.lo >= lo && e.hi <= hi && e.lo <= e.hi)
                        .collect()
                };
                let kind = if !entries.is_empty()
                    && entries.len() <= 1024
                    && entries.iter().all(|e| e.lo == e.hi)
                {
                    NamedKind::Enum
                } else {
                    NamedKind::Range
                };
                let def = if kind == NamedKind::Enum {
                    self.enum_def(d, &ident, prim, &entries)
                } else {
                    self.range_def(d, &ident, prim, &entries)
                };
                self.defs.push(def);
                ValueTy {
                    ty: quote!(types::#ident),
                    prim,
                    bcd,
                    named: Some(Named {
                        path: quote!(types::#ident),
                        kind,
                    }),
                }
            }
            DtKind::Linear(lin) => {
                if prim.is_float() {
                    return Err("a linear conversion of a float is not supported".into());
                }
                let ident = format_ident!("{}", self.scope.take(names::camel(&d.qual)));
                let def = self.linear_def(d, &ident, prim, &lin.comps)?;
                self.defs.push(def);
                ValueTy {
                    ty: quote!(types::#ident),
                    prim,
                    bcd,
                    named: Some(Named {
                        path: quote!(types::#ident),
                        kind: NamedKind::Linear,
                    }),
                }
            }
            DtKind::Record(_) | DtKind::Mux(_) | DtKind::Unsupported(_) => {
                return Err(format!("data type `{}` is not a plain value", d.qual));
            }
        };
        self.by_dt.insert(dt, vt.clone());
        Ok(vt)
    }

    fn doc(&self, d: &DataType) -> String {
        match d.name.pick(&self.langs) {
            Some(n) if !n.is_empty() && n != d.qual => format!(" {n} (`{}`)", d.qual),
            _ => format!(" `{}`", d.qual),
        }
    }

    fn enum_def(
        &self,
        d: &DataType,
        ident: &Ident,
        prim: Prim,
        entries: &[&TextEntry],
    ) -> TokenStream {
        let raw = prim.tokens();
        let doc = self.doc(d);
        let mut scope = Scope::camel();
        scope.reserve("Other");
        let mut seen = HashSet::new();
        let mut variants = Vec::new();
        for e in entries {
            if !seen.insert(e.lo) {
                continue; // the first entry of a value wins, as in the interpreter
            }
            let base = match self.label(e) {
                Some(l) => names::camel_label(l, 40),
                None if e.lo < 0 => format!("ValueM{}", e.lo.unsigned_abs()),
                None => format!("Value{:X}", e.lo),
            };
            variants.push((
                format_ident!("{}", scope.take(base)),
                e.lo,
                self.label(e).map(str::to_string),
            ));
        }
        let names_: Vec<&Ident> = variants.iter().map(|v| &v.0).collect();
        let values: Vec<Literal> = variants.iter().map(|v| lit(v.1)).collect();
        let docs: Vec<String> = variants
            .iter()
            .map(|v| format!(" raw value {}", v.1))
            .collect();
        let labels: Vec<TokenStream> = variants
            .iter()
            .map(|v| match &v.2 {
                Some(l) => quote!(Some(#l)),
                None => quote!(None),
            })
            .collect();
        quote! {
            #[doc = #doc]
            #[derive(Clone, Copy, Debug, PartialEq, Eq)]
            pub enum #ident {
                #( #[doc = #docs] #names_, )*
                /// A value the description gives no name.
                Other(#raw),
            }

            impl #ident {
                pub const fn from_raw(raw: #raw) -> Self {
                    match raw {
                        #( #values => Self::#names_, )*
                        other => Self::Other(other),
                    }
                }

                pub const fn raw(self) -> #raw {
                    match self {
                        #( Self::#names_ => #values, )*
                        Self::Other(raw) => raw,
                    }
                }

                /// The text the description gives the value.
                pub const fn label(self) -> Option<&'static str> {
                    match self {
                        #( Self::#names_ => #labels, )*
                        Self::Other(_) => None,
                    }
                }
            }
        }
    }

    fn range_def(
        &self,
        d: &DataType,
        ident: &Ident,
        prim: Prim,
        entries: &[&TextEntry],
    ) -> TokenStream {
        let raw = prim.tokens();
        let doc = self.doc(d);
        // Every entry gets an arm, also one without a text: the first entry that holds a value decides.
        let arms: Vec<TokenStream> = entries
            .iter()
            .map(|e| {
                let pat = if e.lo == e.hi {
                    let l = lit(e.lo);
                    quote!(#l)
                } else {
                    let (a, b) = (lit(e.lo), lit(e.hi));
                    quote!(#a..=#b)
                };
                match self.label(e) {
                    Some(label) => quote!(#pat => Some(#label),),
                    None => quote!(#pat => None,),
                }
            })
            .collect();
        quote! {
            #[doc = #doc]
            #[derive(Clone, Copy, Debug, PartialEq, Eq)]
            pub struct #ident(pub #raw);

            impl #ident {
                pub const fn from_raw(raw: #raw) -> Self {
                    Self(raw)
                }

                pub const fn raw(self) -> #raw {
                    self.0
                }

                /// The text the description gives the value.
                pub const fn label(self) -> Option<&'static str> {
                    match self.0 {
                        #( #arms )*
                        _ => None,
                    }
                }
            }
        }
    }

    fn linear_def(
        &self,
        d: &DataType,
        ident: &Ident,
        prim: Prim,
        comps: &[cdd_model::datatype::Comp],
    ) -> Result<TokenStream, String> {
        let raw = prim.tokens();
        let doc = self.doc(d);
        let rt = &self.rt;
        let unit = opt_str(d.phys.unit.as_deref());
        let mut physical = Vec::new();
        let mut inverse = Vec::new();
        for c in comps {
            if !(c.f.is_finite() && c.div.is_finite() && c.o.is_finite()) {
                return Err(format!(
                    "`{}` has a coefficient that is not a finite number",
                    d.qual
                ));
            }
            let (f, div, o) = (
                Literal::f64_suffixed(c.f),
                Literal::f64_suffixed(c.div),
                Literal::f64_suffixed(c.o),
            );
            let lo = c.s.map(|s| {
                let l = lit(s);
                quote!(raw >= #l)
            });
            let hi = c.e.map(|e| {
                let l = lit(e);
                quote!(raw <= #l)
            });
            let cover = match (lo, hi) {
                (Some(a), Some(b)) => quote!(#a && #b),
                (Some(a), None) => a,
                (None, Some(b)) => b,
                (None, None) => quote!(true),
            };
            physical.push(quote! {
                if #cover {
                    return Some((#f / #div) * (raw as f64) + #o);
                }
            });
            inverse.push(quote! {
                {
                    let factor = #f / #div;
                    if factor != 0.0 && factor.is_finite() {
                        let r = #rt::math::round_ties_away((p - #o) / factor);
                        if r.abs() <= 1e30 {
                            let raw = r as i128;
                            if #cover && raw >= i128::from(#raw::MIN) && raw <= i128::from(#raw::MAX) {
                                return Some(Self(raw as #raw));
                            }
                        }
                    }
                }
            });
        }
        Ok(quote! {
            #[doc = #doc]
            #[derive(Clone, Copy, Debug, PartialEq, Eq)]
            pub struct #ident(pub #raw);

            impl #ident {
                pub const UNIT: Option<&'static str> = #unit;

                pub const fn from_raw(raw: #raw) -> Self {
                    Self(raw)
                }

                pub const fn raw(self) -> #raw {
                    self.0
                }

                /// The physical value; `None` when no conversion of the description covers the raw value.
                pub fn physical(self) -> Option<f64> {
                    let raw = i128::from(self.0);
                    #( #physical )*
                    None
                }

                /// The raw value nearest to a physical one; `None` when nothing reaches it.
                pub fn from_physical(p: f64) -> Option<Self> {
                    if !p.is_finite() {
                        return None;
                    }
                    #( #inverse )*
                    None
                }
            }
        })
    }
}
