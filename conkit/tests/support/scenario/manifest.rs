use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Deserializer};

use super::REQUIRED_COVERAGE_KEYS;
use super::steps::Step;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct CoverageKey(pub(super) &'static str);

impl CoverageKey {
    fn parse(value: &str) -> Option<Self> {
        REQUIRED_COVERAGE_KEYS
            .binary_search(&value)
            .ok()
            .map(|index| Self(REQUIRED_COVERAGE_KEYS[index]))
    }

    pub(super) fn as_str(self) -> &'static str {
        self.0
    }
}

impl<'de> Deserialize<'de> for CoverageKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown coverage key {value:?}")))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    version: u32,
    #[serde(default)]
    cargo_workspace: bool,
    #[serde(default)]
    coverage: Vec<CoverageKey>,
    steps: Vec<Step>,
}

impl Manifest {
    pub(super) fn version(&self) -> u32 {
        self.version
    }

    pub(super) fn requires_cargo_workspace(&self) -> bool {
        self.cargo_workspace
    }

    pub(super) fn coverage(&self) -> &[CoverageKey] {
        &self.coverage
    }

    pub(super) fn steps(&self) -> &[Step] {
        &self.steps
    }

    pub(super) fn validate_coverage(&self) -> Result<(), String> {
        let mut keys = BTreeSet::new();
        for key in &self.coverage {
            if !keys.insert(*key) {
                return Err(format!("duplicate coverage key {:?}", key.as_str()));
            }
        }

        if !self.coverage.is_empty() && !self.steps.iter().any(Step::is_run) {
            return Err("coverage is declared but steps contain no run step".to_owned());
        }

        if self
            .coverage
            .iter()
            .any(|key| key.as_str().starts_with("behavior."))
            && !self.steps.iter().any(Step::is_assert_tree)
        {
            return Err(
                "behavior coverage is declared but steps contain no assert_tree step".to_owned(),
            );
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(transparent)]
pub(super) struct ScenarioPath(String);

impl ScenarioPath {
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        if self.0.is_empty() {
            return Err("path must not be empty".to_owned());
        }
        if self.0.contains('\\') {
            return Err("backslashes are not allowed".to_owned());
        }
        let bytes = self.0.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && bytes[2] == b'/'
        {
            return Err("host-absolute paths are not allowed".to_owned());
        }
        if Path::new(&self.0).is_absolute() || self.0.starts_with('/') {
            return Err("path must be scenario-relative".to_owned());
        }
        if self.0.split('/').any(|component| component.contains(':')) {
            return Err("':' is not allowed in portable path components".to_owned());
        }
        if self
            .0
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
        {
            return Err("empty, '.' and '..' path components are not allowed".to_owned());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(transparent)]
pub(super) struct WorkspacePath(String);

impl WorkspacePath {
    pub(super) fn new(value: String) -> Self {
        Self(value)
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        if self.0.contains('\\') {
            return Err("backslashes are not allowed".to_owned());
        }
        if matches!(self.0.as_str(), "/work" | "/input" | "/output") {
            return Ok(());
        }
        let suffix = if let Some(suffix) = self.0.strip_prefix("/work/") {
            suffix
        } else if let Some(suffix) = self.0.strip_prefix("/input/") {
            suffix
        } else if let Some(suffix) = self.0.strip_prefix("/output/") {
            suffix
        } else {
            return Err("path must begin with exactly /work, /input, or /output".to_owned());
        };
        let suffix_bytes = suffix.as_bytes();
        if suffix_bytes.len() >= 3
            && suffix_bytes[0].is_ascii_alphabetic()
            && suffix_bytes[1] == b':'
            && suffix_bytes[2] == b'/'
        {
            return Err("host-absolute suffixes are not allowed".to_owned());
        }
        if suffix.split('/').any(|component| component.contains(':')) {
            return Err("':' is not allowed in portable path components".to_owned());
        }
        if suffix
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
        {
            return Err("empty, '.' and '..' path components are not allowed".to_owned());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(transparent)]
pub(super) struct Argument(String);

impl Argument {
    pub(super) fn new(value: String) -> Self {
        Self(value)
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        if self.0.contains('\\') {
            return Err("backslashes are not allowed".to_owned());
        }
        if self.0.starts_with("${capture.") && self.0.ends_with('}') {
            let name = &self.0[10..self.0.len() - 1];
            if name.is_empty()
                || !name.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
                })
            {
                return Err("capture placeholder contains an invalid name".to_owned());
            }
            return Ok(());
        }
        if self.0.contains("${") {
            return Err("unknown or partial placeholder".to_owned());
        }
        if self.0.starts_with('/') {
            return WorkspacePath::new(self.0.clone()).validate();
        }
        let path_value = self
            .0
            .split_once('=')
            .map_or(self.0.as_str(), |(_, value)| value);
        let bytes = path_value.as_bytes();
        if Path::new(path_value).is_absolute()
            || path_value.starts_with('/')
            || (bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && bytes[2] == b'/')
        {
            return Err("host-absolute paths are not allowed".to_owned());
        }
        if path_value.contains(':') {
            return Err("':' is not allowed in portable path arguments".to_owned());
        }
        if path_value
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
        {
            return Err("empty, '.' and '..' path components are not allowed".to_owned());
        }
        Ok(())
    }

    pub(super) fn validate_for_option(&self, option: Option<&str>) -> Result<(), String> {
        if option != Some("--crate-root") {
            return self.validate();
        }

        if self.0.contains('\\') {
            return Err("backslashes are not allowed".to_owned());
        }
        let Some((crate_id, root)) = self.0.split_once('=') else {
            return Err("crate-root argument must contain '='".to_owned());
        };
        if crate_id.is_empty() {
            return Err("crate-root ID must not be empty".to_owned());
        }
        let Some((kind, path)) = root.split_once(':') else {
            return Err("crate-root argument must contain a target kind and path".to_owned());
        };
        if !matches!(kind, "library" | "binary") {
            return Err("crate-root target kind must be 'library' or 'binary'".to_owned());
        }

        ScenarioPath(path.to_owned()).validate()
    }
}
