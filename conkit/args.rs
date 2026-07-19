//! Clap grammar for the `conkit` executable.
//!
//! The types in this module describe the stable command-line interface only.
//! They retain raw `PathBuf` values and simple flags while validating the
//! relationships between syntax/compiler selection and Cargo-only options,
//! concrete Cargo targets and features, and repeatable typed crate roots.
//! Command execution modules convert the resulting parser state into domain
//! requests.

use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum, ValueHint};

use crate::contracts::{CargoFeatures, CargoTarget, CompilerRequest, RequestedExtraction};

/// Root parser for all CLI input.
#[derive(Debug, Parser)]
#[command(version, about = "Contract Kit", arg_required_else_help = true)]
pub(crate) struct Cli {
    /// Top-level command selected by the user.
    #[command(subcommand)]
    pub(crate) command: Command,
}

/// Top-level command families exposed by the executable.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Check source files against existing contract files.
    Check(CheckCommand),
    /// Generate contract files from source files.
    Generate(GenerateCommand),
    /// Archive the current contract catalog.
    Archive(ArchiveCommand),
    /// Compare current contracts with an archived catalog.
    Diff(DiffCommand),
}

/// Parsed arguments for `conkit check`.
#[derive(Debug, Args)]
pub(crate) struct CheckCommand {
    /// Contract family to check.
    #[command(subcommand)]
    pub(crate) subject: CheckSubject,
}

/// Contract targets accepted by `conkit check`.
#[derive(Debug, Subcommand)]
pub(crate) enum CheckSubject {
    /// Check all implemented contract families.
    All(SignatureCheckArgs),
    /// Check signature contracts only.
    #[command(alias = "signature")]
    Signatures(SignatureCheckArgs),
    /// Check sketch contracts only.
    #[command(alias = "sketch")]
    Sketches(CheckArgs),
}

/// Check arguments for targets that include signature extraction.
#[derive(Debug, Args)]
pub(crate) struct SignatureCheckArgs {
    /// Filesystem and mode arguments shared with sketch-only checks.
    #[command(flatten)]
    pub(crate) common: CheckArgs,

    /// Signature-extraction selection and Cargo inputs.
    #[command(flatten)]
    pub(crate) signature: SignatureOptions,
}

/// Shared filesystem and mode flags for check commands.
#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("check-mode")
        .args(["default_mode", "strict", "warning"])
        .multiple(false)
))]
pub(crate) struct CheckArgs {
    /// Root directory containing source files to inspect.
    #[arg(long, value_name = "DIR", value_hint = ValueHint::DirPath)]
    pub(crate) source: PathBuf,

    /// Root directory containing contract files to compare against.
    #[arg(long, value_name = "DIR", value_hint = ValueHint::DirPath)]
    pub(crate) contracts: PathBuf,

    /// Report file to write for the requested check.
    #[arg(long, value_name = "FILE", value_hint = ValueHint::FilePath)]
    pub(crate) output: PathBuf,

    /// Enforce errors while permitting signature capability warnings.
    #[arg(long = "default", group = "check-mode")]
    pub(crate) default_mode: bool,

    /// Also fail on signature capability warnings.
    #[arg(long, group = "check-mode")]
    pub(crate) strict: bool,

    /// Emit diagnostics without failing the contract check.
    #[arg(long, group = "check-mode")]
    pub(crate) warning: bool,
}

/// Parsed arguments for `conkit generate`.
#[derive(Debug, Args)]
pub(crate) struct GenerateCommand {
    /// Contract family to generate.
    #[command(subcommand)]
    pub(crate) subject: GenerateSubject,
}

/// Contract targets accepted by `conkit generate`.
#[derive(Debug, Subcommand)]
pub(crate) enum GenerateSubject {
    /// Generate every implemented contract family.
    All(SignatureGenerateArgs),
    /// Generate signature contracts only.
    #[command(alias = "signature")]
    Signatures(SignatureGenerateArgs),
    /// Generate sketch contracts only.
    #[command(alias = "sketch")]
    Sketches(GenerateArgs),
}

/// Signature-aware filesystem and extraction flags for generation.
#[derive(Debug, Args)]
pub(crate) struct SignatureGenerateArgs {
    /// Filesystem arguments shared with sketch-only generation.
    #[command(flatten)]
    pub(crate) common: GenerateArgs,

    /// Explicit Rust crate root; KIND must be library or binary.
    #[arg(
        long = "crate-root",
        value_name = "CRATE_ID=KIND:RELATIVE_PATH",
        action = clap::ArgAction::Append
    )]
    pub(crate) crate_roots: Vec<CrateRootArg>,

    /// Signature-extraction selection and Cargo inputs.
    #[command(flatten)]
    pub(crate) signature: SignatureOptions,
}

/// Selects the signature extraction capability used by check or generation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
enum SignatureExtractorArg {
    /// Parse the exact allowlisted Rust sources without invoking Cargo.
    #[default]
    Syntax,
    /// Invoke Contract Kit's pinned, dated nightly Cargo/rustdoc toolchain.
    Compiler,
}

/// Cargo-native options for signature extraction.
#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("cargo-target")
        .args(["library", "binary"])
        .multiple(false)
))]
pub(crate) struct SignatureOptions {
    /// Select portable syntax extraction or opt-in Cargo/rustdoc extraction
    /// (default: syntax).
    #[arg(
        long,
        value_enum,
        default_value_t = SignatureExtractorArg::Syntax,
        hide_default_value = true,
        value_name = "SIGNATURE_EXTRACTOR"
    )]
    signature_extractor: SignatureExtractorArg,

    /// Cargo manifest used by compiler extraction.
    #[arg(
        long,
        value_name = "FILE",
        value_hint = ValueHint::FilePath,
        required_if_eq("signature_extractor", "compiler")
    )]
    manifest_path: Option<PathBuf>,

    /// Cargo package specification selected for compiler extraction.
    #[arg(
        long,
        value_name = "SPEC",
        value_parser = SignatureOptions::parse_package,
        requires = "manifest_path"
    )]
    package: Option<String>,

    /// Select the package library target.
    #[arg(long = "lib", group = "cargo-target", requires = "manifest_path")]
    library: bool,

    /// Select one named package binary target.
    #[arg(
        long = "bin",
        value_name = "NAME",
        value_parser = SignatureOptions::parse_binary,
        group = "cargo-target",
        requires = "manifest_path"
    )]
    binary: Option<String>,

    /// Activate Cargo features; may be repeated or comma-delimited.
    #[arg(
        long,
        value_name = "FEATURES",
        value_parser = SignatureOptions::parse_feature,
        value_delimiter = ',',
        action = clap::ArgAction::Append,
        conflicts_with = "all_features",
        requires = "manifest_path"
    )]
    features: Vec<String>,

    /// Activate every Cargo feature.
    #[arg(
        long,
        conflicts_with_all = ["features", "no_default_features"],
        requires = "manifest_path"
    )]
    all_features: bool,

    /// Do not activate the package's default Cargo features.
    #[arg(long, conflicts_with = "all_features", requires = "manifest_path")]
    no_default_features: bool,

    /// Concrete Rust target triple; Cargo aliases and custom target paths are unsupported.
    #[arg(
        long,
        value_name = "TRIPLE",
        value_parser = SignatureOptions::parse_target,
        requires = "manifest_path"
    )]
    target: Option<String>,
}

impl SignatureOptions {
    /// Converts clap state into the one closed extraction request used at runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when Cargo-only options accompany syntax extraction,
    /// compiler extraction lacks a manifest, or a Cargo feature is repeated.
    pub(crate) fn requested_extraction(
        &self,
    ) -> Result<RequestedExtraction<'_>, SignatureOptionsError> {
        match self.signature_extractor {
            SignatureExtractorArg::Syntax => {
                if self.has_cargo_options() {
                    return Err(SignatureOptionsError::SyntaxWithCargoOptions);
                }
                return Ok(RequestedExtraction::Syntax);
            }
            SignatureExtractorArg::Compiler if self.manifest_path.is_none() => {
                return Err(SignatureOptionsError::CompilerManifestRequired);
            }
            SignatureExtractorArg::Compiler => {}
        }

        let mut distinct = BTreeSet::new();
        for feature in &self.features {
            if !distinct.insert(feature.as_str()) {
                return Err(SignatureOptionsError::DuplicateFeature {
                    feature: feature.clone(),
                });
            }
        }

        let manifest = self
            .manifest_path
            .as_deref()
            .ok_or(SignatureOptionsError::CompilerManifestRequired)?;
        let target = if self.library {
            CargoTarget::Library
        } else if let Some(binary) = self.binary.as_deref() {
            CargoTarget::Binary(binary)
        } else {
            CargoTarget::Automatic
        };
        let features = if self.all_features {
            CargoFeatures::All
        } else if self.features.is_empty() && !self.no_default_features {
            CargoFeatures::Default
        } else {
            CargoFeatures::Selected {
                names: &self.features,
                include_default: !self.no_default_features,
            }
        };

        Ok(RequestedExtraction::Compiler(CompilerRequest::new(
            manifest,
            self.package.as_deref(),
            target,
            features,
            self.target.as_deref(),
        )))
    }

    fn parse_package(value: &str) -> Result<String, SignatureValueError> {
        SignatureValueKind::Package.parse(value)
    }

    fn parse_binary(value: &str) -> Result<String, SignatureValueError> {
        SignatureValueKind::Binary.parse(value)
    }

    fn parse_feature(value: &str) -> Result<String, SignatureValueError> {
        SignatureValueKind::Feature.parse(value)
    }

    fn parse_target(value: &str) -> Result<String, SignatureValueError> {
        SignatureValueKind::Target.parse(value)
    }

    fn has_cargo_options(&self) -> bool {
        self.manifest_path.is_some()
            || self.package.is_some()
            || self.library
            || self.binary.is_some()
            || !self.features.is_empty()
            || self.all_features
            || self.no_default_features
            || self.target.is_some()
    }
}

/// Invalid relationship between extractor selection and Cargo-only options.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum SignatureOptionsError {
    /// Cargo-only options were supplied while syntax extraction was selected.
    #[error(
        "Cargo selection options require `--signature-extractor compiler`; syntax extraction is the default"
    )]
    SyntaxWithCargoOptions,
    /// Compiler extraction did not receive a manifest path.
    #[error("`--signature-extractor compiler` requires `--manifest-path FILE`")]
    CompilerManifestRequired,
    /// One feature was repeated across comma-delimited or repeated options.
    #[error("Cargo feature {feature:?} was supplied more than once")]
    DuplicateFeature { feature: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SignatureValueKind {
    Package,
    Binary,
    Feature,
    Target,
}

impl SignatureValueKind {
    fn parse(self, value: &str) -> Result<String, SignatureValueError> {
        if value.is_empty() {
            return Err(SignatureValueError::Empty { kind: self });
        }
        if value.chars().any(char::is_control) {
            return Err(SignatureValueError::Control { kind: self });
        }
        if value.chars().any(char::is_whitespace) {
            return Err(SignatureValueError::Whitespace { kind: self });
        }
        if self == Self::Feature && value.split('/').any(str::is_empty) {
            return Err(SignatureValueError::FeatureQualification);
        }
        if self == Self::Target
            && (value == "host-tuple"
                || !value.contains('-')
                || value.contains('/')
                || value.contains('\\')
                || value.ends_with(".json"))
        {
            return Err(SignatureValueError::NonConcreteTarget);
        }
        Ok(value.to_owned())
    }
}

impl fmt::Display for SignatureValueKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Package => "Cargo package specification",
            Self::Binary => "Cargo binary target name",
            Self::Feature => "Cargo feature",
            Self::Target => "Cargo target",
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
enum SignatureValueError {
    #[error("{kind} must not be empty")]
    Empty { kind: SignatureValueKind },
    #[error("{kind} must not contain whitespace")]
    Whitespace { kind: SignatureValueKind },
    #[error("{kind} must not contain control characters")]
    Control { kind: SignatureValueKind },
    #[error("Cargo feature qualification must not contain an empty path segment")]
    FeatureQualification,
    #[error(
        "Cargo target must be a concrete Rust target triple; `host-tuple` and custom target paths are unsupported"
    )]
    NonConcreteTarget,
}

/// Shared filesystem flags for generate commands.
#[derive(Debug, Args)]
pub(crate) struct GenerateArgs {
    /// Root directory containing source files to inspect.
    #[arg(long, value_name = "DIR", value_hint = ValueHint::DirPath)]
    pub(crate) source: PathBuf,

    /// Root directory where generated contract files should be written.
    #[arg(long, value_name = "DIR", value_hint = ValueHint::DirPath)]
    pub(crate) contracts: PathBuf,

    /// Adopt matching pre-existing generated outputs into managed ownership.
    #[arg(long)]
    pub(crate) adopt_existing: bool,
}

/// One parsed `--crate-root CRATE_ID=KIND:RELATIVE_PATH` value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CrateRootArg {
    id: String,
    kind: conkit_signature::RustCrateKind,
    root: String,
}

impl CrateRootArg {
    /// Converts the CLI value into the signature domain's logical-path DTO.
    ///
    /// # Errors
    ///
    /// Returns an error when the parsed root is not a valid portable relative
    /// catalog path.
    pub(crate) fn to_domain(
        &self,
    ) -> Result<conkit_signature::RustCrateRoot, conkit_signature::FileCatalogError> {
        Ok(conkit_signature::RustCrateRoot {
            id: self.id.clone(),
            root: conkit_signature::CatalogPath::new(self.root.clone())?,
            kind: self.kind,
        })
    }
}

impl FromStr for CrateRootArg {
    type Err = CrateRootArgError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let Some((id, kind_and_root)) = value.split_once('=') else {
            return Err(CrateRootArgError::MissingSeparator);
        };
        if id.is_empty() {
            return Err(CrateRootArgError::EmptyId);
        }
        if id.chars().any(char::is_whitespace) {
            return Err(CrateRootArgError::IdWhitespace);
        }
        if id.contains(':') || id.contains('=') {
            return Err(CrateRootArgError::IdDelimiter);
        }
        if id.chars().any(char::is_control) {
            return Err(CrateRootArgError::IdControl);
        }
        let Some((kind, root)) = kind_and_root.split_once(':') else {
            return Err(CrateRootArgError::MissingKindSeparator);
        };
        if kind.is_empty() {
            return Err(CrateRootArgError::EmptyKind);
        }
        if kind.trim() != kind {
            return Err(CrateRootArgError::KindWhitespace);
        }
        let kind = match kind {
            "library" => conkit_signature::RustCrateKind::Library,
            "binary" => conkit_signature::RustCrateKind::Binary,
            kind => {
                return Err(CrateRootArgError::UnsupportedKind {
                    kind: kind.to_owned(),
                });
            }
        };
        if root.is_empty() {
            return Err(CrateRootArgError::EmptyRoot);
        }
        if root.trim() != root {
            return Err(CrateRootArgError::RootWhitespace);
        }
        if !std::path::Path::new(root).is_relative() {
            return Err(CrateRootArgError::AbsoluteRoot);
        }

        Ok(Self {
            id: id.to_owned(),
            kind,
            root: root.to_owned(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum CrateRootArgError {
    #[error("crate root must have the form CRATE_ID=KIND:RELATIVE_PATH")]
    MissingSeparator,
    #[error("crate id must not be empty")]
    EmptyId,
    #[error("crate id must not contain whitespace")]
    IdWhitespace,
    #[error("crate id must not contain `:` or `=` identity delimiters")]
    IdDelimiter,
    #[error("crate id must not contain control characters")]
    IdControl,
    #[error("crate root must include a target kind as KIND:RELATIVE_PATH")]
    MissingKindSeparator,
    #[error("crate kind must not be empty")]
    EmptyKind,
    #[error("crate kind has surrounding whitespace")]
    KindWhitespace,
    #[error("unsupported crate kind {kind:?}; crate kind must be either library or binary")]
    UnsupportedKind { kind: String },
    #[error("crate root path must not be empty")]
    EmptyRoot,
    #[error("crate root path has surrounding whitespace")]
    RootWhitespace,
    #[error("crate root path must be relative")]
    AbsoluteRoot,
}

/// Parsed arguments for `conkit archive`.
#[derive(Debug, Args)]
pub(crate) struct ArchiveCommand {
    /// Root directory containing the current contract files.
    #[arg(long, value_name = "DIR", value_hint = ValueHint::DirPath)]
    pub(crate) contracts: PathBuf,

    /// Directory where a timestamped archive file should be created.
    #[arg(long, value_name = "DIR", value_hint = ValueHint::DirPath)]
    pub(crate) archive: PathBuf,

    /// Select gzip archive output.
    #[arg(long)]
    pub(crate) gzip: bool,
}

/// Parsed arguments for `conkit diff`.
#[derive(Debug, Args)]
pub(crate) struct DiffCommand {
    /// Root directory containing the current contract files.
    #[arg(long, value_name = "DIR", value_hint = ValueHint::DirPath)]
    pub(crate) contracts: PathBuf,

    /// Archive file to compare against the current contract catalog.
    #[arg(long, value_name = "FILE", value_hint = ValueHint::FilePath)]
    pub(crate) archive: PathBuf,
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::{CheckSubject, Cli, Command, GenerateSubject, SignatureOptions};
    use crate::contracts::{CargoFeatures, CargoTarget, RequestedExtraction};

    struct CompilerOptionsFixture;

    impl CompilerOptionsFixture {
        fn parse(selection: &[&str]) -> SignatureOptions {
            let mut argv = vec![
                "conkit",
                "generate",
                "signatures",
                "--source",
                "src",
                "--contracts",
                "contracts",
                "--signature-extractor",
                "compiler",
                "--manifest-path",
                "Cargo.toml",
            ];
            argv.extend(selection.iter().copied());
            let cli = Cli::try_parse_from(argv).expect("valid compiler selection");
            let Command::Generate(command) = cli.command else {
                panic!("expected generate command");
            };
            let GenerateSubject::Signatures(args) = command.subject else {
                panic!("expected signature subject");
            };
            args.signature
        }
    }

    #[test]
    fn signature_extraction_defaults_to_portable_syntax_mode() {
        let cli = Cli::try_parse_from([
            "conkit",
            "check",
            "signatures",
            "--source",
            "src",
            "--contracts",
            "contracts",
            "--output",
            "report.yml",
        ])
        .expect("syntax extraction is the default");

        let Command::Check(command) = cli.command else {
            panic!("expected check command");
        };
        let CheckSubject::Signatures(args) = command.subject else {
            panic!("expected signature subject");
        };

        assert!(matches!(
            args.signature
                .requested_extraction()
                .expect("default extraction request"),
            RequestedExtraction::Syntax,
        ));
    }

    #[test]
    fn compiler_extraction_accepts_cargo_native_selection() {
        let options = CompilerOptionsFixture::parse(&[
            "--package",
            "sample@0.1.0",
            "--bin",
            "sample-cli",
            "--features",
            "client,sample/serde",
            "--features",
            "unstable",
            "--target",
            "x86_64-unknown-linux-gnu",
        ]);
        let RequestedExtraction::Compiler(request) = options
            .requested_extraction()
            .expect("validated compiler request")
        else {
            panic!("expected compiler extraction request");
        };
        assert_eq!(request.manifest(), std::path::Path::new("Cargo.toml"));
        assert_eq!(request.package(), Some("sample@0.1.0"));
        assert_eq!(request.target(), CargoTarget::Binary("sample-cli"));
        assert_eq!(
            request.features(),
            CargoFeatures::Selected {
                names: &[
                    "client".to_owned(),
                    "sample/serde".to_owned(),
                    "unstable".to_owned(),
                ],
                include_default: true,
            }
        );
        assert_eq!(request.target_triple(), Some("x86_64-unknown-linux-gnu"));
    }

    #[test]
    fn compiler_target_selection_converts_to_closed_variants() {
        for (selection, expected) in [
            (Vec::new(), CargoTarget::Automatic),
            (vec!["--lib"], CargoTarget::Library),
        ] {
            let options = CompilerOptionsFixture::parse(&selection);
            let RequestedExtraction::Compiler(request) =
                options.requested_extraction().expect("compiler request")
            else {
                panic!("expected compiler extraction request");
            };

            assert_eq!(request.target(), expected);
        }
    }

    #[test]
    fn compiler_feature_selection_converts_to_closed_variants() {
        for selection in [
            Vec::new(),
            vec!["--no-default-features"],
            vec!["--all-features"],
        ] {
            let options = CompilerOptionsFixture::parse(&selection);
            let RequestedExtraction::Compiler(request) =
                options.requested_extraction().expect("compiler request")
            else {
                panic!("expected compiler extraction request");
            };

            match selection.as_slice() {
                [] => assert_eq!(request.features(), CargoFeatures::Default),
                ["--no-default-features"] => assert_eq!(
                    request.features(),
                    CargoFeatures::Selected {
                        names: &[],
                        include_default: false,
                    }
                ),
                ["--all-features"] => {
                    assert_eq!(request.features(), CargoFeatures::All);
                }
                other => panic!("unexpected feature selection fixture: {other:?}"),
            }
        }
    }

    #[test]
    fn compiler_extraction_requires_a_manifest_path() {
        let error = Cli::try_parse_from([
            "conkit",
            "check",
            "all",
            "--source",
            "src",
            "--contracts",
            "contracts",
            "--output",
            "report.yml",
            "--signature-extractor",
            "compiler",
        ])
        .expect_err("compiler extraction without a manifest must fail");

        assert!(error.to_string().contains("--manifest-path"), "{error}");
    }

    #[test]
    fn compiler_target_and_feature_conflicts_fail_in_clap() {
        for conflicting in [
            vec!["--lib", "--bin", "sample"],
            vec!["--features", "serde", "--all-features"],
            vec!["--all-features", "--no-default-features"],
        ] {
            let mut argv = vec![
                "conkit",
                "generate",
                "signatures",
                "--source",
                "src",
                "--contracts",
                "contracts",
                "--signature-extractor",
                "compiler",
                "--manifest-path",
                "Cargo.toml",
            ];
            argv.extend(conflicting);

            let error = Cli::try_parse_from(argv)
                .expect_err("mutually exclusive Cargo selections must fail");
            assert!(error.to_string().contains("cannot be used with"), "{error}");
        }
    }

    #[test]
    fn compiler_text_values_reject_empty_whitespace_and_control_content() {
        for (flag, value, expected) in [
            ("--package", "", "must not be empty"),
            ("--package", "sample package", "must not contain whitespace"),
            ("--bin", " sample", "must not contain whitespace"),
            (
                "--bin",
                "sample\ncli",
                "must not contain control characters",
            ),
            ("--features", "", "must not be empty"),
            ("--features", "serde client", "must not contain whitespace"),
            ("--target", "", "must not be empty"),
            (
                "--target",
                " x86_64-unknown-linux-gnu",
                "must not contain whitespace",
            ),
            (
                "--target",
                "host-tuple",
                "must be a concrete Rust target triple",
            ),
            (
                "--target",
                "targets/custom.json",
                "must be a concrete Rust target triple",
            ),
            (
                "--target",
                "./custom-target.json",
                "must be a concrete Rust target triple",
            ),
        ] {
            let error = Cli::try_parse_from([
                "conkit",
                "generate",
                "signatures",
                "--source",
                "src",
                "--contracts",
                "contracts",
                "--signature-extractor",
                "compiler",
                "--manifest-path",
                "Cargo.toml",
                flag,
                value,
            ])
            .expect_err("invalid Cargo text values must fail during clap parsing");

            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn compiler_feature_selection_rejects_duplicates_across_all_spellings() {
        let options =
            CompilerOptionsFixture::parse(&["--features", "serde,client", "--features", "serde"]);
        let error = options
            .requested_extraction()
            .expect_err("duplicate Cargo features must fail validation");

        assert_eq!(
            error.to_string(),
            "Cargo feature \"serde\" was supplied more than once"
        );
    }

    #[test]
    fn sketch_only_subjects_reject_signature_extraction_flags() {
        for verb in ["check", "generate"] {
            let mut argv = vec![
                "conkit",
                verb,
                "sketches",
                "--source",
                "src",
                "--contracts",
                "contracts",
            ];
            if verb == "check" {
                argv.extend(["--output", "report.yml"]);
            }
            argv.extend([
                "--signature-extractor",
                "compiler",
                "--manifest-path",
                "Cargo.toml",
            ]);

            let error = Cli::try_parse_from(argv)
                .expect_err("sketch-only grammar must reject signature flags");
            assert!(
                error.to_string().contains("--signature-extractor"),
                "{error}"
            );
        }
    }
}
