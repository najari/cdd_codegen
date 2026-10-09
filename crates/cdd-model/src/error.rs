//! Load errors and the diagnostics the model collects while it resolves a document.

use std::fmt;

/// A failure that stops loading altogether.
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    /// The bytes are not valid in the declared (or assumed) text encoding.
    Encoding(String),
    Xml(roxmltree::Error),
    /// Well-formed XML that is not a CANdela diagnostic description.
    NotCdd(String),
    /// A selection (ECU, variant, language) that matches nothing.
    Selection(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "cannot read the file: {e}"),
            Error::Encoding(m) => write!(f, "{m}"),
            Error::Xml(e) => write!(f, "the file is not well-formed XML: {e}"),
            Error::NotCdd(m) => write!(f, "not a CDD file: {m}"),
            Error::Selection(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<roxmltree::Error> for Error {
    fn from(e: roxmltree::Error) -> Self {
        Error::Xml(e)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// Stable diagnostic codes. A message that cannot be generated carries one of these.
pub mod codes {
    /// An `id` reference that points at nothing, or at the wrong kind of element.
    pub const UNRESOLVED_REF: &str = "CDD-REF-001";
    /// A message component the generator does not model (yet).
    pub const UNSUPPORTED_COMPONENT: &str = "CDD-MODEL-001";
    /// A data type the generator does not model (yet).
    pub const UNSUPPORTED_DATATYPE: &str = "CDD-DT-001";
    /// A fixed component whose value the instance never supplies.
    pub const MISSING_VALUE: &str = "CDD-VALUE-001";
    /// A number that does not parse.
    pub const BAD_NUMBER: &str = "CDD-NUM-001";
    /// A byte order other than `21` (big endian) or `12` (little endian).
    pub const BAD_BYTE_ORDER: &str = "CDD-BIT-004";
    /// A bit layout that contradicts itself.
    pub const BAD_LAYOUT: &str = "CDD-BIT-001";
    /// A layout the generator cannot place on byte boundaries.
    pub const UNSUPPORTED_LAYOUT: &str = "CDD-BIT-002";
    /// A variable-length field that is not the last item of its message.
    pub const VARIABLE_NOT_LAST: &str = "CDD-LEN-001";
    /// Two services that a received payload cannot be told apart by.
    pub const AMBIGUOUS_PREFIX: &str = "CDD-ID-001";
}

/// Something worth telling the user about, with where it happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    /// Where: a service key, a data type qualifier, ...
    pub context: String,
}

impl Issue {
    pub fn new(severity: Severity, code: &'static str, message: impl Into<String>) -> Self {
        Issue {
            severity,
            code,
            message: message.into(),
            context: String::new(),
        }
    }

    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Issue::new(Severity::Error, code, message)
    }

    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Issue::new(Severity::Warning, code, message)
    }

    pub fn at(mut self, context: impl Into<String>) -> Self {
        self.context = context.into();
        self
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.code)?;
        if !self.context.is_empty() {
            write!(f, " [{}]", self.context)?;
        }
        write!(f, ": {}", self.message)
    }
}
