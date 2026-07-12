//! Check report destination handling.
//!
//! Check commands can request generated reports from domain crates. This module
//! maps the user-selected output path to a report format and logical catalog
//! path, renders CLI-owned all-family reports, then writes report bytes
//! atomically.

use std::io::Write;
use std::path::{Path, PathBuf};

use atomic_write_file::AtomicWriteFile;
use conkit_signature::{
    CatalogPath as SignatureCatalogPath, FileCatalog as SignatureFileCatalog,
    ReportFormat as SignatureReportFormat, ReportRequest as SignatureReportRequest,
};
use serde::Serialize;

use crate::error::CliError;
use crate::platform::PortablePathRules;

/// User-selected report output file.
#[derive(Debug)]
pub(crate) struct ReportDestination {
    path: PathBuf,
    format: ReportFormatSelection,
}

impl ReportDestination {
    /// Creates a report destination for a local path.
    ///
    /// # Errors
    ///
    /// Returns an error when the path has no file name, uses a non-UTF-8 or
    /// non-portable file-name component, or does not end in a supported YAML
    /// or JSON extension.
    pub(crate) fn new(path: PathBuf) -> Result<Self, CliError> {
        let format = ReportFormatSelection::from_path(&path)?;
        let destination = Self { path, format };
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
    /// Returns an error if no report bytes were returned, if parent directory
    /// creation fails, or if the atomic write cannot be committed.
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
    /// Returns an error if no report bytes were returned, if parent directory
    /// creation fails, or if the atomic write cannot be committed.
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
    /// Returns an error if YAML or JSON rendering fails, if parent directory
    /// creation fails, or if the atomic write cannot be committed.
    pub(crate) fn write_all_check_report(
        &self,
        signatures: &conkit_signature::CheckResponse,
        sketches: &conkit_sketch::CheckResponse,
    ) -> Result<(), CliError> {
        let report = AllCheckReport::new(signatures, sketches);
        let bytes = match self.format {
            ReportFormatSelection::Yaml => serde_yaml::to_string(&report)
                .map(String::into_bytes)
                .map_err(|source| CliError::ReportRender {
                path: self.path.clone(),
                message: source.to_string(),
            })?,
            ReportFormatSelection::Json => {
                serde_json::to_vec_pretty(&report).map_err(|source| CliError::ReportRender {
                    path: self.path.clone(),
                    message: source.to_string(),
                })?
            }
        };

        self.write_bytes(&bytes)
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
        if let Some(parent) = self.path.parent() {
            fs_err::create_dir_all(parent)?;
        }

        let mut output = AtomicWriteFile::open(&self.path)?;
        output.write_all(bytes)?;
        output.commit()?;

        Ok(())
    }
}

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

#[derive(Serialize)]
struct AllCheckReport<'a> {
    passed: bool,
    signatures: SignatureCheckReport<'a>,
    sketches: SketchCheckReport<'a>,
}

impl<'a> AllCheckReport<'a> {
    fn new(
        signatures: &'a conkit_signature::CheckResponse,
        sketches: &'a conkit_sketch::CheckResponse,
    ) -> Self {
        Self {
            passed: signatures.passed && sketches.passed,
            signatures: SignatureCheckReport::new(signatures),
            sketches: SketchCheckReport::new(sketches),
        }
    }
}

#[derive(Serialize)]
struct SignatureCheckReport<'a> {
    passed: bool,
    counts: &'a conkit_signature::SignatureCheckCounts,
    inventory_digest: &'a Option<String>,
    diagnostics: &'a [conkit_signature::CheckDiagnostic],
}

impl<'a> SignatureCheckReport<'a> {
    fn new(response: &'a conkit_signature::CheckResponse) -> Self {
        Self {
            passed: response.passed,
            counts: &response.counts,
            inventory_digest: &response.inventory_digest,
            diagnostics: &response.diagnostics,
        }
    }
}

#[derive(Serialize)]
struct SketchCheckReport<'a> {
    passed: bool,
    counts: &'a conkit_sketch::SketchCheckCounts,
    diagnostics: &'a [conkit_sketch::SketchDiagnostic],
}

impl<'a> SketchCheckReport<'a> {
    fn new(response: &'a conkit_sketch::CheckResponse) -> Self {
        Self {
            passed: response.passed,
            counts: &response.counts,
            diagnostics: &response.diagnostics,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use assert_fs::TempDir;
    use assert_fs::prelude::PathChild;
    use conkit_signature::{CatalogPath, FileCatalog, ReportFormat, ReportRequest};

    use super::ReportDestination;

    #[test]
    fn yml_and_yaml_infer_yaml() {
        assert!(matches!(
            ReportDestination::new(PathBuf::from("output.yml"))
                .expect("report destination")
                .to_signature_request()
                .expect("report request"),
            ReportRequest::Generate {
                format: ReportFormat::Yaml,
                ..
            }
        ));
        assert!(matches!(
            ReportDestination::new(PathBuf::from("output.yaml"))
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
            ReportDestination::new(PathBuf::from("output.json"))
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
            ReportDestination::new(PathBuf::from("output.YmL"))
                .expect("report destination")
                .to_signature_request()
                .expect("YAML report request"),
            ReportRequest::Generate {
                format: ReportFormat::Yaml,
                ..
            }
        ));
        assert!(matches!(
            ReportDestination::new(PathBuf::from("output.JSON"))
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
        let error =
            ReportDestination::new(PathBuf::from("output.txt")).expect_err("unsupported extension");

        assert!(error.to_string().contains("unsupported report extension"));
    }

    #[test]
    fn report_request_uses_logical_catalog_path_not_absolute_path() {
        let temp = TempDir::new().expect("temporary report directory");
        let output = temp.child("output.yml");
        let request = ReportDestination::new(output.path().to_path_buf())
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

        ReportDestination::new(report_path.clone())
            .expect("report destination")
            .write_signature_report(&files)
            .expect("write report");

        assert_eq!(
            std::fs::read_to_string(&report_path).expect("report"),
            "passed: true\n"
        );
    }
}
