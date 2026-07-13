//! Byte-in, byte-out signature contracts for Contract Kit.
//!
//! The `conkit-signature` crate generates signature contracts from Rust source
//! bytes, checks source bytes against existing contracts, resolves linked
//! sketches to exact Rust items, and diffs current contracts against previous
//! contract catalogs.
//! It does not read from or write to the operating system. Callers provide
//! logical [`CatalogPath`] names and bytes through [`FileCatalog`], then decide
//! where returned catalog entries should be stored.
//!
//! Rust signatures live in combined YAML documents with `root`, an exact
//! `files` allowlist, user-named nested `signatures`, signature-to-sketch links,
//! and flattened `sketches`. Versioned `language: rust` shorthand is rejected.
//! Repeated item macros with the same file, module path, and semantic name use
//! one-based declaration occurrence as their structural identity: the first
//! occurrence is unsuffixed, followed by `#2`, `#3`, and so on. Regeneration
//! retains user labels by occurrence, and linked-sketch resolution uses that
//! same occurrence to return the exact Rust item text.
//!
//! # Operations
//!
//! - [`SignatureContractKit::check`] compares actual source signatures with
//!   expected contract signatures and can return a rendered report catalog.
//! - [`SignatureContractKit::generate`] creates or updates combined contract
//!   documents from Rust source bytes.
//! - [`SignatureContractKit::resolve_sketches`] returns exact source text and
//!   metadata for explicitly linked sketches.
//! - [`SignatureContractKit::diff`] compares grouped signature identities in
//!   current and previous contract catalogs.
//!
//! # Boundaries
//!
//! `conkit-signature` owns signature-domain parsing, matching, generation, and diffing
//! over catalog bytes. Filesystem walking, path normalization, persistence,
//! archive transport, and terminal output belong to callers such as the `conkit`
//! crate. This crate validates and preserves signature-owned sketch-link
//! metadata but does not generate sketch code or perform snippet matching.
//!
//! # Async execution
//!
//! Start with [`SignatureContractKit::builder`] to configure the local kit.
//! Public operation futures are executor-neutral, and direct `.await` is the
//! normal integration. A task that is moved into an executor must own both its
//! request and the kit, usually by moving an [`Arc`](std::sync::Arc) clone into
//! an `async move` block. That owning task future and its output satisfy
//! `Send + 'static`; an operation future called directly still borrows the kit.
//!
//! [`WorkOptions`] documents the complete worker-admission, cancellation, and
//! timeout contract. Examples here use `futures_executor::block_on` only to
//! remain self-contained; the crate does not select that executor for callers.
//!
//! # Examples
//!
//! Generate a contract from Rust source bytes, then check the same source
//! against the generated contract.
//!
//! ```
//! use conkit_signature::{
//!     CatalogPath, CheckMode, CheckRequest, ContractScope, FileCatalog,
//!     GenerateDocument, GenerateRequest, GenerateTarget, ReportRequest,
//!     SignatureContractKit,
//! };
//!
//! fn catalog_with(path: &str, bytes: &[u8]) -> Result<FileCatalog, Box<dyn std::error::Error>> {
//!     let mut catalog = FileCatalog::new();
//!     catalog.insert(CatalogPath::new(path)?, bytes.to_vec())?;
//!     Ok(catalog)
//! }
//!
//! let kit = SignatureContractKit::builder().build()?;
//! let source_files = catalog_with("lib.rs", b"pub fn answer() -> u8 { 42 }\n")?;
//!
//! let generated = futures_executor::block_on(kit.generate(GenerateRequest {
//!     source_files: source_files.clone(),
//!     target: GenerateTarget::New(GenerateDocument {
//!         contract_file: CatalogPath::new("main.yml")?,
//!         root: "../src".to_owned(),
//!         files: vec![CatalogPath::new("lib.rs")?],
//!     }),
//!     scope: ContractScope::Signatures,
//! }))?;
//!
//! let contract_path = CatalogPath::new("main.yml")?;
//! let contract_yaml = std::str::from_utf8(
//!     generated.contract_files.get(&contract_path).expect("generated contract"),
//! )?;
//! assert!(contract_yaml.contains("root: ../src"));
//! assert!(contract_yaml.contains("files:"));
//! assert!(contract_yaml.contains("signature_type: function"));
//!
//! let response = futures_executor::block_on(kit.check(CheckRequest {
//!     source_files,
//!     contract_files: generated.contract_files,
//!     report: ReportRequest::None,
//!     scope: ContractScope::Signatures,
//!     mode: CheckMode::Default,
//! }))?;
//!
//! assert!(response.passed);
//! assert!(response.diagnostics.is_empty());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]
#![deny(rustdoc::private_intra_doc_links)]

mod api;
mod error;
mod files;
mod inventory;
mod languages;
mod work;

pub use crate::api::{
    CheckDiagnostic, CheckMode, CheckRequest, CheckResponse, ContractScope, DiffEntry, DiffRequest,
    DiffResponse, GenerateDocument, GenerateRequest, GenerateResponse, GenerateTarget,
    ReportFormat, ReportRequest, ResolveSketchesRequest, ResolveSketchesResponse,
    ResolvedSketchSeed, SignatureCheckCounts, SignatureContractKit, SignatureContractKitBuilder,
};
pub use crate::error::SignatureContractKitError;
pub use crate::files::{CatalogPath, FileCatalog, FileCatalogError};
pub use crate::work::{WorkOptions, WorkParallelism};
