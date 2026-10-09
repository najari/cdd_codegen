#![allow(dead_code)]
//! Shared by the tests: where the samples are, and a visitor that collects what a message describes.

use std::path::PathBuf;

const DEFAULT_ROOT: &str =
    r"C:\Users\Public\Documents\Vector\CANoe\Sample Configurations 13.0.172\CAN";

/// The folder of CANoe's `CAN` sample configurations, if it is there.
pub fn samples() -> Option<PathBuf> {
    let root = std::env::var_os("CANOE_SAMPLES")
        .map_or_else(|| PathBuf::from(DEFAULT_ROOT), PathBuf::from);
    root.is_dir().then_some(root)
}

/// Skips the test (with a note) when CANoe's samples are not installed.
#[macro_export]
macro_rules! require_samples {
    () => {
        match $crate::common::samples() {
            Some(root) => root,
            None => {
                eprintln!(
                    "skipped: CANoe's sample configurations are not installed (set CANOE_SAMPLES)"
                );
                return;
            }
        }
    };
}
