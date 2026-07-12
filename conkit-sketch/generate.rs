use crate::api::{GenerateRequest, GenerateResponse, SketchSeed};
use crate::contract::{SketchContractDocuments, SketchContracts};
use crate::error::SketchContractKitError;
use crate::files::CatalogPath;
use crate::id::SketchId;
use crate::normalize::NormalizedSnippet;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct SketchGenerator {
    request: GenerateRequest,
}

impl SketchGenerator {
    pub(crate) fn new(request: GenerateRequest) -> Self {
        Self { request }
    }

    pub(crate) fn generate(self) -> Result<GenerateResponse, SketchContractKitError> {
        let GenerateRequest {
            contract_files,
            seeds,
        } = self.request;
        let documents = SketchContractDocuments::from_catalog(contract_files)?;
        let contracts = documents.contracts()?;
        let seeds = SketchRefreshSeeds::from_input(seeds, &contracts)?;

        documents.refresh(seeds)
    }
}

pub(crate) struct SketchRefreshSeeds {
    entries: BTreeMap<SketchId, GeneratedSketchCode>,
    documents: BTreeSet<CatalogPath>,
}

impl SketchRefreshSeeds {
    fn from_input(
        seeds: Vec<SketchSeed>,
        contracts: &SketchContracts,
    ) -> Result<Self, SketchContractKitError> {
        let contracts_by_id = contracts
            .entries()
            .iter()
            .map(|contract| (contract.id(), contract))
            .collect::<BTreeMap<_, _>>();
        let mut entries = BTreeMap::new();
        let mut documents = BTreeSet::new();

        for seed in seeds {
            let id = SketchId::from_seed(seed.sketch_id.as_str())?;
            if entries.contains_key(&id) {
                return Err(SketchContractKitError::conversion_failed(format!(
                    "duplicate sketch refresh seed {}",
                    id.as_str()
                )));
            }
            let Some(contract) = contracts_by_id.get(&id) else {
                return Err(SketchContractKitError::conversion_failed(format!(
                    "sketch refresh seed {} has no explicitly linked sketch",
                    id.as_str()
                )));
            };
            contract.validate_seed(&seed, &id)?;
            let code = GeneratedSketchCode::new(seed.code, id.as_str())?;

            documents.insert(seed.contract_file);
            entries.insert(id, code);
        }

        for contract in contracts.entries() {
            if !entries.contains_key(contract.id()) {
                return Err(SketchContractKitError::conversion_failed(format!(
                    "linked sketch {} is missing a refresh seed",
                    contract.id().as_str()
                )));
            }
        }

        Ok(Self { entries, documents })
    }

    pub(crate) fn contains_document(&self, path: &CatalogPath) -> bool {
        self.documents.contains(path)
    }

    pub(crate) fn code_for(&self, id: &SketchId) -> Option<&str> {
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
    fn new(value: impl Into<String>, sketch_id: &str) -> Result<Self, SketchContractKitError> {
        let value = value.into();
        if NormalizedSnippet::from_code(&value).is_empty() {
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
    use super::SketchGenerator;
    use crate::api::{GenerateRequest, SketchSeed};
    use crate::contract::SketchContracts;
    use crate::files::{CatalogPath, FileCatalog};

    const LINKED_CONTRACT: &str = r#"root: ../src
files: [lib.rs]
signatures:
  - answer:
      file: lib.rs
      signature_type: function
      name: answer
      sketch: answer_body
sketches:
  - answer_body:
    signature_type: function
    code: old code
"#;

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
                sketch_id: "answer_body".to_owned(),
                signature_type: "function".to_owned(),
                file: CatalogPath::new("lib.rs").expect("source path"),
                code: "pub fn answer() -> u8 { 42 }".to_owned(),
            }
        }
    }

    #[test]
    fn refresh_replaces_only_linked_sketch_code_in_combined_document() {
        let response = SketchGenerator::new(GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", LINKED_CONTRACT)
                .into_catalog(),
            seeds: vec![SeedFixture::answer()],
        })
        .generate()
        .expect("refresh");
        let output_path = CatalogPath::new("main.yml").expect("path");
        let generated = response
            .contract_files
            .get(&output_path)
            .expect("updated yaml");
        let generated_text = std::str::from_utf8(generated).expect("utf8 yaml");

        assert_eq!(response.sketch_count, 1);
        assert!(generated_text.contains("root: ../src"));
        assert!(generated_text.contains("answer:"));
        assert!(generated_text.contains("sketch: answer_body"));
        assert!(generated_text.contains("answer_body: null"));
        assert!(generated_text.contains("signature_type: function"));
        assert!(generated_text.contains("pub fn answer() -> u8 { 42 }"));
        assert!(!generated_text.contains("old code"));

        let contracts = SketchContracts::from_catalog(response.contract_files)
            .expect("updated combined document parses");
        assert_eq!(contracts.len(), 1);
    }

    #[test]
    fn documents_without_links_and_non_yaml_entries_remain_byte_exact() {
        let document = "root: ../src\nfiles: []\nsignatures: []\nsketches: []\n";
        let nested = "this is deliberately not a combined contract\n";
        let response = SketchGenerator::new(GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", document)
                .with_file("nested/ignored.yml", nested)
                .with_file("contracts/notes.txt", "user notes\n")
                .into_catalog(),
            seeds: Vec::new(),
        })
        .generate()
        .expect("no-op refresh");

        assert_eq!(response.sketch_count, 0);
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
        let untouched = "# preserve this comment\nroot: ../src\nfiles: [other.rs]\nsignatures: []\nsketches: []\n";
        let response = SketchGenerator::new(GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", LINKED_CONTRACT)
                .with_file("untouched.YAML", untouched)
                .into_catalog(),
            seeds: vec![SeedFixture::answer()],
        })
        .generate()
        .expect("targeted refresh");

        assert_eq!(response.sketch_count, 1);
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
        let first = SketchGenerator::new(GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", LINKED_CONTRACT)
                .into_catalog(),
            seeds: vec![seed.clone()],
        })
        .generate()
        .expect("first refresh");
        let second = SketchGenerator::new(GenerateRequest {
            contract_files: first.contract_files.clone(),
            seeds: vec![seed],
        })
        .generate()
        .expect("second refresh");

        assert_eq!(first.sketch_count, second.sketch_count);
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
        };

        let error = SketchGenerator::new(request)
            .generate()
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
        };

        let error = SketchGenerator::new(request)
            .generate()
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
        };

        let error = SketchGenerator::new(request)
            .generate()
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
        };

        let error = SketchGenerator::new(request)
            .generate()
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
        };

        let error = SketchGenerator::new(request)
            .generate()
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
        };

        let error = SketchGenerator::new(request)
            .generate()
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
        };

        let error = SketchGenerator::new(request)
            .generate()
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
        seed.code = "  \n\t".to_owned();
        let request = GenerateRequest {
            contract_files: TestCatalog::new()
                .with_file("main.yml", LINKED_CONTRACT)
                .into_catalog(),
            seeds: vec![seed],
        };

        let error = SketchGenerator::new(request)
            .generate()
            .expect_err("empty code");

        assert!(error.to_string().contains("code must not be empty"));
    }
}
