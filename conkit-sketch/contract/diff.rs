use super::model::{SketchContract, SketchContracts};
use crate::api::{DiffEntry, DiffResponse, SketchField, SketchSnapshot};
use crate::error::SketchContractKitError;
use crate::normalize::NormalizedSnippet;
use crate::work::CancellationProbe;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;

impl SketchContracts {
    pub(crate) fn diff_against(
        &self,
        previous: &Self,
        cancellation: &CancellationProbe,
    ) -> Result<DiffResponse, SketchContractKitError> {
        let mut entries = Vec::new();
        let mut current_index = 0;
        let mut previous_index = 0;
        let mut processed = 0;

        while current_index < self.entries().len() || previous_index < previous.entries().len() {
            cancellation.checkpoint_at(processed)?;
            processed = processed.saturating_add(1);

            match (
                self.entries().get(current_index),
                previous.entries().get(previous_index),
            ) {
                (Some(current), Some(previous)) => match current.id().cmp(previous.id()) {
                    Ordering::Less => {
                        entries.push(DiffEntry::Added {
                            current: current.snapshot(cancellation)?,
                        });
                        current_index += 1;
                    }
                    Ordering::Equal => {
                        let fields = current.changed_fields(previous);
                        if !fields.is_empty() {
                            entries.push(DiffEntry::Changed {
                                previous: previous.snapshot(cancellation)?,
                                current: current.snapshot(cancellation)?,
                                fields,
                            });
                        }
                        current_index += 1;
                        previous_index += 1;
                    }
                    Ordering::Greater => {
                        entries.push(DiffEntry::Removed {
                            previous: previous.snapshot(cancellation)?,
                        });
                        previous_index += 1;
                    }
                },
                (Some(current), None) => {
                    entries.push(DiffEntry::Added {
                        current: current.snapshot(cancellation)?,
                    });
                    current_index += 1;
                }
                (None, Some(previous)) => {
                    entries.push(DiffEntry::Removed {
                        previous: previous.snapshot(cancellation)?,
                    });
                    previous_index += 1;
                }
                (None, None) => break,
            }
        }

        Ok(DiffResponse {
            contract_digest: self.contract_digest(cancellation)?,
            digest_version: 2,
            entries,
        })
    }

    fn contract_digest(
        &self,
        cancellation: &CancellationProbe,
    ) -> Result<String, SketchContractKitError> {
        let mut encoder = SketchDigestEncoder::new(b"conkit-sketch:v2:contract");
        encoder.write_count(self.entries().len());
        for (index, contract) in self.entries().iter().enumerate() {
            cancellation.checkpoint_at(index)?;
            contract.write_semantics(&mut encoder, cancellation)?;
        }
        Ok(encoder.finish())
    }
}

struct SketchDigestEncoder {
    hasher: Sha256,
}

impl SketchDigestEncoder {
    fn new(domain: &[u8]) -> Self {
        let mut encoder = Self {
            hasher: Sha256::new(),
        };
        encoder.write_field(domain);
        encoder
    }

    fn write_count(&mut self, value: usize) {
        self.write_field(&(value as u64).to_be_bytes());
    }

    fn write_field(&mut self, value: &[u8]) {
        self.hasher.update((value.len() as u64).to_be_bytes());
        self.hasher.update(value);
    }

    fn write_normalized(
        &mut self,
        normalized: &NormalizedSnippet,
        cancellation: &CancellationProbe,
    ) -> Result<(), SketchContractKitError> {
        self.write_count(normalized.line_count());
        for (index, line) in normalized.lines().enumerate() {
            cancellation.checkpoint_at(index)?;
            self.write_field(line);
        }
        Ok(())
    }

    fn finish(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let digest = self.hasher.finalize();
        let mut value = String::with_capacity(64);
        for byte in digest {
            value.push(HEX[usize::from(byte >> 4)] as char);
            value.push(HEX[usize::from(byte & 0x0f)] as char);
        }
        value
    }
}

impl SketchContract {
    fn snapshot(
        &self,
        cancellation: &CancellationProbe,
    ) -> Result<SketchSnapshot, SketchContractKitError> {
        Ok(SketchSnapshot {
            sketch_id: self.id().as_str().to_owned(),
            contract_file: self.contract_file().clone(),
            document_index: self.document_index(),
            source_file: self.file().clone(),
            linked_signature: self.linked_signature().as_str().to_owned(),
            signature_type: self.signature_type().as_str().to_owned(),
            matching: self.matching_policy(),
            code_digest: self.code_digest(cancellation)?,
        })
    }

    pub(super) fn changed_fields(&self, previous: &Self) -> Vec<SketchField> {
        let mut fields = Vec::new();
        if self.file() != previous.file() {
            fields.push(SketchField::SourceFile);
        }
        if self.linked_signature() != previous.linked_signature() {
            fields.push(SketchField::LinkedSignature);
        }
        if self.signature_type() != previous.signature_type() {
            fields.push(SketchField::SignatureType);
        }
        if self.occurrence() != previous.occurrence() {
            fields.push(SketchField::Occurrence);
        }
        if self.snippet().normalized() != previous.snippet().normalized() {
            fields.push(SketchField::Code);
        }
        fields
    }

    pub(super) fn code_digest(
        &self,
        cancellation: &CancellationProbe,
    ) -> Result<String, SketchContractKitError> {
        let mut encoder = SketchDigestEncoder::new(b"conkit-sketch:v2:code");
        encoder.write_field(self.normalization().as_str().as_bytes());
        encoder.write_normalized(self.snippet().normalized(), cancellation)?;
        Ok(encoder.finish())
    }

    fn write_semantics(
        &self,
        encoder: &mut SketchDigestEncoder,
        cancellation: &CancellationProbe,
    ) -> Result<(), SketchContractKitError> {
        encoder.write_field(self.id().as_str().as_bytes());
        encoder.write_field(self.file().as_str().as_bytes());
        encoder.write_field(self.linked_signature().as_str().as_bytes());
        encoder.write_field(self.signature_type().as_str().as_bytes());
        encoder.write_field(self.normalization().as_str().as_bytes());
        encoder.write_field(self.occurrence().as_str().as_bytes());
        encoder.write_normalized(self.snippet().normalized(), cancellation)
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::SketchContracts as ParsedSketchContracts;
    use crate::api::{DiffEntry, SketchField};
    use crate::contract::tests::{ContractYaml, SketchContracts, TestCatalog};
    use crate::files::{CatalogPath, FileCatalog};
    use crate::work::CancellationProbe;

    #[test]
    fn semantic_diff_two_pointer_merge_preserves_interleaved_id_order() {
        let previous = DigestSketch::contracts(
            "previous.yml",
            &[
                DigestSketch::named("alpha", "alpha_body", "a.rs", "old alpha"),
                DigestSketch::named("charlie", "charlie_body", "c.rs", "old charlie"),
                DigestSketch::named("echo", "echo_body", "e.rs", "same echo"),
                DigestSketch::named("golf", "golf_body", "g.rs", "old golf"),
            ],
        );
        let current = DigestSketch::contracts(
            "current.yml",
            &[
                DigestSketch::named("bravo", "bravo_body", "b.rs", "new bravo"),
                DigestSketch::named("charlie", "charlie_body", "c.rs", "new charlie"),
                DigestSketch::named("echo", "echo_body", "e.rs", "same echo"),
                DigestSketch::named("foxtrot", "foxtrot_body", "f.rs", "new foxtrot"),
            ],
        );

        let diff = SketchContracts::diff(&current, &previous);
        let ordered = diff
            .entries
            .iter()
            .map(|entry| match entry {
                DiffEntry::Added { current } => ("added", current.sketch_id.as_str()),
                DiffEntry::Removed { previous } => ("removed", previous.sketch_id.as_str()),
                DiffEntry::Changed { current, .. } => ("changed", current.sketch_id.as_str()),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            ordered,
            [
                ("removed", "alpha_body"),
                ("added", "bravo_body"),
                ("changed", "charlie_body"),
                ("added", "foxtrot_body"),
                ("removed", "golf_body"),
            ]
        );
        let DiffEntry::Changed { fields, .. } = &diff.entries[2] else {
            panic!("third ordered entry must be changed");
        };
        assert_eq!(fields.as_slice(), [SketchField::Code]);

        let cancellation = CancellationProbe::new();
        cancellation.cancel();
        assert!(matches!(
            current.diff_against(&previous, &cancellation),
            Err(error) if error.is_operation_cancelled()
        ));
        assert!(matches!(
            current.contract_digest(&cancellation),
            Err(error) if error.is_operation_cancelled()
        ));
        assert!(matches!(
            current.entries()[0].code_digest(&cancellation),
            Err(error) if error.is_operation_cancelled()
        ));
    }

    #[test]
    fn semantic_diff_reports_exact_whitespace_changes_even_after_relocation() {
        let previous = SketchContracts::from_catalog(
            TestCatalog::new()
                .with_file(
                    "previous.yml",
                    &ContractYaml::linked("same", "same_body", "function", "let value = 1;"),
                )
                .into_catalog(),
        )
        .expect("previous");
        let current = SketchContracts::from_catalog(
            TestCatalog::new()
                .with_file(
                    "current.YAML",
                    &ContractYaml::linked("same", "same_body", "function", "  let   value = 1;  "),
                )
                .into_catalog(),
        )
        .expect("current");

        let diff = SketchContracts::diff(&current, &previous);

        assert!(diff.changed());
        assert!(matches!(
            diff.entries.as_slice(),
            [DiffEntry::Changed { fields, .. }]
                if fields.as_slice() == [SketchField::Code]
        ));
    }

    #[test]
    fn semantic_diff_ignores_document_relocation_alone() {
        let previous = SketchContracts::from_catalog(
            TestCatalog::new()
                .with_file(
                    "previous.yml",
                    &ContractYaml::linked("same", "same_body", "function", "let value = 1;"),
                )
                .into_catalog(),
        )
        .expect("previous");
        let current = SketchContracts::from_catalog(
            TestCatalog::new()
                .with_file(
                    "current.YAML",
                    &ContractYaml::linked("same", "same_body", "function", "let value = 1;"),
                )
                .into_catalog(),
        )
        .expect("current");

        let diff = SketchContracts::diff(&current, &previous);

        assert!(!diff.changed());
        assert!(diff.entries.is_empty());
    }

    #[test]
    fn semantic_diff_fields_are_complete_and_canonically_ordered() {
        let previous = DigestSketch::contracts("previous.yml", &[DigestSketch::alpha()]);
        let cases = [
            (
                DigestSketch {
                    file: "other.rs",
                    ..DigestSketch::alpha()
                },
                SketchField::SourceFile,
            ),
            (
                DigestSketch {
                    signature: "renamed",
                    ..DigestSketch::alpha()
                },
                SketchField::LinkedSignature,
            ),
            (
                DigestSketch {
                    signature_type: "method",
                    ..DigestSketch::alpha()
                },
                SketchField::SignatureType,
            ),
            (
                DigestSketch {
                    occurrence: "exactly_one",
                    ..DigestSketch::alpha()
                },
                SketchField::Occurrence,
            ),
            (
                DigestSketch {
                    code: "let alpha = 2;",
                    ..DigestSketch::alpha()
                },
                SketchField::Code,
            ),
        ];

        for (current, expected) in cases {
            let current = DigestSketch::contracts("current.yml", &[current]);
            let diff = SketchContracts::diff(&current, &previous);
            let [DiffEntry::Changed { fields, .. }] = diff.entries.as_slice() else {
                panic!("expected a changed entry for {expected:?}");
            };

            assert_eq!(fields.as_slice(), [expected]);
        }

        let current = DigestSketch::contracts(
            "current.yml",
            &[DigestSketch {
                signature: "renamed",
                id: "alpha_body",
                file: "other.rs",
                signature_type: "method",
                occurrence: "exactly_one",
                code: "let alpha = 2;",
            }],
        );
        let diff = SketchContracts::diff(&current, &previous);
        let [DiffEntry::Changed { fields, .. }] = diff.entries.as_slice() else {
            panic!("expected one entry containing every semantic field change");
        };
        let expected = [
            SketchField::SourceFile,
            SketchField::LinkedSignature,
            SketchField::SignatureType,
            SketchField::Occurrence,
            SketchField::Code,
        ];

        assert_eq!(fields.as_slice(), expected);
        assert_eq!(
            serde_json::to_value(fields).expect("serialize changed sketch fields"),
            serde_json::json!([
                "SourceFile",
                "LinkedSignature",
                "SignatureType",
                "Occurrence",
                "Code"
            ])
        );
    }

    #[test]
    fn digest_v2_exact_golden_vectors_cover_empty_one_and_multiple_sketches() {
        let empty = SketchContracts::from_catalog(FileCatalog::new()).expect("empty contracts");
        let one = SketchContracts::from_catalog(
            TestCatalog::new()
                .with_file(
                    "alpha.yml",
                    &DigestSketch::document(&[DigestSketch::alpha()]),
                )
                .into_catalog(),
        )
        .expect("one sketch");
        let multiple = SketchContracts::from_catalog(
            TestCatalog::new()
                .with_file(
                    "multiple.yml",
                    &DigestSketch::document(&[DigestSketch::alpha(), DigestSketch::beta()]),
                )
                .into_catalog(),
        )
        .expect("multiple sketches");

        assert_eq!(
            SketchContracts::diff(&empty, &empty).contract_digest,
            "d18f319a1aeba7fe24318aa1d5497b449b549e3ccf34853daf4ccce58a616572"
        );

        let one_diff = SketchContracts::diff(&one, &empty);
        assert_eq!(
            one_diff.contract_digest,
            "0d7f34949c3e4a9dba3445397a2289c5eced7f4d54cc60ddcac3654c8d354d07"
        );
        let [DiffEntry::Added { current }] = one_diff.entries.as_slice() else {
            panic!("one-sketch diff must expose its current snapshot");
        };
        assert_eq!(
            current.code_digest,
            "0536db1003c163b329abdb3f1eebd25d8b1be473c8f493ceae3bc6c21441b6fe"
        );
        assert_ne!(current.code_digest, one_diff.contract_digest);

        assert_eq!(
            SketchContracts::diff(&multiple, &empty).contract_digest,
            "d369d21f3eb4b36ba978745e004e78b920b45070b196cb3355e18b39078fcef8"
        );
    }

    #[test]
    fn digest_v2_is_stable_across_yaml_order_catalog_insertion_and_line_endings() {
        let ordered = SketchContracts::from_catalog(
            TestCatalog::new()
                .with_file(
                    "multiple.yml",
                    &DigestSketch::document(&[DigestSketch::alpha(), DigestSketch::beta()]),
                )
                .into_catalog(),
        )
        .expect("ordered contracts");
        let reversed = SketchContracts::from_catalog(
            TestCatalog::new()
                .with_file(
                    "multiple.yml",
                    &DigestSketch::document(&[DigestSketch::beta(), DigestSketch::alpha()]),
                )
                .into_catalog(),
        )
        .expect("reversed contracts");

        let mut forward_catalog = FileCatalog::new();
        forward_catalog
            .insert(
                CatalogPath::new("alpha.yml").expect("alpha path"),
                DigestSketch::document(&[DigestSketch::alpha()]).into_bytes(),
            )
            .expect("insert alpha");
        forward_catalog
            .insert(
                CatalogPath::new("beta.yml").expect("beta path"),
                DigestSketch::document(&[DigestSketch::beta()]).into_bytes(),
            )
            .expect("insert beta");
        let mut reverse_catalog = FileCatalog::new();
        reverse_catalog
            .insert(
                CatalogPath::new("beta.yml").expect("beta path"),
                DigestSketch::document(&[DigestSketch::beta()]).into_bytes(),
            )
            .expect("insert beta");
        reverse_catalog
            .insert(
                CatalogPath::new("alpha.yml").expect("alpha path"),
                DigestSketch::document(&[DigestSketch::alpha()]).into_bytes(),
            )
            .expect("insert alpha");
        let forward = SketchContracts::from_catalog(forward_catalog).expect("forward catalog");
        let reverse = SketchContracts::from_catalog(reverse_catalog).expect("reverse catalog");

        let ordered_digest = SketchContracts::diff(&ordered, &ordered).contract_digest;
        let reordered = SketchContracts::diff(&reversed, &ordered);
        let forward_digest = SketchContracts::diff(&forward, &forward).contract_digest;
        let reinserted = SketchContracts::diff(&reverse, &forward);
        assert!(!reordered.changed());
        assert_eq!(ordered_digest, reordered.contract_digest);
        assert!(!reinserted.changed());
        assert_eq!(forward_digest, reinserted.contract_digest);

        let lf = SketchContracts::from_catalog(
            TestCatalog::new()
                .with_file(
                    "line-endings.yml",
                    &DigestSketch::document(&[DigestSketch::alpha_with_code(
                        "line one\\nline two",
                    )]),
                )
                .into_catalog(),
        )
        .expect("LF sketch");
        let crlf = SketchContracts::from_catalog(
            TestCatalog::new()
                .with_file(
                    "line-endings.yml",
                    &DigestSketch::document(&[DigestSketch::alpha_with_code(
                        "line one\\r\\nline two",
                    )]),
                )
                .into_catalog(),
        )
        .expect("CRLF sketch");
        let line_ending_diff = SketchContracts::diff(&crlf, &lf);

        assert!(!line_ending_diff.changed());
        assert_eq!(
            line_ending_diff.contract_digest,
            SketchContracts::diff(&lf, &crlf).contract_digest
        );
        let empty = DigestSketch::empty_contracts();
        let lf_added = SketchContracts::diff(&lf, &empty);
        let crlf_added = SketchContracts::diff(&crlf, &empty);
        let [
            DiffEntry::Added {
                current: lf_snapshot,
            },
        ] = lf_added.entries.as_slice()
        else {
            panic!("LF sketch must produce an added snapshot");
        };
        let [
            DiffEntry::Added {
                current: crlf_snapshot,
            },
        ] = crlf_added.entries.as_slice()
        else {
            panic!("CRLF sketch must produce an added snapshot");
        };
        assert_eq!(lf_snapshot.code_digest, crlf_snapshot.code_digest);

        let retained_carriage_return = SketchContracts::from_catalog(
            TestCatalog::new()
                .with_file(
                    "isolated-carriage-return.yml",
                    &DigestSketch::document(&[DigestSketch::alpha_with_code(
                        "line one\\r\\r\\nline two",
                    )]),
                )
                .into_catalog(),
        )
        .expect("isolated carriage return sketch");
        let isolated_diff = SketchContracts::diff(&retained_carriage_return, &crlf);

        assert!(isolated_diff.changed());
        assert!(matches!(
            isolated_diff.entries.as_slice(),
            [DiffEntry::Changed { fields, .. }]
                if fields.as_slice() == [SketchField::Code]
        ));
    }

    struct DigestSketch {
        signature: &'static str,
        id: &'static str,
        file: &'static str,
        signature_type: &'static str,
        occurrence: &'static str,
        code: &'static str,
    }

    impl DigestSketch {
        const fn named(
            signature: &'static str,
            id: &'static str,
            file: &'static str,
            code: &'static str,
        ) -> Self {
            Self {
                signature,
                id,
                file,
                signature_type: "function",
                occurrence: "at_least_one",
                code,
            }
        }

        const fn alpha() -> Self {
            Self::alpha_with_code("let alpha = 1;")
        }

        fn contracts(path: &str, sketches: &[Self]) -> ParsedSketchContracts {
            SketchContracts::from_catalog(
                TestCatalog::new()
                    .with_file(path, &Self::document(sketches))
                    .into_catalog(),
            )
            .expect("digest sketch contracts")
        }

        const fn alpha_with_code(code: &'static str) -> Self {
            Self {
                signature: "alpha",
                id: "alpha_body",
                file: "a.rs",
                signature_type: "function",
                occurrence: "at_least_one",
                code,
            }
        }

        const fn beta() -> Self {
            Self {
                signature: "beta",
                id: "beta_body",
                file: "b.rs",
                signature_type: "function",
                occurrence: "exactly_one",
                code: "let beta = 2;",
            }
        }

        fn document(sketches: &[Self]) -> String {
            let crate_root = sketches.first().expect("digest sketch fixture").file;
            let files = sketches
                .iter()
                .map(|sketch| sketch.file)
                .collect::<Vec<_>>()
                .join(", ");
            let signatures = sketches
                .iter()
                .map(|sketch| {
                    format!(
                        "  - {}:\n      file: {}\n      signature_type: {}\n      sketch: {}\n",
                        sketch.signature, sketch.file, sketch.signature_type, sketch.id
                    )
                })
                .collect::<String>();
            let sketch_values = sketches
                .iter()
                .map(|sketch| {
                    format!(
                        "  - {}:\n      file: {}\n      signature: {}\n      signature_type: {}\n      matching: {{ normalization: exact_lines_v1, occurrence: {} }}\n      code: \"{}\"\n",
                        sketch.id,
                        sketch.file,
                        sketch.signature,
                        sketch.signature_type,
                        sketch.occurrence,
                        sketch.code
                    )
                })
                .collect::<String>();

            format!(
                "contract_version: 2\nroot: ../src\nfiles: [{files}]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: example, root: {crate_root}, kind: library }}] }}\nsignatures:\n{signatures}sketches:\n{sketch_values}"
            )
        }

        fn empty_contracts() -> ParsedSketchContracts {
            SketchContracts::from_catalog(FileCatalog::new()).expect("empty contracts")
        }
    }
}
