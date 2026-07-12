use super::{HarnessRepository, Scenario, TempDir};

#[test]
fn sandbox_argument_resolution_enforces_virtual_roots_and_capture_binding() {
    let repository = HarnessRepository::new();
    let path = repository.write_scenario("arguments", &repository.version_manifest(0));
    let scenario = Scenario::load(&path).expect("load arguments scenario");
    let sandbox = scenario.sandbox().expect("arguments sandbox");

    assert_eq!(
        sandbox
            .resolve_argument_for_test("/input/source")
            .expect("input virtual path"),
        sandbox.input().join("source").into_os_string()
    );
    let embedded_path = sandbox
        .resolve_argument_for_test("--source=/input/source")
        .expect_err("virtual paths must remain separate argv entries")
        .to_string();
    let missing = sandbox
        .resolve_argument_for_test("${capture.archive}")
        .expect_err("unbound capture must fail")
        .to_string();
    let prefix = sandbox
        .resolve_argument_for_test("/input-extra")
        .expect_err("root prefix must fail")
        .to_string();

    assert!(missing.contains("has not been bound"), "{missing}");
    assert!(
        prefix.contains("path must begin with exactly /work, /input, or /output"),
        "{prefix}"
    );
    assert!(
        embedded_path.contains("host-absolute paths are not allowed"),
        "{embedded_path}"
    );
    sandbox.close().expect("close arguments sandbox");
}

#[test]
fn actual_crlf_is_not_normalized_in_text_tree_comparison() {
    let expected_crlf = HarnessRepository::new();
    expected_crlf.write_scenario(
        "expected-crlf",
        r#"version: 1
steps:
  - type: overlay
    source: assets/actual.txt
    destination: /output/value.txt
  - type: assert_tree
    actual: /output
    expected: output
    contents: text
"#,
    );
    expected_crlf.write("expected-crlf/assets/actual.txt", b"line one\nline two\n");
    expected_crlf.write(
        "expected-crlf/output/value.txt",
        b"line one\r\nline two\r\n",
    );

    expected_crlf
        .discover()
        .run()
        .expect("CRLF in the checked-in expected file should normalize");

    let actual_crlf = HarnessRepository::new();
    actual_crlf.write_scenario(
        "actual-crlf",
        r#"version: 1
steps:
  - type: overlay
    source: assets/actual.txt
    destination: /output/value.txt
  - type: assert_tree
    actual: /output
    expected: output
    contents: text
"#,
    );
    actual_crlf.write("actual-crlf/assets/actual.txt", b"line one\r\nline two\r\n");
    actual_crlf.write("actual-crlf/output/value.txt", b"line one\nline two\n");

    let message = actual_crlf.run_error();
    assert!(message.contains("value.txt: content mismatch"), "{message}");

    let bytes_repository = HarnessRepository::new();
    bytes_repository.write_scenario(
        "bytes-tree",
        r#"version: 1
steps:
  - type: overlay
    source: assets/actual.txt
    destination: /output/value.txt
  - type: assert_tree
    actual: /output
    expected: output
    contents: bytes
"#,
    );
    bytes_repository.write("bytes-tree/assets/actual.txt", b"line\r\n");
    bytes_repository.write("bytes-tree/output/value.txt", b"line\n");
    let message = bytes_repository.run_error();
    assert!(message.contains("content mismatch"), "{message}");
}

#[test]
fn exhaustive_tree_comparison_reports_shape_and_content_failures() {
    let repository = HarnessRepository::new();
    repository.write_scenario(
        "tree-mismatch",
        r#"version: 1
steps:
  - type: overlay
    source: assets/actual
    destination: /output
  - type: assert_tree
    actual: /output
    expected: output
    contents: text
"#,
    );
    repository.write("tree-mismatch/assets/actual/changed.txt", "actual\n");
    repository.write("tree-mismatch/assets/actual/extra.txt", "extra\n");
    repository.write("tree-mismatch/output/changed.txt", "expected\n");
    repository.write("tree-mismatch/output/missing.txt", "missing\n");

    let message = repository.run_error();

    assert!(
        message.contains("changed.txt: content mismatch"),
        "{message}"
    );
    assert!(message.contains("extra.txt: unexpected"), "{message}");
    assert!(message.contains("missing.txt: missing"), "{message}");
}

#[test]
fn overlay_remove_and_capture_fail_closed_on_invalid_state() {
    let overlay_repository = HarnessRepository::new();
    overlay_repository.write_scenario(
        "overlay-conflict",
        r#"version: 1
steps:
  - type: overlay
    source: assets/tree
    destination: /input/keep.txt
"#,
    );
    overlay_repository.write("overlay-conflict/assets/tree/file.txt", "value\n");
    overlay_repository.write("overlay-conflict/input/keep.txt", "keep\n");
    let overlay = overlay_repository.run_error();
    assert!(overlay.contains("conflicts with destination"), "{overlay}");

    let remove_repository = HarnessRepository::new();
    remove_repository.write_scenario(
        "remove-root",
        r#"version: 1
steps:
  - type: remove
    path: /output
"#,
    );
    let remove = remove_repository.run_error();
    assert!(
        remove.contains("refusing to remove sandbox root"),
        "{remove}"
    );

    let capture_repository = HarnessRepository::new();
    capture_repository.write_scenario(
        "capture-zero",
        r#"version: 1
steps:
  - type: capture
    name: archive
    directory: /output
    selector: { kind: only_file }
"#,
    );
    let capture = capture_repository.run_error();
    assert!(capture.contains("matched 0 files"), "{capture}");

    let duplicate_repository = HarnessRepository::new();
    duplicate_repository.write_scenario(
        "capture-duplicate",
        r#"version: 1
steps:
  - type: overlay
    source: assets/value.txt
    destination: /output/value.txt
  - type: capture
    name: repeated
    directory: /output
    selector: { kind: only_file }
  - type: capture
    name: repeated
    directory: /output
    selector: { kind: only_file }
"#,
    );
    duplicate_repository.write("capture-duplicate/assets/value.txt", "value\n");
    let duplicate = duplicate_repository.run_error();
    assert!(duplicate.contains("already bound"), "{duplicate}");
}

#[test]
fn cargo_workspace_validation_uses_the_copied_implicit_input() {
    let repository = HarnessRepository::new();
    let path = repository.write_scenario(
        "cargo-workspace",
        &format!(
            r#"version: 1
cargo_workspace: true
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
"#,
            env!("CARGO_PKG_VERSION")
        ),
    );
    repository.write(
        "cargo-workspace/input/Cargo.toml",
        "[workspace]\nmembers = [\"member\"]\nresolver = \"3\"\n",
    );
    repository.write(
        "cargo-workspace/input/member/Cargo.toml",
        "[package]\nname = \"fixture-member\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    );
    repository.write(
        "cargo-workspace/input/member/src/lib.rs",
        "pub fn value() {}\n",
    );
    repository.write("cargo-workspace/input/Cargo.lock", "not copied\n");
    repository.write("cargo-workspace/input/target/noise.txt", "not copied\n");

    let scenario = Scenario::load(&path).expect("load cargo workspace scenario");
    let sandbox = scenario.sandbox().expect("validate copied cargo workspace");

    assert!(!sandbox.input().join("Cargo.lock").exists());
    assert!(!sandbox.input().join("target").exists());
    assert!(sandbox.input().join("member/src/lib.rs").is_file());
    sandbox.close().expect("close cargo sandbox");
}

#[test]
fn sandbox_supports_space_and_unicode_parent_paths() {
    let repository = HarnessRepository::new();
    let path = repository.write_scenario("unicode-parent", &repository.version_manifest(0));
    let scenario = Scenario::load(&path).expect("load unicode parent scenario");
    let parent = TempDir::new().expect("outer parent");
    let nested = parent.path().join("space and ünicode");
    std::fs::create_dir_all(&nested).expect("unicode sandbox parent");

    let sandbox = scenario
        .sandbox_in(&nested)
        .expect("sandbox in unicode parent");

    assert!(sandbox.work().starts_with(&nested));
    sandbox.close().expect("close unicode sandbox");
}
