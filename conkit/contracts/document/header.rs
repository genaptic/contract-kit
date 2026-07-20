//! Typed v2 contract-header decoding and domain lowering.

use std::collections::BTreeSet;
use std::path::{Component, Path};

use conkit_signature::{CatalogPath, RustCrateKind, RustCrateRoot};
use serde::{Deserialize, de::IgnoredAny};

use super::super::layout::{DocumentOrigin, LayoutExtraction};
use super::ContractDocumentPlan;
use crate::compiler::COMPILER_EXTRACTOR_VERSION;
use crate::context::ApplicationCancellation;
use crate::error::CliError;
use crate::platform::PortablePathRules;

/// Compiler context persisted by one `rust_compiler_v1` document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContractCompilerContext {
    pub(crate) artifact_schema_version: u16,
    pub(crate) extractor_version: String,
    pub(crate) compiler_version: String,
    pub(crate) rustdoc_format_version: u32,
    pub(crate) target_triple: String,
    pub(crate) package: String,
    pub(crate) target: String,
    pub(crate) features: Vec<String>,
    pub(crate) cfg_values: Vec<String>,
}
pub(super) struct ContractLocation<'operation> {
    pub(super) contract_file: &'operation CatalogPath,
    pub(super) display_path: &'operation Path,
    pub(super) document_index: usize,
    pub(super) cancellation: &'operation ApplicationCancellation,
}

impl ContractLocation<'_> {
    pub(super) fn checkpoint(&self) -> Result<(), CliError> {
        self.cancellation.checkpoint()
    }

    pub(super) fn invalid(&self, message: impl Into<String>) -> CliError {
        CliError::ContractLayout {
            path: self.display_path.to_path_buf(),
            message: format!(
                "YAML document index {} {}",
                self.document_index,
                message.into()
            ),
        }
    }

    pub(super) fn unsupported_version(&self, message: String) -> CliError {
        CliError::UnsupportedContractVersion {
            path: self.display_path.to_path_buf(),
            document_index: self.document_index,
            message,
        }
    }

    pub(super) fn rust_source_path(
        &self,
        value: &str,
        role: &str,
    ) -> Result<CatalogPath, CliError> {
        let logical = CatalogPath::new(value.to_owned()).map_err(|source_error| {
            self.invalid(format!("has invalid {role} {value:?}: {source_error}"))
        })?;
        let rust_source = Path::new(logical.as_str())
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("rs"));
        if !rust_source {
            return Err(self.invalid(format!("{role} {logical} must have a .rs extension")));
        }
        Ok(logical)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DocumentHeader {
    #[serde(default)]
    contract_version: Option<u32>,
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    files: Option<Vec<String>>,
    #[serde(default)]
    extraction: Option<ExtractionHeader>,
    #[serde(default)]
    signatures: Option<Vec<IgnoredAny>>,
    #[serde(default, rename = "sketches")]
    _sketches: Option<IgnoredAny>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtractionHeader {
    mode: ExtractionMode,
    profile: ExtractionProfile,
    crates: Vec<CrateHeader>,
    #[serde(default)]
    compiler: Option<CompilerHeader>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ExtractionMode {
    RustSyntaxV2,
    RustCompilerV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ExtractionProfile {
    RustApiV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CrateHeader {
    id: String,
    root: String,
    kind: RustCrateKind,
}

impl CrateHeader {
    fn validate_id(&self, location: &ContractLocation<'_>) -> Result<(), CliError> {
        location.checkpoint()?;
        let mut characters = self.id.chars();
        if self.id.is_empty()
            || characters.next().is_some_and(char::is_whitespace)
            || characters.next_back().is_some_and(char::is_whitespace)
        {
            return Err(location.invalid(format!(
                "crate id must be nonempty without surrounding whitespace or control characters: {:?}",
                self.id
            )));
        }
        for (byte_index, character) in self.id.char_indices() {
            if byte_index.is_multiple_of(4_096) {
                location.checkpoint()?;
            }
            if character.is_control() {
                return Err(location.invalid(format!(
                    "crate id must be nonempty without surrounding whitespace or control characters: {:?}",
                    self.id
                )));
            }
        }
        Ok(())
    }

    fn into_root(
        self,
        location: &ContractLocation<'_>,
        source_paths: &BTreeSet<CatalogPath>,
        crate_ids: &mut BTreeSet<String>,
    ) -> Result<RustCrateRoot, CliError> {
        self.validate_id(location)?;
        let Self {
            id,
            root: crate_root,
            kind,
        } = self;
        if !crate_ids.insert(id.clone()) {
            return Err(location.invalid(format!("contains duplicate crate id {id:?}")));
        }
        let root = location.rust_source_path(&crate_root, "crate root")?;
        if !source_paths.contains(&root) {
            return Err(location.invalid(format!("crate root {root} must also appear in files")));
        }
        Ok(RustCrateRoot { id, root, kind })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompilerHeader {
    artifact_schema_version: u16,
    extractor_version: String,
    compiler_version: String,
    rustdoc_format_version: u32,
    target_triple: String,
    package: String,
    target: String,
    features: Vec<String>,
    cfg_values: Vec<String>,
    macro_expansion: bool,
    name_resolution: bool,
}

impl CompilerHeader {
    fn validate(&self, location: &ContractLocation<'_>) -> Result<(), CliError> {
        location.checkpoint()?;
        if self.artifact_schema_version != conkit_signature::RUST_COMPILER_ARTIFACT_SCHEMA_VERSION {
            return Err(location.invalid(format!(
                "compiler artifact schema {} is unsupported; expected {}",
                self.artifact_schema_version,
                conkit_signature::RUST_COMPILER_ARTIFACT_SCHEMA_VERSION,
            )));
        }
        for (field, value) in [
            ("extractor_version", self.extractor_version.as_str()),
            ("compiler_version", self.compiler_version.as_str()),
            ("target_triple", self.target_triple.as_str()),
            ("package", self.package.as_str()),
            ("target", self.target.as_str()),
        ] {
            location.checkpoint()?;
            self.validate_required_text(location, field, value)?;
        }
        if self.extractor_version != COMPILER_EXTRACTOR_VERSION {
            return Err(location.invalid(format!(
                "unsupported extractor version {:?}; expected {:?}",
                self.extractor_version, COMPILER_EXTRACTOR_VERSION,
            )));
        }
        if self.rustdoc_format_version != conkit_signature::RUSTDOC_FORMAT_VERSION {
            return Err(location.invalid(format!(
                "rustdoc JSON format {} is unsupported; expected {}",
                self.rustdoc_format_version,
                conkit_signature::RUSTDOC_FORMAT_VERSION,
            )));
        }
        if !self.macro_expansion || !self.name_resolution {
            return Err(location.invalid(
                "compiler extraction must record macro_expansion and name_resolution as true"
                    .to_owned(),
            ));
        }
        self.validate_sorted_values(location, "features", &self.features)?;
        self.validate_sorted_values(location, "cfg_values", &self.cfg_values)
    }

    fn validate_required_text(
        &self,
        location: &ContractLocation<'_>,
        field: &str,
        value: &str,
    ) -> Result<(), CliError> {
        location.checkpoint()?;
        let mut characters = value.chars();
        if value.is_empty()
            || characters.next().is_some_and(char::is_whitespace)
            || characters.next_back().is_some_and(char::is_whitespace)
        {
            return Err(location.invalid(format!(
                "compiler field {field} must be nonempty without surrounding whitespace"
            )));
        }
        for (byte_index, character) in value.char_indices() {
            if byte_index.is_multiple_of(4_096) {
                location.checkpoint()?;
            }
            if character.is_control() {
                return Err(location.invalid(format!(
                    "compiler field {field} must not contain control characters"
                )));
            }
        }
        Ok(())
    }

    fn validate_sorted_values(
        &self,
        location: &ContractLocation<'_>,
        field: &str,
        values: &[String],
    ) -> Result<(), CliError> {
        for value in values {
            location.checkpoint()?;
            self.validate_required_text(location, field, value)?;
        }
        for pair in values.windows(2) {
            location.checkpoint()?;
            if pair[0] >= pair[1] {
                return Err(location.invalid(format!(
                    "compiler field {field} must be sorted and duplicate-free"
                )));
            }
        }
        Ok(())
    }

    fn into_context(
        self,
        location: &ContractLocation<'_>,
    ) -> Result<ContractCompilerContext, CliError> {
        self.validate(location)?;
        Ok(ContractCompilerContext {
            artifact_schema_version: self.artifact_schema_version,
            extractor_version: self.extractor_version,
            compiler_version: self.compiler_version,
            rustdoc_format_version: self.rustdoc_format_version,
            target_triple: self.target_triple,
            package: self.package,
            target: self.target,
            features: self.features,
            cfg_values: self.cfg_values,
        })
    }
}

impl ExtractionHeader {
    fn into_layout(
        self,
        location: &ContractLocation<'_>,
        source_paths: &BTreeSet<CatalogPath>,
    ) -> Result<LayoutExtraction, CliError> {
        let Self {
            mode,
            profile: ExtractionProfile::RustApiV1,
            crates,
            compiler,
        } = self;
        let compiler = match (mode, compiler) {
            (ExtractionMode::RustSyntaxV2, None) => None,
            (ExtractionMode::RustSyntaxV2, Some(_)) => {
                return Err(
                    location.invalid("syntax extraction must not contain compiler metadata")
                );
            }
            (ExtractionMode::RustCompilerV1, Some(compiler)) => {
                Some(compiler.into_context(location)?)
            }
            (ExtractionMode::RustCompilerV1, None) => {
                return Err(location.invalid("compiler extraction is missing field `compiler`"));
            }
        };
        if crates.is_empty() {
            return Err(location.invalid("extraction must declare at least one crate root"));
        }

        let mut crate_ids = BTreeSet::new();
        let mut crate_roots = Vec::with_capacity(crates.len());
        for crate_header in crates {
            location.checkpoint()?;
            crate_roots.push(crate_header.into_root(location, source_paths, &mut crate_ids)?);
        }

        let declared_at =
            DocumentOrigin::new(location.contract_file.clone(), location.document_index);
        Ok(match compiler {
            Some(context) => LayoutExtraction::Compiler {
                crates: crate_roots,
                context,
                declared_at,
            },
            None => LayoutExtraction::Syntax {
                crates: crate_roots,
                declared_at,
            },
        })
    }
}

impl DocumentHeader {
    pub(super) fn into_plan(
        mut self,
        location: &ContractLocation<'_>,
    ) -> Result<ContractDocumentPlan, CliError> {
        location.checkpoint()?;
        match self.contract_version {
            Some(2) => {}
            Some(version @ 0..=1) => {
                return Err(location.unsupported_version(format!(
                        "contract_version {version} is legacy; recreate this contract with conkit generate"
                    )));
            }
            Some(version) => {
                return Err(location.unsupported_version(format!(
                    "contract_version {version} is unsupported; expected contract_version 2"
                )));
            }
            None => {
                return Err(location.unsupported_version(
                    "missing contract_version; recreate this v0.0.1 contract with conkit generate"
                        .to_owned(),
                ));
            }
        }

        let root = self.take_declared_root(location)?;
        let files = self
            .files
            .ok_or_else(|| location.invalid("is missing field `files`"))?;
        let signatures = self
            .signatures
            .ok_or_else(|| location.invalid("is missing field `signatures`"))?;
        self._sketches
            .ok_or_else(|| location.invalid("is missing field `sketches`"))?;
        let signature_count = signatures.len();
        if signature_count > 0 && self.extraction.is_none() {
            return Err(location.invalid(format!(
                "is missing field `extraction` for {signature_count} signature entries"
            )));
        }

        let mut source_paths = Vec::with_capacity(files.len());
        let mut unique_sources = BTreeSet::new();
        for listed in files {
            location.checkpoint()?;
            let logical = location.rust_source_path(&listed, "listed source path")?;
            if !unique_sources.insert(logical.clone()) {
                return Err(
                    location.invalid(format!("contains duplicate listed source path {logical}"))
                );
            }
            source_paths.push(logical);
        }

        let extraction = self
            .extraction
            .map(|extraction| extraction.into_layout(location, &unique_sources))
            .transpose()?;

        Ok(ContractDocumentPlan {
            declared_root: root,
            source_paths,
            extraction,
        })
    }

    fn take_declared_root(&mut self, location: &ContractLocation<'_>) -> Result<String, CliError> {
        location.checkpoint()?;
        let root = self
            .root
            .take()
            .ok_or_else(|| location.invalid("is missing field `root`"))?;
        let declared_path = Path::new(&root);
        if root.is_empty() || root.contains('\\') || declared_path.is_absolute() {
            return Err(location.invalid(format!(
                "contract root must be a nonempty relative path: {root:?}"
            )));
        }

        for component in declared_path.components() {
            location.checkpoint()?;
            match component {
                Component::Normal(value) => PortablePathRules::validate_component(value)?,
                Component::ParentDir => {}
                Component::CurDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(location.invalid(format!(
                        "contract root contains an invalid component: {root:?}"
                    )));
                }
            }
        }

        Ok(root)
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::layout::LayoutExtraction;
    use super::super::tests::DocumentFixture;

    impl DocumentFixture {
        fn compiler_document(&self) -> String {
            self.document(&["lib.rs"], &[("sample", "lib.rs")])
                .replace("mode: rust_syntax_v2", "mode: rust_compiler_v1")
                .replace(
                    "signatures: []",
                    &format!(
                        "  compiler:\n    artifact_schema_version: {}\n    extractor_version: {}\n    compiler_version: rustc-nightly\n    rustdoc_format_version: {}\n    target_triple: x86_64-unknown-linux-gnu\n    package: sample\n    target: sample\n    features: [default, serde]\n    cfg_values: [target_arch=\"x86_64\", unix]\n    macro_expansion: true\n    name_resolution: true\nsignatures: []",
                        conkit_signature::RUST_COMPILER_ARTIFACT_SCHEMA_VERSION,
                        crate::compiler::COMPILER_EXTRACTOR_VERSION,
                        conkit_signature::RUSTDOC_FORMAT_VERSION,
                    ),
                )
        }
    }

    #[test]
    fn compiler_header_requires_and_retains_the_versioned_extraction_mode() {
        let fixture = DocumentFixture::new();
        let document = fixture.compiler_document();

        let parsed = fixture
            .parse(document.as_bytes())
            .expect("complete compiler extraction header");
        let (_, _, plans) = parsed.into_parts();
        let extraction = plans[0]
            .extraction
            .as_ref()
            .expect("compiler extraction plan");

        let LayoutExtraction::Compiler {
            crates,
            context: compiler,
            declared_at: _,
        } = extraction
        else {
            panic!("expected compiler extraction");
        };
        assert_eq!(crates.len(), 1);
        assert_eq!(crates[0].id, "sample");
        assert_eq!(
            compiler.extractor_version,
            crate::compiler::COMPILER_EXTRACTOR_VERSION
        );
        assert_eq!(
            compiler.rustdoc_format_version,
            conkit_signature::RUSTDOC_FORMAT_VERSION
        );
        assert_eq!(compiler.package, "sample");
        assert_eq!(compiler.target, "sample");
        assert_eq!(compiler.features, ["default", "serde"]);

        let missing = document.replace("  compiler:\n", "  absent_compiler:\n");
        let error = fixture
            .parse(missing.as_bytes())
            .expect_err("compiler metadata is required and unknown keys fail closed");
        assert!(error.to_string().contains("unknown field"), "{error}");
        fixture.close();
    }

    #[test]
    fn compiler_header_preflight_rejects_unsupported_versions_and_control_text() {
        let fixture = DocumentFixture::new();
        let valid = fixture.compiler_document();
        let invalid_cases = [
            (
                valid.replace(
                    crate::compiler::COMPILER_EXTRACTOR_VERSION,
                    "unsupported-extractor",
                ),
                "unsupported extractor version",
            ),
            (
                valid.replace(
                    &format!(
                        "rustdoc_format_version: {}",
                        conkit_signature::RUSTDOC_FORMAT_VERSION,
                    ),
                    "rustdoc_format_version: 1",
                ),
                "rustdoc JSON format",
            ),
            (
                valid.replace(
                    "compiler_version: rustc-nightly",
                    "compiler_version: \"rustc\\nnightly\"",
                ),
                "control characters",
            ),
            (
                valid.replace("cfg_values: [", "cfg_values: [' padded ', "),
                "without surrounding whitespace",
            ),
            (
                valid.replace("id: sample", "id: \"sample\\nid\""),
                "control characters",
            ),
        ];

        for (document, expected) in invalid_cases {
            let error = fixture
                .parse(document.as_bytes())
                .expect_err("invalid persisted compiler metadata must fail in static parsing");
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?} in {error}"
            );
        }

        fixture.close();
    }

    #[test]
    fn rejects_missing_legacy_and_future_contract_versions_with_upgrade_guidance() {
        let fixture = DocumentFixture::new();

        for (document, expected) in [
            (
                "root: ../src\nfiles: []\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [] }\n",
                "missing contract_version",
            ),
            (
                "contract_version: 1\nroot: ../src\nfiles: []\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [] }\n",
                "recreate",
            ),
            (
                "contract_version: 3\nroot: ../src\nfiles: []\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [] }\n",
                "contract_version 3",
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
    fn rejects_unknown_or_missing_v2_header_fields() {
        let fixture = DocumentFixture::new();
        let valid = fixture.document(&["lib.rs"], &[("primary", "lib.rs")]);

        for (document, expected) in [
            (
                valid.replacen("signatures: []\n", "unknown: true\nsignatures: []\n", 1),
                "unknown field `unknown`",
            ),
            (
                valid.replacen("root: ../src\n", "", 1),
                "missing field `root`",
            ),
            (
                valid.replacen("files:\n  - lib.rs\n", "", 1),
                "missing field `files`",
            ),
            (
                valid.replacen("signatures: []\n", "", 1),
                "missing field `signatures`",
            ),
            (
                valid.replacen("sketches: []\n", "", 1),
                "missing field `sketches`",
            ),
        ] {
            let error = fixture
                .parse(document.as_bytes())
                .expect_err("invalid v2 header must be rejected");
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?} in {error}"
            );
        }

        fixture.close();
    }

    #[test]
    fn requires_every_typed_crate_root_to_appear_in_the_exact_files_allowlist() {
        let fixture = DocumentFixture::new();
        let error = fixture
            .parse(
                fixture
                    .document(&["lib.rs"], &[("missing", "bin/tool.rs")])
                    .as_bytes(),
            )
            .expect_err("unlisted crate root must fail");

        assert!(
            error.to_string().contains("crate root bin/tool.rs"),
            "{error}"
        );
        assert!(error.to_string().contains("files"), "{error}");
        fixture.close();
    }

    #[test]
    fn requires_extraction_for_nonempty_signatures_and_crates_for_present_extraction() {
        let fixture = DocumentFixture::new();
        let without_extraction = fixture
            .document(&["lib.rs"], &[])
            .replace("signatures: []", "signatures: [signature]");
        let missing = fixture
            .parse(without_extraction.as_bytes())
            .expect_err("signature-bearing documents require extraction");
        assert!(missing.to_string().contains("missing field `extraction`"));

        let empty_crates = fixture.document(&["lib.rs"], &[]).replace(
            "signatures: []",
            "extraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [] }\nsignatures: []",
        );
        let empty = fixture
            .parse(empty_crates.as_bytes())
            .expect_err("present extraction requires crate roots");
        assert!(empty.to_string().contains("at least one crate root"));
        fixture.close();
    }

    #[test]
    fn leaves_signature_and_sketch_payloads_opaque_to_the_cli_header_reader() {
        let fixture = DocumentFixture::new();
        let document = fixture
            .document(&["lib.rs"], &[("primary", "lib.rs")])
            .replace("signatures: []", "signatures: [domain owned]")
            .replace("sketches: []", "sketches: tagged scalar");

        fixture
            .parse(document.as_bytes())
            .expect("domain-owned bodies are ignored by the CLI header reader");
        fixture.close();
    }

    #[test]
    fn rejects_invalid_root_components_and_backslashes() {
        let fixture = DocumentFixture::new();

        for declared in ["", ".", "../src\\nested", "../invalid."] {
            let bytes = fixture.document(&[], &[]).replacen(
                "root: ../src",
                &format!("root: {declared:?}"),
                1,
            );
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
            let bytes = fixture.document(&[listed], &[]);
            assert!(
                fixture.parse(bytes.as_bytes()).is_err(),
                "accepted {listed:?}"
            );
        }

        fixture.close();
    }
}
