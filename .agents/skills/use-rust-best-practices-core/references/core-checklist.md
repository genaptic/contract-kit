# Rust core task checklist

Use this reference whenever a Rust task needs a baseline implementation and review pass before deeper specialization.

## Contents

- [Inspect repository constraints before changing code](#1-inspect-repository-constraints-before-changing-code)
- [Use baseline manifest patterns](#2-baseline-manifest-patterns)
- [Run checked, locked validation](#3-run-checked-locked-validation)
- [Review core concerns](#4-core-review-checklist)
- [Escalate to companion skills](#5-escalate-to-companion-skills-when-you-see-these-signals)
- [Keep warning policy intentional](#6-keep-warning-policy-intentional)
- [Further reading](#further-reading)

## 1. Inspect repository constraints before changing code

Check these in order:

1. `rust-toolchain.toml` / `rust-toolchain`
2. workspace and package `Cargo.toml` files
3. CI workflows and build matrices
4. target triples and platform support statements
5. existing module/layout/testing patterns already used successfully in the repo

### Interpret them this way

- `edition` = language edition used by the package
- `rust-version` = minimum Rust version the package intentionally supports
- workspace lint tables are **not** inherited unless each member opts in with `[lints] workspace = true`
- if the repository already pins an older stable version or a lower MSRV, stay compatible unless the task explicitly includes an upgrade

## 2. Baseline manifest patterns

### Single package baseline

```toml
[package]
name = "my-app"
version = "0.1.0"
edition = "2024"
# Add rust-version only if the repo intentionally supports a specific MSRV.
# rust-version = "1.85"

[lib]
path = "src/lib.rs"

[[bin]]
name = "my-app"
path = "src/main.rs"

[lints.rust]
unsafe_op_in_unsafe_fn = "deny"

[lints.clippy]
dbg_macro = "deny"
todo = "deny"
unwrap_used = "deny"

[dependencies]
anyhow = "1"
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
thiserror = "2"
tracing = "0.1"
```

### Workspace root baseline

```toml
[workspace]
members = ["crates/*", "tests/e2e"]
default-members = ["crates/api", "crates/domain", "crates/worker"]
resolver = "3"

[workspace.package]
edition = "2024"
license = "MIT OR Apache-2.0"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
thiserror = "2"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal", "time"] }
tracing = "0.1"

[workspace.lints.rust]
unsafe_op_in_unsafe_fn = "deny"

[workspace.lints.clippy]
dbg_macro = "deny"
todo = "deny"
unwrap_used = "deny"
```

### Workspace member baseline

```toml
[package]
name = "api"
version = "0.1.0"
edition.workspace = true

[lints]
workspace = true

[dependencies]
serde.workspace = true
thiserror.workspace = true
tokio.workspace = true
tracing.workspace = true
```

### Why this baseline is better than a hard-coded patch pin

- it respects the repo’s own support policy
- it avoids drifting patch-version text in the skill bundle
- it lets new projects stay modern without forcing existing repos to upgrade

## 3. Run checked, locked validation

Use the five checked, locked workspace gates as the default completion set.

### Required workspace gates

```bash
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --doc
cargo test --locked --workspace --all-targets
```

### Optional preliminary package checks

Use package-scoped commands only for fast, non-mutating preliminary feedback.
They do not replace any required workspace gate.

```bash
cargo fmt -p my-crate -- --check
cargo check --locked -p my-crate --all-targets --all-features
cargo clippy --locked -p my-crate --all-targets --all-features -- -D warnings
cargo test --locked -p my-crate --doc
cargo test --locked -p my-crate --all-targets
```

### Additional public API and example checks

```bash
cargo test --locked --workspace --examples
```

## 4. Core review checklist

### Architecture

- Is `main.rs` thin?
- Does reusable logic live in `lib.rs`-backed modules or a library crate?
- Did the change extend a coherent existing module instead of creating a vague `utils` bucket?
- Would `src/bin/` be enough instead of a new package?

### Ownership and data flow

- Are read-only inputs borrowed (`&str`, `&[T]`, `&T`)?
- Are owned conversions delayed until a true ownership boundary?
- Were unnecessary `clone`, `to_string`, or `Arc<Mutex<_>>` additions avoided?

### Errors and panics

- Do library APIs return typed errors?
- Are application boundaries adding context instead of erasing structure too early?
- Are `unwrap` / `expect` limited to tests, examples, or impossible invariants with a clear message?

### Tests and docs

- Is the narrowest useful test level used?
- Were public APIs given doctests when helpful?
- Should a binary smoke test be added for CLI changes?
- Should a workspace e2e test be used instead of an oversized integration test?

### Async and networking

- Are long-lived clients, channels, and pools reused?
- Is blocking work kept out of async contexts?
- Are retries limited to idempotent operations?
- Is concurrency bounded where fan-out could explode?

### Dependencies and platforms

- Is the dependency surface still justified?
- Were default features disabled when they are not needed?
- Is the code portable across the repo’s supported operating systems?
- Are paths, environment handling, and process execution using portable abstractions?

### Unsafe

- Is there any new `unsafe`?
- If yes, is the unsafe block small, documented, and wrapped in a safe abstraction?
- Was blanket `unsafe_code = "forbid"` avoided unless it truly matches the crate’s role?

## 5. Escalate to companion skills when you see these signals

Load the specialized skill when these patterns appear.

### Architecture

- new crate or workspace questions
- `lib.rs` / `main.rs` boundaries
- `src/bin/`, `examples/`, `tests/`, `benches/`
- module layout or `mod.rs` cleanup

### Testing

- failing tests
- new unit/integration/e2e coverage
- doctests
- binary smoke tests
- Docker / testcontainers / dev container test environments

### Async

- `async fn`
- Tokio
- channels, mutexes, spawned tasks
- `select!`, `join!`, graceful shutdown
- fan-out / concurrency limits

### Networking

- `reqwest`
- `tonic`
- `sqlx`
- pools
- retries/timeouts/backpressure
- transport/domain DTO mapping

### Abstractions

- traits vs generics vs enums
- enum-dispatcher behavior families and private trait contracts
- refactoring duplicated logic
- lifetimes
- error types
- panic policy
- unsafe wrappers

### Dependencies / platforms

- dependency additions or updates
- target-specific dependencies
- MSRV policy
- Windows/Linux/macOS support
- path, filesystem, or process execution code

## 6. Keep warning policy intentional

Avoid `#![deny(warnings)]` as a blanket default.

Bad:

```rust
#![deny(warnings)]
```

This often turns unrelated compiler or lint changes into semver-hostile churn for libraries and makes local upgrades noisy.

Better:

```rust
#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_debug_implementations)]
```

And enforce warning-free builds in CI with commands such as:

```bash
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
```

Use crate-level lint settings for intentional policy, and use CI to make the review gate strict.

## Further reading

- Official Codex skills docs: <https://developers.openai.com/codex/skills>
- Latest Rust release index: <https://blog.rust-lang.org/releases/latest/>
- Cargo manifest reference: <https://doc.rust-lang.org/cargo/reference/manifest.html>
- Cargo workspaces: <https://doc.rust-lang.org/cargo/reference/workspaces.html>
- Cargo specifying dependencies: <https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html>
- Cargo rust-version: <https://doc.rust-lang.org/cargo/reference/rust-version.html>
- Cargo lints: <https://doc.rust-lang.org/cargo/reference/lints.html>
- Cargo test: <https://doc.rust-lang.org/cargo/commands/cargo-test.html>
- Rust edition guide: <https://doc.rust-lang.org/edition-guide/>
- Rust book on packages, crates, and modules: <https://doc.rust-lang.org/book/ch07-00-managing-growing-projects-with-packages-crates-and-modules.html>
- Rust API Guidelines checklist: <https://rust-lang.github.io/api-guidelines/checklist.html>
- Rust design patterns: <https://rust-unofficial.github.io/patterns/>
