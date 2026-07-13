mod support;

use assert_fs::prelude::*;
use predicates::prelude::*;
use serde_yaml::Value;
use sha2::{Digest, Sha256};
use support::ConkitCli;

struct GenerateFixture {
    source: assert_fs::fixture::ChildPath,
    contracts: assert_fs::fixture::ChildPath,
    _temp: assert_fs::TempDir,
}

impl GenerateFixture {
    fn new() -> Self {
        let temp = assert_fs::TempDir::new().expect("temp dir");
        let source = temp.child("src");
        let contracts = temp.child("contracts");
        source.create_dir_all().expect("source dir");
        source
            .child("lib.rs")
            .write_str("pub fn answer() -> u8 { 42 }\n")
            .expect("source file");

        Self {
            source,
            contracts,
            _temp: temp,
        }
    }

    fn generate(&self, target: &str) -> assert_cmd::Command {
        let mut command = ConkitCli::command();
        command
            .args(["generate", target, "--source"])
            .arg(self.source.path())
            .arg("--contracts")
            .arg(self.contracts.path());
        command
    }

    fn generate_successfully(&self, target: &str) {
        self.generate(target).assert().success();
    }

    fn contract_path(&self) -> std::path::PathBuf {
        self.contracts.child("main.yml").path().to_path_buf()
    }

    fn ownership_path(&self) -> std::path::PathBuf {
        self.contracts
            .child(".contract-kit/generated-files.json")
            .path()
            .to_path_buf()
    }

    fn contract_text(&self) -> String {
        std::fs::read_to_string(self.contract_path()).expect("combined contract")
    }

    fn ownership_json(&self) -> serde_json::Value {
        let bytes = std::fs::read(self.ownership_path()).expect("ownership metadata");
        serde_json::from_slice(&bytes).expect("valid ownership JSON")
    }

    fn install_owned_answer_sketch(&self, code: &str) {
        let mut document =
            serde_yaml::from_str::<Value>(&self.contract_text()).expect("generated document YAML");
        let mapping = document.as_mapping_mut().expect("document mapping");
        let signatures = mapping
            .get_mut(Value::String("signatures".to_owned()))
            .and_then(Value::as_sequence_mut)
            .expect("signature list");
        let signature = signatures[0]
            .as_mapping_mut()
            .and_then(|entry| entry.values_mut().next())
            .and_then(Value::as_mapping_mut)
            .expect("first signature body");
        signature.insert(
            Value::String("sketch".to_owned()),
            Value::String("answer_body".to_owned()),
        );

        let sketch = serde_yaml::Mapping::from_iter([
            (Value::String("answer_body".to_owned()), Value::Null),
            (
                Value::String("signature_type".to_owned()),
                Value::String("function".to_owned()),
            ),
            (
                Value::String("code".to_owned()),
                Value::String(code.to_owned()),
            ),
        ]);
        mapping.insert(
            Value::String("sketches".to_owned()),
            Value::Sequence(vec![Value::Mapping(sketch)]),
        );
        let bytes = serde_yaml::to_string(&document)
            .expect("render linked document")
            .into_bytes();
        std::fs::write(self.contract_path(), &bytes).expect("write linked document");

        let mut ownership = self.ownership_json();
        let digest = format!("{:x}", Sha256::digest(&bytes));
        ownership["journal"]["files"]["documents"][0]["sha256"] = serde_json::Value::String(digest);
        let bytes = serde_json::to_vec_pretty(&ownership).expect("render ownership");
        std::fs::write(self.ownership_path(), bytes).expect("update owned digest");
    }
}

#[test]
fn fresh_signature_generation_writes_one_original_style_document() {
    let fixture = GenerateFixture::new();

    fixture
        .generate("signatures")
        .assert()
        .success()
        .stdout("generated 1 signature contracts\n")
        .stderr(predicate::str::is_empty());

    let contract = fixture.contract_text();
    assert!(contract.starts_with("root: ../src\nfiles:\n"));
    assert!(contract.contains("- lib.rs"));
    assert!(contract.contains("signatures:"));
    assert!(contract.contains("answer_function:"));
    assert!(contract.contains("sketches: []"));
    assert!(!contract.contains("version:"));
    assert!(!contract.contains("language:"));
}

#[test]
fn fresh_all_generation_creates_signatures_with_zero_opt_in_sketches() {
    let fixture = GenerateFixture::new();

    fixture
        .generate("all")
        .assert()
        .success()
        .stdout("generated 1 signature contracts and 0 sketch contracts\n")
        .stderr(predicate::str::is_empty());

    assert!(fixture.contract_text().contains("sketches: []"));

    let report = fixture._temp.child("strict.yml");
    ConkitCli::command()
        .args(["check", "all", "--source"])
        .arg(fixture.source.path())
        .arg("--contracts")
        .arg(fixture.contracts.path())
        .arg("--output")
        .arg(report.path())
        .arg("--strict")
        .assert()
        .success()
        .stdout("contract check passed: 1 signatures, 0 sketches\n");
}

#[test]
fn sketch_generation_requires_and_refreshes_only_explicit_links() {
    let fixture = GenerateFixture::new();
    fixture.generate_successfully("signatures");
    fixture.install_owned_answer_sketch("stale body");

    fixture
        .generate("sketches")
        .assert()
        .success()
        .stdout("generated 1 sketch contracts\n")
        .stderr(predicate::str::is_empty());

    let contract = fixture.contract_text();
    assert!(contract.contains("answer_function:"));
    assert!(contract.contains("sketch: answer_body"));
    assert!(contract.contains("pub fn answer() -> u8 { 42 }"));
    assert!(!contract.contains("stale body"));

    let fresh = GenerateFixture::new();
    fresh
        .generate("sketches")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "no root-level .yml or .yaml contract documents",
        ));
    fresh
        .contracts
        .child("main.yml")
        .assert(predicate::path::missing());
}

#[test]
fn signature_generation_preserves_the_linked_sketch_section() {
    let fixture = GenerateFixture::new();
    fixture.generate_successfully("signatures");
    fixture.install_owned_answer_sketch("user-authored sketch body");
    let before = serde_yaml::from_str::<Value>(&fixture.contract_text()).expect("before YAML");

    fixture
        .source
        .child("lib.rs")
        .write_str("pub fn answer() -> u8 { 43 }\n")
        .expect("implementation-only change");
    fixture.generate_successfully("signatures");

    let after = serde_yaml::from_str::<Value>(&fixture.contract_text()).expect("after YAML");
    assert_eq!(before["sketches"], after["sketches"]);
    assert!(
        fixture
            .contract_text()
            .contains("user-authored sketch body")
    );
}

#[test]
fn all_generation_refreshes_existing_links_without_creating_new_ones() {
    let fixture = GenerateFixture::new();
    fixture.generate_successfully("signatures");
    fixture.install_owned_answer_sketch("old implementation");
    fixture
        .source
        .child("lib.rs")
        .write_str("pub fn answer() -> u8 { 43 }\n")
        .expect("implementation change");

    fixture
        .generate("all")
        .assert()
        .success()
        .stdout("generated 1 signature contracts and 1 sketch contracts\n");

    let contract = fixture.contract_text();
    assert!(contract.contains("pub fn answer() -> u8 { 43 }"));
    assert!(!contract.contains("old implementation"));
    assert_eq!(contract.matches("answer_body").count(), 2);
}

#[test]
fn all_generation_removes_a_stale_signature_and_its_linked_sketch_together() {
    let fixture = GenerateFixture::new();
    fixture.generate_successfully("signatures");
    fixture.install_owned_answer_sketch("pub fn answer() -> u8 { 42 }");
    fixture
        .source
        .child("lib.rs")
        .write_str("pub fn replacement() {}\n")
        .expect("replacement source");

    fixture
        .generate("all")
        .assert()
        .success()
        .stdout("generated 1 signature contracts and 0 sketch contracts\n");

    let contract = fixture.contract_text();
    assert!(contract.contains("replacement_function:"));
    assert!(contract.contains("sketches: []"));
    assert!(!contract.contains("answer_body"));
}

#[test]
fn repeated_generation_is_deterministic_and_uses_document_ownership_v3() {
    let fixture = GenerateFixture::new();
    fixture
        .contracts
        .create_dir_all()
        .expect("contracts directory");
    fixture
        .contracts
        .child("manual.txt")
        .write_str("user file\n")
        .expect("manual file");
    fixture.generate_successfully("all");
    let first_contract = std::fs::read(fixture.contract_path()).expect("first contract");
    let first = fixture.ownership_json();

    fixture.generate_successfully("all");
    let second_contract = std::fs::read(fixture.contract_path()).expect("second contract");
    let second = fixture.ownership_json();

    assert_eq!(first_contract, second_contract);
    assert_eq!(first["version"], 3);
    assert_eq!(first["journal"]["generation"], 1);
    assert_eq!(second["journal"]["generation"], 2);
    assert_eq!(
        second["journal"]["files"]["documents"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        second["journal"]["files"]["documents"][0]["path"],
        "main.yml"
    );
    fixture.contracts.child("manual.txt").assert("user file\n");
}

#[test]
fn ownership_cannot_claim_a_non_document_user_file() {
    let fixture = GenerateFixture::new();
    fixture.generate_successfully("signatures");
    fixture
        .contracts
        .child("manual.txt")
        .write_str("user file\n")
        .expect("manual file");
    let contract_before =
        std::fs::read(fixture.contract_path()).expect("combined contract before failure");

    let mut ownership = fixture.ownership_json();
    ownership["journal"]["files"]["documents"]
        .as_array_mut()
        .expect("owned documents")
        .push(serde_json::json!({
            "path": "manual.txt",
            "sha256": format!("{:x}", Sha256::digest(b"user file\n")),
        }));
    let mut ownership_before =
        serde_json::to_vec_pretty(&ownership).expect("render forged ownership");
    ownership_before.push(b'\n');
    std::fs::write(fixture.ownership_path(), &ownership_before).expect("install forged ownership");

    fixture
        .generate("signatures")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "owned document path manual.txt must be a direct root .yml or .yaml combined document",
        ));

    assert_eq!(
        std::fs::read(fixture.contract_path()).expect("combined contract after failure"),
        contract_before,
    );
    fixture.contracts.child("manual.txt").assert("user file\n");
    assert_eq!(
        std::fs::read(fixture.ownership_path()).expect("ownership after failure"),
        ownership_before,
    );
}

#[test]
fn generation_recovers_a_reserved_document_before_parsing_contracts() {
    let fixture = GenerateFixture::new();
    let metadata = fixture.contracts.child(".contract-kit");
    metadata
        .create_dir_all()
        .expect("ownership metadata directory");
    let digest = "0".repeat(64);
    let manifest = serde_json::json!({
        "version": 3,
        "journal": {
            "state": "updating",
            "generation": 1,
            "before": { "documents": [] },
            "after": {
                "documents": [{
                    "path": "main.yml",
                    "sha256": digest,
                }],
            },
        },
    });
    let mut manifest_bytes =
        serde_json::to_vec_pretty(&manifest).expect("render updating ownership");
    manifest_bytes.push(b'\n');
    metadata
        .child("generated-files.json")
        .write_binary(&manifest_bytes)
        .expect("updating ownership");
    let reservation = format!(
        "{{\"version\":3,\"generation\":1,\"path\":\"main.yml\",\"sha256\":\"{}\"}}\n",
        "0".repeat(64),
    );
    fixture
        .contracts
        .child("main.yml")
        .write_binary(reservation.as_bytes())
        .expect("reserved combined document");

    fixture
        .generate("signatures")
        .assert()
        .success()
        .stdout("generated 1 signature contracts\n")
        .stderr(predicate::str::is_empty());

    assert!(
        fixture
            .contract_text()
            .starts_with("root: ../src\nfiles:\n")
    );
    let ownership = fixture.ownership_json();
    assert_eq!(ownership["journal"]["state"], "committed");
    assert_eq!(ownership["journal"]["generation"], 2);
    assert_eq!(
        ownership["journal"]["files"]["documents"][0]["path"],
        "main.yml"
    );
}

#[test]
fn matching_document_adoption_is_idempotent() {
    let fixture = GenerateFixture::new();
    let expected = fixture._temp.child("expected");
    ConkitCli::command()
        .args(["generate", "all", "--source"])
        .arg(fixture.source.path())
        .arg("--contracts")
        .arg(expected.path())
        .assert()
        .success();
    fixture.contracts.create_dir_all().expect("contracts dir");
    std::fs::copy(expected.child("main.yml").path(), fixture.contract_path())
        .expect("copy matching document");

    fixture
        .generate("all")
        .arg("--adopt-existing")
        .assert()
        .success()
        .stdout(concat!(
            "generated 1 signature contracts and 0 sketch contracts\n",
            "adopted 1 matching existing contract files\n",
        ));
    fixture
        .generate("all")
        .arg("--adopt-existing")
        .assert()
        .success()
        .stdout(concat!(
            "generated 1 signature contracts and 0 sketch contracts\n",
            "adopted 0 matching existing contract files\n",
        ));
}

#[test]
fn unowned_and_mismatching_adoption_collisions_preserve_user_bytes() {
    for adopt in [false, true] {
        let fixture = GenerateFixture::new();
        fixture.contracts.create_dir_all().expect("contracts dir");
        fixture
            .contracts
            .child("main.yml")
            .write_str("root: ../src\nfiles: [lib.rs]\nsignatures: []\nsketches: []\n")
            .expect("user document");
        let before = std::fs::read(fixture.contract_path()).expect("before");
        let mut command = fixture.generate("signatures");
        if adopt {
            command.arg("--adopt-existing");
        }

        command
            .assert()
            .failure()
            .stdout(predicate::str::is_empty());

        assert_eq!(
            std::fs::read(fixture.contract_path()).expect("after"),
            before
        );
        fixture
            .contracts
            .child(".contract-kit/generated-files.json")
            .assert(predicate::path::missing());
    }
}

#[test]
fn modified_owned_document_and_invalid_v2_ownership_fail_without_mutation() {
    let modified = GenerateFixture::new();
    modified.generate_successfully("signatures");
    let mut document = modified.contract_text();
    document.push_str("# user edit\n");
    std::fs::write(modified.contract_path(), document.as_bytes()).expect("modify owned document");
    let ownership_before = std::fs::read(modified.ownership_path()).expect("ownership before");
    modified
        .generate("signatures")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("modified"));
    assert_eq!(modified.contract_text(), document);
    assert_eq!(
        std::fs::read(modified.ownership_path()).expect("ownership after"),
        ownership_before
    );

    let invalid = GenerateFixture::new();
    invalid.generate_successfully("signatures");
    let contract_before = std::fs::read(invalid.contract_path()).expect("contract before");
    let mut ownership = invalid.ownership_json();
    ownership["version"] = 2.into();
    let invalid_bytes = serde_json::to_vec_pretty(&ownership).expect("invalid ownership");
    std::fs::write(invalid.ownership_path(), &invalid_bytes).expect("write v2 ownership");
    invalid
        .generate("signatures")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("version 2"));
    assert_eq!(
        std::fs::read(invalid.contract_path()).expect("contract after"),
        contract_before
    );
    assert_eq!(
        std::fs::read(invalid.ownership_path()).expect("ownership after"),
        invalid_bytes
    );
}

#[test]
fn invalid_rust_in_all_generation_creates_no_document_or_ownership() {
    let fixture = GenerateFixture::new();
    fixture
        .source
        .child("lib.rs")
        .write_str("pub fn broken(\n")
        .expect("invalid Rust");

    fixture
        .generate("all")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty());

    fixture
        .contracts
        .child("main.yml")
        .assert(predicate::path::missing());
    fixture
        .contracts
        .child(".contract-kit")
        .assert(predicate::path::missing());
}

#[test]
fn equal_and_nested_roots_are_rejected_without_generation() {
    let temp = assert_fs::TempDir::new().expect("temp dir");

    for case in [
        "equal",
        "contracts-inside-source",
        "source-inside-contracts",
    ] {
        let root = temp.child(case).child("outer");
        root.create_dir_all().expect("outer root");
        let (source, contracts) = match case {
            "equal" => (root.path().to_path_buf(), root.path().to_path_buf()),
            "contracts-inside-source" => (root.path().to_path_buf(), root.path().join("contracts")),
            "source-inside-contracts" => (root.path().join("source"), root.path().to_path_buf()),
            _ => unreachable!("known case"),
        };
        std::fs::create_dir_all(&source).expect("source root");
        std::fs::write(source.join("lib.rs"), "pub fn answer() {}\n").expect("source file");

        ConkitCli::command()
            .args(["generate", "all", "--source"])
            .arg(&source)
            .arg("--contracts")
            .arg(&contracts)
            .assert()
            .failure()
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::contains("overlap"));

        assert!(!contracts.join("main.yml").exists());
        assert!(!contracts.join(".contract-kit").exists());
    }
}

#[cfg(unix)]
#[test]
fn dangling_combined_document_symlink_is_rejected_without_ownership() {
    use std::os::unix::fs::symlink;

    let fixture = GenerateFixture::new();
    fixture.contracts.create_dir_all().expect("contracts dir");
    let dangling = fixture.contracts.child("main.yml");
    symlink(fixture._temp.child("missing.yml").path(), dangling.path()).expect("dangling link");

    fixture
        .generate("signatures")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("symbolic link"));

    fixture
        .contracts
        .child(".contract-kit/generated-files.json")
        .assert(predicate::path::missing());
    assert!(
        std::fs::symlink_metadata(dangling.path())
            .expect("link remains")
            .file_type()
            .is_symlink()
    );
}
