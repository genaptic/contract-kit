//! Filesystem-to-catalog adapters.
//!
//! Source reads, contracts persistence, path security, persisted ownership,
//! and runtime reconciliation each have one concrete owner below this facade.

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::context::ApplicationCancellation;
use crate::error::CliError;

mod ownership;
mod path;
mod reconciliation;
mod source;
mod store;

pub(crate) use path::{PathRole, ResolvedPath};
pub(crate) use source::SourceTree;
pub(crate) use store::{
    ContractsStore, ExistingOutputPolicy, GeneratedContracts, GenerationReceipt,
};

/// CLI filesystem budgets applied before bytes cross into either domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CatalogReadLimits {
    traversal_entry_count: u64,
    entry_count: u64,
    total_bytes: u64,
    per_file_bytes: u64,
}

impl CatalogReadLimits {
    /// Creates one complete catalog-read policy.
    pub(crate) const fn new(entry_count: u64, total_bytes: u64, per_file_bytes: u64) -> Self {
        Self {
            traversal_entry_count: 100_000,
            entry_count,
            total_bytes,
            per_file_bytes,
        }
    }

    /// Returns the maximum bytes accepted from one filesystem file.
    pub(crate) const fn per_file_bytes(&self) -> u64 {
        self.per_file_bytes
    }

    /// Starts one budget that may span every catalog input read in an operation.
    pub(crate) fn begin(self, cancellation: &ApplicationCancellation) -> CatalogReadBudget {
        CatalogReadBudget {
            limits: self,
            traversal_entries: 0,
            entries: 0,
            total_bytes: 0,
            cancellation: cancellation.clone(),
        }
    }
}

impl Default for CatalogReadLimits {
    fn default() -> Self {
        Self::new(10_000, 512 * 1024 * 1024, 64 * 1024 * 1024)
    }
}

/// Filesystem resource whose CLI read budget was exceeded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CatalogReadLimitResource {
    /// Number of filesystem entries inspected during broad traversal.
    TraversalEntryCount,
    /// Number of participating catalog entries.
    EntryCount,
    /// Aggregate bytes across participating entries.
    TotalBytes,
    /// Bytes read from one participating file.
    FileBytes,
}

impl fmt::Display for CatalogReadLimitResource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TraversalEntryCount => "catalog traversal entry count",
            Self::EntryCount => "catalog entry count",
            Self::TotalBytes => "catalog total bytes",
            Self::FileBytes => "catalog file bytes",
        })
    }
}

/// Typed evidence for a CLI filesystem catalog budget failure.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error(
    "{resource} limit exceeded: limit {limit}, observed at least {observed_at_least} at {display_path}",
    display_path = .path.display()
)]
pub(crate) struct CatalogReadLimitExceeded {
    resource: CatalogReadLimitResource,
    limit: u64,
    observed_at_least: u64,
    path: PathBuf,
}

impl CatalogReadLimitExceeded {
    fn new(
        resource: CatalogReadLimitResource,
        limit: u64,
        observed_at_least: u64,
        path: PathBuf,
    ) -> Self {
        Self {
            resource,
            limit,
            observed_at_least,
            path,
        }
    }
}

/// Mutable accounting for one deterministic catalog-input operation.
#[derive(Debug)]
pub(crate) struct CatalogReadBudget {
    limits: CatalogReadLimits,
    traversal_entries: u64,
    entries: u64,
    total_bytes: u64,
    cancellation: ApplicationCancellation,
}

#[derive(Debug)]
pub(super) enum CatalogFileRead {
    Complete(Vec<u8>),
    CeilingExceeded,
}

impl CatalogReadBudget {
    const READ_CHUNK_BYTES: usize = 64 * 1024;

    /// Stops a synchronous catalog boundary when the root operation was canceled.
    pub(crate) fn checkpoint(&self) -> Result<(), CliError> {
        self.cancellation.checkpoint()
    }

    /// Returns the root cancellation source shared by this operation ledger.
    pub(crate) fn cancellation(&self) -> &ApplicationCancellation {
        &self.cancellation
    }

    /// Reads one already-opened file in bounded chunks and commits its actual bytes.
    pub(super) fn read_file<R>(&mut self, path: &Path, reader: &mut R) -> Result<Vec<u8>, CliError>
    where
        R: Read + ?Sized,
    {
        match self.read_file_with_ceiling(path, reader, u64::MAX)? {
            CatalogFileRead::Complete(bytes) => Ok(bytes),
            CatalogFileRead::CeilingExceeded => Err(CliError::Io(std::io::Error::other(
                "catalog read crossed an unbounded external ceiling",
            ))),
        }
    }

    pub(super) fn read_file_with_ceiling<R>(
        &mut self,
        path: &Path,
        reader: &mut R,
        wire_ceiling: u64,
    ) -> Result<CatalogFileRead, CliError>
    where
        R: Read + ?Sized,
    {
        let limit = self.read_limit().min(wire_ceiling.saturating_add(1));
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; Self::READ_CHUNK_BYTES];

        while u64::try_from(bytes.len()).unwrap_or(u64::MAX) < limit {
            self.checkpoint()?;
            let remaining = limit.saturating_sub(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
            let capacity = usize::try_from(remaining)
                .unwrap_or(usize::MAX)
                .min(buffer.len());
            let read = reader.read(&mut buffer[..capacity])?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
            if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > wire_ceiling {
                return Ok(CatalogFileRead::CeilingExceeded);
            }
        }

        self.checkpoint()?;
        self.finish_file(path, bytes.len())?;
        Ok(CatalogFileRead::Complete(bytes))
    }

    /// Stream-compares one already-opened file with an expected snapshot while
    /// charging the same operation-wide entry and byte budgets as a normal read.
    pub(super) fn compare_file<R>(
        &mut self,
        path: &Path,
        reader: &mut R,
        expected: &[u8],
    ) -> Result<bool, CliError>
    where
        R: Read + ?Sized,
    {
        let limit = self.read_limit();
        let mut buffer = [0_u8; Self::READ_CHUNK_BYTES];
        let mut bytes_read = 0_usize;
        let mut matches = true;

        while u64::try_from(bytes_read).unwrap_or(u64::MAX) < limit {
            self.checkpoint()?;
            let remaining = limit.saturating_sub(u64::try_from(bytes_read).unwrap_or(u64::MAX));
            let capacity = usize::try_from(remaining)
                .unwrap_or(usize::MAX)
                .min(buffer.len());
            let read = reader.read(&mut buffer[..capacity])?;
            if read == 0 {
                break;
            }
            let expected_end = bytes_read.saturating_add(read);
            matches &= expected
                .get(bytes_read..expected_end)
                .is_some_and(|slice| slice == &buffer[..read]);
            bytes_read = expected_end;
        }

        self.checkpoint()?;
        self.finish_file(path, bytes_read)?;
        Ok(matches && bytes_read == expected.len())
    }

    /// Accounts one already-decoded catalog entry in this operation ledger.
    pub(crate) fn record_entry_bytes(&mut self, path: &Path, bytes: usize) -> Result<(), CliError> {
        self.checkpoint()?;
        self.begin_entry(path)?;
        self.preflight_file(path, u64::try_from(bytes).unwrap_or(u64::MAX))?;
        self.finish_file(path, bytes)?;
        Ok(())
    }

    /// Accounts for one entry inspected by a broad capability-relative walk.
    ///
    /// The path names the directory being enumerated so limit evidence stays
    /// deterministic even though host directory iteration order is not.
    pub(super) fn visit_traversal_entry(
        &mut self,
        directory: &Path,
    ) -> Result<(), CatalogReadLimitExceeded> {
        let entries = self.traversal_entries.saturating_add(1);
        if entries > self.limits.traversal_entry_count {
            return Err(CatalogReadLimitExceeded::new(
                CatalogReadLimitResource::TraversalEntryCount,
                self.limits.traversal_entry_count,
                entries,
                directory.to_path_buf(),
            ));
        }
        self.traversal_entries = entries;
        Ok(())
    }

    /// Reserves one participating entry before opening or byte allocation.
    pub(crate) fn begin_entry(&mut self, path: &Path) -> Result<(), CatalogReadLimitExceeded> {
        let entries = self.entries.saturating_add(1);
        if entries > self.limits.entry_count {
            return Err(CatalogReadLimitExceeded::new(
                CatalogReadLimitResource::EntryCount,
                self.limits.entry_count,
                entries,
                path.to_path_buf(),
            ));
        }
        self.entries = entries;
        Ok(())
    }

    /// Uses opened-handle metadata only as an early byte-budget rejection.
    pub(crate) fn preflight_file(
        &self,
        path: &Path,
        metadata_bytes: u64,
    ) -> Result<(), CatalogReadLimitExceeded> {
        if metadata_bytes > self.limits.per_file_bytes {
            return Err(CatalogReadLimitExceeded::new(
                CatalogReadLimitResource::FileBytes,
                self.limits.per_file_bytes,
                metadata_bytes,
                path.to_path_buf(),
            ));
        }
        let total = self.total_bytes.saturating_add(metadata_bytes);
        if total > self.limits.total_bytes {
            return Err(CatalogReadLimitExceeded::new(
                CatalogReadLimitResource::TotalBytes,
                self.limits.total_bytes,
                total,
                path.to_path_buf(),
            ));
        }
        Ok(())
    }

    /// Returns the maximum bytes this read may consume, including one byte of
    /// evidence that a file or aggregate budget was crossed after metadata.
    pub(crate) fn read_limit(&self) -> u64 {
        let file_limit = self.limits.per_file_bytes.saturating_add(1);
        let remaining_total = self
            .limits
            .total_bytes
            .saturating_sub(self.total_bytes)
            .saturating_add(1);
        file_limit.min(remaining_total)
    }

    /// Commits actual bytes read from the already-opened file.
    pub(crate) fn finish_file(
        &mut self,
        path: &Path,
        bytes: usize,
    ) -> Result<(), CatalogReadLimitExceeded> {
        let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
        if bytes > self.limits.per_file_bytes {
            return Err(CatalogReadLimitExceeded::new(
                CatalogReadLimitResource::FileBytes,
                self.limits.per_file_bytes,
                bytes,
                path.to_path_buf(),
            ));
        }
        let total = self.total_bytes.saturating_add(bytes);
        if total > self.limits.total_bytes {
            return Err(CatalogReadLimitExceeded::new(
                CatalogReadLimitResource::TotalBytes,
                self.limits.total_bytes,
                total,
                path.to_path_buf(),
            ));
        }
        self.total_bytes = total;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read};
    use std::path::Path;

    use assert_fs::prelude::*;
    use conkit_signature::CatalogPath;

    use super::{
        CatalogFileRead, CatalogReadLimitResource, CatalogReadLimits, ContractsStore,
        ExistingOutputPolicy, GeneratedContracts, SourceTree,
    };
    use crate::error::CliError;

    struct ShortReader {
        bytes: &'static [u8],
        offset: usize,
        maximum_read: usize,
    }

    impl ShortReader {
        fn new(bytes: &'static [u8], maximum_read: usize) -> Self {
            Self {
                bytes,
                offset: 0,
                maximum_read,
            }
        }
    }

    impl Read for ShortReader {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            let remaining = &self.bytes[self.offset..];
            let count = remaining.len().min(output.len()).min(self.maximum_read);
            output[..count].copy_from_slice(&remaining[..count]);
            self.offset += count;
            Ok(count)
        }
    }

    struct FailingReader;

    impl Read for FailingReader {
        fn read(&mut self, _output: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("injected catalog read failure"))
        }
    }

    #[test]
    fn ceiling_aware_read_accepts_empty_exact_and_short_reads() {
        let cancellation = crate::context::ApplicationCancellation::new();
        let mut budget = CatalogReadLimits::new(3, 16, 16).begin(&cancellation);

        for (name, bytes, ceiling) in [
            ("empty", b"".as_slice(), 0_u64),
            ("exact", b"four".as_slice(), 4_u64),
        ] {
            let path = Path::new(name);
            budget.begin_entry(path).expect("reserve catalog entry");
            let read = budget
                .read_file_with_ceiling(path, &mut Cursor::new(bytes), ceiling)
                .expect("read at external ceiling");
            assert!(matches!(read, CatalogFileRead::Complete(value) if value == bytes));
        }

        let path = Path::new("short");
        budget.begin_entry(path).expect("reserve short-read entry");
        let read = budget
            .read_file_with_ceiling(path, &mut ShortReader::new(b"abc", 1), 3)
            .expect("short reads complete");
        assert!(matches!(read, CatalogFileRead::Complete(value) if value == b"abc"));
        assert_eq!(budget.total_bytes, 7);
    }

    #[test]
    fn external_ceiling_wins_before_catalog_bytes_are_committed() {
        let cancellation = crate::context::ApplicationCancellation::new();
        let mut budget = CatalogReadLimits::new(2, 64, 64).begin(&cancellation);
        let path = Path::new("wire");
        budget.begin_entry(path).expect("reserve catalog entry");

        let read = budget
            .read_file_with_ceiling(path, &mut Cursor::new(b"four"), 3)
            .expect("external ceiling is a typed outcome");

        assert!(matches!(read, CatalogFileRead::CeilingExceeded));
        assert_eq!(budget.total_bytes, 0);
    }

    #[test]
    fn tighter_catalog_limit_and_io_and_cancellation_keep_typed_precedence() {
        let cancellation = crate::context::ApplicationCancellation::new();
        let mut limited = CatalogReadLimits::new(1, 2, 2).begin(&cancellation);
        let path = Path::new("catalog");
        limited.begin_entry(path).expect("reserve catalog entry");
        let error = limited
            .read_file_with_ceiling(path, &mut Cursor::new(b"three"), 10)
            .expect_err("catalog limit is tighter");
        assert!(matches!(
            error,
            CliError::CatalogReadLimit(ref limit)
                if limit.resource == CatalogReadLimitResource::FileBytes
                    && limit.observed_at_least == 3
        ));

        let mut failing = CatalogReadLimits::default().begin(&cancellation);
        let path = Path::new("io");
        failing.begin_entry(path).expect("reserve catalog entry");
        let error = failing
            .read_file_with_ceiling(path, &mut FailingReader, 10)
            .expect_err("reader failure");
        assert!(matches!(error, CliError::Io(_)));
        assert_eq!(failing.total_bytes, 0);

        let canceled = crate::context::ApplicationCancellation::new();
        canceled.request();
        let mut canceled_budget = CatalogReadLimits::default().begin(&canceled);
        let error = canceled_budget
            .read_file_with_ceiling(Path::new("canceled"), &mut Cursor::new(b"bytes"), 10)
            .expect_err("canceled read");
        assert!(matches!(error, CliError::OperationCanceled));
    }

    #[test]
    fn canceled_operation_stops_before_a_contract_catalog_walk() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let contracts = temp.child("contracts");
        contracts.create_dir_all().expect("contracts root");
        contracts
            .child("main.yml")
            .write_str("contract_version: 2\n")
            .expect("contract input");
        let cancellation = crate::context::ApplicationCancellation::new();
        cancellation.request();
        let mut budget = CatalogReadLimits::default().begin(&cancellation);

        let error = ContractsStore::new(contracts.path().to_path_buf())
            .read_with_budget(&mut budget)
            .expect_err("a canceled walk must stop before catalog construction");

        assert!(matches!(error, CliError::OperationCanceled));
        temp.close().expect("close temporary root");
    }

    #[test]
    fn one_operation_counts_contract_and_source_entries_together() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let contracts = temp.child("contracts");
        contracts.create_dir_all().expect("contracts root");
        contracts
            .child("main.yml")
            .write_str("abc")
            .expect("contract input");
        let source = temp.child("source");
        source.create_dir_all().expect("source root");
        source
            .child("first.rs")
            .write_str("one")
            .expect("first source input");
        source
            .child("second.rs")
            .write_str("two")
            .expect("second source input");
        let limits = CatalogReadLimits::new(2, 64, 64);
        let contracts = ContractsStore::new(contracts.path().to_path_buf()).with_limits(limits);
        let source = SourceTree::open(source.path().to_path_buf())
            .expect("source tree")
            .with_limits(limits);
        let selected = [
            CatalogPath::new("first.rs").expect("first logical path"),
            CatalogPath::new("second.rs").expect("second logical path"),
        ];

        let cancellation = crate::context::ApplicationCancellation::new();
        let mut isolated_contracts = limits.begin(&cancellation);
        assert_eq!(
            contracts
                .read_with_budget(&mut isolated_contracts)
                .expect("isolated contracts read")
                .len(),
            1
        );
        let mut isolated_source = limits.begin(&cancellation);
        assert_eq!(
            source
                .read_selected_with_budget(&selected, &mut isolated_source)
                .expect("isolated source read")
                .len(),
            2,
        );

        let mut operation = limits.begin(&crate::context::ApplicationCancellation::new());
        contracts
            .read_with_budget(&mut operation)
            .expect("first operation leg");
        let error = source
            .read_selected_with_budget(&selected, &mut operation)
            .expect_err("the third participating entry must exceed one operation budget");

        let CliError::CatalogReadLimit(error) = error else {
            panic!("expected an entry-count limit error");
        };
        assert_eq!(error.resource, CatalogReadLimitResource::EntryCount);
        assert_eq!(error.limit, 2);
        assert_eq!(error.observed_at_least, 3);
        assert!(error.path.ends_with("second.rs"));
        drop(operation);
        drop(source);
        drop(contracts);
        temp.close().expect("close temporary root");
    }

    #[test]
    fn one_operation_counts_contract_and_source_bytes_together() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let contracts = temp.child("contracts");
        contracts.create_dir_all().expect("contracts root");
        contracts
            .child("main.yml")
            .write_str("abc")
            .expect("contract input");
        let source = temp.child("source");
        source.create_dir_all().expect("source root");
        source
            .child("selected.rs")
            .write_str("def")
            .expect("source input");
        let limits = CatalogReadLimits::new(4, 5, 4);
        let contracts = ContractsStore::new(contracts.path().to_path_buf()).with_limits(limits);
        let source = SourceTree::open(source.path().to_path_buf())
            .expect("source tree")
            .with_limits(limits);
        let selected = [CatalogPath::new("selected.rs").expect("logical path")];

        let cancellation = crate::context::ApplicationCancellation::new();
        let mut isolated_contracts = limits.begin(&cancellation);
        assert_eq!(
            contracts
                .read_with_budget(&mut isolated_contracts)
                .expect("isolated contracts read")
                .len(),
            1
        );
        let mut isolated_source = limits.begin(&cancellation);
        assert_eq!(
            source
                .read_selected_with_budget(&selected, &mut isolated_source)
                .expect("isolated source read")
                .len(),
            1,
        );

        let mut operation = limits.begin(&crate::context::ApplicationCancellation::new());
        contracts
            .read_with_budget(&mut operation)
            .expect("first operation leg");
        let error = source
            .read_selected_with_budget(&selected, &mut operation)
            .expect_err("six cumulative bytes must exceed a five-byte operation budget");

        let CliError::CatalogReadLimit(error) = error else {
            panic!("expected a total-byte limit error");
        };
        assert_eq!(error.resource, CatalogReadLimitResource::TotalBytes);
        assert_eq!(error.limit, 5);
        assert_eq!(error.observed_at_least, 6);
        assert!(error.path.ends_with("selected.rs"));
        drop(operation);
        drop(source);
        drop(contracts);
        temp.close().expect("close temporary root");
    }

    #[test]
    fn generation_reconciliation_reuses_the_input_catalog_budget() {
        let temp = assert_fs::TempDir::new().expect("temporary root");
        let contracts = temp.child("contracts");
        contracts.create_dir_all().expect("contracts root");
        contracts
            .child("main.yml")
            .write_str("before")
            .expect("contract input");
        let limits = CatalogReadLimits::new(2, 64, 64);
        let store = ContractsStore::new(contracts.path().to_path_buf()).with_limits(limits);
        let cancellation = crate::context::ApplicationCancellation::new();
        let mut operation = limits.begin(&cancellation);
        let baseline = store
            .read_with_budget(&mut operation)
            .expect("initial generation input read");
        let mut documents = conkit_signature::FileCatalog::new();
        documents
            .insert(
                CatalogPath::new("main.yml").expect("document path"),
                b"after".to_vec(),
            )
            .expect("generated document");

        let error = store
            .write_generated_with_budget(
                GeneratedContracts::new(baseline, documents),
                ExistingOutputPolicy::Reject,
                operation,
            )
            .expect_err("reconciliation rereads must share the invocation budget");

        let CliError::CatalogReadLimit(error) = error else {
            panic!("expected an entry-count limit error");
        };
        assert_eq!(error.resource, CatalogReadLimitResource::EntryCount);
        assert_eq!(error.limit, 2);
        assert_eq!(error.observed_at_least, 3);
        assert!(
            error.path.ends_with("main.yml"),
            "unexpected limit path {}",
            error.path.display()
        );
        contracts.child("main.yml").assert("before");
        drop(store);
        temp.close().expect("close temporary root");
    }
}
