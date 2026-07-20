use crate::contract::{SketchContract, SketchContracts, SketchNormalization, SketchOccurrence};
use crate::error::SketchContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::inventory::{
    DiagnosticExcerpt, MatchCandidate, SketchDiagnostic, SketchInventoryComparison, SketchLocation,
    SourceLineSpan,
};
use crate::limits::{
    DiagnosticBytes, DiagnosticLimits, DiagnosticReservation, MatchingLimits, MatchingUsage,
    SketchLimits,
};
use crate::normalize::NormalizedSnippet;
use crate::work::CancellationProbe;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct SourceCatalog {
    files: BTreeMap<CatalogPath, Vec<u8>>,
    source_catalog_entry_count: usize,
    referenced_source_file_count: usize,
}

impl SourceCatalog {
    pub(crate) fn from_catalog(
        catalog: FileCatalog,
        contracts: &SketchContracts,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SketchContractKitError> {
        let source_catalog_entry_count = catalog.len();
        let selection = SourceSelection::from_contracts(contracts, cancellation)?;
        let referenced_source_file_count = selection.len();
        let mut files = BTreeMap::new();
        for (path, bytes) in catalog.into_entries() {
            cancellation.checkpoint()?;
            if selection.contains(&path) {
                files.insert(path, bytes);
            }
        }

        Ok(Self {
            files,
            source_catalog_entry_count,
            referenced_source_file_count,
        })
    }

    pub(crate) fn catalog_entry_count(&self) -> usize {
        self.source_catalog_entry_count
    }

    pub(crate) fn referenced_file_count(&self) -> usize {
        self.referenced_source_file_count
    }

    pub(crate) fn present_referenced_file_count(&self) -> usize {
        self.files.len()
    }

    fn groups<'a>(
        &'a self,
        contracts: &'a SketchContracts,
        cancellation: &CancellationProbe,
    ) -> Result<Vec<SourceGroup<'a>>, SketchContractKitError> {
        let mut grouped =
            BTreeMap::<(CatalogPath, SketchNormalization), Vec<&SketchContract>>::new();

        for contract in contracts.entries() {
            cancellation.checkpoint()?;
            grouped
                .entry((contract.file().clone(), contract.normalization()))
                .or_default()
                .push(contract);
        }

        let mut groups = Vec::with_capacity(grouped.len());
        for ((source_file, normalization), sketches) in grouped {
            cancellation.checkpoint()?;
            groups.push(SourceGroup {
                source: self.files.get(&source_file).map(Vec::as_slice),
                source_file,
                normalization,
                sketches,
            });
        }
        Ok(groups)
    }
}

struct SourceGroup<'a> {
    source_file: CatalogPath,
    normalization: SketchNormalization,
    source: Option<&'a [u8]>,
    sketches: Vec<&'a SketchContract>,
}

impl SourceGroup<'_> {
    fn append_diagnostics(
        self,
        matching: &MatchingLimits,
        diagnostics: &DiagnosticLimits,
        cancellation: &CancellationProbe,
        usage: &mut MatchingUsage<'_>,
        output: &mut SketchDiagnostics,
    ) -> Result<usize, SketchContractKitError> {
        cancellation.checkpoint()?;
        let Some(source_bytes) = self.source else {
            for contract in &self.sketches {
                cancellation.checkpoint()?;
                output.push(
                    SketchDiagnostic::missing_file(self.location(contract)),
                    diagnostics,
                )?;
            }
            return Ok(0);
        };
        let source = self.normalization.normalize_source(
            source_bytes,
            matching,
            &self.source_file,
            cancellation,
        )?;

        let mut matched_sketch_count = 0;
        for (index, contract) in self.sketches.iter().enumerate() {
            cancellation.checkpoint_at(index)?;
            let evaluation = self.evaluate(contract, &source, matching, usage, cancellation)?;
            let location = self.location(contract);
            match evaluation {
                MatchEvaluation::Satisfied => {
                    matched_sketch_count += 1;
                }
                MatchEvaluation::Missing => {
                    let mut scan = MatchScan::new(
                        contract.snippet().normalized(),
                        &source,
                        &self.source_file,
                        usage,
                        cancellation,
                    );
                    output.push_not_matched(
                        location,
                        self.normalization,
                        &mut scan,
                        diagnostics,
                    )?;
                }
                MatchEvaluation::OccurrenceMismatch {
                    actual,
                    spans,
                    spans_truncated,
                } => output.push(
                    SketchDiagnostic::occurrence_mismatch(
                        location,
                        contract.occurrence(),
                        actual,
                        spans,
                        spans_truncated,
                    ),
                    diagnostics,
                )?,
            }
        }
        Ok(matched_sketch_count)
    }

    fn evaluate(
        &self,
        contract: &SketchContract,
        source: &NormalizedSnippet,
        matching: &MatchingLimits,
        usage: &mut MatchingUsage<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<MatchEvaluation, SketchContractKitError> {
        MatchScan::new(
            contract.snippet().normalized(),
            source,
            &self.source_file,
            usage,
            cancellation,
        )
        .evaluate(contract.occurrence(), matching.retained_span_maximum())
    }

    fn location(&self, contract: &SketchContract) -> SketchLocation {
        SketchLocation::new(
            contract.id().as_str(),
            contract.contract_file().clone(),
            contract.document_index(),
            self.source_file.clone(),
        )
    }
}

struct SketchDiagnostics {
    entries: Vec<SketchDiagnostic>,
    bytes: DiagnosticBytes,
}

impl SketchDiagnostics {
    fn new(limits: &DiagnosticLimits) -> Result<Self, SketchContractKitError> {
        Ok(Self {
            entries: Vec::new(),
            bytes: DiagnosticBytes::new(limits)?,
        })
    }

    fn push(
        &mut self,
        diagnostic: SketchDiagnostic,
        limits: &DiagnosticLimits,
    ) -> Result<(), SketchContractKitError> {
        let reservation =
            self.bytes
                .reserve(&diagnostic, 0, limits, Some(diagnostic.contract_file()))?;
        self.push_reserved(diagnostic, reservation)
    }

    fn push_not_matched(
        &mut self,
        location: SketchLocation,
        normalization: SketchNormalization,
        scan: &mut MatchScan<'_, '_>,
        limits: &DiagnosticLimits,
    ) -> Result<(), SketchContractKitError> {
        self.bytes
            .preflight_count(limits, Some(&location.contract_file))?;
        let position = scan.best_candidate()?;
        let mut evidence = position.map(|position| {
            position.evidence(scan.expected, scan.source, limits.excerpt_maximum())
        });
        let skeleton = SketchDiagnostic::not_matched(
            location.clone(),
            normalization,
            evidence.as_ref().map(MatchCandidateEvidence::skeleton),
        );
        let reservation =
            self.bytes
                .reserve(&skeleton, 0, limits, Some(&location.contract_file))?;
        if let Some(evidence) = evidence.as_mut() {
            evidence.measure(scan.cancellation)?;
        }
        let additional_bytes = evidence
            .as_ref()
            .map_or(0, MatchCandidateEvidence::additional_json_bytes);
        let reservation = self.bytes.add_bytes(
            reservation,
            additional_bytes,
            limits,
            Some(&location.contract_file),
        )?;
        let candidate = evidence
            .map(|evidence| evidence.materialize(&location.contract_file, scan.cancellation))
            .transpose()?;
        self.push_reserved(
            SketchDiagnostic::not_matched(location, normalization, candidate),
            reservation,
        )
    }

    fn push_reserved(
        &mut self,
        diagnostic: SketchDiagnostic,
        reservation: DiagnosticReservation,
    ) -> Result<(), SketchContractKitError> {
        self.entries.try_reserve(1).map_err(|source| {
            SketchContractKitError::write_failed(diagnostic.contract_file(), source.to_string())
        })?;
        self.entries.push(diagnostic);
        self.bytes.commit(reservation);
        Ok(())
    }

    fn into_entries(self) -> Vec<SketchDiagnostic> {
        self.entries
    }
}

enum MatchEvaluation {
    Satisfied,
    Missing,
    OccurrenceMismatch {
        actual: usize,
        spans: Vec<SourceLineSpan>,
        spans_truncated: bool,
    },
}

#[derive(Clone, Copy)]
struct MatchCandidatePosition {
    source_start: usize,
    source_end: usize,
    first_difference: usize,
}

impl MatchCandidatePosition {
    fn evidence<'source>(
        self,
        expected: &'source NormalizedSnippet,
        source: &'source NormalizedSnippet,
        maximum: usize,
    ) -> MatchCandidateEvidence<'source> {
        let expected =
            DiagnosticExcerptEvidence::new(expected.line_at(self.first_difference), maximum);
        let actual = DiagnosticExcerptEvidence::new(
            source.line_at(self.source_start.saturating_add(self.first_difference)),
            maximum,
        );
        MatchCandidateEvidence {
            position: self,
            expected,
            actual,
        }
    }

    const fn source_span(self) -> SourceLineSpan {
        SourceLineSpan::new(self.source_start.saturating_add(1), self.source_end)
    }

    const fn expected_line(self) -> usize {
        self.first_difference.saturating_add(1)
    }

    const fn source_line(self) -> usize {
        self.source_start
            .saturating_add(self.first_difference)
            .saturating_add(1)
    }
}

struct MatchCandidateEvidence<'source> {
    position: MatchCandidatePosition,
    expected: DiagnosticExcerptEvidence<'source>,
    actual: DiagnosticExcerptEvidence<'source>,
}

impl MatchCandidateEvidence<'_> {
    fn measure(&mut self, cancellation: &CancellationProbe) -> Result<(), SketchContractKitError> {
        self.expected.measure(cancellation)?;
        self.actual.measure(cancellation)
    }

    fn skeleton(&self) -> MatchCandidate {
        MatchCandidate::new(
            self.position.source_span(),
            self.position.expected_line(),
            self.position.source_line(),
            self.expected.skeleton(),
            self.actual.skeleton(),
        )
    }

    fn additional_json_bytes(&self) -> u64 {
        self.expected
            .additional_json_bytes
            .saturating_add(self.actual.additional_json_bytes)
    }

    fn materialize(
        self,
        file: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<MatchCandidate, SketchContractKitError> {
        Ok(MatchCandidate::new(
            self.position.source_span(),
            self.position.expected_line(),
            self.position.source_line(),
            self.expected.materialize(file, cancellation)?,
            self.actual.materialize(file, cancellation)?,
        ))
    }
}

struct DiagnosticExcerptEvidence<'source> {
    bytes: Option<&'source [u8]>,
    retained_bytes: usize,
    escaped_bytes: usize,
    additional_json_bytes: u64,
    truncated: bool,
}

impl<'source> DiagnosticExcerptEvidence<'source> {
    fn new(bytes: Option<&'source [u8]>, maximum: usize) -> Self {
        let Some(bytes) = bytes else {
            return Self {
                bytes: None,
                retained_bytes: 0,
                escaped_bytes: 0,
                additional_json_bytes: 0,
                truncated: false,
            };
        };
        Self {
            bytes: Some(bytes),
            retained_bytes: bytes.len().min(maximum),
            escaped_bytes: 0,
            additional_json_bytes: 0,
            truncated: bytes.len() > maximum,
        }
    }

    fn measure(&mut self, cancellation: &CancellationProbe) -> Result<(), SketchContractKitError> {
        let Some(bytes) = self.bytes else {
            return Ok(());
        };
        let mut escaped_bytes = 0_usize;
        let mut json_escapes = 0_u64;
        for chunk in bytes[..self.retained_bytes].chunks(64 * 1024) {
            cancellation.checkpoint()?;
            for byte in chunk {
                for escaped in std::ascii::escape_default(*byte) {
                    escaped_bytes = escaped_bytes.saturating_add(1);
                    if matches!(escaped, b'"' | b'\\') {
                        json_escapes = json_escapes.saturating_add(1);
                    }
                }
            }
        }
        self.escaped_bytes = escaped_bytes;
        self.additional_json_bytes = u64::try_from(escaped_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(json_escapes);
        Ok(())
    }

    fn skeleton(&self) -> DiagnosticExcerpt {
        match self.bytes {
            Some(_) => DiagnosticExcerpt::Bytes {
                escaped: String::new(),
                truncated: self.truncated,
            },
            None => DiagnosticExcerpt::missing(),
        }
    }

    fn materialize(
        self,
        file: &CatalogPath,
        cancellation: &CancellationProbe,
    ) -> Result<DiagnosticExcerpt, SketchContractKitError> {
        let Some(bytes) = self.bytes else {
            return Ok(DiagnosticExcerpt::missing());
        };
        cancellation.checkpoint()?;
        let mut escaped = String::new();
        escaped
            .try_reserve_exact(self.escaped_bytes)
            .map_err(|source| SketchContractKitError::write_failed(file, source.to_string()))?;
        for chunk in bytes[..self.retained_bytes].chunks(64 * 1024) {
            cancellation.checkpoint()?;
            for byte in chunk {
                for escaped_byte in std::ascii::escape_default(*byte) {
                    escaped.push(char::from(escaped_byte));
                }
            }
        }
        debug_assert_eq!(escaped.len(), self.escaped_bytes);
        Ok(DiagnosticExcerpt::Bytes {
            escaped,
            truncated: self.truncated,
        })
    }
}

struct MatchScan<'source, 'limits> {
    expected: &'source NormalizedSnippet,
    source: &'source NormalizedSnippet,
    source_file: &'source CatalogPath,
    usage: &'source mut MatchingUsage<'limits>,
    cancellation: &'source CancellationProbe,
}

impl<'source, 'limits> MatchScan<'source, 'limits> {
    fn new(
        expected: &'source NormalizedSnippet,
        source: &'source NormalizedSnippet,
        source_file: &'source CatalogPath,
        usage: &'source mut MatchingUsage<'limits>,
        cancellation: &'source CancellationProbe,
    ) -> Self {
        Self {
            expected,
            source,
            source_file,
            usage,
            cancellation,
        }
    }

    fn evaluate(
        &mut self,
        occurrence: SketchOccurrence,
        retained_span_maximum: usize,
    ) -> Result<MatchEvaluation, SketchContractKitError> {
        let mut actual = 0;
        let mut spans = Vec::new();

        for source_start in 0..self.source.line_count() {
            self.cancellation.checkpoint_at(source_start)?;
            if !self.expected.matches_at(
                self.source,
                source_start,
                self.source_file,
                self.usage,
                self.cancellation,
            )? {
                continue;
            }

            self.usage.record_occurrence_candidate(self.source_file)?;
            if occurrence == SketchOccurrence::AtLeastOne {
                return Ok(MatchEvaluation::Satisfied);
            }

            actual += 1;
            if spans.len() < retained_span_maximum {
                spans.push(SourceLineSpan::new(
                    source_start + 1,
                    source_start + self.expected.line_count(),
                ));
            }
        }

        Ok(match actual {
            0 => MatchEvaluation::Missing,
            1 => MatchEvaluation::Satisfied,
            _ => MatchEvaluation::OccurrenceMismatch {
                actual,
                spans_truncated: actual > spans.len(),
                spans,
            },
        })
    }

    fn best_candidate(&mut self) -> Result<Option<MatchCandidatePosition>, SketchContractKitError> {
        if self.expected.is_empty() || self.source.is_empty() {
            return Ok(None);
        }

        let mut best_source_start = 0;
        let mut best_score = 0;
        let mut has_candidate = false;
        let mut comparison_count = 0_usize;

        for source_start in 0..self.source.line_count() {
            self.cancellation.checkpoint_at(source_start)?;
            let mut score = 0;
            for expected_index in 0..self.expected.line_count() {
                self.cancellation.checkpoint_at(comparison_count)?;
                comparison_count = comparison_count.saturating_add(1);
                self.usage.record_line_comparison(self.source_file)?;
                if self.expected.line_at(expected_index)
                    == self.source.line_at(source_start + expected_index)
                {
                    score += 1;
                }
            }

            if !has_candidate || score > best_score {
                best_source_start = source_start;
                best_score = score;
                has_candidate = true;
            }
        }

        if !has_candidate {
            return Ok(None);
        }

        let mut first_difference = None;
        for expected_index in 0..self.expected.line_count() {
            self.cancellation.checkpoint_at(expected_index)?;
            if self.expected.line_at(expected_index)
                != self.source.line_at(best_source_start + expected_index)
            {
                first_difference = Some(expected_index);
                break;
            }
        }
        let Some(first_difference) = first_difference else {
            return Ok(None);
        };
        if self.expected.line_at(first_difference).is_none() {
            return Ok(None);
        }
        let source_end =
            (best_source_start + self.expected.line_count()).min(self.source.line_count());

        Ok(Some(MatchCandidatePosition {
            source_start: best_source_start,
            source_end,
            first_difference,
        }))
    }
}

struct SourceSelection {
    paths: BTreeSet<CatalogPath>,
}

impl SourceSelection {
    fn from_contracts(
        contracts: &SketchContracts,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SketchContractKitError> {
        let mut paths = BTreeSet::new();
        for contract in contracts.entries() {
            cancellation.checkpoint()?;
            paths.insert(contract.file().clone());
        }

        Ok(Self { paths })
    }

    fn contains(&self, path: &CatalogPath) -> bool {
        self.paths.contains(path)
    }

    fn len(&self) -> usize {
        self.paths.len()
    }
}

pub(crate) struct SketchMatcher {
    sources: SourceCatalog,
    contracts: SketchContracts,
}

impl SketchMatcher {
    pub(crate) fn new(sources: SourceCatalog, contracts: SketchContracts) -> Self {
        Self { sources, contracts }
    }

    pub(crate) fn check(
        self,
        limits: &SketchLimits,
        cancellation: &CancellationProbe,
    ) -> Result<SketchInventoryComparison, SketchContractKitError> {
        let source_catalog_entry_count = self.sources.catalog_entry_count();
        let referenced_source_file_count = self.sources.referenced_file_count();
        let present_referenced_source_file_count = self.sources.present_referenced_file_count();
        let contract_document_count = self.contracts.contract_document_count();
        let sketch_count = self.contracts.len();
        let mut diagnostics = SketchDiagnostics::new(&limits.diagnostics)?;
        let mut matching_usage = limits.matching.usage();
        let mut matched_sketch_count = 0;
        for group in self.sources.groups(&self.contracts, cancellation)? {
            cancellation.checkpoint()?;
            matched_sketch_count += group.append_diagnostics(
                &limits.matching,
                &limits.diagnostics,
                cancellation,
                &mut matching_usage,
                &mut diagnostics,
            )?;
        }
        let diagnostics = diagnostics.into_entries();

        Ok(SketchInventoryComparison::new(
            source_catalog_entry_count,
            referenced_source_file_count,
            present_referenced_source_file_count,
            contract_document_count,
            sketch_count,
            matched_sketch_count,
            diagnostics,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{DiagnosticExcerptEvidence, MatchScan, SketchMatcher, SourceCatalog};
    use crate::api::CheckMode;
    use crate::contract::{SketchContracts, SketchNormalization, SketchOccurrence};
    use crate::files::{CatalogPath, FileCatalog};
    use crate::inventory::{
        DiagnosticExcerpt, MatchCandidate, SketchDiagnostic, SketchInventoryComparison,
        SketchLocation, SourceLineSpan,
    };
    use crate::limits::SketchLimits;
    use crate::work::CancellationProbe;

    const MAXIMUM_OCCURRENCE_SPANS: usize = 32;
    const MAXIMUM_EXCERPT_BYTES: usize = 256;

    #[test]
    fn cancelled_occurrence_and_candidate_scans_return_typed_errors() {
        let cancellation = CancellationProbe::new();
        let limits = SketchLimits::default();
        let contract = CatalogPath::new("contract.yml").expect("contract path");
        let source_file = CatalogPath::new("source.rs").expect("source path");
        let expected = SketchNormalization::ExactLinesV1
            .normalize_snippet(
                b"expected\nlines",
                &limits.matching,
                &contract,
                &cancellation,
            )
            .expect("expected normalization");
        let source = SketchNormalization::ExactLinesV1
            .normalize_source(
                b"different\nlines\nrepeated\ncontent",
                &limits.matching,
                &source_file,
                &cancellation,
            )
            .expect("source normalization");
        let mut matching_usage = limits.matching.usage();
        cancellation.cancel();

        let Err(occurrence_error) = MatchScan::new(
            &expected,
            &source,
            &source_file,
            &mut matching_usage,
            &cancellation,
        )
        .evaluate(SketchOccurrence::ExactlyOne, MAXIMUM_OCCURRENCE_SPANS) else {
            panic!("cancelled occurrence scan must fail");
        };
        assert!(
            occurrence_error.to_string().contains("cancelled"),
            "{occurrence_error}"
        );

        let Err(candidate_error) = MatchScan::new(
            &expected,
            &source,
            &source_file,
            &mut matching_usage,
            &cancellation,
        )
        .best_candidate() else {
            panic!("cancelled candidate scan must fail");
        };
        assert!(
            candidate_error.to_string().contains("cancelled"),
            "{candidate_error}"
        );
    }

    struct TestCatalog {
        catalog: FileCatalog,
    }

    impl TestCatalog {
        fn new() -> Self {
            Self {
                catalog: FileCatalog::new(),
            }
        }

        fn with_file(mut self, path: &str, contents: &str) -> Self {
            self.catalog
                .insert(
                    CatalogPath::new(path).expect("test path"),
                    contents.as_bytes().to_vec(),
                )
                .expect("insert test file");
            self
        }

        fn with_bytes(mut self, path: &str, contents: Vec<u8>) -> Self {
            self.catalog
                .insert(CatalogPath::new(path).expect("test path"), contents)
                .expect("insert test file");
            self
        }

        fn into_catalog(self) -> FileCatalog {
            self.catalog
        }
    }

    struct TestCheck {
        sources: FileCatalog,
        contracts: FileCatalog,
    }

    impl TestCheck {
        fn new(sources: FileCatalog, contracts: FileCatalog) -> Self {
            Self { sources, contracts }
        }

        fn run(self) -> Vec<SketchDiagnostic> {
            self.try_run(&SketchLimits::default()).expect("check")
        }

        fn try_run(
            self,
            limits: &SketchLimits,
        ) -> Result<Vec<SketchDiagnostic>, crate::error::SketchContractKitError> {
            Ok(self.try_comparison(limits)?.diagnostics().to_vec())
        }

        fn try_comparison(
            self,
            limits: &SketchLimits,
        ) -> Result<SketchInventoryComparison, crate::error::SketchContractKitError> {
            let cancellation = CancellationProbe::new();
            let mut yaml_budget = limits.yaml_budget();
            let contracts = SketchContracts::from_catalog(
                self.contracts,
                limits,
                &mut yaml_budget,
                &cancellation,
            )?;
            let sources = SourceCatalog::from_catalog(self.sources, &contracts, &cancellation)?;
            SketchMatcher::new(sources, contracts).check(limits, &cancellation)
        }
    }

    struct TestSketch<'a> {
        id: &'a str,
        file: &'a str,
        code: &'a str,
        occurrence: SketchOccurrence,
    }

    impl<'a> TestSketch<'a> {
        fn at_least_one(id: &'a str, file: &'a str, code: &'a str) -> Self {
            Self {
                id,
                file,
                code,
                occurrence: SketchOccurrence::AtLeastOne,
            }
        }

        fn exactly_one(id: &'a str, file: &'a str, code: &'a str) -> Self {
            Self {
                id,
                file,
                code,
                occurrence: SketchOccurrence::ExactlyOne,
            }
        }

        fn occurrence_yaml(&self) -> &'static str {
            match self.occurrence {
                SketchOccurrence::AtLeastOne => "at_least_one",
                SketchOccurrence::ExactlyOne => "exactly_one",
            }
        }
    }

    struct TestContract;

    impl TestContract {
        fn single(sketch: TestSketch<'_>) -> String {
            Self::many(std::slice::from_ref(&sketch))
        }

        fn many(sketches: &[TestSketch<'_>]) -> String {
            let files = sketches
                .iter()
                .map(|sketch| sketch.file)
                .collect::<std::collections::BTreeSet<_>>();
            let crate_root = sketches
                .first()
                .map(|sketch| sketch.file)
                .expect("test contract needs at least one sketch");
            let mut yaml = "contract_version: 2\nroot: .\nfiles:\n".to_owned();
            for file in files {
                yaml.push_str(&format!("  - {file}\n"));
            }
            yaml.push_str(&format!(
                "extraction: {{ mode: rust_syntax_v2, profile: rust_api_v1, crates: [{{ id: example, root: {crate_root}, kind: library }}] }}\nsignatures:\n"
            ));
            for sketch in sketches {
                yaml.push_str(&format!(
                    "  - {id}_signature:\n      file: {file}\n      signature_type: function\n      sketch: {id}\n",
                    id = sketch.id,
                    file = sketch.file,
                ));
            }
            yaml.push_str("sketches:\n");
            for sketch in sketches {
                yaml.push_str(&format!(
                    "  - {id}:\n      file: {file}\n      signature: {id}_signature\n      signature_type: function\n      matching: {{ normalization: exact_lines_v1, occurrence: {occurrence} }}\n      code: |\n",
                    id = sketch.id,
                    file = sketch.file,
                    occurrence = sketch.occurrence_yaml(),
                ));
                for line in sketch.code.split('\n') {
                    yaml.push_str("        ");
                    yaml.push_str(line);
                    yaml.push('\n');
                }
            }
            yaml
        }

        fn location(id: &str, source_file: &str) -> SketchLocation {
            SketchLocation::new(
                id,
                CatalogPath::new("main.yml").expect("contract path"),
                0,
                CatalogPath::new(source_file).expect("source path"),
            )
        }
    }

    #[test]
    fn exact_match_passes_without_diagnostics() {
        let sources = TestCatalog::new()
            .with_file("src/lib.rs", "fn answer() -> u8 {\n    42\n}\n")
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single(TestSketch::at_least_one(
                    "answer",
                    "src/lib.rs",
                    "fn answer() -> u8 {\n    42\n}",
                )),
            )
            .into_catalog();

        assert!(TestCheck::new(sources, contracts).run().is_empty());
    }

    #[test]
    fn matcher_derives_scope_and_outcome_counts_from_real_results() {
        let sources = TestCatalog::new()
            .with_file("src/present.rs", "value")
            .with_file("src/unreferenced.rs", "ignored")
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::many(&[
                    TestSketch::at_least_one("matched", "src/present.rs", "value"),
                    TestSketch::at_least_one("missing", "src/missing.rs", "value"),
                ]),
            )
            .into_catalog();

        let response = TestCheck::new(sources, contracts)
            .try_comparison(&SketchLimits::default())
            .expect("check")
            .into_response(CheckMode::Enforce);

        assert_eq!(response.counts.source_catalog_entry_count, 2);
        assert_eq!(response.counts.referenced_source_file_count, 2);
        assert_eq!(response.counts.present_referenced_source_file_count, 1);
        assert_eq!(response.counts.contract_document_count, 1);
        assert_eq!(response.counts.sketch_count, 2);
        assert_eq!(response.counts.matched_sketch_count, 1);
        assert_eq!(response.counts.failed_sketch_count, 1);
        assert_eq!(response.diagnostics.len(), 1);
        assert!(!response.passed);
    }

    #[test]
    fn matching_work_budgets_fail_closed_before_unbounded_search() {
        let contract_yaml =
            TestContract::single(TestSketch::exactly_one("value", "src/lib.rs", "value"));

        let mut comparison_limits = SketchLimits::default();
        comparison_limits.matching.line_comparisons = 0;
        let comparison_error = TestCheck::new(
            TestCatalog::new()
                .with_file("src/lib.rs", "value\n")
                .into_catalog(),
            TestCatalog::new()
                .with_file("main.yml", &contract_yaml)
                .into_catalog(),
        )
        .try_run(&comparison_limits)
        .expect_err("zero comparison budget must stop matching");
        let comparison = comparison_error
            .limit_exceeded()
            .expect("typed comparison limit");
        assert_eq!(
            comparison.resource,
            crate::limits::LimitResource::MatchingLineComparisons,
        );
        assert_eq!(comparison.observed_at_least, 1);

        let mut occurrence_limits = SketchLimits::default();
        occurrence_limits.matching.occurrence_candidates = 0;
        let occurrence_error = TestCheck::new(
            TestCatalog::new()
                .with_file("src/lib.rs", "value\n")
                .into_catalog(),
            TestCatalog::new()
                .with_file("main.yml", &contract_yaml)
                .into_catalog(),
        )
        .try_run(&occurrence_limits)
        .expect_err("zero occurrence budget must stop matching");
        let occurrence = occurrence_error
            .limit_exceeded()
            .expect("typed occurrence limit");
        assert_eq!(
            occurrence.resource,
            crate::limits::LimitResource::OccurrenceCandidateCount,
        );
        assert_eq!(occurrence.observed_at_least, 1);
    }

    #[test]
    fn diagnostic_count_is_rejected_before_nearest_candidate_scanning() {
        let mut limits = SketchLimits::default();
        limits.diagnostics.count = 0;
        limits.matching.line_comparisons = 1;
        let error = TestCheck::new(
            TestCatalog::new()
                .with_file("src/lib.rs", "actual")
                .into_catalog(),
            TestCatalog::new()
                .with_file(
                    "main.yml",
                    &TestContract::single(TestSketch::at_least_one(
                        "value",
                        "src/lib.rs",
                        "expected",
                    )),
                )
                .into_catalog(),
        )
        .try_run(&limits)
        .expect_err("disabled diagnostic count must fail before candidate evidence work");

        assert_eq!(
            error.limit_exceeded().expect("typed limit").resource,
            crate::limits::LimitResource::DiagnosticCount,
        );
    }

    #[test]
    fn diagnostic_byte_preflight_exactly_matches_shared_escaped_evidence() {
        let sketches = [
            TestSketch::at_least_one("first", "src/first.bin", "expected\"\\first"),
            TestSketch::at_least_one("second", "src/second.bin", "expected\"\\second"),
        ];
        let contract = TestContract::many(&sketches);
        let sources = TestCatalog::new()
            .with_bytes("src/first.bin", vec![b'"', b'\\', b'\n', 0xff])
            .with_bytes("src/second.bin", vec![0xff, b'\\', b'"'])
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file("main.yml", &contract)
            .into_catalog();
        let expected = TestCheck::new(sources, contracts).run();
        let exact_bytes = u64::try_from(
            serde_json::to_vec(&expected)
                .expect("serialize complete diagnostic array")
                .len(),
        )
        .expect("diagnostic byte count");

        let mut exact_limits = SketchLimits::default();
        exact_limits.diagnostics.serialized_bytes = exact_bytes;
        let exact = TestCheck::new(
            TestCatalog::new()
                .with_bytes("src/first.bin", vec![b'"', b'\\', b'\n', 0xff])
                .with_bytes("src/second.bin", vec![0xff, b'\\', b'"'])
                .into_catalog(),
            TestCatalog::new()
                .with_file("main.yml", &contract)
                .into_catalog(),
        )
        .try_run(&exact_limits)
        .expect("exact aggregate diagnostic byte boundary");
        assert_eq!(exact, expected);

        let mut crossing_limits = exact_limits;
        crossing_limits.diagnostics.serialized_bytes = exact_bytes.saturating_sub(1);
        let error = TestCheck::new(
            TestCatalog::new()
                .with_bytes("src/first.bin", vec![b'"', b'\\', b'\n', 0xff])
                .with_bytes("src/second.bin", vec![0xff, b'\\', b'"'])
                .into_catalog(),
            TestCatalog::new()
                .with_file("main.yml", &contract)
                .into_catalog(),
        )
        .try_run(&crossing_limits)
        .expect_err("one byte below exact aggregate evidence must fail");
        let limit = error.limit_exceeded().expect("typed diagnostic byte limit");
        assert_eq!(
            limit.resource,
            crate::limits::LimitResource::DiagnosticBytes
        );
        assert_eq!(limit.limit, exact_bytes.saturating_sub(1));
        assert_eq!(limit.observed_at_least, exact_bytes);
    }

    #[test]
    fn large_invalid_candidate_hits_tiny_diagnostic_budget_before_evidence_allocation() {
        let mut limits = SketchLimits::default();
        limits.diagnostics.serialized_bytes = 1_024;
        limits.diagnostics.excerpt_bytes = 256 * 1024;
        let error = TestCheck::new(
            TestCatalog::new()
                .with_bytes("src/large.bin", vec![0xff; 1024 * 1024])
                .into_catalog(),
            TestCatalog::new()
                .with_file(
                    "main.yml",
                    &TestContract::single(TestSketch::at_least_one(
                        "large",
                        "src/large.bin",
                        "expected",
                    )),
                )
                .into_catalog(),
        )
        .try_run(&limits)
        .expect_err("large escaped evidence must fail its aggregate preflight");
        let limit = error.limit_exceeded().expect("typed diagnostic byte limit");

        assert_eq!(
            limit.resource,
            crate::limits::LimitResource::DiagnosticBytes
        );
        assert_eq!(limit.limit, 1_024);
        assert_eq!(limit.observed_at_least, 1_025);
    }

    #[test]
    fn excerpt_preflight_and_materialization_share_exact_escaping_and_cancellation() {
        let bytes = [b'a', b'"', b'\\', b'\n', 0xff];
        let cancellation = CancellationProbe::new();
        let mut evidence = DiagnosticExcerptEvidence::new(Some(bytes.as_slice()), bytes.len());
        let skeleton = evidence.skeleton();
        let skeleton_bytes = serde_json::to_vec(&skeleton)
            .expect("serialize empty evidence skeleton")
            .len();
        evidence
            .measure(&cancellation)
            .expect("allocation-free evidence scan");
        let additional_bytes = evidence.additional_json_bytes;
        let materialized = evidence
            .materialize(
                &CatalogPath::new("src/lib.bin").expect("source path"),
                &cancellation,
            )
            .expect("second-pass evidence materialization");
        let materialized_bytes = serde_json::to_vec(&materialized)
            .expect("serialize materialized evidence")
            .len();
        assert_eq!(
            u64::try_from(materialized_bytes.saturating_sub(skeleton_bytes))
                .expect("serialized evidence difference"),
            additional_bytes,
        );
        assert_eq!(
            materialized,
            DiagnosticExcerpt::Bytes {
                escaped: "a\\\"\\\\\\n\\xff".to_owned(),
                truncated: false,
            }
        );

        let first_pass_cancellation = CancellationProbe::new();
        first_pass_cancellation.cancel();
        let mut first_pass_evidence =
            DiagnosticExcerptEvidence::new(Some(bytes.as_slice()), bytes.len());
        let first_pass = first_pass_evidence.measure(&first_pass_cancellation);
        assert!(matches!(first_pass, Err(error) if error.is_operation_cancelled()));

        let second_pass_cancellation = CancellationProbe::new();
        let mut evidence = DiagnosticExcerptEvidence::new(Some(bytes.as_slice()), bytes.len());
        evidence
            .measure(&second_pass_cancellation)
            .expect("active first pass");
        second_pass_cancellation.cancel();
        let second_pass = evidence.materialize(
            &CatalogPath::new("src/lib.bin").expect("source path"),
            &second_pass_cancellation,
        );
        assert!(matches!(second_pass, Err(error) if error.is_operation_cancelled()));

        let missing = DiagnosticExcerptEvidence::new(None, 0);
        assert_eq!(missing.additional_json_bytes, 0);
        assert_eq!(missing.skeleton(), DiagnosticExcerpt::Missing);
    }

    #[test]
    fn unrelated_binary_source_file_does_not_fail_matching_sketch() {
        let sources = TestCatalog::new()
            .with_file("src/lib.rs", "fn answer() -> u8 {\n    42\n}\n")
            .with_bytes("assets/blob.bin", vec![0, 159, 255])
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single(TestSketch::at_least_one(
                    "answer",
                    "src/lib.rs",
                    "fn answer() -> u8 {\n    42\n}",
                )),
            )
            .into_catalog();

        assert!(TestCheck::new(sources, contracts).run().is_empty());
    }

    #[test]
    fn whitespace_changes_emit_not_matched_with_nearest_evidence() {
        let sources = TestCatalog::new()
            .with_file("src/lib.rs", "let message = \"a b\";\n")
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single(TestSketch::at_least_one(
                    "message",
                    "src/lib.rs",
                    "let message = \"a  b\";",
                )),
            )
            .into_catalog();

        assert_eq!(
            TestCheck::new(sources, contracts).run(),
            vec![SketchDiagnostic::not_matched(
                TestContract::location("message", "src/lib.rs"),
                crate::contract::SketchNormalization::ExactLinesV1,
                Some(MatchCandidate::new(
                    SourceLineSpan::new(1, 1),
                    1,
                    1,
                    DiagnosticExcerpt::Bytes {
                        escaped: "let message = \\\"a  b\\\";".to_owned(),
                        truncated: false,
                    },
                    DiagnosticExcerpt::Bytes {
                        escaped: "let message = \\\"a b\\\";".to_owned(),
                        truncated: false,
                    },
                )),
            )]
        );
    }

    #[test]
    fn at_least_one_accepts_duplicate_occurrences() {
        let sources = TestCatalog::new()
            .with_file("src/lib.rs", "value\nvalue\n")
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single(TestSketch::at_least_one("value", "src/lib.rs", "value")),
            )
            .into_catalog();

        let mut limits = SketchLimits::default();
        limits.matching.line_comparisons = 1;
        limits.matching.occurrence_candidates = 1;

        assert!(
            TestCheck::new(sources, contracts)
                .try_run(&limits)
                .expect("at-least-one must stop after the first occurrence")
                .is_empty()
        );
    }

    #[test]
    fn exactly_one_accepts_one_occurrence() {
        let sources = TestCatalog::new()
            .with_file("src/lib.rs", "before\nvalue\nafter\n")
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single(TestSketch::exactly_one("value", "src/lib.rs", "value")),
            )
            .into_catalog();

        assert!(TestCheck::new(sources, contracts).run().is_empty());
    }

    #[test]
    fn exactly_one_rejects_zero_occurrences_as_not_matched() {
        let sources = TestCatalog::new()
            .with_file("src/lib.rs", "other\n")
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single(TestSketch::exactly_one("value", "src/lib.rs", "value")),
            )
            .into_catalog();

        assert!(matches!(
            TestCheck::new(sources, contracts).run().as_slice(),
            [SketchDiagnostic::NotMatched { .. }]
        ));
    }

    #[test]
    fn exactly_one_rejects_duplicate_overlapping_occurrences() {
        let sources = TestCatalog::new()
            .with_file("src/lib.rs", "a\na\na\n")
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single(TestSketch::exactly_one("pair", "src/lib.rs", "a\na")),
            )
            .into_catalog();

        assert_eq!(
            TestCheck::new(sources, contracts).run(),
            vec![SketchDiagnostic::occurrence_mismatch(
                TestContract::location("pair", "src/lib.rs"),
                SketchOccurrence::ExactlyOne,
                2,
                vec![SourceLineSpan::new(1, 2), SourceLineSpan::new(2, 3)],
                false,
            )]
        );
    }

    #[test]
    fn exactly_one_reports_exact_count_with_bounded_spans() {
        let occurrence_count = MAXIMUM_OCCURRENCE_SPANS + 3;
        let source = std::iter::repeat_n("value", occurrence_count)
            .collect::<Vec<_>>()
            .join("\n");
        let sources = TestCatalog::new()
            .with_file("src/lib.rs", &source)
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single(TestSketch::exactly_one("value", "src/lib.rs", "value")),
            )
            .into_catalog();

        let diagnostics = TestCheck::new(sources, contracts).run();
        let [
            SketchDiagnostic::OccurrenceMismatch {
                actual,
                spans,
                spans_truncated,
                ..
            },
        ] = diagnostics.as_slice()
        else {
            panic!("expected one occurrence diagnostic");
        };

        assert_eq!(*actual, occurrence_count);
        assert_eq!(spans.len(), MAXIMUM_OCCURRENCE_SPANS);
        assert_eq!(spans.first(), Some(&SourceLineSpan::new(1, 1)));
        assert_eq!(
            spans.last(),
            Some(&SourceLineSpan::new(
                MAXIMUM_OCCURRENCE_SPANS,
                MAXIMUM_OCCURRENCE_SPANS
            ))
        );
        assert!(*spans_truncated);
    }

    #[test]
    fn configured_span_and_excerpt_bounds_change_only_diagnostic_evidence() {
        let mut span_limits = SketchLimits::default();
        span_limits.matching.retained_occurrence_spans = 1;
        let duplicate = TestCheck::new(
            TestCatalog::new()
                .with_file("src/lib.rs", "value\nvalue\n")
                .into_catalog(),
            TestCatalog::new()
                .with_file(
                    "main.yml",
                    &TestContract::single(TestSketch::exactly_one("value", "src/lib.rs", "value")),
                )
                .into_catalog(),
        )
        .try_run(&span_limits)
        .expect("bounded spans");
        assert!(matches!(
            duplicate.as_slice(),
            [SketchDiagnostic::OccurrenceMismatch {
                actual: 2,
                spans,
                spans_truncated: true,
                ..
            }] if spans == &[SourceLineSpan::new(1, 1)]
        ));

        let mut excerpt_limits = SketchLimits::default();
        excerpt_limits.diagnostics.excerpt_bytes = 1;
        let mismatch = TestCheck::new(
            TestCatalog::new()
                .with_file("src/lib.rs", "actual")
                .into_catalog(),
            TestCatalog::new()
                .with_file(
                    "main.yml",
                    &TestContract::single(TestSketch::at_least_one(
                        "value",
                        "src/lib.rs",
                        "expected",
                    )),
                )
                .into_catalog(),
        )
        .try_run(&excerpt_limits)
        .expect("bounded excerpts");
        assert!(matches!(
            mismatch.as_slice(),
            [SketchDiagnostic::NotMatched {
                candidate: Some(MatchCandidate {
                    expected: DiagnosticExcerpt::Bytes {
                        truncated: true,
                        ..
                    },
                    actual: DiagnosticExcerpt::Bytes {
                        truncated: true,
                        ..
                    },
                    ..
                }),
                ..
            }]
        ));
    }

    #[test]
    fn nearest_candidate_prefers_the_earliest_equal_score() {
        let sources = TestCatalog::new()
            .with_file(
                "src/lib.rs",
                "same\nfirst mismatch\nsame\nsecond mismatch\n",
            )
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single(TestSketch::at_least_one(
                    "candidate",
                    "src/lib.rs",
                    "same\nexpected",
                )),
            )
            .into_catalog();

        let diagnostics = TestCheck::new(sources, contracts).run();
        let [
            SketchDiagnostic::NotMatched {
                candidate: Some(candidate),
                ..
            },
        ] = diagnostics.as_slice()
        else {
            panic!("expected one nearest-candidate diagnostic");
        };

        assert_eq!(candidate.source, SourceLineSpan::new(1, 2));
        assert_eq!(candidate.expected_line, 2);
        assert_eq!(candidate.source_line, 2);
    }

    #[test]
    fn nearest_candidate_prefers_a_later_strictly_higher_score() {
        let sources = TestCatalog::new()
            .with_file("src/lib.rs", "one\nwrong\nwrong\none\ntwo\nstill wrong\n")
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single(TestSketch::at_least_one(
                    "candidate",
                    "src/lib.rs",
                    "one\ntwo\nthree",
                )),
            )
            .into_catalog();

        let diagnostics = TestCheck::new(sources, contracts).run();
        let [
            SketchDiagnostic::NotMatched {
                candidate: Some(candidate),
                ..
            },
        ] = diagnostics.as_slice()
        else {
            panic!("expected one nearest-candidate diagnostic");
        };

        assert_eq!(candidate.source, SourceLineSpan::new(4, 6));
        assert_eq!(candidate.expected_line, 3);
        assert_eq!(candidate.source_line, 6);
    }

    #[test]
    fn candidate_marks_an_actual_line_missing_when_expected_is_longer() {
        let sources = TestCatalog::new()
            .with_file("src/lib.rs", "same")
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single(TestSketch::at_least_one(
                    "longer",
                    "src/lib.rs",
                    "same\nneeded",
                )),
            )
            .into_catalog();

        let diagnostics = TestCheck::new(sources, contracts).run();
        let [
            SketchDiagnostic::NotMatched {
                candidate: Some(candidate),
                ..
            },
        ] = diagnostics.as_slice()
        else {
            panic!("expected one missing-line diagnostic");
        };

        assert_eq!(candidate.source, SourceLineSpan::new(1, 1));
        assert_eq!(candidate.expected_line, 2);
        assert_eq!(candidate.source_line, 2);
        assert_eq!(candidate.actual, DiagnosticExcerpt::Missing);
    }

    #[test]
    fn empty_source_has_no_candidate_window() {
        let sources = TestCatalog::new()
            .with_bytes("src/lib.rs", Vec::new())
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single(TestSketch::at_least_one("missing", "src/lib.rs", "value")),
            )
            .into_catalog();

        assert_eq!(
            TestCheck::new(sources, contracts).run(),
            vec![SketchDiagnostic::not_matched(
                TestContract::location("missing", "src/lib.rs"),
                crate::contract::SketchNormalization::ExactLinesV1,
                None,
            )]
        );
    }

    #[test]
    fn candidate_excerpts_escape_invalid_bytes_and_bound_raw_input() {
        let long_actual = vec![0xff; MAXIMUM_EXCERPT_BYTES + 1];
        let sources = TestCatalog::new()
            .with_bytes("src/lib.rs", long_actual)
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single(TestSketch::at_least_one("bytes", "src/lib.rs", "expected")),
            )
            .into_catalog();

        let diagnostics = TestCheck::new(sources, contracts).run();
        let [
            SketchDiagnostic::NotMatched {
                candidate: Some(candidate),
                ..
            },
        ] = diagnostics.as_slice()
        else {
            panic!("expected one byte-evidence diagnostic");
        };

        assert_eq!(
            candidate.actual,
            DiagnosticExcerpt::Bytes {
                escaped: "\\xff".repeat(MAXIMUM_EXCERPT_BYTES),
                truncated: true,
            }
        );
    }

    #[test]
    fn missing_source_file_emits_missing_file() {
        let sources = TestCatalog::new().into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::single(TestSketch::at_least_one(
                    "missing",
                    "src/missing.rs",
                    "let value = 42;",
                )),
            )
            .into_catalog();

        assert_eq!(
            TestCheck::new(sources, contracts).run(),
            vec![SketchDiagnostic::missing_file(TestContract::location(
                "missing",
                "src/missing.rs",
            ))]
        );
    }

    #[test]
    fn multiple_sketches_in_multiple_files_can_pass() {
        let sources = TestCatalog::new()
            .with_file("src/a.rs", "fn a() -> u8 { 1 }\n")
            .with_file("src/b.rs", "fn b() -> u8 { 2 }\n")
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::many(&[
                    TestSketch::at_least_one("a", "src/a.rs", "fn a() -> u8 { 1 }"),
                    TestSketch::at_least_one("b", "src/b.rs", "fn b() -> u8 { 2 }"),
                ]),
            )
            .into_catalog();

        assert!(TestCheck::new(sources, contracts).run().is_empty());
    }

    #[test]
    fn multiple_sketches_share_one_source_policy_group() {
        let source_files = TestCatalog::new()
            .with_file("src/lib.rs", "first\nsecond\n")
            .into_catalog();
        let contract_files = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::many(&[
                    TestSketch::at_least_one("first", "src/lib.rs", "first"),
                    TestSketch::at_least_one("missing", "src/lib.rs", "third"),
                ]),
            )
            .into_catalog();
        let limits = SketchLimits::default();
        let cancellation = CancellationProbe::new();
        let mut yaml_budget = limits.yaml_budget();
        let contracts =
            SketchContracts::from_catalog(contract_files, &limits, &mut yaml_budget, &cancellation)
                .expect("contracts");
        let sources =
            SourceCatalog::from_catalog(source_files, &contracts, &cancellation).expect("sources");

        assert_eq!(
            sources
                .groups(&contracts, &cancellation)
                .expect("source groups")
                .len(),
            1
        );

        let comparison = SketchMatcher::new(sources, contracts)
            .check(&limits, &cancellation)
            .expect("check");
        assert!(matches!(
            comparison.diagnostics(),
            [SketchDiagnostic::NotMatched { sketch, .. }] if sketch.sketch_id == "missing"
        ));
    }

    #[test]
    fn diagnostics_are_sorted_across_source_groups() {
        let sources = TestCatalog::new()
            .with_file("src/a.rs", "let value = 0;\n")
            .into_catalog();
        let contracts = TestCatalog::new()
            .with_file(
                "main.yml",
                &TestContract::many(&[
                    TestSketch::at_least_one("zeta", "src/z.rs", "let z = 1;"),
                    TestSketch::at_least_one("alpha", "src/a.rs", "let value = 1;"),
                ]),
            )
            .into_catalog();

        assert_eq!(
            TestCheck::new(sources, contracts).run(),
            vec![
                SketchDiagnostic::not_matched(
                    TestContract::location("alpha", "src/a.rs"),
                    crate::contract::SketchNormalization::ExactLinesV1,
                    Some(MatchCandidate::new(
                        SourceLineSpan::new(1, 1),
                        1,
                        1,
                        DiagnosticExcerpt::Bytes {
                            escaped: "let value = 1;".to_owned(),
                            truncated: false,
                        },
                        DiagnosticExcerpt::Bytes {
                            escaped: "let value = 0;".to_owned(),
                            truncated: false,
                        },
                    )),
                ),
                SketchDiagnostic::missing_file(TestContract::location("zeta", "src/z.rs",)),
            ]
        );
    }
}
