#![no_main]

//! Exercises UTF-8 validation, `syn` parsing, inventory conversion, semantic
//! attribute handling, and deterministic rendering through new-contract
//! generation. Raw bytes are retained so malformed Rust remains in scope.

use conkit_fuzz::{DEFAULT_INPUT_CEILING, signature::SignatureFuzzHarness};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    if input.len() > DEFAULT_INPUT_CEILING {
        return;
    }

    SignatureFuzzHarness::with(|harness| {
        let _ = harness.generate_single_source(input.to_vec());
    });
});
