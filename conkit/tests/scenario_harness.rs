mod support;

#[allow(dead_code)]
#[path = "support/scenario.rs"]
mod scenario;

#[path = "scenario_harness/execution.rs"]
mod execution;
#[path = "scenario_harness/filesystem.rs"]
mod filesystem;
#[path = "scenario_harness/manifest.rs"]
mod manifest;
#[path = "scenario_harness/suite.rs"]
mod suite;

use std::path::{Path, PathBuf};

use assert_fs::TempDir;
use scenario::{Scenario, Suite};
use support::ConkitCli;

struct HarnessRepository {
    root: TempDir,
}

impl HarnessRepository {
    fn new() -> Self {
        Self {
            root: TempDir::new().expect("synthetic scenario root"),
        }
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    fn scenario_path(&self, id: &str) -> PathBuf {
        self.path().join(id)
    }

    fn write_scenario(&self, id: &str, manifest: &str) -> PathBuf {
        let scenario = self.scenario_path(id);
        std::fs::create_dir_all(&scenario).expect("synthetic scenario directory");
        std::fs::write(scenario.join("scenario.yml"), manifest)
            .expect("synthetic scenario manifest");
        scenario
    }

    fn write(&self, relative: &str, contents: impl AsRef<[u8]>) {
        let path = self.path().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("synthetic file parent");
        }
        std::fs::write(path, contents).expect("synthetic file");
    }

    fn version_manifest(&self, exit_code: i32) -> String {
        format!(
            r#"version: 1
steps:
  - type: run
    argv: [conkit, --version]
    expect:
      exit_code: {exit_code}
      stdout:
        kind: exact
        value: |
          conkit {}
      stderr: {{ kind: empty }}
"#,
            env!("CARGO_PKG_VERSION")
        )
    }

    fn coverage_manifest(&self, coverage: &[&str]) -> String {
        let mut manifest = "version: 1\ncoverage:\n".to_owned();
        for key in coverage {
            manifest.push_str(&format!("  - {key}\n"));
        }
        manifest.push_str(&format!(
            r#"steps:
  - type: run
    argv: [conkit, --version]
    expect:
      exit_code: 0
      stdout:
        kind: exact
        value: |
          conkit {}
      stderr: {{ kind: empty }}
  - type: assert_tree
    actual: /input
    expected: input
    contents: bytes
"#,
            env!("CARGO_PKG_VERSION")
        ));
        manifest
    }

    fn discover(&self) -> Suite {
        Suite::discover(self.path()).expect("discover synthetic scenarios")
    }

    fn load_error(&self, id: &str, manifest: &str) -> String {
        let path = self.write_scenario(id, manifest);
        let Err(error) = Scenario::load(&path) else {
            panic!("scenario {id} should fail to load");
        };
        error.to_string()
    }

    fn run_error(&self) -> String {
        self.discover()
            .run()
            .expect_err("synthetic suite should fail")
            .to_string()
    }

    fn coverage_error(&self) -> String {
        self.discover()
            .audit_cli_coverage()
            .expect_err("synthetic suite coverage should be incomplete")
            .to_string()
    }
}
