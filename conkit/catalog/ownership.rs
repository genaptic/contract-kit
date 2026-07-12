//! Persisted version-3 generated-file ownership values.
//!
//! Ownership is limited to direct root-level `.yml` and `.yaml` combined
//! documents and records no contract-family provenance. This module validates
//! and serializes persisted state; filesystem orchestration lives in sibling
//! catalog owners.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use conkit_signature::CatalogPath;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::path::PortableCatalogPathKey;
use crate::contracts::ContractDocumentPath;
use crate::error::CliError;

/// Versioned ownership state stored in the reserved metadata namespace.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OwnershipManifest {
    version: u32,
    journal: OwnershipJournal,
}

/// Persisted committed or in-progress ownership state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum OwnershipJournal {
    /// One fully committed generation.
    Committed {
        generation: u64,
        files: OwnedCatalog,
    },
    /// A recoverable transition between complete ownership catalogs.
    Updating {
        generation: u64,
        before: OwnedCatalog,
        after: OwnedCatalog,
    },
}

/// Sorted, portable set of owned combined documents.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OwnedCatalog {
    documents: Vec<OwnedFile>,
}

/// Logical path and digest for one owned combined document.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OwnedFile {
    path: String,
    sha256: ContentDigest,
}

/// Lowercase hexadecimal SHA-256 digest stored in ownership metadata.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(super) struct ContentDigest(String);

/// Minimal version probe used before parsing a version-specific manifest.
#[derive(Debug, Deserialize)]
pub(super) struct VersionProbe {
    version: u32,
}

/// Serialized marker reserving one not-yet-committed generated output.
#[derive(Serialize)]
pub(super) struct ReservationMarker<'file> {
    version: u32,
    generation: u64,
    path: &'file str,
    sha256: &'file ContentDigest,
}

impl OwnershipManifest {
    /// Current ownership wire-format version.
    pub(super) const VERSION: u32 = 3;
    /// Reserved contracts-root metadata directory.
    pub(super) const DIRECTORY: &'static str = ".contract-kit";
    /// Persisted ownership manifest file name.
    pub(super) const FILE_NAME: &'static str = "generated-files.json";
    /// Prefix used by individually atomic manifest writes.
    pub(super) const TEMPORARY_PREFIX: &'static str = ".generated-files.json.";
    /// Random suffix width used by the atomic-write implementation.
    pub(super) const ATOMIC_TEMPORARY_SUFFIX_LENGTH: usize = 6;

    /// Builds a committed ownership state.
    pub(super) fn committed(generation: u64, files: OwnedCatalog) -> Self {
        Self {
            version: Self::VERSION,
            journal: OwnershipJournal::Committed { generation, files },
        }
    }

    /// Builds an updating ownership state from complete before/after catalogs.
    pub(super) fn updating(generation: u64, before: OwnedCatalog, after: OwnedCatalog) -> Self {
        Self {
            version: Self::VERSION,
            journal: OwnershipJournal::Updating {
                generation,
                before,
                after,
            },
        }
    }

    /// Returns whether this manifest records an interrupted update.
    pub(super) fn is_updating(&self) -> bool {
        matches!(&self.journal, OwnershipJournal::Updating { .. })
    }

    /// Consumes the manifest and returns its persisted journal state.
    pub(super) fn into_journal(self) -> OwnershipJournal {
        self.journal
    }

    /// Returns the versioned ownership manifest path below a contracts root.
    pub(super) fn path(root: &Path) -> PathBuf {
        root.join(Self::DIRECTORY).join(Self::FILE_NAME)
    }

    /// Parses and validates a version-3 ownership manifest.
    pub(super) fn from_bytes(path: &Path, bytes: &[u8]) -> Result<Self, CliError> {
        let manifest = serde_json::from_slice::<Self>(bytes).map_err(|source| {
            CliError::InvalidGeneratedOwnership {
                path: path.to_path_buf(),
                message: source.to_string(),
            }
        })?;
        manifest.validate(path)?;
        Ok(manifest)
    }

    /// Validates and serializes this manifest as pretty JSON plus one newline.
    pub(super) fn to_bytes(&self, path: &Path) -> Result<Vec<u8>, CliError> {
        self.validate(path)?;
        let mut bytes = serde_json::to_vec_pretty(self).map_err(|source| {
            CliError::InvalidGeneratedOwnership {
                path: path.to_path_buf(),
                message: source.to_string(),
            }
        })?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    fn validate(&self, path: &Path) -> Result<(), CliError> {
        if self.version != Self::VERSION {
            return Err(CliError::InvalidGeneratedOwnership {
                path: path.to_path_buf(),
                message: format!("unsupported ownership version {}", self.version),
            });
        }

        match &self.journal {
            OwnershipJournal::Committed { generation, files } => {
                Self::validate_generation(*generation, path)?;
                files.validate(path)
            }
            OwnershipJournal::Updating {
                generation,
                before,
                after,
            } => {
                Self::validate_generation(*generation, path)?;
                before.validate(path)?;
                after.validate(path)?;
                before.validate_transition(after, path)
            }
        }
    }

    fn validate_generation(generation: u64, path: &Path) -> Result<(), CliError> {
        if generation == 0 {
            Err(CliError::InvalidGeneratedOwnership {
                path: path.to_path_buf(),
                message: "ownership generation must be greater than zero".to_owned(),
            })
        } else {
            Ok(())
        }
    }
}

impl OwnedCatalog {
    /// Builds a deterministically sorted catalog from owned values.
    pub(super) fn from_files(mut documents: Vec<OwnedFile>) -> Self {
        documents.sort_by(|left, right| left.path.cmp(&right.path));
        Self { documents }
    }

    /// Iterates over owned documents in persisted order.
    pub(super) fn entries(&self) -> impl Iterator<Item = &OwnedFile> {
        self.documents.iter()
    }

    /// Finds one owned document by its exact logical spelling.
    pub(super) fn find(&self, path: &str) -> Option<&OwnedFile> {
        self.entries().find(|file| file.path == path)
    }

    /// Validates paths, digests, portable identity, and deterministic sorting.
    pub(super) fn validate(&self, manifest_path: &Path) -> Result<(), CliError> {
        let mut keys = BTreeMap::<PortableCatalogPathKey, String>::new();
        for file in self.entries() {
            let logical = CatalogPath::new(file.path.clone()).map_err(|source| {
                CliError::InvalidGeneratedOwnership {
                    path: manifest_path.to_path_buf(),
                    message: source.to_string(),
                }
            })?;
            Self::validate_logical_path(&logical, manifest_path)?;
            file.sha256.validate(manifest_path)?;
            let key = PortableCatalogPathKey::new(&logical);
            if let Some(previous) = keys.insert(key, file.path.clone()) {
                return Err(CliError::PortableGeneratedPathCollision {
                    first: previous,
                    second: file.path.clone(),
                });
            }
        }
        if !self
            .documents
            .windows(2)
            .all(|pair| pair[0].path < pair[1].path)
        {
            return Err(CliError::InvalidGeneratedOwnership {
                path: manifest_path.to_path_buf(),
                message: "owned document paths must be unique and sorted".to_owned(),
            });
        }
        Ok(())
    }

    /// Rejects case-only path changes across an ownership transition.
    pub(super) fn validate_transition(
        &self,
        after: &Self,
        manifest_path: &Path,
    ) -> Result<(), CliError> {
        for before_file in self.entries() {
            let logical = CatalogPath::new(before_file.path.clone()).map_err(|source| {
                CliError::InvalidGeneratedOwnership {
                    path: manifest_path.to_path_buf(),
                    message: source.to_string(),
                }
            })?;
            let key = PortableCatalogPathKey::new(&logical);
            if let Some(after_file) = after.find_by_key(&key)
                && before_file.path != after_file.path
            {
                return Err(CliError::CaseOnlyGeneratedPathChange {
                    previous: before_file.path.clone(),
                    current: after_file.path.clone(),
                });
            }
        }
        Ok(())
    }

    fn find_by_key(&self, key: &PortableCatalogPathKey) -> Option<&OwnedFile> {
        self.entries().find_map(|file| {
            let logical = CatalogPath::new(file.path.clone()).ok()?;
            (PortableCatalogPathKey::new(&logical) == *key).then_some(file)
        })
    }

    fn validate_logical_path(logical: &CatalogPath, manifest_path: &Path) -> Result<(), CliError> {
        if logical
            .as_str()
            .split('/')
            .next()
            .is_some_and(|component| component.eq_ignore_ascii_case(OwnershipManifest::DIRECTORY))
        {
            return Err(CliError::InvalidGeneratedOwnership {
                path: manifest_path.to_path_buf(),
                message: format!("reserved ownership path {logical}"),
            });
        }

        if ContractDocumentPath::try_from(logical.clone()).is_err() {
            return Err(CliError::InvalidGeneratedOwnership {
                path: manifest_path.to_path_buf(),
                message: format!(
                    "owned document path {logical} must be a direct root .yml or .yaml combined document"
                ),
            });
        }

        Ok(())
    }
}

impl OwnedFile {
    /// Builds one persisted owned-file value.
    pub(super) fn new(path: String, sha256: ContentDigest) -> Self {
        Self { path, sha256 }
    }

    /// Returns the persisted logical path.
    pub(super) fn path(&self) -> &str {
        &self.path
    }

    /// Returns the persisted content digest.
    pub(super) fn digest(&self) -> &ContentDigest {
        &self.sha256
    }
}

impl ContentDigest {
    /// Computes a lowercase hexadecimal SHA-256 digest.
    pub(super) fn of(bytes: &[u8]) -> Self {
        Self(format!("{:x}", Sha256::digest(bytes)))
    }

    fn validate(&self, manifest_path: &Path) -> Result<(), CliError> {
        let valid = self.0.len() == 64
            && self
                .0
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'));
        if valid {
            Ok(())
        } else {
            Err(CliError::InvalidGeneratedOwnership {
                path: manifest_path.to_path_buf(),
                message: format!("invalid SHA-256 digest {:?}", self.0),
            })
        }
    }
}

impl VersionProbe {
    /// Parses only the ownership version before version-specific decoding.
    pub(super) fn from_bytes(path: &Path, bytes: &[u8]) -> Result<Self, CliError> {
        serde_json::from_slice(bytes).map_err(|source| CliError::InvalidGeneratedOwnership {
            path: path.to_path_buf(),
            message: source.to_string(),
        })
    }

    /// Returns the probed ownership wire-format version.
    pub(super) fn version(&self) -> u32 {
        self.version
    }
}

impl<'file> ReservationMarker<'file> {
    /// Builds a generation- and digest-bound reservation marker.
    pub(super) fn new(generation: u64, file: &'file OwnedFile) -> Self {
        Self {
            version: OwnershipManifest::VERSION,
            generation,
            path: file.path(),
            sha256: file.digest(),
        }
    }

    /// Serializes this marker as compact JSON plus one newline.
    pub(super) fn to_bytes(&self) -> Result<Vec<u8>, CliError> {
        let mut bytes =
            serde_json::to_vec(self).map_err(|source| CliError::InvalidGeneratedOwnership {
                path: PathBuf::from(self.path),
                message: source.to_string(),
            })?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        ContentDigest, OwnedCatalog, OwnedFile, OwnershipManifest, ReservationMarker, VersionProbe,
    };

    #[test]
    fn ownership_accepts_only_direct_root_yaml_documents() {
        let manifest_path = Path::new("generated-files.json");

        for accepted in ["main.yml", "main.YAML", "MAIN.YmL"] {
            let manifest = OwnershipManifest::committed(
                1,
                OwnedCatalog::from_files(vec![OwnedFile::new(
                    accepted.to_owned(),
                    ContentDigest::of(b"generated\n"),
                )]),
            );
            manifest
                .to_bytes(manifest_path)
                .expect("direct YAML document ownership");
        }

        for rejected in ["manual.txt", "main.json", "nested/main.yml"] {
            let manifest = OwnershipManifest::committed(
                1,
                OwnedCatalog::from_files(vec![OwnedFile::new(
                    rejected.to_owned(),
                    ContentDigest::of(b"generated\n"),
                )]),
            );
            let error = manifest
                .to_bytes(manifest_path)
                .expect_err("non-document ownership must fail");
            assert!(
                error
                    .to_string()
                    .contains("must be a direct root .yml or .yaml combined document"),
                "unexpected error for {rejected}: {error}"
            );
        }
    }

    #[test]
    fn updating_ownership_serializes_without_family_provenance() {
        let manifest_path = Path::new("generated-files.json");
        let manifest = OwnershipManifest::updating(
            2,
            OwnedCatalog::from_files(vec![OwnedFile::new(
                "before.yml".to_owned(),
                ContentDigest::of(b"before\n"),
            )]),
            OwnedCatalog::from_files(vec![OwnedFile::new(
                "after.yml".to_owned(),
                ContentDigest::of(b"after\n"),
            )]),
        );

        assert_eq!(
            String::from_utf8(
                manifest
                    .to_bytes(manifest_path)
                    .expect("updating ownership bytes"),
            )
            .expect("ownership JSON is UTF-8"),
            concat!(
                "{\n",
                "  \"version\": 3,\n",
                "  \"journal\": {\n",
                "    \"state\": \"updating\",\n",
                "    \"generation\": 2,\n",
                "    \"before\": {\n",
                "      \"documents\": [\n",
                "        {\n",
                "          \"path\": \"before.yml\",\n",
                "          \"sha256\": \"9160d4be34c8695bd172a76c7c7966587ea5a4d991ad22c87b2b91af54aa9ebb\"\n",
                "        }\n",
                "      ]\n",
                "    },\n",
                "    \"after\": {\n",
                "      \"documents\": [\n",
                "        {\n",
                "          \"path\": \"after.yml\",\n",
                "          \"sha256\": \"7b9a72466d3960eb2aacccfc848939453490db0678bd4725def3f789b891c919\"\n",
                "        }\n",
                "      ]\n",
                "    }\n",
                "  }\n",
                "}\n",
            ),
        );
    }

    #[test]
    fn owned_catalog_construction_sorts_and_manifest_round_trips() {
        let manifest_path = Path::new("generated-files.json");
        let catalog = OwnedCatalog::from_files(vec![
            OwnedFile::new("z.yml".to_owned(), ContentDigest::of(b"z\n")),
            OwnedFile::new("a.yaml".to_owned(), ContentDigest::of(b"a\n")),
        ]);
        assert_eq!(
            catalog.entries().map(OwnedFile::path).collect::<Vec<_>>(),
            ["a.yaml", "z.yml"]
        );

        let manifest = OwnershipManifest::committed(7, catalog);
        let encoded = manifest
            .to_bytes(manifest_path)
            .expect("committed manifest");
        assert_eq!(
            String::from_utf8(encoded.clone()).expect("ownership JSON is UTF-8"),
            concat!(
                "{\n",
                "  \"version\": 3,\n",
                "  \"journal\": {\n",
                "    \"state\": \"committed\",\n",
                "    \"generation\": 7,\n",
                "    \"files\": {\n",
                "      \"documents\": [\n",
                "        {\n",
                "          \"path\": \"a.yaml\",\n",
                "          \"sha256\": \"87428fc522803d31065e7bce3cf03fe475096631e5e07bbd7a0fde60c4cf25c7\"\n",
                "        },\n",
                "        {\n",
                "          \"path\": \"z.yml\",\n",
                "          \"sha256\": \"c865f6c5ab8d1b0bcd383a5e1e3879d22681c96bf462c269b7581d523fbe70ab\"\n",
                "        }\n",
                "      ]\n",
                "    }\n",
                "  }\n",
                "}\n",
            )
        );
        let decoded =
            OwnershipManifest::from_bytes(manifest_path, &encoded).expect("round-trip manifest");

        assert_eq!(decoded, manifest);
    }

    #[test]
    fn ownership_rejects_zero_generation_invalid_digest_and_unsorted_paths() {
        let manifest_path = Path::new("generated-files.json");
        let zero = OwnershipManifest::committed(0, OwnedCatalog::default())
            .to_bytes(manifest_path)
            .expect_err("zero generation");
        assert!(zero.to_string().contains("greater than zero"));

        let invalid_digest = br#"{
  "version": 3,
  "journal": {
    "state": "committed",
    "generation": 1,
    "files": {
      "documents": [{"path":"main.yml","sha256":"INVALID"}]
    }
  }
}"#;
        let digest_error = OwnershipManifest::from_bytes(manifest_path, invalid_digest)
            .expect_err("invalid digest");
        assert!(digest_error.to_string().contains("invalid SHA-256 digest"));

        let unsorted = format!(
            concat!(
                "{{\"version\":3,\"journal\":{{\"state\":\"committed\",",
                "\"generation\":1,\"files\":{{\"documents\":[",
                "{{\"path\":\"z.yml\",\"sha256\":\"{digest}\"}},",
                "{{\"path\":\"a.yml\",\"sha256\":\"{digest}\"}}]}}}}}}"
            ),
            digest = "0".repeat(64),
        );
        let sorting_error = OwnershipManifest::from_bytes(manifest_path, unsorted.as_bytes())
            .expect_err("unsorted catalog");
        assert!(sorting_error.to_string().contains("unique and sorted"));
    }

    #[test]
    fn ownership_rejects_unknown_fields_and_uppercase_digest_bytes() {
        let manifest_path = Path::new("generated-files.json");
        let digest = "0".repeat(64);
        let unknown_fields = [
            concat!(
                "{\"version\":3,\"journal\":{\"state\":\"committed\",",
                "\"generation\":1,\"files\":{\"documents\":[]}},",
                "\"unknown\":true}"
            )
            .to_owned(),
            concat!(
                "{\"version\":3,\"journal\":{\"state\":\"committed\",",
                "\"generation\":1,\"files\":{\"documents\":[]},",
                "\"unknown\":true}}"
            )
            .to_owned(),
            concat!(
                "{\"version\":3,\"journal\":{\"state\":\"committed\",",
                "\"generation\":1,\"files\":{\"documents\":[],",
                "\"unknown\":true}}}"
            )
            .to_owned(),
            format!(
                concat!(
                    "{{\"version\":3,\"journal\":{{\"state\":\"committed\",",
                    "\"generation\":1,\"files\":{{\"documents\":[",
                    "{{\"path\":\"main.yml\",\"sha256\":\"{digest}\",",
                    "\"unknown\":true}}]}}}}}}"
                ),
                digest = digest,
            ),
        ];

        for document in unknown_fields {
            let error = OwnershipManifest::from_bytes(manifest_path, document.as_bytes())
                .expect_err("unknown persisted field");
            assert!(error.to_string().contains("unknown field"));
        }

        let uppercase_digest = format!(
            concat!(
                "{{\"version\":3,\"journal\":{{\"state\":\"committed\",",
                "\"generation\":1,\"files\":{{\"documents\":[",
                "{{\"path\":\"main.yml\",\"sha256\":\"{}\"}}]}}}}}}"
            ),
            "A".repeat(64),
        );
        let error = OwnershipManifest::from_bytes(manifest_path, uppercase_digest.as_bytes())
            .expect_err("uppercase digest");
        assert!(error.to_string().contains("invalid SHA-256 digest"));
    }

    #[test]
    fn ownership_rejects_duplicate_and_ascii_case_equivalent_paths() {
        let manifest_path = Path::new("generated-files.json");
        for (first, second) in [("main.yml", "main.yml"), ("Main.yml", "main.YML")] {
            let catalog = OwnedCatalog::from_files(vec![
                OwnedFile::new(first.to_owned(), ContentDigest::of(b"first\n")),
                OwnedFile::new(second.to_owned(), ContentDigest::of(b"second\n")),
            ]);
            let error = OwnershipManifest::committed(1, catalog)
                .to_bytes(manifest_path)
                .expect_err("portable duplicate path");

            assert!(
                error
                    .to_string()
                    .contains("portable generated path collision"),
                "unexpected collision error: {error}"
            );
        }
    }

    #[test]
    fn updating_ownership_rejects_case_only_path_transitions() {
        let manifest_path = Path::new("generated-files.json");
        let before = OwnedCatalog::from_files(vec![OwnedFile::new(
            "Main.yml".to_owned(),
            ContentDigest::of(b"before\n"),
        )]);
        let after = OwnedCatalog::from_files(vec![OwnedFile::new(
            "main.yml".to_owned(),
            ContentDigest::of(b"after\n"),
        )]);

        let error = OwnershipManifest::updating(2, before, after)
            .to_bytes(manifest_path)
            .expect_err("case-only transition");

        assert!(error.to_string().contains("changes only ASCII case"));
    }

    #[test]
    fn ownership_versions_are_probed_and_obsolete_versions_are_not_migrated() {
        let manifest_path = Path::new("generated-files.json");
        let obsolete = br#"{
  "version": 2,
  "journal": {
    "state": "committed",
    "generation": 1,
    "files": {"documents": []}
  }
}"#;

        let probe = VersionProbe::from_bytes(manifest_path, obsolete).expect("version probe");
        assert_eq!(probe.version(), 2);
        let error = OwnershipManifest::from_bytes(manifest_path, obsolete)
            .expect_err("obsolete ownership version");
        assert!(
            error
                .to_string()
                .contains("unsupported ownership version 2")
        );
    }

    #[test]
    fn reservation_marker_uses_version_generation_path_and_digest() {
        let owned = OwnedFile::new("main.yml".to_owned(), ContentDigest::of(b"generated\n"));
        let bytes = ReservationMarker::new(9, &owned)
            .to_bytes()
            .expect("reservation marker");

        assert_eq!(
            String::from_utf8(bytes).expect("marker JSON is UTF-8"),
            concat!(
                "{\"version\":3,\"generation\":9,\"path\":\"main.yml\",",
                "\"sha256\":\"9f5936ff15d3a2ba7d3d8f21858338a6c1e2adc9fe34c685c7de5b4a00caa29a\"}\n"
            )
        );
    }

    #[test]
    fn ownership_manifest_constants_name_the_version_three_namespace() {
        assert_eq!(OwnershipManifest::VERSION, 3);
        assert_eq!(OwnershipManifest::DIRECTORY, ".contract-kit");
        assert_eq!(OwnershipManifest::FILE_NAME, "generated-files.json");
        assert_eq!(
            OwnershipManifest::TEMPORARY_PREFIX,
            ".generated-files.json."
        );
        assert_eq!(OwnershipManifest::ATOMIC_TEMPORARY_SUFFIX_LENGTH, 6);
        assert_eq!(
            OwnershipManifest::path(Path::new("contracts")),
            Path::new("contracts/.contract-kit/generated-files.json")
        );
    }
}
