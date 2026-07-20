//! Combined contract-document facade and physical-file orchestration.
//!
//! This module recognizes direct-root `.yml` and `.yaml` files, parses only the
//! CLI-owned mandatory-v2 header, binds each declared root to the selected
//! source tree, and retains original bytes. Its YAML child owns operation-wide
//! raw and semantic resource accounting; signature and sketch payload semantics
//! remain in their domain crates.

mod header;
pub(super) mod yaml;

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use conkit_signature::CatalogPath;
use serde_saphyr::{DuplicateKeyPolicy, MergeKeyPolicy};

use super::layout::LayoutExtraction;
use crate::error::{CliError, DuplicateContractKey};

pub(crate) use header::ContractCompilerContext;
use header::{ContractLocation, DocumentHeader};
use yaml::ContractYamlStream;
pub(super) use yaml::{ContractYamlLimits, ContractYamlUsage};

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

/// One physical YAML file with its indexed v2 headers and original bytes.
#[derive(Debug)]
pub(super) struct ContractDocument {
    path: ContractDocumentPath,
    bytes: Vec<u8>,
    plans: Vec<ContractDocumentPlan>,
}

/// One semantic document header emitted while parsing a physical YAML file.
#[derive(Debug)]
pub(super) struct ContractDocumentPlan {
    declared_root: String,
    pub(super) source_paths: Vec<CatalogPath>,
    pub(super) extraction: Option<LayoutExtraction>,
}

impl ContractDocument {
    /// Parses one checked combined document and binds its root to `source`.
    ///
    /// # Errors
    ///
    /// Returns an error if the header is invalid, its declared root does not
    /// resolve to `source`, or a listed source is not a portable Rust path.
    pub(super) fn parse(
        path: ContractDocumentPath,
        bytes: Vec<u8>,
        contracts: &Path,
        source: &Path,
        canonical_source: &Path,
        usage: &mut ContractYamlUsage<'_>,
    ) -> Result<Self, CliError> {
        let document = Self::from_bytes(path, bytes, usage)?;
        let document_path = contracts.join(document.path.as_catalog_path().as_str());
        for (document_index, plan) in document.plans.iter().enumerate() {
            let location = ContractLocation {
                contract_file: document.path.as_catalog_path(),
                display_path: &document_path,
                document_index,
                cancellation: usage.cancellation(),
            };
            location.checkpoint()?;
            Self::validate_root_binding(
                &location,
                contracts,
                source,
                canonical_source,
                &plan.declared_root,
            )?;
        }
        Ok(document)
    }

    /// Consumes the parsed document into its aggregate-layout values.
    pub(super) fn into_parts(self) -> (ContractDocumentPath, Vec<u8>, Vec<ContractDocumentPlan>) {
        (self.path, self.bytes, self.plans)
    }

    pub(super) fn validate_bytes(
        path: ContractDocumentPath,
        bytes: &[u8],
        usage: &mut ContractYamlUsage<'_>,
    ) -> Result<(), CliError> {
        Self::parse_plans(path.as_catalog_path(), bytes, usage).map(|_| ())
    }

    fn from_bytes(
        path: ContractDocumentPath,
        bytes: Vec<u8>,
        usage: &mut ContractYamlUsage<'_>,
    ) -> Result<Self, CliError> {
        let plans = Self::parse_plans(path.as_catalog_path(), &bytes, usage)?;
        Ok(Self { path, bytes, plans })
    }

    fn parse_plans(
        contract_file: &CatalogPath,
        bytes: &[u8],
        usage: &mut ContractYamlUsage<'_>,
    ) -> Result<Vec<ContractDocumentPlan>, CliError> {
        let document_path = Path::new(contract_file.as_str());
        let source =
            std::str::from_utf8(bytes).map_err(|source_error| CliError::ContractLayout {
                path: document_path.to_path_buf(),
                message: format!("contract YAML must be valid UTF-8: {source_error}"),
            })?;
        let stream = ContractYamlStream::inspect(document_path, source, usage)?;

        let semantic_report = Rc::new(RefCell::new(None));
        let report_sink = Rc::clone(&semantic_report);
        let options = serde_saphyr::options! {
            budget: usage.semantic_parser_budget(stream.raw_report()),
            alias_limits: usage.semantic_alias_limits(stream.raw_report()),
            duplicate_keys: DuplicateKeyPolicy::Error,
            merge_keys: MergeKeyPolicy::Error,
            strict_booleans: true,
        }
        .with_budget_report(move |report| {
            *report_sink.borrow_mut() = Some(report);
        });
        usage.cancellation().checkpoint()?;
        let headers = serde_saphyr::from_multiple_with_options::<DocumentHeader>(source, options)
            .map_err(|source_error| {
            let semantic_error = source_error.without_snippet();
            if let Some(limit) =
                usage.semantic_limit_error(document_path, stream.raw_report(), semantic_error)
            {
                return limit;
            }
            let document_index = stream.document_index_for_error(&source_error);
            match semantic_error {
                serde_saphyr::Error::DuplicateMappingKey { key, location } => {
                    CliError::from(DuplicateContractKey {
                        path: document_path.to_path_buf(),
                        document_index,
                        key: key.clone(),
                        location: *location,
                    })
                }
                _ => CliError::ContractLayout {
                    path: document_path.to_path_buf(),
                    message: format!(
                        "YAML document index {document_index} is invalid: {source_error}"
                    ),
                },
            }
        })?;
        usage.cancellation().checkpoint()?;
        let report =
            semantic_report
                .borrow_mut()
                .take()
                .ok_or_else(|| CliError::ContractLayout {
                    path: document_path.to_path_buf(),
                    message: "semantic YAML parser did not return its resource report".to_owned(),
                })?;
        usage.record_replay_report(document_path, stream.raw_report(), &report)?;

        let mut plans = Vec::new();
        for (document_index, header) in headers.into_iter().enumerate() {
            let location = ContractLocation {
                contract_file,
                display_path: document_path,
                document_index,
                cancellation: usage.cancellation(),
            };
            location.checkpoint()?;
            plans.push(header.into_plan(&location)?);
        }

        if plans.len() != stream.document_count() {
            return Err(CliError::ContractLayout {
                path: document_path.to_path_buf(),
                message: format!(
                    "semantic YAML decoding returned {} documents for a physical stream containing {}",
                    plans.len(),
                    stream.document_count()
                ),
            });
        }

        Ok(plans)
    }

    fn validate_root_binding(
        location: &ContractLocation<'_>,
        contracts: &Path,
        source: &Path,
        canonical_source: &Path,
        declared: &str,
    ) -> Result<(), CliError> {
        location.checkpoint()?;
        let declared_path = Path::new(declared);
        let resolved = contracts.join(declared_path);
        let canonical_declared = fs_err::canonicalize(&resolved).map_err(|source_error| {
            location.invalid(format!(
                "failed to resolve contract root {declared:?} relative to the document: {source_error}"
            ))
        })?;
        if canonical_declared != canonical_source {
            return Err(location.invalid(format!(
                "contract root {declared:?} resolves to {}, not selected source {}",
                canonical_declared.display(),
                source.display()
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use assert_fs::prelude::*;
    use conkit_signature::CatalogPath;

    use super::{ContractDocument, ContractDocumentPath, ContractYamlUsage};
    use crate::context::ApplicationCancellation;
    use crate::error::CliError;

    pub(super) struct DocumentFixture {
        root: assert_fs::TempDir,
        contracts: PathBuf,
        source: PathBuf,
        canonical_source: PathBuf,
    }

    impl DocumentFixture {
        pub(super) fn new() -> Self {
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

        pub(super) fn parse(
            &self,
            bytes: &[u8],
        ) -> Result<ContractDocument, crate::error::CliError> {
            let cancellation = ApplicationCancellation::new();
            let mut usage = ContractYamlUsage::new(&cancellation);
            ContractDocument::parse(
                ContractDocumentPath::try_from(
                    CatalogPath::new("main.yml").expect("document path"),
                )
                .expect("checked document path"),
                bytes.to_vec(),
                &self.contracts,
                &self.source,
                &self.canonical_source,
                &mut usage,
            )
        }

        pub(super) fn document(&self, files: &[&str], crate_roots: &[(&str, &str)]) -> String {
            let files = if files.is_empty() {
                " []\n".to_owned()
            } else {
                format!(
                    "\n{}",
                    files
                        .iter()
                        .map(|file| format!("  - {file}\n"))
                        .collect::<String>()
                )
            };
            let extraction = if crate_roots.is_empty() {
                String::new()
            } else {
                format!(
                    "extraction:\n  mode: rust_syntax_v2\n  profile: rust_api_v1\n  crates:\n{}",
                    crate_roots
                        .iter()
                        .map(|(id, root)| {
                            format!("    - id: {id}\n      root: {root}\n      kind: library\n")
                        })
                        .collect::<String>()
                )
            };

            format!(
                "contract_version: 2\nroot: ../src\nfiles:{files}{extraction}signatures: []\nsketches: []\n"
            )
        }

        pub(super) fn close(self) {
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
    fn parses_header_and_preserves_original_bytes_and_listed_rust_paths() {
        let fixture = DocumentFixture::new();
        let first = fixture.document(&["lib.RS"], &[("primary", "lib.RS")]);
        let second = fixture.document(
            &["nested/transport.rs"],
            &[("transport", "nested/transport.rs")],
        );
        let bytes = format!("{first}---\n{second}").into_bytes();

        let document = fixture.parse(&bytes).expect("valid document stream");
        let (path, parsed_bytes, plans) = document.into_parts();

        assert_eq!(path.as_catalog_path().as_str(), "main.yml");
        assert_eq!(parsed_bytes, bytes);
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].source_paths[0].as_str(), "lib.RS");
        assert_eq!(plans[1].source_paths[0].as_str(), "nested/transport.rs");
        fixture.close();
    }

    #[test]
    fn duplicate_keys_use_a_cloneable_typed_error_with_the_physical_document_location() {
        let fixture = DocumentFixture::new();
        let nested = fixture
            .document(&["lib.rs"], &[("primary", "lib.rs")])
            .replacen(
                "  mode: rust_syntax_v2\n",
                "  mode: rust_syntax_v2\n  mode: rust_syntax_v2\n",
                1,
            );
        let nested_error = fixture
            .parse(nested.as_bytes())
            .expect_err("duplicate nested key must fail");
        let nested_duplicate = match nested_error {
            CliError::DuplicateContractKey(duplicate) => duplicate,
            other => panic!("expected typed duplicate-key error, received {other}"),
        };
        assert_eq!(nested_duplicate.document_index, 0);
        assert_eq!(nested_duplicate.key.as_deref(), Some("mode"));

        let first = fixture.document(&["lib.rs"], &[("primary", "lib.rs")]);
        let second = fixture
            .document(&["other.rs"], &[("other", "other.rs")])
            .replacen("root: ../src\n", "root: ../src\nroot: ../src\n", 1);
        let expected_line = first.lines().count() as u64 + 4;
        let error = fixture
            .parse(format!("{first}---\n{second}").as_bytes())
            .expect_err("duplicate key in the second physical document must fail");

        let duplicate = match error {
            CliError::DuplicateContractKey(duplicate) => duplicate,
            other => panic!("expected typed duplicate-key error, received {other}"),
        };
        let cloned = duplicate.clone();

        assert_eq!(duplicate, cloned);
        assert_eq!(duplicate.path, PathBuf::from("main.yml"));
        assert_eq!(duplicate.document_index, 1);
        assert_eq!(duplicate.key.as_deref(), Some("root"));
        assert_eq!(duplicate.location.line(), expected_line);
        assert_eq!(duplicate.location.column(), 1);
        fixture.close();
    }

    #[test]
    fn enforces_strict_nested_yaml_fields_and_merge_policy() {
        let fixture = DocumentFixture::new();
        let valid = fixture.document(&["lib.rs"], &[("primary", "lib.rs")]);

        for (document, expected) in [
            (
                valid.replacen(
                    "      kind: library\n",
                    "      kind: library\n      unknown: true\n",
                    1,
                ),
                "unknown field `unknown`",
            ),
            (
                valid.replacen(
                    "  mode: rust_syntax_v2\n",
                    "  <<: &defaults { mode: rust_syntax_v2 }\n  mode: rust_syntax_v2\n",
                    1,
                ),
                "merge",
            ),
        ] {
            let error = fixture
                .parse(document.as_bytes())
                .expect_err("strict nested YAML policy must reject the input");
            assert!(
                error.to_string().to_lowercase().contains(expected),
                "expected {expected:?} in {error}"
            );
        }

        fixture.close();
    }

    #[test]
    fn accepts_anchors_aliases_and_explicit_tags_in_domain_owned_payloads() {
        let fixture = DocumentFixture::new();
        let document = fixture
            .document(&["lib.rs"], &[("primary", "lib.rs")])
            .replace("  - lib.rs\n", "  - &source_file lib.rs\n")
            .replace("      root: lib.rs\n", "      root: *source_file\n")
            .replace("signatures: []", "signatures: [!signature domain-owned]")
            .replace("sketches: []", "sketches: !sketches [domain-owned]");

        fixture
            .parse(document.as_bytes())
            .expect("anchors, aliases, and opaque explicit tags are accepted");
        fixture.close();
    }

    #[test]
    fn rejects_invalid_utf8_before_yaml_decoding() {
        let fixture = DocumentFixture::new();
        let error = fixture
            .parse(&[0xff])
            .expect_err("contract bytes must be UTF-8");

        assert!(error.to_string().contains("valid UTF-8"), "{error}");
        fixture.close();
    }

    #[test]
    fn cancellation_stops_aggregate_header_validation_before_parsing() {
        let fixture = DocumentFixture::new();
        let document = fixture.document(&["lib.rs"], &[("primary", "lib.rs")]);
        let canceled = ApplicationCancellation::new();
        canceled.request();
        let mut usage = ContractYamlUsage::new(&canceled);
        let error = ContractDocument::validate_bytes(
            ContractDocumentPath::try_from(
                CatalogPath::new("canceled.yml").expect("canceled document path"),
            )
            .expect("checked canceled path"),
            document.as_bytes(),
            &mut usage,
        )
        .expect_err("cancellation must stop header validation before parsing");

        assert!(matches!(error, CliError::OperationCanceled));
        fixture.close();
    }

    #[test]
    fn reports_the_zero_based_document_index_for_later_document_failures() {
        let fixture = DocumentFixture::new();
        let first = fixture.document(&["lib.rs"], &[("primary", "lib.rs")]);
        let second = fixture
            .document(&["other.rs"], &[("other", "other.rs")])
            .replacen("root: ../src", "root: ../missing", 1);
        let error = fixture
            .parse(format!("{first}---\n{second}").as_bytes())
            .expect_err("second document must fail");

        assert!(error.to_string().contains("document index 1"), "{error}");

        let malformed = format!("{first}---\ncontract_version: 2\nroot: ../src\nfiles: [lib.rs\n");
        let error = fixture
            .parse(malformed.as_bytes())
            .expect_err("raw YAML syntax failure in the second document must retain its index");
        assert!(error.to_string().contains("document index 1"), "{error}");
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

        let document = fixture
            .document(&[], &[])
            .replacen("root: ../src", "root: ../other", 1);
        let error = fixture
            .parse(document.as_bytes())
            .expect_err("root mismatch must fail");

        assert!(error.to_string().contains("not selected source"));
        fixture.close();
    }
}
