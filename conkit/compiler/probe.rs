//! Private one-shot Cargo-owned rustdoc probe protocol.

use std::ffi::{OsStr, OsString};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitCode};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::error::CompilerError;
use super::extractor::CompilerConfiguration;
use super::limits::CompilerUsage;
use super::process::CompilerOperation;
use super::project::{CargoProject, CompilerWorkspace};

pub(super) const RUSTDOC_PROBE_PATH_ENV: &str = "CONKIT_INTERNAL_RUSTDOC_PROBE_PATH";
pub(super) const RUSTDOC_PROBE_TOKEN_ENV: &str = "CONKIT_INTERNAL_RUSTDOC_PROBE_TOKEN";
pub(super) const RUSTDOC_PROBE_CRATE_ENV: &str = "CONKIT_INTERNAL_RUSTDOC_PROBE_CRATE";
pub(super) const RUSTDOC_PROBE_SOURCE_ENV: &str = "CONKIT_INTERNAL_RUSTDOC_PROBE_SOURCE";
pub(super) const RUSTDOC_PROBE_TARGET_DIR_ENV: &str = "CONKIT_INTERNAL_RUSTDOC_PROBE_TARGET_DIR";
pub(super) const RUSTDOC_PROBE_CRATE_TYPES_ENV: &str = "CONKIT_INTERNAL_RUSTDOC_PROBE_CRATE_TYPES";
pub(super) const RUSTDOC_PROBE_EXIT_CODE: u8 = 73;
pub(super) const RUSTDOC_PROBE_MAX_ARGUMENT_COUNT: usize = 4_096;
pub(super) const RUSTDOC_PROBE_MAX_ARGUMENT_BYTES: u64 = 64 * 1024;
pub(super) const RUSTDOC_PROBE_MAX_CAPTURE_BYTES: u64 = 64 * 1024;
/// Private process entry used only when Cargo launches this executable as a
/// one-shot rustdoc argument probe.
pub(crate) struct RustdocProbe {
    state: ProbeState,
    capture_path: PathBuf,
    token: String,
    crate_name: String,
    source_path: PathBuf,
    target_directory: PathBuf,
    crate_types: Vec<String>,
}

impl RustdocProbe {
    /// Captures one token-bound rustdoc invocation when the private probe
    /// environment is complete. Normal CLI invocations return `None`.
    pub(crate) fn run_if_requested() -> Option<ExitCode> {
        match Self::from_environment() {
            Ok(None) => None,
            Ok(Some(probe)) => Some(match probe.capture(std::env::args_os().skip(1)) {
                Ok(()) => ExitCode::from(RUSTDOC_PROBE_EXIT_CODE),
                Err(_) => ExitCode::FAILURE,
            }),
            Err(_) => Some(ExitCode::FAILURE),
        }
    }

    fn from_environment() -> Result<Option<Self>, RustdocProbeError> {
        let Some(capture_path) = std::env::var_os(RUSTDOC_PROBE_PATH_ENV) else {
            return Ok(None);
        };
        let token = std::env::var(RUSTDOC_PROBE_TOKEN_ENV)
            .map_err(|_| RustdocProbeError::IncompleteEnvironment)?;
        let crate_name = std::env::var(RUSTDOC_PROBE_CRATE_ENV)
            .map_err(|_| RustdocProbeError::IncompleteEnvironment)?;
        let source_path = std::env::var_os(RUSTDOC_PROBE_SOURCE_ENV)
            .map(PathBuf::from)
            .ok_or(RustdocProbeError::IncompleteEnvironment)?;
        let target_directory = std::env::var_os(RUSTDOC_PROBE_TARGET_DIR_ENV)
            .map(PathBuf::from)
            .ok_or(RustdocProbeError::IncompleteEnvironment)?;
        let crate_types = std::env::var(RUSTDOC_PROBE_CRATE_TYPES_ENV)
            .map_err(|_| RustdocProbeError::IncompleteEnvironment)?
            .split(',')
            .map(str::to_owned)
            .collect();

        Self::child(
            PathBuf::from(capture_path),
            token,
            crate_name,
            source_path,
            target_directory,
            crate_types,
        )
        .map(Some)
    }

    pub(super) fn child(
        capture_path: PathBuf,
        token: String,
        crate_name: String,
        source_path: PathBuf,
        target_directory: PathBuf,
        mut crate_types: Vec<String>,
    ) -> Result<Self, RustdocProbeError> {
        if token.len() != 64
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(RustdocProbeError::InvalidToken);
        }
        if crate_name.is_empty()
            || !crate_name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(RustdocProbeError::InvalidCrateName);
        }
        crate_types.sort();
        crate_types.dedup();
        if crate_types.is_empty()
            || crate_types.iter().any(|crate_type| {
                !matches!(
                    crate_type.as_str(),
                    "bin" | "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro"
                )
            })
        {
            return Err(RustdocProbeError::InvalidCrateType);
        }
        if !source_path.is_absolute() || !target_directory.is_absolute() {
            return Err(RustdocProbeError::RelativeBoundaryPath);
        }
        let expected_name = format!(".conkit-rustdoc-probe-{token}.json");
        if capture_path.file_name() != Some(OsStr::new(&expected_name))
            || capture_path.parent() != Some(target_directory.as_path())
            || target_directory
                .file_name()
                .and_then(OsStr::to_str)
                .is_none_or(|name| !name.starts_with("conkit-rustdoc-"))
            || capture_path.exists()
        {
            return Err(RustdocProbeError::InvalidCapturePath);
        }

        Ok(Self {
            state: ProbeState::Child,
            capture_path,
            token,
            crate_name,
            source_path,
            target_directory,
            crate_types,
        })
    }

    pub(super) fn parent(
        workspace: &CompilerWorkspace,
        project: &CargoProject,
    ) -> Result<Self, CompilerError> {
        let seed = tempfile::Builder::new()
            .prefix(".conkit-rustdoc-probe-seed-")
            .rand_bytes(32)
            .tempfile_in(workspace.target_directory())
            .map_err(CompilerError::RustdocProbeWorkspace)?;
        let seed_path = seed.path().to_path_buf();
        seed.close().map_err(CompilerError::RustdocProbeWorkspace)?;
        let mut hasher = Sha256::new();
        hasher.update(seed_path.as_os_str().as_encoded_bytes());
        let token = format!("{:x}", hasher.finalize());
        let capture_path = workspace
            .target_directory()
            .join(format!(".conkit-rustdoc-probe-{token}.json"));
        let executable = std::env::current_exe().map_err(CompilerError::CurrentExecutable)?;
        let source_path = fs_err::canonicalize(project.target_root()).map_err(|source| {
            CompilerError::CargoTargetUnavailable {
                path: project.target_root().to_path_buf(),
                source,
            }
        })?;

        Ok(Self {
            state: ProbeState::Parent { executable },
            capture_path,
            token,
            crate_name: project.target_name().replace('-', "_"),
            source_path,
            target_directory: workspace.target_directory().to_path_buf(),
            crate_types: project.crate_types().to_vec(),
        })
    }

    fn capture(&self, arguments: impl Iterator<Item = OsString>) -> Result<(), RustdocProbeError> {
        if !matches!(&self.state, ProbeState::Child) {
            unreachable!("only a child rustdoc probe captures arguments");
        }
        let record = match self.record(arguments) {
            Ok(record) => record,
            Err(source) => ProbeRecord {
                token: self.token.clone(),
                exit_code: RUSTDOC_PROBE_EXIT_CODE,
                cfg_values: Vec::new(),
                rejection: Some(source.to_string()),
            },
        };
        let bytes = serde_json::to_vec(&record)
            .map_err(|source| RustdocProbeError::Encode(source.to_string()))?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > RUSTDOC_PROBE_MAX_CAPTURE_BYTES {
            return Err(RustdocProbeError::CaptureLimit);
        }
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&self.capture_path)
            .map_err(RustdocProbeError::Write)?;
        file.write_all(&bytes).map_err(RustdocProbeError::Write)?;
        file.sync_all().map_err(RustdocProbeError::Write)
    }

    fn record(
        &self,
        incoming: impl Iterator<Item = OsString>,
    ) -> Result<ProbeRecord, RustdocProbeError> {
        let mut arguments = Vec::new();
        let mut argument_bytes = 0_u64;
        for argument in incoming {
            if arguments.len() >= RUSTDOC_PROBE_MAX_ARGUMENT_COUNT {
                return Err(RustdocProbeError::ArgumentLimit);
            }
            argument_bytes = argument_bytes
                .checked_add(u64::try_from(argument.as_encoded_bytes().len()).unwrap_or(u64::MAX))
                .ok_or(RustdocProbeError::ArgumentLimit)?;
            if argument_bytes > RUSTDOC_PROBE_MAX_ARGUMENT_BYTES {
                return Err(RustdocProbeError::ArgumentLimit);
            }
            arguments.push(argument);
        }

        let mut cfg_values = Vec::new();
        let mut crate_names = Vec::new();
        let mut crate_types = Vec::new();
        let mut output_directories = Vec::new();
        let mut source_matches = 0_usize;
        let mut unstable_options = 0_usize;
        let mut json_outputs = 0_usize;
        let mut document_hidden_items = 0_usize;
        let mut document_private_items = 0_usize;

        for (index, argument) in arguments.iter().enumerate() {
            let argument_path = Path::new(argument);
            if argument_path.extension() == Some(OsStr::new("rs"))
                && fs_err::canonicalize(argument_path).is_ok_and(|path| path == self.source_path)
            {
                source_matches += 1;
            }
            let Some(text) = argument.to_str() else {
                continue;
            };
            match text {
                "--cfg" => cfg_values.push(Self::utf8_value(&arguments, index, "--cfg")?),
                "--crate-name" => {
                    crate_names.push(Self::utf8_value(&arguments, index, "--crate-name")?);
                }
                "--crate-type" => {
                    crate_types.push(Self::utf8_value(&arguments, index, "--crate-type")?);
                }
                "-o" => {
                    output_directories.push(PathBuf::from(Self::os_value(&arguments, index, "-o")?))
                }
                "--out-dir" => output_directories.push(PathBuf::from(Self::os_value(
                    &arguments,
                    index,
                    "--out-dir",
                )?)),
                "-Z" => {
                    if Self::utf8_value(&arguments, index, "-Z")? == "unstable-options" {
                        unstable_options += 1;
                    }
                }
                "--output-format" => {
                    if Self::utf8_value(&arguments, index, "--output-format")? == "json" {
                        json_outputs += 1;
                    }
                }
                "--document-hidden-items" => document_hidden_items += 1,
                "--document-private-items" => document_private_items += 1,
                _ => {
                    if let Some(value) = text.strip_prefix("--cfg=") {
                        cfg_values.push(Self::validated_cfg(value)?);
                    } else if let Some(value) = text.strip_prefix("--crate-name=") {
                        crate_names.push(value.to_owned());
                    } else if let Some(value) = text.strip_prefix("--crate-type=") {
                        crate_types.push(value.to_owned());
                    } else if let Some(value) = text.strip_prefix("--out-dir=") {
                        output_directories.push(PathBuf::from(value));
                    } else if text == "-Zunstable-options" {
                        unstable_options += 1;
                    } else if text == "--output-format=json" {
                        json_outputs += 1;
                    }
                }
            }
        }

        let expected_private_items = usize::from(
            crate_types
                .first()
                .is_some_and(|crate_type| crate_type == "bin"),
        );
        if crate_names.first().map(String::as_str) != Some(self.crate_name.as_str())
            || crate_names.len() != 1
            || crate_types.len() != 1
            || !self.crate_types.contains(&crate_types[0])
            || source_matches != 1
            || output_directories.len() != 1
            || !output_directories[0].is_absolute()
            || output_directories[0]
                .components()
                .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
            || !output_directories[0].starts_with(&self.target_directory)
            || unstable_options != 1
            || json_outputs != 1
            || document_hidden_items != 1
            || document_private_items != expected_private_items
        {
            return Err(RustdocProbeError::UnexpectedInvocation);
        }
        for cfg in &mut cfg_values {
            *cfg = Self::validated_cfg(cfg)?;
        }
        cfg_values.sort();
        cfg_values.dedup();

        Ok(ProbeRecord {
            token: self.token.clone(),
            exit_code: RUSTDOC_PROBE_EXIT_CODE,
            cfg_values,
            rejection: None,
        })
    }

    pub(super) fn apply(&self, command: &mut Command) {
        let ProbeState::Parent { executable } = &self.state else {
            unreachable!("only a parent rustdoc probe configures Cargo");
        };
        command
            .env("RUSTDOC", executable)
            .env(RUSTDOC_PROBE_PATH_ENV, &self.capture_path)
            .env(RUSTDOC_PROBE_TOKEN_ENV, &self.token)
            .env(RUSTDOC_PROBE_CRATE_ENV, &self.crate_name)
            .env(RUSTDOC_PROBE_SOURCE_ENV, &self.source_path)
            .env(RUSTDOC_PROBE_TARGET_DIR_ENV, &self.target_directory)
            .env(RUSTDOC_PROBE_CRATE_TYPES_ENV, self.crate_types.join(","));
    }

    pub(super) fn read_record(&self, usage: &CompilerUsage) -> Result<Vec<String>, CompilerError> {
        if !matches!(&self.state, ProbeState::Parent { .. }) {
            unreachable!("only a parent rustdoc probe reads the capture");
        }
        let bytes = usage.read_artifact(
            CompilerOperation::RustdocConfigurationProbe,
            &self.capture_path,
            RUSTDOC_PROBE_MAX_CAPTURE_BYTES,
        )?;
        let record: ProbeRecord = serde_json::from_slice(&bytes).map_err(|source| {
            CompilerError::InvalidRustdocProbeCapture {
                message: source.to_string(),
            }
        })?;
        if record.token != self.token || record.exit_code != RUSTDOC_PROBE_EXIT_CODE {
            return Err(CompilerError::InvalidRustdocProbeCapture {
                message: "probe token or recognized exit code did not match the active session"
                    .to_owned(),
            });
        }
        if let Some(rejection) = record.rejection {
            return Err(CompilerError::InvalidRustdocProbeCapture { message: rejection });
        }
        Ok(record.cfg_values)
    }

    fn utf8_value(
        arguments: &[OsString],
        index: usize,
        option: &'static str,
    ) -> Result<String, RustdocProbeError> {
        Self::os_value(arguments, index, option)?
            .to_str()
            .map(str::to_owned)
            .ok_or(RustdocProbeError::NonUtf8Option { option })
    }

    fn os_value<'arguments>(
        arguments: &'arguments [OsString],
        index: usize,
        option: &'static str,
    ) -> Result<&'arguments OsStr, RustdocProbeError> {
        arguments
            .get(index.saturating_add(1))
            .map(OsString::as_os_str)
            .ok_or(RustdocProbeError::MissingOptionValue { option })
    }

    fn validated_cfg(value: &str) -> Result<String, RustdocProbeError> {
        CompilerConfiguration::validated_value(value)
            .map_err(|_| RustdocProbeError::InvalidCfgValue)
    }
}

pub(super) enum ProbeState {
    Child,
    Parent { executable: PathBuf },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProbeRecord {
    token: String,
    exit_code: u8,
    cfg_values: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rejection: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum RustdocProbeError {
    #[error("private rustdoc probe environment is incomplete")]
    IncompleteEnvironment,
    #[error("private rustdoc probe token is invalid")]
    InvalidToken,
    #[error("private rustdoc probe crate name is invalid")]
    InvalidCrateName,
    #[error("private rustdoc probe crate type is invalid")]
    InvalidCrateType,
    #[error("private rustdoc probe boundary paths must be absolute")]
    RelativeBoundaryPath,
    #[error("private rustdoc probe capture path is invalid")]
    InvalidCapturePath,
    #[error("private rustdoc probe arguments exceeded their byte limit")]
    ArgumentLimit,
    #[error("private rustdoc probe capture exceeded its byte limit")]
    CaptureLimit,
    #[error("private rustdoc probe option {option} is missing its value")]
    MissingOptionValue { option: &'static str },
    #[error("private rustdoc probe option {option} is not UTF-8")]
    NonUtf8Option { option: &'static str },
    #[error("private rustdoc probe cfg value is invalid")]
    InvalidCfgValue,
    #[error("Cargo produced an unexpected rustdoc invocation")]
    UnexpectedInvocation,
    #[error("failed to encode private rustdoc probe capture: {0}")]
    Encode(String),
    #[error("failed to write private rustdoc probe capture")]
    Write(#[source] std::io::Error),
}

#[cfg(test)]
mod tests {
    use crate::compiler::tests::*;

    #[test]
    fn rustdoc_probe_accepts_expected_library_and_binary_invocations() {
        let target = tempfile::Builder::new()
            .prefix("conkit-rustdoc-")
            .tempdir()
            .expect("probe target directory");
        let source = target.path().join("lib.rs");
        std::fs::write(&source, "pub fn answer() {}\n").expect("probe source");
        let source = std::fs::canonicalize(source).expect("canonical probe source");
        let library = CompilerFixture::child_probe(target.path(), &source, 'a', "lib");
        let arguments = CompilerFixture::rustdoc_probe_arguments(&source, target.path());
        let record = library
            .record(arguments.into_iter())
            .expect("recognized Cargo rustdoc invocation");
        assert_eq!(record.exit_code, RUSTDOC_PROBE_EXIT_CODE);
        assert_eq!(record.cfg_values, ["docsrs", "feature=\"serde\""]);

        let binary = CompilerFixture::child_probe(target.path(), &source, 'b', "bin");
        let mut arguments = CompilerFixture::rustdoc_probe_arguments(&source, target.path());
        arguments[2] = OsString::from("--crate-type=bin");
        arguments.push(OsString::from("--document-private-items"));
        binary
            .record(arguments.into_iter())
            .expect("recognized Cargo binary rustdoc invocation");
    }

    #[test]
    fn rustdoc_probe_output_directory_spellings_are_strict() {
        let target = tempfile::Builder::new()
            .prefix("conkit-rustdoc-")
            .tempdir()
            .expect("probe target directory");
        let source = target.path().join("lib.rs");
        std::fs::write(&source, "pub fn answer() {}\n").expect("probe source");
        let source = std::fs::canonicalize(source).expect("canonical probe source");
        let probe = CompilerFixture::child_probe(target.path(), &source, 'f', "lib");
        let cargo_arguments = CompilerFixture::rustdoc_probe_arguments(&source, target.path());
        let output_directory = target.path().join("doc");

        let mut separated_long = cargo_arguments.clone();
        separated_long[4] = OsString::from("--out-dir");
        probe
            .record(separated_long.into_iter())
            .expect("recognized separated long output-directory option");

        let mut joined_long = cargo_arguments.clone();
        joined_long[4] = OsString::from(format!("--out-dir={}", output_directory.display()));
        joined_long.remove(5);
        probe
            .record(joined_long.into_iter())
            .expect("recognized joined long output-directory option");

        for unsupported_option in [
            format!("-o={}", output_directory.display()),
            format!("-o{}", output_directory.display()),
        ] {
            let mut unsupported_short = cargo_arguments.clone();
            unsupported_short[4] = OsString::from(unsupported_option);
            unsupported_short.remove(5);
            assert!(matches!(
                probe.record(unsupported_short.into_iter()),
                Err(RustdocProbeError::UnexpectedInvocation)
            ));
        }

        let mut missing_short = cargo_arguments.clone();
        missing_short.truncate(5);
        assert!(matches!(
            probe.record(missing_short.into_iter()),
            Err(RustdocProbeError::MissingOptionValue { option: "-o" })
        ));

        let mut missing_long = cargo_arguments.clone();
        missing_long[4] = OsString::from("--out-dir");
        missing_long.truncate(5);
        assert!(matches!(
            probe.record(missing_long.into_iter()),
            Err(RustdocProbeError::MissingOptionValue {
                option: "--out-dir"
            })
        ));

        let mut duplicate = cargo_arguments.clone();
        duplicate.push(OsString::from("--out-dir"));
        duplicate.push(output_directory.into_os_string());
        assert!(matches!(
            probe.record(duplicate.into_iter()),
            Err(RustdocProbeError::UnexpectedInvocation)
        ));

        let invalid_directories = [
            PathBuf::from("doc"),
            target.path().join("nested").join("..").join("doc"),
            target
                .path()
                .parent()
                .expect("probe target parent")
                .join("outside-doc"),
        ];
        for invalid_directory in invalid_directories {
            let mut invalid = cargo_arguments.clone();
            invalid[5] = invalid_directory.into_os_string();
            assert!(matches!(
                probe.record(invalid.into_iter()),
                Err(RustdocProbeError::UnexpectedInvocation)
            ));
        }
    }

    #[test]
    fn rustdoc_probe_record_requires_active_token_exit_and_strict_shape() {
        let target = tempfile::Builder::new()
            .prefix("conkit-rustdoc-")
            .tempdir()
            .expect("probe target directory");
        let capture_path = target.path().join("capture.json");
        let probe = RustdocProbe {
            state: ProbeState::Parent {
                executable: PathBuf::from("conkit"),
            },
            capture_path: capture_path.clone(),
            token: "c".repeat(64),
            crate_name: "sample".to_owned(),
            source_path: target.path().join("lib.rs"),
            target_directory: target.path().to_path_buf(),
            crate_types: vec!["lib".to_owned()],
        };
        let usage = CompilerUsage::new(CompilerLimits::default(), CompilerFixture::cancellation());
        let records = [
            ProbeRecord {
                token: probe.token.clone(),
                exit_code: 0,
                cfg_values: vec!["docsrs".to_owned()],
                rejection: None,
            },
            ProbeRecord {
                token: probe.token.clone(),
                exit_code: RUSTDOC_PROBE_EXIT_CODE,
                cfg_values: Vec::new(),
                rejection: Some("Cargo produced an unexpected rustdoc invocation".to_owned()),
            },
        ];
        for record in records {
            std::fs::write(
                &capture_path,
                serde_json::to_vec(&record).expect("probe record fixture"),
            )
            .expect("write probe record");
            assert!(matches!(
                probe.read_record(&usage),
                Err(CompilerError::InvalidRustdocProbeCapture { .. })
            ));
        }

        std::fs::write(
            &capture_path,
            format!(
                "{{\"token\":\"{}\",\"exit_code\":{},\"cfg_values\":[],\"unexpected\":true}}",
                probe.token, RUSTDOC_PROBE_EXIT_CODE
            ),
        )
        .expect("write strict-shape fixture");
        assert!(matches!(
            probe.read_record(&usage),
            Err(CompilerError::InvalidRustdocProbeCapture { .. })
        ));

        std::fs::write(
            &capture_path,
            serde_json::to_vec(&ProbeRecord {
                token: probe.token.clone(),
                exit_code: RUSTDOC_PROBE_EXIT_CODE,
                cfg_values: vec!["docsrs".to_owned()],
                rejection: None,
            })
            .expect("accepted record fixture"),
        )
        .expect("write accepted record");
        assert_eq!(
            probe.read_record(&usage).expect("accepted record"),
            ["docsrs"]
        );
    }

    #[test]
    fn rustdoc_probe_rejects_spoofed_limits_and_create_new_collisions() {
        let target = tempfile::Builder::new()
            .prefix("conkit-rustdoc-")
            .tempdir()
            .expect("probe target directory");
        let source = target.path().join("lib.rs");
        std::fs::write(&source, "pub fn answer() {}\n").expect("probe source");
        let source = std::fs::canonicalize(source).expect("canonical probe source");
        let probe = CompilerFixture::child_probe(target.path(), &source, 'd', "lib");

        let mut private_items = CompilerFixture::rustdoc_probe_arguments(&source, target.path());
        private_items.push(OsString::from("--document-private-items"));
        assert!(matches!(
            probe.record(private_items.into_iter()),
            Err(RustdocProbeError::UnexpectedInvocation)
        ));
        assert!(matches!(
            probe.record(
                vec![OsString::from("x".repeat(
                    usize::try_from(RUSTDOC_PROBE_MAX_ARGUMENT_BYTES).expect("test limit") + 1
                ))]
                .into_iter(),
            ),
            Err(RustdocProbeError::ArgumentLimit)
        ));
        assert!(matches!(
            probe.record(
                (0..super::RUSTDOC_PROBE_MAX_ARGUMENT_COUNT + 1).map(|_| OsString::from("x")),
            ),
            Err(RustdocProbeError::ArgumentLimit)
        ));

        probe
            .capture(std::iter::empty())
            .expect("a rejected invocation still writes bounded evidence");
        assert!(matches!(&probe.state, ProbeState::Child));
        let record: ProbeRecord = serde_json::from_slice(
            &std::fs::read(&probe.capture_path).expect("captured rejection"),
        )
        .expect("probe rejection record");
        assert_eq!(record.exit_code, RUSTDOC_PROBE_EXIT_CODE);
        assert!(record.rejection.is_some());
        assert!(matches!(
            probe.capture(std::iter::empty()),
            Err(RustdocProbeError::Write(source)) if source.kind() == ErrorKind::AlreadyExists
        ));
    }

    #[test]
    fn rustdoc_probe_parent_overrides_only_the_pinned_rustdoc_boundary() {
        let probe = RustdocProbe {
            state: ProbeState::Parent {
                executable: PathBuf::from("/test/conkit"),
            },
            capture_path: PathBuf::from("/test/conkit-rustdoc-1/capture.json"),
            token: "e".repeat(64),
            crate_name: "sample".to_owned(),
            source_path: PathBuf::from("/test/src/lib.rs"),
            target_directory: PathBuf::from("/test/conkit-rustdoc-1"),
            crate_types: vec!["lib".to_owned()],
        };
        let mut command = Command::new("cargo");
        CargoEnvironment::RustdocProbe(&probe).apply(&mut command);
        let environment = command
            .get_envs()
            .map(|(name, value)| (name.to_owned(), value.map(OsStr::to_owned)))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            environment.get(OsStr::new("RUSTDOC")),
            Some(&Some(OsString::from("/test/conkit")))
        );
        assert_eq!(
            environment.get(OsStr::new(super::RUSTDOC_PROBE_TOKEN_ENV)),
            Some(&Some(OsString::from("e".repeat(64))))
        );
        assert_eq!(
            environment.get(OsStr::new(super::RUSTDOC_PROBE_CRATE_TYPES_ENV)),
            Some(&Some(OsString::from("lib")))
        );
    }
}
