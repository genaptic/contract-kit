use crate::error::SignatureContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::inventory::{
    InventoryChangeCategory, InventoryComparison, InventoryDiagnostic, InventoryDiff,
    InventoryDiffEntry, SignatureDigest,
};
use crate::languages::SignatureParser;
use crate::languages::rust::rustdoc::RustCompilerArtifact;
use crate::limits::SignatureLimits;
use crate::work::{AsyncWorkPool, CancellationProbe, WorkOptions};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Public handle for signature contract operations.
///
/// Build a kit with [`SignatureContractKit::builder`], then call the async
/// methods with in-memory catalog requests. Each handle either owns a local
/// Rayon pool or reuses a caller-supplied shared pool for complete CPU-bound
/// workflows; the returned futures remain independent of any particular async
/// runtime. Worker threads, active root operations, and pending admission are
/// configured independently. See [`WorkOptions`] for the complete admission,
/// cancellation, and scheduling contract.
///
/// Rust contracts use mandatory v2 extraction. Each signature-bearing
/// document records `rust_syntax_v2` or `rust_compiler_v1`, `rust_api_v1`, an
/// exact source allowlist, and explicit crate IDs, root paths, and target kinds.
/// Compiler mode additionally records the validated compiler/rustdoc schema,
/// target, features, cfg, and Cargo package/target identity supplied by the
/// host-produced artifact.
///
/// # Examples
///
/// ```
/// use conkit_signature::SignatureContractKit;
///
/// let kit = SignatureContractKit::builder().build()?;
/// # let _ = kit;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// An operation placed in a spawned task must own its request and kit. A
/// runtime may spawn the owning `async move` future below; this executor-neutral
/// example polls it directly.
///
/// ```
/// use conkit_signature::{
///     CatalogPath, ContractScope, FileCatalog, GenerateDocument, GenerateRequest,
///     GenerateTarget, RustCrateKind, RustCrateRoot, SignatureContractKit,
/// };
/// use std::sync::Arc;
///
/// let kit = Arc::new(SignatureContractKit::builder().build()?);
/// let mut source_files = FileCatalog::new();
/// source_files.insert(
///     CatalogPath::new("lib.rs")?,
///     b"pub fn answer() -> u8 { 42 }\n".to_vec(),
/// )?;
/// let request = GenerateRequest {
///     extraction: conkit_signature::RustExtractionInput::Syntax,
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
///     scope: ContractScope::Signatures,
/// };
/// let task_kit = Arc::clone(&kit);
/// let task = async move { task_kit.generate(request).await };
/// let response = futures_executor::block_on(task)?;
///
/// assert_eq!(response.counts.signature_count, 1);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct SignatureContractKit {
    parser: Arc<SignatureParser>,
    work: AsyncWorkPool,
}

#[derive(Default)]
/// Builder for [`SignatureContractKit`].
///
/// The default builder uses [`WorkerPool::RuntimeDefault`](crate::WorkerPool::RuntimeDefault),
/// one active root operation, and no pending queue. Use
/// [`SignatureContractKitBuilder::with_work_options`] to configure worker
/// ownership, active operations, and pending admission independently. See
/// [`WorkOptions`] for the complete scheduling contract.
///
/// # Examples
///
/// ```
/// use conkit_signature::{SignatureContractKitBuilder, WorkOptions, WorkerPool};
/// use std::num::NonZeroUsize;
///
/// let kit = SignatureContractKitBuilder::default()
///     .with_work_options(WorkOptions {
///         pool: WorkerPool::Dedicated {
///             worker_threads: NonZeroUsize::MIN,
///         },
///         max_in_flight_operations: NonZeroUsize::MIN,
///         max_pending_operations: 8,
///     })
///     .build()?;
/// # let _ = kit;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct SignatureContractKitBuilder {
    work: WorkOptions,
    limits: SignatureLimits,
}

impl SignatureContractKitBuilder {
    /// Configures CPU work scheduling for the kit.
    ///
    /// This replaces the builder's current [`WorkOptions`]. Worker ownership,
    /// active root operations, and pending admission are independent;
    /// [`WorkOptions`] documents runtime independence, cancellation, and
    /// scheduling guarantees.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_signature::{SignatureContractKitBuilder, WorkOptions, WorkerPool};
    /// use std::num::NonZeroUsize;
    ///
    /// let builder = SignatureContractKitBuilder::default().with_work_options(WorkOptions {
    ///     pool: WorkerPool::Dedicated {
    ///         worker_threads: NonZeroUsize::MIN,
    ///     },
    ///     max_in_flight_operations: NonZeroUsize::MIN,
    ///     max_pending_operations: 8,
    /// });
    /// let kit = builder.build()?;
    /// # let _ = kit;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_work_options(mut self, work: WorkOptions) -> Self {
        self.work = work;
        self
    }

    /// Replaces the resource budgets enforced for every operation.
    ///
    /// The supplied [`SignatureLimits`] replaces the complete default limit
    /// set rather than merging individual fields. Limit failures are returned
    /// as [`SignatureContractKitError`] and expose typed evidence through
    /// [`SignatureContractKitError::limit_exceeded`].
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_signature::{
    ///     CatalogLimits, CatalogPath, CheckMode, CheckRequest, FileCatalog,
    ///     LimitResource, ReportRequest, SignatureContractKit, SignatureLimits,
    /// };
    ///
    /// let limits = SignatureLimits {
    ///     catalog: CatalogLimits {
    ///         entry_count: 0,
    ///         ..CatalogLimits::default()
    ///     },
    ///     ..SignatureLimits::default()
    /// };
    /// let kit = SignatureContractKit::builder().with_limits(limits).build()?;
    /// let mut source_files = FileCatalog::new();
    /// source_files.insert(CatalogPath::new("lib.rs")?, Vec::new())?;
    ///
    /// let error = futures_executor::block_on(kit.check(CheckRequest {
    ///     source_files,
    ///     contract_files: FileCatalog::new(),
    ///     extraction: Default::default(),
    ///     report: ReportRequest::None,
    ///     mode: CheckMode::Default,
    /// }))
    /// .unwrap_err();
    /// let evidence = error.limit_exceeded().expect("typed limit evidence");
    /// assert_eq!(evidence.resource, LimitResource::CatalogEntryCount);
    /// assert_eq!(evidence.limit, 0);
    /// assert_eq!(evidence.observed_at_least, 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_limits(mut self, limits: SignatureLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Builds a local signature contract kit.
    ///
    /// # Errors
    ///
    /// Returns [`SignatureContractKitError`] if a local Rayon pool cannot be
    /// initialized or if active plus pending operation capacity overflows
    /// `usize`. A caller-supplied [`WorkerPool::Shared`](crate::WorkerPool::Shared)
    /// is reused but still receives independent per-kit admission limits.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_signature::SignatureContractKitBuilder;
    ///
    /// let kit = SignatureContractKitBuilder::default().build()?;
    /// # let _ = kit;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn build(self) -> Result<SignatureContractKit, SignatureContractKitError> {
        Ok(SignatureContractKit {
            parser: Arc::new(SignatureParser::new(self.limits)),
            work: AsyncWorkPool::new(self.work)?,
        })
    }
}

impl SignatureContractKit {
    /// Starts configuring a [`SignatureContractKit`].
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_signature::SignatureContractKit;
    ///
    /// let kit = SignatureContractKit::builder().build()?;
    /// # let _ = kit;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn builder() -> SignatureContractKitBuilder {
        SignatureContractKitBuilder::default()
    }

    /// Checks source files against contract files.
    ///
    /// Rust contract files are interpreted as mandatory `contract_version: 2`
    /// combined documents with `rust_syntax_v2` or `rust_compiler_v1`
    /// extraction metadata, `profile: rust_api_v1`, explicit crate roots, an
    /// exact `files` allowlist, signatures, and nested sketches. The request's
    /// [`RustExtractionInput`] must match every signature-bearing document;
    /// there is no syntax/compiler fallback. Compiler extraction requires
    /// exactly one such document and exact agreement between its recorded
    /// extraction context and the supplied host artifact. Rust decoding,
    /// graph traversal, owner resolution, and inventory construction are
    /// bounded by each document's allowlist. Callers decide how those bytes
    /// are read from or written to local files.
    ///
    /// Syntax extraction retains modeled declarations and emits capability
    /// warnings for compiler-dependent facts such as `cfg`, macro expansion,
    /// and reexport resolution. [`CheckMode::Default`] permits warning-only
    /// results, [`CheckMode::Strict`] requires no diagnostics, and
    /// [`CheckMode::Warning`] preserves all diagnostics while passing a
    /// completed check. Unsupported reachable syntax fails closed.
    ///
    /// # Errors
    ///
    /// Returns [`SignatureContractKitError`] when source or contract parsing,
    /// extraction, comparison, or optional report rendering fails. This
    /// includes an extraction-mode mismatch, a missing or non-unique compiler
    /// document, invalid compiler artifact or artifact/document disagreement,
    /// a configured resource limit, immediate work-queue saturation,
    /// cancellation, or worker completion failure.
    ///
    /// # Panics
    ///
    /// If the background operation panics on a Rayon worker, its panic payload
    /// resumes unwinding on the thread that polls this future to completion.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_signature::{
    ///     CatalogPath, CheckMode, CheckRequest, ContractScope, FileCatalog,
    ///     GenerateDocument, GenerateRequest, GenerateTarget, ReportRequest, RustCrateKind,
    ///     RustCrateRoot, SignatureContractKit,
    /// };
    ///
    /// fn catalog_with(path: &str, bytes: &[u8]) -> Result<FileCatalog, Box<dyn std::error::Error>> {
    ///     let mut catalog = FileCatalog::new();
    ///     catalog.insert(CatalogPath::new(path)?, bytes.to_vec())?;
    ///     Ok(catalog)
    /// }
    ///
    /// let kit = SignatureContractKit::builder().build()?;
    /// let source_files = catalog_with("lib.rs", b"pub fn answer() -> u8 { 42 }\n")?;
    /// let generated = futures_executor::block_on(kit.generate(GenerateRequest {
    ///     extraction: conkit_signature::RustExtractionInput::Syntax,
    ///     source_files: source_files.clone(),
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
    ///     scope: ContractScope::Signatures,
    /// }))?;
    ///
    /// let response = futures_executor::block_on(kit.check(CheckRequest {
    ///     extraction: conkit_signature::RustExtractionInput::Syntax,
    ///     source_files,
    ///     contract_files: generated.contract_files,
    ///     report: ReportRequest::None,
    ///     mode: CheckMode::Default,
    /// }))?;
    ///
    /// assert!(response.passed);
    /// assert!(response.diagnostics.is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn check(
        &self,
        request: CheckRequest,
    ) -> Result<CheckResponse, SignatureContractKitError> {
        let parser = Arc::clone(&self.parser);

        self.work
            .execute(move |cancellation| SignatureCheck::new(parser, cancellation, request).run())
            .await?
    }

    /// Generates contract files from source files.
    ///
    /// Rust source entries produce combined user-named YAML contract documents.
    /// A new document requires explicit [`RustCrateRoot`] values; target kind
    /// and logical module identity are not inferred from `lib.rs`, `main.rs`, or
    /// any other filename. Existing documents retain their recorded extraction
    /// context and must match the requested extraction mode. For an existing
    /// document, a semantic no-op returns the original bytes exactly. A real
    /// change edits only affected signature nodes, preserves retained source
    /// spans, comments, presentation, and sketches, and is semantically
    /// reparsed before its bytes are returned.
    ///
    /// # Errors
    ///
    /// Returns [`SignatureContractKitError`] when Rust source decoding,
    /// extraction, ownership resolution, or contract conversion fails; when a
    /// new or existing document layout is invalid; when the requested
    /// extraction mode disagrees with an existing document; when compiler mode
    /// lacks exactly one selected crate or its artifact does not agree with the
    /// document; when lossless editing, verification reparsing, label
    /// allocation, or output rendering fails; when a configured resource limit
    /// is exceeded; or when work is rejected, canceled, or does not complete.
    ///
    /// # Panics
    ///
    /// If the background operation panics on a Rayon worker, its panic payload
    /// resumes unwinding on the thread that polls this future to completion.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_signature::{CatalogPath, ContractScope, FileCatalog, GenerateDocument, GenerateRequest, GenerateTarget, RustCrateKind, RustCrateRoot, SignatureContractKit};
    ///
    /// let kit = SignatureContractKit::builder().build()?;
    /// let mut source_files = FileCatalog::new();
    /// source_files.insert(
    ///     CatalogPath::new("lib.rs")?,
    ///     b"pub fn answer() -> u8 { 42 }\n".to_vec(),
    /// )?;
    ///
    /// let response = futures_executor::block_on(kit.generate(GenerateRequest {
    ///     extraction: conkit_signature::RustExtractionInput::Syntax,
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
    ///     scope: ContractScope::Signatures,
    /// }))?;
    ///
    /// assert_eq!(response.counts.signature_count, 1);
    /// assert_eq!(response.counts.preserved_sketch_count, 0);
    /// assert!(response.contract_files.get(&CatalogPath::new("main.yml")?).is_some());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn generate(
        &self,
        request: GenerateRequest,
    ) -> Result<GenerateResponse, SignatureContractKitError> {
        let parser = Arc::clone(&self.parser);

        self.work
            .execute(move |cancellation| {
                let mut catalog_usage = parser.limits().catalog.usage();
                cancellation.checkpoint()?;
                catalog_usage.record(&request.source_files, &cancellation)?;
                cancellation.checkpoint()?;
                if let GenerateTarget::Existing(contract_files) = &request.target {
                    catalog_usage.record(contract_files, &cancellation)?;
                    cancellation.checkpoint()?;
                }
                let GenerateRequest {
                    source_files,
                    target,
                    extraction,
                    scope,
                } = request;
                parser.generate_contract_files(
                    source_files,
                    target,
                    extraction,
                    scope,
                    &cancellation,
                )
            })
            .await?
    }

    /// Resolves every explicitly linked sketch to its exact Rust source item.
    ///
    /// Structurally repeatable items with the same crate, module path, item
    /// kind, and semantic name are distinguished by one-based declaration
    /// occurrence. Contract signature order reconstructs those occurrences, so
    /// each link selects the exact corresponding item text. A compiler-created
    /// item remains valid signature identity, but its generated provenance
    /// cannot satisfy a linked sketch that requires exact source bytes.
    ///
    /// # Errors
    ///
    /// Returns [`SignatureContractKitError`] when Rust source bytes cannot be
    /// decoded or extracted; when a participating combined document fails
    /// YAML, layout, ownership, link, or extraction-mode validation; when a
    /// compiler artifact is invalid or disagrees with the one required
    /// document; when linked source files, exact Rust items, or valid exact
    /// source spans cannot be found (including compiler-generated provenance);
    /// when a configured resource limit is exceeded; or when work is rejected,
    /// canceled, or does not complete.
    ///
    /// # Panics
    ///
    /// If the background operation panics on a Rayon worker, its panic payload
    /// resumes unwinding on the thread that polls this future to completion.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_signature::{
    ///     CatalogPath, FileCatalog, ResolveSketchesRequest, SignatureContractKit,
    /// };
    ///
    /// fn catalog(path: &str, bytes: &[u8]) -> Result<FileCatalog, Box<dyn std::error::Error>> {
    ///     let mut files = FileCatalog::new();
    ///     files.insert(CatalogPath::new(path)?, bytes.to_vec())?;
    ///     Ok(files)
    /// }
    ///
    /// let kit = SignatureContractKit::builder().build()?;
    /// let response = futures_executor::block_on(kit.resolve_sketches(
    ///     ResolveSketchesRequest {
    ///         extraction: conkit_signature::RustExtractionInput::Syntax,
    ///         source_files: catalog(
    ///             "lib.rs",
    ///             b"include!(\"first.rs\");\ninclude!(\"second.rs\");\n",
    ///         )?,
    ///         contract_files: catalog(
    ///             "main.yml",
    ///             br#"contract_version: 2
    /// root: ../src
    /// files: [lib.rs]
    /// extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
    /// signatures:
    ///   - first_include:
    ///       file: lib.rs
    ///       signature_type: macro
    ///       name: include
    ///       tokens: 'include ! ("first.rs")'
    ///       sketch: first
    ///   - second_include:
    ///       file: lib.rs
    ///       signature_type: macro
    ///       name: include
    ///       tokens: 'include ! ("second.rs")'
    ///       sketch: second
    /// sketches:
    ///   - first:
    ///       file: lib.rs
    ///       signature: first_include
    ///       signature_type: macro
    ///       matching: { normalization: exact_lines_v1, occurrence: exactly_one }
    ///       code: old
    ///   - second:
    ///       file: lib.rs
    ///       signature: second_include
    ///       signature_type: macro
    ///       matching: { normalization: exact_lines_v1, occurrence: exactly_one }
    ///       code: old
    /// "#,
    ///         )?,
    ///     },
    /// ))?;
    ///
    /// assert_eq!(response.seeds.len(), 2);
    /// assert_eq!(response.seeds[0].contract_file.as_str(), "main.yml");
    /// assert_eq!(response.seeds[0].sketch_id, "first");
    /// assert_eq!(response.seeds[0].signature_type, "macro");
    /// assert_eq!(response.seeds[0].file.as_str(), "lib.rs");
    /// assert_eq!(response.seeds[0].code, "include!(\"first.rs\");");
    /// assert_eq!(response.seeds[1].sketch_id, "second");
    /// assert_eq!(response.seeds[1].code, "include!(\"second.rs\");");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn resolve_sketches(
        &self,
        request: ResolveSketchesRequest,
    ) -> Result<ResolveSketchesResponse, SignatureContractKitError> {
        let parser = Arc::clone(&self.parser);

        self.work
            .execute(move |cancellation| {
                let mut catalog_usage = parser.limits().catalog.usage();
                cancellation.checkpoint()?;
                catalog_usage.record(&request.source_files, &cancellation)?;
                cancellation.checkpoint()?;
                catalog_usage.record(&request.contract_files, &cancellation)?;
                cancellation.checkpoint()?;
                parser.resolve_sketches(request, &cancellation)
            })
            .await?
    }

    /// Diffs current signature contract files against previous contract files.
    ///
    /// Both catalogs are parsed as mandatory-v2 documents. Entries classify
    /// changes to caller-visible source semantics, extraction context, user
    /// labels, and contract-only document metadata. The response digest is the
    /// complete current contract identity; it is not a digest of the previous
    /// catalog or of the diff entries. Repeated semantic labels across physical
    /// documents are paired deterministically by their sorted source and
    /// contract digests before residual additions and removals are reported.
    ///
    /// # Errors
    ///
    /// Returns [`SignatureContractKitError`] when either catalog contains an
    /// invalid contract document, when inventory or deterministic duplicate-
    /// label pairing fails, when diagnostic/diff or other configured limits
    /// are exceeded, or when work is rejected, canceled, or does not complete.
    ///
    /// # Panics
    ///
    /// If the background operation panics on a Rayon worker, its panic payload
    /// resumes unwinding on the thread that polls this future to completion.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_signature::{
    ///     CatalogPath, ContractScope, DiffRequest, FileCatalog, GenerateDocument,
    ///     GenerateRequest, GenerateTarget, RustCrateKind, RustCrateRoot,
    ///     SignatureContractKit,
    /// };
    ///
    /// fn source(path: &str, bytes: &[u8]) -> Result<FileCatalog, Box<dyn std::error::Error>> {
    ///     let mut catalog = FileCatalog::new();
    ///     catalog.insert(CatalogPath::new(path)?, bytes.to_vec())?;
    ///     Ok(catalog)
    /// }
    ///
    /// let kit = SignatureContractKit::builder().build()?;
    /// let contracts = futures_executor::block_on(kit.generate(GenerateRequest {
    ///     extraction: conkit_signature::RustExtractionInput::Syntax,
    ///     source_files: source("lib.rs", b"pub fn answer() {}\n")?,
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
    ///     scope: ContractScope::Signatures,
    /// }))?
    /// .contract_files;
    /// let diff = futures_executor::block_on(kit.diff(DiffRequest {
    ///     current_contract_files: contracts.clone(),
    ///     previous_contract_files: contracts,
    /// }))?;
    ///
    /// assert!(!diff.changed());
    /// assert!(diff.entries.is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn diff(
        &self,
        request: DiffRequest,
    ) -> Result<DiffResponse, SignatureContractKitError> {
        let parser = Arc::clone(&self.parser);

        self.work
            .execute(move |cancellation| SignatureDiff::new(parser, cancellation, request).run())
            .await?
    }
}

struct SignatureCheck {
    parser: Arc<SignatureParser>,
    cancellation: CancellationProbe,
    request: CheckRequest,
}

impl SignatureCheck {
    fn new(
        parser: Arc<SignatureParser>,
        cancellation: CancellationProbe,
        request: CheckRequest,
    ) -> Self {
        Self {
            parser,
            cancellation,
            request,
        }
    }

    fn run(self) -> Result<CheckResponse, SignatureContractKitError> {
        let CheckRequest {
            source_files,
            contract_files,
            extraction,
            report,
            mode,
        } = self.request;
        let limits = self.parser.limits();
        let mut catalog_usage = limits.catalog.usage();
        self.cancellation.checkpoint()?;
        catalog_usage.record(&source_files, &self.cancellation)?;
        self.cancellation.checkpoint()?;
        catalog_usage.record(&contract_files, &self.cancellation)?;
        self.cancellation.checkpoint()?;
        let parsed = self.parser.parse_check_inventories(
            source_files,
            contract_files,
            extraction,
            &self.cancellation,
        )?;
        let comparison = parsed.compare(&limits.diagnostics, &self.cancellation)?;
        let mut response = CheckResponse::from_inventory_comparison(
            comparison,
            parsed.capability_warning_messages(),
            mode,
            &limits.diagnostics,
            &self.cancellation,
        )?;

        response.report_files = report.render(&response, limits, &self.cancellation)?;
        Ok(response)
    }
}

struct SignatureDiff {
    parser: Arc<SignatureParser>,
    cancellation: CancellationProbe,
    request: DiffRequest,
}

impl SignatureDiff {
    fn new(
        parser: Arc<SignatureParser>,
        cancellation: CancellationProbe,
        request: DiffRequest,
    ) -> Self {
        Self {
            parser,
            cancellation,
            request,
        }
    }

    fn run(self) -> Result<DiffResponse, SignatureContractKitError> {
        let DiffRequest {
            current_contract_files,
            previous_contract_files,
        } = self.request;
        let limits = self.parser.limits();
        let mut catalog_usage = limits.catalog.usage();
        self.cancellation.checkpoint()?;
        catalog_usage.record(&current_contract_files, &self.cancellation)?;
        self.cancellation.checkpoint()?;
        catalog_usage.record(&previous_contract_files, &self.cancellation)?;
        self.cancellation.checkpoint()?;
        let (current, previous) = self.parser.parse_contract_diff_inventories(
            current_contract_files,
            previous_contract_files,
            &self.cancellation,
        )?;

        DiffResponse::from_inventory_diff(
            current.diff_against(&previous, &limits.diagnostics, &self.cancellation)?,
            &limits.diagnostics,
            &self.cancellation,
        )
    }
}

impl ReportRequest {
    fn render(
        self,
        response: &CheckResponse,
        limits: &SignatureLimits,
        cancellation: &CancellationProbe,
    ) -> Result<FileCatalog, SignatureContractKitError> {
        cancellation.checkpoint()?;
        match self {
            ReportRequest::None => Ok(FileCatalog::new()),
            ReportRequest::Generate {
                format,
                output_file,
            } => {
                let report = response.report_view();
                let mut output = limits.output.meter(cancellation);
                let bytes = match format {
                    ReportFormat::Yaml => output.serialize_yaml(&output_file, &report)?,
                    ReportFormat::Json => output.serialize_pretty_json(&output_file, &report)?,
                };
                let mut files = FileCatalog::new();
                files.insert(output_file, bytes)?;

                Ok(files)
            }
        }
    }
}

#[derive(Clone, Copy)]
enum CheckReportLayout {
    Standalone,
    Embedded,
}

struct CheckReportView<'response> {
    response: &'response CheckResponse,
    layout: CheckReportLayout,
}

impl<'response> CheckReportView<'response> {
    fn new(response: &'response CheckResponse, layout: CheckReportLayout) -> Self {
        Self { response, layout }
    }
}

impl Serialize for CheckReportView<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct as _;

        let mut report = serializer.serialize_struct("CheckReport", 5)?;
        report.serialize_field("passed", &self.response.passed)?;
        match self.layout {
            CheckReportLayout::Standalone => {
                report
                    .serialize_field("source_shape_digest", &self.response.source_shape_digest)?;
                report.serialize_field("digest_version", &self.response.digest_version)?;
                report.serialize_field("counts", &self.response.counts)?;
            }
            CheckReportLayout::Embedded => {
                report.serialize_field("counts", &self.response.counts)?;
                report
                    .serialize_field("source_shape_digest", &self.response.source_shape_digest)?;
                report.serialize_field("digest_version", &self.response.digest_version)?;
            }
        }
        report.serialize_field("diagnostics", &self.response.diagnostics)?;
        report.end()
    }
}

/// Selects whether signature generation may coordinate linked-sketch cleanup.
///
/// [`ContractScope::Signatures`] preserves every valid sketch link and rejects
/// a signature update that would orphan one. [`ContractScope::All`] allows a
/// stale signature and its linked sketch record to be removed together before
/// the caller asks the sketch domain to refresh surviving links.
///
/// # Examples
///
/// Updating existing documents with signature-only scope rejects removal of a
/// linked signature. All-family scope removes the signature and its linked
/// sketch together.
///
/// ```
/// use conkit_signature::{
///     CatalogPath, ContractScope, FileCatalog, GenerateRequest, GenerateTarget,
///     SignatureContractKit,
/// };
///
/// fn catalog(path: &str, bytes: &[u8]) -> Result<FileCatalog, Box<dyn std::error::Error>> {
///     let mut files = FileCatalog::new();
///     files.insert(CatalogPath::new(path)?, bytes.to_vec())?;
///     Ok(files)
/// }
///
/// let kit = SignatureContractKit::builder().build()?;
/// let source_files = catalog("main.rs", b"// main was removed\n")?;
/// let existing = catalog(
///     "main.yml",
///     br#"contract_version: 2
/// root: ../src
/// files: [main.rs]
/// extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: app, root: main.rs, kind: binary }] }
/// signatures:
///   - main:
///       file: main.rs
///       signature_type: function
///       name: main
///       visibility: private
///       sketch: main
/// sketches:
///   - main:
///       file: main.rs
///       signature: main
///       signature_type: function
///       matching: { normalization: exact_lines_v1, occurrence: exactly_one }
///       code: |
///         fn main() {}
/// "#,
/// )?;
///
/// let signatures_only = futures_executor::block_on(kit.generate(GenerateRequest {
///     extraction: conkit_signature::RustExtractionInput::Syntax,
///     source_files: source_files.clone(),
///     target: GenerateTarget::Existing(existing.clone()),
///     scope: ContractScope::Signatures,
/// }));
/// assert!(signatures_only.unwrap_err().to_string().contains("orphan"));
///
/// let all = futures_executor::block_on(kit.generate(GenerateRequest {
///     extraction: conkit_signature::RustExtractionInput::Syntax,
///     source_files,
///     target: GenerateTarget::Existing(existing),
///     scope: ContractScope::All,
/// }))?;
/// let yaml = std::str::from_utf8(
///     all.contract_files
///         .get(&CatalogPath::new("main.yml")?)
///         .expect("updated contract"),
/// )?;
/// assert_eq!(all.counts.signature_count, 0);
/// assert_eq!(all.counts.preserved_sketch_count, 0);
/// assert!(!yaml.contains("name: main"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ContractScope {
    /// Coordinate signatures with linked records for an all-family workflow.
    ///
    /// Generation returns exact source seeds for every surviving linked sketch
    /// in [`GenerateResponse::resolved_sketch_seeds`]. Those seeds come from
    /// the same parsed source projection used to regenerate the signature
    /// documents, so callers do not need a second extraction request.
    All,
    /// Process signatures while preserving the sketch section.
    Signatures,
}

/// Determines whether diagnostics fail a check response.
///
/// [`CheckMode::Default`] permits syntax-extraction capability warnings but
/// fails source/contract comparison errors. [`CheckMode::Strict`] requires a
/// diagnostic-free response. [`CheckMode::Warning`] preserves every
/// diagnostic while allowing the response to pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CheckMode {
    /// Permit capability warnings but fail comparison errors.
    Default,
    /// Fail the check when any error or capability warning is emitted.
    Strict,
    /// Keep diagnostics but allow the check response to pass.
    Warning,
}

impl CheckMode {
    fn passed(self, diagnostics: &[CheckDiagnostic]) -> bool {
        match self {
            Self::Default => diagnostics
                .iter()
                .all(|diagnostic| diagnostic.severity() == DiagnosticSeverity::Warning),
            Self::Strict => diagnostics.is_empty(),
            Self::Warning => true,
        }
    }
}

/// Severity assigned to one completed signature-check diagnostic.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    /// The source/API contract does not match and enforcing modes must fail.
    Error,
    /// Syntax extraction retained a fact that needs compiler context.
    Warning,
}

/// Semantic category assigned to one signature-check diagnostic.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckDiagnosticCategory {
    /// A caller-visible source/API signature differs from the contract.
    SourceSemantics,
    /// Syntax extraction cannot fully evaluate a retained semantic fact.
    SyntaxCapability,
}

/// Output encoding for generated check reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ReportFormat {
    /// Render the report as YAML bytes.
    Yaml,
    /// Render the report as pretty JSON bytes.
    Json,
}

/// Selects the Rust extraction capability for a source-backed operation.
///
/// Syntax extraction is the portable default and derives signatures from the
/// supplied source catalog plus contract crate-root metadata. Compiler
/// extraction consumes one versioned, host-produced rustdoc artifact; this
/// crate still performs no process, Cargo, or filesystem work. Request fields
/// carrying this enum use its Serde default when omitted, and the tagged wire
/// shape distinguishes `syntax` from `compiler` without an implicit fallback.
///
/// # Examples
///
/// ```
/// use conkit_signature::RustExtractionInput;
///
/// assert_eq!(RustExtractionInput::default(), RustExtractionInput::Syntax);
/// assert_eq!(
///     serde_json::to_value(RustExtractionInput::Syntax)?,
///     serde_json::json!({ "mode": "syntax" }),
/// );
/// assert_eq!(
///     serde_json::from_value::<RustExtractionInput>(
///         serde_json::json!({ "mode": "syntax" }),
///     )?,
///     RustExtractionInput::Syntax,
/// );
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode", content = "artifact")]
pub enum RustExtractionInput {
    /// Use the mandatory-v2, allowlist-bounded syntax extractor.
    #[default]
    Syntax,
    /// Use compiler-resolved public API facts from the supplied artifact.
    Compiler(RustCompilerArtifact),
}

/// Request for [`SignatureContractKit::check`].
///
/// The caller owns filesystem discovery and provides both source and contract
/// bytes as [`FileCatalog`] values. Rust contract files use the mandatory-v2
/// combined shape with `profile: rust_api_v1`, `rust_syntax_v2` or
/// `rust_compiler_v1` extraction metadata, explicit typed crate roots, `root`,
/// an exact `files` allowlist, user-named `signatures`, and nested `sketches`.
/// Missing, legacy, future, unknown, or request-mismatched extraction versions
/// fail closed. Compiler extraction requires exactly one signature-bearing
/// document whose full compiler, Cargo target, crate-root, feature, cfg, and
/// schema context agrees with the supplied artifact. The selected
/// [`CheckMode`] controls whether deterministic syntax-capability warnings fail
/// an otherwise matching syntax check.
///
/// # Examples
///
/// ```
/// use conkit_signature::{
///     CheckMode, CheckRequest, FileCatalog, ReportRequest,
/// };
///
/// let request = CheckRequest {
///     extraction: conkit_signature::RustExtractionInput::Syntax,
///     source_files: FileCatalog::new(),
///     contract_files: FileCatalog::new(),
///     report: ReportRequest::None,
///     mode: CheckMode::Default,
/// };
///
/// assert_eq!(request.mode, CheckMode::Default);
/// let mut wire = serde_json::to_value(&request)?;
/// wire.as_object_mut().expect("request object").remove("extraction");
/// let defaulted: CheckRequest = serde_json::from_value(wire)?;
/// assert_eq!(
///     defaulted.extraction,
///     conkit_signature::RustExtractionInput::Syntax,
/// );
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CheckRequest {
    /// Source files to parse and compare.
    ///
    /// Entries use logical [`CatalogPath`] names such as `src/lib.rs`; this
    /// crate does not traverse directories or read operating-system paths.
    pub source_files: FileCatalog,
    /// Contract files to parse and compare against `source_files`.
    ///
    /// Rust signature contracts must be direct root-level combined YAML
    /// documents. Syntax requests reject compiler documents. Compiler requests
    /// require exactly one signature-bearing compiler document and exact
    /// artifact/document context agreement.
    pub contract_files: FileCatalog,
    /// Rust extraction capability and optional compiler artifact.
    ///
    /// Omission during deserialization selects [`RustExtractionInput::Syntax`].
    #[serde(default)]
    pub extraction: RustExtractionInput,
    /// Optional report output request.
    pub report: ReportRequest,
    /// Diagnostic pass/fail behavior.
    pub mode: CheckMode,
}

/// Optional report output for [`SignatureContractKit::check`].
///
/// # Examples
///
/// ```
/// use conkit_signature::{CatalogPath, ReportFormat, ReportRequest};
///
/// let report = ReportRequest::Generate {
///     format: ReportFormat::Yaml,
///     output_file: CatalogPath::new("reports/check.yaml")?,
/// };
/// assert!(matches!(report, ReportRequest::Generate { .. }));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ReportRequest {
    /// Do not generate a report file.
    None,
    /// Generate one report file in the requested format.
    Generate {
        /// Report byte encoding.
        format: ReportFormat,
        /// Logical catalog path for the report file.
        output_file: CatalogPath,
    },
}

/// Response returned by [`SignatureContractKit::check`].
///
/// [`Self::report_view`] and [`Self::embedded_report_view`] borrow this response
/// and expose the two stable report layouts without copying a second public
/// report DTO. Both omit `report_files`; the embedded view changes field order
/// for composition inside the CLI's mixed-domain report.
///
/// # Examples
///
/// ```
/// use conkit_signature::{
///     CatalogPath, CheckResponse, FileCatalog, SignatureCheckCounts,
/// };
///
/// let mut report_files = FileCatalog::new();
/// report_files.insert(CatalogPath::new("reports/check.json")?, b"stored".to_vec())?;
/// let response = CheckResponse {
///     passed: true,
///     source_shape_digest: "digest".to_owned(),
///     digest_version: 2,
///     counts: SignatureCheckCounts {
///         source_signature_count: 1,
///         contract_signature_count: 1,
///     },
///     diagnostics: Vec::new(),
///     report_files,
/// };
///
/// let standalone = serde_json::to_value(response.report_view())?;
/// let embedded = serde_json::to_value(response.embedded_report_view())?;
/// assert_eq!(standalone["source_shape_digest"], "digest");
/// assert_eq!(embedded["counts"]["source_signature_count"], 1);
/// assert!(standalone.get("report_files").is_none());
/// assert!(embedded.get("report_files").is_none());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CheckResponse {
    /// Whether the check passed under the requested [`CheckMode`].
    pub passed: bool,
    /// Digest for caller-visible source/API shape only.
    ///
    /// This excludes user signature labels, extraction context, document root,
    /// sketch linkage, and other contract-only metadata. Use
    /// [`DiffResponse::contract_digest`] for complete current contract identity.
    pub source_shape_digest: String,
    /// Canonical signature digest format version.
    pub digest_version: u16,
    /// Source and contract signature totals.
    pub counts: SignatureCheckCounts,
    /// Comparison errors followed by deterministic syntax-capability warnings.
    ///
    /// Compiler extraction produces comparison errors only.
    pub diagnostics: Vec<CheckDiagnostic>,
    /// Generated report files, or an empty catalog when no report was requested.
    pub report_files: FileCatalog,
}

impl CheckResponse {
    /// Borrows the standalone check-report payload without `report_files`.
    ///
    /// The returned opaque value implements [`Serialize`] and preserves the
    /// established standalone field order.
    pub fn report_view(&self) -> impl Serialize + '_ {
        CheckReportView::new(self, CheckReportLayout::Standalone)
    }

    /// Borrows the check-report payload used inside a combined CLI report.
    ///
    /// This layout preserves the established embedded field order and omits
    /// `report_files`.
    pub fn embedded_report_view(&self) -> impl Serialize + '_ {
        CheckReportView::new(self, CheckReportLayout::Embedded)
    }

    fn from_inventory_comparison(
        comparison: InventoryComparison,
        capability_warnings: impl IntoIterator<Item = String>,
        mode: CheckMode,
        limits: &crate::limits::DiagnosticLimits,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let mut usage = limits.usage()?;
        let mut diagnostics = Vec::new();
        for diagnostic in comparison.diagnostics() {
            cancellation.checkpoint()?;
            let diagnostic = CheckDiagnostic::from_inventory_diagnostic(diagnostic);
            usage.record(&diagnostic)?;
            diagnostics.push(diagnostic);
        }
        for message in capability_warnings {
            cancellation.checkpoint()?;
            let diagnostic = CheckDiagnostic::Warning { message };
            usage.record(&diagnostic)?;
            diagnostics.push(diagnostic);
        }
        let passed = mode.passed(&diagnostics);

        Ok(Self {
            passed,
            source_shape_digest: comparison.source_shape_digest().render(),
            digest_version: SignatureDigest::VERSION,
            counts: SignatureCheckCounts {
                source_signature_count: comparison.source_signature_count(),
                contract_signature_count: comparison.contract_signature_count(),
            },
            diagnostics,
            report_files: FileCatalog::new(),
        })
    }
}

/// Signature totals reported by [`CheckResponse`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SignatureCheckCounts {
    /// Number of top-level signature groups parsed from source files.
    pub source_signature_count: usize,
    /// Number of top-level signature groups parsed from contract files.
    pub contract_signature_count: usize,
}

/// A diagnostic emitted while checking actual source signatures against
/// expected contract signatures.
///
/// `Missing` identifies an expected contract signature absent from source,
/// while `Extra` identifies an unexpected source signature absent from the
/// contract.
///
/// # Examples
///
/// ```
/// use conkit_signature::{
///     CatalogPath, CheckDiagnostic, CheckDiagnosticCategory, CheckMode, CheckRequest,
///     ContractScope, DiagnosticSeverity, FileCatalog, GenerateDocument,
///     GenerateRequest, GenerateTarget, ReportRequest, RustCrateKind, RustCrateRoot,
///     SignatureContractKit,
/// };
///
/// let kit = SignatureContractKit::builder().build()?;
/// let mut expected_source = FileCatalog::new();
/// expected_source.insert(
///     CatalogPath::new("lib.rs")?,
///     b"pub fn expected_only() {}\n".to_vec(),
/// )?;
/// let expected_contracts = futures_executor::block_on(kit.generate(GenerateRequest {
///     extraction: conkit_signature::RustExtractionInput::Syntax,
///     source_files: expected_source,
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
///     scope: ContractScope::Signatures,
/// }))?
/// .contract_files;
/// let mut actual_source = FileCatalog::new();
/// actual_source.insert(
///     CatalogPath::new("lib.rs")?,
///     b"pub fn actual_only() {}\n".to_vec(),
/// )?;
///
/// let response = futures_executor::block_on(kit.check(CheckRequest {
///     extraction: conkit_signature::RustExtractionInput::Syntax,
///     source_files: actual_source,
///     contract_files: expected_contracts,
///     report: ReportRequest::None,
///     mode: CheckMode::Strict,
/// }))?;
///
/// assert!(response.diagnostics.iter().any(|diagnostic| matches!(
///     diagnostic,
///     CheckDiagnostic::Missing { signature_id } if signature_id.contains("expected_only")
/// )));
/// assert!(response.diagnostics.iter().any(|diagnostic| matches!(
///     diagnostic,
///     CheckDiagnostic::Extra { signature_id } if signature_id.contains("actual_only")
/// )));
/// let missing = response
///     .diagnostics
///     .iter()
///     .find(|diagnostic| matches!(diagnostic, CheckDiagnostic::Missing { .. }))
///     .expect("missing diagnostic");
/// assert_eq!(missing.severity(), DiagnosticSeverity::Error);
/// assert_eq!(missing.category(), CheckDiagnosticCategory::SourceSemantics);
///
/// let warning = CheckDiagnostic::Warning {
///     message: "syntax extraction retained cfg".to_owned(),
/// };
/// assert_eq!(warning.severity(), DiagnosticSeverity::Warning);
/// assert_eq!(warning.category(), CheckDiagnosticCategory::SyntaxCapability);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CheckDiagnostic {
    /// An expected contract signature is absent from source files.
    Missing {
        /// Opaque signature identifier.
        signature_id: String,
    },
    /// An unexpected source signature is absent from contract files.
    Extra {
        /// Opaque signature identifier.
        signature_id: String,
    },
    /// A signature exists in both catalogs but has different digest bytes.
    Mismatched {
        /// Opaque signature identifier.
        signature_id: String,
        /// Digest from the contract inventory.
        expected_digest: String,
        /// Digest from the source inventory.
        actual_digest: String,
    },
    /// Syntax-extraction capability evidence retained by `rust_syntax_v2`.
    ///
    /// The warning explains a semantic fact that syntax extraction preserves
    /// but cannot evaluate, such as conditional compilation, macro expansion,
    /// or a reexport target. [`CheckMode`] determines whether the retained
    /// warning fails the response. Unsupported reachable syntax is an operation
    /// error rather than a warning or silently omitted declaration.
    Warning {
        /// Warning text.
        message: String,
    },
}

impl CheckDiagnostic {
    /// Returns the pass/fail severity used by [`CheckMode`].
    pub fn severity(&self) -> DiagnosticSeverity {
        match self {
            Self::Missing { .. } | Self::Extra { .. } | Self::Mismatched { .. } => {
                DiagnosticSeverity::Error
            }
            Self::Warning { .. } => DiagnosticSeverity::Warning,
        }
    }

    /// Returns the semantic boundary that produced this diagnostic.
    pub fn category(&self) -> CheckDiagnosticCategory {
        match self {
            Self::Missing { .. } | Self::Extra { .. } | Self::Mismatched { .. } => {
                CheckDiagnosticCategory::SourceSemantics
            }
            Self::Warning { .. } => CheckDiagnosticCategory::SyntaxCapability,
        }
    }

    fn from_inventory_diagnostic(diagnostic: &InventoryDiagnostic) -> Self {
        match diagnostic {
            InventoryDiagnostic::Missing { signature_id } => Self::Missing {
                signature_id: signature_id.as_str().to_owned(),
            },
            InventoryDiagnostic::Extra { signature_id } => Self::Extra {
                signature_id: signature_id.as_str().to_owned(),
            },
            InventoryDiagnostic::Mismatched {
                signature_id,
                expected_digest,
                actual_digest,
            } => Self::Mismatched {
                signature_id: signature_id.as_str().to_owned(),
                expected_digest: expected_digest.render(),
                actual_digest: actual_digest.render(),
            },
        }
    }
}

/// Request for [`SignatureContractKit::generate`].
///
/// Generation parses source bytes from a [`FileCatalog`] and returns generated
/// contract bytes in [`GenerateResponse`]. The crate does not write generated
/// files to disk.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GenerateRequest {
    /// Source files to parse into contract files.
    ///
    /// Entries use logical [`CatalogPath`] names such as `src/lib.rs`; the
    /// target determines which combined document owns each source entry.
    pub source_files: FileCatalog,
    /// Existing combined documents to update or a typed layout for a new one.
    pub target: GenerateTarget,
    /// Rust extraction capability and optional compiler artifact.
    ///
    /// Omission during deserialization selects [`RustExtractionInput::Syntax`].
    #[serde(default)]
    pub extraction: RustExtractionInput,
    /// Linked-sketch cleanup policy for [`GenerateTarget::Existing`].
    ///
    /// This has no effect on [`GenerateTarget::New`].
    pub scope: ContractScope,
}

/// Selects whether signature generation creates or updates combined documents.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum GenerateTarget {
    /// Update existing combined contract documents and retain stable labels for
    /// structurally unchanged Rust items.
    ///
    /// Repeatable items with the same crate, module path, item kind, and
    /// semantic name retain labels by one-based declaration occurrence, even
    /// when their retained syntax changes.
    ///
    /// Only direct root-level `.yml` and `.yaml` entries are considered and
    /// returned. Nested YAML and non-YAML catalog entries are ignored and
    /// omitted from [`GenerateResponse::contract_files`]. The complete typed
    /// proposal is compared before a lossless tree is loaded: an exact
    /// semantic no-op returns the original bytes, while a real edit preserves
    /// untouched byte spans, comments, presentation, and the sketch section,
    /// then reparses to verify the proposed semantics. With
    /// [`ContractScope::Signatures`], generation preserves linked sketches and
    /// rejects signature removal that would orphan one. With
    /// [`ContractScope::All`], it removes the stale linked sketch record along
    /// with the signature.
    Existing(FileCatalog),
    /// Create one new combined contract document with the requested layout.
    New(GenerateDocument),
}

/// Layout used when creating a combined contract document.
///
/// New signature documents always use `contract_version: 2` and
/// `profile: rust_api_v1`. [`GenerateRequest::extraction`] selects the closed
/// `rust_syntax_v2` or `rust_compiler_v1` mode. The caller supplies every
/// logical crate root explicitly; compiler artifacts must agree with that
/// identity, and there is no filename-based root, target-kind, or module
/// inference at this domain boundary.
///
/// The output path must be a direct root-level `.yml` or `.yaml` path, `root`
/// must contain non-whitespace text, and `files` must be a nonempty exact
/// allowlist of logical `.rs` paths. Crate IDs must be valid, nonempty, and
/// unique; every crate root must occur in the allowlist. Input file and crate
/// order is nonsemantic and generation canonicalizes it. Compiler generation
/// additionally requires exactly one selected crate target matching the
/// artifact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GenerateDocument {
    /// Logical catalog path for the generated YAML document.
    pub contract_file: CatalogPath,
    /// Nonblank user-facing source root written to the document.
    pub root: String,
    /// Exact portable Rust file allowlist owned by the document.
    pub files: Vec<CatalogPath>,
    /// Explicit Rust crate roots interpreted within the document's allowlist.
    ///
    /// Crate IDs must be unique. Distinct crate IDs may intentionally project
    /// the same physical root into separate logical crate contexts. Every root
    /// must name an allowlisted `.rs` entry and records its target kind
    /// independently of its filename. Input order is not semantic; generation
    /// orders roots deterministically before parser dispatch.
    pub crates: Vec<RustCrateRoot>,
}

/// One explicit Rust crate root used by Rust extraction.
///
/// This identity is the root of one allowlist-bounded logical module graph.
/// The source path may be conventional or nonconventional; [`RustCrateKind`]
/// never derives from the basename. Syntax extraction accepts one or more
/// unique roots. Compiler extraction requires exactly one root, and its ID,
/// logical path, and kind must equal the selected artifact target.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct RustCrateRoot {
    /// Stable crate identity within one contract document.
    pub id: String,
    /// Allowlisted logical Rust source path containing the crate root.
    pub root: CatalogPath,
    /// Cargo-style target kind used to interpret the root.
    pub kind: RustCrateKind,
}

/// Target kind for an explicitly selected Rust crate root.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RustCrateKind {
    /// Interpret the explicit root as a library target.
    Library,
    /// Interpret the explicit root as a binary target.
    Binary,
}

/// Response returned by [`SignatureContractKit::generate`].
///
/// Rust signature generation emits mandatory-v2 combined YAML with the
/// selected versioned extraction metadata, explicit typed crate roots, `root`,
/// an exact `files` allowlist, user-named `signatures`, and nested sketches that
/// retain file/signature/type linkage and matching policy. Existing semantic
/// no-ops retain their input bytes exactly; edited documents are losslessly
/// patched and semantically verified before return.
///
/// # Examples
///
/// ```
/// use conkit_signature::{CatalogPath, ContractScope, FileCatalog, GenerateDocument, GenerateRequest, GenerateTarget, RustCrateKind, RustCrateRoot, SignatureContractKit};
///
/// let kit = SignatureContractKit::builder().build()?;
/// let mut source_files = FileCatalog::new();
/// source_files.insert(
///     CatalogPath::new("lib.rs")?,
///     b"pub unsafe extern \"C\" fn c_api(value: i32) -> i32 { value }\n".to_vec(),
/// )?;
///
/// let response = futures_executor::block_on(kit.generate(GenerateRequest {
///     extraction: conkit_signature::RustExtractionInput::Syntax,
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
///     scope: ContractScope::Signatures,
/// }))?;
/// let contract_path = CatalogPath::new("main.yml")?;
/// let contract_yaml = std::str::from_utf8(
///     response.contract_files.get(&contract_path).expect("generated contract"),
/// )?;
///
/// assert!(contract_yaml.contains("root: ../src"));
/// assert!(contract_yaml.contains("files:"));
/// assert!(contract_yaml.contains("signature_type: function"));
/// assert!(contract_yaml.contains("qualifiers:"));
/// assert!(contract_yaml.contains("abi: C"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GenerateResponse {
    /// Generated contract files.
    ///
    /// Rust signature entries are combined YAML catalog bytes.
    pub contract_files: FileCatalog,
    /// Precise totals for documents validated and bytes returned.
    pub counts: SignatureGenerationCounts,
    /// Exact source seeds for surviving linked sketches in an all-family run.
    ///
    /// This is populated only for [`ContractScope::All`]. Signature-only and
    /// new-document generation return an empty vector. Sketch-only callers can
    /// continue to use [`SignatureContractKit::resolve_sketches`].
    pub resolved_sketch_seeds: Vec<ResolvedSketchSeed>,
    /// Bounded, deterministic warnings for syntax facts requiring compiler context.
    ///
    /// Compiler extraction returns an empty vector. Syntax extraction retains
    /// these warnings instead of silently discarding them during generation.
    pub capability_warnings: Vec<String>,
}

/// Precise generation totals for [`GenerateResponse`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SignatureGenerationCounts {
    /// Number of semantic YAML documents validated or newly generated.
    pub document_count: usize,
    /// Number of top-level grouped signature records in returned documents.
    pub signature_count: usize,
    /// Number of linked sketch records preserved in returned documents.
    ///
    /// The `conkit-signature` crate does not generate sketch contract code.
    /// All-scope generation returns exact source seeds for these surviving
    /// links, and the caller delegates their refresh to the sketch domain.
    pub preserved_sketch_count: usize,
    /// Number of documents whose proposed semantic model differs from its input.
    ///
    /// Every newly generated document contributes to this count.
    pub semantically_changed_document_count: usize,
    /// Number of documents whose returned source bytes differ from their input.
    ///
    /// Every newly generated document contributes to this count. Existing
    /// semantic no-ops retain their original bytes and do not contribute.
    pub byte_changed_document_count: usize,
}

/// Request for [`SignatureContractKit::resolve_sketches`].
///
/// The selected extraction mode must match every signature-bearing document.
/// Compiler mode requires one document that exactly agrees with the supplied
/// artifact. It can resolve only items with validated exact provenance;
/// compiler-generated public items have no source text to return.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolveSketchesRequest {
    /// Rust source bytes indexed by portable source-relative paths.
    pub source_files: FileCatalog,
    /// Combined root-level YAML documents containing links and sketches.
    pub contract_files: FileCatalog,
    /// Rust extraction capability and optional compiler artifact.
    ///
    /// Omission during deserialization selects [`RustExtractionInput::Syntax`].
    #[serde(default)]
    pub extraction: RustExtractionInput,
}

/// Exact linked Rust items returned by sketch resolution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolveSketchesResponse {
    /// Linked sketch seeds ordered by contract path, physical document index,
    /// then sketch identifier.
    pub seeds: Vec<ResolvedSketchSeed>,
    /// Bounded, deterministic warnings for syntax facts requiring compiler context.
    ///
    /// Compiler extraction returns an empty vector. Syntax extraction retains
    /// these warnings instead of silently discarding them during resolution.
    pub capability_warnings: Vec<String>,
}

/// Runtime-neutral source seed for one explicitly linked sketch.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolvedSketchSeed {
    /// Combined YAML file containing both the link and nested sketch record.
    pub contract_file: CatalogPath,
    /// Zero-based physical YAML document index within [`Self::contract_file`].
    pub document_index: usize,
    /// User-named sketch identifier.
    pub sketch_id: String,
    /// Linked top-level signature kind such as `function` or `struct`.
    pub signature_type: String,
    /// Portable source-relative Rust file containing the linked item.
    pub file: CatalogPath,
    /// Exact Rust item text selected from the source file.
    pub code: String,
}

/// Request for [`SignatureContractKit::diff`].
///
/// Both catalogs are accumulated against one operation's catalog, YAML,
/// signature, and diagnostic budgets. Only direct root-level `.yml` and
/// `.yaml` contract entries participate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiffRequest {
    /// Current contract files to compare.
    pub current_contract_files: FileCatalog,
    /// Previous contract files decoded from an archive or another store.
    pub previous_contract_files: FileCatalog,
}

/// Response returned by [`SignatureContractKit::diff`].
///
/// Entries are deterministic and classify differences as source semantics,
/// extraction context, labels, or document metadata. When multiple physical
/// groups share one semantic label, exact sorted peers are retained before
/// residual groups are paired, added, or removed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiffResponse {
    /// Digest for complete current contract identity, including extraction and
    /// document context.
    ///
    /// This digest covers the current catalog only and is independent of entry
    /// ordering. It is not a digest of the previous catalog or diff payload.
    pub contract_digest: String,
    /// Canonical signature digest format version.
    pub digest_version: u16,
    /// Diff results for top-level grouped contract identities.
    pub entries: Vec<DiffEntry>,
}

impl DiffResponse {
    /// Returns whether the diff contains at least one semantic entry.
    ///
    /// This is exactly `!self.entries.is_empty()`; the current contract digest
    /// may be nonempty even when this method returns `false`.
    pub fn changed(&self) -> bool {
        !self.entries.is_empty()
    }

    fn from_inventory_diff(
        diff: InventoryDiff,
        limits: &crate::limits::DiagnosticLimits,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let mut usage = limits.usage()?;
        let mut entries = Vec::with_capacity(diff.entries().len());
        for entry in diff.entries() {
            cancellation.checkpoint()?;
            let entry = DiffEntry::from_inventory_diff_entry(entry);
            usage.record(&entry)?;
            entries.push(entry);
        }
        Ok(Self {
            contract_digest: diff.contract_digest().render(),
            digest_version: SignatureDigest::VERSION,
            entries,
        })
    }
}

/// One changed top-level signature group reported by [`DiffResponse`].
///
/// Entries are ordered by semantic signature label and deterministic digest
/// pairing. Physical document location is used to disambiguate parsing but is
/// not itself the displayed semantic label.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DiffEntry {
    /// A signature group exists in the current contracts but not the previous set.
    Added {
        /// User contract signature label used as the group identity.
        signature_id: String,
        /// Semantic facets introduced with the group.
        categories: BTreeSet<DiffCategory>,
    },
    /// A signature group exists in the previous contracts but not the current set.
    Removed {
        /// User contract signature label used as the group identity.
        signature_id: String,
        /// Semantic facets removed with the group.
        categories: BTreeSet<DiffCategory>,
    },
    /// A signature group exists in both contract sets but has different digest bytes.
    Changed {
        /// User contract signature label used as the group identity.
        signature_id: String,
        /// Group digest from the current contract files.
        current_digest: String,
        /// Group digest from the previous contract files.
        previous_digest: String,
        /// Exact semantic facets that differ.
        categories: BTreeSet<DiffCategory>,
    },
}

/// Semantic facet reported by one [`DiffEntry`].
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffCategory {
    /// Caller-visible Rust source/API semantics changed.
    SourceSemantics,
    /// Extraction mode, profile, crate roots, or exact file participation changed.
    ExtractionContext,
    /// A user-owned signature label changed while source shape remained stable.
    Labels,
    /// Contract-only root, sketch linkage, or kind metadata changed.
    DocumentMetadata,
}

impl DiffEntry {
    /// Returns the exact semantic facets represented by this diff entry.
    pub fn categories(&self) -> &BTreeSet<DiffCategory> {
        match self {
            Self::Added { categories, .. }
            | Self::Removed { categories, .. }
            | Self::Changed { categories, .. } => categories,
        }
    }

    fn from_inventory_diff_entry(entry: &InventoryDiffEntry) -> Self {
        match entry {
            InventoryDiffEntry::Added {
                signature_id,
                categories,
            } => Self::Added {
                signature_id: signature_id.as_str().to_owned(),
                categories: categories.iter().copied().map(DiffCategory::from).collect(),
            },
            InventoryDiffEntry::Removed {
                signature_id,
                categories,
            } => Self::Removed {
                signature_id: signature_id.as_str().to_owned(),
                categories: categories.iter().copied().map(DiffCategory::from).collect(),
            },
            InventoryDiffEntry::Changed {
                signature_id,
                current_digest,
                previous_digest,
                categories,
            } => Self::Changed {
                signature_id: signature_id.as_str().to_owned(),
                current_digest: current_digest.render(),
                previous_digest: previous_digest.render(),
                categories: categories.iter().copied().map(DiffCategory::from).collect(),
            },
        }
    }
}

impl From<InventoryChangeCategory> for DiffCategory {
    fn from(category: InventoryChangeCategory) -> Self {
        match category {
            InventoryChangeCategory::SourceSemantics => Self::SourceSemantics,
            InventoryChangeCategory::ExtractionContext => Self::ExtractionContext,
            InventoryChangeCategory::Labels => Self::Labels,
            InventoryChangeCategory::DocumentMetadata => Self::DocumentMetadata,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{SignatureEntry, SignatureId, SignatureInventory};

    struct SignatureLimitFixture {
        source_files: FileCatalog,
        contract_files: FileCatalog,
    }

    impl SignatureLimitFixture {
        fn new() -> Self {
            let mut source_files = FileCatalog::new();
            source_files
                .insert(
                    CatalogPath::new("lib.rs").expect("source path"),
                    b"pub fn first() {}\npub fn second() {}\n".to_vec(),
                )
                .expect("source file");
            let mut contract_files = FileCatalog::new();
            contract_files
                .insert(
                    CatalogPath::new("main.yml").expect("contract path"),
                    br#"contract_version: 2
root: ../src
files: [lib.rs]
extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: sample, root: lib.rs, kind: library }] }
signatures:
  - first_function:
      file: lib.rs
      signature_type: function
      name: first
      visibility: public
      sketch: first_body
sketches:
  - first_body:
      file: lib.rs
      signature: first_function
      signature_type: function
      matching: { normalization: exact_lines_v1, occurrence: exactly_one }
      code: old
"#
                    .to_vec(),
                )
                .expect("contract file");
            Self {
                source_files,
                contract_files,
            }
        }

        fn kit(signature_limit: u64) -> SignatureContractKit {
            SignatureContractKit::builder()
                .with_limits(SignatureLimits {
                    rust: crate::limits::RustExtractionLimits {
                        signatures: signature_limit,
                        ..crate::limits::RustExtractionLimits::default()
                    },
                    ..SignatureLimits::default()
                })
                .build()
                .expect("limited kit")
        }

        fn assert_signature_limit(error: &SignatureContractKitError, limit: u64) {
            let exceeded = error.limit_exceeded().expect("typed signature limit");
            assert_eq!(
                exceeded.resource,
                crate::limits::LimitResource::SignatureCount
            );
            assert_eq!(exceeded.limit, limit);
            assert_eq!(exceeded.observed_at_least, limit + 1);
        }
    }

    #[test]
    fn check_modes_apply_error_and_warning_thresholds() {
        let warning = CheckDiagnostic::Warning {
            message: "rust_syntax_v2 capability warning".to_owned(),
        };
        let error = CheckDiagnostic::Extra {
            signature_id: "extra".to_owned(),
        };

        assert!(CheckMode::Default.passed(&[]));
        assert!(CheckMode::Default.passed(std::slice::from_ref(&warning)));
        assert!(!CheckMode::Default.passed(std::slice::from_ref(&error)));
        assert!(!CheckMode::Default.passed(&[error.clone(), warning.clone()]));

        assert!(CheckMode::Strict.passed(&[]));
        assert!(!CheckMode::Strict.passed(std::slice::from_ref(&warning)));
        assert!(!CheckMode::Strict.passed(std::slice::from_ref(&error)));

        assert!(CheckMode::Warning.passed(&[]));
        assert!(CheckMode::Warning.passed(&[error, warning]));
    }

    #[test]
    fn generation_checks_the_signature_budget_before_retaining_each_source_signature() {
        let fixture = SignatureLimitFixture::new();
        let error =
            futures_executor::block_on(SignatureLimitFixture::kit(1).generate(GenerateRequest {
                extraction: RustExtractionInput::Syntax,
                source_files: fixture.source_files,
                target: GenerateTarget::New(GenerateDocument {
                    contract_file: CatalogPath::new("generated.yml").expect("contract path"),
                    root: "../src".to_owned(),
                    files: vec![CatalogPath::new("lib.rs").expect("source path")],
                    crates: vec![RustCrateRoot {
                        id: "sample".to_owned(),
                        root: CatalogPath::new("lib.rs").expect("source path"),
                        kind: RustCrateKind::Library,
                    }],
                }),
                scope: ContractScope::Signatures,
            }))
            .expect_err("the second source signature must cross the budget");

        SignatureLimitFixture::assert_signature_limit(&error, 1);
    }

    #[test]
    fn checking_uses_one_aggregate_signature_budget_for_contract_and_source_acceptance() {
        let fixture = SignatureLimitFixture::new();
        let error = futures_executor::block_on(SignatureLimitFixture::kit(2).check(CheckRequest {
            extraction: RustExtractionInput::Syntax,
            source_files: fixture.source_files,
            contract_files: fixture.contract_files,
            report: ReportRequest::None,
            mode: CheckMode::Default,
        }))
        .expect_err("the second source signature must cross the aggregate budget");

        SignatureLimitFixture::assert_signature_limit(&error, 2);
    }

    #[test]
    fn sketch_resolution_uses_the_same_aggregate_signature_acceptance_budget() {
        let fixture = SignatureLimitFixture::new();
        let error = futures_executor::block_on(SignatureLimitFixture::kit(2).resolve_sketches(
            ResolveSketchesRequest {
                extraction: RustExtractionInput::Syntax,
                source_files: fixture.source_files,
                contract_files: fixture.contract_files,
            },
        ))
        .expect_err("the required source projection must stop at the signature budget");

        SignatureLimitFixture::assert_signature_limit(&error, 2);
    }

    #[test]
    fn check_response_orders_inventory_errors_before_capability_warnings() {
        let id = SignatureId::new("extra");
        let mut source = SignatureInventory::default();
        source
            .insert(SignatureEntry::from_grouped_canonical_bytes(
                id.clone(),
                id,
                b"extra",
            ))
            .expect("test inventory should accept its only entry");
        let comparison = source
            .compare_against(
                &SignatureInventory::default(),
                &crate::limits::DiagnosticLimits::default(),
                &crate::work::CancellationProbe::new(),
            )
            .expect("test inventories should compare");

        let response = CheckResponse::from_inventory_comparison(
            comparison,
            vec!["rust_syntax_v2 capability warning: conditional API".to_owned()],
            CheckMode::Default,
            &crate::limits::DiagnosticLimits::default(),
            &crate::work::CancellationProbe::new(),
        )
        .expect("diagnostics fit default limits");

        assert!(matches!(
            response.diagnostics.as_slice(),
            [
                CheckDiagnostic::Extra { signature_id },
                CheckDiagnostic::Warning { message },
            ] if signature_id == "extra"
                && message == "rust_syntax_v2 capability warning: conditional API"
        ));
        assert!(!response.passed);
    }

    #[test]
    fn check_response_streams_inventory_errors_and_capability_warnings_through_one_byte_budget() {
        let id = SignatureId::new("extra");
        let mut source = SignatureInventory::default();
        source
            .insert(SignatureEntry::from_grouped_canonical_bytes(
                id.clone(),
                id,
                b"extra",
            ))
            .expect("test inventory should accept its only entry");
        let comparison = source
            .compare_against(
                &SignatureInventory::default(),
                &crate::limits::DiagnosticLimits::default(),
                &crate::work::CancellationProbe::new(),
            )
            .expect("test inventories should compare");
        let warning = "rust_syntax_v2 capability warning: conditional API".to_owned();
        let expected = vec![
            CheckDiagnostic::Extra {
                signature_id: "extra".to_owned(),
            },
            CheckDiagnostic::Warning {
                message: warning.clone(),
            },
        ];
        let serialized_bytes = serde_json::to_vec(&expected)
            .expect("public diagnostics")
            .len();
        let limits = crate::limits::DiagnosticLimits {
            count: 2,
            serialized_bytes: u64::try_from(serialized_bytes - 1).expect("fixture size"),
        };

        let error = CheckResponse::from_inventory_comparison(
            comparison,
            std::iter::once(warning),
            CheckMode::Default,
            &limits,
            &crate::work::CancellationProbe::new(),
        )
        .expect_err("the warning must cross the shared aggregate byte budget");
        let exceeded = error.limit_exceeded().expect("typed diagnostic budget");
        assert_eq!(
            exceeded.resource,
            crate::limits::LimitResource::DiagnosticBytes
        );
        assert_eq!(
            exceeded.observed_at_least,
            u64::try_from(serialized_bytes).expect("fixture size")
        );
    }

    #[test]
    fn report_views_preserve_standalone_and_embedded_wire_order() {
        let mut report_files = FileCatalog::new();
        report_files
            .insert(
                CatalogPath::new("reports/nested.json").expect("report path"),
                b"nested".to_vec(),
            )
            .expect("report file");
        let response = CheckResponse {
            passed: true,
            source_shape_digest: "digest".to_owned(),
            digest_version: 2,
            counts: SignatureCheckCounts {
                source_signature_count: 1,
                contract_signature_count: 1,
            },
            diagnostics: Vec::new(),
            report_files,
        };
        let standalone = CheckReportView::new(&response, CheckReportLayout::Standalone);
        let embedded = CheckReportView::new(&response, CheckReportLayout::Embedded);

        assert_eq!(
            serde_json::to_string_pretty(&standalone).expect("standalone JSON"),
            concat!(
                "{\n",
                "  \"passed\": true,\n",
                "  \"source_shape_digest\": \"digest\",\n",
                "  \"digest_version\": 2,\n",
                "  \"counts\": {\n",
                "    \"source_signature_count\": 1,\n",
                "    \"contract_signature_count\": 1\n",
                "  },\n",
                "  \"diagnostics\": []\n",
                "}",
            )
        );
        assert_eq!(
            serde_saphyr::to_string(&standalone).expect("standalone YAML"),
            concat!(
                "passed: true\n",
                "source_shape_digest: digest\n",
                "digest_version: 2\n",
                "counts:\n",
                "  source_signature_count: 1\n",
                "  contract_signature_count: 1\n",
                "diagnostics: []\n",
            )
        );
        assert_eq!(
            serde_json::to_string_pretty(&embedded).expect("embedded JSON"),
            concat!(
                "{\n",
                "  \"passed\": true,\n",
                "  \"counts\": {\n",
                "    \"source_signature_count\": 1,\n",
                "    \"contract_signature_count\": 1\n",
                "  },\n",
                "  \"source_shape_digest\": \"digest\",\n",
                "  \"digest_version\": 2,\n",
                "  \"diagnostics\": []\n",
                "}",
            )
        );
        assert_eq!(
            serde_saphyr::to_string(&embedded).expect("embedded YAML"),
            concat!(
                "passed: true\n",
                "counts:\n",
                "  source_signature_count: 1\n",
                "  contract_signature_count: 1\n",
                "source_shape_digest: digest\n",
                "digest_version: 2\n",
                "diagnostics: []\n",
            )
        );
    }
}
