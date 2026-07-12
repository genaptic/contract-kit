use super::{HarnessRepository, Scenario, Suite};

#[test]
fn coverage_registry_is_one_sorted_184_key_source_of_truth() {
    let keys = Suite::required_coverage_keys();

    assert_eq!(keys.len(), 184);
    assert!(keys.windows(2).all(|pair| pair[0] < pair[1]));
    for required in [
        "behavior.check.contracts.missing-listed-source",
        "behavior.check.contracts.multiple-documents",
        "behavior.check.contracts.overlapping-files",
        "behavior.check.contracts.root-mismatch",
        "behavior.check.contracts.signature-unlisted-file",
        "behavior.check.sketch.whitespace-normalized",
        "behavior.diff.error.archive.trailing-data",
        "behavior.check.diagnostic.sketch.orphan-link",
        "behavior.generate.error.invalid-linked-sketch-resolution",
        "behavior.generate.linked-sketch.refresh",
        "behavior.generate.preserve.signatures-when-sketches-selected",
        "behavior.generate.preserve.sketches-when-signatures-selected",
    ] {
        assert!(keys.binary_search(&required).is_ok(), "missing {required}");
    }
    for obsolete in [
        "behavior.check.diagnostic.sketch.missing-file",
        "behavior.generate.cleanup.signatures-selected",
        "behavior.generate.cleanup.sketches-selected",
        "behavior.generate.error.empty-sketch-source",
        "behavior.generate.fresh.sketches",
    ] {
        assert!(
            keys.binary_search(&obsolete).is_err(),
            "obsolete key remains: {obsolete}"
        );
    }
}

#[test]
fn old_cases_fixture_and_scalar_test_formats_are_rejected() {
    let repository = HarnessRepository::new();
    let cases = repository.load_error(
        "old-cases",
        r#"version: 1
cases:
  - name: old
    steps: []
"#,
    );
    let fixture = repository.load_error(
        "old-fixture",
        r#"version: 1
fixture: { source: input }
steps:
  - type: run
    argv: [conkit, --version]
    expect:
      exit_code: 0
      stdout: { kind: empty }
      stderr: { kind: empty }
"#,
    );
    let test_field = repository.load_error(
        "old-test-field",
        r#"version: 1
test: conkit --version
steps: []
"#,
    );
    let scalar = repository.load_error("old-scalar", "conkit --version\n");

    assert!(cases.contains("unknown field `cases`"), "{cases}");
    assert!(fixture.contains("unknown field `fixture`"), "{fixture}");
    assert!(test_field.contains("unknown field `test`"), "{test_field}");
    assert!(scalar.contains("invalid type"), "{scalar}");
}

#[test]
fn manifest_rejects_versions_empty_steps_unknown_fields_and_malformed_yaml() {
    let repository = HarnessRepository::new();
    let version = repository.load_error(
        "bad-version",
        "version: 2\nsteps:\n  - type: run\n    argv: [conkit, --version]\n    expect:\n      exit_code: 0\n      stdout: { kind: empty }\n      stderr: { kind: empty }\n",
    );
    let empty = repository.load_error("empty-steps", "version: 1\nsteps: []\n");
    let unknown = repository.load_error(
        "unknown-top-level",
        "version: 1\nunknown: true\nsteps: []\n",
    );
    let malformed = repository.load_error("malformed", "version: [\n");

    assert!(version.contains("unsupported version 2"), "{version}");
    assert!(empty.contains("steps must not be empty"), "{empty}");
    assert!(unknown.contains("unknown field `unknown`"), "{unknown}");
    assert!(malformed.contains("could not parse"), "{malformed}");
}

#[test]
fn manifest_accepts_known_coverage_and_rejects_unknown_scalar_and_duplicates() {
    let repository = HarnessRepository::new();
    let known_path = repository.write_scenario(
        "known-coverage",
        &repository.coverage_manifest(&[
            "behavior.archive.format.gzip.omitted",
            "behavior.check.contracts.multiple-documents",
            "behavior.diff.error.archive.trailing-data",
            "behavior.generate.linked-sketch.refresh",
            "grammar.version",
            "surface.command.check",
        ]),
    );
    std::fs::create_dir_all(known_path.join("input")).expect("known input");
    Scenario::load(&known_path).expect("known coverage should parse");

    let unknown = repository.load_error(
        "unknown-coverage",
        "version: 1\ncoverage: [behavior.not-real]\nsteps: []\n",
    );
    let scalar = repository.load_error(
        "scalar-coverage",
        "version: 1\ncoverage: grammar.version\nsteps: []\n",
    );
    let duplicate = repository.load_error(
        "duplicate-coverage",
        &repository.coverage_manifest(&["grammar.version", "grammar.version"]),
    );

    assert!(unknown.contains("unknown coverage key"), "{unknown}");
    assert!(scalar.contains("invalid type"), "{scalar}");
    assert!(duplicate.contains("duplicate coverage key"), "{duplicate}");
}

#[test]
fn coverage_requires_run_and_behavior_requires_exhaustive_tree_evidence() {
    let repository = HarnessRepository::new();
    let without_run = repository.load_error(
        "coverage-without-run",
        r#"version: 1
coverage: [grammar.version]
steps:
  - type: assert_tree
    actual: /output
    expected: output
    contents: bytes
"#,
    );
    let behavior_without_tree = repository.load_error(
        "behavior-without-tree",
        r#"version: 1
coverage: [behavior.archive.destination.created]
steps:
  - type: run
    argv: [conkit, --version]
    expect:
      exit_code: 0
      stdout: { kind: empty }
      stderr: { kind: empty }
"#,
    );

    assert!(without_run.contains("no run step"), "{without_run}");
    assert!(
        behavior_without_tree.contains("no assert_tree step"),
        "{behavior_without_tree}"
    );
}

#[test]
fn coverage_audit_aggregates_leaf_manifests_and_reports_sorted_missing_keys() {
    let repository = HarnessRepository::new();
    let keys = Suite::required_coverage_keys();
    repository.write_scenario("coverage-a", &repository.coverage_manifest(&keys[..90]));
    repository.write_scenario("coverage-b", &repository.coverage_manifest(&keys[90..]));

    repository
        .discover()
        .audit_cli_coverage()
        .expect("all 184 keys should aggregate across leaves");

    let incomplete = HarnessRepository::new();
    incomplete.write_scenario("no-coverage", &incomplete.version_manifest(0));
    let actual = incomplete.coverage_error();
    let expected = format!(
        "CLI coverage is incomplete: {} required keys missing\n{}",
        keys.len(),
        keys.join("\n")
    );
    assert_eq!(actual, expected);
}

#[test]
fn manifest_rejects_unknown_step_and_nested_expectation_fields() {
    let repository = HarnessRepository::new();
    let step = repository.load_error(
        "unknown-step-field",
        r#"version: 1
steps:
  - type: run
    argv: [conkit, --version]
    extra: true
    expect:
      exit_code: 0
      stdout: { kind: empty }
      stderr: { kind: empty }
"#,
    );
    let expectation = repository.load_error(
        "unknown-expect-field",
        r#"version: 1
steps:
  - type: run
    argv: [conkit, --version]
    expect:
      exit_code: 0
      extra: true
      stdout: { kind: empty }
      stderr: { kind: empty }
"#,
    );
    let stream = repository.load_error(
        "unknown-stream-kind",
        r#"version: 1
steps:
  - type: run
    argv: [conkit, --version]
    expect:
      exit_code: 0
      stdout: { kind: prefix, value: conkit }
      stderr: { kind: empty }
"#,
    );

    assert!(step.contains("unknown field `extra`"), "{step}");
    assert!(
        expectation.contains("unknown field `extra`"),
        "{expectation}"
    );
    assert!(stream.contains("unknown variant `prefix`"), "{stream}");
}

#[test]
fn manifest_rejects_empty_or_non_conkit_argv_and_invalid_stream_fragments() {
    let repository = HarnessRepository::new();
    let empty_argv = repository.load_error(
        "empty-argv",
        r#"version: 1
steps:
  - type: run
    argv: []
    expect:
      exit_code: 0
      stdout: { kind: empty }
      stderr: { kind: empty }
"#,
    );
    let executable = repository.load_error(
        "wrong-executable",
        r#"version: 1
steps:
  - type: run
    argv: [cargo, --version]
    expect:
      exit_code: 0
      stdout: { kind: empty }
      stderr: { kind: empty }
"#,
    );
    let fragments = repository.load_error(
        "empty-fragments",
        r#"version: 1
steps:
  - type: run
    argv: [conkit, --version]
    expect:
      exit_code: 0
      stdout: { kind: contains_in_order, value: [] }
      stderr: { kind: empty }
"#,
    );

    assert!(
        empty_argv.contains("argv must not be empty"),
        "{empty_argv}"
    );
    assert!(
        executable.contains("argv[0] must be exactly \"conkit\""),
        "{executable}"
    );
    assert!(
        fragments.contains("requires at least one value"),
        "{fragments}"
    );
}

#[test]
fn manifest_rejects_path_traversal_backslashes_and_host_absolute_arguments() {
    let repository = HarnessRepository::new();
    let scenario_path = repository.load_error(
        "scenario-traversal",
        r#"version: 1
steps:
  - type: overlay
    source: ../outside
    destination: /input/value
"#,
    );
    let workspace_path = repository.load_error(
        "workspace-traversal",
        r#"version: 1
steps:
  - type: remove
    path: /input/../outside
"#,
    );
    let backslash = repository.load_error(
        "backslash",
        r#"version: 1
steps:
  - type: run
    argv: [conkit, 'bad\path']
    expect:
      exit_code: 0
      stdout: { kind: empty }
      stderr: { kind: empty }
"#,
    );
    let absolute = repository.load_error(
        "absolute",
        r#"version: 1
steps:
  - type: run
    argv: [conkit, '/host']
    expect:
      exit_code: 0
      stdout: { kind: empty }
      stderr: { kind: empty }
"#,
    );

    assert!(scenario_path.contains("'..'"), "{scenario_path}");
    assert!(workspace_path.contains("'..'"), "{workspace_path}");
    assert!(backslash.contains("backslashes"), "{backslash}");
    assert!(
        absolute.contains("path must begin with exactly /work, /input, or /output"),
        "{absolute}"
    );
}
