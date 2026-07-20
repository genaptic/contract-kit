//! Cargo/rustdoc-backed signature extraction owned by the CLI process adapter.
//!
//! The extractor selects one package library or binary through locked Cargo,
//! invokes the pinned dated nightly, captures rustdoc-specific arguments with
//! a private one-shot probe, and runs every child in an isolated target tree.
//! Output, artifact traversal, semantic resources, elapsed time, cancellation,
//! and cleanup are bounded. Before returning one versioned in-memory artifact,
//! the extractor revalidates the exact source snapshot through the caller's
//! cumulative catalog-read ledger and translates only allowlisted local-crate
//! spans. Cargo/rustdoc process policy stays here; signature semantics stay in
//! `conkit-signature`.

mod error;
mod extractor;
mod limits;
mod probe;
mod process;
mod project;
mod source;

use std::path::Path;

use cargo_metadata::Metadata;
use conkit_signature::{
    FileCatalog, RUST_COMPILER_ARTIFACT_SCHEMA_VERSION, RustCompilerArtifact, RustCompilerCrate,
    RustCrateRoot,
};

use crate::catalog::CatalogReadBudget;
use crate::contracts::CompilerRequest;

pub(crate) use error::CompilerError;
use extractor::{CompilerConfiguration, CompilerIdentity};
pub(crate) use limits::CompilerCancellation;
use limits::{CompilerLimits, CompilerUsage};
pub(crate) use probe::RustdocProbe;
use process::{CargoProgram, CompilerOperation, CompilerSemanticResource};
use project::{CargoProject, CompilerWorkspace};
use source::CompilerSourceTranslator;

/// The one nightly toolchain whose rustdoc JSON format is supported by this
/// extractor implementation.
pub(crate) const SUPPORTED_COMPILER_TOOLCHAIN: &str = "nightly-2026-07-01";

/// Semantic version of the CLI-owned compiler extractor, independent from the
/// package release version.
pub(crate) const COMPILER_EXTRACTOR_VERSION: &str = "conkit-rustdoc-json-v1";

/// One concrete, bounded owner for all compiler-backed extraction work.
pub(crate) struct CompilerExtractor {
    cargo: CargoProgram,
    limits: CompilerLimits,
    cancellation: CompilerCancellation,
}

/// Expected logical crate identity for one selected Cargo target.
#[derive(Clone, Copy)]
pub(crate) enum CompilerCrateSelection<'root> {
    /// Derive the logical crate identity from the selected Cargo target.
    Inferred,
    /// Require the selected Cargo target to match this explicit or persisted root.
    Expected(&'root RustCrateRoot),
}

impl CompilerExtractor {
    /// Creates the default Cargo/rustdoc adapter bound to root cancellation.
    pub(crate) fn new(cancellation: CompilerCancellation) -> Self {
        Self {
            cargo: CargoProgram::supported(),
            limits: CompilerLimits::default(),
            cancellation,
        }
    }

    /// Rejects a persisted or explicitly requested compiler-root set before
    /// any warning, manifest access, workspace creation, or Cargo process.
    ///
    /// # Errors
    ///
    /// Returns an error when an explicit or persisted selection contains any
    /// number of crate roots other than exactly one.
    pub(crate) fn validate_expected_crates<'root>(
        &self,
        expected_crates: Option<&'root [RustCrateRoot]>,
    ) -> Result<CompilerCrateSelection<'root>, CompilerError> {
        match expected_crates {
            None => Ok(CompilerCrateSelection::Inferred),
            Some([expected]) => Ok(CompilerCrateSelection::Expected(expected)),
            Some(expected) => Err(CompilerError::CompilerCrateCount {
                expected: 1,
                actual: expected.len(),
            }),
        }
    }

    /// Produces one versioned, in-memory compiler artifact.
    ///
    /// The call is deliberately synchronous. The executable has one outer
    /// `block_on` boundary, and commands invoke this method before borrowing a
    /// domain work permit or crossing an `.await`, so no async worker or lock is
    /// blocked by the Cargo child process.
    ///
    /// # Errors
    ///
    /// Returns an error on cancellation; invalid manifest, package, target, or
    /// persisted-root selection; Cargo/rustdoc launch, output, timeout, cleanup,
    /// or resource-limit failure; malformed or incompatible rustdoc output; a
    /// target mismatch; source-provenance failure; or a changed source snapshot.
    pub(crate) fn extract(
        &self,
        request: &CompilerRequest<'_>,
        source_root: &Path,
        source_files: &FileCatalog,
        crate_selection: CompilerCrateSelection<'_>,
        catalog_reads: &mut CatalogReadBudget,
    ) -> Result<RustCompilerArtifact, CompilerError> {
        let usage = CompilerUsage::new(self.limits, self.cancellation.clone());
        usage.checkpoint(CompilerOperation::Metadata)?;

        let manifest = self.manifest_path(request.manifest())?;
        usage.checkpoint(CompilerOperation::Metadata)?;
        let current_directory =
            manifest
                .parent()
                .ok_or_else(|| CompilerError::ManifestHasNoParent {
                    path: manifest.clone(),
                })?;
        let workspace = CompilerWorkspace::new()?;
        usage.checkpoint(CompilerOperation::Metadata)?;
        let package_id = if let Some(package) = request.package() {
            let output = self.run(
                &usage,
                CompilerOperation::PackageId,
                current_directory,
                self.package_id_arguments(&manifest, package),
                Some(workspace.target_directory()),
            )?;
            Some(
                std::str::from_utf8(&output.stdout)
                    .map_err(|source| CompilerError::InvalidPackageIdOutput {
                        message: source.to_string(),
                    })?
                    .trim()
                    .to_owned(),
            )
        } else {
            None
        };
        let metadata_output = self.run(
            &usage,
            CompilerOperation::Metadata,
            current_directory,
            self.metadata_arguments(request, &manifest, None),
            Some(workspace.target_directory()),
        )?;
        let metadata: Metadata =
            serde_json::from_slice(&metadata_output.stdout).map_err(|source| {
                CompilerError::InvalidCargoMetadata {
                    message: source.to_string(),
                }
            })?;
        usage.account_metadata(&metadata)?;
        let unresolved_project =
            CargoProject::select(metadata, package_id.as_deref(), request.target())?;
        let resolved_metadata = self.run(
            &usage,
            CompilerOperation::Metadata,
            current_directory,
            self.metadata_arguments(request, &manifest, Some(&unresolved_project)),
            Some(workspace.target_directory()),
        )?;
        let resolved_metadata: Metadata = serde_json::from_slice(&resolved_metadata.stdout)
            .map_err(|source| CompilerError::InvalidCargoMetadata {
                message: source.to_string(),
            })?;
        usage.account_metadata(&resolved_metadata)?;
        let project =
            CargoProject::select(resolved_metadata, package_id.as_deref(), request.target())?;
        let crate_root = project.crate_identity(source_root, crate_selection)?;

        let compiler_version = self.run(
            &usage,
            CompilerOperation::CompilerVersion,
            current_directory,
            project.rustc_arguments(
                request,
                &manifest,
                workspace.target_directory(),
                None,
                ["-vV"],
            ),
            Some(workspace.target_directory()),
        )?;
        let compiler_identity = CompilerIdentity::parse(
            &compiler_version.utf8_stdout(CompilerOperation::CompilerVersion)?,
        )?;
        let rustdoc_cfg_values = self.probe_rustdoc_configuration(
            &usage,
            request,
            &manifest,
            current_directory,
            &workspace,
            &project,
        )?;

        self.run(
            &usage,
            CompilerOperation::Rustdoc,
            current_directory,
            project.rustdoc_arguments(request, &manifest, workspace.target_directory()),
            Some(workspace.target_directory()),
        )?;
        usage.checkpoint(CompilerOperation::Rustdoc)?;
        let rustdoc_json = workspace.read_rustdoc_json(&project, &usage)?;
        usage.checkpoint(CompilerOperation::Rustdoc)?;
        let document = self.decode_rustdoc(&rustdoc_json, project.kind())?;
        usage.account_semantic(
            CompilerOperation::Rustdoc,
            CompilerSemanticResource::RustdocItems,
            u64::try_from(document.index.items.len()).unwrap_or(u64::MAX),
        )?;
        if let Some(expected) = request.target_triple()
            && document.target.triple != expected
        {
            return Err(CompilerError::RustdocTargetMismatch {
                expected: expected.to_owned(),
                actual: document.target.triple.clone(),
            });
        }
        let compiler_configuration = self.run(
            &usage,
            CompilerOperation::Configuration,
            current_directory,
            project.rustc_arguments(
                request,
                &manifest,
                workspace.target_directory(),
                Some(&document.target.triple),
                ["--print", "cfg"],
            ),
            Some(workspace.target_directory()),
        )?;
        let cfg_values = CompilerConfiguration::merge(
            &usage,
            &compiler_configuration.utf8_stdout(CompilerOperation::Configuration)?,
            rustdoc_cfg_values,
        )?;
        usage.checkpoint(CompilerOperation::Rustdoc)?;
        self.verify_source_snapshot(&usage, source_root, source_files, catalog_reads)?;
        usage.checkpoint(CompilerOperation::Rustdoc)?;
        let source_paths = CompilerSourceTranslator::new(
            &usage,
            &document,
            current_directory,
            source_root,
            source_files,
            &crate_root.root,
        )?
        .into_mappings()?;
        let compiler_crate = RustCompilerCrate {
            id: crate_root.id.clone(),
            package: project.package_name().to_owned(),
            target: project.target_name().to_owned(),
            root: crate_root.root.clone(),
            root_item_id: document.root.0,
            kind: project.kind(),
        };
        usage.checkpoint(CompilerOperation::Rustdoc)?;

        Ok(RustCompilerArtifact {
            schema_version: RUST_COMPILER_ARTIFACT_SCHEMA_VERSION,
            extractor_version: COMPILER_EXTRACTOR_VERSION.to_owned(),
            compiler_version: compiler_identity.contract_value(),
            rustdoc_format_version: document.format_version,
            target_triple: document.target.triple,
            features: project.features().to_vec(),
            cfg_values,
            crates: vec![compiler_crate],
            rustdoc_json,
            source_paths,
        })
    }
}

#[cfg(test)]
mod tests {
    pub(super) use std::collections::BTreeMap;
    pub(super) use std::ffi::{OsStr, OsString};
    pub(super) use std::io::{Cursor, ErrorKind, Write as _};
    pub(super) use std::path::{Path, PathBuf};
    pub(super) use std::process::Command;
    pub(super) use std::sync::Arc;
    pub(super) use std::sync::atomic::{AtomicBool, Ordering};
    pub(super) use std::time::{Duration, Instant};

    pub(super) use cargo_metadata::Metadata;
    pub(super) use conkit_signature::{
        CatalogPath, CompilerSourceProvenance, FileCatalog, RustCrateKind, RustCrateRoot,
    };

    pub(super) use super::error::CompilerError;
    pub(super) use super::extractor::{
        CompilerConfiguration, CompilerIdentity, RustdocId, RustdocSourceDocument,
        RustdocSourceIndex, RustdocSourceItem, RustdocSourceSpan, RustdocTarget,
    };
    pub(super) use super::limits::{
        CompilerCancellation, CompilerLimits, CompilerTemporaryTree, CompilerUsage,
        TemporaryTreeConsistency,
    };
    pub(super) use super::probe::{
        ProbeRecord, ProbeState, RUSTDOC_PROBE_EXIT_CODE, RUSTDOC_PROBE_MAX_ARGUMENT_BYTES,
        RustdocProbe, RustdocProbeError,
    };
    pub(super) use super::process::{
        BoundedPipe, BoundedPipes, CargoEnvironment, CargoOutput, CargoProcess, CargoProgram,
        CompilerOperation, CompilerSemanticResource, CompilerStream,
        PROCESS_CLEANUP_EVIDENCE_BYTES, ProcessCleanupEvidence, ProcessReaderStartupFailure,
    };
    pub(super) use super::project::CargoProject;
    pub(super) use super::source::{CompilerSourceTranslator, SourceEndpointResolver};
    pub(super) use super::{
        COMPILER_EXTRACTOR_VERSION, CompilerCrateSelection, CompilerExtractor,
        SUPPORTED_COMPILER_TOOLCHAIN,
    };
    pub(super) use crate::catalog::CatalogReadLimits;
    pub(super) use crate::context::ApplicationCancellation;
    pub(super) use crate::contracts::{CargoFeatures, CargoTarget, CompilerRequest};

    pub(super) struct CompilerFixture;

    impl CompilerFixture {
        pub(super) fn cancellation() -> CompilerCancellation {
            CompilerCancellation::from_flag(Arc::new(AtomicBool::new(false)))
        }

        pub(super) fn request() -> CompilerRequest<'static> {
            Self::request_with_target(Some("x86_64-unknown-linux-gnu"))
        }

        pub(super) fn request_with_target(
            target_triple: Option<&'static str>,
        ) -> CompilerRequest<'static> {
            CompilerRequest::new(
                Path::new("/workspace/Cargo.toml"),
                None,
                CargoTarget::Automatic,
                CargoFeatures::Selected {
                    names: &[],
                    include_default: false,
                },
                target_triple,
            )
        }

        pub(super) fn metadata() -> Metadata {
            serde_json::from_value(serde_json::json!({
                "packages": [{
                    "name": "sample",
                    "version": "0.1.0",
                    "authors": [],
                    "id": "path+file:///workspace#sample@0.1.0",
                    "source": null,
                    "description": null,
                    "dependencies": [],
                    "license": null,
                    "license_file": null,
                    "targets": [
                        {
                            "kind": ["lib"],
                            "crate_types": ["lib"],
                            "name": "sample",
                            "src_path": "/workspace/src/lib.rs",
                            "edition": "2024",
                            "doc": true,
                            "doctest": true,
                            "test": true
                        },
                        {
                            "kind": ["bin"],
                            "crate_types": ["bin"],
                            "name": "sample-cli",
                            "src_path": "/workspace/src/main.rs",
                            "edition": "2024",
                            "doc": true,
                            "doctest": false,
                            "test": true
                        }
                    ],
                    "features": {"default": [], "serde": []},
                    "manifest_path": "/workspace/Cargo.toml",
                    "categories": [],
                    "keywords": [],
                    "readme": null,
                    "repository": null,
                    "homepage": null,
                    "documentation": null,
                    "edition": "2024",
                    "metadata": null,
                    "links": null,
                    "publish": null,
                    "default_run": null,
                    "rust_version": null
                }],
                "workspace_members": ["path+file:///workspace#sample@0.1.0"],
                "workspace_default_members": ["path+file:///workspace#sample@0.1.0"],
                "resolve": {
                    "nodes": [{
                        "id": "path+file:///workspace#sample@0.1.0",
                        "dependencies": [],
                        "deps": [],
                        "features": ["serde"]
                    }],
                    "root": "path+file:///workspace#sample@0.1.0"
                },
                "workspace_root": "/workspace",
                "target_directory": "/workspace/target",
                "build_directory": "/workspace/target/build",
                "metadata": null,
                "version": 1
            }))
            .expect("Cargo metadata fixture")
        }

        pub(super) fn source_span(
            filename: impl Into<PathBuf>,
            begin: (usize, usize),
            end: (usize, usize),
        ) -> RustdocSourceSpan {
            RustdocSourceSpan {
                filename: filename.into(),
                begin,
                end,
            }
        }

        pub(super) fn source_item(
            id: u32,
            crate_id: u32,
            span: Option<RustdocSourceSpan>,
        ) -> RustdocSourceItem {
            RustdocSourceItem {
                id: RustdocId(id),
                crate_id,
                span,
            }
        }

        pub(super) fn source_document(
            includes_private: bool,
            additional: impl IntoIterator<Item = RustdocSourceItem>,
        ) -> RustdocSourceDocument {
            let mut items = vec![Self::source_item(0, 0, None)];
            items.extend(additional);
            items.sort_unstable_by_key(|item| item.id);
            RustdocSourceDocument {
                root: RustdocId(0),
                index: RustdocSourceIndex { items },
                target: RustdocTarget {
                    triple: "x86_64-unknown-linux-gnu".to_owned(),
                },
                format_version: conkit_signature::RUSTDOC_FORMAT_VERSION,
                includes_private,
            }
        }

        pub(super) fn rustdoc_projection_value() -> serde_json::Value {
            serde_json::json!({
                "root": 0,
                "index": {
                    "0": { "id": 0, "crate_id": 0, "span": null },
                    "9": {
                        "id": 9,
                        "crate_id": 0,
                        "span": {
                            "filename": "lib.rs",
                            "begin": [1, 1],
                            "end": [1, 1]
                        }
                    }
                },
                "target": { "triple": "x86_64-unknown-linux-gnu" },
                "format_version": conkit_signature::RUSTDOC_FORMAT_VERSION,
                "includes_private": false
            })
        }

        pub(super) fn decode_projection(
            value: &serde_json::Value,
            kind: RustCrateKind,
        ) -> Result<RustdocSourceDocument, CompilerError> {
            let bytes = serde_json::to_vec(value).expect("rustdoc projection fixture");
            CompilerExtractor::new(Self::cancellation()).decode_rustdoc(&bytes, kind)
        }

        pub(super) fn projection_without(pointer: &str) -> serde_json::Value {
            let mut value = Self::rustdoc_projection_value();
            let (parent, field) = pointer.rsplit_once('/').expect("field JSON pointer");
            value
                .pointer_mut(parent)
                .and_then(serde_json::Value::as_object_mut)
                .expect("projection object")
                .remove(field);
            value
        }

        pub(super) fn rustdoc_probe_arguments(source: &Path, target: &Path) -> Vec<OsString> {
            vec![
                OsString::from("--crate-name"),
                OsString::from("sample"),
                OsString::from("--crate-type=lib"),
                source.as_os_str().to_owned(),
                OsString::from("-o"),
                target.join("doc").into_os_string(),
                OsString::from("-Zunstable-options"),
                OsString::from("--output-format=json"),
                OsString::from("--document-hidden-items"),
                OsString::from("--cfg"),
                OsString::from("docsrs"),
                OsString::from("--cfg=feature=\"serde\""),
            ]
        }

        pub(super) fn child_probe(
            target: &Path,
            source: &Path,
            token_byte: char,
            crate_type: &str,
        ) -> RustdocProbe {
            let token = token_byte.to_string().repeat(64);
            RustdocProbe::child(
                target.join(format!(".conkit-rustdoc-probe-{token}.json")),
                token,
                "sample".to_owned(),
                source.to_path_buf(),
                target.to_path_buf(),
                vec![crate_type.to_owned()],
            )
            .expect("valid private probe")
        }

        pub(super) fn primary_error(error: &CompilerError) -> &CompilerError {
            match error {
                CompilerError::ProcessFailureWithEvidence { primary, .. } => {
                    Self::primary_error(primary)
                }
                _ => error,
            }
        }

        pub(super) fn process_limits(timeout: Duration, stdout_bytes: u64) -> CompilerLimits {
            CompilerLimits {
                stdout_bytes,
                stderr_bytes: 4 * 1024,
                timeout,
                cleanup_timeout: Duration::from_millis(250),
                poll_interval: Duration::from_millis(1),
                temporary_tree_scan_interval: Duration::from_millis(10),
                ..CompilerLimits::default()
            }
        }
    }

    pub(super) struct SourceMappingFixture {
        pub(super) root: assert_fs::TempDir,
        pub(super) catalog: FileCatalog,
        pub(super) crate_root: CatalogPath,
    }

    impl SourceMappingFixture {
        pub(super) fn new(files: &[(&str, &[u8])]) -> Self {
            let root = assert_fs::TempDir::new().expect("source root");
            let mut catalog = FileCatalog::new();
            for (path, bytes) in files {
                std::fs::write(root.path().join(path), bytes).expect("physical source");
                catalog
                    .insert(
                        CatalogPath::new(*path).expect("source path"),
                        bytes.to_vec(),
                    )
                    .expect("source entry");
            }
            Self {
                root,
                catalog,
                crate_root: CatalogPath::new(files[0].0).expect("crate root"),
            }
        }

        pub(super) fn span(
            &self,
            path: &str,
            begin: (usize, usize),
            end: (usize, usize),
        ) -> RustdocSourceSpan {
            CompilerFixture::source_span(self.root.path().join(path), begin, end)
        }

        pub(super) fn translate(
            &self,
            document: &RustdocSourceDocument,
            limits: CompilerLimits,
        ) -> Result<Vec<conkit_signature::CompilerSourcePath>, CompilerError> {
            let usage = CompilerUsage::new(limits, CompilerFixture::cancellation());
            CompilerSourceTranslator::new(
                &usage,
                document,
                self.root.path(),
                self.root.path(),
                &self.catalog,
                &self.crate_root,
            )?
            .into_mappings()
        }
    }

    #[derive(Clone, Copy)]
    pub(super) enum ProcessBehavior {
        Success,
        Nonzero,
        Busy,
        RunawayOutput,
    }

    impl ProcessBehavior {
        pub(super) const ALL: [Self; 4] = [
            Self::Success,
            Self::Nonzero,
            Self::Busy,
            Self::RunawayOutput,
        ];

        pub(super) fn marker(self) -> &'static str {
            match self {
                Self::Success => "process-success",
                Self::Nonzero => "process-nonzero",
                Self::Busy => "process-busy",
                Self::RunawayOutput => "process-runaway-output",
            }
        }

        pub(super) fn program(self, root: &Path) -> CargoProgram {
            std::fs::write(root.join(self.marker()), b"selected").expect("process behavior marker");
            CargoProgram {
                executable: std::env::current_exe()
                    .expect("current test executable")
                    .into_os_string(),
                prefix: ["--ignored", "cargo_process_helper", "--nocapture"]
                    .into_iter()
                    .map(OsString::from)
                    .collect(),
            }
        }

        pub(super) fn detect(root: &Path) -> Self {
            let mut selected = Self::ALL
                .into_iter()
                .filter(|behavior| root.join(behavior.marker()).is_file());
            let behavior = selected.next().expect("one process behavior marker");
            assert!(
                selected.next().is_none(),
                "only one process behavior marker"
            );
            behavior
        }

        pub(super) fn run(self) {
            match self {
                Self::Success => {
                    println!("bounded success output");
                    eprintln!("bounded success diagnostic");
                }
                Self::Nonzero => std::process::exit(17),
                Self::Busy => loop {
                    std::thread::park_timeout(Duration::from_millis(10));
                },
                Self::RunawayOutput => {
                    let mut output = std::io::stdout().lock();
                    let bytes = [b'x'; 8 * 1024];
                    while output.write_all(&bytes).is_ok() {}
                }
            }
        }
    }

    pub(super) struct ProcessFixture {
        pub(super) root: assert_fs::TempDir,
        pub(super) cargo: CargoProgram,
        pub(super) usage: Arc<CompilerUsage>,
    }

    impl ProcessFixture {
        pub(super) fn new(
            behavior: ProcessBehavior,
            limits: CompilerLimits,
            cancellation: CompilerCancellation,
        ) -> Self {
            let root = assert_fs::TempDir::new().expect("process root");
            let cargo = behavior.program(root.path());
            Self {
                root,
                cargo,
                usage: CompilerUsage::new(limits, cancellation),
            }
        }

        pub(super) fn spawn(&self, operation: CompilerOperation) -> CargoProcess<'_> {
            CargoProcess::spawn(
                &self.cargo,
                operation,
                self.root.path(),
                Vec::new(),
                None,
                &self.usage,
                CargoEnvironment::Pinned,
            )
            .expect("spawn process helper")
        }
    }

    #[test]
    fn expected_compiler_root_validation_rejects_multiple_roots() {
        let expected = [
            RustCrateRoot {
                id: "first".to_owned(),
                root: CatalogPath::new("lib.rs").expect("first root"),
                kind: RustCrateKind::Library,
            },
            RustCrateRoot {
                id: "second".to_owned(),
                root: CatalogPath::new("second.rs").expect("second root"),
                kind: RustCrateKind::Library,
            },
        ];
        let error = CompilerExtractor::new(CompilerFixture::cancellation())
            .validate_expected_crates(Some(&expected))
            .err()
            .expect("aggregate root count must fail during preflight");

        assert!(matches!(
            error,
            CompilerError::CompilerCrateCount {
                expected: 1,
                actual: 2,
            }
        ));
    }
}
