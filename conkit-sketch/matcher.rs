use crate::contract::{SketchContract, SketchContracts};
use crate::error::SketchContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::inventory::{SketchDiagnostic, SketchInventoryComparison};
use crate::normalize::NormalizedSnippet;
use rayon::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct SourceCatalog {
    files: BTreeMap<CatalogPath, NormalizedSnippet>,
    source_file_count: usize,
}

impl SourceCatalog {
    pub(crate) fn from_catalog(catalog: FileCatalog, contracts: &SketchContracts) -> Self {
        let source_file_count = catalog.len();
        let selection = SourceSelection::from_contracts(contracts);
        let files = catalog
            .into_entries()
            .filter(|(path, _)| selection.contains(path))
            .map(|(path, bytes)| (path, NormalizedSnippet::from_bytes(&bytes)))
            .collect();

        Self {
            files,
            source_file_count,
        }
    }

    pub(crate) fn count(&self) -> usize {
        self.source_file_count
    }

    fn check_contract(&self, contract: &SketchContract) -> Option<SketchDiagnostic> {
        let Some(source) = self.files.get(contract.file()) else {
            return Some(SketchDiagnostic::missing_file(
                contract.id(),
                contract.file(),
            ));
        };

        let expected = contract.snippet().normalized();
        if expected.is_empty() {
            return Some(SketchDiagnostic::empty_snippet(contract.id()));
        }

        if expected.occurs_in(source) {
            None
        } else {
            Some(SketchDiagnostic::not_matched(
                contract.id(),
                contract.file(),
            ))
        }
    }
}

struct SourceSelection {
    paths: BTreeSet<CatalogPath>,
}

impl SourceSelection {
    fn from_contracts(contracts: &SketchContracts) -> Self {
        let paths = contracts
            .entries()
            .iter()
            .map(|contract| contract.file().clone())
            .collect();

        Self { paths }
    }

    fn contains(&self, path: &CatalogPath) -> bool {
        self.paths.contains(path)
    }
}

pub(crate) struct SketchMatcher {
    sources: SourceCatalog,
    contracts: SketchContracts,
}

impl SketchMatcher {
    pub(crate) fn new(sources: SourceCatalog, contracts: SketchContracts) -> Self {
        Self { sources, contracts }
    }

    pub(crate) fn check(self) -> Result<SketchInventoryComparison, SketchContractKitError> {
        let source_file_count = self.sources.count();
        let contract_file_count = self.contracts.contract_file_count();
        let sketch_count = self.contracts.len();
        let diagnostics = self
            .contracts
            .entries()
            .par_iter()
            .filter_map(|contract| self.sources.check_contract(contract))
            .collect::<Vec<_>>();

        Ok(SketchInventoryComparison::new(
            source_file_count,
            contract_file_count,
            sketch_count,
            diagnostics,
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::{SketchMatcher, SourceCatalog};
    use crate::contract::SketchContracts;
    use crate::files::{CatalogPath, FileCatalog};
    use crate::inventory::SketchDiagnostic;

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

        fn with_bytes(mut self, path: &str, contents: Vec<u8>) -> Self {
            self.catalog
                .insert(CatalogPath::new(path).expect("test path"), contents)
                .expect("insert test file");
            self
        }

        fn into_catalog(self) -> FileCatalog {
            self.catalog
        }
    }

    struct TestCheck {
        sources: FileCatalog,
        contracts: FileCatalog,
    }

    impl TestCheck {
        fn new(sources: FileCatalog, contracts: FileCatalog) -> Self {
            Self { sources, contracts }
        }

        fn run(self) -> Vec<SketchDiagnostic> {
            let contracts = SketchContracts::from_catalog(self.contracts).expect("contracts");
            let sources = SourceCatalog::from_catalog(self.sources, &contracts);
            let comparison = SketchMatcher::new(sources, contracts)
                .check()
                .expect("check");

            comparison.diagnostics().to_vec()
        }
    }

    struct TestContract;

    impl TestContract {
        fn single(id: &str, file: &str, code: &str) -> String {
            Self::many(&[(id, file, code)])
        }

        fn many(sketches: &[(&str, &str, &str)]) -> String {
            let files = sketches
                .iter()
                .map(|(_, file, _)| *file)
                .collect::<std::collections::BTreeSet<_>>();
            let mut yaml = "root: .\nfiles:\n".to_owned();
            for file in files {
                yaml.push_str(&format!("  - {file}\n"));
            }
            yaml.push_str("signatures:\n");
            for (id, file, _) in sketches {
                yaml.push_str(&format!(
                    "  - {id}_signature:\n      file: {file}\n      signature_type: function\n      sketch: {id}\n"
                ));
            }
            yaml.push_str("sketches:\n");
            for (id, _, code) in sketches {
                yaml.push_str(&format!(
                    "  - {id}:\n    signature_type: function\n    code: |\n"
                ));
                for line in code.lines() {
                    yaml.push_str("      ");
                    yaml.push_str(line);
                    yaml.push('\n');
                }
            }
            yaml
        }
    }

    #[test]
    fn exact_match_passes_without_diagnostics() {
        let sources = TestCatalog::new()
            .with_file("src/lib.rs", "fn answer() -> u8 {\n    42\n}\n")
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single("answer", "src/lib.rs", "fn answer() -> u8 {\n    42\n}"),
            )
            .into_catalog();

        assert!(TestCheck::new(sources, contracts).run().is_empty());
    }

    #[test]
    fn unrelated_binary_source_file_does_not_fail_matching_sketch() {
        let sources = TestCatalog::new()
            .with_file("src/lib.rs", "fn answer() -> u8 {\n    42\n}\n")
            .with_bytes("assets/blob.bin", vec![0, 159, 255])
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single("answer", "src/lib.rs", "fn answer() -> u8 {\n    42\n}"),
            )
            .into_catalog();

        assert!(TestCheck::new(sources, contracts).run().is_empty());
    }

    #[test]
    fn whitespace_insensitive_match_passes() {
        let sources = TestCatalog::new()
            .with_file("src/lib.rs", "\tfn   answer() -> u8 {\n\t\t42\n\t}\n")
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single("answer", "src/lib.rs", "fn answer() -> u8 {\n    42\n}"),
            )
            .into_catalog();

        assert!(TestCheck::new(sources, contracts).run().is_empty());
    }

    #[test]
    fn token_mismatch_emits_not_matched() {
        let sources = TestCatalog::new()
            .with_file("src/lib.rs", "fn answer() -> u8 {\n    41\n}\n")
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single("answer", "src/lib.rs", "fn answer() -> u8 {\n    42\n}"),
            )
            .into_catalog();

        assert_eq!(
            TestCheck::new(sources, contracts).run(),
            vec![SketchDiagnostic::NotMatched {
                sketch_id: "answer".to_owned(),
                file: "src/lib.rs".to_owned(),
            }]
        );
    }

    #[test]
    fn missing_line_mismatch_emits_not_matched() {
        let sources = TestCatalog::new()
            .with_file("src/lib.rs", "fn answer() -> u8 {\n}\n")
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single("answer", "src/lib.rs", "fn answer() -> u8 {\n    42\n}"),
            )
            .into_catalog();

        assert_eq!(
            TestCheck::new(sources, contracts).run(),
            vec![SketchDiagnostic::NotMatched {
                sketch_id: "answer".to_owned(),
                file: "src/lib.rs".to_owned(),
            }]
        );
    }

    #[test]
    fn reordered_lines_emit_not_matched() {
        let sources = TestCatalog::new()
            .with_file("src/lib.rs", "let second = 2;\nlet first = 1;\n")
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single("order", "src/lib.rs", "let first = 1;\nlet second = 2;"),
            )
            .into_catalog();

        assert_eq!(
            TestCheck::new(sources, contracts).run(),
            vec![SketchDiagnostic::NotMatched {
                sketch_id: "order".to_owned(),
                file: "src/lib.rs".to_owned(),
            }]
        );
    }

    #[test]
    fn missing_source_file_emits_missing_file() {
        let sources = TestCatalog::new().into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single("missing", "src/missing.rs", "let value = 42;"),
            )
            .into_catalog();

        assert_eq!(
            TestCheck::new(sources, contracts).run(),
            vec![SketchDiagnostic::MissingFile {
                sketch_id: "missing".to_owned(),
                file: "src/missing.rs".to_owned(),
            }]
        );
    }

    #[test]
    fn multiple_sketches_in_multiple_files_can_pass() {
        let sources = TestCatalog::new()
            .with_file("src/a.rs", "fn a() -> u8 { 1 }\n")
            .with_file("src/b.rs", "fn b() -> u8 { 2 }\n")
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::many(&[
                    ("a", "src/a.rs", "fn a() -> u8 { 1 }"),
                    ("b", "src/b.rs", "fn b() -> u8 { 2 }"),
                ]),
            )
            .into_catalog();

        assert!(TestCheck::new(sources, contracts).run().is_empty());
    }

    #[test]
    fn diagnostics_are_sorted_after_parallel_matching() {
        let sources = TestCatalog::new()
            .with_file("src/a.rs", "let value = 0;\n")
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::many(&[
                    ("zeta", "src/z.rs", "let z = 1;"),
                    ("alpha", "src/a.rs", "let value = 1;"),
                ]),
            )
            .into_catalog();

        assert_eq!(
            TestCheck::new(sources, contracts).run(),
            vec![
                SketchDiagnostic::NotMatched {
                    sketch_id: "alpha".to_owned(),
                    file: "src/a.rs".to_owned(),
                },
                SketchDiagnostic::MissingFile {
                    sketch_id: "zeta".to_owned(),
                    file: "src/z.rs".to_owned(),
                },
            ]
        );
    }
}
