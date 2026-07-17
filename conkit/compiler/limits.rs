//! Compiler cancellation, operation-wide resource accounting, and temporary-tree limits.

use std::fs::File;
use std::io::{ErrorKind, Read};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cargo_metadata::Metadata;

use super::error::CompilerError;
use super::process::{CompilerOperation, CompilerSemanticResource, CompilerStream};

/// Cloneable view of the root command's cancellation state.
#[derive(Clone, Debug)]
pub(crate) struct CompilerCancellation {
    cancelled: Arc<AtomicBool>,
}

impl CompilerCancellation {
    /// Observes the exact flag owned by the root command lifecycle.
    pub(crate) fn from_flag(cancelled: Arc<AtomicBool>) -> Self {
        Self { cancelled }
    }

    fn requested(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// One operation-wide deadline and resource ledger shared by every subprocess,
/// pipe reader, artifact reader, mapping pass, and cleanup step in an
/// extraction.
#[derive(Debug)]
pub(super) struct CompilerUsage {
    limits: CompilerLimits,
    deadline: Instant,
    cancellation: CompilerCancellation,
    stdout_bytes: AtomicU64,
    stderr_bytes: AtomicU64,
    artifact_bytes: AtomicU64,
    cleanup_evidence_bytes: AtomicU64,
    metadata_packages: AtomicU64,
    metadata_targets: AtomicU64,
    rustdoc_items: AtomicU64,
    source_mappings: AtomicU64,
}

impl CompilerUsage {
    pub(super) fn new(limits: CompilerLimits, cancellation: CompilerCancellation) -> Arc<Self> {
        let started = Instant::now();
        let deadline = started.checked_add(limits.timeout).unwrap_or(started);
        Arc::new(Self {
            limits,
            deadline,
            cancellation,
            stdout_bytes: AtomicU64::new(0),
            stderr_bytes: AtomicU64::new(0),
            artifact_bytes: AtomicU64::new(0),
            cleanup_evidence_bytes: AtomicU64::new(0),
            metadata_packages: AtomicU64::new(0),
            metadata_targets: AtomicU64::new(0),
            rustdoc_items: AtomicU64::new(0),
            source_mappings: AtomicU64::new(0),
        })
    }

    pub(super) fn limits(&self) -> &CompilerLimits {
        &self.limits
    }

    pub(super) fn checkpoint(&self, operation: CompilerOperation) -> Result<(), CompilerError> {
        if self.cancellation.requested() {
            return Err(CompilerError::CompilerExtractionCancelled);
        }
        if Instant::now() >= self.deadline {
            return Err(CompilerError::ProcessTimeout {
                operation,
                timeout: self.limits.timeout,
            });
        }
        Ok(())
    }

    pub(super) fn account_output(
        &self,
        operation: CompilerOperation,
        stream: CompilerStream,
        bytes: u64,
    ) -> Result<(), CompilerError> {
        self.checkpoint(operation)?;
        let (counter, limit) = match stream {
            CompilerStream::Stdout => (&self.stdout_bytes, self.limits.stdout_bytes),
            CompilerStream::Stderr => (&self.stderr_bytes, self.limits.stderr_bytes),
        };
        let observed = self.add(counter, bytes);
        if observed > limit {
            return Err(CompilerError::ProcessOutputLimit {
                operation,
                stream,
                limit,
                observed_at_least: observed,
            });
        }
        Ok(())
    }

    pub(super) fn account_artifact(
        &self,
        operation: CompilerOperation,
        path: &Path,
        bytes: u64,
    ) -> Result<(), CompilerError> {
        self.checkpoint(operation)?;
        let observed = self.add(&self.artifact_bytes, bytes);
        if observed > self.limits.artifact_bytes {
            return Err(CompilerError::CompilerArtifactLimit {
                operation,
                path: path.to_path_buf(),
                limit: self.limits.artifact_bytes,
                observed_at_least: observed,
            });
        }
        Ok(())
    }

    pub(super) fn account_semantic(
        &self,
        operation: CompilerOperation,
        resource: CompilerSemanticResource,
        amount: u64,
    ) -> Result<(), CompilerError> {
        self.checkpoint(operation)?;
        let (counter, limit) = match resource {
            CompilerSemanticResource::MetadataPackages => {
                (&self.metadata_packages, self.limits.metadata_packages)
            }
            CompilerSemanticResource::MetadataTargets => {
                (&self.metadata_targets, self.limits.metadata_targets)
            }
            CompilerSemanticResource::RustdocItems => {
                (&self.rustdoc_items, self.limits.rustdoc_items)
            }
            CompilerSemanticResource::SourceMappings => {
                (&self.source_mappings, self.limits.source_mappings)
            }
        };
        let observed = self.add(counter, amount);
        if observed > limit {
            return Err(CompilerError::CompilerSemanticLimit {
                operation,
                resource,
                limit,
                observed_at_least: observed,
            });
        }
        Ok(())
    }

    pub(super) fn account_metadata(&self, metadata: &Metadata) -> Result<(), CompilerError> {
        self.account_semantic(
            CompilerOperation::Metadata,
            CompilerSemanticResource::MetadataPackages,
            u64::try_from(metadata.packages.len()).unwrap_or(u64::MAX),
        )?;
        let mut targets = 0_u64;
        for package in &metadata.packages {
            self.checkpoint(CompilerOperation::Metadata)?;
            targets =
                targets.saturating_add(u64::try_from(package.targets.len()).unwrap_or(u64::MAX));
        }
        self.account_semantic(
            CompilerOperation::Metadata,
            CompilerSemanticResource::MetadataTargets,
            targets,
        )
    }

    pub(super) fn remaining_artifact_bytes(&self) -> u64 {
        self.limits
            .artifact_bytes
            .saturating_sub(self.artifact_bytes.load(Ordering::Acquire))
    }

    pub(super) fn remaining_source_mappings(&self) -> u64 {
        self.limits
            .source_mappings
            .saturating_sub(self.source_mappings.load(Ordering::Acquire))
    }

    pub(super) fn read_artifact(
        &self,
        operation: CompilerOperation,
        path: &Path,
        file_limit: u64,
    ) -> Result<Vec<u8>, CompilerError> {
        self.checkpoint(operation)?;
        let file = File::open(path).map_err(|source| CompilerError::CompilerArtifactRead {
            operation,
            path: path.to_path_buf(),
            source,
        })?;
        let read_limit = file_limit
            .min(self.remaining_artifact_bytes())
            .saturating_add(1);
        let mut reader = file.take(read_limit);
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 8 * 1024];
        loop {
            self.checkpoint(operation)?;
            let read =
                reader
                    .read(&mut buffer)
                    .map_err(|source| CompilerError::CompilerArtifactRead {
                        operation,
                        path: path.to_path_buf(),
                        source,
                    })?;
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
        }
        let observed = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if observed > file_limit {
            return Err(CompilerError::CompilerArtifactFileLimit {
                operation,
                path: path.to_path_buf(),
                limit: file_limit,
                observed_at_least: observed,
            });
        }
        self.account_artifact(operation, path, observed)?;
        Ok(bytes)
    }

    pub(super) fn output_observed(&self, stream: CompilerStream) -> u64 {
        match stream {
            CompilerStream::Stdout => self.stdout_bytes.load(Ordering::Acquire),
            CompilerStream::Stderr => self.stderr_bytes.load(Ordering::Acquire),
        }
    }

    pub(super) fn reserve_cleanup_evidence(&self, requested: u64) -> u64 {
        let limit = self.limits.cleanup_evidence_bytes;
        loop {
            let current = self.cleanup_evidence_observed();
            let granted = requested.min(limit.saturating_sub(current));
            if granted == 0 {
                return 0;
            }
            if self
                .cleanup_evidence_bytes
                .compare_exchange_weak(
                    current,
                    current.saturating_add(granted),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return granted;
            }
        }
    }

    pub(in crate::compiler) fn cleanup_evidence_observed(&self) -> u64 {
        self.cleanup_evidence_bytes.load(Ordering::Acquire)
    }

    pub(super) fn cancellation_requested(&self) -> bool {
        self.cancellation.requested()
    }

    fn add(&self, counter: &AtomicU64, bytes: u64) -> u64 {
        counter
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                Some(current.saturating_add(bytes))
            })
            .unwrap_or_else(|current| current)
            .saturating_add(bytes)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TemporaryTreeConsistency {
    Mutating,
    Quiescent,
}

impl TemporaryTreeConsistency {
    fn ignores_kind(self, descendant: bool, kind: ErrorKind) -> bool {
        descendant && matches!(self, Self::Mutating) && kind == ErrorKind::NotFound
    }

    fn ignores(self, descendant: bool, source: &walkdir::Error) -> bool {
        source
            .io_error()
            .is_some_and(|source| self.ignores_kind(descendant, source.kind()))
    }
}

pub(super) struct CompilerTemporaryTree<'operation> {
    pub(super) root: &'operation Path,
    pub(super) operation: CompilerOperation,
    pub(super) usage: &'operation CompilerUsage,
    pub(super) next_scan: Instant,
}

impl CompilerTemporaryTree<'_> {
    pub(super) fn inspect_if_due(&mut self, now: Instant) -> Result<(), CompilerError> {
        if now < self.next_scan {
            return Ok(());
        }

        self.inspect(TemporaryTreeConsistency::Mutating)?;
        let completed = Instant::now();
        self.next_scan = completed
            .checked_add(self.usage.limits().temporary_tree_scan_interval)
            .unwrap_or(completed);
        Ok(())
    }

    pub(super) fn inspect_after_completion(&self) -> Result<(), CompilerError> {
        self.inspect(TemporaryTreeConsistency::Quiescent)
    }

    fn inspect(&self, consistency: TemporaryTreeConsistency) -> Result<(), CompilerError> {
        let mut total = 0_u64;
        let mut entries = 0_u64;
        for candidate in walkdir::WalkDir::new(self.root).follow_links(false) {
            self.usage.checkpoint(self.operation)?;
            let entry = match candidate {
                Ok(entry) => entry,
                Err(source) if consistency.ignores(source.depth() > 0, &source) => continue,
                Err(source) => {
                    return Err(CompilerError::TemporaryArtifactWalk {
                        path: self.root.to_path_buf(),
                        source,
                    });
                }
            };
            let file_bytes = if entry.file_type().is_file() {
                match entry.metadata() {
                    Ok(metadata) => Some(metadata.len()),
                    Err(source) if consistency.ignores(entry.depth() > 0, &source) => continue,
                    Err(source) => {
                        return Err(CompilerError::TemporaryArtifactMetadata {
                            path: entry.path().to_path_buf(),
                            message: source.to_string(),
                        });
                    }
                }
            } else {
                None
            };

            entries = entries.saturating_add(1);
            if entries > self.usage.limits().temporary_tree_entries {
                return Err(CompilerError::TemporaryArtifactEntryLimit {
                    path: self.root.to_path_buf(),
                    limit: self.usage.limits().temporary_tree_entries,
                    observed_at_least: entries,
                });
            }
            if let Some(file_bytes) = file_bytes {
                total = total.saturating_add(file_bytes);
                if total > self.usage.limits().temporary_tree_bytes {
                    return Err(CompilerError::TemporaryArtifactLimit {
                        path: self.root.to_path_buf(),
                        limit: self.usage.limits().temporary_tree_bytes,
                        observed_at_least: total,
                    });
                }
            }
        }
        Ok(())
    }
}

/// Resource budgets for one compiler extraction.
#[derive(Clone, Copy, Debug)]
pub(super) struct CompilerLimits {
    pub(super) stdout_bytes: u64,
    pub(super) stderr_bytes: u64,
    pub(super) artifact_bytes: u64,
    pub(super) cleanup_evidence_bytes: u64,
    pub(super) metadata_packages: u64,
    pub(super) metadata_targets: u64,
    pub(super) rustdoc_items: u64,
    pub(super) source_mappings: u64,
    pub(super) temporary_tree_entries: u64,
    pub(super) temporary_tree_bytes: u64,
    pub(super) timeout: Duration,
    pub(super) cleanup_timeout: Duration,
    pub(super) poll_interval: Duration,
    pub(super) temporary_tree_scan_interval: Duration,
}

impl Default for CompilerLimits {
    fn default() -> Self {
        Self {
            stdout_bytes: 32 * 1024 * 1024,
            stderr_bytes: 16 * 1024 * 1024,
            artifact_bytes: 256 * 1024 * 1024,
            cleanup_evidence_bytes: 16 * 1024,
            metadata_packages: 10_000,
            metadata_targets: 100_000,
            rustdoc_items: 1_000_000,
            source_mappings: 1_000_000,
            temporary_tree_entries: 100_000,
            temporary_tree_bytes: 2 * 1024 * 1024 * 1024,
            timeout: Duration::from_secs(10 * 60),
            cleanup_timeout: Duration::from_secs(2),
            poll_interval: Duration::from_millis(25),
            temporary_tree_scan_interval: Duration::from_secs(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::compiler::tests::*;

    #[test]
    fn compiler_limits_are_finite_and_independently_bounded() {
        let limits = CompilerLimits::default();

        assert!(limits.stdout_bytes > 0);
        assert!(limits.stderr_bytes > 0);
        assert!(limits.artifact_bytes > 0);
        assert!(limits.cleanup_evidence_bytes > 0);
        assert!(limits.metadata_packages > 0);
        assert!(limits.metadata_targets >= limits.metadata_packages);
        assert!(limits.rustdoc_items > 0);
        assert!(limits.source_mappings <= limits.rustdoc_items);
        assert!(limits.temporary_tree_entries > 0);
        assert!(limits.temporary_tree_bytes >= limits.artifact_bytes);
        assert!(limits.timeout > Duration::ZERO);
        assert!(limits.cleanup_timeout > Duration::ZERO);
        assert!(limits.poll_interval > Duration::ZERO);
        assert!(limits.temporary_tree_scan_interval > limits.poll_interval);
    }

    #[test]
    fn mutating_tree_ignores_only_not_found() {
        assert!(TemporaryTreeConsistency::Mutating.ignores_kind(true, ErrorKind::NotFound));
        assert!(!TemporaryTreeConsistency::Mutating.ignores_kind(false, ErrorKind::NotFound));
        assert!(!TemporaryTreeConsistency::Quiescent.ignores_kind(true, ErrorKind::NotFound));
        assert!(
            !TemporaryTreeConsistency::Mutating.ignores_kind(true, ErrorKind::PermissionDenied)
        );
        assert!(
            !TemporaryTreeConsistency::Quiescent.ignores_kind(true, ErrorKind::PermissionDenied)
        );
    }

    #[test]
    fn missing_mutating_tree_returns_a_typed_walk_error() {
        let root = assert_fs::TempDir::new().expect("temporary target parent");
        let missing = root.path().join("missing");
        let limits = CompilerLimits::default();
        let usage = CompilerUsage::new(limits, CompilerFixture::cancellation());
        let due = Instant::now();
        let error = CompilerTemporaryTree {
            root: &missing,
            operation: CompilerOperation::Rustdoc,
            usage: usage.as_ref(),
            next_scan: due,
        }
        .inspect_if_due(due)
        .expect_err("a missing live target root must fail");

        assert!(matches!(error, CompilerError::TemporaryArtifactWalk { .. }));
        root.close().expect("close temporary target parent");
    }

    #[test]
    fn temporary_tree_meter_defers_rescan_until_due() {
        let root = assert_fs::TempDir::new().expect("temporary target tree");
        let limits = CompilerLimits {
            temporary_tree_entries: 1,
            temporary_tree_bytes: 32,
            temporary_tree_scan_interval: Duration::from_secs(1),
            ..CompilerLimits::default()
        };
        let usage = CompilerUsage::new(limits, CompilerFixture::cancellation());
        let started = Instant::now();
        let mut tree = CompilerTemporaryTree {
            root: root.path(),
            operation: CompilerOperation::Rustdoc,
            usage: usage.as_ref(),
            next_scan: started,
        };

        tree.inspect_if_due(started)
            .expect("initial root-only scan remains within the entry budget");
        std::fs::write(root.path().join("artifact"), b"new").expect("temporary compiler artifact");
        let due = tree.next_scan;
        tree.inspect_if_due(
            due.checked_sub(Duration::from_nanos(1))
                .expect("scan deadline subtraction"),
        )
        .expect("a scan before the cadence deadline is deferred");

        let error = tree
            .inspect_if_due(due)
            .expect_err("the due scan must observe the added entry");
        assert!(matches!(
            error,
            CompilerError::TemporaryArtifactEntryLimit {
                limit: 1,
                observed_at_least: 2,
                ..
            }
        ));
        root.close().expect("close temporary target tree");
    }

    #[test]
    fn quiescent_tree_meter_enforces_exact_entry_and_byte_limits() {
        let root = assert_fs::TempDir::new().expect("temporary target tree");
        std::fs::write(root.path().join("artifact"), b"four").expect("temporary compiler artifact");

        let exact_limits = CompilerLimits {
            temporary_tree_entries: 2,
            temporary_tree_bytes: 4,
            ..CompilerLimits::default()
        };
        let exact_usage = CompilerUsage::new(exact_limits, CompilerFixture::cancellation());
        CompilerTemporaryTree {
            root: root.path(),
            operation: CompilerOperation::Rustdoc,
            usage: exact_usage.as_ref(),
            next_scan: Instant::now(),
        }
        .inspect_after_completion()
        .expect("exact entry and byte limits are inclusive");

        let entry_limits = CompilerLimits {
            temporary_tree_entries: 1,
            temporary_tree_bytes: 4,
            ..CompilerLimits::default()
        };
        let entry_usage = CompilerUsage::new(entry_limits, CompilerFixture::cancellation());
        let entry_error = CompilerTemporaryTree {
            root: root.path(),
            operation: CompilerOperation::Rustdoc,
            usage: entry_usage.as_ref(),
            next_scan: Instant::now(),
        }
        .inspect_after_completion()
        .expect_err("one entry beyond the limit must fail");
        assert!(matches!(
            entry_error,
            CompilerError::TemporaryArtifactEntryLimit {
                limit: 1,
                observed_at_least: 2,
                ..
            }
        ));

        let byte_limits = CompilerLimits {
            temporary_tree_entries: 2,
            temporary_tree_bytes: 3,
            ..CompilerLimits::default()
        };
        let byte_usage = CompilerUsage::new(byte_limits, CompilerFixture::cancellation());
        let byte_error = CompilerTemporaryTree {
            root: root.path(),
            operation: CompilerOperation::Rustdoc,
            usage: byte_usage.as_ref(),
            next_scan: Instant::now(),
        }
        .inspect_after_completion()
        .expect_err("one byte beyond the limit must fail");
        assert!(matches!(
            byte_error,
            CompilerError::TemporaryArtifactLimit {
                limit: 3,
                observed_at_least: 4,
                ..
            }
        ));
        root.close().expect("close temporary target tree");
    }

    #[test]
    fn missing_quiescent_tree_returns_a_typed_walk_error() {
        let root = assert_fs::TempDir::new().expect("temporary target parent");
        let missing = root.path().join("missing");
        let limits = CompilerLimits::default();
        let usage = CompilerUsage::new(limits, CompilerFixture::cancellation());
        let error = CompilerTemporaryTree {
            root: &missing,
            operation: CompilerOperation::Rustdoc,
            usage: usage.as_ref(),
            next_scan: Instant::now(),
        }
        .inspect_after_completion()
        .expect_err("a missing stable target root must fail");

        assert!(matches!(error, CompilerError::TemporaryArtifactWalk { .. }));
        root.close().expect("close temporary target parent");
    }

    #[test]
    fn compiler_usage_enforces_cumulative_output_and_artifact_budgets() {
        let limits = CompilerLimits {
            stdout_bytes: 5,
            stderr_bytes: 5,
            artifact_bytes: 5,
            cleanup_evidence_bytes: 32,
            metadata_packages: 5,
            metadata_targets: 5,
            rustdoc_items: 5,
            source_mappings: 5,
            temporary_tree_entries: 32,
            temporary_tree_bytes: 32,
            timeout: Duration::from_secs(1),
            cleanup_timeout: Duration::from_millis(10),
            poll_interval: Duration::from_millis(1),
            temporary_tree_scan_interval: Duration::from_millis(10),
        };
        let usage = CompilerUsage::new(limits, CompilerFixture::cancellation());

        usage
            .account_output(CompilerOperation::Metadata, CompilerStream::Stdout, 3)
            .expect("first child output remains within the extraction budget");
        let output = usage
            .account_output(CompilerOperation::Rustdoc, CompilerStream::Stdout, 3)
            .expect_err("the next child shares the same stdout budget");
        assert!(matches!(
            output,
            CompilerError::ProcessOutputLimit {
                operation: CompilerOperation::Rustdoc,
                stream: CompilerStream::Stdout,
                limit: 5,
                observed_at_least: 6,
            }
        ));

        usage
            .account_artifact(
                CompilerOperation::RustdocConfigurationProbe,
                Path::new("probe.json"),
                3,
            )
            .expect("probe capture remains within the extraction artifact budget");
        let artifact = usage
            .account_artifact(CompilerOperation::Rustdoc, Path::new("crate.json"), 3)
            .expect_err("rustdoc JSON shares the probe artifact budget");
        assert!(matches!(
            artifact,
            CompilerError::CompilerArtifactLimit {
                operation: CompilerOperation::Rustdoc,
                limit: 5,
                observed_at_least: 6,
                ..
            }
        ));
    }

    #[test]
    fn compiler_usage_has_one_absolute_deadline_and_shared_cancellation_flag() {
        let limits = CompilerLimits {
            timeout: Duration::ZERO,
            ..CompilerLimits::default()
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        let usage = CompilerUsage::new(
            limits,
            CompilerCancellation::from_flag(Arc::clone(&cancelled)),
        );
        let cloned = usage.clone();

        assert_eq!(usage.deadline, cloned.deadline);
        assert!(matches!(
            usage.checkpoint(CompilerOperation::Metadata),
            Err(CompilerError::ProcessTimeout {
                operation: CompilerOperation::Metadata,
                ..
            })
        ));

        cancelled.store(true, Ordering::Release);
        assert!(matches!(
            cloned.checkpoint(CompilerOperation::Rustdoc),
            Err(CompilerError::CompilerExtractionCancelled)
        ));
    }

    #[test]
    fn compiler_usage_enforces_cumulative_metadata_and_rustdoc_counts() {
        let limits = CompilerLimits {
            metadata_packages: 1,
            metadata_targets: 2,
            rustdoc_items: 2,
            source_mappings: 1,
            ..CompilerLimits::default()
        };
        let usage = CompilerUsage::new(limits, CompilerFixture::cancellation());

        usage
            .account_metadata(&CompilerFixture::metadata())
            .expect("one package and its two targets remain within budget");
        assert!(matches!(
            usage.account_metadata(&CompilerFixture::metadata()),
            Err(CompilerError::CompilerSemanticLimit {
                resource: CompilerSemanticResource::MetadataPackages,
                limit: 1,
                observed_at_least: 2,
                ..
            })
        ));

        usage
            .account_semantic(
                CompilerOperation::Rustdoc,
                CompilerSemanticResource::RustdocItems,
                2,
            )
            .expect("two rustdoc items remain within budget");
        assert!(matches!(
            usage.account_semantic(
                CompilerOperation::Rustdoc,
                CompilerSemanticResource::RustdocItems,
                1,
            ),
            Err(CompilerError::CompilerSemanticLimit {
                resource: CompilerSemanticResource::RustdocItems,
                limit: 2,
                observed_at_least: 3,
                ..
            })
        ));

        usage
            .account_semantic(
                CompilerOperation::Rustdoc,
                CompilerSemanticResource::SourceMappings,
                1,
            )
            .expect("first source mapping remains within budget");
        assert!(matches!(
            usage.account_semantic(
                CompilerOperation::Rustdoc,
                CompilerSemanticResource::SourceMappings,
                1,
            ),
            Err(CompilerError::CompilerSemanticLimit {
                resource: CompilerSemanticResource::SourceMappings,
                limit: 1,
                observed_at_least: 2,
                ..
            })
        ));
    }
}
