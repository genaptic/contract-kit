mod support;

use std::collections::BTreeMap;

use assert_fs::prelude::*;
use predicates::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use support::{ConkitCli, TWO_ROOT_COMPILER_COMBINED_CONTRACT};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OwnedAnswerContract {
    contract_version: u32,
    root: String,
    files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    extraction: Option<OwnedExtraction>,
    signatures: Vec<BTreeMap<String, OwnedAnswerSignature>>,
    sketches: Vec<BTreeMap<String, OwnedAnswerSketch>>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct OwnedExtraction {
    mode: String,
    profile: String,
    crates: Vec<OwnedCrate>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct OwnedCrate {
    id: String,
    root: String,
    kind: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OwnedAnswerSignature {
    crate_id: String,
    file: String,
    signature_type: String,
    name: String,
    visibility: String,
    #[serde(default)]
    parameters: Vec<OwnedParameter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    return_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sketch: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OwnedParameter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pattern: Option<String>,
    #[serde(rename = "type")]
    parameter_type: String,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct OwnedAnswerSketch {
    file: String,
    signature: String,
    signature_type: String,
    matching: OwnedMatching,
    code: String,
}

impl OwnedAnswerSketch {
    fn answer(code: &str) -> Self {
        Self {
            file: "lib.rs".to_owned(),
            signature: "answer_function".to_owned(),
            signature_type: "function".to_owned(),
            matching: OwnedMatching {
                normalization: OwnedNormalization::ExactLinesV1,
                occurrence: OwnedOccurrence::AtLeastOne,
            },
            code: code.to_owned(),
        }
    }
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct OwnedMatching {
    normalization: OwnedNormalization,
    occurrence: OwnedOccurrence,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum OwnedNormalization {
    ExactLinesV1,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum OwnedOccurrence {
    AtLeastOne,
}

impl OwnedAnswerContract {
    fn answer_sketch(&self) -> &OwnedAnswerSketch {
        self.sketches
            .first()
            .and_then(|entry| entry.get("answer_body"))
            .expect("answer_body sketch")
    }
}

struct FidelityYaml {
    sketch_code: &'static str,
}

impl FidelityYaml {
    fn matching_source() -> Self {
        Self {
            sketch_code: "pub fn answer() -> u8 { 42 }",
        }
    }

    fn stale_sketch() -> Self {
        Self {
            sketch_code: "stale sketch body",
        }
    }

    fn render_crlf(&self) -> Vec<u8> {
        let yaml = r#"---
# first physical document; all presentation choices are intentional
contract_version: !!int 2
root: >-
  ../src
files: [&source "lib.rs"]
extraction: {mode: rust_syntax_v2, profile: 'rust_api_v1', crates: [{id: "main", root: *source, kind: library}]}
signatures:
  # comment adjacent to the signature node
  - answer_function:
      crate_id: main
      file: *source
      signature_type: 'function'
      name: "answer"
      visibility: public
      parameters: []
      return_type: u8
      sketch: answer_body
sketches:
  - answer_body:
      file: *source
      signature: answer_function
      signature_type: function
      matching: {normalization: exact_lines_v1, occurrence: at_least_one}
      code: |-
        __SKETCH_CODE__
# after-targeted-sketch
...
---
# second physical document must remain byte-exact when the first changes
contract_version: 2
root: '../src'
files: [other.rs]
extraction: {mode: rust_syntax_v2, profile: rust_api_v1, crates: [{id: other, root: other.rs, kind: library}]}
signatures:
  - other_function:
      crate_id: other
      file: other.rs
      signature_type: function
      name: other
      visibility: public
      parameters: []
sketches: []
...
"#
        .replace("__SKETCH_CODE__", self.sketch_code)
        .replace('\n', "\r\n");
        yaml.into_bytes()
    }
}

struct GenerateFixture {
    source: assert_fs::fixture::ChildPath,
    contracts: assert_fs::fixture::ChildPath,
    _temp: assert_fs::TempDir,
}

impl GenerateFixture {
    fn new() -> Self {
        let fixture = Self::empty();
        fixture.add_source("lib.rs", "pub fn answer() -> u8 { 42 }\n");
        fixture
    }

    fn empty() -> Self {
        let temp = assert_fs::TempDir::new().expect("temp dir");
        let source = temp.child("src");
        let contracts = temp.child("contracts");
        source.create_dir_all().expect("source dir");

        Self {
            source,
            contracts,
            _temp: temp,
        }
    }

    fn with_single_source(path: &str, source: &str) -> Self {
        let fixture = Self::empty();
        fixture.add_source(path, source);
        fixture
    }

    fn add_source(&self, path: &str, source: &str) {
        self.source
            .child(path)
            .write_str(source)
            .expect("source file");
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

    fn generate_with_crate_roots(&self, target: &str, crate_roots: &[&str]) -> assert_cmd::Command {
        let mut command = self.generate(target);
        for crate_root in crate_roots {
            command.args(["--crate-root", crate_root]);
        }
        command
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

    fn contract_bytes(&self) -> Vec<u8> {
        std::fs::read(self.contract_path()).expect("combined contract bytes")
    }

    fn sha256(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let digest = Sha256::digest(bytes);
        let mut value = String::with_capacity(64);
        for byte in digest {
            value.push(HEX[usize::from(byte >> 4)] as char);
            value.push(HEX[usize::from(byte & 0x0f)] as char);
        }
        value
    }

    fn semantic_documents(&self) -> Vec<OwnedAnswerContract> {
        let options = serde_saphyr::options! {
            duplicate_keys: serde_saphyr::DuplicateKeyPolicy::Error,
            merge_keys: serde_saphyr::MergeKeyPolicy::Error,
            strict_booleans: true,
        };
        serde_saphyr::from_slice_multiple_with_options(&self.contract_bytes(), options)
            .expect("strict semantic reparse of generated documents")
    }

    fn generated_contract(&self) -> OwnedAnswerContract {
        self.semantic_documents()
            .into_iter()
            .next()
            .expect("generated semantic document")
    }

    fn ownership_json(&self) -> serde_json::Value {
        let bytes = self.ownership_bytes();
        serde_json::from_slice(&bytes).expect("valid ownership JSON")
    }

    fn ownership_bytes(&self) -> Vec<u8> {
        std::fs::read(self.ownership_path()).expect("ownership metadata")
    }

    fn install_owned_answer_sketch(&self, code: &str) {
        let mut document = serde_saphyr::from_str::<OwnedAnswerContract>(&self.contract_text())
            .expect("generated document YAML");
        document.signatures[0]
            .values_mut()
            .next()
            .expect("first signature body")
            .sketch = Some("answer_body".to_owned());
        document.sketches = vec![BTreeMap::from([(
            "answer_body".to_owned(),
            OwnedAnswerSketch::answer(code),
        )])];
        let bytes = serde_saphyr::to_string(&document)
            .expect("render linked document")
            .into_bytes();
        self.install_owned_contract(&bytes);
    }

    fn install_owned_contract(&self, bytes: &[u8]) {
        std::fs::write(self.contract_path(), bytes).expect("write linked document");

        let mut ownership = self.ownership_json();
        let digest = Self::sha256(bytes);
        ownership["journal"]["files"]["documents"][0]["sha256"] = serde_json::Value::String(digest);
        let bytes = serde_json::to_vec_pretty(&ownership).expect("render ownership");
        std::fs::write(self.ownership_path(), bytes).expect("update owned digest");
    }
}

#[test]
fn invalid_signature_options_precede_generation_filesystem_and_recovery() {
    let fixture = GenerateFixture::empty();
    let missing_source = fixture.source.child("missing-source");
    let contracts = fixture.contracts.child("missing-contracts");

    for subject in ["all", "signatures"] {
        ConkitCli::command()
            .args(["generate", subject, "--source"])
            .arg(missing_source.path())
            .arg("--contracts")
            .arg(contracts.path())
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

    contracts.assert(predicate::path::missing());
}

#[test]
fn existing_compiler_root_preflight_precedes_manifest_warning_and_domain_publication() {
    let fixture = GenerateFixture::new();
    fixture.generate_successfully("signatures");
    fixture.add_source("other.rs", "pub fn other() {}\n");
    fixture.install_owned_contract(TWO_ROOT_COMPILER_COMBINED_CONTRACT.as_bytes());
    let contract_before = fixture.contract_bytes();
    let ownership_before = fixture.ownership_bytes();
    let unusable_manifest = fixture._temp.child("unusable-manifest");
    unusable_manifest
        .create_dir_all()
        .expect("unusable manifest directory");

    fixture
        .generate("all")
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

    assert_eq!(fixture.contract_bytes(), contract_before);
    assert_eq!(fixture.ownership_bytes(), ownership_before);
}

#[test]
fn fresh_signature_generation_writes_one_original_style_document() {
    let fixture = GenerateFixture::new();

    fixture
        .generate("signatures")
        .assert()
        .success()
        .stdout(
            "signature generation completed: documents 1, signatures 1, preserved sketches 0, semantically changed documents 1, byte-changed documents 1\n",
        )
        .stderr(predicate::str::is_empty());

    let contract = fixture.contract_text();
    assert!(contract.starts_with("contract_version: 2\nroot: ../src\nfiles:\n"));
    assert!(contract.contains("extraction:"));
    assert!(contract.contains("- lib.rs"));
    assert!(contract.contains("signatures:"));
    assert!(contract.contains("answer_function:"));
    assert!(contract.contains("sketches: []"));
    assert!(!contract.contains("version: 1"));
    assert!(!contract.contains("language:"));
}

#[test]
fn fresh_all_generation_creates_signatures_with_zero_opt_in_sketches() {
    let fixture = GenerateFixture::new();

    fixture
        .generate("all")
        .assert()
        .success()
        .stdout(
            "contract generation completed: documents 1, signatures 1, preserved sketches 0, semantically changed documents 1, byte-changed documents 1; linked sketches 0, refreshed 0, changed 0, changed documents 0\n",
        )
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
fn signature_targets_accept_repeated_crate_roots_and_persist_ids_roots_and_kinds() {
    for target in ["signatures", "all"] {
        let fixture = GenerateFixture::new();
        fixture.add_source("main.rs", "pub fn application() {}\n");

        fixture
            .generate_with_crate_roots(
                target,
                &["library=library:lib.rs", "application=binary:main.rs"],
            )
            .assert()
            .success()
            .stderr(predicate::str::is_empty());

        let contract = fixture.generated_contract();
        assert_eq!(
            contract.files,
            vec!["lib.rs".to_owned(), "main.rs".to_owned()],
        );
        let extraction = contract.extraction.expect("generated extraction");
        assert_eq!(extraction.mode, "rust_syntax_v2");
        assert_eq!(extraction.profile, "rust_api_v1");
        let crates = extraction
            .crates
            .into_iter()
            .map(|crate_root| (crate_root.id, (crate_root.root, crate_root.kind)))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            crates,
            BTreeMap::from([
                (
                    "application".to_owned(),
                    ("main.rs".to_owned(), "binary".to_owned()),
                ),
                (
                    "library".to_owned(),
                    ("lib.rs".to_owned(), "library".to_owned()),
                ),
            ]),
            "wrong extraction metadata for generate {target}",
        );
    }
}

#[test]
fn fresh_generation_infers_one_conventional_root_and_its_target_kind() {
    for (root, source, expected_kind) in [
        ("lib.rs", "pub fn library() {}\n", "library"),
        ("main.rs", "pub fn application() {}\n", "binary"),
    ] {
        let fixture = GenerateFixture::with_single_source(root, source);

        fixture
            .generate("signatures")
            .assert()
            .success()
            .stderr(predicate::str::is_empty());

        let extraction = fixture
            .generated_contract()
            .extraction
            .expect("generated extraction");
        assert_eq!(extraction.crates.len(), 1);
        let crate_root = &extraction.crates[0];
        assert_eq!(crate_root.root, root);
        assert_eq!(crate_root.kind, expected_kind);
        assert!(!crate_root.id.is_empty());
        assert_eq!(crate_root.id.trim(), crate_root.id.as_str());
    }
}

#[test]
fn implicit_generation_rejects_ambiguous_multiple_and_disconnected_roots() {
    for (sources, expected) in [
        (
            [
                ("lib.rs", "pub fn library() {}\n"),
                ("main.rs", "fn main() {}\n"),
            ],
            "requires explicit --crate-root",
        ),
        (
            [
                ("first.rs", "pub fn first() {}\n"),
                ("second.rs", "pub fn second() {}\n"),
            ],
            "requires explicit --crate-root",
        ),
        (
            [
                ("lib.rs", "pub fn library() {}\n"),
                ("orphan.rs", "pub fn orphan() {}\n"),
            ],
            "allowlisted Rust source orphan.rs is disconnected; declare it as a crate root or link it with mod",
        ),
    ] {
        let fixture = GenerateFixture::empty();
        for (path, source) in sources {
            fixture.add_source(path, source);
        }

        fixture
            .generate("signatures")
            .assert()
            .failure()
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::contains(expected));

        fixture
            .contracts
            .child("main.yml")
            .assert(predicate::path::missing());
        fixture
            .contracts
            .child(".contract-kit")
            .assert(predicate::path::missing());
    }
}

#[test]
fn explicit_roots_must_claim_every_disconnected_rust_source() {
    let fixture = GenerateFixture::new();
    fixture.add_source("orphan.rs", "pub fn orphan() {}\n");

    fixture
        .generate_with_crate_roots("signatures", &["library=library:lib.rs"])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(
            predicate::str::contains("allowlisted Rust source orphan.rs is disconnected").and(
                predicate::str::contains("declare it as a crate root or link it with mod"),
            ),
        );

    fixture
        .contracts
        .child("main.yml")
        .assert(predicate::path::missing());
}

#[test]
fn crate_root_values_reject_malformed_and_duplicate_ids() {
    let fixture = GenerateFixture::new();
    for (value, expected) in [
        ("app", "must have the form CRATE_ID=KIND:RELATIVE_PATH"),
        (
            "app=lib.rs",
            "must include a target kind as KIND:RELATIVE_PATH",
        ),
        ("=library:lib.rs", "crate id must not be empty"),
        (
            " app=library:lib.rs",
            "crate id must not contain whitespace",
        ),
        (
            "my app=library:lib.rs",
            "crate id must not contain whitespace",
        ),
        (
            "app:id=library:lib.rs",
            "crate id must not contain `:` or `=` identity delimiters",
        ),
        (
            "app\u{1}=library:lib.rs",
            "crate id must not contain control characters",
        ),
        ("app=:lib.rs", "crate kind must not be empty"),
        (
            "app= library:lib.rs",
            "crate kind has surrounding whitespace",
        ),
        (
            "app=executable:lib.rs",
            "crate kind must be either library or binary",
        ),
        ("app=library:", "crate root path must not be empty"),
        (
            "app=library: lib.rs",
            "crate root path has surrounding whitespace",
        ),
    ] {
        fixture
            .generate_with_crate_roots("signatures", &[value])
            .assert()
            .failure()
            .stdout(predicate::str::is_empty())
            .stderr(predicate::str::contains(expected));
    }

    fixture.add_source("main.rs", "fn main() {}\n");
    fixture
        .generate_with_crate_roots("signatures", &["app=library:lib.rs", "app=binary:main.rs"])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("duplicate crate id app"));

    fixture
        .contracts
        .child("main.yml")
        .assert(predicate::path::missing());
}

#[test]
fn explicit_crate_roots_validate_files_and_accept_nonconventional_library_and_binary_roots() {
    let missing = GenerateFixture::new();
    missing
        .generate_with_crate_roots("signatures", &["app=library:missing.rs"])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(
            predicate::str::contains("crate root missing.rs")
                .and(predicate::str::contains("does not exist")),
        );
    missing
        .contracts
        .child("main.yml")
        .assert(predicate::path::missing());

    let non_rust = GenerateFixture::empty();
    non_rust.add_source("notes.txt", "not Rust\n");
    non_rust
        .generate_with_crate_roots("signatures", &["app=library:notes.txt"])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(
            predicate::str::contains("crate root notes.txt")
                .and(predicate::str::contains("Rust source file")),
        );
    non_rust
        .contracts
        .child("main.yml")
        .assert(predicate::path::missing());

    let nonconventional = GenerateFixture::empty();
    nonconventional.add_source("custom_library_root.rs", "pub fn custom_library() {}\n");
    nonconventional.add_source("tool_entry.rs", "fn main() {}\n");
    nonconventional
        .generate_with_crate_roots(
            "signatures",
            &[
                "custom_library=library:custom_library_root.rs",
                "custom_binary=binary:tool_entry.rs",
            ],
        )
        .assert()
        .success()
        .stderr(predicate::str::is_empty());
    let extraction = nonconventional
        .generated_contract()
        .extraction
        .expect("generated extraction");
    assert_eq!(
        extraction.crates,
        vec![
            OwnedCrate {
                id: "custom_binary".to_owned(),
                root: "tool_entry.rs".to_owned(),
                kind: "binary".to_owned(),
            },
            OwnedCrate {
                id: "custom_library".to_owned(),
                root: "custom_library_root.rs".to_owned(),
                kind: "library".to_owned(),
            },
        ],
    );
}

#[test]
fn an_existing_contract_rejects_crate_root_overrides_without_mutation() {
    let fixture = GenerateFixture::new();
    fixture.generate_successfully("signatures");
    let contract_before = fixture.contract_bytes();
    let ownership_before =
        std::fs::read(fixture.ownership_path()).expect("ownership before override");

    fixture
        .generate_with_crate_roots("signatures", &["renamed=library:lib.rs"])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "--crate-root cannot override extraction in existing contract documents",
        ));

    assert_eq!(fixture.contract_bytes(), contract_before);
    assert_eq!(
        std::fs::read(fixture.ownership_path()).expect("ownership after override"),
        ownership_before,
    );
}

#[test]
fn an_existing_contract_rejects_an_unlisted_extraction_root() {
    let fixture = GenerateFixture::new();
    fixture
        .contracts
        .create_dir_all()
        .expect("contracts directory");
    fixture
        .contracts
        .child("main.yml")
        .write_str(
            "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: orphan, root: orphan.rs, kind: library }] }\nsignatures: []\nsketches: []\n",
        )
        .expect("contract with unlisted root");

    fixture
        .generate("signatures")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(
            predicate::str::contains("crate root orphan.rs")
                .and(predicate::str::contains("must also appear in files")),
        );

    fixture
        .contracts
        .child(".contract-kit")
        .assert(predicate::path::missing());
}

#[test]
fn sketch_generation_rejects_signature_only_crate_root_options() {
    let fixture = GenerateFixture::new();

    fixture
        .generate_with_crate_roots("sketches", &["app=library:lib.rs"])
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "unexpected argument '--crate-root'",
        ));
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
        .stdout(
            "sketch generation completed: linked 1, refreshed 1, changed 1, changed documents 1\n",
        )
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
fn sketch_generation_reports_exact_noop_as_refreshed_but_unchanged() {
    let fixture = GenerateFixture::new();
    fixture.generate_successfully("signatures");
    fixture.install_owned_answer_sketch("pub fn answer() -> u8 { 42 }");
    let before = fixture.contract_bytes();

    fixture
        .generate("sketches")
        .assert()
        .success()
        .stdout(
            "sketch generation completed: linked 1, refreshed 1, changed 0, changed documents 0\n",
        )
        .stderr(predicate::str::is_empty());

    assert_eq!(fixture.contract_bytes(), before);
}

#[test]
fn signature_generation_preserves_the_linked_sketch_section() {
    let fixture = GenerateFixture::new();
    fixture.generate_successfully("signatures");
    fixture.install_owned_answer_sketch("user-authored sketch body");
    let before = serde_saphyr::from_str::<OwnedAnswerContract>(&fixture.contract_text())
        .expect("before YAML");

    fixture
        .source
        .child("lib.rs")
        .write_str("pub fn answer() -> u8 { 43 }\n")
        .expect("implementation-only change");
    fixture.generate_successfully("signatures");

    let after = serde_saphyr::from_str::<OwnedAnswerContract>(&fixture.contract_text())
        .expect("after YAML");
    assert_eq!(before.sketches, after.sketches);
    assert!(
        fixture
            .contract_text()
            .contains("user-authored sketch body")
    );
}

#[test]
fn formatted_multidocument_noop_is_byte_exact_for_both_generation_families() {
    let fixture = GenerateFixture::new();
    fixture.generate_successfully("signatures");
    fixture.add_source("other.rs", "pub fn other() {}\n");
    let expected = FidelityYaml::matching_source().render_crlf();
    fixture.install_owned_contract(&expected);

    let initial = fixture.semantic_documents();
    assert_eq!(initial.len(), 2);
    assert_eq!(initial[0].root, "../src");
    assert_eq!(
        initial[0].answer_sketch().code,
        "pub fn answer() -> u8 { 42 }"
    );

    fixture.generate_successfully("signatures");
    assert_eq!(fixture.contract_bytes(), expected);

    fixture.generate_successfully("sketches");
    assert_eq!(fixture.contract_bytes(), expected);
    assert_eq!(fixture.semantic_documents().len(), 2);
}

#[test]
fn signature_edit_preserves_sketch_extraction_and_untargeted_document_bytes() {
    let fixture = GenerateFixture::new();
    fixture.generate_successfully("signatures");
    fixture.add_source("other.rs", "pub fn other() {}\n");
    let original = FidelityYaml::matching_source().render_crlf();
    fixture.install_owned_contract(&original);
    fixture
        .source
        .child("lib.rs")
        .write_str("pub fn answer() -> u16 { 42 }\n")
        .expect("signature change");

    fixture.generate_successfully("signatures");
    let changed = fixture.contract_bytes();
    assert_ne!(changed, original);

    let before = std::str::from_utf8(&original).expect("original contract UTF-8");
    let after = std::str::from_utf8(&changed).expect("changed contract UTF-8");
    assert_eq!(
        before
            .split_once("signatures:")
            .expect("original signatures marker")
            .0,
        after
            .split_once("signatures:")
            .expect("changed signatures marker")
            .0,
    );
    assert_eq!(
        before
            .split_once("sketches:")
            .expect("original sketches marker")
            .1,
        after
            .split_once("sketches:")
            .expect("changed sketches marker")
            .1,
    );
    let second_document = "---\r\n# second physical document";
    assert_eq!(
        before
            .split_once(second_document)
            .expect("original second document")
            .1,
        after
            .split_once(second_document)
            .expect("changed second document")
            .1,
    );
    assert!(after.contains("return_type: u16"));
    assert!(
        changed
            .iter()
            .enumerate()
            .all(|(index, byte)| { *byte != b'\n' || index > 0 && changed[index - 1] == b'\r' })
    );

    let reparsed = fixture.semantic_documents();
    assert_eq!(reparsed.len(), 2);
    assert_eq!(
        reparsed[0].signatures[0]
            .values()
            .next()
            .expect("changed answer signature")
            .return_type
            .as_deref(),
        Some("u16"),
    );

    fixture.generate_successfully("signatures");
    assert_eq!(fixture.contract_bytes(), changed);
}

#[test]
fn sketch_edit_preserves_signature_extraction_and_untargeted_document_bytes() {
    let fixture = GenerateFixture::new();
    fixture.generate_successfully("signatures");
    fixture.add_source("other.rs", "pub fn other() {}\n");
    let original = FidelityYaml::stale_sketch().render_crlf();
    fixture.install_owned_contract(&original);

    fixture.generate_successfully("sketches");
    let changed = fixture.contract_bytes();
    assert_ne!(changed, original);

    let before = std::str::from_utf8(&original).expect("original contract UTF-8");
    let after = std::str::from_utf8(&changed).expect("changed contract UTF-8");
    assert_eq!(
        before
            .split_once("      code:")
            .expect("original code scalar")
            .0,
        after
            .split_once("      code:")
            .expect("changed code scalar")
            .0,
    );
    assert_eq!(
        before
            .split_once("# after-targeted-sketch")
            .expect("original post-sketch bytes")
            .1,
        after
            .split_once("# after-targeted-sketch")
            .expect("changed post-sketch bytes")
            .1,
    );
    assert!(after.contains("      code: |-\r\n        pub fn answer() -> u8 { 42 }\r\n"));
    assert!(
        changed
            .iter()
            .enumerate()
            .all(|(index, byte)| { *byte != b'\n' || index > 0 && changed[index - 1] == b'\r' })
    );

    let reparsed = fixture.semantic_documents();
    assert_eq!(reparsed.len(), 2);
    assert_eq!(
        reparsed[0].answer_sketch().code,
        "pub fn answer() -> u8 { 42 }"
    );

    fixture.generate_successfully("sketches");
    assert_eq!(fixture.contract_bytes(), changed);
}

#[test]
fn all_generation_refreshes_linked_sketch_after_signature_change() {
    let fixture = GenerateFixture::new();
    fixture.generate_successfully("signatures");
    fixture.install_owned_answer_sketch("old implementation");
    assert_eq!(fixture.ownership_json()["journal"]["generation"], 1);
    fixture
        .source
        .child("lib.rs")
        .write_str("pub fn answer() -> u16 { 43 }\n")
        .expect("signature and implementation change");

    fixture
        .generate("all")
        .assert()
        .success()
        .stdout(
            "contract generation completed: documents 1, signatures 1, preserved sketches 1, semantically changed documents 1, byte-changed documents 1; linked sketches 1, refreshed 1, changed 1, changed documents 1\n",
        )
        .stderr(predicate::str::is_empty());

    let contract = fixture.contract_text();
    assert!(contract.contains("return_type: u16"));
    assert!(contract.contains("pub fn answer() -> u16 { 43 }"));
    assert!(!contract.contains("old implementation"));
    assert_eq!(contract.matches("answer_body").count(), 2);

    let document = fixture.generated_contract();
    let signature = document.signatures[0]
        .values()
        .next()
        .expect("refreshed answer signature");
    assert_eq!(signature.return_type.as_deref(), Some("u16"));
    assert_eq!(signature.sketch.as_deref(), Some("answer_body"));
    assert_eq!(
        document.answer_sketch().code,
        "pub fn answer() -> u16 { 43 }"
    );

    let contract_digest = GenerateFixture::sha256(&fixture.contract_bytes());
    let ownership = fixture.ownership_json();
    assert_eq!(ownership["journal"]["generation"], 2);
    assert_eq!(
        ownership["journal"]["files"]["documents"][0]["sha256"].as_str(),
        Some(contract_digest.as_str()),
    );
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
        .stdout(
            "contract generation completed: documents 1, signatures 1, preserved sketches 0, semantically changed documents 1, byte-changed documents 1; linked sketches 0, refreshed 0, changed 0, changed documents 0\n",
        );

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
            "sha256": GenerateFixture::sha256(b"user file\n"),
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
        .stdout(
            "signature generation completed: documents 1, signatures 1, preserved sketches 0, semantically changed documents 1, byte-changed documents 1\n",
        )
        .stderr(predicate::str::is_empty());

    let recovered = serde_saphyr::from_str::<OwnedAnswerContract>(&fixture.contract_text())
        .expect("recovered v2 combined contract");
    assert_eq!(recovered.contract_version, 2);
    assert_eq!(recovered.root, "../src");
    assert_eq!(recovered.files, vec!["lib.rs".to_owned()]);
    assert!(recovered.extraction.is_some());
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
            "contract generation completed: documents 1, signatures 1, preserved sketches 0, semantically changed documents 0, byte-changed documents 0; linked sketches 0, refreshed 0, changed 0, changed documents 0\n",
            "adopted 1 matching existing contract files\n",
        ));
    fixture
        .generate("all")
        .arg("--adopt-existing")
        .assert()
        .success()
        .stdout(concat!(
            "contract generation completed: documents 1, signatures 1, preserved sketches 0, semantically changed documents 0, byte-changed documents 0; linked sketches 0, refreshed 0, changed 0, changed documents 0\n",
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
