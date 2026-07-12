//! Runtime coordination for generated-document ownership.
//!
//! Domain work finishes before this module is entered. Reconciliation then
//! holds the generation lock while it recovers any interrupted journal,
//! validates the exact generation baseline, preflights every mutation, and
//! coordinates individually atomic file replacements with version-3
//! ownership state.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsStr;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use conkit_signature::{CatalogPath, FileCatalog};

use super::ownership::{
    ContentDigest, OwnedCatalog, OwnedFile, OwnershipJournal, OwnershipManifest, ReservationMarker,
    VersionProbe,
};
use super::path::{PathRole, PortableCatalogPathKey, ResolvedPath};
use super::store::{ContractsStore, ExistingOutputPolicy, GeneratedContracts, GenerationReceipt};
use crate::contracts::ContractDocumentPath;
use crate::error::CliError;

/// Ownership metadata found during a runtime load.
enum OwnershipDocument {
    Missing,
    Current(OwnershipManifest),
}

/// Exclusive contracts-root generation lock.
pub(super) struct GenerationLock {
    _file: File,
}

/// One validated document requested by a completed domain generation.
struct GeneratedOutput {
    document: ContractDocumentPath,
    destination: PathBuf,
    bytes: Vec<u8>,
    digest: ContentDigest,
}

/// On-disk bytes expected immediately before one generated write.
enum DestinationExpectation {
    Missing,
    Digest(ContentDigest),
    Reservation,
}

/// One preflighted generated-output mutation.
enum OutputMutation {
    Write {
        output: GeneratedOutput,
        expected: DestinationExpectation,
    },
    Remove {
        owned: OwnedFile,
        destination: PathBuf,
    },
}

/// Digest-bound reservation cleanup performed during rollback or recovery.
struct Reservation {
    destination: PathBuf,
    marker: Vec<u8>,
}

/// One locked, fully preflighted ownership reconciliation.
pub(super) struct CatalogReconciliation<'store> {
    store: &'store ContractsStore,
    root: ResolvedPath,
    _lock: GenerationLock,
    generation: u64,
    before: OwnedCatalog,
    after: OwnedCatalog,
    mutations: Vec<OutputMutation>,
    adopted_count: usize,
}

impl ContractsStore {
    /// Recovers an interrupted generated-document update before layout parsing.
    ///
    /// Missing or committed ownership requires no lock and creates no metadata.
    pub(crate) fn recover_interrupted_generation(&self) -> Result<(), CliError> {
        self.validate_reserved_namespace()?;
        let needs_recovery = OwnershipDocument::load(self)?.is_updating();
        if !needs_recovery {
            return Ok(());
        }

        let root = ResolvedPath::new(PathRole::Contracts, self.path().to_path_buf())?;
        let _lock = GenerationLock::acquire(self, &root)?;
        self.remove_abandoned_manifest_temporaries()?;
        self.validate_reserved_namespace()?;

        let ownership = OwnershipDocument::load(self)?;
        if ownership.is_updating() {
            let _ = ownership.recover(self, &root)?;
        }

        Ok(())
    }
}

impl OwnershipDocument {
    fn load(store: &ContractsStore) -> Result<Self, CliError> {
        let path = OwnershipManifest::path(store.path());
        let bytes = match fs_err::read(&path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(Self::Missing),
            Err(source) => return Err(CliError::Io(source)),
        };
        let probe = VersionProbe::from_bytes(&path, &bytes)?;

        if probe.version() == OwnershipManifest::VERSION {
            Ok(Self::Current(OwnershipManifest::from_bytes(&path, &bytes)?))
        } else {
            Err(CliError::InvalidGeneratedOwnership {
                path,
                message: format!("unsupported ownership version {}", probe.version()),
            })
        }
    }

    fn is_updating(&self) -> bool {
        matches!(self, Self::Current(manifest) if manifest.is_updating())
    }

    fn recover(
        self,
        store: &ContractsStore,
        root: &ResolvedPath,
    ) -> Result<(u64, OwnedCatalog), CliError> {
        match self {
            Self::Missing => Ok((0, OwnedCatalog::default())),
            Self::Current(manifest) => match manifest.into_journal() {
                OwnershipJournal::Committed { generation, files } => Ok((generation, files)),
                OwnershipJournal::Updating {
                    generation,
                    before,
                    after,
                } => Self::recover_updating(store, root, generation, before, after),
            },
        }
    }

    fn recover_updating(
        store: &ContractsStore,
        root: &ResolvedPath,
        generation: u64,
        before: OwnedCatalog,
        after: OwnedCatalog,
    ) -> Result<(u64, OwnedCatalog), CliError> {
        let paths = before
            .entries()
            .map(|file| file.path().to_owned())
            .chain(after.entries().map(|file| file.path().to_owned()))
            .collect::<BTreeSet<_>>();
        let mut recovered_files = Vec::new();
        let mut reservations = Vec::new();

        for value in paths {
            let previous = before.find(&value);
            let next = after.find(&value);
            let logical =
                CatalogPath::new(value).map_err(|source| CliError::InvalidGeneratedOwnership {
                    path: OwnershipManifest::path(store.path()),
                    message: source.to_string(),
                })?;
            let destination = store.validated_output_path(root, &logical)?;

            match fs_err::symlink_metadata(&destination) {
                Ok(metadata) if !metadata.file_type().is_file() => {
                    return Err(CliError::GeneratedOutputNotFile { path: destination });
                }
                Ok(_) => {
                    let bytes = fs_err::read(&destination)?;
                    let digest = ContentDigest::of(&bytes);
                    if let Some(file) = next
                        && digest == *file.digest()
                    {
                        recovered_files.push(file.clone());
                    } else if let Some(file) = previous
                        && digest == *file.digest()
                    {
                        recovered_files.push(file.clone());
                    } else if let Some(file) = next
                        && bytes == ReservationMarker::new(generation, file).to_bytes()?
                    {
                        reservations.push(Reservation::new(destination, bytes));
                        if let Some(file) = previous {
                            recovered_files.push(file.clone());
                        }
                    } else {
                        return Err(CliError::GeneratedOwnershipRecoveryConflict {
                            path: destination,
                        });
                    }
                }
                Err(source) if source.kind() == ErrorKind::NotFound => {
                    if let (Some(_), Some(file)) = (previous, next) {
                        recovered_files.push(file.clone());
                    }
                }
                Err(source) => return Err(CliError::Io(source)),
            }
        }

        let recovered = OwnedCatalog::from_files(recovered_files);
        let ownership_path = OwnershipManifest::path(store.path());
        recovered.validate(&ownership_path)?;
        for reservation in reservations {
            reservation.remove_if_unchanged()?;
        }

        let committed = OwnershipManifest::committed(generation, recovered.clone());
        store.atomic_write(root, &ownership_path, &committed.to_bytes(&ownership_path)?)?;

        Ok((generation, recovered))
    }
}

impl GenerationLock {
    /// File name recognized by contracts-store namespace validation.
    pub(super) const FILE_NAME: &'static str = "generation.lock";

    fn acquire(store: &ContractsStore, root: &ResolvedPath) -> Result<Self, CliError> {
        let directory = store.path().join(OwnershipManifest::DIRECTORY);
        root.ensure_generated_path(&directory)?;
        fs_err::create_dir_all(&directory)?;

        let path = directory.join(Self::FILE_NAME);
        root.ensure_generated_path(&path)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(CliError::GenerationInProgress { path }),
            Err(TryLockError::Error(source)) => Err(CliError::Io(source)),
        }
    }
}

impl GeneratedOutput {
    fn collect(
        store: &ContractsStore,
        root: &ResolvedPath,
        catalog: FileCatalog,
    ) -> Result<Vec<Self>, CliError> {
        let manifest = OwnershipManifest::path(store.path());
        let mut keys = BTreeMap::<PortableCatalogPathKey, String>::new();
        let mut outputs = Vec::with_capacity(catalog.len());

        for (logical, bytes) in catalog.into_entries() {
            let document = ContractDocumentPath::try_from(logical.clone()).map_err(|_| {
                CliError::InvalidGeneratedOwnership {
                    path: manifest.clone(),
                    message: format!(
                        "generated path {logical} must be a direct root .yml or .yaml combined document"
                    ),
                }
            })?;
            let key = PortableCatalogPathKey::new(&logical);
            if let Some(previous) = keys.insert(key, logical.as_str().to_owned()) {
                return Err(CliError::PortableGeneratedPathCollision {
                    first: previous,
                    second: logical.as_str().to_owned(),
                });
            }
            let destination = store.validated_output_path(root, &logical)?;
            outputs.push(Self {
                document,
                destination,
                digest: ContentDigest::of(&bytes),
                bytes,
            });
        }

        Ok(outputs)
    }

    fn logical(&self) -> &CatalogPath {
        self.document.as_catalog_path()
    }

    fn owned_file(&self) -> OwnedFile {
        OwnedFile::new(self.logical().as_str().to_owned(), self.digest.clone())
    }

    fn marker(&self, generation: u64) -> Result<Vec<u8>, CliError> {
        ReservationMarker::new(generation, &self.owned_file()).to_bytes()
    }

    fn validate_host_spelling(&self, root: &Path) -> Result<(), CliError> {
        let mut directory = root.to_path_buf();
        for component in self.logical().as_str().split('/') {
            let entries = match fs_err::read_dir(&directory) {
                Ok(entries) => entries,
                Err(source) if source.kind() == ErrorKind::NotFound => return Ok(()),
                Err(source) => return Err(CliError::Io(source)),
            };
            let expected = OsStr::new(component);
            let mut exact = None;
            for entry in entries {
                let entry = entry?;
                let name = entry.file_name();
                if name.eq_ignore_ascii_case(expected) && name != expected {
                    return Err(CliError::PortableGeneratedPathCollision {
                        first: entry.path().display().to_string(),
                        second: self.logical().as_str().to_owned(),
                    });
                }
                if name == expected {
                    exact = Some(entry.path());
                }
            }
            let Some(path) = exact else {
                return Ok(());
            };
            directory = path;
        }
        Ok(())
    }
}

impl OutputMutation {
    fn reserve(&mut self, root: &ResolvedPath, generation: u64) -> Result<(), CliError> {
        let Self::Write { output, expected } = self else {
            return Ok(());
        };
        if !matches!(expected, DestinationExpectation::Missing) {
            return Ok(());
        }

        root.ensure_generated_path(&output.destination)?;
        if let Some(parent) = output.destination.parent() {
            fs_err::create_dir_all(parent)?;
        }
        root.ensure_generated_path(&output.destination)?;
        let marker = output.marker(generation)?;
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output.destination)
        {
            Ok(file) => file,
            Err(source) if source.kind() == ErrorKind::AlreadyExists => {
                return Err(CliError::UnownedGeneratedOutput {
                    path: output.destination.clone(),
                });
            }
            Err(source) => return Err(CliError::Io(source)),
        };
        *expected = DestinationExpectation::Reservation;
        file.write_all(&marker)?;
        file.sync_all()?;
        Ok(())
    }

    fn apply(
        &self,
        store: &ContractsStore,
        root: &ResolvedPath,
        generation: u64,
    ) -> Result<(), CliError> {
        match self {
            Self::Write { output, expected } => {
                output.verify(expected, generation)?;
                store.atomic_write(root, &output.destination, &output.bytes)
            }
            Self::Remove { owned, destination } => {
                Self::remove_owned(store, root, owned, destination)
            }
        }
    }

    fn remove_owned(
        store: &ContractsStore,
        root: &ResolvedPath,
        owned: &OwnedFile,
        destination: &Path,
    ) -> Result<(), CliError> {
        root.ensure_generated_path(destination)?;
        match fs_err::symlink_metadata(destination) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(CliError::GeneratedOutputNotFile {
                    path: destination.to_path_buf(),
                });
            }
            Ok(_) => {
                if ContentDigest::of(&fs_err::read(destination)?) != *owned.digest() {
                    return Err(CliError::ModifiedGeneratedOutput {
                        path: destination.to_path_buf(),
                    });
                }
                fs_err::remove_file(destination)?;
            }
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(()),
            Err(source) => return Err(CliError::Io(source)),
        }

        let mut directory = destination.parent();
        while let Some(candidate) = directory {
            if candidate == store.path() || candidate.ends_with(OwnershipManifest::DIRECTORY) {
                break;
            }
            match fs_err::remove_dir(candidate) {
                Ok(()) => directory = candidate.parent(),
                Err(source)
                    if matches!(
                        source.kind(),
                        ErrorKind::NotFound | ErrorKind::DirectoryNotEmpty
                    ) =>
                {
                    break;
                }
                Err(source) => return Err(CliError::Io(source)),
            }
        }

        Ok(())
    }

    fn reservation(&self, generation: u64) -> Result<Option<Reservation>, CliError> {
        match self {
            Self::Write {
                output,
                expected: DestinationExpectation::Reservation,
            } => Ok(Some(Reservation::new(
                output.destination.clone(),
                output.marker(generation)?,
            ))),
            Self::Write { .. } | Self::Remove { .. } => Ok(None),
        }
    }
}

impl GeneratedOutput {
    fn verify(&self, expected: &DestinationExpectation, generation: u64) -> Result<(), CliError> {
        let bytes = match fs_err::read(&self.destination) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == ErrorKind::NotFound => {
                return Err(CliError::GeneratedOwnershipRecoveryConflict {
                    path: self.destination.clone(),
                });
            }
            Err(source) => return Err(CliError::Io(source)),
        };
        let matches = match expected {
            DestinationExpectation::Missing => false,
            DestinationExpectation::Digest(digest) => ContentDigest::of(&bytes) == *digest,
            DestinationExpectation::Reservation => bytes == self.marker(generation)?,
        };
        if matches {
            Ok(())
        } else {
            Err(CliError::GeneratedOwnershipRecoveryConflict {
                path: self.destination.clone(),
            })
        }
    }
}

impl Reservation {
    fn new(destination: PathBuf, marker: Vec<u8>) -> Self {
        Self {
            destination,
            marker,
        }
    }

    fn remove_if_unchanged(self) -> Result<(), CliError> {
        match fs_err::read(&self.destination) {
            Ok(bytes) if bytes == self.marker => {
                fs_err::remove_file(self.destination)?;
                Ok(())
            }
            Err(source) if source.kind() == ErrorKind::NotFound => Ok(()),
            Ok(_) => Err(CliError::GeneratedOwnershipRecoveryConflict {
                path: self.destination,
            }),
            Err(source) => Err(CliError::Io(source)),
        }
    }
}

impl<'store> CatalogReconciliation<'store> {
    /// Locks, recovers, validates, and preflights one generated catalog update.
    pub(super) fn new(
        store: &'store ContractsStore,
        generated: GeneratedContracts,
        policy: ExistingOutputPolicy,
    ) -> Result<Self, CliError> {
        let (baseline, catalog) = generated.into_parts();
        store.validate_reserved_namespace()?;
        let root = ResolvedPath::new(PathRole::Contracts, store.path().to_path_buf())?;
        let generation_lock = GenerationLock::acquire(store, &root)?;
        store.remove_abandoned_manifest_temporaries()?;
        store.validate_reserved_namespace()?;

        let ownership_path = OwnershipManifest::path(store.path());
        let (previous_generation, before) =
            OwnershipDocument::load(store)?.recover(store, &root)?;
        let current = store.read()?;
        if current != baseline {
            return Err(CliError::GenerationInputChanged {
                path: store.path().to_path_buf(),
            });
        }

        let outputs = GeneratedOutput::collect(store, &root, catalog)?;
        let after =
            OwnedCatalog::from_files(outputs.iter().map(GeneratedOutput::owned_file).collect());
        after.validate(&ownership_path)?;
        before.validate_transition(&after, &ownership_path)?;
        for output in &outputs {
            output.validate_host_spelling(store.path())?;
        }

        let generation = previous_generation.checked_add(1).ok_or_else(|| {
            CliError::InvalidGeneratedOwnership {
                path: ownership_path,
                message: "ownership generation is exhausted".to_owned(),
            }
        })?;
        let (mutations, adopted_count) =
            Self::preflight(store, &root, &before, &after, outputs, policy)?;

        Ok(Self {
            store,
            root,
            _lock: generation_lock,
            generation,
            before,
            after,
            mutations,
            adopted_count,
        })
    }

    fn preflight(
        store: &ContractsStore,
        root: &ResolvedPath,
        before: &OwnedCatalog,
        after: &OwnedCatalog,
        outputs: Vec<GeneratedOutput>,
        policy: ExistingOutputPolicy,
    ) -> Result<(Vec<OutputMutation>, usize), CliError> {
        let mut mutations = Vec::new();
        let mut adopted_count = 0;
        let mut existing = Vec::new();

        for output in outputs {
            let previous = before.find(output.logical().as_str());
            match fs_err::symlink_metadata(&output.destination) {
                Ok(metadata) if !metadata.file_type().is_file() => {
                    return Err(CliError::GeneratedOutputNotFile {
                        path: output.destination,
                    });
                }
                Ok(_) => {
                    existing.push(output.destination.clone());
                    let disk_digest = ContentDigest::of(&fs_err::read(&output.destination)?);
                    if let Some(previous) = previous {
                        if disk_digest != *previous.digest() {
                            return Err(CliError::ModifiedGeneratedOutput {
                                path: output.destination,
                            });
                        }
                        if disk_digest != output.digest {
                            mutations.push(OutputMutation::Write {
                                output,
                                expected: DestinationExpectation::Digest(disk_digest),
                            });
                        }
                    } else if policy == ExistingOutputPolicy::AdoptMatching
                        && disk_digest == output.digest
                    {
                        adopted_count += 1;
                    } else {
                        return Err(CliError::UnownedGeneratedOutput {
                            path: output.destination,
                        });
                    }
                }
                Err(source) if source.kind() == ErrorKind::NotFound => {
                    mutations.push(OutputMutation::Write {
                        output,
                        expected: DestinationExpectation::Missing,
                    });
                }
                Err(source) => return Err(CliError::Io(source)),
            }
        }

        for owned in before
            .entries()
            .filter(|owned| after.find(owned.path()).is_none())
        {
            let logical = CatalogPath::new(owned.path().to_owned()).map_err(|source| {
                CliError::InvalidGeneratedOwnership {
                    path: OwnershipManifest::path(store.path()),
                    message: source.to_string(),
                }
            })?;
            let destination = store.validated_output_path(root, &logical)?;
            match fs_err::symlink_metadata(&destination) {
                Ok(metadata) if !metadata.file_type().is_file() => {
                    return Err(CliError::GeneratedOutputNotFile { path: destination });
                }
                Ok(_) => {
                    if ContentDigest::of(&fs_err::read(&destination)?) != *owned.digest() {
                        return Err(CliError::ModifiedGeneratedOutput { path: destination });
                    }
                    existing.push(destination.clone());
                    mutations.push(OutputMutation::Remove {
                        owned: owned.clone(),
                        destination,
                    });
                }
                Err(source) if source.kind() == ErrorKind::NotFound => {}
                Err(source) => return Err(CliError::Io(source)),
            }
        }

        let mut identities = HashMap::<same_file::Handle, PathBuf>::new();
        for path in existing {
            let identity = same_file::Handle::from_path(&path)?;
            if let Some(first) = identities.insert(identity, path.clone()) {
                return Err(CliError::GeneratedOutputAlias {
                    first,
                    second: path,
                });
            }
        }

        Ok((mutations, adopted_count))
    }

    /// Applies the preflighted update and commits its ownership manifest.
    pub(super) fn apply(mut self) -> Result<GenerationReceipt, CliError> {
        let ownership_path = OwnershipManifest::path(self.store.path());
        let updating =
            OwnershipManifest::updating(self.generation, self.before.clone(), self.after.clone());
        self.store.atomic_write(
            &self.root,
            &ownership_path,
            &updating.to_bytes(&ownership_path)?,
        )?;

        if let Err(source) = self.reserve_outputs() {
            return match self.rollback_reservations(&ownership_path) {
                Ok(()) => Err(source),
                Err(rollback) => Err(CliError::GeneratedOwnershipRollback {
                    path: ownership_path,
                    message: rollback.to_string(),
                }),
            };
        }

        for mutation in &self.mutations {
            mutation.apply(self.store, &self.root, self.generation)?;
        }
        self.verify_after_outputs()?;

        let committed = OwnershipManifest::committed(self.generation, self.after);
        self.store.atomic_write(
            &self.root,
            &ownership_path,
            &committed.to_bytes(&ownership_path)?,
        )?;

        Ok(GenerationReceipt::new(self.adopted_count))
    }

    fn reserve_outputs(&mut self) -> Result<(), CliError> {
        for mutation in &mut self.mutations {
            mutation.reserve(&self.root, self.generation)?;
        }
        Ok(())
    }

    fn rollback_reservations(&self, ownership_path: &Path) -> Result<(), CliError> {
        for mutation in &self.mutations {
            if let Some(reservation) = mutation.reservation(self.generation)? {
                reservation.remove_if_unchanged()?;
            }
        }

        let restored = OwnershipManifest::committed(self.generation, self.before.clone());
        self.store.atomic_write(
            &self.root,
            ownership_path,
            &restored.to_bytes(ownership_path)?,
        )
    }

    fn verify_after_outputs(&self) -> Result<(), CliError> {
        for owned in self.after.entries() {
            let logical = CatalogPath::new(owned.path().to_owned()).map_err(|source| {
                CliError::InvalidGeneratedOwnership {
                    path: OwnershipManifest::path(self.store.path()),
                    message: source.to_string(),
                }
            })?;
            let destination = self.store.validated_output_path(&self.root, &logical)?;
            match fs_err::symlink_metadata(&destination) {
                Ok(metadata) if !metadata.file_type().is_file() => {
                    return Err(CliError::GeneratedOutputNotFile { path: destination });
                }
                Ok(_) if ContentDigest::of(&fs_err::read(&destination)?) != *owned.digest() => {
                    return Err(CliError::GeneratedOwnershipRecoveryConflict { path: destination });
                }
                Ok(_) => {}
                Err(source) if source.kind() == ErrorKind::NotFound => {
                    return Err(CliError::GeneratedOwnershipRecoveryConflict { path: destination });
                }
                Err(source) => return Err(CliError::Io(source)),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use assert_fs::fixture::{ChildPath, TempDir};
    use assert_fs::prelude::*;
    use conkit_signature::{CatalogPath, FileCatalog};

    use super::{CatalogReconciliation, GenerationLock};
    use crate::catalog::ownership::{
        ContentDigest, OwnedCatalog, OwnedFile, OwnershipJournal, OwnershipManifest,
        ReservationMarker,
    };
    use crate::catalog::{
        ContractsStore, ExistingOutputPolicy, GeneratedContracts, PathRole, ResolvedPath,
    };
    use crate::error::CliError;

    struct ReconciliationFixture {
        _temp: TempDir,
        contracts: ChildPath,
        store: ContractsStore,
    }

    impl ReconciliationFixture {
        fn new() -> Self {
            let temp = TempDir::new().expect("temporary root");
            let contracts = temp.child("contracts");
            let store = ContractsStore::new(contracts.path().to_path_buf());
            Self {
                _temp: temp,
                contracts,
                store,
            }
        }

        fn generation(&self, entries: &[(&str, &[u8])]) -> GeneratedContracts {
            let mut files = FileCatalog::new();
            for (path, bytes) in entries {
                files
                    .insert(
                        CatalogPath::new((*path).to_owned()).expect("catalog path"),
                        bytes.to_vec(),
                    )
                    .expect("unique generated path");
            }
            let baseline = self.store.read_optional().expect("generation baseline");
            GeneratedContracts::new(baseline, files)
        }

        fn committed_catalog(&self) -> (u64, OwnedCatalog) {
            let path = OwnershipManifest::path(self.store.path());
            let manifest = OwnershipManifest::from_bytes(
                &path,
                &fs_err::read(&path).expect("ownership bytes"),
            )
            .expect("ownership manifest");
            match manifest.into_journal() {
                OwnershipJournal::Committed { generation, files } => (generation, files),
                OwnershipJournal::Updating { .. } => panic!("expected committed ownership"),
            }
        }

        fn write_manifest(&self, manifest: &OwnershipManifest) {
            let path = OwnershipManifest::path(self.store.path());
            fs_err::create_dir_all(path.parent().expect("manifest parent"))
                .expect("metadata directory");
            fs_err::write(
                &path,
                manifest.to_bytes(&path).expect("manifest serialization"),
            )
            .expect("write ownership manifest");
        }

        fn assert_no_atomic_manifest_temporaries(&self) {
            let metadata = self.store.path().join(OwnershipManifest::DIRECTORY);
            for entry in fs_err::read_dir(metadata).expect("ownership metadata directory") {
                let entry = entry.expect("ownership metadata entry");
                let recognized = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.strip_prefix(OwnershipManifest::TEMPORARY_PREFIX))
                    .is_some_and(|suffix| {
                        suffix.len() == OwnershipManifest::ATOMIC_TEMPORARY_SUFFIX_LENGTH
                            && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
                    });
                assert!(
                    !recognized,
                    "unexpected ownership temporary: {}",
                    entry.path().display(),
                );
            }
        }
    }

    #[test]
    fn generated_catalog_accepts_direct_mixed_case_yaml_documents() {
        let fixture = ReconciliationFixture::new();

        fixture
            .store
            .write_generated(
                fixture.generation(&[("first.YML", b"first\n"), ("second.YaMl", b"second\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect("direct mixed-case YAML documents");

        fixture
            .contracts
            .child("first.YML")
            .assert(b"first\n" as &[u8]);
        fixture
            .contracts
            .child("second.YaMl")
            .assert(b"second\n" as &[u8]);
        let (_, owned) = fixture.committed_catalog();
        assert_eq!(
            owned.entries().map(OwnedFile::path).collect::<Vec<_>>(),
            ["first.YML", "second.YaMl"],
        );
    }

    #[test]
    fn generated_catalog_rejects_non_yaml_path_without_mutation() {
        let fixture = ReconciliationFixture::new();

        let error = fixture
            .store
            .write_generated(
                fixture.generation(&[("manual.txt", b"generated\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect_err("non-YAML generated path must fail");

        assert!(
            error
                .to_string()
                .contains("must be a direct root .yml or .yaml combined document"),
            "unexpected error: {error}",
        );
        fixture
            .contracts
            .child("manual.txt")
            .assert(predicates::path::missing());
        fixture
            .contracts
            .child(".contract-kit/generated-files.json")
            .assert(predicates::path::missing());
    }

    #[test]
    fn generated_catalog_rejects_ascii_case_collision_without_mutation() {
        let fixture = ReconciliationFixture::new();

        let error = fixture
            .store
            .write_generated(
                fixture.generation(&[("Lib.yaml", b"first\n"), ("lib.yaml", b"second\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect_err("case-equivalent generated paths must fail");

        assert!(
            error
                .to_string()
                .contains("portable generated path collision")
        );
        fixture
            .contracts
            .child("Lib.yaml")
            .assert(predicates::path::missing());
        fixture
            .contracts
            .child("lib.yaml")
            .assert(predicates::path::missing());
        fixture
            .contracts
            .child(".contract-kit/generated-files.json")
            .assert(predicates::path::missing());
    }

    #[test]
    fn generated_catalog_rejects_nonportable_component_without_mutation() {
        let fixture = ReconciliationFixture::new();

        let error = fixture
            .store
            .write_generated(
                fixture.generation(&[("CON.yml", b"generated\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect_err("nonportable generated component must fail");

        assert!(error.to_string().contains("Windows reserved device name"));
        fixture
            .contracts
            .child("CON.yml")
            .assert(predicates::path::missing());
        fixture
            .contracts
            .child(".contract-kit/generated-files.json")
            .assert(predicates::path::missing());
    }

    #[test]
    fn recovery_probe_does_not_create_metadata_for_missing_or_committed_ownership() {
        let missing = ReconciliationFixture::new();
        missing
            .store
            .recover_interrupted_generation()
            .expect("missing ownership needs no recovery");
        missing
            .contracts
            .child(OwnershipManifest::DIRECTORY)
            .assert(predicates::path::missing());

        let committed = ReconciliationFixture::new();
        committed.write_manifest(&OwnershipManifest::committed(1, OwnedCatalog::default()));
        let ownership_path = OwnershipManifest::path(committed.store.path());
        let before = fs_err::read(&ownership_path).expect("committed ownership bytes");

        committed
            .store
            .recover_interrupted_generation()
            .expect("committed ownership needs no recovery");

        assert_eq!(
            fs_err::read(&ownership_path).expect("unchanged ownership bytes"),
            before,
        );
        committed
            .contracts
            .child(".contract-kit/generation.lock")
            .assert(predicates::path::missing());
    }

    #[test]
    fn older_ownership_versions_are_rejected_without_migration() {
        for version in [1, 2] {
            let fixture = ReconciliationFixture::new();
            fixture
                .contracts
                .child(".contract-kit")
                .create_dir_all()
                .expect("metadata directory");
            fixture
                .contracts
                .child(".contract-kit/generated-files.json")
                .write_str(&format!(r#"{{"version":{version},"journal":{{}}}}"#))
                .expect("obsolete manifest");
            fixture
                .contracts
                .child("main.yml")
                .write_binary(b"user bytes\n")
                .expect("existing output");

            let error = fixture
                .store
                .write_generated(
                    fixture.generation(&[("main.yml", b"generated\n")]),
                    ExistingOutputPolicy::AdoptMatching,
                )
                .expect_err("obsolete ownership must not be migrated");

            assert!(
                error
                    .to_string()
                    .contains(&format!("unsupported ownership version {version}"))
            );
            fixture
                .contracts
                .child("main.yml")
                .assert(b"user bytes\n" as &[u8]);
        }
    }

    #[test]
    fn requested_paths_reject_case_equivalent_host_entries() {
        let fixture = ReconciliationFixture::new();
        fixture.contracts.create_dir_all().expect("contracts root");
        fixture
            .contracts
            .child("Manual.yaml")
            .write_binary(b"manual\n")
            .expect("manual host entry");

        let error = fixture
            .store
            .write_generated(
                fixture.generation(&[("manual.yaml", b"generated\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect_err("portable host spelling collision must fail");

        assert!(
            error
                .to_string()
                .contains("portable generated path collision")
        );
        fixture
            .contracts
            .child("Manual.yaml")
            .assert(b"manual\n" as &[u8]);
        fixture
            .contracts
            .child(".contract-kit/generated-files.json")
            .assert(predicates::path::missing());
    }

    #[test]
    fn temporal_ascii_case_change_is_rejected_without_mutation() {
        let fixture = ReconciliationFixture::new();
        fixture
            .store
            .write_generated(
                fixture.generation(&[("Lib.yaml", b"old\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect("initial generation");
        let ownership = fixture
            .contracts
            .child(".contract-kit/generated-files.json");
        let ownership_before = fs_err::read(ownership.path()).expect("ownership bytes");

        let error = fixture
            .store
            .write_generated(
                fixture.generation(&[("lib.yaml", b"new\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect_err("case-only transition must fail");

        fixture
            .contracts
            .child("Lib.yaml")
            .assert(b"old\n" as &[u8]);
        assert_eq!(
            fs_err::read(ownership.path()).expect("unchanged ownership"),
            ownership_before,
        );
        assert!(error.to_string().contains("changes only ASCII case"));
    }

    #[test]
    fn modified_current_and_stale_outputs_fail_before_current_writes() {
        let fixture = ReconciliationFixture::new();
        fixture
            .store
            .write_generated(
                fixture.generation(&[("current.yaml", b"old\n"), ("stale.yaml", b"stale\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect("initial generation");
        let ownership = fixture
            .contracts
            .child(".contract-kit/generated-files.json");
        let ownership_before = fs_err::read(ownership.path()).expect("ownership bytes");
        fixture
            .contracts
            .child("stale.yaml")
            .write_binary(b"manual stale\n")
            .expect("modify stale output");

        let error = fixture
            .store
            .write_generated(
                fixture.generation(&[("current.yaml", b"new\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect_err("modified stale output must fail preflight");
        assert!(error.to_string().contains("modified outside"));
        fixture
            .contracts
            .child("current.yaml")
            .assert(b"old\n" as &[u8]);
        fixture
            .contracts
            .child("stale.yaml")
            .assert(b"manual stale\n" as &[u8]);
        assert_eq!(
            fs_err::read(ownership.path()).expect("unchanged ownership"),
            ownership_before,
        );

        fixture
            .contracts
            .child("stale.yaml")
            .write_binary(b"stale\n")
            .expect("restore stale output");
        fixture
            .contracts
            .child("current.yaml")
            .write_binary(b"manual current\n")
            .expect("modify current output");
        let error = fixture
            .store
            .write_generated(
                fixture.generation(&[("current.yaml", b"new\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect_err("modified current output must fail preflight");
        assert!(error.to_string().contains("modified outside"));
        fixture
            .contracts
            .child("current.yaml")
            .assert(b"manual current\n" as &[u8]);
        fixture
            .contracts
            .child("stale.yaml")
            .assert(b"stale\n" as &[u8]);
        assert_eq!(
            fs_err::read(ownership.path()).expect("unchanged ownership"),
            ownership_before,
        );
    }

    #[test]
    fn stale_directory_fails_preflight_before_current_write() {
        let fixture = ReconciliationFixture::new();
        fixture
            .store
            .write_generated(
                fixture.generation(&[("current.yaml", b"old\n"), ("stale.yaml", b"stale\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect("initial generation");
        let ownership = fixture
            .contracts
            .child(".contract-kit/generated-files.json");
        let ownership_before = fs_err::read(ownership.path()).expect("ownership bytes");
        fs_err::remove_file(fixture.contracts.child("stale.yaml").path())
            .expect("remove stale file");
        fixture
            .contracts
            .child("stale.yaml")
            .create_dir_all()
            .expect("stale directory");

        let error = fixture
            .store
            .write_generated(
                fixture.generation(&[("current.yaml", b"new\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect_err("stale directory must fail preflight");
        assert!(error.to_string().contains("not a file"));
        fixture
            .contracts
            .child("current.yaml")
            .assert(b"old\n" as &[u8]);
        fixture
            .contracts
            .child("stale.yaml")
            .assert(predicates::path::is_dir());
        assert_eq!(
            fs_err::read(ownership.path()).expect("unchanged ownership"),
            ownership_before,
        );
    }

    #[test]
    fn missing_stale_output_is_dropped_while_current_output_commits() {
        let fixture = ReconciliationFixture::new();
        fixture
            .store
            .write_generated(
                fixture.generation(&[("current.yaml", b"old\n"), ("stale.yaml", b"stale\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect("initial generation");
        fs_err::remove_file(fixture.contracts.child("stale.yaml").path())
            .expect("remove stale output");

        fixture
            .store
            .write_generated(
                fixture.generation(&[("current.yaml", b"new\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect("missing stale output does not block reconciliation");

        fixture
            .contracts
            .child("current.yaml")
            .assert(b"new\n" as &[u8]);
        fixture
            .contracts
            .child("stale.yaml")
            .assert(predicates::path::missing());
        let (_, owned) = fixture.committed_catalog();
        assert_eq!(
            owned.entries().map(OwnedFile::path).collect::<Vec<_>>(),
            ["current.yaml"],
        );
    }

    #[cfg(unix)]
    #[test]
    fn stale_socket_fails_preflight_before_current_write() {
        use std::os::unix::fs::FileTypeExt;
        use std::os::unix::net::UnixListener;

        let fixture = ReconciliationFixture::new();
        fixture
            .store
            .write_generated(
                fixture.generation(&[("current.yaml", b"old\n"), ("stale.yaml", b"stale\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect("initial generation");
        let ownership = fixture
            .contracts
            .child(".contract-kit/generated-files.json");
        let ownership_before = fs_err::read(ownership.path()).expect("ownership bytes");
        fs_err::remove_file(fixture.contracts.child("stale.yaml").path())
            .expect("remove stale file");
        let socket = UnixListener::bind(fixture.contracts.child("stale.yaml").path())
            .expect("bind stale socket");

        let error = fixture
            .store
            .write_generated(
                fixture.generation(&[("current.yaml", b"new\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect_err("stale socket must fail preflight");
        assert!(error.to_string().contains("not a file"));
        fixture
            .contracts
            .child("current.yaml")
            .assert(b"old\n" as &[u8]);
        assert!(
            fs_err::symlink_metadata(fixture.contracts.child("stale.yaml").path())
                .expect("socket remains")
                .file_type()
                .is_socket()
        );
        assert_eq!(
            fs_err::read(ownership.path()).expect("unchanged ownership"),
            ownership_before,
        );
        drop(socket);
    }

    #[test]
    fn interrupted_update_recovers_partial_writes_reservation_and_stale_deletion() {
        let fixture = ReconciliationFixture::new();
        fixture
            .store
            .write_generated(
                fixture.generation(&[("current.yaml", b"old\n"), ("stale.yaml", b"stale\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect("initial generation");
        let (previous_generation, before) = fixture.committed_catalog();
        let generation = previous_generation + 1;
        let after = OwnedCatalog::from_files(vec![
            OwnedFile::new(
                "current.yaml".to_owned(),
                ContentDigest::of(b"partially written\n"),
            ),
            OwnedFile::new("new.yaml".to_owned(), ContentDigest::of(b"planned new\n")),
        ]);
        fixture.write_manifest(&OwnershipManifest::updating(
            generation,
            before,
            after.clone(),
        ));
        fixture
            .contracts
            .child("current.yaml")
            .write_binary(b"partially written\n")
            .expect("partial current write");
        fs_err::remove_file(fixture.contracts.child("stale.yaml").path())
            .expect("completed stale deletion");
        let marker = ReservationMarker::new(
            generation,
            after.find("new.yaml").expect("planned new ownership"),
        )
        .to_bytes()
        .expect("reservation marker");
        fixture
            .contracts
            .child("new.yaml")
            .write_binary(&marker)
            .expect("reserved destination");

        let error = fixture
            .store
            .write_generated(
                fixture.generation(&[
                    ("current.yaml", b"latest source\n"),
                    ("new.yaml", b"planned new\n"),
                ]),
                ExistingOutputPolicy::Reject,
            )
            .expect_err("recovery must invalidate the partial catalog snapshot");
        assert!(error.to_string().contains("contracts changed"));

        fixture
            .store
            .write_generated(
                fixture.generation(&[
                    ("current.yaml", b"latest source\n"),
                    ("new.yaml", b"planned new\n"),
                ]),
                ExistingOutputPolicy::Reject,
            )
            .expect("generation retries from the recovered catalog");

        fixture
            .contracts
            .child("current.yaml")
            .assert(b"latest source\n" as &[u8]);
        fixture
            .contracts
            .child("new.yaml")
            .assert(b"planned new\n" as &[u8]);
        fixture
            .contracts
            .child("stale.yaml")
            .assert(predicates::path::missing());
        let (committed_generation, committed) = fixture.committed_catalog();
        assert_eq!(committed_generation, generation + 1);
        assert_eq!(committed.entries().count(), 2);
    }

    #[test]
    fn interrupted_new_output_with_external_bytes_stays_unowned() {
        let fixture = ReconciliationFixture::new();
        fixture
            .store
            .write_generated(
                fixture.generation(&[("current.yaml", b"old\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect("initial generation");
        let (previous_generation, before) = fixture.committed_catalog();
        let generation = previous_generation + 1;
        let mut after_files = before.entries().cloned().collect::<Vec<_>>();
        after_files.push(OwnedFile::new(
            "new.yaml".to_owned(),
            ContentDigest::of(b"planned\n"),
        ));
        fixture.write_manifest(&OwnershipManifest::updating(
            generation,
            before,
            OwnedCatalog::from_files(after_files),
        ));
        fixture
            .contracts
            .child("new.yaml")
            .write_binary(b"external\n")
            .expect("external output");
        let ownership = fixture
            .contracts
            .child(".contract-kit/generated-files.json");
        let ownership_before = fs_err::read(ownership.path()).expect("updating manifest");

        let error = fixture
            .store
            .write_generated(
                fixture.generation(&[("current.yaml", b"old\n"), ("new.yaml", b"planned\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect_err("unexpected external bytes must stop recovery");

        assert!(error.to_string().contains("unexpected"));
        fixture
            .contracts
            .child("new.yaml")
            .assert(b"external\n" as &[u8]);
        assert_eq!(
            fs_err::read(ownership.path()).expect("unchanged updating manifest"),
            ownership_before,
        );
    }

    #[test]
    fn concurrent_writer_lock_fails_fast() {
        let fixture = ReconciliationFixture::new();
        let root = ResolvedPath::new(PathRole::Contracts, fixture.contracts.path().to_path_buf())
            .expect("resolved contracts root");
        let _lock = GenerationLock::acquire(&fixture.store, &root).expect("first writer lock");

        let error = fixture
            .store
            .write_generated(
                fixture.generation(&[("current.yaml", b"generated\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect_err("second writer must fail fast");

        assert!(error.to_string().contains("another generation"));
        fixture
            .contracts
            .child("current.yaml")
            .assert(predicates::path::missing());
        fixture
            .contracts
            .child(".contract-kit/generated-files.json")
            .assert(predicates::path::missing());
    }

    #[test]
    fn stale_generation_cannot_overwrite_newer_combined_document() {
        let fixture = ReconciliationFixture::new();
        fixture
            .store
            .write_generated(
                fixture.generation(&[("main.yml", b"initial\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect("initial generation");

        let stale = fixture.generation(&[("main.yml", b"stale signature update\n")]);
        let newer = fixture.generation(&[("main.yml", b"newer sketch update\n")]);
        fixture
            .store
            .write_generated(newer, ExistingOutputPolicy::Reject)
            .expect("newer generation");
        let ownership = fixture
            .contracts
            .child(".contract-kit/generated-files.json");
        let ownership_before = fs_err::read(ownership.path()).expect("newer ownership bytes");

        let error = fixture
            .store
            .write_generated(stale, ExistingOutputPolicy::Reject)
            .expect_err("stale generation must not overwrite newer bytes");

        assert!(error.to_string().contains("contracts changed"));
        fixture
            .contracts
            .child("main.yml")
            .assert(b"newer sketch update\n" as &[u8]);
        assert_eq!(
            fs_err::read(ownership.path()).expect("unchanged ownership bytes"),
            ownership_before,
        );
    }

    #[test]
    fn stale_generation_is_rejected_before_requested_output_preflight() {
        let fixture = ReconciliationFixture::new();
        fixture
            .store
            .write_generated(
                fixture.generation(&[("main.yml", b"initial\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect("initial generation");

        let stale = fixture.generation(&[(".contract-kit/blocked.yml", b"must not be written\n")]);
        let newer = fixture.generation(&[("main.yml", b"newer sketch update\n")]);
        fixture
            .store
            .write_generated(newer, ExistingOutputPolicy::Reject)
            .expect("newer generation");
        let ownership = fixture
            .contracts
            .child(".contract-kit/generated-files.json");
        let ownership_before = fs_err::read(ownership.path()).expect("newer ownership bytes");

        let error = fixture
            .store
            .write_generated(stale, ExistingOutputPolicy::Reject)
            .expect_err("stale input must win before requested-output preflight");

        assert!(matches!(error, CliError::GenerationInputChanged { .. }));
        fixture
            .contracts
            .child("main.yml")
            .assert(b"newer sketch update\n" as &[u8]);
        fixture
            .contracts
            .child(".contract-kit/blocked.yml")
            .assert(predicates::path::missing());
        assert_eq!(
            fs_err::read(ownership.path()).expect("unchanged ownership bytes"),
            ownership_before,
        );
        fixture.assert_no_atomic_manifest_temporaries();
    }

    #[test]
    fn empty_generation_baseline_rejects_a_newly_populated_catalog() {
        let fixture = ReconciliationFixture::new();
        let generated = fixture.generation(&[("main.yml", b"generated\n")]);
        fixture
            .contracts
            .child("manual.yml")
            .write_binary(b"external\n")
            .expect("external contract");

        let error = fixture
            .store
            .write_generated(generated, ExistingOutputPolicy::Reject)
            .expect_err("a populated catalog must invalidate an empty baseline");

        assert!(matches!(error, CliError::GenerationInputChanged { .. }));
        fixture
            .contracts
            .child("manual.yml")
            .assert(b"external\n" as &[u8]);
        fixture
            .contracts
            .child("main.yml")
            .assert(predicates::path::missing());
        fixture
            .contracts
            .child(".contract-kit/generated-files.json")
            .assert(predicates::path::missing());
        fixture.assert_no_atomic_manifest_temporaries();
    }

    #[test]
    fn generation_baseline_rejects_a_later_manual_contract_edit() {
        let fixture = ReconciliationFixture::new();
        fixture
            .store
            .write_generated(
                fixture.generation(&[("main.yml", b"initial\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect("initial generation");
        let generated = fixture.generation(&[("main.yml", b"updated\n")]);
        let ownership = fixture
            .contracts
            .child(".contract-kit/generated-files.json");
        let ownership_before = fs_err::read(ownership.path()).expect("initial ownership bytes");
        fixture
            .contracts
            .child("main.yml")
            .write_binary(b"manual edit\n")
            .expect("manual contract edit");

        let error = fixture
            .store
            .write_generated(generated, ExistingOutputPolicy::Reject)
            .expect_err("a manual edit must invalidate the captured baseline");

        assert!(matches!(error, CliError::GenerationInputChanged { .. }));
        fixture
            .contracts
            .child("main.yml")
            .assert(b"manual edit\n" as &[u8]);
        assert_eq!(
            fs_err::read(ownership.path()).expect("unchanged ownership bytes"),
            ownership_before,
        );
        fixture.assert_no_atomic_manifest_temporaries();
    }

    #[test]
    fn unchanged_generation_baseline_commits_normally() {
        let fixture = ReconciliationFixture::new();
        let generated = fixture.generation(&[("main.yml", b"generated\n")]);

        fixture
            .store
            .write_generated(generated, ExistingOutputPolicy::Reject)
            .expect("unchanged baseline generation");

        fixture
            .contracts
            .child("main.yml")
            .assert(b"generated\n" as &[u8]);
        let (generation, committed) = fixture.committed_catalog();
        assert_eq!(generation, 1);
        assert_eq!(committed.entries().count(), 1);
        fixture.assert_no_atomic_manifest_temporaries();
    }

    #[test]
    fn abandoned_manifest_atomic_temporary_is_removed_under_the_lock() {
        let fixture = ReconciliationFixture::new();
        fixture
            .store
            .write_generated(
                fixture.generation(&[("current.yaml", b"generated\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect("initial generation");
        let abandoned = fixture
            .contracts
            .child(".contract-kit/.generated-files.json.A1b2C3");
        abandoned
            .write_binary(b"interrupted manifest bytes")
            .expect("abandoned atomic temporary");

        fixture
            .store
            .write_generated(
                fixture.generation(&[("current.yaml", b"generated\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect("generation should clean the reserved temporary");

        abandoned.assert(predicates::path::missing());
        fixture
            .contracts
            .child("current.yaml")
            .assert(b"generated\n" as &[u8]);
    }

    #[test]
    fn reservation_collision_rolls_back_without_overwriting_external_file() {
        let fixture = ReconciliationFixture::new();
        let reconciliation = CatalogReconciliation::new(
            &fixture.store,
            fixture.generation(&[("new.yaml", b"generated\n")]),
            ExistingOutputPolicy::Reject,
        )
        .expect("preflight missing destination");
        fixture
            .contracts
            .child("new.yaml")
            .write_binary(b"external\n")
            .expect("race external output");

        let error = reconciliation
            .apply()
            .expect_err("reservation collision must fail");

        assert!(error.to_string().contains("unowned"));
        fixture
            .contracts
            .child("new.yaml")
            .assert(b"external\n" as &[u8]);
        let (generation, committed) = fixture.committed_catalog();
        assert_eq!(generation, 1);
        assert!(committed.entries().next().is_none());
    }

    #[test]
    fn adopted_output_is_reverified_before_commit() {
        let fixture = ReconciliationFixture::new();
        fixture
            .contracts
            .child("adopt.yaml")
            .write_binary(b"matching\n")
            .expect("matching legacy output");
        let reconciliation = CatalogReconciliation::new(
            &fixture.store,
            fixture.generation(&[("adopt.yaml", b"matching\n")]),
            ExistingOutputPolicy::AdoptMatching,
        )
        .expect("matching adoption preflight");
        fixture
            .contracts
            .child("adopt.yaml")
            .write_binary(b"external change\n")
            .expect("race external change");

        let error = reconciliation
            .apply()
            .expect_err("changed adopted output must not be committed");

        assert!(error.to_string().contains("unexpected"));
        fixture
            .contracts
            .child("adopt.yaml")
            .assert(b"external change\n" as &[u8]);
        let manifest = fs_err::read_to_string(
            fixture
                .contracts
                .child(".contract-kit/generated-files.json")
                .path(),
        )
        .expect("recoverable updating manifest");
        assert!(manifest.contains(r#""state": "updating""#));
    }

    #[cfg(unix)]
    #[test]
    fn hard_linked_owned_outputs_are_rejected_as_host_aliases() {
        let fixture = ReconciliationFixture::new();
        fixture
            .store
            .write_generated(
                fixture.generation(&[("a.yaml", b"same\n"), ("b.yaml", b"same\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect("initial generation");
        let ownership = fixture
            .contracts
            .child(".contract-kit/generated-files.json");
        let ownership_before = fs_err::read(ownership.path()).expect("ownership bytes");
        fs_err::remove_file(fixture.contracts.child("b.yaml").path())
            .expect("remove second output");
        fs_err::hard_link(
            fixture.contracts.child("a.yaml").path(),
            fixture.contracts.child("b.yaml").path(),
        )
        .expect("hard link outputs");

        let error = fixture
            .store
            .write_generated(
                fixture.generation(&[("a.yaml", b"same\n"), ("b.yaml", b"same\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect_err("host aliases must fail preflight");

        assert!(error.to_string().contains("same host file"));
        assert_eq!(
            fs_err::read(ownership.path()).expect("unchanged ownership"),
            ownership_before,
        );
    }

    #[cfg(unix)]
    #[test]
    fn requested_and_stale_outputs_are_rejected_when_they_alias() {
        let fixture = ReconciliationFixture::new();
        fixture
            .store
            .write_generated(
                fixture.generation(&[("current.yaml", b"same\n"), ("stale.yaml", b"same\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect("initial generation");
        let ownership = fixture
            .contracts
            .child(".contract-kit/generated-files.json");
        let ownership_before = fs_err::read(ownership.path()).expect("ownership bytes");
        fs_err::remove_file(fixture.contracts.child("stale.yaml").path())
            .expect("remove stale output");
        fs_err::hard_link(
            fixture.contracts.child("current.yaml").path(),
            fixture.contracts.child("stale.yaml").path(),
        )
        .expect("alias stale output to requested output");

        let error = fixture
            .store
            .write_generated(
                fixture.generation(&[("current.yaml", b"same\n")]),
                ExistingOutputPolicy::Reject,
            )
            .expect_err("requested-to-stale alias must fail preflight");

        assert!(error.to_string().contains("same host file"));
        assert_eq!(
            fs_err::read(ownership.path()).expect("unchanged ownership"),
            ownership_before,
        );
        fixture
            .contracts
            .child("current.yaml")
            .assert(b"same\n" as &[u8]);
        fixture
            .contracts
            .child("stale.yaml")
            .assert(b"same\n" as &[u8]);
    }
}
