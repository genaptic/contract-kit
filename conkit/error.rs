//! Error type for CLI-owned failures.
//!
//! Domain crates expose their own typed errors. This module adds the failures
//! owned by the executable: process cancellation and signal registration,
//! compiler extraction, bounded catalog reads, portable path handling,
//! mandatory-v2 YAML layout validation, reports and archives, and generated-
//! file ownership and reconciliation.

use std::path::PathBuf;
use std::time::SystemTimeError;

use crate::catalog::{CatalogReadLimitExceeded, PathRole};
use crate::compiler::CompilerError;
use crate::contracts::ContractTarget;

/// Structured duplicate-key failure from a physical combined-contract document.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error(
    "duplicate YAML mapping key {:?} in {path} at YAML document index {document_index}, line {}, column {}",
    .key.as_deref().unwrap_or("<non-scalar>"),
    .location.line(),
    .location.column()
)]
pub(crate) struct DuplicateContractKey {
    /// Logical contract path containing the invalid document.
    pub(crate) path: PathBuf,
    /// Zero-based physical YAML document index.
    pub(crate) document_index: usize,
    /// Scalar key text when the YAML parser can represent it losslessly.
    pub(crate) key: Option<String>,
    /// Complete source location reported by the maintained YAML parser.
    pub(crate) location: serde_saphyr::Location,
}

/// Errors produced by CLI-owned process, parsing, filesystem, and persistence boundaries.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CliError {
    /// Process-level cancellation was requested by the host operating system.
    #[error("operation canceled")]
    OperationCanceled,
    /// The process-level termination handler could not be installed.
    #[error("failed to install the process termination handler")]
    SignalHandler {
        #[source]
        source: ctrlc::Error,
    },
    /// Compiler-backed signature extraction failed at the CLI process boundary.
    #[error(transparent)]
    Compiler(#[from] CompilerError),
    /// A CLI filesystem-to-catalog read exceeded its configured budget.
    #[error(transparent)]
    CatalogReadLimit(#[from] CatalogReadLimitExceeded),
    /// A command path could not be resolved for overlap validation.
    #[error("failed to resolve {role} path {path}")]
    PathResolution {
        role: PathRole,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// Two command paths are equal or one contains the other.
    #[error("{first_role} path {first_path} overlaps {second_role} path {second_path}")]
    OverlappingPaths {
        first_role: PathRole,
        first_path: PathBuf,
        second_role: PathRole,
        second_path: PathBuf,
    },
    /// A path contains a symbolic link that cannot participate in stable ownership.
    #[error("{role} path contains an unsupported symbolic link: {path}")]
    UnsupportedPathSymlink { role: PathRole, path: PathBuf },
    /// The reserved ownership metadata namespace contains an unknown entry.
    #[error("reserved generated-file metadata namespace contains unsupported entry: {path}")]
    ReservedMetadataEntry { path: PathBuf },
    /// Generated ownership metadata is malformed or internally inconsistent.
    #[error("invalid generated-file ownership metadata {path}: {message}")]
    InvalidGeneratedOwnership { path: PathBuf, message: String },
    /// Generation would overwrite a path that the ownership manifest does not own.
    #[error("refusing to overwrite unowned generated output: {path}")]
    UnownedGeneratedOutput { path: PathBuf },
    /// Another writer currently owns the contracts-root generation lock.
    #[error("another generation is already updating contracts root metadata: {path}")]
    GenerationInProgress { path: PathBuf },
    /// The contracts catalog changed after generation began.
    #[error("contracts changed while generation was running; rerun generation: {path}")]
    GenerationInputChanged { path: PathBuf },
    /// An owned generated output no longer matches its committed digest.
    #[error("owned generated output was modified outside Contract Kit: {path}")]
    ModifiedGeneratedOutput { path: PathBuf },
    /// Two logical generated paths are not distinct on portable filesystems.
    #[error("portable generated path collision under ASCII case matching: {first} and {second}")]
    PortableGeneratedPathCollision { first: String, second: String },
    /// A requested output changes only the ASCII case of an owned path.
    #[error(
        "generated path changes only ASCII case from {previous} to {current}; remove it in one generation before adding the new spelling"
    )]
    CaseOnlyGeneratedPathChange { previous: String, current: String },
    /// Two logical generated paths resolve to one existing host file.
    #[error("generated paths resolve to the same host file: {first} and {second}")]
    GeneratedOutputAlias { first: PathBuf, second: PathBuf },
    /// Interrupted ownership recovery found bytes belonging to neither journal state.
    #[error("cannot recover generated-file ownership because output bytes are unexpected: {path}")]
    GeneratedOwnershipRecoveryConflict { path: PathBuf },
    /// Reservation rollback could not restore a committed ownership state.
    #[error("failed to roll back generated-file reservation state {path}: {message}")]
    GeneratedOwnershipRollback { path: PathBuf, message: String },
    /// An owned generated output path names a non-regular filesystem entry.
    #[error("generated output path is not a file: {path}")]
    GeneratedOutputNotFile { path: PathBuf },
    /// A source or contracts root path is not a directory.
    #[error("{role:?} root is not a directory: {path}")]
    RootIsNotDirectory {
        /// The kind of root path being validated.
        role: PathRole,
        /// The path that was expected to be a directory.
        path: PathBuf,
    },
    /// A file path could not be represented relative to the selected root.
    #[error("path is outside the selected root: {path}")]
    PathOutsideRoot { path: PathBuf },
    /// A path contained components that are invalid for contract catalogs.
    #[error("path cannot be represented as a contract catalog path: {path}")]
    InvalidCatalogPath { path: PathBuf },
    /// A source file named by a contract document could not be read.
    #[error("listed source file is unavailable: {path}")]
    ListedSourceUnavailable {
        /// Source path named by a document's `files` allowlist.
        path: PathBuf,
        /// Underlying metadata error.
        #[source]
        source: std::io::Error,
    },
    /// A source path named by a contract document is not a regular file.
    #[error("listed source path is not a regular file: {path}")]
    ListedSourceNotFile { path: PathBuf },
    /// A combined contract document has an invalid filesystem binding.
    #[error("invalid contract layout {path}: {message}")]
    ContractLayout { path: PathBuf, message: String },
    /// A physical YAML contract document contains a duplicate mapping key.
    #[error(transparent)]
    DuplicateContractKey(#[from] DuplicateContractKey),
    /// A combined YAML document is not the current contract format.
    #[error(
        "unsupported contract version in {path} at YAML document index {document_index}: {message}"
    )]
    UnsupportedContractVersion {
        /// Logical or local path containing the unsupported document.
        path: PathBuf,
        /// Zero-based nonempty document index within the YAML stream.
        document_index: usize,
        /// Upgrade or unsupported-version detail.
        message: String,
    },
    /// A host path component could not be converted to UTF-8.
    #[error("path contains a non-UTF-8 component")]
    NonUtf8PathComponent,
    /// A user-supplied file path did not contain a file name.
    #[error("path has no file name: {path}")]
    MissingFileName { path: PathBuf },
    /// A selected archive path is a symlink or another non-regular entry.
    #[error("archive path is not a regular file: {path}")]
    ArchiveNotRegularFile { path: PathBuf },
    /// A report path extension does not map to a supported report format.
    #[error("unsupported report extension for {path}; expected .yml, .yaml, or .json")]
    UnsupportedReportExtension { path: PathBuf },
    /// Rendering a CLI-owned report failed.
    #[error("failed to render report {path}: {message}")]
    ReportRender {
        /// Local report path being rendered.
        path: PathBuf,
        /// Serialization failure message.
        message: String,
    },
    /// A rendered or domain-provided report exceeded the CLI output budget.
    #[error(
        "report output byte limit exceeded for {path}: limit {limit}, observed at least {observed_at_least}"
    )]
    ReportOutputLimit {
        /// Local report path that would have received the oversized output.
        path: PathBuf,
        /// Configured maximum report bytes.
        limit: u64,
        /// Amount known when rendering or validation stopped.
        observed_at_least: u64,
    },
    /// The selected contract target reported a failed check.
    #[error("contract check failed for {target:?}")]
    CheckFailed { target: ContractTarget },
    /// A domain crate returned no bytes for a requested report.
    #[error("missing report bytes from contract check response")]
    MissingReportBytes,
    /// Encoding or decoding the CLI-owned archive payload failed.
    #[error("failed to process archive: {message}")]
    ArchiveProcess { message: String },
    /// An archive write would overwrite an existing file.
    #[error("archive file already exists: {path}")]
    ArchiveAlreadyExists { path: PathBuf },
    /// No collision-safe archive file name was available for a timestamp.
    #[error("could not find an available archive name in {root} for timestamp {unix_nanos}")]
    ArchiveNameExhausted { root: PathBuf, unix_nanos: u128 },
    /// Writing an archive file failed after a path was selected.
    #[error("failed to write archive file {path}")]
    ArchiveWrite {
        /// Local archive path being written.
        path: PathBuf,
        /// Underlying I/O failure.
        #[source]
        source: std::io::Error,
    },
    /// Writing failed and the incomplete archive could not be removed.
    #[error(
        "failed to write archive file {path}; cleanup also failed: write: {write}; cleanup: {cleanup}"
    )]
    ArchiveWriteAndCleanup {
        /// Local archive path being written.
        path: PathBuf,
        /// Original write, flush, or sync failure.
        write: std::io::Error,
        /// Failure while removing the incomplete file.
        cleanup: std::io::Error,
    },
    /// A Windows output path component used a reserved device name.
    #[error("Windows reserved device name cannot be used as a path component: {component}")]
    WindowsReservedDeviceName { component: String },
    /// A Windows output path component ended with a forbidden space or period.
    #[error("Windows path component must not end with a space or period: {component}")]
    WindowsTrailingSpaceOrDot { component: String },
    /// A path component contains a character forbidden by Windows filenames.
    #[error("Windows-invalid character {character:?} in path component: {component}")]
    WindowsInvalidFileNameCharacter { component: String, character: char },
    /// System time could not be represented as a Unix timestamp.
    #[error("system clock is before Unix epoch")]
    Clock { source: SystemTimeError },
    /// Error returned by the sketch domain crate.
    #[error(transparent)]
    Sketch(#[from] conkit_sketch::SketchContractKitError),
    /// Error returned by the in-memory file catalog.
    #[error(transparent)]
    Catalog(#[from] conkit_signature::FileCatalogError),
    /// Error returned by the sketch in-memory file catalog.
    #[error(transparent)]
    SketchCatalog(#[from] conkit_sketch::FileCatalogError),
    /// Error returned while walking a filesystem tree.
    #[error(transparent)]
    WalkDir(#[from] walkdir::Error),
    /// Generic filesystem error at the CLI boundary.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::CliError;

    #[test]
    fn unsupported_report_extension_names_expected_extensions() {
        let error = CliError::UnsupportedReportExtension {
            path: PathBuf::from("report.txt"),
        };

        assert!(error.to_string().contains(".yml, .yaml, or .json"));
    }

    #[test]
    fn windows_reserved_name_message_names_component() {
        let error = CliError::WindowsReservedDeviceName {
            component: "CON".to_owned(),
        };

        assert!(error.to_string().contains("CON"));
    }
}
