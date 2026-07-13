//! CLI-owned contract routing, document layout, and sketch adaptation.
//!
//! The CLI accepts commands that target signatures, sketches, or all families.
//! These types keep that routing local to the executable instead of leaking
//! clap-specific enums into domain crates.

mod document;
mod layout;
mod sketch;

pub(crate) use document::ContractDocumentPath;
pub(crate) use layout::ContractLayout;
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
    /// Use each domain's default comparison policy.
    Default,
    /// Treat diagnostics as a failed check.
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
            Self::Default => conkit_sketch::CheckMode::Default,
            Self::Strict => conkit_sketch::CheckMode::Strict,
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
            conkit_sketch::CheckMode::Default,
        );
        assert_eq!(
            ContractCheckMode::Strict.sketch(),
            conkit_sketch::CheckMode::Strict,
        );
        assert_eq!(
            ContractCheckMode::Warning.sketch(),
            conkit_sketch::CheckMode::Warning,
        );
    }
}
