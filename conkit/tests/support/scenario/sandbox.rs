use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use assert_fs::TempDir;
use cargo_metadata::MetadataCommand;
use walkdir::WalkDir;

use super::error::{HarnessError, StepError};
use super::manifest::{Argument, WorkspacePath};
use super::steps::Step;
use super::suite::Scenario;

pub(crate) struct Sandbox {
    temp: Option<TempDir>,
    work: PathBuf,
    input: PathBuf,
    output: PathBuf,
    captures: BTreeMap<String, PathBuf>,
}

impl Sandbox {
    pub(super) fn new(scenario: &Scenario) -> Result<Self, HarnessError> {
        let temp = TempDir::new().map_err(|error| HarnessError::CreateSandbox {
            message: error.to_string(),
        })?;
        Self::initialize(scenario, temp)
    }

    pub(super) fn new_in(scenario: &Scenario, parent: &Path) -> Result<Self, HarnessError> {
        let temp = TempDir::new_in(parent).map_err(|error| HarnessError::CreateSandbox {
            message: error.to_string(),
        })?;
        Self::initialize(scenario, temp)
    }

    fn initialize(scenario: &Scenario, temp: TempDir) -> Result<Self, HarnessError> {
        let work = temp.path().to_path_buf();
        let input = work.join("input");
        let output = work.join("output");
        let mut sandbox = Self {
            temp: Some(temp),
            work,
            input,
            output,
            captures: BTreeMap::new(),
        };

        let setup = sandbox.prepare(scenario);
        if let Err(execution) = setup {
            return match sandbox.close() {
                Ok(()) => Err(execution),
                Err(cleanup) => Err(HarnessError::ExecutionAndCleanup {
                    execution: Box::new(execution),
                    cleanup: Box::new(cleanup),
                }),
            };
        }
        Ok(sandbox)
    }

    fn prepare(&mut self, scenario: &Scenario) -> Result<(), HarnessError> {
        fs::create_dir_all(&self.input).map_err(|source| HarnessError::Write {
            path: self.input.clone(),
            source,
        })?;
        fs::create_dir_all(&self.output).map_err(|source| HarnessError::Write {
            path: self.output.clone(),
            source,
        })?;

        let source = scenario.input_path();
        match fs::symlink_metadata(&source) {
            Ok(_) => self.copy_input(&source)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source_error) => {
                return Err(HarnessError::Inspect {
                    path: source,
                    source: source_error,
                });
            }
        }
        if scenario.requires_cargo_workspace() {
            self.validate_cargo_workspace()?;
        }
        Ok(())
    }

    fn copy_input(&self, source: &Path) -> Result<(), HarnessError> {
        let metadata =
            fs::symlink_metadata(source).map_err(|source_error| HarnessError::Inspect {
                path: source.to_path_buf(),
                source: source_error,
            })?;
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err(HarnessError::InvalidScenarioPath {
                path: source.to_path_buf(),
                message: "scenario input must be a regular directory".to_owned(),
            });
        }

        let walker = WalkDir::new(source)
            .follow_links(false)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|entry| {
                entry.depth() == 0 || !(entry.file_type().is_dir() && entry.file_name() == "target")
            });
        for entry in walker {
            let entry = entry.map_err(|error| HarnessError::Walk {
                path: source.to_path_buf(),
                message: error.to_string(),
            })?;
            let path = entry.path();
            let relative = path
                .strip_prefix(source)
                .expect("walked input entry is below the scenario input root");
            if relative.as_os_str().is_empty() {
                continue;
            }

            let file_type = entry.file_type();
            if file_type.is_symlink() || (!file_type.is_dir() && !file_type.is_file()) {
                return Err(HarnessError::InvalidScenarioPath {
                    path: path.to_path_buf(),
                    message: "scenario input contains a symlink or special file".to_owned(),
                });
            }
            let skipped_file =
                entry.file_name() == "Cargo.lock" || entry.file_name() == ".DS_Store";
            if skipped_file {
                continue;
            }

            let destination = self.input.join(relative);
            if file_type.is_dir() {
                fs::create_dir_all(&destination).map_err(|source_error| HarnessError::Write {
                    path: destination,
                    source: source_error,
                })?;
            } else {
                if let Some(parent) = destination.parent() {
                    fs::create_dir_all(parent).map_err(|source_error| HarnessError::Write {
                        path: parent.to_path_buf(),
                        source: source_error,
                    })?;
                }
                fs::copy(path, &destination).map_err(|source_error| HarnessError::Write {
                    path: destination,
                    source: source_error,
                })?;
            }
        }
        Ok(())
    }

    fn validate_cargo_workspace(&self) -> Result<(), HarnessError> {
        let manifest = self.input.join("Cargo.toml");
        let metadata = MetadataCommand::new()
            .current_dir(&self.input)
            .manifest_path(&manifest)
            .no_deps()
            .exec()
            .map_err(|error| HarnessError::CargoMetadata {
                manifest: manifest.clone(),
                message: error.to_string(),
            })?;
        let expected = self
            .input
            .canonicalize()
            .map_err(|source| HarnessError::Inspect {
                path: self.input.clone(),
                source,
            })?;
        let metadata_root = metadata.workspace_root.into_std_path_buf();
        let actual = metadata_root
            .canonicalize()
            .map_err(|source| HarnessError::Inspect {
                path: metadata_root,
                source,
            })?;
        if actual != expected {
            return Err(HarnessError::CargoWorkspaceRoot {
                manifest,
                expected,
                actual,
            });
        }
        Ok(())
    }

    pub(super) fn execute(
        &mut self,
        scenario: &Scenario,
        steps: &[Step],
    ) -> Result<(), HarnessError> {
        for (index, step) in steps.iter().enumerate() {
            step.execute(scenario, self)
                .map_err(|source| HarnessError::Step {
                    step: index + 1,
                    source,
                })?;
        }
        Ok(())
    }

    pub(crate) fn work(&self) -> &Path {
        &self.work
    }

    pub(crate) fn input(&self) -> &Path {
        &self.input
    }

    pub(crate) fn output(&self) -> &Path {
        &self.output
    }

    pub(crate) fn close(mut self) -> Result<(), HarnessError> {
        let Some(temp) = self.temp.take() else {
            return Ok(());
        };
        temp.close().map_err(|error| HarnessError::Cleanup {
            message: error.to_string(),
        })
    }

    pub(crate) fn resolve_argument_for_test(&self, value: &str) -> Result<OsString, StepError> {
        let argument = Argument::new(value.to_owned());
        argument
            .validate()
            .map_err(|message| StepError::InvalidArgument {
                value: value.to_owned(),
                message,
            })?;
        self.resolve_argument(&argument)
    }

    pub(super) fn resolve_argument(&self, argument: &Argument) -> Result<OsString, StepError> {
        let value = argument.as_str();
        if value.starts_with("${capture.") && value.ends_with('}') {
            let name = &value[10..value.len() - 1];
            let captured = self
                .captures
                .get(name)
                .ok_or_else(|| StepError::MissingCapture {
                    name: name.to_owned(),
                })?;
            self.validate_sandbox_path(captured)?;
            return Ok(captured.as_os_str().to_owned());
        }
        if value.starts_with('/') {
            return self
                .resolve_workspace_path(&WorkspacePath::new(value.to_owned()))
                .map(|path| path.into_os_string());
        }
        Ok(OsString::from(value))
    }

    pub(super) fn resolve_workspace_path(
        &self,
        path: &WorkspacePath,
    ) -> Result<PathBuf, StepError> {
        path.validate().map_err(|message| StepError::InvalidPath {
            value: path.as_str().to_owned(),
            message,
        })?;
        let value = path.as_str();
        let resolved = if value == "/work" {
            self.work.clone()
        } else if value == "/input" {
            self.input.clone()
        } else if value == "/output" {
            self.output.clone()
        } else if let Some(suffix) = value.strip_prefix("/work/") {
            self.work.join(suffix)
        } else if let Some(suffix) = value.strip_prefix("/input/") {
            self.input.join(suffix)
        } else if let Some(suffix) = value.strip_prefix("/output/") {
            self.output.join(suffix)
        } else {
            return Err(StepError::InvalidPath {
                value: value.to_owned(),
                message: "unknown sandbox root".to_owned(),
            });
        };
        self.validate_sandbox_path(&resolved)?;
        Ok(resolved)
    }

    pub(crate) fn normalize_stream(&self, stream: &str) -> String {
        let mut normalized = stream.replace("\r\n", "\n");
        normalized = self.normalize_sandbox_paths(&normalized);
        normalized
    }

    fn normalize_sandbox_paths(&self, stream: &str) -> String {
        let mut normalized = stream.to_owned();
        let mut roots = Vec::new();
        for (path, replacement) in [
            (&self.input, "/input"),
            (&self.output, "/output"),
            (&self.work, "/work"),
        ] {
            roots.push((path.clone(), replacement));
            if let Ok(canonical) = path.canonicalize()
                && canonical != *path
            {
                roots.push((canonical, replacement));
            }
        }

        for (path, replacement) in roots {
            let native = path.to_string_lossy();
            let slash = native.replace('\\', "/");
            for (index, candidate) in [native.as_ref(), slash.as_str()].into_iter().enumerate() {
                if index == 1 && candidate == native.as_ref() {
                    continue;
                }

                let mut cursor = 0;
                while let Some(relative_start) = normalized[cursor..].find(candidate) {
                    let start = cursor + relative_start;
                    let root_end = start + candidate.len();
                    let left = normalized[..start].chars().next_back();
                    let left_is_boundary = left.is_none_or(|character| {
                        character.is_whitespace()
                            || matches!(
                                character,
                                ':' | '=' | '(' | '[' | '{' | '<' | ',' | ';' | '\'' | '"' | '`'
                            )
                    });
                    let right = normalized[root_end..].chars().next();
                    let right_is_boundary = right.is_none_or(|character| {
                        character.is_whitespace()
                            || matches!(
                                character,
                                '/' | '\\'
                                    | ':'
                                    | ')'
                                    | ']'
                                    | '}'
                                    | '>'
                                    | ','
                                    | ';'
                                    | '\''
                                    | '"'
                                    | '`'
                            )
                    });
                    if !left_is_boundary || !right_is_boundary {
                        cursor = root_end;
                        continue;
                    }

                    let quoted = matches!(left, Some('\'' | '"' | '`'));
                    let suffix_end = if matches!(right, Some('/' | '\\')) {
                        normalized[root_end..]
                            .char_indices()
                            .find_map(|(index, character)| {
                                let is_delimiter =
                                    matches!(character, '\n' | '\r' | ':' | '"' | '\'' | '`')
                                        || (!quoted && character.is_whitespace());
                                is_delimiter.then_some(root_end + index)
                            })
                            .unwrap_or(normalized.len())
                    } else {
                        root_end
                    };
                    let suffix = normalized[root_end..suffix_end].replace('\\', "/");
                    let path = format!("{replacement}{suffix}");
                    normalized.replace_range(start..suffix_end, &path);
                    cursor = start + path.len();
                }
            }
        }
        normalized
    }

    pub(crate) fn validate_sandbox_path(&self, path: &Path) -> Result<(), StepError> {
        let relative = path
            .strip_prefix(&self.work)
            .map_err(|_| StepError::InvalidPath {
                value: path.display().to_string(),
                message: "path is outside the sandbox".to_owned(),
            })?;
        let mut current = self.work.clone();
        for component in relative.components() {
            current.push(component);
            match fs::symlink_metadata(&current) {
                Ok(metadata)
                    if metadata.file_type().is_symlink()
                        || (!metadata.file_type().is_file() && !metadata.file_type().is_dir()) =>
                {
                    return Err(StepError::UnsupportedEntry { path: current });
                }
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) =>
                {
                    break;
                }
                Err(source) => {
                    return Err(StepError::Inspect {
                        path: current,
                        source,
                    });
                }
            }
        }
        Ok(())
    }

    pub(super) fn is_root(&self, path: &Path) -> bool {
        path == self.work || path == self.input || path == self.output
    }

    pub(super) fn has_capture(&self, name: &str) -> bool {
        self.captures.contains_key(name)
    }

    pub(super) fn has_captured_path(&self, path: &Path) -> bool {
        self.captures.values().any(|captured| captured == path)
    }

    pub(super) fn bind_capture(&mut self, name: String, path: PathBuf) {
        self.captures.insert(name, path);
    }

    pub(super) fn overlay(&self, source: &Path, destination: &Path) -> Result<(), StepError> {
        self.validate_sandbox_path(destination)?;
        let metadata = fs::symlink_metadata(source).map_err(|source_error| StepError::Inspect {
            path: source.to_path_buf(),
            source: source_error,
        })?;
        if metadata.file_type().is_symlink()
            || (!metadata.file_type().is_dir() && !metadata.file_type().is_file())
        {
            return Err(StepError::UnsupportedEntry {
                path: source.to_path_buf(),
            });
        }
        if metadata.is_file() {
            return self.overlay_file(source, destination);
        }

        match fs::symlink_metadata(destination) {
            Ok(destination_metadata)
                if destination_metadata.file_type().is_symlink()
                    || !destination_metadata.is_dir() =>
            {
                return Err(StepError::OverlayTypeConflict {
                    source_path: source.to_path_buf(),
                    destination: destination.to_path_buf(),
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir_all(destination).map_err(|source_error| StepError::Write {
                    path: destination.to_path_buf(),
                    source: source_error,
                })?;
            }
            Err(source_error) => {
                return Err(StepError::Inspect {
                    path: destination.to_path_buf(),
                    source: source_error,
                });
            }
        }

        let walker = WalkDir::new(source).follow_links(false).sort_by_file_name();
        for entry in walker {
            let entry = entry.map_err(|error| StepError::Walk {
                path: source.to_path_buf(),
                message: error.to_string(),
            })?;
            let path = entry.path();
            let relative = path
                .strip_prefix(source)
                .expect("walked overlay entry is below overlay root");
            if relative.as_os_str().is_empty() {
                continue;
            }
            let target = destination.join(relative);
            let file_type = entry.file_type();
            if file_type.is_symlink() || (!file_type.is_dir() && !file_type.is_file()) {
                return Err(StepError::UnsupportedEntry {
                    path: path.to_path_buf(),
                });
            }
            if file_type.is_dir() {
                self.validate_sandbox_path(&target)?;
                match fs::symlink_metadata(&target) {
                    Ok(target_metadata) if !target_metadata.is_dir() => {
                        return Err(StepError::OverlayTypeConflict {
                            source_path: path.to_path_buf(),
                            destination: target,
                        });
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        fs::create_dir_all(&target).map_err(|source_error| StepError::Write {
                            path: target,
                            source: source_error,
                        })?;
                    }
                    Err(source_error) => {
                        return Err(StepError::Inspect {
                            path: target,
                            source: source_error,
                        });
                    }
                }
            } else {
                self.overlay_file(path, &target)?;
            }
        }
        Ok(())
    }

    fn overlay_file(&self, source: &Path, destination: &Path) -> Result<(), StepError> {
        self.validate_sandbox_path(destination)?;
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|source_error| StepError::Write {
                path: parent.to_path_buf(),
                source: source_error,
            })?;
        }
        match fs::symlink_metadata(destination) {
            Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
                return Err(StepError::OverlayTypeConflict {
                    source_path: source.to_path_buf(),
                    destination: destination.to_path_buf(),
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source_error) => {
                return Err(StepError::Inspect {
                    path: destination.to_path_buf(),
                    source: source_error,
                });
            }
        }
        fs::copy(source, destination).map_err(|source_error| StepError::Write {
            path: destination.to_path_buf(),
            source: source_error,
        })?;
        Ok(())
    }
}
