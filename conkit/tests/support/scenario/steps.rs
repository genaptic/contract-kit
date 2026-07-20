use std::fs;

use serde::Deserialize;

use crate::support::ConkitCli;

use super::error::StepError;
use super::manifest::{Argument, ScenarioPath, WorkspacePath};
use super::sandbox::Sandbox;
use super::suite::Scenario;
use super::tree::{TreeContents, TreeSnapshot};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Step {
    Run(RunStep),
    Overlay(OverlayStep),
    Remove(RemoveStep),
    Capture(CaptureStep),
    AssertTree(AssertTreeStep),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RunStep {
    argv: Vec<Argument>,
    cwd: Option<WorkspacePath>,
    expect: ProcessExpectation,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessExpectation {
    exit_code: i32,
    stdout: StreamExpectation,
    stderr: StreamExpectation,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum StreamExpectation {
    Empty,
    Exact(String),
    ExactFile(ScenarioPath),
    ContainsInOrder(Vec<String>),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OverlayStep {
    source: ScenarioPath,
    destination: WorkspacePath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RemoveStep {
    path: WorkspacePath,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CaptureStep {
    name: String,
    directory: WorkspacePath,
    selector: CaptureSelector,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum CaptureSelector {
    OnlyFile,
    FileName(String),
    FileNameSuffix(String),
    UncapturedFileNameSuffix(String),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AssertTreeStep {
    actual: WorkspacePath,
    expected: ScenarioPath,
    contents: TreeContents,
}

impl Step {
    pub(super) fn is_run(&self) -> bool {
        matches!(self, Self::Run(_))
    }

    pub(super) fn is_assert_tree(&self) -> bool {
        matches!(self, Self::AssertTree(_))
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        match self {
            Self::Run(step) => step.validate(),
            Self::Overlay(step) => {
                step.source.validate()?;
                step.destination.validate()
            }
            Self::Remove(step) => step.path.validate(),
            Self::Capture(step) => step.validate(),
            Self::AssertTree(step) => {
                step.actual.validate()?;
                step.expected.validate()
            }
        }
    }

    pub(super) fn execute(
        &self,
        scenario: &Scenario,
        sandbox: &mut Sandbox,
    ) -> Result<(), StepError> {
        match self {
            Self::Run(step) => step.execute(scenario, sandbox),
            Self::Overlay(step) => step.execute(scenario, sandbox),
            Self::Remove(step) => step.execute(sandbox),
            Self::Capture(step) => step.execute(sandbox),
            Self::AssertTree(step) => step.execute(scenario, sandbox),
        }
    }
}

impl RunStep {
    fn validate(&self) -> Result<(), String> {
        let Some(executable) = self.argv.first() else {
            return Err("run argv must not be empty".to_owned());
        };
        if executable.as_str() != "conkit" {
            return Err("run argv[0] must be exactly \"conkit\"".to_owned());
        }
        let mut previous = None;
        for argument in &self.argv[1..] {
            argument.validate_for_option(previous)?;
            previous = Some(argument.as_str());
        }
        if let Some(cwd) = &self.cwd {
            cwd.validate()?;
        }
        self.expect.stdout.validate()?;
        self.expect.stderr.validate()
    }

    fn execute(&self, scenario: &Scenario, sandbox: &Sandbox) -> Result<(), StepError> {
        let cwd = match &self.cwd {
            Some(cwd) => sandbox.resolve_workspace_path(cwd)?,
            None => sandbox.work().to_path_buf(),
        };
        let mut command = ConkitCli::command();
        command.current_dir(cwd);
        for argument in &self.argv[1..] {
            command.arg(sandbox.resolve_argument(argument)?);
        }
        let output = command
            .output()
            .map_err(|source| StepError::Launch { source })?;
        let stdout = String::from_utf8(output.stdout).map_err(|error| StepError::StreamUtf8 {
            stream: "stdout",
            message: error.to_string(),
        })?;
        let stderr = String::from_utf8(output.stderr).map_err(|error| StepError::StreamUtf8 {
            stream: "stderr",
            message: error.to_string(),
        })?;
        let stdout = sandbox.normalize_stream(&stdout);
        let stderr = sandbox.normalize_stream(&stderr);
        let actual_code = output.status.code().ok_or(StepError::MissingExitCode)?;
        if actual_code != self.expect.exit_code {
            return Err(StepError::ExitCode {
                expected: self.expect.exit_code,
                actual: actual_code,
                stdout,
                stderr,
            });
        }
        self.expect
            .stdout
            .assert_matches(scenario, "stdout", &stdout)?;
        self.expect
            .stderr
            .assert_matches(scenario, "stderr", &stderr)
    }
}

impl StreamExpectation {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::ExactFile(path) => path.validate(),
            Self::ContainsInOrder(fragments) => {
                if fragments.is_empty() {
                    return Err("contains_in_order requires at least one value".to_owned());
                }
                if fragments.iter().any(String::is_empty) {
                    return Err("contains_in_order values must not be empty".to_owned());
                }
                Ok(())
            }
            Self::Empty | Self::Exact(_) => Ok(()),
        }
    }

    fn assert_matches(
        &self,
        scenario: &Scenario,
        stream: &'static str,
        actual: &str,
    ) -> Result<(), StepError> {
        match self {
            Self::Empty if actual.is_empty() => Ok(()),
            Self::Empty => Err(StepError::StreamMismatch {
                stream,
                message: "expected an empty stream".to_owned(),
                actual: actual.to_owned(),
            }),
            Self::Exact(expected) if expected == actual => Ok(()),
            Self::Exact(expected) => Err(StepError::StreamMismatch {
                stream,
                message: format!("expected exact contents {expected:?}"),
                actual: actual.to_owned(),
            }),
            Self::ExactFile(path) => {
                let expected_path = scenario.resolve_path(path)?;
                let expected =
                    fs::read_to_string(&expected_path).map_err(|source| StepError::Read {
                        path: expected_path,
                        source,
                    })?;
                let expected = expected.replace("\r\n", "\n");
                if expected == actual {
                    Ok(())
                } else {
                    Err(StepError::StreamMismatch {
                        stream,
                        message: format!(
                            "expected exact contents from scenario file {:?}",
                            path.as_str()
                        ),
                        actual: actual.to_owned(),
                    })
                }
            }
            Self::ContainsInOrder(fragments) => {
                let mut remaining = actual;
                for fragment in fragments {
                    let Some(index) = remaining.find(fragment) else {
                        return Err(StepError::StreamMismatch {
                            stream,
                            message: format!("missing ordered fragment {fragment:?}"),
                            actual: actual.to_owned(),
                        });
                    };
                    remaining = &remaining[index + fragment.len()..];
                }
                Ok(())
            }
        }
    }
}

impl OverlayStep {
    fn execute(&self, scenario: &Scenario, sandbox: &Sandbox) -> Result<(), StepError> {
        let source = scenario.resolve_path(&self.source)?;
        let destination = sandbox.resolve_workspace_path(&self.destination)?;
        sandbox.overlay(&source, &destination)
    }
}

impl RemoveStep {
    fn execute(&self, sandbox: &Sandbox) -> Result<(), StepError> {
        let path = sandbox.resolve_workspace_path(&self.path)?;
        if sandbox.is_root(&path) {
            return Err(StepError::RemoveRoot { path });
        }
        sandbox.validate_sandbox_path(&path)?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                StepError::MissingRemoveTarget { path: path.clone() }
            } else {
                StepError::Inspect {
                    path: path.clone(),
                    source: error,
                }
            }
        })?;
        if metadata.file_type().is_symlink() {
            return Err(StepError::UnsupportedEntry { path });
        }
        if metadata.is_dir() {
            fs::remove_dir_all(&path).map_err(|source| StepError::Write { path, source })
        } else if metadata.is_file() {
            fs::remove_file(&path).map_err(|source| StepError::Write { path, source })
        } else {
            Err(StepError::UnsupportedEntry { path })
        }
    }
}

impl CaptureStep {
    fn validate(&self) -> Result<(), String> {
        if self.name.is_empty()
            || !self.name.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            })
        {
            return Err(format!(
                "capture name {:?} must contain only ASCII letters, digits, '_' or '-'",
                self.name
            ));
        }
        self.directory.validate()?;
        self.selector.validate()
    }

    fn execute(&self, sandbox: &mut Sandbox) -> Result<(), StepError> {
        if sandbox.has_capture(&self.name) {
            return Err(StepError::DuplicateCapture {
                name: self.name.clone(),
            });
        }
        let directory = sandbox.resolve_workspace_path(&self.directory)?;
        let entries = fs::read_dir(&directory).map_err(|source| StepError::Read {
            path: directory.clone(),
            source,
        })?;
        let mut matches = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| StepError::Read {
                path: directory.clone(),
                source,
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|source| StepError::Inspect {
                path: path.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink()
                || (!metadata.file_type().is_file() && !metadata.file_type().is_dir())
            {
                return Err(StepError::UnsupportedEntry { path });
            }
            if !metadata.is_file() {
                continue;
            }
            let file_name =
                entry
                    .file_name()
                    .into_string()
                    .map_err(|name| StepError::InvalidPath {
                        value: name.to_string_lossy().into_owned(),
                        message: "capture candidate file name is not valid UTF-8".to_owned(),
                    })?;
            if self.selector.matches(&file_name, &path, sandbox) {
                matches.push(path);
            }
        }
        matches.sort();
        if matches.len() != 1 {
            return Err(StepError::CaptureMatchCount {
                directory,
                count: matches.len(),
            });
        }
        sandbox.bind_capture(self.name.clone(), matches.remove(0));
        Ok(())
    }
}

impl CaptureSelector {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::OnlyFile => Ok(()),
            Self::FileName(value)
            | Self::FileNameSuffix(value)
            | Self::UncapturedFileNameSuffix(value) => {
                if value.is_empty() {
                    Err("capture selector value must not be empty".to_owned())
                } else if value.contains('/') || value.contains('\\') {
                    Err("capture selector value must be a file name, not a path".to_owned())
                } else if value.contains(':') {
                    Err("capture selector value must be a portable file name".to_owned())
                } else if matches!(value.as_str(), "." | "..") {
                    Err("capture selector value must not be '.' or '..'".to_owned())
                } else {
                    Ok(())
                }
            }
        }
    }

    fn matches(&self, file_name: &str, path: &std::path::Path, sandbox: &Sandbox) -> bool {
        match self {
            Self::OnlyFile => true,
            Self::FileName(expected) => file_name == expected,
            Self::FileNameSuffix(suffix) => file_name.ends_with(suffix),
            Self::UncapturedFileNameSuffix(suffix) => {
                file_name.ends_with(suffix) && !sandbox.has_captured_path(path)
            }
        }
    }
}

impl AssertTreeStep {
    fn execute(&self, scenario: &Scenario, sandbox: &Sandbox) -> Result<(), StepError> {
        let actual_root = sandbox.resolve_workspace_path(&self.actual)?;
        let expected_root = scenario.resolve_path(&self.expected)?;
        let actual = TreeSnapshot::read(&actual_root)?;
        let expected = TreeSnapshot::read(&expected_root)?;
        actual.assert_matches(&expected, self.contents)
    }
}
