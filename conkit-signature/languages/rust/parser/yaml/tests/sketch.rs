use super::super::RustSketchResolver;
use super::{catalog, catalog_with, contract_inventory};
use crate::api::ResolveSketchesRequest;
use crate::files::FileCatalog;

#[test]
fn global_contract_collisions_use_one_validation_contract() {
    let cases = [
        (
            "listed source file",
            br#"root: ../src
files: [shared.rs]
signatures: []
sketches: []
"#
            .as_slice(),
            br#"root: ../src
files: [shared.rs]
signatures: []
sketches: []
"#
            .as_slice(),
            "source file shared.rs is listed by both one.yml and two.yaml",
        ),
        (
            "signature label",
            br#"root: ../src
files: [one.rs]
signatures:
  - shared:
      file: one.rs
      signature_type: function
      name: one
sketches: []
"#
            .as_slice(),
            br#"root: ../src
files: [two.rs]
signatures:
  - shared:
      file: two.rs
      signature_type: function
      name: two
sketches: []
"#
            .as_slice(),
            "signature label shared is defined by both one.yml and two.yaml",
        ),
        (
            "sketch id",
            br#"root: ../src
files: [one.rs]
signatures:
  - one:
      file: one.rs
      signature_type: function
      name: one
      sketch: shared
sketches:
  - shared:
    signature_type: function
    code: one
"#
            .as_slice(),
            br#"root: ../src
files: [two.rs]
signatures:
  - two:
      file: two.rs
      signature_type: function
      name: two
      sketch: shared
sketches:
  - shared:
    signature_type: function
    code: two
"#
            .as_slice(),
            "sketch id shared is defined by both one.yml and two.yaml",
        ),
    ];

    for (case, one, two, expected) in cases {
        let contracts = catalog([("one.yml", one), ("two.yaml", two)]);
        let inventory_error = contract_inventory(contracts.clone()).expect_err(case);
        let resolver_error = RustSketchResolver::new(ResolveSketchesRequest {
            source_files: FileCatalog::new(),
            contract_files: contracts,
        })
        .resolve()
        .expect_err(case);

        assert!(
            inventory_error.to_string().contains(expected),
            "{case}: {inventory_error}"
        );
        assert_eq!(
            resolver_error.to_string(),
            inventory_error.to_string(),
            "{case}"
        );
    }
}

#[test]
fn linked_sketch_resolver_returns_exact_source_item_text() {
    let source = catalog_with(
        "main.rs",
        b"fn helper() {}\n\nfn main() {\n    helper();\n}\n",
    );
    let contracts = catalog([
        (
            "main.yml",
            br#"root: ../src
files: [main.rs]
signatures:
  - main:
      file: main.rs
      signature_type: main_method
      sketch: main
sketches:
  - main:
    signature_type: main_method
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

    let response = RustSketchResolver::new(ResolveSketchesRequest {
        source_files: source,
        contract_files: contracts,
    })
    .resolve()
    .expect("resolve");

    assert_eq!(response.seeds.len(), 1);
    assert_eq!(response.seeds[0].sketch_id, "main");
    assert_eq!(response.seeds[0].code, "fn main() {\n    helper();\n}");
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
        br#"root: ../src
files: [lib.rs]
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
    signature_type: function
    code: old
"#,
    );

    let response = RustSketchResolver::new(ResolveSketchesRequest {
        source_files: source,
        contract_files: contracts,
    })
    .resolve()
    .expect("nested item identity must resolve");

    assert_eq!(response.seeds.len(), 1);
    let seed = &response.seeds[0];
    assert_eq!(seed.contract_file.as_str(), "main.yml");
    assert_eq!(seed.file.as_str(), "lib.rs");
    assert_eq!(seed.sketch_id, "nested_answer");
    assert_eq!(seed.signature_type, "function");
    assert_eq!(seed.code, "pub fn answer() -> u8 { 2 }");
}

#[test]
fn linked_sketch_resolver_converts_utf8_columns_to_byte_offsets() {
    let source = catalog_with(
        "lib.rs",
        "const PREFIX: &str = \"é\"; pub fn answer() -> &'static str { \"naïve\" }\n".as_bytes(),
    );
    let contracts = catalog_with(
        "main.yml",
        br#"root: ../src
files: [lib.rs]
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
    signature_type: function
    code: |
      pub fn answer() -> &'static str { "old" }
"#,
    );

    let response = RustSketchResolver::new(ResolveSketchesRequest {
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
        br#"root: ../src
files: [lib.rs]
signatures:
  - declared_macro:
      file: lib.rs
      signature_type: macro
      name: declared
      sketch: declared
  - include_macro:
      file: lib.rs
      signature_type: macro
      name: include
      sketch: included
sketches:
  - declared:
    signature_type: macro
    code: old
  - included:
    signature_type: macro
    code: old
"#,
    );

    let response = RustSketchResolver::new(ResolveSketchesRequest {
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
        br#"root: ../src
files: [lib.rs]
signatures:
  - first_include:
      file: lib.rs
      signature_type: macro
      name: include
      sketch: first
  - second_include:
      file: lib.rs
      signature_type: macro
      name: include
      sketch: second
sketches:
  - first:
    signature_type: macro
    code: old
  - second:
    signature_type: macro
    code: old
"#,
    );

    let response = RustSketchResolver::new(ResolveSketchesRequest {
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
        br#"root: ../src
files: [lib.rs]
signatures:
  - answer:
      file: lib.rs
      signature_type: function
      visibility: public
      sketch: example
sketches:
  - example:
    signature_type: struct
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
