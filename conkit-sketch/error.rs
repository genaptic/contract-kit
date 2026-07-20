use crate::files::FileCatalogError;
use crate::id::SketchIdError;
use crate::limits::LimitExceeded;

/// Error type returned by `conkit-sketch` public operations.
///
/// This wrapper preserves typed lower-level errors internally while presenting
/// one public error type for builders and async operations. It represents
/// failures that prevent an operation from producing a response, such as
/// invalid catalog or contract input, full work admission, configured resource
/// limits, work-capacity overflow, unsafe or unverifiable lossless edits,
/// output rendering failures, and worker failures.
///
/// Valid check outcomes such as a missing source entry or a non-matching
/// snippet are returned as [`SketchDiagnostic`](crate::SketchDiagnostic) values
/// in [`CheckResponse`](crate::CheckResponse), not as this error.
///
/// # Examples
///
/// ```
/// use conkit_sketch::{CatalogPath, FileCatalog, SketchContractKitError};
///
/// let mut catalog = FileCatalog::new();
/// let path = CatalogPath::new("src/lib.rs")?;
/// catalog.insert(path.clone(), Vec::new())?;
/// let error: SketchContractKitError = catalog.insert(path, Vec::new()).unwrap_err().into();
///
/// assert!(error.to_string().contains("duplicate catalog path"));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, thiserror::Error)]
#[error("{kind}")]
pub struct SketchContractKitError {
    kind: SketchContractKitErrorKind,
}

#[derive(Clone, Debug, thiserror::Error)]
enum SketchContractKitErrorKind {
    #[error(
        "unsupported contract version {found} in {location}; recreate this contract using contract_version: 2"
    )]
    UnsupportedContractVersion { location: String, found: String },
    #[error("duplicate YAML mapping key {key} in {location} document {document_index}")]
    DuplicateYamlKey {
        location: String,
        document_index: usize,
        key: String,
    },
    #[error("failed to parse catalog input {location}: {message}")]
    ParseFailed { location: String, message: String },
    #[error("lossless YAML edit is unsupported for {location}: {message}")]
    UnsupportedLosslessEdit { location: String, message: String },
    #[error("lossless YAML edit changed contract semantics for {location}")]
    YamlSemanticMismatch { location: String },
    #[error(
        "cannot refresh aliased code for sketch {sketch_id} in {location} document {document_index}; alias target mutation is not provably local"
    )]
    AliasedSketchCodeMutation {
        location: String,
        document_index: usize,
        sketch_id: String,
    },
    #[error(
        "cannot refresh anchored code for sketch {sketch_id} in {location} document {document_index}; anchor dependents cannot be proven absent"
    )]
    AnchoredSketchCodeMutation {
        location: String,
        document_index: usize,
        sketch_id: String,
    },
    #[error("failed to render catalog output {location}: {message}")]
    WriteFailed { location: String, message: String },
    #[error("failed to convert sketches: {message}")]
    ConversionFailed { message: String },
    #[error("invalid sketch id in {location}: {source}")]
    InvalidSketchId {
        location: String,
        #[source]
        source: SketchIdError,
    },
    #[error(transparent)]
    Catalog(#[from] FileCatalogError),
    #[error(transparent)]
    Limit(#[from] LimitExceeded),
    #[error("work queue is full")]
    QueueFull,
    #[error("operation was cancelled")]
    OperationCancelled,
    #[error("active plus pending work capacity overflowed usize")]
    WorkCapacityOverflow,
    #[error("worker failed: {message}")]
    WorkerFailed { message: String },
}

impl SketchContractKitError {
    /// Returns whether the operation was rejected because active plus pending
    /// admission was full.
    ///
    /// This is distinct from builder-time work-capacity overflow and from a
    /// worker failing after admission.
    pub fn is_queue_full(&self) -> bool {
        matches!(&self.kind, SketchContractKitErrorKind::QueueFull)
    }

    /// Returns typed resource-limit evidence when a configured budget stopped
    /// the operation.
    ///
    /// Returns `None` for queue, validation, rendering, capacity, cancellation,
    /// and worker failures. [`LimitExceeded::observed_at_least`] is a proven
    /// lower bound at the point work stopped rather than a final total.
    pub fn limit_exceeded(&self) -> Option<&LimitExceeded> {
        match &self.kind {
            SketchContractKitErrorKind::Limit(error) => Some(error),
            _ => None,
        }
    }

    pub(crate) fn unsupported_contract_version(
        location: impl ToString,
        found: Option<u16>,
    ) -> Self {
        Self {
            kind: SketchContractKitErrorKind::UnsupportedContractVersion {
                location: location.to_string(),
                found: found.map_or_else(
                    || "missing contract_version".to_owned(),
                    |value| value.to_string(),
                ),
            },
        }
    }

    pub(crate) fn parse_failed(location: impl ToString, message: impl Into<String>) -> Self {
        Self {
            kind: SketchContractKitErrorKind::ParseFailed {
                location: location.to_string(),
                message: message.into(),
            },
        }
    }

    pub(crate) fn duplicate_yaml_key(
        location: impl ToString,
        document_index: usize,
        key: Option<String>,
    ) -> Self {
        Self {
            kind: SketchContractKitErrorKind::DuplicateYamlKey {
                location: location.to_string(),
                document_index,
                key: key.unwrap_or_else(|| "<non-scalar key>".to_owned()),
            },
        }
    }

    pub(crate) fn write_failed(location: impl ToString, message: impl Into<String>) -> Self {
        Self {
            kind: SketchContractKitErrorKind::WriteFailed {
                location: location.to_string(),
                message: message.into(),
            },
        }
    }

    pub(crate) fn unsupported_lossless_edit(
        location: impl ToString,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind: SketchContractKitErrorKind::UnsupportedLosslessEdit {
                location: location.to_string(),
                message: message.into(),
            },
        }
    }

    pub(crate) fn yaml_semantic_mismatch(location: impl ToString) -> Self {
        Self {
            kind: SketchContractKitErrorKind::YamlSemanticMismatch {
                location: location.to_string(),
            },
        }
    }

    pub(crate) fn aliased_sketch_code_mutation(
        location: impl ToString,
        document_index: usize,
        sketch_id: impl Into<String>,
    ) -> Self {
        Self {
            kind: SketchContractKitErrorKind::AliasedSketchCodeMutation {
                location: location.to_string(),
                document_index,
                sketch_id: sketch_id.into(),
            },
        }
    }

    pub(crate) fn anchored_sketch_code_mutation(
        location: impl ToString,
        document_index: usize,
        sketch_id: impl Into<String>,
    ) -> Self {
        Self {
            kind: SketchContractKitErrorKind::AnchoredSketchCodeMutation {
                location: location.to_string(),
                document_index,
                sketch_id: sketch_id.into(),
            },
        }
    }

    pub(crate) fn conversion_failed(message: impl Into<String>) -> Self {
        Self {
            kind: SketchContractKitErrorKind::ConversionFailed {
                message: message.into(),
            },
        }
    }

    pub(crate) fn invalid_sketch_id(location: impl ToString, source: SketchIdError) -> Self {
        Self {
            kind: SketchContractKitErrorKind::InvalidSketchId {
                location: location.to_string(),
                source,
            },
        }
    }

    pub(crate) fn queue_full() -> Self {
        Self {
            kind: SketchContractKitErrorKind::QueueFull,
        }
    }

    pub(crate) fn operation_cancelled() -> Self {
        Self {
            kind: SketchContractKitErrorKind::OperationCancelled,
        }
    }

    pub(crate) fn is_operation_cancelled(&self) -> bool {
        matches!(&self.kind, SketchContractKitErrorKind::OperationCancelled)
    }

    pub(crate) fn work_capacity_overflow() -> Self {
        Self {
            kind: SketchContractKitErrorKind::WorkCapacityOverflow,
        }
    }

    pub(crate) fn worker_failed(message: impl Into<String>) -> Self {
        Self {
            kind: SketchContractKitErrorKind::WorkerFailed {
                message: message.into(),
            },
        }
    }
}

impl From<FileCatalogError> for SketchContractKitError {
    fn from(error: FileCatalogError) -> Self {
        Self {
            kind: SketchContractKitErrorKind::Catalog(error),
        }
    }
}

impl From<LimitExceeded> for SketchContractKitError {
    fn from(error: LimitExceeded) -> Self {
        Self {
            kind: SketchContractKitErrorKind::Limit(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SketchContractKitError;
    use crate::files::{CatalogPath, FileCatalog};

    #[test]
    fn catalog_errors_keep_public_message_context() {
        let path = CatalogPath::new("src/lib.rs").expect("path");
        let mut catalog = FileCatalog::new();
        catalog
            .insert(path.clone(), Vec::new())
            .expect("first insert");

        let error: SketchContractKitError = catalog
            .insert(path, Vec::new())
            .expect_err("duplicate should fail")
            .into();

        assert!(error.to_string().contains("duplicate catalog path"));
    }

    #[test]
    fn constructor_messages_name_operation_context() {
        let parse_error =
            SketchContractKitError::parse_failed("contracts/main.yml", "bad sketches");
        let worker_error = SketchContractKitError::worker_failed("channel closed");

        assert!(
            parse_error
                .to_string()
                .contains("failed to parse catalog input contracts/main.yml")
        );
        assert!(worker_error.to_string().contains("worker failed"));
    }
}
