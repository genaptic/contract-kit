use super::super::{RustContractDocuments, RustYamlRenderer};
use super::{RustYamlTestFixture, catalog, catalog_with, contract_inventory, rendered};
use crate::api::{ContractScope, GenerateDocument, GenerateTarget};
use crate::files::CatalogPath;
use crate::inventory::InventoryDiffEntry;
use crate::languages::rust::parser::{RustParsedFiles, RustSourceFiles};
use crate::languages::rust::source::RustSourceFile;

#[test]
fn generation_emits_combined_original_shape_and_folds_implementations() {
    let source = catalog_with(
        "lib.rs",
        br#"
mod declarations;

pub struct Thing {
    value: i32,
}

impl Thing {
    pub fn value(&self) -> i32 { self.value }
}

pub fn answer() -> i32 { 42 }
"#,
    );
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert!(yaml.starts_with("root: ../src\nfiles:\n- lib.rs\n"));
    assert!(yaml.contains("signature_type: struct"));
    assert!(yaml.contains("signature_type: method"));
    assert!(yaml.contains("receiver: ref"));
    assert!(yaml.contains("signature_type: function"));
    assert!(yaml.contains("sketches: []"));
    assert!(!yaml.contains("version:"));
    assert!(!yaml.contains("language:"));
    assert!(!yaml.contains("signature_type: module"));
    assert!(!yaml.contains("signature_type: implementation"));
    assert_eq!(generated.signature_count, 2);
    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn module_declarations_only_supply_traversal_context() {
    let source = catalog_with(
        "lib.rs",
        br#"mod external;

mod nested {
pub fn answer() {}
}

pub fn root() {}
"#,
    );
    let fixture = RustYamlTestFixture::new(source);
    let parsed = fixture.parsed_for_yaml();
    let parsed_ids = parsed
        .files()
        .iter()
        .flat_map(|file| file.entries())
        .map(|entry| entry.id().render())
        .collect::<Vec<_>>();

    assert_eq!(
        parsed_ids,
        [
            "rust:lib.rs:function:root",
            "rust:lib.rs::nested:function:answer",
        ]
    );
    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert_eq!(fixture.source_inventory().len(), 2);
    assert_eq!(generated.signature_count, 2);
    assert!(yaml.contains("module_path:\n    - nested"), "{yaml}");
    assert!(!yaml.contains("signature_type: module"), "{yaml}");

    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn generated_contract_round_trips_to_source_inventory() {
    let source = catalog_with(
        "lib.rs",
        br#"
pub enum Choice {
    One,
    Two(i32),
}

impl Choice {
pub fn value(&self) -> i32 { 0 }
}

pub static LIMIT: usize = 4;

pub async fn fetch<T: Clone>(value: T) -> T
where
    T: Send,
{
    value
}
"#,
    );
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn repeated_macro_invocations_receive_stable_occurrence_ids() {
    let fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        br#"macro_rules! include { () => {}; }
include!("shared.rs");
include!("shared.rs");
"#,
    ));
    let expected = [
        "rust:lib.rs:macro:include",
        "rust:lib.rs:macro:include#2",
        "rust:lib.rs:macro:include#3",
    ];
    let parsed = fixture.parsed_for_yaml();
    let ids = parsed
        .files()
        .iter()
        .flat_map(|file| file.entries())
        .map(|entry| entry.id().render())
        .collect::<Vec<_>>();

    assert_eq!(ids, expected);

    let edited = RustSourceFiles::from_catalog(catalog_with(
        "lib.rs",
        br#"include!("third.rs");
macro_rules! include { () => {}; }
include!("first.rs");
"#,
    ))
    .parse_all_for_yaml()
    .expect("edited repeated macros");
    let edited_ids = edited
        .files()
        .iter()
        .flat_map(|file| file.entries())
        .map(|entry| entry.id().render())
        .collect::<Vec<_>>();

    assert_eq!(edited_ids, expected);

    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn repeated_macro_round_trip_preserves_numeric_occurrence_order() {
    let source = (1..=11)
        .map(|ordinal| format!("include!(\"{ordinal}.rs\");\n"))
        .collect::<String>();
    let fixture = RustYamlTestFixture::new(catalog_with("lib.rs", source.as_bytes()));
    let expected = (1..=11)
        .map(|ordinal| {
            if ordinal == 1 {
                "rust:lib.rs:macro:include".to_owned()
            } else {
                format!("rust:lib.rs:macro:include#{ordinal}")
            }
        })
        .collect::<Vec<_>>();
    let source_ids = fixture
        .parsed_for_yaml()
        .files()
        .iter()
        .flat_map(|file| file.entries())
        .map(|entry| entry.id().render())
        .collect::<Vec<_>>();

    assert_eq!(source_ids, expected);

    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    let reparsed = RustContractDocuments::parse(generated.contract_files.clone())
        .expect("generated repeated macro contract");
    let reparsed_ids = reparsed
        .documents
        .iter()
        .flat_map(|document| document.document.signatures.iter())
        .filter_map(|signature| signature.entries.first())
        .map(|entry| entry.id().render())
        .collect::<Vec<_>>();

    assert_eq!(reparsed_ids, expected);
    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn repeated_macro_regeneration_retains_labels_by_occurrence() {
    let previous = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"include!(\"alpha.rs\");\ninclude!(\"beta.rs\");\n",
    ));
    let current = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"include!(\"beta.rs\");\ninclude!(\"alpha-v2.rs\");\n",
    ));
    let existing = catalog_with(
        "main.yml",
        br#"root: ../src
files: [lib.rs]
signatures:
  - first_include:
      file: lib.rs
      signature_type: macro
      name: include
      visibility: private
      tokens: include ! ("alpha.rs")
  - second_include:
      file: lib.rs
      signature_type: macro
      name: include
      visibility: private
      tokens: include ! ("beta.rs")
sketches: []
"#,
    );

    let diff = current
        .source_inventory()
        .diff_against(previous.source_inventory())
        .expect("repeated macro diff");
    assert_eq!(diff.entries().len(), 2, "{:#?}", diff.entries());
    assert!(
        diff.entries()
            .iter()
            .all(|entry| matches!(entry, InventoryDiffEntry::Changed { .. })),
        "{:#?}",
        diff.entries()
    );

    let generated = current.render_existing(existing);
    let yaml = rendered(&generated.contract_files, "main.yml");
    let first = yaml
        .split("- first_include:")
        .nth(1)
        .and_then(|value| value.split("- second_include:").next())
        .expect("first retained macro label");
    let second = yaml
        .split("- second_include:")
        .nth(1)
        .expect("second retained macro label");

    assert!(first.contains("tokens: include ! (\"beta.rs\")"), "{yaml}");
    assert!(
        second.contains("tokens: include ! (\"alpha-v2.rs\")"),
        "{yaml}"
    );
    current.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn generated_named_fields_preserve_declaration_order() {
    let source = catalog_with(
        "lib.rs",
        br#"
pub struct Ordered {
    pub zeta: u8,
    alpha: u16,
    pub middle: u32,
}

pub union OrderedUnion {
    pub zeta: u8,
    pub alpha: u16,
    pub middle: u32,
}
"#,
    );
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");
    let struct_fields = yaml
        .split("- ordered_struct:")
        .nth(1)
        .and_then(|value| value.split("- orderedunion_union:").next())
        .expect("ordered struct output");
    let zeta = struct_fields.find("zeta:").expect("zeta field");
    let alpha = struct_fields.find("alpha:").expect("alpha field");
    let middle = struct_fields.find("middle:").expect("middle field");
    let union_fields = yaml
        .split("- orderedunion_union:")
        .nth(1)
        .expect("ordered union output");
    let union_zeta = union_fields.find("zeta:").expect("union zeta field");
    let union_alpha = union_fields.find("alpha:").expect("union alpha field");
    let union_middle = union_fields.find("middle:").expect("union middle field");

    assert!(zeta < alpha && alpha < middle, "{yaml}");
    assert!(
        union_zeta < union_alpha && union_alpha < union_middle,
        "{yaml}"
    );

    fixture.assert_generated_matches_source(generated.contract_files);

    let swapped_yaml = yaml.replacen(
        "      zeta:\n        type: u8\n        visibility: public\n      alpha: u16\n",
        "      alpha: u16\n      zeta:\n        type: u8\n        visibility: public\n",
        1,
    );
    assert_ne!(swapped_yaml, yaml, "struct field order probe must apply");
    let swapped_inventory = contract_inventory(catalog_with("main.yml", swapped_yaml.as_bytes()))
        .expect("swapped field contract inventory");
    let comparison = fixture
        .source_inventory()
        .compare_against(&swapped_inventory)
        .expect("swapped comparison");
    assert!(
        format!("{:?}", comparison.diagnostics()).contains("Mismatched"),
        "{:?}",
        comparison.diagnostics()
    );
}

#[test]
fn explicit_rust_abi_round_trips() {
    let source = catalog_with(
        "lib.rs",
        br#"
pub fn implicit_rust() {}
pub extern "Rust" fn explicit_rust() {}
pub extern fn unnamed_extern() {}
pub extern "C" fn c_abi() {}
pub extern "C-unwind" fn c_unwind_abi() {}
pub extern "externC" fn prefixed_abi() {}
pub extern "my_abi" fn underscored_abi() {}

pub struct AbiOwner;
impl AbiOwner {
    pub extern "externC" fn prefixed_method(&self) {}
}
"#,
    );
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert!(yaml.contains("name: explicit_rust\n    visibility: public\n    abi: Rust"));
    assert!(yaml.contains("name: unnamed_extern\n    visibility: public\n    abi: extern"));
    assert!(yaml.contains("name: c_abi\n    visibility: public\n    abi: C"));
    assert!(yaml.contains("name: c_unwind_abi\n    visibility: public\n    abi: C-unwind"));
    assert!(yaml.contains("name: prefixed_abi\n    visibility: public\n    abi: externC"));
    assert!(yaml.contains("name: underscored_abi\n    visibility: public\n    abi: my_abi"));
    assert!(
        yaml.contains(
            "name: prefixed_method\n      receiver: ref\n      visibility: public\n      abi: externC"
        ),
        "{yaml}"
    );
    let implicit = yaml
        .split("- implicit_rust_function:")
        .nth(1)
        .and_then(|value| value.split("- ").next())
        .expect("implicit Rust function");
    assert!(!implicit.contains("abi:"), "{yaml}");

    fixture.assert_generated_matches_source(generated.contract_files);

    let implicit_contract = yaml.replacen("    abi: Rust\n", "", 1);
    assert_ne!(
        implicit_contract, yaml,
        "explicit Rust ABI probe must apply"
    );
    let implicit_inventory =
        contract_inventory(catalog_with("main.yml", implicit_contract.as_bytes()))
            .expect("implicit ABI contract inventory");
    let comparison = fixture
        .source_inventory()
        .compare_against(&implicit_inventory)
        .expect("implicit ABI comparison");
    assert!(
        format!("{:?}", comparison.diagnostics()).contains("Mismatched"),
        "{:?}",
        comparison.diagnostics()
    );
}

#[test]
fn public_source_main_normalizes_to_the_visibility_free_main_contract() {
    let source = catalog_with("main.rs", b"pub fn main() {}\n");
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_new("main.yml", &["main.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");
    let main = yaml
        .split("signature_type: main_method")
        .nth(1)
        .expect("main method output");
    assert!(!main.contains("visibility:"), "{yaml}");

    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn cross_file_implementation_folds_into_its_local_owner() {
    let source = catalog([
        (
            "models.rs",
            b"pub struct Thing { pub value: u8 }\n".as_slice(),
        ),
        (
            "impls.rs",
            br#"impl crate::models::Thing {
    pub fn value(&self) -> u8 { self.value }
}
"#,
        ),
    ]);
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_new("main.yml", &["impls.rs", "models.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert_eq!(generated.signature_count, 1);
    assert!(yaml.contains("signature_type: struct"), "{yaml}");
    assert!(yaml.contains("signature_type: method"), "{yaml}");
    assert!(!yaml.contains("signature_type: implementation"), "{yaml}");

    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn implementation_source_order_and_inherent_visibility_are_stable() {
    let declaration_first = catalog_with(
        "lib.rs",
        br#"pub struct Thing;

impl Thing {
fn hidden(&self) {}
pub fn visible(&self) {}
}
"#,
    );
    let implementation_first = catalog_with(
        "lib.rs",
        br#"impl Thing {
fn hidden(&self) {}
pub fn visible(&self) {}
}

pub struct Thing;
"#,
    );
    let declaration_fixture = RustYamlTestFixture::new(declaration_first);
    let implementation_fixture = RustYamlTestFixture::new(implementation_first);
    let declaration_generated = declaration_fixture.render_new("main.yml", &["lib.rs"]);
    let implementation_generated = implementation_fixture.render_new("main.yml", &["lib.rs"]);

    assert_eq!(
        declaration_fixture.source_inventory().inventory_digest(),
        implementation_fixture.source_inventory().inventory_digest()
    );
    assert_eq!(
        declaration_generated.contract_files,
        implementation_generated.contract_files
    );
    let yaml = rendered(&declaration_generated.contract_files, "main.yml");
    assert!(
        yaml.contains("name: hidden\n      receiver: ref\n      visibility: private"),
        "{yaml}"
    );
    assert!(
        yaml.contains("name: visible\n      receiver: ref\n      visibility: public"),
        "{yaml}"
    );
    declaration_fixture.assert_generated_matches_source(declaration_generated.contract_files);
}

#[test]
fn yaml_grouping_rejects_unnormalized_owner_names() {
    let path = CatalogPath::new("lib.rs").expect("source path");
    let parsed = RustParsedFiles {
        files: vec![
            RustSourceFile::new(
                path,
                br#"
pub struct Thing;

impl crate::Thing {
    pub fn value(&self) {}
}
"#
                .to_vec(),
            )
            .parse_inventory()
            .expect("raw parsed source"),
        ],
    };

    let error = RustYamlRenderer::new(
        parsed,
        GenerateTarget::New(GenerateDocument {
            contract_file: CatalogPath::new("main.yml").expect("contract path"),
            root: "../src".to_owned(),
            files: vec![CatalogPath::new("lib.rs").expect("source path")],
        }),
        ContractScope::Signatures,
    )
    .render()
    .expect_err("generation must not repair an unnormalized owner name");

    assert!(
        error
            .to_string()
            .contains("cannot fold implementation inherent:crate :: Thing"),
        "{error}"
    );
}

#[test]
fn same_named_non_owner_does_not_replace_the_implementation_owner_group() {
    let source = catalog_with(
        "lib.rs",
        br#"
pub struct Thing;

macro_rules! Thing {
    () => {};
}

impl Thing {
    pub fn value(&self) -> u8 { 1 }
}
"#,
    );
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");
    assert!(yaml.contains("signature_type: struct"), "{yaml}");
    assert!(yaml.contains("signature_type: macro"), "{yaml}");
    assert!(yaml.contains("signature_type: method"), "{yaml}");

    let without_method = yaml.replacen(
        "    methods:\n    - signature_type: method\n      name: value\n      receiver: ref\n      visibility: public\n      return_type: u8\n",
        "",
        1,
    );
    assert_ne!(without_method, yaml, "method-removal probe must apply");
    let incomplete_inventory =
        contract_inventory(catalog_with("main.yml", without_method.as_bytes()))
            .expect("incomplete contract inventory");
    let incomplete = fixture
        .source_inventory()
        .compare_against(&incomplete_inventory)
        .expect("incomplete comparison");
    assert!(
        format!("{:?}", incomplete.diagnostics()).contains("thing_struct"),
        "{:?}",
        incomplete.diagnostics()
    );

    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn local_implementation_methods_are_independent_of_block_partitioning_and_relocation() {
    let first = catalog([
        ("models.rs", b"pub struct Thing;\n".as_slice()),
        (
            "a.rs",
            br#"impl crate::models::Thing {
    pub fn alpha(&self) {}
    pub fn gamma(&self) {}
}
"#,
        ),
        (
            "b.rs",
            b"impl crate::models::Thing { pub fn beta(&self) {} }\n".as_slice(),
        ),
    ]);
    let relocated = catalog([
        ("models.rs", b"pub struct Thing;\n".as_slice()),
        (
            "a.rs",
            b"impl crate::models::Thing { pub fn alpha(&self) {} }\n".as_slice(),
        ),
        (
            "b.rs",
            br#"impl crate::models::Thing {
    pub fn beta(&self) {}
    pub fn gamma(&self) {}
}
"#,
        ),
    ]);

    let first_fixture = RustYamlTestFixture::new(first);
    let relocated_fixture = RustYamlTestFixture::new(relocated);
    let files = ["a.rs", "b.rs", "models.rs"];
    let first_generated = first_fixture.render_new("main.yml", &files);
    let relocated_generated = relocated_fixture.render_new("main.yml", &files);

    assert_eq!(
        first_fixture.source_inventory().inventory_digest(),
        relocated_fixture.source_inventory().inventory_digest()
    );
    assert_eq!(
        first_generated.contract_files,
        relocated_generated.contract_files
    );
    let yaml = rendered(&first_generated.contract_files, "main.yml");
    let alpha = yaml.find("name: alpha").expect("alpha method");
    let beta = yaml.find("name: beta").expect("beta method");
    let gamma = yaml.find("name: gamma").expect("gamma method");
    assert!(alpha < beta && beta < gamma, "{yaml}");

    first_fixture.assert_generated_matches_source(first_generated.contract_files);
    relocated_fixture.assert_generated_matches_source(relocated_generated.contract_files);
}

#[test]
fn distinct_local_implementation_descriptors_have_stable_lossless_ids() {
    let source = catalog([
        ("models.rs", b"pub struct Generic<T>(pub T);\n".as_slice()),
        (
            "clone_impl.rs",
            br#"impl<T: Clone> crate::models::Generic<T> {
    pub fn cloned(&self) {}
}
"#,
        ),
        (
            "copy_impl.rs",
            br#"impl<T: Copy> crate::models::Generic<T> {
    pub fn copied(&self) {}
}
"#,
        ),
    ]);
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_new("main.yml", &["clone_impl.rs", "copy_impl.rs", "models.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");
    assert!(
        yaml.contains("impl_generics:\n      - 'T: Clone'"),
        "{yaml}"
    );
    assert!(yaml.contains("impl_generics:\n      - 'T: Copy'"), "{yaml}");

    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn qualified_cross_file_trait_implementation_uses_private_owner_visibility() {
    let source = catalog([
        ("models.rs", b"struct Hidden;\n".as_slice()),
        (
            "traits.rs",
            b"pub trait Named { fn name(&self) -> &str; }\n".as_slice(),
        ),
        (
            "impls.rs",
            br#"impl crate::traits::Named for crate::models::Hidden {
    fn name(&self) -> &str { "hidden" }
}
"#,
        ),
    ]);
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_new("main.yml", &["impls.rs", "models.rs", "traits.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");
    assert!(
        yaml.contains(
            "name: name\n      receiver: ref\n      visibility: private\n      trait: 'crate :: traits :: Named'"
        ),
        "{yaml}"
    );

    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn unqualified_owner_resolution_is_stable_across_relocation_and_spelling() {
    let unqualified = catalog([
        ("models.rs", b"pub struct Thing;\n".as_slice()),
        (
            "impls.rs",
            b"impl Thing { pub fn value(&self) -> u8 { 1 } }\n".as_slice(),
        ),
        ("relocated.rs", b"".as_slice()),
    ]);
    let qualified = catalog([
        ("models.rs", b"pub struct Thing;\n".as_slice()),
        ("impls.rs", b"".as_slice()),
        (
            "relocated.rs",
            b"impl crate::models::Thing { pub fn value(&self) -> u8 { 2 } }\n".as_slice(),
        ),
    ]);
    let unqualified_fixture = RustYamlTestFixture::new(unqualified);
    let qualified_fixture = RustYamlTestFixture::new(qualified);
    let files = ["impls.rs", "models.rs", "relocated.rs"];
    let unqualified_generated = unqualified_fixture.render_new("main.yml", &files);
    let qualified_generated = qualified_fixture.render_new("main.yml", &files);

    assert_eq!(
        unqualified_fixture.source_inventory().inventory_digest(),
        qualified_fixture.source_inventory().inventory_digest()
    );
    assert_eq!(
        unqualified_generated.contract_files,
        qualified_generated.contract_files
    );
    let yaml = rendered(&unqualified_generated.contract_files, "main.yml");
    assert!(yaml.contains("name: value"), "{yaml}");
    assert!(!yaml.contains("signature_type: implementation"), "{yaml}");

    unqualified_fixture.assert_generated_matches_source(unqualified_generated.contract_files);
}

#[test]
fn ambiguous_unqualified_owner_is_rejected_deterministically() {
    let source = catalog([
        ("right.rs", b"pub struct Thing;\n".as_slice()),
        (
            "impls.rs",
            b"pub struct Thing;\nimpl Thing { pub fn value(&self) {} }\n".as_slice(),
        ),
    ]);
    let check_error = RustSourceFiles::from_catalog(source.clone())
        .parse_all()
        .expect_err("ambiguous check owner must fail")
        .to_string();
    let generation_error = match RustSourceFiles::from_catalog(source).parse_all_for_yaml() {
        Ok(_) => panic!("ambiguous generation owner must fail"),
        Err(error) => error.to_string(),
    };

    assert_eq!(check_error, generation_error);
    assert!(
        check_error.contains("implementation owner Thing is ambiguous across source declarations"),
        "{check_error}"
    );
}

#[test]
fn qualified_implementation_owners_resolve_crate_self_and_super_paths() {
    let source = catalog([
        (
            "local.rs",
            br#"pub struct Local;
impl self::Local { pub fn local(&self) {} }
"#,
        ),
        ("models.rs", b"pub struct CrateOwner;\n".as_slice()),
        (
            "impls.rs",
            b"impl crate::models::CrateOwner { pub fn crate_path(&self) {} }\n".as_slice(),
        ),
        ("nested/mod.rs", b"pub struct Parent;\n".as_slice()),
        (
            "nested/child.rs",
            b"impl super::Parent { pub fn parent(&self) {} }\n".as_slice(),
        ),
    ]);
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_new(
        "main.yml",
        &[
            "impls.rs",
            "local.rs",
            "models.rs",
            "nested/child.rs",
            "nested/mod.rs",
        ],
    );

    assert_eq!(generated.signature_count, 3);
    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn implementation_owner_cannot_traverse_above_the_source_root() {
    let source = catalog_with(
        "lib.rs",
        br#"
pub struct Thing;

impl super::Thing {
pub fn value(&self) {}
}
"#,
    );

    let error = RustSourceFiles::from_catalog(source)
        .parse_all()
        .expect_err("root implementation owner must not traverse above the source root");

    assert!(
        error
            .to_string()
            .contains("implementation owner super::Thing traverses above the source root"),
        "{error}"
    );
}

#[test]
fn qualified_owner_selects_between_same_named_types() {
    let source = catalog([
        ("left.rs", b"pub struct Thing;\n".as_slice()),
        ("right.rs", b"pub struct Thing;\n".as_slice()),
        (
            "impls.rs",
            b"impl crate::right::Thing { pub fn selected(&self) {} }\n".as_slice(),
        ),
    ]);
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_new("main.yml", &["impls.rs", "left.rs", "right.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");
    let right = yaml
        .split("file: right.rs")
        .nth(1)
        .expect("right Thing contract");
    assert!(right.contains("name: selected"), "{yaml}");

    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn methodless_implementation_metadata_round_trips_exactly() {
    let source = catalog_with(
        "lib.rs",
        br#"
pub trait Marker {}
pub unsafe trait UnsafeMarker {}

pub struct Generic<T>(T);
impl<T> Generic<T> where T: Clone {}

pub struct Positive;
impl Marker for Positive {}

pub struct Negative;
impl !Marker for Negative {}

pub struct Unsafe;
unsafe impl UnsafeMarker for Unsafe {}

pub struct Defaulted;
default impl Marker for Defaulted {}
"#,
    );
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");

    for expected in [
        "trait: Marker",
        "trait: '!Marker'",
        "trait: UnsafeMarker",
        "impl_qualifiers:\n      - unsafe",
        "impl_qualifiers:\n      - default",
        "generics:\n      - T",
        "where:\n      - 'T : Clone'",
    ] {
        assert!(yaml.contains(expected), "missing {expected:?}:\n{yaml}");
    }
    assert!(!yaml.contains("signature_type: implementation"), "{yaml}");

    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn type_alias_trait_implementation_round_trips_under_the_alias() {
    let source = catalog_with(
        "lib.rs",
        br#"
pub struct Actual;
pub type Alias = Actual;
pub trait Named { fn name(&self) -> &str; }
impl Named for Alias { fn name(&self) -> &str { "alias" } }
"#,
    );
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert!(yaml.contains("signature_type: type_alias"), "{yaml}");
    assert!(yaml.contains("trait: Named"), "{yaml}");
    assert!(!yaml.contains("signature_type: implementation"), "{yaml}");

    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn union_implementation_round_trips_under_the_union() {
    let source = catalog_with(
        "lib.rs",
        br#"
pub union Number {
    pub bits: u32,
    pub value: f32,
}

impl Number {
    pub fn from_bits(bits: u32) -> Self {
        Self { bits }
    }
}
"#,
    );
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert!(yaml.contains("signature_type: union"), "{yaml}");
    assert!(yaml.contains("name: from_bits"), "{yaml}");
    assert!(!yaml.contains("signature_type: implementation"), "{yaml}");

    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn external_implementation_owner_is_rejected() {
    let source = catalog_with(
        "lib.rs",
        br#"
pub trait Local {
    fn value(&self) -> usize;
}

impl Local for Vec<u8> {
    fn value(&self) -> usize { self.len() }
}
"#,
    );

    let check_error = RustSourceFiles::from_catalog(source.clone())
        .parse_all()
        .expect_err("external owner must be rejected");
    let generation_error = match RustSourceFiles::from_catalog(source).parse_all_for_yaml() {
        Ok(_) => panic!("external generation owner must be rejected"),
        Err(error) => error,
    };

    assert!(
        check_error
            .to_string()
            .contains("cannot resolve implementation owner Vec < u8 >"),
        "{check_error}"
    );
    assert_eq!(check_error.to_string(), generation_error.to_string());
}

#[test]
fn leading_colon_owner_paths_are_not_resolved_as_local() {
    let source = catalog_with(
        "lib.rs",
        br#"
pub mod foo {
    pub struct Thing;
}

impl ::foo::Thing {
    pub fn value(&self) {}
}
"#,
    );

    let error = RustSourceFiles::from_catalog(source)
        .parse_all()
        .expect_err("extern-prelude owner paths must not resolve to local declarations");

    assert!(
        error
            .to_string()
            .contains("cannot resolve implementation owner :: foo :: Thing"),
        "{error}"
    );
}

#[test]
fn specialized_inherent_owner_applications_are_rejected() {
    let source = catalog_with(
        "lib.rs",
        br#"
pub struct Wrapper<T>(T);

impl Wrapper<u8> {
    pub fn byte(&self) {}
}

impl Wrapper<u16> {
    pub fn word(&self) {}
}
"#,
    );
    let check_error = RustSourceFiles::from_catalog(source.clone())
        .parse_all()
        .expect_err("specialized inherent owners must fail");
    let generation_error = match RustSourceFiles::from_catalog(source).parse_all_for_yaml() {
        Ok(_) => panic!("specialized inherent generation owners must fail"),
        Err(error) => error,
    };

    assert!(
        check_error
            .to_string()
            .contains("unsupported local implementation owner application Wrapper < u8 >"),
        "{check_error}"
    );
    assert_eq!(check_error.to_string(), generation_error.to_string());
}

#[test]
fn specialized_trait_owner_applications_are_rejected() {
    let source = catalog_with(
        "lib.rs",
        br#"
pub trait Marker {}
pub struct Wrapper<T>(T);

impl Marker for Wrapper<u16> {}
"#,
    );
    let error = RustSourceFiles::from_catalog(source)
        .parse_all()
        .expect_err("specialized trait owner must fail");

    assert!(
        error
            .to_string()
            .contains("unsupported local implementation owner application Wrapper < u16 >"),
        "{error}"
    );
}

#[test]
fn non_identity_and_qualified_owner_arguments_are_rejected() {
    let cases = [
        (
            "reordered",
            br#"
pub struct Pair<T, U>(T, U);
impl<T, U> Pair<U, T> {}
"#
            .as_slice(),
        ),
        (
            "nested",
            br#"
pub struct Wrapper<T>(T);
impl<T> Wrapper<Vec<T>> {}
"#
            .as_slice(),
        ),
        (
            "qualified segment arguments",
            br#"
pub mod models {
    pub struct Wrapper<T>(T);
}
impl<T> crate::models::<T>::Wrapper<T> {}
"#
            .as_slice(),
        ),
    ];

    for (case, source) in cases {
        let error = RustSourceFiles::from_catalog(catalog_with("lib.rs", source))
            .parse_all()
            .expect_err(case);
        assert!(
            error
                .to_string()
                .contains("unsupported local implementation owner application"),
            "{case}: {error}"
        );
    }
}

#[test]
fn identity_owner_applications_preserve_type_lifetime_and_const_parameters() {
    let source = catalog_with(
        "lib.rs",
        br#"
pub struct Wrapper<'a, T, const N: usize>(&'a [T; N]);

impl<'a, T, const N: usize> Wrapper<'a, T, N> {
    pub fn value(&self) -> &'a T { &self.0[0] }
}
"#,
    );
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn generated_labels_are_unique_after_lossy_sanitization() {
    let source = catalog_with(
        "lib.rs",
        b"pub fn r#type() {}\npub fn r_type() {}\npub fn r__type() {}\n",
    );
    let fixture = RustYamlTestFixture::new(source);
    let first = fixture.render_new("main.yml", &["lib.rs"]);
    let second = fixture.render_new("main.yml", &["lib.rs"]);

    assert_eq!(first.contract_files, second.contract_files);
    let yaml = rendered(&first.contract_files, "main.yml");
    assert!(yaml.contains("- r_type_function:"), "{yaml}");
    assert!(yaml.contains("- lib_rs_r_type_function:"), "{yaml}");
    assert!(yaml.contains("- lib_rs_r_type_function_2:"), "{yaml}");

    fixture.assert_generated_matches_source(first.contract_files);
}

#[test]
fn existing_labels_reserve_lossy_candidates_for_new_and_removed_items() {
    let existing = catalog_with(
        "main.yml",
        br#"root: ../src
files: [lib.rs]
signatures:
  - r_type_function:
      file: lib.rs
      signature_type: function
      name: existing
      visibility: public
  - lib_rs_r_type_function:
      file: lib.rs
      signature_type: function
      name: other
      visibility: public
sketches: []
"#,
    );
    let source = catalog_with(
        "lib.rs",
        b"pub fn existing() {}\npub fn other() {}\npub fn r#type() {}\n",
    );
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_existing(existing);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert!(yaml.contains("- r_type_function:"), "{yaml}");
    assert!(yaml.contains("name: existing"), "{yaml}");
    assert!(yaml.contains("- lib_rs_r_type_function:"), "{yaml}");
    assert!(yaml.contains("name: other"), "{yaml}");
    assert!(yaml.contains("- lib_rs_r_type_function_2:"), "{yaml}");
    assert!(yaml.contains("name: r#type"), "{yaml}");
    fixture.assert_generated_matches_source(generated.contract_files);

    let removed_existing = catalog_with(
        "main.yml",
        br#"root: ../src
files: [lib.rs]
signatures:
  - r_type_function:
      file: lib.rs
      signature_type: function
      name: removed
      visibility: public
  - lib_rs_r_type_function:
      file: lib.rs
      signature_type: function
      name: also_removed
      visibility: public
sketches: []
"#,
    );
    let fixture = RustYamlTestFixture::new(catalog_with("lib.rs", b"pub fn r#type() {}\n"));
    let generated = fixture.render_existing(removed_existing);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert!(!yaml.contains("- r_type_function:"), "{yaml}");
    assert!(!yaml.contains("- lib_rs_r_type_function:"), "{yaml}");
    assert!(yaml.contains("- lib_rs_r_type_function_2:"), "{yaml}");
    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn generation_preserves_stable_label_and_linked_sketch() {
    let source = catalog_with("lib.rs", b"pub fn answer() -> i32 { 42 }\n");
    let existing = catalog_with(
        "main.yml",
        br#"root: ../src
files: [lib.rs]
signatures:
  - public_answer:
      file: lib.rs
      signature_type: function
      name: answer
      visibility: public
      sketch: answer_example
sketches:
  - answer_example:
    signature_type: function
    code: |
      pub fn answer() -> i32 { 0 }
"#,
    );
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_existing(existing);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert!(yaml.contains("- public_answer:"));
    assert!(yaml.contains("sketch: answer_example"));
    assert!(yaml.contains("- answer_example:\n"), "{yaml}");
    assert_eq!(generated.signature_count, 1);
    assert_eq!(generated.sketch_count, 1);
    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn trait_implementation_methods_use_public_contract_visibility() {
    let source = catalog_with(
        "lib.rs",
        br#"
pub trait Named {
    fn name(&self) -> &str;
}

pub struct PublicThing;
struct PrivateThing;

impl Named for PublicThing {
    fn name(&self) -> &str { "public" }
}

impl Named for PrivateThing {
    fn name(&self) -> &str { "thing" }
}
"#,
    );
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert!(
        yaml.contains(
            "name: name\n      receiver: ref\n      visibility: public\n      trait: Named"
        ),
        "{yaml}"
    );
    assert!(
        yaml.contains(
            "name: name\n      receiver: ref\n      visibility: private\n      trait: Named"
        ),
        "{yaml}"
    );

    fixture.assert_generated_matches_source(generated.contract_files);
}
