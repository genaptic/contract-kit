//! Typed failures for CLI-owned compiler extraction.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use super::process::{CompilerOperation, CompilerSemanticResource, CompilerStream};
use crate::catalog::CatalogReadLimitExceeded;

/// Typed failures from CLI-owned compiler extraction.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CompilerError {
    #[error("compiler extraction was cancelled")]
    CompilerExtractionCancelled,
    #[error("Cargo manifest is unavailable: {path:?}")]
    ManifestUnavailable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Cargo manifest path is not a file: {path:?}")]
    ManifestNotFile { path: PathBuf },
    #[error("Cargo manifest path has no parent directory: {path:?}")]
    ManifestHasNoParent { path: PathBuf },
    #[error("failed to create an isolated compiler-extraction workspace")]
    TemporaryWorkspace(#[source] std::io::Error),
    #[error("failed to create an isolated rustdoc-configuration probe")]
    RustdocProbeWorkspace(#[source] std::io::Error),
    #[error("failed to resolve the current executable for the rustdoc-configuration probe")]
    CurrentExecutable(#[source] std::io::Error),
    #[error("Cargo rustdoc unexpectedly accepted the one-shot configuration probe")]
    RustdocProbeUnexpectedSuccess,
    #[error(
        "{cargo}; the expected rustdoc-configuration probe evidence was unavailable or invalid: {probe}"
    )]
    RustdocProbeFailed {
        #[source]
        cargo: Box<CompilerError>,
        probe: Box<CompilerError>,
    },
    #[error("invalid rustdoc-configuration probe capture: {message}")]
    InvalidRustdocProbeCapture { message: String },
    #[error("failed to start Cargo executable {executable:?}")]
    CargoSpawn {
        executable: OsString,
        #[source]
        source: std::io::Error,
    },
    #[error("the required Cargo/rustdoc toolchain {toolchain} is unavailable: {detail}")]
    SupportedToolchainUnavailable {
        toolchain: &'static str,
        detail: String,
    },
    #[error("{operation} failed with status {status:?}: {stderr}")]
    CargoFailed {
        operation: CompilerOperation,
        status: Option<i32>,
        stderr: String,
    },
    #[error(
        "compiler extraction exceeded its cumulative {stream} byte limit {limit} during {operation}; observed at least {observed_at_least}"
    )]
    ProcessOutputLimit {
        operation: CompilerOperation,
        stream: CompilerStream,
        limit: u64,
        observed_at_least: u64,
    },
    #[error("Cargo process did not expose its configured {stream} pipe")]
    MissingProcessPipe { stream: CompilerStream },
    #[error("failed to start the Cargo {stream} reader thread")]
    ProcessReaderSpawn {
        stream: CompilerStream,
        #[source]
        source: std::io::Error,
    },
    #[error("compiler extraction exceeded its absolute timeout of {timeout:?} during {operation}")]
    ProcessTimeout {
        operation: CompilerOperation,
        timeout: Duration,
    },
    #[error("failed to read Cargo process output")]
    ProcessRead(#[source] std::io::Error),
    #[error("Cargo output reader thread panicked")]
    ProcessReaderPanicked,
    #[error("failed to wait for a Cargo process")]
    ProcessWait(#[source] std::io::Error),
    #[error("{primary}; additional bounded process evidence: {evidence}")]
    ProcessFailureWithEvidence {
        #[source]
        primary: Box<CompilerError>,
        evidence: String,
    },
    #[error("{operation} emitted non-UTF-8 stdout: {message}")]
    NonUtf8ProcessOutput {
        operation: CompilerOperation,
        message: String,
    },
    #[error("invalid Cargo metadata JSON: {message}")]
    InvalidCargoMetadata { message: String },
    #[error("Cargo package-id output is not UTF-8: {message}")]
    InvalidPackageIdOutput { message: String },
    #[error("Cargo resolved package ID {id:?}, but metadata did not contain it")]
    ResolvedPackageMissing { id: String },
    #[error("Cargo package selection is ambiguous; pass --package (candidates: {candidates:?})")]
    AmbiguousPackage { candidates: Vec<String> },
    #[error(
        "Cargo metadata does not expose workspace default members; use a supported Cargo toolchain or pass --package"
    )]
    WorkspaceDefaultMembersUnavailable,
    #[error(
        "Cargo target selection is ambiguous; pass --lib or --bin NAME (candidates: {candidates:?})"
    )]
    AmbiguousTarget { candidates: Vec<String> },
    #[error("Cargo package does not contain the requested {requested} target")]
    TargetMissing { requested: String },
    #[error("compiler extraction requires exactly {expected} crate root, found {actual}")]
    CompilerCrateCount { expected: usize, actual: usize },
    #[error("contract crate root {expected} does not match selected Cargo target {actual}")]
    CrateRootMismatch { expected: String, actual: String },
    #[error("Cargo target root {target_root:?} is outside selected source root {source_root:?}")]
    CargoTargetOutsideSourceRoot {
        source_root: PathBuf,
        target_root: PathBuf,
    },
    #[error("Cargo target source is unavailable: {path:?}")]
    CargoTargetUnavailable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid compiler identity: {message}")]
    InvalidCompilerIdentity { message: String },
    #[error("invalid compiler/rustdoc cfg value {value:?}")]
    InvalidCompilerConfiguration { value: String },
    #[error(
        "temporary compiler artifacts exceeded {limit} bytes under {path:?}; observed at least {observed_at_least}"
    )]
    TemporaryArtifactLimit {
        path: PathBuf,
        limit: u64,
        observed_at_least: u64,
    },
    #[error(
        "temporary compiler artifacts exceeded {limit} entries under {path:?}; observed at least {observed_at_least}"
    )]
    TemporaryArtifactEntryLimit {
        path: PathBuf,
        limit: u64,
        observed_at_least: u64,
    },
    #[error("failed to walk temporary compiler artifacts under {path:?}")]
    TemporaryArtifactWalk {
        path: PathBuf,
        #[source]
        source: walkdir::Error,
    },
    #[error("failed to inspect temporary compiler artifact {path:?}: {message}")]
    TemporaryArtifactMetadata { path: PathBuf, message: String },
    #[error("expected exactly one rustdoc JSON artifact under {root:?}, found {count}")]
    RustdocArtifactCount { root: PathBuf, count: usize },
    #[error("failed to read compiler artifact {path:?} during {operation}")]
    CompilerArtifactRead {
        operation: CompilerOperation,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "compiler artifact {path:?} exceeded its file limit {limit} during {operation}; observed at least {observed_at_least}"
    )]
    CompilerArtifactFileLimit {
        operation: CompilerOperation,
        path: PathBuf,
        limit: u64,
        observed_at_least: u64,
    },
    #[error(
        "compiler extraction exceeded its cumulative artifact byte limit {limit} while reading {path:?} during {operation}; observed at least {observed_at_least}"
    )]
    CompilerArtifactLimit {
        operation: CompilerOperation,
        path: PathBuf,
        limit: u64,
        observed_at_least: u64,
    },
    #[error(
        "compiler extraction exceeded its cumulative {resource} limit {limit} during {operation}; observed at least {observed_at_least}"
    )]
    CompilerSemanticLimit {
        operation: CompilerOperation,
        resource: CompilerSemanticResource,
        limit: u64,
        observed_at_least: u64,
    },
    #[error("invalid rustdoc JSON envelope: {message}")]
    InvalidRustdocJson { message: String },
    #[error(
        "rustdoc JSON private-item flag contradicts the selected target: expected {expected}, found {actual}"
    )]
    RustdocPrivateItemsMismatch { expected: bool, actual: bool },
    #[error("rustdoc target mismatch: expected {expected:?}, document recorded {actual:?}")]
    RustdocTargetMismatch { expected: String, actual: String },
    #[error("failed to re-read the exact source snapshot after Cargo execution: {message}")]
    SourceSnapshotUnavailable { message: String },
    #[error("source snapshot revalidation failed: {0}")]
    SourceSnapshotLimit(#[source] CatalogReadLimitExceeded),
    #[error("source {path} changed while compiler extraction was running")]
    SourceChangedDuringExtraction { path: String },
    #[error("selected source root is unavailable: {path:?}")]
    SourceRootUnavailable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("rustdoc-mapped source path is invalid: {path:?}")]
    InvalidMappedSourcePath { path: PathBuf },
    #[error("rustdoc-mapped source path {path:?} is not portable: {message}")]
    InvalidPortableSourcePath { path: PathBuf, message: String },
    #[error("rustdoc-mapped source {path:?} is not UTF-8: {message}")]
    InvalidMappedSourceUtf8 { path: PathBuf, message: String },
    #[error("invalid rustdoc source span {path:?} {begin:?}..={end:?}: {message}")]
    InvalidSourceSpan {
        path: PathBuf,
        begin: (usize, usize),
        end: (usize, usize),
        message: String,
    },
}
