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
//! Rust signatures live in mandatory `contract_version: 2` combined YAML
//! documents with a versioned `rust_syntax_v2` or `rust_compiler_v1`
//! extraction context, `profile: rust_api_v1`, explicit typed crate roots,
//! `root`, an exact `files` allowlist, user-named `signatures`,
//! signature-to-sketch links, and nested `sketches`. Compiler documents also
//! retain compiler/rustdoc schema, target triple, sorted features and cfg,
//! package/target identity, and resolution capabilities. Missing, legacy,
//! future, mismatched, or unknown extraction versions fail closed. Every
//! decoded or mapped Rust source belongs to the owning document's allowlist.
//!
//! The syntax extractor receives crate roots and target kinds from v2 metadata,
//! then builds canonical module identity from inline modules, out-of-line `mod`
//! declarations, and `#[path]`. It does not guess logical modules from arbitrary
//! source paths or fall back to global bare-name implementation-owner lookup.
//! Structurally repeatable items with the same crate, module path, item kind,
//! and semantic name use one-based declaration occurrence as their identity:
//! the first is unsuffixed, followed by `#2`, `#3`, and so on. Regeneration
//! retains user labels by occurrence, and linked-sketch resolution uses that
//! occurrence to return the exact Rust item text.
//!
//! `rust_syntax_v2` does not run Cargo, compile crates, expand macros, evaluate
//! conditional compilation, or perform compiler name resolution. It retains
//! modeled syntax and reports deterministic capability warnings for facts that
//! need compiler context. [`CheckMode::Default`] permits warning-only results,
//! [`CheckMode::Strict`] requires no diagnostics, and [`CheckMode::Warning`]
//! preserves all diagnostics while passing a completed check. Unsupported
//! reachable syntax and invalid attributes, module graphs, visibility, or
//! implementation ownership return typed errors instead of being omitted.
//! `rust_compiler_v1` instead consumes a bounded, host-produced rustdoc JSON
//! artifact. The host owns Cargo/toolchain/process work; this crate validates
//! its schema and tagged exact/compiler-generated source provenance and
//! converts it into the same declaration and inventory graph used by syntax
//! extraction. Compiler extraction retains cfg-selected and macro-generated
//! public items, expands rustdoc-exposed direct and glob reexports, and
//! normalizes generic type-alias applications. A summary-only external glob
//! target or another rustdoc fact lacking a lossless representation fails
//! explicitly. Compiler-generated items participate in signatures but cannot
//! supply exact source text for linked sketches.
//!
//! Functions named `main` are ordinary functions. Equivalent visibility
//! spellings canonicalize to the same semantic visibility. Parameter patterns
//! remain rendering metadata, while `rust_api_v1` digest bytes include callable
//! types and receiver form but exclude ordinary binding and destructuring names.
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
//! [`CheckResponse`] also provides borrowed serializable report views. The
//! standalone and embedded views omit returned report files while preserving
//! their established wire layouts, so storage adapters can compose reports
//! without mirroring signature-domain fields.
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
//! [`WorkOptions`] documents the complete worker-admission and cooperative
//! cancellation contract. Examples here use `futures_executor::block_on` only to
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
//!     RustCrateKind, RustCrateRoot, SignatureContractKit,
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
//!     extraction: conkit_signature::RustExtractionInput::Syntax,
//!     source_files: source_files.clone(),
//!     target: GenerateTarget::New(GenerateDocument {
//!         contract_file: CatalogPath::new("main.yml")?,
//!         root: "../src".to_owned(),
//!         files: vec![CatalogPath::new("lib.rs")?],
//!         crates: vec![RustCrateRoot {
//!             id: "sample".to_owned(),
//!             root: CatalogPath::new("lib.rs")?,
//!             kind: RustCrateKind::Library,
//!         }],
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
//!     extraction: conkit_signature::RustExtractionInput::Syntax,
//!     source_files,
//!     contract_files: generated.contract_files,
//!     report: ReportRequest::None,
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
mod limits;
mod work;

pub use crate::api::{
    CheckDiagnostic, CheckDiagnosticCategory, CheckMode, CheckRequest, CheckResponse,
    ContractScope, DiagnosticSeverity, DiffCategory, DiffEntry, DiffRequest, DiffResponse,
    GenerateDocument, GenerateRequest, GenerateResponse, GenerateTarget, ReportFormat,
    ReportRequest, ResolveSketchesRequest, ResolveSketchesResponse, ResolvedSketchSeed,
    RustCrateKind, RustCrateRoot, RustExtractionInput, SignatureCheckCounts, SignatureContractKit,
    SignatureContractKitBuilder, SignatureGenerationCounts,
};
pub use crate::error::SignatureContractKitError;
pub use crate::files::{CatalogPath, FileCatalog, FileCatalogError};
pub use crate::languages::rust::rustdoc::{
    CompilerSourcePath, CompilerSourceProvenance, RUST_COMPILER_ARTIFACT_SCHEMA_VERSION,
    RUSTDOC_FORMAT_VERSION, RustCompilerArtifact, RustCompilerArtifactFailure, RustCompilerCrate,
};
pub use crate::limits::{
    CatalogLimits, DiagnosticLimits, LimitExceeded, LimitResource, OutputLimits,
    RustExtractionLimits, SignatureLimits, YamlLimits,
};
pub use crate::work::{WorkOptions, WorkerPool};
