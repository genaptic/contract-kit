//! Verified, bounded archive input for `conkit diff`.
//!
//! The final path component is opened atomically without following it: Unix
//! uses `O_NOFOLLOW`, while Windows opens the reparse point itself instead of
//! traversing it. Any opened handle is then verified to be a regular file. Its
//! metadata length is only an early size check; a bounded reader enforces the
//! compressed limit while reading from that same verified handle.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read};
use std::path::PathBuf;

use conkit_signature::FileCatalog;

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
    pub(crate) fn decode_contracts(self) -> Result<FileCatalog, CliError> {
        let file_name = self.validated_file_name()?;
        let file = self.open_regular_file()?;
        let bytes = self.read_compressed_bytes(&file_name, file)?;

        ArchivePayload::decode_gzip(&file_name, &bytes)?.into_contract_files(&file_name)
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

    fn open_regular_file(&self) -> Result<File, CliError> {
        let lexical = match fs_err::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == ErrorKind::NotFound => {
                return match fs_err::metadata(&self.path) {
                    Err(source) => Err(source.into()),
                    Ok(_) => Err(CliError::ArchiveNotRegularFile {
                        path: self.path.clone(),
                    }),
                };
            }
            Err(source) => return Err(source.into()),
        };
        if lexical.file_type().is_symlink() || !lexical.is_file() {
            return Err(CliError::ArchiveNotRegularFile {
                path: self.path.clone(),
            });
        }

        self.open_without_following()
    }

    fn open_without_following(&self) -> Result<File, CliError> {
        let mut options = OpenOptions::new();
        options.read(true);

        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;

            options.custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }

        let file = options.open(&self.path)?;
        let metadata = file.metadata()?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CliError::ArchiveNotRegularFile {
                path: self.path.clone(),
            });
        }

        Ok(file)
    }

    fn read_compressed_bytes(&self, file_name: &str, file: File) -> Result<Vec<u8>, CliError> {
        let size = file.metadata()?.len();
        if size > MAX_COMPRESSED_ARCHIVE_BYTES as u64 {
            return Err(CliError::ArchiveProcess {
                message: format!(
                    "{file_name}: compressed archive exceeds {MAX_COMPRESSED_ARCHIVE_BYTES} bytes"
                ),
            });
        }

        let mut bytes = Vec::with_capacity(
            usize::try_from(size)
                .unwrap_or(MAX_COMPRESSED_ARCHIVE_BYTES)
                .min(MAX_COMPRESSED_ARCHIVE_BYTES),
        );
        file.take(MAX_COMPRESSED_ARCHIVE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|source| CliError::ArchiveProcess {
                message: format!("{file_name}: {source}"),
            })?;
        if bytes.len() > MAX_COMPRESSED_ARCHIVE_BYTES {
            return Err(CliError::ArchiveProcess {
                message: format!(
                    "{file_name}: compressed archive exceeds {MAX_COMPRESSED_ARCHIVE_BYTES} bytes"
                ),
            });
        }

        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use assert_fs::TempDir;
    use conkit_signature::CatalogPath;

    use crate::error::CliError;

    use super::ArchiveSource;

    #[test]
    fn old_version_one_archive_decodes() {
        let temp = TempDir::new().expect("temp dir");
        let archive = temp.path().join("mixed-v1.gzip");
        std::fs::write(
            &archive,
            include_bytes!("../tests/fixtures/archive-v1/mixed-v1.gzip"),
        )
        .expect("archive fixture");

        let catalog = ArchiveSource::new(archive)
            .decode_contracts()
            .expect("version-1 archive");

        assert_eq!(catalog.len(), 1);
        assert!(
            catalog
                .get(&CatalogPath::new("main.yml").expect("combined document path"))
                .is_some()
        );
        temp.close().expect("cleanup");
    }

    #[test]
    fn archive_source_rejects_oversized_compressed_input_before_reading() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("oversized.gzip");
        let file = std::fs::File::create(&path).expect("oversized archive file");
        file.set_len((super::super::MAX_COMPRESSED_ARCHIVE_BYTES + 1) as u64)
            .expect("sparse oversized archive");

        let error = ArchiveSource::new(path)
            .decode_contracts()
            .expect_err("compressed limit must be checked from metadata");

        assert!(error.to_string().contains("compressed archive exceeds"));
        temp.close().expect("cleanup");
    }

    #[test]
    fn opened_archive_growth_is_rejected_before_unbounded_reading() {
        let temp = TempDir::new().expect("temp dir");
        let path = temp.path().join("growing.gzip");
        std::fs::write(&path, b"small").expect("small archive");
        let archive = ArchiveSource::new(path.clone());
        let file = archive.open_regular_file().expect("open small archive");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open archive writer")
            .set_len((super::super::MAX_COMPRESSED_ARCHIVE_BYTES + 1) as u64)
            .expect("grow archive");

        let error = archive
            .read_compressed_bytes("growing.gzip", file)
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
        let file = archive
            .open_regular_file()
            .expect("open exact-limit archive");

        let bytes = archive
            .read_compressed_bytes("exact-limit.gzip", file)
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
        let file = archive.open_regular_file().expect("open original archive");
        std::fs::rename(&path, temp.path().join("original.gzip")).expect("move original archive");
        let replacement = std::fs::File::create(&path).expect("replacement archive");
        replacement
            .set_len((super::super::MAX_COMPRESSED_ARCHIVE_BYTES + 1) as u64)
            .expect("oversized replacement");

        let bytes = archive
            .read_compressed_bytes("replaced.gzip", file)
            .expect("read original open file");

        assert_eq!(bytes, original);
        temp.close().expect("cleanup");
    }

    #[test]
    fn archive_reader_rejects_symlink_and_non_regular_path_before_decode() {
        let temp = TempDir::new().expect("temp dir");
        let directory = temp.path().join("archive-directory.gzip");
        std::fs::create_dir(&directory).expect("archive directory");

        let directory_error = ArchiveSource::new(directory.clone())
            .decode_contracts()
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

            let symlink_error = ArchiveSource::new(link.clone())
                .decode_contracts()
                .expect_err("a resolvable archive symlink must not be followed");
            assert!(matches!(
                symlink_error,
                CliError::ArchiveNotRegularFile { path } if path == link
            ));

            let socket_path = temp.path().join("archive-socket.gzip");
            let socket = UnixListener::bind(&socket_path).expect("archive socket");

            let socket_error = ArchiveSource::new(socket_path.clone())
                .decode_contracts()
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

        ArchiveSource::new(link)
            .open_without_following()
            .expect_err("opening must atomically reject the final symlink");

        temp.close().expect("cleanup");
    }
}
