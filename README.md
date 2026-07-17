<a id="conkit"></a>

# Contract Kit (conkit)

[![CI status](https://github.com/genaptic/contract-kit/actions/workflows/ci.yml/badge.svg?branch=main&event=push)](https://github.com/genaptic/contract-kit/actions/workflows/ci.yml) [![Rustdocs status](https://github.com/genaptic/contract-kit/actions/workflows/docs.yml/badge.svg?branch=main&event=push)](https://github.com/genaptic/contract-kit/actions/workflows/docs.yml) [![Latest GitHub release](https://img.shields.io/github/v/release/genaptic/contract-kit.svg?sort=semver)](https://github.com/genaptic/contract-kit/releases/latest) [![Software license: Apache-2.0](https://img.shields.io/github/license/genaptic/contract-kit.svg)](https://github.com/genaptic/contract-kit/blob/main/LICENSE)

[![conkit-signature on crates.io](https://img.shields.io/crates/v/conkit-signature.svg?label=conkit-signature)](https://crates.io/crates/conkit-signature) [![conkit-signature documentation](https://img.shields.io/docsrs/conkit-signature.svg?label=conkit-signature%20docs)](https://docs.rs/conkit-signature) [![conkit-sketch on crates.io](https://img.shields.io/crates/v/conkit-sketch.svg?label=conkit-sketch)](https://crates.io/crates/conkit-sketch) [![conkit-sketch documentation](https://img.shields.io/docsrs/conkit-sketch.svg?label=conkit-sketch%20docs)](https://docs.rs/conkit-sketch)

Contract Kit (`conkit`) provides hard, reviewable guardrails for spec-driven
software development with AI agents. It captures source signatures and opt-in
code sketches as version-controlled, machine-checkable contracts, helping
agents and reviewers keep implementation changes aligned with agreed intent.

The CLI generates contracts from a source tree, checks source against an
existing contract catalog, archives catalogs, and compares the current catalog
with an archive. Initial signature support targets Rust. Sketches are
language-neutral snippets checked by normalized line matching, and both
contract families live in the same combined contract documents.

<a id="releases-and-api-documentation"></a>

## Installation

### Prebuilt release

Download the archive for your platform and `SHA256SUMS` from the
[latest GitHub Release](https://github.com/genaptic/contract-kit/releases/latest).
Here, `{version}` is the release tag without its leading `v`.

| Platform | Release asset |
| --- | --- |
| Linux x86-64, GNU | `conkit-v{version}-x86_64-unknown-linux-gnu.tar.gz` |
| Windows x86-64, MSVC | `conkit-v{version}-x86_64-pc-windows-msvc.zip` |
| Intel macOS | `conkit-v{version}-x86_64-apple-darwin.tar.gz` |
| Apple Silicon macOS | `conkit-v{version}-aarch64-apple-darwin.tar.gz` |

Verify the downloaded archive against its entry in `SHA256SUMS`. On Linux or
macOS, set `ARCHIVE` to the downloaded file name and check only that entry:

```shell
ARCHIVE="<downloaded archive file name>"

# Linux
awk -v archive="$ARCHIVE" '$2 == archive' SHA256SUMS | sha256sum --check -

# macOS
awk -v archive="$ARCHIVE" '$2 == archive' SHA256SUMS | shasum -a 256 --check -
```

On Windows, use PowerShell to print the archive's SHA-256 digest, then compare
it with the matching `SHA256SUMS` entry:

```powershell
$Archive = "<downloaded archive file name>"
Get-FileHash ".\$Archive" -Algorithm SHA256
```

The release workflow also creates GitHub artifact attestations. If the
[GitHub CLI](https://cli.github.com/) is installed, provenance verification is
available as an additional check:

```shell
gh attestation verify "$ARCHIVE" --repo genaptic/contract-kit
```

In PowerShell:

```powershell
gh attestation verify $Archive --repo genaptic/contract-kit
```

Extract a `.tar.gz` archive on Linux or macOS:

```shell
tar -xzf "$ARCHIVE"
```

In PowerShell, extract the Windows archive:

```powershell
Expand-Archive -Path ".\$Archive" -DestinationPath .
```

Each archive contains a versioned directory with the executable, README, and
license. Move `conkit` (or `conkit.exe` on Windows) into a directory on `PATH`,
then verify the installation:

```shell
conkit --version
```

### Install from source

To install the current source checkout instead of a versioned release, first
install Rust through [rustup](https://rustup.rs/), then run:

```shell
git clone https://github.com/genaptic/contract-kit.git
cd contract-kit
cargo install --locked --path conkit
conkit --version
```

The CLI package is not published to crates.io, so `cargo install conkit` is not
a supported installation path.

## Quick start

From the root of your own Rust project, with Rust sources under `src`, run:

```shell
conkit generate all --source src --contracts contracts \
  --crate-root app=library:lib.rs
conkit check all --source src --contracts contracts --output conkit-report.yml --strict
```

The first command creates an initial combined contract catalog under
`contracts`, including signature contracts and managed-output metadata. A fresh
`generate all` creates zero sketches because sketches are opt-in. Review the
generated outputs before relying on them as project guardrails.

The second command checks the source against that catalog in strict mode and
writes `conkit-report.yml`. It succeeds while the source and contracts agree.

Commit the reviewed contract catalog alongside the source and use it as a
machine-checkable boundary when assigning implementation work to an AI agent.
Update contracts intentionally when the specification changes; otherwise, run
the strict check to expose implementation drift before review or merge.

## Signatures and sketches

- **Signatures** describe Rust declarations and their structure, including
  canonical crate/module identity, semantic visibility, typed attributes,
  parameters and return types, associated items, foreign items, reexports,
  and implementation ownership.
- **Sketches** attach an opt-in code snippet to a named signature. Matching
  normalizes line endings only, preserving indentation, tabs, blank lines,
  horizontal whitespace, comments, and all other non-line-ending bytes.

Use `all` to operate on both families, or select `signatures` or `sketches`
when only one family should participate.

## Command-line interface

### Check contracts

Choose exactly one required target: `all`, `signatures`, or `sketches`.
`signature` and `sketch` are singular aliases.

```shell
conkit check <all|signatures|sketches> \
  --source <DIR> \
  --contracts <DIR> \
  --output <FILE> \
  [--default|--strict|--warning]
```

The mode flags are optional and mutually exclusive. Omitting a mode is
equivalent to passing `--default`. Default mode permits syntax-extraction
capability warnings but fails comparison and extraction errors. Strict mode
requires a diagnostic-free result. Warning mode retains every diagnostic while
allowing a completed check to pass.

`all` and `signatures` additionally accept the signature-extraction options
below. `sketches` intentionally rejects them.

The output file extension selects the report format. `.yml` and `.yaml` produce
YAML; `.json` produces JSON. Extension matching is case-insensitive.

### Generate contracts

Generation also requires one target and accepts the same singular aliases.

```shell
conkit generate <all|signatures> \
  --source <DIR> \
  --contracts <DIR> \
  [--crate-root <CRATE_ID>=<KIND>:<RELATIVE_RS_PATH>]... \
  [--signature-extractor syntax|compiler] \
  [--adopt-existing]

conkit generate sketches \
  --source <DIR> \
  --contracts <DIR> \
  [--adopt-existing]
```

Fresh `all` or `signatures` generation may infer one root only when the selected
source layout contains exactly one conventional root-level `lib.rs` (library)
or `main.rs` (binary). Zero, multiple, nonconventional, or disconnected roots
require one repeated
`--crate-root <CRATE_ID>=<KIND>:<RELATIVE_RS_PATH>` value for every crate.
`KIND` is exactly `library` or `binary`; the path is relative to `--source` and
may use any allowlisted Rust filename. Existing documents already own their
extraction metadata and reject command-line root overrides. Sketch-only
generation does not accept crate-root options.

### Signature extraction

Syntax extraction is the portable default and never invokes Cargo. Compiler
extraction is explicit on `check`/`generate all` and `signatures`:

```shell
conkit check signatures \
  --source src --contracts contracts --output conkit-report.yml \
  --signature-extractor compiler --manifest-path Cargo.toml \
  [--package SPEC] [--lib|--bin NAME] \
  [--features FEATURES|--all-features] [--no-default-features] \
  [--target TRIPLE]
```

Compiler extraction requires the installed `nightly-2026-07-01` toolchain;
toolchain auto-installation is disabled. Every Cargo operation uses locked
dependency resolution. Cargo runs selected build scripts and procedural macros
unsandboxed with the invoking user's permissions, so the CLI warns before
starting. A
private, bounded Cargo-owned rustdoc probe captures the final rustdoc `cfg`
arguments after environment and Cargo configuration are applied; `cfg(doc)` is
also recorded explicitly. Rustdoc JSON supplies the authoritative target when
`--target` is omitted. `--target` accepts a concrete Rust target triple;
Cargo's `host-tuple` alias and custom target paths are rejected so persisted
target identity remains literal and deterministic. Library rustdoc JSON omits
private items, while Cargo necessarily generates binary rustdoc JSON with
private items included. In both cases `conkit-signature` retains only public
children reachable through public modules, including documentation-hidden
public items. Cargo output, runtime, temporary artifacts, rustdoc JSON, and
best-effort process cleanup are bounded; cleanup failures retain the primary
process failure and add bounded evidence. The selected source files are reread
and compared after Cargo completes so a changed snapshot never reaches the
domain artifact. Generated contracts record artifact schema,
`conkit-rustdoc-json-v1`, normalized compiler identity and host, rustdoc format,
target triple, Cargo package and selected target, sorted resolved features/cfg
values, and that macro expansion and name resolution occurred. Existing
compiler checks reject syntax/compiler mode or extraction-context mismatches.
Existing compiler generation applies the same exact-context check before
submitting domain work; it never refreshes persisted Cargo/compiler context
implicitly. `--strict` remains a diagnostic policy and never selects an
extractor.

Compiler signatures retain cfg-selected and spanless macro-generated public
items, resolve direct public reexports, expand recursive glob target sets that
rustdoc exposes in the artifact, and normalize generic type-alias applications
to their underlying compiler types. A summary-only external glob target has no
enumerable item set in one crate's rustdoc JSON and fails explicitly, as does
any other reachable rustdoc fact without a lossless representation. Local
items carry tagged provenance: either an exact allowlisted byte range or
compiler-generated ownership at the selected logical crate root.

`signatures` refreshes the signature section while preserving valid sketch
links and records. `sketches` refreshes only sketches already linked from a
signature. `all` refreshes signatures and returns every surviving link from
that same parsed source projection before the sketch domain refreshes those
seeds; a fresh `generate all` creates signatures with zero sketches because
Contract Kit does not invent opt-in sketch coverage. The first generated
document is `main.yml`; existing root-level documents retain their exact
document-local `files` allowlists, which may intentionally overlap.

The `conkit-sketch` library additionally exposes targeted partial refresh for
editor integrations: supplied IDs are validated and updated while unspecified
sketches remain byte-exact. The CLI intentionally requests full refresh and
does not add sketch-selection flags.

Contract Kit tracks managed outputs in
`<contracts>/.contract-kit/generated-files.json`. During reconciliation:

- managed outputs remain tracked and stale owned documents are removed;
- unselected document sections and manual files are preserved;
- unowned destinations are never overwritten; and
- `--adopt-existing` accepts an unowned document only when its bytes exactly
  match the output that would be generated now.

Locking, recovery journals, baseline revalidation, preflight ordering, and
atomic-write mechanics are described in
[the CLI architecture](https://github.com/genaptic/contract-kit/blob/main/conkit/ARCHITECTURE.md).

Generated logical paths must remain portable. Contract Kit rejects
case-equivalent output collisions, Windows-reserved names and characters,
control characters, and trailing spaces or periods on every host. For a
case-only rename, generate through a non-case-equivalent intermediate name
before generating the final spelling.

### Archive contracts

```shell
conkit archive --contracts <DIR> --archive <DIR> [--gzip]
```

Archive creates a collision-safe, timestamp-named `*-archive.gzip` file. The
catalog ordering and gzip payload are deterministic. `--gzip` is optional and
currently selects the same gzip format as omission because gzip is the only
supported archive format.

### Diff contracts

```shell
conkit diff --contracts <DIR> --archive <FILE>
```

Diff reports deterministic signature entries before sketch entries. A
completed comparison exits successfully whether contracts are unchanged or
changes are present; operational or input errors still fail the command.
Diffing is semantic across both families: YAML formatting, YAML comments, YAML
key order, and sketch document relocation do not count as contract changes.
Indentation, tabs, blank lines, horizontal whitespace, comments, and other
tokens inside a sketch's `code` block remain part of its exact-line normalized
code and are semantic. CRLF/LF spelling and one final line terminator are the
only normalized line-ending differences.

## Contract format

Contracts are root-level `.yml` or `.yaml` combined documents. Every document
declares `contract_version: 2`; versionless and v1 documents fail with an
upgrade error and must be recreated. `root` is
resolved relative to the document and must name the selected `--source`.
`files` is the document's exact, duplicate-free Rust-source allowlist; unlisted
source files are ignored, and separate documents may intentionally overlap.
Signature-bearing documents also declare `mode: rust_syntax_v2` or
`mode: rust_compiler_v1`,
`profile: rust_api_v1`, and at least one explicitly typed crate root, with every
crate root included in `files`. Those are the accepted extraction modes;
`rust_api_v1` is the only API profile accepted by contract format v2. A root's
`kind` is explicit in the
document; after the bounded fresh-layout inference described above, it is never
reconstructed from its physical filename. Canonical documents contain
`contract_version`, `root`, `files`, `extraction`, `signatures`, and `sketches`;
use `sketches: []` when no sketches are linked. A minimal document is:

```yaml
contract_version: 2
root: ../src
files:
  - lib.rs
extraction:
  mode: rust_syntax_v2
  profile: rust_api_v1
  crates:
    - id: example
      root: lib.rs
      kind: library
signatures:
- answer_contract:
    file: lib.rs
    signature_type: function
    name: answer
    visibility: public
    parameters: []
    return_type: u8
sketches: []
```

The one-entry map key (`answer_contract`) is the stable user label. Trait
constants, types, and methods share one `items` sequence. Implementation blocks
are nested below their resolved local struct, enum, union, or type alias in an
`implementations` sequence; each block has its own `items`. Instance methods
declare `receiver`, and items inside Rust modules declare `module_path`.
Callable parameters are explicit records with `pattern` and `type`; patterns
are retained for source rendering but are excluded from the API-compatibility
digest.

Structurally repeatable Rust items that share a crate, logical module, item
kind, and name remain distinct by declaration occurrence. Physical file paths
are locator metadata rather than semantic identity. Generation and
regeneration preserve user labels by occurrence. Syntax items and compiler
items with exact provenance can resolve a linked sketch's source span.
Spanless compiler-generated items still participate in signatures but report
that exact sketch source text is unavailable instead of inventing a range.

The syntax extractor builds an allowlist-bounded module graph from explicit
crate roots, inline modules, out-of-line `mod` declarations, and `#[path]`.
Every traversed module file must appear in that document's exact `files`
allowlist; disconnected files require another explicit crate root. Logical
crate/module identity comes from that graph, never from a filename-derived
module guess.

Implementation owners resolve lexically, including supported `self`, `super`,
`crate`, and explicit import aliases, without a global bare-name fallback.
Moving an implementation block or changing an equivalent qualified owner
spelling does not change the contract; ambiguous owners and implementations
for external types are rejected rather than represented as standalone
signatures. A local implementation may name its bare owner or apply its
declared type, lifetime, and const parameters unchanged and in order;
specialized, reordered, nested, or qualifier-parameterized owner applications
are rejected.

Visibility is semantic: private spellings such as inherited visibility and
`pub(self)` canonicalize together, crate visibility canonicalizes together,
and ancestor-restricted visibility resolves to the canonical module identity.
Every function, including one named `main`, uses the ordinary `function`
representation and preserves its canonical semantic visibility.
`signature_type` is the only kind field, and fields that do not apply to the
selected kind are rejected.

`rust_syntax_v2` is deliberately syntactic. It does not run Cargo, compile the
crate, expand macros, evaluate `cfg`, resolve a reexport target, or normalize
compiler-resolved type identity. It retains modeled syntax and emits
deterministic capability warnings where those compiler facts could change the
effective API. Default checks permit warning-only results, strict checks fail
on a capability warning, and warning mode always passes while preserving the
evidence. Unsupported reachable syntax and invalid attributes, module graphs,
visibility, or owner resolution fail closed instead of disappearing from the
contract.

`rust_compiler_v1` adds a `compiler` mapping under `extraction`, including the
artifact and rustdoc format versions, compiler and extractor versions, target
triple, Cargo package and target, sorted features and cfg values, plus
`macro_expansion: true` and `name_resolution: true`. An incomplete compiler
header is rejected and never silently falls back to syntax extraction.

For callables, omitting `abi` means Rust's implicit ABI. Use `abi: extern` for
an unnamed `extern`, or a bare explicit ABI name such as `abi: Rust` or
`abi: C`. Compatibility spellings such as `extern "C"` are not accepted.

A signature opts into a sketch by naming it. Each sketch is a one-entry map:
the map key is its ID and the nested body records the exact linked file,
signature label, signature type, matching policy, and code.

```yaml
contract_version: 2
root: ../src
files:
  - utils.rs
extraction:
  mode: rust_syntax_v2
  profile: rust_api_v1
  crates:
    - id: example
      root: utils.rs
      kind: library
signatures:
- parse_positive_contract:
    file: utils.rs
    signature_type: function
    name: parse_positive
    visibility: public(crate)
    parameters:
      - { pattern: input, type: "&str" }
    return_type: Result<i32, ParseError>
    sketch: parse_positive_happy_path
sketches:
- parse_positive_happy_path:
    file: utils.rs
    signature: parse_positive_contract
    signature_type: function
    matching:
      normalization: exact_lines_v1
      occurrence: at_least_one
    code: |
      pub(crate) fn parse_positive(input: &str) -> Result<i32, ParseError> {
          let trimmed = input.trim();
          Ok(trimmed.parse::<i32>().map_err(|_| ParseError::Invalid)?)
      }
```

Every sketch must be linked by exactly one signature in the same document and
its `signature_type` must match. Signature labels are unique within their
document and may repeat in another document; sketch IDs are globally unique
across the participating catalog. The nested sketch link facts must agree
exactly with the linking signature. Documents using `version`,
`language`, flattened sketch entries, reverse-only links, or split
signature/sketch dialects are rejected.

Generation preserves the original document bytes when the proposed typed
contract is unchanged. When a signature or sketch changes, Contract Kit edits
only the affected lossless YAML nodes and reparses the result to prove that it
matches the proposed typed document before returning bytes.

Sketch matching applies the explicit `exact_lines_v1` policy to the contract
snippet and source identically. It normalizes CRLF to LF and treats one final
line terminator as nonsemantic, while preserving indentation, tabs, internal
and trailing whitespace, blank lines, isolated carriage returns, and arbitrary
non-line-ending bytes exactly. The normalized sketch lines must form a
contiguous ordered source window. `at_least_one` accepts the first occurrence;
`exactly_one` also rejects duplicate, including overlapping, occurrences and
reports their bounded source spans. Missing files and mismatches retain the
contract document, source path, and bounded nearest-candidate evidence.

<a id="rust-library-boundary"></a>

## Rust libraries and API documentation

- [`conkit-signature`](https://docs.rs/conkit-signature) provides the
  runtime-neutral Rust signature contract APIs.
- [`conkit-sketch`](https://docs.rs/conkit-sketch) provides the runtime-neutral
  sketch contract APIs.

The CLI owns filesystem access, persistence, mixed-catalog orchestration, and
archive encoding. See the
[workspace architecture](https://github.com/genaptic/contract-kit/blob/main/ARCHITECTURE.md)
and
[CLI architecture](https://github.com/genaptic/contract-kit/blob/main/conkit/ARCHITECTURE.md)
for structural details.

## Project documentation and community

- [Contributor guide](https://github.com/genaptic/contract-kit/blob/main/CONTRIBUTING.md)
- [Code of Conduct](https://github.com/genaptic/contract-kit/blob/main/CODE_OF_CONDUCT.md)
- [Workspace architecture](https://github.com/genaptic/contract-kit/blob/main/ARCHITECTURE.md)
- [Changelog](https://github.com/genaptic/contract-kit/blob/main/CHANGELOG.md)
- [GitHub Issues](https://github.com/genaptic/contract-kit/issues)
- [GitHub Releases](https://github.com/genaptic/contract-kit/releases)

### End-to-end scenarios

Checked-in CLI scenarios live under `test/scenarios` and run from isolated
temporary workspaces. See the
[scenario authoring guide](https://github.com/genaptic/contract-kit/blob/main/test/scenarios/README.md)
for the versioned manifest schema, typed steps, fixture rules, and targeted
validation commands.

## License

Contract Kit is licensed under the [Apache License 2.0](LICENSE).
