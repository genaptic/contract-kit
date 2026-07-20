use super::document::{
    SketchContractDocuments, SketchSemanticDocument, SketchSemanticSignature, SketchYamlInput,
};
use super::model::{
    ContractDocumentLocator, SignatureLabel, SignatureType, SketchContract, SketchContracts,
    SketchMatchPolicy, SketchSnippet,
};
use crate::error::SketchContractKitError;
use crate::files::CatalogPath;
use crate::id::SketchId;
use crate::limits::{MatchingLimits, SketchLimits};
use crate::work::CancellationProbe;
use std::collections::{BTreeMap, BTreeSet};

impl SketchContractDocuments {
    pub(crate) fn contracts(
        &self,
        limits: &SketchLimits,
        cancellation: &CancellationProbe,
    ) -> Result<SketchContracts, SketchContractKitError> {
        let mut sketches = BTreeMap::<SketchId, SketchDeclaration>::new();
        let mut links = BTreeMap::<SketchId, Vec<SignatureLink>>::new();
        let mut signature_count = 0_usize;

        for (catalog_name, file) in &self.files {
            for document in &file.documents {
                cancellation.checkpoint()?;
                let input = &document.semantic;
                let location = format!("{catalog_name} document {}", document.index);
                input.validate_root(&location)?;
                let files = ContractFiles::from_input(&input.files, &location, cancellation)?;
                let mut document_signature_labels = BTreeSet::new();
                for (index, value) in input.signatures.iter().enumerate() {
                    cancellation.checkpoint_at(index)?;
                    signature_count = signature_count.saturating_add(1);
                    limits.matching.validate_signature_index_entry_count(
                        signature_count,
                        Some(catalog_name.clone()),
                    )?;
                    let signature = SignatureIndexEntry::from_yaml(
                        value,
                        catalog_name,
                        &location,
                        &files,
                        &limits.matching,
                        cancellation,
                    )?;
                    if !document_signature_labels.insert(signature.label.clone()) {
                        return Err(SketchContractKitError::parse_failed(
                            &location,
                            format!("duplicate signature label {}", signature.label.as_str()),
                        ));
                    }

                    let locator =
                        ContractDocumentLocator::new(catalog_name.clone(), document.index);
                    if let Some(link) = signature.into_link(locator) {
                        links.entry(link.sketch_id.clone()).or_default().push(link);
                    }
                }

                for (index, value) in input.sketches.iter().enumerate() {
                    cancellation.checkpoint_at(index)?;
                    let sketch = SketchDeclaration::from_yaml(
                        value,
                        ContractDocumentLocator::new(catalog_name.clone(), document.index),
                        &location,
                        &files,
                        &limits.matching,
                        cancellation,
                    )?;
                    if let Some(previous) = sketches.insert(sketch.id.clone(), sketch) {
                        return Err(SketchContractKitError::parse_failed(
                            &location,
                            format!(
                                "duplicate sketch id {} (also declared in {})",
                                previous.id.as_str(),
                                previous.locator
                            ),
                        ));
                    }
                    limits
                        .matching
                        .validate_sketch_count(sketches.len(), Some(catalog_name.clone()))?;
                }
            }
        }

        let mut entries = Vec::with_capacity(sketches.len());
        for (index, (sketch_id, sketch)) in sketches.into_iter().enumerate() {
            cancellation.checkpoint_at(index)?;
            let Some(mut sketch_links) = links.remove(&sketch_id) else {
                return Err(SketchContractKitError::parse_failed(
                    &sketch.locator,
                    format!(
                        "orphan sketch {} is not referenced by a signature",
                        sketch_id.as_str()
                    ),
                ));
            };

            if sketch_links.len() != 1 {
                return Err(SketchContractKitError::parse_failed(
                    &sketch.locator,
                    format!(
                        "sketch {} is referenced by more than one signature",
                        sketch_id.as_str()
                    ),
                ));
            }

            let link = sketch_links.pop().ok_or_else(|| {
                SketchContractKitError::parse_failed(
                    &sketch.locator,
                    format!("sketch {} has no signature link", sketch_id.as_str()),
                )
            })?;
            if link.locator != sketch.locator {
                return Err(SketchContractKitError::parse_failed(
                    &link.locator,
                    format!(
                        "signature {} links to sketch {} in another contract document {}",
                        link.label.as_str(),
                        sketch_id.as_str(),
                        sketch.locator
                    ),
                ));
            }
            if link.file != sketch.file {
                return Err(SketchContractKitError::parse_failed(
                    &sketch.locator,
                    format!(
                        "sketch {} file {} does not match linked signature {} file {}",
                        sketch_id.as_str(),
                        sketch.file,
                        link.label.as_str(),
                        link.file
                    ),
                ));
            }
            if link.label != sketch.signature {
                return Err(SketchContractKitError::parse_failed(
                    &sketch.locator,
                    format!(
                        "sketch {} signature {} does not match linked signature {}",
                        sketch_id.as_str(),
                        sketch.signature.as_str(),
                        link.label.as_str()
                    ),
                ));
            }
            if link.signature_type != sketch.signature_type {
                return Err(SketchContractKitError::parse_failed(
                    &sketch.locator,
                    format!(
                        "sketch {} signature_type {} does not match linked signature {} type {}",
                        sketch_id.as_str(),
                        sketch.signature_type.as_str(),
                        link.label.as_str(),
                        link.signature_type.as_str()
                    ),
                ));
            }

            entries.push(SketchContract::from_link(sketch, link));
        }

        if let Some((sketch_id, remaining_links)) = links.into_iter().next() {
            let Some(link) = remaining_links.into_iter().next() else {
                return Err(SketchContractKitError::conversion_failed(
                    "internal sketch link collection was empty",
                ));
            };
            return Err(SketchContractKitError::parse_failed(
                &link.locator,
                format!(
                    "signature {} references missing sketch {}",
                    link.label.as_str(),
                    sketch_id.as_str()
                ),
            ));
        }

        let contract_document_count = self.files.values().map(|file| file.documents.len()).sum();

        Ok(SketchContracts::from_resolved(
            entries,
            contract_document_count,
        ))
    }
}

impl SketchSemanticDocument {
    fn validate_root(&self, location: &str) -> Result<(), SketchContractKitError> {
        if self.root.trim().is_empty() {
            return Err(SketchContractKitError::parse_failed(
                location,
                "contract root must not be empty",
            ));
        }

        Ok(())
    }
}

impl SketchContract {
    fn from_link(sketch: SketchDeclaration, link: SignatureLink) -> Self {
        Self::from_resolved(
            sketch.id,
            sketch.locator,
            link.file,
            link.label,
            sketch.signature_type,
            sketch.policy,
            sketch.snippet,
        )
    }
}

struct ContractFiles {
    paths: BTreeSet<CatalogPath>,
}

impl ContractFiles {
    fn from_input(
        values: &[String],
        location: &str,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SketchContractKitError> {
        let mut paths = BTreeSet::new();

        for (index, value) in values.iter().enumerate() {
            cancellation.checkpoint_at(index)?;
            let path = CatalogPath::new(value.clone()).map_err(|source| {
                SketchContractKitError::parse_failed(
                    location,
                    format!("contract files contains an invalid path: {source}"),
                )
            })?;
            if !paths.insert(path.clone()) {
                return Err(SketchContractKitError::parse_failed(
                    location,
                    format!("duplicate contract file {path}"),
                ));
            }
        }

        Ok(Self { paths })
    }

    fn contains(&self, path: &CatalogPath) -> bool {
        self.paths.contains(path)
    }
}

struct SignatureIndexEntry {
    label: SignatureLabel,
    file: CatalogPath,
    signature_type: SignatureType,
    sketch_id: Option<SketchId>,
}

impl SignatureIndexEntry {
    fn from_yaml(
        value: &SketchSemanticSignature,
        catalog_name: &CatalogPath,
        location: &str,
        files: &ContractFiles,
        limits: &MatchingLimits,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SketchContractKitError> {
        cancellation.checkpoint()?;
        let label = SignatureLabel::new(value.label.clone(), location)?;
        cancellation.checkpoint()?;
        let file = CatalogPath::new(value.file.clone()).map_err(|source| {
            SketchContractKitError::parse_failed(
                location,
                format!("signature {} has invalid file: {source}", label.as_str()),
            )
        })?;
        if !files.contains(&file) {
            return Err(SketchContractKitError::parse_failed(
                location,
                format!(
                    "signature {} references unlisted file {file}",
                    label.as_str()
                ),
            ));
        }

        let signature_type = SignatureType::from_contract(
            value.signature_type.clone(),
            location,
            &format!("signature {}", label.as_str()),
        )?;

        cancellation.checkpoint()?;
        let sketch_id = if let Some(value) = value.sketch.as_deref() {
            Some(
                SketchId::new(value.to_owned(), limits.sketch_id_maximum()).map_err(|source| {
                    SketchContractKitError::invalid_sketch_id(
                        format!("{catalog_name}: signature {} link", label.as_str()),
                        source,
                    )
                })?,
            )
        } else {
            None
        };

        Ok(Self {
            label,
            file,
            signature_type,
            sketch_id,
        })
    }

    fn into_link(self, locator: ContractDocumentLocator) -> Option<SignatureLink> {
        self.sketch_id.map(|sketch_id| SignatureLink {
            label: self.label,
            locator,
            file: self.file,
            signature_type: self.signature_type,
            sketch_id,
        })
    }
}

struct SignatureLink {
    label: SignatureLabel,
    locator: ContractDocumentLocator,
    file: CatalogPath,
    signature_type: SignatureType,
    sketch_id: SketchId,
}

struct SketchDeclaration {
    id: SketchId,
    locator: ContractDocumentLocator,
    file: CatalogPath,
    signature: SignatureLabel,
    signature_type: SignatureType,
    policy: SketchMatchPolicy,
    snippet: SketchSnippet,
}

impl SketchDeclaration {
    fn from_yaml(
        value: &SketchYamlInput,
        locator: ContractDocumentLocator,
        location: &str,
        files: &ContractFiles,
        limits: &MatchingLimits,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SketchContractKitError> {
        cancellation.checkpoint()?;
        let id = SketchId::new(value.id.clone(), limits.sketch_id_maximum()).map_err(|source| {
            SketchContractKitError::invalid_sketch_id(
                format!("{locator}: sketch declaration"),
                source,
            )
        })?;
        cancellation.checkpoint()?;
        let file = CatalogPath::new(value.file.clone()).map_err(|source| {
            SketchContractKitError::parse_failed(
                location,
                format!("sketch {} has invalid file: {source}", id.as_str()),
            )
        })?;
        if !files.contains(&file) {
            return Err(SketchContractKitError::parse_failed(
                location,
                format!("sketch {} references unlisted file {file}", id.as_str()),
            ));
        }
        cancellation.checkpoint()?;
        let signature = SignatureLabel::new(value.signature.clone(), location)?;
        let signature_type = SignatureType::from_contract(
            value.signature_type.clone(),
            location,
            &format!("sketch {}", id.as_str()),
        )?;
        cancellation.checkpoint()?;
        let policy = value.matching;
        let snippet = SketchSnippet::new(
            &value.code,
            policy.normalization(),
            &locator,
            id.as_str(),
            limits,
            cancellation,
        )?;

        Ok(Self {
            id,
            locator,
            file,
            signature,
            signature_type,
            policy,
            snippet,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::document::{
        SketchContractDocuments, SketchSemanticSignature, SketchYamlInput,
    };
    use super::super::model::{
        ContractDocumentLocator, SketchMatchPolicy, SketchNormalization, SketchOccurrence,
    };
    use super::{ContractFiles, SignatureIndexEntry, SketchDeclaration};
    use crate::contract::tests::{ContractYaml, SketchContracts, TestCatalog};
    use crate::files::CatalogPath;
    use crate::limits::SketchLimits;
    use crate::work::CancellationProbe;

    struct LinkValidationCase {
        name: &'static str,
        catalog: TestCatalog,
        expected_diagnostics: &'static [&'static str],
    }

    impl LinkValidationCase {
        fn assert_rejected(self) {
            let Err(error) = SketchContracts::from_catalog(self.catalog.into_catalog()) else {
                panic!("{} must be rejected", self.name);
            };
            let rendered = error.to_string();
            for diagnostic in self.expected_diagnostics {
                assert!(
                    rendered.contains(diagnostic),
                    "{}: expected {diagnostic:?} in {rendered:?}",
                    self.name,
                );
            }
        }
    }

    #[test]
    fn unknown_matching_policy_versions_and_fields_are_rejected() {
        let valid = ContractYaml::linked("answer", "answer_body", "function", "fn answer() {}");
        for (current, invalid) in [
            ("exact_lines_v1", "future_lines_v9"),
            ("at_least_one", "somewhere_once"),
        ] {
            let yaml = valid.replace(current, invalid);
            let error = SketchContracts::from_catalog(
                TestCatalog::new()
                    .with_file("main.yml", &yaml)
                    .into_catalog(),
            )
            .expect_err("unknown matching policy value");
            let rendered = error.to_string();

            assert!(rendered.contains(invalid), "{rendered}");
            assert!(rendered.contains("main.yml document 0"), "{rendered}");
        }

        let unknown_field = valid.replace(
            "occurrence: at_least_one }",
            "occurrence: at_least_one, future: true }",
        );
        let error = SketchContracts::from_catalog(
            TestCatalog::new()
                .with_file("main.yml", &unknown_field)
                .into_catalog(),
        )
        .expect_err("unknown matching policy field");

        assert!(error.to_string().contains("unknown field `future`"));
    }

    #[test]
    fn flattened_v2_sketch_yaml_is_rejected() {
        let yaml = ContractYaml::linked("answer", "answer_body", "function", "fn answer() {}")
            .replace(
                "  - answer_body:\n      file: lib.rs\n      signature: answer\n      signature_type: function\n      matching: { normalization: exact_lines_v1, occurrence: at_least_one }\n      code:",
                "  - answer_body:\n    signature_type: function\n    matching: { normalization: exact_lines_v1, occurrence: at_least_one }\n    code:",
            );
        let catalog = TestCatalog::new()
            .with_file("main.yml", &yaml)
            .into_catalog();

        let error = SketchContracts::from_catalog(catalog).expect_err("flattened sketch");

        assert!(error.to_string().contains("main.yml document 0"));
    }

    #[test]
    fn later_versioned_reverse_link_dialect_is_rejected() {
        let catalog = TestCatalog::new()
            .with_file(
                "main.yml",
                "version: 1\nlanguage: rust\nsketches:\n- answer:\n    file: lib.rs\n    signature: answer\n    code: fn answer() {}\n",
            )
            .into_catalog();

        let error = SketchContracts::from_catalog(catalog).expect_err("later dialect");

        assert!(error.to_string().contains("unknown field `version`"));
    }

    #[test]
    fn signatures_and_sketches_are_mandatory_v2_root_fields() {
        let source = "contract_version: 2\nroot: ../src\nfiles: []\nsignatures: []\nsketches: []\n";

        for (field, line) in [
            ("signatures", "signatures: []\n"),
            ("sketches", "sketches: []\n"),
        ] {
            let yaml = source.replacen(line, "", 1);
            let catalog = TestCatalog::new()
                .with_file("main.yml", &yaml)
                .into_catalog();
            let error = SketchContracts::from_catalog(catalog)
                .expect_err("mandatory v2 collection root must be rejected when absent");
            let rendered = error.to_string();

            assert!(
                rendered.contains(&format!("missing field `{field}`")),
                "{rendered}"
            );
            assert!(rendered.contains("main.yml document 0"), "{rendered}");
        }
    }

    #[test]
    fn extraction_is_opaque_but_required_for_signature_bearing_documents() {
        let without_extraction = TestCatalog::new()
            .with_file(
                "main.yml",
                r#"contract_version: 2
root: ../src
files: [lib.rs]
signatures:
  - answer:
      file: lib.rs
      signature_type: function
sketches: []
"#,
            )
            .into_catalog();
        let error = SketchContracts::from_catalog(without_extraction)
            .expect_err("signature-bearing documents require extraction");
        let rendered = error.to_string();

        assert!(
            rendered.contains("signature-bearing contract document requires extraction"),
            "{rendered}"
        );
        assert!(rendered.contains("main.yml document 0"), "{rendered}");

        let opaque_extraction = TestCatalog::new()
            .with_file(
                "main.yml",
                r#"contract_version: 2
root: ../src
files: [lib.rs]
extraction:
  mode: compiler_v99
  profile: future_api
  crates:
    - signature_owned: [arbitrary, values]
  extension:
    nested: { enabled: true }
signatures:
  - answer:
      file: lib.rs
      signature_type: function
sketches: []
"#,
            )
            .into_catalog();
        let contracts = SketchContracts::from_catalog(opaque_extraction)
            .expect("sketch parsing must not interpret signature-owned extraction fields");

        assert!(contracts.entries().is_empty());
        assert_eq!(contracts.contract_document_count(), 1);

        let empty_signatures = TestCatalog::new()
            .with_file(
                "main.yml",
                "contract_version: 2\nroot: ../src\nfiles: []\nsignatures: []\nsketches: []\n",
            )
            .into_catalog();
        SketchContracts::from_catalog(empty_signatures)
            .expect("empty signature documents do not require extraction");
    }

    #[test]
    fn nested_sketch_body_requires_all_v2_fields() {
        let catalog = TestCatalog::new()
            .with_file(
                "main.yml",
                r#"
contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: example, root: lib.rs, kind: library }] }
signatures:
  - answer:
      file: lib.rs
      signature_type: function
      sketch: answer_body
sketches:
  - answer_body:
      file: lib.rs
      signature: answer
      code: fn answer() {}
"#,
            )
            .into_catalog();

        let error = SketchContracts::from_catalog(catalog).expect_err("incomplete nested body");

        assert!(error.to_string().contains("missing field `signature_type`"));
        assert!(error.to_string().contains("main.yml document 0"));
    }

    #[test]
    fn invalid_links_and_document_scoped_identifiers_report_their_exact_cause() {
        let orphan = r#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: example, root: lib.rs, kind: library }] }
signatures:
  - answer:
      file: lib.rs
      signature_type: function
sketches:
  - answer_body:
      file: lib.rs
      signature: answer
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: at_least_one }
      code: fn answer() {}
"#;
        let missing = r#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: example, root: lib.rs, kind: library }] }
signatures:
  - answer:
      file: lib.rs
      signature_type: function
      sketch: answer_body
sketches: []
"#;
        let multiply_referenced = r#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: example, root: lib.rs, kind: library }] }
signatures:
  - first:
      file: lib.rs
      signature_type: function
      sketch: shared
  - second:
      file: lib.rs
      signature_type: function
      sketch: shared
sketches:
  - shared:
      file: lib.rs
      signature: first
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: at_least_one }
      code: fn shared() {}
"#;
        let mismatched_type =
            ContractYaml::linked("answer", "answer_body", "function", "fn answer() {}").replace(
                "      signature_type: function\n      matching:",
                "      signature_type: method\n      matching:",
            );
        let cross_document_signature = r#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: example, root: lib.rs, kind: library }] }
signatures:
  - answer:
      file: lib.rs
      signature_type: function
      sketch: answer_body
sketches: []
"#;
        let cross_document_sketch = r#"contract_version: 2
root: ../src
files: [other.rs]
signatures: []
sketches:
  - answer_body:
      file: other.rs
      signature: answer
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: at_least_one }
      code: fn answer() {}
"#;
        let first_duplicate = ContractYaml::linked("first", "shared", "function", "fn first() {}");
        let second_duplicate =
            ContractYaml::linked("second", "shared", "function", "fn second() {}")
                .replace("files: [lib.rs]", "files: [other.rs]")
                .replace("root: lib.rs", "root: other.rs")
                .replace("file: lib.rs", "file: other.rs");
        let duplicate_signature = r#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: example, root: lib.rs, kind: library }] }
signatures:
  - answer:
      file: lib.rs
      signature_type: function
  - answer:
      file: lib.rs
      signature_type: function
sketches: []
"#;
        let duplicate_file = "contract_version: 2\nroot: ../src\nfiles: [lib.rs, lib.rs]\nsignatures: []\nsketches: []\n";
        let unlisted_signature_file = "contract_version: 2\nroot: ../src\nfiles: [root.rs]\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: example, root: root.rs, kind: library }] }\nsignatures:\n  - answer:\n      file: lib.rs\n      signature_type: function\nsketches: []\n";

        for case in [
            LinkValidationCase {
                name: "orphan sketch",
                catalog: TestCatalog::new().with_file("main.yml", orphan),
                expected_diagnostics: &["orphan sketch answer_body"],
            },
            LinkValidationCase {
                name: "missing linked sketch",
                catalog: TestCatalog::new().with_file("main.yml", missing),
                expected_diagnostics: &["references missing sketch answer_body"],
            },
            LinkValidationCase {
                name: "multiply referenced sketch",
                catalog: TestCatalog::new().with_file("main.yml", multiply_referenced),
                expected_diagnostics: &["referenced by more than one signature"],
            },
            LinkValidationCase {
                name: "signature type mismatch",
                catalog: TestCatalog::new().with_file("main.yml", &mismatched_type),
                expected_diagnostics: &["signature_type method", "type function"],
            },
            LinkValidationCase {
                name: "cross-document link",
                catalog: TestCatalog::new()
                    .with_file("a.yml", cross_document_signature)
                    .with_file("b.yaml", cross_document_sketch),
                expected_diagnostics: &["another contract document"],
            },
            LinkValidationCase {
                name: "global duplicate sketch id",
                catalog: TestCatalog::new()
                    .with_file("a.yml", &first_duplicate)
                    .with_file("b.yml", &second_duplicate),
                expected_diagnostics: &["duplicate sketch id shared"],
            },
            LinkValidationCase {
                name: "duplicate signature label",
                catalog: TestCatalog::new().with_file("main.yml", duplicate_signature),
                expected_diagnostics: &["duplicate signature label answer", "main.yml document 0"],
            },
            LinkValidationCase {
                name: "duplicate source file",
                catalog: TestCatalog::new().with_file("main.yml", duplicate_file),
                expected_diagnostics: &["duplicate contract file lib.rs", "main.yml document 0"],
            },
            LinkValidationCase {
                name: "unlisted signature file",
                catalog: TestCatalog::new().with_file("main.yml", unlisted_signature_file),
                expected_diagnostics: &["references unlisted file lib.rs"],
            },
        ] {
            case.assert_rejected();
        }
    }

    #[test]
    fn duplicate_signature_labels_are_scoped_to_their_document() {
        let first = ContractYaml::linked("answer", "first_body", "function", "fn first() {}");
        let second = ContractYaml::linked("answer", "second_body", "function", "fn second() {}")
            .replace("files: [lib.rs]", "files: [other.rs]")
            .replace("root: lib.rs", "root: other.rs")
            .replace("file: lib.rs", "file: other.rs");
        let catalog = TestCatalog::new()
            .with_file("a.yml", &first)
            .with_file("b.yml", &second)
            .into_catalog();

        let contracts = SketchContracts::from_catalog(catalog)
            .expect("same label in separate documents is unambiguous");

        assert_eq!(contracts.entries().len(), 2);
    }

    #[test]
    fn separate_documents_may_share_the_same_source_file() {
        let catalog = TestCatalog::new()
            .with_file(
                "main.yml",
                "---\ncontract_version: 2\nroot: ../src\nfiles: [lib.rs]\nsignatures: []\nsketches: []\n---\ncontract_version: 2\nroot: ../src\nfiles: [lib.rs]\nsignatures: []\nsketches: []\n",
            )
            .into_catalog();

        let contracts = SketchContracts::from_catalog(catalog).expect("shared allowlist file");

        assert!(contracts.entries().is_empty());
        assert_eq!(contracts.contract_document_count(), 2);
    }

    #[test]
    fn unknown_root_fields_are_rejected() {
        let catalog = TestCatalog::new()
            .with_file(
                "main.yml",
                "contract_version: 2\nroot: ../src\nfiles: []\nsignatures: []\nsketches: []\nextra: false\n",
            )
            .into_catalog();

        let error = SketchContracts::from_catalog(catalog).expect_err("unknown root field");

        assert!(error.to_string().contains("unknown field `extra`"));
    }

    #[test]
    fn empty_and_whitespace_contract_roots_are_rejected() {
        for root in ["''", "'   '"] {
            let yaml = format!(
                "contract_version: 2\nroot: {root}\nfiles: []\nsignatures: []\nsketches: []\n"
            );
            let catalog = TestCatalog::new()
                .with_file("main.yml", &yaml)
                .into_catalog();

            let error =
                SketchContracts::from_catalog(catalog).expect_err("empty contract root must fail");

            assert!(
                error
                    .to_string()
                    .contains("contract root must not be empty")
            );
        }
    }

    #[test]
    fn cancellation_stops_link_and_entry_resolution() {
        let path = CatalogPath::new("main.yml").expect("path");
        let source = "contract_version: 2\nroot: ../src\nfiles: []\nsignatures: []\nsketches: []\n";
        let limits = SketchLimits::default();

        let link_cancellation = CancellationProbe::new();
        let mut link_budget = limits.yaml_budget();
        let documents = SketchContractDocuments::from_catalog(
            TestCatalog::new()
                .with_file("main.yml", source)
                .into_catalog(),
            &mut link_budget,
            &link_cancellation,
        )
        .expect("semantic contract documents");
        link_cancellation.cancel();
        assert!(matches!(
            documents.contracts(&limits, &link_cancellation),
            Err(error) if error.is_operation_cancelled()
        ));

        let active = CancellationProbe::new();
        let files = ContractFiles::from_input(&["lib.rs".to_owned()], "main.yml", &active)
            .expect("contract files");
        let cancellation = CancellationProbe::new();
        cancellation.cancel();
        let Err(signature_error) = SignatureIndexEntry::from_yaml(
            &SketchSemanticSignature {
                label: "answer".to_owned(),
                file: "lib.rs".to_owned(),
                signature_type: "function".to_owned(),
                sketch: Some("answer_body".to_owned()),
            },
            &path,
            "main.yml document 0",
            &files,
            &limits.matching,
            &cancellation,
        ) else {
            panic!("cancelled signature-index conversion must fail");
        };
        assert!(signature_error.to_string().contains("cancelled"));

        let Err(sketch_error) = SketchDeclaration::from_yaml(
            &SketchYamlInput {
                id: "answer_body".to_owned(),
                file: "lib.rs".to_owned(),
                signature: "answer".to_owned(),
                signature_type: "function".to_owned(),
                matching: SketchMatchPolicy::new(
                    SketchNormalization::ExactLinesV1,
                    SketchOccurrence::AtLeastOne,
                ),
                code: "fn answer() {}".to_owned(),
            },
            ContractDocumentLocator::new(path.clone(), 0),
            "main.yml document 0",
            &files,
            &limits.matching,
            &cancellation,
        ) else {
            panic!("cancelled sketch conversion must fail");
        };
        assert!(sketch_error.to_string().contains("cancelled"));
    }
}
