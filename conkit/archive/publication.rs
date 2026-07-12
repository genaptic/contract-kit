//! Collision-safe archive naming and no-clobber filesystem publication.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::CliError;

const MAX_ARCHIVE_NAME_COLLISIONS: u16 = 1024;

/// Directory where a new archive file should be created.
pub(crate) struct ArchiveDestination {
    root: PathBuf,
}

impl ArchiveDestination {
    /// Creates an archive destination rooted at the user-selected directory.
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Publishes encoded bytes to the next available timestamped archive path.
    ///
    /// # Errors
    ///
    /// Returns an error when a collision-safe name cannot be selected or the
    /// archive cannot be written, synchronized, published, or cleaned up.
    pub(crate) fn publish(self, bytes: Vec<u8>) -> Result<PathBuf, CliError> {
        let base_name = ArchiveName::from_system_time(SystemTime::now())?;
        let destination = self.next_available_path(base_name)?;

        ArchivePublication::create(destination)?.publish(&bytes)
    }

    fn next_available_path(&self, base_name: ArchiveName) -> Result<PathBuf, CliError> {
        for collision_index in 0..MAX_ARCHIVE_NAME_COLLISIONS {
            let name = base_name.with_collision_index(collision_index);
            let candidate = self.root.join(name.file_name());

            if candidate.exists() {
                continue;
            }

            return Ok(candidate);
        }

        Err(CliError::ArchiveNameExhausted {
            root: self.root.clone(),
            unix_nanos: base_name.unix_nanos,
        })
    }
}

/// Timestamp-derived archive file name before it is joined to a root path.
struct ArchiveName {
    unix_nanos: u128,
    collision_index: u16,
}

impl ArchiveName {
    fn new(unix_nanos: u128) -> Self {
        Self {
            unix_nanos,
            collision_index: 0,
        }
    }

    fn from_system_time(time: SystemTime) -> Result<Self, CliError> {
        let unix_nanos = time
            .duration_since(UNIX_EPOCH)
            .map_err(|source| CliError::Clock { source })?
            .as_nanos();

        Ok(Self::new(unix_nanos))
    }

    fn with_collision_index(&self, collision_index: u16) -> Self {
        Self {
            unix_nanos: self.unix_nanos,
            collision_index,
        }
    }

    fn file_name(&self) -> String {
        if self.collision_index == 0 {
            format!("{}-archive.gzip", self.unix_nanos)
        } else {
            format!("{}-{}-archive.gzip", self.unix_nanos, self.collision_index)
        }
    }
}

struct ArchivePublication {
    destination: PathBuf,
    state: PublicationState,
}

enum PublicationState {
    Open { temporary: PathBuf, output: File },
    Closed { temporary: PathBuf },
    Cleaned,
}

impl ArchivePublication {
    fn create(destination: PathBuf) -> Result<Self, CliError> {
        if let Some(parent) = destination.parent() {
            fs_err::create_dir_all(parent)?;
        }
        if destination.exists() {
            return Err(CliError::ArchiveAlreadyExists { path: destination });
        }

        let parent = destination
            .parent()
            .ok_or_else(|| CliError::MissingFileName {
                path: destination.clone(),
            })?;
        let file_name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(CliError::NonUtf8PathComponent)?;

        for collision in 0..=u16::MAX {
            let temporary = parent.join(format!(
                ".{file_name}.{}.{}.tmp",
                std::process::id(),
                collision
            ));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
            {
                Ok(output) => {
                    return Ok(Self {
                        destination,
                        state: PublicationState::Open { temporary, output },
                    });
                }
                Err(source) if source.kind() == ErrorKind::AlreadyExists => {}
                Err(source) => {
                    return Err(CliError::ArchiveWrite {
                        path: temporary,
                        source,
                    });
                }
            }
        }

        Err(CliError::ArchiveWrite {
            path: destination,
            source: std::io::Error::new(
                ErrorKind::AlreadyExists,
                "could not allocate a sibling archive temporary file",
            ),
        })
    }

    fn publish(mut self, bytes: &[u8]) -> Result<PathBuf, CliError> {
        if let Err(write) = self.write_and_sync(bytes) {
            return Err(self.cleanup_after_write(write));
        }

        self.close_output();
        if let Err(source) = fs_err::hard_link(self.temporary_path(), &self.destination) {
            return Err(self.cleanup_after_publication(source));
        }

        let temporary = self.temporary_path().to_path_buf();
        if let Err(source) = self.remove_temporary() {
            return match fs_err::remove_file(&self.destination) {
                Ok(()) => Err(CliError::ArchiveWrite {
                    path: temporary,
                    source,
                }),
                Err(cleanup) => Err(CliError::ArchiveWriteAndCleanup {
                    path: self.destination.clone(),
                    write: source,
                    cleanup,
                }),
            };
        }

        Ok(self.destination.clone())
    }

    fn write_and_sync(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        let PublicationState::Open { output, .. } = &mut self.state else {
            return Err(std::io::Error::other(
                "archive publication output is not open",
            ));
        };

        output.write_all(bytes)?;
        output.flush()?;
        output.sync_all()
    }

    fn close_output(&mut self) {
        let state = std::mem::replace(&mut self.state, PublicationState::Cleaned);
        self.state = match state {
            PublicationState::Open { temporary, output } => {
                drop(output);
                PublicationState::Closed { temporary }
            }
            state => state,
        };
    }

    fn temporary_path(&self) -> &Path {
        match &self.state {
            PublicationState::Open { temporary, .. } | PublicationState::Closed { temporary } => {
                temporary
            }
            PublicationState::Cleaned => {
                unreachable!("a cleaned archive publication has no temporary path")
            }
        }
    }

    fn remove_temporary(&mut self) -> std::io::Result<()> {
        fs_err::remove_file(self.temporary_path())?;
        self.state = PublicationState::Cleaned;
        Ok(())
    }

    fn cleanup_after_write(&mut self, write: std::io::Error) -> CliError {
        self.close_output();
        match self.remove_temporary() {
            Ok(()) => CliError::ArchiveWrite {
                path: self.destination.clone(),
                source: write,
            },
            Err(cleanup) => CliError::ArchiveWriteAndCleanup {
                path: self.destination.clone(),
                write,
                cleanup,
            },
        }
    }

    fn cleanup_after_publication(&mut self, publish: std::io::Error) -> CliError {
        self.close_output();
        match self.remove_temporary() {
            Ok(()) if publish.kind() == ErrorKind::AlreadyExists => {
                CliError::ArchiveAlreadyExists {
                    path: self.destination.clone(),
                }
            }
            Ok(()) => CliError::ArchiveWrite {
                path: self.destination.clone(),
                source: publish,
            },
            Err(cleanup) => CliError::ArchiveWriteAndCleanup {
                path: self.destination.clone(),
                write: publish,
                cleanup,
            },
        }
    }
}

impl Drop for ArchivePublication {
    fn drop(&mut self) {
        let state = std::mem::replace(&mut self.state, PublicationState::Cleaned);
        match state {
            PublicationState::Open { temporary, output } => {
                drop(output);
                let _ = fs_err::remove_file(temporary);
            }
            PublicationState::Closed { temporary } => {
                let _ = fs_err::remove_file(temporary);
            }
            PublicationState::Cleaned => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use assert_fs::TempDir;

    use crate::error::CliError;

    use super::{ArchiveDestination, ArchiveName, ArchivePublication, PublicationState};

    #[test]
    fn archive_name_uses_unix_nanoseconds_without_colons() {
        let name = ArchiveName::new(123_456_789).file_name();

        assert_eq!(name, "123456789-archive.gzip");
        assert!(!name.contains(':'));
    }

    #[test]
    fn archive_name_adds_collision_suffix() {
        let name = ArchiveName::new(123).with_collision_index(2).file_name();

        assert_eq!(name, "123-2-archive.gzip");
    }

    #[cfg(unix)]
    #[test]
    fn archive_publication_and_temporary_cleanup_failures_are_both_reported() {
        let temp = TempDir::new().expect("temp dir");
        let destination = temp.path().join("published.gzip");
        let publication =
            ArchivePublication::create(destination.clone()).expect("archive publication");
        let temporary = publication.temporary_path().to_path_buf();
        std::fs::remove_file(&temporary).expect("unlink open temporary");
        std::fs::create_dir(&temporary).expect("replace temporary with directory");

        let error = publication
            .publish(b"archive bytes")
            .expect_err("publication and cleanup must fail");

        assert!(matches!(
            error,
            CliError::ArchiveWriteAndCleanup { path, .. } if path == destination
        ));
        assert!(!destination.exists());

        std::fs::remove_dir(&temporary).expect("remove temporary directory");
        temp.close().expect("cleanup");
    }

    #[test]
    fn explicit_cleanup_does_not_delete_a_recreated_temporary_on_drop() {
        let temp = TempDir::new().expect("temp dir");
        let destination = temp.path().join("final-archive.gzip");
        let mut publication =
            ArchivePublication::create(destination.clone()).expect("archive publication");
        let temporary = publication.temporary_path().to_path_buf();

        let error = publication.cleanup_after_write(std::io::Error::other("forced write failure"));

        assert!(matches!(
            error,
            CliError::ArchiveWrite { path, .. } if path == destination
        ));
        assert!(!temporary.exists());
        std::fs::write(&temporary, b"replacement").expect("replacement temporary");

        drop(publication);

        assert_eq!(
            std::fs::read(&temporary).expect("replacement must survive drop"),
            b"replacement"
        );
        temp.close().expect("cleanup");
    }

    #[test]
    fn archive_publication_writes_bytes_to_archive_root() {
        let temp = TempDir::new().expect("temp dir");
        let local_path = temp.path().join("123-archive.gzip");
        let publication =
            ArchivePublication::create(local_path.clone()).expect("archive publication");

        let written = publication
            .publish(b"archive bytes")
            .expect("write archive");

        assert_eq!(written, local_path);
        assert_eq!(
            std::fs::read(&written).expect("archive bytes"),
            b"archive bytes"
        );
        assert_eq!(
            std::fs::read_dir(temp.path())
                .expect("archive directory")
                .count(),
            1,
            "publication must remove its sibling temporary file"
        );
        temp.close().expect("cleanup");
    }

    #[test]
    fn archive_destination_skips_existing_timestamp_name() {
        let temp = TempDir::new().expect("temp dir");
        std::fs::write(temp.path().join("123-archive.gzip"), b"existing").expect("existing");

        let destination = ArchiveDestination::new(temp.path().to_path_buf());
        let path = destination
            .next_available_path(ArchiveName::new(123))
            .expect("next archive path");

        assert_eq!(path, temp.path().join("123-1-archive.gzip"));
        temp.close().expect("cleanup");
    }

    #[test]
    fn archive_publication_does_not_overwrite_existing_archive() {
        let temp = TempDir::new().expect("temp dir");
        let local_path = temp.path().join("123-archive.gzip");
        std::fs::write(&local_path, b"existing archive").expect("existing archive");

        let error = ArchivePublication::create(local_path.clone())
            .err()
            .expect("must not overwrite");

        assert!(matches!(error, CliError::ArchiveAlreadyExists { .. }));
        assert_eq!(
            std::fs::read(&local_path).expect("archive bytes"),
            b"existing archive"
        );
        temp.close().expect("cleanup");
    }

    #[test]
    fn publication_race_does_not_clobber_destination_and_removes_temporary() {
        let temp = TempDir::new().expect("temp dir");
        let destination = temp.path().join("raced-archive.gzip");
        let publication = ArchivePublication::create(destination.clone())
            .expect("create sibling temporary archive");
        let temporary = publication.temporary_path().to_path_buf();
        std::fs::write(&destination, b"winner").expect("racing publisher");

        let error = publication
            .publish(b"loser")
            .expect_err("publication must not clobber the race winner");

        assert!(matches!(error, CliError::ArchiveAlreadyExists { .. }));
        assert_eq!(
            std::fs::read(&destination).expect("winner bytes"),
            b"winner"
        );
        assert!(!temporary.exists());
        temp.close().expect("cleanup");
    }

    #[test]
    fn failed_archive_publication_removes_the_sibling_temporary() {
        let temp = TempDir::new().expect("temp dir");
        let initial_destination = temp.path().join("initial-archive.gzip");
        let mut publication = ArchivePublication::create(initial_destination)
            .expect("create sibling temporary archive");
        let temporary = publication.temporary_path().to_path_buf();
        let destination = temp.path().join("missing").join("final-archive.gzip");
        publication.destination = destination.clone();

        let error = publication
            .publish(b"archive bytes")
            .expect_err("publication into a missing directory must fail");

        assert!(matches!(
            error,
            CliError::ArchiveWrite { path, .. } if path == destination
        ));
        assert!(!temporary.exists());
        assert!(!destination.exists());
        temp.close().expect("cleanup");
    }

    #[test]
    fn archive_publication_is_removed_when_abandoned() {
        let temp = TempDir::new().expect("temp dir");
        let destination = temp.path().join("incomplete-archive.gzip");
        let publication = ArchivePublication::create(destination.clone())
            .expect("create sibling temporary archive");
        let temporary = publication.temporary_path().to_path_buf();

        drop(publication);

        assert!(!destination.exists());
        assert!(!temporary.exists());
        temp.close().expect("cleanup");
    }

    #[test]
    fn failed_archive_write_removes_the_sibling_temporary() {
        let temp = TempDir::new().expect("temp dir");
        let destination = temp.path().join("final-archive.gzip");
        let temporary = temp.path().join(".final-archive.gzip.test.tmp");
        std::fs::write(&temporary, b"temporary").expect("temporary file");
        let read_only = std::fs::File::open(&temporary).expect("read-only temporary handle");
        let publication = ArchivePublication {
            destination: destination.clone(),
            state: PublicationState::Open {
                temporary: temporary.clone(),
                output: read_only,
            },
        };

        let error = publication
            .publish(b"archive bytes")
            .expect_err("read-only output must fail");

        assert!(matches!(error, CliError::ArchiveWrite { .. }));
        assert!(!temporary.exists());
        assert!(!destination.exists());
        temp.close().expect("cleanup");
    }
}
