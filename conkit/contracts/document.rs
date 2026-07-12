//! Combined contract document paths and CLI-owned header validation.

use std::path::{Component, Path};

use conkit_signature::CatalogPath;
use serde::{Deserialize, de::IgnoredAny};

use crate::error::CliError;
use crate::platform::PortablePathRules;

/// A direct root-level YAML combined contract document path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContractDocumentPath(CatalogPath);

/// Error returned when a catalog path cannot identify a combined document.
#[derive(Debug, thiserror::Error)]
#[error("{path} is not a direct root .yml or .yaml contract document")]
pub(crate) struct InvalidContractDocumentPath {
    path: CatalogPath,
}

impl TryFrom<CatalogPath> for ContractDocumentPath {
    type Error = InvalidContractDocumentPath;

    fn try_from(path: CatalogPath) -> Result<Self, Self::Error> {
        let direct = !path.as_str().contains('/');
        let yaml = Path::new(path.as_str())
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("yml") || extension.eq_ignore_ascii_case("yaml")
            });

        if direct && yaml {
            Ok(Self(path))
        } else {
            Err(InvalidContractDocumentPath { path })
        }
    }
}

impl ContractDocumentPath {
    /// Borrows the validated logical catalog path.
    pub(crate) fn as_catalog_path(&self) -> &CatalogPath {
        &self.0
    }

    /// Consumes this checked path into its logical catalog path.
    pub(crate) fn into_catalog_path(self) -> CatalogPath {
        self.0
    }
}

/// One parsed document header paired with its original bytes.
pub(super) struct ContractDocument {
    path: ContractDocumentPath,
    bytes: Vec<u8>,
    source_paths: Vec<CatalogPath>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentHeader {
    root: String,
    files: Vec<String>,
    #[serde(rename = "signatures")]
    _signatures: Vec<IgnoredAny>,
    #[serde(default, rename = "sketches")]
    _sketches: Vec<IgnoredAny>,
}

impl ContractDocument {
    /// Parses one checked combined document and binds its root to `source`.
    pub(super) fn parse(
        path: ContractDocumentPath,
        bytes: Vec<u8>,
        contracts: &Path,
        source: &Path,
        canonical_source: &Path,
    ) -> Result<Self, CliError> {
        let document_path = contracts.join(path.as_catalog_path().as_str());
        let DocumentHeader { root, files, .. } = serde_yaml::from_slice::<DocumentHeader>(&bytes)
            .map_err(|source_error| {
            CliError::ContractLayout {
                path: document_path.clone(),
                message: source_error.to_string(),
            }
        })?;

        Self::validate_root(&document_path, contracts, source, canonical_source, &root)?;

        let mut source_paths = Vec::with_capacity(files.len());
        for listed in files {
            let logical = CatalogPath::new(listed.clone()).map_err(|source_error| {
                CliError::ContractLayout {
                    path: document_path.clone(),
                    message: format!("invalid listed source path {listed:?}: {source_error}"),
                }
            })?;
            let rust_source = Path::new(logical.as_str())
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("rs"));
            if !rust_source {
                return Err(CliError::ContractLayout {
                    path: document_path,
                    message: format!(
                        "listed source path {} must have a .rs extension",
                        logical.as_str()
                    ),
                });
            }
            source_paths.push(logical);
        }

        Ok(Self {
            path,
            bytes,
            source_paths,
        })
    }

    /// Consumes the parsed document into its aggregate-layout values.
    pub(super) fn into_parts(self) -> (ContractDocumentPath, Vec<u8>, Vec<CatalogPath>) {
        (self.path, self.bytes, self.source_paths)
    }

    fn validate_root(
        document: &Path,
        contracts: &Path,
        source: &Path,
        canonical_source: &Path,
        declared: &str,
    ) -> Result<(), CliError> {
        let declared_path = Path::new(declared);
        if declared.is_empty() || declared.contains('\\') || declared_path.is_absolute() {
            return Err(CliError::ContractLayout {
                path: document.to_path_buf(),
                message: format!("contract root must be a nonempty relative path: {declared:?}"),
            });
        }

        for component in declared_path.components() {
            match component {
                Component::Normal(value) => PortablePathRules::validate_component(value)?,
                Component::ParentDir => {}
                Component::CurDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(CliError::ContractLayout {
                        path: document.to_path_buf(),
                        message: format!(
                            "contract root contains an invalid component: {declared:?}"
                        ),
                    });
                }
            }
        }

        let resolved = contracts.join(declared_path);
        let canonical_declared = fs_err::canonicalize(&resolved).map_err(|source_error| {
            CliError::ContractLayout {
                path: document.to_path_buf(),
                message: format!(
                    "failed to resolve contract root {declared:?} relative to the document: {source_error}"
                ),
            }
        })?;
        if canonical_declared != canonical_source {
            return Err(CliError::ContractLayout {
                path: document.to_path_buf(),
                message: format!(
                    "contract root {declared:?} resolves to {}, not selected source {}",
                    canonical_declared.display(),
                    source.display()
                ),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use assert_fs::prelude::*;
    use conkit_signature::CatalogPath;

    use super::{ContractDocument, ContractDocumentPath};

    struct DocumentFixture {
        root: assert_fs::TempDir,
        contracts: PathBuf,
        source: PathBuf,
        canonical_source: PathBuf,
    }

    impl DocumentFixture {
        fn new() -> Self {
            let root = assert_fs::TempDir::new().expect("temporary root");
            let source = root.child("src");
            source.create_dir_all().expect("source root");
            let contracts = root.child("contracts");
            contracts.create_dir_all().expect("contracts root");
            let canonical_source = fs_err::canonicalize(source.path()).expect("canonical source");

            Self {
                root,
                contracts: contracts.path().to_path_buf(),
                source: source.path().to_path_buf(),
                canonical_source,
            }
        }

        fn parse(&self, bytes: &[u8]) -> Result<ContractDocument, crate::error::CliError> {
            ContractDocument::parse(
                ContractDocumentPath::try_from(
                    CatalogPath::new("main.yml").expect("document path"),
                )
                .expect("checked document path"),
                bytes.to_vec(),
                &self.contracts,
                &self.source,
                &self.canonical_source,
            )
        }

        fn close(self) {
            self.root.close().expect("close temporary root");
        }
    }

    #[test]
    fn document_paths_are_direct_case_insensitive_yaml_files() {
        for accepted in ["main.yml", "main.YAML", "MAIN.YmL"] {
            let path = CatalogPath::new(accepted).expect("accepted document path");
            assert!(ContractDocumentPath::try_from(path).is_ok());
        }

        for rejected in ["manual.txt", "main.json", "nested/main.yml"] {
            let path = CatalogPath::new(rejected).expect("rejected document path");
            assert!(ContractDocumentPath::try_from(path).is_err());
        }
    }

    #[test]
    fn parses_header_and_preserves_original_bytes_and_rust_paths() {
        let fixture = DocumentFixture::new();
        let bytes = b"root: ../src\nfiles: [lib.RS]\nsignatures: [42]\nsketches: [not: semantic]\n";

        let document = fixture.parse(bytes).expect("valid document");
        let (path, parsed_bytes, sources) = document.into_parts();

        assert_eq!(path.as_catalog_path().as_str(), "main.yml");
        assert_eq!(parsed_bytes, bytes);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].as_str(), "lib.RS");
        fixture.close();
    }

    #[test]
    fn rejects_malformed_or_unknown_header_fields() {
        let fixture = DocumentFixture::new();

        for (document, expected) in [
            ("root: ../src\nfiles: []\n", "missing field `signatures`"),
            (
                "root: ../src\nfiles: []\nsignatures: value\n",
                "expected a sequence",
            ),
            (
                "root: ../src\nfiles: []\nsignatures: []\nsketches: value\n",
                "expected a sequence",
            ),
            (
                "root: ../src\nfiles: []\nsignatures: []\nunknown: true\n",
                "unknown field `unknown`",
            ),
            (
                "version: 1\nlanguage: rust\nsignatures: []\n",
                "unknown field `version`",
            ),
        ] {
            let error = match fixture.parse(document.as_bytes()) {
                Ok(_) => panic!("invalid header must be rejected"),
                Err(error) => error,
            };
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?} in {error}"
            );
        }

        fixture.close();
    }

    #[test]
    fn rejects_roots_that_do_not_bind_to_the_selected_source() {
        let fixture = DocumentFixture::new();
        fixture
            .root
            .child("other")
            .create_dir_all()
            .expect("other root");

        let error =
            match fixture.parse(b"root: ../other\nfiles: []\nsignatures: []\nsketches: []\n") {
                Ok(_) => panic!("root mismatch must fail"),
                Err(error) => error,
            };

        assert!(error.to_string().contains("not selected source"));
        fixture.close();
    }

    #[test]
    fn rejects_invalid_root_components_and_backslashes() {
        let fixture = DocumentFixture::new();

        for declared in ["", ".", "../src\\nested", "../invalid."] {
            let bytes = format!("root: {declared:?}\nfiles: []\nsignatures: []\nsketches: []\n");
            assert!(
                fixture.parse(bytes.as_bytes()).is_err(),
                "accepted {declared:?}"
            );
        }

        fixture.close();
    }

    #[test]
    fn rejects_invalid_or_non_rust_source_allowlist_paths() {
        let fixture = DocumentFixture::new();

        for listed in ["notes.txt", "nested/../lib.rs"] {
            let bytes =
                format!("root: ../src\nfiles: [{listed:?}]\nsignatures: []\nsketches: []\n");
            assert!(
                fixture.parse(bytes.as_bytes()).is_err(),
                "accepted {listed:?}"
            );
        }

        fixture.close();
    }
}
