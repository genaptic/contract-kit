#![no_main]
//! Fuzzes grouped linear matching, overlap counting, and bounded diagnostic
//! construction. A selector forces misses, single hits, duplicates, overlaps,
//! and common-prefix candidates while the remaining source bytes stay raw.

use conkit_fuzz::{DEFAULT_INPUT_CEILING, sketch::SketchFuzzHarness};
use conkit_sketch::{CheckMode, SketchOccurrence};
use libfuzzer_sys::fuzz_target;

const PATTERN: &[u8] = b"needle\nneedle";

struct SketchMatchingInput {
    occurrence: SketchOccurrence,
    source: Vec<u8>,
}

impl SketchMatchingInput {
    fn from_bytes(input: &[u8]) -> Option<Self> {
        let (&selector, payload) = input.split_first()?;

        let occurrence = if selector & 1 == 0 {
            SketchOccurrence::AtLeastOne
        } else {
            SketchOccurrence::ExactlyOne
        };
        let mut source = Vec::new();
        match (selector >> 1) % 5 {
            0 => source.extend_from_slice(payload),
            1 => {
                source.extend_from_slice(PATTERN);
                source.push(b'\n');
                source.extend_from_slice(payload);
            }
            2 => {
                source.extend_from_slice(PATTERN);
                source.push(b'\n');
                source.extend_from_slice(PATTERN);
                source.push(b'\n');
                source.extend_from_slice(payload);
            }
            3 => source.extend_from_slice(b"needle\nneedle\nneedle"),
            4 => source.extend_from_slice(b"needle\nneedlf"),
            _ => unreachable!("matching selector is reduced modulo five"),
        }

        Some(Self { occurrence, source })
    }
}

fuzz_target!(|input: &[u8]| {
    if input.len() > DEFAULT_INPUT_CEILING {
        return;
    }
    let Some(input) = SketchMatchingInput::from_bytes(input) else {
        return;
    };

    SketchFuzzHarness::with(|harness| {
        let contract = harness.matching_contract(input.occurrence);
        harness
            .check_contract(input.source, contract, CheckMode::Warning)
            .expect("bounded arbitrary source bytes must complete sketch matching");
    });
});
