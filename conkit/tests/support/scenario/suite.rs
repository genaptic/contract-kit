use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_saphyr::{DuplicateKeyPolicy, MergeKeyPolicy};
use walkdir::WalkDir;

use super::REQUIRED_COVERAGE_KEYS;
use super::error::{HarnessError, StepError};
use super::manifest::{CoverageKey, Manifest, ScenarioPath};
use super::sandbox::Sandbox;

pub(crate) struct Suite {
    scenarios: Vec<Scenario>,
}

pub(crate) struct Scenario {
    id: String,
    directory: PathBuf,
    manifest: Manifest,
}

impl Suite {
    pub(crate) fn discover_workspace() -> Result<Self, HarnessError> {
        let manifest_directory = Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace =
            manifest_directory
                .parent()
                .ok_or_else(|| HarnessError::InvalidScenarioPath {
                    path: manifest_directory.to_path_buf(),
                    message: "CLI package has no workspace parent".to_owned(),
                })?;

        Self::discover(&workspace.join("test/scenarios"))
    }

    pub(crate) fn discover(root: &Path) -> Result<Self, HarnessError> {
        let mut manifests = Vec::new();
        let walker = WalkDir::new(root).follow_links(false).sort_by_file_name();

        for entry in walker {
            let entry = entry.map_err(|error| HarnessError::Walk {
                path: root.to_path_buf(),
                message: error.to_string(),
            })?;
            if entry.file_type().is_symlink() {
                continue;
            }
            if entry.file_type().is_file() && entry.file_name() == "scenario.yml" {
                manifests.push(entry.into_path());
            }
        }

        manifests.sort();
        if manifests.is_empty() {
            return Err(HarnessError::NoScenarios {
                root: root.to_path_buf(),
            });
        }

        let mut scenarios = Vec::new();
        let mut failures = Vec::new();
        for manifest in manifests {
            let directory = manifest.parent().expect("walked manifest has a parent");
            let relative_id = directory
                .strip_prefix(root)
                .expect("discovered scenario is below root")
                .to_string_lossy()
                .replace('\\', "/");
            let id = if relative_id.is_empty() {
                ".".to_owned()
            } else {
                relative_id
            };
            match Scenario::load_with_id(directory, id) {
                Ok(scenario) => scenarios.push(scenario),
                Err(error) => failures.push(error.to_string()),
            }
        }

        if failures.is_empty() {
            Ok(Self { scenarios })
        } else {
            Err(HarnessError::ScenarioFailures {
                details: failures.join("\n"),
            })
        }
    }

    pub(crate) fn scenario_ids(&self) -> Vec<&str> {
        self.scenarios
            .iter()
            .map(|scenario| scenario.id.as_str())
            .collect()
    }

    pub(crate) fn run(&self) -> Result<(), HarnessError> {
        let mut failures = Vec::new();
        for scenario in &self.scenarios {
            if let Err(error) = scenario.run() {
                failures.push(error.to_string());
            }
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(HarnessError::ScenarioFailures {
                details: failures.join("\n"),
            })
        }
    }

    pub(crate) fn audit_cli_coverage(&self) -> Result<(), HarnessError> {
        let mut covered = BTreeSet::new();
        for scenario in &self.scenarios {
            covered.extend(scenario.manifest.coverage().iter().copied());
        }

        let missing = REQUIRED_COVERAGE_KEYS
            .iter()
            .filter(|&&key| !covered.contains(&CoverageKey(key)))
            .copied()
            .collect::<Vec<_>>();

        if missing.is_empty() {
            Ok(())
        } else {
            Err(HarnessError::IncompleteCoverage {
                count: missing.len(),
                details: missing.join("\n"),
            })
        }
    }

    pub(crate) fn required_coverage_keys() -> &'static [&'static str] {
        REQUIRED_COVERAGE_KEYS
    }
}

impl Scenario {
    pub(crate) fn load(directory: &Path) -> Result<Self, HarnessError> {
        let id = directory
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| directory.display().to_string());
        Self::load_with_id(directory, id)
    }

    fn load_with_id(directory: &Path, id: String) -> Result<Self, HarnessError> {
        let directory_metadata =
            fs::symlink_metadata(directory).map_err(|source| HarnessError::Inspect {
                path: directory.to_path_buf(),
                source,
            })?;
        if directory_metadata.file_type().is_symlink() || !directory_metadata.is_dir() {
            return Err(HarnessError::InvalidScenarioPath {
                path: directory.to_path_buf(),
                message: "scenario root must be a regular directory".to_owned(),
            });
        }
        let manifest_path = directory.join("scenario.yml");
        let contents = fs::read(&manifest_path).map_err(|source| HarnessError::Read {
            path: manifest_path.clone(),
            source,
        })?;
        let options = serde_saphyr::options! {
            duplicate_keys: DuplicateKeyPolicy::Error,
            merge_keys: MergeKeyPolicy::Error,
            strict_booleans: true,
        };
        let manifest: Manifest = serde_saphyr::from_slice_with_options(&contents, options)
            .map_err(|source| HarnessError::Parse {
                path: manifest_path.clone(),
                source: Box::new(source),
            })?;
        let scenario = Self {
            id,
            directory: directory.to_path_buf(),
            manifest,
        };
        scenario.validate(&manifest_path)?;
        Ok(scenario)
    }

    pub(crate) fn sandbox(&self) -> Result<Sandbox, HarnessError> {
        Sandbox::new(self)
    }

    pub(crate) fn sandbox_in(&self, parent: &Path) -> Result<Sandbox, HarnessError> {
        Sandbox::new_in(self, parent)
    }

    fn validate(&self, manifest_path: &Path) -> Result<(), HarnessError> {
        if self.manifest.version() != 1 {
            return Err(HarnessError::InvalidManifest {
                path: manifest_path.to_path_buf(),
                message: format!(
                    "unsupported version {}; expected version 1",
                    self.manifest.version()
                ),
            });
        }
        if self.manifest.steps().is_empty() {
            return Err(HarnessError::InvalidManifest {
                path: manifest_path.to_path_buf(),
                message: "steps must not be empty".to_owned(),
            });
        }
        self.manifest
            .validate_coverage()
            .map_err(|message| HarnessError::InvalidManifest {
                path: manifest_path.to_path_buf(),
                message,
            })?;
        for (index, step) in self.manifest.steps().iter().enumerate() {
            step.validate()
                .map_err(|message| HarnessError::InvalidManifest {
                    path: manifest_path.to_path_buf(),
                    message: format!("step {}: {message}", index + 1),
                })?;
        }
        Ok(())
    }

    fn run(&self) -> Result<(), HarnessError> {
        let mut sandbox = Sandbox::new(self).map_err(|source| HarnessError::Scenario {
            scenario: self.id.clone(),
            source: Box::new(source),
        })?;
        let execution = sandbox.execute(self, self.manifest.steps());
        let cleanup = sandbox.close();

        let result = match (execution, cleanup) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(execution), Ok(())) => Err(execution),
            (Ok(()), Err(cleanup)) => Err(cleanup),
            (Err(execution), Err(cleanup)) => Err(HarnessError::ExecutionAndCleanup {
                execution: Box::new(execution),
                cleanup: Box::new(cleanup),
            }),
        };
        result.map_err(|source| HarnessError::Scenario {
            scenario: self.id.clone(),
            source: Box::new(source),
        })
    }

    pub(super) fn input_path(&self) -> PathBuf {
        self.directory.join("input")
    }

    pub(super) fn requires_cargo_workspace(&self) -> bool {
        self.manifest.requires_cargo_workspace()
    }

    pub(super) fn resolve_path(&self, path: &ScenarioPath) -> Result<PathBuf, StepError> {
        path.validate().map_err(|message| StepError::InvalidPath {
            value: path.as_str().to_owned(),
            message,
        })?;
        let mut resolved = self.directory.clone();
        for component in path.as_str().split('/') {
            resolved.push(component);
            let metadata =
                fs::symlink_metadata(&resolved).map_err(|source| StepError::Inspect {
                    path: resolved.clone(),
                    source,
                })?;
            if metadata.file_type().is_symlink() {
                return Err(StepError::UnsupportedEntry { path: resolved });
            }
        }
        Ok(resolved)
    }
}
