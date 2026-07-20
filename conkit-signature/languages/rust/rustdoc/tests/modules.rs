use super::*;

#[test]
fn compiler_public_module_declaration_is_not_lost_during_containment_lowering() {
    let source = b"pub mod api {}\n";
    let module_id = rustdoc_types::Id(10);
    let mut artifact = CompilerArtifactFixture::exact_module_artifact(
        source,
        CompilerArtifactFixture::module_item(module_id.0, "api", Vec::new()),
        rustdoc_types::Span {
            filename: "lib.rs".into(),
            begin: (1, 1),
            // Real rustdoc JSON ends an inline module span after its header,
            // before the opening brace and body.
            end: (1, 12),
        },
    );
    CompilerArtifactFixture::set_exact_range(&mut artifact, module_id.0, 0, 12);
    let extraction = CompilerArtifactFixture::extract_artifact_from(artifact, source)
        .expect("exact public module declaration");

    let [entry] = extraction.projection().entries() else {
        panic!("one public module declaration must be emitted");
    };
    let RustDeclaration::Module(module) = entry.declaration() else {
        panic!("containment module must use the common module declaration model");
    };
    assert_eq!(module.base().name(), "api");
    assert!(module.is_inline());
    assert_eq!(module.path_override(), None);
    assert!(entry.id().module_id().module_path().segments().is_empty());
    assert_eq!(
        extraction
            .projected_source()
            .source_text(entry)
            .expect("complete inline module source"),
        "pub mod api {}",
    );
}

#[test]
fn compiler_public_module_without_exact_source_shape_fails_closed() {
    let module_id = rustdoc_types::Id(10);
    let mut document = CompilerArtifactFixture::rustdoc_document();
    document.index.insert(
        module_id,
        CompilerArtifactFixture::module_item(module_id.0, "api", Vec::new()),
    );
    CompilerArtifactFixture::add_path(
        &mut document,
        module_id.0,
        "api",
        rustdoc_types::ItemKind::Module,
    );
    CompilerArtifactFixture::set_root_items(&mut document, vec![module_id]);

    let error = CompilerArtifactFixture::document_error(document);

    assert!(matches!(
        error.compiler_artifact_failure(),
        Some(RustCompilerArtifactFailure::UnsupportedItem {
            item_id: 10,
            reason,
            ..
        }) if reason.contains("exact source")
    ));
}

#[test]
fn compiler_module_exact_span_must_contain_one_module_and_reports_its_item_id() {
    let source = b"pub fn api() {}\n";
    let module = CompilerArtifactFixture::module_item(10, "api", Vec::new());
    let artifact = CompilerArtifactFixture::exact_module_artifact(
        source,
        module,
        rustdoc_types::Span {
            filename: "lib.rs".into(),
            begin: (1, 1),
            end: (1, 15),
        },
    );

    let error = CompilerArtifactFixture::error(
        CompilerArtifactFixture::extract_artifact_from(artifact, source),
        "a non-module exact span must fail closed",
    );

    assert!(matches!(
        error.compiler_artifact_failure(),
        Some(RustCompilerArtifactFailure::UnsupportedItem {
            item_id: 10,
            reason,
            ..
        }) if reason.contains("one complete module declaration")
    ));
}

#[test]
fn compiler_crate_root_is_containment_only_not_a_module_declaration() {
    let mut document = CompilerArtifactFixture::rustdoc_document();
    CompilerArtifactFixture::set_root_items(&mut document, Vec::new());

    let extraction = CompilerArtifactFixture::extract(document)
        .expect("an empty crate root remains valid containment");

    assert!(extraction.projection().entries().is_empty());
}

#[test]
fn compiler_private_and_stripped_modules_remain_outside_the_projection() {
    let private_id = rustdoc_types::Id(10);
    let stripped_id = rustdoc_types::Id(11);
    let mut private = CompilerArtifactFixture::module_item(private_id.0, "private_api", Vec::new());
    private.visibility = rustdoc_types::Visibility::Default;
    let mut stripped =
        CompilerArtifactFixture::module_item(stripped_id.0, "stripped_api", Vec::new());
    let rustdoc_types::ItemEnum::Module(stripped_module) = &mut stripped.inner else {
        panic!("fixture stripped item must be a module");
    };
    stripped_module.is_stripped = true;

    let mut document = CompilerArtifactFixture::rustdoc_document();
    document.index.insert(private_id, private);
    document.index.insert(stripped_id, stripped);
    for (id, name) in [
        (private_id.0, "private_api"),
        (stripped_id.0, "stripped_api"),
    ] {
        CompilerArtifactFixture::add_path(&mut document, id, name, rustdoc_types::ItemKind::Module);
    }
    CompilerArtifactFixture::set_root_items(&mut document, vec![private_id, stripped_id]);

    let extraction = CompilerArtifactFixture::extract(document)
        .expect("non-public containment modules are excluded before shape lowering");

    assert!(extraction.projection().entries().is_empty());
}

#[test]
fn compiler_module_uses_exact_out_of_line_path_shape_without_a_raw_attribute() {
    let source = br#"#[path = "platform/api.rs"]
pub mod api;
"#;
    let module_id = rustdoc_types::Id(10);
    let mut module = CompilerArtifactFixture::module_item(module_id.0, "api", Vec::new());
    module.attrs = vec![rustdoc_types::Attribute::Other(
        "#[path = \"platform/api.rs\"]".to_owned(),
    )];
    let artifact = CompilerArtifactFixture::exact_module_artifact(
        source,
        module,
        rustdoc_types::Span {
            filename: "lib.rs".into(),
            begin: (1, 1),
            end: (2, 12),
        },
    );

    let extraction = CompilerArtifactFixture::extract_artifact_from(artifact, source)
        .expect("exact path-directed module declaration");
    let [entry] = extraction.projection().entries() else {
        panic!("one module declaration expected");
    };
    let RustDeclaration::Module(module) = entry.declaration() else {
        panic!("module declaration expected");
    };

    assert!(!module.is_inline());
    assert_eq!(module.path_override(), Some("platform/api.rs"));
    assert!(module.attributes().values().is_empty());
}

#[test]
fn compiler_module_exact_source_name_must_match_rustdoc_identity() {
    let source = b"pub mod source {}\n";
    let module = CompilerArtifactFixture::module_item(10, "api", Vec::new());
    let artifact = CompilerArtifactFixture::exact_module_artifact(
        source,
        module,
        rustdoc_types::Span {
            filename: "lib.rs".into(),
            begin: (1, 1),
            end: (1, 17),
        },
    );

    let error = CompilerArtifactFixture::error(
        CompilerArtifactFixture::extract_artifact_from(artifact, source),
        "contradictory source and rustdoc module names must fail",
    );
    assert!(matches!(
        error.compiler_artifact_failure(),
        Some(RustCompilerArtifactFailure::InvalidItem {
            item_id: 10,
            message,
        }) if message.contains("source") && message.contains("rustdoc")
    ));
}

#[test]
fn compiler_nested_module_declarations_keep_their_declaring_module_identity() {
    let source = b"pub mod outer { pub mod inner {} }\n";
    let outer_id = rustdoc_types::Id(10);
    let inner_id = rustdoc_types::Id(11);
    let mut outer = CompilerArtifactFixture::module_item(outer_id.0, "outer", vec![inner_id]);
    outer.span = Some(rustdoc_types::Span {
        filename: "lib.rs".into(),
        begin: (1, 1),
        end: (1, 34),
    });
    let mut inner = CompilerArtifactFixture::module_item(inner_id.0, "inner", Vec::new());
    inner.span = Some(rustdoc_types::Span {
        filename: "lib.rs".into(),
        begin: (1, 17),
        end: (1, 32),
    });
    let mut document = CompilerArtifactFixture::rustdoc_document();
    document.index.insert(outer_id, outer);
    document.index.insert(inner_id, inner);
    document.paths.insert(
        outer_id,
        rustdoc_types::ItemSummary {
            crate_id: 0,
            path: vec!["sample".to_owned(), "outer".to_owned()],
            kind: rustdoc_types::ItemKind::Module,
        },
    );
    document.paths.insert(
        inner_id,
        rustdoc_types::ItemSummary {
            crate_id: 0,
            path: vec!["sample".to_owned(), "outer".to_owned(), "inner".to_owned()],
            kind: rustdoc_types::ItemKind::Module,
        },
    );
    CompilerArtifactFixture::set_root_items(&mut document, vec![outer_id]);
    let mut artifact = CompilerArtifactFixture::artifact_for(document);
    for (item_id, byte_start, byte_end) in
        [(outer_id.0, 0_u64, 34_u64), (inner_id.0, 16_u64, 32_u64)]
    {
        CompilerArtifactFixture::set_exact_range(&mut artifact, item_id, byte_start, byte_end);
    }

    let extraction = CompilerArtifactFixture::extract_artifact_from(artifact, source)
        .expect("nested exact module declarations");
    let entries = extraction.projection().entries();
    assert_eq!(entries.len(), 2);
    let outer = entries
        .iter()
        .find(|entry| entry.id().name() == "outer")
        .expect("outer declaration");
    let inner = entries
        .iter()
        .find(|entry| entry.id().name() == "inner")
        .expect("inner declaration");

    assert!(outer.id().module_id().module_path().segments().is_empty());
    assert_eq!(inner.id().module_id().module_path().segments(), ["outer"]);
}

#[test]
fn syntax_and_compiler_generation_agree_on_public_module_shape() {
    let source = b"pub mod api {}\n";
    let module = CompilerArtifactFixture::module_item(10, "api", Vec::new());
    let artifact = CompilerArtifactFixture::exact_module_artifact(
        source,
        module,
        rustdoc_types::Span {
            filename: "lib.rs".into(),
            begin: (1, 1),
            end: (1, 14),
        },
    );
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
        source_files: CompilerArtifactFixture::sources_from(source),
        target: GenerateTarget::New(target.clone()),
        scope: ContractScope::Signatures,
    }))
    .expect("syntax module generation");
    let compiler = futures_executor::block_on(kit.generate(GenerateRequest {
        extraction: RustExtractionInput::Compiler(artifact),
        source_files: CompilerArtifactFixture::sources_from(source),
        target: GenerateTarget::New(target),
        scope: ContractScope::Signatures,
    }))
    .expect("compiler module generation");
    let contract_path = CatalogPath::new("main.yml").expect("contract path");

    assert_eq!(syntax.counts.signature_count, 1);
    assert_eq!(compiler.counts.signature_count, 1);
    assert_eq!(
        CompilerArtifactFixture::signature_section(
            syntax
                .contract_files
                .get(&contract_path)
                .expect("syntax contract"),
        ),
        CompilerArtifactFixture::signature_section(
            compiler
                .contract_files
                .get(&contract_path)
                .expect("compiler contract"),
        )
    );
}

#[test]
fn compiler_module_containment_rejects_self_and_multi_module_cycles() {
    for node_count in [1, 2, 3] {
        let error = CompilerArtifactFixture::document_error(CompilerArtifactFixture::module_cycle(
            node_count,
        ));
        assert!(matches!(
            error.compiler_artifact_failure(),
            Some(RustCompilerArtifactFailure::InvalidItem { item_id: 10, message })
                if message.contains("cycle")
        ));
    }
}

#[test]
fn compiler_module_children_remain_depth_first_and_precede_parent_exports() {
    let parent_id = rustdoc_types::Id(10);
    let child_id = rustdoc_types::Id(11);
    let invalid_export_id = rustdoc_types::Id(20);
    let nested_failure_id = rustdoc_types::Id(30);
    let root_failure_id = rustdoc_types::Id(31);
    let mut document = CompilerArtifactFixture::rustdoc_document();
    document.index.insert(
        parent_id,
        CompilerArtifactFixture::module_item(
            parent_id.0,
            "parent",
            vec![invalid_export_id, child_id],
        ),
    );
    document.index.insert(
        child_id,
        CompilerArtifactFixture::module_item(child_id.0, "child", vec![nested_failure_id]),
    );
    document.index.insert(
        invalid_export_id,
        CompilerArtifactFixture::public_item(
            invalid_export_id.0,
            None,
            rustdoc_types::ItemEnum::Use(rustdoc_types::Use {
                source: "sample::missing".to_owned(),
                name: "missing".to_owned(),
                id: None,
                is_glob: true,
            }),
        ),
    );
    for (id, name) in [
        (nested_failure_id, "nested_failure"),
        (root_failure_id, "root_failure"),
    ] {
        document.index.insert(
            id,
            CompilerArtifactFixture::public_item(
                id.0,
                Some(name),
                rustdoc_types::ItemEnum::Macro(format!("macro_rules! {name}")),
            ),
        );
    }
    CompilerArtifactFixture::set_root_items(&mut document, vec![parent_id, root_failure_id]);

    let error = CompilerArtifactFixture::document_error(document);

    assert!(matches!(
        error.compiler_artifact_failure(),
        Some(RustCompilerArtifactFailure::UnsupportedItem { item_id: 30, .. })
    ));
}

#[test]
fn compiler_module_containment_observes_cancellation_at_the_iterative_boundary() {
    let document = CompilerArtifactFixture::rustdoc_document();
    let mut artifact = CompilerArtifactFixture::artifact_for(document.clone());
    let preparation = CancellationProbe::new();
    let limits = RustExtractionLimits::default();
    let context = artifact
        .validate_metadata(&preparation)
        .expect("fixture metadata");
    artifact
        .validate_document(&context, &document, &limits, &preparation)
        .expect("fixture rustdoc document");
    let source_files = CompilerArtifactFixture::sources();
    let source_map = artifact
        .validate_source_map(
            &context,
            &document,
            &CompilerArtifactFixture::allowed_files(),
            &source_files,
            &limits,
            &preparation,
        )
        .expect("fixture source map");
    let sources = artifact
        .parse_sources(&context, &source_map, source_files, &limits, &preparation)
        .expect("fixture source catalog");
    let cancellation = CancellationProbe::new();
    cancellation.cancel();
    let mut usage = limits.usage();

    let index = RustdocIndex {
        context,
        document,
        source_map,
    };
    let error = CompilerArtifactFixture::error(
        CompilerInventory::new(sources, &limits, &mut usage, &cancellation).extract(index),
        "the iterative containment boundary must observe cancellation",
    );

    assert!(error.is_operation_canceled());
}

#[test]
fn compiler_module_containment_rejects_a_completed_module_with_two_parents() {
    let left_id = rustdoc_types::Id(10);
    let right_id = rustdoc_types::Id(11);
    let shared_id = rustdoc_types::Id(12);
    let mut document = CompilerArtifactFixture::rustdoc_document();
    document.index.insert(
        left_id,
        CompilerArtifactFixture::module_item(left_id.0, "left", vec![shared_id]),
    );
    document.index.insert(
        right_id,
        CompilerArtifactFixture::module_item(right_id.0, "right", vec![shared_id]),
    );
    document.index.insert(
        shared_id,
        CompilerArtifactFixture::module_item(shared_id.0, "shared", vec![rustdoc_types::Id(1)]),
    );
    CompilerArtifactFixture::add_path(
        &mut document,
        shared_id.0,
        "shared",
        rustdoc_types::ItemKind::Module,
    );
    CompilerArtifactFixture::set_root_items(&mut document, vec![left_id, right_id]);

    let error = CompilerArtifactFixture::document_error(document);
    assert!(matches!(
        error.compiler_artifact_failure(),
        Some(RustCompilerArtifactFailure::InvalidItem { item_id: 12, message })
            if message.contains("multiple") && message.contains("parent")
    ));
}

#[test]
fn compiler_module_containment_reaches_fail_closed_lowering_without_call_stack_growth() {
    const DEPTH: u32 = 1_024;
    let mut document = CompilerArtifactFixture::rustdoc_document();
    for offset in 0..DEPTH {
        let id = rustdoc_types::Id(10 + offset);
        let child = if offset + 1 == DEPTH {
            rustdoc_types::Id(1)
        } else {
            rustdoc_types::Id(id.0 + 1)
        };
        document.index.insert(
            id,
            CompilerArtifactFixture::module_item(id.0, &format!("m{offset}"), vec![child]),
        );
        CompilerArtifactFixture::add_path(
            &mut document,
            id.0,
            &format!("m{offset}"),
            rustdoc_types::ItemKind::Module,
        );
    }
    CompilerArtifactFixture::set_root_items(&mut document, vec![rustdoc_types::Id(10)]);

    let error = CompilerArtifactFixture::document_error(document);
    assert!(matches!(
        error.compiler_artifact_failure(),
        Some(RustCompilerArtifactFailure::UnsupportedItem {
            item_id: 10,
            reason,
            ..
        }) if reason.contains("exact source")
    ));
}

#[test]
fn direct_public_reexport_uses_canonical_target_and_use_site_attributes() {
    let mut document = CompilerArtifactFixture::rustdoc_document();
    let use_id = rustdoc_types::Id(10);
    let mut reexport = CompilerArtifactFixture::public_item(
        use_id.0,
        Some("renamed_answer"),
        rustdoc_types::ItemEnum::Use(rustdoc_types::Use {
            source: "sample::hidden::answer".to_owned(),
            name: "renamed_answer".to_owned(),
            id: Some(rustdoc_types::Id(1)),
            is_glob: false,
        }),
    );
    reexport.attrs.push(rustdoc_types::Attribute::MustUse {
        reason: Some("use the exported answer".to_owned()),
    });
    document.index.insert(use_id, reexport);
    document.paths.insert(
        rustdoc_types::Id(1),
        rustdoc_types::ItemSummary {
            crate_id: 0,
            path: vec![
                "sample".to_owned(),
                "hidden".to_owned(),
                "answer".to_owned(),
            ],
            kind: rustdoc_types::ItemKind::Function,
        },
    );
    CompilerArtifactFixture::set_root_items(&mut document, vec![use_id]);

    let extraction = CompilerArtifactFixture::extract(document).expect("direct compiler reexport");
    let [entry] = extraction.projection().entries() else {
        panic!("one direct reexport expected");
    };
    let RustDeclaration::Reexport(reexport) = entry.declaration() else {
        panic!("compiler use must remain a reexport declaration");
    };
    assert_eq!(reexport.base().name(), "renamed_answer");
    assert_eq!(reexport.path(), "sample::hidden::answer");
    assert_eq!(reexport.alias(), Some("renamed_answer"));
    assert!(!reexport.base().attributes().values().is_empty());
}

#[test]
fn explicit_reexport_overrides_a_sibling_glob_at_the_module_boundary() {
    let mut document = CompilerArtifactFixture::rustdoc_document();
    let glob_use = rustdoc_types::Id(10);
    let explicit_use = rustdoc_types::Id(11);
    let glob_module = rustdoc_types::Id(20);
    let glob_target = rustdoc_types::Id(30);
    let explicit_target = rustdoc_types::Id(31);
    document.index.insert(
        glob_use,
        CompilerArtifactFixture::public_item(
            glob_use.0,
            None,
            rustdoc_types::ItemEnum::Use(rustdoc_types::Use {
                source: "sample::globbed".to_owned(),
                name: "globbed".to_owned(),
                id: Some(glob_module),
                is_glob: true,
            }),
        ),
    );
    let mut explicit = CompilerArtifactFixture::public_item(
        explicit_use.0,
        Some("shared"),
        rustdoc_types::ItemEnum::Use(rustdoc_types::Use {
            source: "sample::explicit::shared".to_owned(),
            name: "shared".to_owned(),
            id: Some(explicit_target),
            is_glob: false,
        }),
    );
    explicit.attrs.push(rustdoc_types::Attribute::MustUse {
        reason: Some("explicit export wins".to_owned()),
    });
    document.index.insert(explicit_use, explicit);
    document.index.insert(
        glob_module,
        CompilerArtifactFixture::public_item(
            glob_module.0,
            Some("globbed"),
            rustdoc_types::ItemEnum::Module(rustdoc_types::Module {
                is_crate: false,
                items: vec![glob_target],
                is_stripped: true,
            }),
        ),
    );
    let template = document
        .index
        .get(&rustdoc_types::Id(1))
        .expect("fixture function")
        .clone();
    for (id, path) in [
        (glob_target, vec!["sample", "globbed", "shared"]),
        (explicit_target, vec!["sample", "explicit", "shared"]),
    ] {
        let mut function = template.clone();
        function.id = id;
        function.name = Some("shared".to_owned());
        document.index.insert(id, function);
        document.paths.insert(
            id,
            rustdoc_types::ItemSummary {
                crate_id: 0,
                path: path.into_iter().map(ToOwned::to_owned).collect(),
                kind: rustdoc_types::ItemKind::Function,
            },
        );
    }
    CompilerArtifactFixture::set_root_items(&mut document, vec![glob_use, explicit_use]);

    let extraction = CompilerArtifactFixture::extract(document)
        .expect("explicit module export must shadow sibling glob");
    let [entry] = extraction.projection().entries() else {
        panic!("one effective module export expected");
    };
    let RustDeclaration::Reexport(reexport) = entry.declaration() else {
        panic!("effective export must remain a reexport");
    };
    assert_eq!(reexport.path(), "sample::explicit::shared");
    assert!(!reexport.base().attributes().values().is_empty());
}

#[test]
fn glob_reexport_fails_only_when_rustdoc_omits_the_target_item_set() {
    let mut document = CompilerArtifactFixture::rustdoc_document();
    let use_id = rustdoc_types::Id(10);
    let absent_module = rustdoc_types::Id(20);
    document.index.insert(
        use_id,
        CompilerArtifactFixture::public_item(
            use_id.0,
            None,
            rustdoc_types::ItemEnum::Use(rustdoc_types::Use {
                source: "external::api".to_owned(),
                name: "api".to_owned(),
                id: Some(absent_module),
                is_glob: true,
            }),
        ),
    );
    document.paths.insert(
        absent_module,
        rustdoc_types::ItemSummary {
            crate_id: 1,
            path: vec!["external".to_owned(), "api".to_owned()],
            kind: rustdoc_types::ItemKind::Module,
        },
    );
    CompilerArtifactFixture::set_root_items(&mut document, vec![use_id]);

    let error = CompilerArtifactFixture::document_error(document);
    assert!(matches!(
        error.compiler_artifact_failure(),
        Some(RustCompilerArtifactFailure::UnsupportedItem { reason, .. })
            if reason.contains("external::api")
                && reason.contains("not its item set")
    ));
}

#[test]
fn glob_reexport_expands_recursive_public_target_set_and_terminates_cycles() {
    let mut document = CompilerArtifactFixture::rustdoc_document();
    let root_use = rustdoc_types::Id(10);
    let module_a = rustdoc_types::Id(20);
    let module_b = rustdoc_types::Id(21);
    let use_b = rustdoc_types::Id(22);
    let use_a = rustdoc_types::Id(23);
    let answer_a = rustdoc_types::Id(30);
    let answer_b = rustdoc_types::Id(31);

    document.index.insert(
        root_use,
        CompilerArtifactFixture::public_item(
            root_use.0,
            None,
            rustdoc_types::ItemEnum::Use(rustdoc_types::Use {
                source: "sample::a".to_owned(),
                name: "a".to_owned(),
                id: Some(module_a),
                is_glob: true,
            }),
        ),
    );
    document.index.insert(
        module_a,
        CompilerArtifactFixture::public_item(
            module_a.0,
            Some("a"),
            rustdoc_types::ItemEnum::Module(rustdoc_types::Module {
                is_crate: false,
                items: vec![answer_a, use_b],
                is_stripped: false,
            }),
        ),
    );
    document.index.insert(
        module_b,
        CompilerArtifactFixture::public_item(
            module_b.0,
            Some("b"),
            rustdoc_types::ItemEnum::Module(rustdoc_types::Module {
                is_crate: false,
                items: vec![answer_b, use_a],
                is_stripped: false,
            }),
        ),
    );
    document.index.insert(
        use_b,
        CompilerArtifactFixture::public_item(
            use_b.0,
            None,
            rustdoc_types::ItemEnum::Use(rustdoc_types::Use {
                source: "sample::b".to_owned(),
                name: "b".to_owned(),
                id: Some(module_b),
                is_glob: true,
            }),
        ),
    );
    document.index.insert(
        use_a,
        CompilerArtifactFixture::public_item(
            use_a.0,
            None,
            rustdoc_types::ItemEnum::Use(rustdoc_types::Use {
                source: "sample::a".to_owned(),
                name: "a".to_owned(),
                id: Some(module_a),
                is_glob: true,
            }),
        ),
    );
    let function = document
        .index
        .get(&rustdoc_types::Id(1))
        .expect("fixture function")
        .clone();
    let mut first = function.clone();
    first.id = answer_a;
    first.name = Some("alpha".to_owned());
    document.index.insert(answer_a, first);
    let mut second = function;
    second.id = answer_b;
    second.name = Some("beta".to_owned());
    document.index.insert(answer_b, second);
    for external_id in [module_a, module_b, use_a, use_b, answer_a, answer_b] {
        document
            .index
            .get_mut(&external_id)
            .expect("exposed external target item")
            .crate_id = 1;
    }
    for (id, path) in [
        (answer_a, vec!["sample", "a", "alpha"]),
        (answer_b, vec!["sample", "b", "beta"]),
    ] {
        document.paths.insert(
            id,
            rustdoc_types::ItemSummary {
                crate_id: 1,
                path: path.into_iter().map(ToOwned::to_owned).collect(),
                kind: rustdoc_types::ItemKind::Function,
            },
        );
    }
    CompilerArtifactFixture::set_root_items(&mut document, vec![root_use]);

    let extraction = CompilerArtifactFixture::extract(document).expect("recursive glob target set");
    let names = extraction
        .projection()
        .entries()
        .iter()
        .map(|entry| entry.id().name())
        .collect::<Vec<_>>();
    assert_eq!(names, ["alpha", "beta"]);
}

#[test]
fn glob_reexport_reports_conflicting_effective_names_deterministically() {
    let mut document = CompilerArtifactFixture::rustdoc_document();
    let root_use = rustdoc_types::Id(10);
    let target = rustdoc_types::Id(20);
    let module_left = rustdoc_types::Id(21);
    let module_right = rustdoc_types::Id(22);
    let use_left = rustdoc_types::Id(23);
    let use_right = rustdoc_types::Id(24);
    let left = rustdoc_types::Id(30);
    let right = rustdoc_types::Id(31);
    let module = |id, name: &str, items| {
        CompilerArtifactFixture::public_item(
            id,
            Some(name),
            rustdoc_types::ItemEnum::Module(rustdoc_types::Module {
                is_crate: false,
                items,
                is_stripped: false,
            }),
        )
    };
    let glob = |id, name: &str, target_id| {
        CompilerArtifactFixture::public_item(
            id,
            None,
            rustdoc_types::ItemEnum::Use(rustdoc_types::Use {
                source: format!("sample::{name}"),
                name: name.to_owned(),
                id: Some(target_id),
                is_glob: true,
            }),
        )
    };
    document
        .index
        .insert(root_use, glob(root_use.0, "target", target));
    document.index.insert(
        target,
        module(target.0, "target", vec![use_left, use_right]),
    );
    document
        .index
        .insert(module_left, module(module_left.0, "left", vec![left]));
    document
        .index
        .insert(module_right, module(module_right.0, "right", vec![right]));
    document
        .index
        .insert(use_left, glob(use_left.0, "left", module_left));
    document
        .index
        .insert(use_right, glob(use_right.0, "right", module_right));
    let template = document
        .index
        .get(&rustdoc_types::Id(1))
        .expect("fixture function")
        .clone();
    for (id, path) in [
        (left, vec!["sample", "left", "shared"]),
        (right, vec!["sample", "right", "shared"]),
    ] {
        let mut function = template.clone();
        function.id = id;
        function.name = Some("shared".to_owned());
        document.index.insert(id, function);
        document.paths.insert(
            id,
            rustdoc_types::ItemSummary {
                crate_id: 0,
                path: path.into_iter().map(ToOwned::to_owned).collect(),
                kind: rustdoc_types::ItemKind::Function,
            },
        );
    }
    CompilerArtifactFixture::set_root_items(&mut document, vec![root_use]);

    let error = CompilerArtifactFixture::document_error(document);
    assert!(matches!(
        error.compiler_artifact_failure(),
        Some(RustCompilerArtifactFailure::InvalidItem { message, .. })
            if message.contains("sample::left::shared")
                && message.contains("sample::right::shared")
    ));
}
