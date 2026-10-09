//! Looking at a message without knowing its type: every value with its path, raw value, symbolic
//! text, physical value and unit. This is what a trace window shows, and what the tests compare
//! the generated code's decoding with.

use crate::path::Path;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RawValue<'a> {
    Uint(u64),
    Int(i64),
    Float(f64),
    Bytes(&'a [u8]),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldValue<'a> {
    pub raw: RawValue<'a>,
    /// The entry of a text table, or the characters of a text.
    pub text: Option<&'a str>,
    /// The physical value of a linear conversion.
    pub physical: Option<f64>,
    pub unit: Option<&'static str>,
}

impl<'a> FieldValue<'a> {
    pub const fn new(raw: RawValue<'a>) -> Self {
        FieldValue {
            raw,
            text: None,
            physical: None,
            unit: None,
        }
    }

    pub const fn uint(v: u64) -> Self {
        Self::new(RawValue::Uint(v))
    }

    pub const fn int(v: i64) -> Self {
        Self::new(RawValue::Int(v))
    }

    pub const fn float(v: f64) -> Self {
        Self::new(RawValue::Float(v))
    }

    pub const fn bytes(v: &'a [u8]) -> Self {
        Self::new(RawValue::Bytes(v))
    }

    pub const fn text(mut self, t: Option<&'a str>) -> Self {
        self.text = t;
        self
    }

    pub const fn physical(mut self, p: Option<f64>, unit: Option<&'static str>) -> Self {
        self.physical = p;
        self.unit = unit;
        self
    }
}

pub trait Visitor {
    fn field(&mut self, path: &Path<'_>, value: &FieldValue<'_>);
}

pub trait Describe {
    fn describe(&self, path: &Path<'_>, v: &mut dyn Visitor);
}

#[cfg(feature = "alloc")]
pub use text::{describe_to_string, TextVisitor};

#[cfg(feature = "alloc")]
mod text {
    extern crate alloc;
    use super::*;
    use alloc::string::String;
    use core::fmt::Write;

    /// Writes one line per value: `path = raw 'text' = physical unit`.
    pub struct TextVisitor<'w> {
        out: &'w mut String,
    }

    impl<'w> TextVisitor<'w> {
        pub fn new(out: &'w mut String) -> Self {
            TextVisitor { out }
        }
    }

    impl Visitor for TextVisitor<'_> {
        fn field(&mut self, path: &Path<'_>, v: &FieldValue<'_>) {
            let _ = write!(self.out, "{path} = ");
            let _ = match v.raw {
                RawValue::Uint(x) => write!(self.out, "{x} (0x{x:X})"),
                RawValue::Int(x) => write!(self.out, "{x}"),
                RawValue::Float(x) => write!(self.out, "{x}"),
                RawValue::Bytes(b) => {
                    for (i, x) in b.iter().enumerate() {
                        let _ = write!(self.out, "{}{x:02X}", if i == 0 { "" } else { " " });
                    }
                    Ok(())
                }
            };
            if let Some(t) = v.text {
                let _ = write!(self.out, " '{t}'");
            }
            if let Some(p) = v.physical {
                let _ = write!(self.out, " = {p}");
                if let Some(u) = v.unit {
                    let _ = write!(self.out, " {u}");
                }
            }
            self.out.push('\n');
        }
    }

    pub fn describe_to_string<D: Describe + ?Sized>(d: &D) -> String {
        let mut s = String::new();
        d.describe(&Path::ROOT, &mut TextVisitor::new(&mut s));
        s
    }
}
