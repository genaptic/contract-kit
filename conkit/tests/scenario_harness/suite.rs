use super::{HarnessRepository, Scenario, Suite, TempDir};

#[test]
fn suite_discovers_one_scenario_per_leaf_in_stable_relative_order() {
    let repository = HarnessRepository::new();
    repository.write_scenario("z-last", &repository.version_manifest(0));
    repository.write_scenario("a-group/b-first", &repository.version_manifest(0));
    std::fs::create_dir_all(repository.path().join("not-a-scenario")).expect("unrelated directory");

    let suite = repository.discover();

    assert_eq!(suite.scenario_ids(), ["a-group/b-first", "z-last"]);
}

#[test]
fn manifest_is_one_scenario_and_implicit_input_is_copied_in_isolation() {
    let repository = HarnessRepository::new();
    let scenario_path = repository.write_scenario("copy-input", &repository.version_manifest(0));
    repository.write("copy-input/input/nested/value.txt", "immutable\n");

    let scenario = Scenario::load(&scenario_path).expect("load leaf scenario");
    let first = scenario.sandbox().expect("first sandbox");
    let second = scenario.sandbox().expect("second sandbox");

    assert_eq!(
        std::fs::read_to_string(first.input().join("nested/value.txt"))
            .expect("first copied input"),
        "immutable\n"
    );
    assert_eq!(
        std::fs::read_to_string(second.input().join("nested/value.txt"))
            .expect("second copied input"),
        "immutable\n"
    );
    assert!(first.output().is_dir());
    assert!(second.output().is_dir());
    assert_ne!(first.work(), second.work());
    std::fs::write(first.input().join("nested/value.txt"), "mutated\n")
        .expect("mutate sandbox only");
    assert_eq!(
        std::fs::read_to_string(scenario_path.join("input/nested/value.txt"))
            .expect("checked-in input"),
        "immutable\n"
    );

    first.close().expect("close first sandbox");
    second.close().expect("close second sandbox");
}

#[test]
fn scenario_without_input_still_creates_all_virtual_roots() {
    let repository = HarnessRepository::new();
    let path = repository.write_scenario("empty-input", &repository.version_manifest(0));
    let scenario = Scenario::load(&path).expect("load scenario without input");
    let sandbox = scenario.sandbox().expect("sandbox without input");

    assert!(sandbox.work().is_dir());
    assert!(sandbox.input().is_dir());
    assert!(sandbox.output().is_dir());
    assert_eq!(
        std::fs::read_dir(sandbox.input())
            .expect("read empty input")
            .count(),
        0
    );

    sandbox.close().expect("close sandbox");
}

#[test]
fn discovery_rejects_empty_roots_and_aggregates_invalid_leaf_manifests() {
    let empty = HarnessRepository::new();
    let Err(error) = Suite::discover(empty.path()) else {
        panic!("empty scenario root should fail");
    };
    let error = error.to_string();
    assert!(error.contains("no scenario.yml files found"), "{error}");

    let repository = HarnessRepository::new();
    repository.write_scenario("a-invalid", "version: 2\nsteps: []\n");
    repository.write_scenario("b-invalid", "version: 3\nsteps: []\n");
    let Err(aggregated) = Suite::discover(repository.path()) else {
        panic!("all invalid leaves should be reported");
    };
    let aggregated = aggregated.to_string();
    assert!(
        aggregated.contains("a-invalid/scenario.yml"),
        "{aggregated}"
    );
    assert!(
        aggregated.contains("b-invalid/scenario.yml"),
        "{aggregated}"
    );
}

#[cfg(unix)]
#[test]
fn implicit_input_and_scenario_assets_reject_symlinks() {
    use std::os::unix::fs::symlink;

    let input_repository = HarnessRepository::new();
    let input_path =
        input_repository.write_scenario("input-symlink", &input_repository.version_manifest(0));
    input_repository.write("outside.txt", "outside\n");
    std::fs::create_dir_all(input_path.join("input")).expect("input directory");
    symlink(
        input_repository.path().join("outside.txt"),
        input_path.join("input/link.txt"),
    )
    .expect("input symlink");
    let scenario = Scenario::load(&input_path).expect("load input symlink scenario");
    let Err(error) = scenario.sandbox() else {
        panic!("input symlink must fail");
    };
    let error = error.to_string();
    assert!(error.contains("symlink or special file"), "{error}");

    let asset_repository = HarnessRepository::new();
    let asset_path = asset_repository.write_scenario(
        "asset-symlink",
        r#"version: 1
steps:
  - type: overlay
    source: assets/link.txt
    destination: /output/link.txt
"#,
    );
    std::fs::create_dir_all(asset_path.join("assets")).expect("assets directory");
    asset_repository.write("outside.txt", "outside\n");
    symlink(
        asset_repository.path().join("outside.txt"),
        asset_path.join("assets/link.txt"),
    )
    .expect("asset symlink");
    let message = asset_repository.run_error();
    assert!(message.contains("unsupported entry"), "{message}");

    let ancestor_repository = HarnessRepository::new();
    let ancestor_path = ancestor_repository
        .write_scenario("sandbox-ancestor", &ancestor_repository.version_manifest(0));
    let ancestor_scenario = Scenario::load(&ancestor_path).expect("load sandbox ancestor scenario");
    let ancestor_sandbox = ancestor_scenario.sandbox().expect("ancestor sandbox");
    let outside = TempDir::new().expect("outside directory");
    symlink(outside.path(), ancestor_sandbox.output().join("escape"))
        .expect("sandbox ancestor symlink");
    let escaped = ancestor_sandbox
        .validate_sandbox_path(&ancestor_sandbox.output().join("escape/value.txt"))
        .expect_err("symlinked sandbox ancestors must fail")
        .to_string();
    assert!(escaped.contains("unsupported entry"), "{escaped}");
    ancestor_sandbox.close().expect("close ancestor sandbox");
}
