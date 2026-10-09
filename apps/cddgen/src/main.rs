//! `cddgen`: generate Rust tester and ECU-simulation code from CANdela CDD files, and look at what
//! a CDD says.

mod inspect;
mod log;

use anyhow::{bail, Context, Result};
use cdd_codegen::{Config, OnUnsupported, Role};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "cddgen", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum RoleArg {
    Tester,
    Ecu,
    Both,
}

#[derive(Subcommand)]
enum Command {
    /// Generate Rust code for one ECU variant: writes `diag.rs` and `manifest.json` into OUT_DIR.
    Generate {
        /// The `.cdd` file.
        cdd: PathBuf,
        /// An existing directory.
        out_dir: PathBuf,
        /// The ECU, when the document has several.
        #[arg(long)]
        ecu: Option<String>,
        /// The variant (default: the base variant).
        #[arg(long)]
        variant: Option<String>,
        /// Language for names and labels; repeat for fallbacks (default: English, then the document's).
        #[arg(long = "language")]
        languages: Vec<String>,
        /// Which side the code is for.
        #[arg(long, value_enum, default_value = "both")]
        role: RoleArg,
        /// Fail when any message has to be left out.
        #[arg(long)]
        strict: bool,
        /// Name of the runtime crate in the generated code.
        #[arg(long, default_value = "cdd_rt")]
        rt_crate: String,
        /// Negative response code a simulated ECU gives services it does not implement.
        #[arg(long, default_value = "0x12", value_parser = parse_byte)]
        default_nrc: u8,
        /// Do not write: fail when OUT_DIR does not already hold exactly this output.
        #[arg(long)]
        check: bool,
    },
    /// List the ECUs, variants and services of a CDD, or show the layout of one service.
    Inspect(inspect::InspectArgs),
    /// Say which service a payload belongs to and what it holds.
    Decode(inspect::DecodeArgs),
    /// Read a CAN log (.asc, .blf) as diagnostics: ISO-TP, services and values, like CANoe's trace.
    DecodeLog(log::LogArgs),
}

fn parse_byte(s: &str) -> Result<u8, String> {
    let v = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(h) => u8::from_str_radix(h, 16),
        None => s.parse(),
    };
    v.map_err(|_| format!("`{s}` is not a byte (0..=255 or 0x00..=0xFF)"))
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Generate {
            cdd,
            out_dir,
            ecu,
            variant,
            languages,
            role,
            strict,
            rt_crate,
            default_nrc,
            check,
        } => {
            let bytes =
                std::fs::read(&cdd).with_context(|| format!("cannot read `{}`", cdd.display()))?;
            let name = cdd.file_name().map_or_else(
                || cdd.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            let config = Config {
                ecu,
                variant,
                languages,
                role: match role {
                    RoleArg::Tester => Role::Tester,
                    RoleArg::Ecu => Role::Ecu,
                    RoleArg::Both => Role::Both,
                },
                on_unsupported: if strict {
                    OnUnsupported::Error
                } else {
                    OnUnsupported::Skip
                },
                rt_crate,
                default_nrc,
            };
            let artifacts = config
                .generate(&name, &bytes)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let r = &artifacts.report;
            println!(
                "{} of {} services: {} requests, {} responses; {} messages left out",
                r.services_generated,
                r.services_total,
                r.requests_generated,
                r.responses_generated,
                r.skipped.len()
            );
            for s in &r.skipped {
                println!(
                    "  left out {} {} [{}]: {}",
                    s.service, s.message, s.code, s.reason
                );
            }
            for a in &r.ambiguous {
                println!("  ambiguous: {a}");
            }
            if check {
                if !artifacts.matches_directory(&out_dir) {
                    bail!(
                        "`{}` does not hold exactly this output: generate it again",
                        out_dir.display()
                    );
                }
                println!("up to date");
            } else {
                artifacts
                    .write_to_directory(&out_dir)
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                println!("wrote {}", out_dir.join("diag.rs").display());
            }
            Ok(())
        }
        Command::Inspect(args) => inspect::inspect(args),
        Command::Decode(args) => inspect::decode(args),
        Command::DecodeLog(args) => log::decode_log(args),
    }
}
