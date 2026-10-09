//! The path of a value inside a message, built on the stack so describing a message needs no
//! allocation: `Codingstring.VehicleType`, `List[2].DTC`.

use core::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seg {
    Root,
    Name(&'static str),
    Index(usize),
}

#[derive(Clone, Copy)]
pub struct Path<'p> {
    parent: Option<&'p Path<'p>>,
    seg: Seg,
}

impl Path<'static> {
    pub const ROOT: Path<'static> = Path {
        parent: None,
        seg: Seg::Root,
    };
}

impl<'p> Path<'p> {
    /// The path of a child that has its own full name (the generated code stores the full path
    /// of every item, so most values are described with one call of this).
    pub fn name(&'p self, name: &'static str) -> Path<'p> {
        Path {
            parent: Some(self),
            seg: Seg::Name(name),
        }
    }

    pub fn index(&'p self, i: usize) -> Path<'p> {
        Path {
            parent: Some(self),
            seg: Seg::Index(i),
        }
    }

    fn is_root(&self) -> bool {
        self.seg == Seg::Root
    }

    fn write(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(p) = self.parent {
            p.write(f)?;
        }
        match self.seg {
            Seg::Root => Ok(()),
            Seg::Name(n) => {
                if self.parent.is_some_and(|p| !p.is_root()) {
                    f.write_str(".")?;
                }
                f.write_str(n)
            }
            Seg::Index(i) => write!(f, "[{i}]"),
        }
    }
}

impl fmt::Display for Path<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write(f)
    }
}

impl fmt::Debug for Path<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Path({self})")
    }
}

#[cfg(all(test, feature = "alloc"))]
mod tests {
    extern crate alloc;
    use super::*;
    use alloc::format;

    #[test]
    fn dotted_names_and_indexes() {
        let root = Path::ROOT;
        let list = root.name("List");
        let item = list.index(2);
        let dtc = item.name("DTC");
        assert_eq!(format!("{dtc}"), "List[2].DTC");
        assert_eq!(format!("{root}"), "");
        assert_eq!(format!("{}", root.name("SID_RQ")), "SID_RQ");
    }
}
