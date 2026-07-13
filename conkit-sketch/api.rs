use crate::contract::SketchContracts;
use crate::error::SketchContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::generate::SketchGenerator;
use crate::inventory::{SketchCheckCounts, SketchDiagnostic};
use crate::matcher::{SketchMatcher, SourceCatalog};
use crate::report::{ReportFiles, ReportRequest};
use crate::work::{AsyncWorkPool, WorkOptions};
use serde::{Deserialize, Serialize};

/// Public handle for sketch contract operations.
///
/// Build a kit with [`SketchContractKit::builder`], then call the async
/// methods with in-memory catalog requests. Each handle owns and reuses one
/// local Rayon pool for CPU-bound work; the returned futures remain independent
/// of any particular async runtime. The configured worker count also bounds
/// the number of admitted root operations. See [`WorkOptions`] for the complete
/// admission, cancellation, caller-deadline, and scheduling contract.
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
///     mode: CheckMode::Default,
/// };
/// let task = async move { task_kit.check(request).await };
/// let response = futures_executor::block_on(task)?;
///
/// assert!(response.passed);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct SketchContractKit {
    inner: SketchContractKitInner,
}

enum SketchContractKitInner {
    Local(LocalSketchContractKit),
}

/// Builder for [`SketchContractKit`].
///
/// The default builder uses
/// [`WorkParallelism::RuntimeDefault`](crate::WorkParallelism::RuntimeDefault),
/// deriving root-operation admission from Rayon's selected worker count. Use
/// [`SketchContractKitBuilder::with_work_options`] with
/// [`WorkParallelism::Fixed`](crate::WorkParallelism::Fixed) to select one
/// explicit non-zero value for both worker count and admitted root operations.
/// See [`WorkOptions`] for the complete scheduling contract.
///
/// # Examples
///
/// ```
/// use conkit_sketch::{SketchContractKitBuilder, WorkOptions, WorkParallelism};
/// use std::num::NonZeroUsize;
///
/// let kit = SketchContractKitBuilder::default()
///     .with_work_options(WorkOptions {
///         parallelism: WorkParallelism::Fixed(NonZeroUsize::MIN),
///     })
///     .build()?;
/// # let _ = kit;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Default)]
pub struct SketchContractKitBuilder {
    work: WorkOptions,
}

impl SketchContractKitBuilder {
    /// Configures CPU work scheduling for the kit.
    ///
    /// This replaces the builder's current [`WorkOptions`]. Fixed parallelism
    /// sets both the Rayon worker count and per-kit root-operation admission
    /// capacity; [`WorkOptions`] documents runtime independence, cancellation,
    /// caller deadlines, host-side task bounds, and scheduling guarantees.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_sketch::{SketchContractKitBuilder, WorkOptions, WorkParallelism};
    /// use std::num::NonZeroUsize;
    ///
    /// let builder = SketchContractKitBuilder::default().with_work_options(WorkOptions {
    ///     parallelism: WorkParallelism::Fixed(NonZeroUsize::MIN),
    /// });
    /// let kit = builder.build()?;
    /// # let _ = kit;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_work_options(mut self, work: WorkOptions) -> Self {
        self.work = work;
        self
    }

    /// Builds a local sketch contract kit.
    ///
    /// # Errors
    ///
    /// Returns [`SketchContractKitError`] if the internal work pool cannot be
    /// initialized.
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
            inner: SketchContractKitInner::Local(LocalSketchContractKit::new(self.work)?),
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
    /// flattened sketches are validated before matching begins.
    ///
    /// Source entries are normalized only when referenced by a valid sketch,
    /// although [`SketchCheckCounts::source_file_count`] includes every supplied
    /// source entry. Malformed UTF-8 source bytes are supported through the
    /// crate's byte-preserving normalization fallback.
    ///
    /// Missing source files and non-matching snippets are returned as sorted
    /// [`SketchDiagnostic`] values. They are not operation errors.
    /// [`CheckMode`] controls whether those diagnostics set
    /// [`CheckResponse::passed`] to `false`. If requested, report bytes are
    /// returned in [`CheckResponse::report_files`]; this method performs no
    /// filesystem I/O.
    ///
    /// # Errors
    ///
    /// Returns [`SketchContractKitError`] when contract YAML is malformed or
    /// violates the field, identifier, path, link, kind, or nonempty-code
    /// rules; when signature labels or sketch identifiers are duplicated; when
    /// response invariants or report rendering fail; or when background work
    /// cannot complete.
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
    ///     SketchContractKit, SketchDiagnostic,
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
    ///     br#"root: ../src
    /// files: [lib.rs]
    /// signatures:
    ///   - answer_signature:
    ///       file: lib.rs
    ///       signature_type: function
    ///       sketch: answer_body
    /// sketches:
    ///   - answer_body:
    ///     signature_type: function
    ///     code: pub fn answer() -> u8 { 42 }
    /// "#,
    /// )?;
    /// let kit = SketchContractKit::builder().build()?;
    ///
    /// let response = futures_executor::block_on(kit.check(CheckRequest {
    ///     source_files,
    ///     contract_files,
    ///     report: ReportRequest::None,
    ///     mode: CheckMode::Strict,
    /// }))?;
    ///
    /// assert!(!response.passed);
    /// assert_eq!(
    ///     response.diagnostics,
    ///     vec![SketchDiagnostic::NotMatched {
    ///         sketch_id: "answer_body".to_owned(),
    ///         file: "lib.rs".to_owned(),
    ///     }],
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn check(
        &self,
        request: CheckRequest,
    ) -> Result<CheckResponse, SketchContractKitError> {
        <Self as SketchContractKitBackend>::check(self, request).await
    }

    /// Refreshes explicitly linked sketches in combined contract documents.
    ///
    /// Each [`SketchSeed`] identifies an existing flattened sketch and the
    /// signature link that produced its exact source text. Generation replaces
    /// only that sketch's `code` field. The document's `root`, `files`,
    /// `signatures`, link direction, identifier, and `signature_type` remain
    /// intact. Documents without linked sketches, nested YAML entries, and
    /// non-YAML catalog entries are returned byte-for-byte unchanged. The
    /// response contains every input catalog entry, and its `sketch_count` is
    /// the number of linked sketches refreshed.
    ///
    /// # Errors
    ///
    /// Returns [`SketchContractKitError`] when a combined document is invalid;
    /// when a seed is missing, duplicated, or does not exactly match its linked
    /// document, identifier, signature type, and source file; when refreshed
    /// code normalizes to empty; when YAML rendering fails; or when background
    /// work cannot complete.
    ///
    /// # Panics
    ///
    /// If the background operation panics on a Rayon worker, its panic payload
    /// resumes unwinding on the thread that polls this future to completion.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_sketch::{CatalogPath, FileCatalog, GenerateRequest, SketchContractKit, SketchSeed};
    ///
    /// let contract_path = CatalogPath::new("main.yml")?;
    /// let mut contract_files = FileCatalog::new();
    /// contract_files.insert(contract_path.clone(), br#"root: ../src
    /// files: [lib.rs]
    /// signatures:
    ///   - answer:
    ///       file: lib.rs
    ///       signature_type: function
    ///       sketch: answer_body
    /// sketches:
    ///   - answer_body:
    ///     signature_type: function
    ///     code: old code
    /// "#.to_vec())?;
    /// let kit = SketchContractKit::builder().build()?;
    ///
    /// let response = futures_executor::block_on(kit.generate(GenerateRequest {
    ///     contract_files,
    ///     seeds: vec![SketchSeed {
    ///         contract_file: contract_path.clone(),
    ///         sketch_id: "answer_body".to_owned(),
    ///         signature_type: "function".to_owned(),
    ///         file: CatalogPath::new("lib.rs")?,
    ///         code: "pub fn answer() -> u8 { 42 }".to_owned(),
    ///     }],
    /// }))?;
    /// let yaml = std::str::from_utf8(
    ///     response.contract_files.get(&contract_path).expect("updated contract"),
    /// )?;
    ///
    /// assert_eq!(response.sketch_count, 1);
    /// assert!(yaml.contains("pub fn answer() -> u8 { 42 }"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn generate(
        &self,
        request: GenerateRequest,
    ) -> Result<GenerateResponse, SketchContractKitError> {
        <Self as SketchContractKitBackend>::generate(self, request).await
    }

    /// Compares current sketch contracts with a previous contract catalog.
    ///
    /// Sketch identifiers are identity. For a sketch present in both catalogs,
    /// the linked logical source file, linked signature label, `signature_type`,
    /// and normalized code are semantic. The containing contract document, YAML
    /// formatting, YAML comments outside `code`, and mapping order are
    /// nonsemantic.
    ///
    /// Normalization removes blank lines and collapses whitespace within each
    /// line, so those changes alone are ignored. Line order and every token or
    /// comment inside `code` remain semantic.
    ///
    /// # Errors
    ///
    /// Returns [`SketchContractKitError`] when either catalog contains invalid
    /// sketch contract input or background work cannot complete.
    ///
    /// # Panics
    ///
    /// If the background operation panics on a Rayon worker, its panic payload
    /// resumes unwinding on the thread that polls this future to completion.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_sketch::{CatalogPath, DiffEntry, DiffRequest, FileCatalog, SketchContractKit};
    ///
    /// fn contract_catalog(code: &str) -> Result<FileCatalog, Box<dyn std::error::Error>> {
    ///     let yaml = format!(r#"root: ../src
    /// files: [lib.rs]
    /// signatures:
    ///   - answer_signature:
    ///       file: lib.rs
    ///       signature_type: function
    ///       sketch: answer_body
    /// sketches:
    ///   - answer_body:
    ///     signature_type: function
    ///     code: {code}
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
    /// assert!(response.changed);
    /// assert_eq!(
    ///     response.entries,
    ///     vec![DiffEntry::Changed {
    ///         sketch_id: "answer_body".to_owned(),
    ///     }],
    /// );
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn diff(&self, request: DiffRequest) -> Result<DiffResponse, SketchContractKitError> {
        <Self as SketchContractKitBackend>::diff(self, request).await
    }
}

pub(crate) trait SketchContractKitBackend {
    async fn check(&self, request: CheckRequest) -> Result<CheckResponse, SketchContractKitError>;

    async fn generate(
        &self,
        request: GenerateRequest,
    ) -> Result<GenerateResponse, SketchContractKitError>;

    async fn diff(&self, request: DiffRequest) -> Result<DiffResponse, SketchContractKitError>;
}

impl SketchContractKitBackend for SketchContractKit {
    async fn check(&self, request: CheckRequest) -> Result<CheckResponse, SketchContractKitError> {
        match &self.inner {
            SketchContractKitInner::Local(backend) => backend.check(request).await,
        }
    }

    async fn generate(
        &self,
        request: GenerateRequest,
    ) -> Result<GenerateResponse, SketchContractKitError> {
        match &self.inner {
            SketchContractKitInner::Local(backend) => backend.generate(request).await,
        }
    }

    async fn diff(&self, request: DiffRequest) -> Result<DiffResponse, SketchContractKitError> {
        match &self.inner {
            SketchContractKitInner::Local(backend) => backend.diff(request).await,
        }
    }
}

struct LocalSketchContractKit {
    work: AsyncWorkPool,
}

impl LocalSketchContractKit {
    fn new(work: WorkOptions) -> Result<Self, SketchContractKitError> {
        Ok(Self {
            work: AsyncWorkPool::new(work)?,
        })
    }
}

impl SketchContractKitBackend for LocalSketchContractKit {
    async fn check(&self, request: CheckRequest) -> Result<CheckResponse, SketchContractKitError> {
        self.work
            .execute(move || SketchCheck::new(request).run())
            .await?
    }

    async fn generate(
        &self,
        request: GenerateRequest,
    ) -> Result<GenerateResponse, SketchContractKitError> {
        self.work
            .execute(move || SketchGenerator::new(request).generate())
            .await?
    }

    async fn diff(&self, request: DiffRequest) -> Result<DiffResponse, SketchContractKitError> {
        self.work
            .execute(move || SketchDiff::new(request).run())
            .await?
    }
}

struct SketchDiff {
    request: DiffRequest,
}

impl SketchDiff {
    fn new(request: DiffRequest) -> Self {
        Self { request }
    }

    fn run(self) -> Result<DiffResponse, SketchContractKitError> {
        let current = SketchContracts::from_catalog(self.request.current_contract_files)?;
        let previous = SketchContracts::from_catalog(self.request.previous_contract_files)?;

        Ok(current.diff_against(&previous))
    }
}

struct SketchCheck {
    request: CheckRequest,
}

impl SketchCheck {
    fn new(request: CheckRequest) -> Self {
        Self { request }
    }

    fn run(self) -> Result<CheckResponse, SketchContractKitError> {
        let CheckRequest {
            source_files,
            contract_files,
            report,
            mode,
        } = self.request;
        let contracts = SketchContracts::from_catalog(contract_files)?;
        let sources = SourceCatalog::from_catalog(source_files, &contracts);
        let comparison = SketchMatcher::new(sources, contracts).check()?;
        let mut response = comparison.into_response(mode);

        response.report_files = ReportFiles::new(report).render(&response)?;
        Ok(response)
    }
}

/// Controls how diagnostics affect [`CheckResponse::passed`].
///
/// Diagnostics are always retained in [`CheckResponse::diagnostics`]. The mode
/// changes only the response's pass/fail value.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CheckMode {
    /// Fail the response when one or more diagnostics are present.
    ///
    /// This currently has the same pass/fail behavior as [`CheckMode::Strict`].
    Default,
    /// Fail the response when one or more diagnostics are present.
    ///
    /// This currently has the same pass/fail behavior as [`CheckMode::Default`].
    Strict,
    /// Preserve diagnostics while keeping the response passing.
    Warning,
}

/// Request for [`SketchContractKit::check`].
///
/// The caller owns filesystem discovery and provides both source and contract
/// bytes as [`FileCatalog`] values. The complete source catalog contributes to
/// [`SketchCheckCounts::source_file_count`], while matching normalizes only
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
///     mode: CheckMode::Default,
/// };
///
/// assert_eq!(request.mode, CheckMode::Default);
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

/// Response returned by [`SketchContractKit::check`].
///
/// Contract syntax and validation failures are returned as
/// [`SketchContractKitError`], not stored here. This response contains ordinary
/// match outcomes, with diagnostics sorted deterministically by sketch
/// identifier, optional file path, and diagnostic kind.
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

/// Request for [`SketchContractKit::generate`].
///
/// Generation consumes combined contract bytes plus exact linked-sketch seeds
/// and returns the updated catalog; it never reads or writes operating-system
/// paths.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GenerateRequest {
    /// Existing combined contract documents to update.
    pub contract_files: FileCatalog,
    /// One exact refresh seed for every explicitly linked sketch.
    pub seeds: Vec<SketchSeed>,
}

/// Response returned by [`SketchContractKit::generate`].
///
/// The response contains the complete input catalog. Output entries retain
/// deterministic logical path order, updated documents preserve their existing
/// sketch order, and nested YAML, non-YAML entries, and untargeted root
/// documents pass through byte-for-byte. Identical requests produce identical
/// catalog order and bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GenerateResponse {
    /// Complete contract catalog, including unchanged passthrough entries.
    pub contract_files: FileCatalog,
    /// Number of explicitly linked sketches refreshed.
    pub sketch_count: usize,
}

/// Request for [`SketchContractKit::diff`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiffRequest {
    /// Current sketch contract catalog.
    pub current_contract_files: FileCatalog,
    /// Previous sketch contract catalog.
    pub previous_contract_files: FileCatalog,
}

/// Response returned by [`SketchContractKit::diff`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiffResponse {
    /// Whether any sketch was added, removed, or semantically changed.
    pub changed: bool,
    /// Deterministically ordered sketch changes.
    pub entries: Vec<DiffEntry>,
}

/// One semantic sketch change returned by [`DiffResponse`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DiffEntry {
    /// A sketch exists only in the current catalog.
    Added {
        /// User-defined sketch identifier.
        sketch_id: String,
    },
    /// A sketch exists only in the previous catalog.
    Removed {
        /// User-defined sketch identifier.
        sketch_id: String,
    },
    /// A sketch exists in both catalogs with different semantic fields.
    Changed {
        /// User-defined sketch identifier.
        sketch_id: String,
    },
}

/// Caller-supplied refresh data for one linked sketch.
///
/// A signature-domain resolver can produce this runtime-neutral data from an
/// exact source span. The `conkit-sketch` crate validates it against the combined
/// document without depending on a signature parser.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SketchSeed {
    /// Combined document containing both the signature link and sketch.
    pub contract_file: CatalogPath,
    /// User-facing flattened sketch identifier.
    pub sketch_id: String,
    /// Signature kind copied from the linked signature.
    pub signature_type: String,
    /// Logical source entry declared by the linked signature.
    pub file: CatalogPath,
    /// Exact source text, required to be nonempty after normalization.
    pub code: String,
}
