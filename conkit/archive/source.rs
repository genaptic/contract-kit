//! Verified, bounded archive input for `conkit diff`.
//!
//! The selected parent directory is opened once as a capability, and the final
//! component is opened relative to that stable handle without following a
//! symlink or reparse point. Any opened handle is then verified to be a regular
//! file. Its metadata length is only an early size check; a bounded reader
//! enforces the compressed limit while reading from that same verified handle.

use std::path::{Path, PathBuf};

use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, File, OpenOptions};
use conkit_signature::FileCatalog;

use crate::catalog::{CatalogFileRead, CatalogReadBudget};
use crate::error::CliError;
use crate::platform::PortablePathRules;

use super::{ArchivePayload, MAX_COMPRESSED_ARCHIVE_BYTES};

/// User-selected archive file read for `conkit diff`.
#[derive(Debug)]
pub(crate) struct ArchiveSource {
    path: PathBuf,
}

impl ArchiveSource {
    /// Creates an archive reader for a local file path.
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Reads and decodes the archived contract catalog through one verified
    /// regular-file handle.
    ///
    /// Opening atomically refuses to follow a final-component symlink on Unix
    /// or reparse point on Windows. The handle's metadata length provides an
    /// early size rejection, while bounded reading from that same handle
    /// enforces the compressed archive limit if the file grows.
    ///
    /// # Errors
    ///
    /// Returns an error when the path has no valid portable UTF-8 file name,
    /// is a symlink or non-regular entry, cannot be opened or read, exceeds a
    /// wire limit, or contains an invalid archive payload.
    pub(crate) fn decode_contracts(
        self,
        budget: &mut CatalogReadBudget,
    ) -> Result<FileCatalog, CliError> {
        budget.checkpoint()?;
        let file_name = self.validated_file_name()?;
        let parent = self.open_parent_directory()?;
        let file = self.open_regular_file(&parent, &file_name)?;
        let bytes = self.read_compressed_bytes(&file_name, file, budget)?;

        ArchivePayload::decode_gzip(&file_name, &bytes, budget)?
            .into_contract_files(&file_name, budget)
    }

    fn validated_file_name(&self) -> Result<String, CliError> {
        let file_name = self
            .path
            .file_name()
            .ok_or_else(|| CliError::MissingFileName {
                path: self.path.clone(),
            })?;
        PortablePathRules::validate_component(file_name)?;

        file_name
            .to_str()
            .map(ToOwned::to_owned)
            .ok_or(CliError::NonUtf8PathComponent)
    }

    fn open_parent_directory(&self) -> Result<Dir, CliError> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| CliError::MissingFileName {
                path: self.path.clone(),
            })?;
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };

        Dir::open_ambient_dir(parent, ambient_authority()).map_err(CliError::from)
    }

    fn open_regular_file(&self, parent: &Dir, file_name: &str) -> Result<File, CliError> {
        let mut options = OpenOptions::new();
        options.read(true);
        options.follow(FollowSymlinks::No);

        #[cfg(unix)]
        {
            use cap_fs_ext::OpenOptionsSyncExt;

            options.nonblock(true);
        }

        let file = match parent.open_with(file_name, &options) {
            Ok(file) => file,
            Err(source) => {
                return match parent.symlink_metadata(file_name) {
                    Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                        Err(CliError::ArchiveNotRegularFile {
                            path: self.path.clone(),
                        })
                    }
                    _ => Err(source.into()),
                };
            }
        };
        let metadata = file.metadata()?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CliError::ArchiveNotRegularFile {
                path: self.path.clone(),
            });
        }

        Ok(file)
    }

    fn read_compressed_bytes(
        &self,
        file_name: &str,
        mut file: File,
        budget: &mut CatalogReadBudget,
    ) -> Result<Vec<u8>, CliError> {
        budget.checkpoint()?;
        let size = file.metadata()?.len();
        if size > MAX_COMPRESSED_ARCHIVE_BYTES as u64 {
            return Err(Self::compressed_limit_error(file_name));
        }
        budget.begin_entry(&self.path)?;
        budget.preflight_file(&self.path, size)?;
        match budget.read_file_with_ceiling(
            &self.path,
            &mut file,
            MAX_COMPRESSED_ARCHIVE_BYTES as u64,
        ) {
            Ok(CatalogFileRead::Complete(bytes)) => Ok(bytes),
            Ok(CatalogFileRead::CeilingExceeded) => Err(Self::compressed_limit_error(file_name)),
            Err(CliError::Io(source)) => Err(CliError::ArchiveProcess {
                message: format!("{file_name}: {source}"),
            }),
            Err(error) => Err(error),
        }
    }

    fn compressed_limit_error(file_name: &str) -> CliError {
        CliError::ArchiveProcess {
            message: format!(
                "{file_name}: compressed archive exceeds {MAX_COMPRESSED_ARCHIVE_BYTES} bytes"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use assert_fs::TempDir;
    use conkit_signature::CatalogPath;

    use crate::catalog::{CatalogReadBudget, CatalogReadLimits};
    use crate::context::ApplicationCancellation;
    use crate::error::CliError;

    use super::ArchiveSource;

    struct ArchiveFixture;

    impl ArchiveFixture {
        fn budget() -> CatalogReadBudget {
            CatalogReadLimits::default().begin(&ApplicationCancellation::new())
        }

        fn limited_budget(
            entry_count: u64,
            total_bytes: u64,
            per_file_bytes: u64,
        ) -> CatalogReadBudget {
            CatalogReadLimits::new(entry_count, total_bytes, per_file_bytes)
                .begin(&ApplicationCancellation::new())
        }

        fn decode(source: ArchiveSource) -> Result<conkit_signature::FileCatalog, CliError> {
            source.decode_contracts(&mut Self::budget())
        }
    }

    #[test]
    fn old_version_one_archive_decodes() {
        let temp = TempDir::new().expect("temp dir");
        let archive = temp.path().join("mixed-v1.gzip");
        std::fs::write(
            &archive,
            include_bytes!("../tests/fixtures/archive-v1/mixed-v1.gzip"),
        )
        .expect("archive fixture");

        let catalog =
            ArchiveFixture::decode(ArchiveSource::new(archive)).expect("version-1 archive");

        assert_eq!(catalog.len(), 1);
        assert!(
            catalog
                .get(&CatalogPath::new("main.yml").expect("combined document path"))
                .is_some()
        );
        temp.close().expect("cleanup");
    }

    #[test]
    fn compressed_archive_is_a_participating_catalog_entry() {
        let temp = TempDir::new().expect("temp dir");
        let archive = temp.path().join("mixed-v1.gzip");
        std::fs::write(
            &archive,
            include_bytes!("../tests/fixtures/archive-v1/mixed-v1.gzip"),
        )
        .expect("archive fixture");
        let mut budget = ArchiveFixture::limited_budget(1, 4 * 1024, 4 * 1024);

        let error = ArchiveSource::new(archive.clone())
            .decode_contracts(&mut budget)
            .expect_err("the decoded entry must follow the physical archive entry");

        let CliError::CatalogReadLimit(error) = error else {
            panic!("expected an aggregate catalog entry limit");
        };
        assert!(
            error
                .to_string()
                .contains("catalog entry count limit exceeded: limit 1, observed at least 2"),
            "unexpected aggregate catalog limit: {error}",
        );
        temp.close().expect("cleanup");
    }

    #[test]
    fn current_archive_and_decoded_entry_share_exact_operation_limits() {
        let temp = TempDir::new().expect("temp dir");
        let archive = temp.path().join("mixed-v1.gzip");
        let compressed = include_bytes!("../tests/fixtures/archive-v1/mixed-v1.gzip");
        std::fs::write(&archive, compressed).expect("archive fixture");
        let archived_path = CatalogPath::new("main.yml").expect("archived document path");
        let archived =
            ArchiveFixture::decode(ArchiveSource::new(archive.clone())).expect("fixture catalog");
        let archived_bytes = archived
            .get(&archived_path)
            .expect("archived document")
            .len();
        let current_path = std::path::Path::new("current.yml");
        let current_bytes = 1_usize;
        let exact_total = current_bytes
            .saturating_add(compressed.len())
            .saturating_add(archived_bytes);
        let exact_total = u64::try_from(exact_total).expect("exact operation total");

        let mut exact = ArchiveFixture::limited_budget(3, exact_total, exact_total);
        exact
            .record_entry_bytes(current_path, current_bytes)
            .expect("current catalog entry");
        let decoded = ArchiveSource::new(archive.clone())
            .decode_contracts(&mut exact)
            .expect("the exact combined operation limits must succeed");
        assert_eq!(
            decoded.get(&archived_path).map(<[u8]>::len),
            Some(archived_bytes)
        );

        let mut entry_limited = ArchiveFixture::limited_budget(2, exact_total, exact_total);
        entry_limited
            .record_entry_bytes(current_path, current_bytes)
            .expect("current catalog entry");
        let entry_error = ArchiveSource::new(archive.clone())
            .decode_contracts(&mut entry_limited)
            .expect_err("the decoded document must be operation entry three");
        let CliError::CatalogReadLimit(entry_error) = entry_error else {
            panic!("expected an aggregate catalog entry limit");
        };
        assert!(
            entry_error
                .to_string()
                .contains("catalog entry count limit exceeded: limit 2, observed at least 3"),
            "unexpected aggregate entry limit: {entry_error}",
        );
        assert!(entry_error.to_string().contains("main.yml"));

        let mut byte_limited =
            ArchiveFixture::limited_budget(3, exact_total.saturating_sub(1), exact_total);
        byte_limited
            .record_entry_bytes(current_path, current_bytes)
            .expect("current catalog entry");
        let byte_error = ArchiveSource::new(archive)
            .decode_contracts(&mut byte_limited)
            .expect_err("the decoded document must cross the combined byte limit");
        let CliError::CatalogReadLimit(byte_error) = byte_error else {
            panic!("expected an aggregate catalog byte limit");
        };
        assert!(
            byte_error.to_string().contains(&format!(
                "catalog total bytes limit exceeded: limit {}, observed at least {exact_total}",
                exact_total - 1,
            )),
            "unexpected aggregate byte limit: {byte_error}",
        );
        assert!(byte_error.to_string().contains("main.yml"));
        temp.close().expect("cleanup");
    }

    #[test]
    fn compressed_archive_actual_bytes_are_committed_to_the_operation_budget() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("physical.gzip");
        let physical = b"not-gzip";
        std::fs::write(&path, physical).expect("physical archive bytes");
        let archive = ArchiveSource::new(path.clone());
        let file_name = archive.validated_file_name().expect("archive file name");
        let parent = archive
            .open_parent_directory()
            .expect("open archive parent");
        let file = archive
            .open_regular_file(&parent, &file_name)
            .expect("open physical archive");
        let mut budget = ArchiveFixture::limited_budget(
            2,
            u64::try_from(physical.len()).expect("physical length"),
            u64::try_from(physical.len()).expect("physical length"),
        );

        let bytes = archive
            .read_compressed_bytes(&file_name, file, &mut budget)
            .expect("the exact physical byte boundary must succeed");
        assert_eq!(bytes, physical);
        let error = budget
            .record_entry_bytes(std::path::Path::new("next.yml"), 1)
            .expect_err("the next byte must cross the committed operation total");

        assert!(
            error.to_string().contains(&format!(
                "catalog total bytes limit exceeded: limit {}, observed at least {}",
                physical.len(),
                physical.len() + 1,
            )),
            "unexpected aggregate catalog limit: {error}",
        );
        temp.close().expect("cleanup");
    }

    #[test]
    fn catalog_file_limit_precedes_invalid_gzip_decode() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("invalid.gzip");
        let invalid = b"not-gzip";
        std::fs::write(&path, invalid).expect("invalid archive bytes");
        let mut budget = ArchiveFixture::limited_budget(
            2,
            4 * 1024,
            u64::try_from(invalid.len() - 1).expect("invalid length"),
        );

        let error = ArchiveSource::new(path.clone())
            .decode_contracts(&mut budget)
            .expect_err("the physical file budget must be enforced before gzip decode");

        let CliError::CatalogReadLimit(error) = error else {
            panic!("expected a physical archive file-byte limit");
        };
        assert!(
            error.to_string().contains(&format!(
                "catalog file bytes limit exceeded: limit {}, observed at least {}",
                invalid.len() - 1,
                invalid.len(),
            )),
            "unexpected physical archive limit: {error}",
        );
        assert!(error.to_string().contains(&path.display().to_string()));
        temp.close().expect("cleanup");
    }

    #[test]
    fn catalog_total_limit_precedes_invalid_gzip_decode() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("invalid.gzip");
        let invalid = b"not-gzip";
        std::fs::write(&path, invalid).expect("invalid archive bytes");
        let mut budget = ArchiveFixture::limited_budget(
            2,
            u64::try_from(invalid.len() - 1).expect("invalid length"),
            4 * 1024,
        );

        let error = ArchiveSource::new(path.clone())
            .decode_contracts(&mut budget)
            .expect_err("the aggregate budget must be enforced before gzip decode");

        let CliError::CatalogReadLimit(error) = error else {
            panic!("expected a physical archive total-byte limit");
        };
        assert!(
            error.to_string().contains(&format!(
                "catalog total bytes limit exceeded: limit {}, observed at least {}",
                invalid.len() - 1,
                invalid.len(),
            )),
            "unexpected physical archive limit: {error}",
        );
        assert!(error.to_string().contains(&path.display().to_string()));
        temp.close().expect("cleanup");
    }

    #[test]
    fn archive_source_rejects_oversized_compressed_input_before_reading() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("oversized.gzip");
        let file = std::fs::File::create(&path).expect("oversized archive file");
        file.set_len((super::super::MAX_COMPRESSED_ARCHIVE_BYTES + 1) as u64)
            .expect("sparse oversized archive");

        let error = ArchiveFixture::decode(ArchiveSource::new(path))
            .expect_err("compressed limit must be checked from metadata");

        assert!(error.to_string().contains("compressed archive exceeds"));
        temp.close().expect("cleanup");
    }

    #[test]
    fn cancellation_after_archive_open_precedes_oversized_metadata_rejection() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("oversized.gzip");
        let file = std::fs::File::create(&path).expect("oversized archive file");
        file.set_len((super::super::MAX_COMPRESSED_ARCHIVE_BYTES + 1) as u64)
            .expect("sparse oversized archive");
        let archive = ArchiveSource::new(path);
        let file_name = archive.validated_file_name().expect("archive file name");
        let parent = archive
            .open_parent_directory()
            .expect("open archive parent");
        let file = archive
            .open_regular_file(&parent, &file_name)
            .expect("open oversized archive");
        let cancellation = ApplicationCancellation::new();
        let mut budget = CatalogReadLimits::default().begin(&cancellation);
        cancellation.request();

        let error = archive
            .read_compressed_bytes(&file_name, file, &mut budget)
            .expect_err("cancellation must precede metadata size rejection");

        assert!(matches!(error, CliError::OperationCanceled));
        temp.close().expect("cleanup");
    }

    #[test]
    fn opened_archive_growth_is_rejected_before_unbounded_reading() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("growing.gzip");
        std::fs::write(&path, b"small").expect("small archive");
        let archive = ArchiveSource::new(path.clone());
        let file_name = archive.validated_file_name().expect("archive file name");
        let parent = archive
            .open_parent_directory()
            .expect("open archive parent");
        let file = archive
            .open_regular_file(&parent, &file_name)
            .expect("open small archive");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open archive writer")
            .set_len((super::super::MAX_COMPRESSED_ARCHIVE_BYTES + 1) as u64)
            .expect("grow archive");

        let error = archive
            .read_compressed_bytes("growing.gzip", file, &mut ArchiveFixture::budget())
            .expect_err("growth beyond the compressed limit must fail");

        assert!(error.to_string().contains("compressed archive exceeds"));
        temp.close().expect("cleanup");
    }

    #[test]
    fn compressed_archive_at_the_exact_limit_is_accepted_by_the_bounded_reader() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("exact-limit.gzip");
        let archive_file = std::fs::File::create(&path).expect("archive file");
        archive_file
            .set_len(super::super::MAX_COMPRESSED_ARCHIVE_BYTES as u64)
            .expect("exact-limit sparse archive");
        let archive = ArchiveSource::new(path);
        let file_name = archive.validated_file_name().expect("archive file name");
        let parent = archive
            .open_parent_directory()
            .expect("open archive parent");
        let file = archive
            .open_regular_file(&parent, &file_name)
            .expect("open exact-limit archive");

        let bytes = archive
            .read_compressed_bytes("exact-limit.gzip", file, &mut ArchiveFixture::budget())
            .expect("the exact compressed limit must be accepted");

        assert_eq!(bytes.len(), super::super::MAX_COMPRESSED_ARCHIVE_BYTES);
        temp.close().expect("cleanup");
    }

    #[test]
    fn archive_read_uses_the_verified_open_file_after_path_replacement() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("replaced.gzip");
        let original = include_bytes!("../tests/fixtures/archive-v1/mixed-v1.gzip");
        std::fs::write(&path, original).expect("original archive");
        let archive = ArchiveSource::new(path.clone());
        let file_name = archive.validated_file_name().expect("archive file name");
        let parent = archive
            .open_parent_directory()
            .expect("open archive parent");
        let file = archive
            .open_regular_file(&parent, &file_name)
            .expect("open original archive");
        std::fs::rename(&path, temp.path().join("original.gzip")).expect("move original archive");
        let replacement = std::fs::File::create(&path).expect("replacement archive");
        replacement
            .set_len((super::super::MAX_COMPRESSED_ARCHIVE_BYTES + 1) as u64)
            .expect("oversized replacement");

        let bytes = archive
            .read_compressed_bytes("replaced.gzip", file, &mut ArchiveFixture::budget())
            .expect("read original open file");

        assert_eq!(bytes, original);
        temp.close().expect("cleanup");
    }

    #[test]
    fn archive_parent_retarget_does_not_redirect_open_or_read() {
        let temp = TempDir::new().expect("temp dir");
        let selected_parent = temp.path().join("selected");
        let anchored_parent = temp.path().join("anchored");
        std::fs::create_dir(&selected_parent).expect("selected parent");
        let selected_archive = selected_parent.join("contracts.gzip");
        let original = include_bytes!("../tests/fixtures/archive-v1/mixed-v1.gzip");
        std::fs::write(&selected_archive, original).expect("original archive");
        let archive = ArchiveSource::new(selected_archive.clone());
        let file_name = archive.validated_file_name().expect("archive file name");
        let parent = archive
            .open_parent_directory()
            .expect("anchor selected parent");

        std::fs::rename(&selected_parent, &anchored_parent).expect("move selected parent");
        std::fs::create_dir(&selected_parent).expect("replacement parent");
        let replacement = b"replacement archive bytes";
        std::fs::write(&selected_archive, replacement).expect("replacement archive");

        let file = archive
            .open_regular_file(&parent, &file_name)
            .expect("open from anchored parent");
        let bytes = archive
            .read_compressed_bytes(&file_name, file, &mut ArchiveFixture::budget())
            .expect("read anchored archive");

        assert_eq!(bytes, original);
        assert_eq!(
            std::fs::read(&selected_archive).expect("replacement bytes"),
            replacement,
            "opening through the anchored parent must not touch the replacement tree"
        );
        temp.close().expect("cleanup");
    }

    #[test]
    fn archive_reader_rejects_symlink_and_non_regular_path_before_decode() {
        let temp = TempDir::new().expect("temp dir");
        let directory = temp.path().join("archive-directory.gzip");
        std::fs::create_dir(&directory).expect("archive directory");

        let directory_error = ArchiveFixture::decode(ArchiveSource::new(directory.clone()))
            .expect_err("archive directory must be rejected");

        assert!(matches!(
            directory_error,
            CliError::ArchiveNotRegularFile { path } if path == directory
        ));

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            use std::os::unix::net::UnixListener;

            let valid = temp.path().join("valid.gzip");
            let link = temp.path().join("archive-link.gzip");
            std::fs::write(
                &valid,
                include_bytes!("../tests/fixtures/archive-v1/mixed-v1.gzip"),
            )
            .expect("valid archive target");
            symlink(&valid, &link).expect("resolvable archive symlink");

            let symlink_error = ArchiveFixture::decode(ArchiveSource::new(link.clone()))
                .expect_err("a resolvable archive symlink must not be followed");
            assert!(matches!(
                symlink_error,
                CliError::ArchiveNotRegularFile { path } if path == link
            ));

            let socket_path = temp.path().join("archive-socket.gzip");
            let socket = UnixListener::bind(&socket_path).expect("archive socket");

            let socket_error = ArchiveFixture::decode(ArchiveSource::new(socket_path.clone()))
                .expect_err("archive socket must be rejected");
            assert!(matches!(
                socket_error,
                CliError::ArchiveNotRegularFile { path } if path == socket_path
            ));

            drop(socket);
        }
        temp.close().expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn archive_open_does_not_follow_a_final_file_symlink() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp dir");
        let target = temp.path().join("target.gzip");
        let link = temp.path().join("archive-link.gzip");
        std::fs::write(&target, b"archive").expect("archive target");
        symlink(&target, &link).expect("resolvable archive symlink");

        let archive = ArchiveSource::new(link);
        let file_name = archive.validated_file_name().expect("archive file name");
        let parent = archive
            .open_parent_directory()
            .expect("open archive parent");

        archive
            .open_regular_file(&parent, &file_name)
            .expect_err("opening must atomically reject the final symlink");

        temp.close().expect("cleanup");
    }
}
