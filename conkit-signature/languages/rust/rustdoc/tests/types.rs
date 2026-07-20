use super::*;

#[test]
fn generic_alias_application_normalizes_to_underlying_compiler_type() {
    let alias_id = 2;
    let mut aliased_document = CompilerArtifactFixture::rustdoc_document();
    aliased_document.index.insert(
        rustdoc_types::Id(alias_id),
        CompilerArtifactFixture::type_alias_item(
            alias_id,
            "Alias",
            vec![rustdoc_types::GenericParamDef {
                name: "T".to_owned(),
                kind: rustdoc_types::GenericParamDefKind::Type {
                    bounds: Vec::new(),
                    default: None,
                    is_synthetic: false,
                },
            }],
            rustdoc_types::Type::Generic("T".to_owned()),
        ),
    );
    CompilerArtifactFixture::add_path(
        &mut aliased_document,
        alias_id,
        "Alias",
        rustdoc_types::ItemKind::TypeAlias,
    );
    let string_id = rustdoc_types::Id(100);
    let string_type = CompilerArtifactFixture::resolved_type(string_id.0, "String", None);
    aliased_document.paths.insert(
        string_id,
        rustdoc_types::ItemSummary {
            crate_id: 1,
            path: vec!["alloc".to_owned(), "string".to_owned(), "String".to_owned()],
            kind: rustdoc_types::ItemKind::Struct,
        },
    );
    CompilerArtifactFixture::set_function_output(
        &mut aliased_document,
        CompilerArtifactFixture::resolved_type(
            alias_id,
            "Alias",
            Some(rustdoc_types::GenericArgs::AngleBracketed {
                args: vec![rustdoc_types::GenericArg::Type(string_type.clone())],
                constraints: Vec::new(),
            }),
        ),
    );
    CompilerArtifactFixture::set_root_items(
        &mut aliased_document,
        vec![rustdoc_types::Id(alias_id), rustdoc_types::Id(1)],
    );
    let aliased =
        CompilerArtifactFixture::extract(aliased_document).expect("generic alias extraction");

    let mut direct_document = CompilerArtifactFixture::rustdoc_document();
    direct_document.paths.insert(
        string_id,
        rustdoc_types::ItemSummary {
            crate_id: 1,
            path: vec!["alloc".to_owned(), "string".to_owned(), "String".to_owned()],
            kind: rustdoc_types::ItemKind::Struct,
        },
    );
    CompilerArtifactFixture::set_function_output(&mut direct_document, string_type);
    let direct =
        CompilerArtifactFixture::extract(direct_document).expect("direct compiler type extraction");
    let aliased_function = aliased
        .projection()
        .entries()
        .iter()
        .find(|entry| matches!(entry.declaration(), RustDeclaration::Function(_)))
        .expect("aliased function")
        .declaration()
        .clone();
    let direct_function = direct.projection().entries()[0].declaration().clone();

    assert_eq!(aliased_function, direct_function);
    assert!(
        aliased
            .projection()
            .entries()
            .iter()
            .any(|entry| matches!(entry.declaration(), RustDeclaration::TypeAlias(_)))
    );
}

#[test]
fn nested_alias_substitution_applies_lifetime_type_and_const_defaults() {
    let inner_id = 2;
    let outer_id = 3;
    let mut document = CompilerArtifactFixture::rustdoc_document();
    document.index.insert(
        rustdoc_types::Id(inner_id),
        CompilerArtifactFixture::type_alias_item(
            inner_id,
            "Inner",
            vec![rustdoc_types::GenericParamDef {
                name: "T".to_owned(),
                kind: rustdoc_types::GenericParamDefKind::Type {
                    bounds: Vec::new(),
                    default: None,
                    is_synthetic: false,
                },
            }],
            rustdoc_types::Type::Generic("T".to_owned()),
        ),
    );
    let inner_application = CompilerArtifactFixture::resolved_type(
        inner_id,
        "Inner",
        Some(rustdoc_types::GenericArgs::AngleBracketed {
            args: vec![rustdoc_types::GenericArg::Type(
                rustdoc_types::Type::Generic("T".to_owned()),
            )],
            constraints: Vec::new(),
        }),
    );
    document.index.insert(
        rustdoc_types::Id(outer_id),
        CompilerArtifactFixture::type_alias_item(
            outer_id,
            "Outer",
            vec![
                rustdoc_types::GenericParamDef {
                    name: "'a".to_owned(),
                    kind: rustdoc_types::GenericParamDefKind::Lifetime {
                        outlives: Vec::new(),
                    },
                },
                rustdoc_types::GenericParamDef {
                    name: "T".to_owned(),
                    kind: rustdoc_types::GenericParamDefKind::Type {
                        bounds: Vec::new(),
                        default: Some(rustdoc_types::Type::Primitive("u8".to_owned())),
                        is_synthetic: false,
                    },
                },
                rustdoc_types::GenericParamDef {
                    name: "N".to_owned(),
                    kind: rustdoc_types::GenericParamDefKind::Const {
                        type_: rustdoc_types::Type::Primitive("usize".to_owned()),
                        default: Some("4".to_owned()),
                    },
                },
            ],
            rustdoc_types::Type::BorrowedRef {
                lifetime: Some("'a".to_owned()),
                is_mutable: false,
                type_: Box::new(rustdoc_types::Type::Array {
                    type_: Box::new(inner_application),
                    len: "N".to_owned(),
                }),
            },
        ),
    );
    for (id, name) in [(inner_id, "Inner"), (outer_id, "Outer")] {
        CompilerArtifactFixture::add_path(
            &mut document,
            id,
            name,
            rustdoc_types::ItemKind::TypeAlias,
        );
    }
    CompilerArtifactFixture::set_function_output(
        &mut document,
        CompilerArtifactFixture::resolved_type(
            outer_id,
            "Outer",
            Some(rustdoc_types::GenericArgs::AngleBracketed {
                args: vec![rustdoc_types::GenericArg::Lifetime("'static".to_owned())],
                constraints: Vec::new(),
            }),
        ),
    );
    let mut explicit = document
        .index
        .get(&rustdoc_types::Id(1))
        .expect("fixture function")
        .clone();
    explicit.id = rustdoc_types::Id(4);
    explicit.name = Some("explicit".to_owned());
    let rustdoc_types::ItemEnum::Function(explicit_function) = &mut explicit.inner else {
        panic!("fixture item must remain a function");
    };
    explicit_function.sig.output = Some(CompilerArtifactFixture::resolved_type(
        outer_id,
        "Outer",
        Some(rustdoc_types::GenericArgs::AngleBracketed {
            args: vec![
                rustdoc_types::GenericArg::Lifetime("'static".to_owned()),
                rustdoc_types::GenericArg::Type(rustdoc_types::Type::Primitive("u16".to_owned())),
                rustdoc_types::GenericArg::Const(rustdoc_types::Constant {
                    expr: "8".to_owned(),
                    value: Some("8".to_owned()),
                    is_literal: true,
                }),
            ],
            constraints: Vec::new(),
        }),
    ));
    document.index.insert(explicit.id, explicit);
    CompilerArtifactFixture::add_path(
        &mut document,
        4,
        "explicit",
        rustdoc_types::ItemKind::Function,
    );
    CompilerArtifactFixture::set_root_items(
        &mut document,
        vec![
            rustdoc_types::Id(inner_id),
            rustdoc_types::Id(outer_id),
            rustdoc_types::Id(1),
            rustdoc_types::Id(4),
        ],
    );
    let normalized = CompilerArtifactFixture::extract(document).expect("nested alias defaults");

    let mut direct_document = CompilerArtifactFixture::rustdoc_document();
    CompilerArtifactFixture::set_function_output(
        &mut direct_document,
        rustdoc_types::Type::BorrowedRef {
            lifetime: Some("'static".to_owned()),
            is_mutable: false,
            type_: Box::new(rustdoc_types::Type::Array {
                type_: Box::new(rustdoc_types::Type::Primitive("u8".to_owned())),
                len: "4".to_owned(),
            }),
        },
    );
    let mut explicit = direct_document
        .index
        .get(&rustdoc_types::Id(1))
        .expect("fixture function")
        .clone();
    explicit.id = rustdoc_types::Id(4);
    explicit.name = Some("explicit".to_owned());
    let rustdoc_types::ItemEnum::Function(explicit_function) = &mut explicit.inner else {
        panic!("fixture item must remain a function");
    };
    explicit_function.sig.output = Some(rustdoc_types::Type::BorrowedRef {
        lifetime: Some("'static".to_owned()),
        is_mutable: false,
        type_: Box::new(rustdoc_types::Type::Array {
            type_: Box::new(rustdoc_types::Type::Primitive("u16".to_owned())),
            len: "8".to_owned(),
        }),
    });
    direct_document.index.insert(explicit.id, explicit);
    CompilerArtifactFixture::add_path(
        &mut direct_document,
        4,
        "explicit",
        rustdoc_types::ItemKind::Function,
    );
    CompilerArtifactFixture::set_root_items(
        &mut direct_document,
        vec![rustdoc_types::Id(1), rustdoc_types::Id(4)],
    );
    let direct =
        CompilerArtifactFixture::extract(direct_document).expect("direct substituted type");
    let normalized_functions = normalized
        .projection()
        .entries()
        .iter()
        .filter_map(|entry| match entry.declaration() {
            RustDeclaration::Function(function) => Some(function.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let direct_functions = direct
        .projection()
        .entries()
        .iter()
        .filter_map(|entry| match entry.declaration() {
            RustDeclaration::Function(function) => Some(function.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(normalized_functions, direct_functions);
}

#[test]
fn alias_cycles_fail_with_exact_item_chain_detection() {
    let mut document = CompilerArtifactFixture::rustdoc_document();
    document.index.insert(
        rustdoc_types::Id(2),
        CompilerArtifactFixture::type_alias_item(
            2,
            "First",
            Vec::new(),
            CompilerArtifactFixture::resolved_type(3, "Second", None),
        ),
    );
    document.index.insert(
        rustdoc_types::Id(3),
        CompilerArtifactFixture::type_alias_item(
            3,
            "Second",
            Vec::new(),
            CompilerArtifactFixture::resolved_type(2, "First", None),
        ),
    );
    for (id, name) in [(2, "First"), (3, "Second")] {
        CompilerArtifactFixture::add_path(
            &mut document,
            id,
            name,
            rustdoc_types::ItemKind::TypeAlias,
        );
    }
    CompilerArtifactFixture::set_root_items(
        &mut document,
        vec![
            rustdoc_types::Id(2),
            rustdoc_types::Id(3),
            rustdoc_types::Id(1),
        ],
    );
    CompilerArtifactFixture::set_function_output(
        &mut document,
        CompilerArtifactFixture::resolved_type(2, "First", None),
    );

    let error = CompilerArtifactFixture::document_error(document);
    assert!(matches!(
        error.compiler_artifact_failure(),
        Some(RustCompilerArtifactFailure::UnsupportedType { reason, .. })
            if reason.contains("recursive type alias cycle")
    ));
}

#[test]
fn alias_arity_and_associated_constraints_fail_explicitly() {
    for (arguments, expected) in [
        (
            rustdoc_types::GenericArgs::AngleBracketed {
                args: vec![
                    rustdoc_types::GenericArg::Type(rustdoc_types::Type::Primitive(
                        "u8".to_owned(),
                    )),
                    rustdoc_types::GenericArg::Type(rustdoc_types::Type::Primitive(
                        "u16".to_owned(),
                    )),
                ],
                constraints: Vec::new(),
            },
            "at most 1 generic arguments",
        ),
        (
            rustdoc_types::GenericArgs::AngleBracketed {
                args: vec![rustdoc_types::GenericArg::Type(
                    rustdoc_types::Type::Primitive("u8".to_owned()),
                )],
                constraints: vec![rustdoc_types::AssocItemConstraint {
                    name: "Item".to_owned(),
                    args: None,
                    binding: rustdoc_types::AssocItemConstraintKind::Equality(
                        rustdoc_types::Term::Type(rustdoc_types::Type::Primitive("u8".to_owned())),
                    ),
                }],
            },
            "associated-item constraints",
        ),
    ] {
        let mut document = CompilerArtifactFixture::rustdoc_document();
        document.index.insert(
            rustdoc_types::Id(2),
            CompilerArtifactFixture::type_alias_item(
                2,
                "Alias",
                vec![rustdoc_types::GenericParamDef {
                    name: "T".to_owned(),
                    kind: rustdoc_types::GenericParamDefKind::Type {
                        bounds: Vec::new(),
                        default: None,
                        is_synthetic: false,
                    },
                }],
                rustdoc_types::Type::Generic("T".to_owned()),
            ),
        );
        CompilerArtifactFixture::add_path(
            &mut document,
            2,
            "Alias",
            rustdoc_types::ItemKind::TypeAlias,
        );
        CompilerArtifactFixture::set_root_items(&mut document, vec![rustdoc_types::Id(1)]);
        CompilerArtifactFixture::set_function_output(
            &mut document,
            CompilerArtifactFixture::resolved_type(2, "Alias", Some(arguments)),
        );
        let error = CompilerArtifactFixture::document_error(document);
        assert!(matches!(
            error.compiler_artifact_failure(),
            Some(RustCompilerArtifactFailure::UnsupportedType { reason, .. })
                if reason.contains(expected)
        ));
    }
}

#[test]
fn syntax_and_compiler_generation_agree_across_the_modeled_rust_api_surface() {
    let sources =
        CompilerArtifactFixture::sources_from(CompilerArtifactFixture::supported_surface_source());
    let target = GenerateDocument {
        contract_file: CatalogPath::new("main.yml").expect("contract path"),
        root: "../src".to_owned(),
        files: vec![CatalogPath::new("lib.rs").expect("source path")],
        crates: vec![RustCrateRoot {
            id: "sample".to_owned(),
            root: CatalogPath::new("lib.rs").expect("crate root"),
            kind: RustCrateKind::Library,
        }],
    };
    let kit = SignatureContractKit::builder()
        .build()
        .expect("signature kit");
    let syntax = futures_executor::block_on(kit.generate(GenerateRequest {
        extraction: RustExtractionInput::Syntax,
        source_files: sources.clone(),
        target: GenerateTarget::New(target.clone()),
        scope: ContractScope::Signatures,
    }))
    .expect("syntax surface generation");
    let compiler = futures_executor::block_on(kit.generate(GenerateRequest {
        extraction: RustExtractionInput::Compiler(CompilerArtifactFixture::artifact_for(
            CompilerArtifactFixture::supported_surface_document(),
        )),
        source_files: sources,
        target: GenerateTarget::New(target),
        scope: ContractScope::Signatures,
    }))
    .expect("compiler surface generation");
    let contract_path = CatalogPath::new("main.yml").expect("contract path");

    assert_eq!(syntax.counts.signature_count, 5);
    assert_eq!(compiler.counts.signature_count, 5);
    assert_eq!(
        CompilerArtifactFixture::signature_section(
            syntax
                .contract_files
                .get(&contract_path)
                .expect("syntax contract")
        ),
        CompilerArtifactFixture::signature_section(
            compiler
                .contract_files
                .get(&contract_path)
                .expect("compiler contract")
        ),
        "functions, aggregates, traits, associated items, generics, attributes, visibility, and implementation ownership must share one canonical surface",
    );
}

#[test]
fn compiler_rust_representation_is_the_implicit_default_and_retains_modifiers() {
    let mut default_document = CompilerArtifactFixture::supported_surface_document();
    default_document
        .index
        .get_mut(&rustdoc_types::Id(2))
        .expect("record item")
        .attrs = vec![rustdoc_types::Attribute::Repr(
        rustdoc_types::AttributeRepr {
            kind: rustdoc_types::ReprKind::Rust,
            align: None,
            packed: None,
            int: None,
        },
    )];
    let default = CompilerArtifactFixture::extract(default_document)
        .expect("default Rust representation must extract");
    let record = default
        .projection()
        .entries()
        .iter()
        .find_map(|entry| match entry.declaration() {
            RustDeclaration::Structure(record) => Some(record),
            _ => None,
        })
        .expect("record declaration");
    assert!(record.base().attributes().values().is_empty());

    let mut modified_document = CompilerArtifactFixture::supported_surface_document();
    modified_document
        .index
        .get_mut(&rustdoc_types::Id(2))
        .expect("record item")
        .attrs = vec![rustdoc_types::Attribute::Repr(
        rustdoc_types::AttributeRepr {
            kind: rustdoc_types::ReprKind::Rust,
            align: Some(16),
            packed: Some(2),
            int: None,
        },
    )];
    let modified = CompilerArtifactFixture::extract(modified_document)
        .expect("Rust representation modifiers must extract without a synthetic Rust hint");
    let repr = modified
        .projection()
        .entries()
        .iter()
        .find_map(|entry| match entry.declaration() {
            RustDeclaration::Structure(record) => match record.base().attributes().values() {
                [RustAttribute::Repr(repr)] => Some(repr),
                _ => None,
            },
            _ => None,
        })
        .expect("Rust representation modifiers");
    assert_eq!(
        repr.hints()
            .iter()
            .map(RustReprHint::as_str)
            .collect::<Vec<_>>(),
        ["align(16)", "packed(2)"]
    );
}
