use conkit_sketch::{
    CatalogPath, CheckMode, CheckRequest, DiffEntry, DiffRequest, FileCatalog, GenerateRequest,
    ReportFormat, ReportRequest, SketchContractKit, SketchContractKitBuilder, SketchDiagnostic,
    SketchSeed, WorkOptions, WorkParallelism,
};
use std::future::Future;
use std::num::NonZeroUsize;
use std::sync::Arc;

mod spawn_compatibility {
    use super::*;

    struct SpawnContract;

    impl SpawnContract {
        fn assert_send<F>(future: F)
        where
            F: Future + Send,
        {
            drop(future);
        }

        fn assert_spawn_compatible<F>(future: F)
        where
            F: Future + Send + 'static,
            F::Output: Send + 'static,
        {
            drop(future);
        }

        fn assert_send_static<T: Send + 'static>() {}

        fn assert_send_sync_static<T: Send + Sync + 'static>() {}

        fn check_request() -> CheckRequest {
            CheckRequest {
                source_files: FileCatalog::new(),
                contract_files: FileCatalog::new(),
                report: ReportRequest::None,
                mode: CheckMode::Default,
            }
        }

        fn generate_request() -> GenerateRequest {
            GenerateRequest {
                contract_files: FileCatalog::new(),
                seeds: Vec::new(),
            }
        }

        fn diff_request() -> DiffRequest {
            DiffRequest {
                current_contract_files: FileCatalog::new(),
                previous_contract_files: FileCatalog::new(),
            }
        }
    }

    #[test]
    fn public_types_satisfy_send_static_contracts() {
        SpawnContract::assert_send_static::<SketchContractKitBuilder>();
        SpawnContract::assert_send_sync_static::<SketchContractKit>();
    }

    #[test]
    fn directly_borrowed_operation_futures_are_send() {
        let kit = SketchContractKit::builder().build().expect("kit");

        SpawnContract::assert_send(kit.check(SpawnContract::check_request()));
        SpawnContract::assert_send(kit.generate(SpawnContract::generate_request()));
        SpawnContract::assert_send(kit.diff(SpawnContract::diff_request()));
    }

    #[test]
    fn owning_operation_tasks_are_spawn_compatible() {
        let kit = Arc::new(SketchContractKit::builder().build().expect("kit"));
        let task_kit = Arc::clone(&kit);
        SpawnContract::assert_spawn_compatible(async move {
            task_kit.check(SpawnContract::check_request()).await
        });

        let kit = Arc::new(SketchContractKit::builder().build().expect("kit"));
        let task_kit = Arc::clone(&kit);
        SpawnContract::assert_spawn_compatible(async move {
            task_kit.generate(SpawnContract::generate_request()).await
        });

        let kit = Arc::new(SketchContractKit::builder().build().expect("kit"));
        let task_kit = Arc::clone(&kit);
        SpawnContract::assert_spawn_compatible(async move {
            task_kit.diff(SpawnContract::diff_request()).await
        });
    }
}

mod builder_tests {
    use super::*;

    #[test]
    fn build_constructs_contract_kit() {
        SketchContractKitBuilder::default()
            .build()
            .expect("builder should construct local sketch kit");
    }

    #[test]
    fn builder_accepts_work_options() {
        let options = WorkOptions {
            parallelism: WorkParallelism::Fixed(NonZeroUsize::new(1).expect("nonzero")),
        };

        SketchContractKitBuilder::default()
            .with_work_options(options)
            .build()
            .expect("builder should construct local sketch kit");
    }
}

mod dto_tests {
    use super::*;

    #[test]
    fn public_dtos_are_constructed_from_catalogs() {
        let report_file = CatalogPath::new("reports/check.yaml").expect("path");
        let request = CheckRequest {
            source_files: CatalogFixture::new()
                .with_file("src/lib.rs", "fn answer() -> u8 { 42 }\n")
                .into_catalog(),
            contract_files: CatalogFixture::new()
                .with_file(
                    "main.yml",
                    "root: ../src\nfiles: []\nsignatures: []\nsketches: []\n",
                )
                .into_catalog(),
            report: ReportRequest::Generate {
                format: ReportFormat::Yaml,
                output_file: report_file.clone(),
            },
            mode: CheckMode::Strict,
        };
        let seed = SketchSeed {
            contract_file: CatalogPath::new("main.yml").expect("path"),
            sketch_id: "answer".to_owned(),
            signature_type: "function".to_owned(),
            file: CatalogPath::new("src/lib.rs").expect("path"),
            code: "fn answer() -> u8 { 42 }".to_owned(),
        };
        let generate = GenerateRequest {
            contract_files: request.contract_files.clone(),
            seeds: vec![seed.clone()],
        };
        let diff = DiffRequest {
            current_contract_files: request.contract_files.clone(),
            previous_contract_files: FileCatalog::new(),
        };

        assert_eq!(request.mode, CheckMode::Strict);
        assert_eq!(generate.seeds, vec![seed]);
        assert_eq!(diff.current_contract_files.len(), 1);
        match request.report {
            ReportRequest::Generate {
                format,
                output_file,
            } => {
                assert_eq!(format, ReportFormat::Yaml);
                assert_eq!(output_file, report_file);
            }
            ReportRequest::None => panic!("report request should carry output path"),
        }
    }

    #[test]
    fn check_request_json_round_trips_nonempty_catalogs() {
        let request = CheckFixture::matching().request(
            ReportRequest::Generate {
                format: ReportFormat::Json,
                output_file: CatalogPath::new("reports/check.json").expect("path"),
            },
            CheckMode::Strict,
        );

        let json = serde_json::to_string(&request).expect("serialize request");
        let round_tripped =
            serde_json::from_str::<CheckRequest>(&json).expect("deserialize request");

        assert_eq!(round_tripped, request);
    }
}

mod async_api_tests {
    use super::*;

    #[test]
    fn check_passes_for_matching_sketch() {
        let kit = SketchContractKitBuilder::default().build().expect("kit");
        let response = futures_executor::block_on(
            kit.check(CheckFixture::matching().request(ReportRequest::None, CheckMode::Default)),
        )
        .expect("check");

        assert!(response.passed);
        assert!(response.diagnostics.is_empty());
        assert!(response.report_files.is_empty());
        assert_eq!(response.counts.source_file_count, 1);
        assert_eq!(response.counts.contract_file_count, 1);
        assert_eq!(response.counts.sketch_count, 1);
        assert_eq!(response.counts.matched_sketch_count, 1);
        assert_eq!(response.counts.failed_sketch_count, 0);
    }

    #[test]
    fn check_returns_yaml_report_bytes() {
        let kit = SketchContractKitBuilder::default().build().expect("kit");
        let report_path = CatalogPath::new("reports/check.yaml").expect("path");
        let response = futures_executor::block_on(kit.check(CheckFixture::matching().request(
            ReportRequest::Generate {
                format: ReportFormat::Yaml,
                output_file: report_path.clone(),
            },
            CheckMode::Default,
        )))
        .expect("check");
        let report = std::str::from_utf8(
            response
                .report_files
                .get(&report_path)
                .expect("report file"),
        )
        .expect("yaml report utf8");

        assert!(report.contains("passed: true"));
        assert!(report.contains("sketch_count: 1"));
        assert!(report.contains("diagnostics"));
    }

    #[test]
    fn check_returns_json_report_bytes() {
        let kit = SketchContractKitBuilder::default().build().expect("kit");
        let report_path = CatalogPath::new("reports/check.json").expect("path");
        let response = futures_executor::block_on(kit.check(CheckFixture::matching().request(
            ReportRequest::Generate {
                format: ReportFormat::Json,
                output_file: report_path.clone(),
            },
            CheckMode::Default,
        )))
        .expect("check");
        let value = serde_json::from_slice::<serde_json::Value>(
            response
                .report_files
                .get(&report_path)
                .expect("report file"),
        )
        .expect("json report");

        assert_eq!(value["passed"], true);
        assert_eq!(value["counts"]["sketch_count"], 1);
        assert!(value["diagnostics"].is_array());
    }

    #[test]
    fn strict_mismatch_fails_with_not_matched_diagnostic() {
        let kit = SketchContractKitBuilder::default().build().expect("kit");
        let response = futures_executor::block_on(
            kit.check(CheckFixture::mismatched().request(ReportRequest::None, CheckMode::Strict)),
        )
        .expect("check");

        assert!(!response.passed);
        assert_eq!(
            response.diagnostics,
            vec![SketchDiagnostic::NotMatched {
                sketch_id: "answer_body".to_owned(),
                file: "src/lib.rs".to_owned(),
            }]
        );
    }

    #[test]
    fn warning_mismatch_preserves_diagnostics_but_passes() {
        let kit = SketchContractKitBuilder::default().build().expect("kit");
        let response = futures_executor::block_on(
            kit.check(CheckFixture::mismatched().request(ReportRequest::None, CheckMode::Warning)),
        )
        .expect("check");

        assert!(response.passed);
        assert_eq!(response.diagnostics.len(), 1);
    }

    #[test]
    fn public_check_ignores_unreferenced_binary_source_bytes() {
        let kit = SketchContractKitBuilder::default().build().expect("kit");
        let mut source_files = CatalogFixture::new()
            .with_file("src/lib.rs", "pub fn answer() -> u8 {\n    42\n}\n")
            .into_catalog();
        source_files
            .insert(
                CatalogPath::new("assets/blob.bin").expect("path"),
                vec![0, 159, 255],
            )
            .expect("binary fixture");

        let response = futures_executor::block_on(
            kit.check(CheckRequest {
                source_files,
                contract_files: CatalogFixture::new()
                    .with_file("main.yml", CheckFixture::matching_contract())
                    .into_catalog(),
                report: ReportRequest::None,
                mode: CheckMode::Strict,
            }),
        )
        .expect("check");

        assert!(response.passed);
        assert_eq!(response.counts.source_file_count, 2);
    }

    #[test]
    fn public_check_normalizes_unicode_whitespace() {
        let kit = SketchContractKitBuilder::default().build().expect("kit");
        let source_files = CatalogFixture::new()
            .with_file(
                "src/lib.rs",
                "pub fn answer() {\n    let\u{00a0}value = 42;\n}\n",
            )
            .into_catalog();
        let contract_files = CatalogFixture::new()
            .with_file(
                "main.yml",
                r#"
root: ../src
files: [src/lib.rs]
signatures:
  - answer_signature:
      file: src/lib.rs
      signature_type: function
      sketch: answer_body
sketches:
  - answer_body:
    signature_type: function
    code: |
      pub fn answer() {
          let value = 42;
      }
"#,
            )
            .into_catalog();

        let response = futures_executor::block_on(kit.check(CheckRequest {
            source_files,
            contract_files,
            report: ReportRequest::None,
            mode: CheckMode::Strict,
        }))
        .expect("check");

        assert!(response.passed);
        assert!(response.diagnostics.is_empty());
    }

    #[test]
    fn refreshed_linked_sketch_can_be_checked() {
        let kit = SketchContractKitBuilder::default().build().expect("kit");
        let source_files = CatalogFixture::new()
            .with_file("src/lib.rs", "pub fn answer() -> u8 { 42 }\n")
            .into_catalog();
        let contract_file = CatalogPath::new("main.yml").expect("path");
        let generated = futures_executor::block_on(
            kit.generate(GenerateRequest {
                contract_files: CatalogFixture::new()
                    .with_file("main.yml", CheckFixture::matching_contract())
                    .into_catalog(),
                seeds: vec![SketchSeed {
                    contract_file: contract_file.clone(),
                    sketch_id: "answer_body".to_owned(),
                    signature_type: "function".to_owned(),
                    file: CatalogPath::new("src/lib.rs").expect("path"),
                    code: "pub fn answer() -> u8 { 42 }".to_owned(),
                }],
            }),
        )
        .expect("generate");
        let generated_yaml = std::str::from_utf8(
            generated
                .contract_files
                .get(&contract_file)
                .expect("generated yaml"),
        )
        .expect("generated yaml utf8");

        assert!(generated_yaml.contains("sketches"));
        assert!(generated_yaml.contains("sketch: answer_body"));
        assert!(generated_yaml.contains("answer_body: null"));

        let response = futures_executor::block_on(kit.check(CheckRequest {
            source_files,
            contract_files: generated.contract_files,
            report: ReportRequest::None,
            mode: CheckMode::Strict,
        }))
        .expect("check generated");

        assert!(response.passed);
    }

    #[test]
    fn diff_reports_semantic_sketch_changes() {
        let kit = SketchContractKitBuilder::default().build().expect("kit");
        let previous_contract_files = CatalogFixture::new()
            .with_file(
                "previous.yml",
                &CheckFixture::linked_contract("let value = 1;"),
            )
            .into_catalog();
        let current_contract_files = CatalogFixture::new()
            .with_file(
                "current.yml",
                &CheckFixture::linked_contract("let value = 2;"),
            )
            .into_catalog();

        let response = futures_executor::block_on(kit.diff(DiffRequest {
            current_contract_files,
            previous_contract_files,
        }))
        .expect("diff");

        assert!(response.changed);
        assert_eq!(
            response.entries,
            vec![DiffEntry::Changed {
                sketch_id: "answer_body".to_owned(),
            }]
        );
    }
}

mod error_tests {
    use super::*;

    #[test]
    fn parse_errors_include_catalog_path_and_signature_label() {
        let kit = SketchContractKitBuilder::default().build().expect("kit");
        let source_files = CatalogFixture::new()
            .with_file("src/lib.rs", "pub fn answer() -> u8 { 42 }\n")
            .into_catalog();
        let contract_files = CatalogFixture::new()
            .with_file(
                "main.yml",
                r#"
root: ../src
files: [src/lib.rs]
signatures:
  - answer_signature:
      file: ../src/lib.rs
      signature_type: function
      sketch: answer_body
sketches:
  - answer_body:
    signature_type: function
    code: pub fn answer() -> u8 { 42 }
"#,
            )
            .into_catalog();
        let error = futures_executor::block_on(kit.check(CheckRequest {
            source_files,
            contract_files,
            report: ReportRequest::None,
            mode: CheckMode::Strict,
        }))
        .expect_err("invalid file path should fail");
        let message = error.to_string();

        assert!(message.contains("main.yml"));
        assert!(message.contains("answer_signature"));
    }

    #[test]
    fn duplicate_catalog_path_errors_keep_public_message() {
        let mut catalog = FileCatalog::new();
        let path = CatalogPath::new("src/lib.rs").expect("path");
        catalog
            .insert(path.clone(), Vec::new())
            .expect("first insert");
        let error = catalog
            .insert(path, Vec::new())
            .expect_err("duplicate should fail");

        assert!(error.to_string().contains("duplicate catalog path"));
    }
}

mod boundary_tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn production_sources_do_not_import_forbidden_boundaries() {
        let forbidden = [
            "conkit_signature::",
            "use conkit_signature",
            "clap",
            "tokio",
            "async_trait",
            "std::fs",
            "fs_err",
            "std::process::exit",
        ];

        for source in ProductionSources::new().rust_files() {
            let contents = fs::read_to_string(&source).expect("read source");
            for marker in forbidden {
                assert!(
                    !contents.contains(marker),
                    "{} contains forbidden marker {marker}",
                    source.display()
                );
            }
        }
    }

    #[test]
    fn production_cfg_test_is_limited_to_local_test_modules() {
        for source in ProductionSources::new().rust_files() {
            let contents = fs::read_to_string(&source).expect("read source");
            let lines = contents.lines().collect::<Vec<_>>();

            for (index, line) in lines.iter().enumerate() {
                if line.trim() == "#[cfg(test)]" {
                    let next = lines
                        .iter()
                        .skip(index + 1)
                        .find(|candidate| !candidate.trim().is_empty())
                        .copied()
                        .unwrap_or_default();
                    assert!(
                        next.trim().starts_with("mod tests"),
                        "{} has production-scope cfg(test) near line {}",
                        source.display(),
                        index + 1
                    );
                }
            }
        }
    }

    #[test]
    fn public_api_dtos_do_not_use_os_paths() {
        let api = ProductionSources::new().manifest_dir().join("api.rs");
        let contents = fs::read_to_string(&api).expect("read api");

        for marker in ["PathBuf", "std::path::Path", "use std::path"] {
            assert!(
                !contents.contains(marker),
                "api.rs should not expose OS path marker {marker}"
            );
        }
    }

    struct ProductionSources {
        manifest_dir: PathBuf,
    }

    impl ProductionSources {
        fn new() -> Self {
            Self {
                manifest_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            }
        }

        fn manifest_dir(&self) -> &Path {
            &self.manifest_dir
        }

        fn rust_files(&self) -> Vec<PathBuf> {
            [
                "api.rs",
                "contract.rs",
                "error.rs",
                "files.rs",
                "generate.rs",
                "id.rs",
                "inventory.rs",
                "lib.rs",
                "matcher.rs",
                "normalize.rs",
                "report.rs",
                "work.rs",
            ]
            .into_iter()
            .map(|file| self.manifest_dir.join(file))
            .collect()
        }
    }
}

struct CatalogFixture {
    catalog: FileCatalog,
}

impl CatalogFixture {
    fn new() -> Self {
        Self {
            catalog: FileCatalog::new(),
        }
    }

    fn with_file(mut self, path: &str, contents: &str) -> Self {
        self.catalog
            .insert(
                CatalogPath::new(path).expect("fixture path"),
                contents.as_bytes().to_vec(),
            )
            .expect("insert fixture");
        self
    }

    fn into_catalog(self) -> FileCatalog {
        self.catalog
    }
}

struct CheckFixture {
    source_files: FileCatalog,
    contract_files: FileCatalog,
}

impl CheckFixture {
    fn matching() -> Self {
        Self {
            source_files: CatalogFixture::new()
                .with_file("src/lib.rs", "pub fn answer() -> u8 {\n    42\n}\n")
                .into_catalog(),
            contract_files: CatalogFixture::new()
                .with_file("main.yml", Self::matching_contract())
                .into_catalog(),
        }
    }

    fn mismatched() -> Self {
        Self {
            source_files: CatalogFixture::new()
                .with_file("src/lib.rs", "pub fn answer() -> u8 {\n    41\n}\n")
                .into_catalog(),
            contract_files: CatalogFixture::new()
                .with_file("main.yml", Self::matching_contract())
                .into_catalog(),
        }
    }

    fn request(self, report: ReportRequest, mode: CheckMode) -> CheckRequest {
        CheckRequest {
            source_files: self.source_files,
            contract_files: self.contract_files,
            report,
            mode,
        }
    }

    fn matching_contract() -> &'static str {
        r#"
root: ../src
files: [src/lib.rs]
signatures:
  - answer_signature:
      file: src/lib.rs
      signature_type: function
      name: answer
      sketch: answer_body
sketches:
  - answer_body:
    signature_type: function
    code: |
      pub fn answer() -> u8 {
          42
      }
"#
    }

    fn linked_contract(code: &str) -> String {
        format!(
            "root: ../src\nfiles: [src/lib.rs]\nsignatures:\n  - answer_signature:\n      file: src/lib.rs\n      signature_type: function\n      sketch: answer_body\nsketches:\n  - answer_body:\n    signature_type: function\n    code: '{code}'\n"
        )
    }
}
