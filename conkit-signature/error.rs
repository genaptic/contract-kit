use crate::files::FileCatalogError;
use crate::inventory::SignatureId;

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
    kind: SignatureContractKitErrorKind,
}

#[derive(Clone, Debug, thiserror::Error)]
enum SignatureContractKitErrorKind {
    #[error("failed to write catalog output {location}: {message}")]
    WriteFailed { location: String, message: String },
    #[error("failed to parse {location}: {message}")]
    ParseFailed { location: String, message: String },
    #[error("failed to convert signatures: {message}")]
    ConversionFailed { message: String },
    #[error(transparent)]
    Catalog(#[from] FileCatalogError),
    #[error(transparent)]
    Inventory(#[from] InventoryError),
    #[error("worker failed: {message}")]
    WorkerFailed { message: String },
}

impl SignatureContractKitError {
    pub(crate) fn write_failed(location: impl ToString, message: impl Into<String>) -> Self {
        Self {
            kind: SignatureContractKitErrorKind::WriteFailed {
                location: location.to_string(),
                message: message.into(),
            },
        }
    }

    pub(crate) fn parse_failed(location: impl ToString, message: impl Into<String>) -> Self {
        Self {
            kind: SignatureContractKitErrorKind::ParseFailed {
                location: location.to_string(),
                message: message.into(),
            },
        }
    }

    pub(crate) fn conversion_failed(message: impl Into<String>) -> Self {
        Self {
            kind: SignatureContractKitErrorKind::ConversionFailed {
                message: message.into(),
            },
        }
    }

    pub(crate) fn worker_failed(message: impl Into<String>) -> Self {
        Self {
            kind: SignatureContractKitErrorKind::WorkerFailed {
                message: message.into(),
            },
        }
    }
}

impl From<FileCatalogError> for SignatureContractKitError {
    fn from(error: FileCatalogError) -> Self {
        Self {
            kind: SignatureContractKitErrorKind::Catalog(error),
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
        id: SignatureId,
        existing_group: SignatureId,
        incoming_group: SignatureId,
    },
}

impl From<InventoryError> for SignatureContractKitError {
    fn from(error: InventoryError) -> Self {
        Self {
            kind: SignatureContractKitErrorKind::Inventory(error),
        }
    }
}
