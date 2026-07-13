use crate::error::SignatureContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::inventory::{
    InventoryComparison, InventoryDiagnostic, InventoryDiff, InventoryDiffEntry,
};
use crate::languages::{SignatureParser, SignatureParserBackend};
use crate::work::{AsyncWorkPool, WorkOptions};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Public handle for signature contract operations.
///
/// Build a kit with [`SignatureContractKit::builder`], then call the async
/// methods with in-memory catalog requests. Each handle owns and reuses one
/// local Rayon pool for complete CPU-bound workflows; the returned futures
/// remain independent of any particular async runtime. The selected worker
/// count also bounds admitted root operations. See [`WorkOptions`] for the
/// complete admission, cancellation, caller-deadline, and scheduling contract.
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
///     GenerateTarget, SignatureContractKit,
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
///     source_files,
///     target: GenerateTarget::New(GenerateDocument {
///         contract_file: CatalogPath::new("main.yml")?,
///         root: "../src".to_owned(),
///         files: vec![CatalogPath::new("lib.rs")?],
///     }),
///     scope: ContractScope::Signatures,
/// };
/// let task_kit = Arc::clone(&kit);
/// let task = async move { task_kit.generate(request).await };
/// let response = futures_executor::block_on(task)?;
///
/// assert_eq!(response.signature_count, 1);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct SignatureContractKit {
    inner: SignatureContractKitInner,
}

enum SignatureContractKitInner {
    Local(LocalSignatureContractKit),
}

#[derive(Default)]
/// Builder for [`SignatureContractKit`].
///
/// The default builder uses
/// [`WorkParallelism::RuntimeDefault`](crate::WorkParallelism::RuntimeDefault),
/// deriving root-operation admission from Rayon's selected worker count. Use
/// [`SignatureContractKitBuilder::with_work_options`] with
/// [`WorkParallelism::Fixed`](crate::WorkParallelism::Fixed) to select one
/// explicit non-zero value for both worker count and admitted root operations.
/// See [`WorkOptions`] for the complete scheduling contract.
///
/// # Examples
///
/// ```
/// use conkit_signature::{SignatureContractKitBuilder, WorkOptions, WorkParallelism};
/// use std::num::NonZeroUsize;
///
/// let kit = SignatureContractKitBuilder::default()
///     .with_work_options(WorkOptions {
///         parallelism: WorkParallelism::Fixed(NonZeroUsize::MIN),
///     })
///     .build()?;
/// # let _ = kit;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct SignatureContractKitBuilder {
    work: WorkOptions,
}

impl SignatureContractKitBuilder {
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
    /// use conkit_signature::{SignatureContractKitBuilder, WorkOptions, WorkParallelism};
    /// use std::num::NonZeroUsize;
    ///
    /// let builder = SignatureContractKitBuilder::default().with_work_options(WorkOptions {
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

    /// Builds a local signature contract kit.
    ///
    /// # Errors
    ///
    /// Returns [`SignatureContractKitError`] if the internal work pool cannot
    /// be initialized.
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
            inner: SignatureContractKitInner::Local(LocalSignatureContractKit::new(self.work)?),
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
    /// Rust contract files are interpreted as combined `root`/`files` YAML documents;
    /// callers decide how those bytes are read from or written to local files.
    ///
    /// # Errors
    ///
    /// Returns [`SignatureContractKitError`] when source parsing, contract
    /// parsing, comparison, or optional report rendering fails, or when
    /// background work does not complete.
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
    ///     GenerateDocument, GenerateRequest, GenerateTarget, ReportRequest, SignatureContractKit,
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
    ///     source_files: source_files.clone(),
    ///     target: GenerateTarget::New(GenerateDocument {
    ///         contract_file: CatalogPath::new("main.yml")?,
    ///         root: "../src".to_owned(),
    ///         files: vec![CatalogPath::new("lib.rs")?],
    ///     }),
    ///     scope: ContractScope::Signatures,
    /// }))?;
    ///
    /// let response = futures_executor::block_on(kit.check(CheckRequest {
    ///     source_files,
    ///     contract_files: generated.contract_files,
    ///     report: ReportRequest::None,
    ///     scope: ContractScope::Signatures,
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
        <Self as SignatureContractKitBackend>::check(self, request).await
    }

    /// Generates contract files from source files.
    ///
    /// Rust source entries produce combined user-named YAML contract documents.
    ///
    /// # Errors
    ///
    /// Returns [`SignatureContractKitError`] when Rust source decoding,
    /// parsing, ownership resolution, or contract conversion fails; when a new
    /// or existing document layout is invalid; when a participating existing
    /// document fails YAML, ownership, or link validation; when labels or
    /// output bytes cannot be rendered; or when background work does not
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
    /// use conkit_signature::{CatalogPath, ContractScope, FileCatalog, GenerateDocument, GenerateRequest, GenerateTarget, SignatureContractKit};
    ///
    /// let kit = SignatureContractKit::builder().build()?;
    /// let mut source_files = FileCatalog::new();
    /// source_files.insert(
    ///     CatalogPath::new("lib.rs")?,
    ///     b"pub fn answer() -> u8 { 42 }\n".to_vec(),
    /// )?;
    ///
    /// let response = futures_executor::block_on(kit.generate(GenerateRequest {
    ///     source_files,
    ///     target: GenerateTarget::New(GenerateDocument {
    ///         contract_file: CatalogPath::new("main.yml")?,
    ///         root: "../src".to_owned(),
    ///         files: vec![CatalogPath::new("lib.rs")?],
    ///     }),
    ///     scope: ContractScope::Signatures,
    /// }))?;
    ///
    /// assert_eq!(response.signature_count, 1);
    /// assert_eq!(response.sketch_count, 0);
    /// assert!(response.contract_files.get(&CatalogPath::new("main.yml")?).is_some());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn generate(
        &self,
        request: GenerateRequest,
    ) -> Result<GenerateResponse, SignatureContractKitError> {
        <Self as SignatureContractKitBackend>::generate(self, request).await
    }

    /// Resolves every explicitly linked sketch to its exact Rust source item.
    ///
    /// Item macros with the same file, module path, and semantic name are
    /// distinguished by one-based declaration occurrence. Contract signature
    /// order reconstructs those occurrences, so each link selects the exact
    /// corresponding macro item text.
    ///
    /// # Errors
    ///
    /// Returns [`SignatureContractKitError`] when Rust source bytes cannot be
    /// decoded or parsed; when a participating combined document fails YAML,
    /// layout, ownership, or link validation; when linked source files, exact
    /// Rust items, or valid source spans cannot be found; or when background
    /// work does not complete.
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
    ///         source_files: catalog(
    ///             "lib.rs",
    ///             b"include!(\"first.rs\");\ninclude!(\"second.rs\");\n",
    ///         )?,
    ///         contract_files: catalog(
    ///             "main.yml",
    ///             br#"root: ../src
    /// files: [lib.rs]
    /// signatures:
    ///   - first_include:
    ///       file: lib.rs
    ///       signature_type: macro
    ///       name: include
    ///       sketch: first
    ///   - second_include:
    ///       file: lib.rs
    ///       signature_type: macro
    ///       name: include
    ///       sketch: second
    /// sketches:
    ///   - first:
    ///     signature_type: macro
    ///     code: old
    ///   - second:
    ///     signature_type: macro
    ///     code: old
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
        <Self as SignatureContractKitBackend>::resolve_sketches(self, request).await
    }

    /// Diffs current signature contract files against previous contract files.
    ///
    /// # Errors
    ///
    /// Returns [`SignatureContractKitError`] when contract parsing or inventory
    /// diffing fails, or when background work does not complete.
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
    ///     GenerateRequest, GenerateTarget, SignatureContractKit,
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
    ///     source_files: source("lib.rs", b"pub fn answer() {}\n")?,
    ///     target: GenerateTarget::New(GenerateDocument {
    ///         contract_file: CatalogPath::new("main.yml")?,
    ///         root: "../src".to_owned(),
    ///         files: vec![CatalogPath::new("lib.rs")?],
    ///     }),
    ///     scope: ContractScope::Signatures,
    /// }))?
    /// .contract_files;
    /// let diff = futures_executor::block_on(kit.diff(DiffRequest {
    ///     current_contract_files: contracts.clone(),
    ///     previous_contract_files: contracts,
    /// }))?;
    ///
    /// assert!(!diff.changed);
    /// assert!(diff.entries.is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub async fn diff(
        &self,
        request: DiffRequest,
    ) -> Result<DiffResponse, SignatureContractKitError> {
        <Self as SignatureContractKitBackend>::diff(self, request).await
    }
}

pub(crate) trait SignatureContractKitBackend {
    async fn check(
        &self,
        request: CheckRequest,
    ) -> Result<CheckResponse, SignatureContractKitError>;

    async fn generate(
        &self,
        request: GenerateRequest,
    ) -> Result<GenerateResponse, SignatureContractKitError>;

    async fn resolve_sketches(
        &self,
        request: ResolveSketchesRequest,
    ) -> Result<ResolveSketchesResponse, SignatureContractKitError>;

    async fn diff(&self, request: DiffRequest) -> Result<DiffResponse, SignatureContractKitError>;
}

impl SignatureContractKitBackend for SignatureContractKit {
    async fn check(
        &self,
        request: CheckRequest,
    ) -> Result<CheckResponse, SignatureContractKitError> {
        match &self.inner {
            SignatureContractKitInner::Local(inner) => inner.check(request).await,
        }
    }

    async fn generate(
        &self,
        request: GenerateRequest,
    ) -> Result<GenerateResponse, SignatureContractKitError> {
        match &self.inner {
            SignatureContractKitInner::Local(inner) => inner.generate(request).await,
        }
    }

    async fn resolve_sketches(
        &self,
        request: ResolveSketchesRequest,
    ) -> Result<ResolveSketchesResponse, SignatureContractKitError> {
        match &self.inner {
            SignatureContractKitInner::Local(inner) => inner.resolve_sketches(request).await,
        }
    }

    async fn diff(&self, request: DiffRequest) -> Result<DiffResponse, SignatureContractKitError> {
        match &self.inner {
            SignatureContractKitInner::Local(inner) => inner.diff(request).await,
        }
    }
}

struct SignatureCheck {
    parser: Arc<SignatureParser>,
    request: CheckRequest,
}

impl SignatureCheck {
    fn new(parser: Arc<SignatureParser>, request: CheckRequest) -> Self {
        Self { parser, request }
    }

    fn run(self) -> Result<CheckResponse, SignatureContractKitError> {
        let CheckRequest {
            source_files,
            contract_files,
            report,
            scope: _,
            mode,
        } = self.request;
        let (source, contract) = self
            .parser
            .parse_check_inventories(source_files, contract_files)?;
        let comparison = source.compare_against(&contract)?;
        let mut response = CheckResponse::from_inventory_comparison(comparison, mode);

        response.report_files = ReportFiles::new(report).render(&response)?;
        Ok(response)
    }
}

struct SignatureDiff {
    parser: Arc<SignatureParser>,
    request: DiffRequest,
}

impl SignatureDiff {
    fn new(parser: Arc<SignatureParser>, request: DiffRequest) -> Self {
        Self { parser, request }
    }

    fn run(self) -> Result<DiffResponse, SignatureContractKitError> {
        let DiffRequest {
            current_contract_files,
            previous_contract_files,
        } = self.request;
        let current = self
            .parser
            .parse_contract_inventory(current_contract_files)?;
        let previous = self
            .parser
            .parse_contract_inventory(previous_contract_files)?;

        Ok(DiffResponse::from_inventory_diff(
            current.diff_against(&previous)?,
        ))
    }
}

struct LocalSignatureContractKit {
    parser: Arc<SignatureParser>,
    work: AsyncWorkPool,
}

impl LocalSignatureContractKit {
    fn new(work: WorkOptions) -> Result<Self, SignatureContractKitError> {
        Ok(Self {
            parser: Arc::new(SignatureParser::default()),
            work: AsyncWorkPool::new(work)?,
        })
    }
}

impl SignatureContractKitBackend for LocalSignatureContractKit {
    async fn check(
        &self,
        request: CheckRequest,
    ) -> Result<CheckResponse, SignatureContractKitError> {
        let parser = Arc::clone(&self.parser);

        self.work
            .execute(move || SignatureCheck::new(parser, request).run())
            .await?
    }

    async fn generate(
        &self,
        request: GenerateRequest,
    ) -> Result<GenerateResponse, SignatureContractKitError> {
        let parser = Arc::clone(&self.parser);

        self.work
            .execute(move || {
                let GenerateRequest {
                    source_files,
                    target,
                    scope,
                } = request;
                parser.generate_contract_files(source_files, target, scope)
            })
            .await?
    }

    async fn resolve_sketches(
        &self,
        request: ResolveSketchesRequest,
    ) -> Result<ResolveSketchesResponse, SignatureContractKitError> {
        let parser = Arc::clone(&self.parser);

        self.work
            .execute(move || parser.resolve_sketches(request))
            .await?
    }

    async fn diff(&self, request: DiffRequest) -> Result<DiffResponse, SignatureContractKitError> {
        let parser = Arc::clone(&self.parser);

        self.work
            .execute(move || SignatureDiff::new(parser, request).run())
            .await?
    }
}

struct ReportFiles {
    request: ReportRequest,
}

impl ReportFiles {
    fn new(request: ReportRequest) -> Self {
        Self { request }
    }

    fn render(&self, response: &CheckResponse) -> Result<FileCatalog, SignatureContractKitError> {
        match &self.request {
            ReportRequest::None => Ok(FileCatalog::new()),
            ReportRequest::Generate {
                format,
                output_file,
            } => {
                let report = CheckReport::from_response(response);
                let bytes = match format {
                    ReportFormat::Yaml => serde_yaml::to_string(&report)
                        .map(String::into_bytes)
                        .map_err(|source| {
                            SignatureContractKitError::write_failed(output_file, source.to_string())
                        })?,
                    ReportFormat::Json => serde_json::to_vec_pretty(&report).map_err(|source| {
                        SignatureContractKitError::write_failed(output_file, source.to_string())
                    })?,
                };
                let mut files = FileCatalog::new();
                files.insert(output_file.clone(), bytes)?;

                Ok(files)
            }
        }
    }
}

#[derive(Serialize)]
struct CheckReport<'a> {
    passed: bool,
    counts: &'a SignatureCheckCounts,
    inventory_digest: &'a Option<String>,
    diagnostics: &'a [CheckDiagnostic],
}

impl<'a> CheckReport<'a> {
    fn from_response(response: &'a CheckResponse) -> Self {
        Self {
            passed: response.passed,
            counts: &response.counts,
            inventory_digest: &response.inventory_digest,
            diagnostics: &response.diagnostics,
        }
    }
}

/// Selects whether signature generation may coordinate linked-sketch cleanup.
///
/// Checking currently treats both variants identically and compares the same
/// signature inventory.
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
///     br#"root: ../src
/// files: [main.rs]
/// signatures:
///   - main:
///       file: main.rs
///       signature_type: main_method
///       sketch: main
/// sketches:
///   - main:
///     signature_type: main_method
///     code: |
///       fn main() {}
/// "#,
/// )?;
///
/// let signatures_only = futures_executor::block_on(kit.generate(GenerateRequest {
///     source_files: source_files.clone(),
///     target: GenerateTarget::Existing(existing.clone()),
///     scope: ContractScope::Signatures,
/// }));
/// assert!(signatures_only.unwrap_err().to_string().contains("orphan"));
///
/// let all = futures_executor::block_on(kit.generate(GenerateRequest {
///     source_files,
///     target: GenerateTarget::Existing(existing),
///     scope: ContractScope::All,
/// }))?;
/// let yaml = std::str::from_utf8(
///     all.contract_files
///         .get(&CatalogPath::new("main.yml")?)
///         .expect("updated contract"),
/// )?;
/// assert_eq!(all.signature_count, 0);
/// assert_eq!(all.sketch_count, 0);
/// assert!(!yaml.contains("main_method"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ContractScope {
    /// Coordinate signatures with linked records for an all-family workflow.
    All,
    /// Process signatures while preserving the sketch section.
    Signatures,
}

/// Determines whether diagnostics fail a check response.
///
/// [`CheckMode::Default`] and [`CheckMode::Strict`] currently both fail a
/// response containing any diagnostic. [`CheckMode::Warning`] preserves the
/// diagnostics while allowing the response to pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CheckMode {
    /// Fail the check when the comparison emits any diagnostic.
    Default,
    /// Fail the check when the comparison emits any diagnostic.
    ///
    /// This currently has the same pass/fail behavior as [`CheckMode::Default`].
    Strict,
    /// Keep diagnostics but allow the check response to pass.
    Warning,
}

/// Output encoding for generated check reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ReportFormat {
    /// Render the report as YAML bytes.
    Yaml,
    /// Render the report as pretty JSON bytes.
    Json,
}

/// Request for [`SignatureContractKit::check`].
///
/// The caller owns filesystem discovery and provides both source and contract
/// bytes as [`FileCatalog`] values. Rust contract files use the combined
/// `root`/`files`/user-named `signatures`/flattened `sketches` document shape.
/// Versioned `language: rust` shorthand is rejected.
///
/// # Examples
///
/// ```
/// use conkit_signature::{
///     CheckMode, CheckRequest, ContractScope, FileCatalog, ReportRequest,
/// };
///
/// let request = CheckRequest {
///     source_files: FileCatalog::new(),
///     contract_files: FileCatalog::new(),
///     report: ReportRequest::None,
///     scope: ContractScope::Signatures,
///     mode: CheckMode::Default,
/// };
///
/// assert_eq!(request.scope, ContractScope::Signatures);
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
    /// Rust signature contracts must be root-level combined YAML documents.
    pub contract_files: FileCatalog,
    /// Optional report output request.
    pub report: ReportRequest,
    /// Scope marker for the check.
    ///
    /// Both variants currently compare the same signature inventory.
    pub scope: ContractScope,
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
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CheckResponse {
    /// Whether the check passed under the requested [`CheckMode`].
    pub passed: bool,
    /// Source and contract signature totals.
    pub counts: SignatureCheckCounts,
    /// Digest for the compared, grouped source inventory.
    ///
    /// Every successful check currently returns `Some`; the optional shape is
    /// retained as part of the public response contract.
    pub inventory_digest: Option<String>,
    /// Differences found during the check.
    pub diagnostics: Vec<CheckDiagnostic>,
    /// Generated report files, or an empty catalog when no report was requested.
    pub report_files: FileCatalog,
}

impl CheckResponse {
    fn from_inventory_comparison(comparison: InventoryComparison, mode: CheckMode) -> Self {
        let diagnostics = comparison
            .diagnostics()
            .iter()
            .map(CheckDiagnostic::from_inventory_diagnostic)
            .collect::<Vec<_>>();
        let passed = diagnostics.is_empty() || mode == CheckMode::Warning;

        Self {
            passed,
            counts: SignatureCheckCounts {
                source_signature_count: comparison.source_signature_count(),
                contract_signature_count: comparison.contract_signature_count(),
            },
            inventory_digest: Some(comparison.inventory_digest().as_str().to_owned()),
            diagnostics,
            report_files: FileCatalog::new(),
        }
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
///     CatalogPath, CheckDiagnostic, CheckMode, CheckRequest, ContractScope,
///     FileCatalog, GenerateDocument, GenerateRequest, GenerateTarget, ReportRequest,
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
///     source_files: expected_source,
///     target: GenerateTarget::New(GenerateDocument {
///         contract_file: CatalogPath::new("main.yml")?,
///         root: "../src".to_owned(),
///         files: vec![CatalogPath::new("lib.rs")?],
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
///     source_files: actual_source,
///     contract_files: expected_contracts,
///     report: ReportRequest::None,
///     scope: ContractScope::Signatures,
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
    /// A warning diagnostic representation.
    ///
    /// The current checker does not emit this variant. The variant itself is
    /// not inherently non-failing; [`CheckMode`] determines response pass/fail
    /// behavior.
    Warning {
        /// Warning text.
        message: String,
    },
}

impl CheckDiagnostic {
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
                expected_digest: expected_digest.as_str().to_owned(),
                actual_digest: actual_digest.as_str().to_owned(),
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
    /// Repeated item macros with the same file, module path, and semantic name
    /// retain labels by one-based declaration occurrence, even when their token
    /// text changes.
    ///
    /// Only direct root-level `.yml` and `.yaml` entries are considered and
    /// returned. Nested YAML and non-YAML catalog entries are ignored and
    /// omitted from [`GenerateResponse::contract_files`]. With
    /// [`ContractScope::Signatures`], generation preserves linked sketches and
    /// rejects signature removal that would orphan one. With
    /// [`ContractScope::All`], it removes the stale linked sketch record along
    /// with the signature.
    Existing(FileCatalog),
    /// Create one new combined contract document with the requested layout.
    New(GenerateDocument),
}

/// Layout used when creating a combined contract document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GenerateDocument {
    /// Logical catalog path for the generated YAML document.
    pub contract_file: CatalogPath,
    /// User-facing source root written to the document.
    pub root: String,
    /// Exact portable Rust file allowlist owned by the document.
    pub files: Vec<CatalogPath>,
}

/// Response returned by [`SignatureContractKit::generate`].
///
/// Rust signature generation emits combined YAML with `root`, an exact `files`
/// allowlist, user-named nested `signatures`, and flattened `sketches`.
///
/// # Examples
///
/// ```
/// use conkit_signature::{CatalogPath, ContractScope, FileCatalog, GenerateDocument, GenerateRequest, GenerateTarget, SignatureContractKit};
///
/// let kit = SignatureContractKit::builder().build()?;
/// let mut source_files = FileCatalog::new();
/// source_files.insert(
///     CatalogPath::new("lib.rs")?,
///     b"pub unsafe extern \"C\" fn c_api(value: i32) -> i32 { value }\n".to_vec(),
/// )?;
///
/// let response = futures_executor::block_on(kit.generate(GenerateRequest {
///     source_files,
///     target: GenerateTarget::New(GenerateDocument {
///         contract_file: CatalogPath::new("main.yml")?,
///         root: "../src".to_owned(),
///         files: vec![CatalogPath::new("lib.rs")?],
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
    /// Number of top-level grouped signature records generated.
    pub signature_count: usize,
    /// Number of linked sketch records retained in the returned documents.
    ///
    /// The `conkit-signature` crate does not generate their code; the caller resolves
    /// surviving links and delegates refresh to the sketch domain.
    pub sketch_count: usize,
}

/// Request for [`SignatureContractKit::resolve_sketches`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolveSketchesRequest {
    /// Rust source bytes indexed by portable source-relative paths.
    pub source_files: FileCatalog,
    /// Combined root-level YAML documents containing links and sketches.
    pub contract_files: FileCatalog,
}

/// Exact linked Rust items returned by sketch resolution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolveSketchesResponse {
    /// Linked sketch seeds ordered by contract path, then sketch identifier.
    pub seeds: Vec<ResolvedSketchSeed>,
}

/// Runtime-neutral source seed for one explicitly linked sketch.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolvedSketchSeed {
    /// Combined document containing both the link and flattened sketch record.
    pub contract_file: CatalogPath,
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
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiffRequest {
    /// Current contract files to compare.
    pub current_contract_files: FileCatalog,
    /// Previous contract files decoded from an archive or another store.
    pub previous_contract_files: FileCatalog,
}

/// Response returned by [`SignatureContractKit::diff`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiffResponse {
    /// Whether any contract entries changed.
    pub changed: bool,
    /// Diff results for top-level grouped contract identities.
    pub entries: Vec<DiffEntry>,
}

impl DiffResponse {
    fn from_inventory_diff(diff: InventoryDiff) -> Self {
        Self {
            changed: diff.changed(),
            entries: diff
                .entries()
                .iter()
                .map(DiffEntry::from_inventory_diff_entry)
                .collect(),
        }
    }
}

/// One changed top-level signature group reported by [`DiffResponse`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DiffEntry {
    /// A signature group exists in the current contracts but not the previous set.
    Added {
        /// User contract signature label used as the group identity.
        signature_id: String,
    },
    /// A signature group exists in the previous contracts but not the current set.
    Removed {
        /// User contract signature label used as the group identity.
        signature_id: String,
    },
    /// A signature group exists in both contract sets but has different digest bytes.
    Changed {
        /// User contract signature label used as the group identity.
        signature_id: String,
        /// Group digest from the current contract files.
        current_digest: String,
        /// Group digest from the previous contract files.
        previous_digest: String,
    },
}

impl DiffEntry {
    fn from_inventory_diff_entry(entry: &InventoryDiffEntry) -> Self {
        match entry {
            InventoryDiffEntry::Added { signature_id } => Self::Added {
                signature_id: signature_id.as_str().to_owned(),
            },
            InventoryDiffEntry::Removed { signature_id } => Self::Removed {
                signature_id: signature_id.as_str().to_owned(),
            },
            InventoryDiffEntry::Changed {
                signature_id,
                current_digest,
                previous_digest,
            } => Self::Changed {
                signature_id: signature_id.as_str().to_owned(),
                current_digest: current_digest.as_str().to_owned(),
                previous_digest: previous_digest.as_str().to_owned(),
            },
        }
    }
}
