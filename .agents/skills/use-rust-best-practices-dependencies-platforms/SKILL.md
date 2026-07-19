---
name: use-rust-best-practices-dependencies-platforms
description: "Apply this when choosing or updating Rust dependencies, setting Cargo versions/features/workspace inheritance, auditing supply-chain risk, deciding between common ecosystem crates, or making code portable across Windows, Linux, and macOS. Use it to enforce Cargo hygiene, explicit MSRV and dependency policy, security checks, target-specific dependency patterns, and portable path/process abstractions."
---

# Rust Dependencies And Platforms Best Practices

## Purpose

Guide dependency selection, Cargo hygiene, supply-chain review, MSRV policy,
and cross-platform behavior.

## Trigger Boundary

Use this skill when the task changes dependencies, Cargo features, workspace
inheritance, toolchain policy, security posture, or Windows/Linux/macOS
portability.

## Prerequisites

- Load `use-rust-best-practices-networking` when runtime client behavior
  dominates.
- Load `use-rust-best-practices-architecture` when crate or workspace structure
  dominates.

## Workflow

1. Prefer `std` and Cargo-native capabilities before adding dependencies.
2. Centralize shared versions and lints in the workspace when that improves
   consistency.
3. Avoid wildcard versions and accidental default features.
4. Evaluate crates for maintenance, docs, MSRV fit, feature surface,
   unsafe/FFI footprint, and platform support.
5. Isolate platform-specific behavior behind `cfg(...)` modules and portable
   path or process abstractions.
6. In Contract Kit, preserve the Rust 1.97 MSRV and pinned 1.97.0 toolchain,
   edition 2024, workspace-inherited package/dependency/lint policy, and the
   checked-in `deny.toml` hardening policy. Keep `fuzz` excluded from the
   three-member production workspace and independently locked.

## Output Rules

- Treat project-maintained docs and repo-local policy as authoritative.
- For `conkit` production code, do not add compiler-private crate dependencies
  whose names contain the `rust[c]` prefix.
- Keep `rustdoc-types` as an allowed compiler-output DTO dependency owned only
  by `conkit-signature`; do not treat it as permission for compiler-private
  dependencies or spread it to other production crates.
- Prefer `Path`/`PathBuf` and `std::process::Command` over stringly-typed OS
  assumptions.

## References

- `references/dependencies-and-platforms.md`
- `assets/templates/project_dirs.rs`
- `assets/templates/target_specific_deps.toml`
- `assets/templates/member_with_workspace_lints.toml`
