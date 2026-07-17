//! Isolated compiler workspace and Cargo package/target selection.

use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

use cargo_metadata::{Metadata, Package, Target, TargetKind};
use conkit_signature::{CatalogPath, RustCrateKind, RustCrateRoot};

use super::CompilerCrateSelection;
use super::error::CompilerError;
use super::limits::CompilerUsage;
use super::process::CompilerOperation;
use crate::contracts::{CargoFeatures, CargoTarget, CompilerRequest};
use crate::platform::PortablePathRules;

pub(super) struct CompilerWorkspace {
    root: tempfile::TempDir,
}

impl CompilerWorkspace {
    pub(super) fn new() -> Result<Self, CompilerError> {
        let root = tempfile::Builder::new()
            .prefix("conkit-rustdoc-")
            .tempdir()
            .map_err(CompilerError::TemporaryWorkspace)?;
        Ok(Self { root })
    }

    pub(super) fn target_directory(&self) -> &Path {
        self.root.path()
    }

    pub(super) fn read_rustdoc_json(
        &self,
        project: &CargoProject,
        usage: &CompilerUsage,
    ) -> Result<Vec<u8>, CompilerError> {
        let mut candidates = Vec::new();
        let expected_name = format!("{}.json", project.target_name().replace('-', "_"));
        for entry in walkdir::WalkDir::new(self.root.path()).follow_links(false) {
            usage.checkpoint(CompilerOperation::Rustdoc)?;
            let entry = entry.map_err(|source| CompilerError::TemporaryArtifactWalk {
                path: self.root.path().to_path_buf(),
                source,
            })?;
            if entry.file_type().is_file()
                && entry.file_name() == OsStr::new(&expected_name)
                && entry
                    .path()
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|parent| parent == OsStr::new("doc"))
            {
                candidates.push(entry.path().to_path_buf());
            }
        }
        candidates.sort();
        let [path] = candidates.as_slice() else {
            return Err(CompilerError::RustdocArtifactCount {
                root: self.root.path().to_path_buf(),
                count: candidates.len(),
            });
        };
        usage.read_artifact(
            CompilerOperation::Rustdoc,
            path,
            usage.limits().artifact_bytes,
        )
    }
}

pub(super) struct CargoProject {
    package_id: String,
    package_name: String,
    target_name: String,
    target_root: PathBuf,
    kind: RustCrateKind,
    crate_types: Vec<String>,
    has_default_feature: bool,
    features: Vec<String>,
}

impl CargoProject {
    pub(super) fn select(
        metadata: Metadata,
        requested_package_id: Option<&str>,
        requested_target: CargoTarget<'_>,
    ) -> Result<Self, CompilerError> {
        let package = Self::select_package(&metadata, requested_package_id)?;
        let target = Self::select_target(package, requested_target)?;
        let kind = if Self::is_library(target) {
            RustCrateKind::Library
        } else {
            RustCrateKind::Binary
        };
        let mut crate_types = target
            .crate_types
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        crate_types.sort();
        crate_types.dedup();
        let mut features = metadata
            .resolve
            .as_ref()
            .and_then(|resolve| resolve.nodes.iter().find(|node| node.id == package.id))
            .map(|node| {
                node.features
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        features.sort();
        features.dedup();

        Ok(Self {
            package_id: package.id.repr.clone(),
            package_name: package.name.to_string(),
            target_name: target.name.clone(),
            target_root: target.src_path.as_std_path().to_path_buf(),
            kind,
            crate_types,
            has_default_feature: package.features.contains_key("default"),
            features,
        })
    }

    fn select_package<'a>(
        metadata: &'a Metadata,
        requested_package_id: Option<&str>,
    ) -> Result<&'a Package, CompilerError> {
        if let Some(id) = requested_package_id {
            return metadata
                .packages
                .iter()
                .find(|package| package.id.repr == id)
                .ok_or_else(|| CompilerError::ResolvedPackageMissing { id: id.to_owned() });
        }
        if let Some(package) = metadata.root_package() {
            return Ok(package);
        }
        if !metadata.workspace_default_members.is_available() {
            return Err(CompilerError::WorkspaceDefaultMembersUnavailable);
        }
        let defaults = metadata.workspace_default_packages();
        let [package] = defaults.as_slice() else {
            return Err(CompilerError::AmbiguousPackage {
                candidates: defaults
                    .iter()
                    .map(|package| package.name.to_string())
                    .collect(),
            });
        };
        Ok(package)
    }

    fn select_target<'a>(
        package: &'a Package,
        requested: CargoTarget<'_>,
    ) -> Result<&'a Target, CompilerError> {
        match requested {
            CargoTarget::Library => {
                let libraries = package
                    .targets
                    .iter()
                    .filter(|target| Self::is_library(target))
                    .collect::<Vec<_>>();
                Self::one_target(libraries, "library")
            }
            CargoTarget::Binary(name) => package
                .targets
                .iter()
                .find(|target| target.is_bin() && target.name == name)
                .ok_or_else(|| CompilerError::TargetMissing {
                    requested: format!("binary {name:?}"),
                }),
            CargoTarget::Automatic => {
                let libraries = package
                    .targets
                    .iter()
                    .filter(|target| Self::is_library(target))
                    .collect::<Vec<_>>();
                if libraries.len() == 1 {
                    return Ok(libraries[0]);
                }
                if libraries.len() > 1 {
                    return Err(CompilerError::AmbiguousTarget {
                        candidates: libraries.iter().map(|target| target.name.clone()).collect(),
                    });
                }
                let binaries = package
                    .targets
                    .iter()
                    .filter(|target| target.is_bin())
                    .collect::<Vec<_>>();
                Self::one_target(binaries, "binary")
            }
        }
    }

    fn one_target<'a>(
        candidates: Vec<&'a Target>,
        requested: &str,
    ) -> Result<&'a Target, CompilerError> {
        match candidates.as_slice() {
            [] => Err(CompilerError::TargetMissing {
                requested: requested.to_owned(),
            }),
            [target] => Ok(*target),
            _ => Err(CompilerError::AmbiguousTarget {
                candidates: candidates
                    .iter()
                    .map(|target| target.name.clone())
                    .collect(),
            }),
        }
    }

    fn is_library(target: &Target) -> bool {
        target.kind.iter().any(|kind| {
            matches!(
                kind,
                TargetKind::Lib
                    | TargetKind::RLib
                    | TargetKind::DyLib
                    | TargetKind::CDyLib
                    | TargetKind::StaticLib
                    | TargetKind::ProcMacro
            )
        })
    }

    pub(super) fn crate_identity(
        &self,
        source_root: &Path,
        selection: CompilerCrateSelection<'_>,
    ) -> Result<RustCrateRoot, CompilerError> {
        let canonical_source = fs_err::canonicalize(source_root).map_err(|source| {
            CompilerError::SourceRootUnavailable {
                path: source_root.to_path_buf(),
                source,
            }
        })?;
        let canonical_target = fs_err::canonicalize(&self.target_root).map_err(|source| {
            CompilerError::CargoTargetUnavailable {
                path: self.target_root.clone(),
                source,
            }
        })?;
        let relative = canonical_target
            .strip_prefix(&canonical_source)
            .map_err(|_| CompilerError::CargoTargetOutsideSourceRoot {
                source_root: canonical_source.clone(),
                target_root: canonical_target.clone(),
            })?;
        let logical = CatalogPath::new(
            relative
                .components()
                .map(|component| {
                    let Component::Normal(value) = component else {
                        return Err(CompilerError::InvalidMappedSourcePath {
                            path: relative.to_path_buf(),
                        });
                    };
                    PortablePathRules::validate_component(value).map_err(|source| {
                        CompilerError::InvalidPortableSourcePath {
                            path: relative.to_path_buf(),
                            message: source.to_string(),
                        }
                    })?;
                    value.to_str().map(str::to_owned).ok_or_else(|| {
                        CompilerError::InvalidMappedSourcePath {
                            path: relative.to_path_buf(),
                        }
                    })
                })
                .collect::<Result<Vec<_>, _>>()?
                .join("/"),
        )
        .map_err(|source| CompilerError::InvalidPortableSourcePath {
            path: relative.to_path_buf(),
            message: source.to_string(),
        })?;
        let derived = RustCrateRoot {
            id: self.target_name.clone(),
            root: logical,
            kind: self.kind,
        };
        let CompilerCrateSelection::Expected(expected) = selection else {
            return Ok(derived);
        };
        if expected.root != derived.root || expected.kind != derived.kind {
            return Err(CompilerError::CrateRootMismatch {
                expected: format!("{} ({:?})", expected.root, expected.kind),
                actual: format!("{} ({:?})", derived.root, derived.kind),
            });
        }
        Ok((*expected).clone())
    }

    pub(super) fn package_name(&self) -> &str {
        &self.package_name
    }

    pub(super) fn target_name(&self) -> &str {
        &self.target_name
    }

    pub(super) fn target_root(&self) -> &Path {
        &self.target_root
    }

    pub(super) fn crate_types(&self) -> &[String] {
        &self.crate_types
    }

    pub(super) fn kind(&self) -> RustCrateKind {
        self.kind
    }

    pub(super) fn features(&self) -> &[String] {
        &self.features
    }

    pub(super) fn has_default_feature(&self) -> bool {
        self.has_default_feature
    }

    pub(super) fn rustc_arguments<const N: usize>(
        &self,
        request: &CompilerRequest<'_>,
        manifest: &Path,
        target_directory: &Path,
        target_override: Option<&str>,
        compiler_arguments: [&str; N],
    ) -> Vec<OsString> {
        let mut arguments = vec![OsString::from("rustc"), OsString::from("--locked")];
        self.append_cargo_selection(request, manifest, target_override, &mut arguments);
        arguments.push(OsString::from("--target-dir"));
        arguments.push(target_directory.as_os_str().to_owned());
        arguments.push(OsString::from("--"));
        arguments.extend(compiler_arguments.into_iter().map(OsString::from));
        arguments
    }

    pub(super) fn rustdoc_arguments(
        &self,
        request: &CompilerRequest<'_>,
        manifest: &Path,
        target_directory: &Path,
    ) -> Vec<OsString> {
        let mut arguments = vec![OsString::from("rustdoc"), OsString::from("--locked")];
        self.append_cargo_selection(request, manifest, None, &mut arguments);
        arguments.extend([
            OsString::from("--target-dir"),
            target_directory.as_os_str().to_owned(),
            OsString::from("--"),
            OsString::from("-Z"),
            OsString::from("unstable-options"),
            OsString::from("--output-format"),
            OsString::from("json"),
            OsString::from("--document-hidden-items"),
        ]);
        arguments
    }

    fn append_cargo_selection(
        &self,
        request: &CompilerRequest<'_>,
        manifest: &Path,
        target_override: Option<&str>,
        arguments: &mut Vec<OsString>,
    ) {
        arguments.extend([
            OsString::from("--manifest-path"),
            manifest.as_os_str().to_owned(),
            OsString::from("--package"),
            OsString::from(self.package_id.as_str()),
        ]);
        match self.kind {
            RustCrateKind::Library => arguments.push(OsString::from("--lib")),
            RustCrateKind::Binary => {
                arguments.push(OsString::from("--bin"));
                arguments.push(OsString::from(self.target_name.as_str()));
            }
        }
        match request.features() {
            CargoFeatures::Default => {}
            CargoFeatures::Selected {
                names,
                include_default,
            } => {
                if !names.is_empty() {
                    arguments.push(OsString::from("--features"));
                    arguments.push(OsString::from(names.join(",")));
                }
                if !include_default {
                    arguments.push(OsString::from("--no-default-features"));
                }
            }
            CargoFeatures::All => arguments.push(OsString::from("--all-features")),
        }
        if let Some(target) = target_override.or(request.target_triple()) {
            arguments.push(OsString::from("--target"));
            arguments.push(OsString::from(target));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::compiler::tests::*;

    #[test]
    fn cargo_project_prefers_the_sole_library_without_an_explicit_target() {
        let project =
            CargoProject::select(CompilerFixture::metadata(), None, CargoTarget::Automatic)
                .expect("one default package and one library");

        assert_eq!(project.package_name(), "sample");
        assert_eq!(project.target_name(), "sample");
        assert_eq!(project.kind(), RustCrateKind::Library);
        assert_eq!(project.crate_types(), ["lib"]);
        assert_eq!(project.features(), ["serde"]);
    }

    #[test]
    fn cargo_project_requires_an_explicit_package_for_multiple_workspace_defaults() {
        let first_id = "path+file:///workspace#sample@0.1.0";
        let second_id = "path+file:///workspace/second#second@0.1.0";
        let mut metadata =
            serde_json::to_value(CompilerFixture::metadata()).expect("serializable metadata");
        {
            let packages = metadata["packages"]
                .as_array_mut()
                .expect("metadata packages");
            let mut second = packages[0].clone();
            second["name"] = serde_json::json!("second");
            second["id"] = serde_json::json!(second_id);
            second["manifest_path"] = serde_json::json!("/workspace/second/Cargo.toml");
            packages.push(second);
        }
        metadata["workspace_members"] = serde_json::json!([first_id, second_id]);
        metadata["workspace_default_members"] = serde_json::json!([first_id, second_id]);
        metadata["resolve"]["root"] = serde_json::Value::Null;
        let metadata = serde_json::from_value(metadata).expect("ambiguous workspace metadata");

        let error = CargoProject::select(metadata, None, CargoTarget::Automatic)
            .err()
            .expect("multiple default packages require --package");

        match error {
            CompilerError::AmbiguousPackage { candidates } => {
                assert_eq!(candidates, ["sample", "second"]);
            }
            other => panic!("expected ambiguous-package error, got {other:?}"),
        }
    }

    #[test]
    fn rustdoc_json_flags_are_forwarded_after_cargos_argument_delimiter() {
        let project =
            CargoProject::select(CompilerFixture::metadata(), None, CargoTarget::Automatic)
                .expect("selected library target");
        let arguments = project.rustdoc_arguments(
            &CompilerFixture::request(),
            Path::new("/workspace/Cargo.toml"),
            Path::new("/temporary/target"),
        );
        let arguments = arguments
            .iter()
            .map(|argument| argument.to_string_lossy())
            .collect::<Vec<_>>();
        let delimiter = arguments
            .iter()
            .position(|argument| argument == "--")
            .expect("rustdoc argument delimiter");

        assert_eq!(
            &arguments[delimiter + 1..],
            [
                "-Z",
                "unstable-options",
                "--output-format",
                "json",
                "--document-hidden-items"
            ]
        );
        assert!(
            !arguments
                .iter()
                .any(|argument| argument == "--document-private-items")
        );
    }

    #[test]
    fn every_cargo_operation_is_locked_and_uses_the_pinned_toolchain() {
        let extractor = CompilerExtractor::new(CompilerFixture::cancellation());
        let request = CompilerFixture::request();
        let project = CargoProject::select(CompilerFixture::metadata(), None, request.target())
            .expect("selected library target");
        let manifest = Path::new("/workspace/Cargo.toml");
        let target_directory = Path::new("/temporary/target");
        let command_arguments = [
            extractor.metadata_arguments(&request, manifest, None),
            extractor.package_id_arguments(manifest, "sample"),
            project.rustc_arguments(&request, manifest, target_directory, None, ["-vV"]),
            project.rustdoc_arguments(&request, manifest, target_directory),
        ];

        assert_eq!(
            extractor.cargo.prefix,
            [OsString::from(format!("+{SUPPORTED_COMPILER_TOOLCHAIN}"))]
        );
        for arguments in command_arguments {
            assert_eq!(arguments.get(1), Some(&OsString::from("--locked")));
        }
        assert_eq!(COMPILER_EXTRACTOR_VERSION, "conkit-rustdoc-json-v1");
    }

    #[test]
    fn typed_feature_modes_preserve_metadata_and_target_arguments() {
        let extractor = CompilerExtractor::new(CompilerFixture::cancellation());
        let project =
            CargoProject::select(CompilerFixture::metadata(), None, CargoTarget::Automatic)
                .expect("selected library target");
        let selected = vec!["serde".to_owned()];
        let cases = [
            (
                CargoFeatures::Default,
                vec!["--features", "sample/default"],
                Vec::new(),
            ),
            (
                CargoFeatures::Selected {
                    names: &selected,
                    include_default: true,
                },
                vec!["--features", "sample/default,sample/serde"],
                vec!["--features", "serde"],
            ),
            (
                CargoFeatures::Selected {
                    names: &selected,
                    include_default: false,
                },
                vec!["--features", "sample/serde", "--no-default-features"],
                vec!["--features", "serde", "--no-default-features"],
            ),
            (
                CargoFeatures::All,
                vec!["--all-features"],
                vec!["--all-features"],
            ),
        ];

        for (features, expected_metadata, expected_target) in cases {
            let mut metadata = Vec::new();
            extractor.append_feature_arguments(features, &project, &mut metadata);
            let request = CompilerRequest::new(
                Path::new("/workspace/Cargo.toml"),
                None,
                CargoTarget::Automatic,
                features,
                None,
            );
            let mut target = Vec::new();
            project.append_cargo_selection(
                &request,
                Path::new("/workspace/Cargo.toml"),
                None,
                &mut target,
            );
            assert_eq!(
                metadata,
                expected_metadata
                    .into_iter()
                    .map(OsString::from)
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                &target[5..],
                expected_target
                    .into_iter()
                    .map(OsString::from)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn cargo_project_requires_an_explicit_choice_for_multiple_binary_targets() {
        let mut metadata = CompilerFixture::metadata();
        let package = metadata.packages.first_mut().expect("fixture package");
        package.targets.retain(|target| target.is_bin());
        let mut duplicate = package.targets[0].clone();
        duplicate.name = "second-cli".to_owned();
        package.targets.push(duplicate);

        let selected =
            CargoProject::select(metadata.clone(), None, CargoTarget::Binary("second-cli"))
                .expect("explicit binary target");
        assert_eq!(selected.target_name(), "second-cli");
        assert_eq!(selected.kind(), RustCrateKind::Binary);

        let error = CargoProject::select(metadata, None, CargoTarget::Automatic)
            .err()
            .expect("two binaries are ambiguous without --bin");

        assert!(matches!(error, CompilerError::AmbiguousTarget { .. }));
    }

    #[test]
    fn expected_crate_root_must_match_the_cargo_target() {
        let root = assert_fs::TempDir::new().expect("temporary source root");
        let source = root.path().join("src");
        std::fs::create_dir(&source).expect("source directory");
        let target = source.join("lib.rs");
        std::fs::write(&target, "pub fn answer() {}\n").expect("crate root");
        let mut project =
            CargoProject::select(CompilerFixture::metadata(), None, CargoTarget::Automatic)
                .expect("selected project");
        project.target_root = target;
        let expected = RustCrateRoot {
            id: "sample".to_owned(),
            root: CatalogPath::new("different.rs").expect("different root"),
            kind: RustCrateKind::Library,
        };

        let error = project
            .crate_identity(&source, CompilerCrateSelection::Expected(&expected))
            .expect_err("persisted crate identity must match Cargo metadata");

        assert!(matches!(error, CompilerError::CrateRootMismatch { .. }));
        root.close().expect("close temporary source root");
    }
}
