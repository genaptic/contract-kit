//! Validated source-tree reads.

use std::path::{Path, PathBuf};

use cap_std::fs::{Dir, File};
use conkit_signature::{CatalogPath, FileCatalog};

use crate::error::CliError;

use super::path::{CatalogDirectory, PathRole};
use super::{CatalogReadBudget, CatalogReadLimits};

/// Source-only filesystem capabilities.
#[derive(Debug)]
pub(crate) struct SourceTree {
    directory: CatalogDirectory,
    limits: CatalogReadLimits,
}

impl SourceTree {
    /// Opens and validates a source root.
    ///
    /// # Errors
    ///
    /// Returns an error if the path cannot be inspected or does not resolve to
    /// an existing directory.
    pub(crate) fn open(path: PathBuf) -> Result<Self, CliError> {
        let directory = CatalogDirectory::source(path);
        directory.validate_directory()?;
        Ok(Self {
            directory,
            limits: CatalogReadLimits::default(),
        })
    }

    /// Replaces the filesystem catalog budgets for this source tree.
    pub(crate) fn with_limits(mut self, limits: CatalogReadLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Returns the selected source root.
    pub(crate) fn path(&self) -> &Path {
        self.directory.path()
    }

    /// Reads every Rust source against a caller-owned operation budget.
    ///
    /// # Errors
    ///
    /// Returns an error if the source root or a participating descendant cannot
    /// be traversed and read securely, cancellation is requested, a catalog
    /// limit is exceeded, or a logical source path cannot be represented.
    pub(crate) fn read_rust_sources_with_budget(
        &self,
        budget: &mut CatalogReadBudget,
    ) -> Result<FileCatalog, CliError> {
        let root = self.directory.capability()?;
        let mut catalog = FileCatalog::new();
        self.read_rust_directory(&root, Path::new(""), budget, &mut catalog)?;

        Ok(catalog)
    }

    fn read_rust_directory(
        &self,
        directory: &Dir,
        relative: &Path,
        budget: &mut CatalogReadBudget,
        catalog: &mut FileCatalog,
    ) -> Result<(), CliError> {
        for entry in self.directory.sorted_entries(directory, relative, budget)? {
            budget.checkpoint()?;
            let name = entry.file_name();
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                continue;
            }

            if file_type.is_dir() {
                let child = self
                    .directory
                    .open_directory_child(directory, &name, relative)?;
                let child_relative = relative.join(&name);
                self.read_rust_directory(&child, &child_relative, budget, catalog)?;
                continue;
            }

            if !file_type.is_file()
                || !Path::new(&name)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("rs"))
            {
                continue;
            }

            let entry_relative = relative.join(&name);
            let physical = self.directory.path().join(&entry_relative);
            budget.begin_entry(&physical)?;
            let logical = self.directory.logical_path(&physical)?;
            let leaf = self.directory.discovered_leaf(directory, &name, relative)?;
            let mut file = leaf
                .open_regular()?
                .ok_or_else(|| CliError::Io(std::io::ErrorKind::NotFound.into()))?;
            budget.preflight_file(&physical, file.metadata()?.len())?;
            let bytes = budget.read_file(&physical, &mut file)?;
            catalog.insert(logical, bytes)?;
        }
        Ok(())
    }

    /// Reads an exact source allowlist against a caller-owned operation budget.
    ///
    /// # Errors
    ///
    /// Returns an error on cancellation or a catalog-limit breach, or when an
    /// allowlisted source cannot be securely opened, read, or inserted into the
    /// logical catalog.
    pub(crate) fn read_selected_with_budget(
        &self,
        selected: &[CatalogPath],
        budget: &mut CatalogReadBudget,
    ) -> Result<FileCatalog, CliError> {
        let mut catalog = FileCatalog::new();

        for logical in selected {
            budget.checkpoint()?;
            let physical = self.directory.path().join(logical.as_str());
            budget.begin_entry(&physical)?;
            let mut file = self.open_selected_file(logical)?;
            budget.preflight_file(&physical, file.metadata()?.len())?;
            let bytes = budget
                .read_file(&physical, &mut file)
                .map_err(|error| match error {
                    CliError::Io(source) => CliError::ListedSourceUnavailable {
                        path: physical.clone(),
                        source,
                    },
                    error => error,
                })?;
            catalog.insert(logical.clone(), bytes)?;
        }

        Ok(catalog)
    }

    /// Stream-compares the selected files with a previously read source
    /// snapshot without constructing a second catalog.
    ///
    /// # Errors
    ///
    /// Returns an error on cancellation or a catalog-limit breach, or when an
    /// expected source cannot be securely opened or read for comparison.
    pub(crate) fn first_changed_snapshot_with_budget(
        &self,
        expected: &FileCatalog,
        budget: &mut CatalogReadBudget,
    ) -> Result<Option<CatalogPath>, CliError> {
        for (logical, expected_bytes) in expected.iter() {
            budget.checkpoint()?;
            let physical = self.directory.path().join(logical.as_str());
            budget.begin_entry(&physical)?;
            let mut file = self.open_selected_file(logical)?;
            budget.preflight_file(&physical, file.metadata()?.len())?;
            let matches = budget
                .compare_file(&physical, &mut file, expected_bytes)
                .map_err(|error| match error {
                    CliError::Io(source) => CliError::ListedSourceUnavailable {
                        path: physical.clone(),
                        source,
                    },
                    error => error,
                })?;
            if !matches {
                return Ok(Some(logical.clone()));
            }
        }

        Ok(None)
    }

    fn open_selected_file(&self, logical: &CatalogPath) -> Result<File, CliError> {
        let physical = self.directory.path().join(logical.as_str());
        let leaf = self
            .directory
            .existing_leaf(Path::new(logical.as_str()), PathRole::Source)
            .map_err(|error| match error {
                CliError::Io(source) => CliError::ListedSourceUnavailable {
                    path: physical.clone(),
                    source,
                },
                CliError::PathResolution { source, .. }
                    if source.kind() == std::io::ErrorKind::InvalidInput =>
                {
                    CliError::ListedSourceNotFile {
                        path: physical.clone(),
                    }
                }
                error => error,
            })?;
        match leaf.open_regular() {
            Ok(Some(file)) => Ok(file),
            Ok(None) => Err(CliError::ListedSourceUnavailable {
                path: physical,
                source: std::io::Error::from(std::io::ErrorKind::NotFound),
            }),
            Err(CliError::Io(source)) => Err(CliError::ListedSourceUnavailable {
                path: physical,
                source,
            }),
            Err(CliError::PathResolution { source, .. })
                if source.kind() == std::io::ErrorKind::InvalidInput =>
            {
                Err(CliError::ListedSourceNotFile { path: physical })
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};

    use assert_fs::prelude::*;
    use conkit_signature::CatalogPath;

    use super::SourceTree;
    use crate::catalog::{CatalogReadLimitResource, CatalogReadLimits};
    use crate::error::CliError;

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

        let tree = SourceTree::open(temp.path().to_path_buf()).expect("source tree");
        let cancellation = crate::context::ApplicationCancellation::new();
        let mut budget = tree.limits.begin(&cancellation);
        let catalog = tree
            .read_selected_with_budget(
                &[CatalogPath::new("listed.rs").expect("logical path")],
                &mut budget,
            )
            .expect("selected catalog");

        assert_eq!(catalog.len(), 1);
        assert!(
            catalog
                .get(&CatalogPath::new("listed.rs").expect("logical path"))
                .is_some()
        );
        drop(tree);
        temp.close().expect("close temporary root");
    }

    #[test]
    fn selected_source_requires_every_declared_file() {
        let temp = assert_fs::TempDir::new().expect("temporary root");

        let tree = SourceTree::open(temp.path().to_path_buf()).expect("source tree");
        let cancellation = crate::context::ApplicationCancellation::new();
        let mut budget = tree.limits.begin(&cancellation);
        let error = tree
            .read_selected_with_budget(
                &[CatalogPath::new("missing.rs").expect("logical path")],
                &mut budget,
            )
            .expect_err("missing listed source must fail");

        assert!(
            error
                .to_string()
                .contains("listed source file is unavailable")
        );
        drop(tree);
        temp.close().expect("close temporary root");
    }

    #[test]
    fn selected_source_requires_a_final_regular_file() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        temp.child("nested")
            .create_dir_all()
            .expect("selected directory");

        let tree = SourceTree::open(temp.path().to_path_buf()).expect("source tree");
        let cancellation = crate::context::ApplicationCancellation::new();
        let mut budget = tree.limits.begin(&cancellation);
        let error = tree
            .read_selected_with_budget(
                &[CatalogPath::new("nested").expect("logical path")],
                &mut budget,
            )
            .expect_err("a selected directory is not a source file");

        assert!(error.to_string().contains("is not a regular file"));
        drop(tree);
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
        let cancellation = crate::context::ApplicationCancellation::new();
        let mut budget = tree.limits.begin(&cancellation);
        let escaping = tree
            .read_selected_with_budget(
                &[CatalogPath::new("external-link/secret.rs").expect("logical path")],
                &mut budget,
            )
            .expect_err("a selected source must not follow an ancestor symlink");
        let internal = tree
            .read_selected_with_budget(
                &[CatalogPath::new("internal-link/lib.rs").expect("logical path")],
                &mut budget,
            )
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
        let cancellation = crate::context::ApplicationCancellation::new();
        let mut budget = tree.limits.begin(&cancellation);
        let catalog = tree
            .read_selected_with_budget(std::slice::from_ref(&logical), &mut budget)
            .expect("selected source below root symlink");
        let walked = tree
            .read_rust_sources_with_budget(&mut budget)
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
    fn source_root_capability_survives_symlink_retargeting() {
        use std::os::unix::fs::symlink;

        let temp = assert_fs::TempDir::new().expect("temporary root");
        let original = temp.child("original source");
        let replacement = temp.child("replacement source");
        let selected = temp.child("selected source");
        original.create_dir_all().expect("original source root");
        replacement
            .create_dir_all()
            .expect("replacement source root");
        original
            .child("lib.rs")
            .write_str("pub fn original() {}\n")
            .expect("original source");
        replacement
            .child("lib.rs")
            .write_str("pub fn replacement() {}\n")
            .expect("replacement source");
        symlink(original.path(), selected.path()).expect("selected root symlink");

        let tree = SourceTree::open(selected.path().to_path_buf())
            .expect("source tree anchors the selected root");
        std::fs::remove_file(selected.path()).expect("remove selected root symlink");
        symlink(replacement.path(), selected.path()).expect("retarget selected root symlink");

        let logical = CatalogPath::new("lib.rs").expect("logical path");
        let cancellation = crate::context::ApplicationCancellation::new();
        let mut budget = tree.limits.begin(&cancellation);
        let selected_catalog = tree
            .read_selected_with_budget(std::slice::from_ref(&logical), &mut budget)
            .expect("selected read remains anchored");
        let walked_catalog = tree
            .read_rust_sources_with_budget(&mut budget)
            .expect("walk remains anchored");

        assert_eq!(
            selected_catalog.get(&logical),
            Some(&b"pub fn original() {}\n"[..])
        );
        assert_eq!(
            walked_catalog.get(&logical),
            Some(&b"pub fn original() {}\n"[..])
        );
        temp.close().expect("close temporary root");
    }

    #[cfg(unix)]
    #[test]
    fn source_root_capability_survives_directory_rename_and_replacement() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let selected = temp.child("source");
        let anchored = temp.child("anchored source");
        selected.create_dir_all().expect("selected source root");
        selected
            .child("lib.rs")
            .write_str("pub fn original() {}\n")
            .expect("original source");

        let tree = SourceTree::open(selected.path().to_path_buf())
            .expect("source tree anchors the selected root");
        std::fs::rename(selected.path(), anchored.path()).expect("rename anchored root");
        selected.create_dir_all().expect("replacement source root");
        selected
            .child("lib.rs")
            .write_str("pub fn replacement() {}\n")
            .expect("replacement source");

        let logical = CatalogPath::new("lib.rs").expect("logical path");
        let cancellation = crate::context::ApplicationCancellation::new();
        let mut budget = tree.limits.begin(&cancellation);
        let catalog = tree
            .read_selected_with_budget(std::slice::from_ref(&logical), &mut budget)
            .expect("read remains anchored after root rename");

        assert_eq!(catalog.get(&logical), Some(&b"pub fn original() {}\n"[..]));
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

        let tree = SourceTree::open(source.path().to_path_buf()).expect("source tree");
        let cancellation = crate::context::ApplicationCancellation::new();
        let mut budget = tree.limits.begin(&cancellation);
        let error = tree
            .read_selected_with_budget(
                &[CatalogPath::new("linked.rs").expect("logical path")],
                &mut budget,
            )
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

        let tree = SourceTree::open(temp.path().to_path_buf()).expect("source tree");
        let cancellation = crate::context::ApplicationCancellation::new();
        let mut budget = tree.limits.begin(&cancellation);
        let catalog = tree
            .read_selected_with_budget(std::slice::from_ref(&logical), &mut budget)
            .expect("selected Unicode source");

        assert_eq!(catalog.get(&logical), Some(&b"pub fn snow() {}\n"[..]));
        drop(tree);
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

        let tree = SourceTree::open(temp.path().to_path_buf()).expect("source tree");
        let cancellation = crate::context::ApplicationCancellation::new();
        let mut budget = tree.limits.begin(&cancellation);
        let result = tree.read_selected_with_budget(&selected, &mut budget);

        assert!(result.is_err(), "a later failure must return no catalog");
        drop(tree);
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
        let mut opened = tree
            .open_selected_file(&logical)
            .expect("verified source handle");

        std::fs::rename(selected.path(), source.child("original.rs").path())
            .expect("move selected source");
        selected
            .write_str("pub fn replacement() {}\n")
            .expect("replacement source");
        let mut bytes = Vec::new();
        opened.read_to_end(&mut bytes).expect("read opened source");

        assert_eq!(bytes, b"pub fn original() {}\n");
        drop(opened);
        drop(tree);
        temp.close().expect("close temporary root");
    }

    #[test]
    fn opened_source_growth_is_bounded_to_one_evidence_byte() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let source = temp.child("source");
        source.create_dir_all().expect("source root");
        let selected = source.child("listed.rs");
        selected.write_binary(b"four").expect("selected source");
        let tree = SourceTree::open(source.path().to_path_buf()).expect("source tree");
        let logical = CatalogPath::new("listed.rs").expect("logical path");
        let mut opened = tree
            .open_selected_file(&logical)
            .expect("verified source handle");
        let mut budget =
            CatalogReadLimits::new(1, 64, 4).begin(&crate::context::ApplicationCancellation::new());
        budget.begin_entry(selected.path()).expect("entry budget");
        budget
            .preflight_file(selected.path(), opened.metadata().expect("metadata").len())
            .expect("metadata is initially within the limit");

        std::fs::OpenOptions::new()
            .append(true)
            .open(selected.path())
            .expect("open growing file")
            .write_all(b"more")
            .expect("grow selected source");
        let mut bytes = Vec::new();
        (&mut opened)
            .take(budget.read_limit())
            .read_to_end(&mut bytes)
            .expect("bounded handle read");
        let error = budget
            .finish_file(selected.path(), bytes.len())
            .expect_err("growth after metadata must exceed the file budget");

        assert_eq!(bytes, b"fourm");
        assert_eq!(error.resource, CatalogReadLimitResource::FileBytes);
        assert_eq!(error.observed_at_least, 5);
        drop(opened);
        drop(tree);
        temp.close().expect("close temporary root");
    }

    #[test]
    fn rust_source_read_is_deterministic_and_ignores_non_rust_files() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        temp.child("z.rs").touch().expect("Rust source");
        temp.child("a.RS").touch().expect("uppercase Rust source");
        temp.child("notes.txt").touch().expect("non-Rust source");

        let tree = SourceTree::open(temp.path().to_path_buf()).expect("source tree");
        let cancellation = crate::context::ApplicationCancellation::new();
        let mut budget = tree.limits.begin(&cancellation);
        let catalog = tree
            .read_rust_sources_with_budget(&mut budget)
            .expect("Rust source catalog");
        let paths = catalog
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>();

        assert_eq!(paths, ["a.RS", "z.rs"]);
        drop(tree);
        temp.close().expect("close temporary root");
    }

    #[test]
    fn selected_source_limits_ignore_unlisted_files_and_name_the_limited_path() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        temp.child("listed.rs")
            .write_binary(b"four")
            .expect("listed source");
        temp.child("ignored.rs")
            .write_binary(b"this unlisted file exceeds every byte budget")
            .expect("unlisted source");
        let listed = CatalogPath::new("listed.rs").expect("listed path");
        let tree = SourceTree::open(temp.path().to_path_buf())
            .expect("source tree")
            .with_limits(CatalogReadLimits::new(1, 4, 4));

        let cancellation = crate::context::ApplicationCancellation::new();
        let mut budget = tree.limits.begin(&cancellation);
        let catalog = tree
            .read_selected_with_budget(std::slice::from_ref(&listed), &mut budget)
            .expect("only the allowlisted source participates");
        assert_eq!(catalog.get(&listed), Some(&b"four"[..]));

        temp.child("listed.rs")
            .write_binary(b"five!")
            .expect("oversized listed source");
        let mut budget = tree.limits.begin(&cancellation);
        let error = tree
            .read_selected_with_budget(std::slice::from_ref(&listed), &mut budget)
            .expect_err("the listed file must obey its byte limit");
        let CliError::CatalogReadLimit(error) = error else {
            panic!("expected typed catalog read limit")
        };
        assert_eq!(error.resource, CatalogReadLimitResource::FileBytes);
        assert_eq!(error.limit, 4);
        assert_eq!(error.observed_at_least, 5);
        assert_eq!(error.path, temp.child("listed.rs").path());
        drop(tree);
        temp.close().expect("close temporary root");
    }

    #[test]
    fn rust_source_limits_stop_in_deterministic_entry_then_total_order() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        temp.child("a.rs").write_binary(b"aaa").expect("a source");
        temp.child("b.rs").write_binary(b"bbb").expect("b source");

        let entry_tree = SourceTree::open(temp.path().to_path_buf())
            .expect("source tree")
            .with_limits(CatalogReadLimits::new(1, 64, 64));
        let cancellation = crate::context::ApplicationCancellation::new();
        let mut budget = entry_tree.limits.begin(&cancellation);
        let entry_error = entry_tree
            .read_rust_sources_with_budget(&mut budget)
            .expect_err("the second sorted Rust entry must exceed the entry budget");
        let CliError::CatalogReadLimit(entry_error) = entry_error else {
            panic!("expected typed entry limit")
        };
        assert_eq!(entry_error.resource, CatalogReadLimitResource::EntryCount);
        assert_eq!(entry_error.observed_at_least, 2);
        assert_eq!(entry_error.path, temp.child("b.rs").path());

        let total_tree = SourceTree::open(temp.path().to_path_buf())
            .expect("source tree")
            .with_limits(CatalogReadLimits::new(2, 5, 64));
        let mut budget = total_tree.limits.begin(&cancellation);
        let total_error = total_tree
            .read_rust_sources_with_budget(&mut budget)
            .expect_err("the second sorted Rust entry must exceed total bytes");
        let CliError::CatalogReadLimit(total_error) = total_error else {
            panic!("expected typed total-byte limit")
        };
        assert_eq!(total_error.resource, CatalogReadLimitResource::TotalBytes);
        assert_eq!(total_error.limit, 5);
        assert_eq!(total_error.observed_at_least, 6);
        assert_eq!(total_error.path, temp.child("b.rs").path());
        drop(total_tree);
        drop(entry_tree);
        temp.close().expect("close temporary root");
    }

    #[test]
    fn ignored_entries_still_obey_the_traversal_budget() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        temp.child("a.txt").touch().expect("ignored entry a");
        temp.child("b.txt").touch().expect("ignored entry b");
        temp.child("c.txt").touch().expect("ignored entry c");
        temp.child("lib.rs")
            .touch()
            .expect("participating Rust source");

        let limits = CatalogReadLimits {
            traversal_entry_count: 2,
            ..CatalogReadLimits::new(10, 64, 64)
        };
        let tree = SourceTree::open(temp.path().to_path_buf())
            .expect("source tree")
            .with_limits(limits);
        let cancellation = crate::context::ApplicationCancellation::new();
        let mut budget = limits.begin(&cancellation);
        let error = tree
            .read_rust_sources_with_budget(&mut budget)
            .expect_err("ignored entries must not make traversal unbounded");
        let CliError::CatalogReadLimit(error) = error else {
            panic!("expected typed traversal limit")
        };

        assert_eq!(
            error.resource,
            CatalogReadLimitResource::TraversalEntryCount
        );
        assert_eq!(error.limit, 2);
        assert_eq!(error.observed_at_least, 3);
        assert_eq!(error.path, temp.path());
        drop(tree);
        temp.close().expect("close temporary root");
    }

    #[cfg(unix)]
    #[test]
    fn selected_special_file_is_rejected_without_blocking() {
        use std::os::unix::net::UnixListener;

        let temp = assert_fs::TempDir::new().expect("temporary root");
        let socket = temp.child("socket.rs");
        let _listener = UnixListener::bind(socket.path()).expect("Unix-domain socket");

        let tree = SourceTree::open(temp.path().to_path_buf()).expect("source tree");
        let cancellation = crate::context::ApplicationCancellation::new();
        let mut budget = tree.limits.begin(&cancellation);
        let error = tree
            .read_selected_with_budget(
                &[CatalogPath::new("socket.rs").expect("logical path")],
                &mut budget,
            )
            .expect_err("a socket is not a selected regular file");

        assert!(error.to_string().contains("not a regular file"));
        temp.close().expect("close temporary root");
    }
}
