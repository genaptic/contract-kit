use super::*;

#[test]
fn artifact_schema_and_rustdoc_format_mismatches_fail_before_conversion() {
    let cancellation = CancellationProbe::new();
    let mut schema = CompilerArtifactFixture::artifact();
    schema.schema_version += 1;
    let mut format = CompilerArtifactFixture::artifact();
    format.rustdoc_format_version += 1;
    for (mut artifact, schema_mismatch) in [(schema, true), (format, false)] {
        let error = artifact
            .validate_metadata(&cancellation)
            .expect_err("unsupported compiler schema");
        assert!(matches!(
            (schema_mismatch, error.compiler_artifact_failure()),
            (
                true,
                Some(RustCompilerArtifactFailure::SchemaVersion { .. })
            ) | (
                false,
                Some(RustCompilerArtifactFailure::RustdocFormat { .. })
            )
        ));
    }
}

#[test]
fn compiler_crate_identity_must_be_singular_and_match_the_rustdoc_root() {
    let mut absent = CompilerArtifactFixture::artifact();
    absent.crates.clear();
    let mut mismatched = CompilerArtifactFixture::artifact();
    mismatched.crates[0].root_item_id = 99;
    for (artifact, expected_field) in [(absent, "crates"), (mismatched, "crates[].root_item_id")] {
        let error = CompilerArtifactFixture::extraction_error(artifact);
        assert!(matches!(
            error.compiler_artifact_failure(),
            Some(RustCompilerArtifactFailure::InvalidMetadata { field, .. })
                if *field == expected_field
        ));
    }
}

#[test]
fn compiler_context_sorts_and_deduplicates_features_and_cfg_values() {
    let mut artifact = CompilerArtifactFixture::artifact();
    let context = artifact
        .validate_metadata(&CancellationProbe::new())
        .expect("valid compiler metadata");
    assert_eq!(context.features(), ["alpha", "zeta"]);
    assert_eq!(context.cfg_values(), ["target_pointer_width=64", "unix"]);
}

#[test]
fn compiler_projection_contains_only_cfg_selected_reachable_items() {
    let mut document = CompilerArtifactFixture::rustdoc_document();
    let mut disabled = document
        .index
        .get(&rustdoc_types::Id(1))
        .expect("fixture function")
        .clone();
    disabled.id = rustdoc_types::Id(2);
    disabled.name = Some("disabled_answer".to_owned());
    document.index.insert(disabled.id, disabled);
    document.paths.insert(
        rustdoc_types::Id(2),
        rustdoc_types::ItemSummary {
            crate_id: 0,
            path: vec!["sample".to_owned(), "disabled_answer".to_owned()],
            kind: rustdoc_types::ItemKind::Function,
        },
    );

    let extraction =
        CompilerArtifactFixture::extract(document).expect("cfg-evaluated rustdoc projection");

    assert_eq!(extraction.projection().entries().len(), 1);
    assert_eq!(extraction.projection().entries()[0].id().name(), "answer");
}

#[test]
fn rustdoc_private_item_flag_must_match_the_selected_target_kind() {
    for (kind, includes_private) in [
        (RustCrateKind::Library, true),
        (RustCrateKind::Binary, false),
    ] {
        let mut artifact = CompilerArtifactFixture::artifact();
        artifact.crates[0].kind = kind;
        let mut document = CompilerArtifactFixture::rustdoc_document();
        document.includes_private = includes_private;
        artifact.rustdoc_json =
            serde_json::to_vec(&document).expect("contradictory rustdoc fixture JSON");
        let error = CompilerArtifactFixture::extraction_error(artifact);

        assert!(matches!(
            error.compiler_artifact_failure(),
            Some(RustCompilerArtifactFailure::InvalidMetadata {
                field: "rustdoc_json.includes_private",
                ..
            })
        ));
    }
}

#[test]
fn decoded_target_must_match_the_host_envelope() {
    let mut artifact = CompilerArtifactFixture::artifact();
    let mut document = CompilerArtifactFixture::rustdoc_document();
    document.target.triple = "aarch64-apple-darwin".to_owned();
    artifact.rustdoc_json = serde_json::to_vec(&document).expect("fixture rustdoc JSON");
    let error = CompilerArtifactFixture::extraction_error(artifact);
    assert!(matches!(
        error.compiler_artifact_failure(),
        Some(RustCompilerArtifactFailure::TargetMismatch { .. })
    ));
}

#[test]
fn external_unreachable_items_do_not_parse_unlisted_sources() {
    let mut artifact = CompilerArtifactFixture::artifact();
    let mut document = CompilerArtifactFixture::rustdoc_document();
    let mut unreachable = document
        .index
        .get(&rustdoc_types::Id(1))
        .expect("fixture function")
        .clone();
    unreachable.id = rustdoc_types::Id(9);
    unreachable.crate_id = 1;
    unreachable.name = Some("external_answer".to_owned());
    document.index.insert(unreachable.id, unreachable);
    artifact.rustdoc_json = serde_json::to_vec(&document).expect("fixture rustdoc JSON");
    let mut sources = CompilerArtifactFixture::sources();
    sources
        .insert(
            CatalogPath::new("unreachable.rs").expect("fixture path"),
            vec![0xff],
        )
        .expect("insert unreachable fixture source");

    let limits = RustExtractionLimits::default();
    let mut usage = limits.usage();
    let extraction = artifact
        .extract(
            sources,
            &CompilerArtifactFixture::allowed_files(),
            &limits,
            &mut usage,
            &CancellationProbe::new(),
        )
        .expect("unreachable mapped source must remain unparsed");
    assert_eq!(extraction.projection().entries().len(), 1);
}

#[test]
fn private_binary_exact_sources_are_limited_during_source_map_validation() {
    let private_id = rustdoc_types::Id(2);
    let private_path = CatalogPath::new("private.rs").expect("private source path");
    let mut document = CompilerArtifactFixture::rustdoc_document();
    document.includes_private = true;
    let mut private_function = document
        .index
        .get(&rustdoc_types::Id(1))
        .expect("fixture function")
        .clone();
    private_function.id = private_id;
    private_function.name = Some("private_answer".to_owned());
    private_function.visibility = rustdoc_types::Visibility::Default;
    private_function.span = Some(rustdoc_types::Span {
        filename: "private.rs".into(),
        begin: (1, 1),
        end: (1, 1),
    });
    CompilerArtifactFixture::root_items(&mut document).push(private_id);
    document.index.insert(private_id, private_function);

    let mut artifact = CompilerArtifactFixture::artifact_for(document);
    artifact.crates[0].kind = RustCrateKind::Binary;
    CompilerArtifactFixture::set_provenance(
        &mut artifact,
        private_id.0,
        CompilerSourceProvenance::Exact {
            file: private_path.clone(),
            byte_start: 0,
            byte_end: 1,
        },
    );

    let per_file_bytes =
        u64::try_from(CompilerArtifactFixture::source().len()).expect("fixture source length");
    let limits = RustExtractionLimits {
        per_file_bytes,
        ..RustExtractionLimits::default()
    };
    let mut oversized_sources = CompilerArtifactFixture::sources();
    oversized_sources
        .insert(
            private_path.clone(),
            vec![b'x'; usize::try_from(per_file_bytes + 1).expect("oversized fixture length")],
        )
        .expect("oversized private source");
    let allowed_files = BTreeSet::from([
        CatalogPath::new("lib.rs").expect("crate root path"),
        private_path.clone(),
    ]);
    let mut usage = limits.usage();
    let error = CompilerArtifactFixture::error(
        artifact.clone().extract(
            oversized_sources,
            &allowed_files,
            &limits,
            &mut usage,
            &CancellationProbe::new(),
        ),
        "an exact mapped private source must respect Rust file limits",
    );
    let exceeded = error.limit_exceeded().expect("typed private-source limit");
    assert_eq!(
        exceeded.resource,
        crate::limits::LimitResource::RustSourceFileBytes
    );
    assert_eq!(exceeded.limit, per_file_bytes);
    assert_eq!(exceeded.observed_at_least, per_file_bytes + 1);
    assert_eq!(exceeded.file.as_ref(), Some(&private_path));

    let mut boundary_sources = CompilerArtifactFixture::sources();
    boundary_sources
        .insert(
            private_path,
            vec![b'x'; usize::try_from(per_file_bytes).expect("boundary fixture length")],
        )
        .expect("boundary private source");
    let mut usage = limits.usage();
    let extraction = artifact
        .extract(
            boundary_sources,
            &allowed_files,
            &limits,
            &mut usage,
            &CancellationProbe::new(),
        )
        .expect("the exact byte boundary must preserve private filtering");
    assert_eq!(extraction.projection().entries().len(), 1);
}
