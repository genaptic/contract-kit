use super::{
    RustContractDocuments, RustYamlTestFixture, catalog, catalog_with, contract_inventory, rendered,
};
use crate::api::{ContractScope, GenerateTarget};
use crate::files::CatalogPath;
use crate::inventory::InventoryDiffEntry;
use crate::limits::{LimitResource, SignatureLimits};

#[test]
fn unchanged_existing_v2_generation_returns_original_bytes_exactly() {
    let source = catalog_with("lib.rs", b"pub fn answer() -> i32 { 42 }\n");
    let fixture = RustYamlTestFixture::new(source);
    let bytes = b"# owner comment\r\ncontract_version: 2\r\nroot: '../src'\r\nfiles: [lib.rs]\r\nextraction:\r\n  mode: rust_syntax_v2\r\n  profile: rust_api_v1\r\n  crates: [{ id: sample, root: lib.rs, kind: library }]\r\nsignatures:\r\n  - answer_function:\r\n      file: lib.rs\r\n      signature_type: function\r\n      name: answer\r\n      visibility: public\r\n      return_type: i32\r\nsketches: [] # preserved\r\n";

    let plan = super::RustGenerationPlan::parse(
        GenerateTarget::Existing(catalog_with("main.yml", bytes)),
        &crate::limits::YamlLimits::default(),
    )
    .expect("unchanged generation plan");
    let mut limits = SignatureLimits::default();
    limits.output.scratch_bytes = 0;
    let generated = fixture
        .render_plan_with_limits(plan, ContractScope::Signatures, limits)
        .expect("unchanged generation needs no output scratch");

    assert_eq!(
        generated
            .contract_files
            .get(&CatalogPath::new("main.yml").expect("path")),
        Some(bytes.as_slice())
    );
    assert_eq!(generated.counts.document_count, 1);
    assert_eq!(generated.counts.signature_count, 1);
    assert_eq!(generated.counts.preserved_sketch_count, 0);
    assert_eq!(generated.counts.semantically_changed_document_count, 0);
    assert_eq!(generated.counts.byte_changed_document_count, 0);
}

#[test]
fn alignment_one_packing_no_op_preserves_authored_yaml_bytes() {
    let fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"#[repr(packed)]\npub struct Packet;\n",
    ));
    let bytes = br#"# preserve explicit alignment-one spelling
contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - packet_struct:
      file: lib.rs
      signature_type: struct
      name: Packet
      visibility: public
      attributes:
        - repr: [packed(1)]
sketches: []
"#;

    let generated = fixture.render_existing(catalog_with("main.yml", bytes));

    assert_eq!(
        generated
            .contract_files
            .get(&CatalogPath::new("main.yml").expect("contract path")),
        Some(bytes.as_slice())
    );
    assert_eq!(generated.counts.semantically_changed_document_count, 0);
    assert_eq!(generated.counts.byte_changed_document_count, 0);
}

#[test]
fn new_generation_renders_alignment_one_packing_canonically() {
    let fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"#[repr(packed(1))]\npub struct Packet;\n",
    ));

    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert!(yaml.contains("- packed"), "{yaml}");
    assert!(!yaml.contains("packed(1)"), "{yaml}");
    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn generated_yaml_obeys_the_exact_output_byte_boundary() {
    let fixture = RustYamlTestFixture::new(catalog_with("lib.rs", b"pub fn answer() {}\n"));
    let path = CatalogPath::new("main.yml").expect("contract path");
    let expected = fixture
        .render_new(path.as_str(), &["lib.rs"])
        .contract_files
        .get(&path)
        .expect("baseline generated YAML")
        .to_vec();
    let source = CatalogPath::new("lib.rs").expect("source path");
    let plan = || {
        super::RustGenerationPlan::parse(
            GenerateTarget::New(crate::api::GenerateDocument {
                contract_file: path.clone(),
                root: "../src".to_owned(),
                files: vec![source.clone()],
                crates: vec![crate::api::RustCrateRoot {
                    id: "sample".to_owned(),
                    root: source.clone(),
                    kind: crate::api::RustCrateKind::Library,
                }],
            }),
            &crate::limits::YamlLimits::default(),
        )
        .expect("fresh generation plan")
    };
    let expected_bytes = u64::try_from(expected.len()).expect("fixture size");

    let mut exact_limits = SignatureLimits::default();
    exact_limits.output.generated_bytes = expected_bytes;
    let exact = fixture
        .render_plan_with_limits(plan(), ContractScope::Signatures, exact_limits)
        .expect("exact output boundary");
    assert_eq!(exact.contract_files.get(&path), Some(expected.as_slice()));

    let mut crossing_limits = SignatureLimits::default();
    crossing_limits.output.generated_bytes = expected_bytes - 1;
    let error = fixture
        .render_plan_with_limits(plan(), ContractScope::Signatures, crossing_limits)
        .expect_err("one byte below the canonical document must fail");
    let limit = error.limit_exceeded().expect("typed output limit");
    assert_eq!(limit.resource, LimitResource::GeneratedOutputBytes);
    assert_eq!(limit.limit, expected_bytes - 1);
    assert_eq!(limit.observed_at_least, expected_bytes);
    assert_eq!(limit.file.as_ref(), Some(&path));
}

#[test]
fn removal_only_generation_needs_no_output_scratch() {
    let fixture = RustYamlTestFixture::new(catalog_with("lib.rs", b"pub fn retained() {}\n"));
    let bytes = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - retained_function:
      file: lib.rs
      signature_type: function
      name: retained
      visibility: public
  - removed_function:
      file: lib.rs
      signature_type: function
      name: removed
      visibility: public
sketches: []
"#;
    let plan = super::RustGenerationPlan::parse(
        GenerateTarget::Existing(catalog_with("main.yml", bytes)),
        &crate::limits::YamlLimits::default(),
    )
    .expect("existing generation plan");
    let mut limits = SignatureLimits::default();
    limits.output.scratch_bytes = 0;

    let generated = fixture
        .render_plan_with_limits(plan, ContractScope::Signatures, limits)
        .expect("deleting one stale entry must not allocate replacement text");
    let edited = rendered(&generated.contract_files, "main.yml");

    assert!(edited.contains("retained_function"), "{edited}");
    assert!(!edited.contains("removed_function"), "{edited}");
    contract_inventory(generated.contract_files).expect("removal-only edit reparses");
}

#[test]
fn changed_signature_reports_the_nominal_scratch_limit_before_returning_output() {
    let fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"pub fn answer(value: i64) -> i64 { value }\n",
    ));
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
sketches: []
"#;
    let plan = super::RustGenerationPlan::parse(
        GenerateTarget::Existing(catalog_with("main.yml", bytes)),
        &crate::limits::YamlLimits::default(),
    )
    .expect("existing generation plan");
    let mut limits = SignatureLimits::default();
    limits.output.scratch_bytes = 0;

    let error = fixture
        .render_plan_with_limits(plan, ContractScope::Signatures, limits)
        .expect_err("changed signature preview must use the scratch budget");
    let limit = error.limit_exceeded().expect("typed scratch limit");

    assert_eq!(limit.resource, LimitResource::OutputScratchBytes);
    assert_eq!(limit.limit, 0);
    assert_eq!(limit.observed_at_least, 1);
    assert_eq!(
        limit.file.as_ref().map(CatalogPath::as_str),
        Some("main.yml")
    );
}

#[test]
fn changed_signature_edit_preserves_crlf_through_the_changed_node() {
    let fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"pub fn answer(value: i64) -> i64 { value }\n",
    ));
    let bytes = b"# retained\r\ncontract_version: 2\r\nroot: ../src\r\nfiles: [lib.rs]\r\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }\r\nsignatures:\r\n  - answer_function:\r\n      file: lib.rs\r\n      signature_type: function\r\n      name: answer\r\n      visibility: public\r\nsketches: []\r\n";

    let generated = fixture.render_existing(catalog_with("main.yml", bytes));
    let edited = generated
        .contract_files
        .get(&CatalogPath::new("main.yml").expect("path"))
        .expect("changed document");

    assert!(edited.windows(2).any(|window| window == b"\r\n"));
    assert!(
        edited
            .iter()
            .enumerate()
            .all(|(index, byte)| *byte != b'\n' || index > 0 && edited[index - 1] == b'\r'),
        "changed CRLF document contains a bare line feed"
    );
    contract_inventory(generated.contract_files).expect("CRLF edit must semantically reparse");
}

#[test]
fn changed_document_converts_generated_text_without_normalizing_retained_mixed_breaks() {
    let fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"pub fn answer(value: i64) -> i64 { value }\n",
    ));
    let bytes = b"# first physical break selects CRLF\r\ncontract_version: 2\r\nroot: ../src\r\nfiles: [lib.rs]\r\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }\r\n# retained mixed break\nsignatures:\r\n  - answer_function:\r\n      file: lib.rs\r\n      signature_type: function\r\n      name: answer\r\n      visibility: public\r\nsketches: []\r\n";

    let generated = fixture.render_existing(catalog_with("main.yml", bytes));
    let edited = generated
        .contract_files
        .get(&CatalogPath::new("main.yml").expect("path"))
        .expect("changed document");
    let signature_start = edited
        .windows(b"  - answer_function:".len())
        .position(|window| window == b"  - answer_function:")
        .expect("generated signature start");
    let signature_end = edited
        .windows(b"sketches: []".len())
        .position(|window| window == b"sketches: []")
        .expect("retained sketches field");
    let changed_signature = &edited[signature_start..signature_end];

    assert!(
        edited
            .windows(b"# retained mixed break\nsignatures:\r\n".len())
            .any(|window| window == b"# retained mixed break\nsignatures:\r\n"),
        "retained mixed presentation was normalized"
    );
    assert!(
        changed_signature
            .iter()
            .enumerate()
            .all(|(index, byte)| *byte != b'\n'
                || index > 0 && changed_signature[index - 1] == b'\r'),
        "generated signature contains a bare line feed"
    );
    contract_inventory(generated.contract_files)
        .expect("mixed-presentation edit must semantically reparse");
}

#[test]
fn changed_documents_preserve_each_physical_document_line_ending() {
    let fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"pub fn alpha(value: i64) -> i64 { value }\npub fn beta(value: i64) -> i64 { value }\n",
    ));
    let first = "---\ncontract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: alpha, root: lib.rs, kind: library }] }\nsignatures:\n  - alpha_function:\n      file: lib.rs\n      signature_type: function\n      name: alpha\n      visibility: public\nsketches: []\n";
    let second = "---\r\ncontract_version: 2\r\nroot: ../src\r\nfiles: [lib.rs]\r\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: beta, root: lib.rs, kind: library }] }\r\nsignatures:\r\n  - beta_function:\r\n      file: lib.rs\r\n      signature_type: function\r\n      name: beta\r\n      visibility: public\r\nsketches: []\r\n";
    let bytes = format!("{first}{second}");

    let generated = fixture.render_existing(catalog_with("main.yml", bytes.as_bytes()));
    let edited = generated
        .contract_files
        .get(&CatalogPath::new("main.yml").expect("path"))
        .expect("changed YAML stream");
    let second_start = edited
        .windows(b"---\r\n".len())
        .position(|window| window == b"---\r\n")
        .expect("retained CRLF document marker");
    let (first_edited, second_edited) = edited.split_at(second_start);

    assert!(
        first_edited.iter().all(|byte| *byte != b'\r'),
        "LF document gained a carriage return"
    );
    assert!(
        second_edited
            .iter()
            .enumerate()
            .all(|(index, byte)| *byte != b'\n' || index > 0 && second_edited[index - 1] == b'\r'),
        "CRLF document contains a generated bare line feed"
    );
    assert_eq!(generated.counts.semantically_changed_document_count, 2);
    contract_inventory(generated.contract_files)
        .expect("mixed-line-ending stream must semantically reparse");
}

#[test]
fn changed_cr_only_document_preserves_cr_and_is_idempotent() {
    let fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"pub fn answer(value: i64) -> i64 { value }\n",
    ));
    let bytes = "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }\nsignatures:\n  - answer_function:\n      file: lib.rs\n      signature_type: function\n      name: answer\n      visibility: public\nsketches: []\n"
        .replace('\n', "\r");

    let first = fixture.render_existing(catalog_with("main.yml", bytes.as_bytes()));
    let first_bytes = first
        .contract_files
        .get(&CatalogPath::new("main.yml").expect("path"))
        .expect("changed CR-only document")
        .to_vec();

    assert!(first_bytes.contains(&b'\r'));
    assert!(
        !first_bytes.contains(&b'\n'),
        "CR-only document gained a line feed"
    );
    contract_inventory(first.contract_files).expect("CR-only edit must semantically reparse");

    let second = fixture.render_existing(catalog_with("main.yml", &first_bytes));
    assert_eq!(
        second
            .contract_files
            .get(&CatalogPath::new("main.yml").expect("path")),
        Some(first_bytes.as_slice())
    );
}

#[test]
fn cr_only_removal_deletes_the_stale_entry_indentation_without_scratch() {
    let fixture = RustYamlTestFixture::new(catalog_with("lib.rs", b"pub fn retained() {}\n"));
    let bytes = "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }\nsignatures:\n  - retained_function:\n      file: lib.rs\n      signature_type: function\n      name: retained\n      visibility: public\n  - removed_function:\n      file: lib.rs\n      signature_type: function\n      name: removed\n      visibility: public\nsketches: []\n"
        .replace('\n', "\r");
    let plan = super::RustGenerationPlan::parse(
        GenerateTarget::Existing(catalog_with("main.yml", bytes.as_bytes())),
        &crate::limits::YamlLimits::default(),
    )
    .expect("CR-only generation plan");
    let mut limits = SignatureLimits::default();
    limits.output.scratch_bytes = 0;

    let generated = fixture
        .render_plan_with_limits(plan, ContractScope::Signatures, limits)
        .expect("CR-only removal must not require scratch text");
    let edited = generated
        .contract_files
        .get(&CatalogPath::new("main.yml").expect("path"))
        .expect("changed CR-only document");

    assert!(!edited.contains(&b'\n'));
    assert!(
        edited
            .windows(b"\rsketches:".len())
            .any(|window| window == b"\rsketches:")
    );
    assert!(!edited.windows(7).any(|window| window == b"removed"));
    contract_inventory(generated.contract_files).expect("CR-only removal must reparse");
}

#[test]
fn flow_signature_edit_preserves_anchors_tags_comments_and_is_idempotent() {
    let fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"pub fn answer(value: i64) -> i64 { value }\n",
    ));
    let bytes = br#"# owner
contract_version: 2
root: !!str ../src
files: [&source_file lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: *source_file, kind: library }] }
# keep before sequence
signatures: [{ answer_function: { file: lib.rs, signature_type: function, name: answer, visibility: public } }] # keep after sequence
sketches: []
"#;

    let first = fixture.render_existing(catalog_with("main.yml", bytes));
    let first_bytes = first
        .contract_files
        .get(&CatalogPath::new("main.yml").expect("path"))
        .expect("first generation")
        .to_vec();
    let first_text = String::from_utf8(first_bytes.clone()).expect("UTF-8 output");

    assert!(first_text.contains("root: !!str ../src"), "{first_text}");
    assert!(
        first_text.contains("files: [&source_file lib.rs]"),
        "{first_text}"
    );
    assert!(first_text.contains("root: *source_file"), "{first_text}");
    assert!(
        first_text.contains("# keep before sequence"),
        "{first_text}"
    );
    assert!(first_text.contains("# keep after sequence"), "{first_text}");
    contract_inventory(first.contract_files).expect("flow edit must semantically reparse");

    let second = fixture.render_existing(catalog_with("main.yml", &first_bytes));
    assert_eq!(
        second
            .contract_files
            .get(&CatalogPath::new("main.yml").expect("path")),
        Some(first_bytes.as_slice())
    );
}

#[test]
fn changed_anchored_signature_fails_closed_when_a_preserved_sketch_uses_its_alias() {
    let fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"pub fn answer(value: i64) -> i64 { value }\n",
    ));
    let bytes = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - answer_function:
      file: &signature_file lib.rs
      signature_type: function
      name: answer
      visibility: public
      sketch: example
sketches:
  - example:
      file: *signature_file
      signature: answer_function
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: at_least_one }
      code: answer();
"#;
    let plan = super::RustGenerationPlan::parse(
        GenerateTarget::Existing(catalog_with("main.yml", bytes)),
        &crate::limits::YamlLimits::default(),
    )
    .expect("anchored v2 document must be a valid generation target");

    let error = fixture
        .render_plan(plan, ContractScope::Signatures)
        .expect_err("replacing an anchor used by a preserved node must fail closed");
    let rendered = error.to_string();

    assert!(
        rendered.contains("lossless YAML edit is unsupported for main.yml"),
        "{rendered}"
    );
    assert!(
        rendered.contains("edited YAML failed semantic reparse"),
        "{rendered}"
    );
}

#[test]
fn generation_rejects_signature_output_without_v2_extraction_before_editing() {
    let fixture = RustYamlTestFixture::new(catalog_with("lib.rs", b"pub fn answer() {}\n"));
    let bytes = br#"# bytes must remain untouched on failure
contract_version: 2
root: ../src
files: [lib.rs]
signatures: []
sketches: []
"#;
    let plan = super::RustGenerationPlan::parse(
        GenerateTarget::Existing(catalog_with("main.yml", bytes)),
        &crate::limits::YamlLimits::default(),
    )
    .expect("empty non-signature document is parseable");
    let error = fixture
        .render_plan(plan, ContractScope::Signatures)
        .expect_err("signature-bearing output requires extraction metadata");

    assert!(
        error.to_string().contains(
            "cannot generate signatures into main.yml document 0 without extraction metadata"
        ),
        "{error}"
    );
}

#[test]
fn document_render_equality_retains_parameter_pattern_metadata() {
    let fixture =
        RustYamlTestFixture::new(catalog_with("lib.rs", b"pub fn answer(current: i32) {}\n"));
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
      parameters:
        - { pattern: previous, type: i32 }
sketches: []
"#;

    let generated = fixture.render_existing(catalog_with("main.yml", bytes));
    let edited = rendered(&generated.contract_files, "main.yml");

    assert!(edited.contains("pattern: \"current\""), "{edited}");
    assert!(edited.contains("type: i32"), "{edited}");
    assert!(!edited.contains("pattern: \"previous\""), "{edited}");
    contract_inventory(generated.contract_files).expect("pattern edit must semantically reparse");
}

#[test]
fn changed_signature_edit_preserves_sketches_and_untouched_root_bytes() {
    let fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"pub fn answer(value: i64) -> i64 { value }\n",
    ));
    let bytes = br#"# exact leading comment
contract_version: 2
root: '../src' # exact root comment
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - answer_function:
      file: lib.rs
      signature_type: function
      name: answer
      visibility: public
      sketch: example
sketches:
  - example:
      file: lib.rs
      signature: answer_function
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: at_least_one }
      code: |-
        // sketch bytes stay exact
        answer();
"#;

    let generated = fixture.render_existing(catalog_with("main.yml", bytes));
    let edited = rendered(&generated.contract_files, "main.yml");

    assert!(edited.starts_with("# exact leading comment\n"), "{edited}");
    assert!(
        edited.contains("root: '../src' # exact root comment"),
        "{edited}"
    );
    assert!(
        edited.contains(
            "sketches:\n  - example:\n      file: lib.rs\n      signature: answer_function\n      signature_type: function\n      matching: { normalization: exact_lines_v1, occurrence: at_least_one }\n      code: |-\n        // sketch bytes stay exact\n        answer();\n"
        ),
        "{edited}"
    );
    assert_eq!(generated.counts.document_count, 1);
    assert_eq!(generated.counts.signature_count, 1);
    assert_eq!(generated.counts.preserved_sketch_count, 1);
    assert_eq!(generated.counts.semantically_changed_document_count, 1);
    assert_eq!(generated.counts.byte_changed_document_count, 1);
    contract_inventory(generated.contract_files).expect("edited bytes must semantically reparse");
}

#[test]
fn changed_signature_edit_preserves_compact_indentless_sketches_exactly() {
    let fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"pub fn answer(value: i64) -> i64 { value }\n",
    ));
    let sketch_suffix = br#"sketches:
- example:
    file: lib.rs
    signature: answer_function
    signature_type: function
    matching: { normalization: exact_lines_v1, occurrence: at_least_one }
    code: |-
      // indentless sketch bytes stay exact
      answer();
"#;
    let mut bytes = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
- answer_function:
    file: lib.rs
    signature_type: function
    name: answer
    visibility: public
    sketch: example
- obsolete_function:
    file: lib.rs
    signature_type: function
    name: obsolete
    visibility: public
"#
    .to_vec();
    bytes.extend_from_slice(sketch_suffix);

    let generated = fixture.render_existing(catalog_with("main.yml", &bytes));
    let edited = generated
        .contract_files
        .get(&CatalogPath::new("main.yml").expect("path"))
        .expect("changed compact document");

    assert!(
        edited.ends_with(sketch_suffix),
        "{}",
        String::from_utf8_lossy(edited)
    );
    contract_inventory(generated.contract_files)
        .expect("compact indentless edit must semantically reparse");
}

#[test]
fn added_signature_precedes_compact_indentless_sketches() {
    let fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"pub fn answer() {}\npub fn added() {}\n",
    ));
    let sketch_suffix = br#"sketches:
- example:
    file: lib.rs
    signature: answer_function
    signature_type: function
    matching: { normalization: exact_lines_v1, occurrence: at_least_one }
    code: answer();
"#;
    let mut bytes = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
- answer_function:
    file: lib.rs
    signature_type: function
    name: answer
    visibility: public
    sketch: example
"#
    .to_vec();
    bytes.extend_from_slice(sketch_suffix);

    let generated = fixture.render_existing(catalog_with("main.yml", &bytes));
    let edited = generated
        .contract_files
        .get(&CatalogPath::new("main.yml").expect("path"))
        .expect("changed compact document");
    let edited_text = String::from_utf8_lossy(edited);

    assert!(edited_text.contains("added_function:"), "{edited_text}");
    assert!(edited.ends_with(sketch_suffix), "{edited_text}");
    contract_inventory(generated.contract_files)
        .expect("compact indentless insertion must semantically reparse");
}

#[test]
fn all_scope_returns_linked_sketch_seeds_from_the_generation_projection() {
    let fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"pub fn answer(value: i64) -> i64 { value }\n",
    ));
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
      matching: { normalization: exact_lines_v1, occurrence: exactly_one }
      code: stale
"#;
    let plan = super::RustGenerationPlan::parse(
        GenerateTarget::Existing(catalog_with("main.yml", bytes)),
        &crate::limits::YamlLimits::default(),
    )
    .expect("existing generation plan");
    let signature_only_plan = super::RustGenerationPlan::parse(
        GenerateTarget::Existing(catalog_with("main.yml", bytes)),
        &crate::limits::YamlLimits::default(),
    )
    .expect("signature-only generation plan");
    let signature_only = fixture
        .render_plan(signature_only_plan, ContractScope::Signatures)
        .expect("signature-only generation");
    assert!(signature_only.resolved_sketch_seeds.is_empty());
    let generated = fixture
        .render_plan(plan, ContractScope::All)
        .expect("all-scope generation");
    let yaml = rendered(&generated.contract_files, "main.yml");
    let signature_start = yaml
        .find("  - answer_function:\n")
        .expect("rendered answer signature");
    let signature_end = yaml[signature_start..]
        .find("\nsketches:")
        .map_or(yaml.len(), |length| signature_start + length);
    let rendered_keys = yaml[signature_start..signature_end]
        .lines()
        .filter_map(|line| line.strip_prefix("      "))
        .filter(|line| !line.starts_with(' ') && !line.starts_with("- "))
        .filter_map(|line| line.split_once(':').map(|(key, _)| key))
        .collect::<Vec<_>>();

    assert_eq!(generated.resolved_sketch_seeds.len(), 1);
    assert_eq!(
        rendered_keys,
        [
            "crate_id",
            "file",
            "signature_type",
            "name",
            "visibility",
            "parameters",
            "return_type",
            "sketch",
        ],
        "{yaml}"
    );
    let seed = &generated.resolved_sketch_seeds[0];
    assert_eq!(seed.contract_file.as_str(), "main.yml");
    assert_eq!(seed.document_index, 0);
    assert_eq!(seed.sketch_id, "answer_example");
    assert_eq!(seed.signature_type, "function");
    assert_eq!(seed.file.as_str(), "lib.rs");
    assert_eq!(seed.code, "pub fn answer(value: i64) -> i64 { value }");
}

#[test]
fn removing_a_signature_cannot_orphan_its_nested_v2_sketch() {
    let fixture = RustYamlTestFixture::new(catalog_with("lib.rs", b"pub fn retained() {}\n"));
    let bytes = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - removed_function:
      file: lib.rs
      signature_type: function
      name: removed
      visibility: public
      sketch: removed_example
sketches:
  - removed_example:
      file: lib.rs
      signature: removed_function
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: exactly_one }
      code: pub fn removed() {}
"#;
    let plan = super::RustGenerationPlan::parse(
        GenerateTarget::Existing(catalog_with("main.yml", bytes)),
        &crate::limits::YamlLimits::default(),
    )
    .expect("existing generation plan");
    let error = fixture
        .render_plan(plan, ContractScope::Signatures)
        .expect_err("linked sketch cannot be orphaned");

    assert!(
        error
            .to_string()
            .contains("orphan preserved sketch removed_example")
    );
}

#[test]
fn all_scope_removes_a_stale_signature_and_its_linked_sketch_together() {
    let fixture = RustYamlTestFixture::new(catalog_with("lib.rs", b"// removed\n"));
    let bytes = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
- removed_function:
    file: lib.rs
    signature_type: function
    name: removed
    visibility: public
    sketch: removed_example
sketches:
- removed_example:
    file: lib.rs
    signature: removed_function
    signature_type: function
    matching: { normalization: exact_lines_v1, occurrence: exactly_one }
    code: pub fn removed() {}
"#;
    let plan = super::RustGenerationPlan::parse(
        GenerateTarget::Existing(catalog_with("main.yml", bytes)),
        &crate::limits::YamlLimits::default(),
    )
    .expect("existing generation plan");
    let generated = fixture
        .render_plan(plan, ContractScope::All)
        .expect("all scope may remove stale linked records together");
    let edited = rendered(&generated.contract_files, "main.yml");

    assert_eq!(generated.counts.signature_count, 0);
    assert_eq!(generated.counts.preserved_sketch_count, 0);
    assert_eq!(generated.counts.semantically_changed_document_count, 1);
    assert_eq!(generated.counts.byte_changed_document_count, 1);
    assert!(generated.resolved_sketch_seeds.is_empty());
    assert!(edited.contains("signatures: []"), "{edited}");
    assert!(edited.contains("sketches: []"), "{edited}");
    assert!(!edited.contains("removed_example"), "{edited}");
    contract_inventory(generated.contract_files).expect("coordinated cleanup must reparse");
}

#[test]
fn changed_signature_edit_preserves_unchanged_signature_node_bytes() {
    let fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"pub fn first(value: i64) {}\npub fn second() -> i32 { 2 }\n",
    ));
    let unchanged = "  # keeper comment\n  - 'second_function': # keeper inline\n      file: \"lib.rs\"\n      signature_type: function\n      name: second\n      visibility: public\n      return_type: 'i32'\n";
    let bytes = format!(
        "contract_version: 2\nroot: ../src\nfiles: [lib.rs]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: sample, root: lib.rs, kind: library }}] }}\nsignatures:\n  - first_function:\n      file: lib.rs\n      signature_type: function\n      name: first\n      visibility: public\n{unchanged}sketches: []\n"
    );

    let generated = fixture.render_existing(catalog_with("main.yml", bytes.as_bytes()));
    let edited = rendered(&generated.contract_files, "main.yml");

    assert!(edited.contains(unchanged), "{edited}");
    contract_inventory(generated.contract_files).expect("granular edit must semantically reparse");
}

#[test]
fn changed_document_in_yaml_stream_preserves_untouched_document_bytes() {
    let fixture = RustYamlTestFixture::new(catalog([
        ("a.rs", b"pub mod b;\npub fn a(value: i32) {}\n".as_slice()),
        ("b.rs", b"pub fn b() {}\n".as_slice()),
    ]));
    let second = "---\n# untouched second\ncontract_version: 2\nroot: ../src\nfiles: [b.rs]\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: b, root: b.rs, kind: library }] }\nsignatures:\n  - b_function:\n      file: b.rs\n      signature_type: function\n      name: b\n      visibility: public\nsketches: []\n";
    let bytes = format!(
        "---\ncontract_version: 2\nroot: ../src\nfiles: [a.rs, b.rs]\nextraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: a, root: a.rs, kind: library }}] }}\nsignatures:\n  - a_function:\n      file: a.rs\n      signature_type: function\n      name: a\n      visibility: public\nsketches: []\n{second}"
    );

    let generated = fixture.render_existing(catalog_with("main.yml", bytes.as_bytes()));
    let edited = rendered(&generated.contract_files, "main.yml");

    assert!(edited.ends_with(second), "{edited}");
    assert_eq!(generated.counts.document_count, 2);
    assert_eq!(generated.counts.semantically_changed_document_count, 1);
    assert_eq!(generated.counts.byte_changed_document_count, 1);
    contract_inventory(generated.contract_files).expect("edited YAML stream must reparse");
}

#[test]
fn changed_documents_release_scratch_before_the_next_document() {
    let parameters = "a0: i64, a1: i64, a2: i64, a3: i64, a4: i64, a5: i64, a6: i64, a7: i64, a8: i64, a9: i64, a10: i64, a11: i64, a12: i64, a13: i64, a14: i64, a15: i64";
    let first_source = format!("pub fn alpha({parameters}) -> i64 {{ a0 }}\n");
    let second_source = format!("pub fn beta({parameters}) -> i64 {{ a0 }}\n");
    let source = format!("{first_source}{second_source}");
    let fixture = RustYamlTestFixture::new(catalog_with("lib.rs", source.as_bytes()));
    let bytes = br#"---
contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: a, root: lib.rs, kind: library }] }
signatures:
  - alpha_function:
      file: lib.rs
      signature_type: function
      name: alpha
      visibility: public
sketches: []
---
contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: b, root: lib.rs, kind: library }] }
signatures:
  - beta_function:
      file: lib.rs
      signature_type: function
      name: beta
      visibility: public
sketches: []
"#;
    let plan = super::RustGenerationPlan::parse(
        GenerateTarget::Existing(catalog_with("main.yml", bytes)),
        &crate::limits::YamlLimits::default(),
    )
    .expect("multi-document generation plan");
    let mut limits = SignatureLimits::default();
    limits.output.scratch_bytes = 4 * 1024;

    let generated = fixture
        .render_plan_with_limits(plan, ContractScope::Signatures, limits)
        .expect("each document-local peak fits even though both previews do not");
    let edited = generated
        .contract_files
        .get(&CatalogPath::new("main.yml").expect("path"))
        .expect("generated stream");

    assert!(edited.windows(5).any(|window| window == b"alpha"));
    assert!(edited.windows(4).any(|window| window == b"beta"));
    assert_eq!(generated.counts.semantically_changed_document_count, 2);
    contract_inventory(generated.contract_files).expect("streamed documents reparse");
}

#[test]
fn generation_emits_combined_original_shape_and_folds_implementations() {
    let source = catalog_with(
        "lib.rs",
        br#"
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

    assert!(yaml.contains("contract_version: 2"));
    assert!(yaml.contains("root: ../src"));
    assert!(yaml.contains("mode: rust_syntax_v2"));
    assert!(yaml.contains("profile: rust_api_v1"));
    assert!(yaml.contains("signature_type: struct"));
    assert!(yaml.contains("signature_type: method"));
    assert!(yaml.contains("receiver: ref"));
    assert!(yaml.contains("signature_type: function"));
    assert!(yaml.contains("sketches: []"));
    assert!(!yaml.contains("\nversion:"));
    assert!(!yaml.contains("language:"));
    assert!(!yaml.contains("signature_type: implementation"));
    assert_eq!(generated.counts.signature_count, 2);
    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn module_declarations_are_signatures_while_still_driving_traversal() {
    let source = catalog_with(
        "lib.rs",
        br#"mod nested {
pub fn answer() {}
}

pub fn root() {}
"#,
    );
    let fixture = RustYamlTestFixture::new(source.clone());
    let parsed = fixture.parsed_for_yaml();
    let limits = crate::limits::SignatureLimits::default();
    let mut diagnostics =
        crate::languages::rust::parser::source_graph::RustCapabilityDiagnostics::new(
            &limits.diagnostics,
        );
    let projection = parsed
        .project_for_extraction(
            &RustYamlTestFixture::extraction(&source),
            &mut limits.rust.usage(),
            &mut diagnostics,
            &limits.diagnostics,
            &crate::work::CancellationProbe::new(),
        )
        .expect("source projection");
    let parsed_items = projection
        .entries()
        .iter()
        .map(|entry| {
            (
                entry.id().name().to_owned(),
                entry.id().module_id().module_path().segments().to_vec(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        parsed_items,
        [
            ("root".to_owned(), Vec::new()),
            ("nested".to_owned(), Vec::new()),
            ("answer".to_owned(), vec!["nested".to_owned()]),
        ]
    );
    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert_eq!(fixture.source_inventory().len(), 3);
    assert_eq!(generated.counts.signature_count, 3);
    assert!(yaml.contains("module_path:\n    - nested"), "{yaml}");
    assert!(yaml.contains("signature_type: module"), "{yaml}");
    assert!(yaml.contains("name: nested"), "{yaml}");
    assert!(yaml.contains("inline: true"), "{yaml}");

    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn module_visibility_attributes_inline_shape_and_path_override_round_trip() {
    let fixture = RustYamlTestFixture::new(catalog([
        (
            "lib.rs",
            br#"#[cfg(feature = "transport")]
#[path = "platform/transport.rs"]
pub mod transport;

#[must_use = "inspect module"]
pub(crate) mod inline {}
"#
            .as_slice(),
        ),
        ("platform/transport.rs", b"pub fn send() {}\n".as_slice()),
    ]));
    let generated = fixture.render_new("main.yml", &["lib.rs", "platform/transport.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert_eq!(generated.counts.signature_count, 3, "{yaml}");
    assert_eq!(yaml.matches("signature_type: module").count(), 2, "{yaml}");
    assert_eq!(yaml.matches("inline: true").count(), 1, "{yaml}");
    assert!(yaml.contains("path: platform/transport.rs"), "{yaml}");
    assert!(yaml.contains("visibility: public"), "{yaml}");
    assert!(yaml.contains("visibility: crate"), "{yaml}");
    assert!(yaml.contains("cfg:"), "{yaml}");
    assert!(yaml.contains("feature = \"transport\""), "{yaml}");
    assert!(yaml.contains("must_use: inspect module"), "{yaml}");
    assert!(
        generated
            .capability_warnings
            .iter()
            .any(|warning| warning.contains("cfg/cfg_attr on module"))
    );

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
    let source = catalog_with(
        "lib.rs",
        br#"macro_rules! include { () => {}; }
include!("shared.rs");
include!("shared.rs");
"#,
    );
    let fixture = RustYamlTestFixture::new(source.clone());
    let expected = [("include", 1), ("include", 2), ("include", 3)];
    let parsed = fixture.parsed_for_yaml();
    let limits = crate::limits::SignatureLimits::default();
    let mut diagnostics =
        crate::languages::rust::parser::source_graph::RustCapabilityDiagnostics::new(
            &limits.diagnostics,
        );
    let projection = parsed
        .project_for_extraction(
            &RustYamlTestFixture::extraction(&source),
            &mut limits.rust.usage(),
            &mut diagnostics,
            &limits.diagnostics,
            &crate::work::CancellationProbe::new(),
        )
        .expect("source projection");
    let ids = projection
        .entries()
        .iter()
        .map(|entry| (entry.id().name(), entry.id().render()))
        .collect::<Vec<_>>();

    assert_eq!(ids.len(), expected.len());
    for ((name, id), (expected_name, occurrence)) in ids.iter().zip(expected) {
        assert_eq!(*name, expected_name);
        assert!(id.ends_with(&format!(":occurrence:1:{occurrence}")));
    }

    let edited_source = catalog_with(
        "lib.rs",
        br#"include!("third.rs");
macro_rules! include { () => {}; }
include!("first.rs");
"#,
    );
    let edited_fixture = RustYamlTestFixture::new(edited_source.clone());
    let edited = edited_fixture.parsed_for_yaml();
    let edited_limits = crate::limits::SignatureLimits::default();
    let mut edited_diagnostics =
        crate::languages::rust::parser::source_graph::RustCapabilityDiagnostics::new(
            &edited_limits.diagnostics,
        );
    let edited_projection = edited
        .project_for_extraction(
            &RustYamlTestFixture::extraction(&edited_source),
            &mut edited_limits.rust.usage(),
            &mut edited_diagnostics,
            &edited_limits.diagnostics,
            &crate::work::CancellationProbe::new(),
        )
        .expect("edited source projection");
    let edited_ids = edited_projection
        .entries()
        .iter()
        .map(|entry| (entry.id().name(), entry.id().render()))
        .collect::<Vec<_>>();

    assert_eq!(edited_ids.len(), expected.len());
    for ((name, id), (expected_name, occurrence)) in edited_ids.iter().zip(expected) {
        assert_eq!(*name, expected_name);
        assert!(id.ends_with(&format!(":occurrence:1:{occurrence}")));
    }

    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn repeated_macro_round_trip_preserves_numeric_occurrence_order() {
    let source = (1..=11)
        .map(|ordinal| format!("include!(\"{ordinal}.rs\");\n"))
        .collect::<String>();
    let source_files = catalog_with("lib.rs", source.as_bytes());
    let fixture = RustYamlTestFixture::new(source_files.clone());
    let expected = (1..=11).collect::<Vec<_>>();
    let parsed = fixture.parsed_for_yaml();
    let limits = crate::limits::SignatureLimits::default();
    let mut diagnostics =
        crate::languages::rust::parser::source_graph::RustCapabilityDiagnostics::new(
            &limits.diagnostics,
        );
    let projection = parsed
        .project_for_extraction(
            &RustYamlTestFixture::extraction(&source_files),
            &mut limits.rust.usage(),
            &mut diagnostics,
            &limits.diagnostics,
            &crate::work::CancellationProbe::new(),
        )
        .expect("source projection");
    let source_ids = projection
        .entries()
        .iter()
        .map(|entry| entry.id().render())
        .collect::<Vec<_>>();

    assert_eq!(source_ids.len(), expected.len());
    for (ordinal, id) in expected.iter().zip(&source_ids) {
        assert!(id.ends_with(&format!(
            ":occurrence:{}:{ordinal}",
            ordinal.to_string().len()
        )));
    }

    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    let reparsed = RustContractDocuments::parse(
        generated.contract_files.clone(),
        &crate::limits::YamlLimits::default(),
    )
    .expect("generated repeated macro contract");
    let reparsed_ids = reparsed
        .documents
        .iter()
        .flat_map(|document| document.document.signatures.iter())
        .filter_map(|signature| signature.entries.first())
        .map(|entry| entry.id().render())
        .collect::<Vec<_>>();

    assert_eq!(reparsed_ids.len(), expected.len());
    for (ordinal, id) in expected.iter().zip(&reparsed_ids) {
        assert!(id.ends_with(&format!(
            ":occurrence:{}:{ordinal}",
            ordinal.to_string().len()
        )));
    }
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
        br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
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
        .diff_against(
            previous.source_inventory(),
            &crate::limits::DiagnosticLimits::default(),
            &crate::work::CancellationProbe::new(),
        )
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
    let comparison =
        fixture.compare_contract_files(catalog_with("main.yml", swapped_yaml.as_bytes()));
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

    for (name, abi) in [
        ("explicit_rust", "Rust"),
        ("unnamed_extern", "extern"),
        ("c_abi", "C"),
        ("c_unwind_abi", "C-unwind"),
        ("prefixed_abi", "externC"),
        ("underscored_abi", "my_abi"),
    ] {
        assert!(yaml.contains(&format!("name: {name}")), "{yaml}");
        assert!(yaml.contains(&format!("abi: {abi}")), "{yaml}");
    }
    let prefixed_method = yaml
        .split("name: prefixed_method")
        .nth(1)
        .expect("prefixed method output");
    assert!(prefixed_method.contains("receiver: ref"), "{yaml}");
    assert!(prefixed_method.contains("visibility: public"), "{yaml}");
    assert!(prefixed_method.contains("abi: externC"), "{yaml}");
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
    let comparison =
        fixture.compare_contract_files(catalog_with("main.yml", implicit_contract.as_bytes()));
    assert!(
        format!("{:?}", comparison.diagnostics()).contains("Mismatched"),
        "{:?}",
        comparison.diagnostics()
    );
}

#[test]
fn one_physical_source_generates_distinct_signatures_for_two_explicit_crates() {
    let fixture = RustYamlTestFixture::new(catalog_with("shared.rs", b"pub fn shared() {}\n"));
    let existing = catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [shared.rs]
extraction:
  mode: rust_syntax_v2
  profile: rust_api_v1
  crates:
    - { id: library_api, root: shared.rs, kind: library }
    - { id: binary_api, root: shared.rs, kind: binary }
signatures: []
sketches: []
"#,
    );

    let generated = fixture.render_existing(existing);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert_eq!(generated.counts.signature_count, 2, "{yaml}");
    assert_eq!(yaml.matches("name: shared").count(), 2, "{yaml}");
    assert!(yaml.contains("crate_id: library_api"), "{yaml}");
    assert!(yaml.contains("crate_id: binary_api"), "{yaml}");
    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn nested_library_and_binary_main_are_ordinary_visibility_preserving_functions() {
    let library_fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        br#"pub fn main() {}

pub mod nested {
    pub fn main() {}
}
"#,
    ));
    let library_generated = library_fixture.render_existing(catalog_with(
        "library.yml",
        br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: library_api, root: lib.rs, kind: library }] }
signatures: []
sketches: []
"#,
    ));
    let binary_fixture = RustYamlTestFixture::new(catalog_with("main.rs", b"fn main() {}\n"));
    let binary_generated = binary_fixture.render_existing(catalog_with(
        "binary.yml",
        br#"contract_version: 2
root: ../src
files: [main.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: app, root: main.rs, kind: binary }] }
signatures: []
sketches: []
"#,
    ));

    let library = rendered(&library_generated.contract_files, "library.yml");
    let binary = rendered(&binary_generated.contract_files, "binary.yml");

    assert!(!library.contains("main_method"), "{library}");
    assert!(!binary.contains("main_method"), "{binary}");
    assert_eq!(library.matches("signature_type: function").count(), 2);
    assert_eq!(library.matches("name: main").count(), 2);
    assert_eq!(library.matches("visibility: public").count(), 3);
    assert!(library.contains("signature_type: module"), "{library}");
    assert!(library.contains("inline: true"), "{library}");
    assert!(library.contains("crate_id: library_api"), "{library}");
    assert!(library.contains("module_path:"), "{library}");
    assert!(library.contains("nested"), "{library}");
    assert!(binary.contains("signature_type: function"), "{binary}");
    assert!(binary.contains("name: main"), "{binary}");
    assert!(binary.contains("visibility: private"), "{binary}");
    assert!(binary.contains("crate_id: app"), "{binary}");

    library_fixture.assert_generated_matches_source(library_generated.contract_files);
    binary_fixture.assert_generated_matches_source(binary_generated.contract_files);
}

#[test]
fn every_supported_declaration_foreign_item_and_associated_item_round_trips() {
    let fixture = RustYamlTestFixture::new(catalog([
        (
            "lib.rs",
            br#"pub mod api;

pub const LIMIT: usize = 4;
pub enum Choice { Ready }
pub extern crate core as rust_core;
pub fn execute(value: usize) -> usize { value }

unsafe extern "C" {
    pub fn native_execute(value: i32) -> i32;
    pub static mut NATIVE_LIMIT: usize;
    pub type NativeHandle;
    native_items!();
}

pub struct Handler;
pub trait Service {
    const LIMIT: usize;
    type Output: Clone;
    fn execute(&self, value: usize) -> usize;
}

impl Service for Handler {
    const LIMIT: usize = 4;
    type Output = usize;
    fn execute(&self, value: usize) -> usize { value }
}

macro_rules! contract_item { () => {}; }
pub static GLOBAL: usize = 4;
pub trait ServiceAlias = Send + Sync;
pub type Value = usize;
pub union Number { pub integer: u32, pub float: f32 }
pub use crate::api::PublicHandler;

pub mod inline {
    pub fn nested() {}
}
"#
            .as_slice(),
        ),
        ("api.rs", b"pub struct PublicHandler;\n".as_slice()),
    ]));
    let generated = fixture.render_new("main.yml", &["lib.rs", "api.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");

    let rendered_keys = |signature_type: &str| {
        let marker = format!("    signature_type: {signature_type}");
        let marker_index = yaml
            .find(&marker)
            .unwrap_or_else(|| panic!("rendered declaration kind {signature_type}: {yaml}"));
        let start = yaml[..marker_index]
            .rfind("\n- ")
            .expect("rendered signature start");
        let rest = &yaml[marker_index..];
        let end = rest
            .find("\n- ")
            .map_or(yaml.len(), |length| marker_index + length);
        yaml[start..end]
            .lines()
            .filter_map(|line| line.strip_prefix("    "))
            .filter(|line| !line.starts_with(' ') && !line.starts_with("- "))
            .filter_map(|line| line.split_once(':').map(|(key, _)| key))
            .collect::<Vec<_>>()
    };

    #[rustfmt::skip]
    let expected_keys: [(&str, &str); 14] = [
        ("constant", "crate_id file signature_type name visibility type value"),
        ("enum", "crate_id file signature_type name visibility variants"),
        ("extern_crate", "crate_id file signature_type name visibility alias"),
        ("function", "crate_id file signature_type name visibility parameters return_type"),
        ("foreign_module", "crate_id file signature_type name items abi unsafe"),
        ("macro", "crate_id file signature_type name visibility tokens"),
        ("module", "crate_id file signature_type name visibility"),
        ("static", "crate_id file signature_type name visibility type"),
        ("struct", "crate_id file signature_type name visibility implementations"),
        ("trait", "crate_id file signature_type name visibility items"),
        ("trait_alias", "crate_id file signature_type name visibility supertraits"),
        ("type_alias", "crate_id file signature_type name visibility target_type"),
        ("union", "crate_id file signature_type name visibility fields"),
        ("reexport", "crate_id file signature_type name visibility path"),
    ];
    for (signature_type, keys) in expected_keys {
        assert!(
            yaml.contains(&format!("signature_type: {signature_type}")),
            "missing {signature_type}: {yaml}"
        );
        assert_eq!(
            rendered_keys(signature_type).join(" "),
            keys,
            "{signature_type}: {yaml}"
        );
    }
    for associated_type in [
        "associated_constant",
        "associated_type",
        "method",
        "foreign_function",
        "foreign_macro",
        "foreign_static",
        "foreign_type",
    ] {
        assert!(
            yaml.contains(&format!("signature_type: {associated_type}")),
            "missing {associated_type}: {yaml}"
        );
    }
    assert!(yaml.contains("implementations:"), "{yaml}");
    assert!(yaml.contains("items:"), "{yaml}");
    assert!(yaml.contains("tokens: native_items ! ()"), "{yaml}");
    assert!(!yaml.contains("\n      methods:"), "{yaml}");
    assert!(!yaml.contains("signature_type: implementation"), "{yaml}");
    assert_eq!(yaml.matches("signature_type: module").count(), 2, "{yaml}");
    assert_eq!(yaml.matches("module_path:").count(), 2, "{yaml}");
    assert!(yaml.contains("api"), "{yaml}");
    assert!(yaml.contains("inline"), "{yaml}");
    assert!(yaml.contains("crate_id: sample"), "{yaml}");

    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn top_level_macro_tokens_render_canonically_and_round_trip() {
    let fixture = RustYamlTestFixture::new(catalog_with("lib.rs", b"contract_item!();\n"));
    let first = fixture.render_new("main.yml", &["lib.rs"]);
    let first_yaml = rendered(&first.contract_files, "main.yml");

    assert!(first_yaml.contains("signature_type: macro"), "{first_yaml}");
    assert!(
        first_yaml.contains("tokens: contract_item ! ()"),
        "{first_yaml}"
    );
    assert!(first_yaml.contains("visibility: private"), "{first_yaml}");
    fixture.assert_generated_matches_source(first.contract_files.clone());

    let second = fixture.render_existing(first.contract_files.clone());
    assert_eq!(second.contract_files, first.contract_files);
}

#[test]
fn semantic_attributes_round_trip_at_every_modeled_yaml_owner() {
    let fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        br#"#[repr(C, align(16))]
#[derive(Clone, serde::Serialize, Clone)]
#[must_use = "consume the value"]
pub struct Packet {
    #[cfg(feature = "tag")]
    pub tag: usize,
}

#[non_exhaustive]
pub enum Status {
    #[cfg(feature = "ready")]
    Ready,
}

#[deprecated(since = "2.0.0", note = "use execute_v2")]
#[doc(hidden)]
#[contract_runtime(mode = "stable")]
pub fn execute<#[cfg(feature = "generic")] T>(
    #[cfg(feature = "argument")] value: T,
) -> T {
    value
}

pub trait Service {
    #[deprecated(note = "use NewOutput")]
    type Output;

    #[must_use = "inspect the result"]
    fn execute(&self) -> bool;
}

pub struct Handler;
impl Service for Handler {
    #[cfg(feature = "output")]
    type Output = usize;

    #[must_use = "inspect the result"]
    fn execute(&self) -> bool { true }
}

#[link(name = "contract_native", kind = "static")]
unsafe extern "C" {
    #[link_name = "native_execute_v2"]
    pub fn native_execute();
}

#[unsafe(no_mangle)]
#[unsafe(export_name = "contract_export")]
#[unsafe(link_section = ".contract")]
pub extern "C" fn exported() {}
"#,
    ));
    let existing = catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures: []
sketches: []
"#,
    );

    let generated = fixture.render_existing(existing);
    let yaml = rendered(&generated.contract_files, "main.yml");

    for attribute in [
        "derive:",
        "repr:",
        "non_exhaustive",
        "cfg:",
        "deprecated:",
        "must_use:",
        "doc_hidden",
        "no_mangle",
        "export_name:",
        "link_section:",
        "link_name:",
        "link:",
        "unresolved:",
    ] {
        assert!(yaml.contains(attribute), "missing {attribute}: {yaml}");
    }
    assert!(
        yaml.contains("derive:\n        - Clone\n        - serde::Serialize\n        - Clone"),
        "derive order and duplicates were not preserved: {yaml}"
    );
    assert!(!yaml.contains("ordinary prose"), "{yaml}");

    fixture.assert_generated_matches_source(generated.contract_files);
}

struct RustYamlVisibilityCase {
    name: &'static str,
    left: &'static [u8],
    right: &'static [u8],
}

impl RustYamlVisibilityCase {
    fn assert_semantically_equivalent(self) {
        let left = RustYamlTestFixture::new(catalog_with("lib.rs", self.left));
        let right = RustYamlTestFixture::new(catalog_with("lib.rs", self.right));

        assert_eq!(
            left.source_inventory()
                .source_shape_digest(&crate::work::CancellationProbe::new())
                .expect("left visibility source-shape digest"),
            right
                .source_inventory()
                .source_shape_digest(&crate::work::CancellationProbe::new())
                .expect("right visibility source-shape digest"),
            "{} changed rust_api_v1 identity",
            self.name
        );

        let target = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures: []
sketches: []
"#;
        let left = left.render_existing(catalog_with("main.yml", target));
        let right = right.render_existing(catalog_with("main.yml", target));
        assert_eq!(
            left.contract_files, right.contract_files,
            "{} did not render to one canonical visibility",
            self.name
        );
    }
}

#[test]
fn equivalent_rust_visibility_spellings_generate_one_semantic_yaml_value() {
    let cases = [
        RustYamlVisibilityCase {
            name: "inherited/private/self",
            left: b"fn visible() {}\n",
            right: b"pub(self) fn visible() {}\n",
        },
        RustYamlVisibilityCase {
            name: "crate/in-crate",
            left: b"pub(crate) fn visible() {}\n",
            right: b"pub(in crate) fn visible() {}\n",
        },
        RustYamlVisibilityCase {
            name: "super/canonical ancestor",
            left: br#"mod outer {
    mod inner {
        pub(super) fn visible() {}
    }
}
"#,
            right: br#"mod outer {
    mod inner {
        pub(in crate::outer) fn visible() {}
    }
}
"#,
        },
    ];

    for case in cases {
        case.assert_semantically_equivalent();
    }
}

#[test]
fn patterns_render_faithfully_but_do_not_change_rust_api_identity() {
    let named = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"pub fn submit(value: (Request, usize)) {}\n",
    ));
    let destructured = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"pub fn submit((request, attempts): (Request, usize)) {}\n",
    ));

    assert_eq!(
        named
            .source_inventory()
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("named-parameter source-shape digest"),
        destructured
            .source_inventory()
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("destructured-parameter source-shape digest"),
        "parameter binding shape entered rust_api_v1"
    );

    let target = br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures: []
sketches: []
"#;
    let named = named.render_existing(catalog_with("main.yml", target));
    let destructured = destructured.render_existing(catalog_with("main.yml", target));
    let named_yaml = rendered(&named.contract_files, "main.yml");
    let destructured_yaml = rendered(&destructured.contract_files, "main.yml");

    assert_ne!(named_yaml, destructured_yaml);
    assert!(named_yaml.contains("pattern: \"value\""), "{named_yaml}");
    assert!(
        destructured_yaml.contains("(request , attempts)"),
        "{destructured_yaml}"
    );
    assert!(
        destructured_yaml.contains("(Request, usize)"),
        "{destructured_yaml}"
    );

    let patterns = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        br#"pub struct Point { pub x: i32, pub y: i32 }

pub fn patterns(
    mut mutable: i32,
    ref borrowed: i32,
    whole @ _: i32,
    (left, right): (i32, i32),
    Point { x, y }: Point,
    [first, second]: [i32; 2],
) {}
"#,
    ));
    let generated = patterns.render_existing(catalog_with("main.yml", target));
    let yaml = rendered(&generated.contract_files, "main.yml");

    for pattern in [
        "mut mutable",
        "ref borrowed",
        "whole @ _",
        "(left , right)",
        "Point { x , y }",
        "[first , second]",
    ] {
        assert!(yaml.contains(pattern), "missing pattern {pattern}: {yaml}");
    }
    patterns.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn cross_file_implementation_folds_into_its_local_owner() {
    let source = catalog([
        ("lib.rs", b"mod models;\nmod impls;\n".as_slice()),
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
    let generated = fixture.render_new("main.yml", &["lib.rs", "impls.rs", "models.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert_eq!(generated.counts.signature_count, 3);
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
        declaration_fixture
            .source_inventory()
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("declaration-first source-shape digest"),
        implementation_fixture
            .source_inventory()
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("implementation-first source-shape digest")
    );
    assert_eq!(
        declaration_generated.contract_files,
        implementation_generated.contract_files
    );
    let yaml = rendered(&declaration_generated.contract_files, "main.yml");
    let hidden = yaml
        .split("name: hidden")
        .nth(1)
        .and_then(|section| section.split("name: visible").next())
        .expect("hidden method output");
    let visible = yaml
        .split("name: visible")
        .nth(1)
        .expect("visible method output");
    assert!(hidden.contains("receiver: ref"), "{yaml}");
    assert!(!hidden.contains("visibility:"), "{yaml}");
    assert!(visible.contains("receiver: ref"), "{yaml}");
    assert!(visible.contains("visibility: public"), "{yaml}");
    declaration_fixture.assert_generated_matches_source(declaration_generated.contract_files);
}

#[test]
fn reversed_hand_authored_implementation_items_match_the_source_inventory() {
    let fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        br#"pub struct Thing;

impl Thing {
    pub fn alpha(&self) {}
    pub fn beta(&self) {}
}
"#,
    ));
    let contract = catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - thing_struct:
      file: lib.rs
      signature_type: struct
      name: Thing
      visibility: public
      implementations:
        - items:
            - signature_type: method
              name: beta
              receiver: ref
              visibility: public
            - signature_type: method
              name: alpha
              receiver: ref
              visibility: public
sketches: []
"#,
    );

    let comparison = fixture.compare_contract_files(contract);

    assert!(
        comparison.diagnostics().is_empty(),
        "{:#?}",
        comparison.diagnostics()
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
        "    implementations:\n    - items:\n      - signature_type: method\n        name: value\n        receiver: ref\n        visibility: public\n        return_type: u8\n",
        "",
        1,
    );
    assert_ne!(without_method, yaml, "method-removal probe must apply");
    let incomplete =
        fixture.compare_contract_files(catalog_with("main.yml", without_method.as_bytes()));
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
        ("lib.rs", b"mod models;\nmod a;\nmod b;\n".as_slice()),
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
        ("lib.rs", b"mod models;\nmod a;\nmod b;\n".as_slice()),
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
    let files = ["lib.rs", "a.rs", "b.rs", "models.rs"];
    let first_generated = first_fixture.render_new("main.yml", &files);
    let relocated_generated = relocated_fixture.render_new("main.yml", &files);

    assert_eq!(
        first_fixture
            .source_inventory()
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("first-layout source-shape digest"),
        relocated_fixture
            .source_inventory()
            .source_shape_digest(&crate::work::CancellationProbe::new())
            .expect("relocated-layout source-shape digest")
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
        (
            "lib.rs",
            b"mod models;\nmod clone_impl;\nmod copy_impl;\n".as_slice(),
        ),
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
    let generated = fixture.render_new(
        "main.yml",
        &["lib.rs", "clone_impl.rs", "copy_impl.rs", "models.rs"],
    );
    let yaml = rendered(&generated.contract_files, "main.yml");
    assert!(yaml.contains("T: Clone"), "{yaml}");
    assert!(yaml.contains("T: Copy"), "{yaml}");

    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn qualified_cross_file_trait_implementation_uses_private_owner_visibility() {
    let source = catalog([
        (
            "lib.rs",
            b"mod models;\nmod traits;\nmod impls;\n".as_slice(),
        ),
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
    let generated = fixture.render_new(
        "main.yml",
        &["lib.rs", "impls.rs", "models.rs", "traits.rs"],
    );
    let yaml = rendered(&generated.contract_files, "main.yml");
    let hidden = yaml
        .split("- hidden_struct:")
        .nth(1)
        .and_then(|section| section.split("- named_trait:").next())
        .expect("hidden owner output");
    assert!(hidden.contains("visibility: private"), "{yaml}");
    assert!(hidden.contains("crate :: traits :: Named"), "{yaml}");
    assert!(hidden.contains("name: name"), "{yaml}");
    assert!(hidden.contains("receiver: ref"), "{yaml}");

    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn qualified_implementation_owners_resolve_crate_self_and_super_paths() {
    let source = catalog([
        (
            "lib.rs",
            b"mod local;\nmod models;\nmod impls;\nmod nested;\n".as_slice(),
        ),
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
        (
            "nested/mod.rs",
            b"pub mod child;\npub struct Parent;\n".as_slice(),
        ),
        (
            "nested/child.rs",
            b"impl super::Parent { pub fn parent(&self) {} }\n".as_slice(),
        ),
    ]);
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_new(
        "main.yml",
        &[
            "lib.rs",
            "impls.rs",
            "local.rs",
            "models.rs",
            "nested/child.rs",
            "nested/mod.rs",
        ],
    );

    assert_eq!(generated.counts.signature_count, 8);
    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn qualified_owner_selects_between_same_named_types() {
    let source = catalog([
        ("lib.rs", b"mod left;\nmod right;\nmod impls;\n".as_slice()),
        ("left.rs", b"pub struct Thing;\n".as_slice()),
        ("right.rs", b"pub struct Thing;\n".as_slice()),
        (
            "impls.rs",
            b"impl crate::right::Thing { pub fn selected(&self) {} }\n".as_slice(),
        ),
    ]);
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_new("main.yml", &["lib.rs", "impls.rs", "left.rs", "right.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");
    assert!(yaml.contains("- thing_struct:"), "{yaml}");
    assert!(yaml.contains("- sample_right_thing_struct:"), "{yaml}");
    assert!(!yaml.contains("left_rs_thing_struct"), "{yaml}");
    assert!(!yaml.contains("right_rs_thing_struct"), "{yaml}");
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
        "!Marker",
        "trait: UnsafeMarker",
        "impl_qualifiers:\n      - unsafe",
        "impl_qualifiers:\n      - default",
        "generics:\n      - T",
        "T : Clone",
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
        "pub fn 東京() {}\npub fn 大阪() {}\npub fn 서울() {}\n".as_bytes(),
    );
    let fixture = RustYamlTestFixture::new(source);
    let first = fixture.render_new("main.yml", &["lib.rs"]);
    let second = fixture.render_new("main.yml", &["lib.rs"]);

    assert_eq!(first.contract_files, second.contract_files);
    let yaml = rendered(&first.contract_files, "main.yml");
    assert!(yaml.contains("- _function:"), "{yaml}");
    assert!(yaml.contains("- sample_function:"), "{yaml}");
    assert!(yaml.contains("- sample_function_2:"), "{yaml}");

    fixture.assert_generated_matches_source(first.contract_files);
}

#[test]
fn generated_label_ordinals_scale_across_one_sanitized_prefix() {
    let mut source = String::new();
    for offset in 0..256_u32 {
        let identifier = char::from_u32(0x4e00 + offset).expect("CJK identifier character");
        source.push_str(&format!("pub fn {identifier}() {{}}\n"));
    }
    let fixture = RustYamlTestFixture::new(catalog_with("lib.rs", source.as_bytes()));
    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert!(yaml.contains("- _function:"), "{yaml}");
    assert!(yaml.contains("- sample_function:"), "{yaml}");
    assert!(yaml.contains("- sample_function_255:"), "{yaml}");
    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn existing_labels_reserve_lossy_candidates_for_new_and_removed_items() {
    let existing = catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - _function:
      file: lib.rs
      signature_type: function
      name: existing
      visibility: public
  - sample_function:
      file: lib.rs
      signature_type: function
      name: other
      visibility: public
sketches: []
"#,
    );
    let source = catalog_with(
        "lib.rs",
        "pub fn existing() {}\npub fn other() {}\npub fn 東京() {}\n".as_bytes(),
    );
    let fixture = RustYamlTestFixture::new(source);
    let generated = fixture.render_existing(existing);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert!(yaml.contains("- _function:"), "{yaml}");
    assert!(yaml.contains("name: existing"), "{yaml}");
    assert!(yaml.contains("- sample_function:"), "{yaml}");
    assert!(yaml.contains("name: other"), "{yaml}");
    assert!(yaml.contains("- sample_function_2:"), "{yaml}");
    assert!(yaml.contains("name: 東京"), "{yaml}");
    fixture.assert_generated_matches_source(generated.contract_files);

    let removed_existing = catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - _function:
      file: lib.rs
      signature_type: function
      name: removed
      visibility: public
  - sample_function:
      file: lib.rs
      signature_type: function
      name: also_removed
      visibility: public
sketches: []
"#,
    );
    let fixture = RustYamlTestFixture::new(catalog_with("lib.rs", "pub fn 東京() {}\n".as_bytes()));
    let generated = fixture.render_existing(removed_existing);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert!(!yaml.contains("- _function:"), "{yaml}");
    assert!(!yaml.contains("- sample_function:"), "{yaml}");
    assert!(yaml.contains("- sample_function_2:"), "{yaml}");
    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn raw_source_identifiers_render_one_unraw_semantic_name() {
    let fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"pub fn r#type<r#match>(value: r#match) -> r#match { value }\n",
    ));
    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert!(yaml.contains("- type_function:"), "{yaml}");
    assert!(yaml.contains("name: type"), "{yaml}");
    assert!(yaml.contains("r#match"), "{yaml}");
    assert!(!yaml.contains("r#type"), "{yaml}");
    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn raw_named_type_path_generates_valid_yaml_type_text_without_warning() {
    let fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"pub struct r#type;\npub fn value() -> r#type { r#type }\n",
    ));
    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert!(yaml.contains("r#type"), "{yaml}");
    assert!(!yaml.contains("return_type: type"), "{yaml}");
    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn raw_reexport_path_renders_canonical_semantic_segments() {
    let fixture = RustYamlTestFixture::new(catalog_with(
        "lib.rs",
        b"pub mod r#type { pub fn r#match() {} }\npub use r#type::r#match;\n",
    ));
    let generated = fixture.render_new("main.yml", &["lib.rs"]);
    let yaml = rendered(&generated.contract_files, "main.yml");

    assert!(yaml.contains("signature_type: reexport"), "{yaml}");
    assert!(yaml.contains("path: type::match"), "{yaml}");
    assert!(!yaml.contains("r#type"), "{yaml}");
    assert!(!yaml.contains("r#match"), "{yaml}");
    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn generation_preserves_stable_label_and_linked_sketch() {
    let source = catalog_with("lib.rs", b"pub fn answer() -> i32 { 42 }\n");
    let existing = catalog_with(
        "main.yml",
        br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - public_answer:
      file: lib.rs
      signature_type: function
      name: answer
      visibility: public
      sketch: answer_example
sketches:
  - answer_example:
      file: lib.rs
      signature: public_answer
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: at_least_one }
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
    assert_eq!(generated.counts.document_count, 1);
    assert_eq!(generated.counts.signature_count, 1);
    assert_eq!(generated.counts.preserved_sketch_count, 1);
    assert_eq!(generated.counts.semantically_changed_document_count, 1);
    assert_eq!(generated.counts.byte_changed_document_count, 1);
    fixture.assert_generated_matches_source(generated.contract_files);
}

#[test]
fn trait_implementation_methods_preserve_inherited_visibility() {
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

    let private_owner = yaml
        .split("- privatething_struct:")
        .nth(1)
        .and_then(|section| section.split("- publicthing_struct:").next())
        .expect("private owner output");
    let public_owner = yaml
        .split("- publicthing_struct:")
        .nth(1)
        .and_then(|section| section.split("- named_trait:").next())
        .expect("public owner output");
    assert!(private_owner.contains("visibility: private"), "{yaml}");
    assert!(public_owner.contains("visibility: public"), "{yaml}");
    assert_eq!(private_owner.matches("visibility:").count(), 1, "{yaml}");
    assert_eq!(public_owner.matches("visibility:").count(), 2, "{yaml}");
    assert!(private_owner.contains("trait: Named"), "{yaml}");
    assert!(public_owner.contains("trait: Named"), "{yaml}");
    assert!(private_owner.contains("name: name"), "{yaml}");
    assert!(public_owner.contains("name: name"), "{yaml}");

    fixture.assert_generated_matches_source(generated.contract_files);
}
