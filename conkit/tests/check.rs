mod support;

use assert_fs::prelude::*;
use predicates::prelude::*;
use support::{ConkitCli, TWO_ROOT_COMPILER_COMBINED_CONTRACT};

const MATCHING_SOURCE: &str = "pub fn answer() -> u8 { 42 }\n";
const MISMATCHING_SOURCE: &str = "pub fn answer() -> u16 { 43 }\n";

const MATCHING_COMBINED_CONTRACT: &str = r#"contract_version: 2
root: ../src
files:
  - lib.rs
extraction:
  mode: rust_syntax_v2
  profile: rust_api_v1
  crates:
    - id: fixture
      root: lib.rs
      kind: library
signatures:
  - answer_function:
      file: lib.rs
      signature_type: function
      name: answer
      visibility: public
      parameters: []
      return_type: u8
      sketch: answer_body
sketches:
  - answer_body:
      file: lib.rs
      signature: answer_function
      signature_type: function
      matching:
        normalization: exact_lines_v1
        occurrence: at_least_one
      code: |
        pub fn answer() -> u8 { 42 }
"#;

const MISMATCHING_SKETCH_CONTRACT: &str = r#"contract_version: 2
root: ../src
files:
  - lib.rs
extraction:
  mode: rust_syntax_v2
  profile: rust_api_v1
  crates:
    - id: fixture
      root: lib.rs
      kind: library
signatures:
  - answer_function:
      file: lib.rs
      signature_type: function
      name: answer
      visibility: public
      parameters: []
      return_type: u8
      sketch: answer_body
sketches:
  - answer_body:
      file: lib.rs
      signature: answer_function
      signature_type: function
      matching:
        normalization: exact_lines_v1
        occurrence: at_least_one
      code: |
        pub fn answer() -> u8 { 43 }
"#;

const COMPILER_COMBINED_CONTRACT: &str = r#"contract_version: 2
root: ../src
files: [lib.rs]
extraction:
  mode: rust_compiler_v1
  profile: rust_api_v1
  crates: [{ id: fixture, root: lib.rs, kind: library }]
  compiler:
    artifact_schema_version: 1
    extractor_version: conkit-rustdoc-json-v1
    compiler_version: rustc-nightly
    rustdoc_format_version: 60
    target_triple: x86_64-unknown-linux-gnu
    package: fixture
    target: fixture
    features: []
    cfg_values: [target_arch="x86_64"]
    macro_expansion: true
    name_resolution: true
signatures: []
sketches: []
"#;

const MATCHING_SIGNATURE_YAML_REPORT: &str = r#"passed: true
source_shape_digest: 9c4a93d7bca1a78e3514645d0e6906d5e7fb1d68e16e8002155f8f800bc51525
digest_version: 2
counts:
  source_signature_count: 1
  contract_signature_count: 1
diagnostics: []
"#;

const MATCHING_SKETCH_JSON_REPORT: &str = r#"{
  "passed": true,
  "counts": {
    "source_catalog_entry_count": 1,
    "referenced_source_file_count": 1,
    "present_referenced_source_file_count": 1,
    "contract_document_count": 1,
    "sketch_count": 1,
    "matched_sketch_count": 1,
    "failed_sketch_count": 0
  },
  "diagnostics": []
}"#;

struct CheckFixture {
    source: assert_fs::fixture::ChildPath,
    contracts: assert_fs::fixture::ChildPath,
    output: assert_fs::fixture::ChildPath,
    _temp: assert_fs::TempDir,
}

impl CheckFixture {
    fn new() -> Self {
        let temp = assert_fs::TempDir::new().expect("temp dir");
        let source = temp.child("src");
        let contracts = temp.child("contracts");
        let output = temp.child("report.yml");
        source.create_dir_all().expect("source dir");
        contracts.create_dir_all().expect("contracts dir");
        source
            .child("lib.rs")
            .write_str(MATCHING_SOURCE)
            .expect("source file");

        Self {
            source,
            contracts,
            output,
            _temp: temp,
        }
    }

    fn write_contract(&self, name: &str, contents: &str) {
        self.contracts
            .child(name)
            .write_str(contents)
            .expect("contract document");
    }

    fn generate_signatures(&self) {
        ConkitCli::command()
            .args(["generate", "signatures", "--source"])
            .arg(self.source.path())
            .arg("--contracts")
            .arg(self.contracts.path())
            .assert()
            .success();
    }

    fn check(&self, target: &str) -> assert_cmd::Command {
        self.check_to(target, Some("--strict"), self.output.path())
    }

    fn check_to(
        &self,
        target: &str,
        mode: Option<&str>,
        output: &std::path::Path,
    ) -> assert_cmd::Command {
        let mut command = ConkitCli::command();
        command
            .args(["check", target, "--source"])
            .arg(self.source.path())
            .arg("--contracts")
            .arg(self.contracts.path())
            .arg("--output")
            .arg(output);
        if let Some(mode) = mode {
            command.arg(mode);
        }
        command
    }

    fn install_matrix_catalog(&self, matching: bool) {
        let contract = MATCHING_COMBINED_CONTRACT
            .replace("id: fixture", "id: sample")
            .replace("answer_function", "answer");
        self.write_contract("main.yml", &contract);
        self.source
            .child("lib.rs")
            .write_str(if matching {
                MATCHING_SOURCE
            } else {
                MISMATCHING_SOURCE
            })
            .expect("matrix source file");
    }

    fn run_matrix_case(&self, case: CheckMatrixCase) {
        self.install_matrix_catalog(case.matching);
        let output = self
            .check_to(
                case.target.argument(),
                case.mode.argument(),
                self.output.path(),
            )
            .output()
            .expect("run matrix check command");

        assert_eq!(
            output.status.success(),
            case.expected_success,
            "wrong exit status for {case:?}; stdout: {}; stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        self.output.assert(predicate::path::is_file());

        let report = std::fs::read(self.output.path()).expect("matrix report bytes");
        let report =
            serde_saphyr::from_slice::<serde_json::Value>(&report).expect("matrix report YAML");
        self.assert_matrix_report(case, &report);
    }

    fn assert_matrix_report(&self, case: CheckMatrixCase, report: &serde_json::Value) {
        assert_eq!(
            report["passed"].as_bool(),
            Some(case.expected_success),
            "wrong report pass state for {case:?}",
        );

        match case.target {
            CheckTargetCase::All => {
                assert!(report.get("counts").is_none(), "wrong all shape: {report}");
                assert!(
                    report.get("diagnostics").is_none(),
                    "wrong all shape: {report}"
                );
                let signatures = &report["signatures"];
                let sketches = &report["sketches"];
                assert!(signatures.is_object(), "missing signatures: {report}");
                assert!(sketches.is_object(), "missing sketches: {report}");
                assert_eq!(signatures["passed"].as_bool(), Some(case.expected_success));
                assert_eq!(sketches["passed"].as_bool(), Some(case.expected_success));
                assert_eq!(
                    signatures["diagnostics"].as_array().map(Vec::is_empty),
                    Some(case.matching)
                );
                assert_eq!(
                    sketches["diagnostics"].as_array().map(Vec::is_empty),
                    Some(case.matching)
                );
                assert!(signatures["source_shape_digest"].is_string());
                assert_eq!(signatures["digest_version"].as_u64(), Some(2));
                assert_eq!(
                    sketches["counts"]["source_catalog_entry_count"].as_u64(),
                    Some(1)
                );
            }
            CheckTargetCase::Signatures => {
                assert!(report.get("signatures").is_none(), "wrong shape: {report}");
                assert!(report.get("sketches").is_none(), "wrong shape: {report}");
                assert!(report["source_shape_digest"].is_string());
                assert_eq!(report["digest_version"].as_u64(), Some(2));
                assert_eq!(report["counts"]["source_signature_count"].as_u64(), Some(1));
                assert_eq!(
                    report["counts"]["contract_signature_count"].as_u64(),
                    Some(1)
                );
                assert!(report["counts"].get("sketch_count").is_none());
                assert_eq!(
                    report["diagnostics"].as_array().map(Vec::is_empty),
                    Some(case.matching)
                );
            }
            CheckTargetCase::Sketches => {
                assert!(report.get("signatures").is_none(), "wrong shape: {report}");
                assert!(report.get("sketches").is_none(), "wrong shape: {report}");
                assert!(report.get("source_shape_digest").is_none());
                assert!(report.get("digest_version").is_none());
                assert_eq!(
                    report["counts"]["source_catalog_entry_count"].as_u64(),
                    Some(1)
                );
                assert_eq!(report["counts"]["sketch_count"].as_u64(), Some(1));
                assert!(report["counts"].get("source_signature_count").is_none());
                assert_eq!(
                    report["diagnostics"].as_array().map(Vec::is_empty),
                    Some(case.matching)
                );
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum CheckTargetCase {
    All,
    Signatures,
    Sketches,
}

impl CheckTargetCase {
    const ALL: [Self; 3] = [Self::All, Self::Signatures, Self::Sketches];

    fn argument(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Signatures => "signatures",
            Self::Sketches => "sketches",
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum CheckModeCase {
    ExplicitDefault,
    Omitted,
    Strict,
    Warning,
}

impl CheckModeCase {
    const ALL: [Self; 4] = [
        Self::ExplicitDefault,
        Self::Omitted,
        Self::Strict,
        Self::Warning,
    ];

    fn argument(self) -> Option<&'static str> {
        match self {
            Self::ExplicitDefault => Some("--default"),
            Self::Omitted => None,
            Self::Strict => Some("--strict"),
            Self::Warning => Some("--warning"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct CheckMatrixCase {
    target: CheckTargetCase,
    mode: CheckModeCase,
    matching: bool,
    expected_success: bool,
}

#[test]
fn check_target_mode_and_outcome_matrix_has_all_24_behavioral_cases() {
    let mut case_count = 0;
    for target in CheckTargetCase::ALL {
        for mode in CheckModeCase::ALL {
            for matching in [true, false] {
                let case = CheckMatrixCase {
                    target,
                    mode,
                    matching,
                    expected_success: matching || matches!(mode, CheckModeCase::Warning),
                };
                CheckFixture::new().run_matrix_case(case);
                case_count += 1;
            }
        }
    }
    assert_eq!(case_count, 24);
}

#[test]
fn standalone_signature_yaml_and_sketch_json_reports_are_byte_exact() {
    let fixture = CheckFixture::new();
    fixture.install_matrix_catalog(true);

    fixture
        .check_to(
            CheckTargetCase::Signatures.argument(),
            CheckModeCase::ExplicitDefault.argument(),
            fixture.output.path(),
        )
        .assert()
        .success();
    assert_eq!(
        std::fs::read(fixture.output.path()).expect("signature YAML report"),
        MATCHING_SIGNATURE_YAML_REPORT.as_bytes(),
    );

    let sketch_json = fixture._temp.child("sketch-report.json");
    fixture
        .check_to(
            CheckTargetCase::Sketches.argument(),
            CheckModeCase::ExplicitDefault.argument(),
            sketch_json.path(),
        )
        .assert()
        .success();
    assert_eq!(
        std::fs::read(sketch_json.path()).expect("sketch JSON report"),
        MATCHING_SKETCH_JSON_REPORT.as_bytes(),
    );
}

#[test]
fn generated_signature_document_strict_checks_and_writes_report() {
    let fixture = CheckFixture::new();
    fixture.generate_signatures();

    let contract = std::fs::read_to_string(fixture.contracts.child("main.yml").path())
        .expect("generated combined contract");
    assert!(contract.starts_with("contract_version: 2\nroot: ../src\nfiles:\n"));
    assert!(contract.contains("extraction:"));
    assert!(contract.contains("signatures:"));
    assert!(contract.contains("sketches: []"));
    assert!(!contract.contains("version: 1"));
    assert!(!contract.contains("language:"));

    fixture
        .check("signatures")
        .assert()
        .success()
        .stdout(predicate::str::contains("signature check passed"))
        .stderr(predicate::str::is_empty());

    let report = std::fs::read_to_string(fixture.output.path()).expect("report");
    assert!(report.contains("passed: true"));
    assert!(report.contains("source_shape_digest:"));
    assert!(report.contains("digest_version: 2"));
}

#[test]
fn compiler_contract_rejects_the_default_syntax_extractor_before_domain_work() {
    let fixture = CheckFixture::new();
    fixture.write_contract("main.yml", COMPILER_COMBINED_CONTRACT);

    fixture
        .check("signatures")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "existing contracts require compiler extraction",
        ));
    fixture.output.assert(predicate::path::missing());
}

#[test]
fn compiler_aggregate_root_preflight_precedes_manifest_warning_and_report_publication() {
    let fixture = CheckFixture::new();
    fixture
        .source
        .child("other.rs")
        .write_str("pub fn other() {}\n")
        .expect("second compiler root source");
    fixture.write_contract("main.yml", TWO_ROOT_COMPILER_COMBINED_CONTRACT);
    let unusable_manifest = fixture._temp.child("unusable-manifest");
    unusable_manifest
        .create_dir_all()
        .expect("unusable manifest directory");

    fixture
        .check("signatures")
        .args(["--signature-extractor", "compiler", "--manifest-path"])
        .arg(unusable_manifest.path())
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(
            predicate::str::contains(
                "compiler extraction requires exactly one aggregate crate root, found 2",
            )
            .and(
                predicate::str::contains("warning: compiler signature extraction invokes Cargo")
                    .not(),
            )
            .and(predicate::str::contains("unusable-manifest").not()),
        );
    fixture.output.assert(predicate::path::missing());
}

#[test]
fn invalid_signature_options_precede_check_filesystem_validation() {
    let fixture = CheckFixture::new();
    let missing_source = fixture.source.child("missing-source");
    let missing_contracts = fixture.contracts.child("missing-contracts");
    let output = fixture.output.path().to_path_buf();

    for subject in ["all", "signatures"] {
        ConkitCli::command()
            .args(["check", subject, "--source"])
            .arg(missing_source.path())
            .arg("--contracts")
            .arg(missing_contracts.path())
            .arg("--output")
            .arg(&output)
            .args(["--manifest-path", "Cargo.toml"])
            .assert()
            .failure()
            .stdout(predicate::str::is_empty())
            .stderr(
                predicate::str::contains(
                    "Cargo selection options require `--signature-extractor compiler`",
                )
                .and(predicate::str::contains("missing-source").not())
                .and(predicate::str::contains("missing-contracts").not()),
            );
    }

    fixture.output.assert(predicate::path::missing());
}

#[test]
fn signature_mismatch_writes_a_failing_report_before_exit() {
    let fixture = CheckFixture::new();
    fixture.generate_signatures();
    fixture
        .source
        .child("lib.rs")
        .write_str("pub fn answer() -> u16 { 42 }\n")
        .expect("changed source");

    fixture
        .check("signatures")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("contract check failed"));

    let report = std::fs::read_to_string(fixture.output.path()).expect("report");
    assert!(report.contains("passed: false"));
    assert!(report.contains("Mismatched"));
}

#[test]
fn linked_sketch_strict_checks_in_the_combined_document() {
    let fixture = CheckFixture::new();
    fixture.write_contract("main.yml", MATCHING_COMBINED_CONTRACT);

    fixture
        .check("sketches")
        .assert()
        .success()
        .stdout(
            "sketch check passed: matched 1, failed 0, total 1; referenced sources present 1/1; catalog entries 1; contract documents 1\n",
        )
        .stderr(predicate::str::is_empty());

    let report = std::fs::read_to_string(fixture.output.path()).expect("report");
    assert!(report.contains("passed: true"));
    assert!(report.contains("source_catalog_entry_count: 1"));
    assert!(report.contains("referenced_source_file_count: 1"));
    assert!(report.contains("present_referenced_source_file_count: 1"));
    assert!(report.contains("contract_document_count: 1"));
    assert!(report.contains("sketch_count: 1"));
}

#[test]
fn linked_sketch_mismatch_writes_a_failing_report_before_exit() {
    let fixture = CheckFixture::new();
    fixture.write_contract("main.yml", MISMATCHING_SKETCH_CONTRACT);

    fixture
        .check("sketches")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("contract check failed"));

    let report = std::fs::read_to_string(fixture.output.path()).expect("report");
    assert!(report.contains("passed: false"));
    assert!(report.contains("NotMatched"));
}

#[test]
fn check_rejects_flattened_v2_sketches_and_nested_sketches_without_matching() {
    let valid_sketch = r#"sketches:
  - answer_body:
      file: lib.rs
      signature: answer_function
      signature_type: function
      matching:
        normalization: exact_lines_v1
        occurrence: at_least_one
      code: |
        pub fn answer() -> u8 { 42 }
"#;
    let flattened = MATCHING_COMBINED_CONTRACT.replace(
        valid_sketch,
        r#"sketches:
  - answer_body:
    file: lib.rs
    signature: answer_function
    signature_type: function
    matching:
      normalization: exact_lines_v1
      occurrence: at_least_one
    code: |
      pub fn answer() -> u8 { 42 }
"#,
    );
    let missing_matching = MATCHING_COMBINED_CONTRACT.replace(
        valid_sketch,
        r#"sketches:
  - answer_body:
      file: lib.rs
      signature: answer_function
      signature_type: function
      code: |
        pub fn answer() -> u8 { 42 }
"#,
    );
    assert_ne!(flattened, MATCHING_COMBINED_CONTRACT);
    assert_ne!(missing_matching, MATCHING_COMBINED_CONTRACT);

    for (name, contract) in [
        ("flattened.yml", flattened.as_str()),
        ("missing-matching.yml", missing_matching.as_str()),
    ] {
        let fixture = CheckFixture::new();
        fixture.write_contract(name, contract);

        fixture
            .check("sketches")
            .assert()
            .failure()
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::is_empty().not());
        fixture.output.assert(predicate::path::missing());
    }
}

#[test]
fn check_all_uses_one_combined_document_and_report() {
    let fixture = CheckFixture::new();
    fixture.write_contract("main.yml", MATCHING_COMBINED_CONTRACT);

    fixture
        .check("all")
        .assert()
        .success()
        .stdout("contract check passed: 1 signatures, 1 sketches\n")
        .stderr(predicate::str::is_empty());

    let report = std::fs::read_to_string(fixture.output.path()).expect("combined report");
    let report = serde_saphyr::from_str::<serde_json::Value>(&report)
        .expect("valid report YAML from maintained parser");
    assert!(report["signatures"].is_object());
    assert!(report["sketches"].is_object());
    assert_eq!(report["signatures"]["digest_version"], 2);
    assert!(report["signatures"]["source_shape_digest"].is_string());
    assert_eq!(
        report["sketches"]["counts"]["source_catalog_entry_count"].as_u64(),
        Some(1)
    );
    assert_eq!(
        report["sketches"]["counts"]["referenced_source_file_count"].as_u64(),
        Some(1)
    );
    assert_eq!(
        report["sketches"]["counts"]["present_referenced_source_file_count"].as_u64(),
        Some(1)
    );
    assert_eq!(
        report["sketches"]["counts"]["contract_document_count"].as_u64(),
        Some(1)
    );
    assert_eq!(
        report["sketches"]["counts"]["sketch_count"].as_u64(),
        Some(1)
    );
}

#[test]
fn check_rejects_legacy_and_future_contract_versions_without_fallback() {
    let fixture = CheckFixture::new();
    fixture.write_contract(
        "main.yml",
        r#"contract_version: 1
root: ../src
files: []
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [] }
signatures: []
sketches: []
"#,
    );

    fixture
        .check("all")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("recreate"));
    fixture.output.assert(predicate::path::missing());

    fixture.write_contract(
        "main.yml",
        r#"contract_version: 3
root: ../src
files: []
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [] }
signatures: []
sketches: []
"#,
    );
    fixture
        .check("all")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("contract_version 3"));
}

#[test]
fn mixed_case_yaml_and_json_extensions_are_supported() {
    let fixture = CheckFixture::new();
    fixture.write_contract("main.YmL", MATCHING_COMBINED_CONTRACT);
    let json_output = fixture._temp.child("report.JsOn");

    ConkitCli::command()
        .args(["check", "all", "--source"])
        .arg(fixture.source.path())
        .arg("--contracts")
        .arg(fixture.contracts.path())
        .arg("--output")
        .arg(json_output.path())
        .arg("--strict")
        .assert()
        .success();

    let report = std::fs::read_to_string(json_output.path()).expect("JSON report");
    let report = serde_json::from_str::<serde_json::Value>(&report).expect("valid JSON report");
    assert_eq!(report["signatures"]["passed"], true);
    assert_eq!(report["sketches"]["counts"]["sketch_count"], 1);
}

#[test]
fn multiple_documents_use_disjoint_exact_source_allowlists() {
    let fixture = CheckFixture::new();
    fixture
        .source
        .child("other.rs")
        .write_str("pub fn other() {}\n")
        .expect("second source");
    fixture
        .source
        .child("ignored.rs")
        .write_str("this is not valid Rust\n")
        .expect("unlisted source");
    fixture.write_contract(
        "answer.yml",
        r#"contract_version: 2
root: ../src
files: [lib.rs]
extraction:
  mode: rust_syntax_v2
  profile: rust_api_v1
  crates: [{ id: answer, root: lib.rs, kind: library }]
signatures:
  - answer_function:
      file: lib.rs
      signature_type: function
      name: answer
      visibility: public
      parameters: []
      return_type: u8
sketches: []
"#,
    );
    fixture.write_contract(
        "other.YAML",
        r#"contract_version: 2
root: ../src
files: [other.rs]
extraction:
  mode: rust_syntax_v2
  profile: rust_api_v1
  crates: [{ id: other, root: other.rs, kind: library }]
signatures:
  - other_function:
      file: other.rs
      signature_type: function
      name: other
      visibility: public
      parameters: []
sketches: []
"#,
    );

    fixture
        .check("signatures")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "2 source signatures, 2 contract signatures",
        ));
}

#[test]
fn missing_listed_source_fails_without_a_report() {
    let missing = CheckFixture::new();
    missing.write_contract(
        "main.yml",
        "contract_version: 2\nroot: ../src\nfiles: [missing.rs]\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: missing, root: missing.rs, kind: library }] }\nsignatures: []\nsketches: []\n",
    );
    missing
        .check("signatures")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("missing.rs"));
    missing.output.assert(predicate::path::missing());
}

#[test]
fn overlapping_document_allowlists_are_checked_as_independent_projections() {
    let overlap = CheckFixture::new();
    let matching = r#"contract_version: 2
root: ../src
files: [lib.rs]
extraction:
  mode: rust_syntax_v2
  profile: rust_api_v1
  crates: [{ id: fixture, root: lib.rs, kind: library }]
signatures:
  - answer_function:
      file: lib.rs
      signature_type: function
      name: answer
      visibility: public
      parameters: []
      return_type: u8
sketches: []
"#;
    overlap.write_contract("first.yml", matching);
    overlap.write_contract("second.yaml", matching);
    overlap
        .check("signatures")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "2 source signatures, 2 contract signatures",
        ));
    overlap.output.assert(predicate::path::is_file());
}

#[test]
fn report_persistence_failures_emit_no_success_stdout_for_any_target() {
    let fixture = CheckFixture::new();
    fixture.write_contract("main.yml", MATCHING_COMBINED_CONTRACT);

    for target in ["signatures", "sketches", "all"] {
        let blocked = fixture._temp.child(format!("{target}.yml"));
        blocked.create_dir_all().expect("blocking output directory");

        ConkitCli::command()
            .args(["check", target, "--source"])
            .arg(fixture.source.path())
            .arg("--contracts")
            .arg(fixture.contracts.path())
            .arg("--output")
            .arg(blocked.path())
            .arg("--strict")
            .assert()
            .failure()
            .stdout(predicate::str::is_empty());
    }
}

#[test]
fn check_rejects_overlapping_roots_and_report_paths_without_writing() {
    let temp = assert_fs::TempDir::new().expect("temp dir");
    let source = temp.child("project");
    let contracts = source.child("contracts");
    let outside_output = temp.child("outside.yml");
    source.create_dir_all().expect("source dir");
    contracts.create_dir_all().expect("contracts dir");

    ConkitCli::command()
        .args(["check", "signatures", "--source"])
        .arg(source.path())
        .arg("--contracts")
        .arg(contracts.path())
        .arg("--output")
        .arg(outside_output.path())
        .arg("--warning")
        .assert()
        .failure()
        .stderr(predicate::str::contains("overlap"));
    outside_output.assert(predicate::path::missing());
}
