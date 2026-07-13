use crate::files::FileCatalogError;
use crate::inventory::InventoryError;

/// Error type returned by `conkit-sketch` public operations.
///
/// This wrapper preserves typed lower-level errors internally while presenting
/// one public error type for builders and async operations. It represents
/// failures that prevent an operation from producing a response, such as
/// invalid catalog or contract input, output rendering failures, and worker
/// failures.
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
    #[error("failed to parse catalog input {location}: {message}")]
    ParseFailed { location: String, message: String },
    #[error("failed to render catalog output {location}: {message}")]
    WriteFailed { location: String, message: String },
    #[error("failed to convert sketches: {message}")]
    ConversionFailed { message: String },
    #[error(transparent)]
    Catalog(#[from] FileCatalogError),
    #[error(transparent)]
    Inventory(#[from] InventoryError),
    #[error("worker failed: {message}")]
    WorkerFailed { message: String },
}

impl SketchContractKitError {
    pub(crate) fn parse_failed(location: impl ToString, message: impl Into<String>) -> Self {
        Self {
            kind: SketchContractKitErrorKind::ParseFailed {
                location: location.to_string(),
                message: message.into(),
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

    pub(crate) fn conversion_failed(message: impl Into<String>) -> Self {
        Self {
            kind: SketchContractKitErrorKind::ConversionFailed {
                message: message.into(),
            },
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

impl From<InventoryError> for SketchContractKitError {
    fn from(error: InventoryError) -> Self {
        Self {
            kind: SketchContractKitErrorKind::Inventory(error),
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
