use crate::contract::SketchNormalization;
use crate::error::SketchContractKitError;
use crate::files::CatalogPath;
use crate::limits::{CatalogLimits, LimitExceeded, LimitResource, MatchingLimits, MatchingUsage};
use crate::work::CancellationProbe;
use std::ops::Range;

impl SketchNormalization {
    pub(crate) fn normalize_snippet(
        self,
        bytes: &[u8],
        limits: &MatchingLimits,
        contract_file: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<NormalizedSnippet, SketchContractKitError> {
        let budget = NormalizationBudget::snippet(limits, contract_file);
        budget.validate_raw_snippet(bytes.len())?;
        match self {
            Self::ExactLinesV1 => NormalizedSnippet::from_bytes(bytes, &budget, cancellation),
        }
    }

    pub(crate) fn normalize_source(
        self,
        bytes: &[u8],
        limits: &MatchingLimits,
        source_file: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<NormalizedSnippet, SketchContractKitError> {
        let budget = NormalizationBudget::source(limits, source_file);
        match self {
            Self::ExactLinesV1 => NormalizedSnippet::from_bytes(bytes, &budget, cancellation),
        }
    }
}

struct NormalizationBudget<'limits> {
    byte_limit: u64,
    line_limit: u64,
    byte_resource: LimitResource,
    line_resource: LimitResource,
    file: &'limits CatalogPath,
}

impl<'limits> NormalizationBudget<'limits> {
    fn snippet(limits: &MatchingLimits, file: &'limits CatalogPath) -> Self {
        Self {
            byte_limit: limits.snippet_bytes,
            line_limit: limits.snippet_lines,
            byte_resource: LimitResource::SnippetBytes,
            line_resource: LimitResource::SnippetLines,
            file,
        }
    }

    fn source(limits: &MatchingLimits, file: &'limits CatalogPath) -> Self {
        Self {
            byte_limit: limits.normalized_source_bytes,
            line_limit: limits.normalized_source_lines,
            byte_resource: LimitResource::NormalizedSourceBytes,
            line_resource: LimitResource::NormalizedSourceLines,
            file,
        }
    }

    fn validate_raw_snippet(&self, bytes: usize) -> Result<(), LimitExceeded> {
        self.validate(self.byte_resource, self.byte_limit, bytes)
    }

    fn validate_normalized_byte(&self, bytes: usize) -> Result<(), LimitExceeded> {
        self.validate(self.byte_resource, self.byte_limit, bytes)
    }

    fn validate_line(&self, lines: usize) -> Result<(), LimitExceeded> {
        self.validate(self.line_resource, self.line_limit, lines)
    }

    fn validate(
        &self,
        resource: LimitResource,
        limit: u64,
        observed: usize,
    ) -> Result<(), LimitExceeded> {
        let observed_at_least = CatalogLimits::observed(observed);
        if observed_at_least > limit {
            return Err(LimitExceeded::new(
                resource,
                limit,
                observed_at_least,
                Some(self.file.clone()),
            ));
        }
        Ok(())
    }

    fn byte_capacity(&self, input_bytes: usize) -> usize {
        let bounded = self.byte_limit.saturating_add(1);
        input_bytes.min(usize::try_from(bounded).unwrap_or(usize::MAX))
    }
}

#[derive(Debug)]
pub(crate) struct NormalizedSnippet {
    bytes: Vec<u8>,
    lines: Vec<Range<usize>>,
}

impl NormalizedSnippet {
    fn from_bytes(
        bytes: &[u8],
        budget: &NormalizationBudget<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SketchContractKitError> {
        let mut normalized = Vec::with_capacity(budget.byte_capacity(bytes.len()));
        let mut lines = Vec::new();
        let mut line_count = 0_usize;
        let mut line_start = 0;
        let mut index = 0;
        let mut processed = 0_usize;

        while index < bytes.len() {
            cancellation.checkpoint_at(processed)?;
            processed = processed.saturating_add(1);
            let (byte, consumed) = if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n')
            {
                (b'\n', 2)
            } else {
                (bytes[index], 1)
            };
            if byte == b'\n' && index.saturating_add(consumed) == bytes.len() {
                break;
            }

            let next_bytes = normalized.len().saturating_add(1);
            budget.validate_normalized_byte(next_bytes)?;
            if line_count == 0 {
                line_count = 1;
                budget.validate_line(line_count)?;
            }
            if byte == b'\n' {
                let next_lines = line_count.saturating_add(1);
                budget.validate_line(next_lines)?;
                line_count = next_lines;
                lines.push(line_start..normalized.len());
                normalized.push(byte);
                line_start = normalized.len();
            } else {
                normalized.push(byte);
            }
            index += consumed;
        }

        lines.push(line_start..normalized.len());

        cancellation.checkpoint()?;
        Ok(Self {
            bytes: normalized,
            lines,
        })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.line(&self.lines[0]).is_empty()
    }

    pub(crate) fn line_count(&self) -> usize {
        if self.is_empty() { 0 } else { self.lines.len() }
    }

    pub(crate) fn line_at(&self, index: usize) -> Option<&[u8]> {
        self.lines.get(index).map(|range| self.line(range))
    }

    pub(crate) fn lines(&self) -> impl Iterator<Item = &[u8]> {
        self.lines
            .iter()
            .take(self.line_count())
            .map(|range| self.line(range))
    }

    pub(crate) fn matches_at(
        &self,
        source: &Self,
        source_start: usize,
        source_file: &CatalogPath,
        usage: &mut MatchingUsage<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<bool, SketchContractKitError> {
        let expected_line_count = self.line_count();
        let source_line_count = source.line_count();

        if expected_line_count == 0
            || source_start > source_line_count
            || expected_line_count > source_line_count.saturating_sub(source_start)
        {
            return Ok(false);
        }

        for expected_index in 0..expected_line_count {
            cancellation.checkpoint_at(expected_index)?;
            usage.record_line_comparison(source_file)?;
            if self.line_at(expected_index) != source.line_at(source_start + expected_index) {
                return Ok(false);
            }
        }

        Ok(true)
    }

    fn line(&self, range: &Range<usize>) -> &[u8] {
        &self.bytes[range.clone()]
    }

    fn matches_lines(&self, source: &Self, source_lines: &[Range<usize>]) -> bool {
        self.lines.len() == source_lines.len()
            && self
                .lines
                .iter()
                .zip(source_lines)
                .all(|(expected, actual)| self.line(expected) == source.line(actual))
    }
}

impl PartialEq for NormalizedSnippet {
    fn eq(&self, other: &Self) -> bool {
        self.matches_lines(other, &other.lines)
    }
}

impl Eq for NormalizedSnippet {}

#[cfg(test)]
mod tests {
    use super::NormalizedSnippet as Normalized;
    use crate::contract::SketchNormalization;
    use crate::error::SketchContractKitError;
    use crate::files::CatalogPath;
    use crate::limits::{LimitResource, MatchingLimits};
    use crate::work::CancellationProbe;
    use proptest::prelude::*;

    struct NormalizedSnippet;

    impl NormalizedSnippet {
        fn from_bytes(bytes: &[u8]) -> Normalized {
            SketchNormalization::ExactLinesV1
                .normalize_snippet(
                    bytes,
                    &MatchingLimits::default(),
                    &CatalogPath::new("contract.yml").expect("contract path"),
                    &CancellationProbe::new(),
                )
                .expect("test normalization")
        }

        fn reference_from_bytes(bytes: &[u8]) -> Normalized {
            let mut normalized = Vec::with_capacity(bytes.len());
            let mut index = 0;
            while index < bytes.len() {
                let (byte, consumed) =
                    if bytes[index] == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
                        (b'\n', 2)
                    } else {
                        (bytes[index], 1)
                    };
                if byte == b'\n' && index.saturating_add(consumed) == bytes.len() {
                    break;
                }
                normalized.push(byte);
                index += consumed;
            }

            let mut lines = Vec::new();
            let mut line_start = 0;
            for (index, byte) in normalized.iter().enumerate() {
                if *byte == b'\n' {
                    lines.push(line_start..index);
                    line_start = index + 1;
                }
            }
            lines.push(line_start..normalized.len());

            Normalized {
                bytes: normalized,
                lines,
            }
        }

        fn with_line_endings(lines: &[Vec<u8>], line_ending: &[u8]) -> Vec<u8> {
            let payload_bytes = lines.iter().map(Vec::len).sum::<usize>();
            let line_ending_bytes = line_ending
                .len()
                .saturating_mul(lines.len().saturating_sub(1));
            let mut source = Vec::with_capacity(payload_bytes.saturating_add(line_ending_bytes));

            for (index, line) in lines.iter().enumerate() {
                if index > 0 {
                    source.extend_from_slice(line_ending);
                }
                source.extend_from_slice(line);
            }

            source
        }

        fn canonical_raw_bytes(normalized: &Normalized) -> Vec<u8> {
            let mut bytes = Vec::with_capacity(
                normalized
                    .bytes
                    .len()
                    .saturating_add(normalized.line_count().max(1).saturating_mul(2)),
            );
            if normalized.is_empty() {
                bytes.extend_from_slice(b"\r\n");
                return bytes;
            }

            for line in normalized.lines() {
                bytes.extend_from_slice(line);
                bytes.extend_from_slice(b"\r\n");
            }
            bytes
        }

        fn limit(error: SketchContractKitError) -> crate::limits::LimitExceeded {
            error
                .limit_exceeded()
                .cloned()
                .expect("typed normalization limit")
        }

        fn matches_at(
            expected: &Normalized,
            source: &Normalized,
            source_start: usize,
        ) -> Result<bool, SketchContractKitError> {
            let limits = MatchingLimits::default();
            let mut usage = limits.usage();
            expected.matches_at(
                source,
                source_start,
                &CatalogPath::new("source.bin").expect("source path"),
                &mut usage,
                &CancellationProbe::new(),
            )
        }
    }

    #[test]
    fn exact_lines_distinguish_internal_literal_and_comment_spacing() {
        for (expected, changed) in [
            (r#"let message = "a  b";"#, r#"let message = "a b";"#),
            ("// two  spaces", "// two spaces"),
            (r##"let value = r#"a  b"#;"##, r##"let value = r#"a b"#;"##),
        ] {
            assert_ne!(
                NormalizedSnippet::from_bytes(expected.as_bytes()),
                NormalizedSnippet::from_bytes(changed.as_bytes()),
                "spacing must remain semantic for {expected:?}"
            );
        }
    }

    #[test]
    fn exact_lines_distinguish_indentation_and_tabs() {
        for (expected, changed) in [
            ("if ready:\n    execute()", "if ready:\nexecute()"),
            ("parent:\n  child: value", "parent:\nchild: value"),
            ("target:\n\tcommand", "target:\ncommand"),
            ("let\tvalue = 42;", "let value = 42;"),
        ] {
            assert_ne!(
                NormalizedSnippet::from_bytes(expected.as_bytes()),
                NormalizedSnippet::from_bytes(changed.as_bytes()),
                "indentation and tabs must remain semantic for {expected:?}"
            );
        }
    }

    #[test]
    fn exact_lines_preserve_multiline_content_and_internal_blank_lines() {
        for (expected, changed) in [
            (
                "message = \"\"\"first\n\nthird\"\"\"",
                "message = \"\"\"first\nthird\"\"\"",
            ),
            (
                "cat <<EOF\nfirst\n\nthird\nEOF",
                "cat <<EOF\nfirst\nthird\nEOF",
            ),
            (
                "BEGIN\nkey=one\n\nkey=two\nEND",
                "BEGIN\nkey=one\nkey=two\nEND",
            ),
        ] {
            assert_ne!(
                NormalizedSnippet::from_bytes(expected.as_bytes()),
                NormalizedSnippet::from_bytes(changed.as_bytes()),
                "internal blank lines must remain semantic for {expected:?}"
            );
        }
    }

    #[test]
    fn exact_lines_preserve_leading_internal_and_trailing_horizontal_bytes() {
        for (expected, changed) in [
            (" value", "value"),
            ("value ", "value"),
            ("value  here", "value here"),
            ("\tvalue\t", "value"),
        ] {
            assert_ne!(
                NormalizedSnippet::from_bytes(expected.as_bytes()),
                NormalizedSnippet::from_bytes(changed.as_bytes()),
                "horizontal bytes must remain semantic for {expected:?}"
            );
        }
    }

    #[test]
    fn crlf_and_lf_normalize_the_same() {
        let crlf = NormalizedSnippet::from_bytes(b"fn answer() {\r\n    todo!()\r\n}\r\n");
        let lf = NormalizedSnippet::from_bytes(b"fn answer() {\n    todo!()\n}\n");

        assert_eq!(crlf, lf);
    }

    #[test]
    fn isolated_carriage_returns_remain_semantic() {
        let isolated = NormalizedSnippet::from_bytes(b"first\rsecond\n");
        let removed = NormalizedSnippet::from_bytes(b"firstsecond\n");

        assert_ne!(isolated, removed);
        assert_eq!(isolated.bytes, b"first\rsecond");
    }

    #[test]
    fn carriage_return_immediately_before_crlf_remains_in_the_preceding_line() {
        let retained = NormalizedSnippet::from_bytes(b"first\r\r\nsecond");
        let ordinary_crlf = NormalizedSnippet::from_bytes(b"first\r\nsecond");
        let isolated_only = NormalizedSnippet::from_bytes(b"first\rsecond");

        assert_ne!(retained, ordinary_crlf);
        assert_eq!(retained.line_at(0), Some(b"first\r".as_slice()));
        assert_eq!(retained.line_at(1), Some(b"second".as_slice()));
        assert_eq!(isolated_only.line_at(0), Some(b"first\rsecond".as_slice()));
    }

    #[test]
    fn exactly_one_final_line_terminator_is_nonsemantic() {
        let without_terminator = NormalizedSnippet::from_bytes(b"first\nsecond");
        let with_lf = NormalizedSnippet::from_bytes(b"first\nsecond\n");
        let with_crlf = NormalizedSnippet::from_bytes(b"first\r\nsecond\r\n");

        assert_eq!(without_terminator, with_lf);
        assert_eq!(without_terminator, with_crlf);
        assert_eq!(without_terminator.bytes, with_lf.bytes);
        assert_eq!(without_terminator.bytes, with_crlf.bytes);
        assert_eq!(
            NormalizedSnippet::from_bytes(b""),
            NormalizedSnippet::from_bytes(b"\n")
        );
        assert!(NormalizedSnippet::from_bytes(b"\n").is_empty());
    }

    #[test]
    fn additional_final_blank_lines_remain_semantic() {
        let one_terminator = NormalizedSnippet::from_bytes(b"value\n");
        let final_blank_line = NormalizedSnippet::from_bytes(b"value\n\n");
        let two_final_blank_lines = NormalizedSnippet::from_bytes(b"value\n\n\n");

        assert_ne!(one_terminator, final_blank_line);
        assert_ne!(final_blank_line, two_final_blank_lines);
        assert_eq!(final_blank_line.lines.len(), 2);
        assert_eq!(two_final_blank_lines.lines.len(), 3);
    }

    #[test]
    fn arbitrary_invalid_bytes_are_preserved_without_switching_whitespace_policy() {
        let bytes = b"\xC2\xA0\tleft\xFFright\xE2\x80\x83  \r\nnext\n";
        let normalized = NormalizedSnippet::from_bytes(bytes);

        assert_eq!(
            normalized.bytes,
            b"\xC2\xA0\tleft\xFFright\xE2\x80\x83  \nnext"
        );
        assert_ne!(
            normalized,
            NormalizedSnippet::from_bytes(b" \tleft\xFFright   \nnext\n")
        );
    }

    #[test]
    fn normalization_is_idempotent_for_fixed_arbitrary_byte_fixtures() {
        for bytes in [
            b"".as_slice(),
            b"value\n",
            b"value\n\n",
            b"first\r\nsecond\r\n",
            b"first\r\r\nsecond",
            b"\xFF\t\xC2\xA0\r\n\n",
        ] {
            let once = NormalizedSnippet::from_bytes(bytes);
            let canonical = NormalizedSnippet::canonical_raw_bytes(&once);
            let twice = NormalizedSnippet::from_bytes(&canonical);

            assert_eq!(
                once, twice,
                "normalization must be idempotent for {bytes:?}"
            );
        }
    }

    proptest! {
        #[test]
        fn exact_line_normalization_is_idempotent_for_arbitrary_bytes(
            bytes in prop::collection::vec(any::<u8>(), 0..4096),
        ) {
            let once = NormalizedSnippet::from_bytes(&bytes);
            let reference = NormalizedSnippet::reference_from_bytes(&bytes);
            prop_assert_eq!(&once.bytes, &reference.bytes);
            prop_assert_eq!(&once.lines, &reference.lines);
            let canonical = NormalizedSnippet::canonical_raw_bytes(&once);
            let twice = NormalizedSnippet::from_bytes(&canonical);

            prop_assert_eq!(once, twice);
        }

        #[test]
        fn logical_lines_have_equivalent_lf_and_crlf_spellings(
            lines in prop::collection::vec(
                prop::collection::vec(
                    any::<u8>().prop_filter("line feed excluded from payload", |byte| *byte != b'\n'),
                    0..128,
                )
                .prop_filter(
                    "payload does not end with an ambiguous carriage return",
                    |line| line.last() != Some(&b'\r'),
                ),
                1..64,
            ),
        ) {
            let lf = NormalizedSnippet::with_line_endings(&lines, b"\n");
            let crlf = NormalizedSnippet::with_line_endings(&lines, b"\r\n");

            prop_assert_eq!(
                NormalizedSnippet::from_bytes(&lf),
                NormalizedSnippet::from_bytes(&crlf),
            );
        }

        #[test]
        fn bytes_without_line_feeds_are_preserved_exactly(
            bytes in prop::collection::vec(
                any::<u8>().prop_filter("line feed excluded", |byte| *byte != b'\n'),
                0..4096,
            ),
        ) {
            let normalized = NormalizedSnippet::from_bytes(&bytes);

            prop_assert_eq!(normalized.line_at(0), Some(bytes.as_slice()));
        }

        #[test]
        fn only_the_final_carriage_return_in_a_run_forms_crlf(
            carriage_returns in 1_usize..128,
        ) {
            let mut source = b"prefix".to_vec();
            source.extend(std::iter::repeat_n(b'\r', carriage_returns));
            source.extend_from_slice(b"\nsuffix");
            let mut expected_first_line = b"prefix".to_vec();
            expected_first_line.extend(std::iter::repeat_n(
                b'\r',
                carriage_returns.saturating_sub(1),
            ));
            let normalized = NormalizedSnippet::from_bytes(&source);

            prop_assert_eq!(normalized.line_at(0), Some(expected_first_line.as_slice()));
            prop_assert_eq!(normalized.line_at(1), Some(b"suffix".as_slice()));
        }

        #[test]
        fn exact_line_window_matching_never_panics_for_arbitrary_bytes(
            expected in prop::collection::vec(any::<u8>(), 0..2048),
            source in prop::collection::vec(any::<u8>(), 0..4096),
            source_start in any::<usize>(),
        ) {
            let expected = NormalizedSnippet::from_bytes(&expected);
            let source = NormalizedSnippet::from_bytes(&source);

            let _ = NormalizedSnippet::matches_at(&expected, &source, source_start);
        }
    }

    #[test]
    fn policy_dispatch_uses_exact_line_normalization() {
        let through_policy = SketchNormalization::ExactLinesV1
            .normalize_source(
                b"value\r\n",
                &MatchingLimits::default(),
                &CatalogPath::new("source.bin").expect("source path"),
                &CancellationProbe::new(),
            )
            .expect("normalization within limits");
        let exact = NormalizedSnippet::from_bytes(b"value\n");

        assert_eq!(through_policy, exact);
    }

    #[test]
    fn normalization_stops_at_configured_byte_and_line_crossings() {
        let contract = CatalogPath::new("contract.yml").expect("contract path");
        let source = CatalogPath::new("source.bin").expect("source path");

        let snippet_bytes = MatchingLimits {
            snippet_bytes: 1,
            ..MatchingLimits::default()
        };
        let error = SketchNormalization::ExactLinesV1
            .normalize_snippet(b"ab", &snippet_bytes, &contract, &CancellationProbe::new())
            .expect_err("raw snippet bytes must be rejected before normalization");
        let error = NormalizedSnippet::limit(error);
        assert_eq!(error.resource, LimitResource::SnippetBytes);
        assert_eq!(error.observed_at_least, 2);

        let snippet_lines = MatchingLimits {
            snippet_lines: 1,
            ..MatchingLimits::default()
        };
        let error = SketchNormalization::ExactLinesV1
            .normalize_snippet(
                b"a\nb",
                &snippet_lines,
                &contract,
                &CancellationProbe::new(),
            )
            .expect_err("snippet lines must stop at the first crossing");
        let error = NormalizedSnippet::limit(error);
        assert_eq!(error.resource, LimitResource::SnippetLines);
        assert_eq!(error.observed_at_least, 2);

        let source_bytes = MatchingLimits {
            normalized_source_bytes: 1,
            ..MatchingLimits::default()
        };
        let error = SketchNormalization::ExactLinesV1
            .normalize_source(b"ab", &source_bytes, &source, &CancellationProbe::new())
            .expect_err("normalized source bytes must stop at the first crossing");
        let error = NormalizedSnippet::limit(error);
        assert_eq!(error.resource, LimitResource::NormalizedSourceBytes);
        assert_eq!(error.observed_at_least, 2);

        let source_lines = MatchingLimits {
            normalized_source_lines: 1,
            ..MatchingLimits::default()
        };
        let error = SketchNormalization::ExactLinesV1
            .normalize_source(b"a\nb", &source_lines, &source, &CancellationProbe::new())
            .expect_err("normalized source lines must stop at the first crossing");
        let error = NormalizedSnippet::limit(error);
        assert_eq!(error.resource, LimitResource::NormalizedSourceLines);
        assert_eq!(error.observed_at_least, 2);

        let normalized = SketchNormalization::ExactLinesV1
            .normalize_source(b"a\r\n", &source_bytes, &source, &CancellationProbe::new())
            .expect("raw CRLF bytes may exceed a normalized-byte limit");
        assert_eq!(normalized.line_at(0), Some(b"a".as_slice()));
    }

    #[test]
    fn cancelled_normalization_returns_the_typed_operation_error() {
        let cancellation = CancellationProbe::new();
        cancellation.cancel();

        let error = SketchNormalization::ExactLinesV1
            .normalize_source(
                &[b'x'; 8_192],
                &MatchingLimits::default(),
                &CatalogPath::new("source.bin").expect("source path"),
                &cancellation,
            )
            .expect_err("cancelled normalization must stop at a real byte-loop checkpoint");

        assert!(error.to_string().contains("cancelled"), "{error}");
    }

    #[test]
    fn exact_line_sequence_matching_preserves_order_and_contiguity() {
        let expected = NormalizedSnippet::from_bytes(b"let first = 1;\nlet second = 2;");
        let reordered = NormalizedSnippet::from_bytes(b"let second = 2;\nlet first = 1;");
        let interrupted =
            NormalizedSnippet::from_bytes(b"let first = 1;\nlet inserted = 99;\nlet second = 2;");

        assert!(!NormalizedSnippet::matches_at(&expected, &reordered, 0).expect("matching"));
        assert!(!NormalizedSnippet::matches_at(&expected, &interrupted, 0).expect("matching"));
    }

    #[test]
    fn source_can_contain_snippet_inside_larger_file() {
        let expected = NormalizedSnippet::from_bytes(b"let value = 42;\nvalue");
        let source = NormalizedSnippet::from_bytes(
            b"fn answer() -> u8 {\nlet value = 42;\nvalue\n}\nfn other() {}",
        );

        assert!(NormalizedSnippet::matches_at(&expected, &source, 1).expect("matching"));
    }

    #[test]
    fn empty_snippet_never_matches() {
        let expected = NormalizedSnippet::from_bytes(b"");
        let source = NormalizedSnippet::from_bytes(b"fn answer() -> u8 { 42 }");

        assert!(expected.is_empty());
        assert!(!NormalizedSnippet::matches_at(&expected, &source, 0).expect("matching"));
    }
}
