use crate::api::CheckResponse;
use crate::error::SketchContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::limits::{GeneratedBytes, OutputLimits};
use crate::work::CancellationProbe;
use serde::{Deserialize, Serialize};

/// Output encoding for generated sketch check reports.
///
/// The selected variant controls serialization directly; the checker does not
/// infer a format from the report's logical output path or its extension.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ReportFormat {
    /// Render the report as UTF-8 YAML bytes.
    Yaml,
    /// Render the report as pretty-printed UTF-8 JSON bytes.
    Json,
}

/// Selects whether a sketch check returns an encoded report.
///
/// Reports are byte-in, byte-out values: the checker never creates directories
/// or writes to the filesystem. [`ReportRequest::None`] leaves
/// [`CheckResponse::report_files`] empty. [`ReportRequest::Generate`] returns a
/// catalog with exactly one entry at the requested logical path. That entry
/// serializes the response's `passed` value, counts, and deterministically
/// ordered diagnostics; it does not recursively include `report_files`.
///
/// If report rendering fails, [`SketchContractKit::check`](crate::SketchContractKit::check)
/// returns a [`SketchContractKitError`] instead
/// of a partial response.
///
/// # Examples
///
/// ```
/// use conkit_sketch::{CatalogPath, ReportFormat, ReportRequest};
///
/// let output_file = CatalogPath::new("reports/sketch-check.json")?;
/// let request = ReportRequest::Generate {
///     format: ReportFormat::Json,
///     output_file: output_file.clone(),
/// };
///
/// assert_eq!(
///     request,
///     ReportRequest::Generate {
///         format: ReportFormat::Json,
///         output_file,
///     }
/// );
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ReportRequest {
    /// Return an empty report catalog.
    None,
    /// Return one encoded report in the report catalog.
    Generate {
        /// Encoding to use, independent of the output path's extension.
        format: ReportFormat,
        /// Logical catalog key for the returned report bytes.
        ///
        /// This is not an operating-system path and is never written by the
        /// crate.
        output_file: CatalogPath,
    },
}

impl ReportRequest {
    pub(crate) fn render(
        self,
        response: &CheckResponse,
        limits: &OutputLimits,
        cancellation: &CancellationProbe,
    ) -> Result<FileCatalog, SketchContractKitError> {
        cancellation.checkpoint()?;
        match self {
            Self::None => Ok(FileCatalog::new()),
            Self::Generate {
                format,
                output_file,
            } => {
                let report = response.report_view();
                let generated = GeneratedBytes::new(limits, cancellation);
                let mut output = generated.returned_buffer(&output_file);
                let bytes = match format {
                    ReportFormat::Yaml => {
                        let rendering = serde_saphyr::to_fmt_writer(&mut output, &report);
                        output.finish(rendering)?
                    }
                    ReportFormat::Json => {
                        let rendering = serde_json::to_writer_pretty(&mut output, &report);
                        output.finish(rendering)?
                    }
                };
                cancellation.checkpoint()?;
                let mut files = FileCatalog::new();
                files.insert(output_file, bytes)?;
                Ok(files)
            }
        }
    }
}
pub(crate) struct CheckReportView<'response> {
    response: &'response CheckResponse,
}

impl<'response> CheckReportView<'response> {
    pub(crate) fn new(response: &'response CheckResponse) -> Self {
        Self { response }
    }
}

impl Serialize for CheckReportView<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct as _;

        let mut report = serializer.serialize_struct("CheckReport", 3)?;
        report.serialize_field("passed", &self.response.passed)?;
        report.serialize_field("counts", &self.response.counts)?;
        report.serialize_field("diagnostics", &self.response.diagnostics)?;
        report.end()
    }
}

#[cfg(test)]
mod tests {
    use super::{CheckReportView, ReportFormat, ReportRequest};
    use crate::api::CheckResponse;
    use crate::contract::{SketchNormalization, SketchOccurrence};
    use crate::files::{CatalogPath, FileCatalog};
    use crate::inventory::{
        DiagnosticExcerpt, MatchCandidate, SketchCheckCounts, SketchDiagnostic, SketchLocation,
        SourceLineSpan,
    };
    use crate::limits::{LimitResource, OutputLimits};
    use crate::work::CancellationProbe;
    use serde::Deserialize;

    struct TestResponse {
        response: CheckResponse,
    }

    impl TestResponse {
        fn with_rich_diagnostics() -> Self {
            Self {
                response: CheckResponse {
                    passed: false,
                    counts: SketchCheckCounts {
                        source_catalog_entry_count: 4,
                        referenced_source_file_count: 3,
                        present_referenced_source_file_count: 2,
                        contract_document_count: 3,
                        sketch_count: 3,
                        matched_sketch_count: 0,
                        failed_sketch_count: 3,
                    },
                    diagnostics: vec![
                        SketchDiagnostic::OccurrenceMismatch {
                            sketch: SketchLocation::new(
                                "duplicate",
                                CatalogPath::new("contracts/a.yaml").expect("contract path"),
                                0,
                                CatalogPath::new("src/a.rs").expect("source path"),
                            ),
                            expected: SketchOccurrence::ExactlyOne,
                            actual: 4,
                            spans: vec![SourceLineSpan::new(2, 3), SourceLineSpan::new(6, 7)],
                            spans_truncated: true,
                        },
                        SketchDiagnostic::NotMatched {
                            sketch: SketchLocation::new(
                                "invalid-bytes",
                                CatalogPath::new("contracts/b.yaml").expect("contract path"),
                                1,
                                CatalogPath::new("src/b.bin").expect("source path"),
                            ),
                            normalization: SketchNormalization::ExactLinesV1,
                            candidate: Some(MatchCandidate::new(
                                SourceLineSpan::new(10, 11),
                                2,
                                11,
                                DiagnosticExcerpt::Bytes {
                                    escaped: "a\\xff\\n".to_owned(),
                                    truncated: true,
                                },
                                DiagnosticExcerpt::missing(),
                            )),
                        },
                        SketchDiagnostic::MissingFile {
                            sketch: SketchLocation::new(
                                "missing",
                                CatalogPath::new("contracts/c.yaml").expect("contract path"),
                                2,
                                CatalogPath::new("src/missing.rs").expect("source path"),
                            ),
                        },
                    ],
                    report_files: FileCatalog::new(),
                },
            }
        }

        fn response(&self) -> &CheckResponse {
            &self.response
        }

        fn rendered_bytes(&self, format: ReportFormat, output: &str) -> Vec<u8> {
            let output_file = CatalogPath::new(output).expect("output path");
            let files = ReportRequest::Generate {
                format,
                output_file: output_file.clone(),
            }
            .render(
                self.response(),
                &OutputLimits::default(),
                &CancellationProbe::new(),
            )
            .expect("render report");

            files.get(&output_file).expect("report bytes").to_vec()
        }
    }

    #[test]
    fn none_request_returns_empty_catalog() {
        let response = TestResponse::with_rich_diagnostics();
        let files = ReportRequest::None
            .render(
                response.response(),
                &OutputLimits::default(),
                &CancellationProbe::new(),
            )
            .expect("render report");

        assert!(files.is_empty());
    }

    #[test]
    fn report_view_preserves_exact_standalone_and_embedded_wire_shape() {
        let mut report_files = FileCatalog::new();
        report_files
            .insert(
                CatalogPath::new("reports/nested.json").expect("report path"),
                b"nested".to_vec(),
            )
            .expect("report file");
        let response = CheckResponse {
            passed: true,
            counts: SketchCheckCounts {
                source_catalog_entry_count: 1,
                referenced_source_file_count: 1,
                present_referenced_source_file_count: 1,
                contract_document_count: 1,
                sketch_count: 1,
                matched_sketch_count: 1,
                failed_sketch_count: 0,
            },
            diagnostics: Vec::new(),
            report_files,
        };
        let view = CheckReportView::new(&response);

        assert_eq!(
            serde_json::to_string_pretty(&view).expect("report JSON"),
            concat!(
                "{\n",
                "  \"passed\": true,\n",
                "  \"counts\": {\n",
                "    \"source_catalog_entry_count\": 1,\n",
                "    \"referenced_source_file_count\": 1,\n",
                "    \"present_referenced_source_file_count\": 1,\n",
                "    \"contract_document_count\": 1,\n",
                "    \"sketch_count\": 1,\n",
                "    \"matched_sketch_count\": 1,\n",
                "    \"failed_sketch_count\": 0\n",
                "  },\n",
                "  \"diagnostics\": []\n",
                "}",
            )
        );
        assert_eq!(
            serde_saphyr::to_string(&view).expect("report YAML"),
            concat!(
                "passed: true\n",
                "counts:\n",
                "  source_catalog_entry_count: 1\n",
                "  referenced_source_file_count: 1\n",
                "  present_referenced_source_file_count: 1\n",
                "  contract_document_count: 1\n",
                "  sketch_count: 1\n",
                "  matched_sketch_count: 1\n",
                "  failed_sketch_count: 0\n",
                "diagnostics: []\n",
            )
        );
    }

    #[test]
    fn generated_report_bytes_respect_the_output_budget() {
        let response = TestResponse::with_rich_diagnostics();
        let error = ReportRequest::Generate {
            format: ReportFormat::Json,
            output_file: CatalogPath::new("report.json").expect("output path"),
        }
        .render(
            response.response(),
            &OutputLimits {
                generated_bytes: 1,
                ..OutputLimits::default()
            },
            &CancellationProbe::new(),
        )
        .expect_err("report must exceed one byte");

        assert_eq!(
            error.limit_exceeded().expect("typed limit").resource,
            LimitResource::GeneratedOutputBytes
        );
    }

    #[test]
    fn cancelled_report_serialization_preserves_the_typed_operation_error() {
        let response = TestResponse::with_rich_diagnostics();
        let cancellation = CancellationProbe::new();
        cancellation.cancel();

        for (format, output_file) in [
            (ReportFormat::Yaml, "report.yml"),
            (ReportFormat::Json, "report.json"),
        ] {
            let error = ReportRequest::Generate {
                format,
                output_file: CatalogPath::new(output_file).expect("output path"),
            }
            .render(response.response(), &OutputLimits::default(), &cancellation)
            .expect_err("cancelled report serialization must stop");

            assert!(error.is_operation_cancelled());
            assert!(error.limit_exceeded().is_none());
        }
    }

    #[derive(Debug, Deserialize, Eq, PartialEq)]
    struct ParsedSketchCheckReport {
        passed: bool,
        counts: SketchCheckCounts,
        diagnostics: Vec<SketchDiagnostic>,
    }

    impl ParsedSketchCheckReport {
        fn from_response(response: &CheckResponse) -> Self {
            Self {
                passed: response.passed,
                counts: response.counts.clone(),
                diagnostics: response.diagnostics.clone(),
            }
        }

        fn assert_rich_evidence(&self) {
            assert_eq!(self.diagnostics.len(), 3);
            assert_eq!(
                self.diagnostics[0],
                SketchDiagnostic::OccurrenceMismatch {
                    sketch: SketchLocation::new(
                        "duplicate",
                        CatalogPath::new("contracts/a.yaml").expect("contract path"),
                        0,
                        CatalogPath::new("src/a.rs").expect("source path"),
                    ),
                    expected: SketchOccurrence::ExactlyOne,
                    actual: 4,
                    spans: vec![SourceLineSpan::new(2, 3), SourceLineSpan::new(6, 7)],
                    spans_truncated: true,
                }
            );
            assert_eq!(
                self.diagnostics[1],
                SketchDiagnostic::NotMatched {
                    sketch: SketchLocation::new(
                        "invalid-bytes",
                        CatalogPath::new("contracts/b.yaml").expect("contract path"),
                        1,
                        CatalogPath::new("src/b.bin").expect("source path"),
                    ),
                    normalization: SketchNormalization::ExactLinesV1,
                    candidate: Some(MatchCandidate::new(
                        SourceLineSpan::new(10, 11),
                        2,
                        11,
                        DiagnosticExcerpt::Bytes {
                            escaped: "a\\xff\\n".to_owned(),
                            truncated: true,
                        },
                        DiagnosticExcerpt::Missing,
                    )),
                }
            );
            assert_eq!(
                self.diagnostics[2],
                SketchDiagnostic::MissingFile {
                    sketch: SketchLocation::new(
                        "missing",
                        CatalogPath::new("contracts/c.yaml").expect("contract path"),
                        2,
                        CatalogPath::new("src/missing.rs").expect("source path"),
                    ),
                }
            );
        }
    }

    #[test]
    fn yaml_report_uses_the_maintained_semantic_stack_and_round_trips() {
        let response = TestResponse::with_rich_diagnostics();
        let bytes = response.rendered_bytes(ReportFormat::Yaml, "reports/output.yml");
        let parsed = serde_saphyr::from_slice::<ParsedSketchCheckReport>(&bytes)
            .expect("semantic YAML report");

        assert_eq!(
            parsed,
            ParsedSketchCheckReport::from_response(response.response())
        );
        parsed.assert_rich_evidence();
        assert!(
            !std::str::from_utf8(&bytes)
                .expect("yaml is utf8")
                .contains("report_files")
        );
    }

    #[test]
    fn json_report_round_trips_complete_location_policy_and_evidence() {
        let response = TestResponse::with_rich_diagnostics();
        let bytes = response.rendered_bytes(ReportFormat::Json, "reports/output.json");
        let parsed =
            serde_json::from_slice::<ParsedSketchCheckReport>(&bytes).expect("JSON report");

        assert_eq!(
            parsed,
            ParsedSketchCheckReport::from_response(response.response())
        );
        parsed.assert_rich_evidence();
        assert!(
            !std::str::from_utf8(&bytes)
                .expect("json is utf8")
                .contains("report_files")
        );
    }

    #[test]
    fn json_report_has_stable_explicit_evidence_shape() {
        let response = TestResponse::with_rich_diagnostics();
        let bytes = response.rendered_bytes(ReportFormat::Json, "reports/output.json");
        let value = serde_json::from_slice::<serde_json::Value>(&bytes).expect("JSON report");

        assert_eq!(
            value["diagnostics"][0]["OccurrenceMismatch"]["sketch"]["document_index"],
            0
        );
        assert_eq!(
            value["diagnostics"][0]["OccurrenceMismatch"]["spans"][1]["end"],
            7
        );
        assert_eq!(
            value["diagnostics"][1]["NotMatched"]["normalization"],
            "exact_lines_v1"
        );
        assert_eq!(
            value["diagnostics"][1]["NotMatched"]["candidate"]["expected"]["Bytes"]["escaped"],
            "a\\xff\\n"
        );
        assert_eq!(
            value["diagnostics"][1]["NotMatched"]["candidate"]["expected"]["Bytes"]["truncated"],
            true
        );
        assert_eq!(
            value["diagnostics"][1]["NotMatched"]["candidate"]["actual"],
            "Missing"
        );
    }

    #[test]
    fn report_serialization_is_deterministic_in_both_formats() {
        let response = TestResponse::with_rich_diagnostics();

        for (format, output) in [
            (ReportFormat::Yaml, "reports/output.yml"),
            (ReportFormat::Json, "reports/output.json"),
        ] {
            let first = response.rendered_bytes(format, output);
            let second = response.rendered_bytes(format, output);

            assert_eq!(first, second);
        }
    }
}
