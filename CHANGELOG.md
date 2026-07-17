# Changelog

All notable changes to Contract Kit are documented in this file. The project
uses Semantic Versioning and tags coordinated workspace releases as
`vX.Y.Z`.

## [Unreleased]

### Changed

- Prepare the coordinated 0.2.0 API by changing sketch diff field collections
  from `BTreeSet<SketchField>` to canonical `Vec<SketchField>` values and
  removing the unobservable `SketchField::Normalization` variant. All other
  CLI, report, digest, ordering, and filesystem behavior remains unchanged.
- Reduce duplicated extraction, YAML, matching, limit, report, and test
  infrastructure, then divide the surviving compiler, Rust signature, and
  sketch implementation into focused private modules. This internal cleanup
  keeps the existing CLI grammar, wire bytes, digest values, diagnostics,
  resource ceilings, cancellation, and filesystem effects unchanged.

### Fixed

- Rate-limit live Cargo target-tree inspection, tolerate only transient
  descendant disappearance, and enforce one exact scan after process exit.
- Treat the pre-rename checkpoint as the final cancellation boundary for
  individually atomic generated-file publication.
- Reject cyclic or multiply owned rustdoc module containment through an
  iterative traversal; canonicalize implicit Rust representation, validate the
  complete `cfg` predicate grammar, omit valid empty `cfg_attr`, and merge split
  compiler impl partitions deterministically through the shared parsed-entry
  model.
- Emit exact-source public module declarations during compiler extraction,
  fail closed when rustdoc cannot provide their source shape, and canonicalize
  alignment-one `repr(packed)` and `repr(packed(1))` identically.
- Independently bound live generated/editing scratch in both domain crates and
  stream signature edits per document and sketch edits per target without
  retaining full-document replacements or copied preservation evidence.
- Preserve each edited sketch scalar/document's LF, CRLF, or CR presentation
  and allocate mismatch excerpts only after exact diagnostic-budget preflight.
- Charge verified compressed archive inputs and decoded archived entries to the
  same command-wide catalog ledger, and preserve each edited signature
  document's LF, CRLF, or CR presentation without rewriting retained bytes.

## [0.1.0] - 2026-07-14

This is a coordinated breaking release. Recreate v0.0.1 contract catalogs;
there is no compatibility parser or automatic migration for the former
signature, sketch-normalization, or digest semantics.

### Added

- Require explicit contract format v2 extraction and sketch-matching policy.
- Model Rust constants, reexports, foreign declarations, associated items,
  semantic visibility, typed attributes, explicit crate roots, and logical
  modules; unsupported reachable syntax now fails closed.
- Add optional Cargo/rustdoc-backed extraction with pinned, locked, bounded
  toolchain execution and versioned artifacts. Syntax extraction remains the
  portable default and reports its compiler-context limitations.
- Add exact-line sketch occurrence policies, bounded mismatch evidence,
  contextual semantic diffs, partial library refresh, and precise counts.
- Add domain-specific resource limits, independently bounded active and
  pending operations, cooperative cancellation, and caller-owned shared Rayon
  pools.
- Connect process Ctrl-C/termination handling to the root executor, domain
  cancellation probes, and bounded compiler process-group/job cleanup without
  reporting cancellation after a completed persistence commit.
- Bound CLI contract-header YAML cumulatively across physical files and diff
  inputs, including alias-replayed nodes and scalar bytes, with cooperative
  physical-stream cancellation checkpoints.
- Add property tests, an isolated fuzz workspace, dependency policy, scheduled
  hardening, and benchmark baselines without speculative matcher strategies.

### Changed

- Preserve meaningful sketch whitespace and arbitrary non-line-ending bytes;
  only CRLF/LF spelling and one final terminator are normalized.
- Treat every function named `main` as an ordinary function and exclude
  parameter binding patterns from API-compatibility digests.
- Apply exact source allowlists before decoding, parsing, owner resolution,
  document-local projection, and label allocation.
- Preserve unchanged YAML bytes and use lazy lossless edits plus semantic
  reparse verification for actual signature or sketch changes.
- Replace the archived `serde_yaml` dependency in all three packages with the
  maintained semantic parser and domain-owned lossless editor.
- Declare Rust 1.97 as the coordinated minimum supported Rust version for all
  three workspace packages and enforce it during release preflight.
- Separate source/API-shape checking from complete contract-context diffing,
  strengthen digest/report invariants, and remove ignored or unreachable API
  states.

### Removed

- Remove versionless/v1 contract parsing, `main_method`, filename-derived
  module identity, global bare-owner fallback, whitespace-collapse matching,
  duplicate sketch check modes, silent sketch-ID trimming, and whole-document
  YAML regeneration.
- Keep exactly the existing three production packages; no shared core crate or
  compatibility object graph was introduced.

## [0.0.1] - 2026-07-13

- Establish the initial `conkit` command-line interface.
- Provide runtime-neutral `conkit-signature` and `conkit-sketch` libraries.
- Add native Linux, Windows, Intel macOS, and Apple Silicon macOS releases.

[Unreleased]: https://github.com/genaptic/contract-kit/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/genaptic/contract-kit/compare/v0.0.1...v0.1.0
[0.0.1]: https://github.com/genaptic/contract-kit/releases/tag/v0.0.1
