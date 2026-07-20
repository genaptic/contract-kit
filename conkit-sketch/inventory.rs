use crate::api::{CheckMode, CheckResponse};
use crate::contract::{SketchNormalization, SketchOccurrence};
use crate::files::{CatalogPath, FileCatalog};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// Complete logical location of one parsed sketch and its referenced source.
///
/// The contract file and document index locate the authoring site, while the
/// source file locates the bytes that participated in matching. All paths are
/// validated logical catalog paths rather than operating-system paths.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct SketchLocation {
    /// User-defined sketch identifier.
    pub sketch_id: String,
    /// Logical path of the combined contract document containing the sketch.
    pub contract_file: CatalogPath,
    /// Zero-based YAML document index within `contract_file`.
    pub document_index: usize,
    /// Logical source path referenced by the sketch.
    pub source_file: CatalogPath,
}

impl SketchLocation {
    pub(crate) fn new(
        sketch_id: impl Into<String>,
        contract_file: CatalogPath,
        document_index: usize,
        source_file: CatalogPath,
    ) -> Self {
        Self {
            sketch_id: sketch_id.into(),
            contract_file,
            document_index,
            source_file,
        }
    }
}

/// One-based inclusive source-line range for a matching occurrence or candidate.
///
/// Exactly-one occurrence evidence is recorded in source-start order. Ranges
/// may overlap because every contiguous start position is counted.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct SourceLineSpan {
    /// One-based first source line in the range.
    pub start: usize,
    /// One-based final source line in the range, included in the range.
    pub end: usize,
}

impl SourceLineSpan {
    pub(crate) const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

/// Safely rendered line evidence for a failed sketch match.
///
/// Retained raw bytes are rendered with [`std::ascii::escape_default`], so
/// quotes, backslashes, controls, and invalid UTF-8 are safe in text and
/// serialized reports. Truncation is decided on the retained raw-byte prefix
/// before escaping; escape expansion is still charged to the aggregate
/// diagnostic-byte budget.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DiagnosticExcerpt {
    /// Escaped source or expected bytes that are safe for text reports.
    Bytes {
        /// ASCII-escaped representation of the retained raw-byte prefix.
        escaped: String,
        /// Whether the complete raw line exceeded the excerpt-retention budget.
        truncated: bool,
    },
    /// No aligned line exists. The current matcher emits this for the actual
    /// source side when an expected snippet extends past the source.
    Missing,
}

impl DiagnosticExcerpt {
    pub(crate) const fn missing() -> Self {
        Self::Missing
    }
}

/// Bounded evidence for the closest aligned source window after a mismatch.
///
/// Candidate scanning happens only after exact matching finds no occurrence.
/// Every possible source start is scored by the number of expected lines equal
/// to their aligned source lines. The highest score wins; an equal score keeps
/// the earliest source start. Evidence identifies the first differing expected
/// line. When the expected snippet extends past the selected source window,
/// [`Self::actual`] is [`DiagnosticExcerpt::Missing`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MatchCandidate {
    /// One-based inclusive source range covered by the aligned candidate.
    pub source: SourceLineSpan,
    /// One-based expected snippet line containing the first difference.
    pub expected_line: usize,
    /// One-based source line aligned with `expected_line`.
    pub source_line: usize,
    /// Expected snippet-line evidence.
    pub expected: DiagnosticExcerpt,
    /// Actual source evidence, or [`DiagnosticExcerpt::Missing`] when the
    /// expected line extends past the available source.
    pub actual: DiagnosticExcerpt,
}

impl MatchCandidate {
    pub(crate) const fn new(
        source: SourceLineSpan,
        expected_line: usize,
        source_line: usize,
        expected: DiagnosticExcerpt,
        actual: DiagnosticExcerpt,
    ) -> Self {
        Self {
            source,
            expected_line,
            source_line,
            expected,
            actual,
        }
    }
}

/// A per-sketch diagnostic from a completed check.
///
/// Diagnostics describe matching outcomes rather than operation failures.
/// Contract parsing, validation, and report-rendering failures instead return a
/// [`SketchContractKitError`](crate::SketchContractKitError) from
/// [`SketchContractKit::check`](crate::SketchContractKit::check), without a
/// [`CheckResponse`]. When a check does complete, it retains diagnostics in
/// every mode: [`CheckMode::Enforce`] fails a response that has diagnostics,
/// while [`CheckMode::Warning`] allows it to pass.
///
/// A response has at most one diagnostic per parsed sketch. Diagnostics are
/// ordered deterministically by their complete [`SketchLocation`] and then by
/// diagnostic kind. Evidence does not affect ordering. File values are logical
/// catalog paths, not operating-system paths or language-specific parser data.
///
/// # Examples
///
/// Inspect exact occurrence totals separately from bounded span evidence.
///
/// ```
/// use conkit_sketch::{
///     CatalogPath, SketchDiagnostic, SketchLocation, SketchOccurrence,
///     SourceLineSpan,
/// };
///
/// let diagnostic = SketchDiagnostic::OccurrenceMismatch {
///     sketch: SketchLocation {
///         sketch_id: "pair".to_owned(),
///         contract_file: CatalogPath::new("main.yml")?,
///         document_index: 0,
///         source_file: CatalogPath::new("lib.rs")?,
///     },
///     expected: SketchOccurrence::ExactlyOne,
///     actual: 3,
///     spans: vec![SourceLineSpan { start: 1, end: 2 }],
///     spans_truncated: true,
/// };
///
/// match diagnostic {
///     SketchDiagnostic::OccurrenceMismatch {
///         actual,
///         spans,
///         spans_truncated,
///         ..
///     } => {
///         assert_eq!(actual, 3);
///         assert_eq!(spans[0], SourceLineSpan { start: 1, end: 2 });
///         assert!(spans_truncated);
///     }
///     _ => unreachable!("constructed an occurrence diagnostic"),
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SketchDiagnostic {
    /// The sketch references a logical file absent from the source catalog.
    MissingFile {
        /// Contract and source location of the sketch.
        sketch: SketchLocation,
    },
    /// The source file existed, but no occurrence satisfied the sketch.
    NotMatched {
        /// Contract and source location of the sketch.
        sketch: SketchLocation,
        /// Versioned normalization used for this comparison.
        normalization: SketchNormalization,
        /// Closest bounded source evidence, when a candidate window exists.
        candidate: Option<MatchCandidate>,
    },
    /// Matching occurrences violated the sketch's explicit occurrence policy.
    OccurrenceMismatch {
        /// Contract and source location of the sketch.
        sketch: SketchLocation,
        /// Occurrence policy required by the contract.
        expected: SketchOccurrence,
        /// Exact number of matching occurrences found in the source, including
        /// overlapping occurrences.
        actual: usize,
        /// Bounded one-based inclusive occurrence ranges in source-start order.
        spans: Vec<SourceLineSpan>,
        /// Whether additional occurrence ranges were omitted from `spans`.
        ///
        /// Omission affects evidence only; the `actual` member remains exact.
        spans_truncated: bool,
    },
}

impl SketchDiagnostic {
    pub(crate) fn missing_file(sketch: SketchLocation) -> Self {
        Self::MissingFile { sketch }
    }

    pub(crate) fn not_matched(
        sketch: SketchLocation,
        normalization: SketchNormalization,
        candidate: Option<MatchCandidate>,
    ) -> Self {
        Self::NotMatched {
            sketch,
            normalization,
            candidate,
        }
    }

    pub(crate) fn occurrence_mismatch(
        sketch: SketchLocation,
        expected: SketchOccurrence,
        actual: usize,
        spans: Vec<SourceLineSpan>,
        spans_truncated: bool,
    ) -> Self {
        Self::OccurrenceMismatch {
            sketch,
            expected,
            actual,
            spans,
            spans_truncated,
        }
    }

    fn compare(left: &Self, right: &Self) -> Ordering {
        left.location()
            .cmp(right.location())
            .then_with(|| left.kind_rank().cmp(&right.kind_rank()))
    }

    fn location(&self) -> &SketchLocation {
        match self {
            Self::MissingFile { sketch }
            | Self::NotMatched { sketch, .. }
            | Self::OccurrenceMismatch { sketch, .. } => sketch,
        }
    }

    pub(crate) fn contract_file(&self) -> &CatalogPath {
        &self.location().contract_file
    }

    fn kind_rank(&self) -> u8 {
        match self {
            Self::MissingFile { .. } => 0,
            Self::NotMatched { .. } => 1,
            Self::OccurrenceMismatch { .. } => 2,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SketchInventoryComparison {
    counts: SketchCheckCounts,
    diagnostics: Vec<SketchDiagnostic>,
}

impl SketchInventoryComparison {
    pub(crate) fn new(
        source_catalog_entry_count: usize,
        referenced_source_file_count: usize,
        present_referenced_source_file_count: usize,
        contract_document_count: usize,
        sketch_count: usize,
        matched_sketch_count: usize,
        mut diagnostics: Vec<SketchDiagnostic>,
    ) -> Self {
        let failed_sketch_count = diagnostics.len();
        debug_assert!(present_referenced_source_file_count <= source_catalog_entry_count);
        debug_assert!(present_referenced_source_file_count <= referenced_source_file_count);
        debug_assert_eq!(
            matched_sketch_count.checked_add(failed_sketch_count),
            Some(sketch_count)
        );

        diagnostics.sort_by(SketchDiagnostic::compare);

        Self {
            counts: SketchCheckCounts {
                source_catalog_entry_count,
                referenced_source_file_count,
                present_referenced_source_file_count,
                contract_document_count,
                sketch_count,
                matched_sketch_count,
                failed_sketch_count,
            },
            diagnostics,
        }
    }

    pub(crate) fn diagnostics(&self) -> &[SketchDiagnostic] {
        &self.diagnostics
    }

    pub(crate) fn passed(&self, mode: CheckMode) -> bool {
        mode.passed(self.diagnostics())
    }

    pub(crate) fn into_response(self, mode: CheckMode) -> CheckResponse {
        let passed = self.passed(mode);
        let counts = self.counts;

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
    pub source_catalog_entry_count: usize,
    /// Number of unique logical source paths referenced by parsed sketches.
    pub referenced_source_file_count: usize,
    /// Number of unique referenced paths present in the supplied catalog.
    pub present_referenced_source_file_count: usize,
    /// Number of semantic YAML documents parsed across root contract files.
    pub contract_document_count: usize,
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

#[cfg(test)]
mod tests {
    use super::{
        DiagnosticExcerpt, MatchCandidate, SketchDiagnostic, SketchInventoryComparison,
        SketchLocation, SourceLineSpan,
    };
    use crate::api::CheckMode;
    use crate::contract::{SketchNormalization, SketchOccurrence};
    use crate::files::CatalogPath;

    #[test]
    fn comparison_counts_match_scope_and_diagnostics() {
        let comparison = SketchInventoryComparison::new(
            3,
            2,
            1,
            2,
            4,
            3,
            vec![SketchDiagnostic::not_matched(
                SketchLocation::new(
                    "b",
                    CatalogPath::new("contracts/b.yaml").expect("contract path"),
                    1,
                    CatalogPath::new("src/b.rs").expect("source path"),
                ),
                SketchNormalization::ExactLinesV1,
                None,
            )],
        );

        let response = comparison.into_response(CheckMode::Enforce);
        let counts = &response.counts;

        assert_eq!(counts.source_catalog_entry_count, 3);
        assert_eq!(counts.referenced_source_file_count, 2);
        assert_eq!(counts.present_referenced_source_file_count, 1);
        assert_eq!(counts.contract_document_count, 2);
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
            1,
            1,
            0,
            vec![SketchDiagnostic::not_matched(
                SketchLocation::new(
                    "a",
                    CatalogPath::new("contracts/a.yaml").expect("contract path"),
                    0,
                    CatalogPath::new("src/a.rs").expect("source path"),
                ),
                SketchNormalization::ExactLinesV1,
                None,
            )],
        );

        assert!(comparison.passed(CheckMode::Warning));
        assert!(!comparison.passed(CheckMode::Enforce));
    }

    #[test]
    fn into_response_preserves_diagnostics_but_applies_mode() {
        let response = SketchInventoryComparison::new(
            1,
            1,
            0,
            1,
            1,
            0,
            vec![SketchDiagnostic::missing_file(SketchLocation::new(
                "a",
                CatalogPath::new("contracts/a.yaml").expect("contract path"),
                0,
                CatalogPath::new("src/a.rs").expect("source path"),
            ))],
        )
        .into_response(CheckMode::Warning);

        assert!(response.passed);
        assert_eq!(response.diagnostics.len(), 1);
        assert!(response.report_files.is_empty());
    }

    #[test]
    fn diagnostics_sort_by_complete_location_then_variant_rank() {
        let same_location = SketchLocation::new(
            "a",
            CatalogPath::new("contracts/a.yaml").expect("contract path"),
            1,
            CatalogPath::new("src/a.rs").expect("source path"),
        );
        let later_document = SketchLocation::new(
            "a",
            CatalogPath::new("contracts/a.yaml").expect("contract path"),
            2,
            CatalogPath::new("src/a.rs").expect("source path"),
        );
        let later_identifier = SketchLocation::new(
            "z",
            CatalogPath::new("contracts/a.yaml").expect("contract path"),
            0,
            CatalogPath::new("src/z.rs").expect("source path"),
        );

        let comparison = SketchInventoryComparison::new(
            1,
            3,
            1,
            3,
            5,
            0,
            vec![
                SketchDiagnostic::missing_file(later_identifier.clone()),
                SketchDiagnostic::occurrence_mismatch(
                    same_location.clone(),
                    SketchOccurrence::ExactlyOne,
                    2,
                    vec![SourceLineSpan::new(3, 4), SourceLineSpan::new(8, 9)],
                    false,
                ),
                SketchDiagnostic::not_matched(
                    later_document.clone(),
                    SketchNormalization::ExactLinesV1,
                    None,
                ),
                SketchDiagnostic::not_matched(
                    same_location.clone(),
                    SketchNormalization::ExactLinesV1,
                    None,
                ),
                SketchDiagnostic::missing_file(same_location.clone()),
            ],
        );

        assert_eq!(
            comparison.diagnostics(),
            &[
                SketchDiagnostic::MissingFile {
                    sketch: same_location.clone(),
                },
                SketchDiagnostic::NotMatched {
                    sketch: same_location.clone(),
                    normalization: SketchNormalization::ExactLinesV1,
                    candidate: None,
                },
                SketchDiagnostic::OccurrenceMismatch {
                    sketch: same_location,
                    expected: SketchOccurrence::ExactlyOne,
                    actual: 2,
                    spans: vec![SourceLineSpan::new(3, 4), SourceLineSpan::new(8, 9)],
                    spans_truncated: false,
                },
                SketchDiagnostic::NotMatched {
                    sketch: later_document,
                    normalization: SketchNormalization::ExactLinesV1,
                    candidate: None,
                },
                SketchDiagnostic::MissingFile {
                    sketch: later_identifier,
                },
            ]
        );
    }

    #[test]
    fn rich_diagnostics_round_trip_with_locations_and_bounded_evidence() {
        let location = SketchLocation::new(
            "checkout",
            CatalogPath::new("contracts/api.yaml").expect("contract path"),
            3,
            CatalogPath::new("src/api.rs").expect("source path"),
        );
        let candidate = MatchCandidate::new(
            SourceLineSpan::new(41, 43),
            2,
            42,
            DiagnosticExcerpt::Bytes {
                escaped: "expected\\xff".to_owned(),
                truncated: true,
            },
            DiagnosticExcerpt::missing(),
        );
        let diagnostics = vec![
            SketchDiagnostic::missing_file(location.clone()),
            SketchDiagnostic::not_matched(
                location.clone(),
                SketchNormalization::ExactLinesV1,
                Some(candidate),
            ),
            SketchDiagnostic::occurrence_mismatch(
                location,
                SketchOccurrence::ExactlyOne,
                4,
                vec![SourceLineSpan::new(1, 2), SourceLineSpan::new(10, 11)],
                true,
            ),
        ];

        let bytes = serde_json::to_vec(&diagnostics).expect("serialize diagnostics");
        let round_trip = serde_json::from_slice::<Vec<SketchDiagnostic>>(&bytes)
            .expect("deserialize diagnostics");

        assert_eq!(round_trip, diagnostics);
        assert!(matches!(
            &round_trip[1],
            SketchDiagnostic::NotMatched {
                sketch,
                normalization: SketchNormalization::ExactLinesV1,
                candidate: Some(MatchCandidate {
                    source: SourceLineSpan { start: 41, end: 43 },
                    expected_line: 2,
                    source_line: 42,
                    expected: DiagnosticExcerpt::Bytes {
                        escaped,
                        truncated: true,
                    },
                    actual: DiagnosticExcerpt::Missing,
                }),
            } if sketch.document_index == 3
                && sketch.contract_file.as_str() == "contracts/api.yaml"
                && sketch.source_file.as_str() == "src/api.rs"
                && escaped == "expected\\xff"
        ));
    }

    #[test]
    fn diagnostic_excerpts_escape_arbitrary_bytes_before_serialization() {
        let excerpt = DiagnosticExcerpt::Bytes {
            escaped: "a\\n\\xff".to_owned(),
            truncated: true,
        };

        assert_eq!(
            excerpt,
            DiagnosticExcerpt::Bytes {
                escaped: "a\\n\\xff".to_owned(),
                truncated: true,
            }
        );
    }

    #[test]
    fn source_line_spans_are_one_based_and_inclusive() {
        let span = SourceLineSpan::new(1, 3);

        assert_eq!(span.start, 1);
        assert_eq!(span.end, 3);
    }
}
