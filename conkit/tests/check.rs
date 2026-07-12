mod support;

use assert_fs::prelude::*;
use predicates::prelude::*;
use support::ConkitCli;

const MATCHING_COMBINED_CONTRACT: &str = r#"root: ../src
files:
  - lib.rs
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
    signature_type: function
    code: |
      pub fn answer() -> u8 { 42 }
"#;

const MISMATCHING_SKETCH_CONTRACT: &str = r#"root: ../src
files:
  - lib.rs
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
    signature_type: function
    code: |
      pub fn answer() -> u8 { 43 }
"#;

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
            .write_str("pub fn answer() -> u8 { 42 }\n")
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
        let mut command = ConkitCli::command();
        command
            .args(["check", target, "--source"])
            .arg(self.source.path())
            .arg("--contracts")
            .arg(self.contracts.path())
            .arg("--output")
            .arg(self.output.path())
            .arg("--strict");
        command
    }
}

#[test]
fn generated_signature_document_strict_checks_and_writes_report() {
    let fixture = CheckFixture::new();
    fixture.generate_signatures();

    let contract = std::fs::read_to_string(fixture.contracts.child("main.yml").path())
        .expect("generated combined contract");
    assert!(contract.starts_with("root: ../src\nfiles:\n"));
    assert!(contract.contains("signatures:"));
    assert!(contract.contains("sketches: []"));
    assert!(!contract.contains("version:"));
    assert!(!contract.contains("language:"));

    fixture
        .check("signatures")
        .assert()
        .success()
        .stdout(predicate::str::contains("signature check passed"))
        .stderr(predicate::str::is_empty());

    let report = std::fs::read_to_string(fixture.output.path()).expect("report");
    assert!(report.contains("passed: true"));
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
        .stdout("sketch check passed: 1 matched sketches, 0 failed sketches\n")
        .stderr(predicate::str::is_empty());

    let report = std::fs::read_to_string(fixture.output.path()).expect("report");
    assert!(report.contains("passed: true"));
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
    let report = serde_yaml::from_str::<serde_yaml::Value>(&report).expect("valid report YAML");
    assert!(report["signatures"].is_mapping());
    assert!(report["sketches"].is_mapping());
    assert_eq!(
        report["sketches"]["counts"]["sketch_count"].as_u64(),
        Some(1)
    );
}

#[test]
fn check_rejects_the_later_version_and_language_dialect() {
    let fixture = CheckFixture::new();
    fixture.write_contract(
        "main.yml",
        r#"version: 1
language: rust
signatures: []
sketches: []
"#,
    );

    fixture
        .check("all")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("unknown field `version`"));
    fixture.output.assert(predicate::path::missing());
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
        r#"root: ../src
files: [lib.rs]
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
        r#"root: ../src
files: [other.rs]
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
fn missing_listed_source_and_overlapping_file_claims_fail_without_reports() {
    let missing = CheckFixture::new();
    missing.write_contract(
        "main.yml",
        "root: ../src\nfiles: [missing.rs]\nsignatures: []\nsketches: []\n",
    );
    missing
        .check("signatures")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("missing.rs"));
    missing.output.assert(predicate::path::missing());

    let overlap = CheckFixture::new();
    let empty = "root: ../src\nfiles: [lib.rs]\nsignatures: []\nsketches: []\n";
    overlap.write_contract("first.yml", empty);
    overlap.write_contract("second.yaml", empty);
    overlap
        .check("signatures")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("overlaps"));
    overlap.output.assert(predicate::path::missing());
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
