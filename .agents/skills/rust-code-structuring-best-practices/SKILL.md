---
name: rust-code-structuring-best-practices
description: "Apply this when structuring, refactoring, reviewing, or planning Rust code around domain structs, enums, impl blocks, receiver methods, associated constructors, builders, and strict standalone-function removal. Use when a task involves primitive-soup parameters, repeated argument groups, stringly typed or Option-heavy state, god structs, enum variants carrying data, Rust item type aliases, same-root lifecycle or state suffix families, moving behavior from free functions to methods, or enforcing that standalone functions are off-limits except main or explicit human-written spec carveouts."
---

# Rust Code Structuring Best Practices

## Purpose

Use Rust's type system to make domain structure explicit. Prefer meaningful
domain structs for grouped data, data-carrying enums for mutually exclusive
cases, and `impl` receiver methods for behavior that reads, mutates, consumes,
or constructs a domain type.

This skill is intentionally stricter than generic Rust style guidance for this
workspace. In `conkit` library code, behavior belongs on structs, enums, builder
types, or explicit trait contracts. Free-standing helper functions are not the
default design shape.

## Trigger Boundary

Use this skill when a Rust task involves:

- choosing between structs, enums, traits, methods, or builders
- replacing primitive-soup parameters or repeated argument groups
- removing stringly typed state, boolean mode flags, or Option-heavy state
- moving behavior from free functions into receiver methods
- reviewing whether standalone functions are allowed
- preventing same-root lifecycle/state suffix struct families
- preventing Rust item type aliases as design shortcuts

## Core Policy

Standalone functions are off-limits in this repository except for `main` in a
binary entrypoint or a specific human-written spec carveout that explicitly
requires a free function.

Apply this policy before writing or accepting Rust code:

1. If behavior reads, mutates, or consumes a domain value, put it in
   `impl Type`.
2. If behavior constructs a domain value, use `Type::new`, `Type::build`,
   `Type::from_*`, `TryFrom`, or a builder owned by the type.
3. If several arguments travel together, create a named struct and attach the
   behavior to that struct or the owning domain type.
4. If logic switches on strings, booleans, or optional fields to model states,
   create a data-carrying enum and attach behavior to the enum.
5. Do not create lifecycle or state suffix families for one logical object.
   When you start writing `FooStarted`, `FooCompleted`, `FooFailed`,
   `FooInputs`, `FooExecution`, `FooPlan`, `FooTask`, or a similar
   same-root-name group, stop and re-architect.
6. Do not use Rust item type aliases at any visibility. Replace them with the
   concrete payload type at the use site, a real struct/newtype when the
   concept needs a name, a data-carrying enum when states differ, or a trait
   associated type inside a trait contract.
7. If a helper appears source-neutral, keep looking for the owning struct,
   enum, builder, or private trait contract.

## Workflow

1. Inventory the domain values, state transitions, repeated argument groups,
   and existing impl blocks before editing.
2. Name the domain owner first, then move behavior to receiver methods on that
   owner or on a state enum.
3. Prefer one owner struct plus a data-carrying enum over suffix-family structs
   that copy identity fields.
4. Prefer associated constructors, `TryFrom`, and builders over constructor
   helper functions.
5. Prefer exhaustive enum matches and receiver methods over string labels,
   boolean switches, or generic `kind` helpers.
6. Before finishing, search the touched Rust files for `type ` aliases and
   standalone `fn` items. Permit only `main` or an explicit human-written spec
   carveout; methods and trait items are associated items, not standalone
   functions.

## Output Rules

- Keep behavior close to the data it operates on.
- Keep structs cohesive; split only when responsibilities, invariants, or
  ownership boundaries genuinely differ.
- Keep enums closed and data-carrying when modeling mutually exclusive states.
- Keep trait contracts explicit when polymorphism is required.
- Reject convenience aliases, suffix families, and loose helpers as migration
  shims.
- When a human-authored spec explicitly requires a standalone function, preserve
  it and state the carveout in the final answer.

## References

- Read `references/structs-enums-receiver-methods.md` when the task needs
  detailed examples, reviewer heuristics, or edge-case handling for structs,
  enums, receiver methods, and standalone-function exceptions.
