use super::{CatalogLimits, LimitExceeded, LimitResource};
use crate::files::CatalogPath;
use serde::{Deserialize, Serialize};

/// Sketch identity, normalization, matching-work, and retained-evidence budgets.
///
/// Line comparisons and encountered occurrence candidates accumulate across
/// every source group and sketch in one complete check. Crossing either ceiling
/// is a hard [`LimitExceeded`] operation failure rather than a truncated search
/// or a false match result. `retained_occurrence_spans` is different: matching
/// still computes the exact occurrence count and truncates only the returned
/// span vector.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MatchingLimits {
    /// Maximum parsed sketch contracts.
    pub sketches: u64,
    /// Maximum minimal signature-index entries parsed across contract documents.
    pub signature_index_entries: u64,
    /// Maximum UTF-8 byte length of one exact sketch ID.
    pub sketch_id_bytes: u64,
    /// Maximum source bytes in one contract snippet or generation seed.
    pub snippet_bytes: u64,
    /// Maximum normalized lines in one contract snippet or generation seed.
    pub snippet_lines: u64,
    /// Maximum bytes in one normalized referenced source.
    pub normalized_source_bytes: u64,
    /// Maximum lines in one normalized referenced source.
    pub normalized_source_lines: u64,
    /// Maximum exact line comparisons across one complete check operation.
    pub line_comparisons: u64,
    /// Maximum exact occurrences encountered across one complete check operation.
    pub occurrence_candidates: u64,
    /// Maximum occurrence spans retained as diagnostic evidence.
    ///
    /// This does not cap occurrence counting. Additional spans set
    /// [`SketchDiagnostic::OccurrenceMismatch::spans_truncated`](crate::SketchDiagnostic::OccurrenceMismatch)
    /// while `actual` remains exact.
    pub retained_occurrence_spans: u64,
}

impl Default for MatchingLimits {
    fn default() -> Self {
        Self {
            sketches: 500_000,
            signature_index_entries: 500_000,
            sketch_id_bytes: 256,
            snippet_bytes: 1024 * 1024,
            snippet_lines: 100_000,
            normalized_source_bytes: 64 * 1024 * 1024,
            normalized_source_lines: 1_000_000,
            line_comparisons: 10_000_000,
            occurrence_candidates: 1_000_000,
            retained_occurrence_spans: 32,
        }
    }
}

impl MatchingLimits {
    pub(crate) fn validate_sketch_count(
        &self,
        count: usize,
        file: Option<CatalogPath>,
    ) -> Result<(), LimitExceeded> {
        self.validate(LimitResource::SketchCount, self.sketches, count, file)
    }

    pub(crate) fn validate_signature_index_entry_count(
        &self,
        count: usize,
        file: Option<CatalogPath>,
    ) -> Result<(), LimitExceeded> {
        self.validate(
            LimitResource::SignatureIndexEntryCount,
            self.signature_index_entries,
            count,
            file,
        )
    }

    pub(crate) fn sketch_id_maximum(&self) -> usize {
        CatalogLimits::parser_limit(self.sketch_id_bytes)
    }

    pub(crate) fn retained_span_maximum(&self) -> usize {
        CatalogLimits::parser_limit(self.retained_occurrence_spans)
    }

    pub(crate) fn usage(&self) -> MatchingUsage<'_> {
        MatchingUsage {
            limits: self,
            line_comparisons: 0,
            occurrence_candidates: 0,
        }
    }

    fn validate(
        &self,
        resource: LimitResource,
        limit: u64,
        observed: usize,
        file: Option<CatalogPath>,
    ) -> Result<(), LimitExceeded> {
        let observed_at_least = CatalogLimits::observed(observed);
        if observed_at_least > limit {
            return Err(LimitExceeded::new(resource, limit, observed_at_least, file));
        }
        Ok(())
    }
}

pub(crate) struct MatchingUsage<'limits> {
    limits: &'limits MatchingLimits,
    line_comparisons: u64,
    occurrence_candidates: u64,
}

impl MatchingUsage<'_> {
    pub(crate) fn record_line_comparison(
        &mut self,
        file: &CatalogPath,
    ) -> Result<(), LimitExceeded> {
        self.line_comparisons = self.line_comparisons.saturating_add(1);
        self.validate(
            LimitResource::MatchingLineComparisons,
            self.limits.line_comparisons,
            self.line_comparisons,
            file,
        )
    }

    pub(crate) fn record_occurrence_candidate(
        &mut self,
        file: &CatalogPath,
    ) -> Result<(), LimitExceeded> {
        self.occurrence_candidates = self.occurrence_candidates.saturating_add(1);
        self.validate(
            LimitResource::OccurrenceCandidateCount,
            self.limits.occurrence_candidates,
            self.occurrence_candidates,
            file,
        )
    }

    fn validate(
        &self,
        resource: LimitResource,
        limit: u64,
        observed_at_least: u64,
        file: &CatalogPath,
    ) -> Result<(), LimitExceeded> {
        if observed_at_least > limit {
            return Err(LimitExceeded::new(
                resource,
                limit,
                observed_at_least,
                Some(file.clone()),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::MatchingLimits;
    use crate::files::CatalogPath;
    use crate::limits::LimitResource;

    #[test]
    fn matching_limits_cover_sketch_count_budget() {
        let path = CatalogPath::new("main.yml").expect("path");
        let limits = MatchingLimits {
            sketches: 1,
            ..MatchingLimits::default()
        };

        assert_eq!(
            limits
                .validate_sketch_count(2, Some(path.clone()))
                .expect_err("sketch count")
                .resource,
            LimitResource::SketchCount
        );

        let limits = MatchingLimits {
            signature_index_entries: 1,
            ..MatchingLimits::default()
        };
        let error = limits
            .validate_signature_index_entry_count(2, Some(path.clone()))
            .expect_err("signature index count");
        assert_eq!(error.resource, LimitResource::SignatureIndexEntryCount);
        assert_eq!(error.observed_at_least, 2);
        assert_eq!(error.file.as_ref(), Some(&path));
    }

    #[test]
    fn matching_work_limits_stop_at_the_first_excess_comparison_and_occurrence() {
        let path = CatalogPath::new("source.rs").expect("source path");
        let comparison_limits = MatchingLimits {
            line_comparisons: 0,
            ..MatchingLimits::default()
        };
        let comparison = comparison_limits
            .usage()
            .record_line_comparison(&path)
            .expect_err("first comparison must exceed a zero budget");
        assert_eq!(comparison.resource, LimitResource::MatchingLineComparisons);
        assert_eq!(comparison.observed_at_least, 1);
        assert_eq!(comparison.file.as_ref(), Some(&path));

        let occurrence_limits = MatchingLimits {
            occurrence_candidates: 0,
            ..MatchingLimits::default()
        };
        let occurrence = occurrence_limits
            .usage()
            .record_occurrence_candidate(&path)
            .expect_err("first occurrence must exceed a zero budget");
        assert_eq!(occurrence.resource, LimitResource::OccurrenceCandidateCount);
        assert_eq!(occurrence.observed_at_least, 1);
        assert_eq!(occurrence.file.as_ref(), Some(&path));
    }
}
