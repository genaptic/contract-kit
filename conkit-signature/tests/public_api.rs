use conkit_signature::{
    CatalogPath, CheckDiagnostic, CheckMode, CheckRequest, ContractScope, DiffEntry, DiffRequest,
    FileCatalog, GenerateDocument, GenerateRequest, GenerateTarget, ReportFormat, ReportRequest,
    ResolveSketchesRequest, SignatureContractKit, SignatureContractKitBuilder, WorkOptions,
    WorkParallelism,
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
                scope: ContractScope::Signatures,
                mode: CheckMode::Default,
            }
        }

        fn generate_request() -> GenerateRequest {
            GenerateRequest {
                source_files: FileCatalog::new(),
                target: GenerateTarget::New(GenerateDocument {
                    contract_file: CatalogPath::new("main.yml").expect("contract path"),
                    root: "../src".to_owned(),
                    files: vec![CatalogPath::new("lib.rs").expect("source path")],
                }),
                scope: ContractScope::Signatures,
            }
        }

        fn resolve_sketches_request() -> ResolveSketchesRequest {
            ResolveSketchesRequest {
                source_files: FileCatalog::new(),
                contract_files: FileCatalog::new(),
            }
        }

        fn diff_request() -> DiffRequest {
            DiffRequest {
                current_contract_files: FileCatalog::new(),
                previous_contract_files: FileCatalog::new(),
            }
        }

        fn assert_public_contracts() {
            Self::assert_send_static::<SignatureContractKitBuilder>();
            Self::assert_send_sync_static::<SignatureContractKit>();

            let kit = SignatureContractKitBuilder::default().build().expect("kit");
            Self::assert_send(kit.check(Self::check_request()));
            Self::assert_send(kit.generate(Self::generate_request()));
            Self::assert_send(kit.resolve_sketches(Self::resolve_sketches_request()));
            Self::assert_send(kit.diff(Self::diff_request()));

            let kit = Arc::new(kit);

            let task_kit = Arc::clone(&kit);
            Self::assert_spawn_compatible(
                async move { task_kit.check(Self::check_request()).await },
            );

            let task_kit = Arc::clone(&kit);
            Self::assert_spawn_compatible(async move {
                task_kit.generate(Self::generate_request()).await
            });

            let task_kit = Arc::clone(&kit);
            Self::assert_spawn_compatible(async move {
                task_kit
                    .resolve_sketches(Self::resolve_sketches_request())
                    .await
            });

            let task_kit = Arc::clone(&kit);
            Self::assert_spawn_compatible(async move { task_kit.diff(Self::diff_request()).await });
        }
    }

    #[test]
    fn public_operations_support_send_and_owning_spawn_contracts() {
        SpawnContract::assert_public_contracts();
    }
}

mod builder_tests {
    use super::*;

    #[test]
    fn build_constructs_contract_kit() {
        SignatureContractKitBuilder::default()
            .build()
            .expect("builder should construct the local contract kit");
    }

    #[test]
    fn builder_accepts_work_options() {
        let options = WorkOptions {
            parallelism: WorkParallelism::Fixed(NonZeroUsize::new(1).expect("nonzero")),
        };

        SignatureContractKitBuilder::default()
            .with_work_options(options)
            .build()
            .expect("builder should construct the local contract kit");
    }
}

mod dto_tests {
    use super::*;

    #[test]
    fn public_dtos_are_constructed_from_catalogs() {
        let source_files = catalog_with("src/lib.rs", b"pub fn answer() -> u8 { 1 }\n");
        let contract_files = catalog_with(
            "main.yml",
            b"root: ../src\nfiles: []\nsignatures: []\nsketches: []\n",
        );
        let report_file = CatalogPath::new("reports/check.yaml").expect("report path");

        let check = CheckRequest {
            source_files: source_files.clone(),
            contract_files: contract_files.clone(),
            report: ReportRequest::Generate {
                format: ReportFormat::Yaml,
                output_file: report_file.clone(),
            },
            scope: ContractScope::All,
            mode: CheckMode::Strict,
        };
        let generate = GenerateRequest {
            source_files,
            target: new_target("src/lib.rs"),
            scope: ContractScope::Signatures,
        };
        let diff = DiffRequest {
            current_contract_files: contract_files,
            previous_contract_files: FileCatalog::new(),
        };

        assert_eq!(check.scope, ContractScope::All);
        assert_eq!(check.mode, CheckMode::Strict);
        assert_eq!(generate.scope, ContractScope::Signatures);
        assert!(diff.previous_contract_files.is_empty());

        match check.report {
            ReportRequest::Generate {
                format,
                output_file,
            } => {
                assert_eq!(format, ReportFormat::Yaml);
                assert_eq!(output_file, report_file);
            }
            ReportRequest::None => panic!("report request should carry output catalog path"),
        }
    }
}

mod async_api_tests {
    use super::*;

    #[test]
    fn check_passes_for_generated_contract_catalog() {
        let kit = SignatureContractKitBuilder::default().build().expect("kit");
        let source_files = catalog_with("src/lib.rs", b"pub fn answer() -> u8 { 1 }\n");
        let generated = futures_executor::block_on(kit.generate(GenerateRequest {
            source_files: source_files.clone(),
            target: new_target("src/lib.rs"),
            scope: ContractScope::Signatures,
        }))
        .expect("generate");
        let request = CheckRequest {
            source_files,
            contract_files: generated.contract_files,
            report: ReportRequest::None,
            scope: ContractScope::Signatures,
            mode: CheckMode::Default,
        };

        let response = futures_executor::block_on(kit.check(request)).expect("check");

        assert!(response.passed);
        assert!(response.diagnostics.is_empty());
        assert_eq!(response.counts.source_signature_count, 1);
        assert_eq!(response.counts.contract_signature_count, 1);
        assert!(response.inventory_digest.is_some());
    }

    #[test]
    fn check_scope_variants_use_the_same_signature_inventory() {
        let kit = SignatureContractKitBuilder::default().build().expect("kit");
        let source_files = catalog_with("lib.rs", b"pub fn answer() -> u8 { 1 }\n");
        let contract_files = futures_executor::block_on(kit.generate(GenerateRequest {
            source_files: source_files.clone(),
            target: new_target("lib.rs"),
            scope: ContractScope::Signatures,
        }))
        .expect("generate")
        .contract_files;
        let signatures = futures_executor::block_on(kit.check(CheckRequest {
            source_files: source_files.clone(),
            contract_files: contract_files.clone(),
            report: ReportRequest::None,
            scope: ContractScope::Signatures,
            mode: CheckMode::Default,
        }))
        .expect("signature check");
        let all = futures_executor::block_on(kit.check(CheckRequest {
            source_files,
            contract_files,
            report: ReportRequest::None,
            scope: ContractScope::All,
            mode: CheckMode::Default,
        }))
        .expect("all-scope signature check");

        assert_eq!(all, signatures);
    }

    #[test]
    fn check_rejects_a_missing_listed_source_before_parsing_unlisted_input() {
        let kit = SignatureContractKitBuilder::default().build().expect("kit");
        let request = CheckRequest {
            source_files: catalog_with("unlisted.rs", b"this is deliberately invalid Rust"),
            contract_files: catalog_with(
                "main.yml",
                b"root: ../src\nfiles: [missing.rs]\nsignatures: []\nsketches: []\n",
            ),
            report: ReportRequest::None,
            scope: ContractScope::Signatures,
            mode: CheckMode::Default,
        };

        let error = futures_executor::block_on(kit.check(request))
            .expect_err("a listed source must exist in the supplied catalog");

        assert!(
            error
                .to_string()
                .contains("listed source file is missing from source catalog"),
            "{error}"
        );
    }

    #[test]
    fn generate_returns_contract_catalog() {
        let kit = SignatureContractKitBuilder::default().build().expect("kit");
        let request = GenerateRequest {
            source_files: catalog_with("src/lib.rs", b"pub fn answer() {}\n"),
            target: new_target("src/lib.rs"),
            scope: ContractScope::Signatures,
        };

        let response = futures_executor::block_on(kit.generate(request)).expect("generate");
        let contract_path = CatalogPath::new("main.yml").expect("contract path");
        let contract_bytes = response
            .contract_files
            .get(&contract_path)
            .expect("generated contract file");
        let contract_yaml = std::str::from_utf8(contract_bytes).expect("generated YAML");

        assert_eq!(response.signature_count, 1);
        assert_eq!(response.sketch_count, 0);
        assert!(contract_yaml.contains("answer"));
        for marker in [
            concat!("canonical", "_json"),
            "file_path",
            "BaseCanonical",
            "FunctionCanonical",
            "RustSignatureCanonicalForm",
        ] {
            assert!(
                !contract_yaml.contains(marker),
                "generated contract YAML should not contain internal marker {marker}"
            );
        }
    }

    #[test]
    fn check_reports_mismatched_and_missing_signatures() {
        let kit = SignatureContractKitBuilder::default().build().expect("kit");
        let contract_files = futures_executor::block_on(kit.generate(GenerateRequest {
            source_files: catalog_with(
                "src/lib.rs",
                b"pub fn changed() -> u8 { 1 }\npub fn missing() -> u8 { 1 }\n",
            ),
            target: new_target("src/lib.rs"),
            scope: ContractScope::Signatures,
        }))
        .expect("generate")
        .contract_files;
        let request = CheckRequest {
            source_files: catalog_with("src/lib.rs", b"pub fn changed() -> u16 { 1 }\n"),
            contract_files,
            report: ReportRequest::None,
            scope: ContractScope::Signatures,
            mode: CheckMode::Default,
        };

        let response = futures_executor::block_on(kit.check(request)).expect("check");

        assert!(!response.passed);
        assert!(response.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            CheckDiagnostic::Mismatched { signature_id, .. } if signature_id.contains("changed")
        )));
        assert!(response.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            CheckDiagnostic::Missing { signature_id } if signature_id.contains("missing")
        )));
    }

    #[test]
    fn check_diagnostics_use_contract_as_expected_and_source_as_actual() {
        let kit = SignatureContractKitBuilder::default().build().expect("kit");
        let contract_files = futures_executor::block_on(kit.generate(GenerateRequest {
            source_files: catalog_with("src/lib.rs", b"pub fn expected_only() {}\n"),
            target: new_target("src/lib.rs"),
            scope: ContractScope::Signatures,
        }))
        .expect("generate expected contracts")
        .contract_files;

        let response = futures_executor::block_on(kit.check(CheckRequest {
            source_files: catalog_with("src/lib.rs", b"pub fn actual_only() {}\n"),
            contract_files,
            report: ReportRequest::None,
            scope: ContractScope::Signatures,
            mode: CheckMode::Strict,
        }))
        .expect("check");

        assert!(response.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            CheckDiagnostic::Missing { signature_id } if signature_id.contains("expected_only")
        )));
        assert!(response.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic,
            CheckDiagnostic::Extra { signature_id } if signature_id.contains("actual_only")
        )));
    }

    #[test]
    fn check_returns_yaml_report_bytes() {
        let kit = SignatureContractKitBuilder::default().build().expect("kit");
        let source_files = catalog_with("src/lib.rs", b"pub fn answer() -> u8 { 1 }\n");
        let contract_files = futures_executor::block_on(kit.generate(GenerateRequest {
            source_files: source_files.clone(),
            target: new_target("src/lib.rs"),
            scope: ContractScope::Signatures,
        }))
        .expect("generate")
        .contract_files;
        let report_path = CatalogPath::new("reports/check.yaml").expect("report path");
        let request = CheckRequest {
            source_files,
            contract_files,
            report: ReportRequest::Generate {
                format: ReportFormat::Yaml,
                output_file: report_path.clone(),
            },
            scope: ContractScope::Signatures,
            mode: CheckMode::Default,
        };

        let response = futures_executor::block_on(kit.check(request)).expect("check");
        let report = std::str::from_utf8(
            response
                .report_files
                .get(&report_path)
                .expect("YAML report file"),
        )
        .expect("YAML report utf8");

        assert!(report.contains("passed: true"));
        assert!(!report.contains("report_files"));
    }

    #[test]
    fn check_returns_json_report_bytes() {
        let kit = SignatureContractKitBuilder::default().build().expect("kit");
        let source_files = catalog_with("src/lib.rs", b"pub fn answer() -> u8 { 1 }\n");
        let contract_files = futures_executor::block_on(kit.generate(GenerateRequest {
            source_files: source_files.clone(),
            target: new_target("src/lib.rs"),
            scope: ContractScope::Signatures,
        }))
        .expect("generate")
        .contract_files;
        let report_path = CatalogPath::new("reports/check.json").expect("report path");
        let request = CheckRequest {
            source_files,
            contract_files,
            report: ReportRequest::Generate {
                format: ReportFormat::Json,
                output_file: report_path.clone(),
            },
            scope: ContractScope::Signatures,
            mode: CheckMode::Default,
        };

        let response = futures_executor::block_on(kit.check(request)).expect("check");
        let report = std::str::from_utf8(
            response
                .report_files
                .get(&report_path)
                .expect("JSON report file"),
        )
        .expect("JSON report utf8");

        assert!(report.contains("\"passed\": true"));
        assert!(!report.contains("report_files"));
    }

    #[test]
    fn diff_reports_unchanged_for_equal_contract_catalogs() {
        let kit = SignatureContractKitBuilder::default().build().expect("kit");
        let contract_files = futures_executor::block_on(kit.generate(GenerateRequest {
            source_files: catalog_with("src/lib.rs", b"pub fn answer() -> u8 { 1 }\n"),
            target: new_target("src/lib.rs"),
            scope: ContractScope::Signatures,
        }))
        .expect("generate")
        .contract_files;
        let request = DiffRequest {
            current_contract_files: contract_files.clone(),
            previous_contract_files: contract_files,
        };

        let response = futures_executor::block_on(kit.diff(request)).expect("diff");

        assert!(!response.changed);
        assert!(response.entries.is_empty());
    }

    #[test]
    fn diff_reports_added_removed_and_changed_contracts() {
        let kit = SignatureContractKitBuilder::default().build().expect("kit");
        let archived_contracts = futures_executor::block_on(kit.generate(GenerateRequest {
            source_files: catalog_with(
                "src/lib.rs",
                b"pub fn same() -> u8 { 1 }\npub fn changed() -> u8 { 1 }\npub fn removed() -> u8 { 1 }\n",
            ),
            target: new_target("src/lib.rs"),
            scope: ContractScope::Signatures,
        }))
        .expect("generate archived")
        .contract_files;
        let current_contracts = futures_executor::block_on(kit.generate(GenerateRequest {
            source_files: catalog_with(
                "src/lib.rs",
                b"pub fn same() -> u8 { 1 }\npub fn changed() -> u16 { 1 }\npub fn added() -> u8 { 1 }\n",
            ),
            target: new_target("src/lib.rs"),
            scope: ContractScope::Signatures,
        }))
        .expect("generate current")
        .contract_files;
        let response = futures_executor::block_on(kit.diff(DiffRequest {
            current_contract_files: current_contracts,
            previous_contract_files: archived_contracts,
        }))
        .expect("diff");

        assert!(response.changed);
        assert!(response.entries.iter().any(|entry| matches!(
            entry,
            DiffEntry::Added { signature_id } if signature_id.contains("added")
        )));
        assert!(response.entries.iter().any(|entry| matches!(
            entry,
            DiffEntry::Removed { signature_id } if signature_id.contains("removed")
        )));
        assert!(response.entries.iter().any(|entry| matches!(
            entry,
            DiffEntry::Changed { signature_id, .. } if signature_id.contains("changed")
        )));
    }

    #[test]
    fn resolve_sketches_returns_neutral_exact_source_seed() {
        let kit = SignatureContractKitBuilder::default().build().expect("kit");
        let response = futures_executor::block_on(kit.resolve_sketches(ResolveSketchesRequest {
            source_files: catalog_with("main.rs", b"fn main() {\n    run();\n}\n"),
            contract_files: catalog_with(
                "main.yml",
                br#"root: ../src
files: [main.rs]
signatures:
  - main:
      file: main.rs
      signature_type: main_method
      sketch: main
sketches:
  - main:
    signature_type: main_method
    code: |
      fn main() {}
"#,
            ),
        }))
        .expect("resolve");

        assert_eq!(response.seeds.len(), 1);
        assert_eq!(response.seeds[0].contract_file.as_str(), "main.yml");
        assert_eq!(response.seeds[0].file.as_str(), "main.rs");
        assert_eq!(response.seeds[0].signature_type, "main_method");
        assert_eq!(response.seeds[0].code, "fn main() {\n    run();\n}");
    }
}

mod public_boundary_tests {
    #[test]
    fn diagnostic_documentation_defines_contract_as_expected_and_source_as_actual() {
        let api = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/api.rs"))
            .expect("api source should be readable");

        for required in [
            "checking actual source signatures against",
            "expected contract signatures",
            "An expected contract signature is absent from source files.",
            "An unexpected source signature is absent from contract files.",
            "Digest from the contract inventory.",
            "Digest from the source inventory.",
        ] {
            assert!(
                api.contains(required),
                "diagnostic documentation should contain {required:?}"
            );
        }

        assert!(!api.contains("A signature exists in source files but not in contract files."));
        assert!(!api.contains("A signature exists in contract files but not in source files."));
    }

    #[test]
    fn api_does_not_restore_path_based_public_boundary() {
        let api = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/api.rs"))
            .expect("api source should be readable");

        for forbidden in [
            "source_root",
            "contracts_root",
            "archive_root",
            "archive_path",
            "output_path",
            "written_files",
            "CheckReportWriter",
            "phase_pending",
            "TODO(phase",
        ] {
            assert!(
                !api.contains(forbidden),
                "api.rs should not contain stale public boundary marker {forbidden}"
            );
        }
    }

    #[test]
    fn production_signature_code_uses_catalog_boundary() {
        ProductionSourceScan::new(env!("CARGO_MANIFEST_DIR"))
            .assert_no_forbidden_boundary_markers();
    }

    #[test]
    fn signature_does_not_own_archive_transport() {
        let root = env!("CARGO_MANIFEST_DIR");
        let manifest = std::fs::read_to_string(format!("{root}/Cargo.toml"))
            .expect("signature manifest should be readable");

        assert!(!std::path::Path::new(root).join("archive.rs").exists());
        assert!(!manifest.contains("flate2"));
    }

    #[test]
    fn rust_yaml_private_dtos_are_not_exported() {
        let root = env!("CARGO_MANIFEST_DIR");
        let export_files = [
            format!("{root}/lib.rs"),
            format!("{root}/languages/mod.rs"),
            format!("{root}/languages/rust/mod.rs"),
        ];

        for export_file in export_files {
            let source = std::fs::read_to_string(&export_file).expect("export file should read");
            for marker in [
                "RustYamlFunction",
                "RustYamlStruct",
                "RustYamlEnum",
                "RustYamlTrait",
                "RustYamlImplementation",
                "RustYamlUnion",
                "RustYamlModule",
                "RustYamlStatic",
                "RustYamlMacro",
                "RustYamlTypeAlias",
            ] {
                assert!(
                    !source.contains(marker),
                    "{export_file} should not export private Rust YAML DTO {marker}"
                );
            }
        }
    }

    struct ProductionSourceScan {
        root: &'static str,
    }

    impl ProductionSourceScan {
        fn new(root: &'static str) -> Self {
            Self { root }
        }

        fn assert_no_forbidden_boundary_markers(&self) {
            for source in self.production_sources() {
                source.assert_absent([
                    "std::fs",
                    "PathBuf",
                    "WalkDir",
                    "parse_directory",
                    "from_file",
                    "read_to_string",
                    "write_inventory",
                    "written_files",
                    "GeneratedContract",
                    "ContractText",
                    "phase_pending",
                    "TODO(phase",
                    "mod archive;",
                    "ArchiveFormat",
                    "ArchiveRequest",
                    "ArchiveResponse",
                    "ArchiveRepository",
                    "archive_failed",
                    "decode_archive",
                ]);
            }
        }

        fn production_sources(&self) -> Vec<ProductionSource> {
            let mut sources = Vec::new();
            self.collect_sources(std::path::Path::new(self.root), &mut sources);
            sources
        }

        fn collect_sources(
            &self,
            directory: &std::path::Path,
            sources: &mut Vec<ProductionSource>,
        ) {
            for entry in std::fs::read_dir(directory).expect("source directory should be readable")
            {
                let entry = entry.expect("source entry should be readable");
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().and_then(std::ffi::OsStr::to_str) != Some("tests") {
                        self.collect_sources(&path, sources);
                    }
                    continue;
                }

                if path.extension().and_then(std::ffi::OsStr::to_str) != Some("rs") {
                    continue;
                }

                let text = std::fs::read_to_string(&path).expect("source file should be readable");
                let production_text = text
                    .split_once("#[cfg(test)]")
                    .map_or(text.as_str(), |(production, _)| production)
                    .to_owned();

                sources.push(ProductionSource {
                    path: path.display().to_string(),
                    text: production_text,
                });
            }
        }
    }

    struct ProductionSource {
        path: String,
        text: String,
    }

    impl ProductionSource {
        fn assert_absent<const N: usize>(&self, markers: [&str; N]) {
            for marker in markers {
                assert!(
                    !self.text.contains(marker),
                    "{} should not contain stale production marker {marker}",
                    self.path
                );
            }
        }
    }
}

fn catalog_with(path: &str, bytes: &[u8]) -> FileCatalog {
    let mut catalog = FileCatalog::new();
    catalog
        .insert(
            CatalogPath::new(path).expect("test catalog path"),
            bytes.to_vec(),
        )
        .expect("test catalog insert");
    catalog
}

fn new_target(source_file: &str) -> GenerateTarget {
    GenerateTarget::New(GenerateDocument {
        contract_file: CatalogPath::new("main.yml").expect("contract path"),
        root: "../src".to_owned(),
        files: vec![CatalogPath::new(source_file).expect("source file")],
    })
}
