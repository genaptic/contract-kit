use crate::support::PublicFixture;
use conkit_signature::{
    CatalogPath, CheckMode, CheckRequest, DiffCategory, DiffEntry, ReportFormat, ReportRequest,
    ResolveSketchesRequest, RustCrateKind, RustExtractionInput,
};

#[test]
fn generated_contracts_check_and_diff_through_catalog_boundaries() {
    let fixture = PublicFixture::new();
    let baseline_sources =
        PublicFixture::catalog([("src/lib.rs", b"pub fn answer() -> u8 { 1 }\n".as_slice())]);
    let generated = fixture.generate(
        baseline_sources.clone(),
        &["src/lib.rs"],
        vec![PublicFixture::crate_root(
            "sample",
            "src/lib.rs",
            RustCrateKind::Library,
        )],
    );

    assert_eq!(generated.counts.document_count, 1);
    assert_eq!(generated.counts.signature_count, 1);
    assert_eq!(generated.counts.semantically_changed_document_count, 1);
    assert_eq!(generated.counts.byte_changed_document_count, 1);
    assert!(PublicFixture::generated_yaml(&generated).contains("answer"));

    let check = fixture.check(
        baseline_sources,
        generated.contract_files.clone(),
        CheckMode::Default,
    );
    assert!(check.passed);
    assert!(check.diagnostics.is_empty());
    assert_eq!(check.counts.source_signature_count, 1);
    assert_eq!(check.counts.contract_signature_count, 1);
    assert_eq!(check.digest_version, 2);
    PublicFixture::assert_digest(&check.source_shape_digest);

    let current = fixture.generate(
        PublicFixture::catalog([("src/lib.rs", b"pub fn answer() -> u16 { 1 }\n".as_slice())]),
        &["src/lib.rs"],
        vec![PublicFixture::crate_root(
            "sample",
            "src/lib.rs",
            RustCrateKind::Library,
        )],
    );
    let diff = fixture.diff(current.contract_files, generated.contract_files);

    assert!(diff.changed());
    assert_eq!(diff.digest_version, 2);
    PublicFixture::assert_digest(&diff.contract_digest);
    assert!(diff.entries.iter().any(|entry| matches!(
        entry,
        DiffEntry::Changed {
            signature_id,
            categories,
            ..
        } if signature_id.contains("answer")
            && categories == &[DiffCategory::SourceSemantics].into_iter().collect()
    )));
}

#[test]
fn check_reports_preserve_public_yaml_and_json_layouts() {
    let fixture = PublicFixture::new();
    let source_files =
        PublicFixture::catalog([("src/lib.rs", b"pub fn answer() -> u8 { 1 }\n".as_slice())]);
    let contract_files = fixture
        .generate(
            source_files.clone(),
            &["src/lib.rs"],
            vec![PublicFixture::crate_root(
                "sample",
                "src/lib.rs",
                RustCrateKind::Library,
            )],
        )
        .contract_files;

    for (format, path) in [
        (ReportFormat::Yaml, "reports/check.yaml"),
        (ReportFormat::Json, "reports/check.json"),
    ] {
        let is_yaml = matches!(format, ReportFormat::Yaml);
        let report_path = CatalogPath::new(path).expect("report path");
        let response = futures_executor::block_on(fixture.kit.check(CheckRequest {
            extraction: RustExtractionInput::Syntax,
            source_files: source_files.clone(),
            contract_files: contract_files.clone(),
            report: ReportRequest::Generate {
                format,
                output_file: report_path.clone(),
            },
            mode: CheckMode::Default,
        }))
        .expect("check report");
        let bytes = response
            .report_files
            .get(&report_path)
            .expect("generated report");
        let report: serde_json::Value = if is_yaml {
            serde_saphyr::from_slice(bytes).expect("semantic YAML report")
        } else {
            serde_json::from_slice(bytes).expect("JSON report")
        };

        assert_eq!(report["passed"], true);
        assert_eq!(report["digest_version"], 2);
        assert!(report["source_shape_digest"].is_string());
        assert!(report.get("inventory_digest").is_none());
        assert!(report.get("report_files").is_none());
    }
}

#[test]
fn resolve_sketches_returns_neutral_exact_source_seed() {
    let fixture = PublicFixture::new();
    let response = futures_executor::block_on(fixture.kit.resolve_sketches(ResolveSketchesRequest {
        extraction: RustExtractionInput::Syntax,
        source_files: PublicFixture::catalog([(
            "main.rs",
            b"fn main() {\n    run();\n}\n".as_slice(),
        )]),
        contract_files: PublicFixture::catalog([(
            "main.yml",
            br#"contract_version: 2
root: ../src
files: [main.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: app, root: main.rs, kind: binary }] }
signatures:
  - main:
      file: main.rs
      signature_type: function
      name: main
      visibility: private
      sketch: main
sketches:
  - main:
      file: main.rs
      signature: main
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: exactly_one }
      code: |
        fn main() {}
"#
            .as_slice(),
        )]),
    }))
    .expect("resolve");

    assert_eq!(response.seeds.len(), 1);
    assert_eq!(response.seeds[0].contract_file.as_str(), "main.yml");
    assert_eq!(response.seeds[0].document_index, 0);
    assert_eq!(response.seeds[0].file.as_str(), "main.rs");
    assert_eq!(response.seeds[0].signature_type, "function");
    assert_eq!(response.seeds[0].code, "fn main() {\n    run();\n}");
}

#[test]
fn typed_new_generation_supports_multiple_crate_roots() {
    let fixture = PublicFixture::new();
    let response = fixture.generate(
        PublicFixture::catalog([
            ("lib.rs", b"pub fn library_entry() {}\n".as_slice()),
            ("main.rs", b"pub fn binary_entry() {}\n".as_slice()),
        ]),
        &["lib.rs", "main.rs"],
        vec![
            PublicFixture::crate_root("library_api", "lib.rs", RustCrateKind::Library),
            PublicFixture::crate_root("binary_app", "main.rs", RustCrateKind::Binary),
        ],
    );
    let yaml = PublicFixture::generated_yaml(&response);

    assert_eq!(response.counts.signature_count, 2);
    for required in [
        "id: library_api",
        "root: lib.rs",
        "kind: library",
        "id: binary_app",
        "root: main.rs",
        "kind: binary",
        "crate_id: library_api",
        "crate_id: binary_app",
    ] {
        assert!(yaml.contains(required), "missing {required:?} in:\n{yaml}");
    }
}

#[test]
fn extraction_context_changes_are_typed_in_contract_diff() {
    let fixture = PublicFixture::new();
    let source = PublicFixture::catalog([("entry.rs", b"pub fn answer() {}\n".as_slice())]);
    let previous = fixture.generate(
        source.clone(),
        &["entry.rs"],
        vec![PublicFixture::crate_root(
            "example",
            "entry.rs",
            RustCrateKind::Library,
        )],
    );
    let current = fixture.generate(
        source,
        &["entry.rs"],
        vec![PublicFixture::crate_root(
            "example",
            "entry.rs",
            RustCrateKind::Binary,
        )],
    );
    let response = fixture.diff(current.contract_files, previous.contract_files);

    assert!(response.changed());
    assert!(response.entries.iter().any(|entry| matches!(
        entry,
        DiffEntry::Changed {
            signature_id,
            current_digest,
            previous_digest,
            categories,
        } if signature_id.contains("answer")
            && current_digest != previous_digest
            && categories == &[DiffCategory::ExtractionContext].into_iter().collect()
    )));
}

#[test]
fn syntax_mode_reports_cfg_capability_warnings_with_mode_specific_pass_policy() {
    let fixture = PublicFixture::new();
    let source = PublicFixture::catalog([(
        "lib.rs",
        b"#[cfg(feature = \"durable\")] pub fn conditional() {}\n".as_slice(),
    )]);
    let contracts = fixture
        .generate(
            source.clone(),
            &["lib.rs"],
            vec![PublicFixture::crate_root(
                "sample",
                "lib.rs",
                RustCrateKind::Library,
            )],
        )
        .contract_files;
    let default = fixture.check(source.clone(), contracts.clone(), CheckMode::Default);
    let strict = fixture.check(source.clone(), contracts.clone(), CheckMode::Strict);
    let warning = fixture.check(source, contracts, CheckMode::Warning);

    assert!(default.passed, "{:?}", default.diagnostics);
    assert!(!strict.passed);
    assert!(warning.passed);
    assert!(PublicFixture::has_cfg_capability_warning(&default));
    assert_eq!(default.diagnostics, strict.diagnostics);
    assert_eq!(default.diagnostics, warning.diagnostics);
}
