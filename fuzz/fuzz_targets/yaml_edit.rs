#![no_main]
//! Drives a real changed-document sketch refresh through the lossless editor,
//! then verifies counts, reparses and enforces the returned semantic document,
//! and repeats the refresh as a byte-idempotence oracle. Input bytes become
//! replacement Unicode with a fixed nonempty prefix.

use conkit_fuzz::{DEFAULT_INPUT_CEILING, sketch::SketchFuzzHarness};
use conkit_sketch::{CheckMode, GenerateMode};
use libfuzzer_sys::fuzz_target;

const CONTRACT: &[u8] = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction:
  mode: rust_syntax_v2
  profile: rust_api_v1
  crates: [{ id: fuzz, root: lib.rs, kind: library }]
signatures:
  - fuzz_signature:
      file: lib.rs
      signature_type: function
      sketch: fuzz_body
sketches:
  - fuzz_body:
      file: lib.rs
      signature: fuzz_signature
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: at_least_one }
      code: |-
        original
"#;

fuzz_target!(|input: &[u8]| {
    if input.len() > DEFAULT_INPUT_CEILING {
        return;
    }

    SketchFuzzHarness::with(|harness| {
        let contract_files = harness.contract_catalog(CONTRACT.to_vec());
        let mut code = String::from("fuzz:");
        code.push_str(&String::from_utf8_lossy(input));
        let seed = harness.seed("fuzz_body", "function", code.clone());

        let generated = harness
            .generate(
                contract_files,
                vec![seed.clone()],
                GenerateMode::FullRefresh,
            )
            .expect("valid arbitrary sketch replacement must generate");
        assert_eq!(generated.counts.linked_sketch_count, 1);
        assert_eq!(generated.counts.refreshed_sketch_count, 1);
        assert_eq!(generated.counts.changed_sketch_count, 1);
        assert_eq!(generated.counts.changed_document_count, 1);

        let checked = harness
            .check_catalog(
                code.into_bytes(),
                generated.contract_files.clone(),
                CheckMode::Enforce,
            )
            .expect("generated sketch contract must check");
        assert!(checked.passed);
        assert!(checked.diagnostics.is_empty());
        assert_eq!(checked.counts.sketch_count, 1);
        assert_eq!(checked.counts.matched_sketch_count, 1);
        assert_eq!(checked.counts.failed_sketch_count, 0);

        let repeated = harness
            .generate(
                generated.contract_files.clone(),
                vec![seed],
                GenerateMode::FullRefresh,
            )
            .expect("unchanged sketch regeneration must succeed");
        assert_eq!(repeated.counts.linked_sketch_count, 1);
        assert_eq!(repeated.counts.refreshed_sketch_count, 1);
        assert_eq!(repeated.counts.changed_sketch_count, 0);
        assert_eq!(repeated.counts.changed_document_count, 0);
        assert_eq!(repeated.contract_files, generated.contract_files);
    });
});
