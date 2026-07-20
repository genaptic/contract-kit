#![no_main]

//! Builds bounded module-graph shapes from arbitrary bytes. One to sixteen
//! files, crate roots, and edges cover conventional and explicit paths, inline
//! and conditional declarations, missing targets, cycles, repeated physical
//! roots, and multiply claimed files without a second graph implementation.

use conkit_fuzz::signature::SignatureFuzzHarness;
use conkit_signature::{CatalogPath, FileCatalog, RustCrateKind, RustCrateRoot};
use libfuzzer_sys::fuzz_target;
use std::fmt::Write;

const MAX_INPUT_BYTES: usize = 4096;

struct SourceGraphFixture {
    source_files: FileCatalog,
    files: Vec<CatalogPath>,
    crates: Vec<RustCrateRoot>,
}

impl SourceGraphFixture {
    fn from_input(input: &[u8], harness: &SignatureFuzzHarness) -> Self {
        let mut input = BoundedGraphInput::new(input);
        let node_count = input.bounded_count();
        let root_count = input.bounded_count();
        let edge_count = input.bounded_count();
        let mut sources = (0..node_count)
            .map(|index| {
                format!(
                    "pub struct Node{index:02};\nimpl Node{index:02} {{ pub fn value() -> u8 {{ {index} }} }}\n"
                )
            })
            .collect::<Vec<_>>();

        for edge_index in 0..edge_count {
            let source_index = input.index(node_count);
            let target_index = input.index(node_count);
            let edge_kind = input.next() % 5;
            let source = sources
                .get_mut(source_index)
                .expect("bounded graph source index must exist");
            match edge_kind {
                0 => {
                    writeln!(
                        source,
                        "#[path = \"node_{target_index:02}.rs\"] mod edge_{source_index:02}_{edge_index:02}_{target_index:02};"
                    )
                    .expect("writing to a String must succeed");
                }
                1 => {
                    writeln!(source, "mod node_{target_index:02};")
                        .expect("writing to a String must succeed");
                }
                2 => {
                    writeln!(
                        source,
                        "#[cfg(any())]\n#[path = \"node_{target_index:02}.rs\"] mod conditional_{source_index:02}_{edge_index:02}_{target_index:02};"
                    )
                    .expect("writing to a String must succeed");
                }
                3 => {
                    writeln!(
                        source,
                        "#[path = \"missing_{target_index:02}_{edge_index:02}.rs\"] mod missing_{source_index:02}_{edge_index:02}_{target_index:02};"
                    )
                    .expect("writing to a String must succeed");
                }
                4 => {
                    writeln!(
                        source,
                        "mod inline_{source_index:02}_{edge_index:02}_{target_index:02} {{ pub struct Node{target_index:02}; }}"
                    )
                    .expect("writing to a String must succeed");
                }
                _ => unreachable!("edge kind is reduced modulo five"),
            }
        }

        let mut files = Vec::with_capacity(node_count);
        let mut entries = Vec::with_capacity(node_count);
        for (index, source) in sources.into_iter().enumerate() {
            let path = CatalogPath::new(format!("node_{index:02}.rs"))
                .expect("bounded graph node path must be valid");
            files.push(path.clone());
            entries.push((path, source.into_bytes()));
        }
        let source_files = harness.source_files(entries);

        let mut crates = Vec::with_capacity(root_count);
        for root_index in 0..root_count {
            let node_index = input.index(node_count);
            crates.push(RustCrateRoot {
                id: format!("fuzz_{root_index:02}"),
                root: CatalogPath::new(format!("node_{node_index:02}.rs"))
                    .expect("bounded graph root path must be valid"),
                kind: if input.next() & 1 == 0 {
                    RustCrateKind::Library
                } else {
                    RustCrateKind::Binary
                },
            });
        }

        Self {
            source_files,
            files,
            crates,
        }
    }
}

struct BoundedGraphInput<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> BoundedGraphInput<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    fn next(&mut self) -> u8 {
        let value = if self.bytes.is_empty() {
            self.cursor as u8
        } else {
            self.bytes[self.cursor % self.bytes.len()]
        };
        self.cursor = self.cursor.wrapping_add(1);
        value
    }

    fn bounded_count(&mut self) -> usize {
        usize::from(self.next() % 16) + 1
    }

    fn index(&mut self, length: usize) -> usize {
        usize::from(self.next()) % length
    }
}

fuzz_target!(|input: &[u8]| {
    if input.len() > MAX_INPUT_BYTES {
        return;
    }

    SignatureFuzzHarness::with(|harness| {
        let fixture = SourceGraphFixture::from_input(input, harness);
        let _ = harness.generate_new(fixture.source_files, fixture.files, fixture.crates);
    });
});
