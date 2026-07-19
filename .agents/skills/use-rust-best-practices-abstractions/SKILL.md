---
name: use-rust-best-practices-abstractions
description: "Apply this when refactoring Rust code to remove duplication, designing or extending structs and methods, choosing between concrete code, generics, traits, trait objects, or enums, handling errors and panics, introducing or simplifying lifetimes, or implementing enum-dispatcher style closed implementation families. Use it when Codex must decide whether to keep code concrete, extract a reusable abstraction, or redesign an API to be more idiomatic and maintainable."
---

# Rust Abstractions Best Practices

## Purpose

Guide decisions about duplication removal, API shape, polymorphism, lifetimes,
errors, and safe abstraction boundaries.

## Trigger Boundary

Use this skill when a Rust task changes APIs, data structures, traits,
generics, lifetimes, error types, panic policy, or duplicated domain logic.

## Prerequisites

- Load `rust-code-structuring-best-practices` first when the task is primarily
  about struct and enum organization, receiver methods, associated
  constructors, primitive soup, repeated parameter groups, stringly typed
  state, or eliminating standalone functions and loose helpers.
- Load `use-rust-best-practices-architecture` when the task is mostly about
  package or module structure.
- Load `use-rust-best-practices-async` or
  `use-rust-best-practices-networking` when runtime behavior dominates.
- Read `references/enum-dispatch-trait-pattern.md` before adding, removing,
  or refactoring enum-dispatch behavior.

## Workflow

1. Keep code concrete until duplication or extension pressure is real.
2. Refactor repeated data plus behavior into structs with methods.
3. Do not replace duplication with Rust item type aliases or same-root
   stateful suffix carrier structs. Collapse repeated lifecycle/state carriers
   into one owner struct plus a data-carrying state enum, introduce a real
   newtype/struct only when it owns invariants, or use trait associated types
   only inside trait/impl contracts.
4. For `conkit` reductive refactors, do not replace duplication with
   `macro_rules!`, proc-macro indirection, generated dispatch, generated
   tables, or codegen-style hidden abstraction.
5. Require enum dispatch plus a private trait only when a closed behavior
   family has multiple real concrete implementations. Keep a unique,
   implementation-agnostic trait contract and hand-written exhaustive dispatch
   for that family; do not introduce the pattern for a single concrete owner.
   In Contract Kit, `RustExtractionBackend` is the current syntax/compiler
   example. `SketchContractKit` remains a direct concrete behavior owner and
   must not gain a backend trait or inner dispatch enum without a second real
   implementation. Shared contract modules own only implementation-agnostic
   trait definitions, shared handles, and default helpers; concrete impls live
   in the owning implementation subtree. Exported dispatchers may use opaque
   public structs over private inner enums, while crate-private internal
   dispatch enums can remain plain private enums. Private dispatch and config
   selection enums are routing mechanisms only; do not reuse them as family
   identity, provenance, capability, diagnostic-label, or error-label helpers.
   Dispatcher methods use explicit exhaustive `match` arms and receiver-method
   syntax. Prefer `backend.method(args).await`; payload dispatch through
   `Trait::method(backend, args).await` is invalid in this workspace. A public
   root handle may use `<Self as Trait>::method(self, args).await` only to enter
   its own trait impl and avoid same-name inherent-method recursion.
6. Introduce traits at the usage boundary when multiple implementations or test
   doubles are genuinely useful.
7. Choose enums for closed sets, generics for static polymorphism, and trait
   objects for open runtime-extensible sets.
8. Use explicit lifetimes only when the borrow relationship cannot be elided.
9. Return `Result` for recoverable failures and reserve panic for bugs or truly
   impossible invariants.

## Output Rules

- Avoid speculative traits or generics.
- Prefer concrete structs, closed enums, typed data, helper methods, and
  inherent methods over macro-based deduplication in this workspace.
- Reject item type aliases and lifecycle/state suffix carrier families as
  abstraction mechanisms. They are guardrail bypasses, not simplifications.
- Preserve private enum-dispatch trait contracts where a real closed
  multi-implementation family exists; do not delete, bypass, publicize, or
  macro-generate them.
- Do not manufacture a dispatcher for a single concrete owner. In particular,
  retain direct receiver methods on `SketchContractKit`.
- For an exported closed-family dispatcher, require the public root handle and
  every concrete variant payload receiver to implement the same private trait.
  The handle's trait impl should match over its private inner enum and call
  operations with receiver-method syntax.
- Reject reusable implementation-family kind/provenance helpers such as
  implementation-family display-name utilities, capability-label routers, or
  diagnostic/error labels. Family-specific facts belong in the owning
  concrete payload or documented data/config type, not in a shared shortcut.
- Permit `<Self as Trait>::method(self, ...)` only in public root handle
  methods that bridge into the handle's own private trait impl; never use it
  for payload dispatch arms.
- Keep concrete enum-dispatch impls in the owning implementation subtree
  instead of the shared contract module.
- Keep `unsafe` small and wrap it behind safe APIs when it is truly necessary.

## References

- `references/abstractions.md`
- `references/enum-dispatch-trait-pattern.md`
- `assets/templates/enum_dispatch.rs`
- `assets/templates/typed_error.rs`
