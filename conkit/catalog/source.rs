//! Validated source-tree reads.

use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use conkit_signature::{CatalogPath, FileCatalog};
use walkdir::WalkDir;

use crate::error::CliError;
use crate::platform::PortablePathRules;

use super::path::{CatalogDirectory, PathRole, ResolvedPath};

/// Source-only filesystem capabilities.
#[derive(Debug)]
pub(crate) struct SourceTree {
    directory: CatalogDirectory,
}

impl SourceTree {
    /// Opens and validates a source root.
    pub(crate) fn open(path: PathBuf) -> Result<Self, CliError> {
        let directory = CatalogDirectory::source(path);
        directory.validate_directory()?;
        Ok(Self { directory })
    }

    /// Returns the selected source root.
    pub(crate) fn path(&self) -> &Path {
        self.directory.path()
    }

    /// Reads every Rust source file below this root in deterministic order.
    pub(crate) fn read_rust_sources(&self) -> Result<FileCatalog, CliError> {
        self.directory.validate_directory()?;

        let mut catalog = FileCatalog::new();
        for entry in WalkDir::new(self.directory.path())
            .follow_links(false)
            .sort_by_file_name()
        {
            let entry = entry?;
            if !entry.file_type().is_file()
                || !entry
                    .path()
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("rs"))
            {
                continue;
            }

            let logical = self.directory.logical_path(entry.path())?;
            catalog.insert(logical, fs_err::read(entry.path())?)?;
        }

        Ok(catalog)
    }

    /// Reads only the explicitly selected logical files below this root.
    ///
    /// Each selected path is checked for containment, opened once, checked
    /// again for valid components and containment, and compared with the
    /// current path identity. Its bytes are then read through that same opened
    /// handle.
    pub(crate) fn read_selected(&self, selected: &[CatalogPath]) -> Result<FileCatalog, CliError> {
        self.directory.validate_directory()?;
        let root = ResolvedPath::new(PathRole::Source, self.directory.path().to_path_buf())?;
        let mut catalog = FileCatalog::new();

        for logical in selected {
            let mut file = self.open_selected_file(logical, &root)?;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)
                .map_err(|source| CliError::ListedSourceUnavailable {
                    path: self.directory.path().join(logical.as_str()),
                    source,
                })?;
            catalog.insert(logical.clone(), bytes)?;
        }

        Ok(catalog)
    }

    fn open_selected_file(
        &self,
        logical: &CatalogPath,
        root: &ResolvedPath,
    ) -> Result<File, CliError> {
        let lexical = self.selected_path(logical)?;
        let before = ResolvedPath::new(PathRole::Source, lexical.clone())?;
        root.ensure_within(&before)?;

        let file = File::open(before.resolved_path()).map_err(|source| {
            CliError::ListedSourceUnavailable {
                path: lexical.clone(),
                source,
            }
        })?;
        if !file.metadata()?.is_file() {
            return Err(CliError::ListedSourceNotFile { path: lexical });
        }

        self.selected_path(logical)?;
        let after = ResolvedPath::new(PathRole::Source, lexical.clone())?;
        root.ensure_within(&after)?;
        let opened = same_file::Handle::from_file(file.try_clone()?)?;
        let current = same_file::Handle::from_path(&lexical)?;
        if opened != current {
            return Err(CliError::ListedSourceChanged { path: lexical });
        }

        Ok(file)
    }

    fn selected_path(&self, logical: &CatalogPath) -> Result<PathBuf, CliError> {
        let mut path = self.directory.path().to_path_buf();
        let mut components = logical.as_str().split('/').peekable();

        while let Some(component) = components.next() {
            PortablePathRules::validate_component(OsStr::new(component))?;
            path.push(component);
            let metadata = fs_err::symlink_metadata(&path).map_err(|source| {
                CliError::ListedSourceUnavailable {
                    path: path.clone(),
                    source,
                }
            })?;
            if metadata.file_type().is_symlink() {
                return Err(CliError::UnsupportedPathSymlink {
                    role: PathRole::Source,
                    path,
                });
            }

            let expected_type_matches = if components.peek().is_some() {
                metadata.is_dir()
            } else {
                metadata.is_file()
            };
            if !expected_type_matches {
                return Err(CliError::ListedSourceNotFile { path });
            }
        }

        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use assert_fs::prelude::*;
    use conkit_signature::CatalogPath;

    use super::SourceTree;
    use crate::catalog::{PathRole, ResolvedPath};

    #[test]
    fn source_tree_open_requires_an_existing_directory() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let missing = temp.child("missing");
        let file = temp.child("source.rs");
        file.touch().expect("source file");

        let missing_error = SourceTree::open(missing.path().to_path_buf())
            .expect_err("missing source root must fail");
        let file_error =
            SourceTree::open(file.path().to_path_buf()).expect_err("source file root must fail");

        assert!(missing_error.to_string().contains("is not a directory"));
        assert!(file_error.to_string().contains("is not a directory"));
        temp.close().expect("close temporary root");
    }

    #[test]
    fn selected_source_reads_only_declared_files() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        temp.child("listed.rs")
            .write_str("pub fn listed() {}\n")
            .expect("listed source");
        temp.child("ignored.rs")
            .write_str("pub fn ignored() {}\n")
            .expect("ignored source");

        let catalog = SourceTree::open(temp.path().to_path_buf())
            .expect("source tree")
            .read_selected(&[CatalogPath::new("listed.rs").expect("logical path")])
            .expect("selected catalog");

        assert_eq!(catalog.len(), 1);
        assert!(
            catalog
                .get(&CatalogPath::new("listed.rs").expect("logical path"))
                .is_some()
        );
        temp.close().expect("close temporary root");
    }

    #[test]
    fn selected_source_requires_every_declared_file() {
        let temp = assert_fs::TempDir::new().expect("temporary root");

        let error = SourceTree::open(temp.path().to_path_buf())
            .expect("source tree")
            .read_selected(&[CatalogPath::new("missing.rs").expect("logical path")])
            .expect_err("missing listed source must fail");

        assert!(
            error
                .to_string()
                .contains("listed source file is unavailable")
        );
        temp.close().expect("close temporary root");
    }

    #[test]
    fn selected_source_requires_a_final_regular_file() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        temp.child("nested")
            .create_dir_all()
            .expect("selected directory");

        let error = SourceTree::open(temp.path().to_path_buf())
            .expect("source tree")
            .read_selected(&[CatalogPath::new("nested").expect("logical path")])
            .expect_err("a selected directory is not a source file");

        assert!(error.to_string().contains("is not a regular file"));
        temp.close().expect("close temporary root");
    }

    #[cfg(unix)]
    #[test]
    fn read_selected_rejects_symlinked_ancestor() {
        use std::os::unix::fs::symlink;

        let temp = assert_fs::TempDir::new().expect("temporary root");
        let source = temp.child("source");
        let outside = temp.child("outside");
        let actual = source.child("actual");
        source.create_dir_all().expect("source root");
        outside.create_dir_all().expect("outside root");
        actual.create_dir_all().expect("actual source directory");
        outside
            .child("secret.rs")
            .write_str("pub fn secret() {}\n")
            .expect("outside source");
        actual
            .child("lib.rs")
            .write_str("pub fn inside() {}\n")
            .expect("inside source");
        symlink(outside.path(), source.child("external-link").path())
            .expect("escaping ancestor symlink");
        symlink("actual", source.child("internal-link").path()).expect("internal ancestor symlink");

        let tree = SourceTree::open(source.path().to_path_buf()).expect("source tree");
        let escaping = tree
            .read_selected(&[CatalogPath::new("external-link/secret.rs").expect("logical path")])
            .expect_err("a selected source must not follow an ancestor symlink");
        let internal = tree
            .read_selected(&[CatalogPath::new("internal-link/lib.rs").expect("logical path")])
            .expect_err("selected sources must not follow internal ancestor symlinks");

        assert!(escaping.to_string().contains("symbolic link"));
        assert!(internal.to_string().contains("symbolic link"));
        temp.close().expect("close temporary root");
    }

    #[cfg(unix)]
    #[test]
    fn read_selected_accepts_a_source_root_directory_symlink() {
        use std::os::unix::fs::symlink;

        let temp = assert_fs::TempDir::new().expect("temporary root");
        let actual = temp.child("actual source");
        let selected = temp.child("selected source");
        actual
            .child("nested")
            .create_dir_all()
            .expect("actual source directory");
        actual
            .child("nested/lib.rs")
            .write_str("pub fn linked_root() {}\n")
            .expect("source below actual root");
        symlink(actual.path(), selected.path()).expect("source root symlink");
        let logical = CatalogPath::new("nested/lib.rs").expect("logical path");

        let tree = SourceTree::open(selected.path().to_path_buf())
            .expect("source tree below root symlink");
        let catalog = tree
            .read_selected(std::slice::from_ref(&logical))
            .expect("selected source below root symlink");
        let walked = tree
            .read_rust_sources()
            .expect("Rust walk below root symlink");

        assert_eq!(
            catalog.get(&logical),
            Some(&b"pub fn linked_root() {}\n"[..])
        );
        assert_eq!(
            walked.get(&logical),
            Some(&b"pub fn linked_root() {}\n"[..])
        );
        temp.close().expect("close temporary root");
    }

    #[cfg(unix)]
    #[test]
    fn selected_source_rejects_a_final_component_symlink() {
        use std::os::unix::fs::symlink;

        let temp = assert_fs::TempDir::new().expect("temporary root");
        let source = temp.child("source");
        source.create_dir_all().expect("source root");
        source
            .child("actual.rs")
            .write_str("pub fn actual() {}\n")
            .expect("actual source");
        symlink("actual.rs", source.child("linked.rs").path()).expect("final source symlink");

        let error = SourceTree::open(source.path().to_path_buf())
            .expect("source tree")
            .read_selected(&[CatalogPath::new("linked.rs").expect("logical path")])
            .expect_err("selected sources must not follow final symlinks");

        assert!(error.to_string().contains("symbolic link"));
        temp.close().expect("close temporary root");
    }

    #[test]
    fn selected_source_reads_nested_paths_with_spaces_and_unicode() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let nested = temp.child("nested space");
        nested.create_dir_all().expect("nested source directory");
        nested
            .child("雪.rs")
            .write_str("pub fn snow() {}\n")
            .expect("Unicode source");
        let logical = CatalogPath::new("nested space/雪.rs").expect("logical path");

        let catalog = SourceTree::open(temp.path().to_path_buf())
            .expect("source tree")
            .read_selected(std::slice::from_ref(&logical))
            .expect("selected Unicode source");

        assert_eq!(catalog.get(&logical), Some(&b"pub fn snow() {}\n"[..]));
        temp.close().expect("close temporary root");
    }

    #[test]
    fn selected_source_failure_after_a_valid_entry_returns_no_catalog() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        temp.child("listed.rs")
            .write_str("pub fn listed() {}\n")
            .expect("listed source");
        let selected = [
            CatalogPath::new("listed.rs").expect("valid path"),
            CatalogPath::new("missing.rs").expect("missing path"),
        ];

        let result = SourceTree::open(temp.path().to_path_buf())
            .expect("source tree")
            .read_selected(&selected);

        assert!(result.is_err(), "a later failure must return no catalog");
        temp.close().expect("close temporary root");
    }

    #[test]
    fn selected_source_reads_from_the_verified_open_file() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let source = temp.child("source");
        source.create_dir_all().expect("source root");
        let selected = source.child("listed.rs");
        selected
            .write_str("pub fn original() {}\n")
            .expect("selected source");
        let tree = SourceTree::open(source.path().to_path_buf()).expect("source tree");
        let logical = CatalogPath::new("listed.rs").expect("logical path");
        let resolved =
            ResolvedPath::new(PathRole::Source, source.path().to_path_buf()).expect("source root");
        let mut opened = tree
            .open_selected_file(&logical, &resolved)
            .expect("verified source handle");

        std::fs::rename(selected.path(), source.child("original.rs").path())
            .expect("move selected source");
        selected
            .write_str("pub fn replacement() {}\n")
            .expect("replacement source");
        let mut bytes = Vec::new();
        opened.read_to_end(&mut bytes).expect("read opened source");

        assert_eq!(bytes, b"pub fn original() {}\n");
        temp.close().expect("close temporary root");
    }

    #[test]
    fn rust_source_read_is_deterministic_and_ignores_non_rust_files() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        temp.child("z.rs").touch().expect("Rust source");
        temp.child("a.RS").touch().expect("uppercase Rust source");
        temp.child("notes.txt").touch().expect("non-Rust source");

        let catalog = SourceTree::open(temp.path().to_path_buf())
            .expect("source tree")
            .read_rust_sources()
            .expect("Rust source catalog");
        let paths = catalog
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>();

        assert_eq!(paths, ["a.RS", "z.rs"]);
        temp.close().expect("close temporary root");
    }
}
