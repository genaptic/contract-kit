//! Collision-safe archive naming and no-clobber filesystem publication.
//!
//! The user-selected destination is created if necessary and opened once as a
//! directory capability. Collision checks, sibling-temporary creation,
//! no-clobber hard-link publication, explicit cleanup, and `Drop` cleanup all
//! remain relative to that stable handle if the ambient root path is replaced.

use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, File, OpenOptions};

use crate::context::ApplicationCancellation;
use crate::error::CliError;

const MAX_ARCHIVE_NAME_COLLISIONS: u16 = 1024;

/// Directory where a new archive file should be created.
pub(crate) struct ArchiveDestination {
    root: PathBuf,
    cancellation: ApplicationCancellation,
}

impl ArchiveDestination {
    /// Creates an archive destination rooted at the user-selected directory.
    pub(crate) fn new(root: PathBuf, cancellation: &ApplicationCancellation) -> Self {
        Self {
            root,
            cancellation: cancellation.clone(),
        }
    }

    /// Publishes encoded bytes to the next available timestamped archive path.
    ///
    /// # Errors
    ///
    /// Returns an error when a collision-safe name cannot be selected or the
    /// archive cannot be written, synchronized, published, or cleaned up.
    pub(crate) fn publish(self, bytes: Vec<u8>) -> Result<PathBuf, CliError> {
        self.cancellation.checkpoint()?;
        fs_err::create_dir_all(&self.root)?;
        let root = self.open_root()?;
        self.publish_into(root, &bytes)
    }

    fn open_root(&self) -> Result<Dir, CliError> {
        Dir::open_ambient_dir(&self.root, ambient_authority()).map_err(CliError::from)
    }

    fn publish_into(self, root: Dir, bytes: &[u8]) -> Result<PathBuf, CliError> {
        let base_name = ArchiveName::from_system_time(SystemTime::now())?;
        let destination = self.next_available_path(&root, base_name)?;

        ArchivePublication::create(root, destination, self.cancellation)?.publish(bytes)
    }

    fn next_available_path(&self, root: &Dir, base_name: ArchiveName) -> Result<PathBuf, CliError> {
        for collision_index in 0..MAX_ARCHIVE_NAME_COLLISIONS {
            self.cancellation.checkpoint()?;
            let name = base_name.with_collision_index(collision_index);
            let file_name = name.file_name();
            let candidate = self.root.join(&file_name);

            if root.try_exists(&file_name)? {
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
    root: Dir,
    destination: PathBuf,
    destination_name: PathBuf,
    state: PublicationState,
    cancellation: ApplicationCancellation,
}

enum PublicationState {
    Open { temporary: PathBuf, output: File },
    Closed { temporary: PathBuf },
    Cleaned,
}

impl ArchivePublication {
    fn create(
        root: Dir,
        destination: PathBuf,
        cancellation: ApplicationCancellation,
    ) -> Result<Self, CliError> {
        cancellation.checkpoint()?;
        let file_name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned)
            .ok_or(CliError::NonUtf8PathComponent)?;
        let destination_name = PathBuf::from(&file_name);
        if root.try_exists(&destination_name)? {
            return Err(CliError::ArchiveAlreadyExists { path: destination });
        }

        for collision in 0..=u16::MAX {
            cancellation.checkpoint()?;
            let temporary = PathBuf::from(format!(
                ".{file_name}.{}.{}.tmp",
                std::process::id(),
                collision
            ));
            let mut options = OpenOptions::new();
            options
                .write(true)
                .create_new(true)
                .follow(FollowSymlinks::No);
            match root.open_with(&temporary, &options) {
                Ok(output) => {
                    return Ok(Self {
                        root,
                        destination,
                        destination_name,
                        state: PublicationState::Open { temporary, output },
                        cancellation,
                    });
                }
                Err(source) if source.kind() == ErrorKind::AlreadyExists => {}
                Err(source) => {
                    return Err(CliError::ArchiveWrite {
                        path: destination
                            .parent()
                            .unwrap_or_else(|| Path::new(""))
                            .join(temporary),
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
            if self.cancellation.checkpoint().is_err() {
                return Err(self.cleanup_after_cancellation());
            }
            return Err(self.cleanup_after_write(write));
        }

        self.close_output();
        if self.cancellation.checkpoint().is_err() {
            return Err(self.cleanup_after_cancellation());
        }
        if let Err(source) =
            self.root
                .hard_link(self.temporary_path(), &self.root, &self.destination_name)
        {
            return Err(self.cleanup_after_publication(source));
        }

        let temporary = self.temporary_display_path();
        if let Err(source) = self.remove_temporary() {
            return match self.root.remove_file(&self.destination_name) {
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

        for chunk in bytes.chunks(64 * 1024) {
            self.cancellation
                .checkpoint()
                .map_err(std::io::Error::other)?;
            output.write_all(chunk)?;
        }
        self.cancellation
            .checkpoint()
            .map_err(std::io::Error::other)?;
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

    fn temporary_display_path(&self) -> PathBuf {
        self.destination
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(self.temporary_path())
    }

    fn remove_temporary(&mut self) -> std::io::Result<()> {
        let temporary = self.temporary_path().to_path_buf();
        self.root.remove_file(temporary)?;
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

    fn cleanup_after_cancellation(&mut self) -> CliError {
        self.close_output();
        match self.remove_temporary() {
            Ok(()) => CliError::OperationCanceled,
            Err(cleanup) => CliError::ArchiveWriteAndCleanup {
                path: self.destination.clone(),
                write: std::io::Error::new(
                    ErrorKind::Interrupted,
                    "archive publication was canceled",
                ),
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
                let _ = self.root.remove_file(temporary);
            }
            PublicationState::Closed { temporary } => {
                let _ = self.root.remove_file(temporary);
            }
            PublicationState::Cleaned => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use assert_fs::TempDir;
    use cap_std::ambient_authority;
    use cap_std::fs::Dir;

    use crate::context::ApplicationCancellation;
    use crate::error::CliError;

    use super::{ArchiveDestination, ArchiveName, ArchivePublication, PublicationState};

    fn publication(root: Dir, destination: PathBuf) -> Result<ArchivePublication, CliError> {
        ArchivePublication::create(root, destination, ApplicationCancellation::new())
    }

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
        let root =
            Dir::open_ambient_dir(temp.path(), ambient_authority()).expect("open archive root");
        let publication = publication(root, destination.clone()).expect("archive publication");
        let temporary = publication.temporary_display_path();
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
        let root =
            Dir::open_ambient_dir(temp.path(), ambient_authority()).expect("open archive root");
        let mut publication = publication(root, destination.clone()).expect("archive publication");
        let temporary = publication.temporary_display_path();

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
        let root =
            Dir::open_ambient_dir(temp.path(), ambient_authority()).expect("open archive root");
        let publication = publication(root, local_path.clone()).expect("archive publication");

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

        let destination =
            ArchiveDestination::new(temp.path().to_path_buf(), &ApplicationCancellation::new());
        let root = destination.open_root().expect("open archive root");
        let path = destination
            .next_available_path(&root, ArchiveName::new(123))
            .expect("next archive path");

        assert_eq!(path, temp.path().join("123-1-archive.gzip"));
        drop(root);
        temp.close().expect("cleanup");
    }

    #[test]
    fn canceled_archive_publication_stops_before_creating_the_destination() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path().join("archives");
        let cancellation = ApplicationCancellation::new();
        cancellation.request();

        let error = ArchiveDestination::new(root.clone(), &cancellation)
            .publish(b"archive bytes".to_vec())
            .expect_err("pre-canceled publication must stop before filesystem mutation");

        assert!(matches!(error, CliError::OperationCanceled));
        assert!(!root.exists());
        temp.close().expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn archive_destination_root_retarget_does_not_redirect_publication() {
        let temp = TempDir::new().expect("temp dir");
        let selected_root = temp.path().join("selected");
        let anchored_root = temp.path().join("anchored");
        std::fs::create_dir(&selected_root).expect("selected archive root");
        let destination =
            ArchiveDestination::new(selected_root.clone(), &ApplicationCancellation::new());
        let root = destination.open_root().expect("anchor archive root");

        std::fs::rename(&selected_root, &anchored_root).expect("move selected root");
        std::fs::create_dir(&selected_root).expect("replacement root");
        let outside_guard = selected_root.join("outside-guard");
        std::fs::write(&outside_guard, b"outside bytes").expect("outside guard");

        let written = destination
            .publish_into(root, b"archive bytes")
            .expect("publish through anchored root");
        let file_name = written.file_name().expect("archive file name");

        assert_eq!(
            std::fs::read(anchored_root.join(file_name)).expect("anchored archive"),
            b"archive bytes"
        );
        assert!(!selected_root.join(file_name).exists());
        assert_eq!(
            std::fs::read(outside_guard).expect("outside guard bytes"),
            b"outside bytes",
            "publication must not modify the replacement tree"
        );
        temp.close().expect("cleanup");
    }

    #[test]
    fn archive_publication_does_not_overwrite_existing_archive() {
        let temp = TempDir::new().expect("temp dir");
        let local_path = temp.path().join("123-archive.gzip");
        std::fs::write(&local_path, b"existing archive").expect("existing archive");
        let root =
            Dir::open_ambient_dir(temp.path(), ambient_authority()).expect("open archive root");

        let error = publication(root, local_path.clone())
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
        let root =
            Dir::open_ambient_dir(temp.path(), ambient_authority()).expect("open archive root");
        let publication =
            publication(root, destination.clone()).expect("create sibling temporary archive");
        let temporary = publication.temporary_display_path();
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
        let root =
            Dir::open_ambient_dir(temp.path(), ambient_authority()).expect("open archive root");
        let mut publication =
            publication(root, initial_destination).expect("create sibling temporary archive");
        let temporary = publication.temporary_display_path();
        let destination = temp.path().join("missing").join("final-archive.gzip");
        publication.destination = destination.clone();
        publication.destination_name = PathBuf::from("missing").join("final-archive.gzip");

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
        let root =
            Dir::open_ambient_dir(temp.path(), ambient_authority()).expect("open archive root");
        let publication =
            publication(root, destination.clone()).expect("create sibling temporary archive");
        let temporary = publication.temporary_display_path();

        drop(publication);

        assert!(!destination.exists());
        assert!(!temporary.exists());
        temp.close().expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn abandoned_publication_cleanup_stays_with_the_anchored_root() {
        let temp = TempDir::new().expect("temp dir");
        let selected_root = temp.path().join("selected");
        let anchored_root = temp.path().join("anchored");
        std::fs::create_dir(&selected_root).expect("selected archive root");
        let root = Dir::open_ambient_dir(&selected_root, ambient_authority())
            .expect("anchor archive root");
        let destination = selected_root.join("abandoned.gzip");
        let publication = publication(root, destination).expect("create sibling temporary archive");
        let temporary_name = publication.temporary_path().to_path_buf();

        std::fs::rename(&selected_root, &anchored_root).expect("move selected root");
        std::fs::create_dir(&selected_root).expect("replacement root");
        let outside_temporary = selected_root.join(&temporary_name);
        std::fs::write(&outside_temporary, b"outside replacement")
            .expect("outside replacement temporary");

        drop(publication);

        assert!(!anchored_root.join(&temporary_name).exists());
        assert_eq!(
            std::fs::read(outside_temporary).expect("outside temporary bytes"),
            b"outside replacement",
            "Drop cleanup must remain relative to the anchored root"
        );
        temp.close().expect("cleanup");
    }

    #[test]
    fn failed_archive_write_removes_the_sibling_temporary() {
        let temp = TempDir::new().expect("temp dir");
        let destination = temp.path().join("final-archive.gzip");
        let temporary_name = PathBuf::from(".final-archive.gzip.test.tmp");
        let temporary = temp.path().join(&temporary_name);
        std::fs::write(&temporary, b"temporary").expect("temporary file");
        let root =
            Dir::open_ambient_dir(temp.path(), ambient_authority()).expect("open archive root");
        let read_only = root
            .open(&temporary_name)
            .expect("read-only temporary handle");
        let publication = ArchivePublication {
            root,
            destination: destination.clone(),
            destination_name: PathBuf::from("final-archive.gzip"),
            state: PublicationState::Open {
                temporary: temporary_name,
                output: read_only,
            },
            cancellation: ApplicationCancellation::new(),
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
