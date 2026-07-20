use conkit_sketch::{CatalogPath, CheckMode, CheckRequest, FileCatalog, ReportRequest};

pub(super) struct CatalogFixture {
    catalog: FileCatalog,
}

impl CatalogFixture {
    pub(super) fn new() -> Self {
        Self {
            catalog: FileCatalog::new(),
        }
    }

    pub(super) fn with_file(mut self, path: &str, contents: &str) -> Self {
        self.catalog
            .insert(
                CatalogPath::new(path).expect("fixture path"),
                contents.as_bytes().to_vec(),
            )
            .expect("insert fixture");
        self
    }

    pub(super) fn into_catalog(self) -> FileCatalog {
        self.catalog
    }
}

pub(super) struct CheckFixture {
    source_files: FileCatalog,
    contract_files: FileCatalog,
}

impl CheckFixture {
    pub(super) fn matching() -> Self {
        Self {
            source_files: CatalogFixture::new()
                .with_file("src/lib.rs", "pub fn answer() -> u8 {\n    42\n}\n")
                .into_catalog(),
            contract_files: CatalogFixture::new()
                .with_file("main.yml", Self::matching_contract())
                .into_catalog(),
        }
    }

    pub(super) fn mismatched() -> Self {
        Self {
            source_files: CatalogFixture::new()
                .with_file("src/lib.rs", "pub fn answer() -> u8 {\n    41\n}\n")
                .into_catalog(),
            contract_files: CatalogFixture::new()
                .with_file("main.yml", Self::matching_contract())
                .into_catalog(),
        }
    }

    pub(super) fn request(self, report: ReportRequest, mode: CheckMode) -> CheckRequest {
        CheckRequest {
            source_files: self.source_files,
            contract_files: self.contract_files,
            report,
            mode,
        }
    }

    pub(super) fn matching_contract() -> &'static str {
        r#"
contract_version: 2
root: ../src
files: [src/lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: example, root: src/lib.rs, kind: library }] }
signatures:
  - answer_signature:
      file: src/lib.rs
      signature_type: function
      name: answer
      sketch: answer_body
sketches:
  - answer_body:
      file: src/lib.rs
      signature: answer_signature
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: at_least_one }
      code: |
        pub fn answer() -> u8 {
            42
        }
"#
    }

    pub(super) fn linked_contract(code: &str) -> String {
        format!(
            "contract_version: 2\nroot: ../src\nfiles: [src/lib.rs]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: example, root: src/lib.rs, kind: library }}] }}\nsignatures:\n  - answer_signature:\n      file: src/lib.rs\n      signature_type: function\n      sketch: answer_body\nsketches:\n  - answer_body:\n      file: src/lib.rs\n      signature: answer_signature\n      signature_type: function\n      matching: {{ normalization: exact_lines_v1, occurrence: at_least_one }}\n      code: '{code}'\n"
        )
    }
}
