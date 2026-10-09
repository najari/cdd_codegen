//! Generates Rust tester and ECU-simulation code from a CANdela CDD.
//!
//! ```no_run
//! let bytes = std::fs::read("ecu.cdd").unwrap();
//! let artifacts = cdd_codegen::Config::default().generate("ecu.cdd", &bytes).unwrap();
//! artifacts.write_to_directory("generated").unwrap();
//! ```
//!
//! The generated file needs the `cdd-rt` crate. It holds, for one variant of one ECU:
//!
//! * `service::<name>`: the request and the positive response of every service, as structs that
//!   encode, decode and describe themselves;
//! * `types`: the enums and newtypes of text tables and linear conversions;
//! * `AnyRequest` / `AnyResponse`: what a payload of unknown service decodes to;
//! * `server`: a simulated ECU, a [`Responder`](cdd_rt::ecu::Responder) that calls one handler
//!   method per service;
//! * `nrc`: the negative response codes of the description.

mod artifact;
mod message;
mod names;
mod types;

pub use artifact::Artifacts;

use cdd_model::error::{codes, Issue};
use cdd_model::ir::MsgKind;
use cdd_model::{Cdd, Protocol, Service, Variant};
use message::{Gen, MessageCode};
use names::Scope;
use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};
use serde_json::json;
use sha2::{Digest, Sha256};
use types::Types;

pub const GENERATOR_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Which side the code is for. The messages are the same on both sides; the role picks the extras.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Role {
    /// Tester: sends requests, understands responses (`AnyResponse`, the `Request` trait).
    Tester,
    /// Simulated ECU: understands requests, answers them (`AnyRequest`, `server`).
    Ecu,
    /// Both, so a tester can be tried against a simulated ECU in one process.
    #[default]
    Both,
}

impl Role {
    fn tester(self) -> bool {
        matches!(self, Role::Tester | Role::Both)
    }

    fn ecu(self) -> bool {
        matches!(self, Role::Ecu | Role::Both)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OnUnsupported {
    /// Leave the message out and say so in the report.
    #[default]
    Skip,
    /// Fail when any message has to be left out.
    Error,
}

#[derive(Clone, Debug)]
pub struct Config {
    /// The ECU to generate for. Required when the document has several.
    pub ecu: Option<String>,
    /// The variant. Default: the base variant.
    pub variant: Option<String>,
    /// Languages for names and labels, best first. Default: English, then the document's own.
    pub languages: Vec<String>,
    pub role: Role,
    pub on_unsupported: OnUnsupported,
    /// The name of the runtime crate in the generated code.
    pub rt_crate: String,
    /// The negative response code a simulated ECU gives a service its handler does not implement.
    pub default_nrc: u8,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            ecu: None,
            variant: None,
            languages: Vec::new(),
            role: Role::Both,
            on_unsupported: OnUnsupported::Skip,
            rt_crate: "cdd_rt".into(),
            // "Subfunction not supported", what CANoe's example ECU answers for the rest.
            default_nrc: 0x12,
        }
    }
}

#[derive(Debug)]
pub enum Error {
    Model(cdd_model::Error),
    /// The ECU or variant asked for is not there, or has to be named.
    Selection(String),
    /// `OnUnsupported::Error` and a message was left out.
    Unsupported(Vec<Issue>),
    /// The generated tokens are not valid Rust: a generator bug.
    Internal(String),
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Model(e) => write!(f, "{e}"),
            Error::Selection(m) => write!(f, "{m}"),
            Error::Unsupported(issues) => {
                writeln!(f, "{} message(s) cannot be generated:", issues.len())?;
                for i in issues {
                    writeln!(f, "  {i}")?;
                }
                Ok(())
            }
            Error::Internal(m) => write!(
                f,
                "internal error: the generated code is not valid Rust: {m}"
            ),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<cdd_model::Error> for Error {
    fn from(e: cdd_model::Error) -> Self {
        Error::Model(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// A message left out of the generated code, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub service: String,
    pub message: &'static str,
    pub code: &'static str,
    pub reason: String,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub services_total: usize,
    pub services_generated: usize,
    pub requests_generated: usize,
    pub responses_generated: usize,
    pub skipped: Vec<Skipped>,
    /// Services whose requests (or responses) a received payload cannot be told apart by.
    pub ambiguous: Vec<String>,
}

/// The languages names and labels come from: the configured ones, else English, then the document's.
pub fn preferred_languages(cdd: &Cdd, configured: &[String]) -> Vec<String> {
    if !configured.is_empty() {
        return configured.to_vec();
    }
    let mut v: Vec<String> = ["en-US", "en-GB", "en"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    v.extend(cdd.languages.iter().cloned());
    v
}

/// What the generator made of one service.
struct ServiceOut {
    module: syn::Ident,
    module_name: String,
    variant: syn::Ident,
    request: Option<MessageCode>,
    response: Option<MessageCode>,
    tokens: TokenStream,
}

impl Config {
    /// Generates the code for the selected ECU variant of the CDD in `bytes`.
    pub fn generate(&self, source_name: &str, bytes: &[u8]) -> Result<Artifacts, Error> {
        let cdd = Cdd::from_bytes(bytes)?;
        self.generate_from(source_name, bytes, &cdd)
    }

    pub fn generate_from(
        &self,
        source_name: &str,
        bytes: &[u8],
        cdd: &Cdd,
    ) -> Result<Artifacts, Error> {
        let ecu = match &self.ecu {
            Some(q) => cdd.ecu(q).ok_or_else(|| {
                Error::Selection(format!(
                    "no ECU `{q}`; the document has: {}",
                    list(cdd.ecus.iter().map(|e| e.qual.as_str()))
                ))
            })?,
            None if cdd.ecus.len() == 1 => &cdd.ecus[0],
            None => {
                return Err(Error::Selection(format!(
                    "the document has several ECUs, choose one: {}",
                    list(cdd.ecus.iter().map(|e| e.qual.as_str()))
                )))
            }
        };
        let variant = match &self.variant {
            Some(q) => ecu.variant(q).ok_or_else(|| {
                Error::Selection(format!(
                    "no variant `{q}` in ECU `{}`; it has: {}",
                    ecu.qual,
                    list(ecu.variants.iter().map(|v| v.qual.as_str()))
                ))
            })?,
            None => ecu
                .default_variant()
                .ok_or_else(|| Error::Selection(format!("ECU `{}` has no variant", ecu.qual)))?,
        };
        let langs = preferred_languages(cdd, &self.languages);
        let rt_ident = format_ident!("{}", self.rt_crate);
        let rt = quote!(::#rt_ident);
        let mut types = Types::new(cdd, langs.clone(), rt.clone());

        let mut report = Report {
            services_total: variant.services.len(),
            ..Report::default()
        };
        let mut outs: Vec<(usize, ServiceOut)> = Vec::new();
        let mut modules = Scope::default();
        let mut variants = Scope::camel();
        for (i, svc) in variant.services.iter().enumerate() {
            let mut g = Gen::new(cdd, &mut types, rt.clone());
            let mut gen_one = |slot: &cdd_model::Slot,
                               kind: MsgKind,
                               ty: &str,
                               report: &mut Report|
             -> Option<MessageCode> {
                match slot {
                    cdd_model::Slot::Absent => None,
                    cdd_model::Slot::Unsupported(issue) => {
                        report.skipped.push(Skipped {
                            service: svc.key.clone(),
                            message: kind.label(),
                            code: issue.code,
                            reason: issue.message.clone(),
                        });
                        None
                    }
                    cdd_model::Slot::Ok(m) => match g.message(svc, m, ty) {
                        Ok(code) => Some(code),
                        Err(issue) => {
                            report.skipped.push(Skipped {
                                service: svc.key.clone(),
                                message: kind.label(),
                                code: issue.code,
                                reason: issue.message,
                            });
                            None
                        }
                    },
                }
            };
            let request = gen_one(&svc.request, MsgKind::Request, "Request", &mut report);
            let response = gen_one(&svc.positive, MsgKind::Positive, "Response", &mut report);
            if request.is_none() && response.is_none() {
                continue;
            }
            let aux = g.take_aux();
            let module = format_ident!("{}", modules.take(names::snake(&svc.shortcut)));
            let variant_name = format_ident!("{}", variants.take(names::camel(&svc.shortcut)));
            let tokens = self.service_module(
                svc,
                &module,
                request.as_ref(),
                response.as_ref(),
                &aux,
                &rt,
                &langs,
            );
            report.services_generated += 1;
            report.requests_generated += usize::from(request.is_some());
            report.responses_generated += usize::from(response.is_some());
            outs.push((
                i,
                ServiceOut {
                    module_name: module.to_string(),
                    module,
                    variant: variant_name,
                    request,
                    response,
                    tokens,
                },
            ));
        }

        if self.on_unsupported == OnUnsupported::Error && !report.skipped.is_empty() {
            let issues = report
                .skipped
                .iter()
                .map(|s| {
                    Issue::error(static_code(s.code), s.reason.clone())
                        .at(format!("{} {}", s.service, s.message))
                })
                .collect();
            return Err(Error::Unsupported(issues));
        }

        let file = self.assemble(
            cdd,
            variant,
            &outs,
            &mut types,
            &rt,
            source_name,
            bytes,
            ecu.qual.as_str(),
            &mut report,
        );
        let code = format_code(file)?;
        let header = format!(
            "// @generated by cddgen {GENERATOR_VERSION} from `{source_name}`. Do not edit: generate it again.\n\
             // ECU `{}`, variant `{}`, {}.\n\
             // Profile maturity: Experimental. The reading of the description has not been checked\n\
             // against CANoe or CANdelaStudio.\n\n",
            ecu.qual,
            variant.qual,
            cdd.protocol.label()
        );
        let code = format!("{header}{code}");
        let manifest = self.manifest(
            cdd,
            variant,
            ecu.qual.as_str(),
            source_name,
            bytes,
            &code,
            &langs,
            &outs,
            &report,
        );
        Ok(Artifacts::new(code, manifest, report))
    }

    #[allow(clippy::too_many_arguments)]
    fn manifest(
        &self,
        cdd: &Cdd,
        variant: &Variant,
        ecu: &str,
        source_name: &str,
        bytes: &[u8],
        code: &str,
        langs: &[String],
        outs: &[(usize, ServiceOut)],
        report: &Report,
    ) -> String {
        let status = |generated: bool, slot: &cdd_model::Slot| match (generated, slot) {
            (true, _) => json!("generated"),
            (false, cdd_model::Slot::Absent) => json!("absent"),
            (false, _) => json!("skipped"),
        };
        let services: Vec<serde_json::Value> = variant
            .services
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let out = outs.iter().find(|(j, _)| *j == i).map(|(_, o)| o);
                json!({
                    "key": s.key,
                    "name": s.shortcut,
                    "module": out.map(|o| o.module_name.clone()),
                    "sid": s.sid,
                    "request": status(out.is_some_and(|o| o.request.is_some()), &s.request),
                    "response": status(out.is_some_and(|o| o.response.is_some()), &s.positive),
                })
            })
            .collect();
        let skipped: Vec<serde_json::Value> = report
            .skipped
            .iter()
            .map(|s| json!({ "service": s.service, "message": s.message, "code": s.code, "reason": s.reason }))
            .collect();
        let manifest = json!({
            "generator": { "name": "cddgen", "version": GENERATOR_VERSION },
            "source": {
                "name": source_name,
                "sha256": sha256_hex(bytes),
                "encoding": cdd.encoding,
                "dtd_version": cdd.dtd_version,
            },
            "ecu": ecu,
            "variant": variant.qual,
            "protocol": cdd.protocol.label(),
            "profile_maturity": "Experimental",
            "options": {
                "role": format!("{:?}", self.role),
                "languages": langs,
                "rt_crate": self.rt_crate,
                "default_nrc": self.default_nrc,
                "on_unsupported": format!("{:?}", self.on_unsupported),
            },
            "code_sha256": sha256_hex(code.as_bytes()),
            "totals": {
                "services": report.services_total,
                "services_generated": report.services_generated,
                "requests_generated": report.requests_generated,
                "responses_generated": report.responses_generated,
                "messages_skipped": report.skipped.len(),
            },
            "services": services,
            "skipped": skipped,
            "ambiguous": report.ambiguous,
        });
        // The map is sorted by key, so the same input gives the same bytes.
        serde_json::to_string_pretty(&manifest).expect("a JSON value serializes") + "\n"
    }
    #[allow(clippy::too_many_arguments)]
    fn service_module(
        &self,
        svc: &Service,
        module: &syn::Ident,
        request: Option<&MessageCode>,
        response: Option<&MessageCode>,
        aux: &[TokenStream],
        rt: &TokenStream,
        langs: &[String],
    ) -> TokenStream {
        let name = svc.shortcut.as_str();
        let key = svc.key.as_str();
        let doc = svc.name.pick(langs).filter(|n| !n.is_empty()).map_or_else(
            || format!(" Service `{name}`"),
            |n| format!(" {n} (`{name}`)"),
        );
        let sid = match svc.sid {
            Some(s) => {
                let l = Literal::u8_unsuffixed(s);
                quote!(Some(#l))
            }
            None => quote!(None),
        };
        let nrcs: Vec<Literal> = svc
            .nrcs
            .iter()
            .map(|n| Literal::u8_unsuffixed(*n))
            .collect();
        let req = request.map(|m| &m.tokens);
        let resp = response.map(|m| &m.tokens);
        let request_impl = match (self.role.tester(), request, response, svc.sid) {
            (true, Some(rq), Some(rs), Some(sid)) => {
                let sid = Literal::u8_unsuffixed(sid);
                let (rq_ty, rs_ty) = (&rq.ty, &rs.ty);
                let rq_full = if rq.lifetime {
                    quote!(#rq_ty<'_>)
                } else {
                    quote!(#rq_ty)
                };
                let rs_full = if rs.lifetime {
                    quote!(#rs_ty<'r>)
                } else {
                    quote!(#rs_ty)
                };
                quote! {
                    impl #rt::Request for #rq_full {
                        const SID: u8 = #sid;
                        type Response<'r> = #rs_full;
                    }
                }
            }
            _ => quote!(),
        };
        quote! {
            #[doc = #doc]
            pub mod #module {
                #![allow(unused_imports)]
                use super::super::types;

                pub const NAME: &str = #name;
                pub const KEY: &str = #key;
                pub const SID: Option<u8> = #sid;
                /// The negative response codes the description lists for the service.
                pub const NRCS: &[u8] = &[ #(#nrcs),* ];

                #req
                #resp
                #request_impl
                #(#aux)*
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn assemble(
        &self,
        cdd: &Cdd,
        variant: &Variant,
        outs: &[(usize, ServiceOut)],
        types: &mut Types<'_>,
        rt: &TokenStream,
        source_name: &str,
        bytes: &[u8],
        ecu: &str,
        report: &mut Report,
    ) -> TokenStream {
        let sha = sha256_hex(bytes);
        let protocol = match cdd.protocol {
            Protocol::Uds => quote!(#rt::Protocol::Uds),
            Protocol::Kwp2000 => quote!(#rt::Protocol::Kwp2000),
            Protocol::Unknown => quote!(#rt::Protocol::Unknown),
        };
        let variant_name = variant.qual.as_str();

        let modules: Vec<&TokenStream> = outs.iter().map(|(_, o)| &o.tokens).collect();
        let type_defs = &types.defs;

        let info: Vec<TokenStream> = outs
            .iter()
            .map(|(i, o)| {
                let svc = &variant.services[*i];
                let (name, key) = (svc.shortcut.as_str(), svc.key.as_str());
                let sid = svc.sid.map_or_else(|| quote!(None), |s| {
                    let l = Literal::u8_unsuffixed(s);
                    quote!(Some(#l))
                });
                let nrcs: Vec<Literal> = svc.nrcs.iter().map(|n| Literal::u8_unsuffixed(*n)).collect();
                let (rq, rs) = (o.request.is_some(), o.response.is_some());
                let opt = |b: Option<bool>| match b {
                    Some(true) => quote!(Some(true)),
                    Some(false) => quote!(Some(false)),
                    None => quote!(None),
                };
                let (func, phys) = (opt(svc.addressing.functional), opt(svc.addressing.physical));
                quote!(#rt::ServiceInfo { name: #name, key: #key, sid: #sid, has_request: #rq, has_response: #rs, nrcs: &[ #(#nrcs),* ], functional: #func, physical: #phys })
            })
            .collect();

        let nrc_module = self.nrc_module(cdd, types.langs());
        let any_request = if self.role.ecu() {
            self.any_enum(outs, true, rt, report)
        } else {
            quote!()
        };
        let any_response = if self.role.tester() {
            self.any_enum(outs, false, rt, report)
        } else {
            quote!()
        };
        let server = if self.role.ecu() {
            self.server_module(variant, outs, rt)
        } else {
            quote!()
        };

        quote! {
            pub const DOCUMENT: &str = #source_name;
            pub const SOURCE_SHA256: &str = #sha;
            pub const ECU: &str = #ecu;
            pub const VARIANT: &str = #variant_name;
            pub const PROTOCOL: #rt::Protocol = #protocol;

            /// Every service of the variant that has code, with what the description says about it.
            pub static SERVICES: &[#rt::ServiceInfo] = &[ #(#info),* ];

            /// The enums and newtypes of the description's text tables and linear conversions.
            #[allow(clippy::all, dead_code, unreachable_patterns)]
            pub mod types {
                #(#type_defs)*
            }

            #[allow(clippy::all, dead_code, unused_variables, unused_mut, unused_comparisons, unreachable_patterns)]
            pub mod service {
                #(#modules)*
            }

            #nrc_module
            #any_request
            #any_response
            #server
        }
    }

    fn nrc_module(&self, cdd: &Cdd, langs: &[String]) -> TokenStream {
        let mut scope = Scope::default();
        let mut consts = Vec::new();
        let mut arms = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for n in &cdd.nrcs {
            let ident = format_ident!("{}", scope.take(names::shouty(&n.qual)));
            let v = Literal::u8_unsuffixed(n.value);
            consts.push(quote!(pub const #ident: u8 = #v;));
            // Two entries with one value: the first name wins, as in the interpreter.
            if seen.insert(n.value) {
                if let Some(t) = n.name.pick(langs).filter(|t| !t.is_empty()) {
                    arms.push(quote!(#v => Some(#t),));
                }
            }
        }
        quote! {
            /// The negative response codes of the description.
            #[allow(dead_code)]
            pub mod nrc {
                #(#consts)*

                /// The name the description gives a code.
                pub fn name(code: u8) -> Option<&'static str> {
                    match code {
                        #(#arms)*
                        _ => None,
                    }
                }
            }
        }
    }
    /// `AnyRequest` or `AnyResponse`: the message a payload of unknown service is, tried most
    /// specific first.
    fn any_enum(
        &self,
        outs: &[(usize, ServiceOut)],
        request: bool,
        rt: &TokenStream,
        report: &mut Report,
    ) -> TokenStream {
        let (enum_name, table_name, fn_alias, what) = if request {
            (
                format_ident!("AnyRequest"),
                format_ident!("REQUEST_TABLE"),
                format_ident!("DecodeRequest"),
                "request",
            )
        } else {
            (
                format_ident!("AnyResponse"),
                format_ident!("RESPONSE_TABLE"),
                format_ident!("DecodeResponse"),
                "positive response",
            )
        };
        let entries: Vec<(&ServiceOut, &MessageCode)> = outs
            .iter()
            .filter_map(|(_, o)| {
                (if request {
                    o.request.as_ref()
                } else {
                    o.response.as_ref()
                })
                .map(|m| (o, m))
            })
            .collect();
        if entries.is_empty() {
            return quote!();
        }
        let lt = entries.iter().any(|(_, m)| m.lifetime);
        let (enum_gen, enum_use) = if lt {
            (quote!(<'a>), quote!(#enum_name<'a>))
        } else {
            (quote!(), quote!(#enum_name))
        };
        let variants: Vec<TokenStream> = entries
            .iter()
            .map(|(o, m)| {
                let (v, module, ty) = (&o.variant, &o.module, &m.ty);
                let full = if m.lifetime {
                    quote!(service::#module::#ty<'a>)
                } else {
                    quote!(service::#module::#ty)
                };
                quote!(#v(#full))
            })
            .collect();
        let name_arms: Vec<TokenStream> = entries
            .iter()
            .map(|(o, m)| {
                let (v, module, ty) = (&o.variant, &o.module, &m.ty);
                quote!(Self::#v(_) => <service::#module::#ty as #rt::Message>::NAME,)
            })
            .collect();
        let describe_arms: Vec<TokenStream> = entries
            .iter()
            .map(|(o, _)| {
                let v = &o.variant;
                quote!(Self::#v(m) => #rt::Describe::describe(m, &#rt::Path::ROOT, v),)
            })
            .collect();

        let encode_arms: Vec<TokenStream> = entries
            .iter()
            .map(|(o, _)| {
                let v = &o.variant;
                quote!(Self::#v(m) => #rt::Encode::encode(m, out),)
            })
            .collect();

        // Most constants first: a request that fixes more bytes is the more specific.
        let mut order: Vec<usize> = (0..entries.len()).collect();
        let spec = |i: usize| -> usize { entries[i].1.prefix_bytes };
        order.sort_by_key(|i| std::cmp::Reverse(spec(*i)));
        // Services with the very same constants cannot be told apart by them.
        let mut seen: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        for (o, m) in &entries {
            if let Some(first) = seen.insert(m.prefix_key.as_str(), o.module_name.as_str()) {
                report.ambiguous.push(format!(
                    "{what}s of `{first}` and `{}` start with the same constants",
                    o.module_name.as_str()
                ));
            }
        }
        let rows: Vec<TokenStream> = order
            .iter()
            .map(|i| {
                let (o, m) = entries[*i];
                let (v, module, ty) = (&o.variant, &o.module, &m.ty);
                quote!((<service::#module::#ty as #rt::Message>::PREFIX, |p| <service::#module::#ty as #rt::Decode>::decode(p).map(#enum_name::#v)))
            })
            .collect();
        let fn_ty = if lt {
            quote!(for<'a> fn(&'a [u8]) -> Result<#enum_use, #rt::DecodeError>)
        } else {
            quote!(fn(&[u8]) -> Result<#enum_use, #rt::DecodeError>)
        };
        let doc = format!(" Any {what} of the description.");
        let method_lt = if lt { quote!(<'a>) } else { quote!() };
        let payload_ty = if lt { quote!(&'a [u8]) } else { quote!(&[u8]) };
        quote! {
            #[doc = #doc]
            #[derive(Clone, Copy, Debug, PartialEq)]
            #[allow(clippy::large_enum_variant)]
            pub enum #enum_name #enum_gen {
                #(#variants),*
            }

            type #fn_alias = #fn_ty;

            #[allow(clippy::type_complexity)]
            static #table_name: &[(&[#rt::PrefixByte], #fn_alias)] = &[ #(#rows),* ];

            impl #method_lt #enum_use {
                /// What `payload` is. `None` when no constant of any service matches, `Some(Err(_))`
                /// when one matches but the rest does not decode.
                pub fn decode(payload: #payload_ty) -> Option<Result<Self, #rt::DecodeError>> {
                    let mut first_error = None;
                    for (prefix, decode) in #table_name {
                        if #rt::msg::prefix_matches(prefix, payload) {
                            match decode(payload) {
                                Ok(m) => return Some(Ok(m)),
                                Err(e) => {
                                    first_error.get_or_insert(e);
                                }
                            }
                        }
                    }
                    first_error.map(Err)
                }

                /// The CAPL name of the service.
                pub fn name(&self) -> &'static str {
                    match self {
                        #(#name_arms)*
                    }
                }

                /// Every value with its path, raw, symbolic and physical meaning.
                pub fn describe(&self, v: &mut dyn #rt::Visitor) {
                    match self {
                        #(#describe_arms)*
                    }
                }

                /// Writes the message into out and returns its length.
                pub fn encode(&self, out: &mut [u8]) -> Result<usize, #rt::EncodeError> {
                    match self {
                        #(#encode_arms)*
                    }
                }
            }
        }
    }

    /// The simulated ECU.
    fn server_module(
        &self,
        variant: &Variant,
        outs: &[(usize, ServiceOut)],
        rt: &TokenStream,
    ) -> TokenStream {
        let default_nrc = Literal::u8_unsuffixed(self.default_nrc);
        let mut methods = Vec::new();
        let mut arms = Vec::new();
        for (i, o) in outs {
            let Some(rq) = &o.request else { continue };
            let svc = &variant.services[*i];
            let (module, rq_ty) = (&o.module, &rq.ty);
            let method = format_ident!("on_{}", o.module);
            let variant_ident = &o.variant;
            let rq_full = if rq.lifetime {
                quote!(service::#module::#rq_ty<'_>)
            } else {
                quote!(service::#module::#rq_ty)
            };
            let sid = Literal::u8_unsuffixed(svc.sid.unwrap_or(0));
            let doc = format!("The request of `{}`. The default answers with negative response code `Self::DEFAULT_NRC`.", svc.shortcut);
            let suppress = if rq.suppress {
                quote!(r.suppress_positive_response)
            } else {
                quote!(false)
            };
            match &o.response {
                Some(rs) => {
                    let rs_ty = &rs.ty;
                    let rs_full = if rs.lifetime {
                        quote!(service::#module::#rs_ty<'_>)
                    } else {
                        quote!(service::#module::#rs_ty)
                    };
                    methods.push(quote! {
                        #[doc = #doc]
                        fn #method(&mut self, request: &#rq_full) -> #rt::Reply<#rs_full> {
                            let _ = request;
                            #rt::Reply::nrc(#sid, Self::DEFAULT_NRC)
                        }
                    });
                    arms.push(quote! {
                        AnyRequest::#variant_ident(r) => {
                            let reply = self.handler.#method(&r);
                            #rt::ecu::finish(reply, #suppress, out)
                        }
                    });
                }
                None => {
                    methods.push(quote! {
                        #[doc = "The request of a service that has no response; nothing is sent."]
                        fn #method(&mut self, request: &#rq_full) {
                            let _ = request;
                        }
                    });
                    arms.push(quote! {
                        AnyRequest::#variant_ident(r) => {
                            self.handler.#method(&r);
                            Ok(0)
                        }
                    });
                }
            }
        }
        quote! {
            /// A simulated ECU: implement [`server::Handler`] for the services it should answer.
            #[allow(clippy::all, dead_code, unused_variables)]
            pub mod server {
                use super::{service, AnyRequest};
                use #rt::ecu::Responder;

                /// One method per service. Every method has a default that refuses the request, so an
                /// ECU implements only the services it simulates.
                pub trait Handler {
                    /// The negative response code for a service the handler does not implement.
                    const DEFAULT_NRC: u8 = #default_nrc;

                    #(#methods)*

                    /// A request no service of the description matches. `Some(code)` answers with that
                    /// negative response code, `None` says nothing.
                    fn on_unknown(&mut self, request: &[u8]) -> Option<u8> {
                        let _ = request;
                        Some(#rt::nrc::SERVICE_NOT_SUPPORTED)
                    }
                }

                /// Runs a [`Handler`] as a [`Responder`].
                pub struct Server<H> {
                    pub handler: H,
                }

                impl<H> Server<H> {
                    pub fn new(handler: H) -> Self {
                        Server { handler }
                    }
                }

                impl<H: Handler> Responder for Server<H> {
                    fn respond(&mut self, request: &[u8], out: &mut [u8]) -> Result<usize, #rt::EncodeError> {
                        let Some(&sid) = request.first() else { return Ok(0) };
                        match AnyRequest::decode(request) {
                            None => match self.handler.on_unknown(request) {
                                Some(nrc) => #rt::ecu::negative(out, sid, nrc),
                                None => Ok(0),
                            },
                            Some(Err(_)) => #rt::ecu::negative(out, sid, #rt::nrc::INCORRECT_MESSAGE_LENGTH_OR_INVALID_FORMAT),
                            Some(Ok(req)) => match req {
                                #(#arms)*
                            },
                        }
                    }
                }
            }
        }
    }
}

fn list<'a>(items: impl Iterator<Item = &'a str>) -> String {
    items
        .map(|s| format!("`{s}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A report code kept as text maps back to the constant it came from.
fn static_code(code: &str) -> &'static str {
    [
        codes::UNRESOLVED_REF,
        codes::UNSUPPORTED_COMPONENT,
        codes::UNSUPPORTED_DATATYPE,
        codes::MISSING_VALUE,
        codes::BAD_NUMBER,
        codes::BAD_BYTE_ORDER,
        codes::BAD_LAYOUT,
        codes::UNSUPPORTED_LAYOUT,
        codes::VARIABLE_NOT_LAST,
        codes::AMBIGUOUS_PREFIX,
    ]
    .into_iter()
    .find(|c| *c == code)
    .unwrap_or(codes::UNSUPPORTED_COMPONENT)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

fn format_code(tokens: TokenStream) -> Result<String, Error> {
    let text = tokens.to_string();
    match syn::parse_file(&text) {
        Ok(file) => Ok(prettyplease::unparse(&file)),
        Err(e) => {
            // Keep what was generated so the bug can be found.
            let path = std::env::temp_dir().join("cddgen-invalid.rs");
            let kept = std::fs::write(&path, &text)
                .map(|()| format!("; the tokens are in {}", path.display()))
                .unwrap_or_default();
            Err(Error::Internal(format!("{e}{kept}")))
        }
    }
}
