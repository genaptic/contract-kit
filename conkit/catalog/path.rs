//! Path resolution and filesystem-to-catalog mapping.

use std::ffi::OsString;
use std::fmt;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use conkit_signature::CatalogPath;

use crate::error::CliError;
use crate::platform::PortablePathRules;

/// User-visible role assigned to a filesystem path during overlap validation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PathRole {
    /// Source tree supplied by `--source`.
    Source,
    /// Contract tree supplied by `--contracts`.
    Contracts,
    /// Report file supplied by `--output`.
    Report,
    /// Archive directory supplied by `--archive` during archive creation.
    ArchiveDirectory,
    /// Archive file supplied by `--archive` during diffing.
    ArchiveFile,
    /// File or directory managed below the contracts root.
    GeneratedOutput,
}

impl fmt::Display for PathRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Source => "source",
            Self::Contracts => "contracts",
            Self::Report => "report",
            Self::ArchiveDirectory => "archive directory",
            Self::ArchiveFile => "archive file",
            Self::GeneratedOutput => "generated output",
        })
    }
}

/// Symlink-aware absolute path used for containment and overlap decisions.
#[derive(Debug)]
pub(crate) struct ResolvedPath {
    role: PathRole,
    original: PathBuf,
    resolved: PathBuf,
}

impl ResolvedPath {
    /// Resolves a path through its nearest existing ancestor.
    ///
    /// Existing components are canonicalized so containment decisions account
    /// for symlinks, while any missing suffix retains its lexical spelling.
    ///
    /// # Errors
    ///
    /// Returns an error if the current directory or an existing ancestor cannot
    /// be resolved, or if an unresolved existing component is a symlink.
    pub(crate) fn new(role: PathRole, path: PathBuf) -> Result<Self, CliError> {
        let absolute = if path.is_absolute() {
            path.clone()
        } else {
            std::env::current_dir()
                .map_err(|source| CliError::PathResolution {
                    role,
                    path: path.clone(),
                    source,
                })?
                .join(&path)
        };
        let mut existing = absolute;
        let mut missing_suffix = Vec::<OsString>::new();

        let resolved_ancestor = loop {
            match fs_err::canonicalize(&existing) {
                Ok(resolved) => break resolved,
                Err(source) if source.kind() == ErrorKind::NotFound => {
                    if Self::entry_exists_without_symlink(role, &existing)? {
                        return Err(CliError::PathResolution {
                            role,
                            path: path.clone(),
                            source,
                        });
                    }
                    let component = existing
                        .components()
                        .next_back()
                        .map(|component| component.as_os_str().to_os_string())
                        .ok_or_else(|| CliError::PathResolution {
                            role,
                            path: path.clone(),
                            source: std::io::Error::new(
                                ErrorKind::NotFound,
                                "no existing path ancestor",
                            ),
                        })?;
                    missing_suffix.push(component);
                    if !existing.pop() {
                        return Err(CliError::PathResolution {
                            role,
                            path,
                            source: std::io::Error::new(
                                ErrorKind::NotFound,
                                "no existing path ancestor",
                            ),
                        });
                    }
                }
                Err(source) => {
                    return Err(CliError::PathResolution { role, path, source });
                }
            }
        };
        let mut resolved = resolved_ancestor;

        for component in missing_suffix.into_iter().rev() {
            if component == "." {
                continue;
            }
            if component == ".." {
                resolved.pop();
            } else {
                resolved.push(component);
            }
        }

        Ok(Self {
            role,
            original: path,
            resolved,
        })
    }

    fn entry_exists_without_symlink(role: PathRole, path: &Path) -> Result<bool, CliError> {
        match fs_err::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                Err(CliError::UnsupportedPathSymlink {
                    role,
                    path: path.to_path_buf(),
                })
            }
            Ok(_) => Ok(true),
            Err(source) if source.kind() == ErrorKind::NotFound => Ok(false),
            Err(source) => Err(CliError::PathResolution {
                role,
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Requires every resolved path to be disjoint from every other path.
    ///
    /// # Errors
    ///
    /// Returns an error if either path in any pair contains the other.
    pub(crate) fn ensure_disjoint(paths: &[Self]) -> Result<(), CliError> {
        for (index, left) in paths.iter().enumerate() {
            for right in paths.iter().skip(index + 1) {
                if left.contains(right) || right.contains(left) {
                    return Err(CliError::OverlappingPaths {
                        first_role: left.role,
                        first_path: left.original.clone(),
                        second_role: right.role,
                        second_path: right.original.clone(),
                    });
                }
            }
        }

        Ok(())
    }

    /// Returns a portable relative path from this location to `target`.
    ///
    /// # Errors
    ///
    /// Returns an error if the resolved paths do not share a filesystem prefix.
    pub(crate) fn relative_path_to(&self, target: &Self) -> Result<PathBuf, CliError> {
        let origin = self.resolved.components().collect::<Vec<_>>();
        let destination = target.resolved.components().collect::<Vec<_>>();
        let shared = origin
            .iter()
            .zip(&destination)
            .take_while(|(left, right)| Self::components_match(**left, **right))
            .count();

        if shared == 0 {
            return Err(CliError::PathResolution {
                role: target.role,
                path: target.original.clone(),
                source: std::io::Error::new(
                    ErrorKind::InvalidInput,
                    "paths do not share a filesystem prefix",
                ),
            });
        }

        let mut relative = PathBuf::new();
        for _ in origin.iter().skip(shared) {
            relative.push("..");
        }
        for component in destination.iter().skip(shared) {
            relative.push(component.as_os_str());
        }

        Ok(relative)
    }

    /// Requires `candidate` to resolve within this path.
    ///
    /// # Errors
    ///
    /// Returns an error if `candidate` resolves outside this path.
    pub(super) fn ensure_within(&self, candidate: &Self) -> Result<(), CliError> {
        if self.contains(candidate) {
            Ok(())
        } else {
            Err(CliError::PathEscapesRoot {
                root: self.original.clone(),
                path: candidate.original.clone(),
            })
        }
    }

    /// Requires a generated path to stay below this root without symlink descendants.
    ///
    /// # Errors
    ///
    /// Returns an error if the path is not a normal lexical descendant, an
    /// existing component is a symlink or cannot be resolved, or the resolved
    /// path escapes this root.
    pub(super) fn ensure_generated_path(&self, path: &Path) -> Result<(), CliError> {
        let relative =
            path.strip_prefix(&self.original)
                .map_err(|_| CliError::PathEscapesRoot {
                    root: self.original.clone(),
                    path: path.to_path_buf(),
                })?;
        let mut current = self.original.clone();

        for component in relative.components() {
            let Component::Normal(component) = component else {
                return Err(CliError::PathEscapesRoot {
                    root: self.original.clone(),
                    path: path.to_path_buf(),
                });
            };
            current.push(component);

            if !Self::entry_exists_without_symlink(PathRole::GeneratedOutput, &current)? {
                break;
            }
        }

        let candidate = Self::new(PathRole::GeneratedOutput, path.to_path_buf())?;
        self.ensure_within(&candidate)
    }

    /// Returns the canonicalized path plus any normalized missing suffix.
    pub(super) fn resolved_path(&self) -> &Path {
        &self.resolved
    }

    fn contains(&self, candidate: &Self) -> bool {
        let mut root_components = self.resolved.components();
        let mut candidate_components = candidate.resolved.components();

        loop {
            match (root_components.next(), candidate_components.next()) {
                (None, _) => return true,
                (Some(_), None) => return false,
                (Some(root), Some(candidate)) if Self::components_match(root, candidate) => {}
                (Some(_), Some(_)) => return false,
            }
        }
    }

    #[cfg(windows)]
    fn components_match(left: Component<'_>, right: Component<'_>) -> bool {
        left.as_os_str().eq_ignore_ascii_case(right.as_os_str())
    }

    #[cfg(not(windows))]
    fn components_match(left: Component<'_>, right: Component<'_>) -> bool {
        left == right
    }
}

/// Stable comparison key for paths on common ASCII-case-insensitive filesystems.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct PortableCatalogPathKey(String);

impl PortableCatalogPathKey {
    /// Builds a portable key without changing the path spelling used on disk.
    pub(crate) fn new(path: &CatalogPath) -> Self {
        Self(path.as_str().to_ascii_lowercase())
    }
}

/// Capability-neutral filesystem root shared by catalog owners.
#[derive(Debug)]
pub(super) struct CatalogDirectory {
    path: PathBuf,
    role: PathRole,
}

impl CatalogDirectory {
    /// Creates a source-root directory mapping.
    pub(super) fn source(path: PathBuf) -> Self {
        Self {
            path,
            role: PathRole::Source,
        }
    }

    /// Creates a contracts-root directory mapping.
    pub(super) fn contracts(path: PathBuf) -> Self {
        Self {
            path,
            role: PathRole::Contracts,
        }
    }

    /// Returns the selected filesystem root.
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    /// Requires this root to resolve to an existing directory.
    ///
    /// The selected root may itself be a symlink when its target is a
    /// directory.
    ///
    /// # Errors
    ///
    /// Returns an error if the root cannot be inspected or does not resolve to
    /// an existing directory.
    pub(super) fn validate_directory(&self) -> Result<(), CliError> {
        match fs_err::symlink_metadata(&self.path) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let resolved = fs_err::metadata(&self.path)?;
                if resolved.is_dir() {
                    Ok(())
                } else {
                    Err(CliError::RootIsNotDirectory {
                        role: self.role,
                        path: self.path.clone(),
                    })
                }
            }
            Ok(_) => Err(CliError::RootIsNotDirectory {
                role: self.role,
                path: self.path.clone(),
            }),
            Err(source) if source.kind() == ErrorKind::NotFound => {
                Err(CliError::RootIsNotDirectory {
                    role: self.role,
                    path: self.path.clone(),
                })
            }
            Err(source) => Err(CliError::Io(source)),
        }
    }

    /// Converts a file below this root into a logical catalog path.
    ///
    /// # Errors
    ///
    /// Returns an error if the file is outside the root, contains a non-normal,
    /// non-UTF-8, or nonportable component, or does not form a valid catalog
    /// path.
    pub(super) fn logical_path(&self, file: &Path) -> Result<CatalogPath, CliError> {
        let relative = file
            .strip_prefix(&self.path)
            .map_err(|_| CliError::PathOutsideRoot {
                path: file.to_path_buf(),
            })?;
        let mut parts = Vec::new();

        for component in relative.components() {
            match component {
                Component::Normal(value) => {
                    PortablePathRules::validate_component(value)?;
                    parts.push(
                        value
                            .to_str()
                            .ok_or(CliError::NonUtf8PathComponent)?
                            .to_owned(),
                    );
                }
                _ => {
                    return Err(CliError::InvalidCatalogPath {
                        path: file.to_path_buf(),
                    });
                }
            }
        }

        CatalogPath::new(parts.join("/")).map_err(CliError::from)
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use assert_fs::prelude::*;
    use conkit_signature::CatalogPath;

    use super::{CatalogDirectory, PathRole, PortableCatalogPathKey, ResolvedPath};

    #[test]
    fn maps_nested_os_paths_to_portable_catalog_paths() {
        let root = PathBuf::from("project");
        let file = root.join("src").join("lib.rs");
        let unicode = root.join("nested space").join("雪.rs");
        let directory = CatalogDirectory::source(root);

        let logical = directory.logical_path(&file).expect("logical path");
        let unicode_logical = directory
            .logical_path(&unicode)
            .expect("Unicode logical path");

        assert_eq!(logical.as_str(), "src/lib.rs");
        assert_eq!(unicode_logical.as_str(), "nested space/雪.rs");
    }

    #[test]
    fn rejects_paths_outside_root() {
        let root = PathBuf::from("project");
        let file = PathBuf::from("other/src/lib.rs");

        let error = CatalogDirectory::source(root)
            .logical_path(&file)
            .expect_err("outside root");

        assert!(error.to_string().contains("outside"));
    }

    #[test]
    fn rejects_dotdot_and_nonportable_components() {
        let root = PathBuf::from("project");
        let file = root.join("src").join("..").join("lib.rs");
        let reserved = root.join("CON.rs");
        let directory = CatalogDirectory::source(root);

        let error = directory.logical_path(&file).expect_err("dotdot");
        let reserved_error = directory
            .logical_path(&reserved)
            .expect_err("Windows device name");

        assert!(error.to_string().contains("contract catalog path"));
        assert!(
            reserved_error
                .to_string()
                .contains("Windows reserved device name")
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_non_utf8_logical_components() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let root = PathBuf::from("project");
        let file = root.join(OsString::from_vec(vec![0xff]));

        let error = CatalogDirectory::source(root)
            .logical_path(&file)
            .expect_err("non-UTF-8 component");

        assert!(error.to_string().contains("UTF-8"));
    }

    #[test]
    fn resolved_paths_compute_relative_location() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let contracts = ResolvedPath::new(
            PathRole::Contracts,
            temp.child("contracts").path().to_path_buf(),
        )
        .expect("contracts path");
        let source = ResolvedPath::new(PathRole::Source, temp.child("src").path().to_path_buf())
            .expect("source path");

        assert_eq!(
            contracts.relative_path_to(&source).expect("relative path"),
            PathBuf::from("..").join("src")
        );
        temp.close().expect("close temporary root");
    }

    #[test]
    fn resolved_paths_detect_equality_nesting_missing_components_and_unicode() {
        let temp = assert_fs::TempDir::new().expect("temp dir");
        let root = temp.child("røøt");
        let sibling = temp.child("sibling");
        root.create_dir_all().expect("root");
        sibling.create_dir_all().expect("sibling");
        let root_path =
            ResolvedPath::new(PathRole::Source, root.path().to_path_buf()).expect("resolved root");
        let same_path = ResolvedPath::new(
            PathRole::Contracts,
            root.path().join("missing").join("..").join("."),
        )
        .expect("normalized equal path");
        let nested_path = ResolvedPath::new(
            PathRole::Report,
            root.path().join("missing").join("report.yml"),
        )
        .expect("missing nested path");
        let sibling_path = ResolvedPath::new(PathRole::Contracts, sibling.path().to_path_buf())
            .expect("resolved sibling");

        assert!(ResolvedPath::ensure_disjoint(&[root_path, same_path]).is_err());
        let root_path =
            ResolvedPath::new(PathRole::Source, root.path().to_path_buf()).expect("resolved root");
        assert!(ResolvedPath::ensure_disjoint(&[root_path, nested_path]).is_err());
        let root_path =
            ResolvedPath::new(PathRole::Source, root.path().to_path_buf()).expect("resolved root");
        assert!(ResolvedPath::ensure_disjoint(&[root_path, sibling_path]).is_ok());
        temp.close().expect("close temporary root");
    }

    #[test]
    fn resolved_path_containment_rejects_candidates_outside_root() {
        let temp = assert_fs::TempDir::new().expect("temp dir");
        let root = temp.child("contracts");
        root.create_dir_all().expect("contracts root");
        let resolved_root = ResolvedPath::new(PathRole::Contracts, root.path().to_path_buf())
            .expect("resolved root");
        let inside = ResolvedPath::new(
            PathRole::GeneratedOutput,
            root.path().join("missing/output.yaml"),
        )
        .expect("inside path");
        let outside = ResolvedPath::new(
            PathRole::GeneratedOutput,
            temp.path().join("outside/output.yaml"),
        )
        .expect("outside path");

        assert!(resolved_root.ensure_within(&inside).is_ok());
        assert!(resolved_root.ensure_within(&outside).is_err());
        temp.close().expect("close temporary root");
    }

    #[cfg(unix)]
    #[test]
    fn resolved_paths_detect_symlink_aliases_and_escapes() {
        use std::os::unix::fs::symlink;

        let temp = assert_fs::TempDir::new().expect("temp dir");
        let contracts = temp.child("contracts");
        let outside = temp.child("outside");
        contracts.create_dir_all().expect("contracts");
        outside.create_dir_all().expect("outside");
        symlink(contracts.path(), temp.child("contracts-alias").path()).expect("root symlink");
        symlink(outside.path(), contracts.child("escape").path()).expect("escape symlink");
        let root = ResolvedPath::new(PathRole::Contracts, contracts.path().to_path_buf())
            .expect("resolved root");
        let alias = ResolvedPath::new(
            PathRole::Source,
            temp.child("contracts-alias").path().to_path_buf(),
        )
        .expect("resolved alias");

        assert!(ResolvedPath::ensure_disjoint(&[root, alias]).is_err());

        let root = ResolvedPath::new(PathRole::Contracts, contracts.path().to_path_buf())
            .expect("resolved root");
        let escaped = ResolvedPath::new(
            PathRole::GeneratedOutput,
            contracts.child("escape/output.yaml").path().to_path_buf(),
        )
        .expect("resolved escaped output");
        assert!(root.ensure_within(&escaped).is_err());
        temp.close().expect("close temporary root");
    }

    #[cfg(unix)]
    #[test]
    fn generated_paths_reject_symlink_descendants_and_final_entries() {
        use std::os::unix::fs::symlink;

        let temp = assert_fs::TempDir::new().expect("temporary root");
        let contracts = temp.child("contracts");
        let outside = temp.child("outside");
        contracts.create_dir_all().expect("contracts root");
        outside.create_dir_all().expect("outside directory");
        outside.child("external.yml").touch().expect("outside file");
        symlink(outside.path(), contracts.child("linked").path())
            .expect("descendant directory symlink");
        symlink(
            outside.child("external.yml").path(),
            contracts.child("linked.yml").path(),
        )
        .expect("final file symlink");

        let root = ResolvedPath::new(PathRole::Contracts, contracts.path().to_path_buf())
            .expect("resolved contracts root");

        assert!(
            root.ensure_generated_path(contracts.child("linked/main.yml").path())
                .is_err()
        );
        assert!(
            root.ensure_generated_path(contracts.child("linked.yml").path())
                .is_err()
        );
        temp.close().expect("close temporary root");
    }

    #[test]
    fn portable_catalog_keys_compare_ascii_case_insensitively() {
        let upper = CatalogPath::new("Main.YML").expect("upper path");
        let lower = CatalogPath::new("main.yml").expect("lower path");

        assert_eq!(
            PortableCatalogPathKey::new(&upper),
            PortableCatalogPathKey::new(&lower)
        );
    }

    #[test]
    fn host_component_comparison_matches_target_policy() {
        let upper = Path::new("Name")
            .components()
            .next()
            .expect("upper component");
        let lower = Path::new("name")
            .components()
            .next()
            .expect("lower component");

        #[cfg(windows)]
        assert!(ResolvedPath::components_match(upper, lower));
        #[cfg(not(windows))]
        assert!(!ResolvedPath::components_match(upper, lower));
    }
}
