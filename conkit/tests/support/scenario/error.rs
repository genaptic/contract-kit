use std::io;
use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum HarnessError {
    #[error("no scenario.yml files found below {root}", root = .root.display())]
    NoScenarios { root: PathBuf },

    #[error("could not read {path}: {source}", path = .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("could not write {path}: {source}", path = .path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("could not inspect {path}: {source}", path = .path.display())]
    Inspect {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("could not walk {path}: {message}", path = .path.display())]
    Walk { path: PathBuf, message: String },

    #[error("could not parse {path}: {source}", path = .path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },

    #[error("invalid scenario manifest {path}: {message}", path = .path.display())]
    InvalidManifest { path: PathBuf, message: String },

    #[error("invalid scenario path {path}: {message}", path = .path.display())]
    InvalidScenarioPath { path: PathBuf, message: String },

    #[error("could not create temporary scenario directory: {message}")]
    CreateSandbox { message: String },

    #[error("could not close temporary scenario directory: {message}")]
    Cleanup { message: String },

    #[error(
        "scenario execution failed and cleanup also failed:\nexecution: {execution}\ncleanup: {cleanup}"
    )]
    ExecutionAndCleanup {
        execution: Box<HarnessError>,
        cleanup: Box<HarnessError>,
    },

    #[error("cargo metadata failed for {manifest}: {message}", manifest = .manifest.display())]
    CargoMetadata { manifest: PathBuf, message: String },

    #[error(
        "cargo metadata workspace root mismatch for {manifest}: expected {expected}, got {actual}",
        manifest = .manifest.display(),
        expected = .expected.display(),
        actual = .actual.display()
    )]
    CargoWorkspaceRoot {
        manifest: PathBuf,
        expected: PathBuf,
        actual: PathBuf,
    },

    #[error("step {step}: {source}")]
    Step {
        step: usize,
        #[source]
        source: StepError,
    },

    #[error("{scenario}: {source}")]
    Scenario {
        scenario: String,
        #[source]
        source: Box<HarnessError>,
    },

    #[error("scenario failures:\n{details}")]
    ScenarioFailures { details: String },

    #[error("CLI coverage is incomplete: {count} required keys missing\n{details}")]
    IncompleteCoverage { count: usize, details: String },
}

#[derive(Debug, Error)]
pub(crate) enum StepError {
    #[error("invalid path {value:?}: {message}")]
    InvalidPath { value: String, message: String },

    #[error("invalid argument {value:?}: {message}")]
    InvalidArgument { value: String, message: String },

    #[error("capture {name:?} has not been bound")]
    MissingCapture { name: String },

    #[error("capture {name:?} is already bound")]
    DuplicateCapture { name: String },

    #[error("could not launch conkit: {source}")]
    Launch {
        #[source]
        source: io::Error,
    },

    #[error("conkit exited without an exit code")]
    MissingExitCode,

    #[error("expected exit code {expected}, got {actual}\nstdout:\n{stdout}\nstderr:\n{stderr}")]
    ExitCode {
        expected: i32,
        actual: i32,
        stdout: String,
        stderr: String,
    },

    #[error("{stream} was not valid UTF-8: {message}")]
    StreamUtf8 {
        stream: &'static str,
        message: String,
    },

    #[error("{stream} did not match: {message}\nactual:\n{actual}")]
    StreamMismatch {
        stream: &'static str,
        message: String,
        actual: String,
    },

    #[error("could not inspect {path}: {source}", path = .path.display())]
    Inspect {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("could not read {path}: {source}", path = .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("could not write {path}: {source}", path = .path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("could not walk {path}: {message}", path = .path.display())]
    Walk { path: PathBuf, message: String },

    #[error(
        "unsupported entry at {path}: expected a regular file or directory",
        path = .path.display()
    )]
    UnsupportedEntry { path: PathBuf },

    #[error(
        "overlay source {source_path} conflicts with destination {destination}",
        source_path = .source_path.display(),
        destination = .destination.display()
    )]
    OverlayTypeConflict {
        source_path: PathBuf,
        destination: PathBuf,
    },

    #[error("remove target does not exist: {path}", path = .path.display())]
    MissingRemoveTarget { path: PathBuf },

    #[error("refusing to remove sandbox root {path}", path = .path.display())]
    RemoveRoot { path: PathBuf },

    #[error(
        "capture selector for {directory} matched {count} files; expected exactly one",
        directory = .directory.display()
    )]
    CaptureMatchCount { directory: PathBuf, count: usize },

    #[error("tree comparison requires a directory at {path}", path = .path.display())]
    TreeRoot { path: PathBuf },

    #[error("tree comparison failed:\n{details}")]
    TreeMismatch { details: String },
}
