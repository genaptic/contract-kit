//! Aggregate binding between combined documents and source-tree paths.

use std::collections::BTreeSet;
use std::path::{Component, Path};

use conkit_signature::{CatalogPath, FileCatalog, RustCrateKind, RustCrateRoot};

use super::document::{
    ContractCompilerContext, ContractDocument, ContractDocumentPath, ContractYamlLimits,
    ContractYamlUsage,
};
use crate::catalog::{CatalogReadBudget, ContractsStore, PathRole, ResolvedPath, SourceTree};
use crate::context::ApplicationCancellation;
use crate::error::CliError;
use crate::platform::PortablePathRules;

/// Validated root-level combined documents and their source allowlist.
#[derive(Debug)]
pub(crate) struct ContractLayout {
    documents: FileCatalog,
    document_count: usize,
    source_paths: Vec<CatalogPath>,
    extraction: Result<Option<LayoutExtraction>, ExtractionConflict>,
}

/// Stateful command-side v2 header validator reused across every input catalog
/// that participates in one archive or diff operation.
pub(crate) struct ContractFormatValidator<'cancellation> {
    yaml: ContractYamlUsage<'cancellation>,
}

impl<'cancellation> ContractFormatValidator<'cancellation> {
    /// Starts one operation-wide validation budget.
    pub(crate) fn new(cancellation: &'cancellation ApplicationCancellation) -> Self {
        Self::with_yaml_limits(cancellation, ContractYamlLimits::default())
    }

    fn with_yaml_limits(
        cancellation: &'cancellation ApplicationCancellation,
        limits: ContractYamlLimits,
    ) -> Self {
        Self {
            yaml: ContractYamlUsage::with_limits(cancellation, limits),
        }
    }

    /// Validates every direct-root YAML entry and retains cumulative accounting
    /// for a subsequent catalog in the same operation.
    pub(crate) fn validate(&mut self, catalog: &FileCatalog) -> Result<(), CliError> {
        for (catalog_path, bytes) in catalog.iter() {
            self.yaml.cancellation().checkpoint()?;
            let Ok(document_path) = ContractDocumentPath::try_from(catalog_path.clone()) else {
                continue;
            };
            ContractDocument::validate_bytes(document_path, bytes, &mut self.yaml)?;
        }
        Ok(())
    }
}

/// One extraction capability shared by every signature-bearing document in a
/// loaded layout.
#[derive(Clone, Debug)]
pub(crate) enum LayoutExtraction {
    Syntax {
        crates: Vec<RustCrateRoot>,
        declared_at: DocumentOrigin,
    },
    Compiler {
        crates: Vec<RustCrateRoot>,
        context: ContractCompilerContext,
        declared_at: DocumentOrigin,
    },
}

/// Physical contract-document location that first declared an extraction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DocumentOrigin {
    contract_file: CatalogPath,
    document_index: usize,
}

#[derive(Debug)]
struct ExtractionConflict {
    declared_at: DocumentOrigin,
    message: String,
}

impl DocumentOrigin {
    pub(super) fn new(contract_file: CatalogPath, document_index: usize) -> Self {
        Self {
            contract_file,
            document_index,
        }
    }

    fn invalid(&self, message: impl Into<String>) -> CliError {
        CliError::ContractLayout {
            path: Path::new(self.contract_file.as_str()).to_path_buf(),
            message: format!(
                "YAML document index {} {}",
                self.document_index,
                message.into(),
            ),
        }
    }
}

impl LayoutExtraction {
    /// Returns the persisted crate roots used to validate compiler target
    /// selection or construct syntax extraction.
    pub(crate) fn crates(&self) -> &[RustCrateRoot] {
        match self {
            Self::Syntax { crates, .. } | Self::Compiler { crates, .. } => crates,
        }
    }

    /// Reports whether the persisted layout requires compiler extraction.
    pub(crate) fn is_compiler(&self) -> bool {
        matches!(self, Self::Compiler { .. })
    }

    fn declared_at(&self) -> &DocumentOrigin {
        match self {
            Self::Syntax { declared_at, .. } | Self::Compiler { declared_at, .. } => declared_at,
        }
    }

    fn crates_mut(&mut self) -> &mut Vec<RustCrateRoot> {
        match self {
            Self::Syntax { crates, .. } | Self::Compiler { crates, .. } => crates,
        }
    }

    fn merge(
        &mut self,
        incoming: Self,
        seen_crates: &mut BTreeSet<RustCrateRoot>,
        cancellation: &ApplicationCancellation,
    ) -> Result<Option<ExtractionConflict>, CliError> {
        cancellation.checkpoint()?;
        match (&*self, &incoming) {
            (Self::Syntax { .. }, Self::Syntax { .. }) => {}
            (
                Self::Compiler { context, .. },
                Self::Compiler {
                    context: incoming, ..
                },
            ) if context == incoming => {}
            (Self::Compiler { .. }, Self::Compiler { declared_at, .. }) => {
                return Ok(Some(ExtractionConflict::new(
                    declared_at,
                    "declares compiler context that differs from another participating document",
                )));
            }
            (_, incoming) => {
                return Ok(Some(ExtractionConflict::new(
                    incoming.declared_at(),
                    "mixes syntax and compiler extraction in one operation",
                )));
            }
        }

        let incoming_crates = match incoming {
            Self::Syntax { crates, .. } | Self::Compiler { crates, .. } => crates,
        };
        for crate_root in incoming_crates {
            cancellation.checkpoint()?;
            if seen_crates.insert(crate_root.clone()) {
                self.crates_mut().push(crate_root);
            }
        }
        Ok(None)
    }

    /// Rejects runtime Cargo context that differs from a persisted compiler
    /// document before the artifact enters an asynchronous domain request.
    pub(crate) fn validate_compiler_artifact(
        &self,
        artifact: &conkit_signature::RustCompilerArtifact,
        contracts: &ContractsStore,
        cancellation: &ApplicationCancellation,
    ) -> Result<(), CliError> {
        cancellation.checkpoint()?;
        let Self::Compiler { context, .. } = self else {
            return Err(CliError::ContractLayout {
                path: contracts.path().to_path_buf(),
                message: "compiler artifact cannot be applied to rust_syntax_v2 contracts"
                    .to_owned(),
            });
        };
        let [compiler_crate] = artifact.crates.as_slice() else {
            return Err(CliError::ContractLayout {
                path: contracts.path().to_path_buf(),
                message: format!(
                    "compiler artifact contains {} crates; persisted context requires exactly one",
                    artifact.crates.len(),
                ),
            });
        };
        cancellation.checkpoint()?;
        let matches = context.artifact_schema_version == artifact.schema_version
            && context.extractor_version.as_str() == artifact.extractor_version.as_str()
            && context.compiler_version.as_str() == artifact.compiler_version.as_str()
            && context.rustdoc_format_version == artifact.rustdoc_format_version
            && context.target_triple.as_str() == artifact.target_triple.as_str()
            && context.package.as_str() == compiler_crate.package.as_str()
            && context.target.as_str() == compiler_crate.target.as_str()
            && context.features.as_slice() == artifact.features.as_slice()
            && context.cfg_values.as_slice() == artifact.cfg_values.as_slice();
        if !matches {
            let actual = ContractCompilerContext {
                artifact_schema_version: artifact.schema_version,
                extractor_version: artifact.extractor_version.clone(),
                compiler_version: artifact.compiler_version.clone(),
                rustdoc_format_version: artifact.rustdoc_format_version,
                target_triple: artifact.target_triple.clone(),
                package: compiler_crate.package.clone(),
                target: compiler_crate.target.clone(),
                features: artifact.features.clone(),
                cfg_values: artifact.cfg_values.clone(),
            };
            cancellation.checkpoint()?;
            return Err(CliError::ContractLayout {
                path: contracts.path().to_path_buf(),
                message: format!(
                    "selected Cargo/compiler context differs from persisted rust_compiler_v1 metadata (expected {context:?}, actual {actual:?})"
                ),
            });
        }
        Ok(())
    }
}

impl ExtractionConflict {
    fn new(declared_at: &DocumentOrigin, message: impl Into<String>) -> Self {
        Self {
            declared_at: declared_at.clone(),
            message: message.into(),
        }
    }

    fn error(&self) -> CliError {
        self.declared_at.invalid(self.message.clone())
    }
}

impl ContractLayout {
    /// Parses all direct root-level YAML documents and binds them to `source`.
    ///
    /// # Errors
    ///
    /// Returns an error if the source root cannot be resolved, a combined
    /// document is malformed or targets another source root.
    pub(crate) fn load(
        contracts: &ContractsStore,
        source: &SourceTree,
        catalog: &FileCatalog,
        cancellation: &ApplicationCancellation,
    ) -> Result<Self, CliError> {
        cancellation.checkpoint()?;
        let canonical_source = fs_err::canonicalize(source.path()).map_err(|source_error| {
            CliError::ContractLayout {
                path: source.path().to_path_buf(),
                message: format!("failed to canonicalize selected source root: {source_error}"),
            }
        })?;
        if !canonical_source.is_dir() {
            return Err(CliError::ContractLayout {
                path: source.path().to_path_buf(),
                message: "selected source root is not a directory".to_owned(),
            });
        }

        let mut documents = FileCatalog::new();
        let mut document_count = 0;
        let mut participating_sources = BTreeSet::new();
        let mut extraction = Ok(None);
        let mut seen_crates = BTreeSet::new();
        let mut yaml = ContractYamlUsage::new(cancellation);

        for (catalog_path, bytes) in catalog.iter() {
            cancellation.checkpoint()?;
            let Ok(document_path) = ContractDocumentPath::try_from(catalog_path.clone()) else {
                continue;
            };
            let document = ContractDocument::parse(
                document_path,
                bytes.to_vec(),
                contracts.path(),
                source.path(),
                &canonical_source,
                &mut yaml,
            )?;
            let (document_path, bytes, plans) = document.into_parts();
            document_count += plans.len();

            for plan in plans {
                cancellation.checkpoint()?;
                for source_path in plan.source_paths {
                    cancellation.checkpoint()?;
                    participating_sources.insert(source_path);
                }
                if let Some(incoming) = plan.extraction {
                    match &mut extraction {
                        Ok(None) => {
                            for crate_root in incoming.crates() {
                                cancellation.checkpoint()?;
                                seen_crates.insert(crate_root.clone());
                            }
                            extraction = Ok(Some(incoming));
                        }
                        Ok(Some(selected)) => {
                            if let Some(conflict) =
                                selected.merge(incoming, &mut seen_crates, cancellation)?
                            {
                                extraction = Err(conflict);
                            }
                        }
                        Err(_) => {}
                    }
                }
            }

            documents.insert(document_path.into_catalog_path(), bytes)?;
        }

        let mut source_paths = Vec::with_capacity(participating_sources.len());
        for source_path in participating_sources {
            cancellation.checkpoint()?;
            source_paths.push(source_path);
        }
        if let Ok(Some(LayoutExtraction::Compiler {
            crates,
            declared_at,
            ..
        })) = &extraction
            && crates.len() != 1
        {
            extraction = Err(ExtractionConflict::new(
                declared_at,
                format!(
                    "compiler extraction requires exactly one aggregate crate root, found {}",
                    crates.len(),
                ),
            ));
        }

        Ok(Self {
            documents,
            document_count,
            source_paths,
            extraction,
        })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.document_count == 0
    }

    /// Consumes the layout into its participating combined documents.
    pub(crate) fn into_documents(self) -> FileCatalog {
        self.documents
    }

    /// Reads exactly the source files claimed by the combined documents.
    ///
    /// # Errors
    ///
    /// Returns an error when an allowlisted file cannot be securely opened,
    /// validated, or represented in the source catalog.
    pub(crate) fn read_sources(
        &self,
        source: &SourceTree,
        catalog_reads: &mut CatalogReadBudget,
    ) -> Result<FileCatalog, CliError> {
        source.read_selected_with_budget(&self.source_paths, catalog_reads)
    }

    /// Reads the exact existing allowlist union or, for a fresh layout, the
    /// Rust catalog from which a syntax or compiler target will be selected.
    pub(crate) fn read_signature_sources(
        &self,
        source: &SourceTree,
        catalog_reads: &mut CatalogReadBudget,
    ) -> Result<FileCatalog, CliError> {
        if self.is_empty() {
            source.read_rust_sources_with_budget(catalog_reads)
        } else {
            self.read_sources(source, catalog_reads)
        }
    }

    /// Borrows the canonical extraction declared by the loaded layout.
    /// Compiler extraction additionally requires exactly one distinct
    /// aggregate crate root before source extraction or Cargo execution.
    pub(crate) fn extraction(
        &self,
        cancellation: &ApplicationCancellation,
    ) -> Result<Option<&LayoutExtraction>, CliError> {
        cancellation.checkpoint()?;
        match &self.extraction {
            Ok(extraction) => Ok(extraction.as_ref()),
            Err(failure) => Err(failure.error()),
        }
    }

    /// Requires at least one root-level combined contract document.
    ///
    /// # Errors
    ///
    /// Returns an error when the layout contains no combined documents.
    pub(crate) fn require_documents(&self, contracts: &ContractsStore) -> Result<(), CliError> {
        if self.is_empty() {
            Err(CliError::ContractLayout {
                path: contracts.path().to_path_buf(),
                message: "no root-level .yml or .yaml contract documents were found".to_owned(),
            })
        } else {
            Ok(())
        }
    }

    /// Builds the source catalog and signature generation target.
    ///
    /// # Errors
    ///
    /// Returns an error when source inputs cannot be securely read or a fresh
    /// document root cannot be represented as a portable relative path.
    pub(crate) fn into_signature_generation(
        self,
        contracts: &ContractsStore,
        source: &SourceTree,
        source_files: FileCatalog,
        crate_roots: Vec<RustCrateRoot>,
        cancellation: &ApplicationCancellation,
    ) -> Result<(FileCatalog, conkit_signature::GenerateTarget), CliError> {
        if !self.is_empty() {
            return Ok((
                source_files,
                conkit_signature::GenerateTarget::Existing(self.documents),
            ));
        }

        self.new_signature_generation(contracts, source, source_files, crate_roots, cancellation)
    }

    /// Builds the exact source and document catalogs for sketch generation.
    ///
    /// # Errors
    ///
    /// Returns an error when no combined document exists or an allowlisted
    /// source file cannot be securely read.
    pub(crate) fn into_sketch_generation(
        self,
        contracts: &ContractsStore,
        source: &SourceTree,
        catalog_reads: &mut CatalogReadBudget,
    ) -> Result<(FileCatalog, FileCatalog), CliError> {
        self.require_documents(contracts)?;
        let source_files = self.read_sources(source, catalog_reads)?;
        Ok((source_files, self.documents))
    }

    fn new_signature_generation(
        self,
        contracts: &ContractsStore,
        source: &SourceTree,
        source_files: FileCatalog,
        requested_crates: Vec<RustCrateRoot>,
        cancellation: &ApplicationCancellation,
    ) -> Result<(FileCatalog, conkit_signature::GenerateTarget), CliError> {
        cancellation.checkpoint()?;
        let crates = self.validate_or_infer_crates(
            contracts,
            &source_files,
            requested_crates,
            cancellation,
        )?;
        let contract_root = ResolvedPath::new(PathRole::Contracts, contracts.path().to_path_buf())?;
        let source_root = ResolvedPath::new(PathRole::Source, source.path().to_path_buf())?;
        let relative = contract_root.relative_path_to(&source_root)?;
        let mut root_parts = Vec::new();
        for component in relative.components() {
            match component {
                Component::ParentDir => root_parts.push("..".to_owned()),
                Component::Normal(value) => {
                    PortablePathRules::validate_component(value)?;
                    root_parts.push(
                        value
                            .to_str()
                            .ok_or(CliError::NonUtf8PathComponent)?
                            .to_owned(),
                    );
                }
                Component::CurDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(CliError::ContractLayout {
                        path: contracts.path().to_path_buf(),
                        message: "cannot represent the selected source as a relative contract root"
                            .to_owned(),
                    });
                }
            }
        }

        let mut files = Vec::with_capacity(source_files.len());
        for (path, _) in source_files.iter() {
            cancellation.checkpoint()?;
            files.push(path.clone());
        }
        let document = conkit_signature::GenerateDocument {
            contract_file: CatalogPath::new("main.yml")?,
            root: root_parts.join("/"),
            files,
            crates,
        };

        Ok((
            source_files,
            conkit_signature::GenerateTarget::New(document),
        ))
    }

    fn validate_or_infer_crates(
        &self,
        contracts: &ContractsStore,
        source_files: &FileCatalog,
        requested: Vec<RustCrateRoot>,
        cancellation: &ApplicationCancellation,
    ) -> Result<Vec<RustCrateRoot>, CliError> {
        if !requested.is_empty() {
            for crate_root in &requested {
                cancellation.checkpoint()?;
                let is_rust = crate_root
                    .root
                    .as_str()
                    .rsplit_once('.')
                    .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("rs"));
                if !is_rust {
                    return Err(CliError::ContractLayout {
                        path: contracts.path().to_path_buf(),
                        message: format!(
                            "crate root {} must identify a Rust source file",
                            crate_root.root
                        ),
                    });
                }
                if source_files.get(&crate_root.root).is_none() {
                    return Err(CliError::ContractLayout {
                        path: contracts.path().to_path_buf(),
                        message: format!(
                            "crate root {} does not exist in the selected Rust source catalog",
                            crate_root.root
                        ),
                    });
                }
            }
            return Ok(requested);
        }

        let mut conventional = Vec::new();
        for (path, _) in source_files.iter() {
            cancellation.checkpoint()?;
            match path.as_str() {
                "lib.rs" => conventional.push(RustCrateRoot {
                    id: "library".to_owned(),
                    root: path.clone(),
                    kind: RustCrateKind::Library,
                }),
                "main.rs" => conventional.push(RustCrateRoot {
                    id: "application".to_owned(),
                    root: path.clone(),
                    kind: RustCrateKind::Binary,
                }),
                _ => {}
            }
        }
        let [crate_root] = conventional.as_slice() else {
            return Err(self.explicit_crate_root_required(contracts));
        };

        Ok(vec![crate_root.clone()])
    }

    fn explicit_crate_root_required(&self, contracts: &ContractsStore) -> CliError {
        CliError::ContractLayout {
            path: contracts.path().to_path_buf(),
            message: "fresh signature generation requires explicit --crate-root because the Rust source layout has zero or multiple conventional crate roots"
                .to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use assert_fs::prelude::*;
    use conkit_signature::{
        CatalogPath, FileCatalog, RustCompilerArtifact, RustCompilerCrate, RustCrateKind,
        RustCrateRoot,
    };

    use super::super::document::yaml::ContractYamlCounters;
    use super::{
        ContractCompilerContext, ContractFormatValidator, ContractLayout, ContractYamlLimits,
        DocumentOrigin, LayoutExtraction,
    };
    use crate::catalog::{CatalogReadLimits, ContractsStore, SourceTree};
    use crate::context::ApplicationCancellation;
    use crate::error::CliError;

    struct LayoutDocument;

    impl LayoutDocument {
        fn origin(contract_file: &str, document_index: usize) -> DocumentOrigin {
            DocumentOrigin::new(
                CatalogPath::new(contract_file).expect("contract document path"),
                document_index,
            )
        }

        fn with_source(crate_id: &str, source: &str) -> String {
            format!(
                "contract_version: 2\nroot: ../src\nfiles: [{source}]\nextraction:\n  mode: rust_syntax_v2\n  profile: rust_api_v1\n  crates:\n    - id: {crate_id}\n      root: {source}\n      kind: library\nsignatures: []\nsketches: []\n"
            )
        }

        fn compiler_with_source(crate_id: &str, source: &str) -> String {
            Self::with_source(crate_id, source)
                .replace("mode: rust_syntax_v2", "mode: rust_compiler_v1")
                .replace(
                    "signatures: []",
                    &format!(
                        "  compiler:\n    artifact_schema_version: {}\n    extractor_version: {}\n    compiler_version: rustc-nightly\n    rustdoc_format_version: {}\n    target_triple: x86_64-unknown-linux-gnu\n    package: sample\n    target: sample\n    features: []\n    cfg_values: [unix]\n    macro_expansion: true\n    name_resolution: true\nsignatures: []",
                        conkit_signature::RUST_COMPILER_ARTIFACT_SCHEMA_VERSION,
                        crate::compiler::COMPILER_EXTRACTOR_VERSION,
                        conkit_signature::RUSTDOC_FORMAT_VERSION,
                    ),
                )
        }
    }

    #[test]
    fn format_validator_shares_yaml_usage_across_diff_catalogs_and_cancellation() {
        let mut current = FileCatalog::new();
        current
            .insert(
                CatalogPath::new("current.yml").expect("current document path"),
                LayoutDocument::with_source("current", "lib.rs").into_bytes(),
            )
            .expect("current document");
        let mut previous = FileCatalog::new();
        previous
            .insert(
                CatalogPath::new("previous.yml").expect("previous document path"),
                LayoutDocument::with_source("previous", "lib.rs").into_bytes(),
            )
            .expect("previous document");

        let cancellation = ApplicationCancellation::new();
        let mut validator = ContractFormatValidator::with_yaml_limits(
            &cancellation,
            ContractYamlLimits {
                ceiling: ContractYamlCounters {
                    documents: 1,
                    ..ContractYamlLimits::default().ceiling
                },
            },
        );
        validator
            .validate(&current)
            .expect("the first diff side is within the shared budget");
        let error = validator
            .validate(&previous)
            .expect_err("the second diff side must share the first side's usage");
        assert!(error.to_string().contains("document count"), "{error}");
        assert!(error.to_string().contains("previous.yml"), "{error}");

        let canceled = ApplicationCancellation::new();
        canceled.request();
        let error = ContractFormatValidator::new(&canceled)
            .validate(&current)
            .expect_err("cancellation must stop catalog header validation");
        assert!(matches!(error, CliError::OperationCanceled));
    }

    #[test]
    fn format_validator_rejects_a_lexically_invalid_root_without_source_binding() {
        let absolute_root = if cfg!(windows) {
            r"C:\absolute"
        } else {
            "/absolute"
        };
        let mut catalog = FileCatalog::new();
        catalog
            .insert(
                CatalogPath::new("invalid-root.yml").expect("contract document path"),
                LayoutDocument::with_source("sample", "lib.rs")
                    .replace("root: ../src", &format!("root: {absolute_root}"))
                    .into_bytes(),
            )
            .expect("contract document");

        let cancellation = ApplicationCancellation::new();
        let error = ContractFormatValidator::new(&cancellation)
            .validate(&catalog)
            .expect_err("format-only validation must enforce lexical root invariants");

        assert!(error.to_string().contains("invalid-root.yml"), "{error}");
        assert!(
            error
                .to_string()
                .contains("contract root must be a nonempty relative path"),
            "{error}"
        );
    }

    #[test]
    fn persisted_compiler_context_rejects_a_different_cargo_target_before_domain_work() {
        let contracts = ContractsStore::new("contracts".into());
        let root = RustCrateRoot {
            id: "sample".to_owned(),
            root: CatalogPath::new("lib.rs").expect("crate root"),
            kind: RustCrateKind::Library,
        };
        let extraction = LayoutExtraction::Compiler {
            crates: vec![root.clone()],
            context: ContractCompilerContext {
                artifact_schema_version: conkit_signature::RUST_COMPILER_ARTIFACT_SCHEMA_VERSION,
                extractor_version: crate::compiler::COMPILER_EXTRACTOR_VERSION.to_owned(),
                compiler_version: "rustc-nightly".to_owned(),
                rustdoc_format_version: conkit_signature::RUSTDOC_FORMAT_VERSION,
                target_triple: "x86_64-unknown-linux-gnu".to_owned(),
                package: "sample".to_owned(),
                target: "sample".to_owned(),
                features: vec!["default".to_owned()],
                cfg_values: vec!["unix".to_owned()],
            },
            declared_at: LayoutDocument::origin("main.yml", 0),
        };
        let mut artifact = RustCompilerArtifact {
            schema_version: conkit_signature::RUST_COMPILER_ARTIFACT_SCHEMA_VERSION,
            extractor_version: crate::compiler::COMPILER_EXTRACTOR_VERSION.to_owned(),
            compiler_version: "rustc-nightly".to_owned(),
            rustdoc_format_version: conkit_signature::RUSTDOC_FORMAT_VERSION,
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            features: vec!["default".to_owned()],
            cfg_values: vec!["unix".to_owned()],
            crates: vec![RustCompilerCrate {
                id: root.id,
                package: "sample".to_owned(),
                target: "sample".to_owned(),
                root: root.root,
                root_item_id: 0,
                kind: RustCrateKind::Library,
            }],
            rustdoc_json: Vec::new(),
            source_paths: Vec::new(),
        };

        let cancellation = crate::context::ApplicationCancellation::new();
        extraction
            .validate_compiler_artifact(&artifact, &contracts, &cancellation)
            .expect("identical persisted and runtime context");
        artifact.crates[0].target = "different".to_owned();
        let error = extraction
            .validate_compiler_artifact(&artifact, &contracts, &cancellation)
            .expect_err("a different Cargo target must fail before an await");
        assert!(error.to_string().contains("persisted rust_compiler_v1"));
    }

    #[test]
    fn persisted_compiler_layout_rejects_multiple_aggregate_roots_without_a_process() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let source = temp.child("src");
        source.create_dir_all().expect("source root");
        source
            .child("lib.rs")
            .write_str("pub fn library() {}\n")
            .expect("library root");
        source
            .child("bin.rs")
            .write_str("pub fn binary() {}\n")
            .expect("binary root");
        let contracts = temp.child("contracts");
        contracts.create_dir_all().expect("contracts root");
        let source = SourceTree::open(source.path().to_path_buf()).expect("source tree");
        let contracts = ContractsStore::new(contracts.path().to_path_buf());
        let mut catalog = FileCatalog::new();
        catalog
            .insert(
                CatalogPath::new("library.yml").expect("library document path"),
                LayoutDocument::compiler_with_source("library", "lib.rs").into_bytes(),
            )
            .expect("library document");
        catalog
            .insert(
                CatalogPath::new("binary.yml").expect("binary document path"),
                LayoutDocument::compiler_with_source("binary", "bin.rs").into_bytes(),
            )
            .expect("binary document");

        let layout = ContractLayout::load(
            &contracts,
            &source,
            &catalog,
            &crate::context::ApplicationCancellation::new(),
        )
        .expect("each compiler document is independently valid");
        let error = layout
            .extraction(&crate::context::ApplicationCancellation::new())
            .expect_err("one compiler operation cannot aggregate two roots");

        assert!(
            error
                .to_string()
                .contains("requires exactly one aggregate crate root, found 2"),
            "{error}"
        );
        drop(layout);
        drop(contracts);
        drop(source);
        temp.close().expect("close temporary root");
    }

    #[test]
    fn aggregate_extraction_errors_wait_until_the_complete_layout_is_requested() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let source = temp.child("src");
        source.create_dir_all().expect("source root");
        source.child("lib.rs").touch().expect("crate root");
        let contracts = temp.child("contracts");
        contracts.create_dir_all().expect("contracts root");
        let source = SourceTree::open(source.path().to_path_buf()).expect("source tree");
        let contracts = ContractsStore::new(contracts.path().to_path_buf());
        let mut catalog = FileCatalog::new();
        for (path, document) in [
            ("a.yml", LayoutDocument::with_source("sample", "lib.rs")),
            (
                "b.yml",
                LayoutDocument::compiler_with_source("sample", "lib.rs"),
            ),
        ] {
            catalog
                .insert(
                    CatalogPath::new(path).expect("document path"),
                    document.into_bytes(),
                )
                .expect("contract document");
        }

        let mut malformed = catalog.clone();
        malformed
            .insert(
                CatalogPath::new("z.yml").expect("malformed path"),
                b"[".to_vec(),
            )
            .expect("malformed document");
        let error = ContractLayout::load(
            &contracts,
            &source,
            &malformed,
            &ApplicationCancellation::new(),
        )
        .expect_err("later malformed YAML retains precedence");
        assert!(error.to_string().contains("z.yml"), "{error}");

        let layout = ContractLayout::load(
            &contracts,
            &source,
            &catalog,
            &ApplicationCancellation::new(),
        )
        .expect("aggregate extraction is deferred");
        let error = layout
            .extraction(&ApplicationCancellation::new())
            .expect_err("requesting the mixed extraction must fail");
        assert!(error.to_string().contains("b.yml"), "{error}");
        assert!(
            error.to_string().contains("mixes syntax and compiler"),
            "{error}"
        );

        let mut contexts = FileCatalog::new();
        for (path, target) in [("a.yml", "sample"), ("b.yml", "different")] {
            contexts
                .insert(
                    CatalogPath::new(path).expect("context document path"),
                    LayoutDocument::compiler_with_source("sample", "lib.rs")
                        .replace("target: sample", &format!("target: {target}"))
                        .into_bytes(),
                )
                .expect("context document");
        }
        let layout = ContractLayout::load(
            &contracts,
            &source,
            &contexts,
            &ApplicationCancellation::new(),
        )
        .expect("context mismatch is deferred");
        let error = layout
            .extraction(&ApplicationCancellation::new())
            .expect_err("different compiler contexts must fail");
        assert!(error.to_string().contains("b.yml"), "{error}");
        assert!(
            error.to_string().contains("context that differs"),
            "{error}"
        );
        drop(layout);
        drop(contracts);
        drop(source);
        temp.close().expect("close temporary root");
    }

    #[test]
    fn loads_root_documents_and_ignores_non_documents() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let source = temp.child("src");
        source.create_dir_all().expect("source root");
        source
            .child("lib.rs")
            .write_str("pub fn answer() -> u8 { 42 }\n")
            .expect("source file");
        let contracts = temp.child("contracts");
        contracts.create_dir_all().expect("contracts root");
        let source = SourceTree::open(source.path().to_path_buf()).expect("source tree");
        let contracts = ContractsStore::new(contracts.path().to_path_buf());
        let mut catalog = FileCatalog::new();
        catalog
            .insert(
                CatalogPath::new("main.YmL").expect("document path"),
                LayoutDocument::with_source("primary", "lib.rs").into_bytes(),
            )
            .expect("root document");
        catalog
            .insert(
                CatalogPath::new("nested/ignored.yaml").expect("nested path"),
                b"not: a contract\n".to_vec(),
            )
            .expect("nested document");

        let layout = ContractLayout::load(
            &contracts,
            &source,
            &catalog,
            &crate::context::ApplicationCancellation::new(),
        )
        .expect("valid layout");
        let mut catalog_reads =
            CatalogReadLimits::default().begin(&crate::context::ApplicationCancellation::new());
        let selected = layout
            .read_signature_sources(&source, &mut catalog_reads)
            .expect("existing source selection");
        let (sources, target) = layout
            .into_signature_generation(
                &contracts,
                &source,
                selected,
                Vec::new(),
                &crate::context::ApplicationCancellation::new(),
            )
            .expect("existing generation input");

        assert_eq!(sources.len(), 1);
        let conkit_signature::GenerateTarget::Existing(documents) = target else {
            panic!("expected existing-document target");
        };
        assert_eq!(documents.len(), 1);
        drop(contracts);
        drop(source);
        temp.close().expect("close temporary root");
    }

    #[test]
    fn aggregates_document_count_sources_and_extraction_without_retaining_plans() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let source = temp.child("src");
        source.create_dir_all().expect("source root");
        source.child("lib.rs").touch().expect("library source");
        source.child("bin.rs").touch().expect("binary source");
        let contracts = temp.child("contracts");
        contracts.create_dir_all().expect("contracts root");
        let source = SourceTree::open(source.path().to_path_buf()).expect("source tree");
        let contracts = ContractsStore::new(contracts.path().to_path_buf());
        let mut catalog = FileCatalog::new();
        let documents = format!(
            "{}---\n{}---\n{}",
            LayoutDocument::with_source("library", "lib.rs"),
            LayoutDocument::with_source("library", "lib.rs"),
            LayoutDocument::with_source("binary", "bin.rs"),
        );
        catalog
            .insert(
                CatalogPath::new("targets.yml").expect("document path"),
                documents.into_bytes(),
            )
            .expect("multi-document file");

        let layout = ContractLayout::load(
            &contracts,
            &source,
            &catalog,
            &crate::context::ApplicationCancellation::new(),
        )
        .expect("valid multi-document layout");
        assert_eq!(layout.document_count, 3);
        let Some(LayoutExtraction::Syntax {
            crates,
            declared_at,
        }) = layout
            .extraction(&crate::context::ApplicationCancellation::new())
            .expect("canonical extraction")
        else {
            panic!("expected syntax extraction");
        };
        assert_eq!(declared_at.contract_file.as_str(), "targets.yml");
        assert_eq!(declared_at.document_index, 0);
        assert_eq!(
            crates
                .iter()
                .map(|crate_root| crate_root.id.as_str())
                .collect::<Vec<_>>(),
            ["library", "binary"],
        );
        let mut catalog_reads =
            CatalogReadLimits::default().begin(&crate::context::ApplicationCancellation::new());
        assert_eq!(
            layout
                .read_sources(&source, &mut catalog_reads)
                .expect("source union")
                .len(),
            2,
        );
        drop(layout);
        drop(contracts);
        drop(source);
        temp.close().expect("close temporary root");
    }

    #[test]
    fn requires_at_least_one_combined_document() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let source = temp.child("src");
        source.create_dir_all().expect("source root");
        let contracts = temp.child("contracts");
        contracts.create_dir_all().expect("contracts root");
        let source = SourceTree::open(source.path().to_path_buf()).expect("source tree");
        let contracts = ContractsStore::new(contracts.path().to_path_buf());
        let layout = ContractLayout::load(
            &contracts,
            &source,
            &FileCatalog::new(),
            &crate::context::ApplicationCancellation::new(),
        )
        .expect("empty layout");

        let error = layout
            .require_documents(&contracts)
            .expect_err("documents are required");

        assert!(error.to_string().contains("no root-level"));
        drop(layout);
        drop(contracts);
        drop(source);
        temp.close().expect("close temporary root");
    }

    #[test]
    fn fresh_generation_targets_main_with_a_relative_root_and_rust_files() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let source = temp.child("src");
        source.create_dir_all().expect("source root");
        source.child("lib.rs").touch().expect("Rust source");
        source.child("notes.txt").touch().expect("ignored source");
        let contracts = temp.child("contracts");
        let source = SourceTree::open(source.path().to_path_buf()).expect("source tree");
        let contracts = ContractsStore::new(contracts.path().to_path_buf());
        let layout = ContractLayout::load(
            &contracts,
            &source,
            &FileCatalog::new(),
            &crate::context::ApplicationCancellation::new(),
        )
        .expect("empty generation layout");
        let mut catalog_reads =
            CatalogReadLimits::default().begin(&crate::context::ApplicationCancellation::new());
        let selected = layout
            .read_signature_sources(&source, &mut catalog_reads)
            .expect("fresh Rust source selection");

        let (sources, target) = layout
            .into_signature_generation(
                &contracts,
                &source,
                selected,
                Vec::new(),
                &crate::context::ApplicationCancellation::new(),
            )
            .expect("generation input");

        assert_eq!(sources.len(), 1);
        let conkit_signature::GenerateTarget::New(document) = target else {
            panic!("expected new-document target");
        };
        assert_eq!(document.contract_file.as_str(), "main.yml");
        assert_eq!(document.root, "../src");
        assert_eq!(document.files.len(), 1);
        assert_eq!(document.files[0].as_str(), "lib.rs");
        assert_eq!(document.crates.len(), 1);
        assert_eq!(document.crates[0].id, "library");
        assert_eq!(document.crates[0].root.as_str(), "lib.rs");
        assert_eq!(document.crates[0].kind, RustCrateKind::Library);
        drop(contracts);
        drop(source);
        temp.close().expect("close temporary root");
    }
}
