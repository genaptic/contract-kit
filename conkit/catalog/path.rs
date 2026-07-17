//! Path resolution and filesystem-to-catalog mapping.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::{ErrorKind, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

#[cfg(unix)]
use cap_fs_ext::OpenOptionsSyncExt as _;
use cap_fs_ext::{DirExt as _, FollowSymlinks, OpenOptionsFollowExt as _};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, DirEntry, File, OpenOptions};
use conkit_signature::CatalogPath;

use crate::error::CliError;
use crate::platform::PortablePathRules;

use super::CatalogReadBudget;

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
    /// be resolved, if a missing suffix begins below a non-directory ancestor,
    /// or if an unresolved existing component is a symlink.
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
        let mut missing_source = None;

        let resolved_ancestor = loop {
            match fs_err::canonicalize(&existing) {
                Ok(resolved) => {
                    if let Some(source) = missing_source.take() {
                        // Windows can report a child below a file as not found,
                        // so verify the ancestor before restoring the suffix.
                        let metadata = fs_err::metadata(&resolved).map_err(|source| {
                            CliError::PathResolution {
                                role,
                                path: path.clone(),
                                source,
                            }
                        })?;
                        if !metadata.is_dir() {
                            return Err(CliError::PathResolution { role, path, source });
                        }
                    }
                    break resolved;
                }
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
                    missing_source.get_or_insert(source);
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

/// Filesystem root capability shared by catalog owners.
#[derive(Debug)]
pub(super) struct CatalogDirectory {
    path: PathBuf,
    role: PathRole,
    root: Mutex<Option<Dir>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CatalogTraversal {
    Existing,
    CreateParents,
}

/// One final filesystem name bound to an already-opened parent capability.
///
/// The ambient root spelling is retained only for diagnostics. All reads and
/// mutations use `parent` plus `name`, so replacing the selected root path
/// after it has been opened cannot redirect the operation.
#[derive(Debug)]
pub(super) struct CatalogLeaf {
    parent: Dir,
    name: OsString,
    display_path: PathBuf,
    role: PathRole,
}

impl CatalogDirectory {
    /// Creates a source-root directory mapping.
    pub(super) fn source(path: PathBuf) -> Self {
        Self {
            path,
            role: PathRole::Source,
            root: Mutex::new(None),
        }
    }

    /// Creates a contracts-root directory mapping.
    pub(super) fn contracts(path: PathBuf) -> Self {
        Self {
            path,
            role: PathRole::Contracts,
            root: Mutex::new(None),
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
        self.capability().map(drop)
    }

    /// Returns a clone of the root capability, opening the selected ambient
    /// path only once. A symlink used as the selected root is followed at this
    /// one authority boundary; all descendant operations are handle-relative.
    pub(super) fn capability(&self) -> Result<Dir, CliError> {
        self.acquire_root(CatalogTraversal::Existing)
    }

    /// Returns whether the selected root is lexically absent and has not
    /// already been retained. A dangling root symlink is not absent.
    pub(super) fn is_lexically_absent(&self) -> Result<bool, CliError> {
        if self
            .root
            .lock()
            .map_err(|_| Self::capability_lock_error())?
            .is_some()
        {
            return Ok(false);
        }

        match fs_err::symlink_metadata(&self.path) {
            Ok(_) => Ok(false),
            Err(source) if source.kind() == ErrorKind::NotFound => Ok(true),
            Err(source) => Err(CliError::Io(source)),
        }
    }

    fn acquire_root(&self, traversal: CatalogTraversal) -> Result<Dir, CliError> {
        let mut retained = self
            .root
            .lock()
            .map_err(|_| Self::capability_lock_error())?;
        if let Some(root) = retained.as_ref() {
            return root.try_clone().map_err(CliError::from);
        }

        let open = Dir::open_ambient_dir(&self.path, ambient_authority());
        let root = match open {
            Ok(root) => root,
            Err(source)
                if traversal == CatalogTraversal::CreateParents
                    && source.kind() == ErrorKind::NotFound
                    && matches!(
                        fs_err::symlink_metadata(&self.path),
                        Err(ref metadata_source)
                            if metadata_source.kind() == ErrorKind::NotFound
                    ) =>
            {
                Dir::create_ambient_dir_all(&self.path, ambient_authority())?;
                Dir::open_ambient_dir(&self.path, ambient_authority())?
            }
            Err(source)
                if source.kind() == ErrorKind::NotFound
                    || source.kind() == ErrorKind::NotADirectory =>
            {
                return Err(CliError::RootIsNotDirectory {
                    role: self.role,
                    path: self.path.clone(),
                });
            }
            Err(source) => return Err(CliError::Io(source)),
        };
        let caller = root.try_clone()?;
        *retained = Some(root);
        Ok(caller)
    }

    fn capability_lock_error() -> CliError {
        CliError::Io(std::io::Error::other(
            "catalog root capability lock is poisoned",
        ))
    }

    /// Resolves one final name below existing no-follow directory components.
    ///
    /// Missing components retain the operating system error so callers can
    /// preserve exact diagnostics while deciding whether absence is optional.
    pub(super) fn existing_leaf(
        &self,
        relative: &Path,
        role: PathRole,
    ) -> Result<CatalogLeaf, CliError> {
        self.leaf(relative, role, CatalogTraversal::Existing)
    }

    /// Resolves one final name, creating absent parent directories relative to
    /// the retained root capability.
    pub(super) fn create_leaf(
        &self,
        relative: &Path,
        role: PathRole,
    ) -> Result<CatalogLeaf, CliError> {
        self.leaf(relative, role, CatalogTraversal::CreateParents)
    }

    fn leaf(
        &self,
        relative: &Path,
        role: PathRole,
        traversal: CatalogTraversal,
    ) -> Result<CatalogLeaf, CliError> {
        let mut components = relative.components().peekable();
        let mut parent = self.acquire_root(traversal)?;
        let mut opened_relative = PathBuf::new();

        while let Some(component) = components.next() {
            let Component::Normal(component) = component else {
                return Err(CliError::InvalidCatalogPath {
                    path: self.path.join(relative),
                });
            };
            PortablePathRules::validate_component(component)?;

            if components.peek().is_none() {
                return Ok(CatalogLeaf {
                    parent,
                    name: component.to_os_string(),
                    display_path: self.path.join(relative),
                    role,
                });
            }

            let next = match parent.symlink_metadata(component) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(CliError::UnsupportedPathSymlink {
                        role,
                        path: self.path.join(&opened_relative).join(component),
                    });
                }
                Ok(metadata) if !metadata.is_dir() => {
                    return Err(CliError::PathResolution {
                        role,
                        path: self.path.join(&opened_relative).join(component),
                        source: std::io::Error::new(
                            ErrorKind::InvalidInput,
                            "catalog path ancestor is not a directory",
                        ),
                    });
                }
                Ok(_) => parent.open_dir_nofollow(component)?,
                Err(source) if source.kind() == ErrorKind::NotFound => match traversal {
                    CatalogTraversal::Existing => return Err(CliError::Io(source)),
                    CatalogTraversal::CreateParents => {
                        self.create_directory_child(&parent, component, &opened_relative)?
                    }
                },
                Err(source) => return Err(CliError::Io(source)),
            };
            opened_relative.push(component);
            parent = next;
        }

        Err(CliError::InvalidCatalogPath {
            path: self.path.join(relative),
        })
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

    /// Enumerates one retained directory, accounting before allocation and
    /// sorting the bounded result by exact host spelling.
    pub(super) fn sorted_entries(
        &self,
        directory: &Dir,
        relative: &Path,
        budget: &mut CatalogReadBudget,
    ) -> Result<Vec<DirEntry>, CliError> {
        let display_directory = self.path.join(relative);
        let mut entries = Vec::new();
        for entry in directory.entries()? {
            budget.checkpoint()?;
            let entry = entry?;
            budget.visit_traversal_entry(&display_directory)?;
            entries.push(entry);
        }
        budget.checkpoint()?;
        entries.sort_by_key(|entry| entry.file_name());
        budget.checkpoint()?;
        Ok(entries)
    }

    /// Opens a discovered directory without following its entry if it is
    /// replaced by a symlink or reparse point.
    pub(super) fn open_directory_child(
        &self,
        parent: &Dir,
        name: &OsStr,
        relative: &Path,
    ) -> Result<Dir, CliError> {
        PortablePathRules::validate_component(name)?;
        let display_path = self.path.join(relative).join(name);
        let metadata = parent.symlink_metadata(name)?;
        if metadata.file_type().is_symlink() {
            return Err(CliError::UnsupportedPathSymlink {
                role: self.role,
                path: display_path,
            });
        }
        if !metadata.is_dir() {
            return Err(CliError::PathResolution {
                role: self.role,
                path: display_path,
                source: std::io::Error::new(
                    ErrorKind::InvalidInput,
                    "catalog path ancestor is not a directory",
                ),
            });
        }
        parent.open_dir_nofollow(name).map_err(CliError::from)
    }

    /// Opens one existing directory child or creates it relative to its
    /// already-open parent, never following a competing symlink.
    pub(super) fn create_directory_child(
        &self,
        parent: &Dir,
        name: &OsStr,
        relative: &Path,
    ) -> Result<Dir, CliError> {
        PortablePathRules::validate_component(name)?;
        match parent.symlink_metadata(name) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(CliError::UnsupportedPathSymlink {
                    role: self.role,
                    path: self.path.join(relative).join(name),
                });
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(CliError::PathResolution {
                    role: self.role,
                    path: self.path.join(relative).join(name),
                    source: std::io::Error::new(
                        ErrorKind::InvalidInput,
                        "generated path ancestor is not a directory",
                    ),
                });
            }
            Ok(_) => {}
            Err(source) if source.kind() == ErrorKind::NotFound => match parent.create_dir(name) {
                Ok(()) => {}
                Err(create_source) if create_source.kind() == ErrorKind::AlreadyExists => {}
                Err(create_source) => return Err(CliError::Io(create_source)),
            },
            Err(source) => return Err(CliError::Io(source)),
        }
        self.open_directory_child(parent, name, relative)
    }

    /// Anchors a name returned by a capability-relative directory traversal.
    pub(super) fn discovered_leaf(
        &self,
        parent: &Dir,
        name: &OsStr,
        relative: &Path,
    ) -> Result<CatalogLeaf, CliError> {
        PortablePathRules::validate_component(name)?;
        Ok(CatalogLeaf {
            parent: parent.try_clone()?,
            name: name.to_os_string(),
            display_path: self.path.join(relative).join(name),
            role: self.role,
        })
    }
}

impl CatalogLeaf {
    /// Returns the diagnostic-only ambient spelling for this anchored name.
    pub(super) fn display_path(&self) -> &Path {
        &self.display_path
    }

    /// Duplicates the parent capability while preserving the same anchored
    /// filesystem directory.
    pub(super) fn try_clone(&self) -> Result<Self, CliError> {
        Ok(Self {
            parent: self.parent.try_clone()?,
            name: self.name.clone(),
            display_path: self.display_path.clone(),
            role: self.role,
        })
    }

    /// Returns the opened parent directory's entry names in deterministic order.
    pub(super) fn sorted_sibling_names(
        &self,
        budget: &mut CatalogReadBudget,
    ) -> Result<Vec<OsString>, CliError> {
        let directory = self.display_path.parent().unwrap_or_else(|| Path::new(""));
        let mut names = Vec::new();
        for entry in self.parent.entries()? {
            budget.checkpoint()?;
            budget.visit_traversal_entry(directory)?;
            let entry = entry?;
            names.push(entry.file_name());
        }
        budget.checkpoint()?;
        names.sort();
        budget.checkpoint()?;
        Ok(names)
    }

    /// Opens this final name as a regular file without following it.
    ///
    /// Descendant directory components were already opened no-follow when this
    /// value was built. A missing final name remains the original I/O error so
    /// the owning workflow can either surface it or treat it as optional.
    pub(super) fn open_regular(&self) -> Result<Option<File>, CliError> {
        let metadata = match self.parent.symlink_metadata(&self.name) {
            Ok(metadata) => metadata,
            Err(source) => return Err(CliError::Io(source)),
        };
        if metadata.file_type().is_symlink() {
            return Err(CliError::UnsupportedPathSymlink {
                role: self.role,
                path: self.display_path.clone(),
            });
        }
        if !metadata.is_file() {
            return Err(self.not_regular_error());
        }

        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        #[cfg(unix)]
        options.nonblock(true);
        let file = self.parent.open_with(&self.name, &options)?;
        if !file.metadata()?.is_file() {
            return Err(self.not_regular_error());
        }
        Ok(Some(file))
    }

    /// Opens or creates the regular read/write file used for the generation
    /// lock and converts that same capability-opened handle for standard file
    /// locking.
    pub(super) fn open_lock_file(&self) -> Result<std::fs::File, CliError> {
        match self.parent.symlink_metadata(&self.name) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(self.not_regular_error());
            }
            Ok(_) => {}
            Err(source) if source.kind() == ErrorKind::NotFound => {}
            Err(source) => return Err(CliError::Io(source)),
        }

        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .follow(FollowSymlinks::No);
        let file = self.parent.open_with(&self.name, &options)?;
        if !file.metadata()?.is_file() {
            return Err(self.not_regular_error());
        }
        Ok(file.into_std())
    }

    /// Creates this final name exclusively, writes and synchronizes all bytes,
    /// and removes only that same anchored name if the write fails.
    pub(super) fn write_new_synced(&self, bytes: &[u8]) -> Result<(), CliError> {
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        let mut file = self.parent.open_with(&self.name, &options)?;
        if !file.metadata()?.is_file() {
            return Err(self.not_regular_error());
        }
        if let Err(source) = file
            .write_all(bytes)
            .and_then(|()| file.flush())
            .and_then(|()| file.sync_all())
        {
            drop(file);
            return match self.parent.remove_file(&self.name) {
                Ok(()) => Err(CliError::Io(source)),
                Err(cleanup) if cleanup.kind() == ErrorKind::NotFound => Err(CliError::Io(source)),
                Err(cleanup) => Err(CliError::Io(std::io::Error::new(
                    source.kind(),
                    format!("{source}; created-file cleanup also failed: {cleanup}"),
                ))),
            };
        }
        Ok(())
    }

    /// Removes this anchored final name and synchronizes its parent where the
    /// platform supports directory synchronization.
    pub(super) fn remove_file(&self) -> Result<(), CliError> {
        self.parent.remove_file(&self.name)?;
        self.sync_parent()
    }

    /// Opens a collision-resistant sibling temporary with caller-selected
    /// no-follow options.
    pub(super) fn open_sibling(
        &self,
        name: &OsStr,
        options: &OpenOptions,
    ) -> Result<File, CliError> {
        PortablePathRules::validate_component(name)?;
        self.parent.open_with(name, options).map_err(CliError::from)
    }

    /// Atomically renames one sibling over this anchored final name.
    pub(super) fn rename_sibling(&self, sibling: &OsStr) -> Result<(), CliError> {
        self.parent
            .rename(sibling, &self.parent, &self.name)
            .map_err(CliError::from)
    }

    /// Removes one sibling temporary from this anchored parent.
    pub(super) fn remove_sibling(&self, sibling: &OsStr) -> Result<(), std::io::Error> {
        self.parent.remove_file(sibling)
    }

    /// Synchronizes the parent directory where the platform supports it.
    pub(super) fn sync_parent(&self) -> Result<(), CliError> {
        #[cfg(unix)]
        self.parent.try_clone()?.into_std_file().sync_all()?;
        Ok(())
    }

    /// Returns the final name for temporary-name derivation.
    pub(super) fn name(&self) -> &OsStr {
        &self.name
    }

    fn not_regular_error(&self) -> CliError {
        if self.role == PathRole::GeneratedOutput {
            CliError::GeneratedOutputNotFile {
                path: self.display_path.clone(),
            }
        } else {
            CliError::PathResolution {
                role: self.role,
                path: self.display_path.clone(),
                source: std::io::Error::new(
                    ErrorKind::InvalidInput,
                    "catalog path is not a regular file",
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;
    use std::path::{Path, PathBuf};

    use assert_fs::prelude::*;
    use conkit_signature::CatalogPath;

    use crate::error::CliError;

    use super::{CatalogDirectory, PathRole, PortableCatalogPathKey, ResolvedPath};

    #[test]
    fn leaf_boundary_distinguishes_existing_create_and_final_absence() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let root = temp.child("contracts");
        root.create_dir_all().expect("contracts root");
        let directory = CatalogDirectory::contracts(root.path().to_path_buf());

        let missing_ancestor = directory
            .existing_leaf(Path::new("nested/main.yml"), PathRole::Contracts)
            .expect_err("missing ancestor");
        assert!(
            matches!(missing_ancestor, CliError::Io(ref source) if source.kind() == std::io::ErrorKind::NotFound)
        );

        let leaf = directory
            .create_leaf(Path::new("nested/main.yml"), PathRole::GeneratedOutput)
            .expect("create parent capabilities");
        let missing_final = leaf.open_regular().expect_err("missing final name");
        assert!(
            matches!(missing_final, CliError::Io(ref source) if source.kind() == std::io::ErrorKind::NotFound)
        );
        leaf.write_new_synced(b"contract\n")
            .expect("create regular final name");
        let mut file = leaf
            .open_regular()
            .expect("open final name")
            .expect("created final name exists");
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).expect("read opened leaf");
        assert_eq!(bytes, b"contract\n");
        drop(file);
        drop(leaf);
        drop(directory);
        temp.close().expect("close temporary root");
    }

    #[test]
    fn leaf_boundary_rejects_non_directory_ancestors_and_non_regular_finals() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let root = temp.child("contracts");
        root.create_dir_all().expect("contracts root");
        root.child("ancestor").touch().expect("file ancestor");
        root.child("directory.yml")
            .create_dir_all()
            .expect("directory final name");
        let directory = CatalogDirectory::contracts(root.path().to_path_buf());

        let ancestor = directory
            .existing_leaf(Path::new("ancestor/main.yml"), PathRole::Contracts)
            .expect_err("file ancestor");
        assert!(matches!(ancestor, CliError::PathResolution { .. }));

        let final_name = directory
            .existing_leaf(Path::new("directory.yml"), PathRole::Contracts)
            .expect("resolve final directory")
            .open_regular()
            .expect_err("directory is not a regular file");
        assert!(matches!(final_name, CliError::PathResolution { .. }));
        drop(directory);
        temp.close().expect("close temporary root");
    }

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

    #[cfg(unix)]
    #[test]
    fn verified_catalog_open_rejects_a_file_replaced_by_a_symlink_after_discovery() {
        use std::os::unix::fs::symlink;

        let temp = assert_fs::TempDir::new().expect("temporary root");
        let root = temp.child("catalog");
        let outside = temp.child("outside.yml");
        let discovered = root.child("main.yml");
        root.create_dir_all().expect("catalog root");
        discovered.write_str("inside\n").expect("discovered file");
        outside.write_str("outside\n").expect("outside file");
        assert!(
            std::fs::symlink_metadata(discovered.path())
                .expect("discovery metadata")
                .is_file(),
            "the test must discover a regular file before replacement"
        );

        std::fs::rename(discovered.path(), root.child("original.yml").path())
            .expect("move discovered file");
        symlink(outside.path(), discovered.path()).expect("replacement symlink");

        let directory = CatalogDirectory::contracts(root.path().to_path_buf());
        let error = directory
            .existing_leaf(Path::new("main.yml"), PathRole::Contracts)
            .expect("resolve final name")
            .open_regular()
            .expect_err("a replacement symlink must never be followed");

        assert!(error.to_string().contains("symbolic link"));
        temp.close().expect("close temporary root");
    }

    #[cfg(unix)]
    #[test]
    fn verified_catalog_open_rejects_an_ancestor_replaced_by_a_symlink_after_discovery() {
        use std::os::unix::fs::symlink;

        let temp = assert_fs::TempDir::new().expect("temporary root");
        let root = temp.child("catalog");
        let nested = root.child("nested");
        let discovered = nested.child("main.yml");
        let outside = temp.child("outside");
        root.create_dir_all().expect("catalog root");
        nested.create_dir_all().expect("nested directory");
        discovered.write_str("inside\n").expect("discovered file");
        outside.create_dir_all().expect("outside directory");
        outside
            .child("main.yml")
            .write_str("outside\n")
            .expect("outside file");
        assert!(
            std::fs::symlink_metadata(discovered.path())
                .expect("discovery metadata")
                .is_file(),
            "the test must discover a regular file before ancestor replacement"
        );

        std::fs::rename(nested.path(), root.child("original-nested").path())
            .expect("move discovered ancestor");
        symlink(outside.path(), nested.path()).expect("replacement ancestor symlink");

        let directory = CatalogDirectory::contracts(root.path().to_path_buf());
        let error = directory
            .existing_leaf(Path::new("nested/main.yml"), PathRole::Contracts)
            .expect_err("a replacement ancestor symlink must never be followed");

        assert!(error.to_string().contains("symbolic link"));
        temp.close().expect("close temporary root");
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
    fn resolved_paths_reject_a_missing_suffix_below_a_file() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let blocking_file = temp.child("blocked");
        blocking_file
            .write_str("not a directory")
            .expect("blocking file");
        let report = blocking_file.path().join("report.yml");

        let error = ResolvedPath::new(PathRole::Report, report.clone())
            .expect_err("a file cannot be a path ancestor");
        let CliError::PathResolution { role, path, .. } = error else {
            panic!("expected path resolution error, got {error}");
        };

        assert_eq!(role, PathRole::Report);
        assert_eq!(path, report);
        temp.close().expect("close temporary root");
    }

    #[cfg(unix)]
    #[test]
    fn resolved_paths_detect_symlink_aliases() {
        use std::os::unix::fs::symlink;

        let temp = assert_fs::TempDir::new().expect("temp dir");
        let contracts = temp.child("contracts");
        contracts.create_dir_all().expect("contracts");
        symlink(contracts.path(), temp.child("contracts-alias").path()).expect("root symlink");
        let root = ResolvedPath::new(PathRole::Contracts, contracts.path().to_path_buf())
            .expect("resolved root");
        let alias = ResolvedPath::new(
            PathRole::Source,
            temp.child("contracts-alias").path().to_path_buf(),
        )
        .expect("resolved alias");

        assert!(ResolvedPath::ensure_disjoint(&[root, alias]).is_err());
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
