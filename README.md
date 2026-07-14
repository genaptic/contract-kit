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
conkit generate all --source src --contracts contracts
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

- **Signatures** describe Rust items and their structure, including names,
  visibility, parameters, return types, ownership, and module placement.
- **Sketches** attach an opt-in code snippet to a named signature. Matching
  ignores blank lines and formatting-only whitespace while preserving token and
  line-order meaning.

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
equivalent to passing `--default`. Default and strict checks fail when contract
diagnostics exist; warning mode retains diagnostics while allowing a completed
check to pass.

The output file extension selects the report format. `.yml` and `.yaml` produce
YAML; `.json` produces JSON. Extension matching is case-insensitive.

### Generate contracts

Generation also requires one target and accepts the same singular aliases.

```shell
conkit generate <all|signatures|sketches> \
  --source <DIR> \
  --contracts <DIR> \
  [--adopt-existing]
```

`signatures` refreshes the signature section while preserving valid sketch
links and records. `sketches` refreshes only sketches already linked from a
signature. `all` refreshes signatures first and then every surviving link; a
fresh `generate all` creates signatures with zero sketches because Contract
Kit does not invent opt-in sketch coverage. The first generated document is
`main.yml`; existing root-level documents retain their disjoint `files`
ownership.

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
key order, sketch document relocation, and whitespace-only snippet edits do
not count as contract changes. Comments and other tokens inside a sketch's
`code` block remain part of its normalized code and are semantic.

## Contract format

Contracts are root-level `.yml` or `.yaml` combined documents. `root` is
resolved relative to the document and must name the selected `--source`.
`files` is the document's exact, disjoint Rust-source allowlist; unlisted source
files are ignored. Canonical documents contain `root`, `files`, `signatures`,
and `sketches`; use `sketches: []` when no sketches are linked. A minimal
document is:

```yaml
root: ../src
files:
  - lib.rs
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

The one-entry map key (`answer_contract`) is the stable user label. Methods are
nested below their owning type or trait; instance methods declare `receiver`,
and items inside Rust modules declare `module_path`.

Repeated Rust item macros that share the same source file, module path, and
name remain distinct by declaration occurrence. Generation and regeneration
preserve their user labels by occurrence, and a linked sketch resolves the
corresponding macro occurrence.

Implementation methods are folded into a source-declared local struct, enum,
union, or type alias. Moving an implementation block or changing an equivalent
qualified owner spelling does not change the contract; ambiguous owners and
implementations for external types are rejected rather than represented as a
standalone signature. A local implementation may name its bare owner or apply
its declared type, lifetime, and const parameters unchanged and in order;
specialized, reordered, nested, or qualifier-parameterized owner applications
are rejected. `signature_type` is the only kind field, and fields that do not
apply to the selected kind are rejected.

For callables, omitting `abi` means Rust's implicit ABI. Use `abi: extern` for
an unnamed `extern`, or a bare explicit ABI name such as `abi: Rust` or
`abi: C`. Compatibility spellings such as `extern "C"` are not accepted.

A signature opts into a sketch by naming it. The matching sketch is flattened:
its identifier is the null-valued key beside `signature_type` and `code`.

```yaml
root: ../src
files:
  - utils.rs
signatures:
- parse_positive_contract:
    file: utils.rs
    signature_type: function
    name: parse_positive
    visibility: public(crate)
    parameters:
      - input: "&str"
    return_type: Result<i32, ParseError>
    sketch: parse_positive_happy_path
sketches:
- parse_positive_happy_path:
  signature_type: function
  code: |
    pub(crate) fn parse_positive(input: &str) -> Result<i32, ParseError> {
        let trimmed = input.trim();
        Ok(trimmed.parse::<i32>().map_err(|_| ParseError::Invalid)?)
    }
```

Every sketch must be linked by exactly one signature in the same document and
its `signature_type` must match. Signature labels and sketch IDs are globally
unique across all participating documents. Documents using `version`,
`language`, reverse sketch-to-signature links, or split signature/sketch
dialects are rejected.

Sketch matching normalizes the contract snippet and source identically: blank
lines are ignored, indentation does not matter, and repeated whitespace within
a line collapses to one space. After normalization, all sketch lines must form
one contiguous ordered window in the source. Changed tokens, changed line
order, missing lines, or nonempty normalized lines inserted inside that window
still fail matching.

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
