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
use crate::context::ApplicationCancellation;
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
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(super) struct OwnedFile {
    path: CatalogPath,
    sha256: ContentDigest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedFileDocument {
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
    ///
    /// # Errors
    ///
    /// Returns an error if the bytes are not a valid version-3 manifest or its
    /// generation, document paths, digests, ordering, or transition is invalid.
    pub(super) fn from_bytes(
        path: &Path,
        bytes: &[u8],
        cancellation: &ApplicationCancellation,
    ) -> Result<Self, CliError> {
        cancellation.checkpoint()?;
        let manifest = match serde_json::from_slice::<Self>(bytes) {
            Ok(manifest) => manifest,
            Err(source) => {
                let message = source.to_string();
                if let Some(message) = OwnedFile::path_error_message(&message) {
                    cancellation.checkpoint()?;
                    return Err(CliError::InvalidGeneratedOwnership {
                        path: path.to_path_buf(),
                        message,
                    });
                }
                return Err(CliError::InvalidGeneratedOwnership {
                    path: path.to_path_buf(),
                    message,
                });
            }
        };
        cancellation.checkpoint()?;
        manifest.validate(path, cancellation)?;
        Ok(manifest)
    }

    /// Validates and serializes this manifest as pretty JSON plus one newline.
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest is invalid or JSON serialization fails.
    pub(super) fn to_bytes(
        &self,
        path: &Path,
        cancellation: &ApplicationCancellation,
    ) -> Result<Vec<u8>, CliError> {
        self.validate(path, cancellation)?;
        cancellation.checkpoint()?;
        let mut bytes = serde_json::to_vec_pretty(self).map_err(|source| {
            CliError::InvalidGeneratedOwnership {
                path: path.to_path_buf(),
                message: source.to_string(),
            }
        })?;
        cancellation.checkpoint()?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    fn validate(
        &self,
        path: &Path,
        cancellation: &ApplicationCancellation,
    ) -> Result<(), CliError> {
        cancellation.checkpoint()?;
        if self.version != Self::VERSION {
            return Err(CliError::InvalidGeneratedOwnership {
                path: path.to_path_buf(),
                message: format!("unsupported ownership version {}", self.version),
            });
        }

        match &self.journal {
            OwnershipJournal::Committed { generation, files } => {
                Self::validate_generation(*generation, path)?;
                files.validate(path, cancellation)
            }
            OwnershipJournal::Updating {
                generation,
                before,
                after,
            } => {
                Self::validate_generation(*generation, path)?;
                before.validate(path, cancellation)?;
                after.validate(path, cancellation)?;
                before.validate_transition(after, cancellation)
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
    pub(super) fn find(&self, path: &CatalogPath) -> Option<&OwnedFile> {
        self.documents
            .binary_search_by(|file| file.path.cmp(path))
            .ok()
            .map(|index| &self.documents[index])
    }

    /// Validates paths, digests, portable identity, and deterministic sorting.
    ///
    /// # Errors
    ///
    /// Returns an error if a document path or digest is invalid, paths collide
    /// under portable identity, or entries are not uniquely sorted.
    pub(super) fn validate(
        &self,
        manifest_path: &Path,
        cancellation: &ApplicationCancellation,
    ) -> Result<(), CliError> {
        let mut keys = BTreeMap::<PortableCatalogPathKey, String>::new();
        let mut previous_path = None::<&CatalogPath>;
        for file in self.entries() {
            cancellation.checkpoint()?;
            if previous_path.is_some_and(|previous| previous >= &file.path) {
                return Err(CliError::InvalidGeneratedOwnership {
                    path: manifest_path.to_path_buf(),
                    message: "owned document paths must be unique and sorted".to_owned(),
                });
            }
            previous_path = Some(&file.path);
            Self::validate_logical_path(&file.path, manifest_path)?;
            file.sha256.validate(manifest_path)?;
            let key = PortableCatalogPathKey::new(&file.path);
            if let Some(previous) = keys.insert(key, file.path.as_str().to_owned()) {
                return Err(CliError::PortableGeneratedPathCollision {
                    first: previous,
                    second: file.path.as_str().to_owned(),
                });
            }
        }
        Ok(())
    }

    /// Rejects case-only path changes across an ownership transition.
    ///
    /// # Errors
    ///
    /// Returns an error if a previous path is invalid or the transition changes
    /// only the case of a portable path.
    pub(super) fn validate_transition(
        &self,
        after: &Self,
        cancellation: &ApplicationCancellation,
    ) -> Result<(), CliError> {
        let mut after_by_key = BTreeMap::new();
        for after_file in after.entries() {
            cancellation.checkpoint()?;
            after_by_key.insert(PortableCatalogPathKey::new(&after_file.path), after_file);
        }
        for before_file in self.entries() {
            cancellation.checkpoint()?;
            let key = PortableCatalogPathKey::new(&before_file.path);
            if let Some(after_file) = after_by_key.get(&key)
                && before_file.path != after_file.path
            {
                return Err(CliError::CaseOnlyGeneratedPathChange {
                    previous: before_file.path.as_str().to_owned(),
                    current: after_file.path.as_str().to_owned(),
                });
            }
        }
        Ok(())
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
    const PATH_ERROR_PREFIX: &'static str = "invalid persisted ownership path: ";

    /// Builds one persisted owned-file value.
    pub(super) fn new(path: CatalogPath, sha256: ContentDigest) -> Self {
        Self { path, sha256 }
    }

    /// Returns the persisted logical path.
    pub(super) fn path(&self) -> &CatalogPath {
        &self.path
    }

    /// Returns the persisted content digest.
    pub(super) fn digest(&self) -> &ContentDigest {
        &self.sha256
    }

    fn path_error_message(message: &str) -> Option<String> {
        let message = message.strip_prefix(Self::PATH_ERROR_PREFIX)?;
        let message = message
            .rsplit_once(" at line ")
            .filter(|(_, location)| location.contains(" column "))
            .map_or(message, |(message, _)| message);
        Some(message.to_owned())
    }
}

impl<'de> Deserialize<'de> for OwnedFile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let document = OwnedFileDocument::deserialize(deserializer)?;
        let path = CatalogPath::new(document.path).map_err(|source| {
            serde::de::Error::custom(format!("{}{source}", Self::PATH_ERROR_PREFIX))
        })?;
        Ok(Self::new(path, document.sha256))
    }
}

impl ContentDigest {
    /// Computes a lowercase hexadecimal SHA-256 digest.
    pub(super) fn of(
        bytes: &[u8],
        cancellation: &ApplicationCancellation,
    ) -> Result<Self, CliError> {
        let mut digest = Sha256::new();
        for chunk in bytes.chunks(64 * 1024) {
            cancellation.checkpoint()?;
            digest.update(chunk);
        }
        cancellation.checkpoint()?;
        Ok(Self(format!("{:x}", digest.finalize())))
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
    ///
    /// # Errors
    ///
    /// Returns an error if the bytes do not contain a valid JSON version probe.
    pub(super) fn from_bytes(
        path: &Path,
        bytes: &[u8],
        cancellation: &ApplicationCancellation,
    ) -> Result<Self, CliError> {
        cancellation.checkpoint()?;
        let probe = serde_json::from_slice::<Self>(bytes).map_err(|source| {
            CliError::InvalidGeneratedOwnership {
                path: path.to_path_buf(),
                message: source.to_string(),
            }
        })?;
        cancellation.checkpoint()?;
        Ok(probe)
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
            path: file.path().as_str(),
            sha256: file.digest(),
        }
    }

    /// Serializes this marker as compact JSON plus one newline.
    ///
    /// # Errors
    ///
    /// Returns an error if the reservation marker cannot be serialized as JSON.
    pub(super) fn to_bytes(
        &self,
        cancellation: &ApplicationCancellation,
    ) -> Result<Vec<u8>, CliError> {
        cancellation.checkpoint()?;
        let mut bytes =
            serde_json::to_vec(self).map_err(|source| CliError::InvalidGeneratedOwnership {
                path: PathBuf::from(self.path),
                message: source.to_string(),
            })?;
        cancellation.checkpoint()?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use conkit_signature::CatalogPath;

    use super::{
        ContentDigest, OwnedCatalog, OwnedFile, OwnershipManifest, ReservationMarker, VersionProbe,
    };
    use crate::context::ApplicationCancellation;
    use crate::error::CliError;

    struct OwnershipFixture;

    impl OwnershipFixture {
        fn cancellation() -> ApplicationCancellation {
            ApplicationCancellation::new()
        }

        fn digest(bytes: &[u8]) -> ContentDigest {
            ContentDigest::of(bytes, &Self::cancellation()).expect("content digest")
        }

        fn owned(path: &str, bytes: &[u8]) -> OwnedFile {
            OwnedFile::new(
                CatalogPath::new(path).expect("owned catalog path"),
                Self::digest(bytes),
            )
        }

        fn encode(
            manifest: &OwnershipManifest,
            path: &Path,
        ) -> Result<Vec<u8>, crate::error::CliError> {
            manifest.to_bytes(path, &Self::cancellation())
        }

        fn decode(path: &Path, bytes: &[u8]) -> Result<OwnershipManifest, crate::error::CliError> {
            OwnershipManifest::from_bytes(path, bytes, &Self::cancellation())
        }
    }

    #[test]
    fn ownership_accepts_only_direct_root_yaml_documents() {
        let manifest_path = Path::new("generated-files.json");

        for accepted in ["main.yml", "main.YAML", "MAIN.YmL"] {
            let manifest = OwnershipManifest::committed(
                1,
                OwnedCatalog::from_files(vec![OwnershipFixture::owned(accepted, b"generated\n")]),
            );
            OwnershipFixture::encode(&manifest, manifest_path)
                .expect("direct YAML document ownership");
        }

        for rejected in ["manual.txt", "main.json", "nested/main.yml"] {
            let manifest = OwnershipManifest::committed(
                1,
                OwnedCatalog::from_files(vec![OwnershipFixture::owned(rejected, b"generated\n")]),
            );
            let error = OwnershipFixture::encode(&manifest, manifest_path)
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
            OwnedCatalog::from_files(vec![OwnershipFixture::owned("before.yml", b"before\n")]),
            OwnedCatalog::from_files(vec![OwnershipFixture::owned("after.yml", b"after\n")]),
        );

        assert_eq!(
            String::from_utf8(
                OwnershipFixture::encode(&manifest, manifest_path)
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
            OwnershipFixture::owned("z.yml", b"z\n"),
            OwnershipFixture::owned("a.yaml", b"a\n"),
        ]);
        assert_eq!(
            catalog
                .entries()
                .map(OwnedFile::path)
                .map(CatalogPath::as_str)
                .collect::<Vec<_>>(),
            ["a.yaml", "z.yml"]
        );

        let manifest = OwnershipManifest::committed(7, catalog);
        let encoded =
            OwnershipFixture::encode(&manifest, manifest_path).expect("committed manifest");
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
            OwnershipFixture::decode(manifest_path, &encoded).expect("round-trip manifest");

        assert_eq!(decoded, manifest);
    }

    #[test]
    fn ownership_rejects_zero_generation_invalid_digest_and_unsorted_paths() {
        let manifest_path = Path::new("generated-files.json");
        let zero_manifest = OwnershipManifest::committed(0, OwnedCatalog::default());
        let zero =
            OwnershipFixture::encode(&zero_manifest, manifest_path).expect_err("zero generation");
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
        let digest_error =
            OwnershipFixture::decode(manifest_path, invalid_digest).expect_err("invalid digest");
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
        let sorting_error = OwnershipFixture::decode(manifest_path, unsorted.as_bytes())
            .expect_err("unsorted catalog");
        assert!(sorting_error.to_string().contains("unique and sorted"));

        let invalid_path = format!(
            concat!(
                "{{\"version\":3,\"journal\":{{\"state\":\"committed\",",
                "\"generation\":1,\"files\":{{\"documents\":[",
                "{{\"path\":\"../main.yml\",\"sha256\":\"{}\"}}]}}}}}}"
            ),
            "0".repeat(64),
        );
        let path_error = OwnershipFixture::decode(manifest_path, invalid_path.as_bytes())
            .expect_err("typed path rejects invalid persisted spelling");
        let expected_path_error = CatalogPath::new("../main.yml")
            .expect_err("invalid path fixture")
            .to_string();
        assert!(matches!(
            path_error,
            CliError::InvalidGeneratedOwnership { message, .. }
                if message == expected_path_error
        ));

        let cancellation = ApplicationCancellation::new();
        cancellation.request();
        let canceled =
            OwnershipManifest::from_bytes(manifest_path, invalid_path.as_bytes(), &cancellation)
                .expect_err("cancellation retains precedence after parsing an invalid path");
        assert!(matches!(canceled, CliError::OperationCanceled));
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
            let error = OwnershipFixture::decode(manifest_path, document.as_bytes())
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
        let error = OwnershipFixture::decode(manifest_path, uppercase_digest.as_bytes())
            .expect_err("uppercase digest");
        assert!(error.to_string().contains("invalid SHA-256 digest"));
    }

    #[test]
    fn ownership_rejects_duplicate_and_ascii_case_equivalent_paths() {
        let manifest_path = Path::new("generated-files.json");
        let duplicate = OwnedCatalog::from_files(vec![
            OwnershipFixture::owned("main.yml", b"first\n"),
            OwnershipFixture::owned("main.yml", b"second\n"),
        ]);
        let duplicate =
            OwnershipFixture::encode(&OwnershipManifest::committed(1, duplicate), manifest_path)
                .expect_err("exact duplicate path");
        assert!(matches!(
            duplicate,
            CliError::InvalidGeneratedOwnership { message, .. }
                if message == "owned document paths must be unique and sorted"
        ));

        let case_equivalent = OwnedCatalog::from_files(vec![
            OwnershipFixture::owned("Main.yml", b"first\n"),
            OwnershipFixture::owned("main.YML", b"second\n"),
        ]);
        let case_equivalent = OwnershipFixture::encode(
            &OwnershipManifest::committed(1, case_equivalent),
            manifest_path,
        )
        .expect_err("ASCII-case-equivalent path");
        assert!(matches!(
            case_equivalent,
            CliError::PortableGeneratedPathCollision { first, second }
                if first == "Main.yml" && second == "main.YML"
        ));
    }

    #[test]
    fn updating_ownership_rejects_case_only_path_transitions() {
        let manifest_path = Path::new("generated-files.json");
        let before =
            OwnedCatalog::from_files(vec![OwnershipFixture::owned("Main.yml", b"before\n")]);
        let after = OwnedCatalog::from_files(vec![OwnershipFixture::owned("main.yml", b"after\n")]);

        let manifest = OwnershipManifest::updating(2, before, after);
        let error =
            OwnershipFixture::encode(&manifest, manifest_path).expect_err("case-only transition");

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

        let probe =
            VersionProbe::from_bytes(manifest_path, obsolete, &OwnershipFixture::cancellation())
                .expect("version probe");
        assert_eq!(probe.version(), 2);
        let error = OwnershipFixture::decode(manifest_path, obsolete)
            .expect_err("obsolete ownership version");
        assert!(
            error
                .to_string()
                .contains("unsupported ownership version 2")
        );
    }

    #[test]
    fn reservation_marker_uses_version_generation_path_and_digest() {
        let owned = OwnershipFixture::owned("main.yml", b"generated\n");
        let bytes = ReservationMarker::new(9, &owned)
            .to_bytes(&OwnershipFixture::cancellation())
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
    fn ownership_hashing_and_validation_observe_cancellation() {
        let cancellation = OwnershipFixture::cancellation();
        cancellation.request();
        let digest_error = ContentDigest::of(b"generated\n", &cancellation)
            .expect_err("pre-canceled hashing must stop");
        let manifest = OwnershipManifest::committed(1, OwnedCatalog::default());
        let manifest_error = manifest
            .to_bytes(Path::new("generated-files.json"), &cancellation)
            .expect_err("pre-canceled ownership validation must stop");

        assert!(matches!(
            digest_error,
            crate::error::CliError::OperationCanceled
        ));
        assert!(matches!(
            manifest_error,
            crate::error::CliError::OperationCanceled
        ));
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
