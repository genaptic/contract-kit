use super::super::RustContractDocuments;
use super::{catalog, catalog_with, contract_inventory};
use crate::files::CatalogPath;
use crate::languages::rust::parser::RustSourceFiles;

#[test]
fn restored_document_matches_owner_with_nested_methods() {
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
root: ../src
files: [lib.rs]
signatures:
  - thing_struct:
      file: lib.rs
      signature_type: struct
      name: Thing
      visibility: public
      fields:
        value: i32
      methods:
        - signature_type: method
          name: new
          visibility: public
          parameters:
            - value: i32
          return_type: Self
        - signature_type: method
          name: value
          visibility: public
          receiver: ref
          return_type: i32
sketches: []
"#,
    );

    let source_inventory = RustSourceFiles::from_catalog(source)
        .parse_all()
        .expect("source");
    let contract_inventory = contract_inventory(contracts).expect("contract");
    let comparison = source_inventory
        .compare_against(&contract_inventory)
        .expect("comparison");

    assert_eq!(comparison.source_signature_count(), 1);
    assert_eq!(comparison.contract_signature_count(), 1);
    assert!(comparison.diagnostics().is_empty());
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
        "root: ../src\nfiles: [{ value: lib.rs }]\nsignatures: []\nsketches: []\n",
        "root: ../src\nfiles: [lib.rs]\nsignatures:\n  - answer:\n      file: { value: lib.rs }\n      signature_type: function\n      name: answer\nsketches: []\n",
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
            b"root: ../src\nfiles: []\nsignatures: []\nsketches: []\n".as_slice(),
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
            br#"root: ../src
files: [z.rs]
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
            br#"root: ../src
files: [a.rs]
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

    let documents = RustContractDocuments::parse(contracts).expect("parsed documents");
    assert_eq!(
        documents.source_allowlist(),
        std::collections::BTreeSet::from([
            CatalogPath::new("a.rs").expect("a path"),
            CatalogPath::new("z.rs").expect("z path"),
        ])
    );

    let inventory = documents.into_inventory().expect("merged inventory");
    assert_eq!(inventory.len(), 2);
}

#[test]
fn rust_syntax_abi_compatibility_spelling_is_rejected() {
    for abi in ["'extern \"C\"'", "' C '", "'C ABI'", "'\"C\"'"] {
        let yaml = format!(
            r#"root: ../src
files: [lib.rs]
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
            "top-level module",
            "signature_type: module\n      name: nested",
            "unknown variant `module`",
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
            "main visibility",
            "signature_type: main_method\n      visibility: public",
            "field visibility is not allowed for signature_type main_method",
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
            "signature_type: type_alias\n      name: Value\n      target_type: u8\n      parameters:\n        - value: u8",
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
            "signature_type: trait\n      name: Named\n      methods:\n        - signature_type: method\n          name: call\n          trait: Other",
            "trait declaration methods cannot carry implementation fields",
        ),
        (
            "empty abi",
            "signature_type: function\n      name: answer\n      abi: ''",
            "abi must be `extern` or a nonempty ABI name",
        ),
        (
            "empty scalar abi",
            "signature_type: function\n      name: answer\n      abi:",
            "invalid type",
        ),
        (
            "null abi",
            "signature_type: function\n      name: answer\n      abi: null",
            "invalid type",
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
            "root: ../src\nfiles: [lib.rs]\nsignatures:\n  - answer:\n      file: lib.rs\n      {body}\nsketches: []\n"
        );
        let error = contract_inventory(catalog_with("main.yml", yaml.as_bytes())).expect_err(case);

        assert!(error.to_string().contains(expected), "{case}: {error}");
    }
}

#[test]
fn trait_declaration_methods_reject_implementation_field_presence() {
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
            "root: ../src\nfiles: [lib.rs]\nsignatures:\n  - named_trait:\n      file: lib.rs\n      signature_type: trait\n      name: Named\n      methods:\n        - signature_type: method\n          name: call\n          {field}\nsketches: []\n"
        );
        let error = contract_inventory(catalog_with("main.yml", yaml.as_bytes()))
            .expect_err("implementation field presence must fail on trait methods");
        assert!(
            error
                .to_string()
                .contains("trait declaration methods cannot carry implementation fields"),
            "{field}: {error}"
        );
    }
}

#[test]
fn owner_methods_reject_null_or_empty_implementation_fields() {
    let fields = [
        ("trait: ''", "Rust YAML trait must not be empty"),
        ("trait: null", "Rust YAML trait must not be null"),
        (
            "impl_qualifiers: null",
            "Rust YAML impl_qualifiers must be a list",
        ),
        (
            "impl_generics: null",
            "Rust YAML impl_generics must not be null",
        ),
        ("impl_where: null", "Rust YAML impl_where must not be null"),
    ];

    for (field, expected) in fields {
        let yaml = format!(
            "root: ../src\nfiles: [lib.rs]\nsignatures:\n  - owner_struct:\n      file: lib.rs\n      signature_type: struct\n      name: Owner\n      methods:\n        - signature_type: method\n          name: call\n          {field}\nsketches: []\n"
        );
        let error = contract_inventory(catalog_with("main.yml", yaml.as_bytes()))
            .expect_err("noncanonical implementation field value must fail");
        assert!(error.to_string().contains(expected), "{field}: {error}");
    }
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
            "root: ../src\nfiles: [lib.rs]\nsignatures:\n  - owner_struct:\n      file: lib.rs\n      signature_type: struct\n      name: Owner\n      implementations:\n        - {field}\nsketches: []\n"
        );
        let error = contract_inventory(catalog_with("main.yml", yaml.as_bytes()))
            .expect_err("methodless implementation fields must use canonical values");

        assert!(error.to_string().contains(expected), "{field}: {error}");
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
            signature_type: "main_method",
            required: "",
            fields: &[
                ("visibility", "public"),
                ("fields", "{ value: u8 }"),
                ("variants", "[One]"),
                ("methods", "[]"),
                ("implementations", "[]"),
                ("supertraits", "[Clone]"),
                ("type", "u8"),
                ("target_type", "u8"),
                ("mutable", "true"),
                ("tokens", "value"),
            ],
        },
        InvalidFieldCase {
            signature_type: "function",
            required: "",
            fields: &[
                ("fields", "{ value: u8 }"),
                ("variants", "[One]"),
                ("methods", "[]"),
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
                ("methods", "[]"),
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
                ("methods", "[]"),
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
    ];

    for case in cases {
        for (field, value) in case.fields {
            let signature_type = case.signature_type;
            let required = case.required;
            let yaml = format!(
                "root: ../src\nfiles: [lib.rs]\nsignatures:\n  - item:\n      file: lib.rs\n      signature_type: {signature_type}\n{required}      {field}: {value}\nsketches: []\n"
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
                "root: ../src\nfiles: [lib.rs]\nsignatures:\n  - item:\n      file: lib.rs\n      signature_type: {signature_type}\n{required}      {field}: null\nsketches: []\n"
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
        br#"root: ../src
files: [other.rs]
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
        br#"root: ../src
files: [lib.rs]
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
        error.to_string().contains(
            "signature id rust:lib.rs:function:answer is assigned to multiple groups: first_answer and second_answer"
        ),
        "{error}"
    );
}
