use super::{ConkitCli, HarnessRepository, Scenario, TempDir};

#[test]
fn suite_executes_all_five_ordered_step_types() {
    let repository = HarnessRepository::new();
    repository.write_scenario(
        "all-steps",
        &format!(
            r#"version: 1
steps:
  - type: run
    argv: [conkit, --version]
    expect:
      exit_code: 0
      stdout:
        kind: exact
        value: |
          conkit {}
      stderr: {{ kind: empty }}
  - type: overlay
    source: assets/temporary.txt
    destination: /output/temporary.txt
  - type: capture
    name: temporary
    directory: /output
    selector: {{ kind: only_file }}
  - type: remove
    path: /output/temporary.txt
  - type: assert_tree
    actual: /output
    expected: output
    contents: bytes
"#,
            env!("CARGO_PKG_VERSION")
        ),
    );
    repository.write("all-steps/assets/temporary.txt", "temporary\n");
    std::fs::create_dir_all(repository.scenario_path("all-steps").join("output"))
        .expect("empty expected output");

    repository
        .discover()
        .run()
        .expect("all ordered step kinds should execute");
}

#[test]
fn capture_supports_every_selector_and_substitutes_a_captured_argv_path() {
    let repository = HarnessRepository::new();
    repository.write_scenario(
        "capture-selectors",
        r#"version: 1
steps:
  - type: overlay
    source: assets/captures
    destination: /output/captures
  - type: capture
    name: only
    directory: /output/captures/one
    selector: { kind: only_file }
  - type: capture
    name: named
    directory: /output/captures/named
    selector: { kind: file_name, value: alpha.txt }
  - type: capture
    name: suffix
    directory: /output/captures/suffix
    selector: { kind: file_name_suffix, value: .log }
  - type: capture
    name: first-archive
    directory: /output/captures/uncaptured
    selector: { kind: file_name, value: first.gzip }
  - type: capture
    name: second-archive
    directory: /output/captures/uncaptured
    selector: { kind: uncaptured_file_name_suffix, value: .gzip }
  - type: run
    argv: [conkit, "${capture.only}"]
    expect:
      exit_code: 2
      stdout: { kind: empty }
      stderr:
        kind: contains_in_order
        value:
          - "error: unrecognized subcommand"
          - "/output/captures/one/only.data"
  - type: assert_tree
    actual: /output
    expected: output
    contents: bytes
"#,
    );
    for (relative, contents) in [
        ("one/only.data", "one\n"),
        ("named/alpha.txt", "alpha\n"),
        ("named/beta.txt", "beta\n"),
        ("suffix/alpha.txt", "alpha\n"),
        ("suffix/beta.log", "beta\n"),
        ("uncaptured/first.gzip", "first\n"),
        ("uncaptured/second.gzip", "second\n"),
    ] {
        repository.write(
            &format!("capture-selectors/assets/captures/{relative}"),
            contents,
        );
        repository.write(
            &format!("capture-selectors/output/captures/{relative}"),
            contents,
        );
    }

    repository
        .discover()
        .run()
        .expect("selectors and captured argv substitution should work");
}

#[test]
fn exact_file_stream_expectations_load_leaf_local_snapshots() {
    let repository = HarnessRepository::new();
    repository.write_scenario(
        "exact-file",
        r#"version: 1
steps:
  - type: run
    argv: [conkit, --version]
    expect:
      exit_code: 0
      stdout: { kind: exact_file, value: assets/version.txt }
      stderr: { kind: empty }
"#,
    );
    repository.write(
        "exact-file/assets/version.txt",
        format!("conkit {}\n", env!("CARGO_PKG_VERSION")),
    );

    repository
        .discover()
        .run()
        .expect("exact file snapshot should match");
}

#[test]
fn exact_file_mismatches_name_the_stream_and_snapshot() {
    let repository = HarnessRepository::new();
    repository.write_scenario(
        "exact-file-mismatch",
        r#"version: 1
steps:
  - type: run
    argv: [conkit, --version]
    expect:
      exit_code: 0
      stdout: { kind: exact_file, value: assets/version.txt }
      stderr: { kind: empty }
"#,
    );
    repository.write("exact-file-mismatch/assets/version.txt", "wrong\n");

    let message = repository.run_error();

    assert!(message.contains("exact-file-mismatch: step 1"), "{message}");
    assert!(message.contains("stdout did not match"), "{message}");
    assert!(message.contains("assets/version.txt"), "{message}");
}

#[test]
fn run_normalizes_sandbox_paths_and_supports_ordered_fragments() {
    let repository = HarnessRepository::new();
    repository.write_scenario(
        "normalized-error",
        r#"version: 1
steps:
  - type: run
    argv: [conkit, /input/not-a-command]
    expect:
      exit_code: 2
      stdout: { kind: empty }
      stderr:
        kind: contains_in_order
        value:
          - "error: unrecognized subcommand"
          - "/input/not-a-command"
"#,
    );

    repository
        .discover()
        .run()
        .expect("normalized error stream should match ordered fragments");
}

#[test]
fn stream_normalization_handles_canonical_sandbox_aliases() {
    let repository = HarnessRepository::new();
    let path = repository.write_scenario("canonical-paths", &repository.version_manifest(0));
    let scenario = Scenario::load(&path).expect("load canonical path scenario");
    let sandbox = scenario.sandbox().expect("canonical path sandbox");
    let canonical_input = std::fs::canonicalize(sandbox.input()).expect("canonical input");
    let stream = format!(
        "root {} differs from {}\n",
        canonical_input.join("other").display(),
        canonical_input.join("src").display()
    );

    assert_eq!(
        sandbox.normalize_stream(&stream),
        "root /input/other differs from /input/src\n"
    );
    sandbox.close().expect("close canonical path sandbox");
}

#[test]
fn stream_normalization_only_changes_substituted_sandbox_paths() {
    let repository = HarnessRepository::new();
    let path = repository.write_scenario("windows-shaped-stream", &repository.version_manifest(0));
    let scenario = Scenario::load(&path).expect("load stream scenario");
    let parent = TempDir::new().expect("temporary sandbox parent");
    let unicode_parent = parent.path().join("parent with spaces Ω");
    std::fs::create_dir_all(&unicode_parent).expect("unicode sandbox parent");
    let sandbox = scenario
        .sandbox_in(&unicode_parent)
        .expect("sandbox in unicode parent");
    let input_native = sandbox.input().to_string_lossy().into_owned();
    let input_slash = input_native.replace('\\', "/");
    let output_native = sandbox.output().to_string_lossy().into_owned();
    let work_native = sandbox.work().to_string_lossy().into_owned();
    let displayed_name = ConkitCli::displayed_name();
    let stream = format!(
        "native input: {input_native}\\nested\\file.rs regex \\d+\r\n\
         slash input: `{input_slash}/dir with space\\naïve.rs` regex \\w+\r\n\
         repeated: {input_native}\\one then {input_native}\\two\r\n\
         output: {output_native}\\report.yml literal C:\\temp\r\n\
         work: {work_native}\\scratch\\file\r\n\
         native prefix: {input_native}-extra\\value\r\n\
         embedded: prefix{input_native}\\value\r\n\
         virtual prefixes: /input-extra\\value /outputting\\value /workbench\\value\r\n\
         literal virtual: /input\\literal\r\n\
         executable: {displayed_name}\r\n"
    );
    let expected = format!(
        "native input: /input/nested/file.rs regex \\d+\n\
         slash input: `/input/dir with space/naïve.rs` regex \\w+\n\
         repeated: /input/one then /input/two\n\
         output: /output/report.yml literal C:\\temp\n\
         work: /work/scratch/file\n\
         native prefix: /work/input-extra/value\n\
         embedded: prefix{input_native}\\value\n\
         virtual prefixes: /input-extra\\value /outputting\\value /workbench\\value\n\
         literal virtual: /input\\literal\n\
         executable: {displayed_name}\n"
    );

    assert_eq!(sandbox.normalize_stream(&stream), expected);

    sandbox.close().expect("close stream sandbox");
}

#[test]
fn run_failures_name_the_leaf_and_step_without_case_names() {
    let repository = HarnessRepository::new();
    repository.write_scenario("failing-leaf", &repository.version_manifest(7));

    let message = repository.run_error();

    assert!(message.contains("failing-leaf: step 1"), "{message}");
    assert!(!message.contains(" / "), "{message}");
    assert!(message.contains("expected exit code 7, got 0"), "{message}");
}
