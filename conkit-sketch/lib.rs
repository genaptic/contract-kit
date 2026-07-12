//! Byte-in, byte-out sketch contracts for Contract Kit.
//!
//! The `conkit-sketch` crate checks source bytes against expected code snippets stored
//! in combined contract YAML, refreshes linked sketch code, and compares sketch
//! catalogs semantically. Callers provide logical [`CatalogPath`] names and bytes
//! through [`FileCatalog`], then decide where returned reports or updated combined
//! documents should be stored.
//!
//! Sketch matching is intentionally language-neutral. The crate normalizes
//! contract snippets and source files the same way, then verifies that each
//! expected normalized line sequence appears in the declared source file.
//! A minimal signature index follows each top-level signature's `sketch` link
//! to infer the sketch's source file and signature type. Rust parsing and
//! AST-derived source extraction remain outside this crate.
//!
//! # Contracts and matching
//!
//! Each YAML document contains exactly `root`, `files`, `signatures`, and
//! `sketches`. A flattened sketch record has a null-valued identifier beside
//! `signature_type` and `code`; its linked signature owns the `file` and
//! `sketch` fields. The later `version`/`language` reverse-link dialect is not
//! accepted. Only direct root-level `.yaml` and `.yml` catalog entries are
//! considered; nested YAML entries are ignored.
//!
//! Matching collapses whitespace within each line, removes empty lines, and
//! compares the remaining lines as one contiguous ordered sequence. Source
//! bytes do not need to be UTF-8: malformed source lines use an ASCII-whitespace
//! fallback that preserves every other byte. A sketch still matches only the
//! logical source path declared by its linked signature.
//!
//! # Semantic diffing
//!
//! [`SketchContractKit::diff`] treats the sketch identifier as identity. For a
//! sketch present in both catalogs, the linked source file, linked signature
//! label, `signature_type`, and normalized code are semantic. The containing
//! contract document, YAML formatting, YAML comments outside `code`, and mapping
//! order are nonsemantic.
//!
//! Normalization removes blank lines and collapses whitespace within each line,
//! so those changes alone do not produce a diff. Line order and every token or
//! comment inside `code` remain semantic.
//!
//! # Generation and reports
//!
//! Generation accepts one [`SketchSeed`] for every explicit signature link and
//! refreshes only the flattened sketch's `code`. It preserves the combined
//! document's root, file allowlist, signatures, links, identifiers, and kinds.
//! The response returns the complete input catalog, including unchanged nested
//! YAML and non-YAML passthrough entries, and counts the linked sketches that
//! were refreshed. Returned catalog bytes are deterministic and are never
//! written to the operating system by this crate.
//!
//! A check can return YAML or JSON report bytes through [`ReportRequest`]. The
//! caller chooses the logical output path and persists those bytes if desired.
//!
//! # Errors and diagnostics
//!
//! Malformed YAML, invalid contract fields, duplicate sketch identifiers, and
//! invalid generation seeds return [`SketchContractKitError`]. A valid sketch
//! that references a missing source file or whose snippet does not match is
//! represented by [`SketchDiagnostic`] in a successful [`CheckResponse`].
//! [`CheckMode`] determines whether those diagnostics make the response fail.
//!
//! # Runtime and storage boundaries
//!
//! `conkit-sketch` owns sketch YAML parsing, normalization, matching, semantic
//! diffing, diagnostics, report bytes, and generation from caller-provided
//! resolved seeds. Filesystem walking, path normalization, terminal output, and
//! cross-domain orchestration belong to callers such as the `conkit` crate.
//!
//! Start with [`SketchContractKit::builder`] to configure the local kit and call
//! the async operations. The futures are runtime-neutral; the crate uses a
//! reusable Rayon-backed work pool internally for CPU-bound work. Catalogs,
//! diagnostics, generated entries, and rendered output remain deterministic
//! regardless of worker scheduling.
//!
//! # Examples
//!
//! Refresh a linked sketch, then check the same source against it.
//!
//! ```
//! use conkit_sketch::{
//!     CatalogPath, CheckMode, CheckRequest, FileCatalog, GenerateRequest,
//!     ReportRequest, SketchContractKit, SketchSeed,
//! };
//!
//! let source_path = CatalogPath::new("lib.rs")?;
//! let contract_path = CatalogPath::new("main.yml")?;
//! let mut source_files = FileCatalog::new();
//! source_files.insert(
//!     source_path.clone(),
//!     b"pub fn answer() -> u8 { 42 }\n".to_vec(),
//! )?;
//! let mut contract_files = FileCatalog::new();
//! contract_files.insert(contract_path.clone(), br#"root: ../src
//! files: [lib.rs]
//! signatures:
//!   - answer_signature:
//!       file: lib.rs
//!       signature_type: function
//!       sketch: answer_body
//! sketches:
//!   - answer_body:
//!     signature_type: function
//!     code: old code
//! "#.to_vec())?;
//!
//! let kit = SketchContractKit::builder().build()?;
//! let generated = futures_executor::block_on(kit.generate(GenerateRequest {
//!     contract_files,
//!     seeds: vec![SketchSeed {
//!         contract_file: contract_path.clone(),
//!         sketch_id: "answer_body".to_owned(),
//!         signature_type: "function".to_owned(),
//!         file: source_path.clone(),
//!         code: "pub fn answer() -> u8 { 42 }".to_owned(),
//!     }],
//! }))?;
//!
//! assert_eq!(generated.sketch_count, 1);
//! assert!(generated
//!     .contract_files
//!     .get(&contract_path)
//!     .is_some());
//!
//! let checked = futures_executor::block_on(kit.check(CheckRequest {
//!     source_files,
//!     contract_files: generated.contract_files,
//!     report: ReportRequest::None,
//!     mode: CheckMode::Strict,
//! }))?;
//!
//! assert!(checked.passed);
//! assert!(checked.diagnostics.is_empty());
//! assert_eq!(checked.counts.matched_sketch_count, 1);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]
#![deny(rustdoc::private_intra_doc_links)]

mod api;
mod contract;
mod error;
mod files;
mod generate;
mod id;
mod inventory;
mod matcher;
mod normalize;
mod report;
mod work;

pub use crate::api::{
    CheckMode, CheckRequest, CheckResponse, DiffEntry, DiffRequest, DiffResponse, GenerateRequest,
    GenerateResponse, SketchContractKit, SketchContractKitBuilder, SketchSeed,
};
pub use crate::error::SketchContractKitError;
pub use crate::files::{CatalogPath, FileCatalog, FileCatalogError};
pub use crate::inventory::{SketchCheckCounts, SketchDiagnostic};
pub use crate::report::{ReportFormat, ReportRequest};
pub use crate::work::{WorkOptions, WorkParallelism};
