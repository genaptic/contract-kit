use super::{catalog, catalog_with, contract_inventory};
use crate::api::{ResolveSketchesRequest, RustExtractionInput};
use crate::languages::rust::parser::SignatureParser;
use crate::limits::{LimitResource, OutputLimits, SignatureLimits};

struct SketchResolutionFixture {
    request: ResolveSketchesRequest,
}

impl SketchResolutionFixture {
    fn new(request: ResolveSketchesRequest) -> Self {
        Self { request }
    }

    fn resolve(
        self,
    ) -> Result<crate::api::ResolveSketchesResponse, crate::error::SignatureContractKitError> {
        self.resolve_with_limits(SignatureLimits::default())
    }

    fn resolve_with_limits(
        self,
        limits: SignatureLimits,
    ) -> Result<crate::api::ResolveSketchesResponse, crate::error::SignatureContractKitError> {
        SignatureParser::new(limits)
            .resolve_sketches(self.request, &crate::work::CancellationProbe::new())
    }
}

#[test]
fn resolved_seed_code_is_rejected_before_crossing_the_generated_output_budget() {
    let source_item = "pub fn answer() -> u8 { 42 }";
    let request = ResolveSketchesRequest {
        extraction: RustExtractionInput::Syntax,
        source_files: catalog_with("lib.rs", format!("{source_item}\n").as_bytes()),
        contract_files: catalog_with(
            "main.yml",
            br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - answer_function:
      file: lib.rs
      signature_type: function
      name: answer
      visibility: public
      return_type: u8
      sketch: answer_body
sketches:
  - answer_body:
      file: lib.rs
      signature: answer_function
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: exactly_one }
      code: old
"#,
        ),
    };
    let limits = SignatureLimits {
        output: OutputLimits {
            generated_bytes: u64::try_from(source_item.len() - 1).expect("fixture size"),
            ..OutputLimits::default()
        },
        ..SignatureLimits::default()
    };

    let error = SketchResolutionFixture::new(request)
        .resolve_with_limits(limits)
        .expect_err("seed code must obey the aggregate generated-output budget");
    let limit = error.limit_exceeded().expect("typed output budget");
    assert_eq!(limit.resource, LimitResource::GeneratedOutputBytes);
    assert_eq!(
        limit.file.as_ref().map(crate::CatalogPath::as_str),
        Some("lib.rs")
    );
    assert_eq!(
        limit.observed_at_least,
        u64::try_from(source_item.len()).expect("fixture size")
    );
}

#[test]
fn document_local_signature_labels_and_overlapping_files_are_independent() {
    let contracts = catalog([
        (
            "one.yml",
            br#"contract_version: 2
root: ../src
files: [shared.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: one, root: shared.rs, kind: library }] }
signatures:
  - shared:
      file: shared.rs
      signature_type: function
      name: one
      sketch: shared_one
sketches:
  - shared_one:
      file: shared.rs
      signature: shared
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: at_least_one }
      code: one
"#
            .as_slice(),
        ),
        (
            "two.yaml",
            br#"contract_version: 2
root: ../src
files: [shared.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: two, root: shared.rs, kind: library }] }
signatures:
  - shared:
      file: shared.rs
      signature_type: function
      name: two
      sketch: shared_two
sketches:
  - shared_two:
      file: shared.rs
      signature: shared
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: at_least_one }
      code: two
"#
            .as_slice(),
        ),
    ]);
    let inventory = contract_inventory(contracts.clone()).expect("document-local inventory");
    assert_eq!(inventory.len(), 2);

    let response = SketchResolutionFixture::new(ResolveSketchesRequest {
        extraction: RustExtractionInput::Syntax,
        source_files: catalog_with("shared.rs", b"pub fn one() {}\n\npub fn two() {}\n"),
        contract_files: contracts,
    })
    .resolve()
    .expect("document-local sketch resolution");

    assert_eq!(response.seeds.len(), 2);
    assert_eq!(response.seeds[0].contract_file.as_str(), "one.yml");
    assert_eq!(response.seeds[0].document_index, 0);
    assert_eq!(response.seeds[0].sketch_id, "shared_one");
    assert_eq!(response.seeds[0].code, "pub fn one() {}");
    assert_eq!(response.seeds[1].contract_file.as_str(), "two.yaml");
    assert_eq!(response.seeds[1].document_index, 0);
    assert_eq!(response.seeds[1].sketch_id, "shared_two");
    assert_eq!(response.seeds[1].code, "pub fn two() {}");
}

#[test]
fn sketch_ids_are_unique_across_physical_contract_documents() {
    let contracts = catalog([
        (
            "one.yml",
            br#"contract_version: 2
root: ../src
files: [shared.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: one, root: shared.rs, kind: library }] }
signatures:
  - shared:
      file: shared.rs
      signature_type: function
      name: one
      sketch: duplicate
sketches:
  - duplicate:
      file: shared.rs
      signature: shared
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: at_least_one }
      code: one
"#
            .as_slice(),
        ),
        (
            "two.yaml",
            br#"contract_version: 2
root: ../src
files: [shared.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: two, root: shared.rs, kind: library }] }
signatures:
  - shared:
      file: shared.rs
      signature_type: function
      name: two
      sketch: duplicate
sketches:
  - duplicate:
      file: shared.rs
      signature: shared
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: at_least_one }
      code: two
"#
            .as_slice(),
        ),
    ]);

    let error = contract_inventory(contracts).expect_err("duplicate sketch ids must fail");

    assert!(
        error.to_string().contains(
            "duplicate sketch id duplicate is declared in both one.yml document 0 and two.yaml document 0"
        ),
        "{error}"
    );
}

#[test]
fn sketch_ids_are_unique_across_documents_in_one_physical_contract_file() {
    let contracts = catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [shared.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: first, root: shared.rs, kind: library }] }
signatures:
  - first:
      file: shared.rs
      signature_type: function
      name: one
      sketch: duplicate
sketches:
  - duplicate:
      file: shared.rs
      signature: first
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: at_least_one }
      code: one
---
contract_version: 2
root: ../src
files: [shared.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: second, root: shared.rs, kind: library }] }
signatures:
  - second:
      file: shared.rs
      signature_type: function
      name: two
      sketch: duplicate
sketches:
  - duplicate:
      file: shared.rs
      signature: second
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: at_least_one }
      code: two
"#,
    );

    let error = contract_inventory(contracts).expect_err("duplicate sketch ids must fail");

    assert!(
        error.to_string().contains(
            "duplicate sketch id duplicate is declared in both main.yml document 0 and main.yml document 1"
        ),
        "{error}"
    );
}

#[test]
fn resolved_seeds_retain_zero_based_physical_document_indexes() {
    let contracts = catalog_with(
        "main.yml",
        br#"---
contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: first, root: lib.rs, kind: library }] }
signatures:
  - linked:
      file: lib.rs
      signature_type: function
      name: first
      sketch: first_body
sketches:
  - first_body:
      file: lib.rs
      signature: linked
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: exactly_one }
      code: old
---
contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: second, root: lib.rs, kind: library }] }
signatures:
  - linked:
      file: lib.rs
      signature_type: function
      name: second
      sketch: second_body
sketches:
  - second_body:
      file: lib.rs
      signature: linked
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: exactly_one }
      code: old
"#,
    );
    let response = SketchResolutionFixture::new(ResolveSketchesRequest {
        extraction: RustExtractionInput::Syntax,
        source_files: catalog_with("lib.rs", b"pub fn first() {}\npub fn second() {}\n"),
        contract_files: contracts,
    })
    .resolve()
    .expect("multi-document seeds");

    assert_eq!(response.seeds.len(), 2);
    assert_eq!(response.seeds[0].document_index, 0);
    assert_eq!(response.seeds[0].code, "pub fn first() {}");
    assert_eq!(response.seeds[1].document_index, 1);
    assert_eq!(response.seeds[1].code, "pub fn second() {}");
}

#[test]
fn linked_sketch_resolver_returns_exact_source_item_text() {
    let source = catalog_with(
        "main.rs",
        b"fn helper() {}\n\n#[cfg(feature = \"app\")]\nfn main() {\n    helper();\n}\n",
    );
    let contracts = catalog([
        (
            "main.yml",
            br#"contract_version: 2
root: ../src
files: [main.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: app, root: main.rs, kind: binary }] }
signatures:
  - main:
      crate_id: app
      file: main.rs
      signature_type: function
      name: main
      visibility: private
      sketch: main
sketches:
  - main:
      file: main.rs
      signature: main
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: exactly_one }
      code: |
        fn main() {}
"#
            .as_slice(),
        ),
        (
            "nested/ignored.yml",
            b"this nested document is deliberately invalid YAML".as_slice(),
        ),
    ]);

    let response = SketchResolutionFixture::new(ResolveSketchesRequest {
        extraction: RustExtractionInput::Syntax,
        source_files: source,
        contract_files: contracts,
    })
    .resolve()
    .expect("resolve");

    assert_eq!(response.seeds.len(), 1);
    assert_eq!(response.seeds[0].sketch_id, "main");
    assert_eq!(
        response.seeds[0].code,
        "#[cfg(feature = \"app\")]\nfn main() {\n    helper();\n}"
    );
    assert!(response.capability_warnings.iter().any(|warning| {
        warning.contains("cfg/cfg_attr conditional compilation")
            && warning.contains("rust_syntax_v2")
    }));
}

#[test]
fn linked_sketch_resolver_uses_nested_module_identity() {
    let source = catalog_with(
        "lib.rs",
        br#"pub fn answer() -> u8 { 1 }

mod nested {
pub fn answer() -> u8 { 2 }
}
"#,
    );
    let contracts = catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - nested_answer:
      file: lib.rs
      module_path: [nested]
      signature_type: function
      name: answer
      visibility: public
      return_type: u8
      sketch: nested_answer
sketches:
  - nested_answer:
      file: lib.rs
      signature: nested_answer
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: exactly_one }
      code: old
"#,
    );

    let response = SketchResolutionFixture::new(ResolveSketchesRequest {
        extraction: RustExtractionInput::Syntax,
        source_files: source,
        contract_files: contracts,
    })
    .resolve()
    .expect("nested item identity must resolve");

    assert_eq!(response.seeds.len(), 1);
    let seed = &response.seeds[0];
    assert_eq!(seed.contract_file.as_str(), "main.yml");
    assert_eq!(seed.document_index, 0);
    assert_eq!(seed.file.as_str(), "lib.rs");
    assert_eq!(seed.sketch_id, "nested_answer");
    assert_eq!(seed.signature_type, "function");
    assert_eq!(seed.code, "pub fn answer() -> u8 { 2 }");
}

#[test]
fn linked_sketch_resolver_uses_out_of_line_module_graph_span() {
    let source = catalog([
        (
            "lib.rs",
            b"pub fn send() -> u8 { 0 }\n\npub mod transport;\n".as_slice(),
        ),
        (
            "transport.rs",
            br#"const PREFIX: &str = "transport";

pub fn send() -> u8 {
    7
}
"#
            .as_slice(),
        ),
    ]);
    let contracts = catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [lib.rs, transport.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - transport_send:
      crate_id: sample
      file: transport.rs
      module_path: [transport]
      signature_type: function
      name: send
      visibility: public
      return_type: u8
      sketch: transport_send_body
sketches:
  - transport_send_body:
      file: transport.rs
      signature: transport_send
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: exactly_one }
      code: old
"#,
    );

    let response = SketchResolutionFixture::new(ResolveSketchesRequest {
        extraction: RustExtractionInput::Syntax,
        source_files: source,
        contract_files: contracts,
    })
    .resolve()
    .expect("out-of-line graph span");

    assert_eq!(response.seeds.len(), 1);
    assert_eq!(response.seeds[0].file.as_str(), "transport.rs");
    assert_eq!(response.seeds[0].sketch_id, "transport_send_body");
    assert_eq!(response.seeds[0].code, "pub fn send() -> u8 {\n    7\n}");
}

#[test]
fn linked_sketch_resolver_parses_only_the_required_module_closure() {
    let source = catalog([
        (
            "lib.rs",
            b"pub mod linked;\npub mod unrelated;\n".as_slice(),
        ),
        ("linked.rs", b"pub fn execute() { work(); }\n".as_slice()),
        ("unrelated.rs", b"pub fn \xff".as_slice()),
    ]);
    let contracts = catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [lib.rs, linked.rs, unrelated.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - linked_execute:
      crate_id: sample
      file: linked.rs
      module_path: [linked]
      signature_type: function
      name: execute
      visibility: public
      sketch: linked_execute_body
sketches:
  - linked_execute_body:
      file: linked.rs
      signature: linked_execute
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: exactly_one }
      code: old
"#,
    );
    let response = SketchResolutionFixture::new(ResolveSketchesRequest {
        extraction: RustExtractionInput::Syntax,
        source_files: source,
        contract_files: contracts,
    })
    .resolve()
    .expect("unlinked malformed sibling must not be decoded");

    assert_eq!(response.seeds.len(), 1);
    assert_eq!(response.seeds[0].file.as_str(), "linked.rs");
    assert_eq!(response.seeds[0].code, "pub fn execute() { work(); }");
}

#[test]
fn linked_sketch_resolver_discovers_missing_intermediate_contract_metadata() {
    let source = catalog([
        ("lib.rs", b"pub mod a;\npub mod unrelated;\n".as_slice()),
        ("a.rs", b"pub mod b;\n".as_slice()),
        ("a/b.rs", b"pub fn execute() { work(); }\n".as_slice()),
        ("unrelated.rs", b"pub fn \xff".as_slice()),
    ]);
    let contracts = catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [lib.rs, a.rs, a/b.rs, unrelated.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - nested_execute:
      crate_id: sample
      file: a/b.rs
      module_path: [a, b]
      signature_type: function
      name: execute
      visibility: public
      sketch: nested_execute_body
sketches:
  - nested_execute_body:
      file: a/b.rs
      signature: nested_execute
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: exactly_one }
      code: old
"#,
    );

    let response = SketchResolutionFixture::new(ResolveSketchesRequest {
        extraction: RustExtractionInput::Syntax,
        source_files: source,
        contract_files: contracts,
    })
    .resolve()
    .expect("required graph must discover intermediate module files");

    assert_eq!(response.seeds.len(), 1);
    assert_eq!(response.seeds[0].file.as_str(), "a/b.rs");
    assert_eq!(response.seeds[0].code, "pub fn execute() { work(); }");
}

#[test]
fn linked_sketch_resolver_uses_path_override_logical_module_and_physical_span() {
    let source = catalog([
        (
            "lib.rs",
            br#"#[path = "platform/worker_impl.rs"]
pub mod worker;
"#
            .as_slice(),
        ),
        (
            "platform/worker_impl.rs",
            "const PREFIX: &str = \"naïve\";\n\npub fn execute(value: usize) -> usize {\n    value + 1\n}\n"
                .as_bytes(),
        ),
    ]);
    let contracts = catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [lib.rs, platform/worker_impl.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - worker_execute:
      crate_id: sample
      file: platform/worker_impl.rs
      module_path: [worker]
      signature_type: function
      name: execute
      visibility: public
      parameters:
        - { pattern: value, type: usize }
      return_type: usize
      sketch: worker_execute_body
sketches:
  - worker_execute_body:
      file: platform/worker_impl.rs
      signature: worker_execute
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: exactly_one }
      code: old
"#,
    );

    let response = SketchResolutionFixture::new(ResolveSketchesRequest {
        extraction: RustExtractionInput::Syntax,
        source_files: source,
        contract_files: contracts,
    })
    .resolve()
    .expect("#[path] graph span");

    assert_eq!(response.seeds.len(), 1);
    assert_eq!(response.seeds[0].file.as_str(), "platform/worker_impl.rs");
    assert_eq!(response.seeds[0].sketch_id, "worker_execute_body");
    assert_eq!(
        response.seeds[0].code,
        "pub fn execute(value: usize) -> usize {\n    value + 1\n}"
    );
}

#[test]
fn linked_module_sketch_resolves_the_declaration_span_not_the_child_file() {
    let source = catalog([
        (
            "lib.rs",
            br#"#[path = "platform/worker_impl.rs"]
pub mod worker;
"#
            .as_slice(),
        ),
        (
            "platform/worker_impl.rs",
            b"pub fn execute() {}\n".as_slice(),
        ),
    ]);
    let contracts = catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [lib.rs, platform/worker_impl.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - worker_module:
      crate_id: sample
      file: lib.rs
      signature_type: module
      name: worker
      visibility: public
      path: platform/worker_impl.rs
      sketch: worker_declaration
sketches:
  - worker_declaration:
      file: lib.rs
      signature: worker_module
      signature_type: module
      matching: { normalization: exact_lines_v1, occurrence: exactly_one }
      code: old
"#,
    );

    let response = SketchResolutionFixture::new(ResolveSketchesRequest {
        extraction: RustExtractionInput::Syntax,
        source_files: source,
        contract_files: contracts,
    })
    .resolve()
    .expect("module declaration span");

    assert_eq!(response.seeds.len(), 1);
    assert_eq!(response.seeds[0].file.as_str(), "lib.rs");
    assert_eq!(response.seeds[0].signature_type, "module");
    assert_eq!(
        response.seeds[0].code,
        "#[path = \"platform/worker_impl.rs\"]\npub mod worker;"
    );
}

#[test]
fn linked_sketch_resolver_converts_utf8_columns_to_byte_offsets() {
    let source = catalog_with(
        "lib.rs",
        "const PREFIX: &str = \"é\"; pub fn answer() -> &'static str { \"naïve\" }\n".as_bytes(),
    );
    let contracts = catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - answer_function:
      file: lib.rs
      signature_type: function
      name: answer
      visibility: public
      return_type: "&'static str"
      sketch: answer
sketches:
  - answer:
      file: lib.rs
      signature: answer_function
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: exactly_one }
      code: |
        pub fn answer() -> &'static str { "old" }
"#,
    );

    let response = SketchResolutionFixture::new(ResolveSketchesRequest {
        extraction: RustExtractionInput::Syntax,
        source_files: source,
        contract_files: contracts,
    })
    .resolve()
    .expect("Unicode columns must resolve to byte boundaries");

    assert_eq!(response.seeds.len(), 1);
    assert_eq!(
        response.seeds[0].code,
        "pub fn answer() -> &'static str { \"naïve\" }"
    );
}

#[test]
fn linked_sketch_resolver_matches_declared_and_invoked_macros() {
    let source = catalog_with(
        "lib.rs",
        br#"macro_rules! declared {
    () => {};
}

include!("generated.rs");
"#,
    );
    let contracts = catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - declared_macro:
      file: lib.rs
      signature_type: macro
      name: declared
      tokens: 'macro_rules ! { () => { } ; }'
      sketch: declared
  - include_macro:
      file: lib.rs
      signature_type: macro
      name: include
      tokens: 'include ! ("generated.rs")'
      sketch: included
sketches:
  - declared:
      file: lib.rs
      signature: declared_macro
      signature_type: macro
      matching: { normalization: exact_lines_v1, occurrence: exactly_one }
      code: old
  - included:
      file: lib.rs
      signature: include_macro
      signature_type: macro
      matching: { normalization: exact_lines_v1, occurrence: exactly_one }
      code: old
"#,
    );

    let response = SketchResolutionFixture::new(ResolveSketchesRequest {
        extraction: RustExtractionInput::Syntax,
        source_files: source,
        contract_files: contracts,
    })
    .resolve()
    .expect("declared and invoked macros must both resolve");

    assert_eq!(response.seeds.len(), 2);
    assert_eq!(response.seeds[0].sketch_id, "declared");
    assert_eq!(
        response.seeds[0].code,
        "macro_rules! declared {\n    () => {};\n}"
    );
    assert_eq!(response.seeds[1].sketch_id, "included");
    assert_eq!(response.seeds[1].code, "include!(\"generated.rs\");");
}

#[test]
fn linked_sketch_resolver_selects_each_macro_occurrence() {
    let source = catalog_with(
        "lib.rs",
        b"include!(\"first.rs\");\ninclude!(\"second.rs\");\n",
    );
    let contracts = catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - first_include:
      file: lib.rs
      signature_type: macro
      name: include
      tokens: 'include ! ("first.rs")'
      sketch: first
  - second_include:
      file: lib.rs
      signature_type: macro
      name: include
      tokens: 'include ! ("second.rs")'
      sketch: second
sketches:
  - first:
      file: lib.rs
      signature: first_include
      signature_type: macro
      matching: { normalization: exact_lines_v1, occurrence: exactly_one }
      code: old
  - second:
      file: lib.rs
      signature: second_include
      signature_type: macro
      matching: { normalization: exact_lines_v1, occurrence: exactly_one }
      code: old
"#,
    );

    let response = SketchResolutionFixture::new(ResolveSketchesRequest {
        extraction: RustExtractionInput::Syntax,
        source_files: source,
        contract_files: contracts,
    })
    .resolve()
    .expect("repeated macro sketches must resolve");

    assert_eq!(response.seeds.len(), 2);
    assert_eq!(response.seeds[0].sketch_id, "first");
    assert_eq!(response.seeds[0].code, "include!(\"first.rs\");");
    assert_eq!(response.seeds[1].sketch_id, "second");
    assert_eq!(response.seeds[1].code, "include!(\"second.rs\");");
}

#[test]
fn orphan_and_mismatched_sketch_links_are_rejected() {
    let error = contract_inventory(catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - answer:
      file: lib.rs
      signature_type: function
      visibility: public
      sketch: example
sketches:
  - example:
      file: lib.rs
      signature: answer
      signature_type: struct
      matching: { normalization: exact_lines_v1, occurrence: exactly_one }
      code: |
        fn answer() {}
"#,
    ))
    .expect_err("kind mismatch must fail");

    assert!(
        error
            .to_string()
            .contains("does not match linked signature type")
    );
}
