---
name: use-rust-best-practices-testing
description: "Apply this when adding, moving, fixing, or reviewing Rust tests. Use it to choose between unit, integration, doctest, binary-smoke, and end-to-end tests; to place tests correctly in source files, crate-level tests/, or dedicated workspace e2e crates; and to build test flows that use fakes for unit tests but live dependencies for integration and e2e tests via testcontainers, Docker Compose, dev containers, or test credentials."
---

# Rust Testing Best Practices

## Purpose

Guide test placement, scope, and live-dependency strategy for Rust code.

## Trigger Boundary

Use this skill when the task adds, moves, fixes, or reviews Rust tests or test
strategy.

## Prerequisites

- Load `use-rust-best-practices-architecture` when the question is mostly about
  where tests should live.
- Load `use-rust-best-practices-async` when async timing or shutdown dominates.
- Load `use-rust-best-practices-abstractions` when tests guard
  enum-dispatch trait contracts or implementation-family parity.

## Workflow

1. Prefer the narrowest test level that proves the behavior.
2. Keep unit tests in source files and use fakes or mocks there.
3. Keep integration tests under the crate being tested and use the public API
   with real dependencies.
4. Add doctests for public APIs that benefit from executable examples.
5. Use dedicated workspace e2e crates or app boundaries for full-system tests.
6. In Contract Kit, keep structural checks focused on the real dispatch
   contracts: native `AppCommand` dispatch in `conkit` and the private
   `RustExtractionBackend` syntax/compiler family in `conkit-signature`.
7. Centralize workspace source and dependency policy in
   `conkit/tests/dependency_policy.rs`; do not restore parallel source scanners
   in the domain crates.
8. Cover nominal resource limits at input, parsing, extraction, and output
   boundaries. Cover active and pending admission independently, immediate
   saturation, queued-drop release, cooperative running cancellation, and
   permit release before completion becomes observable.

## Output Rules

- Unit tests should not require real infrastructure.
- Test-only helpers must live inside `#[cfg(test)] mod tests` or crate-level
  `tests/`. Do not add `#[cfg(test)]` impls, methods, imports, fields, trait
  impls, or constructors to production modules as test shims.
- Integration tests should not mock dependencies you do not own.
- E2E tests should validate the external contract, not internal details.
- For `conkit`, do not invoke the `rust[c]` executable directly from tests; use
  Cargo-level validation, doctests, package tests, and scenario runners.
- Structural tests should enforce the actual `AppCommand` and
  `RustExtractionBackend` topology, including receiver-style dispatch and
  exhaustive arms, without imposing enum dispatch on direct owners such as
  `SketchContractKit`.
- Keep workspace dependency/source policy in
  `conkit/tests/dependency_policy.rs`; do not duplicate it in domain source
  scanners.

## References

- `references/testing.md`
- `assets/templates/unit_test_with_fake.rs`
- `assets/templates/bin_smoke_test.rs`
- `assets/templates/doctest_examples.rs`
