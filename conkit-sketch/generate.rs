use crate::api::{
    GenerateMode, GenerateRequest, GenerateResponse, SketchGenerationCounts, SketchSeed,
};
use crate::contract::{SketchContractDocuments, SketchContracts, SketchNormalization};
use crate::error::SketchContractKitError;
use crate::files::CatalogPath;
use crate::id::SketchId;
use crate::limits::{MatchingLimits, SketchLimits};
use crate::work::CancellationProbe;
use std::collections::{BTreeMap, BTreeSet};

impl GenerateRequest {
    pub(crate) fn run(
        self,
        limits: &SketchLimits,
        cancellation: &CancellationProbe,
    ) -> Result<GenerateResponse, SketchContractKitError> {
        let Self {
            contract_files,
            seeds,
            mode,
        } = self;
        let mut catalog_usage = limits.catalog_usage();
        catalog_usage.record(&contract_files, cancellation)?;
        let mut yaml_budget = limits.yaml_budget();
        cancellation.checkpoint()?;
        let documents =
            SketchContractDocuments::from_catalog(contract_files, &mut yaml_budget, cancellation)?;
        let contracts = documents.contracts(limits, cancellation)?;
        let counts = SketchGenerationCounts::new(contracts.len());
        let seeds = SketchRefreshSeeds::from_input(
            seeds,
            &contracts,
            mode,
            &limits.matching,
            cancellation,
        )?;
        documents.refresh(seeds, counts, limits, &mut yaml_budget, cancellation)
    }
}
pub(crate) struct SketchRefreshSeeds {
    entries: BTreeMap<SketchId, GeneratedSketchCode>,
    target_files: BTreeSet<CatalogPath>,
}

impl SketchRefreshSeeds {
    fn from_input(
        seeds: Vec<SketchSeed>,
        contracts: &SketchContracts,
        mode: GenerateMode,
        limits: &MatchingLimits,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SketchContractKitError> {
        limits.validate_sketch_count(seeds.len(), None)?;
        let mut entries = BTreeMap::new();
        let mut target_files = BTreeSet::new();

        for (index, seed) in seeds.into_iter().enumerate() {
            cancellation.checkpoint_at(index)?;
            let id = SketchId::new(seed.sketch_id.clone(), limits.sketch_id_maximum()).map_err(
                |source| {
                    SketchContractKitError::invalid_sketch_id(
                        format!(
                            "{} document {} refresh seed",
                            seed.contract_file, seed.document_index
                        ),
                        source,
                    )
                },
            )?;
            if entries.contains_key(&id) {
                return Err(SketchContractKitError::conversion_failed(format!(
                    "duplicate sketch refresh seed {}",
                    id.as_str()
                )));
            }
            let Some(contract) = contracts.get(&id) else {
                return Err(SketchContractKitError::conversion_failed(format!(
                    "sketch refresh seed {} has no explicitly linked sketch",
                    id.as_str()
                )));
            };
            contract.validate_seed(&seed, &id)?;
            let code = GeneratedSketchCode::new(
                seed.code,
                id.as_str(),
                limits,
                &seed.contract_file,
                cancellation,
            )?;

            target_files.insert(seed.contract_file);
            entries.insert(id, code);
        }

        let refresh = Self {
            entries,
            target_files,
        };
        if mode == GenerateMode::FullRefresh {
            refresh.validate_full_coverage(contracts, cancellation)?;
        }

        Ok(refresh)
    }

    fn validate_full_coverage(
        &self,
        contracts: &SketchContracts,
        cancellation: &CancellationProbe,
    ) -> Result<(), SketchContractKitError> {
        for (index, contract) in contracts.entries().iter().enumerate() {
            cancellation.checkpoint_at(index)?;
            if !self.entries.contains_key(contract.id()) {
                return Err(SketchContractKitError::conversion_failed(format!(
                    "linked sketch {} is missing a refresh seed",
                    contract.id().as_str()
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn targets_file(&self, path: &CatalogPath) -> bool {
        self.target_files.contains(path)
    }

    pub(crate) fn code_for(&self, id: &str) -> Option<&str> {
        self.entries.get(id).map(GeneratedSketchCode::as_str)
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

struct GeneratedSketchCode {
    value: String,
}

impl GeneratedSketchCode {
    fn new(
        value: impl Into<String>,
        sketch_id: &str,
        limits: &MatchingLimits,
        contract_file: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SketchContractKitError> {
        let value = value.into();
        if SketchNormalization::ExactLinesV1
            .normalize_snippet(value.as_bytes(), limits, contract_file, cancellation)?
            .is_empty()
        {
            return Err(SketchContractKitError::conversion_failed(format!(
                "sketch {sketch_id} code must not be empty"
            )));
        }

        Ok(Self { value })
    }

    fn as_str(&self) -> &str {
        &self.value
    }
}

#[cfg(test)]
mod tests {
    use super::SketchRefreshSeeds;
    use crate::api::{GenerateMode, GenerateRequest, SketchGenerationCounts, SketchSeed};
    use crate::contract::SketchContracts;
    use crate::files::{CatalogPath, FileCatalog};
    use crate::limits::{LimitResource, SketchLimits};
    use crate::work::CancellationProbe;

    const LINKED_CONTRACT: &str = r#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: example, root: lib.rs, kind: library }] }
signatures:
  - answer:
      file: lib.rs
      signature_type: function
      name: answer
      sketch: answer_body
sketches:
  - answer_body:
      file: lib.rs
      signature: answer
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: at_least_one }
      code: |-
        old code
"#;

    #[test]
    fn cancelled_full_refresh_coverage_returns_a_typed_error() {
        let limits = SketchLimits::default();
        let cancellation = CancellationProbe::new();
        let mut yaml_budget = limits.yaml_budget();
        let contracts = SketchContracts::from_catalog(
            TestCatalog::new()
                .with_file("main.yml", LINKED_CONTRACT)
                .into_catalog(),
            &limits,
            &mut yaml_budget,
            &cancellation,
        )
        .expect("linked contracts");
        cancellation.cancel();

        let empty_refresh = SketchRefreshSeeds {
            entries: std::collections::BTreeMap::new(),
            target_files: std::collections::BTreeSet::new(),
        };
        let Err(error) = empty_refresh.validate_full_coverage(&contracts, &cancellation) else {
            panic!("cancelled full-refresh coverage must fail");
        };

        assert!(error.to_string().contains("cancelled"), "{error}");
    }

    struct TestCatalog {
        catalog: FileCatalog,
    }

    impl TestCatalog {
        fn new() -> Self {
            Self {
                catalog: FileCatalog::new(),
            }
        }

        fn with_file(mut self, path: &str, contents: &str) -> Self {
            self.catalog
                .insert(
                    CatalogPath::new(path).expect("test path"),
                    contents.as_bytes().to_vec(),
                )
                .expect("insert test file");
            self
        }

        fn into_catalog(self) -> FileCatalog {
            self.catalog
        }
    }

    struct SeedFixture;

    impl SeedFixture {
        fn answer() -> SketchSeed {
            SketchSeed {
                contract_file: CatalogPath::new("main.yml").expect("contract path"),
                document_index: 0,
                sketch_id: "answer_body".to_owned(),
                signature_type: "function".to_owned(),
                file: CatalogPath::new("lib.rs").expect("source path"),
                code: "pub fn answer() -> u8 { 42 }".to_owned(),
            }
        }

        fn linked_scalar(code_yaml: &str) -> String {
            LINKED_CONTRACT.replacen(
                "code: |-\n        old code",
                &format!("code: {code_yaml}"),
                1,
            )
        }

        fn ancestor_anchored_documents() -> [String; 4] {
            let body_anchored = r#"contract_version: 2
root: ../src
files: [lib.rs]
sketches:
  - answer_body: &shared
      file: lib.rs
      signature: answer
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: at_least_one }
      code: old
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: example, root: lib.rs, kind: library }] }
signatures:
  - answer:
      file: lib.rs
      signature_type: function
      name: answer
      sketch: answer_body
      opaque_extension: *shared
"#;
            let entry_anchored = body_anchored.replacen(
                "  - answer_body: &shared",
                "  - &shared\n    answer_body:",
                1,
            );
            let sequence_anchored = body_anchored.replacen(
                "sketches:\n  - answer_body: &shared",
                "sketches: &shared\n  - answer_body:",
                1,
            );
            let document_anchored = r#"--- &shared {contract_version: 2, root: ../src, files: [lib.rs], extraction: {mode: rust_syntax_v2, profile: rust_api_v1, crates: [{id: example, root: lib.rs, kind: library}]}, signatures: [{answer: {file: lib.rs, signature_type: function, name: answer, sketch: answer_body}}], sketches: [{answer_body: {file: lib.rs, signature: answer, signature_type: function, matching: {normalization: exact_lines_v1, occurrence: at_least_one}, code: old}}]}
"#
            .to_owned();

            [
                body_anchored.to_owned(),
                entry_anchored,
                sequence_anchored,
                document_anchored,
            ]
        }

        fn linked(contract_file: &str, sketch_id: &str, file: &str, code: &str) -> SketchSeed {
            Self::linked_in(contract_file, 0, sketch_id, file, code)
        }

        fn linked_in(
            contract_file: &str,
            document_index: usize,
            sketch_id: &str,
            file: &str,
            code: &str,
        ) -> SketchSeed {
            SketchSeed {
                contract_file: CatalogPath::new(contract_file).expect("contract path"),
                document_index,
                sketch_id: sketch_id.to_owned(),
                signature_type: "function".to_owned(),
                file: CatalogPath::new(file).expect("source path"),
                code: code.to_owned(),
            }
        }

        fn one_sketch_document(signature: &str, sketch: &str, file: &str, code: &str) -> String {
            format!(
                "contract_version: 2\nroot: ../src\nfiles: [{file}]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: example, root: {file}, kind: library }}] }}\nsignatures:\n  - {signature}:\n      file: {file}\n      signature_type: function\n      sketch: {sketch}\nsketches:\n  - {sketch}:\n      file: {file}\n      signature: {signature}\n      signature_type: function\n      matching: {{ normalization: exact_lines_v1, occurrence: at_least_one }}\n      code: \"{code}\"\n"
            )
        }

        fn two_sketch_document() -> String {
            "contract_version: 2\nroot: ../src\nfiles: [a.rs, b.rs]\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: example, root: a.rs, kind: library }] }\nsignatures:\n  - alpha:\n      file: a.rs\n      signature_type: function\n      sketch: alpha_body\n  - beta:\n      file: b.rs\n      signature_type: function\n      sketch: beta_body\nsketches:\n  - alpha_body:\n      file: a.rs\n      signature: alpha\n      signature_type: function\n      matching: { normalization: exact_lines_v1, occurrence: at_least_one }\n      code: \"let alpha = 1;\"\n  - beta_body:\n      file: b.rs\n      signature: beta\n      signature_type: function\n      matching: { normalization: exact_lines_v1, occurrence: at_least_one }\n      code: \"let beta = 2;\"\n"
                .to_owned()
        }
    }

    #[test]
    fn refresh_replaces_only_linked_sketch_code_in_combined_document() {
        let response = (GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", LINKED_CONTRACT)
                .into_catalog(),
            seeds: vec![SeedFixture::answer()],
            mode: GenerateMode::FullRefresh,
        })
        .run(&SketchLimits::default(), &CancellationProbe::new())
        .expect("refresh");
        let output_path = CatalogPath::new("main.yml").expect("path");
        let generated = response
            .contract_files
            .get(&output_path)
            .expect("updated yaml");
        let generated_text = std::str::from_utf8(generated).expect("utf8 yaml");

        assert_eq!(
            response.counts,
            SketchGenerationCounts {
                linked_sketch_count: 1,
                refreshed_sketch_count: 1,
                changed_sketch_count: 1,
                changed_document_count: 1,
            }
        );
        assert!(generated_text.contains("root: ../src"));
        assert!(generated_text.contains("answer:"));
        assert!(generated_text.contains("sketch: answer_body"));
        assert!(generated_text.contains("- answer_body:"));
        assert!(generated_text.contains("signature_type: function"));
        assert!(generated_text.contains("pub fn answer() -> u8 { 42 }"));
        assert!(!generated_text.contains("old code"));

        let limits = SketchLimits::default();
        let mut yaml_budget = limits.yaml_budget();
        let contracts = SketchContracts::from_catalog(
            response.contract_files,
            &limits,
            &mut yaml_budget,
            &CancellationProbe::new(),
        )
        .expect("updated combined document parses");
        assert_eq!(contracts.len(), 1);
    }

    #[test]
    fn changed_anchored_and_aliased_code_fail_closed_during_full_refresh() {
        let anchored = SeedFixture::linked_scalar("&code 'old'");
        let ancestor_anchored = SeedFixture::ancestor_anchored_documents();
        let ancestor_anchored_alias = ancestor_anchored[0]
            .replacen("root: ../src", "root: &scalar old", 1)
            .replacen("code: old", "code: *scalar", 1);
        let aliased =
            SeedFixture::linked_scalar("*shared").replacen("root: ../src", "root: &shared old", 1);
        let anchored_error = "cannot refresh anchored code for sketch answer_body in main.yml document 0; anchor dependents cannot be proven absent";
        let cases = [
            (&anchored, anchored_error),
            (&ancestor_anchored[0], anchored_error),
            (&ancestor_anchored[1], anchored_error),
            (&ancestor_anchored[2], anchored_error),
            (&ancestor_anchored[3], anchored_error),
            (&ancestor_anchored_alias, anchored_error),
            (
                &aliased,
                "cannot refresh aliased code for sketch answer_body in main.yml document 0; alias target mutation is not provably local",
            ),
        ];

        for (document, expected) in cases {
            let error = (GenerateRequest {
                contract_files: TestCatalog::new()
                    .with_file("main.yml", document)
                    .into_catalog(),
                seeds: vec![SeedFixture::linked(
                    "main.yml",
                    "answer_body",
                    "lib.rs",
                    "changed",
                )],
                mode: GenerateMode::FullRefresh,
            })
            .run(&SketchLimits::default(), &CancellationProbe::new())
            .expect_err("unsafe scalar mutation must fail closed");

            assert_eq!(error.to_string(), expected);
        }
    }

    #[test]
    fn unchanged_anchored_and_aliased_code_use_the_byte_exact_noop_path() {
        let anchored = SeedFixture::linked_scalar("&code 'old'");
        let ancestor_anchored = SeedFixture::ancestor_anchored_documents();
        let ancestor_anchored_alias = ancestor_anchored[0]
            .replacen("root: ../src", "root: &scalar old", 1)
            .replacen("code: old", "code: *scalar", 1);
        let aliased =
            SeedFixture::linked_scalar("*shared").replacen("root: ../src", "root: &shared old", 1);
        let limits = SketchLimits {
            output: crate::limits::OutputLimits {
                scratch_bytes: 0,
                ..crate::limits::OutputLimits::default()
            },
            ..SketchLimits::default()
        };
        let path = CatalogPath::new("main.yml").expect("path");

        for document in [
            &anchored,
            &ancestor_anchored[0],
            &ancestor_anchored[1],
            &ancestor_anchored[2],
            &ancestor_anchored[3],
            &ancestor_anchored_alias,
            &aliased,
        ] {
            let response = (GenerateRequest {
                contract_files: TestCatalog::new()
                    .with_file("main.yml", document)
                    .into_catalog(),
                seeds: vec![SeedFixture::linked(
                    "main.yml",
                    "answer_body",
                    "lib.rs",
                    "old",
                )],
                mode: GenerateMode::FullRefresh,
            })
            .run(&limits, &CancellationProbe::new())
            .expect("unchanged unsafe scalar must use the no-op path");

            assert_eq!(
                response.counts,
                SketchGenerationCounts {
                    linked_sketch_count: 1,
                    refreshed_sketch_count: 1,
                    changed_sketch_count: 0,
                    changed_document_count: 0,
                }
            );
            assert_eq!(
                response.contract_files.get(&path),
                Some(document.as_bytes())
            );
        }
    }

    #[test]
    fn refresh_preserves_opaque_yaml_and_quoted_fallback_exactly() {
        let extraction = r#"extraction:
  mode: compiler_v99 # signature-owned comment
  profile: future_api
  crates:
    - id: future
      root: &signature_file lib.rs
      kind: library
  extension: !future { enabled: true }
"#;
        let signature = r#"signatures:
  - answer:
      file: *signature_file
      signature_type: function
      name: answer
      sketch: answer_body
"#;
        let document = format!(
            "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\n{extraction}{signature}sketches:\n  - answer_body:\n      file: lib.rs\n      signature: answer\n      signature_type: function\n      matching: {{ normalization: exact_lines_v1, occurrence: at_least_one }}\n      code: old code\n"
        );
        let response = (GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", &document)
                .into_catalog(),
            seeds: vec![SeedFixture::answer()],
            mode: GenerateMode::FullRefresh,
        })
        .run(&SketchLimits::default(), &CancellationProbe::new())
        .expect("refresh with opaque signature-owned YAML");
        let generated = response
            .contract_files
            .get(&CatalogPath::new("main.yml").expect("path"))
            .expect("updated YAML");
        let generated = std::str::from_utf8(generated).expect("UTF-8 YAML");

        let expected = document.replacen(
            "code: old code",
            "code: \"pub fn answer() -> u8 { 42 }\"",
            1,
        );
        assert_eq!(generated, expected);
    }

    #[test]
    fn refresh_preserves_compact_indentless_yaml_with_quoted_fallback() {
        let document = "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: example, root: lib.rs, kind: library }] }\nsignatures:\n- answer:\n    file: lib.rs\n    signature_type: function\n    sketch: answer_body\nsketches:\n- answer_body:\n    file: lib.rs\n    signature: answer\n    signature_type: function\n    matching: { normalization: exact_lines_v1, occurrence: at_least_one }\n    code: old code\n";
        let response = (GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", document)
                .into_catalog(),
            seeds: vec![SeedFixture::answer()],
            mode: GenerateMode::FullRefresh,
        })
        .run(&SketchLimits::default(), &CancellationProbe::new())
        .expect("refresh compact indentless sequences");
        let generated = response
            .contract_files
            .get(&CatalogPath::new("main.yml").expect("path"))
            .expect("updated YAML");
        let generated = std::str::from_utf8(generated).expect("UTF-8 YAML");

        let expected = document.replacen(
            "code: old code",
            "code: \"pub fn answer() -> u8 { 42 }\"",
            1,
        );
        assert_eq!(generated, expected);
    }

    #[test]
    fn documents_without_links_and_non_yaml_entries_remain_byte_exact() {
        let document =
            "contract_version: 2\nroot: ../src\nfiles: []\nsignatures: []\nsketches: []\n";
        let nested = "this is deliberately not a combined contract\n";
        let response = (GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", document)
                .with_file("nested/ignored.yml", nested)
                .with_file("contracts/notes.txt", "user notes\n")
                .into_catalog(),
            seeds: Vec::new(),
            mode: GenerateMode::FullRefresh,
        })
        .run(&SketchLimits::default(), &CancellationProbe::new())
        .expect("no-op refresh");

        assert_eq!(
            response.counts,
            SketchGenerationCounts {
                linked_sketch_count: 0,
                refreshed_sketch_count: 0,
                changed_sketch_count: 0,
                changed_document_count: 0,
            }
        );
        assert_eq!(
            response
                .contract_files
                .get(&CatalogPath::new("main.yml").expect("path")),
            Some(document.as_bytes())
        );
        assert_eq!(
            response
                .contract_files
                .get(&CatalogPath::new("nested/ignored.yml").expect("path")),
            Some(nested.as_bytes())
        );
        assert_eq!(
            response
                .contract_files
                .get(&CatalogPath::new("contracts/notes.txt").expect("path")),
            Some(&b"user notes\n"[..])
        );
    }

    #[test]
    fn targeted_refresh_preserves_other_root_documents_verbatim() {
        let untouched = "# preserve this comment\ncontract_version: 2\nroot: ../src\nfiles: [other.rs]\nsignatures: []\nsketches: []\n";
        let response = (GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", LINKED_CONTRACT)
                .with_file("untouched.YAML", untouched)
                .into_catalog(),
            seeds: vec![SeedFixture::answer()],
            mode: GenerateMode::FullRefresh,
        })
        .run(&SketchLimits::default(), &CancellationProbe::new())
        .expect("targeted refresh");

        assert_eq!(response.counts.linked_sketch_count, 1);
        assert_eq!(response.counts.refreshed_sketch_count, 1);
        assert_eq!(
            response
                .contract_files
                .get(&CatalogPath::new("untouched.YAML").expect("path")),
            Some(untouched.as_bytes())
        );
    }

    #[test]
    fn repeated_refresh_produces_identical_catalog_bytes() {
        let seed = SeedFixture::answer();
        let first = (GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", LINKED_CONTRACT)
                .into_catalog(),
            seeds: vec![seed.clone()],
            mode: GenerateMode::FullRefresh,
        })
        .run(&SketchLimits::default(), &CancellationProbe::new())
        .expect("first refresh");
        let second = (GenerateRequest {
            contract_files: first.contract_files.clone(),
            seeds: vec![seed],
            mode: GenerateMode::FullRefresh,
        })
        .run(&SketchLimits::default(), &CancellationProbe::new())
        .expect("second refresh");

        assert_eq!(first.counts.linked_sketch_count, 1);
        assert_eq!(first.counts.refreshed_sketch_count, 1);
        assert_eq!(first.counts.changed_sketch_count, 1);
        assert_eq!(second.counts.linked_sketch_count, 1);
        assert_eq!(second.counts.refreshed_sketch_count, 1);
        assert_eq!(second.counts.changed_sketch_count, 0);
        assert_eq!(first.contract_files, second.contract_files);
    }

    #[test]
    fn duplicate_seed_ids_are_rejected() {
        let seed = SeedFixture::answer();
        let request = GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", LINKED_CONTRACT)
                .into_catalog(),
            seeds: vec![seed.clone(), seed],
            mode: GenerateMode::FullRefresh,
        };

        let error = request
            .run(&SketchLimits::default(), &CancellationProbe::new())
            .expect_err("duplicate seed ids");

        assert!(
            error
                .to_string()
                .contains("duplicate sketch refresh seed answer_body")
        );
    }

    #[test]
    fn every_linked_sketch_requires_a_seed() {
        let request = GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", LINKED_CONTRACT)
                .into_catalog(),
            seeds: Vec::new(),
            mode: GenerateMode::FullRefresh,
        };

        let error = request
            .run(&SketchLimits::default(), &CancellationProbe::new())
            .expect_err("missing seed");

        assert!(
            error
                .to_string()
                .contains("linked sketch answer_body is missing a refresh seed")
        );
    }

    #[test]
    fn seed_for_unknown_sketch_is_rejected() {
        let mut seed = SeedFixture::answer();
        seed.sketch_id = "other_body".to_owned();
        let request = GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", LINKED_CONTRACT)
                .into_catalog(),
            seeds: vec![seed],
            mode: GenerateMode::FullRefresh,
        };

        let error = request
            .run(&SketchLimits::default(), &CancellationProbe::new())
            .expect_err("unknown seed");

        assert!(
            error
                .to_string()
                .contains("has no explicitly linked sketch")
        );
    }

    #[test]
    fn seed_contract_file_must_match_linked_document() {
        let mut seed = SeedFixture::answer();
        seed.contract_file = CatalogPath::new("other.yml").expect("path");
        let request = GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", LINKED_CONTRACT)
                .into_catalog(),
            seeds: vec![seed],
            mode: GenerateMode::FullRefresh,
        };

        let error = request
            .run(&SketchLimits::default(), &CancellationProbe::new())
            .expect_err("wrong document");

        assert!(error.to_string().contains("targets contract document"));
        assert!(error.to_string().contains("expected main.yml"));
    }

    #[test]
    fn seed_source_file_must_match_linked_signature() {
        let mut seed = SeedFixture::answer();
        seed.file = CatalogPath::new("other.rs").expect("path");
        let request = GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", LINKED_CONTRACT)
                .into_catalog(),
            seeds: vec![seed],
            mode: GenerateMode::FullRefresh,
        };

        let error = request
            .run(&SketchLimits::default(), &CancellationProbe::new())
            .expect_err("wrong source");

        assert!(error.to_string().contains("targets source file other.rs"));
        assert!(error.to_string().contains("expected lib.rs"));
    }

    #[test]
    fn seed_signature_type_must_match_linked_signature() {
        let mut seed = SeedFixture::answer();
        seed.signature_type = "method".to_owned();
        let request = GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", LINKED_CONTRACT)
                .into_catalog(),
            seeds: vec![seed],
            mode: GenerateMode::FullRefresh,
        };

        let error = request
            .run(&SketchLimits::default(), &CancellationProbe::new())
            .expect_err("wrong signature type");

        assert!(error.to_string().contains("signature_type method"));
        assert!(error.to_string().contains("expected function"));
    }

    #[test]
    fn seed_signature_type_must_not_be_empty() {
        let mut seed = SeedFixture::answer();
        seed.signature_type = "  \t".to_owned();
        let request = GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", LINKED_CONTRACT)
                .into_catalog(),
            seeds: vec![seed],
            mode: GenerateMode::FullRefresh,
        };

        let error = request
            .run(&SketchLimits::default(), &CancellationProbe::new())
            .expect_err("empty signature type");

        assert!(
            error
                .to_string()
                .contains("signature_type must not be empty")
        );
    }

    #[test]
    fn empty_seed_code_is_rejected_after_normalization() {
        let mut seed = SeedFixture::answer();
        seed.code = "\n".to_owned();
        let request = GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", LINKED_CONTRACT)
                .into_catalog(),
            seeds: vec![seed],
            mode: GenerateMode::FullRefresh,
        };

        let error = request
            .run(&SketchLimits::default(), &CancellationProbe::new())
            .expect_err("empty code");

        assert!(error.to_string().contains("code must not be empty"));
    }

    #[test]
    fn two_changed_sketches_in_one_document_count_one_changed_document() {
        let response = (GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", &SeedFixture::two_sketch_document())
                .into_catalog(),
            seeds: vec![
                SeedFixture::linked("main.yml", "alpha_body", "a.rs", "let alpha = 3;"),
                SeedFixture::linked("main.yml", "beta_body", "b.rs", "let beta = 4;"),
            ],
            mode: GenerateMode::FullRefresh,
        })
        .run(&SketchLimits::default(), &CancellationProbe::new())
        .expect("refresh two sketches in one document");

        assert_eq!(
            response.counts,
            SketchGenerationCounts {
                linked_sketch_count: 2,
                refreshed_sketch_count: 2,
                changed_sketch_count: 2,
                changed_document_count: 1,
            }
        );
    }

    #[test]
    fn changed_documents_with_the_same_index_in_different_files_are_counted_separately() {
        let response = (GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file(
                    "alpha.yml",
                    &SeedFixture::one_sketch_document(
                        "alpha",
                        "alpha_body",
                        "a.rs",
                        "let alpha = 1;",
                    ),
                )
                .with_file(
                    "beta.yml",
                    &SeedFixture::one_sketch_document("beta", "beta_body", "b.rs", "let beta = 2;"),
                )
                .into_catalog(),
            seeds: vec![
                SeedFixture::linked("alpha.yml", "alpha_body", "a.rs", "let alpha = 3;"),
                SeedFixture::linked("beta.yml", "beta_body", "b.rs", "let beta = 4;"),
            ],
            mode: GenerateMode::FullRefresh,
        })
        .run(&SketchLimits::default(), &CancellationProbe::new())
        .expect("refresh sketches in two physical documents");

        assert_eq!(
            response.counts,
            SketchGenerationCounts {
                linked_sketch_count: 2,
                refreshed_sketch_count: 2,
                changed_sketch_count: 2,
                changed_document_count: 2,
            }
        );
    }

    #[test]
    fn two_changed_documents_in_one_yaml_stream_are_counted_separately() {
        let stream = format!(
            "{}---\n{}",
            SeedFixture::one_sketch_document("alpha", "alpha_body", "a.rs", "let alpha = 1;",),
            SeedFixture::one_sketch_document("beta", "beta_body", "b.rs", "let beta = 2;",)
        );
        let response = (GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", &stream)
                .into_catalog(),
            seeds: vec![
                SeedFixture::linked_in("main.yml", 0, "alpha_body", "a.rs", "let alpha = 3;"),
                SeedFixture::linked_in("main.yml", 1, "beta_body", "b.rs", "let beta = 4;"),
            ],
            mode: GenerateMode::FullRefresh,
        })
        .run(&SketchLimits::default(), &CancellationProbe::new())
        .expect("refresh two documents in one stream");

        assert_eq!(
            response.counts,
            SketchGenerationCounts {
                linked_sketch_count: 2,
                refreshed_sketch_count: 2,
                changed_sketch_count: 2,
                changed_document_count: 2,
            }
        );
    }

    #[test]
    fn seed_signature_type_must_match_exactly_without_whitespace_normalization() {
        let mut seed = SeedFixture::answer();
        seed.signature_type = " function ".to_owned();
        let error = (GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", LINKED_CONTRACT)
                .into_catalog(),
            seeds: vec![seed],
            mode: GenerateMode::FullRefresh,
        })
        .run(&SketchLimits::default(), &CancellationProbe::new())
        .expect_err("surrounding whitespace must not match the signature type");

        assert!(error.to_string().contains("signature_type  function "));
        assert!(error.to_string().contains("expected function"));
    }

    #[test]
    fn generated_contract_catalog_respects_the_output_budget() {
        let mut limits = SketchLimits::default();
        limits.output.generated_bytes = 1;
        let error = (GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", LINKED_CONTRACT)
                .into_catalog(),
            seeds: vec![SeedFixture::answer()],
            mode: GenerateMode::FullRefresh,
        })
        .run(&limits, &CancellationProbe::new())
        .expect_err("generated catalog must exceed one byte");

        assert_eq!(
            error.limit_exceeded().expect("typed output limit").resource,
            LimitResource::GeneratedOutputBytes
        );
    }
}
