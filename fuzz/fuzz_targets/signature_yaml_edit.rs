#![no_main]

//! Exercises changed-document signature CST editing, semantic reparsing, and
//! byte idempotence through public APIs. Arbitrary bytes become a valid Rust
//! `must_use` message while a fixed callable-shape change guarantees that the
//! first existing-contract generation reaches the lossless editor.

use conkit_fuzz::signature::SignatureFuzzHarness;
use conkit_signature::{CheckMode, FileCatalog};
use libfuzzer_sys::fuzz_target;

const MAX_INPUT_BYTES: usize = 64 * 1024;
const BASELINE_SOURCE: &[u8] = b"#[must_use = \"baseline\"]\npub fn fuzz() -> usize { 0 }\n";

std::thread_local! {
    static HARNESS: SignatureYamlEditHarness = SignatureYamlEditHarness::new();
}

struct SignatureYamlEditHarness {
    baseline_contracts: FileCatalog,
}

impl SignatureYamlEditHarness {
    fn new() -> Self {
        let baseline_contracts = SignatureFuzzHarness::with(|harness| {
            harness
                .generate_single_source(BASELINE_SOURCE.to_vec())
                .expect("static signature baseline must generate")
                .contract_files
        });

        Self { baseline_contracts }
    }

    fn run(&self, input: &[u8]) {
        if input.len() > MAX_INPUT_BYTES {
            return;
        }

        SignatureFuzzHarness::with(|harness| {
            let changed_sources = harness.source_catalog(Self::changed_source(input));
            let generated = harness
                .generate_existing(changed_sources.clone(), self.baseline_contracts.clone())
                .expect("valid changed signature source must generate");
            assert_eq!(generated.counts.document_count, 1);
            assert_eq!(generated.counts.signature_count, 1);
            assert_eq!(generated.counts.preserved_sketch_count, 0);
            assert_eq!(generated.counts.semantically_changed_document_count, 1);
            assert_eq!(generated.counts.byte_changed_document_count, 1);

            let checked = harness
                .check_catalogs(
                    changed_sources.clone(),
                    generated.contract_files.clone(),
                    CheckMode::Strict,
                )
                .expect("generated signature contract must check");
            assert!(checked.passed);
            assert!(checked.diagnostics.is_empty());
            assert_eq!(checked.counts.source_signature_count, 1);
            assert_eq!(checked.counts.contract_signature_count, 1);

            let repeated = harness
                .generate_existing(changed_sources, generated.contract_files.clone())
                .expect("unchanged signature regeneration must succeed");
            assert_eq!(repeated.counts.document_count, 1);
            assert_eq!(repeated.counts.signature_count, 1);
            assert_eq!(repeated.counts.preserved_sketch_count, 0);
            assert_eq!(repeated.counts.semantically_changed_document_count, 0);
            assert_eq!(repeated.counts.byte_changed_document_count, 0);
            assert_eq!(repeated.contract_files, generated.contract_files);
        });
    }

    fn changed_source(input: &[u8]) -> Vec<u8> {
        let message = String::from_utf8_lossy(input);
        let message = message.as_ref();
        format!("#[must_use = {message:?}]\npub fn fuzz(value: &[u8]) -> usize {{ value.len() }}\n")
            .into_bytes()
    }
}

fuzz_target!(|input: &[u8]| HARNESS.with(|harness| harness.run(input)));
