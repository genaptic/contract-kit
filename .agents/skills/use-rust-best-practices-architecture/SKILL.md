---
name: use-rust-best-practices-architecture
description: "Apply this when creating or restructuring Rust packages, workspaces, crates, modules, lib.rs/main.rs boundaries, target layout, or test placement. Use it when deciding whether to add a new crate or extend an existing one, when choosing between src/bin and a separate package, when replacing outdated module layouts, and when organizing code to minimize clone-heavy flows and borrow-checker friction."
---

# Rust Architecture Best Practices

## Purpose

Guide Rust package, crate, module, target, and test-tree ownership decisions.

## Trigger Boundary

Use this skill when the task changes project layout, crate boundaries, module
structure, binary/library boundaries, or test placement.

## Prerequisites

- Load `use-rust-best-practices-testing` when the task is mostly about test
  implementation details.
- Load `use-rust-best-practices-abstractions` when the task is mostly about
  trait, generic, error, or lifetime design.
- Read `../use-rust-best-practices-abstractions/references/enum-dispatch-trait-pattern.md`
  before moving enum-dispatch contract modules or implementation subtrees.

## Workflow

1. Keep binaries thin and move reusable behavior into libraries.
2. Organize by functional domain and behavior, not vague buckets.
3. Add a new crate only when it introduces a real ownership, dependency,
   deployment, reuse, or release boundary.
4. Put tests at the narrowest level that still exercises the behavior you need.
5. Prefer clone-minimizing ownership transitions at parsing, persistence,
   network, and task-spawn edges.
6. Use an enum plus private trait only for a closed family with multiple real
   concrete implementations. Keep its contract module implementation-agnostic,
   concrete impls in their owning subtrees, and dispatch explicit and
   exhaustive. Match arms call receiver methods rather than dispatch-contract
   UFCS. Exported dispatchers may use opaque public wrappers; crate-private
   dispatch enums may remain plain private enums. Do not turn routing enums
   into shared identity, provenance, capability, diagnostic, or error helpers.
7. Preserve Contract Kit's three production crates. Keep mixed-catalog archive
   transport, bounded filesystem reads, compiler-process extraction, process
   cancellation, and the application-owned shared Rayon pool in `conkit`.
   Keep signature and sketch semantics, nominal limits and work options,
   independent admission, and direct domain revalidation in their owning
   crates. Do not add `conkit-core` or a shared cross-domain error trait.
8. Treat `RustExtractionBackend` as the current closed syntax/compiler family.
   Keep `SketchContractKit` concrete and direct unless a second real
   implementation establishes a polymorphic boundary.

## Output Rules

- Prefer one coherent domain crate over many tiny pseudo-layer crates.
- Preserve the `conkit`, `conkit-signature`, and `conkit-sketch` ownership
  boundaries; do not introduce `conkit-core` or cross-domain error traits.
- Use `src/bin/` before introducing a separate package for small extra binaries
  that share the same domain and dependencies.
- Do not centralize backend-, provider-, model-, database-, runtime-, or
  source-specific enum-dispatch impls in shared contract modules.
- Do not write enum-dispatch arms as `Trait::method(receiver, args)`; fix the
  receiver/type structure so `receiver.method(args)` resolves to the private
  trait method.
- Keep `<Self as Trait>::method(self, ...)` limited to root public handles;
  do not use it inside payload-dispatch match arms.
- Do not add shared backend/provider/source kind or provenance utilities as
  shortcuts around module ownership; route behavior through the private trait
  and keep implementation facts in the owning subtree.
- Do not hide missing implementation-family variants behind wildcard or
  catch-all dispatcher arms.
- Do not add an enum-dispatch layer to a single concrete behavior owner such as
  `SketchContractKit`.

## References

- `references/architecture.md`
- `assets/templates/workspace-root/Cargo.toml`
- `assets/templates/thin-lib-bin/src/lib.rs`
- `assets/templates/thin-lib-bin/src/main.rs`
