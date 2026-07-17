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
//! Each YAML document contains exactly `contract_version`, `root`, `files`,
//! `signatures`, and `sketches`, plus `extraction` when signatures are present.
//! Every sketch is a one-entry mapping from its identifier to a nested body
//! containing `file`, `signature`, `signature_type`, `matching`, and `code`.
//! The later `version`/`language` reverse-link dialect is not accepted. Only
//! direct root-level `.yaml` and `.yml` catalog entries are considered; nested
//! YAML entries are ignored.
//!
//! Every v2 sketch declares `matching.normalization: exact_lines_v1` and an
//! occurrence policy. Exact-line normalization converts CRLF to LF and treats
//! one final line terminator as nonsemantic. It preserves indentation, internal
//! and trailing whitespace, tabs, blank lines, isolated carriage returns, and
//! arbitrary non-UTF-8 bytes. [`SketchOccurrence::AtLeastOne`] accepts the first
//! contiguous match; [`SketchOccurrence::ExactlyOne`] requires one occurrence
//! and reports duplicate source spans. Sketch identifiers are exact: the parser
//! rejects empty values, surrounding whitespace, control characters, and
//! over-limit values instead of trimming or Unicode-normalizing them.
//!
//! # Semantic diffing
//!
//! [`SketchContractKit::diff`] treats the sketch identifier as identity. For a
//! sketch present in both catalogs, the linked source file, linked signature
//! label, `signature_type`, [`SketchMatchPolicy`], and normalized code are
//! semantic. The containing contract document, YAML formatting, YAML comments
//! outside `code`, and mapping order are nonsemantic.
//!
//! CRLF/LF spelling and one final line terminator are the only nonsemantic code
//! differences under [`SketchNormalization::ExactLinesV1`]. Line order,
//! indentation, blank lines, horizontal whitespace, tokens, and comments inside
//! `code` remain semantic.
//!
//! # Generation and reports
//!
//! Full generation accepts one [`SketchSeed`] for every explicit signature
//! link; partial generation updates only supplied exact IDs. Both refresh only
//! the nested sketch body's `code`. They preserve the combined
//! document's root, file allowlist, signatures, links, identifiers, and kinds.
//! The response returns the complete input catalog, including unchanged nested
//! YAML and non-YAML passthrough entries, and counts the linked sketches that
//! were refreshed. Returned catalog bytes are deterministic and are never
//! written to the operating system by this crate.
//!
//! A check can return YAML or JSON report bytes through [`ReportRequest`]. The
//! caller chooses the logical output path and persists those bytes if desired.
//! [`CheckResponse::report_view`] exposes the same report payload as a borrowed
//! serializable view that omits returned report files, allowing callers to
//! embed it without recreating sketch-domain fields.
//!
//! # Errors and diagnostics
//!
//! Malformed YAML, invalid contract fields, duplicate sketch identifiers, and
//! invalid generation seeds return [`SketchContractKitError`]. A valid sketch
//! that references a missing source file, whose snippet does not match, or
//! whose occurrence count violates its policy is represented by
//! [`SketchDiagnostic`] in a successful [`CheckResponse`]. Diagnostics retain
//! [`SketchLocation`] context and may include a nearest [`MatchCandidate`] or
//! bounded [`SourceLineSpan`] evidence. [`CheckMode`] determines whether those
//! diagnostics make the response fail.
//!
//! # Runtime and storage boundaries
//!
//! `conkit-sketch` owns sketch YAML parsing, normalization, matching, semantic
//! diffing, diagnostics, report bytes, and generation from caller-provided
//! resolved seeds. Filesystem walking, path normalization, terminal output, and
//! cross-domain orchestration belong to callers such as the `conkit` crate.
//!
//! Start with [`SketchContractKit::builder`] to configure the local kit and call
//! its operations directly with `.await`. The futures are executor-neutral: the
//! crate does not select the executor that polls them. To place an operation in
//! a spawned task, that task must own both its request and the kit, commonly by
//! moving an [`Arc`](std::sync::Arc) clone into an `async move` block. That
//! owning future and its output satisfy `Send + 'static`; a method future which
//! still borrows a local kit is not promised to be `'static`.
//!
//! CPU work runs on a reusable Rayon-backed pool. [`WorkerPool`] selects a
//! default, dedicated, or application-shared pool, while [`WorkOptions`]
//! independently bounds active and pending root operations. [`SketchLimits`]
//! bounds catalog, YAML, matching, diagnostic, and generated-output resources.
//! Catalogs, diagnostics, generated entries, and rendered output remain
//! deterministic regardless of worker scheduling.
//!
//! # Examples
//!
//! Refresh a linked sketch, then check the same source against it.
//!
//! ```
//! use conkit_sketch::{
//!     CatalogPath, CheckMode, CheckRequest, FileCatalog, GenerateMode,
//!     GenerateRequest, ReportRequest, SketchContractKit, SketchSeed,
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
//! contract_files.insert(contract_path.clone(), br#"contract_version: 2
//! root: ../src
//! files: [lib.rs]
//! extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: example, root: lib.rs, kind: library }] }
//! signatures:
//!   - answer_signature:
//!       file: lib.rs
//!       signature_type: function
//!       sketch: answer_body
//! sketches:
//!   - answer_body:
//!       file: lib.rs
//!       signature: answer_signature
//!       signature_type: function
//!       matching: { normalization: exact_lines_v1, occurrence: at_least_one }
//!       code: |-
//!         old code
//! "#.to_vec())?;
//!
//! let kit = SketchContractKit::builder().build()?;
//! let generated = futures_executor::block_on(kit.generate(GenerateRequest {
//!     contract_files,
//!     seeds: vec![SketchSeed {
//!         contract_file: contract_path.clone(),
//!         document_index: 0,
//!         sketch_id: "answer_body".to_owned(),
//!         signature_type: "function".to_owned(),
//!         file: source_path.clone(),
//!         code: "pub fn answer() -> u8 { 42 }".to_owned(),
//!     }],
//!     mode: GenerateMode::FullRefresh,
//! }))?;
//!
//! assert_eq!(generated.counts.refreshed_sketch_count, 1);
//! assert!(generated
//!     .contract_files
//!     .get(&contract_path)
//!     .is_some());
//!
//! let checked = futures_executor::block_on(kit.check(CheckRequest {
//!     source_files,
//!     contract_files: generated.contract_files,
//!     report: ReportRequest::None,
//!     mode: CheckMode::Enforce,
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
mod limits;
mod matcher;
mod normalize;
mod report;
mod work;

pub use crate::api::{
    CheckMode, CheckRequest, CheckResponse, DiffEntry, DiffRequest, DiffResponse, GenerateMode,
    GenerateRequest, GenerateResponse, SketchContractKit, SketchContractKitBuilder, SketchField,
    SketchGenerationCounts, SketchSeed, SketchSnapshot,
};
pub use crate::contract::{SketchMatchPolicy, SketchNormalization, SketchOccurrence};
pub use crate::error::SketchContractKitError;
pub use crate::files::{CatalogPath, FileCatalog, FileCatalogError};
pub use crate::inventory::{
    DiagnosticExcerpt, MatchCandidate, SketchCheckCounts, SketchDiagnostic, SketchLocation,
    SourceLineSpan,
};
pub use crate::limits::{
    CatalogLimits, DiagnosticLimits, LimitExceeded, LimitResource, MatchingLimits, OutputLimits,
    SketchLimits, YamlLimits,
};
pub use crate::report::{ReportFormat, ReportRequest};
pub use crate::work::{WorkOptions, WorkerPool};
