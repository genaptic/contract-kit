# End-to-End Scenario Guide

Every directory containing `scenario.yml` is one independently owned end-to-end
scenario. The global runner discovers manifests recursively, does not follow
symlinks, sorts IDs by their path relative to `test/scenarios`, and executes
all leaves before aggregating failures.

```text
test/scenarios/<area>/<scenario-name>/
├── scenario.yml
├── input/       # optional, copied implicitly
├── output/      # expected exhaustive tree when needed
└── assets/      # optional overlays or stream snapshots
```

Do not use sibling references, shared fixtures, fixture factories, symlinks,
aggregate `behavior`/`grammar` manifests, or a hardcoded registration list.

## Manifest version 1

Version 1 is deliberately one-scenario-per-manifest:

```yaml
version: 1
coverage:
  - behavior.check.matrix.all.strict.matching
steps:
  - type: run
    argv:
      - conkit
      - check
      - all
      - --source
      - /input/src
      - --contracts
      - /input/contracts
      - --output
      - /output/report.yml
      - --strict
    expect:
      exit_code: 0
      stdout:
        kind: exact
        value: |
          contract check passed: 1 signatures, 0 sketches
      stderr:
        kind: empty

  - type: assert_tree
    actual: /output
    expected: output
    contents: text
```

`cargo_workspace` defaults to `false`; `coverage` defaults to empty. `steps`
must be nonempty. Unknown fields, unknown coverage keys, duplicate keys, the
removed `cases`/`fixture` aggregate shape, and the legacy scalar command shape
are errors. The scenario ID is its relative directory, not a manifest field.

When `input/` exists, the harness recursively copies it into a fresh sandbox.
It always creates isolated `/work`, `/input`, and `/output` roots. A new sandbox
is created and explicitly closed for every manifest.

## Run and stream expectations

`run.argv` is an array whose first element must be exactly `conkit`. The harness
strips that conceptual name and starts a fresh compiled `conkit`; it never
uses a shell and supports no quoting, pipeline, redirection, stdin, or
manifest-controlled environment. `cwd` is optional and defaults to `/work`.

Every run asserts an exact exit code and both UTF-8 streams. Expectations are:

- `empty`: no logical text;
- `exact`: inline text equals the normalized stream;
- `exact_file`: text loaded from a leaf-local file equals the normalized
  stream;
- `contains_in_order`: every nonempty stable fragment occurs in order.

```yaml
stdout:
  kind: exact_file
  value: assets/root-help.txt
```

Use `exact_file` for substantial help and parser-error snapshots. Use
`contains_in_order` only for timestamped paths, platform-owned I/O suffixes,
or codec/parser-library wording. Exit codes and the other stream remain exact.

Logical comparison normalizes CRLF, the real sandbox roots to `/work`,
`/input`, and `/output`, and path separators only within substituted sandbox
paths. The executable already identifies itself as `conkit` on every platform;
unrelated backslashes remain exact.
Test processes inherit the parent environment except for deterministic Clap
presentation: `COLUMNS=100`, `LINES=24`, and `NO_COLOR=1` are set;
`CLICOLOR` and `CLICOLOR_FORCE` are removed.

## Ordered mutation steps

The five closed step types are:

- `run`: execute `conkit` and assert process results.
- `overlay`: copy one leaf-local regular file or recursively merge a local
  directory into a sandbox destination. It creates directories and replaces
  regular files without deleting unrelated entries.
- `remove`: require and remove one sandbox file or directory tree. The three
  sandbox roots themselves cannot be removed.
- `capture`: bind exactly one direct regular file selected by `only_file`,
  `file_name`, `file_name_suffix`, or `uncaptured_file_name_suffix`. A later
  argv element may be exactly `${capture.name}`.
- `assert_tree`: compare an actual sandbox directory with a leaf-local expected
  directory recursively, including empty directories and entry types.

All checked-in and sandbox trees reject symlinks and special files. Overlay
type conflicts, missing removals, zero/multiple capture matches, duplicate
capture names, and use-before-bind are errors.

`assert_tree.contents: text` requires UTF-8 and normalizes CRLF only in the
checked-in expected bytes. `contents: bytes` compares raw bytes. Missing,
extra, type, and content mismatches are all reported in sorted path order.
There is no ignore list: expected trees include every observable report,
combined contract document, ownership manifest, empty `generation.lock`, and
sentinel.

## Path and fixture isolation

Sandbox paths must be exactly `/work`, `/input`, or `/output`, or a descendant
using `/`. Scenario-relative inputs must remain under their own leaf. Host
absolute paths, `.`/`..`, empty components, backslashes, colon-bearing portable
components, prefix tricks such as `/input-extra`, partial placeholders, and
unknown captures are rejected.

Fixture copying skips only local debris that cannot be canonical input:
`target` directories, `Cargo.lock`, and `.DS_Store`. When
`cargo_workspace: true`, Cargo metadata runs with `--no-deps` against the
copied `/input/Cargo.toml` and must report `/input` as its workspace root.

Areas with generated text use inherited `.gitattributes` files to pin source,
overlays, and goldens to LF. `archive-rust`, `check-rust`, and `generate-rust`
own area-level text rules; diff leaves retain local rules where corrupt or
generated `*.gzip` fixtures must be binary. Deliberate binary inputs use
`-text`, and corrupt gzip fixtures keep readable payload source when one exists
and deterministic gzip metadata.

An overlay may reuse its own leaf's checked-in `output/` only for an inert
sentinel or filesystem blocker whose exact bytes are also the expected final
state. Semantic contracts, malformed inputs, ownership states, archives, and
lifecycle stimuli remain independent under `input/` or `assets/`, even when
their bytes happen to match a later output.

## Product evidence rules

- `grammar.*` is an exact help, version, alias, parser-error, or conflict
  snapshot.
- `surface.*` proves only command/target/mode/format/option reachability.
- `behavior.*` requires executable process evidence plus at least one
  exhaustive tree assertion.

Coverage declarations are evidence labels, not implementation comments. A key
may appear in more than one leaf, but every required key must appear at least
once.

Semantic check mismatches write exact reports before exiting. Operational
failures do not announce success and normally produce no report. Generation
success includes version-3 document ownership and the empty lock. Writer
preflight failures may leave the lock but must preserve user/committed bytes;
domain and overlap failures occur before ownership creation. Fresh all-family
generation creates zero sketches; sketch generation refreshes only explicit
signature links. A signature-only update preserves the sketch section, while
all-family generation may remove a stale signature and its link together.

Dynamic archives are captured, validated by exact `conkit diff` output, removed,
and followed by exhaustive input/output assertions. Changed diffs exit `0`,
with signature entries before sketch entries. Archive corruption leaves the
input and sentinel trees unchanged.

## Required coverage registry

The closed, lexicographically sorted registry is
[`REQUIRED_COVERAGE_KEYS`](../../conkit/tests/support/scenario.rs).
Unknown keys fail manifest deserialization, and the checked-in coverage audit
requires executable evidence for every entry.

## Validation

Run focused gates from the workspace root:

```shell
cargo test --locked -p conkit --bin conkit archive
cargo test --locked -p conkit --test check
cargo test --locked -p conkit --test generate
cargo test --locked -p conkit --test archive_diff
cargo test --locked -p conkit --test scenario_harness
cargo test --locked -p conkit --test scenarios
cargo test --locked -p conkit --test cli_help
```

Then run the checked, locked workspace gates from the root `AGENTS.md`. Verify
that scenario execution changed no checked-in input, output, archive,
ownership, or temporary file.
