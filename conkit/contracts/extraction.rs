//! Typed CLI extraction selection and requested-versus-persisted reconciliation.

use std::path::Path;

use conkit_signature::{FileCatalog, RustCrateRoot, RustExtractionInput};

use super::LayoutExtraction;
use crate::catalog::{CatalogReadBudget, ContractsStore, SourceTree};
use crate::context::CommandContext;
use crate::error::CliError;

/// One extraction capability selected after clap parsing and validation.
#[derive(Clone, Copy, Debug)]
pub(crate) enum RequestedExtraction<'args> {
    Syntax,
    Compiler(CompilerRequest<'args>),
}

/// Validated Cargo/compiler arguments borrowed from the clap-owned input.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CompilerRequest<'args> {
    manifest: &'args Path,
    package: Option<&'args str>,
    target: CargoTarget<'args>,
    features: CargoFeatures<'args>,
    target_triple: Option<&'args str>,
}

impl<'args> CompilerRequest<'args> {
    pub(crate) fn new(
        manifest: &'args Path,
        package: Option<&'args str>,
        target: CargoTarget<'args>,
        features: CargoFeatures<'args>,
        target_triple: Option<&'args str>,
    ) -> Self {
        Self {
            manifest,
            package,
            target,
            features,
            target_triple,
        }
    }

    pub(crate) fn manifest(&self) -> &'args Path {
        self.manifest
    }

    pub(crate) fn package(&self) -> Option<&'args str> {
        self.package
    }

    pub(crate) fn target(&self) -> CargoTarget<'args> {
        self.target
    }

    pub(crate) fn features(&self) -> CargoFeatures<'args> {
        self.features
    }

    pub(crate) fn target_triple(&self) -> Option<&'args str> {
        self.target_triple
    }
}

/// One mutually exclusive Cargo target selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CargoTarget<'args> {
    Automatic,
    Library,
    Binary(&'args str),
}

/// One mutually exclusive Cargo feature selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CargoFeatures<'args> {
    Default,
    Selected {
        names: &'args [String],
        include_default: bool,
    },
    All,
}

/// Operation policy applied while reconciling requested and persisted modes.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ExtractionUse<'layout> {
    Check {
        persisted: Option<&'layout LayoutExtraction>,
    },
    Generation {
        fresh: bool,
        persisted: Option<&'layout LayoutExtraction>,
        explicit_crates: &'layout [RustCrateRoot],
    },
}

/// Sole owner of CLI extraction reconciliation.
pub(crate) struct SignatureExtractionCoordinator<'args, 'operation> {
    requested: RequestedExtraction<'args>,
    contracts: &'operation ContractsStore,
}

/// Opaque validated work remaining after requested and persisted modes agree.
pub(crate) enum ExtractionDecision<'args, 'layout> {
    Syntax(&'layout [RustCrateRoot]),
    Establish(CompilerRequest<'args>, Option<&'layout [RustCrateRoot]>),
    Validate(CompilerRequest<'args>, &'layout LayoutExtraction),
}

enum CompilerAbsence<'layout> {
    Check,
    Existing,
    Establish(Option<&'layout [RustCrateRoot]>),
}

impl<'args, 'operation> SignatureExtractionCoordinator<'args, 'operation> {
    const CANNOT_OVERRIDE_SYNTAX: &'static str =
        "compiler extraction cannot override existing rust_syntax_v2 contract metadata";
    const REQUIRES_COMPILER: &'static str = "existing contracts require compiler extraction; pass --signature-extractor compiler and --manifest-path FILE";

    pub(crate) fn new(
        requested: RequestedExtraction<'args>,
        contracts: &'operation ContractsStore,
    ) -> Self {
        Self {
            requested,
            contracts,
        }
    }

    /// Rejects existing-layout crate overrides at their historical pre-read boundary.
    pub(crate) fn validate_generation_roots(
        &self,
        fresh: bool,
        explicit_crates: &[RustCrateRoot],
    ) -> Result<(), CliError> {
        if !fresh && !explicit_crates.is_empty() {
            return Err(self.invalid(
                "--crate-root cannot override extraction in existing contract documents",
            ));
        }
        Ok(())
    }

    /// Reconciles the one requested capability with persisted layout facts.
    pub(crate) fn reconcile<'layout>(
        &self,
        usage: ExtractionUse<'layout>,
    ) -> Result<ExtractionDecision<'args, 'layout>, CliError> {
        let (persisted, syntax_crates, compiler_absence) = match usage {
            ExtractionUse::Check { persisted } => (persisted, &[][..], CompilerAbsence::Check),
            ExtractionUse::Generation {
                fresh,
                persisted,
                explicit_crates,
            } => (
                persisted,
                explicit_crates,
                if fresh {
                    CompilerAbsence::Establish(
                        (!explicit_crates.is_empty()).then_some(explicit_crates),
                    )
                } else {
                    CompilerAbsence::Existing
                },
            ),
        };
        match (self.requested, persisted) {
            (RequestedExtraction::Syntax, None | Some(LayoutExtraction::Syntax { .. })) => Ok(
                ExtractionDecision::Syntax(syntax_crates),
            ),
            (
                RequestedExtraction::Compiler(request),
                Some(extraction @ LayoutExtraction::Compiler { .. }),
            ) => Ok(ExtractionDecision::Validate(request, extraction)),
            (RequestedExtraction::Syntax, Some(LayoutExtraction::Compiler { .. })) => {
                Err(self.invalid(Self::REQUIRES_COMPILER))
            }
            (RequestedExtraction::Compiler(_), Some(LayoutExtraction::Syntax { .. })) => {
                Err(self.invalid(Self::CANNOT_OVERRIDE_SYNTAX))
            }
            (RequestedExtraction::Compiler(request), None) => match compiler_absence {
                CompilerAbsence::Establish(expected) => {
                    Ok(ExtractionDecision::Establish(request, expected))
                }
                CompilerAbsence::Check => Err(self.invalid(
                    "compiler extraction requires a signature-bearing rust_compiler_v1 contract document",
                )),
                CompilerAbsence::Existing => Err(self.invalid(
                    "compiler extraction cannot update an existing layout without rust_compiler_v1 metadata",
                )),
            }
        }
    }

    fn invalid(&self, message: &str) -> CliError {
        CliError::ContractLayout {
            path: self.contracts.path().to_path_buf(),
            message: message.to_owned(),
        }
    }
}

impl<'args, 'layout> ExtractionDecision<'args, 'layout> {
    /// Performs the sole compiler acquisition path before domain submission.
    pub(crate) fn acquire(
        self,
        context: &CommandContext,
        source: &SourceTree,
        source_files: &FileCatalog,
        contracts: &ContractsStore,
        catalog_reads: &mut CatalogReadBudget,
    ) -> anyhow::Result<(RustExtractionInput, Vec<RustCrateRoot>)> {
        let (request, expected_crates, persisted) = match self {
            Self::Syntax(generation_crates) => {
                return Ok((RustExtractionInput::Syntax, generation_crates.to_vec()));
            }
            Self::Establish(request, expected) => (request, expected, None),
            Self::Validate(request, extraction) => {
                (request, Some(extraction.crates()), Some(extraction))
            }
        };
        let compiler = context.compiler();
        let crate_selection = compiler.validate_expected_crates(expected_crates)?;
        context.output().print_compiler_extraction_warning()?;
        let artifact = compiler.extract(
            &request,
            source.path(),
            source_files,
            crate_selection,
            catalog_reads,
        )?;
        let generation_crates = if let Some(extraction) = persisted {
            extraction.validate_compiler_artifact(&artifact, contracts, context.cancellation())?;
            Vec::new()
        } else {
            artifact
                .crates
                .iter()
                .map(conkit_signature::RustCompilerCrate::crate_root)
                .collect()
        };
        Ok((RustExtractionInput::Compiler(artifact), generation_crates))
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use conkit_signature::{CatalogPath, RustCrateKind, RustCrateRoot};

    use super::{
        CargoFeatures, CargoTarget, CompilerRequest, ExtractionDecision, ExtractionUse,
        RequestedExtraction, SignatureExtractionCoordinator,
    };
    use crate::catalog::ContractsStore;
    use crate::contracts::document::ContractCompilerContext;
    use crate::contracts::layout::{DocumentOrigin, LayoutExtraction};

    struct ReconciliationFixture {
        contracts: ContractsStore,
        syntax: LayoutExtraction,
        compiler: LayoutExtraction,
    }

    impl ReconciliationFixture {
        fn new() -> Self {
            let root = RustCrateRoot {
                id: "sample".to_owned(),
                root: CatalogPath::new("lib.rs").expect("root path"),
                kind: RustCrateKind::Library,
            };
            let origin =
                DocumentOrigin::new(CatalogPath::new("main.yml").expect("document path"), 0);
            Self {
                contracts: ContractsStore::new(PathBuf::from("contracts")),
                syntax: LayoutExtraction::Syntax {
                    crates: vec![root.clone()],
                    declared_at: origin.clone(),
                },
                compiler: LayoutExtraction::Compiler {
                    crates: vec![root],
                    context: ContractCompilerContext {
                        artifact_schema_version: 1,
                        extractor_version: "1".to_owned(),
                        compiler_version: "rustc".to_owned(),
                        rustdoc_format_version: 1,
                        target_triple: "target".to_owned(),
                        package: "sample".to_owned(),
                        target: "sample".to_owned(),
                        features: Vec::new(),
                        cfg_values: Vec::new(),
                    },
                    declared_at: origin,
                },
            }
        }

        fn compiler_request() -> RequestedExtraction<'static> {
            RequestedExtraction::Compiler(CompilerRequest::new(
                Path::new("Cargo.toml"),
                None,
                CargoTarget::Automatic,
                CargoFeatures::Default,
                None,
            ))
        }

        fn reconcile<'layout>(
            &self,
            requested: RequestedExtraction<'static>,
            usage: ExtractionUse<'layout>,
        ) -> Result<ExtractionDecision<'static, 'layout>, crate::error::CliError> {
            SignatureExtractionCoordinator::new(requested, &self.contracts).reconcile(usage)
        }
    }

    #[test]
    fn check_reconciliation_covers_every_requested_and_persisted_pair() {
        let fixture = ReconciliationFixture::new();
        for (compiler, kind, persisted) in [
            (false, 0, None),
            (false, 1, Some(&fixture.syntax)),
            (false, 2, Some(&fixture.compiler)),
            (true, 0, None),
            (true, 1, Some(&fixture.syntax)),
            (true, 2, Some(&fixture.compiler)),
        ] {
            let requested = if compiler {
                ReconciliationFixture::compiler_request()
            } else {
                RequestedExtraction::Syntax
            };
            match fixture.reconcile(requested, ExtractionUse::Check { persisted }) {
                Ok(ExtractionDecision::Syntax(crates)) if !compiler && kind < 2 => {
                    assert!(crates.is_empty());
                }
                Ok(ExtractionDecision::Validate(..)) if compiler && kind == 2 => {}
                Err(error) if !compiler && kind == 2 => {
                    assert!(error.to_string().contains("require compiler extraction"));
                }
                Err(error) if compiler && kind == 0 => {
                    assert!(error.to_string().contains("requires a signature-bearing"));
                }
                Err(error) if compiler && kind == 1 => {
                    assert!(error.to_string().contains("cannot override existing"));
                }
                _ => panic!("unexpected reconciliation outcome"),
            }
        }
    }

    #[test]
    fn generation_reconciliation_owns_fresh_existing_and_root_policy() {
        let fixture = ReconciliationFixture::new();
        let compiler = ReconciliationFixture::compiler_request();
        assert!(matches!(
            fixture.reconcile(
                compiler,
                ExtractionUse::Generation {
                    fresh: true,
                    persisted: None,
                    explicit_crates: &[],
                },
            ),
            Ok(ExtractionDecision::Establish(_, None)),
        ));
        let Err(error) = fixture.reconcile(
            compiler,
            ExtractionUse::Generation {
                fresh: false,
                persisted: None,
                explicit_crates: &[],
            },
        ) else {
            panic!("existing layout needs compiler metadata");
        };
        assert!(
            error
                .to_string()
                .contains("cannot update an existing layout")
        );

        let explicit = [fixture.syntax.crates()[0].clone()];
        for (requested, compiler) in [(RequestedExtraction::Syntax, false), (compiler, true)] {
            match fixture
                .reconcile(
                    requested,
                    ExtractionUse::Generation {
                        fresh: true,
                        persisted: None,
                        explicit_crates: &explicit,
                    },
                )
                .expect("fresh explicit roots")
            {
                ExtractionDecision::Syntax(crates) if !compiler => assert_eq!(crates, explicit),
                ExtractionDecision::Establish(_, Some(crates)) if compiler => {
                    assert_eq!(crates, explicit);
                }
                _ => panic!("unexpected explicit-root decision"),
            }
        }
    }
}
