use super::support::{CatalogFixture, CheckFixture};
use conkit_sketch::{
    CatalogPath, CheckMode, CheckRequest, DiffEntry, DiffRequest, GenerateMode, GenerateRequest,
    ReportFormat, ReportRequest, SketchContractKitBuilder, SketchDiagnostic, SketchField,
    SketchSeed,
};

#[test]
fn check_passes_for_matching_sketch() {
    let kit = SketchContractKitBuilder::default().build().expect("kit");
    let response = futures_executor::block_on(
        kit.check(CheckFixture::matching().request(ReportRequest::None, CheckMode::Enforce)),
    )
    .expect("check");

    assert!(response.passed);
    assert!(response.diagnostics.is_empty());
    assert!(response.report_files.is_empty());
}

#[test]
fn check_returns_the_exact_yaml_report_boundary() {
    let kit = SketchContractKitBuilder::default().build().expect("kit");
    let report_path = CatalogPath::new("reports/check.yaml").expect("report path");
    let response = futures_executor::block_on(kit.check(CheckFixture::matching().request(
        ReportRequest::Generate {
            format: ReportFormat::Yaml,
            output_file: report_path.clone(),
        },
        CheckMode::Enforce,
    )))
    .expect("check");

    assert_eq!(
        response
            .report_files
            .get(&report_path)
            .expect("report bytes"),
        concat!(
            "passed: true\n",
            "counts:\n",
            "  source_catalog_entry_count: 1\n",
            "  referenced_source_file_count: 1\n",
            "  present_referenced_source_file_count: 1\n",
            "  contract_document_count: 1\n",
            "  sketch_count: 1\n",
            "  matched_sketch_count: 1\n",
            "  failed_sketch_count: 0\n",
            "diagnostics: []\n",
        )
        .as_bytes(),
    );
}

#[test]
fn enforce_and_warning_preserve_one_mismatch_with_distinct_pass_policies() {
    let kit = SketchContractKitBuilder::default().build().expect("kit");
    let enforce = futures_executor::block_on(
        kit.check(CheckFixture::mismatched().request(ReportRequest::None, CheckMode::Enforce)),
    )
    .expect("enforce check");
    let warning = futures_executor::block_on(
        kit.check(CheckFixture::mismatched().request(ReportRequest::None, CheckMode::Warning)),
    )
    .expect("warning check");

    assert!(!enforce.passed);
    assert!(warning.passed);
    for response in [&enforce, &warning] {
        let [SketchDiagnostic::NotMatched { sketch, .. }] = response.diagnostics.as_slice() else {
            panic!("expected one not-matched diagnostic");
        };
        assert_eq!(sketch.sketch_id, "answer_body");
        assert_eq!(sketch.contract_file.as_str(), "main.yml");
        assert_eq!(sketch.document_index, 0);
        assert_eq!(sketch.source_file.as_str(), "src/lib.rs");
    }
}

#[test]
fn public_check_ignores_unreferenced_binary_source_bytes() {
    let kit = SketchContractKitBuilder::default().build().expect("kit");
    let mut request = CheckFixture::matching().request(ReportRequest::None, CheckMode::Enforce);
    request
        .source_files
        .insert(
            CatalogPath::new("assets/blob.bin").expect("binary path"),
            vec![0, 159, 255],
        )
        .expect("binary fixture");

    let response = futures_executor::block_on(kit.check(request)).expect("check");

    assert!(response.passed);
    assert_eq!(response.counts.source_catalog_entry_count, 2);
}

#[test]
fn refreshed_linked_sketch_can_be_checked() {
    let kit = SketchContractKitBuilder::default().build().expect("kit");
    let contract_file = CatalogPath::new("main.yml").expect("contract path");
    let generated = futures_executor::block_on(
        kit.generate(GenerateRequest {
            contract_files: CatalogFixture::new()
                .with_file("main.yml", CheckFixture::matching_contract())
                .into_catalog(),
            seeds: vec![SketchSeed {
                contract_file: contract_file.clone(),
                document_index: 0,
                sketch_id: "answer_body".to_owned(),
                signature_type: "function".to_owned(),
                file: CatalogPath::new("src/lib.rs").expect("source path"),
                code: "pub fn answer() -> u8 { 42 }".to_owned(),
            }],
            mode: GenerateMode::FullRefresh,
        }),
    )
    .expect("generate");
    let response = futures_executor::block_on(
        kit.check(CheckRequest {
            source_files: CatalogFixture::new()
                .with_file("src/lib.rs", "pub fn answer() -> u8 { 42 }\n")
                .into_catalog(),
            contract_files: generated.contract_files,
            report: ReportRequest::None,
            mode: CheckMode::Enforce,
        }),
    )
    .expect("check generated contracts");

    assert!(response.passed);
}

#[test]
fn diff_reports_one_semantic_sketch_change() {
    let kit = SketchContractKitBuilder::default().build().expect("kit");
    let response = futures_executor::block_on(
        kit.diff(DiffRequest {
            current_contract_files: CatalogFixture::new()
                .with_file(
                    "current.yml",
                    &CheckFixture::linked_contract("let value = 2;"),
                )
                .into_catalog(),
            previous_contract_files: CatalogFixture::new()
                .with_file(
                    "previous.yml",
                    &CheckFixture::linked_contract("let value = 1;"),
                )
                .into_catalog(),
        }),
    )
    .expect("diff");

    assert!(response.changed());
    assert_eq!(response.digest_version, 2);
    let [
        DiffEntry::Changed {
            previous,
            current,
            fields,
        },
    ] = response.entries.as_slice()
    else {
        panic!("expected one contextual code change");
    };
    assert_eq!(previous.sketch_id, "answer_body");
    assert_eq!(previous.contract_file.as_str(), "previous.yml");
    assert_eq!(current.sketch_id, "answer_body");
    assert_eq!(current.contract_file.as_str(), "current.yml");
    assert_eq!(fields.as_slice(), [SketchField::Code]);
    assert_ne!(previous.code_digest, current.code_digest);
}
