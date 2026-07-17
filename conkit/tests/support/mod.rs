use assert_cmd::Command;

#[allow(dead_code)]
pub(crate) const TWO_ROOT_COMPILER_COMBINED_CONTRACT: &str = r#"contract_version: 2
root: ../src
files: [lib.rs, other.rs]
extraction:
  mode: rust_compiler_v1
  profile: rust_api_v1
  crates:
    - { id: fixture, root: lib.rs, kind: library }
    - { id: other, root: other.rs, kind: library }
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

pub(crate) struct ConkitCli;

impl ConkitCli {
    pub(crate) fn command() -> Command {
        let mut command = Command::cargo_bin("conkit").expect("conkit CLI binary");
        command
            .env("COLUMNS", "100")
            .env("LINES", "24")
            .env("NO_COLOR", "1")
            .env_remove("CLICOLOR")
            .env_remove("CLICOLOR_FORCE");
        command
    }

    #[allow(dead_code)]
    pub(crate) const fn displayed_name() -> &'static str {
        "conkit"
    }
}
