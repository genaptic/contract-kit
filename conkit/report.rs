//! Check report destination handling.
//!
//! Check commands can request generated reports from domain crates. This module
//! maps the user-selected output path to a report format and logical catalog
//! path, renders CLI-owned all-family reports, and replaces each report
//! individually and atomically. Rendering and copying use a cancellation-aware
//! hard byte ceiling whose first boundary failure remains authoritative through
//! later writes and flushes before commit.

use std::io::Write;
use std::path::{Path, PathBuf};

use atomic_write_file::AtomicWriteFile;
use conkit_signature::{
    CatalogPath as SignatureCatalogPath, FileCatalog as SignatureFileCatalog,
    ReportFormat as SignatureReportFormat, ReportRequest as SignatureReportRequest,
};
use serde::{Serialize, Serializer};

use crate::bounded_output::{BoundedOutput, BoundedOutputFailure};
use crate::context::ApplicationCancellation;
use crate::error::CliError;
use crate::platform::PortablePathRules;

/// User-selected report output file.
#[derive(Debug)]
pub(crate) struct ReportDestination {
    path: PathBuf,
    format: ReportFormatSelection,
    max_output_bytes: u64,
    cancellation: ApplicationCancellation,
}

impl ReportDestination {
    /// Creates a report destination for a local path.
    ///
    /// # Errors
    ///
    /// Returns an error when the path has no file name, uses a non-UTF-8 or
    /// non-portable file-name component, or does not end in a supported YAML
    /// or JSON extension.
    pub(crate) fn new(
        path: PathBuf,
        max_output_bytes: u64,
        cancellation: &ApplicationCancellation,
    ) -> Result<Self, CliError> {
        let format = ReportFormatSelection::from_path(&path)?;
        let destination = Self {
            path,
            format,
            max_output_bytes,
            cancellation: cancellation.clone(),
        };
        destination.catalog_path_value()?;

        Ok(destination)
    }

    /// Builds the signature-domain report request.
    ///
    /// The domain crate receives a logical `reports/<file-name>` path rather
    /// than the caller's absolute or platform-specific output path.
    ///
    /// # Errors
    ///
    /// Returns an error if the selected file name cannot be represented as a
    /// valid signature-domain catalog path.
    pub(crate) fn to_signature_request(&self) -> Result<SignatureReportRequest, CliError> {
        Ok(SignatureReportRequest::Generate {
            format: self.signature_format(),
            output_file: self.signature_catalog_path()?,
        })
    }

    /// Builds the sketch-domain report request.
    ///
    /// The domain crate receives a logical `reports/<file-name>` path rather
    /// than the caller's absolute or platform-specific output path.
    ///
    /// # Errors
    ///
    /// Returns an error if the selected file name cannot be represented as a
    /// valid sketch-domain catalog path.
    pub(crate) fn to_sketch_request(&self) -> Result<conkit_sketch::ReportRequest, CliError> {
        Ok(conkit_sketch::ReportRequest::Generate {
            format: self.sketch_format(),
            output_file: self.sketch_catalog_path()?,
        })
    }

    /// Writes the single report payload returned by the `conkit-signature` crate.
    ///
    /// # Errors
    ///
    /// Returns an error if no report bytes were returned, the report exceeds
    /// the configured output limit, parent directory creation fails, or the
    /// atomic write cannot be committed.
    pub(crate) fn write_signature_report(
        &self,
        files: &SignatureFileCatalog,
    ) -> Result<(), CliError> {
        let bytes = files
            .iter()
            .next()
            .map(|(_, bytes)| bytes)
            .ok_or(CliError::MissingReportBytes)?;

        self.write_bytes(bytes)
    }

    /// Writes the single report payload returned by the `conkit-sketch` crate.
    ///
    /// # Errors
    ///
    /// Returns an error if no report bytes were returned, the report exceeds
    /// the configured output limit, parent directory creation fails, or the
    /// atomic write cannot be committed.
    pub(crate) fn write_sketch_report(
        &self,
        files: &conkit_sketch::FileCatalog,
    ) -> Result<(), CliError> {
        let bytes = files
            .iter()
            .next()
            .map(|(_, bytes)| bytes)
            .ok_or(CliError::MissingReportBytes)?;

        self.write_bytes(bytes)
    }

    /// Writes a CLI-owned report that combines signature and sketch responses.
    ///
    /// # Errors
    ///
    /// Returns an error if YAML or JSON rendering fails, rendering crosses the
    /// configured output limit, parent directory creation fails, or the atomic
    /// write cannot be committed.
    pub(crate) fn write_all_check_report(
        &self,
        signatures: &conkit_signature::CheckResponse,
        sketches: &conkit_sketch::CheckResponse,
    ) -> Result<(), CliError> {
        self.cancellation.checkpoint()?;
        let report = AllCheckReport::new(signatures, sketches);
        let mut output = self.open_output()?;
        let rendered = match self.format {
            ReportFormatSelection::Yaml => serde_saphyr::to_io_writer(&mut output, &report)
                .map_err(|source| source.to_string()),
            ReportFormatSelection::Json => serde_json::to_writer_pretty(&mut output, &report)
                .map_err(|source| source.to_string()),
        };
        if let Err(message) = rendered {
            self.cancellation.checkpoint()?;
            return match output.failure() {
                Some(failure) => Err(self.output_failure_error(failure)),
                None => Err(CliError::ReportRender {
                    path: self.path.clone(),
                    message,
                }),
            };
        }

        self.commit_output(output)
    }

    fn signature_format(&self) -> SignatureReportFormat {
        match self.format {
            ReportFormatSelection::Yaml => SignatureReportFormat::Yaml,
            ReportFormatSelection::Json => SignatureReportFormat::Json,
        }
    }

    fn sketch_format(&self) -> conkit_sketch::ReportFormat {
        match self.format {
            ReportFormatSelection::Yaml => conkit_sketch::ReportFormat::Yaml,
            ReportFormatSelection::Json => conkit_sketch::ReportFormat::Json,
        }
    }

    /// Converts the local output file name into a logical report catalog path.
    ///
    /// # Errors
    ///
    /// Returns an error when the output has no portable UTF-8 file name or the
    /// derived `reports/<file-name>` value is not a valid signature catalog path.
    fn signature_catalog_path(&self) -> Result<SignatureCatalogPath, CliError> {
        SignatureCatalogPath::new(self.catalog_path_value()?).map_err(CliError::from)
    }

    fn sketch_catalog_path(&self) -> Result<conkit_sketch::CatalogPath, CliError> {
        conkit_sketch::CatalogPath::new(self.catalog_path_value()?).map_err(CliError::from)
    }

    fn catalog_path_value(&self) -> Result<String, CliError> {
        let file_name = self
            .path
            .file_name()
            .ok_or_else(|| CliError::MissingFileName {
                path: self.path.clone(),
            })?;

        PortablePathRules::validate_component(file_name)?;

        let file_name = file_name.to_str().ok_or(CliError::NonUtf8PathComponent)?;
        Ok(format!("reports/{file_name}"))
    }

    fn write_bytes(&self, bytes: &[u8]) -> Result<(), CliError> {
        self.cancellation.checkpoint()?;
        let observed_at_least = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if observed_at_least > self.max_output_bytes {
            return Err(CliError::ReportOutputLimit {
                path: self.path.clone(),
                limit: self.max_output_bytes,
                observed_at_least,
            });
        }

        let mut output = self.open_output()?;
        if let Err(source) = output.write_all(bytes) {
            self.cancellation.checkpoint()?;
            return match output.failure() {
                Some(failure) => Err(self.output_failure_error(failure)),
                None => Err(CliError::Io(source)),
            };
        }
        self.commit_output(output)
    }

    fn open_output(&self) -> Result<BoundedOutput<'_, AtomicWriteFile>, CliError> {
        if let Some(parent) = self.path.parent() {
            fs_err::create_dir_all(parent)?;
        }

        Ok(BoundedOutput::new(
            AtomicWriteFile::open(&self.path)?,
            &self.cancellation,
            self.max_output_bytes,
        ))
    }

    fn commit_output(
        &self,
        mut output: BoundedOutput<'_, AtomicWriteFile>,
    ) -> Result<(), CliError> {
        if let Err(source) = output.flush() {
            return Err(match output.failure() {
                Some(failure) => self.output_failure_error(failure),
                None => CliError::Io(source),
            });
        }
        self.cancellation.checkpoint()?;
        output.into_inner().commit()?;
        Ok(())
    }

    fn output_failure_error(&self, failure: BoundedOutputFailure) -> CliError {
        match failure {
            BoundedOutputFailure::Cancelled => CliError::OperationCanceled,
            BoundedOutputFailure::Limit { observed_at_least } => CliError::ReportOutputLimit {
                path: self.path.clone(),
                limit: self.max_output_bytes,
                observed_at_least,
            },
        }
    }
}

/// One individually atomic report write with a hard output-byte ceiling.
#[derive(Debug, Clone, Copy)]
enum ReportFormatSelection {
    Yaml,
    Json,
}

impl ReportFormatSelection {
    fn from_path(path: &Path) -> Result<Self, CliError> {
        let Some(extension) = path.extension() else {
            return Err(CliError::UnsupportedReportExtension {
                path: path.to_path_buf(),
            });
        };

        if extension.eq_ignore_ascii_case("yml") || extension.eq_ignore_ascii_case("yaml") {
            Ok(Self::Yaml)
        } else if extension.eq_ignore_ascii_case("json") {
            Ok(Self::Json)
        } else {
            Err(CliError::UnsupportedReportExtension {
                path: path.to_path_buf(),
            })
        }
    }
}

struct AllCheckReport<'a> {
    signatures: &'a conkit_signature::CheckResponse,
    sketches: &'a conkit_sketch::CheckResponse,
}

impl<'a> AllCheckReport<'a> {
    fn new(
        signatures: &'a conkit_signature::CheckResponse,
        sketches: &'a conkit_sketch::CheckResponse,
    ) -> Self {
        Self {
            signatures,
            sketches,
        }
    }
}

impl Serialize for AllCheckReport<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeStruct as _;

        let mut report = serializer.serialize_struct("AllCheckReport", 3)?;
        report.serialize_field("passed", &(self.signatures.passed && self.sketches.passed))?;
        report.serialize_field("signatures", &self.signatures.embedded_report_view())?;
        report.serialize_field("sketches", &self.sketches.report_view())?;
        report.end()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::path::PathBuf;

    use assert_fs::TempDir;
    use assert_fs::prelude::PathChild;
    use conkit_signature::{CatalogPath, FileCatalog, ReportFormat, ReportRequest};

    use super::ReportDestination;
    use crate::context::ApplicationCancellation;
    use crate::error::CliError;

    const TEST_REPORT_LIMIT: u64 = 1024 * 1024;

    fn destination(path: PathBuf, limit: u64) -> Result<ReportDestination, CliError> {
        ReportDestination::new(path, limit, &ApplicationCancellation::new())
    }

    #[test]
    fn yml_and_yaml_infer_yaml() {
        assert!(matches!(
            destination(PathBuf::from("output.yml"), TEST_REPORT_LIMIT)
                .expect("report destination")
                .to_signature_request()
                .expect("report request"),
            ReportRequest::Generate {
                format: ReportFormat::Yaml,
                ..
            }
        ));
        assert!(matches!(
            destination(PathBuf::from("output.yaml"), TEST_REPORT_LIMIT)
                .expect("report destination")
                .to_signature_request()
                .expect("report request"),
            ReportRequest::Generate {
                format: ReportFormat::Yaml,
                ..
            }
        ));
    }

    #[test]
    fn json_infers_json() {
        assert!(matches!(
            destination(PathBuf::from("output.json"), TEST_REPORT_LIMIT)
                .expect("report destination")
                .to_signature_request()
                .expect("report request"),
            ReportRequest::Generate {
                format: ReportFormat::Json,
                ..
            }
        ));
    }

    #[test]
    fn mixed_case_extensions_infer_formats() {
        assert!(matches!(
            destination(PathBuf::from("output.YmL"), TEST_REPORT_LIMIT)
                .expect("report destination")
                .to_signature_request()
                .expect("YAML report request"),
            ReportRequest::Generate {
                format: ReportFormat::Yaml,
                ..
            }
        ));
        assert!(matches!(
            destination(PathBuf::from("output.JSON"), TEST_REPORT_LIMIT)
                .expect("report destination")
                .to_signature_request()
                .expect("JSON report request"),
            ReportRequest::Generate {
                format: ReportFormat::Json,
                ..
            }
        ));
    }

    #[test]
    fn unsupported_extension_fails() {
        let error = destination(PathBuf::from("output.txt"), TEST_REPORT_LIMIT)
            .expect_err("unsupported extension");

        assert!(error.to_string().contains("unsupported report extension"));
    }

    #[test]
    fn report_request_uses_logical_catalog_path_not_absolute_path() {
        let temp = TempDir::new().expect("temporary report directory");
        let output = temp.child("output.yml");
        let request = destination(output.path().to_path_buf(), TEST_REPORT_LIMIT)
            .expect("report destination")
            .to_signature_request()
            .expect("report request");

        match request {
            ReportRequest::Generate { output_file, .. } => {
                assert_eq!(output_file.as_str(), "reports/output.yml");
            }
            ReportRequest::None => panic!("expected report generation"),
        }
    }

    #[test]
    fn report_write_uses_requested_path() {
        let temp = TempDir::new().expect("temporary report directory");
        let output = temp.child("output.yml");
        let report_path = output.path().to_path_buf();
        let mut files = FileCatalog::new();
        files
            .insert(
                CatalogPath::new("reports/output.yml").expect("catalog path"),
                b"passed: true\n".to_vec(),
            )
            .expect("insert report");

        destination(report_path.clone(), TEST_REPORT_LIMIT)
            .expect("report destination")
            .write_signature_report(&files)
            .expect("write report");

        assert_eq!(
            std::fs::read_to_string(&report_path).expect("report"),
            "passed: true\n"
        );
    }

    #[test]
    fn report_output_accepts_the_exact_limit() {
        let temp = TempDir::new().expect("temporary report directory");
        let report_path = temp.child("exact.json").path().to_path_buf();
        let destination = destination(report_path.clone(), 3).expect("report destination");

        destination
            .write_bytes(b"abc")
            .expect("the exact report output limit must be accepted");

        assert_eq!(std::fs::read(report_path).expect("report bytes"), b"abc");
    }

    #[test]
    fn canceled_report_write_does_not_publish_output() {
        let temp = TempDir::new().expect("temporary report directory");
        let report_path = temp.child("canceled.json").path().to_path_buf();
        let cancellation = ApplicationCancellation::new();
        cancellation.request();
        let destination =
            ReportDestination::new(report_path.clone(), TEST_REPORT_LIMIT, &cancellation)
                .expect("report destination");

        let error = destination
            .write_bytes(b"not published")
            .expect_err("a canceled report must not be published");

        assert!(matches!(error, CliError::OperationCanceled));
        assert!(!report_path.exists());
    }

    #[test]
    fn cancellation_after_report_bytes_before_flush_is_typed_and_does_not_publish_output() {
        let temp = TempDir::new().expect("temporary report directory");
        let report_path = temp.child("canceled.json").path().to_path_buf();
        std::fs::write(&report_path, b"existing report").expect("existing report");
        let cancellation = ApplicationCancellation::new();
        let destination =
            ReportDestination::new(report_path.clone(), TEST_REPORT_LIMIT, &cancellation)
                .expect("report destination");
        let mut output = destination.open_output().expect("atomic report output");
        output
            .write_all(b"replacement report")
            .expect("replacement report bytes");
        cancellation.request();

        let error = destination
            .commit_output(output)
            .expect_err("cancellation before flush must prevent publication");

        assert!(matches!(error, CliError::OperationCanceled));
        assert_eq!(
            std::fs::read(&report_path).expect("preserved report bytes"),
            b"existing report"
        );
        temp.close().expect("cleanup");
    }

    #[test]
    fn report_output_rejects_limit_plus_one_before_opening_the_destination() {
        let temp = TempDir::new().expect("temporary report directory");
        let report_path = temp.child("oversized.json").path().to_path_buf();
        let destination = destination(report_path.clone(), 3).expect("report destination");

        let error = destination
            .write_bytes(b"abcd")
            .expect_err("limit plus one must be rejected");

        assert!(matches!(
            error,
            crate::error::CliError::ReportOutputLimit {
                path,
                limit: 3,
                observed_at_least: 4,
            } if path == report_path
        ));
        assert!(!report_path.exists());
    }

    #[test]
    fn combined_yaml_report_uses_the_maintained_semantic_serializer() {
        let temp = TempDir::new().expect("temporary report directory");
        let output = temp.child("combined.yml");
        let report_destination = destination(output.path().to_path_buf(), TEST_REPORT_LIMIT)
            .expect("report destination");
        let signatures = conkit_signature::CheckResponse {
            passed: true,
            counts: conkit_signature::SignatureCheckCounts {
                source_signature_count: 1,
                contract_signature_count: 1,
            },
            source_shape_digest: "source-shape-digest".to_owned(),
            digest_version: 2,
            diagnostics: Vec::new(),
            report_files: FileCatalog::new(),
        };
        let sketches = conkit_sketch::CheckResponse {
            passed: true,
            counts: conkit_sketch::SketchCheckCounts {
                source_catalog_entry_count: 4,
                referenced_source_file_count: 3,
                present_referenced_source_file_count: 2,
                contract_document_count: 2,
                sketch_count: 3,
                matched_sketch_count: 2,
                failed_sketch_count: 1,
            },
            diagnostics: vec![conkit_sketch::SketchDiagnostic::MissingFile {
                sketch: conkit_sketch::SketchLocation {
                    sketch_id: "missing".to_owned(),
                    contract_file: conkit_sketch::CatalogPath::new("main.yml")
                        .expect("contract path"),
                    document_index: 1,
                    source_file: conkit_sketch::CatalogPath::new("src/missing.rs")
                        .expect("source path"),
                },
            }],
            report_files: conkit_sketch::FileCatalog::new(),
        };

        report_destination
            .write_all_check_report(&signatures, &sketches)
            .expect("write combined YAML report");
        let first = std::fs::read(output.path()).expect("first report bytes");
        let parsed: serde_json::Value =
            serde_saphyr::from_slice(&first).expect("maintained parser accepts report");

        assert_eq!(parsed["passed"], true);
        assert_eq!(parsed["signatures"]["counts"]["source_signature_count"], 1);
        assert_eq!(
            parsed["signatures"]["source_shape_digest"],
            "source-shape-digest"
        );
        assert_eq!(parsed["signatures"]["digest_version"], 2);
        assert_eq!(
            parsed["sketches"]["counts"]["source_catalog_entry_count"],
            4
        );
        assert_eq!(
            parsed["sketches"]["counts"]["referenced_source_file_count"],
            3
        );
        assert_eq!(
            parsed["sketches"]["counts"]["present_referenced_source_file_count"],
            2
        );
        assert_eq!(parsed["sketches"]["counts"]["contract_document_count"], 2);
        assert_eq!(
            parsed["sketches"]["diagnostics"][0]["MissingFile"]["sketch"]["document_index"],
            1
        );
        report_destination
            .write_all_check_report(&signatures, &sketches)
            .expect("repeat combined YAML report");
        assert_eq!(
            std::fs::read(output.path()).expect("second report bytes"),
            first
        );

        let limited_path = temp.child("limited.yml").path().to_path_buf();
        std::fs::write(&limited_path, b"existing report").expect("existing report");
        let limited = destination(limited_path.clone(), 1).expect("limited destination");
        let error = limited
            .write_all_check_report(&signatures, &sketches)
            .expect_err("streamed combined report must honor the output limit");
        assert!(matches!(
            error,
            crate::error::CliError::ReportOutputLimit {
                path,
                limit: 1,
                ..
            } if path == limited_path
        ));
        assert_eq!(
            std::fs::read(limited_path).expect("preserved report bytes"),
            b"existing report"
        );
    }

    #[test]
    fn combined_report_bytes_match_checked_in_yaml_and_json_goldens() {
        let temp = TempDir::new().expect("temporary report directory");
        let mut signature_report_files = FileCatalog::new();
        signature_report_files
            .insert(
                CatalogPath::new("reports/nested.json").expect("signature report path"),
                b"nested".to_vec(),
            )
            .expect("signature report file");
        let mut sketch_report_files = conkit_sketch::FileCatalog::new();
        sketch_report_files
            .insert(
                conkit_sketch::CatalogPath::new("reports/nested.json").expect("sketch report path"),
                b"nested".to_vec(),
            )
            .expect("sketch report file");
        let mut signatures = conkit_signature::CheckResponse {
            passed: true,
            source_shape_digest: "9c4a93d7bca1a78e3514645d0e6906d5e7fb1d68e16e8002155f8f800bc51525"
                .to_owned(),
            digest_version: 2,
            counts: conkit_signature::SignatureCheckCounts {
                source_signature_count: 1,
                contract_signature_count: 1,
            },
            diagnostics: Vec::new(),
            report_files: signature_report_files,
        };
        let sketches = conkit_sketch::CheckResponse {
            passed: true,
            counts: conkit_sketch::SketchCheckCounts {
                source_catalog_entry_count: 1,
                referenced_source_file_count: 1,
                present_referenced_source_file_count: 1,
                contract_document_count: 1,
                sketch_count: 1,
                matched_sketch_count: 1,
                failed_sketch_count: 0,
            },
            diagnostics: Vec::new(),
            report_files: sketch_report_files,
        };

        let json = temp.child("combined.json");
        destination(json.path().to_path_buf(), TEST_REPORT_LIMIT)
            .expect("JSON destination")
            .write_all_check_report(&signatures, &sketches)
            .expect("combined JSON report");
        assert_eq!(
            std::fs::read(json.path()).expect("combined JSON bytes"),
            include_bytes!(
                "../test/scenarios/check-rust/matrix-all-default-matching/output/report.json"
            )
        );

        signatures.passed = false;
        signatures.diagnostics = vec![conkit_signature::CheckDiagnostic::Mismatched {
            signature_id: "main.yml::document:0::answer".to_owned(),
            expected_digest: "ee547bfe03a5625ef1fc5fe94769de79b6e1cdfd54027ffca0e40050c9925d5d"
                .to_owned(),
            actual_digest: "09c30fb0d814208de56b221fea9b0327022c607edb43f94ffe191933ab050294"
                .to_owned(),
        }];
        let yaml = temp.child("combined.yml");
        destination(yaml.path().to_path_buf(), TEST_REPORT_LIMIT)
            .expect("YAML destination")
            .write_all_check_report(&signatures, &sketches)
            .expect("combined YAML report");
        assert_eq!(
            std::fs::read(yaml.path()).expect("combined YAML bytes"),
            include_bytes!(
                "../test/scenarios/check-rust/combined-signatures-fail/output/report.yml"
            )
        );
        temp.close().expect("cleanup");
    }
}
