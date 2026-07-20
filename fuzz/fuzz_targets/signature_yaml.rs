#![no_main]

//! Fuzzes signature YAML parsing and checking through the public byte/catalog
//! API. The harness treats typed parse and resource-limit errors as expected
//! rejections while allowing panics and sanitizer findings to surface.

use conkit_fuzz::{DEFAULT_INPUT_CEILING, signature::SignatureFuzzHarness};
use conkit_signature::CheckMode;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    if input.len() > DEFAULT_INPUT_CEILING {
        return;
    }

    SignatureFuzzHarness::with(|harness| {
        let _ = harness.check_contract(Vec::new(), input.to_vec(), CheckMode::Warning);
    });
});
