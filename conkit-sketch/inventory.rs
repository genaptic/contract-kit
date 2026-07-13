use crate::api::{CheckMode, CheckResponse};
use crate::files::FileCatalog;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// A per-sketch diagnostic from a completed check.
///
/// Diagnostics describe matching outcomes rather than operation failures.
/// Contract parsing, validation, and report-rendering failures instead return a
/// [`SketchContractKitError`](crate::SketchContractKitError) from
/// [`SketchContractKit::check`](crate::SketchContractKit::check), without a
/// [`CheckResponse`]. When a check does complete, it retains diagnostics in
/// every mode: [`CheckMode::Default`] and [`CheckMode::Strict`] fail a response
/// that has diagnostics, while [`CheckMode::Warning`] allows it to pass.
///
/// A response has at most one diagnostic per parsed sketch. Diagnostics are
/// ordered deterministically by sketch identifier, optional logical file path,
/// and diagnostic kind. File values are logical catalog paths represented as
/// strings, not operating-system paths or language-specific parser data.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SketchDiagnostic {
    /// The sketch references a logical file absent from the source catalog.
    MissingFile {
        /// The user-defined sketch identifier.
        sketch_id: String,
        /// The logical source file path named by the sketch.
        file: String,
    },
    /// The sketch snippet was empty after normalization.
    ///
    /// The public checker rejects an empty normalized snippet as a
    /// [`SketchContractKitError`](crate::SketchContractKitError) while parsing
    /// contracts, before constructing a response. This variant is therefore
    /// not emitted by [`SketchContractKit::check`](crate::SketchContractKit::check).
    EmptySnippet {
        /// The user-defined sketch identifier.
        sketch_id: String,
    },
    /// The source file existed, but the normalized snippet was not found as a
    /// contiguous normalized line sequence.
    NotMatched {
        /// The user-defined sketch identifier.
        sketch_id: String,
        /// The logical source file path named by the sketch.
        file: String,
    },
    /// The contract catalog contained a duplicate sketch identifier.
    ///
    /// The public checker rejects duplicate identifiers as a
    /// [`SketchContractKitError`](crate::SketchContractKitError) while parsing
    /// contracts, before constructing a response. This variant is therefore
    /// not emitted by [`SketchContractKit::check`](crate::SketchContractKit::check).
    DuplicateSketch {
        /// The duplicated user-defined sketch identifier.
        sketch_id: String,
    },
}

impl SketchDiagnostic {
    pub(crate) fn missing_file(sketch_id: impl ToString, file: impl ToString) -> Self {
        Self::MissingFile {
            sketch_id: sketch_id.to_string(),
            file: file.to_string(),
        }
    }

    pub(crate) fn empty_snippet(sketch_id: impl ToString) -> Self {
        Self::EmptySnippet {
            sketch_id: sketch_id.to_string(),
        }
    }

    pub(crate) fn not_matched(sketch_id: impl ToString, file: impl ToString) -> Self {
        Self::NotMatched {
            sketch_id: sketch_id.to_string(),
            file: file.to_string(),
        }
    }

    fn compare(left: &Self, right: &Self) -> Ordering {
        left.sketch_id()
            .cmp(right.sketch_id())
            .then_with(|| left.file().cmp(&right.file()))
            .then_with(|| left.kind_rank().cmp(&right.kind_rank()))
    }

    fn sketch_id(&self) -> &str {
        match self {
            Self::MissingFile { sketch_id, .. }
            | Self::EmptySnippet { sketch_id }
            | Self::NotMatched { sketch_id, .. }
            | Self::DuplicateSketch { sketch_id } => sketch_id,
        }
    }

    fn file(&self) -> Option<&str> {
        match self {
            Self::MissingFile { file, .. } | Self::NotMatched { file, .. } => Some(file.as_str()),
            Self::EmptySnippet { .. } | Self::DuplicateSketch { .. } => None,
        }
    }

    fn kind_rank(&self) -> u8 {
        match self {
            Self::MissingFile { .. } => 0,
            Self::EmptySnippet { .. } => 1,
            Self::NotMatched { .. } => 2,
            Self::DuplicateSketch { .. } => 3,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SketchInventoryComparison {
    source_file_count: usize,
    contract_file_count: usize,
    sketch_count: usize,
    diagnostics: Vec<SketchDiagnostic>,
}

impl SketchInventoryComparison {
    pub(crate) fn new(
        source_file_count: usize,
        contract_file_count: usize,
        sketch_count: usize,
        mut diagnostics: Vec<SketchDiagnostic>,
    ) -> Result<Self, InventoryError> {
        if diagnostics.len() > sketch_count {
            return Err(InventoryError::ComparisonFailed {
                message: format!(
                    "diagnostic count {} exceeds sketch count {}",
                    diagnostics.len(),
                    sketch_count
                ),
            });
        }

        diagnostics.sort_by(SketchDiagnostic::compare);

        Ok(Self {
            source_file_count,
            contract_file_count,
            sketch_count,
            diagnostics,
        })
    }

    pub(crate) fn diagnostics(&self) -> &[SketchDiagnostic] {
        &self.diagnostics
    }

    pub(crate) fn passed(&self, mode: CheckMode) -> bool {
        self.diagnostics().is_empty() || mode == CheckMode::Warning
    }

    pub(crate) fn counts(&self) -> SketchCheckCounts {
        let failed_sketch_count = self.diagnostics().len();
        let matched_sketch_count = self.sketch_count.saturating_sub(failed_sketch_count);

        SketchCheckCounts {
            source_file_count: self.source_file_count,
            contract_file_count: self.contract_file_count,
            sketch_count: self.sketch_count,
            matched_sketch_count,
            failed_sketch_count,
        }
    }

    pub(crate) fn into_response(self, mode: CheckMode) -> CheckResponse {
        let passed = self.passed(mode);
        let counts = self.counts();

        CheckResponse {
            passed,
            counts,
            diagnostics: self.diagnostics,
            report_files: FileCatalog::new(),
        }
    }
}

/// Scope and outcome totals for a completed sketch check.
///
/// For a [`CheckResponse`] returned by the checker, every parsed sketch has one
/// outcome and therefore
/// `matched_sketch_count + failed_sketch_count == sketch_count`.
/// `failed_sketch_count` also equals the response's diagnostic count. It counts
/// sketches with diagnostics even in [`CheckMode::Warning`], where those
/// diagnostics do not make the response fail.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SketchCheckCounts {
    /// Number of entries supplied in the source catalog.
    ///
    /// This includes unreferenced entries; only files named by parsed sketches
    /// participate in matching.
    pub source_file_count: usize,
    /// Number of direct root-level `.yaml` and `.yml` entries parsed.
    ///
    /// Nested YAML entries are not counted. Every counted entry must be a valid
    /// combined contract document. Extension matching is ASCII
    /// case-insensitive.
    pub contract_file_count: usize,
    /// Number of successfully parsed sketch entries across contract files.
    ///
    /// Invalid entries and duplicate identifiers fail the operation before a
    /// response and its counts are returned.
    pub sketch_count: usize,
    /// Number of sketches that matched their declared source file without a
    /// diagnostic.
    pub matched_sketch_count: usize,
    /// Number of sketches that emitted diagnostics.
    ///
    /// This equals [`CheckResponse::diagnostics`]'s length, including when
    /// warning mode permits the response to pass.
    pub failed_sketch_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum InventoryError {
    #[error("failed sketch inventory comparison: {message}")]
    ComparisonFailed { message: String },
}

#[cfg(test)]
mod tests {
    use super::{SketchDiagnostic, SketchInventoryComparison};
    use crate::api::CheckMode;

    #[test]
    fn comparison_counts_match_scope_and_diagnostics() {
        let comparison = SketchInventoryComparison::new(
            3,
            2,
            4,
            vec![SketchDiagnostic::not_matched("b", "src/b.rs")],
        )
        .expect("comparison");

        let counts = comparison.counts();

        assert_eq!(counts.source_file_count, 3);
        assert_eq!(counts.contract_file_count, 2);
        assert_eq!(counts.sketch_count, 4);
        assert_eq!(counts.matched_sketch_count, 3);
        assert_eq!(counts.failed_sketch_count, 1);
    }

    #[test]
    fn warning_mode_passes_with_diagnostics() {
        let comparison = SketchInventoryComparison::new(
            1,
            1,
            1,
            vec![SketchDiagnostic::not_matched("a", "src/a.rs")],
        )
        .expect("comparison");

        assert!(comparison.passed(CheckMode::Warning));
        assert!(!comparison.passed(CheckMode::Default));
        assert!(!comparison.passed(CheckMode::Strict));
    }

    #[test]
    fn into_response_preserves_diagnostics_but_applies_mode() {
        let response = SketchInventoryComparison::new(
            1,
            1,
            1,
            vec![SketchDiagnostic::missing_file("a", "src/a.rs")],
        )
        .expect("comparison")
        .into_response(CheckMode::Warning);

        assert!(response.passed);
        assert_eq!(response.diagnostics.len(), 1);
        assert!(response.report_files.is_empty());
    }

    #[test]
    fn diagnostics_sort_deterministically() {
        let comparison = SketchInventoryComparison::new(
            1,
            1,
            2,
            vec![
                SketchDiagnostic::missing_file("z", "src/z.rs"),
                SketchDiagnostic::not_matched("a", "src/a.rs"),
            ],
        )
        .expect("comparison");

        assert_eq!(
            comparison.diagnostics(),
            &[
                SketchDiagnostic::NotMatched {
                    sketch_id: "a".to_owned(),
                    file: "src/a.rs".to_owned(),
                },
                SketchDiagnostic::MissingFile {
                    sketch_id: "z".to_owned(),
                    file: "src/z.rs".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn diagnostics_serialize_as_data_carrying_enum() {
        let value = serde_json::to_value(SketchDiagnostic::NotMatched {
            sketch_id: "a".to_owned(),
            file: "src/a.rs".to_owned(),
        })
        .expect("serialize diagnostic");

        assert!(value.get("NotMatched").is_some());
    }
}
