//! Turns a parsed CDD into the resolved model.
//!
//! A service is spread over three layers that point at each other with `id` references:
//!
//! * the **instance** layer (`ECU > VAR > DIAGCLASS > DIAGINST > SERVICE`) holds the values
//!   (`STATICVALUE`) and the data objects (`SIMPLECOMPCONT`);
//! * the **template** layer (`DCLTMPLS`) says which static component gets which value
//!   (`SHSTATIC`) and which proxy component gets which data (`SHPROXY`);
//! * the **protocol** layer (`PROTOCOLSERVICES`) holds the skeleton of the request, the
//!   positive response and the negative response.

use crate::datatype::{
    parse_datatype, parse_members, BitChildMember, BitsMember, ByteOrder, DataTypes, DtKind,
    Member, MuxDef, ObjMember, Qty,
};
use crate::error::{codes, Error, Issue};
use crate::ir::{
    assign_paths, items_fixed_bytes, BitChild, Bits, Const, Field, Group, Item, Message, MsgKind,
    Mux, MuxCase, NrcSlot, Repeat, RepeatCount, Role, Shape,
};
use crate::model::{Addressing, Cdd, Ecu, Nrc, Protocol, Service, Slot, Variant};
use crate::xml::{attr_int, kid, kids, kids_named, local_text, qual, tag, text_of};
use roxmltree::Node;
use std::collections::{BTreeSet, HashMap};

type R<T> = Result<T, Issue>;

/// Records inside records deeper than this are refused.
const MAX_DEPTH: usize = 8;

/// The most elements a variable-length field may have when the document names no maximum.
const DEFAULT_MAX_ELEMS: u32 = 4095;

struct DidDef {
    members: Vec<Member>,
}

struct Resolver<'a, 'i> {
    ids: HashMap<&'a str, Node<'a, 'i>>,
    dts: DataTypes,
    dids: HashMap<String, DidDef>,
    nrc_table: Vec<Nrc>,
    issues: Vec<Issue>,
    /// Component id to the name and bit width (0 when it is not a plain number) of the first item
    /// it produced in the message being built. `selref` of an iteration or a multiplexer points at
    /// one of these.
    produced: HashMap<&'a str, (String, u32)>,
    /// Data object `id` to its name and bit width (0 when it is not a plain number): a multiplexer
    /// type names the object that holds its selector with `dataObjectRef`.
    objects: HashMap<String, (String, u32)>,
}

/// What the instance supplies to one service.
struct SvcCtx<'a, 'i> {
    /// `STATICCOMP` id to the value `STATICVALUE` gives it.
    statics: HashMap<&'a str, i128>,
    /// Proxy component id to the `SIMPLECOMPCONT` that fills it.
    contents: HashMap<&'a str, Node<'a, 'i>>,
    /// Proxy component id to the `MUXCOMPCONT` that fills a multiplexer component.
    mux_contents: HashMap<&'a str, Node<'a, 'i>>,
}

struct ServiceNodes<'a, 'i> {
    class_path: Vec<String>,
    instance: Node<'a, 'i>,
    service: Node<'a, 'i>,
}

pub(crate) fn build(text: &str, encoding: String) -> Result<Cdd, Error> {
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(text, opts)?;
    let root = doc.root_element();
    if tag(root) != "CANDELA" {
        return Err(Error::NotCdd(format!(
            "the root element is `{}`, not `CANDELA`",
            tag(root)
        )));
    }
    let ecudoc =
        kid(root, "ECUDOC").ok_or_else(|| Error::NotCdd("there is no ECUDOC element".into()))?;

    let mut ids = HashMap::new();
    for n in doc.descendants().filter(Node::is_element) {
        if let Some(id) = n.attribute("id") {
            ids.entry(id).or_insert(n);
        }
    }

    let mut issues = Vec::new();
    let mut dts = kid(ecudoc, "DATATYPES")
        .map(|d| DataTypes::parse(d, &mut issues))
        .unwrap_or_default();
    dts.add_inline(ecudoc, &mut issues);
    let nrc_table = kid(ecudoc, "NEGRESCODES")
        .map(|t| {
            kids_named(t, "NEGRESCODE")
                .filter_map(|n| {
                    let value = attr_int(n, "v").filter(|v| (0..=255).contains(v))? as u8;
                    Some(Nrc {
                        value,
                        qual: qual(n).unwrap_or_default(),
                        name: local_text(n, "NAME"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let dids = kid(ecudoc, "DIDS")
        .map(|d| {
            kids_named(d, "DID")
                .filter_map(|n| {
                    let id = n.attribute("id")?.to_string();
                    let members = kid(n, "STRUCTURE").map(parse_members).unwrap_or_default();
                    Some((id, DidDef { members }))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut r = Resolver {
        ids,
        dts,
        dids,
        nrc_table,
        issues,
        produced: HashMap::new(),
        objects: HashMap::new(),
    };

    let protocol = match kid(ecudoc, "PROTOCOLSTANDARD").map(text_of).as_deref() {
        Some("UDS") => Protocol::Uds,
        Some("KWP") => Protocol::Kwp2000,
        _ => Protocol::Unknown,
    };
    let languages = ecudoc
        .attribute("languages")
        .map(|l| {
            l.trim_matches(|c| c == '(' || c == ')')
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let ecus = kids_named(ecudoc, "ECU").map(|e| r.build_ecu(e)).collect();

    Ok(Cdd {
        protocol,
        dtd_version: root.attribute("dtdvers").unwrap_or("").to_string(),
        encoding,
        languages,
        datatypes: r.dts,
        nrcs: r.nrc_table,
        ecus,
        issues: r.issues,
    })
}

fn role_of(spec: Option<&str>) -> Role {
    match spec {
        Some("sid") => Role::Sid,
        Some("sub") => Role::Sub,
        Some("id" | "lid" | "did" | "rid") => Role::Id,
        _ => Role::Other,
    }
}

fn all_ones(bits: u32) -> u64 {
    if bits >= 64 {
        u64::MAX
    } else {
        (1u64 << bits) - 1
    }
}

fn is_component(t: &str) -> bool {
    matches!(
        t,
        "CONSTCOMP"
            | "STATICCOMP"
            | "SIMPLEPROXYCOMP"
            | "STATUSDTCPROXYCOMP"
            | "GROUPOFDTCPROXYCOMP"
            | "CONTENTCOMP"
            | "EOSITERCOMP"
            | "NUMITERCOMP"
            | "MUXCOMP"
            | "DOMAINDATAPROXYCOMP"
    )
}

fn flag(n: Node<'_, '_>, name: &str) -> Option<bool> {
    match n.attribute(name)? {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

impl<'a, 'i> Resolver<'a, 'i> {
    fn build_ecu(&mut self, ecu: Node<'a, 'i>) -> Ecu {
        let qual_ = qual(ecu).unwrap_or_else(|| "Ecu".to_string());
        let variants = kids_named(ecu, "VAR")
            .map(|v| self.build_variant(&qual_, v))
            .collect();
        Ecu {
            qual: qual_,
            name: local_text(ecu, "NAME"),
            variants,
        }
    }

    fn build_variant(&mut self, ecu: &str, var: Node<'a, 'i>) -> Variant {
        let qual_ = qual(var).unwrap_or_else(|| "Variant".to_string());
        let mut found = Vec::new();
        self.collect_services(var, &mut Vec::new(), &mut found);

        let mut keys: HashMap<String, usize> = HashMap::new();
        let mut shortcuts: HashMap<String, usize> = HashMap::new();
        let mut services = Vec::with_capacity(found.len());
        for sn in found {
            let mut svc = self.build_service(ecu, &qual_, &sn);
            svc.key = dedupe(&mut keys, svc.key, "~");
            svc.shortcut = dedupe(&mut shortcuts, svc.shortcut, "_");
            services.push(svc);
        }
        Variant {
            qual: qual_,
            name: local_text(var, "NAME"),
            base: var.attribute("base") == Some("1"),
            services,
        }
    }

    /// Every `SERVICE` below the variant: instances inside classes, and instances borrowed from the
    /// base variant with `DIAGINSTREF`.
    fn collect_services(
        &self,
        node: Node<'a, 'i>,
        class_path: &mut Vec<String>,
        out: &mut Vec<ServiceNodes<'a, 'i>>,
    ) {
        for c in kids(node) {
            match tag(c) {
                "DIAGCLASS" => {
                    class_path.push(qual(c).unwrap_or_default());
                    self.collect_services(c, class_path, out);
                    class_path.pop();
                }
                "DIAGINST" => self.collect_instance(c, class_path, out),
                "DIAGINSTREF" => {
                    if let Some(target) =
                        c.attribute("idref").and_then(|r| self.ids.get(r)).copied()
                    {
                        if tag(target) == "DIAGINST" {
                            self.collect_instance(target, class_path, out);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn collect_instance(
        &self,
        inst: Node<'a, 'i>,
        class_path: &[String],
        out: &mut Vec<ServiceNodes<'a, 'i>>,
    ) {
        for s in kids_named(inst, "SERVICE") {
            out.push(ServiceNodes {
                class_path: class_path.to_vec(),
                instance: inst,
                service: s,
            });
        }
    }

    /// Follows `tmplref` from a `DCLSRVTMPL` to the `PROTOCOLSERVICE` it is built on.
    fn protocol_service(&self, service: Node<'a, 'i>) -> R<Node<'a, 'i>> {
        let mut cur = service;
        for _ in 0..6 {
            let r = cur.attribute("tmplref").ok_or_else(|| {
                Issue::error(
                    codes::UNRESOLVED_REF,
                    format!("<{}> has no tmplref", tag(cur)),
                )
            })?;
            let next = *self.ids.get(r).ok_or_else(|| {
                Issue::error(
                    codes::UNRESOLVED_REF,
                    format!("tmplref `{r}` points at nothing"),
                )
            })?;
            if tag(next) == "PROTOCOLSERVICE" {
                return Ok(next);
            }
            cur = next;
        }
        Err(Issue::error(
            codes::UNRESOLVED_REF,
            "the template chain does not end at a protocol service",
        ))
    }

    fn build_service(&mut self, ecu: &str, var: &str, sn: &ServiceNodes<'a, 'i>) -> Service {
        let inst_q = qual(sn.instance).unwrap_or_default();
        let svc_q = qual(sn.service).unwrap_or_default();
        let mut key = format!("{ecu}/{var}");
        for c in &sn.class_path {
            key.push('/');
            key.push_str(c);
        }
        key = format!("{key}/{inst_q}/{svc_q}");
        let shortcut = kid(sn.service, "SHORTCUTQUAL")
            .map(text_of)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("{inst_q}_{svc_q}"));
        let addressing = Addressing {
            functional: flag(sn.service, "func"),
            physical: flag(sn.service, "phys"),
            resp_on_physical: flag(sn.service, "respOnPhys"),
            resp_on_functional: flag(sn.service, "respOnFunc"),
        };
        let mut svc = Service {
            key: key.clone(),
            class_path: sn.class_path.clone(),
            instance: inst_q,
            service: svc_q,
            shortcut,
            name: local_text(sn.service, "NAME"),
            protocol_service: String::new(),
            sid: None,
            addressing,
            request: Slot::Absent,
            positive: Slot::Absent,
            negative: Slot::Absent,
            nrcs: Vec::new(),
        };

        let ps = match self.protocol_service(sn.service) {
            Ok(ps) => ps,
            Err(issue) => {
                let issue = issue.at(key);
                self.issues.push(issue.clone());
                svc.request = Slot::Unsupported(issue.clone());
                svc.positive = Slot::Unsupported(issue.clone());
                svc.negative = Slot::Unsupported(issue);
                return svc;
            }
        };
        svc.protocol_service = qual(ps).unwrap_or_default();
        let ctx = self.service_ctx(sn.instance);
        svc.nrcs = self.nrc_values(sn.instance, ps);

        for (kind, tag_name) in [
            (MsgKind::Request, "REQ"),
            (MsgKind::Positive, "POS"),
            (MsgKind::Negative, "NEG"),
        ] {
            let slot = match kid(ps, tag_name) {
                None => Slot::Absent,
                Some(msg) => match self.build_message(kind, msg, &ctx) {
                    Ok(m) => Slot::Ok(m),
                    Err(issue) => Slot::Unsupported(issue.at(format!("{key} {}", kind.label()))),
                },
            };
            match kind {
                MsgKind::Request => svc.request = slot,
                MsgKind::Positive => svc.positive = slot,
                MsgKind::Negative => svc.negative = slot,
            }
        }
        for slot in [&svc.request, &svc.positive] {
            if let Slot::Unsupported(i) = slot {
                self.issues.push(i.clone());
            }
        }
        svc.sid = svc.request.message().and_then(|m| {
            m.items.iter().find_map(|i| match i {
                Item::Const(c) if c.role == Role::Sid && c.bits == 8 => Some(c.value as u8),
                _ => None,
            })
        });
        svc
    }

    /// The values and data objects the instance gives to the template's static and proxy slots.
    fn service_ctx(&self, inst: Node<'a, 'i>) -> SvcCtx<'a, 'i> {
        let mut statics = HashMap::new();
        let mut contents = HashMap::new();
        let mut mux_contents = HashMap::new();
        for c in kids(inst) {
            match tag(c) {
                "STATICVALUE" => {
                    let (Some(shs), Some(v)) = (
                        c.attribute("shstaticref").and_then(|r| self.ids.get(r)),
                        attr_int(c, "v"),
                    ) else {
                        continue;
                    };
                    for r in kids_named(*shs, "STATICCOMPREF") {
                        if let Some(id) = r.attribute("idref") {
                            statics.insert(id, v);
                        }
                    }
                }
                "SIMPLECOMPCONT" => {
                    let Some(shp) = c.attribute("shproxyref").and_then(|r| self.ids.get(r)) else {
                        continue;
                    };
                    for r in kids_named(*shp, "PROXYCOMPREF") {
                        if let Some(id) = r.attribute("idref") {
                            contents.insert(id, c);
                        }
                    }
                }
                "MUXCOMPCONT" => {
                    let Some(shp) = c.attribute("shproxyref").and_then(|r| self.ids.get(r)) else {
                        continue;
                    };
                    for r in kids_named(*shp, "PROXYCOMPREF") {
                        if let Some(id) = r.attribute("idref") {
                            mux_contents.insert(id, c);
                        }
                    }
                }
                _ => {}
            }
        }
        SvcCtx {
            statics,
            contents,
            mux_contents,
        }
    }

    /// The negative response codes a service lists: the instance's own list, else the protocol's.
    fn nrc_values(&self, inst: Node<'a, 'i>, ps: Node<'a, 'i>) -> Vec<u8> {
        let collect = |root: Node<'a, 'i>| -> BTreeSet<u8> {
            root.descendants()
                .filter(|n| tag(*n) == "NEGRESCODEPROXY")
                .filter_map(|p| self.ids.get(p.attribute("idref")?))
                .filter_map(|n| {
                    attr_int(*n, "v")
                        .filter(|v| (0..=255).contains(v))
                        .map(|v| v as u8)
                })
                .collect()
        };
        let own: BTreeSet<u8> = kids_named(inst, "SIMPLECOMPCONT")
            .flat_map(collect)
            .collect();
        let set = if own.is_empty() { collect(ps) } else { own };
        set.into_iter().collect()
    }

    fn build_message(
        &mut self,
        kind: MsgKind,
        msg: Node<'a, 'i>,
        ctx: &SvcCtx<'a, 'i>,
    ) -> R<Message> {
        self.produced.clear();
        self.objects.clear();
        let mut items = Vec::new();
        for comp in kids(msg).filter(|c| is_component(tag(*c))) {
            items.extend(self.component_items(comp, ctx, 0)?);
        }
        check_variable_last(&items, &self.dts)?;
        assign_paths(&mut items);
        Ok(Message {
            kind,
            qual: qual(msg).unwrap_or_default(),
            items,
        })
    }

    /// The items of one component, remembering the name of the first so a later component can point
    /// at it with `selref`.
    fn component_items(
        &mut self,
        comp: Node<'a, 'i>,
        ctx: &SvcCtx<'a, 'i>,
        depth: usize,
    ) -> R<Vec<Item>> {
        let items = self.component_items_inner(comp, ctx, depth)?;
        if let (Some(id), Some(first)) = (comp.attribute("id"), items.first()) {
            if let Some(name) = item_name(first) {
                let bits = self.number_bits(first);
                self.produced.insert(id, (name.to_string(), bits));
            }
        }
        Ok(items)
    }

    fn component_items_inner(
        &mut self,
        comp: Node<'a, 'i>,
        ctx: &SvcCtx<'a, 'i>,
        depth: usize,
    ) -> R<Vec<Item>> {
        match tag(comp) {
            "CONSTCOMP" => {
                let value = attr_int(comp, "v").ok_or_else(|| {
                    Issue::error(
                        codes::MISSING_VALUE,
                        format!("constant `{}` has no value", qual(comp).unwrap_or_default()),
                    )
                })?;
                Ok(vec![self.fixed_item(comp, value, None)?])
            }
            "STATICCOMP" => {
                let name = qual(comp).unwrap_or_default();
                let value = comp
                    .attribute("id")
                    .and_then(|id| ctx.statics.get(id))
                    .copied()
                    .ok_or_else(|| {
                        Issue::error(
                            codes::MISSING_VALUE,
                            format!("the instance gives static component `{name}` no value"),
                        )
                    })?;
                let dt = comp.attribute("dtref").and_then(|r| self.dts.index(r));
                Ok(vec![self.fixed_item(comp, value, dt)?])
            }
            "SIMPLEPROXYCOMP" | "STATUSDTCPROXYCOMP" | "GROUPOFDTCPROXYCOMP" => {
                self.proxy_items(comp, ctx, depth)
            }
            "CONTENTCOMP" => match kid(comp, "SIMPLECOMPCONT") {
                Some(content) => self.members_to_items(&parse_members(content), depth),
                None => Ok(Vec::new()),
            },
            "EOSITERCOMP" | "NUMITERCOMP" => self.repeat_item(comp, ctx, depth).map(|r| vec![r]),
            "MUXCOMP" => self.mux_component(comp, ctx, depth),
            other => Err(Issue::error(
                codes::UNSUPPORTED_COMPONENT,
                format!(
                    "component {other} `{}` is not supported yet",
                    qual(comp).unwrap_or_default()
                ),
            )),
        }
    }

    /// Width in bits when the item is one unsigned number (what a count or a selector must be), else 0.
    fn number_bits(&self, item: &Item) -> u32 {
        match item {
            Item::Const(c) => c.bits,
            Item::Field(f) => match (f.dt, f.shape) {
                (Some(d), Shape::Atom)
                    if self.dts.get(d).coded.enc == crate::datatype::Enc::Unsigned =>
                {
                    self.dts.get(d).coded.bits
                }
                // A raw slot of a few bytes is read as a big-endian number.
                (None, Shape::Array { count }) if (1..=8).contains(&count) => count * 8,
                _ => 0,
            },
            _ => 0,
        }
    }

    /// A `CONSTCOMP`, or a `STATICCOMP` with the value its instance gave it.
    fn fixed_item(&self, comp: Node<'a, 'i>, value: i128, dt: Option<usize>) -> R<Item> {
        let name = qual(comp).unwrap_or_default();
        let (bits, order) = match dt {
            Some(d) => {
                let c = self.dts.get(d).coded;
                if c.qty == Qty::Field && c.min != c.max {
                    return Err(Issue::error(
                        codes::UNSUPPORTED_LAYOUT,
                        format!("fixed component `{name}` has a variable length"),
                    ));
                }
                (
                    c.bits * if c.qty == Qty::Field { c.min } else { 1 },
                    c.order,
                )
            }
            None => {
                let bl = attr_int(comp, "bl").ok_or_else(|| {
                    Issue::error(
                        codes::BAD_NUMBER,
                        format!("component `{name}` has no bit length"),
                    )
                })?;
                (bl.clamp(0, 1_000_000) as u32, ByteOrder::Big)
            }
        };
        if bits == 0 || !bits.is_multiple_of(8) || bits > 64 {
            return Err(Issue::error(
                codes::UNSUPPORTED_LAYOUT,
                format!("fixed component `{name}` is {bits} bits wide: only whole bytes up to 8 are supported"),
            ));
        }
        if value < 0 || value > i128::from(all_ones(bits)) {
            return Err(Issue::error(
                codes::BAD_NUMBER,
                format!("value {value} of `{name}` does not fit in {bits} bits"),
            ));
        }
        let role = role_of(comp.attribute("spec"));
        let suppress = role == Role::Sub && comp.attribute("respsupbit") == Some("1");
        let full = all_ones(bits);
        let mask = if suppress {
            full & !(1u64 << (bits - 1))
        } else {
            full
        };
        Ok(Item::Const(Const {
            name,
            path: String::new(),
            role,
            value: value as u64,
            bits,
            order,
            mask,
            suppress_bit: suppress,
        }))
    }

    fn proxy_items(
        &mut self,
        comp: Node<'a, 'i>,
        ctx: &SvcCtx<'a, 'i>,
        depth: usize,
    ) -> R<Vec<Item>> {
        let name = qual(comp).unwrap_or_default();
        if comp.attribute("dest") == Some("resCode") {
            return Ok(vec![Item::Nrc(NrcSlot {
                name,
                path: String::new(),
            })]);
        }
        if let Some(content) = comp
            .attribute("id")
            .and_then(|id| ctx.contents.get(id))
            .copied()
        {
            return self.members_to_items(&parse_members(content), depth);
        }
        // The instance gives the slot no content.
        if let Some(d) = comp.attribute("dtref").and_then(|r| self.dts.index(r)) {
            return Ok(vec![self.field_item(&name, d, None)?]);
        }
        if comp.attribute("must") == Some("0") {
            return Ok(Vec::new());
        }
        // Declared but never described: raw bytes of the declared size.
        let min_bits = attr_int(comp, "minbl").unwrap_or(8).clamp(0, 1 << 20) as u32;
        let max_bits = attr_int(comp, "maxbl").map(|b| b.clamp(0, 1 << 24) as u32);
        let (min, max) = (min_bits / 8, max_bits.map_or(DEFAULT_MAX_ELEMS, |b| b / 8));
        if !min_bits.is_multiple_of(8) || max_bits.is_some_and(|b| !b.is_multiple_of(8)) {
            return Err(Issue::error(
                codes::UNSUPPORTED_LAYOUT,
                format!("raw slot `{name}` is not a whole number of bytes"),
            ));
        }
        let shape = if min == max {
            Shape::Array { count: min }
        } else {
            Shape::Rest { min, max }
        };
        Ok(vec![Item::Field(Field {
            name,
            path: String::new(),
            dt: None,
            shape,
            default: None,
        })])
    }

    fn repeat_item(&mut self, comp: Node<'a, 'i>, ctx: &SvcCtx<'a, 'i>, depth: usize) -> R<Item> {
        let name = qual(comp).unwrap_or_default();
        if depth >= MAX_DEPTH {
            return Err(Issue::error(
                codes::UNSUPPORTED_LAYOUT,
                format!("`{name}` is nested too deeply"),
            ));
        }
        let mut element = Vec::new();
        for c in kids(comp).filter(|c| is_component(tag(*c))) {
            element.extend(self.component_items(c, ctx, depth + 1)?);
        }
        match items_fixed_bytes(&element, &self.dts) {
            Some(n) if n > 0 => {}
            _ => {
                return Err(Issue::error(
                    codes::UNSUPPORTED_LAYOUT,
                    format!("repeated element of `{name}` has no fixed size"),
                ))
            }
        }
        let count = if tag(comp) == "NUMITERCOMP" {
            let selref = comp.attribute("selref").ok_or_else(|| {
                Issue::error(
                    codes::UNRESOLVED_REF,
                    format!("`{name}` names no component that holds its count"),
                )
            })?;
            let (field, bits) = self.produced.get(selref).cloned().ok_or_else(|| {
                Issue::error(
                    codes::UNRESOLVED_REF,
                    format!("the count of `{name}` is not an earlier component of the message"),
                )
            })?;
            if bits == 0 {
                return Err(Issue::error(
                    codes::UNSUPPORTED_LAYOUT,
                    format!("the count `{field}` of `{name}` is not a plain unsigned number"),
                ));
            }
            let mask = attr_int(comp, "selbm")
                .filter(|m| *m > 0 && *m <= i128::from(u64::MAX))
                .map_or(u64::MAX, |m| m as u64);
            RepeatCount::Counted {
                field: vec![field],
                mask: mask & all_ones(bits),
            }
        } else {
            let min = attr_int(comp, "minNumOfItems")
                .unwrap_or(0)
                .clamp(0, u32::MAX as i128) as u32;
            let max = attr_int(comp, "maxNumOfItems").map(|m| m.clamp(0, u32::MAX as i128) as u32);
            RepeatCount::ToEnd { min, max }
        };
        Ok(Item::Repeat(Repeat {
            name,
            path: String::new(),
            element,
            count,
        }))
    }

    fn members_to_items(&mut self, members: &[Member], depth: usize) -> R<Vec<Item>> {
        if depth > MAX_DEPTH {
            return Err(Issue::error(
                codes::UNSUPPORTED_LAYOUT,
                "records are nested too deeply",
            ));
        }
        let mut out = Vec::new();
        for m in members {
            match m {
                Member::Obj(o) => out.push(self.obj_item(o, depth)?),
                Member::Bits(b) => out.push(self.bits_item(b)?),
                Member::Gap { bits } => {
                    let bits = bits.ok_or_else(|| {
                        Issue::error(codes::BAD_NUMBER, "a gap has no bit length")
                    })?;
                    if bits == 0 || !bits.is_multiple_of(8) {
                        return Err(Issue::error(
                            codes::UNSUPPORTED_LAYOUT,
                            format!("a gap of {bits} bits outside a bit container"),
                        ));
                    }
                    out.push(Item::Gap(bits / 8));
                }
                Member::DidRef { name, did } => {
                    let did_id = did.as_deref().ok_or_else(|| {
                        Issue::error(
                            codes::UNRESOLVED_REF,
                            format!("DID reference `{name}` names no DID"),
                        )
                    })?;
                    let members = self
                        .dids
                        .get(did_id)
                        .map(|d| d.members.clone())
                        .ok_or_else(|| {
                            Issue::error(
                                codes::UNRESOLVED_REF,
                                format!("DID `{did_id}` does not exist"),
                            )
                        })?;
                    let items = self.members_to_items(&members, depth + 1)?;
                    out.push(Item::Group(Group {
                        name: name.clone(),
                        path: String::new(),
                        items,
                    }));
                }
                Member::Other { tag, name } => {
                    return Err(Issue::error(
                        codes::UNSUPPORTED_COMPONENT,
                        format!("{tag} `{name}` is not supported yet"),
                    ));
                }
            }
        }
        Ok(out)
    }

    /// A `MUXCOMP`: the instance gives it a `MUXDT` of its own, whose case is chosen by the value
    /// of the earlier component `selref`.
    fn mux_component(
        &mut self,
        comp: Node<'a, 'i>,
        ctx: &SvcCtx<'a, 'i>,
        depth: usize,
    ) -> R<Vec<Item>> {
        let name = qual(comp).unwrap_or_default();
        let selref = comp.attribute("selref").ok_or_else(|| {
            Issue::error(
                codes::UNRESOLVED_REF,
                format!("multiplexer `{name}` names no component that holds its selector"),
            )
        })?;
        let (selector, bits) = self.produced.get(selref).cloned().ok_or_else(|| {
            Issue::error(
                codes::UNRESOLVED_REF,
                format!("the selector of `{name}` is not an earlier component of the message"),
            )
        })?;
        let Some(content) = comp
            .attribute("id")
            .and_then(|id| ctx.mux_contents.get(id))
            .copied()
        else {
            if comp.attribute("must") == Some("0") {
                return Ok(Vec::new());
            }
            return Err(Issue::error(
                codes::MISSING_VALUE,
                format!("the instance gives multiplexer `{name}` no content"),
            ));
        };
        let dt_node = kid(content, "MUXDT")
            .ok_or_else(|| Issue::error(codes::UNSUPPORTED_COMPONENT, format!("the content of multiplexer `{name}` is not a MUXDT (a default content is not supported yet)")))?;
        let dt = parse_datatype(dt_node, &name, dt_node.attribute("id").unwrap_or(""))?;
        let DtKind::Mux(def) = dt.kind else {
            return Err(Issue::error(
                codes::UNSUPPORTED_COMPONENT,
                format!("the content of multiplexer `{name}` is not a MUXDT"),
            ));
        };
        let mask = attr_int(comp, "selbm")
            .filter(|m| *m > 0 && *m <= i128::from(u64::MAX))
            .map_or(u64::MAX, |m| m as u64);
        Ok(vec![
            self.mux_item(&name, &def, selector, bits, mask, depth)?
        ])
    }

    /// The structure of the case the selector chooses, or the default structure.
    fn mux_item(
        &mut self,
        name: &str,
        def: &MuxDef,
        selector: String,
        bits: u32,
        mask: u64,
        depth: usize,
    ) -> R<Item> {
        let default = self.members_to_items(&def.default, depth + 1)?;
        check_variable_last(&default, &self.dts)?;
        if def.cases.is_empty() {
            // Nothing to choose from: the default is the structure.
            return Ok(Item::Group(Group {
                name: name.to_string(),
                path: String::new(),
                items: default,
            }));
        }
        let mut cases = Vec::new();
        for c in &def.cases {
            let items = self.members_to_items(&c.members, depth + 1)?;
            check_variable_last(&items, &self.dts)?;
            cases.push(MuxCase {
                lo: c.lo,
                hi: c.hi,
                items,
            });
        }
        if bits == 0 {
            return Err(Issue::error(
                codes::UNSUPPORTED_LAYOUT,
                format!("the selector `{selector}` of `{name}` is not a plain unsigned number"),
            ));
        }
        if cases.iter().any(|c| c.lo < 0) {
            return Err(Issue::error(
                codes::UNSUPPORTED_LAYOUT,
                format!("multiplexer `{name}` has a case below zero"),
            ));
        }
        let mut ranges: Vec<(i128, i128)> = cases.iter().map(|c| (c.lo, c.hi)).collect();
        ranges.sort_unstable();
        if ranges.windows(2).any(|w| w[0].1 >= w[1].0) {
            return Err(Issue::error(
                codes::BAD_LAYOUT,
                format!("the cases of multiplexer `{name}` overlap"),
            ));
        }
        Ok(Item::Mux(Mux {
            name: name.to_string(),
            path: String::new(),
            selector: vec![selector],
            mask: mask & all_ones(bits),
            default,
            cases,
        }))
    }
    fn obj_item(&mut self, o: &ObjMember, depth: usize) -> R<Item> {
        let dtref = o.dtref.as_deref().ok_or_else(|| {
            Issue::error(
                codes::UNRESOLVED_REF,
                format!("data object `{}` has no dtref", o.name),
            )
        })?;
        let id = self.dts.index(dtref).ok_or_else(|| {
            Issue::error(
                codes::UNRESOLVED_REF,
                format!("data type `{dtref}` of `{}` does not exist", o.name),
            )
        })?;
        enum K {
            Plain,
            Record(Vec<Member>),
            Mux(MuxDef),
            Unsupported(String),
        }
        let k = match &self.dts.get(id).kind {
            DtKind::Ident | DtKind::Text(_) | DtKind::Linear(_) => K::Plain,
            DtKind::Record(children) => K::Record(children.clone()),
            DtKind::Mux(def) => K::Mux(def.clone()),
            DtKind::Unsupported(t) => K::Unsupported(t.clone()),
        };
        match k {
            K::Plain => {
                let item = self.field_item(&o.name, id, o.default)?;
                if let Some(object_id) = &o.id {
                    let bits = self.number_bits(&item);
                    self.objects
                        .insert(object_id.clone(), (o.name.clone(), bits));
                }
                Ok(item)
            }
            K::Mux(def) => {
                let selector_ref = o.object_ref.as_deref().ok_or_else(|| {
                    Issue::error(
                        codes::UNRESOLVED_REF,
                        format!(
                            "multiplexer `{}` names no data object that holds its selector",
                            o.name
                        ),
                    )
                })?;
                let (selector, bits) =
                    self.objects.get(selector_ref).cloned().ok_or_else(|| {
                        Issue::error(
                            codes::UNRESOLVED_REF,
                            format!(
                                "the selector of `{}` is not an earlier data object of the message",
                                o.name
                            ),
                        )
                    })?;
                self.mux_item(&o.name, &def, selector, bits, def.mask, depth)
            }
            K::Record(children) => {
                let items = self.members_to_items(&children, depth + 1)?;
                Ok(Item::Group(Group {
                    name: o.name.clone(),
                    path: String::new(),
                    items,
                }))
            }
            K::Unsupported(t) => Err(Issue::error(
                codes::UNSUPPORTED_DATATYPE,
                format!(
                    "data object `{}` has data type {t}, which is not supported yet",
                    o.name
                ),
            )),
        }
    }

    fn field_item(&self, name: &str, dt: usize, default: Option<i128>) -> R<Item> {
        let c = self.dts.get(dt).coded;
        if !c.bits.is_multiple_of(8) {
            return Err(Issue::error(
                codes::UNSUPPORTED_LAYOUT,
                format!(
                    "`{name}` is {} bits wide and is not inside a bit container",
                    c.bits
                ),
            ));
        }
        let shape = match c.qty {
            Qty::Atom => {
                if c.bits > 64 {
                    return Err(Issue::error(
                        codes::UNSUPPORTED_LAYOUT,
                        format!("`{name}` is {} bits wide: at most 64 are supported", c.bits),
                    ));
                }
                Shape::Atom
            }
            Qty::Field if c.min == c.max => Shape::Array { count: c.min },
            Qty::Field => Shape::Rest {
                min: c.min,
                max: c.max,
            },
        };
        Ok(Item::Field(Field {
            name: name.to_string(),
            path: String::new(),
            dt: Some(dt),
            shape,
            default,
        }))
    }

    fn bits_item(&self, b: &BitsMember) -> R<Item> {
        let dtref = b.dtref.as_deref().ok_or_else(|| {
            Issue::error(
                codes::UNRESOLVED_REF,
                format!("bit container `{}` has no dtref", b.name),
            )
        })?;
        let cdt = self.dts.by_id(dtref).ok_or_else(|| {
            Issue::error(
                codes::UNRESOLVED_REF,
                format!("data type `{dtref}` of `{}` does not exist", b.name),
            )
        })?;
        let c = cdt.coded;
        if c.qty == Qty::Field && c.min != c.max {
            return Err(Issue::error(
                codes::UNSUPPORTED_LAYOUT,
                format!("bit container `{}` has a variable size", b.name),
            ));
        }
        let bits = c.bits * if c.qty == Qty::Field { c.min.max(1) } else { 1 };
        if bits == 0 || !bits.is_multiple_of(8) || bits > 64 {
            return Err(Issue::error(
                codes::UNSUPPORTED_LAYOUT,
                format!("bit container `{}` is {bits} bits wide", b.name),
            ));
        }
        let mut offset = 0u32;
        let mut children = Vec::new();
        for ch in &b.children {
            match ch {
                BitChildMember::Obj(o) => {
                    let dt = o
                        .dtref
                        .as_deref()
                        .and_then(|r| self.dts.index(r))
                        .ok_or_else(|| {
                            Issue::error(
                                codes::UNRESOLVED_REF,
                                format!("data type of bit field `{}` does not resolve", o.name),
                            )
                        })?;
                    let d = self.dts.get(dt);
                    if !matches!(d.kind, DtKind::Ident | DtKind::Text(_) | DtKind::Linear(_))
                        || d.coded.qty != Qty::Atom
                    {
                        return Err(Issue::error(
                            codes::UNSUPPORTED_LAYOUT,
                            format!("bit field `{}` is not a single plain value", o.name),
                        ));
                    }
                    let width = d.coded.bits;
                    children.push(BitChild::Field {
                        name: o.name.clone(),
                        path: String::new(),
                        dt,
                        offset,
                        width,
                        default: o.default,
                    });
                    offset += width;
                }
                BitChildMember::Gap { bits: Some(w) } => {
                    children.push(BitChild::Gap { offset, width: *w });
                    offset += *w;
                }
                BitChildMember::Gap { bits: None } => {
                    return Err(Issue::error(
                        codes::BAD_NUMBER,
                        format!("a gap in `{}` has no bit length", b.name),
                    ));
                }
                BitChildMember::Other(t) => {
                    return Err(Issue::error(
                        codes::UNSUPPORTED_LAYOUT,
                        format!("`{}` contains {t}", b.name),
                    ));
                }
            }
        }
        if offset > bits {
            return Err(Issue::error(
                codes::BAD_LAYOUT,
                format!(
                    "the children of `{}` need {offset} bits but the container has {bits}",
                    b.name
                ),
            ));
        }
        Ok(Item::Bits(Bits {
            name: b.name.clone(),
            path: String::new(),
            bits,
            order: c.order,
            children,
        }))
    }
}

fn item_name(item: &Item) -> Option<&str> {
    match item {
        Item::Const(c) => Some(&c.name),
        Item::Field(f) => Some(&f.name),
        Item::Group(g) => Some(&g.name),
        Item::Bits(b) => Some(&b.name),
        Item::Repeat(r) => Some(&r.name),
        Item::Mux(m) => Some(&m.name),
        Item::Nrc(n) => Some(&n.name),
        Item::Gap(_) => None,
    }
}

/// An item whose size varies must be the last of its message.
fn check_variable_last(items: &[Item], dts: &DataTypes) -> R<()> {
    for (i, item) in items.iter().enumerate() {
        let last = i + 1 == items.len();
        if !last && item.fixed_bytes(dts).is_none() {
            let name = match item {
                Item::Field(f) => f.name.as_str(),
                Item::Group(g) => g.name.as_str(),
                Item::Repeat(r) => r.name.as_str(),
                Item::Mux(m) => m.name.as_str(),
                _ => "",
            };
            return Err(Issue::error(
                codes::VARIABLE_NOT_LAST,
                format!("`{name}` has a variable length but items follow it"),
            ));
        }
    }
    Ok(())
}

fn dedupe(seen: &mut HashMap<String, usize>, name: String, sep: &str) -> String {
    let n = seen.entry(name.clone()).or_insert(0);
    *n += 1;
    if *n == 1 {
        name
    } else {
        format!("{name}{sep}{n}")
    }
}
