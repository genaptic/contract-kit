use super::*;

#[test]
fn compiler_source_provenance_serde_is_tagged_and_rejects_range_sentinels() {
    let generated = CompilerSourcePath {
        rustdoc_item_id: 7,
        provenance: CompilerSourceProvenance::CompilerGenerated {
            crate_root: CatalogPath::new("lib.rs").expect("crate root"),
        },
    };
    let encoded = serde_json::to_value(&generated).expect("source provenance JSON");
    assert_eq!(encoded["rustdoc_item_id"], 7);
    assert_eq!(encoded["provenance"]["kind"], "compiler_generated");
    assert_eq!(encoded["provenance"]["crate_root"], "lib.rs");
    assert_eq!(
        serde_json::from_value::<CompilerSourcePath>(encoded)
            .expect("generated provenance round trip"),
        generated
    );

    let exact = CompilerSourcePath {
        rustdoc_item_id: 8,
        provenance: CompilerSourceProvenance::Exact {
            file: CatalogPath::new("nested/api.rs").expect("exact source"),
            byte_start: 11,
            byte_end: 29,
        },
    };
    let encoded = serde_json::to_value(&exact).expect("exact provenance JSON");
    assert_eq!(encoded["provenance"]["kind"], "exact");
    assert_eq!(encoded["provenance"]["file"], "nested/api.rs");
    assert_eq!(encoded["provenance"]["byte_start"], 11);
    assert_eq!(encoded["provenance"]["byte_end"], 29);
    assert_eq!(
        serde_json::from_value::<CompilerSourcePath>(encoded).expect("exact provenance round trip"),
        exact
    );

    let old_sentinel_shape = serde_json::json!({
        "rustdoc_item_id": 7,
        "file": "lib.rs",
        "byte_start": 0,
        "byte_end": 0
    });
    assert!(serde_json::from_value::<CompilerSourcePath>(old_sentinel_shape).is_err());
}

#[test]
fn compiler_source_provenance_must_match_local_item_and_selected_root() {
    let mut wrong_root = CompilerArtifactFixture::artifact();
    CompilerArtifactFixture::set_provenance(
        &mut wrong_root,
        1,
        CompilerSourceProvenance::CompilerGenerated {
            crate_root: CatalogPath::new("other.rs").expect("different root"),
        },
    );

    let mut external = CompilerArtifactFixture::artifact();
    let mut document = CompilerArtifactFixture::rustdoc_document();
    let mut external_item = document
        .index
        .get(&rustdoc_types::Id(1))
        .expect("fixture function")
        .clone();
    external_item.id = rustdoc_types::Id(9);
    external_item.crate_id = 1;
    document.index.insert(external_item.id, external_item);
    external.rustdoc_json = serde_json::to_vec(&document).expect("external-item fixture JSON");
    external.source_paths.push(CompilerSourcePath {
        rustdoc_item_id: 9,
        provenance: CompilerSourceProvenance::CompilerGenerated {
            crate_root: CatalogPath::new("lib.rs").expect("crate root"),
        },
    });

    let mut unlisted = CompilerArtifactFixture::artifact();
    CompilerArtifactFixture::set_provenance(
        &mut unlisted,
        1,
        CompilerSourceProvenance::Exact {
            file: CatalogPath::new("unlisted.rs").expect("unlisted source"),
            byte_start: 0,
            byte_end: 1,
        },
    );
    for (artifact, expected_item, expected_message) in [
        (wrong_root, 1, None),
        (external, 9, None),
        (unlisted, 1, Some("unlisted source")),
    ] {
        let error = CompilerArtifactFixture::extraction_error(artifact);
        let Some(RustCompilerArtifactFailure::SourceMap { item_id, message }) =
            error.compiler_artifact_failure()
        else {
            panic!("expected source-map failure, found {error}");
        };
        assert_eq!(*item_id, Some(expected_item));
        if let Some(expected_message) = expected_message {
            assert!(message.contains(expected_message));
        }
    }
}

#[test]
fn exact_provenance_must_match_rustdoc_span_kind_and_coordinates() {
    let mut generated_for_spanned = CompilerArtifactFixture::exact_artifact();
    CompilerArtifactFixture::set_provenance(
        &mut generated_for_spanned,
        1,
        CompilerSourceProvenance::CompilerGenerated {
            crate_root: CatalogPath::new("lib.rs").expect("fixture root"),
        },
    );
    let mut exact_for_spanless = CompilerArtifactFixture::artifact();
    CompilerArtifactFixture::set_exact_range(&mut exact_for_spanless, 1, 0, 15);
    let mut wrong_range = CompilerArtifactFixture::exact_artifact();
    CompilerArtifactFixture::set_exact_range(&mut wrong_range, 1, 1, 15);

    for (artifact, expected_message) in [
        (generated_for_spanned, "contradicts the rustdoc source span"),
        (exact_for_spanless, "requires a rustdoc source span"),
        (wrong_range, "rustdoc records"),
    ] {
        let error = CompilerArtifactFixture::extraction_error(artifact);
        assert!(matches!(
            error.compiler_artifact_failure(),
            Some(RustCompilerArtifactFailure::SourceMap { item_id: Some(1), message })
                if message.contains(expected_message)
        ));
    }

    CompilerArtifactFixture::extract_artifact(CompilerArtifactFixture::exact_artifact())
        .expect("matching exact coordinates remain valid");
}

#[test]
fn exact_provenance_uses_unicode_scalar_columns_and_inclusive_multiline_end() {
    let path = CatalogPath::new("lib.rs").expect("source path");
    let mut sources = FileCatalog::new();
    sources
        .insert(path.clone(), "é\nx\n".as_bytes().to_vec())
        .expect("Unicode source");
    let cancellation = CancellationProbe::new();
    let limits = RustExtractionLimits::default();
    let mappings = vec![
        CompilerSourcePath {
            rustdoc_item_id: 1,
            provenance: CompilerSourceProvenance::Exact {
                file: path.clone(),
                byte_start: 0,
                byte_end: 2,
            },
        },
        CompilerSourcePath {
            rustdoc_item_id: 2,
            provenance: CompilerSourceProvenance::Exact {
                file: path.clone(),
                byte_start: 0,
                byte_end: 4,
            },
        },
    ];
    let mut source_index = CompilerSourceIndex::new(&sources, &mappings, &limits, &cancellation)
        .expect("source endpoint requests");
    for (item_id, byte_end, filename, inclusive_end, expected_error) in [
        (
            0,
            2,
            "other.rs",
            (1, 1),
            Some("contradicts rustdoc span file"),
        ),
        (1, 2, "lib.rs", (1, 1), None),
        (2, 4, "lib.rs", (2, 1), None),
    ] {
        let result = CompilerSourceProvenance::Exact {
            file: path.clone(),
            byte_start: 0,
            byte_end,
        }
        .validate_item_span(
            item_id,
            Some(&rustdoc_types::Span {
                filename: filename.into(),
                begin: (1, 1),
                end: inclusive_end,
            }),
            &mut source_index,
            &cancellation,
        );
        if let Some(expected_error) = expected_error {
            let error = CompilerArtifactFixture::error(
                result,
                "logical and rustdoc source files must agree",
            );
            assert!(matches!(
                error.compiler_artifact_failure(),
                Some(RustCompilerArtifactFailure::SourceMap { message, .. })
                    if message.contains(expected_error)
            ));
        } else {
            result.expect("exact scalar coordinates must agree with rustdoc");
        }
    }
}

#[test]
fn compiler_source_endpoint_resolution_handles_crlf_eof_and_invalid_boundaries() {
    let path = CatalogPath::new("lib.rs").expect("source path");
    let cancellation = CancellationProbe::new();
    let limits = RustExtractionLimits::default();
    let source = "a\r\né";
    let index = CompilerSourceFileIndex::new(
        &path,
        7,
        source.as_bytes(),
        vec![5, 3, 2, 1, 0, 3, 5],
        &limits,
        &cancellation,
    )
    .expect("endpoint index");

    assert_eq!(index.endpoints.len(), 5);
    assert_eq!(index.span_coordinates(0, 2), Some(((1, 1), (1, 2))));
    assert_eq!(index.span_coordinates(2, 5), Some(((1, 3), (2, 1))));
    assert_eq!(index.span_coordinates(3, 5), Some(((2, 1), (2, 1))));

    let invalid = CompilerSourceFileIndex::new(
        &path,
        8,
        "é".as_bytes(),
        vec![0, 1, 2],
        &limits,
        &cancellation,
    )
    .expect("invalid boundary remains unresolved");
    assert_eq!(invalid.span_coordinates(0, 2), Some(((1, 1), (1, 1))));
    assert_eq!(invalid.span_coordinates(1, 2), None);

    let invalid_ranges = [(20, 0, 0), (21, 2, 1), (22, 0, 3), (23, 1, 2)];
    let mappings = invalid_ranges
        .iter()
        .map(|&(item_id, byte_start, byte_end)| CompilerSourcePath {
            rustdoc_item_id: item_id,
            provenance: CompilerSourceProvenance::Exact {
                file: path.clone(),
                byte_start,
                byte_end,
            },
        })
        .collect::<Vec<_>>();
    let mut sources = FileCatalog::new();
    sources
        .insert(path.clone(), "é".as_bytes().to_vec())
        .expect("Unicode source");
    let mut source_index = CompilerSourceIndex::new(&sources, &mappings, &limits, &cancellation)
        .expect("source endpoint requests");
    for mapping in &mappings {
        let error = mapping
            .provenance
            .validate_item_span(
                mapping.rustdoc_item_id,
                Some(&rustdoc_types::Span {
                    filename: "lib.rs".into(),
                    begin: (1, 1),
                    end: (1, 1),
                }),
                &mut source_index,
                &cancellation,
            )
            .expect_err("invalid range must fail closed");
        assert!(matches!(
            error.compiler_artifact_failure(),
            Some(RustCompilerArtifactFailure::SourceMap { item_id: Some(actual), message })
                if *actual == mapping.rustdoc_item_id
                    && message.contains("nonempty UTF-8-aligned")
        ));
    }

    let cancelled = CancellationProbe::new();
    cancelled.cancel();
    assert!(matches!(
        CompilerSourceIndex::new(&sources, &mappings, &limits, &cancelled),
        Err(error) if error.is_operation_canceled()
    ));
    let error = CompilerSourceFileIndex::new(&path, 24, b"source", vec![0, 6], &limits, &cancelled)
        .expect_err("endpoint resolution must observe cancellation");
    assert!(error.is_operation_canceled());
}

#[test]
fn compiler_source_index_enforces_file_limits_before_utf8_scanning() {
    let path = CatalogPath::new("private.rs").expect("source path");
    let cancellation = CancellationProbe::new();
    let exceeded_limits = RustExtractionLimits {
        per_file_bytes: 0,
        ..RustExtractionLimits::default()
    };
    let error = CompilerSourceFileIndex::new(
        &path,
        30,
        &[0xff],
        vec![0, 1],
        &exceeded_limits,
        &cancellation,
    )
    .expect_err("the byte limit must precede UTF-8 validation");
    let exceeded = error.limit_exceeded().expect("typed source-file limit");
    assert_eq!(
        exceeded.resource,
        crate::limits::LimitResource::RustSourceFileBytes
    );
    assert_eq!(exceeded.limit, 0);
    assert_eq!(exceeded.observed_at_least, 1);
    assert_eq!(exceeded.file.as_ref(), Some(&path));

    let cancelled = CancellationProbe::new();
    cancelled.cancel();
    let error =
        CompilerSourceFileIndex::new(&path, 31, b"x", vec![0, 1], &exceeded_limits, &cancelled)
            .expect_err("cancellation must precede the source-file limit");
    assert!(error.is_operation_canceled());

    let exact_limits = RustExtractionLimits {
        per_file_bytes: 1,
        ..RustExtractionLimits::default()
    };
    let index =
        CompilerSourceFileIndex::new(&path, 32, b"x", vec![0, 1], &exact_limits, &cancellation)
            .expect("the exact byte boundary must remain admitted");
    assert_eq!(index.span_coordinates(0, 1), Some(((1, 1), (1, 1))));
}

#[test]
fn compiler_source_index_storage_scales_with_unique_endpoints_not_source_scalars() {
    let path = CatalogPath::new("lib.rs").expect("source path");
    let cancellation = CancellationProbe::new();
    let limits = RustExtractionLimits::default();
    let source = vec![b'x'; 64 * 1024];
    let index = CompilerSourceFileIndex::new(
        &path,
        9,
        &source,
        vec![source.len() as u64, 10, 0, 10, source.len() as u64],
        &limits,
        &cancellation,
    )
    .expect("endpoint index");

    assert_eq!(index.endpoints.len(), 3);
    assert!(index.endpoints.len() < source.len() / 1_000);

    let mut invalid_sources = FileCatalog::new();
    invalid_sources
        .insert(path.clone(), vec![0xff])
        .expect("invalid UTF-8 fixture");
    let mappings = [CompilerSourcePath {
        rustdoc_item_id: 30,
        provenance: CompilerSourceProvenance::Exact {
            file: path.clone(),
            byte_start: 0,
            byte_end: 1,
        },
    }];
    let mut invalid_index =
        CompilerSourceIndex::new(&invalid_sources, &mappings, &limits, &cancellation)
            .expect("invalid source endpoint requests");
    let error = invalid_index
        .file(&path, 30, &cancellation)
        .expect_err("first mapping must own the file UTF-8 error");
    assert!(matches!(
        error.compiler_artifact_failure(),
        Some(RustCompilerArtifactFailure::SourceMap {
            item_id: Some(30),
            ..
        })
    ));
}

#[test]
fn reachable_public_item_without_allowlisted_provenance_fails_closed() {
    let mut artifact = CompilerArtifactFixture::artifact();
    let mut document = CompilerArtifactFixture::rustdoc_document();
    document
        .index
        .get_mut(&rustdoc_types::Id(1))
        .expect("fixture public function")
        .span = Some(rustdoc_types::Span {
        filename: "unlisted.rs".into(),
        begin: (1, 1),
        end: (1, 1),
    });
    artifact.rustdoc_json = serde_json::to_vec(&document).expect("unlisted public span fixture");
    artifact
        .source_paths
        .retain(|mapping| mapping.rustdoc_item_id != 1);

    let error = CompilerArtifactFixture::extraction_error(artifact);

    assert!(matches!(
        error.compiler_artifact_failure(),
        Some(RustCompilerArtifactFailure::SourceMap { item_id: Some(1), message })
            if message == "compiler-reachable declaration has no logical source provenance"
    ));
}

#[test]
fn cli_shaped_artifact_includes_spanless_macro_generated_public_function() {
    let extraction = CompilerArtifactFixture::extract(CompilerArtifactFixture::rustdoc_document())
        .expect("mapped macro-compatible function extraction");
    assert_eq!(extraction.projection().entries().len(), 1);
    assert!(matches!(
        extraction.projection().entries()[0].declaration(),
        RustDeclaration::Function(_)
    ));
    let entry = &extraction.projection().entries()[0];
    let error = entry
        .source_span()
        .expect_err("compiler-generated API has no exact source span");
    assert!(error.to_string().contains("crate root lib.rs"));
    assert!(error.to_string().contains("exact source provenance"));
}

#[test]
fn compiler_alignment_one_packing_matches_bare_source_packing() {
    let mut document = CompilerArtifactFixture::supported_surface_document();
    document
        .index
        .get_mut(&rustdoc_types::Id(2))
        .expect("record item")
        .attrs = vec![rustdoc_types::Attribute::Repr(
        rustdoc_types::AttributeRepr {
            kind: rustdoc_types::ReprKind::Rust,
            align: None,
            packed: Some(1),
            int: None,
        },
    )];
    let extraction =
        CompilerArtifactFixture::extract(document).expect("alignment-one compiler packing");
    let repr = extraction
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
        .expect("record packing representation");

    assert_eq!(
        repr.hints()
            .iter()
            .map(RustReprHint::as_str)
            .collect::<Vec<_>>(),
        ["packed"]
    );
}

#[test]
fn public_sketch_resolution_dispatches_through_compiler_source_mappings() {
    let kit = SignatureContractKit::builder()
        .build()
        .expect("compiler kit");
    let response = futures_executor::block_on(kit.resolve_sketches(ResolveSketchesRequest {
        extraction: RustExtractionInput::Compiler(CompilerArtifactFixture::exact_artifact()),
        source_files: CompilerArtifactFixture::sources(),
        contract_files: CompilerArtifactFixture::linked_contracts(),
    }))
    .expect("compiler sketch resolution");

    assert_eq!(response.seeds.len(), 1);
    assert_eq!(response.seeds[0].sketch_id, "answer_example");
    assert_eq!(response.seeds[0].code, "make_answer!();");
}

#[test]
fn compiler_generated_public_item_rejects_exact_sketch_source_request() {
    let kit = SignatureContractKit::builder()
        .build()
        .expect("compiler kit");
    let error = CompilerArtifactFixture::error(
        futures_executor::block_on(kit.resolve_sketches(ResolveSketchesRequest {
            extraction: RustExtractionInput::Compiler(CompilerArtifactFixture::artifact()),
            source_files: CompilerArtifactFixture::sources(),
            contract_files: CompilerArtifactFixture::linked_contracts(),
        })),
        "compiler-generated item has no exact sketch text",
    );

    assert!(error.to_string().contains("compiler-generated declaration"));
    assert!(error.to_string().contains("exact source provenance"));
}

#[test]
fn missing_source_mapping_fails_closed_for_reachable_items() {
    let mut artifact = CompilerArtifactFixture::artifact();
    artifact.source_paths.clear();
    let error = CompilerArtifactFixture::extraction_error(artifact);
    assert!(matches!(
        error.compiler_artifact_failure(),
        Some(RustCompilerArtifactFailure::SourceMap {
            item_id: Some(1),
            ..
        })
    ));
}

#[test]
fn compiler_artifact_and_node_limits_fail_with_typed_resource_evidence() {
    for (limits, expected_resource) in [
        (
            RustExtractionLimits {
                compiler_artifact_bytes: 1,
                ..RustExtractionLimits::default()
            },
            None,
        ),
        (
            RustExtractionLimits {
                compiler_nodes: 0,
                ..RustExtractionLimits::default()
            },
            Some(crate::limits::LimitResource::RustCompilerNodeCount),
        ),
    ] {
        let error = CompilerArtifactFixture::extraction_error_with_limits(
            CompilerArtifactFixture::artifact(),
            limits,
        );
        let limit = error.limit_exceeded().expect("typed limit evidence");
        if let Some(expected_resource) = expected_resource {
            assert_eq!(limit.resource, expected_resource);
        }
    }
}
