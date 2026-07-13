//! Versioned mixed-catalog archive encoding and decoding.
//!
//! Archives contain the complete mixed signature-and-sketch catalog, so their
//! versioned gzip wire format belongs to the CLI persistence boundary. Domain
//! crates receive decoded catalogs and remain responsible for their own diff
//! semantics.

mod publication;
mod source;

use std::io::{Read, Write};

use conkit_signature::{CatalogPath, FileCatalog};
use flate2::bufread::GzDecoder;
use flate2::{Compression, GzBuilder};
use serde::{Deserialize, Serialize};

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
    pub(crate) fn encode(self, contract_files: FileCatalog) -> Result<Vec<u8>, CliError> {
        match self {
            Self::Gzip => ArchivePayload::from_contract_files(contract_files).encode_gzip(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ArchivePayload {
    version: u32,
    contract_files: Vec<ArchiveFileEntry>,
}

impl ArchivePayload {
    fn from_contract_files(contract_files: FileCatalog) -> Self {
        Self {
            version: ARCHIVE_PAYLOAD_VERSION,
            contract_files: contract_files
                .into_entries()
                .map(ArchiveFileEntry::from_catalog_entry)
                .collect(),
        }
    }

    fn encode_gzip(&self) -> Result<Vec<u8>, CliError> {
        self.validate_limits("new archive")?;
        let payload = serde_json::to_vec(self).map_err(|source| CliError::ArchiveProcess {
            message: source.to_string(),
        })?;
        if payload.len() as u64 > MAX_EXPANDED_ARCHIVE_BYTES {
            return Err(CliError::ArchiveProcess {
                message: format!(
                    "new archive: expanded archive exceeds {MAX_EXPANDED_ARCHIVE_BYTES} bytes"
                ),
            });
        }
        let mut encoder = GzBuilder::new()
            .mtime(0)
            .write(Vec::new(), Compression::default());

        encoder
            .write_all(&payload)
            .map_err(|source| CliError::ArchiveProcess {
                message: source.to_string(),
            })?;
        let encoded = encoder
            .finish()
            .map_err(|source| CliError::ArchiveProcess {
                message: source.to_string(),
            })?;
        if encoded.len() > MAX_COMPRESSED_ARCHIVE_BYTES {
            return Err(CliError::ArchiveProcess {
                message: format!(
                    "new archive: compressed archive exceeds {MAX_COMPRESSED_ARCHIVE_BYTES} bytes"
                ),
            });
        }
        Ok(encoded)
    }

    fn decode_gzip(file_name: &str, bytes: &[u8]) -> Result<Self, CliError> {
        Self::validate_compressed_size(file_name, bytes.len())?;
        let mut decoder = GzDecoder::new(bytes);
        let mut payload = Vec::new();
        (&mut decoder)
            .take(MAX_EXPANDED_ARCHIVE_BYTES + 1)
            .read_to_end(&mut payload)
            .map_err(|source| CliError::ArchiveProcess {
                message: format!("{file_name}: failed to decode gzip payload: {source}"),
            })?;
        if payload.len() as u64 > MAX_EXPANDED_ARCHIVE_BYTES {
            return Err(CliError::ArchiveProcess {
                message: format!(
                    "{file_name}: expanded archive exceeds {MAX_EXPANDED_ARCHIVE_BYTES} bytes"
                ),
            });
        }
        if !decoder.into_inner().is_empty() {
            return Err(CliError::ArchiveProcess {
                message: format!("{file_name}: archive contains trailing data"),
            });
        }

        let archive: Self =
            serde_json::from_slice(&payload).map_err(|source| CliError::ArchiveProcess {
                message: format!("{file_name}: failed to parse archive JSON: {source}"),
            })?;
        archive.validate_version(file_name)?;
        archive.validate_limits(file_name)?;

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

    fn validate_limits(&self, file_name: &str) -> Result<(), CliError> {
        if self.contract_files.len() > MAX_ARCHIVE_ENTRIES {
            return Err(CliError::ArchiveProcess {
                message: format!(
                    "{file_name}: too many contract entries; maximum is {MAX_ARCHIVE_ENTRIES}"
                ),
            });
        }

        for entry in &self.contract_files {
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

    fn into_contract_files(self, file_name: &str) -> Result<FileCatalog, CliError> {
        let mut contract_files = FileCatalog::new();

        for entry in self.contract_files {
            let (path, bytes) = entry.into_catalog_entry(file_name)?;
            contract_files
                .insert(path, bytes)
                .map_err(|source| CliError::ArchiveProcess {
                    message: format!("{file_name}: {source}"),
                })?;
        }

        Ok(contract_files)
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

    fn into_catalog_entry(self, file_name: &str) -> Result<(CatalogPath, Vec<u8>), CliError> {
        Ok((self.path.into_catalog_path(file_name)?, self.bytes))
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

    fn into_catalog_path(self, file_name: &str) -> Result<CatalogPath, CliError> {
        CatalogPath::new(self.value).map_err(|source| CliError::ArchiveProcess {
            message: format!("{file_name}: {source}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use conkit_signature::{CatalogPath, FileCatalog};

    use super::{ArchiveFileEntry, ArchiveFormatSelection, ArchivePathDocument, ArchivePayload};

    struct VersionOneFixture;

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

        let error = ArchivePayload::decode_gzip("truncated-v1.gzip", &bytes)
            .expect_err("a truncated valid gzip stream must fail");

        assert!(error.to_string().contains("truncated-v1.gzip"));
    }

    #[test]
    fn encoded_bytes_match_the_pre_move_version_one_codec() {
        let encoded = ArchiveFormatSelection::from_gzip_flag(false)
            .encode(VersionOneFixture::catalog())
            .expect("archive encode");

        assert_eq!(
            encoded,
            include_bytes!("tests/fixtures/archive-v1/mixed-v1.gzip")
        );
    }

    #[test]
    fn combined_catalog_round_trips_exact_bytes() {
        let expected = VersionOneFixture::catalog();
        let encoded = ArchiveFormatSelection::from_gzip_flag(true)
            .encode(expected.clone())
            .expect("archive encode");
        let decoded = ArchivePayload::decode_gzip("round-trip.gzip", &encoded)
            .expect("archive decode")
            .into_contract_files("round-trip.gzip")
            .expect("catalog decode");

        assert_eq!(decoded, expected);
    }

    #[test]
    fn repeated_encoding_is_deterministic() {
        let first = ArchiveFormatSelection::from_gzip_flag(false)
            .encode(VersionOneFixture::catalog())
            .expect("first encode");
        let second = ArchiveFormatSelection::from_gzip_flag(true)
            .encode(VersionOneFixture::catalog())
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

        let error = ArchiveFormatSelection::from_gzip_flag(false)
            .encode(catalog)
            .expect_err("final encoding must validate contract limits");

        assert!(error.to_string().contains("contract entry exceeds"));
    }

    #[test]
    fn invalid_gzip_and_json_return_archive_errors() {
        let gzip_error =
            ArchivePayload::decode_gzip("bad.gzip", b"not gzip").expect_err("invalid gzip");
        let mut encoder = flate2::GzBuilder::new()
            .mtime(0)
            .write(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, b"not json").expect("encode fixture");
        let invalid_json = encoder.finish().expect("finish fixture");
        let json_error =
            ArchivePayload::decode_gzip("bad-json.gzip", &invalid_json).expect_err("invalid JSON");

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
        let mut encoded = ArchiveFormatSelection::from_gzip_flag(false)
            .encode(VersionOneFixture::catalog())
            .expect("archive encode");
        let first_member = encoded.clone();
        encoded.extend_from_slice(b"TRAILING-JUNK");

        let error = ArchivePayload::decode_gzip("trailing.gzip", &encoded)
            .expect_err("trailing bytes must be rejected");
        let mut concatenated = first_member.clone();
        concatenated.extend_from_slice(&first_member);
        let member_error = ArchivePayload::decode_gzip("two-members.gzip", &concatenated)
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
        let expanded_error = ArchivePayload::decode_gzip("expanded.gzip", &encoded)
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
            .validate_limits("large.gzip")
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
            .validate_limits("many.gzip")
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
        let contract_error = ArchivePayload::decode_gzip("large-wire.gzip", &oversized_contract)
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
        let entry_error = ArchivePayload::decode_gzip("many-wire.gzip", &too_many_entries)
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
        .encode_gzip()
        .expect("unsupported fixture");
        let version_error =
            ArchivePayload::decode_gzip("old.gzip", &unsupported).expect_err("unsupported version");

        let invalid_path = ArchivePayload {
            version: 1,
            contract_files: vec![ArchiveFileEntry {
                path: ArchivePathDocument {
                    value: "../escape.yml".to_owned(),
                },
                bytes: Vec::new(),
            }],
        };
        let path_error = invalid_path
            .into_contract_files("invalid-path.gzip")
            .expect_err("invalid path");

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
        }
        .into_contract_files("duplicate.gzip")
        .expect_err("duplicate path");

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
}
