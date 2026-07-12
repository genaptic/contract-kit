mod support;

use assert_fs::prelude::*;
use predicates::prelude::*;
use support::ConkitCli;

#[test]
fn filesystem_paths_with_spaces_and_non_ascii_work() {
    let temp = assert_fs::TempDir::new().expect("temp dir");
    let source = temp.child("source dir Ω");
    let contracts = temp.child("contracts dir Ω");
    let output = temp.child("output dir Ω").child("output.yml");
    source.create_dir_all().expect("source dir");
    source
        .child("lib.rs")
        .write_str("pub fn answer() -> u8 { 42 }\n")
        .expect("source file");

    ConkitCli::command()
        .args(["generate", "signatures", "--source"])
        .arg(source.path())
        .arg("--contracts")
        .arg(contracts.path())
        .assert()
        .success();

    ConkitCli::command()
        .args(["check", "signatures", "--source"])
        .arg(source.path())
        .arg("--contracts")
        .arg(contracts.path())
        .arg("--output")
        .arg(output.path())
        .arg("--strict")
        .assert()
        .success();

    output.assert(predicate::path::exists());
}

#[test]
fn non_directory_roots_fail_with_clear_error() {
    let temp = assert_fs::TempDir::new().expect("temp dir");
    let missing_source = temp.child("missing-source");
    let contracts = temp.child("contracts");
    let output = temp.child("output.yml");
    contracts.create_dir_all().expect("contracts dir");

    ConkitCli::command()
        .args(["check", "signatures", "--source"])
        .arg(missing_source.path())
        .arg("--contracts")
        .arg(contracts.path())
        .arg("--output")
        .arg(output.path())
        .arg("--strict")
        .assert()
        .failure()
        .stderr(predicate::str::contains("root is not a directory"));
}
