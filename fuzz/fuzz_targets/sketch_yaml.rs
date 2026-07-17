#![no_main]
//! Fuzzes sketch YAML parsing, link validation, and check-report construction
//! through the public byte/catalog API. Malformed contracts are expected
//! outcomes; typed limit rejections are accepted, while panics, sanitizer
//! findings, or unbounded behavior remain bugs.

use conkit_fuzz::{DEFAULT_INPUT_CEILING, sketch::SketchFuzzHarness};
use conkit_sketch::CheckMode;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    if input.len() > DEFAULT_INPUT_CEILING {
        return;
    }

    SketchFuzzHarness::with(|harness| {
        let _ = harness.check_contract(
            b"pub fn fuzz() {}\n".to_vec(),
            input.to_vec(),
            CheckMode::Warning,
        );
    });
});
