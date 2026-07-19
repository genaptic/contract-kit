use super::provenance::CompilerSourceIndex;
use crate::api::{RustCrateKind, RustCrateRoot};
use crate::error::SignatureContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::languages::rust::parser::source_graph::RustCrateId;
use crate::languages::rust::parser::{RustParsedProjection, RustProjectedSource};
use crate::languages::rust::source::RustSourceCatalog;
use crate::limits::RustExtractionLimits;
use crate::work::CancellationProbe;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Contract Kit's version for the host-produced compiler artifact envelope.
///
/// Hosts must write this value into [`RustCompilerArtifact::schema_version`].
/// Unknown older or newer envelope versions fail without conversion fallback.
pub const RUST_COMPILER_ARTIFACT_SCHEMA_VERSION: u16 = 1;

/// rustdoc JSON schema understood by the pinned `rustdoc-types` dependency.
///
/// The envelope and decoded rustdoc document must both report this exact
/// format. The host remains responsible for selecting a compatible compiler
/// and producing the rustdoc JSON bytes.
pub const RUSTDOC_FORMAT_VERSION: u32 = rustdoc_types::FORMAT_VERSION;

/// Host-resolved identity for the one local crate represented by rustdoc JSON.
///
/// Artifact schema version 1 accepts exactly one selected Cargo target. Its
/// logical ID, root, and target kind must agree with the signature-bearing
/// contract document; `root_item_id` must agree with the decoded rustdoc root.
/// Library targets require public-only rustdoc output, while binary targets
/// require rustdoc output that includes private items.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustCompilerCrate {
    /// Stable contract crate ID, independent of package display names.
    pub id: String,
    /// Nonempty Cargo package name selected by the host.
    pub package: String,
    /// Nonempty Cargo target name selected by the host.
    pub target: String,
    /// Allowlisted logical `.rs` path to this target's crate root.
    pub root: CatalogPath,
    /// Inner `u32` value of the rustdoc JSON crate `root` ID.
    pub root_item_id: u32,
    /// Selected Cargo target role.
    pub kind: RustCrateKind,
}

impl RustCompilerCrate {
    /// Returns the exact crate-root identity persisted by fresh generation.
    pub fn crate_root(&self) -> RustCrateRoot {
        RustCrateRoot {
            id: self.id.clone(),
            root: self.root.clone(),
            kind: self.kind,
        }
    }
}

/// Explicit source provenance for one compiler-reachable rustdoc item.
///
/// Exact provenance is accepted only for a nonempty UTF-8-aligned byte range
/// whose logical filename and one-indexed Unicode-scalar line/column range
/// agree with the rustdoc span. Compiler-generated provenance is accepted only
/// when rustdoc supplies no span and must name the selected crate root. These
/// variants are mutually exclusive; provenance is never guessed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CompilerSourceProvenance {
    /// The item has an exact range in an allowlisted logical Rust source file.
    Exact {
        /// Logical source path in the accompanying source catalog.
        file: CatalogPath,
        /// Inclusive UTF-8 byte offset in `file`.
        byte_start: u64,
        /// Exclusive UTF-8 byte offset in `file`, greater than `byte_start`.
        byte_end: u64,
    },
    /// The compiler created the item and rustdoc supplies no exact source span.
    CompilerGenerated {
        /// Selected logical crate root that owns the generated public item.
        crate_root: CatalogPath,
    },
}

/// Explicit host translation from one rustdoc item to logical source provenance.
///
/// Mappings must be unique by local rustdoc item ID, refer only to the selected
/// local crate, and identify an allowlisted source path. Every admitted
/// compiler-reachable declaration needs a mapping; omitted mappings are
/// tolerated only for rustdoc items that never enter the reachable API graph.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompilerSourcePath {
    /// Inner `u32` value of the rustdoc item ID.
    pub rustdoc_item_id: u32,
    /// Exact source range or explicit compiler-generated crate-root ownership.
    pub provenance: CompilerSourceProvenance,
}

/// Versioned, in-memory compiler extraction artifact assembled by a host.
///
/// The host owns toolchain selection, Cargo target resolution, feature/cfg
/// evaluation, process execution, and rustdoc JSON production. This crate
/// performs no subprocess or filesystem work: it validates the envelope,
/// decodes one selected crate, validates every admitted source mapping, and
/// lowers compiler-resolved facts into the same contract model used by syntax
/// extraction. Unsupported or contradictory facts fail closed; compiler mode
/// never falls back to syntax extraction.
///
/// A valid artifact must use [`RUST_COMPILER_ARTIFACT_SCHEMA_VERSION`] and
/// [`RUSTDOC_FORMAT_VERSION`], contain exactly one [`RustCompilerCrate`], and
/// agree with the selected signature-bearing document's compiler/extractor
/// versions, target triple, normalized feature and cfg sets, package/target,
/// crate root, root item, and target kind.
///
/// Producing a valid artifact requires an external Cargo/rustdoc host workflow,
/// so this integration example is compile-checked but not run by rustdoc.
///
/// ```no_run
/// use conkit_signature::{
///     CatalogPath, CheckMode, CheckRequest, FileCatalog, ReportRequest,
///     RustCompilerArtifact, RustExtractionInput, SignatureContractKit,
/// };
///
/// fn host_file(
///     logical: &str,
///     physical: &str,
/// ) -> Result<FileCatalog, Box<dyn std::error::Error>> {
///     let mut catalog = FileCatalog::new();
///     catalog.insert(CatalogPath::new(logical)?, std::fs::read(physical)?)?;
///     Ok(catalog)
/// }
///
/// // An external host has already selected the Cargo target, invoked rustdoc,
/// // normalized provenance, and serialized the complete artifact envelope.
/// let artifact: RustCompilerArtifact = serde_json::from_slice(&std::fs::read(
///     "target/conkit/rustdoc-artifact.json",
/// )?)?;
/// let kit = SignatureContractKit::builder().build()?;
/// let response = futures_executor::block_on(kit.check(CheckRequest {
///     source_files: host_file("src/lib.rs", "src/lib.rs")?,
///     contract_files: host_file("main.yml", "contracts/main.yml")?,
///     extraction: RustExtractionInput::Compiler(artifact),
///     report: ReportRequest::None,
///     mode: CheckMode::Strict,
/// }))?;
/// assert!(response.passed);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustCompilerArtifact {
    /// Contract Kit artifact-envelope schema version.
    pub schema_version: u16,
    /// Version of the host-side extractor implementation.
    pub extractor_version: String,
    /// Full compiler version used to produce this artifact.
    pub compiler_version: String,
    /// rustdoc JSON format version claimed by the host.
    pub rustdoc_format_version: u32,
    /// Compilation target triple selected by the host.
    pub target_triple: String,
    /// Enabled Cargo features; validation sorts and deduplicates this set.
    pub features: Vec<String>,
    /// Relevant evaluated `cfg` values; validation sorts and deduplicates this set.
    pub cfg_values: Vec<String>,
    /// Selected local target metadata; schema version 1 requires exactly one entry.
    pub crates: Vec<RustCompilerCrate>,
    /// Complete rustdoc JSON bytes for the selected target.
    pub rustdoc_json: Vec<u8>,
    /// Unique host-normalized provenance for local compiler-reachable items.
    pub source_paths: Vec<CompilerSourcePath>,
}

/// Typed reason a compiler artifact could not be trusted or converted.
///
/// Public operations wrap this value in [`SignatureContractKitError`], where
/// [`SignatureContractKitError::compiler_artifact_failure`] recovers it without
/// parsing display text.
///
/// # Examples
///
/// ```
/// use conkit_signature::{
///     CatalogPath, ContractScope, FileCatalog, GenerateDocument, GenerateRequest,
///     GenerateTarget, RustCompilerArtifact, RustCompilerArtifactFailure,
///     RustCrateKind, RustCrateRoot, RustExtractionInput, SignatureContractKit,
///     RUST_COMPILER_ARTIFACT_SCHEMA_VERSION, RUSTDOC_FORMAT_VERSION,
/// };
///
/// let artifact = RustCompilerArtifact {
///     schema_version: RUST_COMPILER_ARTIFACT_SCHEMA_VERSION + 1,
///     extractor_version: "host-v1".to_owned(),
///     compiler_version: "rustc host".to_owned(),
///     rustdoc_format_version: RUSTDOC_FORMAT_VERSION,
///     target_triple: "example-target".to_owned(),
///     features: Vec::new(),
///     cfg_values: Vec::new(),
///     crates: Vec::new(),
///     rustdoc_json: Vec::new(),
///     source_paths: Vec::new(),
/// };
/// let mut source_files = FileCatalog::new();
/// source_files.insert(CatalogPath::new("lib.rs")?, Vec::new())?;
/// let kit = SignatureContractKit::builder().build()?;
/// let error = futures_executor::block_on(kit.generate(GenerateRequest {
///     source_files,
///     target: GenerateTarget::New(GenerateDocument {
///         contract_file: CatalogPath::new("main.yml")?,
///         root: "../src".to_owned(),
///         files: vec![CatalogPath::new("lib.rs")?],
///         crates: vec![RustCrateRoot {
///             id: "sample".to_owned(),
///             root: CatalogPath::new("lib.rs")?,
///             kind: RustCrateKind::Library,
///         }],
///     }),
///     extraction: RustExtractionInput::Compiler(artifact),
///     scope: ContractScope::Signatures,
/// }))
/// .unwrap_err();
///
/// assert!(matches!(
///     error.compiler_artifact_failure(),
///     Some(RustCompilerArtifactFailure::SchemaVersion { expected, actual })
///         if *expected == RUST_COMPILER_ARTIFACT_SCHEMA_VERSION
///             && *actual == RUST_COMPILER_ARTIFACT_SCHEMA_VERSION + 1
/// ));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RustCompilerArtifactFailure {
    /// The host envelope uses an unsupported Contract Kit schema version.
    #[error("unsupported compiler artifact schema {actual}; expected {expected}")]
    SchemaVersion {
        /// Schema version supported by this crate.
        expected: u16,
        /// Schema version supplied by the host.
        actual: u16,
    },
    /// The envelope or decoded rustdoc document uses a mismatched format.
    #[error(
        "unsupported rustdoc JSON format: expected {expected}, envelope {envelope}, document {document}"
    )]
    RustdocFormat {
        /// Format represented by the linked `rustdoc-types` crate.
        expected: u32,
        /// Format claimed by the host envelope.
        envelope: u32,
        /// Format decoded from the rustdoc JSON document.
        document: u32,
    },
    /// Required artifact metadata is empty, contradictory, or ambiguous.
    #[error("invalid compiler artifact metadata {field}: {message}")]
    InvalidMetadata {
        /// Metadata field with invalid content.
        field: &'static str,
        /// Actionable validation detail.
        message: String,
    },
    /// rustdoc JSON bytes could not be decoded using the pinned schema.
    #[error("invalid rustdoc JSON: {message}")]
    InvalidJson {
        /// Decoder failure detail.
        message: String,
    },
    /// The decoded target triple differs from the host envelope.
    #[error("compiler artifact target mismatch: envelope {envelope:?}, rustdoc {document:?}")]
    TargetMismatch {
        /// Triple claimed by the host envelope.
        envelope: String,
        /// Triple recorded in rustdoc JSON.
        document: String,
    },
    /// A referenced rustdoc item is absent or structurally inconsistent.
    #[error("invalid rustdoc item {item_id}: {message}")]
    InvalidItem {
        /// Inner rustdoc item ID.
        item_id: u32,
        /// Structural validation detail.
        message: String,
    },
    /// One compiler-reachable API item cannot yet be represented losslessly.
    #[error("unsupported compiler-reachable rustdoc item {item_id} ({item_kind}): {reason}")]
    UnsupportedItem {
        /// Inner rustdoc item ID.
        item_id: u32,
        /// rustdoc item-kind name.
        item_kind: String,
        /// Missing semantic representation.
        reason: String,
    },
    /// One compiler-resolved type cannot yet be represented losslessly.
    #[error("unsupported compiler-resolved type in item {item_id} ({type_kind}): {reason}")]
    UnsupportedType {
        /// Top-level item being converted.
        item_id: u32,
        /// rustdoc type-kind name.
        type_kind: String,
        /// Missing semantic representation.
        reason: String,
    },
    /// A logical source translation is absent, duplicated, or invalid.
    #[error("invalid compiler source mapping for item {item_id:?}: {message}")]
    SourceMap {
        /// Inner rustdoc item ID, when the failure is item-specific.
        item_id: Option<u32>,
        /// Source-map validation detail.
        message: String,
    },
}

impl RustCompilerArtifactFailure {
    pub(super) fn invalid_item(
        item_id: u32,
        message: impl Into<String>,
    ) -> SignatureContractKitError {
        Self::InvalidItem {
            item_id,
            message: message.into(),
        }
        .into()
    }

    pub(super) fn source_map(
        item_id: Option<u32>,
        message: impl Into<String>,
    ) -> SignatureContractKitError {
        Self::SourceMap {
            item_id,
            message: message.into(),
        }
        .into()
    }

    pub(super) fn unsupported_item(
        item_id: u32,
        item_kind: impl Into<String>,
        reason: impl Into<String>,
    ) -> SignatureContractKitError {
        Self::UnsupportedItem {
            item_id,
            item_kind: item_kind.into(),
            reason: reason.into(),
        }
        .into()
    }

    pub(super) fn unsupported_type(
        item_id: u32,
        type_kind: impl Into<String>,
        reason: impl Into<String>,
    ) -> SignatureContractKitError {
        Self::UnsupportedType {
            item_id,
            type_kind: type_kind.into(),
            reason: reason.into(),
        }
        .into()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustCompilerExtractionContext {
    pub(super) artifact_schema_version: u16,
    pub(super) extractor_version: String,
    pub(super) compiler_version: String,
    pub(super) rustdoc_format_version: u32,
    pub(super) target_triple: String,
    pub(super) features: Vec<String>,
    pub(super) cfg_values: Vec<String>,
    pub(super) crate_metadata: RustCompilerCrate,
    pub(super) canonical_crate_id: RustCrateId,
}

impl RustCompilerExtractionContext {
    pub(crate) fn artifact_schema_version(&self) -> u16 {
        self.artifact_schema_version
    }

    pub(crate) fn extractor_version(&self) -> &str {
        &self.extractor_version
    }

    pub(crate) fn compiler_version(&self) -> &str {
        &self.compiler_version
    }

    pub(crate) fn rustdoc_format_version(&self) -> u32 {
        self.rustdoc_format_version
    }

    pub(crate) fn target_triple(&self) -> &str {
        &self.target_triple
    }

    pub(crate) fn features(&self) -> &[String] {
        &self.features
    }

    pub(crate) fn cfg_values(&self) -> &[String] {
        &self.cfg_values
    }

    pub(crate) fn crate_metadata(&self) -> &RustCompilerCrate {
        &self.crate_metadata
    }

    pub(crate) fn canonical_crate_id(&self) -> &RustCrateId {
        &self.canonical_crate_id
    }
}

pub(crate) struct RustCompilerExtraction {
    pub(super) context: RustCompilerExtractionContext,
    pub(super) sources: RustSourceCatalog,
    pub(super) projection: RustParsedProjection,
}

impl RustCompilerExtraction {
    pub(crate) fn context(&self) -> &RustCompilerExtractionContext {
        &self.context
    }

    pub(in crate::languages::rust) fn projection(&self) -> &RustParsedProjection {
        &self.projection
    }

    pub(in crate::languages::rust) fn projected_source(&self) -> RustProjectedSource<'_> {
        RustProjectedSource::new(&self.sources, &self.projection)
    }

    pub(in crate::languages::rust) fn into_parts(
        self,
    ) -> (
        RustCompilerExtractionContext,
        RustSourceCatalog,
        RustParsedProjection,
    ) {
        (self.context, self.sources, self.projection)
    }
}

impl RustCompilerArtifact {
    pub(super) fn validate_metadata(
        &mut self,
        cancellation: &CancellationProbe,
    ) -> Result<RustCompilerExtractionContext, SignatureContractKitError> {
        cancellation.checkpoint()?;
        if self.schema_version != RUST_COMPILER_ARTIFACT_SCHEMA_VERSION {
            return Err(RustCompilerArtifactFailure::SchemaVersion {
                expected: RUST_COMPILER_ARTIFACT_SCHEMA_VERSION,
                actual: self.schema_version,
            }
            .into());
        }
        if self.rustdoc_format_version != RUSTDOC_FORMAT_VERSION {
            return Err(RustCompilerArtifactFailure::RustdocFormat {
                expected: RUSTDOC_FORMAT_VERSION,
                envelope: self.rustdoc_format_version,
                document: self.rustdoc_format_version,
            }
            .into());
        }
        Self::validate_required_text("extractor_version", &self.extractor_version, cancellation)?;
        Self::validate_required_text("compiler_version", &self.compiler_version, cancellation)?;
        Self::validate_required_text("target_triple", &self.target_triple, cancellation)?;
        if self.crates.len() != 1 {
            return Err(RustCompilerArtifactFailure::InvalidMetadata {
                field: "crates",
                message: format!(
                    "schema version 1 requires exactly one selected local target, found {}",
                    self.crates.len()
                ),
            }
            .into());
        }
        cancellation.checkpoint()?;
        let crate_metadata = &self.crates[0];
        Self::validate_required_text("crates[].id", &crate_metadata.id, cancellation)?;
        let canonical_crate_id = RustCrateId::new(crate_metadata.id.clone(), cancellation)?;
        Self::validate_required_text("crates[].package", &crate_metadata.package, cancellation)?;
        Self::validate_required_text("crates[].target", &crate_metadata.target, cancellation)?;
        if !crate_metadata.root.has_extension("rs") {
            return Err(RustCompilerArtifactFailure::InvalidMetadata {
                field: "crates[].root",
                message: format!("{} is not a Rust source path", crate_metadata.root),
            }
            .into());
        }

        Self::normalize_set("features", &mut self.features, cancellation)?;
        Self::normalize_set("cfg_values", &mut self.cfg_values, cancellation)?;
        let crate_metadata =
            self.crates
                .pop()
                .ok_or_else(|| RustCompilerArtifactFailure::InvalidMetadata {
                    field: "crates",
                    message: "validated crate metadata was not retained".to_owned(),
                })?;
        Ok(RustCompilerExtractionContext {
            artifact_schema_version: self.schema_version,
            extractor_version: std::mem::take(&mut self.extractor_version),
            compiler_version: std::mem::take(&mut self.compiler_version),
            rustdoc_format_version: self.rustdoc_format_version,
            target_triple: std::mem::take(&mut self.target_triple),
            features: std::mem::take(&mut self.features),
            cfg_values: std::mem::take(&mut self.cfg_values),
            crate_metadata,
            canonical_crate_id,
        })
    }

    fn validate_required_text(
        field: &'static str,
        value: &str,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        cancellation.checkpoint()?;
        if value.is_empty() {
            return Err(RustCompilerArtifactFailure::InvalidMetadata {
                field,
                message: "value cannot be empty".to_owned(),
            }
            .into());
        }
        if value.chars().next().is_some_and(char::is_whitespace)
            || value.chars().next_back().is_some_and(char::is_whitespace)
        {
            return Err(RustCompilerArtifactFailure::InvalidMetadata {
                field,
                message: "value cannot have surrounding whitespace".to_owned(),
            }
            .into());
        }
        for character in value.chars() {
            cancellation.checkpoint()?;
            if character.is_control() {
                return Err(RustCompilerArtifactFailure::InvalidMetadata {
                    field,
                    message: "value cannot contain control characters".to_owned(),
                }
                .into());
            }
        }
        Ok(())
    }

    fn normalize_set(
        field: &'static str,
        values: &mut Vec<String>,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        let mut normalized = BTreeSet::new();
        for value in values.drain(..) {
            Self::validate_required_text(field, &value, cancellation)?;
            normalized.insert(value);
        }
        for value in normalized {
            cancellation.checkpoint()?;
            values.push(value);
        }
        Ok(())
    }

    pub(super) fn validate_document(
        &self,
        context: &RustCompilerExtractionContext,
        document: &rustdoc_types::Crate,
        limits: &RustExtractionLimits,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        cancellation.checkpoint()?;
        if document.format_version != RUSTDOC_FORMAT_VERSION
            || document.format_version != self.rustdoc_format_version
        {
            return Err(RustCompilerArtifactFailure::RustdocFormat {
                expected: RUSTDOC_FORMAT_VERSION,
                envelope: self.rustdoc_format_version,
                document: document.format_version,
            }
            .into());
        }
        if document.target.triple != context.target_triple {
            return Err(RustCompilerArtifactFailure::TargetMismatch {
                envelope: context.target_triple.clone(),
                document: document.target.triple.clone(),
            }
            .into());
        }
        let expected_includes_private =
            matches!(context.crate_metadata.kind, RustCrateKind::Binary);
        if document.includes_private != expected_includes_private {
            return Err(RustCompilerArtifactFailure::InvalidMetadata {
                field: "rustdoc_json.includes_private",
                message: format!(
                    "selected {:?} target requires includes_private={expected_includes_private}, found {}",
                    context.crate_metadata.kind,
                    document.includes_private,
                ),
            }
            .into());
        }
        if document.root.0 != context.crate_metadata.root_item_id {
            return Err(RustCompilerArtifactFailure::InvalidMetadata {
                field: "crates[].root_item_id",
                message: format!(
                    "artifact root {} does not match rustdoc root {}",
                    context.crate_metadata.root_item_id, document.root.0
                ),
            }
            .into());
        }
        if !document.index.contains_key(&document.root) {
            return Err(RustCompilerArtifactFailure::invalid_item(
                document.root.0,
                "rustdoc root is absent from the item index",
            ));
        }
        for (key, item) in &document.index {
            cancellation.checkpoint()?;
            if key != &item.id {
                return Err(RustCompilerArtifactFailure::invalid_item(
                    key.0,
                    format!(
                        "rustdoc index key {} contradicts item payload {}",
                        key.0, item.id.0
                    ),
                ));
            }
        }
        let node_count = document
            .index
            .len()
            .saturating_add(document.paths.len())
            .saturating_add(self.source_paths.len());
        limits.validate_compiler_nodes(node_count)?;
        cancellation.checkpoint()?;
        Ok(())
    }

    pub(super) fn validate_source_map(
        &mut self,
        context: &RustCompilerExtractionContext,
        document: &rustdoc_types::Crate,
        allowed_files: &BTreeSet<CatalogPath>,
        source_files: &FileCatalog,
        limits: &RustExtractionLimits,
        cancellation: &CancellationProbe,
    ) -> Result<BTreeMap<u32, CompilerSourcePath>, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let mut mappings = BTreeMap::new();
        let mut source_index =
            CompilerSourceIndex::new(source_files, &self.source_paths, limits, cancellation)?;
        for mapping in std::mem::take(&mut self.source_paths) {
            cancellation.checkpoint()?;
            let item = document
                .index
                .get(&rustdoc_types::Id(mapping.rustdoc_item_id))
                .ok_or_else(|| {
                    RustCompilerArtifactFailure::source_map(
                        Some(mapping.rustdoc_item_id),
                        "mapping references an item absent from the rustdoc index",
                    )
                })?;
            if item.crate_id != 0 {
                return Err(RustCompilerArtifactFailure::source_map(
                    Some(mapping.rustdoc_item_id),
                    "source mappings may identify only the selected local crate",
                ));
            }
            if !allowed_files.contains(mapping.provenance.file()) {
                return Err(RustCompilerArtifactFailure::source_map(
                    Some(mapping.rustdoc_item_id),
                    format!(
                        "source provenance identifies unlisted source {}",
                        mapping.provenance.file()
                    ),
                ));
            }
            mapping.provenance.validate_item_span(
                mapping.rustdoc_item_id,
                item.span.as_ref(),
                &mut source_index,
                cancellation,
            )?;
            match &mapping.provenance {
                CompilerSourceProvenance::Exact { file, .. } => {
                    if !file.has_extension("rs") {
                        return Err(RustCompilerArtifactFailure::source_map(
                            Some(mapping.rustdoc_item_id),
                            format!("{file} is not a Rust source path"),
                        ));
                    }
                }
                CompilerSourceProvenance::CompilerGenerated { crate_root } => {
                    if crate_root != &context.crate_metadata.root {
                        return Err(RustCompilerArtifactFailure::source_map(
                            Some(mapping.rustdoc_item_id),
                            format!(
                                "compiler-generated provenance must use selected crate root {}, found {crate_root}",
                                context.crate_metadata.root
                            ),
                        ));
                    }
                }
            }
            let item_id = mapping.rustdoc_item_id;
            if mappings.insert(item_id, mapping).is_some() {
                return Err(RustCompilerArtifactFailure::source_map(
                    Some(item_id),
                    "duplicate item mapping",
                ));
            }
        }
        Ok(mappings)
    }

    pub(super) fn parse_sources(
        &self,
        context: &RustCompilerExtractionContext,
        mappings: &BTreeMap<u32, CompilerSourcePath>,
        source_files: FileCatalog,
        limits: &RustExtractionLimits,
        cancellation: &CancellationProbe,
    ) -> Result<RustSourceCatalog, SignatureContractKitError> {
        let mut allowlist = BTreeSet::new();
        for mapping in mappings.values() {
            cancellation.checkpoint()?;
            allowlist.insert(mapping.provenance.file().clone());
        }
        allowlist.insert(context.crate_metadata.root.clone());
        let mut sources = RustSourceCatalog::deferred(&allowlist, source_files, cancellation)?;
        sources.load_syntax(&context.crate_metadata.root, limits, cancellation)?;
        Ok(sources)
    }
}
