mod support;

use assert_fs::prelude::*;
use predicates::prelude::*;
use std::path::PathBuf;
use support::ConkitCli;

const COMBINED_CONTRACT: &str = r#"root: ../src
files: [lib.rs]
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

const CHANGED_COMBINED_CONTRACT: &str = r#"root: ../src
files: [lib.rs]
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
    signature_type: function
    code: |
      pub fn answer() -> u16 { 43 }
"#;

const SKETCH_CHANGED_CONTRACT: &str = r#"root: ../src
files: [lib.rs]
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
        .stdout("contracts unchanged\n")
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
        .stdout("contracts unchanged\n")
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
        .stdout("contracts changed\nsignature changed answer_function\n")
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
        .stdout(concat!(
            "contracts changed\n",
            "signature changed answer_function\n",
            "sketch changed answer_body\n",
        ))
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
        .stdout("contracts changed\nsketch changed answer_body\n")
        .stderr(predicate::str::is_empty());
}

#[test]
fn diff_ignores_document_relocation_mapping_order_and_sketch_whitespace() {
    let fixture = ArchiveFixture::new();
    fixture.archive_contracts();
    std::fs::remove_file(fixture.contracts.child("main.yml").path()).expect("remove original");
    fixture
        .contracts
        .child("relocated.YAML")
        .write_str(
            r#"files: [lib.rs]
root: ../src
sketches:
  - answer_body:
    code: "  pub   fn answer() -> u8 { 42 }  "
    signature_type: function
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
        .stdout("contracts unchanged\n")
        .stderr(predicate::str::is_empty());
}

#[test]
fn diff_rejects_the_later_contract_dialect_from_an_archive() {
    let fixture = ArchiveFixture::new();
    fixture.replace_contract("version: 1\nlanguage: rust\nsignatures: []\nsketches: []\n");
    fixture.archive_contracts();
    fixture.replace_contract(COMBINED_CONTRACT);

    fixture
        .diff()
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("unknown field `version`"));
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
