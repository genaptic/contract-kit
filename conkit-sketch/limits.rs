mod catalog;
mod matching;
mod output;
mod yaml;

pub use catalog::CatalogLimits;
pub use matching::MatchingLimits;
pub use output::{DiagnosticLimits, OutputLimits};
pub use yaml::YamlLimits;

pub(crate) use catalog::CatalogUsage;
pub(crate) use matching::MatchingUsage;
pub(crate) use output::{DiagnosticBytes, DiagnosticReservation, GeneratedBytes, ScratchText};
pub(crate) use yaml::{RawYamlReport, YamlBudget};

use crate::files::CatalogPath;
use serde::{Deserialize, Serialize};
use std::fmt;

impl CatalogLimits {
    pub(crate) fn observed(value: usize) -> u64 {
        u64::try_from(value).unwrap_or(u64::MAX)
    }

    fn parser_limit(value: u64) -> usize {
        usize::try_from(value).unwrap_or(usize::MAX)
    }
}

#[derive(Clone, Copy)]
struct LimitCharge {
    resource: LimitResource,
    limit: u64,
    accumulated: u64,
}

impl LimitCharge {
    const fn new(resource: LimitResource, limit: u64, accumulated: u64) -> Self {
        Self {
            resource,
            limit,
            accumulated,
        }
    }

    fn charge(self, encountered: usize, file: &CatalogPath) -> Result<u64, LimitExceeded> {
        let observed = self
            .accumulated
            .saturating_add(CatalogLimits::observed(encountered));
        if observed > self.limit {
            return Err(LimitExceeded::new(
                self.resource,
                self.limit,
                observed,
                Some(file.clone()),
            ));
        }
        Ok(observed)
    }

    fn breach(
        self,
        encountered: usize,
        already_recorded: usize,
        file: &CatalogPath,
    ) -> LimitExceeded {
        let additional = encountered.saturating_sub(already_recorded);
        LimitExceeded::new(
            self.resource,
            self.limit,
            self.accumulated
                .saturating_add(CatalogLimits::observed(additional)),
            Some(file.clone()),
        )
    }
}

/// Resource ceilings enforced independently by the sketch domain.
///
/// Limits are enforced at input, parsing, normalization, matching, diagnostic,
/// rendering, and generation boundaries. Catalog and YAML meters are owned by
/// one complete operation, so multiple request catalogs, physical YAML files,
/// diff sides, and generation verification reparses cannot reset their
/// respective budgets. Correctness budgets fail with typed [`LimitExceeded`]
/// evidence; only the explicitly bounded occurrence-span and excerpt fields
/// truncate presentation evidence.
///
/// # Examples
///
/// A zero byte budget stops the operation and preserves the first proven lower
/// bound plus the participating logical file.
///
/// ```
/// use conkit_sketch::{
///     CatalogPath, CheckMode, CheckRequest, FileCatalog, LimitResource,
///     ReportRequest, SketchContractKit, SketchLimits,
/// };
///
/// let source_path = CatalogPath::new("lib.rs")?;
/// let mut source_files = FileCatalog::new();
/// source_files.insert(source_path.clone(), b"x".to_vec())?;
/// let mut limits = SketchLimits::default();
/// limits.catalog.total_bytes = 0;
/// let kit = SketchContractKit::builder().with_limits(limits).build()?;
///
/// let error = futures_executor::block_on(kit.check(CheckRequest {
///     source_files,
///     contract_files: FileCatalog::new(),
///     report: ReportRequest::None,
///     mode: CheckMode::Enforce,
/// }))
/// .expect_err("one input byte must exceed a zero-byte budget");
/// let evidence = error.limit_exceeded().expect("typed limit evidence");
///
/// assert_eq!(evidence.resource, LimitResource::CatalogTotalBytes);
/// assert_eq!(evidence.limit, 0);
/// assert_eq!(evidence.observed_at_least, 1);
/// assert_eq!(evidence.file.as_ref(), Some(&source_path));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SketchLimits {
    /// In-memory catalog budgets.
    pub catalog: CatalogLimits,
    /// YAML parser and semantic-tree budgets.
    pub yaml: YamlLimits,
    /// Sketch identity, normalization, and evidence-retention budgets.
    pub matching: MatchingLimits,
    /// Correctness-diagnostic and excerpt budgets.
    pub diagnostics: DiagnosticLimits,
    /// Returned report and generated-contract byte budgets.
    pub output: OutputLimits,
}

impl SketchLimits {
    pub(crate) fn catalog_usage(&self) -> CatalogUsage<'_> {
        self.catalog.usage()
    }

    pub(crate) fn yaml_budget(&self) -> YamlBudget<'_> {
        self.yaml.budget()
    }
}

/// Resource whose configured budget was exceeded.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum LimitResource {
    /// Catalog entry count.
    CatalogEntryCount,
    /// Aggregate catalog bytes.
    CatalogTotalBytes,
    /// Bytes in one catalog entry.
    CatalogFileBytes,
    /// YAML document count.
    YamlDocumentCount,
    /// YAML nesting depth.
    YamlDepth,
    /// YAML semantic node count.
    YamlNodeCount,
    /// YAML alias count.
    YamlAliasCount,
    /// Materialized YAML scalar and alias-expansion bytes.
    YamlAliasExpansionBytes,
    /// Parsed sketch count.
    SketchCount,
    /// Minimal signature-index entry count.
    SignatureIndexEntryCount,
    /// Contract snippet or generation-seed bytes.
    SnippetBytes,
    /// Contract snippet or generation-seed lines.
    SnippetLines,
    /// Normalized referenced-source bytes.
    NormalizedSourceBytes,
    /// Normalized referenced-source lines.
    NormalizedSourceLines,
    /// Exact normalized-line comparisons across one check operation.
    MatchingLineComparisons,
    /// Exact occurrences encountered across one check operation.
    OccurrenceCandidateCount,
    /// Correctness diagnostic count.
    DiagnosticCount,
    /// Aggregate serialized correctness-diagnostic bytes.
    DiagnosticBytes,
    /// Aggregate generated report or contract bytes.
    GeneratedOutputBytes,
    /// Simultaneously retained generated or edit text bytes.
    OutputScratchBytes,
}

/// Typed evidence for one exceeded resource budget.
///
/// [`Self::observed_at_least`] is the lower bound proven at the point processing
/// stopped, not a promise that the complete input was scanned. It saturates
/// safely when platform-sized counts cannot be represented as [`u64`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LimitExceeded {
    /// Resource that exceeded its configured budget.
    pub resource: LimitResource,
    /// Configured maximum.
    pub limit: u64,
    /// Lower bound observed when processing stopped.
    pub observed_at_least: u64,
    /// Participating logical file, when one identifies the breach.
    pub file: Option<CatalogPath>,
}

impl LimitExceeded {
    pub(crate) fn new(
        resource: LimitResource,
        limit: u64,
        observed_at_least: u64,
        file: Option<CatalogPath>,
    ) -> Self {
        Self {
            resource,
            limit,
            observed_at_least,
            file,
        }
    }
}

impl fmt::Display for LimitExceeded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "resource limit {:?} exceeded: limit {}, observed at least {}",
            self.resource, self.limit, self.observed_at_least
        )?;
        if let Some(file) = &self.file {
            write!(formatter, " in {file}")?;
        }
        Ok(())
    }
}

impl std::error::Error for LimitExceeded {}
