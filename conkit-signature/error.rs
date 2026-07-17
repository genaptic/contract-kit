use crate::files::{CatalogPath, FileCatalogError};
use crate::inventory::SignatureId;
use crate::languages::rust::RustSourceSpan;
use crate::languages::rust::parser::RustSymbolTableError;
use crate::languages::rust::parser::source_graph::{RustModuleId, RustSourceGraphError};
use crate::languages::rust::rustdoc::RustCompilerArtifactFailure;
use crate::limits::LimitExceeded;

/// Error type returned by `conkit-signature` public operations.
///
/// This wrapper preserves typed lower-level errors internally while presenting
/// one public error type for builders and async operations.
///
/// # Examples
///
/// ```
/// use conkit_signature::{CatalogPath, FileCatalog, FileCatalogError, SignatureContractKitError};
///
/// let mut catalog = FileCatalog::new();
/// let path = CatalogPath::new("src/lib.rs")?;
/// catalog.insert(path.clone(), Vec::new())?;
/// let error: SignatureContractKitError = catalog.insert(path, Vec::new()).unwrap_err().into();
///
/// assert!(error.to_string().contains("duplicate catalog path"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, thiserror::Error)]
#[error("{kind}")]
pub struct SignatureContractKitError {
    kind: Box<SignatureContractKitErrorKind>,
}

#[derive(Clone, Debug, thiserror::Error)]
enum SignatureContractKitErrorKind {
    #[error(
        "unsupported contract version {version} in {location}; v1 contracts must be recreated and only contract_version 2 is supported"
    )]
    UnsupportedContractVersion { location: String, version: String },
    #[error("duplicate YAML key {key} in {location} at line {line}, column {column}")]
    DuplicateYamlKey {
        location: String,
        key: String,
        line: u64,
        column: u64,
    },
    #[error("cannot generate signatures into {location} without extraction metadata")]
    MissingSignatureExtraction { location: String },
    #[error("lossless YAML edit is unsupported for {location}: {message}")]
    UnsupportedLosslessEdit { location: String, message: String },
    #[error("lossless YAML edit did not reparse to the proposed semantics for {location}")]
    YamlSemanticMismatch { location: String },
    #[error("failed to write catalog output {location}: {message}")]
    WriteFailed { location: String, message: String },
    #[error("failed to parse {location}: {message}")]
    ParseFailed { location: String, message: String },
    #[error("failed to convert signatures: {message}")]
    ConversionFailed { message: String },
    #[error("invalid Rust source in {file} ({module}) at bytes {start}..{end}: {message}")]
    InvalidRustSource {
        file: CatalogPath,
        module: RustModuleId,
        start: usize,
        end: usize,
        message: String,
    },
    #[error("invalid Rust syntax text for {family} {text:?}: {message}")]
    InvalidRustSyntaxText {
        family: RustSyntaxFamily,
        text: String,
        message: String,
    },
    #[error("invalid restricted visibility {requested} from module {current} ({target}): {reason}")]
    InvalidRestrictedVisibility {
        current: RustModuleId,
        requested: String,
        target: RestrictedVisibilityTarget,
        reason: String,
    },
    #[error("unsupported Rust syntax {syntax_kind} in {file} ({module}) at bytes {start}..{end}")]
    UnsupportedRustSyntax {
        file: CatalogPath,
        module: RustModuleId,
        syntax_kind: String,
        start: usize,
        end: usize,
    },
    #[error(transparent)]
    CompilerArtifact(#[from] RustCompilerArtifactFailure),
    #[error(transparent)]
    Catalog(#[from] FileCatalogError),
    #[error(transparent)]
    Inventory(#[from] InventoryError),
    #[error(transparent)]
    RustSourceGraph(#[from] RustSourceGraphError),
    #[error(transparent)]
    RustSymbolTable(#[from] RustSymbolTableError),
    #[error(transparent)]
    Limit(#[from] LimitExceeded),
    #[error("background work queue is full")]
    QueueFull,
    #[error("invalid work options: {message}")]
    InvalidWorkOptions { message: String },
    #[error("signature operation was canceled")]
    OperationCanceled,
    #[error("worker failed: {message}")]
    WorkerFailed { message: String },
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum RustSyntaxFamily {
    Expression,
    TypeParameterBound,
    WherePredicate,
    Pattern,
}

impl std::fmt::Display for RustSyntaxFamily {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Expression => "expression",
            Self::TypeParameterBound => "type parameter bound",
            Self::WherePredicate => "where predicate",
            Self::Pattern => "pattern",
        })
    }
}

#[derive(Clone, Debug)]
enum RestrictedVisibilityTarget {
    Unavailable,
    Resolved(RustModuleId),
}

impl std::fmt::Display for RestrictedVisibilityTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("resolved target unavailable"),
            Self::Resolved(module) => write!(formatter, "resolved target {module}"),
        }
    }
}

impl SignatureContractKitError {
    /// Returns the typed resource-limit evidence when this operation failed a
    /// configured budget.
    pub fn limit_exceeded(&self) -> Option<&LimitExceeded> {
        match self.kind.as_ref() {
            SignatureContractKitErrorKind::Limit(error) => Some(error),
            SignatureContractKitErrorKind::UnsupportedContractVersion { .. }
            | SignatureContractKitErrorKind::DuplicateYamlKey { .. }
            | SignatureContractKitErrorKind::MissingSignatureExtraction { .. }
            | SignatureContractKitErrorKind::UnsupportedLosslessEdit { .. }
            | SignatureContractKitErrorKind::YamlSemanticMismatch { .. }
            | SignatureContractKitErrorKind::WriteFailed { .. }
            | SignatureContractKitErrorKind::ParseFailed { .. }
            | SignatureContractKitErrorKind::ConversionFailed { .. }
            | SignatureContractKitErrorKind::InvalidRustSource { .. }
            | SignatureContractKitErrorKind::InvalidRustSyntaxText { .. }
            | SignatureContractKitErrorKind::InvalidRestrictedVisibility { .. }
            | SignatureContractKitErrorKind::UnsupportedRustSyntax { .. }
            | SignatureContractKitErrorKind::CompilerArtifact(_)
            | SignatureContractKitErrorKind::Catalog(_)
            | SignatureContractKitErrorKind::Inventory(_)
            | SignatureContractKitErrorKind::RustSourceGraph(_)
            | SignatureContractKitErrorKind::RustSymbolTable(_)
            | SignatureContractKitErrorKind::QueueFull
            | SignatureContractKitErrorKind::InvalidWorkOptions { .. }
            | SignatureContractKitErrorKind::OperationCanceled
            | SignatureContractKitErrorKind::WorkerFailed { .. } => None,
        }
    }

    /// Returns typed compiler-artifact evidence when compiler extraction failed.
    pub fn compiler_artifact_failure(&self) -> Option<&RustCompilerArtifactFailure> {
        match self.kind.as_ref() {
            SignatureContractKitErrorKind::CompilerArtifact(error) => Some(error),
            SignatureContractKitErrorKind::UnsupportedContractVersion { .. }
            | SignatureContractKitErrorKind::DuplicateYamlKey { .. }
            | SignatureContractKitErrorKind::MissingSignatureExtraction { .. }
            | SignatureContractKitErrorKind::UnsupportedLosslessEdit { .. }
            | SignatureContractKitErrorKind::YamlSemanticMismatch { .. }
            | SignatureContractKitErrorKind::WriteFailed { .. }
            | SignatureContractKitErrorKind::ParseFailed { .. }
            | SignatureContractKitErrorKind::ConversionFailed { .. }
            | SignatureContractKitErrorKind::InvalidRustSource { .. }
            | SignatureContractKitErrorKind::InvalidRustSyntaxText { .. }
            | SignatureContractKitErrorKind::InvalidRestrictedVisibility { .. }
            | SignatureContractKitErrorKind::UnsupportedRustSyntax { .. }
            | SignatureContractKitErrorKind::Catalog(_)
            | SignatureContractKitErrorKind::Inventory(_)
            | SignatureContractKitErrorKind::RustSourceGraph(_)
            | SignatureContractKitErrorKind::RustSymbolTable(_)
            | SignatureContractKitErrorKind::Limit(_)
            | SignatureContractKitErrorKind::QueueFull
            | SignatureContractKitErrorKind::InvalidWorkOptions { .. }
            | SignatureContractKitErrorKind::OperationCanceled
            | SignatureContractKitErrorKind::WorkerFailed { .. } => None,
        }
    }

    /// Returns whether this operation was rejected because every active and
    /// pending work slot was occupied.
    pub fn is_queue_full(&self) -> bool {
        matches!(self.kind.as_ref(), SignatureContractKitErrorKind::QueueFull)
    }

    pub(crate) fn unsupported_contract_version(
        location: impl ToString,
        version: impl ToString,
    ) -> Self {
        Self {
            kind: Box::new(SignatureContractKitErrorKind::UnsupportedContractVersion {
                location: location.to_string(),
                version: version.to_string(),
            }),
        }
    }

    pub(crate) fn unsupported_lossless_edit(
        location: impl ToString,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind: Box::new(SignatureContractKitErrorKind::UnsupportedLosslessEdit {
                location: location.to_string(),
                message: message.into(),
            }),
        }
    }

    pub(crate) fn duplicate_yaml_key(
        location: impl ToString,
        key: Option<String>,
        line: u64,
        column: u64,
    ) -> Self {
        Self {
            kind: Box::new(SignatureContractKitErrorKind::DuplicateYamlKey {
                location: location.to_string(),
                key: key.unwrap_or_else(|| "<non-scalar>".to_owned()),
                line,
                column,
            }),
        }
    }

    pub(crate) fn missing_signature_extraction(location: impl ToString) -> Self {
        Self {
            kind: Box::new(SignatureContractKitErrorKind::MissingSignatureExtraction {
                location: location.to_string(),
            }),
        }
    }

    pub(crate) fn yaml_semantic_mismatch(location: impl ToString) -> Self {
        Self {
            kind: Box::new(SignatureContractKitErrorKind::YamlSemanticMismatch {
                location: location.to_string(),
            }),
        }
    }

    pub(crate) fn write_failed(location: impl ToString, message: impl Into<String>) -> Self {
        Self {
            kind: Box::new(SignatureContractKitErrorKind::WriteFailed {
                location: location.to_string(),
                message: message.into(),
            }),
        }
    }

    pub(crate) fn parse_failed(location: impl ToString, message: impl Into<String>) -> Self {
        Self {
            kind: Box::new(SignatureContractKitErrorKind::ParseFailed {
                location: location.to_string(),
                message: message.into(),
            }),
        }
    }

    pub(crate) fn conversion_failed(message: impl Into<String>) -> Self {
        Self {
            kind: Box::new(SignatureContractKitErrorKind::ConversionFailed {
                message: message.into(),
            }),
        }
    }

    pub(crate) fn invalid_rust_source(
        module: RustModuleId,
        span: RustSourceSpan,
        message: impl Into<String>,
    ) -> Self {
        let file = span.file().clone();
        let range = span.byte_range();
        Self {
            kind: Box::new(SignatureContractKitErrorKind::InvalidRustSource {
                file,
                module,
                start: range.start,
                end: range.end,
                message: message.into(),
            }),
        }
    }

    pub(crate) fn invalid_rust_syntax_text(
        family: RustSyntaxFamily,
        text: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind: Box::new(SignatureContractKitErrorKind::InvalidRustSyntaxText {
                family,
                text: text.into(),
                message: message.into(),
            }),
        }
    }

    pub(crate) fn invalid_restricted_visibility(
        current: RustModuleId,
        requested: impl Into<String>,
        target: Option<RustModuleId>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            kind: Box::new(SignatureContractKitErrorKind::InvalidRestrictedVisibility {
                current,
                requested: requested.into(),
                target: target
                    .map(RestrictedVisibilityTarget::Resolved)
                    .unwrap_or(RestrictedVisibilityTarget::Unavailable),
                reason: reason.into(),
            }),
        }
    }

    pub(crate) fn unsupported_rust_syntax(
        module: RustModuleId,
        syntax_kind: impl Into<String>,
        span: RustSourceSpan,
    ) -> Self {
        let file = span.file().clone();
        let range = span.byte_range();
        Self {
            kind: Box::new(SignatureContractKitErrorKind::UnsupportedRustSyntax {
                file,
                module,
                syntax_kind: syntax_kind.into(),
                start: range.start,
                end: range.end,
            }),
        }
    }

    pub(crate) fn worker_failed(message: impl Into<String>) -> Self {
        Self {
            kind: Box::new(SignatureContractKitErrorKind::WorkerFailed {
                message: message.into(),
            }),
        }
    }

    pub(crate) fn queue_full() -> Self {
        Self {
            kind: Box::new(SignatureContractKitErrorKind::QueueFull),
        }
    }

    pub(crate) fn invalid_work_options(message: impl Into<String>) -> Self {
        Self {
            kind: Box::new(SignatureContractKitErrorKind::InvalidWorkOptions {
                message: message.into(),
            }),
        }
    }

    pub(crate) fn operation_canceled() -> Self {
        Self {
            kind: Box::new(SignatureContractKitErrorKind::OperationCanceled),
        }
    }

    pub(crate) fn is_operation_canceled(&self) -> bool {
        matches!(
            self.kind.as_ref(),
            SignatureContractKitErrorKind::OperationCanceled
        )
    }
}

impl From<FileCatalogError> for SignatureContractKitError {
    fn from(error: FileCatalogError) -> Self {
        Self {
            kind: Box::new(SignatureContractKitErrorKind::Catalog(error)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum InventoryError {
    #[error("signature digest mismatch for duplicate id: {id}")]
    DuplicateSignatureMismatch { id: SignatureId },
    #[error(
        "signature id {id} is assigned to multiple groups: {existing_group} and {incoming_group}"
    )]
    DuplicateSignatureGroup {
        id: Box<SignatureId>,
        existing_group: Box<SignatureId>,
        incoming_group: Box<SignatureId>,
    },
    #[error("signature inventory group is missing: {id}")]
    MissingSignatureGroup { id: SignatureId },
    #[error("signature inventory entry is missing: {id}")]
    MissingSignatureEntry { id: SignatureId },
}

impl From<InventoryError> for SignatureContractKitError {
    fn from(error: InventoryError) -> Self {
        Self {
            kind: Box::new(SignatureContractKitErrorKind::Inventory(error)),
        }
    }
}

impl From<RustSourceGraphError> for SignatureContractKitError {
    fn from(error: RustSourceGraphError) -> Self {
        Self {
            kind: Box::new(SignatureContractKitErrorKind::RustSourceGraph(error)),
        }
    }
}

impl From<RustSymbolTableError> for SignatureContractKitError {
    fn from(error: RustSymbolTableError) -> Self {
        match error {
            RustSymbolTableError::LimitExceeded(error) => error.into(),
            RustSymbolTableError::OperationCanceled => Self::operation_canceled(),
            error => Self {
                kind: Box::new(SignatureContractKitErrorKind::RustSymbolTable(error)),
            },
        }
    }
}

impl From<LimitExceeded> for SignatureContractKitError {
    fn from(error: LimitExceeded) -> Self {
        Self {
            kind: Box::new(SignatureContractKitErrorKind::Limit(error)),
        }
    }
}

impl From<RustCompilerArtifactFailure> for SignatureContractKitError {
    fn from(error: RustCompilerArtifactFailure) -> Self {
        Self {
            kind: Box::new(SignatureContractKitErrorKind::CompilerArtifact(error)),
        }
    }
}
