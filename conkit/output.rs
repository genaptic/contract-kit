//! User-facing terminal output.
//!
//! Compiler and syntax-capability warnings are written to standard error.
//! Successful check, generation, archive, and diff summaries are written to
//! standard output only after their owning workflow succeeds. Keeping this
//! policy here prevents domain crates from depending on terminals or process-
//! level presentation.

use std::io::{self, Write};
use std::path::Path;

use crate::context::ApplicationCancellation;
use crate::error::CliError;

/// Counts needed to render one successful generation summary.
pub(crate) enum GenerateSummary {
    /// Signature-only generation completed.
    Signatures {
        /// Signature-domain document and semantic-change totals.
        counts: conkit_signature::SignatureGenerationCounts,
    },
    /// Sketch-only generation completed.
    Sketches {
        /// Sketch-domain linked, refreshed, and exact-change totals.
        counts: conkit_sketch::SketchGenerationCounts,
    },
    /// Both generation domains completed.
    All {
        /// Signature-domain document and semantic-change totals.
        signatures: conkit_signature::SignatureGenerationCounts,
        /// Sketch-domain linked, refreshed, and exact-change totals.
        sketches: conkit_sketch::SketchGenerationCounts,
    },
}

/// Formatter for successful command summaries.
pub(crate) struct Output;

impl Output {
    /// Warns before compiler extraction executes dependency build scripts and
    /// procedural macros selected by Cargo.
    ///
    /// # Errors
    ///
    /// Returns an error if the warning cannot be written to standard error.
    pub(crate) fn print_compiler_extraction_warning(&self) -> io::Result<()> {
        let stderr = io::stderr();
        let mut writer = stderr.lock();
        self.write_compiler_extraction_warning(&mut writer)
    }

    fn write_compiler_extraction_warning<W>(&self, writer: &mut W) -> io::Result<()>
    where
        W: Write + ?Sized,
    {
        writeln!(
            writer,
            "warning: compiler signature extraction invokes Cargo and runs selected build scripts and procedural macros unsandboxed with your user permissions"
        )
    }

    /// Prints bounded syntax-extraction capability warnings returned by the
    /// signature domain.
    ///
    /// # Errors
    ///
    /// Returns an error if a warning cannot be written to standard error.
    pub(crate) fn print_signature_capability_warnings(
        &self,
        warnings: &[String],
        cancellation: &ApplicationCancellation,
    ) -> Result<(), CliError> {
        let stderr = io::stderr();
        let mut writer = stderr.lock();
        self.write_signature_capability_warnings(&mut writer, warnings, cancellation)
    }

    fn write_signature_capability_warnings<W>(
        &self,
        writer: &mut W,
        warnings: &[String],
        cancellation: &ApplicationCancellation,
    ) -> Result<(), CliError>
    where
        W: Write + ?Sized,
    {
        for warning in warnings {
            cancellation.checkpoint()?;
            writeln!(writer, "{warning}")?;
        }
        Ok(())
    }

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
            "sketch check passed: matched {}, failed {}, total {}; referenced sources present {}/{}; catalog entries {}; contract documents {}",
            response.counts.matched_sketch_count,
            response.counts.failed_sketch_count,
            response.counts.sketch_count,
            response.counts.present_referenced_source_file_count,
            response.counts.referenced_source_file_count,
            response.counts.source_catalog_entry_count,
            response.counts.contract_document_count,
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
            GenerateSummary::Signatures { counts } => writeln!(
                writer,
                "signature generation completed: documents {}, signatures {}, preserved sketches {}, semantically changed documents {}, byte-changed documents {}",
                counts.document_count,
                counts.signature_count,
                counts.preserved_sketch_count,
                counts.semantically_changed_document_count,
                counts.byte_changed_document_count,
            ),
            GenerateSummary::Sketches { counts } => writeln!(
                writer,
                "sketch generation completed: linked {}, refreshed {}, changed {}, changed documents {}",
                counts.linked_sketch_count,
                counts.refreshed_sketch_count,
                counts.changed_sketch_count,
                counts.changed_document_count,
            ),
            GenerateSummary::All {
                signatures,
                sketches,
            } => writeln!(
                writer,
                "contract generation completed: documents {}, signatures {}, preserved sketches {}, semantically changed documents {}, byte-changed documents {}; linked sketches {}, refreshed {}, changed {}, changed documents {}",
                signatures.document_count,
                signatures.signature_count,
                signatures.preserved_sketch_count,
                signatures.semantically_changed_document_count,
                signatures.byte_changed_document_count,
                sketches.linked_sketch_count,
                sketches.refreshed_sketch_count,
                sketches.changed_sketch_count,
                sketches.changed_document_count,
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
        cancellation: &ApplicationCancellation,
    ) -> Result<(), CliError> {
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        self.write_diff(&mut writer, signatures, sketches, cancellation)
    }

    fn write_diff<W>(
        &self,
        writer: &mut W,
        signatures: &conkit_signature::DiffResponse,
        sketches: &conkit_sketch::DiffResponse,
        cancellation: &ApplicationCancellation,
    ) -> Result<(), CliError>
    where
        W: Write + ?Sized,
    {
        cancellation.checkpoint()?;
        if signatures.changed() || sketches.changed() {
            writeln!(writer, "contracts changed")?;
        } else {
            writeln!(writer, "contracts unchanged")?;
        }

        writeln!(
            writer,
            "signature contract digest v{} {}",
            signatures.digest_version, signatures.contract_digest,
        )?;
        writeln!(
            writer,
            "sketch contract digest v{} {}",
            sketches.digest_version, sketches.contract_digest,
        )?;

        for entry in &signatures.entries {
            cancellation.checkpoint()?;
            match entry {
                conkit_signature::DiffEntry::Added {
                    signature_id,
                    categories,
                } => {
                    write!(writer, "signature added {signature_id} [")?;
                    self.write_signature_categories(writer, categories)?;
                    writeln!(writer, "]")?;
                }
                conkit_signature::DiffEntry::Removed {
                    signature_id,
                    categories,
                } => {
                    write!(writer, "signature removed {signature_id} [")?;
                    self.write_signature_categories(writer, categories)?;
                    writeln!(writer, "]")?;
                }
                conkit_signature::DiffEntry::Changed {
                    signature_id,
                    current_digest,
                    previous_digest,
                    categories,
                } => {
                    write!(
                        writer,
                        "signature changed {signature_id} {previous_digest} -> {current_digest} ["
                    )?;
                    self.write_signature_categories(writer, categories)?;
                    writeln!(writer, "]")?;
                }
            }
        }

        for entry in &sketches.entries {
            cancellation.checkpoint()?;
            match entry {
                conkit_sketch::DiffEntry::Added { current } => {
                    self.write_sketch_snapshot(writer, "added", current)?;
                }
                conkit_sketch::DiffEntry::Removed { previous } => {
                    self.write_sketch_snapshot(writer, "removed", previous)?;
                }
                conkit_sketch::DiffEntry::Changed {
                    previous,
                    current,
                    fields,
                } => {
                    write!(writer, "sketch changed {} [", current.sketch_id)?;
                    self.write_sketch_fields(writer, fields)?;
                    writeln!(writer, "]")?;
                    self.write_sketch_snapshot(writer, "previous", previous)?;
                    self.write_sketch_snapshot(writer, "current", current)?;
                }
            }
        }

        Ok(())
    }

    fn write_signature_categories<W>(
        &self,
        writer: &mut W,
        categories: &std::collections::BTreeSet<conkit_signature::DiffCategory>,
    ) -> io::Result<()>
    where
        W: Write + ?Sized,
    {
        for (index, category) in categories.iter().enumerate() {
            if index > 0 {
                write!(writer, ", ")?;
            }
            let label = match category {
                conkit_signature::DiffCategory::SourceSemantics => "source_semantics",
                conkit_signature::DiffCategory::ExtractionContext => "extraction_context",
                conkit_signature::DiffCategory::Labels => "labels",
                conkit_signature::DiffCategory::DocumentMetadata => "document_metadata",
            };
            write!(writer, "{label}")?;
        }

        Ok(())
    }

    fn write_sketch_fields<W>(
        &self,
        writer: &mut W,
        fields: &[conkit_sketch::SketchField],
    ) -> io::Result<()>
    where
        W: Write + ?Sized,
    {
        for (index, field) in fields.iter().enumerate() {
            if index > 0 {
                write!(writer, ", ")?;
            }
            let label = match field {
                conkit_sketch::SketchField::SourceFile => "source_file",
                conkit_sketch::SketchField::LinkedSignature => "linked_signature",
                conkit_sketch::SketchField::SignatureType => "signature_type",
                conkit_sketch::SketchField::Occurrence => "occurrence",
                conkit_sketch::SketchField::Code => "code",
            };
            write!(writer, "{label}")?;
        }

        Ok(())
    }

    fn write_sketch_snapshot<W>(
        &self,
        writer: &mut W,
        state: &str,
        snapshot: &conkit_sketch::SketchSnapshot,
    ) -> io::Result<()>
    where
        W: Write + ?Sized,
    {
        let normalization = match snapshot.matching.normalization() {
            conkit_sketch::SketchNormalization::ExactLinesV1 => "exact_lines_v1",
        };
        let occurrence = match snapshot.matching.occurrence() {
            conkit_sketch::SketchOccurrence::AtLeastOne => "at_least_one",
            conkit_sketch::SketchOccurrence::ExactlyOne => "exactly_one",
        };

        writeln!(
            writer,
            "sketch {state} {} at {} document {}; source {}; signature {} ({}); matching {normalization}/{occurrence}; code {}",
            snapshot.sketch_id,
            snapshot.contract_file.as_str(),
            snapshot.document_index,
            snapshot.source_file.as_str(),
            snapshot.linked_signature,
            snapshot.signature_type,
            snapshot.code_digest,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
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
            source_shape_digest: "signature-source-shape".to_owned(),
            digest_version: 2,
            diagnostics: Vec::new(),
            report_files: conkit_signature::FileCatalog::new(),
        };
        let sketches = conkit_sketch::CheckResponse {
            passed: true,
            counts: conkit_sketch::SketchCheckCounts {
                source_catalog_entry_count: 5,
                referenced_source_file_count: 2,
                present_referenced_source_file_count: 1,
                contract_document_count: 3,
                sketch_count: 4,
                matched_sketch_count: 4,
                failed_sketch_count: 0,
            },
            diagnostics: Vec::new(),
            report_files: conkit_sketch::FileCatalog::new(),
        };
        let mut bytes = Vec::new();

        output
            .write_compiler_extraction_warning(&mut bytes)
            .expect("compiler extraction warning");
        output
            .write_signature_capability_warnings(
                &mut bytes,
                &["rust_syntax_v2 capability warning: conditional API".to_owned()],
                &crate::context::ApplicationCancellation::new(),
            )
            .expect("syntax capability warning");

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
            .write_generate_summary(
                &mut bytes,
                GenerateSummary::Signatures {
                    counts: conkit_signature::SignatureGenerationCounts {
                        document_count: 2,
                        signature_count: 4,
                        preserved_sketch_count: 3,
                        semantically_changed_document_count: 1,
                        byte_changed_document_count: 1,
                    },
                },
            )
            .expect("signature generation summary");
        output
            .write_generate_summary(
                &mut bytes,
                GenerateSummary::Sketches {
                    counts: conkit_sketch::SketchGenerationCounts {
                        linked_sketch_count: 3,
                        refreshed_sketch_count: 3,
                        changed_sketch_count: 2,
                        changed_document_count: 1,
                    },
                },
            )
            .expect("sketch generation summary");
        output
            .write_generate_summary(
                &mut bytes,
                GenerateSummary::All {
                    signatures: conkit_signature::SignatureGenerationCounts {
                        document_count: 2,
                        signature_count: 4,
                        preserved_sketch_count: 3,
                        semantically_changed_document_count: 1,
                        byte_changed_document_count: 1,
                    },
                    sketches: conkit_sketch::SketchGenerationCounts {
                        linked_sketch_count: 3,
                        refreshed_sketch_count: 3,
                        changed_sketch_count: 2,
                        changed_document_count: 1,
                    },
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
            "warning: compiler signature extraction invokes Cargo and runs selected build scripts and procedural macros unsandboxed with your user permissions\n\
             rust_syntax_v2 capability warning: conditional API\n\
             signature check passed: 3 source signatures, 2 contract signatures\n\
             sketch check passed: matched 4, failed 0, total 4; referenced sources present 1/2; catalog entries 5; contract documents 3\n\
             contract check passed: 2 signatures, 4 sketches\n\
             signature generation completed: documents 2, signatures 4, preserved sketches 3, semantically changed documents 1, byte-changed documents 1\n\
             sketch generation completed: linked 3, refreshed 3, changed 2, changed documents 1\n\
             contract generation completed: documents 2, signatures 4, preserved sketches 3, semantically changed documents 1, byte-changed documents 1; linked sketches 3, refreshed 3, changed 2, changed documents 1\n\
             adopted 1 matching existing contract files\n\
             archived contracts to archives/contracts.gzip\n"
        );
    }

    #[test]
    fn writes_diff_in_signature_then_sketch_order() {
        let signatures = conkit_signature::DiffResponse {
            contract_digest: "signature-contract-digest".to_owned(),
            digest_version: 2,
            entries: vec![
                conkit_signature::DiffEntry::Added {
                    signature_id: "added_signature".to_owned(),
                    categories: BTreeSet::from([conkit_signature::DiffCategory::SourceSemantics]),
                },
                conkit_signature::DiffEntry::Removed {
                    signature_id: "removed_signature".to_owned(),
                    categories: BTreeSet::from([conkit_signature::DiffCategory::ExtractionContext]),
                },
                conkit_signature::DiffEntry::Changed {
                    signature_id: "changed_signature".to_owned(),
                    current_digest: "current".to_owned(),
                    previous_digest: "previous".to_owned(),
                    categories: BTreeSet::from([
                        conkit_signature::DiffCategory::Labels,
                        conkit_signature::DiffCategory::DocumentMetadata,
                    ]),
                },
            ],
        };
        let sketches = conkit_sketch::DiffResponse {
            contract_digest: "sketch-contract-digest".to_owned(),
            digest_version: 2,
            entries: vec![
                conkit_sketch::DiffEntry::Added {
                    current: conkit_sketch::SketchSnapshot {
                        sketch_id: "added_sketch".to_owned(),
                        contract_file: conkit_sketch::CatalogPath::new("current.yml")
                            .expect("current path"),
                        document_index: 2,
                        source_file: conkit_sketch::CatalogPath::new("src/current.rs")
                            .expect("current source"),
                        linked_signature: "added_signature".to_owned(),
                        signature_type: "function".to_owned(),
                        matching: conkit_sketch::SketchMatchPolicy::new(
                            conkit_sketch::SketchNormalization::ExactLinesV1,
                            conkit_sketch::SketchOccurrence::ExactlyOne,
                        ),
                        code_digest: "added-code".to_owned(),
                    },
                },
                conkit_sketch::DiffEntry::Removed {
                    previous: conkit_sketch::SketchSnapshot {
                        sketch_id: "removed_sketch".to_owned(),
                        contract_file: conkit_sketch::CatalogPath::new("previous.yml")
                            .expect("previous path"),
                        document_index: 0,
                        source_file: conkit_sketch::CatalogPath::new("src/previous.rs")
                            .expect("previous source"),
                        linked_signature: "removed_signature".to_owned(),
                        signature_type: "struct".to_owned(),
                        matching: conkit_sketch::SketchMatchPolicy::new(
                            conkit_sketch::SketchNormalization::ExactLinesV1,
                            conkit_sketch::SketchOccurrence::AtLeastOne,
                        ),
                        code_digest: "removed-code".to_owned(),
                    },
                },
                conkit_sketch::DiffEntry::Changed {
                    previous: conkit_sketch::SketchSnapshot {
                        sketch_id: "changed_sketch".to_owned(),
                        contract_file: conkit_sketch::CatalogPath::new("old.yml")
                            .expect("old path"),
                        document_index: 1,
                        source_file: conkit_sketch::CatalogPath::new("src/old.rs")
                            .expect("old source"),
                        linked_signature: "changed_signature".to_owned(),
                        signature_type: "function".to_owned(),
                        matching: conkit_sketch::SketchMatchPolicy::new(
                            conkit_sketch::SketchNormalization::ExactLinesV1,
                            conkit_sketch::SketchOccurrence::AtLeastOne,
                        ),
                        code_digest: "old-code".to_owned(),
                    },
                    current: conkit_sketch::SketchSnapshot {
                        sketch_id: "changed_sketch".to_owned(),
                        contract_file: conkit_sketch::CatalogPath::new("new.yml")
                            .expect("new path"),
                        document_index: 3,
                        source_file: conkit_sketch::CatalogPath::new("src/new.rs")
                            .expect("new source"),
                        linked_signature: "changed_signature".to_owned(),
                        signature_type: "function".to_owned(),
                        matching: conkit_sketch::SketchMatchPolicy::new(
                            conkit_sketch::SketchNormalization::ExactLinesV1,
                            conkit_sketch::SketchOccurrence::ExactlyOne,
                        ),
                        code_digest: "new-code".to_owned(),
                    },
                    fields: vec![
                        conkit_sketch::SketchField::SourceFile,
                        conkit_sketch::SketchField::Occurrence,
                        conkit_sketch::SketchField::Code,
                    ],
                },
            ],
        };
        let mut bytes = Vec::new();

        Output
            .write_diff(
                &mut bytes,
                &signatures,
                &sketches,
                &crate::context::ApplicationCancellation::new(),
            )
            .expect("diff output");

        assert_eq!(
            String::from_utf8(bytes).expect("UTF-8 output"),
            "contracts changed\n\
             signature contract digest v2 signature-contract-digest\n\
             sketch contract digest v2 sketch-contract-digest\n\
             signature added added_signature [source_semantics]\n\
             signature removed removed_signature [extraction_context]\n\
             signature changed changed_signature previous -> current [labels, document_metadata]\n\
             sketch added added_sketch at current.yml document 2; source src/current.rs; signature added_signature (function); matching exact_lines_v1/exactly_one; code added-code\n\
             sketch removed removed_sketch at previous.yml document 0; source src/previous.rs; signature removed_signature (struct); matching exact_lines_v1/at_least_one; code removed-code\n\
             sketch changed changed_sketch [source_file, occurrence, code]\n\
             sketch previous changed_sketch at old.yml document 1; source src/old.rs; signature changed_signature (function); matching exact_lines_v1/at_least_one; code old-code\n\
             sketch current changed_sketch at new.yml document 3; source src/new.rs; signature changed_signature (function); matching exact_lines_v1/exactly_one; code new-code\n"
        );
    }

    #[test]
    fn writes_unchanged_diff_status() {
        let signatures = conkit_signature::DiffResponse {
            contract_digest: "signature-contract-digest".to_owned(),
            digest_version: 2,
            entries: Vec::new(),
        };
        let sketches = conkit_sketch::DiffResponse {
            contract_digest: "sketch-contract-digest".to_owned(),
            digest_version: 2,
            entries: Vec::new(),
        };
        let mut bytes = Vec::new();

        Output
            .write_diff(
                &mut bytes,
                &signatures,
                &sketches,
                &crate::context::ApplicationCancellation::new(),
            )
            .expect("diff output");

        assert_eq!(
            bytes,
            b"contracts unchanged\n\
              signature contract digest v2 signature-contract-digest\n\
              sketch contract digest v2 sketch-contract-digest\n"
        );
    }

    #[test]
    fn cancellation_remains_typed_at_terminal_output_boundaries() {
        let cancellation = crate::context::ApplicationCancellation::new();
        cancellation.request();
        let signatures = conkit_signature::DiffResponse {
            contract_digest: "signature-contract-digest".to_owned(),
            digest_version: 2,
            entries: Vec::new(),
        };
        let sketches = conkit_sketch::DiffResponse {
            contract_digest: "sketch-contract-digest".to_owned(),
            digest_version: 2,
            entries: Vec::new(),
        };
        let mut bytes = Vec::new();

        let warning_error = Output
            .write_signature_capability_warnings(&mut bytes, &["warning".to_owned()], &cancellation)
            .expect_err("warning output must retain typed cancellation");
        let diff_error = Output
            .write_diff(&mut bytes, &signatures, &sketches, &cancellation)
            .expect_err("diff output must retain typed cancellation");

        assert!(matches!(
            warning_error,
            crate::error::CliError::OperationCanceled
        ));
        assert!(matches!(
            diff_error,
            crate::error::CliError::OperationCanceled
        ));
        assert!(bytes.is_empty());
    }

    #[test]
    fn propagates_broken_pipe_errors() {
        let error = Output
            .write_adoption_summary(&mut BrokenWriter, 1)
            .expect_err("broken writer should fail");

        assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    }
}
