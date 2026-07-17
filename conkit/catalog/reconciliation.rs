//! Runtime coordination for generated-document ownership.
//!
//! Domain work finishes before this module is entered. Reconciliation then
//! holds the generation lock while it recovers any interrupted journal,
//! validates the exact generation baseline, preflights every mutation, and
//! coordinates individually atomic file replacements with version-3
//! ownership state.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{File, TryLockError};
use std::io::ErrorKind;
use std::path::PathBuf;

use conkit_signature::{CatalogPath, FileCatalog};

use super::CatalogReadBudget;
use super::ownership::{
    ContentDigest, OwnedCatalog, OwnedFile, OwnershipJournal, OwnershipManifest, ReservationMarker,
    VersionProbe,
};
use super::path::{CatalogLeaf, PortableCatalogPathKey};
use super::store::{
    ContractsStore, ExistingOutputPolicy, FileSnapshot, GeneratedContracts, GenerationReceipt,
};
use crate::context::ApplicationCancellation;
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
    owned: OwnedFile,
    file: CatalogLeaf,
    bytes: Vec<u8>,
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
        file: CatalogLeaf,
    },
}

/// Digest-bound reservation cleanup performed during rollback or recovery.
struct Reservation {
    file: CatalogLeaf,
    marker: Vec<u8>,
}

/// Existing generated names indexed by identity derived from their already-
/// opened file handles.
struct HostIdentities {
    values: HashMap<same_file::Handle, PathBuf>,
}

/// One locked, fully preflighted ownership reconciliation.
pub(super) struct CatalogReconciliation<'store> {
    store: &'store ContractsStore,
    ownership: CatalogLeaf,
    _lock: GenerationLock,
    budget: CatalogReadBudget,
    generation: u64,
    before: OwnedCatalog,
    after: OwnedCatalog,
    mutations: Vec<OutputMutation>,
    adopted_count: usize,
}

impl ContractsStore {
    /// Recovers interrupted ownership through the caller's operation ledger.
    pub(crate) fn recover_interrupted_generation_with_budget(
        &self,
        budget: &mut CatalogReadBudget,
    ) -> Result<(), CliError> {
        budget.checkpoint()?;
        self.validate_reserved_namespace_with_budget(budget)?;
        let needs_recovery = OwnershipDocument::load(self, budget)?.is_updating();
        if !needs_recovery {
            return Ok(());
        }

        budget.checkpoint()?;
        let _lock = GenerationLock::acquire(self)?;
        self.remove_abandoned_manifest_temporaries_with_budget(budget)?;
        self.validate_reserved_namespace_with_budget(budget)?;

        let ownership = OwnershipDocument::load(self, budget)?;
        if ownership.is_updating() {
            let _ = ownership.recover(self, budget)?;
        }

        Ok(())
    }
}

impl OwnershipDocument {
    fn load(store: &ContractsStore, budget: &mut CatalogReadBudget) -> Result<Self, CliError> {
        let path = OwnershipManifest::path(store.path());
        let Some(file) = store.existing_ownership_leaf()? else {
            return Ok(Self::Missing);
        };
        let Some(snapshot) = store.read_leaf(&file, budget)? else {
            return Ok(Self::Missing);
        };
        let probe = VersionProbe::from_bytes(&path, snapshot.bytes(), budget.cancellation())?;

        if probe.version() == OwnershipManifest::VERSION {
            Ok(Self::Current(OwnershipManifest::from_bytes(
                &path,
                snapshot.bytes(),
                budget.cancellation(),
            )?))
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
        budget: &mut CatalogReadBudget,
    ) -> Result<(u64, OwnedCatalog), CliError> {
        match self {
            Self::Missing => Ok((0, OwnedCatalog::default())),
            Self::Current(manifest) => match manifest.into_journal() {
                OwnershipJournal::Committed { generation, files } => Ok((generation, files)),
                OwnershipJournal::Updating {
                    generation,
                    before,
                    after,
                } => Self::recover_updating(store, budget, generation, before, after),
            },
        }
    }

    fn recover_updating(
        store: &ContractsStore,
        budget: &mut CatalogReadBudget,
        generation: u64,
        before: OwnedCatalog,
        after: OwnedCatalog,
    ) -> Result<(u64, OwnedCatalog), CliError> {
        let mut paths = BTreeSet::new();
        for file in before.entries().chain(after.entries()) {
            budget.checkpoint()?;
            paths.insert(file.path().clone());
        }
        let mut recovered_files = Vec::new();
        let mut reservations = Vec::new();

        for logical in paths {
            budget.checkpoint()?;
            let previous = before.find(&logical);
            let next = after.find(&logical);
            let file = store.generated_leaf(&logical)?;

            match store.read_leaf(&file, budget)? {
                Some(snapshot) => {
                    let digest = ContentDigest::of(snapshot.bytes(), budget.cancellation())?;
                    if let Some(file) = next
                        && digest == *file.digest()
                    {
                        recovered_files.push(file.clone());
                    } else if let Some(file) = previous
                        && digest == *file.digest()
                    {
                        recovered_files.push(file.clone());
                    } else if let Some(next_file) = next {
                        let marker = ReservationMarker::new(generation, next_file)
                            .to_bytes(budget.cancellation())?;
                        if snapshot.bytes() == marker {
                            reservations.push(Reservation::new(file, marker));
                            if let Some(previous_file) = previous {
                                recovered_files.push(previous_file.clone());
                            }
                        } else {
                            return Err(CliError::GeneratedOwnershipRecoveryConflict {
                                path: file.display_path().to_path_buf(),
                            });
                        }
                    } else {
                        return Err(CliError::GeneratedOwnershipRecoveryConflict {
                            path: file.display_path().to_path_buf(),
                        });
                    }
                }
                None => {
                    if let (Some(_), Some(file)) = (previous, next) {
                        recovered_files.push(file.clone());
                    }
                }
            }
        }

        let recovered = OwnedCatalog::from_files(recovered_files);
        let ownership_path = OwnershipManifest::path(store.path());
        recovered.validate(&ownership_path, budget.cancellation())?;
        for reservation in reservations {
            budget.checkpoint()?;
            reservation.remove_if_unchanged(store, budget)?;
        }

        let committed = OwnershipManifest::committed(generation, recovered.clone());
        let ownership = store.ownership_leaf_for_write()?;
        store.atomic_write(
            &ownership,
            &committed.to_bytes(&ownership_path, budget.cancellation())?,
            budget.cancellation(),
        )?;

        Ok((generation, recovered))
    }
}

impl GenerationLock {
    /// File name recognized by contracts-store namespace validation.
    pub(super) const FILE_NAME: &'static str = "generation.lock";

    fn acquire(store: &ContractsStore) -> Result<Self, CliError> {
        let lock = store.generation_lock_leaf()?;
        let path = lock.display_path().to_path_buf();
        let file = lock.open_lock_file()?;
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
        catalog: FileCatalog,
        budget: &CatalogReadBudget,
    ) -> Result<Vec<Self>, CliError> {
        let manifest = OwnershipManifest::path(store.path());
        let mut keys = BTreeMap::<PortableCatalogPathKey, String>::new();
        let mut outputs = Vec::with_capacity(catalog.len());

        for (logical, bytes) in catalog.into_entries() {
            budget.checkpoint()?;
            ContractDocumentPath::try_from(logical.clone()).map_err(|_| {
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
            let file = store.generated_leaf(&logical)?;
            let digest = ContentDigest::of(&bytes, budget.cancellation())?;
            outputs.push(Self {
                owned: OwnedFile::new(logical, digest),
                file,
                bytes,
            });
        }

        Ok(outputs)
    }

    fn logical(&self) -> &CatalogPath {
        self.owned.path()
    }

    fn digest(&self) -> &ContentDigest {
        self.owned.digest()
    }

    fn marker(
        &self,
        generation: u64,
        cancellation: &ApplicationCancellation,
    ) -> Result<Vec<u8>, CliError> {
        ReservationMarker::new(generation, &self.owned).to_bytes(cancellation)
    }

    fn validate_host_spellings(
        outputs: &[Self],
        budget: &mut CatalogReadBudget,
    ) -> Result<(), CliError> {
        let Some(first) = outputs.first() else {
            return Ok(());
        };
        let mut requested = BTreeMap::<String, String>::new();
        for output in outputs {
            budget.checkpoint()?;
            let logical = output.logical().as_str().to_owned();
            requested.insert(logical.to_ascii_lowercase(), logical);
        }
        let mut conflicts = BTreeMap::<String, std::ffi::OsString>::new();
        for name in first.file.sorted_sibling_names(budget)? {
            budget.checkpoint()?;
            let Some(name_text) = name.to_str() else {
                continue;
            };
            let key = name_text.to_ascii_lowercase();
            if requested
                .get(&key)
                .is_some_and(|logical| logical != name_text)
            {
                conflicts.entry(key).or_insert(name);
            }
        }
        for output in outputs {
            budget.checkpoint()?;
            let key = output.logical().as_str().to_ascii_lowercase();
            if let Some(name) = conflicts.get(&key) {
                return Err(CliError::PortableGeneratedPathCollision {
                    first: output
                        .file
                        .display_path()
                        .with_file_name(name)
                        .display()
                        .to_string(),
                    second: output.logical().as_str().to_owned(),
                });
            }
        }
        Ok(())
    }
}

impl OutputMutation {
    fn reserve(&mut self, generation: u64, budget: &CatalogReadBudget) -> Result<(), CliError> {
        let Self::Write { output, expected } = self else {
            return Ok(());
        };
        if !matches!(expected, DestinationExpectation::Missing) {
            return Ok(());
        }

        let marker = output.marker(generation, budget.cancellation())?;
        match output.file.write_new_synced(&marker) {
            Ok(()) => {}
            Err(CliError::Io(source)) if source.kind() == ErrorKind::AlreadyExists => {
                return Err(CliError::UnownedGeneratedOutput {
                    path: output.file.display_path().to_path_buf(),
                });
            }
            Err(source) => return Err(source),
        }
        *expected = DestinationExpectation::Reservation;
        Ok(())
    }

    fn apply(
        &self,
        store: &ContractsStore,
        budget: &mut CatalogReadBudget,
        generation: u64,
    ) -> Result<(), CliError> {
        budget.checkpoint()?;
        match self {
            Self::Write { output, expected } => {
                output.verify(store, budget, expected, generation)?;
                store.atomic_write(&output.file, &output.bytes, budget.cancellation())
            }
            Self::Remove { owned, file } => Self::remove_owned(store, budget, owned, file),
        }
    }

    fn remove_owned(
        store: &ContractsStore,
        budget: &mut CatalogReadBudget,
        owned: &OwnedFile,
        file: &CatalogLeaf,
    ) -> Result<(), CliError> {
        let Some(snapshot) = store.read_leaf(file, budget)? else {
            return Ok(());
        };
        if ContentDigest::of(snapshot.bytes(), budget.cancellation())? != *owned.digest() {
            return Err(CliError::ModifiedGeneratedOutput {
                path: file.display_path().to_path_buf(),
            });
        }
        drop(snapshot);
        // Ownership is restricted to direct-root combined documents, so no
        // generated parent-directory cleanup is necessary. This final
        // digest-check-plus-remove sequence is capability-relative, though the
        // filesystem does not provide an atomic unlink-if-same primitive.
        budget.checkpoint()?;
        file.remove_file()
    }

    fn reservation(
        &self,
        generation: u64,
        budget: &CatalogReadBudget,
    ) -> Result<Option<Reservation>, CliError> {
        match self {
            Self::Write {
                output,
                expected: DestinationExpectation::Reservation,
            } => Ok(Some(Reservation::new(
                output.file.try_clone()?,
                output.marker(generation, budget.cancellation())?,
            ))),
            Self::Write { .. } | Self::Remove { .. } => Ok(None),
        }
    }
}

impl GeneratedOutput {
    fn verify(
        &self,
        store: &ContractsStore,
        budget: &mut CatalogReadBudget,
        expected: &DestinationExpectation,
        generation: u64,
    ) -> Result<(), CliError> {
        let Some(snapshot) = store.read_leaf(&self.file, budget)? else {
            return Err(CliError::GeneratedOwnershipRecoveryConflict {
                path: self.file.display_path().to_path_buf(),
            });
        };
        let matches = match expected {
            DestinationExpectation::Missing => false,
            DestinationExpectation::Digest(digest) => {
                ContentDigest::of(snapshot.bytes(), budget.cancellation())? == *digest
            }
            DestinationExpectation::Reservation => {
                snapshot.bytes() == self.marker(generation, budget.cancellation())?
            }
        };
        if matches {
            Ok(())
        } else {
            Err(CliError::GeneratedOwnershipRecoveryConflict {
                path: self.file.display_path().to_path_buf(),
            })
        }
    }
}

impl Reservation {
    fn new(file: CatalogLeaf, marker: Vec<u8>) -> Self {
        Self { file, marker }
    }

    fn remove_if_unchanged(
        self,
        store: &ContractsStore,
        budget: &mut CatalogReadBudget,
    ) -> Result<(), CliError> {
        match store.read_leaf(&self.file, budget)? {
            Some(snapshot) if snapshot.bytes() == self.marker => {
                drop(snapshot);
                budget.checkpoint()?;
                self.file.remove_file()?;
                Ok(())
            }
            None => Ok(()),
            Some(_) => Err(CliError::GeneratedOwnershipRecoveryConflict {
                path: self.file.display_path().to_path_buf(),
            }),
        }
    }
}

impl HostIdentities {
    fn new() -> Self {
        Self {
            values: HashMap::new(),
        }
    }

    fn insert(&mut self, snapshot: FileSnapshot, path: PathBuf) -> Result<(), CliError> {
        if let Some(first) = self.values.insert(snapshot.into_identity(), path.clone()) {
            Err(CliError::GeneratedOutputAlias {
                first,
                second: path,
            })
        } else {
            Ok(())
        }
    }
}

impl<'store> CatalogReconciliation<'store> {
    /// Locks, recovers, validates, and preflights one generated catalog update.
    ///
    /// # Errors
    ///
    /// Returns an error if locking or recovery fails, the generation baseline
    /// changed, ownership or output paths are invalid, a destination conflicts
    /// or aliases another output, or filesystem preflight fails.
    pub(super) fn new(
        store: &'store ContractsStore,
        generated: GeneratedContracts,
        policy: ExistingOutputPolicy,
        mut budget: CatalogReadBudget,
    ) -> Result<Self, CliError> {
        budget.checkpoint()?;
        let (baseline, catalog) = generated.into_parts();
        store.validate_reserved_namespace_with_budget(&mut budget)?;
        let generation_lock = GenerationLock::acquire(store)?;
        store.remove_abandoned_manifest_temporaries_with_budget(&mut budget)?;
        store.validate_reserved_namespace_with_budget(&mut budget)?;

        let ownership_path = OwnershipManifest::path(store.path());
        let (previous_generation, before) =
            OwnershipDocument::load(store, &mut budget)?.recover(store, &mut budget)?;
        let current = store.read_with_budget(&mut budget)?;
        if current != baseline {
            return Err(CliError::GenerationInputChanged {
                path: store.path().to_path_buf(),
            });
        }

        let outputs = GeneratedOutput::collect(store, catalog, &budget)?;
        let mut owned_outputs = Vec::with_capacity(outputs.len());
        for output in &outputs {
            budget.checkpoint()?;
            owned_outputs.push(output.owned.clone());
        }
        let after = OwnedCatalog::from_files(owned_outputs);
        after.validate(&ownership_path, budget.cancellation())?;
        before.validate_transition(&after, budget.cancellation())?;
        GeneratedOutput::validate_host_spellings(&outputs, &mut budget)?;

        let generation = previous_generation.checked_add(1).ok_or_else(|| {
            CliError::InvalidGeneratedOwnership {
                path: ownership_path.clone(),
                message: "ownership generation is exhausted".to_owned(),
            }
        })?;
        let (mutations, adopted_count) =
            Self::preflight(store, &mut budget, &before, &after, outputs, policy)?;
        let ownership = store.ownership_leaf_for_write()?;

        Ok(Self {
            store,
            ownership,
            _lock: generation_lock,
            budget,
            generation,
            before,
            after,
            mutations,
            adopted_count,
        })
    }

    fn preflight(
        store: &ContractsStore,
        budget: &mut CatalogReadBudget,
        before: &OwnedCatalog,
        after: &OwnedCatalog,
        outputs: Vec<GeneratedOutput>,
        policy: ExistingOutputPolicy,
    ) -> Result<(Vec<OutputMutation>, usize), CliError> {
        let mut mutations = Vec::new();
        let mut adopted_count = 0;
        let mut identities = HostIdentities::new();

        for output in outputs {
            budget.checkpoint()?;
            let previous = before.find(output.logical());
            match store.read_leaf(&output.file, budget)? {
                Some(snapshot) => {
                    let path = output.file.display_path().to_path_buf();
                    let disk_digest = ContentDigest::of(snapshot.bytes(), budget.cancellation())?;
                    identities.insert(snapshot, path.clone())?;
                    if let Some(previous) = previous {
                        if disk_digest != *previous.digest() {
                            return Err(CliError::ModifiedGeneratedOutput { path });
                        }
                        if disk_digest != *output.digest() {
                            mutations.push(OutputMutation::Write {
                                output,
                                expected: DestinationExpectation::Digest(disk_digest),
                            });
                        }
                    } else if policy == ExistingOutputPolicy::AdoptMatching
                        && disk_digest == *output.digest()
                    {
                        adopted_count += 1;
                    } else {
                        return Err(CliError::UnownedGeneratedOutput { path });
                    }
                }
                None => {
                    mutations.push(OutputMutation::Write {
                        output,
                        expected: DestinationExpectation::Missing,
                    });
                }
            }
        }

        for owned in before.entries() {
            budget.checkpoint()?;
            if after.find(owned.path()).is_some() {
                continue;
            }
            let file = store.generated_leaf(owned.path())?;
            if let Some(snapshot) = store.read_leaf(&file, budget)? {
                let path = file.display_path().to_path_buf();
                if ContentDigest::of(snapshot.bytes(), budget.cancellation())? != *owned.digest() {
                    return Err(CliError::ModifiedGeneratedOutput { path });
                }
                identities.insert(snapshot, path)?;
                mutations.push(OutputMutation::Remove {
                    owned: owned.clone(),
                    file,
                });
            }
        }

        Ok((mutations, adopted_count))
    }

    /// Applies the preflighted update and commits its ownership manifest.
    ///
    /// # Errors
    ///
    /// Returns an error if ownership serialization or persistence, output
    /// reservation or mutation, post-write verification, or reservation
    /// rollback fails.
    pub(super) fn apply(mut self) -> Result<GenerationReceipt, CliError> {
        self.budget.checkpoint()?;
        let ownership_path = OwnershipManifest::path(self.store.path());
        let updating =
            OwnershipManifest::updating(self.generation, self.before.clone(), self.after.clone());
        self.store.atomic_write(
            &self.ownership,
            &updating.to_bytes(&ownership_path, self.budget.cancellation())?,
            self.budget.cancellation(),
        )?;

        if let Err(source) = self.reserve_outputs() {
            return match self.rollback_reservations() {
                Ok(()) => Err(source),
                Err(rollback) => Err(CliError::GeneratedOwnershipRollback {
                    path: ownership_path,
                    message: rollback.to_string(),
                }),
            };
        }

        for mutation in &self.mutations {
            mutation.apply(self.store, &mut self.budget, self.generation)?;
        }
        self.verify_after_outputs()?;

        let committed = OwnershipManifest::committed(self.generation, self.after);
        self.store.atomic_write(
            &self.ownership,
            &committed.to_bytes(&ownership_path, self.budget.cancellation())?,
            self.budget.cancellation(),
        )?;

        Ok(GenerationReceipt::new(self.adopted_count))
    }

    fn reserve_outputs(&mut self) -> Result<(), CliError> {
        for mutation in &mut self.mutations {
            self.budget.checkpoint()?;
            mutation.reserve(self.generation, &self.budget)?;
        }
        Ok(())
    }

    fn rollback_reservations(&mut self) -> Result<(), CliError> {
        // Cleanup has its own bounded ledger and neutral cancellation source:
        // an exhausted or canceled forward operation must not strand markers.
        let cleanup_cancellation = ApplicationCancellation::new();
        let mut cleanup_budget = self.store.begin_reconciliation_read(&cleanup_cancellation);
        for mutation in &self.mutations {
            if let Some(reservation) = mutation.reservation(self.generation, &cleanup_budget)? {
                reservation.remove_if_unchanged(self.store, &mut cleanup_budget)?;
            }
        }

        let ownership_path = OwnershipManifest::path(self.store.path());
        let restored = OwnershipManifest::committed(self.generation, self.before.clone());
        self.store.atomic_write(
            &self.ownership,
            &restored.to_bytes(&ownership_path, &cleanup_cancellation)?,
            &cleanup_cancellation,
        )
    }

    fn verify_after_outputs(&mut self) -> Result<(), CliError> {
        for owned in self.after.entries() {
            self.budget.checkpoint()?;
            let file = self.store.generated_leaf(owned.path())?;
            let Some(snapshot) = self.store.read_leaf(&file, &mut self.budget)? else {
                return Err(CliError::GeneratedOwnershipRecoveryConflict {
                    path: file.display_path().to_path_buf(),
                });
            };
            if ContentDigest::of(snapshot.bytes(), self.budget.cancellation())? != *owned.digest() {
                return Err(CliError::GeneratedOwnershipRecoveryConflict {
                    path: file.display_path().to_path_buf(),
                });
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
        CatalogReadBudget, CatalogReadLimitResource, CatalogReadLimits, ContractsStore,
        ExistingOutputPolicy, GeneratedContracts, GenerationReceipt,
    };
    use crate::error::CliError;

    struct ReconciliationFixture {
        store: ContractsStore,
        contracts: ChildPath,
        _temp: TempDir,
    }

    impl ReconciliationFixture {
        fn cancellation() -> crate::context::ApplicationCancellation {
            crate::context::ApplicationCancellation::new()
        }

        fn digest(bytes: &[u8]) -> ContentDigest {
            ContentDigest::of(bytes, &Self::cancellation()).expect("content digest")
        }

        fn new() -> Self {
            let temp = TempDir::new().expect("temporary root");
            let contracts = temp.child("contracts");
            let store = ContractsStore::new(contracts.path().to_path_buf());
            Self {
                store,
                contracts,
                _temp: temp,
            }
        }

        fn budget(&self) -> CatalogReadBudget {
            self.store.begin_reconciliation_read(&Self::cancellation())
        }

        fn generation(&self, entries: &[(&str, &[u8])]) -> (GeneratedContracts, CatalogReadBudget) {
            let mut files = FileCatalog::new();
            for (path, bytes) in entries {
                files
                    .insert(
                        CatalogPath::new((*path).to_owned()).expect("catalog path"),
                        bytes.to_vec(),
                    )
                    .expect("unique generated path");
            }
            let mut budget = self.budget();
            let baseline = self
                .store
                .read_optional_with_budget(&mut budget)
                .expect("generation baseline");
            (GeneratedContracts::new(baseline, files), budget)
        }

        fn apply_generation(
            &self,
            entries: &[(&str, &[u8])],
            policy: ExistingOutputPolicy,
        ) -> Result<GenerationReceipt, CliError> {
            let (generated, budget) = self.generation(entries);
            self.store
                .write_generated_with_budget(generated, policy, budget)
        }

        fn recover(&self) -> Result<(), CliError> {
            let mut budget = self.budget();
            self.store
                .recover_interrupted_generation_with_budget(&mut budget)
        }

        fn committed_catalog(&self) -> (u64, OwnedCatalog) {
            let path = OwnershipManifest::path(self.store.path());
            let manifest = OwnershipManifest::from_bytes(
                &path,
                &fs_err::read(&path).expect("ownership bytes"),
                &Self::cancellation(),
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
                manifest
                    .to_bytes(&path, &Self::cancellation())
                    .expect("manifest serialization"),
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
    fn canceled_reconciliation_does_not_create_outputs_or_ownership() {
        let fixture = ReconciliationFixture::new();
        let cancellation = crate::context::ApplicationCancellation::new();
        cancellation.request();
        let budget = CatalogReadLimits::default().begin(&cancellation);

        let (generated, _) = fixture.generation(&[("main.yml", b"generated\n")]);
        let error = fixture
            .store
            .write_generated_with_budget(generated, ExistingOutputPolicy::Reject, budget)
            .expect_err("pre-canceled reconciliation must stop before mutation");

        assert!(matches!(error, CliError::OperationCanceled));
        fixture
            .contracts
            .child("main.yml")
            .assert(predicates::path::missing());
        assert!(!OwnershipManifest::path(fixture.store.path()).exists());
    }

    #[test]
    fn rollback_uses_neutral_cancellation_after_forward_operation_is_canceled() {
        let fixture = ReconciliationFixture::new();
        let (generated, budget) = fixture.generation(&[("main.yml", b"generated\n")]);
        let mut reconciliation = CatalogReconciliation::new(
            &fixture.store,
            generated,
            ExistingOutputPolicy::Reject,
            budget,
        )
        .expect("preflight reconciliation");
        reconciliation
            .reserve_outputs()
            .expect("reserve missing output");
        fixture
            .contracts
            .child("main.yml")
            .assert(predicates::path::exists());

        reconciliation.budget.cancellation().request();
        reconciliation
            .rollback_reservations()
            .expect("neutral rollback ignores forward cancellation");

        fixture
            .contracts
            .child("main.yml")
            .assert(predicates::path::missing());
        let (generation, files) = fixture.committed_catalog();
        assert_eq!(generation, 1);
        assert_eq!(files.entries().count(), 0);
    }

    #[test]
    fn generated_catalog_accepts_direct_mixed_case_yaml_documents() {
        let fixture = ReconciliationFixture::new();

        fixture
            .apply_generation(
                &[("first.YML", b"first\n"), ("second.YaMl", b"second\n")],
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
            owned
                .entries()
                .map(OwnedFile::path)
                .map(CatalogPath::as_str)
                .collect::<Vec<_>>(),
            ["first.YML", "second.YaMl"],
        );
    }

    #[test]
    fn generated_catalog_rejects_non_yaml_path_without_mutation() {
        let fixture = ReconciliationFixture::new();

        let error = fixture
            .apply_generation(
                &[("manual.txt", b"generated\n")],
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
            .apply_generation(
                &[("Lib.yaml", b"first\n"), ("lib.yaml", b"second\n")],
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
            .apply_generation(&[("CON.yml", b"generated\n")], ExistingOutputPolicy::Reject)
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
            .recover()
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
            .recover()
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
                .apply_generation(
                    &[("main.yml", b"generated\n")],
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
            .apply_generation(
                &[("manual.yaml", b"generated\n")],
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
            .apply_generation(&[("Lib.yaml", b"old\n")], ExistingOutputPolicy::Reject)
            .expect("initial generation");
        let ownership = fixture
            .contracts
            .child(".contract-kit/generated-files.json");
        let ownership_before = fs_err::read(ownership.path()).expect("ownership bytes");

        let error = fixture
            .apply_generation(&[("lib.yaml", b"new\n")], ExistingOutputPolicy::Reject)
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
            .apply_generation(
                &[("current.yaml", b"old\n"), ("stale.yaml", b"stale\n")],
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
            .apply_generation(&[("current.yaml", b"new\n")], ExistingOutputPolicy::Reject)
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
            .apply_generation(&[("current.yaml", b"new\n")], ExistingOutputPolicy::Reject)
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
            .apply_generation(
                &[("current.yaml", b"old\n"), ("stale.yaml", b"stale\n")],
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
            .apply_generation(&[("current.yaml", b"new\n")], ExistingOutputPolicy::Reject)
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
            .apply_generation(
                &[("current.yaml", b"old\n"), ("stale.yaml", b"stale\n")],
                ExistingOutputPolicy::Reject,
            )
            .expect("initial generation");
        fs_err::remove_file(fixture.contracts.child("stale.yaml").path())
            .expect("remove stale output");

        fixture
            .apply_generation(&[("current.yaml", b"new\n")], ExistingOutputPolicy::Reject)
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
            owned
                .entries()
                .map(OwnedFile::path)
                .map(CatalogPath::as_str)
                .collect::<Vec<_>>(),
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
            .apply_generation(
                &[("current.yaml", b"old\n"), ("stale.yaml", b"stale\n")],
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
            .apply_generation(&[("current.yaml", b"new\n")], ExistingOutputPolicy::Reject)
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
            .apply_generation(
                &[("current.yaml", b"old\n"), ("stale.yaml", b"stale\n")],
                ExistingOutputPolicy::Reject,
            )
            .expect("initial generation");
        let (previous_generation, before) = fixture.committed_catalog();
        let generation = previous_generation + 1;
        let after = OwnedCatalog::from_files(vec![
            OwnedFile::new(
                CatalogPath::new("current.yaml").expect("current owned path"),
                ReconciliationFixture::digest(b"partially written\n"),
            ),
            OwnedFile::new(
                CatalogPath::new("new.yaml").expect("new owned path"),
                ReconciliationFixture::digest(b"planned new\n"),
            ),
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
        let planned = after
            .find(&CatalogPath::new("new.yaml").expect("new owned path"))
            .expect("planned new ownership");
        let marker = ReservationMarker::new(generation, planned)
            .to_bytes(&ReconciliationFixture::cancellation())
            .expect("reservation marker");
        fixture
            .contracts
            .child("new.yaml")
            .write_binary(&marker)
            .expect("reserved destination");

        let error = fixture
            .apply_generation(
                &[
                    ("current.yaml", b"latest source\n"),
                    ("new.yaml", b"planned new\n"),
                ],
                ExistingOutputPolicy::Reject,
            )
            .expect_err("recovery must invalidate the partial catalog snapshot");
        assert!(error.to_string().contains("contracts changed"));

        fixture
            .apply_generation(
                &[
                    ("current.yaml", b"latest source\n"),
                    ("new.yaml", b"planned new\n"),
                ],
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
            .apply_generation(&[("current.yaml", b"old\n")], ExistingOutputPolicy::Reject)
            .expect("initial generation");
        let (previous_generation, before) = fixture.committed_catalog();
        let generation = previous_generation + 1;
        let mut after_files = before.entries().cloned().collect::<Vec<_>>();
        after_files.push(OwnedFile::new(
            CatalogPath::new("new.yaml").expect("new owned path"),
            ReconciliationFixture::digest(b"planned\n"),
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
            .apply_generation(
                &[("current.yaml", b"old\n"), ("new.yaml", b"planned\n")],
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
        let _lock = GenerationLock::acquire(&fixture.store).expect("first writer lock");

        let error = fixture
            .apply_generation(
                &[("current.yaml", b"generated\n")],
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

    #[cfg(unix)]
    #[test]
    fn preflighted_reconciliation_stays_on_the_anchored_root_after_path_replacement() {
        let fixture = ReconciliationFixture::new();
        fixture
            .apply_generation(&[("main.yml", b"before\n")], ExistingOutputPolicy::Reject)
            .expect("initial generation");
        let (generated, budget) =
            fixture.generation(&[("main.yml", b"after\n"), ("new.yml", b"new\n")]);
        let reconciliation = CatalogReconciliation::new(
            &fixture.store,
            generated,
            ExistingOutputPolicy::Reject,
            budget,
        )
        .expect("anchored reconciliation");
        let anchored = fixture._temp.child("anchored contracts");

        std::fs::rename(fixture.contracts.path(), anchored.path())
            .expect("move the anchored contracts root");
        fixture
            .contracts
            .create_dir_all()
            .expect("replacement contracts root");
        fixture
            .contracts
            .child("main.yml")
            .write_binary(b"replacement\n")
            .expect("replacement document");

        reconciliation
            .apply()
            .expect("reconciliation must stay capability-relative");

        anchored.child("main.yml").assert(b"after\n" as &[u8]);
        anchored.child("new.yml").assert(b"new\n" as &[u8]);
        anchored
            .child(".contract-kit/generated-files.json")
            .assert(predicates::path::exists());
        fixture
            .contracts
            .child("main.yml")
            .assert(b"replacement\n" as &[u8]);
        fixture
            .contracts
            .child("new.yml")
            .assert(predicates::path::missing());
        fixture
            .contracts
            .child(".contract-kit")
            .assert(predicates::path::missing());
    }

    #[cfg(unix)]
    #[test]
    fn preflighted_reconciliation_ignores_a_selected_root_symlink_retarget() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temporary root");
        let original = temp.child("original contracts");
        let replacement = temp.child("replacement contracts");
        let selected = temp.child("selected contracts");
        original.create_dir_all().expect("original contracts root");
        replacement
            .create_dir_all()
            .expect("replacement contracts root");
        symlink(original.path(), selected.path()).expect("selected root symlink");
        let store = ContractsStore::new(selected.path().to_path_buf());

        let mut initial_documents = FileCatalog::new();
        initial_documents
            .insert(
                CatalogPath::new("main.yml").expect("initial logical path"),
                b"before\n".to_vec(),
            )
            .expect("initial document");
        let cancellation = crate::context::ApplicationCancellation::new();
        let mut initial_budget = store.begin_reconciliation_read(&cancellation);
        let initial = GeneratedContracts::new(
            store
                .read_optional_with_budget(&mut initial_budget)
                .expect("initial baseline"),
            initial_documents,
        );
        store
            .write_generated_with_budget(initial, ExistingOutputPolicy::Reject, initial_budget)
            .expect("initial generation");
        let mut next_documents = FileCatalog::new();
        next_documents
            .insert(
                CatalogPath::new("main.yml").expect("updated logical path"),
                b"after\n".to_vec(),
            )
            .expect("updated document");
        next_documents
            .insert(
                CatalogPath::new("new.yml").expect("new logical path"),
                b"new\n".to_vec(),
            )
            .expect("new document");
        let mut next_budget = store.begin_reconciliation_read(&cancellation);
        let baseline = store
            .read_with_budget(&mut next_budget)
            .expect("updated baseline");
        let reconciliation = CatalogReconciliation::new(
            &store,
            GeneratedContracts::new(baseline, next_documents),
            ExistingOutputPolicy::Reject,
            next_budget,
        )
        .expect("anchored reconciliation");

        std::fs::remove_file(selected.path()).expect("remove selected root symlink");
        symlink(replacement.path(), selected.path()).expect("retarget selected root symlink");
        replacement
            .child("main.yml")
            .write_binary(b"replacement\n")
            .expect("replacement document");

        reconciliation
            .apply()
            .expect("reconciliation must ignore symlink retargeting");

        original.child("main.yml").assert(b"after\n" as &[u8]);
        original.child("new.yml").assert(b"new\n" as &[u8]);
        replacement
            .child("main.yml")
            .assert(b"replacement\n" as &[u8]);
        replacement
            .child("new.yml")
            .assert(predicates::path::missing());
        replacement
            .child(".contract-kit")
            .assert(predicates::path::missing());
    }

    #[test]
    fn ownership_reads_accept_the_exact_limit_and_reject_one_more_byte() {
        let exact = ReconciliationFixture::new();
        let manifest = OwnershipManifest::committed(1, OwnedCatalog::default());
        let path = OwnershipManifest::path(exact.store.path());
        let bytes = manifest
            .to_bytes(&path, &ReconciliationFixture::cancellation())
            .expect("ownership bytes");
        exact.write_manifest(&manifest);
        let byte_limit = u64::try_from(bytes.len()).expect("ownership length fits u64");
        let exact_store = ContractsStore::new(exact.contracts.path().to_path_buf())
            .with_limits(CatalogReadLimits::new(1, byte_limit, byte_limit));
        let cancellation = crate::context::ApplicationCancellation::new();
        let mut budget = exact_store.begin_reconciliation_read(&cancellation);
        exact_store
            .recover_interrupted_generation_with_budget(&mut budget)
            .expect("exact-size ownership manifest");

        let excessive = ReconciliationFixture::new();
        excessive.write_manifest(&manifest);
        let excessive_store = ContractsStore::new(excessive.contracts.path().to_path_buf())
            .with_limits(CatalogReadLimits::new(
                1,
                byte_limit.saturating_sub(1),
                byte_limit.saturating_sub(1),
            ));
        let mut budget = excessive_store.begin_reconciliation_read(&cancellation);
        let error = excessive_store
            .recover_interrupted_generation_with_budget(&mut budget)
            .expect_err("one byte beyond the ownership limit must fail");
        let CliError::CatalogReadLimit(error) = error else {
            panic!("expected typed file-byte limit")
        };
        assert_eq!(error.resource, CatalogReadLimitResource::FileBytes);
        assert_eq!(error.limit, byte_limit - 1);
        assert_eq!(error.observed_at_least, byte_limit);
        assert_eq!(
            error.path,
            excessive
                .contracts
                .child(".contract-kit/generated-files.json")
                .path()
        );
    }

    #[test]
    fn stale_generation_cannot_overwrite_newer_combined_document() {
        let fixture = ReconciliationFixture::new();
        fixture
            .apply_generation(&[("main.yml", b"initial\n")], ExistingOutputPolicy::Reject)
            .expect("initial generation");

        let (stale, stale_budget) =
            fixture.generation(&[("main.yml", b"stale signature update\n")]);
        let (newer, newer_budget) = fixture.generation(&[("main.yml", b"newer sketch update\n")]);
        fixture
            .store
            .write_generated_with_budget(newer, ExistingOutputPolicy::Reject, newer_budget)
            .expect("newer generation");
        let ownership = fixture
            .contracts
            .child(".contract-kit/generated-files.json");
        let ownership_before = fs_err::read(ownership.path()).expect("newer ownership bytes");

        let error = fixture
            .store
            .write_generated_with_budget(stale, ExistingOutputPolicy::Reject, stale_budget)
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
            .apply_generation(&[("main.yml", b"initial\n")], ExistingOutputPolicy::Reject)
            .expect("initial generation");

        let (stale, stale_budget) =
            fixture.generation(&[(".contract-kit/blocked.yml", b"must not be written\n")]);
        let (newer, newer_budget) = fixture.generation(&[("main.yml", b"newer sketch update\n")]);
        fixture
            .store
            .write_generated_with_budget(newer, ExistingOutputPolicy::Reject, newer_budget)
            .expect("newer generation");
        let ownership = fixture
            .contracts
            .child(".contract-kit/generated-files.json");
        let ownership_before = fs_err::read(ownership.path()).expect("newer ownership bytes");

        let error = fixture
            .store
            .write_generated_with_budget(stale, ExistingOutputPolicy::Reject, stale_budget)
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
        let (generated, budget) = fixture.generation(&[("main.yml", b"generated\n")]);
        fixture
            .contracts
            .child("manual.yml")
            .write_binary(b"external\n")
            .expect("external contract");

        let error = fixture
            .store
            .write_generated_with_budget(generated, ExistingOutputPolicy::Reject, budget)
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
            .apply_generation(&[("main.yml", b"initial\n")], ExistingOutputPolicy::Reject)
            .expect("initial generation");
        let (generated, budget) = fixture.generation(&[("main.yml", b"updated\n")]);
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
            .write_generated_with_budget(generated, ExistingOutputPolicy::Reject, budget)
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
        let (generated, budget) = fixture.generation(&[("main.yml", b"generated\n")]);

        fixture
            .store
            .write_generated_with_budget(generated, ExistingOutputPolicy::Reject, budget)
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
            .apply_generation(
                &[("current.yaml", b"generated\n")],
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
            .apply_generation(
                &[("current.yaml", b"generated\n")],
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
        let (generated, budget) = fixture.generation(&[("new.yaml", b"generated\n")]);
        let reconciliation = CatalogReconciliation::new(
            &fixture.store,
            generated,
            ExistingOutputPolicy::Reject,
            budget,
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
        let (generated, budget) = fixture.generation(&[("adopt.yaml", b"matching\n")]);
        let reconciliation = CatalogReconciliation::new(
            &fixture.store,
            generated,
            ExistingOutputPolicy::AdoptMatching,
            budget,
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

    #[test]
    fn hard_linked_owned_outputs_are_rejected_as_host_aliases() {
        let fixture = ReconciliationFixture::new();
        fixture
            .apply_generation(
                &[("a.yaml", b"same\n"), ("b.yaml", b"same\n")],
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
            .apply_generation(
                &[("a.yaml", b"same\n"), ("b.yaml", b"same\n")],
                ExistingOutputPolicy::Reject,
            )
            .expect_err("host aliases must fail preflight");

        assert!(error.to_string().contains("same host file"));
        assert_eq!(
            fs_err::read(ownership.path()).expect("unchanged ownership"),
            ownership_before,
        );
    }

    #[test]
    fn requested_and_stale_outputs_are_rejected_when_they_alias() {
        let fixture = ReconciliationFixture::new();
        fixture
            .apply_generation(
                &[("current.yaml", b"same\n"), ("stale.yaml", b"same\n")],
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
            .apply_generation(&[("current.yaml", b"same\n")], ExistingOutputPolicy::Reject)
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
