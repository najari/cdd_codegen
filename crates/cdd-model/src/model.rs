//! The resolved model of a CDD: ECUs, variants, and for every service its three messages.

use crate::datatype::DataTypes;
use crate::error::Issue;
use crate::ir::{Message, MsgKind};
use crate::xml::LocalText;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Protocol {
    Uds,
    Kwp2000,
    /// No `PROTOCOLSTANDARD` in the document.
    Unknown,
}

impl Protocol {
    pub fn label(self) -> &'static str {
        match self {
            Protocol::Uds => "UDS",
            Protocol::Kwp2000 => "KWP2000",
            Protocol::Unknown => "unknown",
        }
    }
}

/// A message the service has, lacks, or has but cannot be generated.
#[derive(Clone, Debug, PartialEq)]
pub enum Slot {
    Absent,
    Ok(Message),
    Unsupported(Issue),
}

impl Slot {
    pub fn message(&self) -> Option<&Message> {
        match self {
            Slot::Ok(m) => Some(m),
            _ => None,
        }
    }

    pub fn issue(&self) -> Option<&Issue> {
        match self {
            Slot::Unsupported(i) => Some(i),
            _ => None,
        }
    }
}

/// How the service may be addressed (`func`, `phys`, `respOnPhys`, `respOnFunc`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Addressing {
    pub functional: Option<bool>,
    pub physical: Option<bool>,
    pub resp_on_physical: Option<bool>,
    pub resp_on_functional: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Service {
    /// Unique within the variant: `<class>/../<instance>/<service>`.
    pub key: String,
    pub class_path: Vec<String>,
    pub instance: String,
    pub service: String,
    /// The name CANoe's CAPL uses (`SHORTCUTQUAL`, else `<instance>_<service>`), unique in the variant.
    pub shortcut: String,
    pub name: LocalText,
    /// Qualifier of the protocol service the instance is built from.
    pub protocol_service: String,
    /// First byte of the request.
    pub sid: Option<u8>,
    pub addressing: Addressing,
    pub request: Slot,
    pub positive: Slot,
    pub negative: Slot,
    /// The negative response codes the service lists (sorted, without duplicates).
    pub nrcs: Vec<u8>,
}

impl Service {
    pub fn slot(&self, kind: MsgKind) -> &Slot {
        match kind {
            MsgKind::Request => &self.request,
            MsgKind::Positive => &self.positive,
            MsgKind::Negative => &self.negative,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub qual: String,
    pub name: LocalText,
    /// The base variant (`base='1'`).
    pub base: bool,
    pub services: Vec<Service>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ecu {
    pub qual: String,
    pub name: LocalText,
    pub variants: Vec<Variant>,
}

/// One entry of the `NEGRESCODES` table.
#[derive(Clone, Debug, PartialEq)]
pub struct Nrc {
    pub value: u8,
    pub qual: String,
    pub name: LocalText,
}

#[derive(Clone, Debug)]
pub struct Cdd {
    pub protocol: Protocol,
    pub dtd_version: String,
    /// The encoding the file was decoded from.
    pub encoding: String,
    /// `languages` of the document, in order.
    pub languages: Vec<String>,
    pub datatypes: DataTypes,
    pub nrcs: Vec<Nrc>,
    pub ecus: Vec<Ecu>,
    /// Problems found while resolving, none of which stopped the load.
    pub issues: Vec<Issue>,
}

impl Cdd {
    pub fn ecu(&self, qual: &str) -> Option<&Ecu> {
        self.ecus.iter().find(|e| e.qual == qual)
    }

    pub fn nrc_name(&self, value: u8) -> Option<&Nrc> {
        self.nrcs.iter().find(|n| n.value == value)
    }
}

impl Ecu {
    pub fn variant(&self, qual: &str) -> Option<&Variant> {
        self.variants.iter().find(|v| v.qual == qual)
    }

    /// The variant to generate when the user names none: the base variant, else the first.
    pub fn default_variant(&self) -> Option<&Variant> {
        self.variants
            .iter()
            .find(|v| v.base)
            .or_else(|| self.variants.first())
    }
}
