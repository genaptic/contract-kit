use super::{
    RustContractDocuments, RustYamlTestFixture, catalog, catalog_with, contract_inventory,
};
use crate::files::CatalogPath;
use crate::languages::rust::types::associated_item::RustAssociatedItem;
use crate::languages::rust::types::callable_type::RustReceiver;
use crate::languages::rust::types::declaration::RustDeclaration;
use crate::languages::rust::types::primitive_types::RustType;

const EMPTY_V2_DOCUMENT: &[u8] = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction:
  mode: rust_syntax_v2
  profile: rust_api_v1
  crates: [{ id: sample, root: lib.rs, kind: library }]
signatures: []
sketches: []
"#;

#[test]
fn compiler_extraction_persists_required_package_target_and_normalized_context() {
    let bytes = format!(
        r#"contract_version: 2
root: ../src
files: [lib.rs]
extraction:
  mode: rust_compiler_v1
  profile: rust_api_v1
  crates: [{{ id: sample, root: lib.rs, kind: library }}]
  compiler:
    artifact_schema_version: 1
    extractor_version: conkit-test-v1
    compiler_version: rustc-test
    rustdoc_format_version: {}
    target_triple: x86_64-unknown-linux-gnu
    features: [zeta, alpha, zeta]
    cfg_values: [unix, 'target_os="linux"', unix]
    package: sample-package
    target: sample-lib
    macro_expansion: true
    name_resolution: true
signatures: []
sketches: []
"#,
        rustdoc_types::FORMAT_VERSION,
    );
    let documents = RustContractDocuments::parse(
        catalog_with("main.yml", bytes.as_bytes()),
        &crate::limits::YamlLimits::default(),
    )
    .expect("compiler extraction document");
    documents
        .compiler_document(&crate::work::CancellationProbe::new())
        .expect("one compiler signature document");
    let extraction = documents.documents[0]
        .document
        .extraction
        .as_ref()
        .expect("compiler extraction metadata");
    let encoded = serde_json::to_string(extraction).expect("serialized compiler context");

    assert!(encoded.contains("\"package\":\"sample-package\""));
    assert!(encoded.contains("\"target\":\"sample-lib\""));
    assert!(encoded.contains("\"features\":[\"alpha\",\"zeta\"]"));
    assert!(encoded.contains("\"cfg_values\":[\"target_os=\\\"linux\\\"\",\"unix\"]"));
}

#[test]
fn compiler_extraction_requires_package_and_target_identity() {
    let template = format!(
        r#"contract_version: 2
root: ../src
files: [lib.rs]
extraction:
  mode: rust_compiler_v1
  profile: rust_api_v1
  crates: [{{ id: sample, root: lib.rs, kind: library }}]
  compiler:
    artifact_schema_version: 1
    extractor_version: conkit-test-v1
    compiler_version: rustc-test
    rustdoc_format_version: {}
    target_triple: x86_64-unknown-linux-gnu
    features: []
    cfg_values: []
    package: sample-package
    target: sample-lib
    macro_expansion: true
    name_resolution: true
signatures: []
sketches: []
"#,
        rustdoc_types::FORMAT_VERSION,
    );
    for (field, line) in [
        ("package", "    package: sample-package\n"),
        ("target", "    target: sample-lib\n"),
    ] {
        let bytes = template.replacen(line, "", 1);
        let error = match RustContractDocuments::parse(
            catalog_with("main.yml", bytes.as_bytes()),
            &crate::limits::YamlLimits::default(),
        ) {
            Ok(_) => panic!("compiler package and target identity are mandatory"),
            Err(error) => error,
        };
        assert!(error.to_string().contains(field), "{field}: {error}");
    }
}

#[test]
fn contract_item_budget_counts_embedded_trait_items_at_the_boundary() {
    let contract = catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - storage:
      crate_id: sample
      file: lib.rs
      signature_type: trait
      name: Storage
      visibility: public
      items:
        - signature_type: associated_type
          name: Error
        - signature_type: associated_constant
          name: DURABLE
          type: bool
sketches: []
"#,
    );
    let yaml_limits = crate::limits::YamlLimits::default();
    let exact_limits = crate::limits::RustExtractionLimits {
        items: 3,
        ..crate::limits::RustExtractionLimits::default()
    };
    let mut exact_usage = exact_limits.usage();
    let mut exact_yaml_usage = yaml_limits.usage();
    super::super::RustContractDocuments::parse(
        contract.clone(),
        &mut exact_yaml_usage,
        &mut exact_usage,
        &crate::work::CancellationProbe::new(),
    )
    .expect("trait plus two associated items must fit the exact boundary");

    let crossing_limits = crate::limits::RustExtractionLimits {
        items: 2,
        ..crate::limits::RustExtractionLimits::default()
    };
    let mut crossing_usage = crossing_limits.usage();
    let mut crossing_yaml_usage = yaml_limits.usage();
    let result = super::super::RustContractDocuments::parse(
        contract,
        &mut crossing_yaml_usage,
        &mut crossing_usage,
        &crate::work::CancellationProbe::new(),
    );
    let Err(error) = result else {
        panic!("the third modeled item must cross the limit");
    };
    let limit = error.limit_exceeded().expect("typed item limit");
    assert_eq!(limit.resource, crate::limits::LimitResource::RustItemCount);
    assert_eq!(limit.limit, 2);
    assert_eq!(limit.observed_at_least, 3);
}

#[test]
fn contract_version_two_is_mandatory_and_future_versions_fail_closed() {
    let cases = [
        (
            "missing",
            b"root: ../src\nfiles: []\nsignatures: []\nsketches: []\n".as_slice(),
            "unsupported contract version missing",
        ),
        (
            "legacy",
            b"contract_version: 1\nroot: ../src\nfiles: []\nsignatures: []\nsketches: []\n"
                .as_slice(),
            "recreate",
        ),
        (
            "future",
            b"contract_version: 3\nroot: ../src\nfiles: []\nsignatures: []\nsketches: []\n"
                .as_slice(),
            "unsupported contract version 3",
        ),
    ];

    for (case, bytes, expected) in cases {
        let error = contract_inventory(catalog_with("main.yml", bytes))
            .expect_err("non-v2 documents must fail closed");
        assert!(error.to_string().contains(expected), "{case}: {error}");
    }

    contract_inventory(catalog_with("main.yml", EMPTY_V2_DOCUMENT))
        .expect("v2 document must parse");
}

#[test]
fn signatures_and_sketches_are_mandatory_v2_root_fields() {
    let source = std::str::from_utf8(EMPTY_V2_DOCUMENT).expect("UTF-8 fixture");

    for (field, line) in [
        ("signatures", "signatures: []\n"),
        ("sketches", "sketches: []\n"),
    ] {
        let bytes = source.replacen(line, "", 1);
        let error = contract_inventory(catalog_with("main.yml", bytes.as_bytes()))
            .expect_err("mandatory v2 collection root must be rejected when absent");
        let rendered = error.to_string();

        assert!(
            rendered.contains(&format!("missing field `{field}`")),
            "{rendered}"
        );
        assert!(rendered.contains("main.yml document 0"), "{rendered}");
    }
}

#[test]
fn duplicate_and_unknown_v2_root_keys_are_rejected() {
    let cases = [
        (
            "duplicate",
            br#"contract_version: 2
contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures: []
sketches: []
"#
            .as_slice(),
            "duplicate",
        ),
        (
            "unknown",
            br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures: []
sketches: []
surprise: true
"#
            .as_slice(),
            "unknown field",
        ),
    ];

    for (case, bytes, expected) in cases {
        let error = contract_inventory(catalog_with("main.yml", bytes))
            .expect_err("invalid root mapping must fail");
        assert!(error.to_string().contains(expected), "{case}: {error}");
    }
}

#[test]
fn nested_v2_sketch_shape_preserves_signature_owned_linkage_and_matching_metadata() {
    let bytes = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - answer_function:
      file: lib.rs
      signature_type: function
      name: answer
      visibility: public
      sketch: answer_example
sketches:
  - answer_example:
      file: lib.rs
      signature: answer_function
      signature_type: function
      matching: { normalization: sketch_owned_policy_v17, occurrence: custom_occurrence }
      code: |-
        pub fn answer() -> i32 { 0 }
"#;
    let documents = RustContractDocuments::parse(
        catalog_with("main.yml", bytes),
        &crate::limits::YamlLimits::default(),
    )
    .expect("nested v2 sketch document");
    let document = &documents.documents[0].document;
    let sketch = &document.sketches[0];

    assert_eq!(sketch.id, "answer_example");
    assert_eq!(sketch.file.as_str(), "lib.rs");
    assert_eq!(sketch.signature, "answer_function");
    assert_eq!(sketch.signature_type, "function");
    assert_eq!(sketch.matching.normalization, "sketch_owned_policy_v17");
    assert_eq!(sketch.matching.occurrence, "custom_occurrence");
    documents
        .into_inventory(&crate::work::CancellationProbe::new())
        .expect("linked inventory");
}

#[test]
fn flattened_incomplete_and_mismatched_v2_sketches_fail_closed() {
    let cases = [
        (
            "flattened",
            "  - answer_example: null\n    file: lib.rs\n    signature: answer_function\n    signature_type: function\n    matching: { normalization: exact_lines_v1, occurrence: at_least_one }\n    code: old",
            "flattened sketch fields are not supported",
        ),
        (
            "missing file",
            "  - answer_example:\n      signature: answer_function\n      signature_type: function\n      matching: { normalization: exact_lines_v1, occurrence: at_least_one }\n      code: old",
            "Rust YAML catalog paths must use shorthand text",
        ),
        (
            "missing signature",
            "  - answer_example:\n      file: lib.rs\n      signature_type: function\n      matching: { normalization: exact_lines_v1, occurrence: at_least_one }\n      code: old",
            "missing field `signature`",
        ),
        (
            "missing signature type",
            "  - answer_example:\n      file: lib.rs\n      signature: answer_function\n      matching: { normalization: exact_lines_v1, occurrence: at_least_one }\n      code: old",
            "missing field `signature_type`",
        ),
        (
            "missing matching",
            "  - answer_example:\n      file: lib.rs\n      signature: answer_function\n      signature_type: function\n      code: old",
            "missing field `matching`",
        ),
        (
            "wrong file",
            "  - answer_example:\n      file: other.rs\n      signature: answer_function\n      signature_type: function\n      matching: { normalization: exact_lines_v1, occurrence: at_least_one }\n      code: old",
            "does not match linked signature file lib.rs",
        ),
        (
            "wrong signature",
            "  - answer_example:\n      file: lib.rs\n      signature: other_function\n      signature_type: function\n      matching: { normalization: exact_lines_v1, occurrence: at_least_one }\n      code: old",
            "does not match linked signature answer_function",
        ),
        (
            "wrong signature type",
            "  - answer_example:\n      file: lib.rs\n      signature: answer_function\n      signature_type: struct\n      matching: { normalization: exact_lines_v1, occurrence: at_least_one }\n      code: old",
            "does not match linked signature type function",
        ),
    ];

    for (case, sketch, expected) in cases {
        let bytes = format!(
            "contract_version: 2\nroot: ../src\nfiles: [lib.rs, other.rs]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: sample, root: lib.rs, kind: library }}] }}\nsignatures:\n  - answer_function:\n      file: lib.rs\n      signature_type: function\n      name: answer\n      visibility: public\n      sketch: answer_example\nsketches:\n{sketch}\n"
        );
        let error = contract_inventory(catalog_with("main.yml", bytes.as_bytes()))
            .expect_err("invalid nested v2 sketch must fail");
        assert!(error.to_string().contains(expected), "{case}: {error}");
    }
}

#[test]
fn duplicate_yaml_key_error_retains_key_physical_document_and_source_location() {
    let bytes = br#"---
contract_version: 2
root: ../src
files: [a.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: a, root: a.rs, kind: library }] }
signatures: []
sketches: []
---
contract_version: 2
root: ../src
root: duplicate
files: [b.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: b, root: b.rs, kind: library }] }
signatures: []
sketches: []
"#;
    let error = contract_inventory(catalog_with("main.yml", bytes))
        .expect_err("duplicate key in second physical document must fail");
    let rendered = error.to_string();
    let cloned = error.clone();

    assert!(rendered.contains("duplicate YAML key root"), "{rendered}");
    assert!(rendered.contains("main.yml document 1"), "{rendered}");
    assert!(rendered.contains("line"), "{rendered}");
    assert!(rendered.contains("column"), "{rendered}");
    assert_eq!(cloned.to_string(), rendered);
}

#[test]
fn merge_keys_and_invalid_utf8_are_rejected_before_semantic_conversion() {
    let merge = br#"<<: { signatures: [] }
contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
sketches: []
"#;
    let merge_error =
        contract_inventory(catalog_with("main.yml", merge)).expect_err("merge keys are disabled");
    assert!(merge_error.to_string().contains("merge"), "{merge_error}");

    let invalid_utf8 = [
        b'c', b'o', b'n', b't', b'r', b'a', b'c', b't', b'_', b'v', b'e', b'r', b's', b'i', b'o',
        b'n', b':', b' ', 0xff,
    ];
    let utf8_error = contract_inventory(catalog_with("main.yml", &invalid_utf8))
        .expect_err("invalid UTF-8 must fail");
    assert!(utf8_error.to_string().contains("UTF-8"), "{utf8_error}");
}

#[test]
fn physical_yaml_file_retains_original_bytes_and_document_indexes() {
    let bytes = br#"# first document
---
contract_version: 2
root: ../src
files: [a.rs]
extraction:
  mode: rust_syntax_v2
  profile: rust_api_v1
  crates: [{ id: a, root: a.rs, kind: library }]
signatures: []
sketches: []
---
# second document
contract_version: 2
root: ../src
files: [b.rs]
extraction:
  mode: rust_syntax_v2
  profile: rust_api_v1
  crates: [{ id: b, root: b.rs, kind: library }]
signatures: []
sketches: []
"#;
    let documents = RustContractDocuments::parse(
        catalog_with("main.yml", bytes),
        &crate::limits::YamlLimits::default(),
    )
    .expect("multi-document physical file");

    assert_eq!(documents.documents.len(), 2);
    assert_eq!(documents.documents[0].location.document_index(), 0);
    assert_eq!(documents.documents[1].location.document_index(), 1);
    assert_eq!(documents.documents[0].original_bytes.as_ref(), bytes);
    assert_eq!(documents.documents[1].original_bytes.as_ref(), bytes);
}

#[test]
fn extraction_roots_must_be_listed_and_extraction_shape_is_strict() {
    let cases = [
        (
            "unlisted root",
            br#"contract_version: 2
root: ../src
files: []
extraction:
  mode: rust_syntax_v2
  profile: rust_api_v1
  crates: [{ id: app, root: lib.rs, kind: library }]
signatures: []
sketches: []
"#
            .as_slice(),
            "crate root lib.rs must appear in files",
        ),
        (
            "empty crate set",
            br#"contract_version: 2
root: ../src
files: []
extraction:
  mode: rust_syntax_v2
  profile: rust_api_v1
  crates: []
signatures: []
sketches: []
"#
            .as_slice(),
            "at least one crate",
        ),
        (
            "surrounding crate id whitespace",
            br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction:
  mode: rust_syntax_v2
  profile: rust_api_v1
  crates: [{ id: ' app ', root: lib.rs, kind: library }]
signatures: []
sketches: []
"#
            .as_slice(),
            "must not contain whitespace",
        ),
        (
            "unknown extraction field",
            br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction:
  mode: rust_syntax_v2
  profile: rust_api_v1
  crates: [{ id: app, root: lib.rs, kind: library }]
  surprise: true
signatures: []
sketches: []
"#
            .as_slice(),
            "unknown field",
        ),
        (
            "unsupported extraction mode",
            br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction:
  mode: rust_syntax_v3
  profile: rust_api_v1
  crates: [{ id: app, root: lib.rs, kind: library }]
signatures: []
sketches: []
"#
            .as_slice(),
            "rust_syntax_v3",
        ),
        (
            "unsupported extraction profile",
            br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction:
  mode: rust_syntax_v2
  profile: rust_api_v2
  crates: [{ id: app, root: lib.rs, kind: library }]
signatures: []
sketches: []
"#
            .as_slice(),
            "rust_api_v2",
        ),
    ];

    for (case, bytes, expected) in cases {
        let error = contract_inventory(catalog_with("main.yml", bytes))
            .expect_err("invalid extraction context must fail");
        assert!(error.to_string().contains(expected), "{case}: {error}");
    }
}

#[test]
fn multi_crate_document_requires_and_retains_signature_crate_identity() {
    let bytes = br#"contract_version: 2
root: ../src
files: [shared.rs]
extraction:
  mode: rust_syntax_v2
  profile: rust_api_v1
  crates:
    - { id: library_api, root: shared.rs, kind: library }
    - { id: binary_api, root: shared.rs, kind: binary }
signatures:
  - library_shared:
      crate_id: library_api
      file: shared.rs
      signature_type: function
      name: shared
      visibility: public
  - binary_shared:
      crate_id: binary_api
      file: shared.rs
      signature_type: function
      name: shared
      visibility: private
sketches: []
"#;
    let documents = RustContractDocuments::parse(
        catalog_with("main.yml", bytes),
        &crate::limits::YamlLimits::default(),
    )
    .expect("same physical source in two explicit crate contexts");
    let document = &documents.documents[0].document;
    let crate_ids = document
        .signatures
        .iter()
        .map(|signature| signature.crate_id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(crate_ids, ["library_api", "binary_api"]);
    assert_eq!(
        documents
            .into_inventory(&crate::work::CancellationProbe::new())
            .expect("multi-crate inventory")
            .len(),
        2
    );

    let missing = String::from_utf8(bytes.to_vec())
        .expect("UTF-8 fixture")
        .replacen("      crate_id: library_api\n", "", 1);
    let missing_error = contract_inventory(catalog_with("main.yml", missing.as_bytes()))
        .expect_err("ambiguous missing crate identity must fail closed");
    assert!(
        missing_error.to_string().contains(
            "signature library_shared requires crate_id because extraction declares multiple crates"
        ),
        "{missing_error}"
    );

    let unknown = String::from_utf8(bytes.to_vec())
        .expect("UTF-8 fixture")
        .replacen("crate_id: binary_api", "crate_id: missing_api", 1);
    let unknown_error = contract_inventory(catalog_with("main.yml", unknown.as_bytes()))
        .expect_err("unknown crate identity must fail closed");
    assert!(
        unknown_error
            .to_string()
            .contains("signature binary_shared references unknown crate_id missing_api"),
        "{unknown_error}"
    );
}

#[test]
fn single_crate_document_immediately_canonicalizes_an_omitted_crate_id() {
    let documents = RustContractDocuments::parse(
        catalog_with(
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
sketches: []
"#,
        ),
        &crate::limits::YamlLimits::default(),
    )
    .expect("unambiguous single-crate shorthand");

    assert_eq!(
        documents.documents[0].document.signatures[0]
            .crate_id
            .as_str(),
        "sample"
    );
}

#[test]
fn final_yaml_rejects_main_method_and_models_main_as_an_ordinary_function() {
    let ordinary = br#"contract_version: 2
root: ../src
files: [main.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: app, root: main.rs, kind: binary }] }
signatures:
  - main_function:
      crate_id: app
      file: main.rs
      signature_type: function
      name: main
      visibility: private
sketches: []
"#;
    contract_inventory(catalog_with("main.yml", ordinary))
        .expect("ordinary binary-root main function");

    let legacy = String::from_utf8(ordinary.to_vec())
        .expect("UTF-8 fixture")
        .replace(
            "signature_type: function\n      name: main",
            "signature_type: main_method",
        );
    let error = contract_inventory(catalog_with("main.yml", legacy.as_bytes()))
        .expect_err("main_method must not survive the v2 migration");

    assert!(
        error.to_string().contains("unknown variant `main_method`"),
        "{error}"
    );
}

#[test]
fn exhaustive_declaration_foreign_and_associated_item_yaml_is_accepted() {
    let bytes = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - limit_constant:
      crate_id: sample
      file: lib.rs
      signature_type: constant
      name: LIMIT
      visibility: public
      type: usize
      value: '4'
  - choice_enum:
      crate_id: sample
      file: lib.rs
      signature_type: enum
      name: Choice
      visibility: public
      variants: [Ready]
  - rust_core_extern_crate:
      crate_id: sample
      file: lib.rs
      signature_type: extern_crate
      name: core
      alias: rust_core
      visibility: public
  - execute_function:
      crate_id: sample
      file: lib.rs
      signature_type: function
      name: execute
      visibility: public
      parameters:
        - { pattern: value, type: usize }
      return_type: usize
  - native_foreign_module:
      crate_id: sample
      file: lib.rs
      signature_type: foreign_module
      abi: C
      unsafe: true
      items:
        - signature_type: foreign_function
          name: native_execute
          visibility: public
          parameters:
            - { pattern: value, type: i32 }
          return_type: i32
        - signature_type: foreign_static
          name: NATIVE_LIMIT
          visibility: public
          mutable: true
          type: usize
        - signature_type: foreign_type
          name: NativeHandle
          visibility: public
        - signature_type: foreign_macro
          tokens: native_items ! ()
  - handler_struct:
      crate_id: sample
      file: lib.rs
      signature_type: struct
      name: Handler
      visibility: public
      implementations:
        - trait: Service
          items:
            - signature_type: associated_constant
              name: LIMIT
              type: usize
              default_value: '4'
            - signature_type: associated_type
              name: Output
              default_type: usize
            - signature_type: method
              name: execute
              receiver: ref
              parameters:
                - { pattern: value, type: usize }
              return_type: usize
  - contract_macro:
      crate_id: sample
      file: lib.rs
      signature_type: macro
      name: contract_item
      tokens: contract_item ! ()
  - transport_module:
      crate_id: sample
      file: lib.rs
      signature_type: module
      name: transport
      visibility: public
      inline: false
      path: platform/transport.rs
      attributes:
        - cfg: 'feature = "transport"'
  - inline_module:
      crate_id: sample
      file: lib.rs
      signature_type: module
      name: inline
      visibility: crate
      inline: true
  - global_static:
      crate_id: sample
      file: lib.rs
      signature_type: static
      name: GLOBAL
      visibility: public
      mutable: false
      type: usize
  - service_trait:
      crate_id: sample
      file: lib.rs
      signature_type: trait
      name: Service
      visibility: public
      items:
        - signature_type: associated_constant
          name: LIMIT
          type: usize
        - signature_type: associated_type
          name: Output
          bounds: [Clone]
        - signature_type: method
          name: execute
          receiver: ref
          parameters:
            - { pattern: value, type: usize }
          return_type: usize
  - service_alias_trait_alias:
      crate_id: sample
      file: lib.rs
      signature_type: trait_alias
      name: ServiceAlias
      visibility: public
      supertraits: [Send, Sync]
  - value_type_alias:
      crate_id: sample
      file: lib.rs
      signature_type: type_alias
      name: Value
      visibility: public
      target_type: usize
  - number_union:
      crate_id: sample
      file: lib.rs
      signature_type: union
      name: Number
      visibility: public
      fields: { integer: u32, float: f32 }
  - public_handler_reexport:
      crate_id: sample
      file: lib.rs
      signature_type: reexport
      path: crate::internal::Handler
      alias: PublicHandler
      visibility: public
sketches: []
"#;
    let documents = RustContractDocuments::parse(
        catalog_with("main.yml", bytes),
        &crate::limits::YamlLimits::default(),
    )
    .expect("complete declaration YAML");
    let signature_types = documents.documents[0]
        .document
        .signatures
        .iter()
        .map(|signature| signature.signature_type.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        signature_types,
        [
            "constant",
            "enum",
            "extern_crate",
            "function",
            "foreign_module",
            "struct",
            "macro",
            "module",
            "module",
            "static",
            "trait",
            "trait_alias",
            "type_alias",
            "union",
            "reexport",
        ]
    );
    documents
        .into_inventory(&crate::work::CancellationProbe::new())
        .expect("all declaration families convert to inventory");
}

#[test]
fn module_yaml_shape_path_visibility_and_attributes_change_inventory_identity() {
    let base = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - transport_module:
      crate_id: sample
      file: lib.rs
      signature_type: module
      name: transport
      visibility: public
      inline: false
      path: platform/transport.rs
      attributes:
        - must_use: load transport
sketches: []
"#;
    let base_inventory =
        contract_inventory(catalog_with("main.yml", base)).expect("base module contract");
    let base_digest = base_inventory
        .source_shape_digest(&crate::work::CancellationProbe::new())
        .expect("base source-shape digest");
    let base = String::from_utf8(base.to_vec()).expect("UTF-8 fixture");
    let changed_documents = [
        base.replace("inline: false", "inline: true"),
        base.replace("platform/transport.rs", "platform/alternate.rs"),
        base.replace("visibility: public", "visibility: private"),
        base.replace("        - must_use: load transport\n", ""),
    ];

    for changed in changed_documents {
        let changed = contract_inventory(catalog_with("main.yml", changed.as_bytes()))
            .expect("changed module contract");
        assert_ne!(
            base_digest,
            changed
                .source_shape_digest(&crate::work::CancellationProbe::new())
                .expect("changed source-shape digest"),
            "module shape, path, visibility, and attributes are API semantics"
        );
    }
}

#[test]
fn repeated_conditional_module_yaml_is_rejected_before_cfg_evaluation() {
    let bytes = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - unix_platform_module:
      crate_id: sample
      file: lib.rs
      signature_type: module
      name: platform
      attributes:
        - cfg: unix
  - windows_platform_module:
      crate_id: sample
      file: lib.rs
      signature_type: module
      name: platform
      attributes:
        - cfg: windows
sketches: []
"#;
    let result = RustContractDocuments::parse(
        catalog_with("main.yml", bytes),
        &crate::limits::YamlLimits::default(),
    );
    let Err(error) = result else {
        panic!("syntax extraction cannot select between duplicate modules");
    };
    let rendered = error.to_string();

    assert!(
        rendered.contains("duplicate Rust module identity"),
        "{rendered}"
    );
    assert!(rendered.contains("platform"), "{rendered}");
}

#[test]
fn ordered_typed_attributes_round_trip_at_nested_yaml_placements() {
    let bytes = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - probe_function:
      crate_id: sample
      file: lib.rs
      signature_type: function
      name: probe
      visibility: public
      attributes:
        - derive: [Clone, serde::Serialize, Clone]
        - repr: [C, align(16), packed(2)]
        - non_exhaustive
        - cfg: 'all(unix, feature = "fast")'
        - cfg_attr:
            predicate: 'target_os = "windows"'
            attributes:
              - must_use: consume
        - deprecated: { since: 2.0.0, note: use replacement }
        - must_use: inspect the result
        - doc_hidden
        - no_mangle
        - export_name: contract_probe
        - link_section: .contract
        - link_name: native_probe
        - link: { name: contract_native, kind: static, modifiers: +bundle }
        - unresolved:
            path: contract_runtime
            arguments: { style: list, value: 'mode = "stable"' }
      parameters:
        - pattern: value
          type: usize
          attributes:
            - cfg: 'feature = "argument"'
      return_type: usize
  - service_trait:
      crate_id: sample
      file: lib.rs
      signature_type: trait
      name: Service
      visibility: public
      items:
        - signature_type: associated_type
          name: Output
          attributes:
            - deprecated: { note: use NewOutput }
        - signature_type: method
          name: execute
          receiver: ref
          attributes:
            - must_use: inspect the result
  - native_foreign_module:
      crate_id: sample
      file: lib.rs
      signature_type: foreign_module
      abi: C
      attributes:
        - link: { name: contract_native, kind: static }
      items:
        - signature_type: foreign_function
          name: native_probe
          visibility: public
          attributes:
            - link_name: native_probe_v2
sketches: []
"#;
    let with_attributes = contract_inventory(catalog_with("main.yml", bytes))
        .expect("typed attributes at every nested YAML owner");
    let without_must_use = String::from_utf8(bytes.to_vec())
        .expect("UTF-8 fixture")
        .replacen("        - must_use: inspect the result\n", "", 1);
    let without_must_use =
        contract_inventory(catalog_with("main.yml", without_must_use.as_bytes()))
            .expect("contract without one semantic attribute");

    assert_ne!(
        with_attributes
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("attributed source-shape digest"),
        without_must_use
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("unattributed source-shape digest"),
        "removing a known semantic attribute must alter API identity"
    );
}

#[test]
fn yaml_alignment_one_packing_has_one_semantic_inventory_representation() {
    let template = r#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - packet:
      crate_id: sample
      file: lib.rs
      signature_type: struct
      name: Packet
      visibility: public
      attributes:
        - repr: [PACKING]
sketches: []
"#;
    let implicit = contract_inventory(catalog_with(
        "main.yml",
        template.replace("PACKING", "packed").as_bytes(),
    ))
    .expect("bare packing contract");
    let explicit = contract_inventory(catalog_with(
        "main.yml",
        template.replace("PACKING", "packed(1)").as_bytes(),
    ))
    .expect("explicit alignment-one packing contract");
    let wider = contract_inventory(catalog_with(
        "main.yml",
        template.replace("PACKING", "packed(2)").as_bytes(),
    ))
    .expect("wider packing contract");

    assert_eq!(implicit, explicit);
    assert_ne!(implicit, wider);
}

#[test]
fn parameter_pattern_text_round_trips_without_affecting_api_identity() {
    let identifier = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - submit_function:
      crate_id: sample
      file: lib.rs
      signature_type: function
      name: submit
      visibility: public
      parameters:
        - { pattern: request, type: '(Request, usize)' }
sketches: []
"#;
    let destructured = String::from_utf8(identifier.to_vec())
        .expect("UTF-8 fixture")
        .replace("pattern: request", "pattern: '(request, attempts)'");
    let changed_type = String::from_utf8(identifier.to_vec())
        .expect("UTF-8 fixture")
        .replace("'(Request, usize)'", "'(Request, u64)'");
    let legacy = String::from_utf8(identifier.to_vec())
        .expect("UTF-8 fixture")
        .replace(
            "- { pattern: request, type: '(Request, usize)' }",
            "- request: '(Request, usize)'",
        );
    let identifier = contract_inventory(catalog_with("main.yml", identifier))
        .expect("identifier parameter contract");
    let destructured = contract_inventory(catalog_with("main.yml", destructured.as_bytes()))
        .expect("destructured parameter contract");
    let changed_type = contract_inventory(catalog_with("main.yml", changed_type.as_bytes()))
        .expect("changed parameter type contract");

    assert_eq!(
        identifier
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("identifier source-shape digest"),
        destructured
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("destructured source-shape digest"),
        "binding and destructuring text must be excluded from rust_api_v1"
    );
    assert_ne!(
        identifier
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("identifier source-shape digest"),
        changed_type
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("changed-type source-shape digest"),
        "parameter type remains API semantic"
    );

    let error = contract_inventory(catalog_with("main.yml", legacy.as_bytes()))
        .expect_err("one-entry parameter maps must not survive v2");
    assert!(
        error
            .to_string()
            .contains("parameters must use explicit pattern and type fields"),
        "{error}"
    );
}

#[test]
fn yaml_rust_syntax_text_is_canonical_across_formatting_only_variants() {
    let compact = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - limit_constant:
      crate_id: sample
      file: lib.rs
      signature_type: constant
      name: LIMIT
      type: usize
      value: '1+2'
  - choice_enum:
      crate_id: sample
      file: lib.rs
      signature_type: enum
      name: Choice
      variants:
        - Ready: { discriminant: '1+2' }
  - service_alias_trait_alias:
      crate_id: sample
      file: lib.rs
      signature_type: trait_alias
      name: ServiceAlias
      supertraits: ['for<''a> Service<&''a Request>']
  - service_trait:
      crate_id: sample
      file: lib.rs
      signature_type: trait
      name: Service
      generics: [T]
      where: ['T:Clone']
      items:
        - signature_type: associated_constant
          name: LIMIT
          type: usize
          default_value: '1+2'
        - signature_type: associated_type
          name: Output
          bounds: ['Service<Item=T>']
        - signature_type: method
          name: submit
          abi: C
          variadic: { pattern: args }
          parameters: [{ pattern: '(request,_)', type: '(Request, usize)' }]
sketches: []
"#;
    let spaced = String::from_utf8(compact.to_vec())
        .expect("UTF-8 fixture")
        .replace("'1+2'", "' 1 + 2 '")
        .replace(
            "for<''a> Service<&''a Request>",
            "for < ''a > Service < & ''a Request >",
        )
        .replace("'T:Clone'", "' T : Clone '")
        .replace("Service<Item=T>", "Service < Item = T >")
        .replace("pattern: args", "pattern: ' args '")
        .replace("'(request,_)'", "' ( request , _ ) '");

    let compact =
        contract_inventory(catalog_with("main.yml", compact)).expect("compact syntax document");
    let spaced = contract_inventory(catalog_with("main.yml", spaced.as_bytes()))
        .expect("spaced syntax document");

    assert_eq!(
        compact
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("compact source-shape digest"),
        spaced
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("spaced source-shape digest")
    );
}

#[test]
fn method_receivers_parse_into_closed_semantic_forms() {
    let base = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - service_trait:
      crate_id: sample
      file: lib.rs
      signature_type: trait
      name: Service
      visibility: public
      items:
        - signature_type: method
          name: execute
          receiver: ref
sketches: []
"#;
    let contract = |receiver: &str| {
        let yaml = String::from_utf8(base.to_vec())
            .expect("UTF-8 receiver fixture")
            .replace("receiver: ref", &format!("receiver: {receiver}"));
        contract_inventory(catalog_with("main.yml", yaml.as_bytes()))
    };

    let shared = contract("ref").expect("shared receiver");
    let explicit_shared = contract("'&self'").expect("explicit shared receiver");
    let mutable = contract("mut").expect("mutable reference receiver");
    let owned = contract("self").expect("owned receiver");
    let typed = contract("'self: Box<Self>'").expect("typed receiver");

    let shared_digest = shared
        .source_shape_digest(&crate::work::CancellationProbe::new())
        .expect("shared-receiver source-shape digest");
    assert_eq!(
        shared_digest,
        explicit_shared
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("explicit-shared source-shape digest")
    );
    assert_ne!(
        shared_digest,
        mutable
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("mutable-receiver source-shape digest")
    );
    assert_ne!(
        shared_digest,
        owned
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("owned-receiver source-shape digest")
    );
    assert_ne!(
        shared_digest,
        typed
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("typed-receiver source-shape digest")
    );

    for invalid in ["static", "'request'", "'&mut value'"] {
        let error = contract(invalid).expect_err("invalid receiver must fail closed");
        assert!(error.to_string().contains("receiver"), "{invalid}: {error}");
    }
}

#[test]
fn yaml_generic_defaults_survive_syn_three_tuple_containers() {
    let template = r#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - packet_struct:
      crate_id: sample
      file: lib.rs
      signature_type: struct
      name: Packet
      visibility: public
      generics: ['T = TYPE_DEFAULT', 'const N: usize = CONST_DEFAULT']
sketches: []
"#;
    let contract = |type_default: &str, const_default: &str| {
        let yaml = template
            .replace("TYPE_DEFAULT", type_default)
            .replace("CONST_DEFAULT", const_default);
        contract_inventory(catalog_with("main.yml", yaml.as_bytes()))
            .expect("generic-default contract")
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("generic-default source-shape digest")
    };

    let baseline = contract("String", "4");
    assert_ne!(baseline, contract("Vec<u8>", "4"));
    assert_ne!(baseline, contract("String", "8"));
}

#[test]
fn typed_method_receivers_resolve_method_generic_parameters() {
    let bytes = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - service_trait:
      file: lib.rs
      signature_type: trait
      name: Service
      items:
        - signature_type: method
          name: consume
          generics: [T]
          receiver: 'self: T'
sketches: []
"#;
    let documents = RustContractDocuments::parse(
        catalog_with("main.yml", bytes),
        &crate::limits::YamlLimits::default(),
    )
    .expect("method-generic typed receiver contract");
    let declaration = documents.documents[0].document.signatures[0].entries[0].declaration();
    let RustDeclaration::Trait(trait_type) = declaration else {
        panic!("fixture must produce a trait declaration");
    };
    let RustAssociatedItem::Method(method) = &trait_type.items()[0] else {
        panic!("fixture must produce a trait method");
    };
    let Some(RustReceiver::Typed { receiver_type, .. }) = method.receiver() else {
        panic!("fixture must produce a typed receiver");
    };

    assert_eq!(receiver_type, &RustType::GenericParameter("T".to_owned()));
}

#[test]
fn omitted_trait_visibility_is_private_and_reexports_are_public_only() {
    let trait_yaml = "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }\nsignatures:\n  - service_trait:\n      file: lib.rs\n      signature_type: trait\n      name: Service\n      items: []\nsketches: []\n";
    let omitted_trait = contract_inventory(catalog_with("main.yml", trait_yaml.as_bytes()))
        .expect("trait with omitted visibility");
    let private_trait = contract_inventory(catalog_with(
        "main.yml",
        trait_yaml
            .replace(
                "      items: []",
                "      visibility: private\n      items: []",
            )
            .as_bytes(),
    ))
    .expect("explicitly private trait");
    let public_trait = contract_inventory(catalog_with(
        "main.yml",
        trait_yaml
            .replace(
                "      items: []",
                "      visibility: public\n      items: []",
            )
            .as_bytes(),
    ))
    .expect("explicitly public trait");

    assert_eq!(
        omitted_trait
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("omitted-trait source-shape digest"),
        private_trait
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("private-trait source-shape digest")
    );
    assert_ne!(
        omitted_trait
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("omitted-trait source-shape digest"),
        public_trait
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("public-trait source-shape digest")
    );

    let reexport_yaml = "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }\nsignatures:\n  - public_handler:\n      file: lib.rs\n      signature_type: reexport\n      path: crate::internal::Handler\n      alias: PublicHandler\nsketches: []\n";
    let omitted_reexport = contract_inventory(catalog_with("main.yml", reexport_yaml.as_bytes()))
        .expect("re-export with omitted visibility");
    let public_reexport = contract_inventory(catalog_with(
        "main.yml",
        reexport_yaml
            .replace(
                "      path: crate::internal::Handler",
                "      visibility: public\n      path: crate::internal::Handler",
            )
            .as_bytes(),
    ))
    .expect("explicitly public re-export");
    let crate_reexport = contract_inventory(catalog_with(
        "main.yml",
        reexport_yaml
            .replace(
                "      path: crate::internal::Handler",
                "      visibility: crate\n      path: crate::internal::Handler",
            )
            .as_bytes(),
    ))
    .expect("restricted non-private re-export");
    let private_error = contract_inventory(catalog_with(
        "main.yml",
        reexport_yaml
            .replace(
                "      path: crate::internal::Handler",
                "      visibility: private\n      path: crate::internal::Handler",
            )
            .as_bytes(),
    ))
    .expect_err("private imports are resolution-only and cannot be contract re-exports");

    assert_eq!(
        omitted_reexport
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("omitted-reexport source-shape digest"),
        public_reexport
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("public-reexport source-shape digest")
    );
    assert_ne!(
        omitted_reexport
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("omitted-reexport source-shape digest"),
        crate_reexport
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("crate-reexport source-shape digest")
    );
    assert!(
        private_error.to_string().contains("non-private"),
        "{private_error}"
    );
}

#[test]
fn root_and_every_physical_document_are_validated() {
    let empty_root = EMPTY_V2_DOCUMENT
        .strip_prefix(b"contract_version: 2\nroot: ../src\n")
        .map(|rest| [b"contract_version: 2\nroot: '   '\n".as_slice(), rest].concat())
        .expect("fixture prefix");
    let error = contract_inventory(catalog_with("main.yml", &empty_root))
        .expect_err("blank document root must fail");
    assert!(
        error.to_string().contains("root must not be empty"),
        "{error}"
    );

    let bytes = [
        EMPTY_V2_DOCUMENT,
        b"---\nnull\n---\n".as_slice(),
        EMPTY_V2_DOCUMENT,
    ]
    .concat();
    let error = contract_inventory(catalog_with("main.yml", &bytes))
        .expect_err("null document in a stream must not be skipped");
    assert!(error.to_string().contains("document 1"), "{error}");
    assert!(error.to_string().contains("empty or null"), "{error}");
}

#[test]
fn restored_document_matches_owner_with_nested_implementation_items() {
    let source = catalog_with(
        "lib.rs",
        br#"
pub struct Thing {
    value: i32,
}

impl Thing {
    pub fn new(value: i32) -> Self { Self { value } }
    pub fn value(&self) -> i32 { self.value }
}
"#,
    );
    let contracts = catalog_with(
        "main.yml",
        br#"
contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - thing_struct:
      file: lib.rs
      signature_type: struct
      name: Thing
      visibility: public
      fields:
        value: i32
      implementations:
        - items:
            - signature_type: method
              name: new
              visibility: public
              parameters:
                - { pattern: value, type: i32 }
              return_type: Self
            - signature_type: method
              name: value
              visibility: public
              receiver: ref
              return_type: i32
sketches: []
"#,
    );

    let source = RustYamlTestFixture::new(source);
    let comparison = source.compare_contract_files(contracts);

    assert_eq!(comparison.source_signature_count(), 1);
    assert_eq!(comparison.contract_signature_count(), 1);
    assert!(
        comparison.diagnostics().is_empty(),
        "{:#?}",
        comparison.diagnostics()
    );
}

#[test]
fn later_version_language_dialect_is_rejected() {
    let error = contract_inventory(catalog_with(
        "main.yml",
        b"version: 1\nlanguage: rust\nsignatures: []\n",
    ))
    .expect_err("later dialect must fail");

    assert!(error.to_string().contains("unknown field"));
    assert!(error.to_string().contains("version"));
}

#[test]
fn contract_catalog_paths_require_shorthand_strings() {
    let documents = [
        "contract_version: 2\nroot: ../src\nfiles: [{ value: lib.rs }]\nsignatures: []\nsketches: []\n",
        "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }\nsignatures:\n  - answer:\n      file: { value: lib.rs }\n      signature_type: function\n      name: answer\nsketches: []\n",
    ];

    for document in documents {
        let error = contract_inventory(catalog_with("main.yml", document.as_bytes()))
            .expect_err("object-form catalog paths must not enter the user contract dialect");

        assert!(
            error
                .to_string()
                .contains("Rust YAML catalog paths must use shorthand text"),
            "{error}"
        );
    }
}

#[test]
fn nested_contract_yaml_is_ignored_and_extensions_are_case_insensitive() {
    let contracts = catalog([
        (
            "nested/ignored.yml",
            b"version: 1\nlanguage: rust\nsignatures: []\n".as_slice(),
        ),
        (
            "MAIN.YAML",
            b"contract_version: 2\nroot: ../src\nfiles: []\nsignatures: []\nsketches: []\n"
                .as_slice(),
        ),
    ]);

    let inventory = contract_inventory(contracts).expect("root contract");

    assert_eq!(inventory.len(), 0);
}

#[test]
fn parsed_documents_supply_the_allowlist_and_inventory_from_one_owner() {
    let contracts = catalog([
        (
            "z.YAML",
            br#"contract_version: 2
root: ../src
files: [z.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: z, root: z.rs, kind: library }] }
signatures:
  - z_function:
      file: z.rs
      signature_type: function
      name: z
sketches: []
"#
            .as_slice(),
        ),
        (
            "a.yml",
            br#"contract_version: 2
root: ../src
files: [a.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: a, root: a.rs, kind: library }] }
signatures:
  - a_function:
      file: a.rs
      signature_type: function
      name: a
sketches: []
"#
            .as_slice(),
        ),
        (
            "nested/ignored.yaml",
            b"this nested document is intentionally not YAML".as_slice(),
        ),
    ]);

    let documents = RustContractDocuments::parse(contracts, &crate::limits::YamlLimits::default())
        .expect("parsed documents");
    assert_eq!(
        documents
            .source_allowlist(&crate::work::CancellationProbe::new())
            .expect("source allowlist"),
        std::collections::BTreeSet::from([
            CatalogPath::new("a.rs").expect("a path"),
            CatalogPath::new("z.rs").expect("z path"),
        ])
    );

    let inventory = documents
        .into_inventory(&crate::work::CancellationProbe::new())
        .expect("merged inventory");
    assert_eq!(inventory.len(), 2);
}

#[test]
fn yaml_cannot_bypass_central_field_variant_and_discriminant_invariants() {
    let cases = [
        (
            "struct field",
            "signature_type: struct\n      name: Record\n      fields:\n        not-valid: u8",
            "invalid Rust identifier for struct field name",
        ),
        (
            "enum variant",
            "signature_type: enum\n      name: Choice\n      variants: [not-valid]",
            "invalid Rust identifier for enum variant name",
        ),
        (
            "enum variant field",
            "signature_type: enum\n      name: Choice\n      variants:\n        - Ready:\n            fields:\n              not-valid: u8",
            "invalid Rust identifier for enum variant field name",
        ),
        (
            "enum discriminant",
            "signature_type: enum\n      name: Choice\n      variants:\n        - Ready:\n            discriminant: '1 +'",
            "invalid Rust syntax text for expression",
        ),
    ];

    for (case, body, expected) in cases {
        let yaml = format!(
            "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: sample, root: lib.rs, kind: library }}] }}\nsignatures:\n  - invalid_declaration:\n      file: lib.rs\n      {body}\nsketches: []\n"
        );
        let error = contract_inventory(catalog_with("main.yml", yaml.as_bytes()))
            .expect_err("invalid declaration metadata must fail at the YAML boundary");

        assert!(error.to_string().contains(expected), "{case}: {error}");
        assert!(error.to_string().contains("main.yml"), "{case}: {error}");
    }
}

#[test]
fn yaml_common_and_method_names_require_canonical_rust_identifiers() {
    let cases = [
        ("function", "signature_type: function\n      name: r#match"),
        ("struct", "signature_type: struct\n      name: r#match"),
        ("enum", "signature_type: enum\n      name: r#match"),
        (
            "trait",
            "signature_type: trait\n      name: r#match\n      items: []",
        ),
        (
            "union",
            "signature_type: union\n      name: r#match\n      fields: { value: u8 }",
        ),
        (
            "static",
            "signature_type: static\n      name: r#match\n      mutable: false\n      type: u8",
        ),
        (
            "type alias",
            "signature_type: type_alias\n      name: r#match\n      target_type: u8",
        ),
        (
            "method",
            "signature_type: trait\n      name: Service\n      items:\n        - signature_type: method\n          name: r#match",
        ),
    ];

    for (case, body) in cases {
        let yaml = format!(
            "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: sample, root: lib.rs, kind: library }}] }}\nsignatures:\n  - invalid_name:\n      file: lib.rs\n      {body}\nsketches: []\n"
        );
        let error = contract_inventory(catalog_with("main.yml", yaml.as_bytes()))
            .expect_err("raw-prefixed YAML declaration names must fail closed");

        assert!(
            error.to_string().contains("invalid Rust identifier"),
            "{case}: {error}"
        );
    }

    let invalid_label_fallback = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - not-valid:
      file: lib.rs
      signature_type: function
sketches: []
"#;
    let error = contract_inventory(catalog_with("main.yml", invalid_label_fallback))
        .expect_err("an invalid label cannot become an implicit Rust item name");
    assert!(
        error.to_string().contains("invalid Rust identifier"),
        "{error}"
    );
}

#[test]
fn rust_syntax_abi_compatibility_spelling_is_rejected() {
    for abi in ["'extern \"C\"'", "' C '", "'C ABI'", "'\"C\"'"] {
        let yaml = format!(
            r#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: sample, root: lib.rs, kind: library }}] }}
signatures:
  - c_abi_function:
      file: lib.rs
      signature_type: function
      name: c_abi
      visibility: public
      abi: {abi}
sketches: []
"#
        );
        let error = contract_inventory(catalog_with("main.yml", yaml.as_bytes()))
            .expect_err("noncanonical ABI spelling must fail");

        assert!(
            error
                .to_string()
                .contains("abi must use the canonical ABI name"),
            "{abi}: {error}"
        );
    }
}

#[test]
fn canonical_signature_type_rejects_aliases_and_kind_inapplicable_fields() {
    let invalid_signatures = [
        (
            "kind alias",
            "kind: function\n      name: answer\n      visibility: public",
            "unknown field `kind`",
        ),
        (
            "function fields",
            "signature_type: function\n      name: answer\n      visibility: public\n      fields:\n        value: u8",
            "field fields is not allowed for signature_type function",
        ),
        (
            "implementation alias",
            "signature_type: impl\n      owner_type: Thing",
            "unknown variant `impl`",
        ),
        (
            "top-level implementation",
            "signature_type: implementation\n      owner_type: Thing",
            "unknown variant `implementation`",
        ),
        (
            "module callable fields",
            "signature_type: module\n      name: nested\n      parameters: []",
            "field parameters is not allowed for signature_type module",
        ),
        (
            "top-level modules",
            "signature_type: modules\n      name: nested",
            "unknown variant `modules`",
        ),
        (
            "top-level test module",
            "signature_type: test_module\n      name: nested",
            "unknown variant `test_module`",
        ),
        (
            "method visibility shortcut",
            "signature_type: struct\n      name: Thing\n      method_visibility: public",
            "unknown field `method_visibility`",
        ),
        (
            "legacy main kind",
            "signature_type: main_method",
            "unknown variant `main_method`",
        ),
        (
            "struct return type",
            "signature_type: struct\n      name: Thing\n      return_type: u8",
            "field return_type is not allowed for signature_type struct",
        ),
        (
            "enum fields",
            "signature_type: enum\n      name: Choice\n      fields:\n        value: u8",
            "field fields is not allowed for signature_type enum",
        ),
        (
            "trait variants",
            "signature_type: trait\n      name: Named\n      variants: [One]",
            "field variants is not allowed for signature_type trait",
        ),
        (
            "static tokens",
            "signature_type: static\n      name: VALUE\n      type: u8\n      tokens: ignored",
            "field tokens is not allowed for signature_type static",
        ),
        (
            "macro mutable",
            "signature_type: macro\n      name: build\n      mutable: true",
            "field mutable is not allowed for signature_type macro",
        ),
        (
            "type alias parameters",
            "signature_type: type_alias\n      name: Value\n      target_type: u8\n      parameters:\n        - { pattern: value, type: u8 }",
            "field parameters is not allowed for signature_type type_alias",
        ),
        (
            "function implementation descriptor",
            "signature_type: function\n      name: answer\n      implementations:\n        - trait: Named",
            "field implementations is not allowed for signature_type function",
        ),
        (
            "trait implementation descriptor",
            "signature_type: trait\n      name: Named\n      implementations:\n        - trait: Other",
            "field implementations is not allowed for signature_type trait",
        ),
        (
            "trait method implementation field",
            "signature_type: trait\n      name: Named\n      items:\n        - signature_type: method\n          name: call\n          trait: Other",
            "unknown field `trait`",
        ),
        (
            "empty abi",
            "signature_type: function\n      name: answer\n      abi: ''",
            "abi must be `extern` or a nonempty ABI name",
        ),
        (
            "empty scalar abi",
            "signature_type: function\n      name: answer\n      abi:",
            "cannot deserialize null into string",
        ),
        (
            "null abi",
            "signature_type: function\n      name: answer\n      abi: null",
            "cannot deserialize null into string",
        ),
        (
            "static missing type",
            "signature_type: static\n      name: VALUE",
            "signature_type static requires field type",
        ),
        (
            "type alias missing target",
            "signature_type: type_alias\n      name: Value",
            "signature_type type_alias requires field target_type",
        ),
    ];

    for (case, body, expected) in invalid_signatures {
        let yaml = format!(
            "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: sample, root: lib.rs, kind: library }}] }}\nsignatures:\n  - answer:\n      file: lib.rs\n      {body}\nsketches: []\n"
        );
        let error = contract_inventory(catalog_with("main.yml", yaml.as_bytes())).expect_err(case);

        assert!(error.to_string().contains(expected), "{case}: {error}");
    }
}

#[test]
fn required_signature_fields_distinguish_missing_from_explicit_null() {
    let cases = [
        ("function", "file", "", "missing field `file`"),
        (
            "function",
            "signature_type",
            "",
            "missing field `signature_type`",
        ),
        (
            "constant",
            "type",
            "      value: '1'\n",
            "signature_type constant requires field type",
        ),
        (
            "constant",
            "value",
            "      type: usize\n",
            "signature_type constant requires field value",
        ),
        (
            "static",
            "type",
            "",
            "signature_type static requires field type",
        ),
        (
            "macro",
            "tokens",
            "",
            "signature_type macro requires field tokens",
        ),
        (
            "type_alias",
            "target_type",
            "",
            "signature_type type_alias requires field target_type",
        ),
        (
            "reexport",
            "path",
            "",
            "signature_type reexport requires field path",
        ),
    ];

    for (signature_type, field, remaining_fields, missing_error) in cases {
        for explicit_null in [false, true] {
            let file = if field == "file" {
                ""
            } else {
                "      file: lib.rs\n"
            };
            let signature_type_field = if field == "signature_type" {
                String::new()
            } else {
                format!("      signature_type: {signature_type}\n")
            };
            let required_field = if explicit_null {
                format!("      {field}: null\n")
            } else {
                String::new()
            };
            let yaml = format!(
                "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: sample, root: lib.rs, kind: library }}] }}\nsignatures:\n  - item:\n{file}{signature_type_field}{remaining_fields}{required_field}sketches: []\n"
            );
            let error = contract_inventory(catalog_with("main.yml", yaml.as_bytes()))
                .expect_err("missing and explicit-null required fields must fail closed");
            let rendered = error.to_string();
            let case = format!("{signature_type}/{field}/explicit_null={explicit_null}");

            if explicit_null {
                assert!(
                    rendered.contains("main.yml document 0"),
                    "{case}: {rendered}"
                );
                assert!(!rendered.contains(missing_error), "{case}: {rendered}");
            } else {
                assert!(rendered.contains(missing_error), "{case}: {rendered}");
            }
        }
    }
}

#[test]
fn shorthand_entry_shape_errors_follow_yaml_encounter_order() {
    let error = contract_inventory(catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - z_first:
      file: lib.rs
      signature_type: function
      fields: []
    a_second:
      file: lib.rs
      signature_type: constant
      value: '1'
sketches: []
"#,
    ))
    .expect_err("the first malformed signature value must fail first");
    let rendered = error.to_string();

    assert!(
        rendered.contains("field fields is not allowed for signature_type function"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("signature_type constant requires field type"),
        "{rendered}"
    );
}

#[test]
fn nested_methods_reject_legacy_implementation_fields_at_the_schema_boundary() {
    let fields = [
        "trait: ''",
        "trait: null",
        "impl_qualifiers: []",
        "impl_qualifiers: null",
        "impl_generics: []",
        "impl_generics: null",
        "impl_where: []",
        "impl_where: null",
    ];

    for field in fields {
        let yaml = format!(
            "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: sample, root: lib.rs, kind: library }}] }}\nsignatures:\n  - named_trait:\n      file: lib.rs\n      signature_type: trait\n      name: Named\n      items:\n        - signature_type: method\n          name: call\n          {field}\nsketches: []\n"
        );
        let error = contract_inventory(catalog_with("main.yml", yaml.as_bytes()))
            .expect_err("implementation field presence must fail on trait methods");
        assert!(
            error.to_string().contains("unknown field"),
            "{field}: {error}"
        );
    }
}

#[test]
fn implementation_methods_reject_explicit_trait_default_body_state() {
    for default_body in [false, true] {
        let yaml = format!(
            "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: sample, root: lib.rs, kind: library }}] }}\nsignatures:\n  - owner_struct:\n      file: lib.rs\n      signature_type: struct\n      name: Owner\n      implementations:\n        - items:\n            - signature_type: method\n              name: call\n              default_body: {default_body}\nsketches: []\n"
        );
        let error = contract_inventory(catalog_with("main.yml", yaml.as_bytes()))
            .expect_err("implementation methods must not accept trait default-body state");

        assert!(
            error
                .to_string()
                .contains("implementation methods cannot specify default_body"),
            "{default_body}: {error}"
        );
    }
}

#[test]
fn trait_methods_retain_explicit_default_body_semantics() {
    for default_body in [false, true] {
        let yaml = format!(
            "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: sample, root: lib.rs, kind: library }}] }}\nsignatures:\n  - service_trait:\n      file: lib.rs\n      signature_type: trait\n      name: Service\n      items:\n        - signature_type: method\n          name: call\n          default_body: {default_body}\nsketches: []\n"
        );
        let documents = RustContractDocuments::parse(
            catalog_with("main.yml", yaml.as_bytes()),
            &crate::limits::YamlLimits::default(),
        )
        .expect("trait default-body state must parse");
        let RustDeclaration::Trait(trait_type) =
            documents.documents[0].document.signatures[0].entries[0].declaration()
        else {
            panic!("expected trait declaration");
        };
        let RustAssociatedItem::Method(method) = &trait_type.items()[0] else {
            panic!("expected trait method");
        };

        assert_eq!(method.has_default_body(), default_body);
    }
}

#[test]
fn top_level_macro_tokens_and_private_visibility_are_mandatory() {
    let cases = [
        ("", "signature_type macro requires field tokens"),
        ("      tokens: ''\n", "macro tokens cannot be empty"),
        (
            "      tokens: 'contract_item ! ('\n",
            "invalid macro tokens",
        ),
        ("      tokens: plain_ident\n", "invalid macro tokens"),
        (
            "      visibility: public\n      tokens: contract_item ! ()\n",
            "top-level macro visibility",
        ),
        (
            "      visibility: crate\n      tokens: contract_item ! ()\n",
            "top-level macro visibility",
        ),
    ];

    for (fields, expected) in cases {
        let yaml = format!(
            "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: sample, root: lib.rs, kind: library }}] }}\nsignatures:\n  - contract_macro:\n      file: lib.rs\n      signature_type: macro\n      name: contract_item\n{fields}sketches: []\n"
        );
        let error = contract_inventory(catalog_with("main.yml", yaml.as_bytes()))
            .expect_err("invalid top-level macro contract must fail");

        assert!(error.to_string().contains(expected), "{fields:?}: {error}");
    }
}

#[test]
fn foreign_macro_tokens_must_form_a_macro_invocation() {
    let yaml = b"contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }\nsignatures:\n  - native_foreign_module:\n      file: lib.rs\n      signature_type: foreign_module\n      items:\n        - signature_type: foreign_macro\n          tokens: plain_ident\nsketches: []\n";
    let error = contract_inventory(catalog_with("main.yml", yaml))
        .expect_err("balanced non-macro tokens must fail for foreign items");

    assert!(
        error.to_string().contains("invalid macro tokens"),
        "{error}"
    );
}

#[test]
fn format_equivalent_macro_tokens_have_one_semantic_inventory() {
    let document = |tokens: &str| {
        format!(
            "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: sample, root: lib.rs, kind: library }}] }}\nsignatures:\n  - contract_macro:\n      file: lib.rs\n      signature_type: macro\n      name: contract_item\n      tokens: '{tokens}'\nsketches: []\n"
        )
    };
    let compact = document("contract_item!()");
    let spaced = document("contract_item ! ( )");
    let compact_inventory = contract_inventory(catalog_with("main.yml", compact.as_bytes()))
        .expect("compact macro contract");
    let spaced_inventory = contract_inventory(catalog_with("main.yml", spaced.as_bytes()))
        .expect("spaced macro contract");

    assert_eq!(
        compact_inventory
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("compact macro source-shape digest"),
        spaced_inventory
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("spaced macro source-shape digest")
    );
}

#[test]
fn methodless_implementations_reject_noncanonical_missing_values() {
    let fields = [
        ("trait: ''", "Rust YAML trait must not be empty"),
        ("trait: null", "Rust YAML trait must not be null"),
        (
            "impl_qualifiers: null",
            "Rust YAML impl_qualifiers must be a list",
        ),
        ("generics: null", "Rust YAML generics must not be null"),
        ("where: null", "Rust YAML where must not be null"),
    ];

    for (field, expected) in fields {
        let yaml = format!(
            "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: sample, root: lib.rs, kind: library }}] }}\nsignatures:\n  - owner_struct:\n      file: lib.rs\n      signature_type: struct\n      name: Owner\n      implementations:\n        - {field}\nsketches: []\n"
        );
        let error = contract_inventory(catalog_with("main.yml", yaml.as_bytes()))
            .expect_err("methodless implementation fields must use canonical values");

        assert!(error.to_string().contains(expected), "{field}: {error}");
    }
}

#[test]
fn implementation_trait_paths_use_central_syntax_validation_without_trimming() {
    for implemented_trait in [" Service", "Service ", "! Service", "crate::"] {
        let yaml = format!(
            "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: sample, root: lib.rs, kind: library }}] }}\nsignatures:\n  - owner_struct:\n      file: lib.rs\n      signature_type: struct\n      name: Owner\n      implementations:\n        - trait: '{implemented_trait}'\nsketches: []\n"
        );
        let error = contract_inventory(catalog_with("main.yml", yaml.as_bytes()))
            .expect_err("invalid implementation trait path must fail at the model boundary");

        assert!(
            error.to_string().contains("implementation trait path"),
            "{implemented_trait:?}: {error}"
        );
    }
}

#[test]
fn trait_supertraits_use_central_syntax_validation_without_trimming() {
    for supertrait in [" Send", "Send ", "Send +"] {
        let yaml = format!(
            "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: sample, root: lib.rs, kind: library }}] }}\nsignatures:\n  - service_trait:\n      file: lib.rs\n      signature_type: trait\n      name: Service\n      supertraits: ['{supertrait}']\nsketches: []\n"
        );
        let error = contract_inventory(catalog_with("main.yml", yaml.as_bytes()))
            .expect_err("invalid trait supertrait must fail at the model boundary");

        assert!(
            error.to_string().contains("trait supertrait"),
            "{supertrait:?}: {error}"
        );
    }
}

#[test]
fn every_signature_kind_rejects_each_inapplicable_field_even_when_null() {
    struct InvalidFieldCase {
        signature_type: &'static str,
        required: &'static str,
        fields: &'static [(&'static str, &'static str)],
    }

    let cases = [
        InvalidFieldCase {
            signature_type: "constant",
            required: "      type: u8\n      value: '1'\n",
            fields: &[("parameters", "[]")],
        },
        InvalidFieldCase {
            signature_type: "function",
            required: "",
            fields: &[
                ("fields", "{ value: u8 }"),
                ("variants", "[One]"),
                ("items", "[]"),
                ("implementations", "[]"),
                ("supertraits", "[Clone]"),
                ("type", "u8"),
                ("target_type", "u8"),
                ("mutable", "true"),
                ("tokens", "value"),
            ],
        },
        InvalidFieldCase {
            signature_type: "struct",
            required: "",
            fields: &[
                ("qualifiers", "[async]"),
                ("abi", "C"),
                ("variadic", "true"),
                ("parameters", "[]"),
                ("return_type", "u8"),
                ("variants", "[One]"),
                ("supertraits", "[Clone]"),
                ("type", "u8"),
                ("target_type", "u8"),
                ("mutable", "true"),
                ("tokens", "value"),
            ],
        },
        InvalidFieldCase {
            signature_type: "enum",
            required: "",
            fields: &[
                ("qualifiers", "[async]"),
                ("abi", "C"),
                ("variadic", "true"),
                ("parameters", "[]"),
                ("return_type", "u8"),
                ("fields", "{ value: u8 }"),
                ("supertraits", "[Clone]"),
                ("type", "u8"),
                ("target_type", "u8"),
                ("mutable", "true"),
                ("tokens", "value"),
            ],
        },
        InvalidFieldCase {
            signature_type: "extern_crate",
            required: "",
            fields: &[("fields", "{ value: u8 }")],
        },
        InvalidFieldCase {
            signature_type: "foreign_module",
            required: "",
            fields: &[("generics", "[T]")],
        },
        InvalidFieldCase {
            signature_type: "trait",
            required: "",
            fields: &[
                ("qualifiers", "[async]"),
                ("abi", "C"),
                ("variadic", "true"),
                ("parameters", "[]"),
                ("return_type", "u8"),
                ("fields", "{ value: u8 }"),
                ("variants", "[One]"),
                ("implementations", "[]"),
                ("type", "u8"),
                ("target_type", "u8"),
                ("mutable", "true"),
                ("tokens", "value"),
            ],
        },
        InvalidFieldCase {
            signature_type: "trait_alias",
            required: "",
            fields: &[("items", "[]")],
        },
        InvalidFieldCase {
            signature_type: "union",
            required: "",
            fields: &[
                ("qualifiers", "[async]"),
                ("abi", "C"),
                ("variadic", "true"),
                ("parameters", "[]"),
                ("return_type", "u8"),
                ("variants", "[One]"),
                ("supertraits", "[Clone]"),
                ("type", "u8"),
                ("target_type", "u8"),
                ("mutable", "true"),
                ("tokens", "value"),
            ],
        },
        InvalidFieldCase {
            signature_type: "static",
            required: "      type: u8\n",
            fields: &[
                ("qualifiers", "[async]"),
                ("abi", "C"),
                ("variadic", "true"),
                ("generics", "[T]"),
                ("where", "['T: Clone']"),
                ("fields", "{ value: u8 }"),
                ("variants", "[One]"),
                ("items", "[]"),
                ("implementations", "[]"),
                ("parameters", "[]"),
                ("return_type", "u8"),
                ("supertraits", "[Clone]"),
                ("target_type", "u8"),
                ("tokens", "value"),
            ],
        },
        InvalidFieldCase {
            signature_type: "macro",
            required: "",
            fields: &[
                ("qualifiers", "[async]"),
                ("abi", "C"),
                ("variadic", "true"),
                ("generics", "[T]"),
                ("where", "['T: Clone']"),
                ("fields", "{ value: u8 }"),
                ("variants", "[One]"),
                ("items", "[]"),
                ("implementations", "[]"),
                ("parameters", "[]"),
                ("return_type", "u8"),
                ("supertraits", "[Clone]"),
                ("type", "u8"),
                ("target_type", "u8"),
                ("mutable", "true"),
            ],
        },
        InvalidFieldCase {
            signature_type: "module",
            required: "",
            fields: &[("return_type", "u8")],
        },
        InvalidFieldCase {
            signature_type: "type_alias",
            required: "      target_type: u8\n",
            fields: &[
                ("qualifiers", "[async]"),
                ("abi", "C"),
                ("variadic", "true"),
                ("parameters", "[]"),
                ("return_type", "u8"),
                ("fields", "{ value: u8 }"),
                ("variants", "[One]"),
                ("supertraits", "[Clone]"),
                ("type", "u8"),
                ("mutable", "true"),
                ("tokens", "value"),
            ],
        },
        InvalidFieldCase {
            signature_type: "reexport",
            required: "      path: crate::Thing\n",
            fields: &[("tokens", "value")],
        },
    ];

    for case in cases {
        for (field, value) in case.fields {
            let signature_type = case.signature_type;
            let required = case.required;
            let yaml = format!(
                "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: sample, root: lib.rs, kind: library }}] }}\nsignatures:\n  - item:\n      file: lib.rs\n      signature_type: {signature_type}\n{required}      {field}: {value}\nsketches: []\n"
            );
            let error = contract_inventory(catalog_with("main.yml", yaml.as_bytes()))
                .expect_err("inapplicable field must fail");
            assert!(
                error.to_string().contains(&format!(
                    "field {field} is not allowed for signature_type {signature_type}"
                )),
                "{signature_type}.{field}: {error}"
            );

            let null_yaml = format!(
                "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: sample, root: lib.rs, kind: library }}] }}\nsignatures:\n  - item:\n      file: lib.rs\n      signature_type: {signature_type}\n{required}      {field}: null\nsketches: []\n"
            );
            contract_inventory(catalog_with("main.yml", null_yaml.as_bytes()))
                .expect_err("an explicit null must not hide an inapplicable signature field");
        }
    }
}

#[test]
fn signature_file_must_be_in_document_allowlist() {
    let error = contract_inventory(catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [other.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: other.rs, kind: library }] }
signatures:
  - answer_function:
      file: lib.rs
      signature_type: function
      name: answer
      visibility: public
sketches: []
"#,
    ))
    .expect_err("unlisted reference must fail");

    assert!(error.to_string().contains("unlisted source file lib.rs"));
}

#[test]
fn duplicate_structural_signature_labels_are_rejected() {
    let contracts = catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - first_answer:
      file: lib.rs
      signature_type: function
      name: answer
      visibility: public
  - second_answer:
      file: lib.rs
      signature_type: function
      name: answer
      visibility: public
sketches: []
"#,
    );

    let error =
        contract_inventory(contracts).expect_err("one structural signature cannot use two labels");

    assert!(
        error
            .to_string()
            .contains("duplicate Rust function identity"),
        "{error}"
    );
}
