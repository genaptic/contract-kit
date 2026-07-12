# conkit

Contract Kit (`conkit`) defines hard, reviewable contract guardrails for
agent-assisted software development. It can generate contracts from a source
tree, check source against existing contracts, archive a contract catalog, and
compare the current catalog with an archive.

The Rust workspace has three members:

- `conkit`: the command-line adapter and Cargo binary target. The installed
  command is `conkit` on Windows, macOS, and Linux.
- `conkit-signature`: Rust signature contracts generated from source structure.
- `conkit-sketch`: language-neutral snippet contracts checked by normalized line
  matching.

Initial signature support targets Rust. Sketches are opt-in snippets linked
from a named signature in the same combined contract document.

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

Contract Kit records managed outputs in
`<contracts>/.contract-kit/generated-files.json`. Version 3 tracks each
combined document once. After domain generation finishes, persistence
reconciliation acquires an exclusive lock, recovers any updating journal, and
revalidates the generation baseline before preflighting the new output.
Reconciliation preserves unselected document sections and manual files,
removes stale owned documents, and refuses to overwrite an unowned
destination.
`--adopt-existing` adopts only unowned documents whose bytes exactly match the
output that would be generated now; any mismatch aborts before generated
output changes. Journal recovery and atomic persistence mechanics are
described in [the CLI architecture](conkit/ARCHITECTURE.md).

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

## Rust library boundary

The `conkit-signature` and `conkit-sketch` crates expose runtime-neutral async kits over
logical `CatalogPath` values and `FileCatalog` byte entries. The signature
domain can also resolve a linked Rust item into neutral seed text; the CLI
adapts that seed into a sketch refresh without coupling the two crates.
Callers decide where returned report or combined-document bytes are persisted.
Archive encoding and decoding belong to the CLI because one archive carries
the complete mixed catalog; each domain receives decoded catalogs and owns
only its semantic diff. Domain crates do not read operating-system roots,
parse command-line arguments, or print terminal output. See the
[workspace architecture](ARCHITECTURE.md) and each crate's rustdoc for the
request and response APIs.

## End-to-end scenarios

Checked-in CLI scenarios live under `test/scenarios` and run from isolated
temporary workspaces. See the
[scenario authoring guide](test/scenarios/README.md) for the versioned manifest
schema, typed steps, fixture rules, and targeted validation commands.
