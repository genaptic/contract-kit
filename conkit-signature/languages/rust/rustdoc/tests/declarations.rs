use super::*;
use crate::languages::rust::parser::source_graph::RustModulePath;

#[test]
fn reachable_macro_extern_type_and_primitive_items_fail_with_precise_capability_limits() {
    let cases = [
        (
            rustdoc_types::ItemEnum::Macro("macro_rules! contract_item".to_owned()),
            "Macro",
            "strips declarative-macro matcher patterns",
        ),
        (
            rustdoc_types::ItemEnum::ProcMacro(rustdoc_types::ProcMacro {
                kind: rustdoc_types::MacroKind::Derive,
                helpers: vec!["contract".to_owned()],
            }),
            "ProcDerive",
            "only the procedural-macro invocation kind and helper names",
        ),
        (
            rustdoc_types::ItemEnum::ExternType,
            "ExternType",
            "enclosing foreign module ABI",
        ),
        (
            rustdoc_types::ItemEnum::Primitive(rustdoc_types::Primitive {
                name: "u8".to_owned(),
                impls: Vec::new(),
            }),
            "Primitive",
            "core-library model",
        ),
    ];

    for (inner, expected_kind, expected_reason) in cases {
        let mut document = CompilerArtifactFixture::rustdoc_document();
        let item_id = rustdoc_types::Id(10);
        document.index.insert(
            item_id,
            CompilerArtifactFixture::public_item(item_id.0, Some("contract_item"), inner),
        );
        CompilerArtifactFixture::set_root_items(&mut document, vec![item_id]);

        let error = CompilerArtifactFixture::document_error(document);
        assert!(
            matches!(
                error.compiler_artifact_failure(),
                Some(RustCompilerArtifactFailure::UnsupportedItem {
                    item_id: 10,
                    item_kind,
                    reason,
                }) if item_kind.contains(expected_kind) && reason.contains(expected_reason)
            ),
            "unexpected {expected_kind} error: {error}"
        );
    }
}

#[test]
fn syntax_and_compiler_generation_agree_on_supported_signature_subset() {
    let mut sources = FileCatalog::new();
    sources
        .insert(
            CatalogPath::new("lib.rs").expect("source path"),
            b"pub fn answer() -> u8 { 42 }\n".to_vec(),
        )
        .expect("supported source");
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
    .expect("syntax generation");
    let compiler = futures_executor::block_on(kit.generate(GenerateRequest {
        extraction: RustExtractionInput::Compiler(CompilerArtifactFixture::artifact()),
        source_files: sources,
        target: GenerateTarget::New(target),
        scope: ContractScope::Signatures,
    }))
    .expect("compiler generation");
    let contract_path = CatalogPath::new("main.yml").expect("contract path");
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
        )
    );
}

#[test]
fn compiler_restricted_visibility_distinguishes_crate_and_self_at_the_crate_root() {
    for (path, expected) in [("crate", Visibility::Crate), ("self", Visibility::Private)] {
        let mut document = CompilerArtifactFixture::supported_surface_document();
        document.paths.insert(
            rustdoc_types::Id(0),
            rustdoc_types::ItemSummary {
                crate_id: 0,
                path: vec!["sample".to_owned()],
                kind: rustdoc_types::ItemKind::Module,
            },
        );
        document
            .index
            .get_mut(&rustdoc_types::Id(3))
            .expect("record field")
            .visibility = rustdoc_types::Visibility::Restricted {
            parent: rustdoc_types::Id(0),
            path: path.to_owned(),
        };

        let extraction = CompilerArtifactFixture::extract(document)
            .expect("crate-root restricted visibility must lower");
        let field = extraction
            .projection()
            .entries()
            .iter()
            .find_map(|entry| match entry.declaration() {
                RustDeclaration::Structure(record) if record.base().name() == "Record" => {
                    record.fields().first()
                }
                _ => None,
            })
            .expect("record field declaration");

        assert_eq!(field.visibility(), &expected, "restricted path {path}");
    }
}

#[test]
fn compiler_restricted_visibility_canonicalizes_valid_nested_scopes() {
    let root_parent = rustdoc_types::Id(30);
    let outer_parent = rustdoc_types::Id(31);
    let inner_parent = rustdoc_types::Id(32);
    let mut document = CompilerArtifactFixture::rustdoc_document();
    for (id, path) in [
        (root_parent, vec!["sample".to_owned()]),
        (outer_parent, vec!["sample".to_owned(), "outer".to_owned()]),
        (
            inner_parent,
            vec!["sample".to_owned(), "outer".to_owned(), "inner".to_owned()],
        ),
    ] {
        document.paths.insert(
            id,
            rustdoc_types::ItemSummary {
                crate_id: 0,
                path,
                kind: rustdoc_types::ItemKind::Module,
            },
        );
    }
    let cancellation = CancellationProbe::new();
    let limits = RustExtractionLimits::default();
    let mut artifact = CompilerArtifactFixture::artifact_for(document.clone());
    let context = artifact
        .validate_metadata(&cancellation)
        .expect("compiler context");
    let index = RustdocIndex {
        context,
        document,
        source_map: std::collections::BTreeMap::new(),
    };
    let outer =
        index.module_id(RustModulePath::new(vec!["outer".to_owned()]).expect("outer module path"));
    let inner = index.module_id(
        RustModulePath::new(vec!["outer".to_owned(), "inner".to_owned()])
            .expect("inner module path"),
    );
    let sources = crate::languages::rust::source::RustSourceCatalog::deferred(
        &CompilerArtifactFixture::allowed_files(),
        CompilerArtifactFixture::sources(),
        &cancellation,
    )
    .expect("deferred compiler sources");
    let mut usage = limits.usage();
    let mut inventory = CompilerInventory::new(sources, &limits, &mut usage, &cancellation);
    let lowerer = super::super::declarations::RustdocDeclarationLowerer {
        index: &index,
        inventory: &mut inventory,
    };

    for (parent, path, current, expected) in [
        (root_parent, "super", &outer, Visibility::Crate),
        (
            outer_parent,
            "super",
            &inner,
            Visibility::Module(outer.clone()),
        ),
        (inner_parent, "self", &inner, Visibility::Private),
    ] {
        assert_eq!(
            lowerer
                .visibility(
                    &rustdoc_types::Visibility::Restricted {
                        parent,
                        path: path.to_owned(),
                    },
                    current,
                )
                .expect("valid restricted visibility"),
            expected,
        );
    }
}

#[test]
fn compiler_restricted_visibility_rejects_invalid_parent_summaries() {
    let cases = [
        (
            rustdoc_types::ItemSummary {
                crate_id: 0,
                path: Vec::new(),
                kind: rustdoc_types::ItemKind::Module,
            },
            "self",
            "parent path is empty",
        ),
        (
            rustdoc_types::ItemSummary {
                crate_id: 0,
                path: vec!["sample".to_owned()],
                kind: rustdoc_types::ItemKind::Module,
            },
            "",
            "visibility path is empty",
        ),
        (
            rustdoc_types::ItemSummary {
                crate_id: 1,
                path: vec!["external".to_owned()],
                kind: rustdoc_types::ItemKind::Module,
            },
            "external",
            "external crate",
        ),
        (
            rustdoc_types::ItemSummary {
                crate_id: 0,
                path: vec!["sample".to_owned()],
                kind: rustdoc_types::ItemKind::Function,
            },
            "self",
            "identify a module",
        ),
        (
            rustdoc_types::ItemSummary {
                crate_id: 0,
                path: vec!["sample".to_owned(), "sibling".to_owned()],
                kind: rustdoc_types::ItemKind::Module,
            },
            "crate::sibling",
            "not an ancestor",
        ),
        (
            rustdoc_types::ItemSummary {
                crate_id: 0,
                path: vec!["sample".to_owned(), "sibling".to_owned()],
                kind: rustdoc_types::ItemKind::Module,
            },
            "crate",
            "does not resolve to the crate root",
        ),
    ];

    for (summary, path, expected_message) in cases {
        let parent = rustdoc_types::Id(30);
        let mut document = CompilerArtifactFixture::supported_surface_document();
        document.paths.insert(parent, summary);
        document
            .index
            .get_mut(&rustdoc_types::Id(3))
            .expect("record field")
            .visibility = rustdoc_types::Visibility::Restricted {
            parent,
            path: path.to_owned(),
        };

        let error = CompilerArtifactFixture::document_error(document);
        assert!(matches!(
            error.compiler_artifact_failure(),
            Some(RustCompilerArtifactFailure::InvalidItem { item_id: 30, message })
                if message.contains(expected_message)
        ));
    }

    let parent = rustdoc_types::Id(30);
    let mut document = CompilerArtifactFixture::supported_surface_document();
    document
        .index
        .get_mut(&rustdoc_types::Id(3))
        .expect("record field")
        .visibility = rustdoc_types::Visibility::Restricted {
        parent,
        path: "self".to_owned(),
    };
    let error = CompilerArtifactFixture::document_error(document);
    assert!(matches!(
        error.compiler_artifact_failure(),
        Some(RustCompilerArtifactFailure::InvalidItem { item_id: 30, message })
            if message.contains("no path summary")
    ));
}

#[test]
fn compiler_implementation_order_and_block_partition_are_nonsemantic() {
    let mut partitioned = CompilerArtifactFixture::supported_surface_document();
    let split_id = rustdoc_types::Id(14);
    let mut split = partitioned
        .index
        .get(&rustdoc_types::Id(11))
        .expect("inherent implementation")
        .clone();
    split.id = split_id;
    let rustdoc_types::ItemEnum::Impl(split_body) = &mut split.inner else {
        panic!("split fixture must remain an implementation");
    };
    split_body.items = vec![rustdoc_types::Id(13)];
    let rustdoc_types::ItemEnum::Impl(original_body) = &mut partitioned
        .index
        .get_mut(&rustdoc_types::Id(11))
        .expect("inherent implementation")
        .inner
    else {
        panic!("original fixture must remain an implementation");
    };
    original_body.items = vec![rustdoc_types::Id(12)];
    let rustdoc_types::ItemEnum::Struct(record) = &mut partitioned
        .index
        .get_mut(&rustdoc_types::Id(2))
        .expect("record item")
        .inner
    else {
        panic!("record fixture must remain a struct");
    };
    record.impls = vec![rustdoc_types::Id(15), split_id, rustdoc_types::Id(11)];
    partitioned.index.insert(split_id, split);
    let partitioned_with_limit = partitioned.clone();

    let extraction = CompilerArtifactFixture::extract(partitioned)
        .expect("split implementation blocks must merge");
    let inherent = extraction
        .projection()
        .entries()
        .iter()
        .filter_map(|entry| match entry.declaration() {
            RustDeclaration::Implementation(implementation)
                if matches!(
                    implementation.implemented_trait(),
                    RustImplementedTrait::Inherent
                ) =>
            {
                Some(implementation)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(inherent.len(), 1);
    assert_eq!(inherent[0].items().len(), 2);
    for entry in extraction.projection().entries() {
        if let RustDeclaration::Implementation(implementation) = entry.declaration() {
            assert_eq!(
                entry.id().name(),
                implementation.owner().id().render(),
                "compiler implementation IDs must use the same full canonical owner identity as syntax extraction",
            );
        }
    }

    let original =
        CompilerArtifactFixture::extract(CompilerArtifactFixture::supported_surface_document())
            .expect("original implementation partition");
    assert_eq!(
        extraction.projection().entries(),
        original.projection().entries()
    );

    let logical_item_count = original
        .projection()
        .entries()
        .iter()
        .map(|entry| entry.declaration().item_count())
        .sum::<usize>();
    let limits = RustExtractionLimits {
        items: u64::try_from(logical_item_count).expect("logical item count"),
        ..RustExtractionLimits::default()
    };
    let mut usage = limits.usage();
    CompilerArtifactFixture::artifact_for(partitioned_with_limit)
        .extract(
            CompilerArtifactFixture::sources(),
            &CompilerArtifactFixture::allowed_files(),
            &limits,
            &mut usage,
            &CancellationProbe::new(),
        )
        .expect("split implementation partitions must consume the merged logical item budget");
}

#[test]
fn compiler_implementation_occurrences_do_not_depend_on_owner_visit_order() {
    let original_document = CompilerArtifactFixture::supported_surface_document();
    let mut reordered_document = original_document.clone();
    CompilerArtifactFixture::root_items(&mut reordered_document).reverse();
    let rustdoc_types::ItemEnum::Struct(record) = &mut reordered_document
        .index
        .get_mut(&rustdoc_types::Id(2))
        .expect("record item")
        .inner
    else {
        panic!("record fixture must remain a struct");
    };
    record.impls.reverse();

    let original =
        CompilerArtifactFixture::extract(original_document).expect("original compiler ordering");
    let reordered =
        CompilerArtifactFixture::extract(reordered_document).expect("reordered compiler artifact");
    assert_eq!(
        original.projection().entries(),
        reordered.projection().entries()
    );
}

#[test]
fn compiler_trait_impl_members_normalize_to_the_nominal_owner_context() {
    let extraction =
        CompilerArtifactFixture::extract_artifact_from(
            CompilerArtifactFixture::artifact_for(
                CompilerArtifactFixture::supported_surface_document(),
            ),
            CompilerArtifactFixture::supported_surface_source(),
        )
        .expect("compiler surface extraction");
    let record = extraction
        .projection()
        .entries()
        .iter()
        .find_map(|entry| match entry.declaration() {
            RustDeclaration::Structure(record) if record.base().name() == "Record" => {
                Some((entry.id(), record.base()))
            }
            _ => None,
        })
        .expect("record owner declaration");
    let implementation = extraction
        .projection()
        .entries()
        .iter()
        .find_map(|entry| match entry.declaration() {
            RustDeclaration::Implementation(implementation)
                if matches!(
                    implementation.implemented_trait(),
                    RustImplementedTrait::Trait { .. }
                ) =>
            {
                Some(implementation)
            }
            _ => None,
        })
        .expect("trait implementation");

    assert_eq!(implementation.owner().id(), record.0);
    assert_eq!(implementation.items().len(), 3);
    for item in implementation.items() {
        match item {
            RustAssociatedItem::Method(method) => {
                assert_eq!(method.function().base().file_path(), record.1.file_path());
                assert_eq!(method.function().base().module_id(), record.1.module_id());
                assert_eq!(method.visibility(), &Visibility::Public);
            }
            RustAssociatedItem::Constant(constant) => {
                assert_eq!(constant.visibility(), &Visibility::Public);
            }
            RustAssociatedItem::Type(associated_type) => {
                assert_eq!(associated_type.visibility(), &Visibility::Public);
            }
        }
    }
}

#[test]
fn compiler_blanket_and_external_owner_impls_remain_explicitly_fail_closed() {
    let mut blanket = CompilerArtifactFixture::supported_surface_document();
    let rustdoc_types::ItemEnum::Impl(blanket_impl) = &mut blanket
        .index
        .get_mut(&rustdoc_types::Id(15))
        .expect("trait implementation")
        .inner
    else {
        panic!("fixture item must remain a trait implementation");
    };
    blanket_impl.blanket_impl = Some(rustdoc_types::Type::Generic("T".to_owned()));
    let blanket_error = CompilerArtifactFixture::document_error(blanket);
    assert!(matches!(
        blanket_error.compiler_artifact_failure(),
        Some(RustCompilerArtifactFailure::UnsupportedItem { reason, .. })
            if reason.contains("blanket implementations")
    ));

    let mut external = CompilerArtifactFixture::supported_surface_document();
    let external_owner_id = rustdoc_types::Id(30);
    external.paths.insert(
        external_owner_id,
        rustdoc_types::ItemSummary {
            crate_id: 1,
            path: vec!["external".to_owned(), "Widget".to_owned()],
            kind: rustdoc_types::ItemKind::Struct,
        },
    );
    let mut external_owner = CompilerArtifactFixture::public_item(
        external_owner_id.0,
        Some("Widget"),
        rustdoc_types::ItemEnum::Struct(rustdoc_types::Struct {
            kind: rustdoc_types::StructKind::Unit,
            generics: rustdoc_types::Generics {
                params: Vec::new(),
                where_predicates: Vec::new(),
            },
            impls: Vec::new(),
        }),
    );
    external_owner.crate_id = 1;
    external.index.insert(external_owner_id, external_owner);
    let rustdoc_types::ItemEnum::Impl(external_impl) = &mut external
        .index
        .get_mut(&rustdoc_types::Id(15))
        .expect("trait implementation")
        .inner
    else {
        panic!("fixture item must remain a trait implementation");
    };
    external_impl.for_ = rustdoc_types::Type::ResolvedPath(rustdoc_types::Path {
        path: "external::Widget".to_owned(),
        id: external_owner_id,
        args: None,
    });
    let external_error = CompilerArtifactFixture::document_error(external);
    assert!(
        matches!(
            external_error.compiler_artifact_failure(),
            Some(RustCompilerArtifactFailure::UnsupportedType { reason, .. })
                if reason.contains("external owner types")
        ),
        "{external_error}"
    );
}

#[test]
fn binary_private_inclusive_rustdoc_keeps_only_public_reachable_declarations() {
    let mut artifact = CompilerArtifactFixture::artifact();
    artifact.crates[0].kind = RustCrateKind::Binary;
    let mut document = CompilerArtifactFixture::rustdoc_document();
    document.includes_private = true;
    let private_id = rustdoc_types::Id(2);
    let mut private_function = document
        .index
        .get(&rustdoc_types::Id(1))
        .expect("fixture public function")
        .clone();
    private_function.id = private_id;
    private_function.name = Some("private_answer".to_owned());
    private_function.visibility = rustdoc_types::Visibility::Default;
    let struct_id = rustdoc_types::Id(3);
    let implementation_id = rustdoc_types::Id(4);
    let private_associated_id = rustdoc_types::Id(5);
    let public_associated_id = rustdoc_types::Id(6);
    let private_implementation_id = rustdoc_types::Id(7);
    let mut public_struct = private_function.clone();
    public_struct.id = struct_id;
    public_struct.name = Some("Widget".to_owned());
    public_struct.visibility = rustdoc_types::Visibility::Public;
    public_struct.inner = rustdoc_types::ItemEnum::Struct(rustdoc_types::Struct {
        kind: rustdoc_types::StructKind::Unit,
        generics: rustdoc_types::Generics {
            params: Vec::new(),
            where_predicates: Vec::new(),
        },
        impls: vec![implementation_id, private_implementation_id],
    });
    let mut implementation = private_function.clone();
    implementation.id = implementation_id;
    implementation.name = None;
    implementation.inner = rustdoc_types::ItemEnum::Impl(rustdoc_types::Impl {
        is_unsafe: false,
        generics: rustdoc_types::Generics {
            params: Vec::new(),
            where_predicates: Vec::new(),
        },
        provided_trait_methods: Vec::new(),
        trait_: None,
        for_: rustdoc_types::Type::ResolvedPath(rustdoc_types::Path {
            path: "Widget".to_owned(),
            id: struct_id,
            args: None,
        }),
        items: vec![private_associated_id, public_associated_id],
        is_negative: false,
        is_synthetic: false,
        blanket_impl: None,
    });
    let mut private_implementation = implementation.clone();
    private_implementation.id = private_implementation_id;
    let rustdoc_types::ItemEnum::Impl(private_implementation_body) =
        &mut private_implementation.inner
    else {
        panic!("fixture private implementation must remain an impl");
    };
    private_implementation_body.items = vec![private_associated_id];
    let mut private_associated = private_function.clone();
    private_associated.id = private_associated_id;
    private_associated.name = Some("private_associated".to_owned());
    let mut public_associated = private_function.clone();
    public_associated.id = public_associated_id;
    public_associated.name = Some("public_associated".to_owned());
    public_associated.visibility = rustdoc_types::Visibility::Public;
    let rustdoc_types::ItemEnum::Module(root) = &mut document
        .index
        .get_mut(&rustdoc_types::Id(0))
        .expect("fixture root module")
        .inner
    else {
        panic!("fixture root must remain a module");
    };
    root.items.push(private_id);
    root.items.push(struct_id);
    document.index.insert(private_id, private_function);
    document.index.insert(struct_id, public_struct);
    document.index.insert(implementation_id, implementation);
    document
        .index
        .insert(private_implementation_id, private_implementation);
    document
        .index
        .insert(private_associated_id, private_associated);
    document
        .index
        .insert(public_associated_id, public_associated);
    document.paths.insert(
        struct_id,
        rustdoc_types::ItemSummary {
            crate_id: 0,
            path: vec!["sample".to_owned(), "Widget".to_owned()],
            kind: rustdoc_types::ItemKind::Struct,
        },
    );
    artifact.rustdoc_json = serde_json::to_vec(&document).expect("binary rustdoc fixture JSON");
    for rustdoc_item_id in [
        struct_id.0,
        implementation_id.0,
        public_associated_id.0,
        private_implementation_id.0,
    ] {
        artifact.source_paths.push(CompilerSourcePath {
            rustdoc_item_id,
            provenance: CompilerSourceProvenance::CompilerGenerated {
                crate_root: CatalogPath::new("lib.rs").expect("fixture crate root"),
            },
        });
    }

    let extraction = CompilerArtifactFixture::extract_artifact(artifact)
        .expect("binary rustdoc may include private items");

    assert_eq!(extraction.projection().entries().len(), 3);
    let implementation = extraction
        .projection()
        .entries()
        .iter()
        .find_map(|entry| match entry.declaration() {
            RustDeclaration::Implementation(implementation) => Some(implementation),
            _ => None,
        })
        .expect("public owner implementation");
    assert_eq!(implementation.items().len(), 1);
}

#[test]
fn public_generate_and_check_dispatch_through_compiler_extraction() {
    let kit = SignatureContractKit::builder()
        .build()
        .expect("compiler kit");
    let artifact = CompilerArtifactFixture::artifact();
    let generated = futures_executor::block_on(kit.generate(GenerateRequest {
        extraction: RustExtractionInput::Compiler(artifact.clone()),
        source_files: CompilerArtifactFixture::sources(),
        target: GenerateTarget::New(GenerateDocument {
            contract_file: CatalogPath::new("main.yml").expect("contract path"),
            root: "../src".to_owned(),
            files: vec![CatalogPath::new("lib.rs").expect("source path")],
            crates: vec![RustCrateRoot {
                id: "sample".to_owned(),
                root: CatalogPath::new("lib.rs").expect("crate root"),
                kind: RustCrateKind::Library,
            }],
        }),
        scope: ContractScope::Signatures,
    }))
    .expect("compiler generation");
    let yaml = std::str::from_utf8(
        generated
            .contract_files
            .get(&CatalogPath::new("main.yml").expect("contract path"))
            .expect("generated compiler contract"),
    )
    .expect("UTF-8 compiler contract");
    assert!(yaml.contains("mode: rust_compiler_v1"));
    assert!(yaml.contains("package: sample"));
    assert!(yaml.contains("target: sample"));

    let checked = futures_executor::block_on(kit.check(CheckRequest {
        extraction: RustExtractionInput::Compiler(artifact),
        source_files: CompilerArtifactFixture::sources(),
        contract_files: generated.contract_files,
        report: ReportRequest::None,
        mode: CheckMode::Strict,
    }))
    .expect("compiler check");
    assert!(checked.passed, "{:#?}", checked.diagnostics);
}

#[test]
fn compiler_all_scope_generation_returns_linked_seed_from_one_extraction() {
    let kit = SignatureContractKit::builder()
        .build()
        .expect("compiler kit");
    let response = futures_executor::block_on(kit.generate(GenerateRequest {
        extraction: RustExtractionInput::Compiler(CompilerArtifactFixture::exact_artifact()),
        source_files: CompilerArtifactFixture::sources(),
        target: GenerateTarget::Existing(CompilerArtifactFixture::linked_contracts()),
        scope: ContractScope::All,
    }))
    .expect("compiler all-scope generation");

    assert_eq!(response.resolved_sketch_seeds.len(), 1);
    assert_eq!(
        response.resolved_sketch_seeds[0].sketch_id,
        "answer_example"
    );
    assert_eq!(response.resolved_sketch_seeds[0].code, "make_answer!();");
}
