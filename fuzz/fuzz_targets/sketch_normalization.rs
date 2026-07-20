#![no_main]
//! Exercises exact-line normalization on arbitrary source bytes through the
//! public checker. The fixed UTF-8 sketch keeps contract parsing deterministic
//! while CR/LF combinations, isolated carriage returns, and invalid UTF-8 all
//! remain under fuzzer control in the source catalog.

use conkit_fuzz::{DEFAULT_INPUT_CEILING, sketch::SketchFuzzHarness};
use conkit_sketch::CheckMode;
use libfuzzer_sys::fuzz_target;

const CONTRACT: &[u8] = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction:
  mode: rust_syntax_v2
  profile: rust_api_v1
  crates: [{ id: fuzz, root: lib.rs, kind: library }]
signatures:
  - normalization_signature:
      file: lib.rs
      signature_type: function
      sketch: normalization_body
sketches:
  - normalization_body:
      file: lib.rs
      signature: normalization_signature
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: at_least_one }
      code: x
"#;

fuzz_target!(|input: &[u8]| {
    if input.len() > DEFAULT_INPUT_CEILING {
        return;
    }

    SketchFuzzHarness::with(|harness| {
        harness
            .check_contract(input.to_vec(), CONTRACT.to_vec(), CheckMode::Warning)
            .expect("arbitrary source bytes must complete sketch normalization");
    });
});
