//! CLI-owned contract routing, document layout, and sketch adaptation.
//!
//! The CLI accepts commands that target signatures, sketches, or all families.
//! This facade keeps target routing, mandatory-v2 combined-document headers,
//! operation-wide YAML accounting, exact source allowlists, requested-versus-
//! persisted extraction reconciliation, and signature-to-sketch adaptation at
//! the executable boundary instead of leaking clap or filesystem state into
//! either domain crate.

mod document;
mod extraction;
mod layout;
mod sketch;

pub(crate) use document::ContractDocumentPath;
pub(crate) use extraction::{
    CargoFeatures, CargoTarget, CompilerRequest, ExtractionUse, RequestedExtraction,
    SignatureExtractionCoordinator,
};
pub(crate) use layout::{ContractFormatValidator, ContractLayout, LayoutExtraction};
pub(crate) use sketch::{
    SketchAdapter, SketchCheckRequest, SketchGenerateRequest, SketchGenerateResponse,
};

/// Contract target selected by a top-level CLI command.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ContractTarget {
    /// Run the command for every contract family supported by this CLI.
    All,
    /// Run the command for signature contracts only.
    Signatures,
    /// Run the command for sketch contracts only.
    Sketches,
}

/// Check mode selected once at the CLI boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContractCheckMode {
    /// Permit signature capability warnings while enforcing sketch diagnostics.
    Default,
    /// Require a diagnostic-free signature check and enforce sketch diagnostics.
    Strict,
    /// Retain diagnostics while allowing the check to pass.
    Warning,
}

impl ContractCheckMode {
    /// Converts this CLI mode into the signature-domain mode.
    pub(crate) fn signature(self) -> conkit_signature::CheckMode {
        match self {
            Self::Default => conkit_signature::CheckMode::Default,
            Self::Strict => conkit_signature::CheckMode::Strict,
            Self::Warning => conkit_signature::CheckMode::Warning,
        }
    }

    /// Converts this CLI mode into the sketch-domain mode.
    pub(crate) fn sketch(self) -> conkit_sketch::CheckMode {
        match self {
            Self::Default | Self::Strict => conkit_sketch::CheckMode::Enforce,
            Self::Warning => conkit_sketch::CheckMode::Warning,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ContractCheckMode;

    #[test]
    fn check_modes_map_exhaustively_to_both_domains() {
        assert_eq!(
            ContractCheckMode::Default.signature(),
            conkit_signature::CheckMode::Default,
        );
        assert_eq!(
            ContractCheckMode::Strict.signature(),
            conkit_signature::CheckMode::Strict,
        );
        assert_eq!(
            ContractCheckMode::Warning.signature(),
            conkit_signature::CheckMode::Warning,
        );
        assert_eq!(
            ContractCheckMode::Default.sketch(),
            conkit_sketch::CheckMode::Enforce,
        );
        assert_eq!(
            ContractCheckMode::Strict.sketch(),
            conkit_sketch::CheckMode::Enforce,
        );
        assert_eq!(
            ContractCheckMode::Warning.sketch(),
            conkit_sketch::CheckMode::Warning,
        );
    }
}
