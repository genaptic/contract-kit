use super::{LimitExceeded, LimitResource};
use crate::error::SketchContractKitError;
use crate::files::FileCatalog;
use crate::work::CancellationProbe;
use serde::{Deserialize, Serialize};

/// Entry and byte budgets shared by every input catalog in one operation.
///
/// Check charges the source and contract catalogs to one meter; diff charges
/// current and previous contract catalogs to one meter; generation charges its
/// complete contract catalog. Entry and aggregate-byte counters therefore do
/// not restart at request-field boundaries. Per-file bytes are checked for each
/// entry before its bytes contribute to the aggregate counter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CatalogLimits {
    /// Maximum number of entries accumulated across all request catalogs.
    pub entry_count: u64,
    /// Maximum aggregate bytes accumulated across all request catalogs.
    pub total_bytes: u64,
    /// Maximum bytes in one catalog entry.
    pub per_file_bytes: u64,
}

impl Default for CatalogLimits {
    fn default() -> Self {
        Self {
            entry_count: 10_000,
            total_bytes: 512 * 1024 * 1024,
            per_file_bytes: 64 * 1024 * 1024,
        }
    }
}

impl CatalogLimits {
    pub(super) fn usage(&self) -> CatalogUsage<'_> {
        CatalogUsage {
            limits: self,
            entries: 0,
            total_bytes: 0,
        }
    }
}

pub(crate) struct CatalogUsage<'limits> {
    limits: &'limits CatalogLimits,
    entries: u64,
    total_bytes: u64,
}

impl CatalogUsage<'_> {
    pub(crate) fn record(
        &mut self,
        catalog: &FileCatalog,
        cancellation: &CancellationProbe,
    ) -> Result<(), SketchContractKitError> {
        cancellation.checkpoint()?;
        let next_entries = self
            .entries
            .saturating_add(CatalogLimits::observed(catalog.len()));
        if next_entries > self.limits.entry_count {
            return Err(LimitExceeded::new(
                LimitResource::CatalogEntryCount,
                self.limits.entry_count,
                next_entries,
                None,
            )
            .into());
        }

        for (path, bytes) in catalog.iter() {
            cancellation.checkpoint()?;
            let file_bytes = CatalogLimits::observed(bytes.len());
            if file_bytes > self.limits.per_file_bytes {
                return Err(LimitExceeded::new(
                    LimitResource::CatalogFileBytes,
                    self.limits.per_file_bytes,
                    file_bytes,
                    Some(path.clone()),
                )
                .into());
            }
            let next_total_bytes = self.total_bytes.saturating_add(file_bytes);
            if next_total_bytes > self.limits.total_bytes {
                return Err(LimitExceeded::new(
                    LimitResource::CatalogTotalBytes,
                    self.limits.total_bytes,
                    next_total_bytes,
                    Some(path.clone()),
                )
                .into());
            }
            self.total_bytes = next_total_bytes;
        }

        self.entries = next_entries;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::CatalogLimits;
    use crate::files::{CatalogPath, FileCatalog};
    use crate::limits::LimitResource;
    use crate::work::CancellationProbe;

    struct LimitFixture {
        catalog: FileCatalog,
    }

    impl LimitFixture {
        fn with(entries: &[(&str, usize)]) -> Self {
            let mut catalog = FileCatalog::new();
            for (path, bytes) in entries {
                catalog
                    .insert(
                        CatalogPath::new(*path).expect("catalog path"),
                        vec![0; *bytes],
                    )
                    .expect("catalog insert");
            }
            Self { catalog }
        }

        fn error(&self, limits: &CatalogLimits) -> super::LimitExceeded {
            let error = limits
                .usage()
                .record(&self.catalog, &CancellationProbe::new())
                .expect_err("fixture must exceed a limit");
            error.limit_exceeded().expect("typed catalog limit").clone()
        }
    }

    #[test]
    fn catalog_limits_have_deterministic_precedence_and_file_context() {
        let entries = LimitFixture::with(&[("z.rs", 8), ("a.rs", 7)]);
        let entry_error = entries.error(&CatalogLimits {
            entry_count: 1,
            total_bytes: 1,
            per_file_bytes: 1,
        });
        assert_eq!(entry_error.resource, LimitResource::CatalogEntryCount);
        assert_eq!(entry_error.file, None);

        let file_error = entries.error(&CatalogLimits {
            entry_count: 2,
            total_bytes: 20,
            per_file_bytes: 6,
        });
        assert_eq!(file_error.resource, LimitResource::CatalogFileBytes);
        assert_eq!(file_error.file.expect("file").as_str(), "a.rs");

        let total_error = entries.error(&CatalogLimits {
            entry_count: 2,
            total_bytes: 10,
            per_file_bytes: 8,
        });
        assert_eq!(total_error.resource, LimitResource::CatalogTotalBytes);
        assert_eq!(total_error.observed_at_least, 15);
    }

    #[test]
    fn catalog_usage_accumulates_entries_and_bytes_across_request_catalogs() {
        let first = LimitFixture::with(&[("first.rs", 3)]);
        let second = LimitFixture::with(&[("second.yml", 4)]);

        let entry_limits = CatalogLimits {
            entry_count: 1,
            total_bytes: 16,
            per_file_bytes: 8,
        };
        let mut entries = entry_limits.usage();
        let cancellation = CancellationProbe::new();
        entries
            .record(&first.catalog, &cancellation)
            .expect("first catalog remains within the operation budget");
        let entry_error = entries
            .record(&second.catalog, &cancellation)
            .expect_err("the second catalog must cross the operation entry budget");
        let entry_error = entry_error.limit_exceeded().expect("typed entry limit");
        assert_eq!(entry_error.resource, LimitResource::CatalogEntryCount);
        assert_eq!(entry_error.observed_at_least, 2);

        let byte_limits = CatalogLimits {
            entry_count: 2,
            total_bytes: 6,
            per_file_bytes: 4,
        };
        let mut bytes = byte_limits.usage();
        bytes
            .record(&first.catalog, &cancellation)
            .expect("first catalog remains within the operation byte budget");
        let byte_error = bytes
            .record(&second.catalog, &cancellation)
            .expect_err("the second catalog must cross the operation byte budget");
        let byte_error = byte_error.limit_exceeded().expect("typed byte limit");
        assert_eq!(byte_error.resource, LimitResource::CatalogTotalBytes);
        assert_eq!(byte_error.observed_at_least, 7);
        assert_eq!(
            byte_error.file.as_ref(),
            Some(&CatalogPath::new("second.yml").expect("path"))
        );
    }

    #[test]
    fn cancelled_catalog_accounting_stops_before_scanning_entries() {
        let fixture = LimitFixture::with(&[("a.rs", 8), ("b.rs", 8)]);
        let cancellation = CancellationProbe::new();
        cancellation.cancel();

        let error = CatalogLimits::default()
            .usage()
            .record(&fixture.catalog, &cancellation)
            .expect_err("cancelled catalog accounting must stop");

        assert!(error.is_operation_cancelled());
        assert!(error.limit_exceeded().is_none());
    }
}
