//! User-facing terminal output.
//!
//! Command handlers call this module after domain work succeeds. Keeping
//! output here prevents domain crates from depending on stdout, formatting
//! choices, or process-level reporting behavior.

use std::io::{self, Write};
use std::path::Path;

/// Counts needed to render one successful generation summary.
pub(crate) enum GenerateSummary {
    /// Signature-only generation completed.
    Signatures { count: usize },
    /// Sketch-only generation completed.
    Sketches { count: usize },
    /// Both generation domains completed.
    All {
        /// Number of grouped signature records generated.
        signature_count: usize,
        /// Number of explicitly linked sketches refreshed.
        sketch_count: usize,
    },
}

/// Formatter for successful command summaries.
pub(crate) struct Output;

impl Output {
    /// Prints the summary for a passing signature check.
    ///
    /// # Errors
    ///
    /// Returns an error if the summary cannot be written to standard output.
    pub(crate) fn print_check_summary(
        &self,
        response: &conkit_signature::CheckResponse,
    ) -> io::Result<()> {
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        self.write_check_summary(&mut writer, response)
    }

    fn write_check_summary<W>(
        &self,
        writer: &mut W,
        response: &conkit_signature::CheckResponse,
    ) -> io::Result<()>
    where
        W: Write + ?Sized,
    {
        writeln!(
            writer,
            "signature check passed: {} source signatures, {} contract signatures",
            response.counts.source_signature_count, response.counts.contract_signature_count
        )
    }

    /// Prints the summary for a passing sketch check.
    ///
    /// # Errors
    ///
    /// Returns an error if the summary cannot be written to standard output.
    pub(crate) fn print_sketch_check_summary(
        &self,
        response: &conkit_sketch::CheckResponse,
    ) -> io::Result<()> {
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        self.write_sketch_check_summary(&mut writer, response)
    }

    fn write_sketch_check_summary<W>(
        &self,
        writer: &mut W,
        response: &conkit_sketch::CheckResponse,
    ) -> io::Result<()>
    where
        W: Write + ?Sized,
    {
        writeln!(
            writer,
            "sketch check passed: {} matched sketches, {} failed sketches",
            response.counts.matched_sketch_count, response.counts.failed_sketch_count
        )
    }

    /// Prints the summary for a passing all-family check.
    ///
    /// # Errors
    ///
    /// Returns an error if the summary cannot be written to standard output.
    pub(crate) fn print_all_check_summary(
        &self,
        signatures: &conkit_signature::CheckResponse,
        sketches: &conkit_sketch::CheckResponse,
    ) -> io::Result<()> {
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        self.write_all_check_summary(&mut writer, signatures, sketches)
    }

    fn write_all_check_summary<W>(
        &self,
        writer: &mut W,
        signatures: &conkit_signature::CheckResponse,
        sketches: &conkit_sketch::CheckResponse,
    ) -> io::Result<()>
    where
        W: Write + ?Sized,
    {
        writeln!(
            writer,
            "contract check passed: {} signatures, {} sketches",
            signatures.counts.contract_signature_count, sketches.counts.sketch_count
        )
    }

    /// Prints the summary for the completed generation target.
    ///
    /// # Errors
    ///
    /// Returns an error if the summary cannot be written to standard output.
    pub(crate) fn print_generate_summary(&self, summary: GenerateSummary) -> io::Result<()> {
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        self.write_generate_summary(&mut writer, summary)
    }

    fn write_generate_summary<W>(&self, writer: &mut W, summary: GenerateSummary) -> io::Result<()>
    where
        W: Write + ?Sized,
    {
        match summary {
            GenerateSummary::Signatures { count } => {
                writeln!(writer, "generated {count} signature contracts")
            }
            GenerateSummary::Sketches { count } => {
                writeln!(writer, "generated {count} sketch contracts")
            }
            GenerateSummary::All {
                signature_count,
                sketch_count,
            } => writeln!(
                writer,
                "generated {signature_count} signature contracts and {sketch_count} sketch contracts"
            ),
        }
    }

    /// Prints the number of matching existing outputs adopted during generation.
    ///
    /// # Errors
    ///
    /// Returns an error if the summary cannot be written to standard output.
    pub(crate) fn print_adoption_summary(&self, adopted_count: usize) -> io::Result<()> {
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        self.write_adoption_summary(&mut writer, adopted_count)
    }

    fn write_adoption_summary<W>(&self, writer: &mut W, adopted_count: usize) -> io::Result<()>
    where
        W: Write + ?Sized,
    {
        writeln!(
            writer,
            "adopted {adopted_count} matching existing contract files"
        )
    }

    /// Prints the path of a newly written archive file.
    ///
    /// # Errors
    ///
    /// Returns an error if the summary cannot be written to standard output.
    pub(crate) fn print_archive_summary(&self, path: &Path) -> io::Result<()> {
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        self.write_archive_summary(&mut writer, path)
    }

    fn write_archive_summary<W>(&self, writer: &mut W, path: &Path) -> io::Result<()>
    where
        W: Write + ?Sized,
    {
        writeln!(writer, "archived contracts to {}", path.display())
    }

    /// Prints a human-readable diff summary.
    ///
    /// # Errors
    ///
    /// Returns an error if the summary cannot be written to standard output.
    pub(crate) fn print_diff(
        &self,
        signatures: &conkit_signature::DiffResponse,
        sketches: &conkit_sketch::DiffResponse,
    ) -> io::Result<()> {
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        self.write_diff(&mut writer, signatures, sketches)
    }

    fn write_diff<W>(
        &self,
        writer: &mut W,
        signatures: &conkit_signature::DiffResponse,
        sketches: &conkit_sketch::DiffResponse,
    ) -> io::Result<()>
    where
        W: Write + ?Sized,
    {
        if signatures.changed || sketches.changed {
            writeln!(writer, "contracts changed")?;
        } else {
            writeln!(writer, "contracts unchanged")?;
        }

        for entry in &signatures.entries {
            match entry {
                conkit_signature::DiffEntry::Added { signature_id } => {
                    writeln!(writer, "signature added {signature_id}")?;
                }
                conkit_signature::DiffEntry::Removed { signature_id } => {
                    writeln!(writer, "signature removed {signature_id}")?;
                }
                conkit_signature::DiffEntry::Changed { signature_id, .. } => {
                    writeln!(writer, "signature changed {signature_id}")?;
                }
            }
        }

        for entry in &sketches.entries {
            match entry {
                conkit_sketch::DiffEntry::Added { sketch_id } => {
                    writeln!(writer, "sketch added {sketch_id}")?;
                }
                conkit_sketch::DiffEntry::Removed { sketch_id } => {
                    writeln!(writer, "sketch removed {sketch_id}")?;
                }
                conkit_sketch::DiffEntry::Changed { sketch_id } => {
                    writeln!(writer, "sketch changed {sketch_id}")?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::path::Path;

    use super::{GenerateSummary, Output};

    struct BrokenWriter;

    impl Write for BrokenWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed output"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn writes_exact_summary_bytes() {
        let output = Output;
        let signatures = conkit_signature::CheckResponse {
            passed: true,
            counts: conkit_signature::SignatureCheckCounts {
                source_signature_count: 3,
                contract_signature_count: 2,
            },
            inventory_digest: Some("digest".to_owned()),
            diagnostics: Vec::new(),
            report_files: conkit_signature::FileCatalog::new(),
        };
        let sketches = conkit_sketch::CheckResponse {
            passed: true,
            counts: conkit_sketch::SketchCheckCounts {
                source_file_count: 1,
                contract_file_count: 1,
                sketch_count: 4,
                matched_sketch_count: 4,
                failed_sketch_count: 0,
            },
            diagnostics: Vec::new(),
            report_files: conkit_sketch::FileCatalog::new(),
        };
        let mut bytes = Vec::new();

        output
            .write_check_summary(&mut bytes, &signatures)
            .expect("signature summary");
        output
            .write_sketch_check_summary(&mut bytes, &sketches)
            .expect("sketch summary");
        output
            .write_all_check_summary(&mut bytes, &signatures, &sketches)
            .expect("all-family summary");
        output
            .write_generate_summary(&mut bytes, GenerateSummary::Signatures { count: 2 })
            .expect("signature generation summary");
        output
            .write_generate_summary(&mut bytes, GenerateSummary::Sketches { count: 4 })
            .expect("sketch generation summary");
        output
            .write_generate_summary(
                &mut bytes,
                GenerateSummary::All {
                    signature_count: 2,
                    sketch_count: 4,
                },
            )
            .expect("all-family generation summary");
        output
            .write_adoption_summary(&mut bytes, 1)
            .expect("adoption summary");
        output
            .write_archive_summary(&mut bytes, Path::new("archives/contracts.gzip"))
            .expect("archive summary");

        assert_eq!(
            String::from_utf8(bytes).expect("UTF-8 output"),
            "signature check passed: 3 source signatures, 2 contract signatures\n\
             sketch check passed: 4 matched sketches, 0 failed sketches\n\
             contract check passed: 2 signatures, 4 sketches\n\
             generated 2 signature contracts\n\
             generated 4 sketch contracts\n\
             generated 2 signature contracts and 4 sketch contracts\n\
             adopted 1 matching existing contract files\n\
             archived contracts to archives/contracts.gzip\n"
        );
    }

    #[test]
    fn writes_diff_in_signature_then_sketch_order() {
        let signatures = conkit_signature::DiffResponse {
            changed: true,
            entries: vec![
                conkit_signature::DiffEntry::Added {
                    signature_id: "added_signature".to_owned(),
                },
                conkit_signature::DiffEntry::Removed {
                    signature_id: "removed_signature".to_owned(),
                },
                conkit_signature::DiffEntry::Changed {
                    signature_id: "changed_signature".to_owned(),
                    current_digest: "current".to_owned(),
                    previous_digest: "previous".to_owned(),
                },
            ],
        };
        let sketches = conkit_sketch::DiffResponse {
            changed: true,
            entries: vec![
                conkit_sketch::DiffEntry::Added {
                    sketch_id: "added_sketch".to_owned(),
                },
                conkit_sketch::DiffEntry::Removed {
                    sketch_id: "removed_sketch".to_owned(),
                },
                conkit_sketch::DiffEntry::Changed {
                    sketch_id: "changed_sketch".to_owned(),
                },
            ],
        };
        let mut bytes = Vec::new();

        Output
            .write_diff(&mut bytes, &signatures, &sketches)
            .expect("diff output");

        assert_eq!(
            String::from_utf8(bytes).expect("UTF-8 output"),
            "contracts changed\n\
             signature added added_signature\n\
             signature removed removed_signature\n\
             signature changed changed_signature\n\
             sketch added added_sketch\n\
             sketch removed removed_sketch\n\
             sketch changed changed_sketch\n"
        );
    }

    #[test]
    fn writes_unchanged_diff_status() {
        let signatures = conkit_signature::DiffResponse {
            changed: false,
            entries: Vec::new(),
        };
        let sketches = conkit_sketch::DiffResponse {
            changed: false,
            entries: Vec::new(),
        };
        let mut bytes = Vec::new();

        Output
            .write_diff(&mut bytes, &signatures, &sketches)
            .expect("diff output");

        assert_eq!(bytes, b"contracts unchanged\n");
    }

    #[test]
    fn propagates_broken_pipe_errors() {
        let error = Output
            .write_adoption_summary(&mut BrokenWriter, 1)
            .expect_err("broken writer should fail");

        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }
}
