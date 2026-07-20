# Contributing to Contract Kit

Thank you for helping improve Contract Kit. Contributions to code,
documentation, tests, and issue reports are welcome.

Participation in this project is governed by the
[Code of Conduct](CODE_OF_CONDUCT.md). Except where a file states otherwise,
contributions are made under the project's [Apache License 2.0](LICENSE).
Changes to `CODE_OF_CONDUCT.md` remain under its stated
[CC BY-SA 4.0 terms](CODE_OF_CONDUCT.md#attribution).

## Before you start

- Search [existing issues](https://github.com/genaptic/contract-kit/issues)
  before opening a new one.
- Use the repository's
  [bug and feature forms](https://github.com/genaptic/contract-kit/issues/new/choose)
  when reporting a problem or proposing a focused enhancement.
- Seek maintainer alignment before broad changes to architecture, public Rust
  APIs, CLI grammar or output, contract formats, persistence behavior, or
  platform support.
- Keep each contribution focused on one coherent problem.

## Development prerequisites

Install:

- Git;
- [rustup](https://rustup.rs/);
- the native linker and build tools normally required by Rust on your host;
  and
- network access for the initial dependency download.

The repository's `rust-toolchain.toml` pins Rust 1.97.0 with `rustfmt` and
Clippy for development. Rustup selects that toolchain automatically when you
work in the checkout. Workspace manifests declare Rust 1.97 as the minimum
supported Rust version (MSRV), and release preflight verifies that all three
packages retain that declaration.

Clone and build the workspace:

```shell
git clone https://github.com/genaptic/contract-kit.git
cd contract-kit
rustup show active-toolchain
cargo build --locked --workspace
cargo run --locked -p conkit -- --version
```

## Repository orientation

Contract Kit is a three-member Rust workspace:

- `conkit` owns the CLI, filesystem and process boundaries, persistence,
  mixed-catalog archive codec, and cross-domain orchestration.
- `conkit-signature` owns Rust signature contract semantics, including
  generation, checking, and semantic diffing.
- `conkit-sketch` owns sketch contract semantics, including generation,
  checking, normalization, and semantic diffing.
- `test/scenarios` contains checked-in end-to-end CLI evidence.

Use the following documents according to their audience:

- [README](README.md): product installation, usage, CLI behavior, and contract
  format.
- [Workspace architecture](ARCHITECTURE.md): workspace boundaries and
  structure; each crate has its own linked architecture guide.
- [Scenario authoring guide](test/scenarios/README.md): canonical scenario
  schema, fixture rules, evidence requirements, and focused validation.
- [`conkit-signature` rustdoc](https://docs.rs/conkit-signature) and
  [`conkit-sketch` rustdoc](https://docs.rs/conkit-sketch): public library APIs.
- [Release guide](RELEASING.md): maintainer-only release procedures.

`AGENTS.md`, nested agent guides, `SKILLS.md`, and `.agents/skills` are
agent-facing operational material. Human contributors should not need them for
ordinary onboarding.

## Making a change

- Preserve the existing crate boundaries unless an agreed architectural change
  explicitly requires otherwise.
- Add or update the narrowest useful tests for changed behavior.
- Use Cargo-level checks, doctests, package tests, and scenario runners. Do not
  invoke the `rustc` executable directly from tests.
- Keep production modules the same shape in test and non-test builds. Do not
  add production-scope `#[cfg(test)]` imports, fields, methods, constructors, or
  trait implementations as test shims.
- Consider Linux, Windows, and macOS behavior for paths, processes, output, and
  filesystem changes.
- Update the document that owns the affected information instead of copying it
  into several guides.
- For scenario or harness changes, follow the
  [scenario authoring guide](test/scenarios/README.md) and run its focused
  validation before the workspace gates.

During development, use the smallest package-scoped check or test that gives
useful feedback. Package-level examples include:

```shell
cargo test --locked -p conkit-signature
cargo test --locked -p conkit-sketch
cargo test --locked -p conkit
```

These focused commands do not replace the final workspace validation.

## Validation

Before requesting review, run the canonical checked, locked workspace gates:

```shell
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --doc
cargo test --locked --workspace --all-targets
```

For public API or rustdoc changes, also build documentation with warnings
denied. On POSIX shells:

```shell
RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --all-features --no-deps
```

On PowerShell:

```powershell
$env:RUSTDOCFLAGS = "-D warnings"
cargo doc --locked --workspace --all-features --no-deps
```

For Markdown-only changes:

1. Ensure every intended new file is staged or marked intent-to-add, for
   example with `git add --intent-to-add -- path/to/new-file.md`.
2. Run both whitespace checks:

   ```shell
   git diff --check
   git diff --cached --check
   ```

   The first checks working-tree changes and intent-to-add files. The second
   checks staged changes and fully staged new files.

Then:

- review the rendered GitHub-flavored Markdown;
- verify local links and heading anchors;
- verify external links; and
- validate every documented command against current CLI behavior.

The repository does not currently provide an automated Markdown linter or link
checker.

## Opening a pull request

- Link the relevant issue, using `Closes #123` when merging the pull request
  should close it.
- Explain what changed, why the selected boundary is appropriate, and any
  alternatives that materially affected the implementation.
- Check only validation commands and reviews that you actually completed.
- Describe Linux, Windows, and macOS impact where relevant.
- Call out public API, CLI, contract-format, persistence, compatibility, and
  release implications explicitly; write `None` when there are none.
- Include the narrowest useful tests and update the owning documentation.
- Wait for the repository's CI and rustdoc checks before merge.

Releases are performed separately by maintainers. Contributors should not
create release tags or publish packages as part of an ordinary pull request;
see [RELEASING.md](RELEASING.md) for the maintained process.
