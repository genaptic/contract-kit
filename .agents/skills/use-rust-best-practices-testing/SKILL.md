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
6. For enum-dispatch families, add focused structural tests that fail if
   private traits or explicit enum impls disappear, owning-module concrete
   impls move, or restoration TODO markers return.

## Output Rules

- Unit tests should not require real infrastructure.
- Test-only helpers must live inside `#[cfg(test)] mod tests` or crate-level
  `tests/`. Do not add `#[cfg(test)]` impls, methods, imports, fields, trait
  impls, or constructors to production modules as test shims.
- Integration tests should not mock dependencies you do not own.
- E2E tests should validate the external contract, not internal details.
- For `conkit`, do not invoke the `rust[c]` executable directly from tests; use
  Cargo-level validation, doctests, package tests, and scenario runners.
- Structural tests should enforce enum-dispatch trait placement and reject
  macro-generated replacements, dispatch-contract UFCS calls, catch-all dispatcher arms,
  and stale restoration TODO markers.

## References

- `references/testing.md`
- `assets/templates/unit_test_with_fake.rs`
- `assets/templates/bin_smoke_test.rs`
- `assets/templates/doctest_examples.rs`
