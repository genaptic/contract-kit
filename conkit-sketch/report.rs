use crate::api::CheckResponse;
use crate::error::SketchContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::inventory::{SketchCheckCounts, SketchDiagnostic};
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

pub(crate) struct ReportFiles {
    request: ReportRequest,
}

impl ReportFiles {
    pub(crate) fn new(request: ReportRequest) -> Self {
        Self { request }
    }

    pub(crate) fn render(
        &self,
        response: &CheckResponse,
    ) -> Result<FileCatalog, SketchContractKitError> {
        match &self.request {
            ReportRequest::None => Ok(FileCatalog::new()),
            ReportRequest::Generate {
                format,
                output_file,
            } => {
                let report = SketchCheckReport::from_response(response);
                let bytes = match format {
                    ReportFormat::Yaml => serde_yaml::to_string(&report)
                        .map(String::into_bytes)
                        .map_err(|source| {
                            SketchContractKitError::write_failed(output_file, source.to_string())
                        })?,
                    ReportFormat::Json => serde_json::to_vec_pretty(&report).map_err(|source| {
                        SketchContractKitError::write_failed(output_file, source.to_string())
                    })?,
                };
                let mut files = FileCatalog::new();
                files.insert(output_file.clone(), bytes)?;

                Ok(files)
            }
        }
    }
}

#[derive(Serialize)]
struct SketchCheckReport<'a> {
    passed: bool,
    counts: &'a SketchCheckCounts,
    diagnostics: &'a [SketchDiagnostic],
}

impl<'a> SketchCheckReport<'a> {
    fn from_response(response: &'a CheckResponse) -> Self {
        Self {
            passed: response.passed,
            counts: &response.counts,
            diagnostics: &response.diagnostics,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ReportFiles, ReportFormat, ReportRequest};
    use crate::api::CheckResponse;
    use crate::files::{CatalogPath, FileCatalog};
    use crate::inventory::{SketchCheckCounts, SketchDiagnostic};

    struct TestResponse {
        response: CheckResponse,
    }

    impl TestResponse {
        fn with_diagnostic() -> Self {
            Self {
                response: CheckResponse {
                    passed: false,
                    counts: SketchCheckCounts {
                        source_file_count: 1,
                        contract_file_count: 1,
                        sketch_count: 1,
                        matched_sketch_count: 0,
                        failed_sketch_count: 1,
                    },
                    diagnostics: vec![SketchDiagnostic::NotMatched {
                        sketch_id: "answer".to_owned(),
                        file: "src/lib.rs".to_owned(),
                    }],
                    report_files: FileCatalog::new(),
                },
            }
        }

        fn response(&self) -> &CheckResponse {
            &self.response
        }
    }

    #[test]
    fn none_request_returns_empty_catalog() {
        let response = TestResponse::with_diagnostic();
        let files = ReportFiles::new(ReportRequest::None)
            .render(response.response())
            .expect("render report");

        assert!(files.is_empty());
    }

    #[test]
    fn yaml_report_includes_counts_and_diagnostics() {
        let response = TestResponse::with_diagnostic();
        let output_file = CatalogPath::new("reports/output.yml").expect("path");
        let files = ReportFiles::new(ReportRequest::Generate {
            format: ReportFormat::Yaml,
            output_file: output_file.clone(),
        })
        .render(response.response())
        .expect("render report");
        let yaml = std::str::from_utf8(files.get(&output_file).expect("report bytes"))
            .expect("yaml is utf8");

        assert!(yaml.contains("passed: false"));
        assert!(yaml.contains("sketch_count: 1"));
        assert!(yaml.contains("answer"));
    }

    #[test]
    fn json_report_includes_counts_and_diagnostics() {
        let response = TestResponse::with_diagnostic();
        let output_file = CatalogPath::new("reports/output.json").expect("path");
        let files = ReportFiles::new(ReportRequest::Generate {
            format: ReportFormat::Json,
            output_file: output_file.clone(),
        })
        .render(response.response())
        .expect("render report");
        let value = serde_json::from_slice::<serde_json::Value>(
            files.get(&output_file).expect("report bytes"),
        )
        .expect("json report");

        assert_eq!(value["passed"], false);
        assert_eq!(value["counts"]["sketch_count"], 1);
        assert_eq!(value["diagnostics"][0]["NotMatched"]["sketch_id"], "answer");
    }
}
