//! Generates the Rust for one message: its struct, and how it is read, written and described.
//!
//! Every IR item has a fixed way to be turned into code:
//!
//! * constants are checked (decode) and written (encode) but are not fields; consecutive
//!   constants are one check, the sub-function's suppress bit is a `bool` field;
//! * a value is a field typed by its data type, an array a `[u8; N]`, the rest of the payload a
//!   `&[u8]`;
//! * a record, a bit container and an element of a repeated part are structs of their own;
//! * a repeated part is a [`List`](cdd_rt::List) of such elements.

use crate::names::{self, Scope};
use crate::types::{Named, NamedKind, Prim, Types};
use cdd_model::datatype::{ByteOrder, Enc};
use cdd_model::error::{codes, Issue};
use cdd_model::ir::{
    items_fixed_bytes, BitChild, Bits, Const, Field, Group, Item, Message, MsgKind, Mux, Repeat,
    RepeatCount, Shape,
};
use cdd_model::{Cdd, Service};
use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};
use std::collections::HashMap;
use syn::Ident;

type R<T> = Result<T, Issue>;

/// The largest payload of a classic ISO-TP message.
const MAX_PAYLOAD: usize = 4095;

fn unsupported(msg: impl Into<String>) -> Issue {
    Issue::error(codes::UNSUPPORTED_LAYOUT, msg)
}

/// Where the number of a count comes from, once it has been read or when it is written.
#[derive(Clone)]
enum CountSrc {
    /// A typed value; `named` when it is an enum or newtype that has `.raw()`.
    Prim { ident: Ident, named: bool },
    /// A raw slot of up to eight bytes, read as a big-endian number.
    Bytes { ident: Ident },
}

/// The pieces of one struct (a message, a record, a bit container, an element) while it is built.
struct Parts {
    names: Scope,
    fields: Vec<(Ident, TokenStream)>,
    decode: Vec<TokenStream>,
    encode: Vec<TokenStream>,
    describe: Vec<TokenStream>,
    /// Counts of repeated parts, by the path of the field holding them.
    counts: HashMap<String, CountSrc>,
    lifetime: bool,
    /// Constants not yet written out: (value, mask) per wire byte.
    pending: Vec<(u8, u8)>,
}

impl Parts {
    fn new() -> Parts {
        let mut names = Scope::default();
        // Locals of the generated functions.
        for n in [
            "r",
            "w",
            "v",
            "out",
            "payload",
            "path",
            "self",
            "raw_bits",
            "sub_byte",
            "raw_count",
            "reader",
            "writer",
        ] {
            names.reserve(n);
        }
        Parts {
            names,
            fields: Vec::new(),
            decode: Vec::new(),
            encode: Vec::new(),
            describe: Vec::new(),
            counts: HashMap::new(),
            lifetime: false,
            pending: Vec::new(),
        }
    }

    fn init(&self) -> TokenStream {
        if self.fields.is_empty() {
            quote!(Self)
        } else {
            let idents: Vec<&Ident> = self.fields.iter().map(|f| &f.0).collect();
            quote!(Self { #(#idents),* })
        }
    }

    fn struct_body(&self) -> TokenStream {
        if self.fields.is_empty() {
            quote!(;)
        } else {
            let (idents, tys): (Vec<&Ident>, Vec<&TokenStream>) =
                self.fields.iter().map(|f| (&f.0, &f.1)).unzip();
            quote!({ #( pub #idents: #tys, )* })
        }
    }
}

pub struct MessageCode {
    pub tokens: TokenStream,
    pub ty: Ident,
    pub lifetime: bool,
    /// The request has a `suppress_positive_response` field.
    pub suppress: bool,
    /// How many constant bytes the message starts with: the more, the more specific.
    pub prefix_bytes: usize,
    /// The constants as text: two messages with the same key cannot be told apart.
    pub prefix_key: String,
}

pub struct Gen<'a, 'c> {
    cdd: &'c Cdd,
    types: &'a mut Types<'c>,
    rt: TokenStream,
    /// Structs of records, bit containers and elements, to go next to the messages.
    aux: Vec<TokenStream>,
    type_names: Scope,
}

fn order_tokens(rt: &TokenStream, o: ByteOrder) -> TokenStream {
    match o {
        ByteOrder::Big => quote!(#rt::Order::Big),
        ByteOrder::Little => quote!(#rt::Order::Little),
    }
}

fn byte(v: u8) -> Literal {
    Literal::u8_unsuffixed(v)
}

impl<'a, 'c> Gen<'a, 'c> {
    pub fn new(cdd: &'c Cdd, types: &'a mut Types<'c>, rt: TokenStream) -> Self {
        let mut type_names = Scope::camel();
        type_names.reserve("Request");
        type_names.reserve("Response");
        Gen {
            cdd,
            types,
            rt,
            aux: Vec::new(),
            type_names,
        }
    }

    /// The structs the messages built so far need.
    pub fn take_aux(&mut self) -> Vec<TokenStream> {
        std::mem::take(&mut self.aux)
    }

    /// Generates `type_name` for the message of `svc`.
    pub fn message(&mut self, svc: &Service, msg: &Message, type_name: &str) -> R<MessageCode> {
        let rt = self.rt.clone();
        let mut p = Parts::new();
        self.items(&msg.items, &mut p)?;
        self.flush(&mut p);

        let ty = format_ident!("{}", type_name);
        let lt = p.lifetime;
        let suppress = p.fields.iter().any(|f| f.0 == "suppress_positive_response");
        let kind = match msg.kind {
            MsgKind::Request => "REQ",
            MsgKind::Positive => "POS",
            MsgKind::Negative => "NEG",
        };
        let name = svc.shortcut.as_str();
        let key = format!("{}/{kind}", svc.key);
        let prefix = msg.prefix(&self.cdd.datatypes);
        let prefix_hex: Vec<String> = prefix.iter().map(|b| format!("{:02X}", b.value)).collect();
        let doc = format!(
            "{} of `{}`{}",
            if msg.kind == MsgKind::Request {
                "The request"
            } else {
                "The positive response"
            },
            name,
            if prefix_hex.is_empty() {
                String::new()
            } else {
                format!(", which starts with `{}`", prefix_hex.join(" "))
            }
        );
        let (min_len, max_len) = msg.len_range(&self.cdd.datatypes);
        let max_len = max_len
            .unwrap_or(MAX_PAYLOAD)
            .min(MAX_PAYLOAD.max(max_len.unwrap_or(0)));
        let prefix_items: Vec<TokenStream> = prefix
            .iter()
            .map(|b| {
                let (o, v, m) = (
                    Literal::usize_unsuffixed(b.offset),
                    byte(b.value),
                    byte(b.mask),
                );
                quote!(#rt::PrefixByte { offset: #o, value: #v, mask: #m })
            })
            .collect();

        let body = p.struct_body();
        let (decode, encode, describe, init) = (&p.decode, &p.encode, &p.describe, p.init());
        let (gen_ty, impl_ty, impl_ty_anon) = if lt {
            (quote!(<'a>), quote!(#ty<'a>), quote!(#ty<'_>))
        } else {
            (quote!(), quote!(#ty), quote!(#ty))
        };
        let tokens = quote! {
            #[doc = #doc]
            #[derive(Clone, Copy, Debug, PartialEq)]
            pub struct #ty #gen_ty #body

            impl<'a> #rt::Decode<'a> for #impl_ty {
                fn decode(payload: &'a [u8]) -> Result<Self, #rt::DecodeError> {
                    let mut reader = #rt::Reader::new(payload);
                    let r = &mut reader;
                    #(#decode)*
                    reader.finish()?;
                    Ok(#init)
                }
            }

            impl #rt::Encode for #impl_ty_anon {
                fn encode(&self, out: &mut [u8]) -> Result<usize, #rt::EncodeError> {
                    let mut writer = #rt::Writer::new(out);
                    let w = &mut writer;
                    #(#encode)*
                    Ok(writer.finish())
                }
            }

            impl #rt::Message for #impl_ty_anon {
                const NAME: &'static str = #name;
                const KEY: &'static str = #key;
                const MIN_LEN: usize = #min_len;
                const MAX_LEN: usize = #max_len;
                const PREFIX: &'static [#rt::PrefixByte] = &[ #(#prefix_items),* ];
            }

            impl #rt::Describe for #impl_ty_anon {
                fn describe(&self, path: &#rt::Path<'_>, v: &mut dyn #rt::Visitor) {
                    #(#describe)*
                }
            }
        };
        let prefix_key = prefix
            .iter()
            .map(|b| format!("{}:{:02x}/{:02x}", b.offset, b.value, b.mask))
            .collect::<Vec<_>>()
            .join(",");
        Ok(MessageCode {
            tokens,
            ty,
            lifetime: lt,
            suppress,
            prefix_bytes: prefix.len(),
            prefix_key,
        })
    }

    // ---- items -------------------------------------------------------------------------------

    /// Writes out the constants collected so far as one check and one write.
    fn flush(&self, p: &mut Parts) {
        if p.pending.is_empty() {
            return;
        }
        let values: Vec<Literal> = p.pending.iter().map(|(v, _)| byte(*v)).collect();
        let masks: Vec<Literal> = p.pending.iter().map(|(_, m)| byte(*m)).collect();
        p.decode
            .push(quote!(r.expect(&[#(#values),*], &[#(#masks),*])?;));
        p.encode.push(quote!(w.put(&[#(#values),*])?;));
        p.pending.clear();
    }

    fn items(&mut self, items: &[Item], p: &mut Parts) -> R<()> {
        for item in items {
            match item {
                Item::Const(c) => self.constant(c, p)?,
                Item::Field(f) => self.field(f, p)?,
                Item::Group(g) => self.group(g, p)?,
                Item::Bits(b) => self.bits(b, p)?,
                Item::Repeat(r) => self.repeat(r, p)?,
                Item::Gap(n) => {
                    self.flush(p);
                    let n = Literal::usize_unsuffixed(*n as usize);
                    p.decode.push(quote!(r.bytes(#n)?;));
                    p.encode.push(quote!(w.zeros(#n)?;));
                }
                Item::Nrc(n) => {
                    return Err(unsupported(format!(
                        "`{}`: a response code slot outside a negative response",
                        n.name
                    )))
                }
                Item::Mux(m) => self.mux(m, p)?,
            }
        }
        Ok(())
    }

    fn constant(&mut self, c: &Const, p: &mut Parts) -> R<()> {
        let rt = self.rt.clone();
        let path = c.path.as_str();
        let shown = Literal::u64_unsuffixed(c.value & c.mask);
        p.describe
            .push(quote!(v.field(&path.name(#path), &#rt::FieldValue::uint(#shown));));
        if c.suppress_bit && c.bits == 8 {
            self.flush(p);
            let ident = format_ident!("{}", p.names.take("suppress_positive_response".into()));
            let (val, mask) = (byte(c.value as u8), byte(c.mask as u8));
            p.decode.push(quote! {
                let sub_byte = r.u8()?;
                if sub_byte & #mask != #val {
                    return Err(#rt::DecodeError::Prefix { offset: r.position() - 1 });
                }
                let #ident = sub_byte & 0x80 != 0;
            });
            p.encode
                .push(quote!(w.u8(#val | if self.#ident { 0x80 } else { 0 })?;));
            p.fields.push((ident, quote!(bool)));
            return Ok(());
        }
        for (v, m) in c.wire_bytes().into_iter().zip(c.wire_mask()) {
            p.pending.push((v, m));
        }
        Ok(())
    }

    // ---- fields ------------------------------------------------------------------------------

    fn field(&mut self, f: &Field, p: &mut Parts) -> R<()> {
        self.flush(p);
        let rt = self.rt.clone();
        let fname = f.path.as_str();
        let ident = format_ident!("{}", p.names.take(names::snake(&f.name)));
        let elem_bytes = f.elem_bytes(&self.cdd.datatypes);
        match (f.dt, f.shape) {
            (Some(dt), Shape::Atom) => self.scalar(&ident, fname, dt, p)?,
            (_, Shape::Array { count }) => {
                let total = elem_bytes * count as usize;
                let n = Literal::usize_unsuffixed(total);
                p.decode.push(quote!(let #ident = r.array::<#n>()?;));
                p.encode.push(quote!(w.put(&self.#ident)?;));
                let (text, unit) = self.bytes_meta(f, &quote!(&self.#ident));
                p.describe.push(quote!(v.field(&path.name(#fname), &#rt::FieldValue::bytes(&self.#ident).text(#text).physical(None, #unit));));
                p.fields.push((ident.clone(), quote!([u8; #n])));
                if f.dt.is_none() && (1..=8).contains(&total) {
                    p.counts.insert(f.path.clone(), CountSrc::Bytes { ident });
                }
            }
            (_, Shape::Rest { min, max }) => {
                let (min_b, max_b) = (
                    Literal::usize_unsuffixed(min as usize * elem_bytes),
                    Literal::usize_unsuffixed(max as usize * elem_bytes),
                );
                let whole = if elem_bytes > 1 {
                    let e = Literal::usize_unsuffixed(elem_bytes);
                    quote!(|| !#ident.len().is_multiple_of(#e))
                } else {
                    quote!()
                };
                let check =
                    |access: TokenStream| quote!(#access.len() < #min_b || #access.len() > #max_b);
                let decode_check = check(quote!(#ident));
                let encode_check = check(quote!(self.#ident));
                let whole_enc = if elem_bytes > 1 {
                    let e = Literal::usize_unsuffixed(elem_bytes);
                    quote!(|| !self.#ident.len().is_multiple_of(#e))
                } else {
                    quote!()
                };
                p.decode.push(quote! {
                    let #ident = r.rest();
                    if #decode_check #whole {
                        return Err(#rt::DecodeError::Length { field: #fname });
                    }
                });
                p.encode.push(quote! {
                    if #encode_check #whole_enc {
                        return Err(#rt::EncodeError::Length { field: #fname });
                    }
                    w.put(self.#ident)?;
                });
                let (text, unit) = self.bytes_meta(f, &quote!(self.#ident));
                p.describe.push(quote!(v.field(&path.name(#fname), &#rt::FieldValue::bytes(self.#ident).text(#text).physical(None, #unit));));
                p.fields.push((ident, quote!(&'a [u8])));
                p.lifetime = true;
            }
            (None, Shape::Atom) => {
                return Err(unsupported(format!(
                    "`{}` is a single value without a data type",
                    f.name
                )))
            }
        }
        Ok(())
    }

    /// The `text` and `unit` of the described value of a byte field.
    fn bytes_meta(&self, f: &Field, bytes: &TokenStream) -> (TokenStream, TokenStream) {
        let Some(dt) = f.dt else {
            return (quote!(None), quote!(None));
        };
        let d = self.cdd.datatypes.get(dt);
        let text = match d.coded.enc {
            Enc::Ascii | Enc::Utf => quote!(core::str::from_utf8(#bytes).ok()),
            _ => quote!(None),
        };
        let unit = match &d.phys.unit {
            Some(u) => quote!(Some(#u)),
            None => quote!(None),
        };
        (text, unit)
    }

    /// A single value of `n` whole bytes.
    fn scalar(&mut self, ident: &Ident, fname: &str, dt: usize, p: &mut Parts) -> R<()> {
        let rt = self.rt.clone();
        let vt = self
            .types
            .value_type(dt)
            .map_err(|m| Issue::error(codes::UNSUPPORTED_DATATYPE, format!("`{fname}`: {m}")))?;
        let d = self.cdd.datatypes.get(dt);
        let bits = d.coded.bits;
        let n = Literal::usize_unsuffixed((bits / 8) as usize);
        let order = order_tokens(&rt, d.coded.order);
        let prim = vt.prim;
        let primt = prim.tokens();

        let raw_read = if vt.bcd {
            quote!(r.bcd(#n, #order, #fname)? as #primt)
        } else {
            match prim {
                Prim::F32 => quote!(f32::from_bits(r.uint(4, #order)? as u32)),
                Prim::F64 => quote!(f64::from_bits(r.uint(8, #order)?)),
                p if p.is_signed() => quote!(r.int(#n, #order, #bits)? as #primt),
                _ => quote!(r.uint(#n, #order)? as #primt),
            }
        };
        let value = match &vt.named {
            Some(Named { path: t, .. }) => quote!(#t::from_raw(#raw_read)),
            None => raw_read,
        };
        p.decode.push(quote!(let #ident = #value;));

        let raw_of = if vt.named.is_some() {
            quote!(self.#ident.raw())
        } else {
            quote!(self.#ident)
        };
        let full = prim.bits() == bits;
        let write = if vt.bcd {
            quote!(w.bcd(u64::from(#raw_of), #n, #order, #fname)?;)
        } else {
            match prim {
                Prim::F32 => quote!(w.uint(u64::from(#raw_of.to_bits()), 4, #order)?;),
                Prim::F64 => quote!(w.uint(#raw_of.to_bits(), 8, #order)?;),
                p if p.is_signed() && full => {
                    quote!(w.uint(i64::from(#raw_of) as u64, #n, #order)?;)
                }
                p if p.is_signed() => {
                    quote!(w.uint(#rt::io::check_int(i64::from(#raw_of), #bits, #fname)?, #n, #order)?;)
                }
                _ if full => quote!(w.uint(u64::from(#raw_of), #n, #order)?;),
                _ => {
                    quote!(w.uint(#rt::io::check_uint(u64::from(#raw_of), #bits, #fname)?, #n, #order)?;)
                }
            }
        };
        p.encode.push(write);

        let shown = self.shown_value(
            &vt,
            prim,
            vt.bcd,
            d.phys.unit.as_deref(),
            &quote!(self.#ident),
        );
        p.describe
            .push(quote!(v.field(&path.name(#fname), &#shown);));
        p.fields.push((ident.clone(), vt.ty.clone()));
        if !prim.is_float() && !prim.is_signed() {
            p.counts.insert(
                fname.to_string(),
                CountSrc::Prim {
                    ident: ident.clone(),
                    named: vt.named.is_some(),
                },
            );
        }
        Ok(())
    }

    /// The `FieldValue` that describes the value held in `acc`.
    fn shown_value(
        &self,
        vt: &crate::types::ValueTy,
        prim: Prim,
        _bcd: bool,
        unit: Option<&str>,
        acc: &TokenStream,
    ) -> TokenStream {
        let rt = &self.rt;
        let raw_of = if vt.named.is_some() {
            quote!(#acc.raw())
        } else {
            acc.clone()
        };
        let base = match prim {
            Prim::F32 => quote!(#rt::FieldValue::float(f64::from(#raw_of))),
            Prim::F64 => quote!(#rt::FieldValue::float(#raw_of)),
            p if p.is_signed() => quote!(#rt::FieldValue::int(i64::from(#raw_of))),
            _ => quote!(#rt::FieldValue::uint(u64::from(#raw_of))),
        };
        let (text, phys) = match &vt.named {
            Some(Named {
                kind: NamedKind::Enum | NamedKind::Range,
                ..
            }) => (quote!(#acc.label()), quote!(None)),
            Some(Named {
                kind: NamedKind::Linear,
                ..
            }) => (quote!(None), quote!(#acc.physical())),
            None => (quote!(None), quote!(None)),
        };
        let unit = match unit {
            Some(u) => quote!(Some(#u)),
            None => quote!(None),
        };
        quote!(#base.text(#text).physical(#phys, #unit))
    }

    // ---- structs of their own ----------------------------------------------------------------

    fn aux_name(&mut self, name: &str) -> Ident {
        format_ident!("{}", self.type_names.take(names::camel(name)))
    }

    fn group(&mut self, g: &Group, p: &mut Parts) -> R<()> {
        self.flush(p);
        let mut sub = Parts::new();
        self.items(&g.items, &mut sub)?;
        self.flush(&mut sub);
        let ty = self.aux_name(&g.name);
        let lt = sub.lifetime;
        self.aux
            .push(self.record_struct(&ty, &sub, &format!("`{}`", g.name)));
        let ident = format_ident!("{}", p.names.take(names::snake(&g.name)));
        p.decode.push(quote!(let #ident = #ty::read(r)?;));
        p.encode.push(quote!(self.#ident.write(w)?;));
        p.describe.push(quote!(self.#ident.describe_in(path, v);));
        p.fields
            .push((ident, if lt { quote!(#ty<'a>) } else { quote!(#ty) }));
        p.lifetime |= lt;
        Ok(())
    }

    /// A struct with `read`, `write` and `describe_in`, for a record or a bit container.
    fn record_struct(&self, ty: &Ident, sub: &Parts, doc: &str) -> TokenStream {
        let rt = &self.rt;
        let doc = format!(" {doc}");
        let lt = sub.lifetime;
        let (gen_ty, impl_gen, use_ty) = if lt {
            (quote!(<'a>), quote!(<'a>), quote!(#ty<'a>))
        } else {
            (quote!(), quote!(), quote!(#ty))
        };
        let body = sub.struct_body();
        let (decode, encode, describe, init) =
            (&sub.decode, &sub.encode, &sub.describe, sub.init());
        let reader_lt = if lt { quote!('a) } else { quote!('_) };
        quote! {
            #[doc = #doc]
            #[derive(Clone, Copy, Debug, PartialEq)]
            pub struct #ty #gen_ty #body

            impl #impl_gen #use_ty {
                fn read(r: &mut #rt::Reader<#reader_lt>) -> Result<Self, #rt::DecodeError> {
                    #(#decode)*
                    Ok(#init)
                }

                fn write(&self, w: &mut #rt::Writer<'_>) -> Result<(), #rt::EncodeError> {
                    #(#encode)*
                    Ok(())
                }

                fn describe_in(&self, path: &#rt::Path<'_>, v: &mut dyn #rt::Visitor) {
                    #(#describe)*
                }
            }
        }
    }

    fn bits(&mut self, b: &Bits, p: &mut Parts) -> R<()> {
        self.flush(p);
        let rt = self.rt.clone();
        let n = Literal::usize_unsuffixed((b.bits / 8) as usize);
        let order = order_tokens(&rt, b.order);
        let mut sub = Parts::new();
        sub.decode.push(quote!(let raw_bits = r.uint(#n, #order)?;));
        sub.encode.push(quote!(let mut raw_bits: u64 = 0;));
        for ch in &b.children {
            let BitChild::Field {
                name,
                path,
                dt,
                offset,
                width,
                ..
            } = ch
            else {
                continue;
            };
            let vt = self
                .types
                .value_type(*dt)
                .map_err(|m| Issue::error(codes::UNSUPPORTED_DATATYPE, format!("`{path}`: {m}")))?;
            if vt.prim.is_float() || vt.bcd {
                return Err(unsupported(format!(
                    "bit field `{path}` is a float or BCD value"
                )));
            }
            let d = self.cdd.datatypes.get(*dt);
            let ident = format_ident!("{}", sub.names.take(names::snake(name)));
            let (off, w_lit) = (
                Literal::u32_unsuffixed(*offset),
                Literal::u32_unsuffixed(*width),
            );
            let mask = if *width >= 64 {
                Literal::u64_unsuffixed(u64::MAX)
            } else {
                Literal::u64_unsuffixed((1u64 << width) - 1)
            };
            let primt = vt.prim.tokens();
            let field_bits = quote!((raw_bits >> #off) & #mask);
            let raw_read = if vt.prim.is_signed() {
                quote!(#rt::io::sign_extend(#field_bits, #w_lit) as #primt)
            } else {
                quote!((#field_bits) as #primt)
            };
            let value = match &vt.named {
                Some(Named { path: t, .. }) => quote!(#t::from_raw(#raw_read)),
                None => raw_read,
            };
            sub.decode.push(quote!(let #ident = #value;));
            let raw_of = if vt.named.is_some() {
                quote!(self.#ident.raw())
            } else {
                quote!(self.#ident)
            };
            let pack = if vt.prim.is_signed() {
                quote!(#rt::io::check_int(i64::from(#raw_of), #w_lit, #path)?)
            } else {
                quote!(#rt::io::check_uint(u64::from(#raw_of), #w_lit, #path)?)
            };
            sub.encode.push(quote!(raw_bits |= #pack << #off;));
            let shown = self.shown_value(
                &vt,
                vt.prim,
                false,
                d.phys.unit.as_deref(),
                &quote!(self.#ident),
            );
            sub.describe
                .push(quote!(v.field(&path.name(#path), &#shown);));
            sub.fields.push((ident, vt.ty.clone()));
        }
        sub.encode.push(quote!(w.uint(raw_bits, #n, #order)?;));
        let ty = self.aux_name(&b.name);
        self.aux
            .push(self.record_struct(&ty, &sub, &format!("`{}`: {} bits", b.name, b.bits)));
        let ident = format_ident!("{}", p.names.take(names::snake(&b.name)));
        p.decode.push(quote!(let #ident = #ty::read(r)?;));
        p.encode.push(quote!(self.#ident.write(w)?;));
        p.describe.push(quote!(self.#ident.describe_in(path, v);));
        p.fields.push((ident, quote!(#ty)));
        Ok(())
    }

    /// The number a count or a selector holds, as read (decode) and as written (encode).
    fn number_exprs(&self, src: &CountSrc) -> (TokenStream, TokenStream) {
        let rt = &self.rt;
        match src {
            CountSrc::Prim {
                ident,
                named: false,
            } => (quote!(u64::from(#ident)), quote!(u64::from(self.#ident))),
            CountSrc::Prim { ident, named: true } => (
                quote!(u64::from(#ident.raw())),
                quote!(u64::from(self.#ident.raw())),
            ),
            CountSrc::Bytes { ident } => (
                quote!(#rt::io::be_uint(&#ident)),
                quote!(#rt::io::be_uint(&self.#ident)),
            ),
        }
    }

    /// A multiplexer: an enum with one variant per case and one for the default. The selector is a
    /// field read before it; reading chooses the variant, and writing checks that the variant is the
    /// one the selector's value chooses.
    fn mux(&mut self, m: &Mux, p: &mut Parts) -> R<()> {
        self.flush(p);
        let rt = self.rt.clone();
        let key = m.selector.first().cloned().unwrap_or_default();
        let src = p.counts.get(&key).cloned().ok_or_else(|| {
            unsupported(format!(
                "`{}`: its selector `{key}` is not a field of the same record",
                m.name
            ))
        })?;
        let (read_sel, write_sel) = self.number_exprs(&src);
        let mask = Literal::u64_unsuffixed(m.mask);
        let enum_ty = self.aux_name(&m.name);
        let mpath = m.path.as_str();

        let mut vscope = Scope::camel();
        vscope.reserve("Default");
        let mut lt = false;
        // (variant, struct type, has a lifetime, low, high)
        let mut variants: Vec<(Ident, Ident, bool, u64, u64)> = Vec::new();
        for case in &m.cases {
            let mut sub = Parts::new();
            self.items(&case.items, &mut sub)?;
            self.flush(&mut sub);
            let (lo, hi) = (case.lo as u64, u64::try_from(case.hi).unwrap_or(u64::MAX));
            let ty = self.aux_name(&format!("{}_case_{lo:x}", m.name));
            self.aux.push(self.record_struct(
                &ty,
                &sub,
                &format!("`{}` when the selector is {lo}..={hi}", m.name),
            ));
            let label = if lo == hi {
                format!("Case{lo:X}")
            } else {
                format!("Case{lo:X}To{hi:X}")
            };
            lt |= sub.lifetime;
            variants.push((
                format_ident!("{}", vscope.take(label)),
                ty,
                sub.lifetime,
                lo,
                hi,
            ));
        }
        let mut dsub = Parts::new();
        self.items(&m.default, &mut dsub)?;
        self.flush(&mut dsub);
        let dty = self.aux_name(&format!("{}_default", m.name));
        self.aux.push(self.record_struct(
            &dty,
            &dsub,
            &format!("`{}` when the selector holds no case", m.name),
        ));
        lt |= dsub.lifetime;

        let with_lt = |ty: &Ident, l: bool| if l { quote!(#ty<'a>) } else { quote!(#ty) };
        let variant_defs: Vec<TokenStream> = variants
            .iter()
            .map(|(v, ty, l, lo, hi)| {
                let doc = format!(" The selector is {lo}..={hi}.");
                let t = with_lt(ty, *l);
                quote!(#[doc = #doc] #v(#t))
            })
            .collect();
        let default_ty = with_lt(&dty, dsub.lifetime);
        let reads: Vec<TokenStream> = variants
            .iter()
            .map(|(v, ty, _, lo, hi)| {
                let (lo, hi) = (Literal::u64_unsuffixed(*lo), Literal::u64_unsuffixed(*hi));
                quote!(#lo..=#hi => Ok(Self::#v(#ty::read(r)?)),)
            })
            .collect();
        let ranges: Vec<TokenStream> = variants
            .iter()
            .map(|(_, _, _, lo, hi)| {
                let (lo, hi) = (Literal::u64_unsuffixed(*lo), Literal::u64_unsuffixed(*hi));
                quote!(#lo..=#hi)
            })
            .collect();
        let writes: Vec<TokenStream> = variants
            .iter()
            .zip(&ranges)
            .map(|((v, _, _, _, _), range)| {
                quote!(Self::#v(c) => {
                    if !matches!(selector, #range) {
                        return Err(#rt::EncodeError::Selector { field: #mpath });
                    }
                    c.write(w)
                })
            })
            .collect();
        let describes: Vec<TokenStream> = variants
            .iter()
            .map(|(v, _, _, _, _)| quote!(Self::#v(c) => c.describe_in(path, v),))
            .collect();
        let (gen_ty, impl_gen, use_ty, reader_lt) = if lt {
            (quote!(<'a>), quote!(<'a>), quote!(#enum_ty<'a>), quote!('a))
        } else {
            (quote!(), quote!(), quote!(#enum_ty), quote!('_))
        };
        let doc = format!(" `{}`: one variant for each case, and the default.", m.name);
        let default_ident = format_ident!("Default");
        self.aux.push(quote! {
            #[doc = #doc]
            #[derive(Clone, Copy, Debug, PartialEq)]
            #[allow(clippy::large_enum_variant)]
            pub enum #enum_ty #gen_ty {
                #(#variant_defs,)*
                /// The selector holds no case.
                #default_ident(#default_ty),
            }

            impl #impl_gen #use_ty {
                fn read(r: &mut #rt::Reader<#reader_lt>, selector: u64) -> Result<Self, #rt::DecodeError> {
                    match selector {
                        #(#reads)*
                        _ => Ok(Self::#default_ident(#dty::read(r)?)),
                    }
                }

                fn write(&self, w: &mut #rt::Writer<'_>, selector: u64) -> Result<(), #rt::EncodeError> {
                    match self {
                        #(#writes)*
                        Self::#default_ident(d) => {
                            if matches!(selector, #(#ranges)|*) {
                                return Err(#rt::EncodeError::Selector { field: #mpath });
                            }
                            d.write(w)
                        }
                    }
                }

                fn describe_in(&self, path: &#rt::Path<'_>, v: &mut dyn #rt::Visitor) {
                    match self {
                        #(#describes)*
                        Self::#default_ident(d) => d.describe_in(path, v),
                    }
                }
            }
        });

        let ident = format_ident!("{}", p.names.take(names::snake(&m.name)));
        p.decode
            .push(quote!(let #ident = #enum_ty::read(r, #read_sel & #mask)?;));
        p.encode
            .push(quote!(self.#ident.write(w, #write_sel & #mask)?;));
        p.describe.push(quote!(self.#ident.describe_in(path, v);));
        p.fields.push((ident, with_lt(&enum_ty, lt)));
        p.lifetime |= lt;
        Ok(())
    }
    fn repeat(&mut self, rep: &Repeat, p: &mut Parts) -> R<()> {
        self.flush(p);
        let rt = self.rt.clone();
        let size = items_fixed_bytes(&rep.element, &self.cdd.datatypes)
            .filter(|n| *n > 0)
            .ok_or_else(|| {
                unsupported(format!(
                    "`{}`: the repeated element has no fixed size",
                    rep.name
                ))
            })?;
        let mut sub = Parts::new();
        self.items(&rep.element, &mut sub)?;
        self.flush(&mut sub);
        if sub.lifetime {
            return Err(unsupported(format!(
                "`{}`: a repeated element holds a variable-length value",
                rep.name
            )));
        }
        let elem = self.aux_name(&format!("{}_element", rep.name));
        let doc = format!("One element of `{}`", rep.name);
        let base = self.record_struct(&elem, &sub, &doc);
        // The struct's own `read`/`write`/`describe_in` back the `Element` impl.
        let size_lit = Literal::usize_unsuffixed(size);
        self.aux.push(quote! {
            #base

            impl #rt::Element for #elem {
                const SIZE: usize = #size_lit;

                fn read(r: &mut #rt::Reader<'_>) -> Result<Self, #rt::DecodeError> {
                    #elem::read(r)
                }

                fn write(&self, w: &mut #rt::Writer<'_>) -> Result<(), #rt::EncodeError> {
                    #elem::write(self, w)
                }

                fn describe(&self, path: &#rt::Path<'_>, v: &mut dyn #rt::Visitor) {
                    self.describe_in(path, v)
                }
            }
        });

        let ident = format_ident!("{}", p.names.take(names::snake(&rep.name)));
        let rpath = rep.path.as_str();
        match &rep.count {
            RepeatCount::ToEnd { min, max } => {
                let min = Literal::usize_unsuffixed(*min as usize);
                let upper = match max {
                    Some(m) => {
                        let m = Literal::usize_unsuffixed(*m as usize);
                        quote!(|| #ident.len() > #m)
                    }
                    None => quote!(),
                };
                let upper_enc = match max {
                    Some(m) => {
                        let m = Literal::usize_unsuffixed(*m as usize);
                        quote!(|| self.#ident.len() > #m)
                    }
                    None => quote!(),
                };
                p.decode.push(quote! {
                    let #ident = #rt::List::<#elem>::from_encoded(r.rest(), #rpath)?;
                    if #ident.len() < #min #upper {
                        return Err(#rt::DecodeError::Length { field: #rpath });
                    }
                });
                p.encode.push(quote! {
                    if self.#ident.len() < #min #upper_enc {
                        return Err(#rt::EncodeError::Length { field: #rpath });
                    }
                    self.#ident.write(w)?;
                });
            }
            RepeatCount::Counted { field, mask } => {
                let key = field.first().cloned().unwrap_or_default();
                let src = p.counts.get(&key).cloned().ok_or_else(|| {
                    unsupported(format!(
                        "`{}`: its count `{key}` is not a field of the same record",
                        rep.name
                    ))
                })?;
                let mask = Literal::u64_unsuffixed(*mask);
                let (read_count, write_count) = self.number_exprs(&src);
                p.decode.push(quote! {
                    let raw_count = (#read_count & #mask) as usize;
                    let #ident = #rt::List::<#elem>::from_encoded(r.bytes(raw_count * #size_lit)?, #rpath)?;
                });
                p.encode.push(quote! {
                    if (#write_count & #mask) as usize != self.#ident.len() {
                        return Err(#rt::EncodeError::Length { field: #rpath });
                    }
                    self.#ident.write(w)?;
                });
            }
        }
        p.describe
            .push(quote!(self.#ident.describe(&path.name(#rpath), v);));
        p.fields.push((ident, quote!(#rt::List<'a, #elem>)));
        p.lifetime = true;
        Ok(())
    }
}
