//! What a generation produces, and how it is put on disk.
//!
//! The code and its manifest are published as a pair: both are written next to their final place
//! first, then moved in, and if the second move fails the first is undone. A reader that finds
//! the manifest's `code_sha256` equal to the hash of `diag.rs` knows the pair is whole.

use crate::{Error, Report};
use std::fs;
use std::path::Path;

pub const CODE_FILE: &str = "diag.rs";
pub const MANIFEST_FILE: &str = "manifest.json";

/// The generated Rust source, the manifest that says where it came from, and what was left out.
#[derive(Clone, Debug)]
pub struct Artifacts {
    pub code: String,
    /// JSON: generator, source hash, options, every service and what became of it.
    pub manifest: String,
    pub report: Report,
}

impl Artifacts {
    pub(crate) fn new(code: String, manifest: String, report: Report) -> Self {
        Artifacts {
            code,
            manifest,
            report,
        }
    }

    /// Writes `diag.rs` and `manifest.json` into `dir`, which must exist. A failure leaves the files
    /// that were there as they were.
    pub fn write_to_directory(&self, dir: impl AsRef<Path>) -> Result<(), Error> {
        let dir = dir.as_ref();
        if !dir.is_dir() {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("`{}` is not a directory", dir.display()),
            )));
        }
        let stage = dir.join(format!(".cddgen-stage-{}", std::process::id()));
        fs::create_dir_all(&stage)?;
        let result = self.publish(dir, &stage);
        let _ = fs::remove_dir_all(&stage);
        result
    }

    fn publish(&self, dir: &Path, stage: &Path) -> Result<(), Error> {
        let files = [
            (CODE_FILE, self.code.as_bytes()),
            (MANIFEST_FILE, self.manifest.as_bytes()),
        ];
        for (name, bytes) in files {
            let mut f = fs::File::create(stage.join(name))?;
            std::io::Write::write_all(&mut f, bytes)?;
            f.sync_all()?;
        }
        let mut backed_up = Vec::new();
        let mut published = Vec::new();
        let moved = (|| -> std::io::Result<()> {
            for (name, _) in files {
                let target = dir.join(name);
                if target.exists() {
                    fs::rename(&target, stage.join(format!("{name}.old")))?;
                    backed_up.push(name);
                }
            }
            for (name, _) in files {
                fs::rename(stage.join(name), dir.join(name))?;
                published.push(name);
            }
            Ok(())
        })();
        if let Err(e) = moved {
            for name in published {
                let _ = fs::remove_file(dir.join(name));
            }
            for name in backed_up {
                let _ = fs::rename(stage.join(format!("{name}.old")), dir.join(name));
            }
            return Err(Error::Io(e));
        }
        Ok(())
    }

    /// Whether `dir` already holds exactly these files (for `--check` in CI).
    pub fn matches_directory(&self, dir: impl AsRef<Path>) -> bool {
        let dir = dir.as_ref();
        let same = |name: &str, text: &str| {
            fs::read_to_string(dir.join(name)).is_ok_and(|on_disk| on_disk == text)
        };
        same(CODE_FILE, &self.code) && same(MANIFEST_FILE, &self.manifest)
    }
}
