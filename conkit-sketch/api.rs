use crate::contract::{SketchContracts, SketchMatchPolicy};
use crate::error::SketchContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::inventory::{SketchCheckCounts, SketchDiagnostic};
use crate::limits::SketchLimits;
use crate::matcher::{SketchMatcher, SourceCatalog};
use crate::report::ReportRequest;
use crate::work::{AsyncWorkPool, CancellationProbe, WorkOptions};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Public handle for sketch contract operations.
///
/// Build a kit with [`SketchContractKit::builder`], then call the async
/// methods with in-memory catalog requests. Each handle reuses the configured
/// local or caller-shared Rayon pool for CPU-bound work; the returned futures
/// remain independent of any particular async runtime. Worker threads, active
/// operations, and pending admission are configured independently. See
/// [`WorkOptions`] for the complete admission, cancellation, and scheduling
/// contract.
///
/// # Examples
///
/// ```
/// use conkit_sketch::SketchContractKit;
///
/// let kit = SketchContractKit::builder().build()?;
/// # let _ = kit;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// An operation placed in a spawned task must own its request and kit. A
/// runtime may spawn the owning `async move` future below; this executor-neutral
/// example polls it directly.
///
/// ```
/// use conkit_sketch::{
///     CheckMode, CheckRequest, FileCatalog, ReportRequest, SketchContractKit,
/// };
/// use std::sync::Arc;
///
/// let kit = Arc::new(SketchContractKit::builder().build()?);
/// let task_kit = Arc::clone(&kit);
/// let request = CheckRequest {
///     source_files: FileCatalog::new(),
///     contract_files: FileCatalog::new(),
///     report: ReportRequest::None,
///     mode: CheckMode::Enforce,
/// };
/// let task = async move { task_kit.check(request).await };
/// let response = futures_executor::block_on(task)?;
///
/// assert!(response.passed);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct SketchContractKit {
    work: AsyncWorkPool,
    limits: Arc<SketchLimits>,
}

/// Builder for [`SketchContractKit`].
///
/// The default builder uses [`WorkerPool::RuntimeDefault`](crate::WorkerPool::RuntimeDefault),
/// one active root operation, no pending admission, and conservative
/// [`SketchLimits`]. Callers may independently opt into a bounded queue and
/// replace scheduling or resource budgets before building.
///
/// # Examples
///
/// ```
/// use conkit_sketch::{SketchContractKitBuilder, WorkOptions, WorkerPool};
/// use std::num::NonZeroUsize;
///
/// let kit = SketchContractKitBuilder::default()
///     .with_work_options(WorkOptions {
///         pool: WorkerPool::Dedicated {
///             worker_threads: NonZeroUsize::MIN,
///         },
///         max_in_flight_operations: NonZeroUsize::MIN,
///         max_pending_operations: 0,
///     })
///     .build()?;
/// # let _ = kit;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Default)]
pub struct SketchContractKitBuilder {
    work: WorkOptions,
    limits: SketchLimits,
}

impl SketchContractKitBuilder {
    /// Configures CPU work scheduling for the kit.
    ///
    /// This replaces the builder's current [`WorkOptions`]. Worker threads,
    /// active operations, and pending operations remain independent.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_sketch::{SketchContractKitBuilder, WorkOptions, WorkerPool};
    /// use std::num::NonZeroUsize;
    ///
    /// let builder = SketchContractKitBuilder::default().with_work_options(WorkOptions {
    ///     pool: WorkerPool::Dedicated {
    ///         worker_threads: NonZeroUsize::MIN,
    ///     },
    ///     max_in_flight_operations: NonZeroUsize::MIN,
    ///     max_pending_operations: 0,
    /// });
    /// let kit = builder.build()?;
    /// # let _ = kit;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_work_options(mut self, work: WorkOptions) -> Self {
        self.work = work;
        self
    }

    /// Replaces the resource budgets enforced by every operation.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_sketch::{SketchContractKitBuilder, SketchLimits};
    ///
    /// let builder = SketchContractKitBuilder::default().with_limits(SketchLimits::default());
    /// let kit = builder.build()?;
    /// # let _ = kit;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_limits(mut self, limits: SketchLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Builds a local sketch contract kit.
    ///
    /// # Errors
    ///
    /// Returns [`SketchContractKitError`] if a kit-owned Rayon pool cannot be
    /// initialized or if `max_in_flight_operations + max_pending_operations`
    /// overflows [`usize`]. A caller-supplied [`WorkerPool::Shared`](crate::WorkerPool::Shared)
    /// is reused directly rather than initializing another pool.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_sketch::SketchContractKitBuilder;
    ///
    /// let kit = SketchContractKitBuilder::default().build()?;
    /// # let _ = kit;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn build(self) -> Result<SketchContractKit, SketchContractKitError> {
        Ok(SketchContractKit {
            work: AsyncWorkPool::new(self.work)?,
            limits: Arc::new(self.limits),
        })
    }
}

impl SketchContractKit {
    /// Starts configuring a [`SketchContractKit`].
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_sketch::SketchContractKit;
    ///
    /// let kit = SketchContractKit::builder().build()?;
    /// # let _ = kit;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn builder() -> SketchContractKitBuilder {
        SketchContractKitBuilder::default()
    }

    /// Checks source files against sketch contract files.
    ///
    /// Only direct root-level contract entries whose logical paths end in
    /// `.yaml` or `.yml` are considered. Every such entry must be a combined document containing
    /// `root`, `files`, `signatures`, and `sketches`. Signature-owned links and
    /// nested sketches are validated before matching begins.
    ///
    /// Source entries are normalized only when referenced by a valid sketch,
    /// although [`SketchCheckCounts::source_catalog_entry_count`] includes every supplied
    /// source entry. Matching groups sketches by logical source path and
    /// versioned normalization policy so each referenced source representation
    /// is built once. [`SketchNormalization::ExactLinesV1`](crate::SketchNormalization::ExactLinesV1)
    /// preserves arbitrary source bytes while converting CRLF to LF.
    ///
    /// Missing source files, non-matching snippets, and occurrence-policy
    /// violations are returned as sorted [`SketchDiagnostic`] values with
    /// contract/source location and bounded mismatch or occurrence evidence.
    /// They are not operation errors.
    /// [`CheckMode`] controls whether those diagnostics set
    /// [`CheckResponse::passed`] to `false`. If requested, report bytes are
    /// returned in [`CheckResponse::report_files`]; this method performs no
    /// filesystem I/O.
    ///
    /// # Errors
    ///
    /// Returns [`SketchContractKitError`] when admission is already at its
    /// active-plus-pending capacity; a configured catalog, YAML, normalization,
    /// matching-work, diagnostic, or report-output limit is exceeded; contract
    /// YAML is malformed or violates the field, identifier, path, link, kind,
    /// or nonempty-code rules; signature labels or sketch identifiers are
    /// duplicated; report rendering fails; or background work cannot complete.
    /// Use [`SketchContractKitError::is_queue_full`] and
    /// [`SketchContractKitError::limit_exceeded`] to inspect the two typed
    /// resource failures.
    ///
    /// # Panics
    ///
    /// If the background operation panics on a Rayon worker, its panic payload
    /// resumes unwinding on the thread that polls this future to completion.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_sketch::{
    ///     CatalogPath, CheckMode, CheckRequest, FileCatalog, ReportRequest,
    ///     SketchContractKit, SketchDiagnostic, SketchNormalization,
    /// };
    ///
    /// fn catalog_with(
    ///     path: &str,
    ///     bytes: &[u8],
    /// ) -> Result<FileCatalog, Box<dyn std::error::Error>> {
    ///     let mut catalog = FileCatalog::new();
    ///     catalog.insert(CatalogPath::new(path)?, bytes.to_vec())?;
    ///     Ok(catalog)
    /// }
    ///
    /// let source_files = catalog_with("lib.rs", b"pub fn answer() -> u8 { 41 }\n")?;
    /// let contract_files = catalog_with(
    ///     "main.yml",
    ///     br#"contract_version: 2
    /// root: ../src
    /// files: [lib.rs]
    /// extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: example, root: lib.rs, kind: library }] }
    /// signatures:
    ///   - answer_signature:
    ///       file: lib.rs
    ///       signature_type: function
    ///       sketch: answer_body
    /// sketches:
    ///   - answer_body:
    ///       file: lib.rs
    ///       signature: answer_signature
    ///       signature_type: function
    ///       matching: { normalization: exact_lines_v1, occurrence: at_least_one }
    ///       code: pub fn answer() -> u8 { 42 }
    /// "#,
    /// )?;
    /// let kit = SketchContractKit::builder().build()?;
    ///
    /// let response = futures_executor::block_on(kit.check(CheckRequest {
    ///     source_files,
    ///     contract_files,
    ///     report: ReportRequest::None,
    ///     mode: CheckMode::Enforce,
    /// }))?;
    ///
    /// assert!(!response.passed);
    /// assert!(matches!(
    ///     response.diagnostics.as_slice(),
    ///     [SketchDiagnostic::NotMatched {
    ///         sketch,
    ///         normalization: SketchNormalization::ExactLinesV1,
    ///         candidate: Some(_),
    ///     }] if sketch.sketch_id == "answer_body"
    ///         && sketch.contract_file.as_str() == "main.yml"
    ///         && sketch.document_index == 0
    ///         && sketch.source_file.as_str() == "lib.rs"
    /// ));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn check(
        &self,
        request: CheckRequest,
    ) -> Result<CheckResponse, SketchContractKitError> {
        let limits = Arc::clone(&self.limits);
        self.work
            .execute(move |cancellation| request.run(&limits, &cancellation))
            .await?
    }

    /// Refreshes explicitly linked sketches in combined contract documents.
    ///
    /// Each [`SketchSeed`] identifies an existing nested sketch and the
    /// signature link that produced its exact source text. Full refresh requires
    /// exactly one seed for every linked sketch; partial refresh targets only
    /// supplied IDs, and an empty partial refresh is an exact catalog-byte
    /// no-op. Seed document path/index, ID, signature type, and source path must
    /// equal the linked contract facts exactly, without trimming or other
    /// normalization.
    ///
    /// Generation replaces only a targeted sketch's `code` field. The
    /// document's `root`, `files`, `signatures`, link direction, identifier,
    /// matching policy, and `signature_type` remain intact. If every targeted
    /// decoded code value in a physical contract file already equals its seed,
    /// that file's original bytes are returned without loading the lossless
    /// editor. A real change preserves scalar presentation where safe, fails
    /// closed when an anchor or alias makes local mutation unsafe, and reparses
    /// the complete edited document before returning it. Files without targeted
    /// changes, nested YAML entries, and non-YAML catalog entries are returned
    /// byte-for-byte unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`SketchContractKitError`] when admission is full; a catalog,
    /// YAML, matching, scratch, or returned-output limit is exceeded; a combined
    /// document is invalid; a seed is missing, duplicated, unknown, or does not
    /// exactly match its linked document, identifier, signature type, and source
    /// file; refreshed code normalizes to empty; a changed scalar is anchored or
    /// aliased; lossless rendering or semantic verification fails; or background
    /// work cannot complete. Verification reparses share the request's original
    /// cumulative YAML budget.
    ///
    /// # Panics
    ///
    /// If the background operation panics on a Rayon worker, its panic payload
    /// resumes unwinding on the thread that polls this future to completion.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_sketch::{
    ///     CatalogPath, FileCatalog, GenerateMode, GenerateRequest,
    ///     SketchContractKit, SketchSeed,
    /// };
    ///
    /// let contract_path = CatalogPath::new("main.yml")?;
    /// let mut contract_files = FileCatalog::new();
    /// contract_files.insert(contract_path.clone(), br#"contract_version: 2
    /// root: ../src
    /// files: [lib.rs]
    /// extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: example, root: lib.rs, kind: library }] }
    /// signatures:
    ///   - answer:
    ///       file: lib.rs
    ///       signature_type: function
    ///       sketch: answer_body
    /// sketches:
    ///   - answer_body:
    ///       file: lib.rs
    ///       signature: answer
    ///       signature_type: function
    ///       matching: { normalization: exact_lines_v1, occurrence: at_least_one }
    ///       code: |-
    ///         old code
    /// "#.to_vec())?;
    /// let kit = SketchContractKit::builder().build()?;
    ///
    /// let response = futures_executor::block_on(kit.generate(GenerateRequest {
    ///     contract_files,
    ///     seeds: vec![SketchSeed {
    ///         contract_file: contract_path.clone(),
    ///         document_index: 0,
    ///         sketch_id: "answer_body".to_owned(),
    ///         signature_type: "function".to_owned(),
    ///         file: CatalogPath::new("lib.rs")?,
    ///         code: "pub fn answer() -> u8 { 42 }".to_owned(),
    ///     }],
    ///     mode: GenerateMode::FullRefresh,
    /// }))?;
    /// let yaml = std::str::from_utf8(
    ///     response.contract_files.get(&contract_path).expect("updated contract"),
    /// )?;
    ///
    /// assert_eq!(response.counts.refreshed_sketch_count, 1);
    /// assert!(yaml.contains("pub fn answer() -> u8 { 42 }"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn generate(
        &self,
        request: GenerateRequest,
    ) -> Result<GenerateResponse, SketchContractKitError> {
        let limits = Arc::clone(&self.limits);
        self.work
            .execute(move |cancellation| request.run(&limits, &cancellation))
            .await?
    }

    /// Compares current sketch contracts with a previous contract catalog.
    ///
    /// Sketch identifiers are identity. For a sketch present in both catalogs,
    /// the linked logical source file, linked signature label, `signature_type`,
    /// matching policy, and normalized code are semantic. The containing
    /// contract document, YAML formatting, YAML comments outside `code`, and
    /// mapping order are nonsemantic.
    ///
    /// Exact-line normalization equates CRLF with LF and ignores one final line
    /// terminator. It preserves indentation, tabs, horizontal whitespace, blank
    /// lines, line order, and every token or comment byte inside `code`, so
    /// changes to any of those preserved bytes are semantic.
    /// Returned entries are merged in exact sketch-ID order. Contract-file and
    /// document-index values in [`SketchSnapshot`] locate authoring sites but do
    /// not participate in identity or semantic field comparison.
    ///
    /// # Errors
    ///
    /// Returns [`SketchContractKitError`] when admission is full; cumulative
    /// catalog or YAML accounting across both sides, or sketch identity and
    /// normalization work on either side, exceeds a configured limit; either
    /// catalog contains invalid sketch contract input; or background work cannot
    /// complete.
    ///
    /// # Panics
    ///
    /// If the background operation panics on a Rayon worker, its panic payload
    /// resumes unwinding on the thread that polls this future to completion.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_sketch::{
    ///     CatalogPath, DiffEntry, DiffRequest, FileCatalog, SketchContractKit,
    ///     SketchField,
    /// };
    /// fn contract_catalog(code: &str) -> Result<FileCatalog, Box<dyn std::error::Error>> {
    ///     let yaml = format!(r#"contract_version: 2
    /// root: ../src
    /// files: [lib.rs]
    /// extraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: example, root: lib.rs, kind: library }}] }}
    /// signatures:
    ///   - answer_signature:
    ///       file: lib.rs
    ///       signature_type: function
    ///       sketch: answer_body
    /// sketches:
    ///   - answer_body:
    ///       file: lib.rs
    ///       signature: answer_signature
    ///       signature_type: function
    ///       matching: {{ normalization: exact_lines_v1, occurrence: at_least_one }}
    ///       code: {code}
    /// "#);
    ///     let mut catalog = FileCatalog::new();
    ///     catalog.insert(CatalogPath::new("main.yml")?, yaml.into_bytes())?;
    ///     Ok(catalog)
    /// }
    ///
    /// let kit = SketchContractKit::builder().build()?;
    /// let response = futures_executor::block_on(kit.diff(DiffRequest {
    ///     current_contract_files: contract_catalog("let value = 2;")?,
    ///     previous_contract_files: contract_catalog("let value = 1;")?,
    /// }))?;
    ///
    /// assert!(response.changed());
    /// assert!(matches!(
    ///     response.entries.as_slice(),
    ///     [DiffEntry::Changed { current, fields, .. }]
    ///         if current.sketch_id == "answer_body"
    ///             && fields.as_slice() == [SketchField::Code]
    /// ));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn diff(&self, request: DiffRequest) -> Result<DiffResponse, SketchContractKitError> {
        let limits = Arc::clone(&self.limits);
        self.work
            .execute(move |cancellation| request.run(&limits, &cancellation))
            .await?
    }
}

/// Controls how diagnostics affect [`CheckResponse::passed`].
///
/// Diagnostics are always retained in [`CheckResponse::diagnostics`]. The mode
/// changes only the response's pass/fail value.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CheckMode {
    /// Fail the response when one or more diagnostics are present.
    Enforce,
    /// Preserve diagnostics while keeping the response passing.
    Warning,
}

impl CheckMode {
    pub(crate) fn passed(self, diagnostics: &[SketchDiagnostic]) -> bool {
        match self {
            Self::Enforce => diagnostics.is_empty(),
            Self::Warning => true,
        }
    }
}

/// Request for [`SketchContractKit::check`].
///
/// The caller owns filesystem discovery and provides both source and contract
/// bytes as [`FileCatalog`] values. The complete source catalog contributes to
/// [`SketchCheckCounts::source_catalog_entry_count`], while matching normalizes only
/// source entries referenced by parsed sketches. Contract parsing considers
/// only direct root-level `.yaml` and `.yml` entries.
///
/// # Examples
///
/// ```
/// use conkit_sketch::{CheckMode, CheckRequest, FileCatalog, ReportRequest};
///
/// let request = CheckRequest {
///     source_files: FileCatalog::new(),
///     contract_files: FileCatalog::new(),
///     report: ReportRequest::None,
///     mode: CheckMode::Enforce,
/// };
///
/// assert_eq!(request.mode, CheckMode::Enforce);
/// assert!(request.source_files.is_empty());
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CheckRequest {
    /// All source entries available for sketch matching.
    ///
    /// Paths are logical [`CatalogPath`] values, not operating-system paths.
    /// Unreferenced entries are counted but do not need to contain UTF-8.
    pub source_files: FileCatalog,
    /// Catalog containing candidate sketch contract documents.
    ///
    /// Only direct root-level `.yaml` and `.yml` entries are parsed. Every
    /// parsed entry must use the combined
    /// `root`/`files`/`signatures`/`sketches` shape.
    pub contract_files: FileCatalog,
    /// Whether to return a YAML or JSON report entry.
    pub report: ReportRequest,
    /// How returned diagnostics affect the response's pass/fail value.
    pub mode: CheckMode,
}

impl CheckRequest {
    fn run(
        self,
        limits: &SketchLimits,
        cancellation: &CancellationProbe,
    ) -> Result<CheckResponse, SketchContractKitError> {
        let Self {
            source_files,
            contract_files,
            report,
            mode,
        } = self;
        let mut catalog_usage = limits.catalog_usage();
        catalog_usage.record(&source_files, cancellation)?;
        catalog_usage.record(&contract_files, cancellation)?;
        let mut yaml_budget = limits.yaml_budget();
        cancellation.checkpoint()?;
        let contracts =
            SketchContracts::from_catalog(contract_files, limits, &mut yaml_budget, cancellation)?;
        cancellation.checkpoint()?;
        let sources = SourceCatalog::from_catalog(source_files, &contracts, cancellation)?;
        let comparison = SketchMatcher::new(sources, contracts).check(limits, cancellation)?;
        cancellation.checkpoint()?;
        let mut response = comparison.into_response(mode);
        response.report_files = report.render(&response, &limits.output, cancellation)?;
        Ok(response)
    }
}

/// Response returned by [`SketchContractKit::check`].
///
/// Contract syntax and validation failures are returned as
/// [`SketchContractKitError`], not stored here. This response contains ordinary
/// match outcomes, with diagnostics sorted deterministically by complete
/// contract/source location and diagnostic kind.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CheckResponse {
    /// Whether the check passed under the requested [`CheckMode`].
    pub passed: bool,
    /// Source, contract, match, and diagnostic totals.
    pub counts: SketchCheckCounts,
    /// At most one diagnostic for each parsed sketch, in deterministic order.
    pub diagnostics: Vec<SketchDiagnostic>,
    /// Report bytes returned for the request.
    ///
    /// This catalog is empty for [`ReportRequest::None`] and contains exactly
    /// the requested logical output entry for [`ReportRequest::Generate`].
    pub report_files: FileCatalog,
}

impl CheckResponse {
    /// Borrows the serialized report payload without `report_files`.
    ///
    /// The opaque view implements [`Serialize`] and is used for both
    /// standalone domain reports and embedded combined CLI reports. It contains
    /// exactly `passed`, `counts`, and `diagnostics`, even when the response
    /// itself already owns generated report bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_sketch::{
    ///     CatalogPath, CheckResponse, FileCatalog, SketchCheckCounts,
    /// };
    ///
    /// let mut report_files = FileCatalog::new();
    /// report_files.insert(CatalogPath::new("report.json")?, b"stored report".to_vec())?;
    /// let response = CheckResponse {
    ///     passed: true,
    ///     counts: SketchCheckCounts {
    ///         source_catalog_entry_count: 0,
    ///         referenced_source_file_count: 0,
    ///         present_referenced_source_file_count: 0,
    ///         contract_document_count: 0,
    ///         sketch_count: 0,
    ///         matched_sketch_count: 0,
    ///         failed_sketch_count: 0,
    ///     },
    ///     diagnostics: Vec::new(),
    ///     report_files,
    /// };
    /// let report = serde_json::to_value(response.report_view())?;
    ///
    /// assert_eq!(report["passed"], true);
    /// assert!(report.get("report_files").is_none());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn report_view(&self) -> impl Serialize + '_ {
        crate::report::CheckReportView::new(self)
    }
}

/// Request for [`SketchContractKit::generate`].
///
/// Generation consumes combined contract bytes plus exact linked-sketch seeds
/// and returns the complete updated catalog; it never reads or writes
/// operating-system paths. [`GenerateMode::FullRefresh`] requires complete seed
/// coverage, while [`GenerateMode::PartialRefresh`] validates and updates only
/// supplied exact IDs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GenerateRequest {
    /// Existing combined contract documents to validate and selectively update.
    pub contract_files: FileCatalog,
    /// Exact refresh seeds selected according to [`GenerateRequest::mode`].
    ///
    /// IDs must be unique. Every seed's document path/index, sketch ID,
    /// signature type, and source path must exactly equal one linked sketch.
    pub seeds: Vec<SketchSeed>,
    /// Whether every linked sketch or only supplied sketch IDs are refreshed.
    pub mode: GenerateMode,
}

/// Selects complete or targeted linked-sketch generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum GenerateMode {
    /// Require and validate exactly one seed for every linked sketch.
    FullRefresh,
    /// Refresh only supplied IDs and preserve every unspecified sketch.
    ///
    /// An empty seed list validates the input contracts, then returns every
    /// original catalog byte unchanged.
    PartialRefresh,
}

/// Scope and exact byte-change totals for completed sketch generation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SketchGenerationCounts {
    /// Number of linked sketches present in the parsed contract catalog.
    pub linked_sketch_count: usize,
    /// Number of supplied sketches validated and targeted by the request.
    pub refreshed_sketch_count: usize,
    /// Number of refreshed sketches whose decoded code value changed exactly.
    ///
    /// This is not a normalized-code comparison: spelling differences that
    /// decode to different strings count even if matching would treat them as
    /// equivalent.
    pub changed_sketch_count: usize,
    /// Number of distinct physical `(contract path, document index)` pairs
    /// containing at least one changed sketch.
    pub changed_document_count: usize,
}

impl SketchGenerationCounts {
    pub(crate) const fn new(linked_sketch_count: usize) -> Self {
        Self {
            linked_sketch_count,
            refreshed_sketch_count: 0,
            changed_sketch_count: 0,
            changed_document_count: 0,
        }
    }

    pub(crate) fn record_refreshed(&mut self, changed: bool) {
        self.refreshed_sketch_count += 1;
        if changed {
            self.changed_sketch_count += 1;
        }
    }

    pub(crate) fn record_changed_document(&mut self) {
        self.changed_document_count += 1;
    }
}

/// Response returned by [`SketchContractKit::generate`].
///
/// The response contains the complete input catalog. Output entries retain
/// deterministic logical path order, updated documents preserve their existing
/// sketch order, and nested YAML, non-YAML entries, and untargeted root
/// documents pass through byte-for-byte. Identical requests produce identical
/// catalog order and bytes. Physical contract files whose targeted decoded code
/// values are all already equal also retain their complete original bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GenerateResponse {
    /// Complete contract catalog, including unchanged passthrough entries.
    pub contract_files: FileCatalog,
    /// Linked, refreshed, exact-change, and changed-document totals.
    pub counts: SketchGenerationCounts,
}

/// Request for [`SketchContractKit::diff`].
///
/// Catalog and YAML resource budgets accumulate across the current and previous
/// sides rather than restarting for each side.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiffRequest {
    /// Current sketch contract catalog.
    pub current_contract_files: FileCatalog,
    /// Previous sketch contract catalog.
    pub previous_contract_files: FileCatalog,
}

impl DiffRequest {
    fn run(
        self,
        limits: &SketchLimits,
        cancellation: &CancellationProbe,
    ) -> Result<DiffResponse, SketchContractKitError> {
        let mut catalog_usage = limits.catalog_usage();
        catalog_usage.record(&self.current_contract_files, cancellation)?;
        catalog_usage.record(&self.previous_contract_files, cancellation)?;
        let mut yaml_budget = limits.yaml_budget();
        cancellation.checkpoint()?;
        let current = SketchContracts::from_catalog(
            self.current_contract_files,
            limits,
            &mut yaml_budget,
            cancellation,
        )?;
        cancellation.checkpoint()?;
        let previous = SketchContracts::from_catalog(
            self.previous_contract_files,
            limits,
            &mut yaml_budget,
            cancellation,
        )?;
        cancellation.checkpoint()?;
        current.diff_against(&previous, cancellation)
    }
}

/// Response returned by [`SketchContractKit::diff`].
///
/// Entries are deterministically ordered by exact sketch ID. [`Self::changed`]
/// is derived solely from whether that entry list is empty.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiffResponse {
    /// Domain-separated SHA-256 digest of the current catalog's complete
    /// semantic sketch identity.
    ///
    /// Version 2 length-frames the exact ID, linked source and signature facts,
    /// stable matching-policy tags, and normalized code lines for every sketch
    /// in ID order. Physical contract locators and YAML presentation are absent.
    pub contract_digest: String,
    /// Canonical digest encoding version for both contract and code digests.
    ///
    /// The current encoding is version `2`.
    pub digest_version: u16,
    /// Sketch changes in exact-ID order.
    pub entries: Vec<DiffEntry>,
}

impl DiffResponse {
    /// Returns whether any sketch was added, removed, or semantically changed.
    ///
    /// This is exactly `!self.entries.is_empty()`; the digest is descriptive and
    /// is not compared to derive the result.
    pub fn changed(&self) -> bool {
        !self.entries.is_empty()
    }
}

/// Context-rich state of one sketch at one side of a semantic diff.
///
/// [`Self::contract_file`] and [`Self::document_index`] are locator evidence for
/// presenting where this snapshot came from. They do not participate in sketch
/// identity, semantic field comparison, or digest construction.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SketchSnapshot {
    /// User-defined sketch identifier.
    pub sketch_id: String,
    /// Logical contract file containing this occurrence.
    pub contract_file: CatalogPath,
    /// Zero-based physical YAML document index within `contract_file`.
    pub document_index: usize,
    /// Logical source file referenced by the linked signature.
    pub source_file: CatalogPath,
    /// Signature label linked to this sketch.
    pub linked_signature: String,
    /// Signature kind copied from the linked signature.
    pub signature_type: String,
    /// Versioned normalization and occurrence semantics.
    pub matching: SketchMatchPolicy,
    /// Version 2 domain-separated SHA-256 digest of normalized code lines.
    ///
    /// The encoding includes the stable normalization tag and length-frames
    /// every line; it does not hash YAML scalar presentation.
    pub code_digest: String,
}

/// Independently observable semantic sketch field.
///
/// The variant declaration order is the canonical order used by the `fields`
/// member of [`DiffEntry::Changed`]. There is no normalization variant because
/// [`SketchNormalization::ExactLinesV1`](crate::SketchNormalization::ExactLinesV1)
/// is mandatory for every valid v2 sketch; occurrence remains configurable and
/// therefore independently observable.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum SketchField {
    /// Exact referenced logical source path.
    SourceFile,
    /// Exact linked signature label.
    LinkedSignature,
    /// Exact linked `signature_type` value.
    SignatureType,
    /// Required occurrence policy (`at_least_one` or `exactly_one`).
    Occurrence,
    /// Code lines after mandatory `exact_lines_v1` normalization.
    Code,
}

/// One semantic sketch change returned by [`DiffResponse`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DiffEntry {
    /// A sketch ID exists only in the current catalog.
    Added {
        /// Current sketch state and locator.
        current: SketchSnapshot,
    },
    /// A sketch ID exists only in the previous catalog.
    Removed {
        /// Previous sketch state and locator.
        previous: SketchSnapshot,
    },
    /// The same exact sketch ID exists in both catalogs with different semantic
    /// fields.
    Changed {
        /// Previous semantic state and locator.
        previous: SketchSnapshot,
        /// Current semantic state and locator.
        current: SketchSnapshot,
        /// Changed fields in canonical [`SketchField`] declaration order.
        fields: Vec<SketchField>,
    },
}

/// Caller-supplied refresh data for one linked sketch.
///
/// A signature-domain resolver can produce this runtime-neutral data from an
/// exact source span. The `conkit-sketch` crate validates it against the combined
/// document without depending on a signature parser. Document path/index,
/// sketch ID, signature type, and source path comparisons are exact: no field is
/// trimmed, case-folded, or otherwise normalized before agreement is checked.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SketchSeed {
    /// Exact combined-document catalog path containing the link and sketch.
    pub contract_file: CatalogPath,
    /// Exact zero-based physical YAML document index within `contract_file`.
    pub document_index: usize,
    /// Exact user-facing sketch identifier.
    pub sketch_id: String,
    /// Exact signature kind copied from the linked signature.
    pub signature_type: String,
    /// Exact logical source entry declared by the linked signature.
    pub file: CatalogPath,
    /// Decoded source text to store in `code`, required to contain at least one
    /// line after the sketch's exact-line normalization policy is applied.
    pub code: String,
}
