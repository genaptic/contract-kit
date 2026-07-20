use crate::support::PublicFixture;
use conkit_signature::{
    CatalogPath, CheckDiagnostic, CheckDiagnosticCategory, CheckMode, CheckRequest, CheckResponse,
    ContractScope, DiagnosticSeverity, DiffCategory, DiffEntry, DiffRequest, DiffResponse,
    FileCatalog, GenerateRequest, GenerateResponse, GenerateTarget, LimitResource, OutputLimits,
    ReportFormat, ReportRequest, RustExtractionInput, SignatureCheckCounts,
    SignatureContractKitBuilder, SignatureContractKitError, SignatureGenerationCounts,
    SignatureLimits, WorkOptions, WorkerPool,
};
use std::num::NonZeroUsize;

struct AliasReplayBudgetFixture {
    yaml: &'static [u8],
    raw: serde_saphyr::budget::BudgetReport,
    semantic: serde_saphyr::budget::BudgetReport,
}

impl AliasReplayBudgetFixture {
    fn new() -> Self {
        let yaml = b"contract_version: 2\nroot: ../src\nfiles: [&source lib.rs]\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: *source, kind: library }] }\nsignatures: &empty []\nsketches: *empty\n";
        let source = std::str::from_utf8(yaml).expect("UTF-8 fixture");
        let raw = serde_saphyr::budget::check_yaml_budget(
            source,
            serde_saphyr::Budget::default(),
            serde_saphyr::budget::EnforcingPolicy::AllContent,
        )
        .expect("raw YAML budget report");
        let semantic = std::rc::Rc::new(std::cell::RefCell::new(None));
        let callback = std::rc::Rc::clone(&semantic);
        let options = serde_saphyr::options! {
            budget: serde_saphyr::budget! {},
            merge_keys: serde_saphyr::MergeKeyPolicy::Error,
        }
        .with_budget_report(move |report| {
            *callback.borrow_mut() = Some(report);
        });
        serde_saphyr::from_multiple_with_options::<serde_json::Value>(source, options)
            .expect("semantic YAML fixture");
        let semantic = semantic
            .borrow_mut()
            .take()
            .expect("semantic YAML budget report");

        Self {
            yaml,
            raw,
            semantic,
        }
    }

    fn catalog(&self, path: &str) -> FileCatalog {
        PublicFixture::catalog([(path, self.yaml)])
    }

    fn operation_nodes(&self) -> u64 {
        u64::try_from(self.semantic.nodes)
            .expect("semantic node count")
            .checked_mul(2)
            .expect("two-catalog node count")
    }

    fn operation_scalar_bytes(&self) -> u64 {
        u64::try_from(self.semantic.total_scalar_bytes)
            .expect("semantic scalar byte count")
            .checked_mul(2)
            .expect("two-catalog scalar byte count")
    }

    fn diff_with_limits(
        &self,
        limits: SignatureLimits,
    ) -> Result<DiffResponse, SignatureContractKitError> {
        let kit = SignatureContractKitBuilder::default()
            .with_limits(limits)
            .build()
            .expect("budget-limited kit");

        futures_executor::block_on(kit.diff(DiffRequest {
            current_contract_files: self.catalog("current.yml"),
            previous_contract_files: self.catalog("previous.yml"),
        }))
    }
}

#[test]
fn builder_accepts_work_options() {
    let options = WorkOptions {
        pool: WorkerPool::Dedicated {
            worker_threads: NonZeroUsize::new(1).expect("nonzero"),
        },
        max_in_flight_operations: NonZeroUsize::new(1).expect("nonzero"),
        max_pending_operations: 0,
    };

    SignatureContractKitBuilder::default()
        .with_work_options(options)
        .build()
        .expect("builder should construct the local contract kit");
}

#[test]
fn builder_applies_catalog_limits_before_parsing() {
    let mut limits = SignatureLimits::default();
    limits.catalog.total_bytes = 1;
    let kit = SignatureContractKitBuilder::default()
        .with_limits(limits)
        .build()
        .expect("kit");

    let error = futures_executor::block_on(kit.generate(GenerateRequest {
        extraction: RustExtractionInput::Syntax,
        source_files: PublicFixture::catalog([("lib.rs", b"pub fn answer() {}".as_slice())]),
        target: PublicFixture::single_target("lib.rs"),
        scope: ContractScope::Signatures,
    }))
    .expect_err("request must be rejected before Rust parsing");
    let limit = error.limit_exceeded().expect("typed catalog limit");

    assert_eq!(limit.resource, LimitResource::CatalogTotalBytes);
    assert_eq!(limit.limit, 1);
    assert_eq!(limit.file.as_ref().map(CatalogPath::as_str), Some("lib.rs"));
}

#[test]
fn generation_verification_reparse_preserves_the_typed_yaml_limit() {
    let document = b"contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }\nsignatures: []\nsketches: []\n";
    let source = std::str::from_utf8(document).expect("UTF-8 fixture");
    let report = serde_saphyr::budget::check_yaml_budget(
        source,
        serde_saphyr::Budget::default(),
        serde_saphyr::budget::EnforcingPolicy::AllContent,
    )
    .expect("initial YAML budget report");
    let initial_nodes = u64::try_from(report.nodes).expect("initial node count");
    let mut limits = SignatureLimits::default();
    limits.yaml.nodes = initial_nodes;
    let kit = SignatureContractKitBuilder::default()
        .with_limits(limits)
        .build()
        .expect("node-limited kit");

    let error = futures_executor::block_on(kit.generate(GenerateRequest {
        extraction: RustExtractionInput::Syntax,
        source_files: PublicFixture::catalog([("lib.rs", b"pub fn answer() {}\n".as_slice())]),
        target: GenerateTarget::Existing(PublicFixture::catalog([("main.yml", document)])),
        scope: ContractScope::Signatures,
    }))
    .expect_err("the verification reparse must share the initial YAML budget");
    let limit = error.limit_exceeded().expect("typed reparse limit");

    assert_eq!(limit.resource, LimitResource::YamlNodeCount);
    assert_eq!(limit.limit, initial_nodes);
    assert_eq!(limit.observed_at_least, initial_nodes.saturating_add(1));
    assert_eq!(
        limit.file.as_ref().map(CatalogPath::as_str),
        Some("main.yml")
    );
}

#[test]
fn alias_replay_budgets_accumulate_across_diff_catalogs() {
    let fixture = AliasReplayBudgetFixture::new();
    assert!(fixture.semantic.nodes.saturating_sub(fixture.raw.nodes) >= 2);
    assert!(fixture.semantic.total_scalar_bytes > fixture.raw.total_scalar_bytes);

    let operation_nodes = fixture.operation_nodes();
    let node_limit_value = operation_nodes
        .checked_sub(1)
        .expect("positive operation node count");
    let mut node_limits = SignatureLimits::default();
    node_limits.yaml.nodes = node_limit_value;
    let node_error = fixture
        .diff_with_limits(node_limits)
        .expect_err("the second catalog's replay nodes must cross the operation budget");
    let node_limit = node_error.limit_exceeded().expect("typed node limit");

    assert_eq!(node_limit.resource, LimitResource::YamlNodeCount);
    assert_eq!(node_limit.limit, node_limit_value);
    assert_eq!(node_limit.observed_at_least, operation_nodes);
    assert_eq!(
        node_limit.file.as_ref().map(CatalogPath::as_str),
        Some("previous.yml")
    );

    let operation_scalar_bytes = fixture.operation_scalar_bytes();
    let scalar_limit_value = operation_scalar_bytes
        .checked_sub(1)
        .expect("positive operation scalar byte count");
    let mut scalar_limits = SignatureLimits::default();
    scalar_limits.yaml.nodes = operation_nodes;
    scalar_limits.yaml.alias_expansion_bytes = scalar_limit_value;
    let scalar_error = fixture
        .diff_with_limits(scalar_limits)
        .expect_err("the second catalog's replayed scalar must cross the operation budget");
    let scalar_limit = scalar_error
        .limit_exceeded()
        .expect("typed alias-expansion limit");

    assert_eq!(
        scalar_limit.resource,
        LimitResource::YamlAliasExpansionBytes
    );
    assert_eq!(scalar_limit.limit, scalar_limit_value);
    assert_eq!(scalar_limit.observed_at_least, operation_scalar_bytes);
    assert_eq!(
        scalar_limit.file.as_ref().map(CatalogPath::as_str),
        Some("previous.yml")
    );

    let mut exact_limits = SignatureLimits::default();
    exact_limits.yaml.nodes = operation_nodes;
    exact_limits.yaml.alias_expansion_bytes = operation_scalar_bytes;
    fixture
        .diff_with_limits(exact_limits)
        .expect("two replaying catalogs must fit at the exact operation boundary");
}

#[test]
fn builder_propagates_the_nominal_output_scratch_limit() {
    let document = b"contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }\nsignatures:\n  - answer_function:\n      file: lib.rs\n      signature_type: function\n      name: answer\n      visibility: public\nsketches: []\n";
    let mut limits = SignatureLimits::default();
    limits.output.scratch_bytes = 0;
    let kit = SignatureContractKitBuilder::default()
        .with_limits(limits)
        .build()
        .expect("scratch-limited kit");

    let error = futures_executor::block_on(kit.generate(GenerateRequest {
        extraction: RustExtractionInput::Syntax,
        source_files: PublicFixture::catalog([(
            "lib.rs",
            b"pub fn answer(value: i64) -> i64 { value }\n".as_slice(),
        )]),
        target: GenerateTarget::Existing(PublicFixture::catalog([("main.yml", document)])),
        scope: ContractScope::Signatures,
    }))
    .expect_err("changed YAML must cross the configured scratch budget");
    let limit = error.limit_exceeded().expect("typed scratch limit");

    assert_eq!(limit.resource, LimitResource::OutputScratchBytes);
    assert_eq!(limit.limit, 0);
    assert_eq!(limit.observed_at_least, 1);
    assert_eq!(
        limit.file.as_ref().map(CatalogPath::as_str),
        Some("main.yml")
    );
}

#[test]
fn output_scratch_limits_default_and_round_trip_without_changing_contracts() {
    let missing_field = serde_json::json!({ "generated_bytes": 17 });
    let defaulted: OutputLimits =
        serde_json::from_value(missing_field).expect("missing scratch field defaults");
    assert_eq!(defaulted.generated_bytes, 17);
    assert_eq!(defaulted.scratch_bytes, 512 * 1024 * 1024);

    let explicit = OutputLimits {
        generated_bytes: 23,
        scratch_bytes: 0,
    };
    let encoded = serde_json::to_value(&explicit).expect("serializable output limits");
    assert_eq!(encoded["scratch_bytes"], 0);
    assert_eq!(
        serde_json::from_value::<OutputLimits>(encoded).expect("output limits round trip"),
        explicit
    );

    let resource = serde_json::to_value(LimitResource::OutputScratchBytes)
        .expect("serializable scratch resource");
    assert_eq!(
        serde_json::from_value::<LimitResource>(resource).expect("resource round trip"),
        LimitResource::OutputScratchBytes
    );
}

#[test]
fn diagnostics_expose_typed_severity_and_category() {
    let source_error = CheckDiagnostic::Extra {
        signature_id: "rust:function:extra".to_owned(),
    };
    let capability_warning = CheckDiagnostic::Warning {
        message: "rust_syntax_v2 cannot evaluate cfg".to_owned(),
    };

    assert_eq!(source_error.severity(), DiagnosticSeverity::Error);
    assert_eq!(
        source_error.category(),
        CheckDiagnosticCategory::SourceSemantics
    );
    assert_eq!(capability_warning.severity(), DiagnosticSeverity::Warning);
    assert_eq!(
        capability_warning.category(),
        CheckDiagnosticCategory::SyntaxCapability
    );
}

#[test]
fn response_dtos_require_v2_digests_and_cohesive_counts() {
    let check = CheckResponse {
        passed: true,
        source_shape_digest: PublicFixture::digest('0'),
        digest_version: 2,
        counts: SignatureCheckCounts {
            source_signature_count: 1,
            contract_signature_count: 1,
        },
        diagnostics: Vec::new(),
        report_files: conkit_signature::FileCatalog::new(),
    };
    let check_value = serde_json::to_value(check).expect("serializable check response");
    assert_eq!(check_value["digest_version"], 2);
    assert!(check_value["source_shape_digest"].is_string());
    assert!(check_value.get("inventory_digest").is_none());
    assert!(
        serde_json::from_value::<CheckResponse>(serde_json::json!({
            "passed": true,
            "digest_version": 2,
            "counts": {
                "source_signature_count": 1,
                "contract_signature_count": 1
            },
            "diagnostics": [],
            "report_files": {}
        }))
        .is_err()
    );

    let unchanged = DiffResponse {
        contract_digest: PublicFixture::digest('1'),
        digest_version: 2,
        entries: Vec::new(),
    };
    assert!(!unchanged.changed());
    let changed = DiffResponse {
        contract_digest: unchanged.contract_digest.clone(),
        digest_version: 2,
        entries: vec![DiffEntry::Changed {
            signature_id: "answer".to_owned(),
            current_digest: PublicFixture::digest('a'),
            previous_digest: PublicFixture::digest('b'),
            categories: [DiffCategory::SourceSemantics].into_iter().collect(),
        }],
    };
    assert!(changed.changed());
    let diff_value = serde_json::to_value(changed).expect("serializable diff response");
    assert_eq!(diff_value["digest_version"], 2);
    assert!(diff_value["contract_digest"].is_string());
    assert!(diff_value.get("changed").is_none());
    assert!(
        serde_json::from_value::<DiffResponse>(serde_json::json!({
            "digest_version": 2,
            "entries": []
        }))
        .is_err()
    );

    let generation = GenerateResponse {
        contract_files: conkit_signature::FileCatalog::new(),
        counts: SignatureGenerationCounts {
            document_count: 2,
            signature_count: 3,
            preserved_sketch_count: 1,
            semantically_changed_document_count: 1,
            byte_changed_document_count: 1,
        },
        resolved_sketch_seeds: Vec::new(),
        capability_warnings: vec!["rust_syntax_v2 capability warning: conditional API".to_owned()],
    };
    let generation_value =
        serde_json::to_value(generation).expect("serializable generation response");
    assert_eq!(generation_value["counts"]["document_count"], 2);
    assert_eq!(generation_value["counts"]["signature_count"], 3);
    assert_eq!(generation_value["counts"]["preserved_sketch_count"], 1);
    assert_eq!(
        generation_value["counts"]["semantically_changed_document_count"],
        1
    );
    assert_eq!(generation_value["counts"]["byte_changed_document_count"], 1);
    assert_eq!(
        generation_value["resolved_sketch_seeds"],
        serde_json::json!([])
    );
    assert!(generation_value.get("signature_count").is_none());
    assert!(generation_value.get("sketch_count").is_none());
    assert!(
        serde_json::from_value::<GenerateResponse>(serde_json::json!({
            "contract_files": {},
            "signature_count": 3,
            "sketch_count": 1
        }))
        .is_err()
    );
}

#[test]
fn request_dtos_are_catalog_owned() {
    let source_files =
        PublicFixture::catalog([("src/lib.rs", b"pub fn answer() -> u8 { 1 }\n".as_slice())]);
    let contract_files = PublicFixture::catalog([(
        "main.yml",
        b"contract_version: 2\nroot: ../src\nfiles: [src/lib.rs]\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: src/lib.rs, kind: library }] }\nsignatures: []\nsketches: []\n"
            .as_slice(),
    )]);
    let report_file = CatalogPath::new("reports/check.yaml").expect("report path");

    let check = CheckRequest {
        extraction: RustExtractionInput::Syntax,
        source_files: source_files.clone(),
        contract_files: contract_files.clone(),
        report: ReportRequest::Generate {
            format: ReportFormat::Yaml,
            output_file: report_file.clone(),
        },
        mode: CheckMode::Strict,
    };
    let generate = GenerateRequest {
        extraction: RustExtractionInput::Syntax,
        source_files,
        target: PublicFixture::single_target("src/lib.rs"),
        scope: ContractScope::Signatures,
    };
    let diff = DiffRequest {
        current_contract_files: contract_files,
        previous_contract_files: conkit_signature::FileCatalog::new(),
    };

    let check_value = serde_json::to_value(&check).expect("serializable check request");
    assert!(check_value.get("scope").is_none());
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
