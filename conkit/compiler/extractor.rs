//! Compiler extraction helpers, narrow rustdoc envelope, and compiler identity.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use conkit_signature::{FileCatalog, RustCrateKind};
use serde::Deserialize;

use super::CompilerExtractor;
use super::error::CompilerError;
use super::limits::CompilerUsage;
use super::probe::RustdocProbe;
use super::process::{CargoEnvironment, CargoOutput, CargoProcess, CompilerOperation};
use super::project::{CargoProject, CompilerWorkspace};
use crate::catalog::{CatalogReadBudget, SourceTree};
use crate::contracts::{CargoFeatures, CompilerRequest};

impl CompilerExtractor {
    pub(super) fn manifest_path(&self, path: &Path) -> Result<PathBuf, CompilerError> {
        let canonical =
            fs_err::canonicalize(path).map_err(|source| CompilerError::ManifestUnavailable {
                path: path.to_path_buf(),
                source,
            })?;
        if !canonical.is_file() {
            return Err(CompilerError::ManifestNotFile { path: canonical });
        }
        Ok(canonical)
    }

    pub(super) fn metadata_arguments(
        &self,
        request: &CompilerRequest<'_>,
        manifest: &Path,
        project: Option<&CargoProject>,
    ) -> Vec<OsString> {
        let mut arguments = vec![
            OsString::from("metadata"),
            OsString::from("--locked"),
            OsString::from("--format-version"),
            OsString::from("1"),
            OsString::from("--manifest-path"),
            manifest.as_os_str().to_owned(),
        ];
        if let Some(project) = project {
            self.append_feature_arguments(request.features(), project, &mut arguments);
        }
        if let Some(target) = request.target_triple() {
            arguments.push(OsString::from("--filter-platform"));
            arguments.push(OsString::from(target));
        }
        arguments
    }

    pub(super) fn package_id_arguments(&self, manifest: &Path, package: &str) -> Vec<OsString> {
        vec![
            OsString::from("pkgid"),
            OsString::from("--locked"),
            OsString::from("--manifest-path"),
            manifest.as_os_str().to_owned(),
            OsString::from("--package"),
            OsString::from(package),
        ]
    }

    pub(in crate::compiler) fn append_feature_arguments(
        &self,
        features: CargoFeatures<'_>,
        project: &CargoProject,
        arguments: &mut Vec<OsString>,
    ) {
        let (names, include_default) = match features {
            CargoFeatures::Default => (&[][..], true),
            CargoFeatures::Selected {
                names,
                include_default,
            } => (names, include_default),
            CargoFeatures::All => {
                arguments.push(OsString::from("--all-features"));
                return;
            }
        };
        let mut qualified = names
            .iter()
            .map(|feature| {
                if feature.contains('/') {
                    feature.clone()
                } else {
                    format!("{}/{feature}", project.package_name())
                }
            })
            .collect::<Vec<_>>();
        if include_default && project.has_default_feature() {
            qualified.push(format!("{}/default", project.package_name()));
        }
        qualified.sort();
        qualified.dedup();
        if !qualified.is_empty() {
            arguments.push(OsString::from("--features"));
            arguments.push(OsString::from(qualified.join(",")));
        }
        if !include_default {
            arguments.push(OsString::from("--no-default-features"));
        }
    }

    pub(super) fn run(
        &self,
        usage: &Arc<CompilerUsage>,
        operation: CompilerOperation,
        current_directory: &Path,
        arguments: Vec<OsString>,
        temporary_tree: Option<&Path>,
    ) -> Result<CargoOutput, CompilerError> {
        CargoProcess::spawn(
            &self.cargo,
            operation,
            current_directory,
            arguments,
            temporary_tree,
            usage,
            CargoEnvironment::Pinned,
        )?
        .execute()
    }

    pub(super) fn probe_rustdoc_configuration(
        &self,
        usage: &Arc<CompilerUsage>,
        request: &CompilerRequest<'_>,
        manifest: &Path,
        current_directory: &Path,
        workspace: &CompilerWorkspace,
        project: &CargoProject,
    ) -> Result<Vec<String>, CompilerError> {
        let probe = RustdocProbe::parent(workspace, project)?;
        let completion = CargoProcess::spawn(
            &self.cargo,
            CompilerOperation::RustdocConfigurationProbe,
            current_directory,
            project.rustdoc_arguments(request, manifest, workspace.target_directory()),
            Some(workspace.target_directory()),
            usage,
            CargoEnvironment::RustdocProbe(&probe),
        )?
        .complete()?;
        if completion.status.success() {
            return Err(CompilerError::RustdocProbeUnexpectedSuccess);
        }
        usage.checkpoint(CompilerOperation::RustdocConfigurationProbe)?;
        match probe.read_record(usage) {
            Ok(cfg_values) => Ok(cfg_values),
            Err(probe) => Err(CompilerError::RustdocProbeFailed {
                cargo: Box::new(completion.output.failure(
                    CompilerOperation::RustdocConfigurationProbe,
                    completion.status,
                )),
                probe: Box::new(probe),
            }),
        }
    }

    pub(super) fn verify_source_snapshot(
        &self,
        usage: &CompilerUsage,
        source_root: &Path,
        source_files: &FileCatalog,
        catalog_reads: &mut CatalogReadBudget,
    ) -> Result<(), CompilerError> {
        usage.checkpoint(CompilerOperation::Rustdoc)?;
        let changed = SourceTree::open(source_root.to_path_buf())
            .and_then(|source| {
                source.first_changed_snapshot_with_budget(source_files, catalog_reads)
            })
            .map_err(|source| match source {
                crate::error::CliError::OperationCanceled => {
                    CompilerError::CompilerExtractionCancelled
                }
                crate::error::CliError::CatalogReadLimit(source) => {
                    CompilerError::SourceSnapshotLimit(source)
                }
                source => CompilerError::SourceSnapshotUnavailable {
                    message: source.to_string(),
                },
            })?;
        if let Some(path) = changed {
            return Err(CompilerError::SourceChangedDuringExtraction {
                path: path.to_string(),
            });
        }
        usage.checkpoint(CompilerOperation::Rustdoc)?;
        Ok(())
    }

    pub(super) fn decode_rustdoc(
        &self,
        bytes: &[u8],
        kind: RustCrateKind,
    ) -> Result<RustdocSourceDocument, CompilerError> {
        let document: RustdocSourceDocument =
            serde_json::from_slice(bytes).map_err(|source| CompilerError::InvalidRustdocJson {
                message: source.to_string(),
            })?;
        let expected = matches!(kind, RustCrateKind::Binary);
        if document.includes_private != expected {
            return Err(CompilerError::RustdocPrivateItemsMismatch {
                expected,
                actual: document.includes_private,
            });
        }
        Ok(document)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct RustdocId(pub(super) u32);

#[derive(Debug, Deserialize)]
pub(super) struct RustdocTarget {
    pub(super) triple: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct RustdocSourceSpan {
    pub(super) filename: PathBuf,
    pub(super) begin: (usize, usize),
    pub(super) end: (usize, usize),
}

impl RustdocSourceSpan {
    fn deserialize_optional<'de, D>(deserializer: D) -> Result<Option<Self>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Option::<Self>::deserialize(deserializer)
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct RustdocSourceItem {
    pub(super) id: RustdocId,
    pub(super) crate_id: u32,
    #[serde(deserialize_with = "RustdocSourceSpan::deserialize_optional")]
    pub(super) span: Option<RustdocSourceSpan>,
}

#[derive(Debug)]
pub(super) struct RustdocSourceIndex {
    pub(super) items: Vec<RustdocSourceItem>,
}

impl<'de> Deserialize<'de> for RustdocSourceIndex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct RustdocSourceIndexVisitor;

        impl<'de> serde::de::Visitor<'de> for RustdocSourceIndexVisitor {
            type Value = RustdocSourceIndex;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a rustdoc item map")
            }

            fn visit_map<A>(self, mut entries: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut items = Vec::new();
                if let Some(capacity) = entries.size_hint() {
                    items
                        .try_reserve(capacity)
                        .map_err(serde::de::Error::custom)?;
                }
                while let Some((key, item)) =
                    entries.next_entry::<RustdocId, RustdocSourceItem>()?
                {
                    if key != item.id {
                        return Err(serde::de::Error::custom(format_args!(
                            "rustdoc item ID mismatch between map key {} and item payload {}",
                            key.0, item.id.0,
                        )));
                    }
                    items.push(item);
                }
                items.sort_unstable_by_key(|item| item.id);
                if let Some(duplicate) = items
                    .windows(2)
                    .find_map(|pair| (pair[0].id == pair[1].id).then_some(pair[0].id))
                {
                    return Err(serde::de::Error::custom(format_args!(
                        "duplicate rustdoc item ID {}",
                        duplicate.0,
                    )));
                }
                Ok(RustdocSourceIndex { items })
            }
        }

        deserializer.deserialize_map(RustdocSourceIndexVisitor)
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct RustdocSourceDocument {
    pub(super) root: RustdocId,
    pub(super) index: RustdocSourceIndex,
    pub(super) target: RustdocTarget,
    pub(super) format_version: u32,
    pub(super) includes_private: bool,
}

pub(super) struct CompilerIdentity {
    release: String,
    commit_hash: String,
    commit_date: String,
    host: String,
}

impl CompilerIdentity {
    pub(super) fn parse(output: &str) -> Result<Self, CompilerError> {
        if output.is_empty()
            || output
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\r'))
        {
            return Err(CompilerError::InvalidCompilerIdentity {
                message: "output is empty or contains unsupported control characters".to_owned(),
            });
        }
        let lines = output
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .collect::<Vec<_>>();
        let banner = lines.first().copied().unwrap_or_default();
        if banner.trim() != banner
            || !banner.starts_with("rustc ")
            || banner.chars().any(char::is_control)
        {
            return Err(CompilerError::InvalidCompilerIdentity {
                message: "first line is not a canonical rustc version banner".to_owned(),
            });
        }
        let mut fields = BTreeMap::new();
        for line in lines.iter().skip(1).filter(|line| !line.is_empty()) {
            let (name, value) =
                line.split_once(": ")
                    .ok_or_else(|| CompilerError::InvalidCompilerIdentity {
                        message: format!("malformed verbose-version field {line:?}"),
                    })?;
            if name.is_empty()
                || value.is_empty()
                || name.trim() != name
                || value.trim() != value
                || name.chars().any(char::is_control)
                || value.chars().any(char::is_control)
                || fields.insert(name, value).is_some()
            {
                return Err(CompilerError::InvalidCompilerIdentity {
                    message: format!("invalid or duplicate verbose-version field {name:?}"),
                });
            }
        }
        let release = Self::required_field(&fields, "release")?;
        let commit_hash = Self::required_field(&fields, "commit-hash")?;
        let commit_date = Self::required_field(&fields, "commit-date")?;
        let host = Self::required_field(&fields, "host")?;
        if !release.ends_with("-nightly")
            || commit_hash.len() != 40
            || !commit_hash.bytes().all(|byte| byte.is_ascii_hexdigit())
            || commit_date.len() != 10
            || !commit_date.bytes().enumerate().all(|(index, byte)| {
                if matches!(index, 4 | 7) {
                    byte == b'-'
                } else {
                    byte.is_ascii_digit()
                }
            })
            || host.is_empty()
            || !host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            || !banner.contains(release)
        {
            return Err(CompilerError::InvalidCompilerIdentity {
                message: "nightly release, commit, date, or host field is invalid".to_owned(),
            });
        }

        Ok(Self {
            release: release.to_owned(),
            commit_hash: commit_hash.to_ascii_lowercase(),
            commit_date: commit_date.to_owned(),
            host: host.to_owned(),
        })
    }

    fn required_field<'a>(
        fields: &BTreeMap<&'a str, &'a str>,
        name: &'static str,
    ) -> Result<&'a str, CompilerError> {
        fields
            .get(name)
            .copied()
            .ok_or_else(|| CompilerError::InvalidCompilerIdentity {
                message: format!("missing verbose-version field {name:?}"),
            })
    }

    pub(super) fn contract_value(&self) -> String {
        format!(
            "rustc {} ({} {}); host={}",
            self.release, self.commit_hash, self.commit_date, self.host
        )
    }
}

pub(super) struct CompilerConfiguration;

impl CompilerConfiguration {
    pub(super) fn merge(
        usage: &CompilerUsage,
        rustc_output: &str,
        rustdoc_values: Vec<String>,
    ) -> Result<Vec<String>, CompilerError> {
        usage.checkpoint(CompilerOperation::Configuration)?;
        let mut ordered = BTreeSet::new();
        ordered.insert(Self::validated_value("doc")?);
        for value in rustdoc_values {
            usage.checkpoint(CompilerOperation::Configuration)?;
            ordered.insert(Self::validated_value(&value)?);
        }
        for line in rustc_output.lines() {
            usage.checkpoint(CompilerOperation::Configuration)?;
            if line.is_empty() {
                continue;
            }
            ordered.insert(Self::validated_value(line)?);
        }
        let mut values = Vec::with_capacity(ordered.len());
        for value in ordered {
            usage.checkpoint(CompilerOperation::Configuration)?;
            values.push(value);
        }
        Ok(values)
    }

    pub(in crate::compiler) fn validated_value(value: &str) -> Result<String, CompilerError> {
        if value.is_empty()
            || value.trim() != value
            || value.len() > 4_096
            || value.chars().any(char::is_control)
        {
            return Err(CompilerError::InvalidCompilerConfiguration {
                value: value.to_owned(),
            });
        }
        Ok(value.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use crate::compiler::tests::*;

    #[test]
    fn json_target_drives_the_followup_cfg_query_when_no_target_was_requested() {
        let request = CompilerFixture::request_with_target(None);
        let project = CargoProject::select(CompilerFixture::metadata(), None, request.target())
            .expect("selected library target");
        let arguments = project.rustc_arguments(
            &request,
            Path::new("/workspace/Cargo.toml"),
            Path::new("/temporary/target"),
            Some("aarch64-unknown-linux-gnu"),
            ["--print", "cfg"],
        );
        let arguments = arguments
            .iter()
            .map(|argument| argument.to_string_lossy())
            .collect::<Vec<_>>();
        let target = arguments
            .windows(2)
            .find(|pair| pair[0] == "--target")
            .expect("authoritative target argument");

        assert_eq!(target[1], "aarch64-unknown-linux-gnu");
    }

    #[test]
    fn compiler_identity_is_control_free_deterministic_and_host_qualified() {
        let hash = "0123456789abcdef0123456789abcdef01234567";
        let identity = CompilerIdentity::parse(&format!(
            "rustc 1.90.0-nightly ({hash} 2026-06-30)\n\
             binary: rustc\n\
             commit-hash: {hash}\n\
             commit-date: 2026-06-30\n\
             host: x86_64-unknown-linux-gnu\n\
             release: 1.90.0-nightly\n\
             LLVM version: 20.1.7\n"
        ))
        .expect("valid verbose nightly identity");

        assert_eq!(
            identity.contract_value(),
            format!("rustc 1.90.0-nightly ({hash} 2026-06-30); host=x86_64-unknown-linux-gnu")
        );
        assert!(!identity.contract_value().chars().any(char::is_control));
    }

    #[test]
    fn malformed_or_control_bearing_compiler_identity_fails_closed() {
        for output in [
            "",
            "rustc 1.90.0-nightly\nrelease: 1.90.0-nightly\n",
            "rustc 1.90.0-nightly\nrelease: 1.90.0-nightly\0\n",
            "rustc stable (hash date)\ncommit-hash: 0123456789abcdef0123456789abcdef01234567\ncommit-date: 2026-06-30\nhost: x86_64-unknown-linux-gnu\nrelease: stable\n",
        ] {
            assert!(matches!(
                CompilerIdentity::parse(output),
                Err(CompilerError::InvalidCompilerIdentity { .. })
            ));
        }
    }

    #[test]
    fn compiler_configuration_merges_rustdoc_specific_cfg_and_cfg_doc() {
        let usage = CompilerUsage::new(CompilerLimits::default(), CompilerFixture::cancellation());
        let values = CompilerConfiguration::merge(
            &usage,
            "target_arch=\"x86_64\"\nfeature=\"serde\"\n",
            vec!["docsrs".to_owned(), "feature=\"serde\"".to_owned()],
        )
        .expect("one validated compiler/rustdoc cfg context");

        assert_eq!(
            values,
            [
                "doc",
                "docsrs",
                "feature=\"serde\"",
                "target_arch=\"x86_64\""
            ]
        );
    }

    #[test]
    fn rustdoc_projection_accepts_unknown_semantics_and_sorts_item_ids() {
        let bytes = br#"{
            "root": 0,
            "index": {
                "9": {
                    "id": 9,
                    "crate_id": 0,
                    "span": {
                        "filename": "lib.rs",
                        "begin": [1, 1],
                        "end": [1, 1],
                        "future_span_fact": true
                    },
                    "visibility": "public",
                    "inner": { "function": {} }
                },
                "0": { "id": 0, "crate_id": 0, "span": null }
            },
            "target": {
                "triple": "x86_64-unknown-linux-gnu",
                "target_features": ["sse2"]
            },
            "format_version": 60,
            "includes_private": false,
            "paths": {},
            "future_document_fact": true
        }"#;

        let document = CompilerExtractor::new(CompilerFixture::cancellation())
            .decode_rustdoc(bytes, RustCrateKind::Library)
            .expect("partial projection accepts unrelated rustdoc semantics");

        assert_eq!(
            document
                .index
                .items
                .iter()
                .map(|item| item.id.0)
                .collect::<Vec<_>>(),
            [0, 9]
        );
        assert_eq!(document.root.0, 0);
        assert_eq!(document.target.triple, "x86_64-unknown-linux-gnu");
    }

    #[test]
    fn rustdoc_projection_requires_every_source_mapping_field() {
        for pointer in [
            "/root",
            "/index",
            "/target",
            "/format_version",
            "/includes_private",
            "/target/triple",
            "/index/9/id",
            "/index/9/crate_id",
            "/index/9/span",
            "/index/9/span/filename",
            "/index/9/span/begin",
            "/index/9/span/end",
        ] {
            let value = CompilerFixture::projection_without(pointer);
            assert!(matches!(
                CompilerFixture::decode_projection(&value, RustCrateKind::Library),
                Err(CompilerError::InvalidRustdocJson { .. })
            ));
        }
        CompilerFixture::decode_projection(
            &CompilerFixture::rustdoc_projection_value(),
            RustCrateKind::Library,
        )
        .expect("an explicit null span remains valid");
    }

    #[test]
    fn rustdoc_projection_rejects_key_identity_and_duplicate_ids() {
        let mismatched = br#"{
            "root": 0,
            "index": { "1": { "id": 2, "crate_id": 0, "span": null } },
            "target": { "triple": "x86_64-unknown-linux-gnu" },
            "format_version": 60,
            "includes_private": false
        }"#;
        let duplicate = br#"{
            "root": 0,
            "index": {
                "1": { "id": 1, "crate_id": 0, "span": null },
                "1": { "id": 1, "crate_id": 0, "span": null }
            },
            "target": { "triple": "x86_64-unknown-linux-gnu" },
            "format_version": 60,
            "includes_private": false
        }"#;
        let extractor = CompilerExtractor::new(CompilerFixture::cancellation());

        for bytes in [mismatched.as_slice(), duplicate.as_slice()] {
            assert!(matches!(
                extractor.decode_rustdoc(bytes, RustCrateKind::Library),
                Err(CompilerError::InvalidRustdocJson { .. })
            ));
        }
    }

    #[test]
    fn rustdoc_private_item_flag_matches_cargo_target_kind() {
        for (target, includes_private, accepted) in [
            (RustCrateKind::Library, false, true),
            (RustCrateKind::Library, true, false),
            (RustCrateKind::Binary, true, true),
            (RustCrateKind::Binary, false, false),
        ] {
            let mut value = CompilerFixture::rustdoc_projection_value();
            value["includes_private"] = serde_json::json!(includes_private);
            let result = CompilerFixture::decode_projection(&value, target);
            if accepted {
                result.expect("Cargo-consistent private-item flag");
            } else {
                assert!(matches!(
                    result,
                    Err(CompilerError::RustdocPrivateItemsMismatch { .. })
                ));
            }
        }
    }

    #[test]
    fn changed_source_snapshot_is_rejected_before_artifact_handoff() {
        let root = assert_fs::TempDir::new().expect("temporary source root");
        std::fs::write(root.path().join("lib.rs"), "pub fn before() {}\n").expect("initial source");
        let mut snapshot = FileCatalog::new();
        snapshot
            .insert(
                CatalogPath::new("lib.rs").expect("logical source path"),
                b"pub fn before() {}\n".to_vec(),
            )
            .expect("source snapshot");
        std::fs::write(root.path().join("lib.rs"), "pub fn after() {}\n").expect("changed source");

        let usage = CompilerUsage::new(CompilerLimits::default(), CompilerFixture::cancellation());
        let cancellation = ApplicationCancellation::new();
        let mut catalog_reads = CatalogReadLimits::default().begin(&cancellation);
        let error = CompilerExtractor::new(CompilerFixture::cancellation())
            .verify_source_snapshot(&usage, root.path(), &snapshot, &mut catalog_reads)
            .expect_err("post-Cargo source changes must fail closed");

        assert!(matches!(
            error,
            CompilerError::SourceChangedDuringExtraction { path } if path == "lib.rs"
        ));
        root.close().expect("close source fixture");
    }

    #[test]
    fn source_snapshot_recheck_uses_the_operation_budget_and_cancellation() {
        let root = assert_fs::TempDir::new().expect("temporary source root");
        std::fs::write(root.path().join("lib.rs"), "pub fn stable() {}\n")
            .expect("physical source");
        let mut snapshot = FileCatalog::new();
        snapshot
            .insert(
                CatalogPath::new("lib.rs").expect("logical source path"),
                b"pub fn stable() {}\n".to_vec(),
            )
            .expect("source snapshot");
        let usage = CompilerUsage::new(CompilerLimits::default(), CompilerFixture::cancellation());
        let application = ApplicationCancellation::new();
        let mut limited = CatalogReadLimits::new(0, 1024, 1024).begin(&application);

        let error = CompilerExtractor::new(CompilerFixture::cancellation())
            .verify_source_snapshot(&usage, root.path(), &snapshot, &mut limited)
            .expect_err("snapshot recheck must share the exhausted entry budget");
        let CompilerError::SourceSnapshotLimit(source) = error else {
            panic!("expected source snapshot catalog limit evidence")
        };
        assert!(
            source
                .to_string()
                .contains("catalog entry count limit exceeded"),
            "unexpected source snapshot limit: {source}",
        );

        let canceled = ApplicationCancellation::new();
        canceled.request();
        let mut canceled_budget = CatalogReadLimits::default().begin(&canceled);
        let error = CompilerExtractor::new(CompilerFixture::cancellation())
            .verify_source_snapshot(&usage, root.path(), &snapshot, &mut canceled_budget)
            .expect_err("application cancellation must stop snapshot revalidation");
        assert!(matches!(error, CompilerError::CompilerExtractionCancelled));
        root.close().expect("close source fixture");
    }
}
