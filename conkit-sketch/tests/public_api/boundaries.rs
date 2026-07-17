use super::support::{CatalogFixture, CheckFixture};
use conkit_sketch::{
    CatalogPath, CheckMode, CheckRequest, FileCatalog, LimitResource, ReportFormat, ReportRequest,
    SketchContractKitBuilder, SketchLimits, WorkOptions, WorkerPool,
};
use std::num::NonZeroUsize;

#[test]
fn builder_applies_work_options_and_public_limits() {
    let mut limits = SketchLimits::default();
    limits.catalog.entry_count = 0;
    let kit = SketchContractKitBuilder::default()
        .with_work_options(WorkOptions {
            pool: WorkerPool::Dedicated {
                worker_threads: NonZeroUsize::MIN,
            },
            max_in_flight_operations: NonZeroUsize::MIN,
            max_pending_operations: 0,
        })
        .with_limits(limits)
        .build()
        .expect("configured kit");

    let error = futures_executor::block_on(
        kit.check(CheckFixture::matching().request(ReportRequest::None, CheckMode::Enforce)),
    )
    .expect_err("zero catalog-entry budget must cross the public error boundary");

    assert_eq!(
        error.limit_exceeded().expect("typed limit").resource,
        LimitResource::CatalogEntryCount,
    );
}

#[test]
fn check_request_json_round_trips_nonempty_catalogs() {
    let request = CheckFixture::matching().request(
        ReportRequest::Generate {
            format: ReportFormat::Yaml,
            output_file: CatalogPath::new("reports/check.yaml").expect("report path"),
        },
        CheckMode::Warning,
    );

    let json = serde_json::to_vec(&request).expect("serialize request");
    let round_tripped = serde_json::from_slice::<CheckRequest>(&json).expect("deserialize request");

    assert_eq!(round_tripped, request);
}

#[test]
fn parse_errors_include_catalog_path_and_signature_label() {
    let kit = SketchContractKitBuilder::default().build().expect("kit");
    let contract = r#"
contract_version: 2
root: ../src
files: [src/lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: example, root: src/lib.rs, kind: library }] }
signatures:
  - answer_signature:
      file: ../src/lib.rs
      signature_type: function
      sketch: answer_body
sketches:
  - answer_body:
      file: src/lib.rs
      signature: answer_signature
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: at_least_one }
      code: pub fn answer() -> u8 { 42 }
"#;
    let error = futures_executor::block_on(
        kit.check(CheckRequest {
            source_files: CatalogFixture::new()
                .with_file("src/lib.rs", "pub fn answer() -> u8 { 42 }\n")
                .into_catalog(),
            contract_files: CatalogFixture::new()
                .with_file("main.yml", contract)
                .into_catalog(),
            report: ReportRequest::None,
            mode: CheckMode::Enforce,
        }),
    )
    .expect_err("invalid signature path must fail");
    let message = error.to_string();

    assert!(message.contains("main.yml"), "{message}");
    assert!(message.contains("answer_signature"), "{message}");
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
