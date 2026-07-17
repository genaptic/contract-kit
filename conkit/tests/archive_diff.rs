mod support;

use assert_fs::prelude::*;
use predicates::prelude::*;
use std::path::PathBuf;
use support::ConkitCli;

const COMBINED_CONTRACT: &str = r#"contract_version: 2
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

const CHANGED_COMBINED_CONTRACT: &str = r#"contract_version: 2
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
      return_type: u16
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
        pub fn answer() -> u16 { 43 }
"#;

const SKETCH_CHANGED_CONTRACT: &str = r#"contract_version: 2
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

struct ArchiveFixture {
    contracts: assert_fs::fixture::ChildPath,
    archive: assert_fs::fixture::ChildPath,
    _temp: assert_fs::TempDir,
}

impl ArchiveFixture {
    fn new() -> Self {
        let temp = assert_fs::TempDir::new().expect("temp dir");
        let contracts = temp.child("contracts");
        let archive = temp.child("archive");
        contracts.create_dir_all().expect("contracts dir");
        contracts
            .child("main.yml")
            .write_str(COMBINED_CONTRACT)
            .expect("combined contract");

        Self {
            contracts,
            archive,
            _temp: temp,
        }
    }

    fn replace_contract(&self, contents: &str) {
        self.contracts
            .child("main.yml")
            .write_str(contents)
            .expect("replace contract");
    }

    fn archive_contracts(&self) {
        ConkitCli::command()
            .args(["archive", "--contracts"])
            .arg(self.contracts.path())
            .arg("--archive")
            .arg(self.archive.path())
            .arg("--gzip")
            .assert()
            .success();
    }

    fn diff(&self) -> assert_cmd::Command {
        let mut command = ConkitCli::command();
        command
            .args(["diff", "--contracts"])
            .arg(self.contracts.path())
            .arg("--archive")
            .arg(self.only_archive_file());
        command
    }

    fn only_archive_file(&self) -> PathBuf {
        let archives = self.archive_files();
        assert_eq!(archives.len(), 1);
        archives.into_iter().next().expect("one archive file")
    }

    fn archive_files(&self) -> Vec<PathBuf> {
        let mut archives = std::fs::read_dir(self.archive.path())
            .expect("archive dir")
            .map(|entry| entry.expect("archive entry").path())
            .collect::<Vec<_>>();
        archives.sort();
        archives
    }
}

#[test]
fn archive_writes_one_windows_safe_gzip_for_a_combined_document() {
    let fixture = ArchiveFixture::new();

    ConkitCli::command()
        .args(["archive", "--contracts"])
        .arg(fixture.contracts.path())
        .arg("--archive")
        .arg(fixture.archive.path())
        .arg("--gzip")
        .assert()
        .success()
        .stdout(predicate::str::contains("archived contracts to"))
        .stderr(predicate::str::is_empty());

    let archives = fixture.archive_files();
    assert_eq!(archives.len(), 1);
    let name = archives[0]
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .expect("archive file name");
    assert!(name.ends_with("-archive.gzip"));
    assert!(!name.contains(':'));
}

#[test]
fn omitted_gzip_creates_the_destination_and_round_trips() {
    let fixture = ArchiveFixture::new();

    ConkitCli::command()
        .args(["archive", "--contracts"])
        .arg(fixture.contracts.path())
        .arg("--archive")
        .arg(fixture.archive.path())
        .assert()
        .success();

    fixture
        .diff()
        .assert()
        .success()
        .stdout(
            predicate::str::starts_with("contracts unchanged\n")
                .and(predicate::str::contains("signature contract digest v2 "))
                .and(predicate::str::contains("sketch contract digest v2 ")),
        )
        .stderr(predicate::str::is_empty());
}

#[test]
fn archive_twice_creates_two_distinct_archive_files() {
    let fixture = ArchiveFixture::new();
    fixture.archive_contracts();
    fixture.archive_contracts();

    let archives = fixture.archive_files();
    assert_eq!(archives.len(), 2);
    assert_ne!(archives[0], archives[1]);
    for archive in archives {
        let name = archive
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .expect("archive file name");
        assert!(name.ends_with("-archive.gzip"));
        assert!(!name.contains(':'));
    }
}

#[test]
fn diff_reports_unchanged_for_the_archived_combined_catalog() {
    let fixture = ArchiveFixture::new();
    fixture.archive_contracts();

    fixture
        .diff()
        .assert()
        .success()
        .stdout(
            predicate::str::starts_with("contracts unchanged\n")
                .and(predicate::str::contains("signature contract digest v2 "))
                .and(predicate::str::contains("sketch contract digest v2 ")),
        )
        .stderr(predicate::str::is_empty());
}

#[test]
fn diff_reports_signature_changes_by_user_label_without_failing() {
    let fixture = ArchiveFixture::new();
    fixture.archive_contracts();
    fixture.replace_contract(&CHANGED_COMBINED_CONTRACT.replace(
        "pub fn answer() -> u16 { 43 }",
        "pub fn answer() -> u8 { 42 }",
    ));

    fixture
        .diff()
        .assert()
        .success()
        .stdout(
            predicate::str::starts_with("contracts changed\n")
                .and(predicate::str::contains("signature contract digest v2 "))
                .and(predicate::str::contains(
                    "signature changed answer_function ",
                ))
                .and(predicate::str::contains("[source_semantics]")),
        )
        .stderr(predicate::str::is_empty());
}

#[test]
fn diff_reports_signatures_before_sketches_and_changed_comparisons_exit_zero() {
    let fixture = ArchiveFixture::new();
    fixture.archive_contracts();
    fixture.replace_contract(CHANGED_COMBINED_CONTRACT);

    fixture
        .diff()
        .assert()
        .success()
        .stdout(
            predicate::str::starts_with("contracts changed\n")
                .and(predicate::str::contains(
                    "signature changed answer_function ",
                ))
                .and(predicate::str::contains("sketch changed answer_body [code]"))
                .and(predicate::str::contains(
                    "sketch previous answer_body at main.yml document 0; source lib.rs; signature answer_function (function); matching exact_lines_v1/at_least_one; code ",
                ))
                .and(predicate::str::contains(
                    "sketch current answer_body at main.yml document 0; source lib.rs; signature answer_function (function); matching exact_lines_v1/at_least_one; code ",
                )),
        )
        .stderr(predicate::str::is_empty());
}

#[test]
fn diff_reports_sketch_only_semantic_changes() {
    let fixture = ArchiveFixture::new();
    fixture.archive_contracts();
    fixture.replace_contract(SKETCH_CHANGED_CONTRACT);

    fixture
        .diff()
        .assert()
        .success()
        .stdout(
            predicate::str::starts_with("contracts changed\n")
                .and(predicate::str::contains(
                    "sketch changed answer_body [code]",
                ))
                .and(predicate::str::contains("sketch previous answer_body"))
                .and(predicate::str::contains("sketch current answer_body")),
        )
        .stderr(predicate::str::is_empty());
}

#[test]
fn diff_ignores_document_relocation_and_mapping_order() {
    let fixture = ArchiveFixture::new();
    fixture.archive_contracts();
    std::fs::remove_file(fixture.contracts.child("main.yml").path()).expect("remove original");
    fixture
        .contracts
        .child("relocated.YAML")
        .write_str(
            r#"contract_version: 2
files: [lib.rs]
root: ../src
extraction:
  mode: rust_syntax_v2
  profile: rust_api_v1
  crates: [{ id: fixture, root: lib.rs, kind: library }]
sketches:
  - answer_body:
      code: |
        pub fn answer() -> u8 { 42 }
      matching: { occurrence: at_least_one, normalization: exact_lines_v1 }
      signature_type: function
      signature: answer_function
      file: lib.rs
signatures:
  - answer_function:
      return_type: u8
      parameters: []
      visibility: public
      name: answer
      signature_type: function
      file: lib.rs
      sketch: answer_body
"#,
        )
        .expect("relocated combined contract");

    fixture
        .diff()
        .assert()
        .success()
        .stdout(
            predicate::str::starts_with("contracts unchanged\n")
                .and(predicate::str::contains("signature contract digest v2 "))
                .and(predicate::str::contains("sketch contract digest v2 ")),
        )
        .stderr(predicate::str::is_empty());
}

#[test]
fn archive_rejects_legacy_and_future_contracts_with_recreation_guidance() {
    let fixture = ArchiveFixture::new();
    fixture.replace_contract(
        "contract_version: 1\nroot: .\nfiles: []\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [] }\nsignatures: []\nsketches: []\n",
    );

    ConkitCli::command()
        .args(["archive", "--contracts"])
        .arg(fixture.contracts.path())
        .arg("--archive")
        .arg(fixture.archive.path())
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("recreate"));
    fixture.archive.assert(predicate::path::missing());

    fixture.replace_contract(
        "contract_version: 3\nroot: .\nfiles: []\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [] }\nsignatures: []\nsketches: []\n",
    );
    ConkitCli::command()
        .args(["archive", "--contracts"])
        .arg(fixture.contracts.path())
        .arg("--archive")
        .arg(fixture.archive.path())
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("contract_version 3"));
}

#[test]
fn archive_rejects_a_lexically_invalid_root_before_publication() {
    let fixture = ArchiveFixture::new();
    fixture.replace_contract(&COMBINED_CONTRACT.replace("root: ../src", "root: /absolute"));

    ConkitCli::command()
        .args(["archive", "--contracts"])
        .arg(fixture.contracts.path())
        .arg("--archive")
        .arg(fixture.archive.path())
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "contract root must be a nonempty relative path",
        ));
    fixture.archive.assert(predicate::path::missing());
}

#[test]
fn diff_rejects_a_wire_v1_archive_containing_legacy_contracts() {
    let fixture = ArchiveFixture::new();
    let archive =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/archive-v1/mixed-v1.gzip");

    ConkitCli::command()
        .args(["diff", "--contracts"])
        .arg(fixture.contracts.path())
        .arg("--archive")
        .arg(archive)
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("recreate"));
}

#[test]
fn archive_rejects_one_legacy_document_inside_a_mixed_stream() {
    let fixture = ArchiveFixture::new();
    let mixed = format!(
        "{COMBINED_CONTRACT}---\ncontract_version: 1\nroot: ../src\nfiles: []\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [] }}\nsignatures: []\nsketches: []\n"
    );
    fixture.replace_contract(&mixed);

    ConkitCli::command()
        .args(["archive", "--contracts"])
        .arg(fixture.contracts.path())
        .arg("--archive")
        .arg(fixture.archive.path())
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("document index 1"))
        .stderr(predicate::str::contains("recreate"));
}

#[test]
fn archive_rejects_contract_and_archive_root_overlap_without_writing() {
    let fixture = ArchiveFixture::new();
    let nested_archive = fixture.contracts.child("archives");

    ConkitCli::command()
        .args(["archive", "--contracts"])
        .arg(fixture.contracts.path())
        .arg("--archive")
        .arg(nested_archive.path())
        .arg("--gzip")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("overlap"));

    nested_archive.assert(predicate::path::missing());
}

#[test]
fn diff_rejects_an_archive_file_inside_contracts_without_mutation() {
    let fixture = ArchiveFixture::new();
    fixture.archive_contracts();
    let archive_inside_contracts = fixture.contracts.child("snapshot.gzip");
    std::fs::copy(fixture.only_archive_file(), archive_inside_contracts.path())
        .expect("copy archive into contracts");
    let contract_before =
        std::fs::read(fixture.contracts.child("main.yml").path()).expect("contract before");

    ConkitCli::command()
        .args(["diff", "--contracts"])
        .arg(fixture.contracts.path())
        .arg("--archive")
        .arg(archive_inside_contracts.path())
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("overlap"));

    assert_eq!(
        std::fs::read(fixture.contracts.child("main.yml").path()).expect("contract after"),
        contract_before
    );
}

#[test]
fn corrupt_archive_failure_leaves_current_contract_bytes_unchanged() {
    let fixture = ArchiveFixture::new();
    fixture.archive.create_dir_all().expect("archive dir");
    let corrupt = fixture.archive.child("corrupt-archive.gzip");
    corrupt.write_binary(b"not gzip").expect("corrupt archive");
    let before = std::fs::read(fixture.contracts.child("main.yml").path()).expect("before");

    ConkitCli::command()
        .args(["diff", "--contracts"])
        .arg(fixture.contracts.path())
        .arg("--archive")
        .arg(corrupt.path())
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "failed to decode contract archive",
        ));

    assert_eq!(
        std::fs::read(fixture.contracts.child("main.yml").path()).expect("after"),
        before
    );
}
