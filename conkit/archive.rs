//! Versioned mixed-catalog archive encoding and decoding.
//!
//! Archives contain the complete mixed signature-and-sketch catalog, so their
//! versioned deterministic gzip wire format belongs to the CLI persistence
//! boundary. Archive orchestration validates mandatory-v2 combined documents
//! before encoding and collision-safe publication. Decoding charges the
//! verified physical entry, compressed bytes, and decoded logical entries to
//! the caller's cumulative catalog ledger; diff orchestration validates v2
//! again. Domain crates receive the same decoded catalog and remain responsible
//! for their own diff semantics.

mod publication;
mod source;

use std::io::{BufReader, Read, Write};
use std::path::Path;

use conkit_signature::{CatalogPath, FileCatalog};
use flate2::bufread::GzDecoder;
use flate2::{Compression, GzBuilder};
use serde::{Deserialize, Serialize};

use crate::bounded_output::{BoundedOutput, BoundedOutputFailure};
use crate::catalog::CatalogReadBudget;
use crate::context::ApplicationCancellation;
use crate::error::CliError;

pub(crate) use publication::ArchiveDestination;
pub(crate) use source::ArchiveSource;

const ARCHIVE_PAYLOAD_VERSION: u32 = 1;
const MAX_COMPRESSED_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;
const MAX_EXPANDED_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 100_000;
const MAX_CONTRACT_BYTES: usize = 16 * 1024 * 1024;

/// CLI archive format selection.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ArchiveFormatSelection {
    /// Version-1 mixed-catalog payload compressed with gzip.
    Gzip,
}

impl ArchiveFormatSelection {
    /// Maps both the omitted and explicit `--gzip` forms to the only format.
    pub(crate) fn from_gzip_flag(_gzip: bool) -> Self {
        Self::Gzip
    }

    /// Encodes a complete mixed contract catalog deterministically.
    ///
    /// # Errors
    ///
    /// Returns an error if the catalog exceeds the entry, per-entry,
    /// expanded-payload, or compressed-payload limits, or if JSON serialization
    /// or gzip compression fails.
    pub(crate) fn encode(
        self,
        contract_files: FileCatalog,
        cancellation: &ApplicationCancellation,
    ) -> Result<Vec<u8>, CliError> {
        match self {
            Self::Gzip => ArchivePayload::from_contract_files(contract_files, cancellation)?
                .encode_gzip(cancellation),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ArchivePayload {
    version: u32,
    contract_files: Vec<ArchiveFileEntry>,
}

struct ArchiveJsonInput<'bytes, 'budget> {
    bytes: &'bytes [u8],
    offset: usize,
    budget: &'budget CatalogReadBudget,
}

impl ArchivePayload {
    fn from_contract_files(
        contract_files: FileCatalog,
        cancellation: &ApplicationCancellation,
    ) -> Result<Self, CliError> {
        let mut entries = Vec::with_capacity(contract_files.len());
        for entry in contract_files.into_entries() {
            cancellation.checkpoint()?;
            entries.push(ArchiveFileEntry::from_catalog_entry(entry));
        }
        Ok(Self {
            version: ARCHIVE_PAYLOAD_VERSION,
            contract_files: entries,
        })
    }

    fn encode_gzip(&self, cancellation: &ApplicationCancellation) -> Result<Vec<u8>, CliError> {
        cancellation.checkpoint()?;
        self.validate_limits("new archive", cancellation)?;
        let mut output = BoundedOutput::new(Vec::new(), cancellation, MAX_EXPANDED_ARCHIVE_BYTES);
        let rendering = serde_json::to_writer(&mut output, self);
        cancellation.checkpoint()?;
        if let Some(failure) = output.failure() {
            return Err(Self::expanded_output_failure(failure));
        }
        rendering.map_err(|source| CliError::ArchiveProcess {
            message: source.to_string(),
        })?;
        let payload = output.into_inner();
        let mut encoder = GzBuilder::new().mtime(0).write(
            BoundedOutput::new(
                Vec::new(),
                cancellation,
                MAX_COMPRESSED_ARCHIVE_BYTES as u64,
            ),
            Compression::default(),
        );

        for chunk in payload.chunks(64 * 1024) {
            cancellation.checkpoint()?;
            if let Err(source) = encoder.write_all(chunk) {
                return Err(Self::compressed_output_error(
                    encoder.get_ref().failure(),
                    source,
                    cancellation,
                ));
            }
        }
        cancellation.checkpoint()?;
        if let Err(source) = encoder.try_finish() {
            return Err(Self::compressed_output_error(
                encoder.get_ref().failure(),
                source,
                cancellation,
            ));
        }
        let encoded = encoder
            .finish()
            .map_err(|source| match cancellation.checkpoint() {
                Err(canceled) => canceled,
                Ok(()) => CliError::ArchiveProcess {
                    message: source.to_string(),
                },
            })?;
        cancellation.checkpoint()?;
        Ok(encoded.into_inner())
    }

    fn expanded_output_failure(failure: BoundedOutputFailure) -> CliError {
        match failure {
            BoundedOutputFailure::Cancelled => CliError::OperationCanceled,
            BoundedOutputFailure::Limit { observed_at_least } => CliError::ArchiveProcess {
                message: format!(
                    "new archive: expanded archive exceeds {MAX_EXPANDED_ARCHIVE_BYTES} bytes (observed at least {observed_at_least})"
                ),
            },
        }
    }

    fn compressed_output_error(
        failure: Option<BoundedOutputFailure>,
        source: std::io::Error,
        cancellation: &ApplicationCancellation,
    ) -> CliError {
        if let Err(canceled) = cancellation.checkpoint() {
            return canceled;
        }
        match failure {
            Some(BoundedOutputFailure::Cancelled) => CliError::OperationCanceled,
            Some(BoundedOutputFailure::Limit { .. }) => CliError::ArchiveProcess {
                message: format!("compressed archive exceeds {MAX_COMPRESSED_ARCHIVE_BYTES} bytes"),
            },
            None => CliError::ArchiveProcess {
                message: source.to_string(),
            },
        }
    }

    fn decode_gzip(
        file_name: &str,
        bytes: &[u8],
        budget: &CatalogReadBudget,
    ) -> Result<Self, CliError> {
        budget.checkpoint()?;
        Self::validate_compressed_size(file_name, bytes.len())?;
        let mut decoder = GzDecoder::new(bytes);
        let mut payload = Vec::new();
        let mut chunk = [0_u8; 64 * 1024];
        loop {
            budget.checkpoint()?;
            let read = decoder
                .read(&mut chunk)
                .map_err(|source| CliError::ArchiveProcess {
                    message: format!("{file_name}: failed to decode gzip payload: {source}"),
                })?;
            if read == 0 {
                break;
            }
            let observed = u64::try_from(payload.len())
                .unwrap_or(u64::MAX)
                .saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
            if observed > MAX_EXPANDED_ARCHIVE_BYTES {
                return Err(CliError::ArchiveProcess {
                    message: format!(
                        "{file_name}: expanded archive exceeds {MAX_EXPANDED_ARCHIVE_BYTES} bytes"
                    ),
                });
            }
            payload.extend_from_slice(&chunk[..read]);
        }
        if !decoder.into_inner().is_empty() {
            return Err(CliError::ArchiveProcess {
                message: format!("{file_name}: archive contains trailing data"),
            });
        }

        budget.checkpoint()?;
        let input = ArchiveJsonInput::new(&payload, budget);
        let mut input = BufReader::with_capacity(64 * 1024, input);
        let archive: Self = serde_json::from_reader(&mut input).map_err(|source| {
            if budget.checkpoint().is_err() {
                CliError::OperationCanceled
            } else {
                CliError::ArchiveProcess {
                    message: format!("{file_name}: failed to parse archive JSON: {source}"),
                }
            }
        })?;
        budget.checkpoint()?;
        archive.validate_version(file_name)?;
        archive.validate_limits(file_name, budget.cancellation())?;

        Ok(archive)
    }

    fn validate_compressed_size(file_name: &str, size: usize) -> Result<(), CliError> {
        if size <= MAX_COMPRESSED_ARCHIVE_BYTES {
            return Ok(());
        }

        Err(CliError::ArchiveProcess {
            message: format!(
                "{file_name}: compressed archive exceeds {MAX_COMPRESSED_ARCHIVE_BYTES} bytes"
            ),
        })
    }

    fn validate_limits(
        &self,
        file_name: &str,
        cancellation: &ApplicationCancellation,
    ) -> Result<(), CliError> {
        if self.contract_files.len() > MAX_ARCHIVE_ENTRIES {
            return Err(CliError::ArchiveProcess {
                message: format!(
                    "{file_name}: too many contract entries; maximum is {MAX_ARCHIVE_ENTRIES}"
                ),
            });
        }

        for entry in &self.contract_files {
            cancellation.checkpoint()?;
            if entry.bytes.len() > MAX_CONTRACT_BYTES {
                return Err(CliError::ArchiveProcess {
                    message: format!(
                        "{file_name}: contract entry exceeds {MAX_CONTRACT_BYTES} bytes: {}",
                        entry.path.value
                    ),
                });
            }
        }

        Ok(())
    }

    fn validate_version(&self, file_name: &str) -> Result<(), CliError> {
        if self.version == ARCHIVE_PAYLOAD_VERSION {
            return Ok(());
        }

        Err(CliError::ArchiveProcess {
            message: format!(
                "{file_name}: unsupported archive payload version {}",
                self.version
            ),
        })
    }

    fn into_contract_files(
        self,
        file_name: &str,
        budget: &mut CatalogReadBudget,
    ) -> Result<FileCatalog, CliError> {
        let mut contract_files = FileCatalog::new();

        for entry in self.contract_files {
            budget.checkpoint()?;
            let (path, bytes) = entry.into_catalog_entry(file_name, budget)?;
            budget.record_entry_bytes(Path::new(path.as_str()), bytes.len())?;
            contract_files
                .insert(path, bytes)
                .map_err(|source| CliError::ArchiveProcess {
                    message: format!("{file_name}: {source}"),
                })?;
        }

        Ok(contract_files)
    }
}

impl<'bytes, 'budget> ArchiveJsonInput<'bytes, 'budget> {
    fn new(bytes: &'bytes [u8], budget: &'budget CatalogReadBudget) -> Self {
        Self {
            bytes,
            offset: 0,
            budget,
        }
    }
}

impl Read for ArchiveJsonInput<'_, '_> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        self.budget.checkpoint().map_err(std::io::Error::other)?;
        let remaining = &self.bytes[self.offset..];
        let read = remaining.len().min(output.len()).min(64 * 1024);
        output[..read].copy_from_slice(&remaining[..read]);
        self.offset = self.offset.saturating_add(read);
        Ok(read)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ArchiveFileEntry {
    path: ArchivePathDocument,
    bytes: Vec<u8>,
}

impl ArchiveFileEntry {
    fn from_catalog_entry((path, bytes): (CatalogPath, Vec<u8>)) -> Self {
        Self {
            path: ArchivePathDocument::from_catalog_path(path),
            bytes,
        }
    }

    fn into_catalog_entry(
        self,
        file_name: &str,
        budget: &CatalogReadBudget,
    ) -> Result<(CatalogPath, Vec<u8>), CliError> {
        Ok((self.path.into_catalog_path(file_name, budget)?, self.bytes))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct ArchivePathDocument {
    value: String,
}

impl ArchivePathDocument {
    fn from_catalog_path(path: CatalogPath) -> Self {
        Self {
            value: path.as_str().to_owned(),
        }
    }

    fn into_catalog_path(
        self,
        file_name: &str,
        budget: &CatalogReadBudget,
    ) -> Result<CatalogPath, CliError> {
        for _ in self.value.as_bytes().chunks(4_096) {
            budget.checkpoint()?;
        }
        CatalogPath::new(self.value).map_err(|source| CliError::ArchiveProcess {
            message: format!("{file_name}: {source}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use conkit_signature::{CatalogPath, FileCatalog};

    use super::{ArchiveFileEntry, ArchiveFormatSelection, ArchivePathDocument, ArchivePayload};
    use crate::catalog::{CatalogReadBudget, CatalogReadLimits};
    use crate::context::ApplicationCancellation;
    use crate::error::CliError;

    struct VersionOneFixture;

    struct ArchiveFixture;

    impl ArchiveFixture {
        fn cancellation() -> ApplicationCancellation {
            ApplicationCancellation::new()
        }

        fn budget() -> CatalogReadBudget {
            CatalogReadLimits::default().begin(&Self::cancellation())
        }

        fn encode(
            format: ArchiveFormatSelection,
            catalog: FileCatalog,
        ) -> Result<Vec<u8>, CliError> {
            format.encode(catalog, &Self::cancellation())
        }

        fn decode(file_name: &str, bytes: &[u8]) -> Result<ArchivePayload, CliError> {
            ArchivePayload::decode_gzip(file_name, bytes, &Self::budget())
        }

        fn catalog(payload: ArchivePayload, file_name: &str) -> Result<FileCatalog, CliError> {
            payload.into_contract_files(file_name, &mut Self::budget())
        }
    }

    impl VersionOneFixture {
        fn catalog() -> FileCatalog {
            let mut catalog = FileCatalog::new();
            catalog
                .insert(
                    CatalogPath::new("main.yml").expect("combined document path"),
                    include_bytes!("tests/fixtures/archive-v1/contracts/main.yml").to_vec(),
                )
                .expect("combined document insert");
            catalog
        }
    }

    #[test]
    fn truncated_old_version_one_archive_is_rejected() {
        let mut bytes = include_bytes!("tests/fixtures/archive-v1/mixed-v1.gzip").to_vec();
        bytes.pop().expect("archive fixture has a gzip trailer");

        let error = ArchiveFixture::decode("truncated-v1.gzip", &bytes)
            .expect_err("a truncated valid gzip stream must fail");

        assert!(error.to_string().contains("truncated-v1.gzip"));
    }

    #[test]
    fn encoded_bytes_match_the_pre_move_version_one_codec() {
        let encoded = ArchiveFixture::encode(
            ArchiveFormatSelection::from_gzip_flag(false),
            VersionOneFixture::catalog(),
        )
        .expect("archive encode");

        assert_eq!(
            encoded,
            include_bytes!("tests/fixtures/archive-v1/mixed-v1.gzip")
        );
    }

    #[test]
    fn combined_catalog_round_trips_exact_bytes() {
        let expected = VersionOneFixture::catalog();
        let encoded = ArchiveFixture::encode(
            ArchiveFormatSelection::from_gzip_flag(true),
            expected.clone(),
        )
        .expect("archive encode");
        let decoded = ArchiveFixture::catalog(
            ArchiveFixture::decode("round-trip.gzip", &encoded).expect("archive decode"),
            "round-trip.gzip",
        )
        .expect("catalog decode");

        assert_eq!(decoded, expected);
    }

    #[test]
    fn repeated_encoding_is_deterministic() {
        let first = ArchiveFixture::encode(
            ArchiveFormatSelection::from_gzip_flag(false),
            VersionOneFixture::catalog(),
        )
        .expect("first encode");
        let second = ArchiveFixture::encode(
            ArchiveFormatSelection::from_gzip_flag(true),
            VersionOneFixture::catalog(),
        )
        .expect("second encode");

        assert_eq!(first, second);
    }

    #[test]
    fn final_encoding_boundary_rejects_oversized_contract_entries() {
        let mut catalog = FileCatalog::new();
        catalog
            .insert(
                CatalogPath::new("large.yml").expect("large contract path"),
                vec![0; super::MAX_CONTRACT_BYTES + 1],
            )
            .expect("large contract insert");

        let error = ArchiveFixture::encode(ArchiveFormatSelection::from_gzip_flag(false), catalog)
            .expect_err("final encoding must validate contract limits");

        assert!(error.to_string().contains("contract entry exceeds"));
    }

    #[test]
    fn invalid_gzip_and_json_return_archive_errors() {
        let gzip_error = ArchiveFixture::decode("bad.gzip", b"not gzip").expect_err("invalid gzip");
        let mut encoder = flate2::GzBuilder::new()
            .mtime(0)
            .write(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, b"not json").expect("encode fixture");
        let invalid_json = encoder.finish().expect("finish fixture");
        let json_error =
            ArchiveFixture::decode("bad-json.gzip", &invalid_json).expect_err("invalid JSON");

        assert!(
            gzip_error
                .to_string()
                .contains("bad.gzip: failed to decode gzip payload")
        );
        assert!(
            json_error
                .to_string()
                .contains("bad-json.gzip: failed to parse archive JSON")
        );
    }

    #[test]
    fn gzip_with_trailing_bytes_or_a_second_member_is_rejected() {
        let mut encoded = ArchiveFixture::encode(
            ArchiveFormatSelection::from_gzip_flag(false),
            VersionOneFixture::catalog(),
        )
        .expect("archive encode");
        let first_member = encoded.clone();
        encoded.extend_from_slice(b"TRAILING-JUNK");

        let error = ArchiveFixture::decode("trailing.gzip", &encoded)
            .expect_err("trailing bytes must be rejected");
        let mut concatenated = first_member.clone();
        concatenated.extend_from_slice(&first_member);
        let member_error = ArchiveFixture::decode("two-members.gzip", &concatenated)
            .expect_err("a second gzip member must be rejected");

        assert!(error.to_string().contains("trailing data"));
        assert!(member_error.to_string().contains("trailing data"));
    }

    #[test]
    fn compressed_and_expanded_archive_limits_are_rejected() {
        let compressed_error = ArchivePayload::validate_compressed_size(
            "compressed.gzip",
            super::MAX_COMPRESSED_ARCHIVE_BYTES + 1,
        )
        .expect_err("compressed limit must be enforced before reading");
        let mut encoder = flate2::GzBuilder::new()
            .mtime(0)
            .write(Vec::new(), flate2::Compression::best());
        let chunk = [b' '; 64 * 1024];
        let full_chunks = super::MAX_EXPANDED_ARCHIVE_BYTES / chunk.len() as u64;
        for _ in 0..full_chunks {
            std::io::Write::write_all(&mut encoder, &chunk).expect("encode expanded fixture");
        }
        std::io::Write::write_all(&mut encoder, b" ").expect("exceed expanded limit");
        let encoded = encoder.finish().expect("finish expanded fixture");
        let expanded_error = ArchiveFixture::decode("expanded.gzip", &encoded)
            .expect_err("the production expanded limit must stop decoding");

        assert!(
            compressed_error
                .to_string()
                .contains("compressed archive exceeds")
        );
        assert!(
            expanded_error
                .to_string()
                .contains("expanded archive exceeds")
        );
    }

    #[test]
    fn payload_entry_and_contract_limits_are_rejected() {
        let entry = ArchiveFileEntry {
            path: ArchivePathDocument {
                value: "large.yml".to_owned(),
            },
            bytes: vec![0; super::MAX_CONTRACT_BYTES + 1],
        };
        let oversized_contract = ArchivePayload {
            version: 1,
            contract_files: vec![entry],
        };
        let contract_error = oversized_contract
            .validate_limits("large.gzip", &ArchiveFixture::cancellation())
            .expect_err("oversized contract must fail");

        let too_many = ArchivePayload {
            version: 1,
            contract_files: std::iter::repeat_with(|| ArchiveFileEntry {
                path: ArchivePathDocument {
                    value: "same.yml".to_owned(),
                },
                bytes: Vec::new(),
            })
            .take(super::MAX_ARCHIVE_ENTRIES + 1)
            .collect(),
        };
        let entry_error = too_many
            .validate_limits("many.gzip", &ArchiveFixture::cancellation())
            .expect_err("too many entries must fail");

        assert!(
            contract_error
                .to_string()
                .contains("contract entry exceeds")
        );
        assert!(
            entry_error
                .to_string()
                .contains("too many contract entries")
        );
    }

    #[test]
    fn decoded_wire_payload_enforces_entry_and_contract_limits_before_catalog_conversion() {
        let encode = |payload: &ArchivePayload| {
            let json = serde_json::to_vec(payload).expect("serialize wire payload");
            let mut encoder = flate2::GzBuilder::new()
                .mtime(0)
                .write(Vec::new(), flate2::Compression::default());
            std::io::Write::write_all(&mut encoder, &json).expect("encode wire payload");
            encoder.finish().expect("finish wire payload")
        };

        let oversized_contract = encode(&ArchivePayload {
            version: 1,
            contract_files: vec![ArchiveFileEntry {
                path: ArchivePathDocument {
                    value: "large.yml".to_owned(),
                },
                bytes: vec![0; super::MAX_CONTRACT_BYTES + 1],
            }],
        });
        let contract_error = ArchiveFixture::decode("large-wire.gzip", &oversized_contract)
            .expect_err("decoded oversized contract must fail");

        let too_many_entries = encode(&ArchivePayload {
            version: 1,
            contract_files: std::iter::repeat_with(|| ArchiveFileEntry {
                path: ArchivePathDocument {
                    value: "same.yml".to_owned(),
                },
                bytes: Vec::new(),
            })
            .take(super::MAX_ARCHIVE_ENTRIES + 1)
            .collect(),
        });
        let entry_error = ArchiveFixture::decode("many-wire.gzip", &too_many_entries)
            .expect_err("decoded entry overflow must fail");

        assert!(
            contract_error
                .to_string()
                .contains("contract entry exceeds")
        );
        assert!(
            entry_error
                .to_string()
                .contains("too many contract entries")
        );
    }

    #[test]
    fn unsupported_version_invalid_path_and_duplicate_path_are_rejected() {
        let unsupported = ArchivePayload {
            version: 999,
            contract_files: Vec::new(),
        }
        .encode_gzip(&ArchiveFixture::cancellation())
        .expect("unsupported fixture");
        let version_error =
            ArchiveFixture::decode("old.gzip", &unsupported).expect_err("unsupported version");

        let invalid_path = ArchivePayload {
            version: 1,
            contract_files: vec![ArchiveFileEntry {
                path: ArchivePathDocument {
                    value: "../escape.yml".to_owned(),
                },
                bytes: Vec::new(),
            }],
        };
        let path_error =
            ArchiveFixture::catalog(invalid_path, "invalid-path.gzip").expect_err("invalid path");

        let duplicate_error = ArchivePayload {
            version: 1,
            contract_files: vec![
                ArchiveFileEntry {
                    path: ArchivePathDocument {
                        value: "same.yml".to_owned(),
                    },
                    bytes: Vec::new(),
                },
                ArchiveFileEntry {
                    path: ArchivePathDocument {
                        value: "same.yml".to_owned(),
                    },
                    bytes: Vec::new(),
                },
            ],
        };
        let duplicate_error =
            ArchiveFixture::catalog(duplicate_error, "duplicate.gzip").expect_err("duplicate path");

        assert!(
            version_error
                .to_string()
                .contains("unsupported archive payload version 999")
        );
        assert!(path_error.to_string().contains("invalid catalog path"));
        assert!(
            duplicate_error
                .to_string()
                .contains("duplicate catalog path")
        );
    }

    #[test]
    fn decoded_catalog_entries_share_the_callers_existing_budget() {
        let cancellation = ArchiveFixture::cancellation();
        let mut budget = CatalogReadLimits::new(1, 64, 64).begin(&cancellation);
        budget
            .record_entry_bytes(std::path::Path::new("current.yml"), 1)
            .expect("current catalog entry");
        let archived = ArchivePayload {
            version: 1,
            contract_files: vec![ArchiveFileEntry {
                path: ArchivePathDocument {
                    value: "previous.yml".to_owned(),
                },
                bytes: vec![b'x'],
            }],
        };

        let error = archived
            .into_contract_files("previous.gzip", &mut budget)
            .expect_err("current and archived entries must share one ledger");

        let CliError::CatalogReadLimit(error) = error else {
            panic!("expected aggregate catalog entry limit");
        };
        assert!(
            error
                .to_string()
                .contains("catalog entry count limit exceeded: limit 1, observed at least 2"),
            "unexpected aggregate catalog limit: {error}",
        );
    }

    #[test]
    fn canceled_archive_decode_stops_before_decompression() {
        let cancellation = ArchiveFixture::cancellation();
        cancellation.request();
        let budget = CatalogReadLimits::default().begin(&cancellation);

        let error = ArchivePayload::decode_gzip(
            "canceled.gzip",
            include_bytes!("tests/fixtures/archive-v1/mixed-v1.gzip"),
            &budget,
        )
        .expect_err("pre-canceled archive decoding must stop immediately");

        assert!(matches!(error, CliError::OperationCanceled));
    }
}
