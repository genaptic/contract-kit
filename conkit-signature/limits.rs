use crate::error::SignatureContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::work::CancellationProbe;
use serde::{Deserialize, Serialize};

/// Resource budgets applied to every signature-domain operation.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SignatureLimits {
    /// In-memory catalog budgets.
    pub catalog: CatalogLimits,
    /// YAML parsing budgets.
    pub yaml: YamlLimits,
    /// Rust extraction budgets.
    pub rust: RustExtractionLimits,
    /// Diagnostic accumulation and excerpt budgets.
    pub diagnostics: DiagnosticLimits,
    /// Generated output budgets.
    pub output: OutputLimits,
}

/// Entry and byte budgets shared by every input catalog in one operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CatalogLimits {
    /// Maximum number of entries.
    pub entry_count: u64,
    /// Maximum aggregate bytes across entries.
    pub total_bytes: u64,
    /// Maximum bytes in one entry.
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
    pub(crate) fn usage(&self) -> CatalogUsage<'_> {
        CatalogUsage {
            limits: self,
            entry_count: 0,
            total_bytes: 0,
        }
    }

    fn observed(value: usize) -> u64 {
        u64::try_from(value).unwrap_or(u64::MAX)
    }
}

/// Incremental catalog accounting for one complete public operation.
pub(crate) struct CatalogUsage<'limits> {
    limits: &'limits CatalogLimits,
    entry_count: u64,
    total_bytes: u64,
}

impl CatalogUsage<'_> {
    pub(crate) fn record(
        &mut self,
        catalog: &FileCatalog,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        cancellation.checkpoint()?;
        let encountered = CatalogLimits::observed(catalog.len());
        let next_entry_count = self.entry_count.saturating_add(encountered);
        if next_entry_count > self.limits.entry_count {
            return Err(LimitExceeded::new(
                LimitResource::CatalogEntryCount,
                self.limits.entry_count,
                next_entry_count,
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
            let next_total = self.total_bytes.saturating_add(file_bytes);
            if next_total > self.limits.total_bytes {
                return Err(LimitExceeded::new(
                    LimitResource::CatalogTotalBytes,
                    self.limits.total_bytes,
                    next_total,
                    Some(path.clone()),
                )
                .into());
            }
            self.total_bytes = next_total;
        }
        self.entry_count = next_entry_count;
        Ok(())
    }
}

/// YAML stream and semantic-tree budgets.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct YamlLimits {
    /// Maximum documents parsed or verified across one complete operation.
    pub documents: u64,
    /// Maximum YAML nesting depth in one physical stream.
    pub depth: u64,
    /// Maximum semantic nodes materialized across one complete operation.
    pub nodes: u64,
    /// Maximum aliases encountered across one complete operation.
    pub aliases: u64,
    /// Maximum scalar bytes materialized across one operation, including alias replay.
    pub alias_expansion_bytes: u64,
}

impl Default for YamlLimits {
    fn default() -> Self {
        Self {
            documents: 1_024,
            depth: 128,
            nodes: 1_000_000,
            aliases: 10_000,
            alias_expansion_bytes: 64 * 1024 * 1024,
        }
    }
}

impl YamlLimits {
    pub(crate) fn usage(&self) -> YamlUsage<'_> {
        YamlUsage {
            limits: self,
            documents: 0,
            nodes: 0,
            aliases: 0,
            materialized_scalar_bytes: 0,
        }
    }

    fn parser_limit(value: u64) -> usize {
        usize::try_from(value).unwrap_or(usize::MAX)
    }

    fn semantic_parser_limit(raw: usize, remaining: u64) -> usize {
        raw.saturating_add(Self::parser_limit(remaining))
    }
}

/// Incremental YAML accounting for every input and verification parse in one operation.
pub(crate) struct YamlUsage<'limits> {
    limits: &'limits YamlLimits,
    documents: u64,
    nodes: u64,
    aliases: u64,
    materialized_scalar_bytes: u64,
}

impl YamlUsage<'_> {
    pub(crate) fn record_documents(
        &mut self,
        count: usize,
        path: Option<&CatalogPath>,
    ) -> Result<(), LimitExceeded> {
        let encountered = CatalogLimits::observed(count);
        let observed = self.documents.saturating_add(encountered);
        if observed > self.limits.documents {
            return Err(LimitExceeded::new(
                LimitResource::YamlDocumentCount,
                self.limits.documents,
                observed,
                path.cloned(),
            ));
        }
        self.documents = observed;
        Ok(())
    }

    pub(crate) fn validate_source(
        &mut self,
        path: &CatalogPath,
        source: &str,
    ) -> Result<Option<serde_saphyr::budget::BudgetReport>, LimitExceeded> {
        let Some(budget) = self.raw_parser_budget() else {
            return Ok(Some(serde_saphyr::budget::BudgetReport::default()));
        };
        let Ok(report) = serde_saphyr::budget::check_yaml_budget(
            source,
            budget,
            serde_saphyr::budget::EnforcingPolicy::AllContent,
        ) else {
            // This is a resource preflight. The immediately following semantic
            // parse owns syntax diagnostics and preserves their source location.
            return Ok(None);
        };
        if let Some(breach) = &report.breached
            && let Some(error) = self.limit_for_raw_breach(path, breach)
        {
            return Err(error);
        }
        self.record_raw_report(path, &report)?;
        Ok(Some(report))
    }

    fn raw_parser_budget(&self) -> Option<serde_saphyr::Budget> {
        serde_saphyr::budget! {
            max_reader_input_bytes: None,
            max_events: usize::MAX,
            max_aliases: YamlLimits::parser_limit(
                self.limits.aliases.saturating_sub(self.aliases)
            ),
            max_anchors: usize::MAX,
            max_depth: YamlLimits::parser_limit(self.limits.depth),
            max_inclusion_depth: 0,
            max_documents: YamlLimits::parser_limit(
                self.limits.documents.saturating_sub(self.documents)
            ),
            max_nodes: YamlLimits::parser_limit(
                self.limits.nodes.saturating_sub(self.nodes)
            ),
            max_total_scalar_bytes: YamlLimits::parser_limit(
                self.limits
                    .alias_expansion_bytes
                    .saturating_sub(self.materialized_scalar_bytes)
            ),
            max_total_comment_bytes: usize::MAX,
            max_merge_keys: usize::MAX,
            enforce_alias_anchor_ratio: false
        }
    }

    pub(crate) fn semantic_parser_budget(
        &self,
        raw: &serde_saphyr::budget::BudgetReport,
    ) -> Option<serde_saphyr::Budget> {
        serde_saphyr::budget! {
            max_reader_input_bytes: None,
            max_events: usize::MAX,
            max_aliases: YamlLimits::semantic_parser_limit(
                raw.aliases,
                self.limits.aliases.saturating_sub(self.aliases)
            ),
            max_anchors: usize::MAX,
            max_depth: YamlLimits::parser_limit(self.limits.depth),
            max_inclusion_depth: 0,
            max_documents: YamlLimits::semantic_parser_limit(
                raw.documents,
                self.limits.documents.saturating_sub(self.documents)
            ),
            max_nodes: YamlLimits::semantic_parser_limit(
                raw.nodes,
                self.limits.nodes.saturating_sub(self.nodes)
            ),
            max_total_scalar_bytes: YamlLimits::semantic_parser_limit(
                raw.total_scalar_bytes,
                self.limits
                    .alias_expansion_bytes
                    .saturating_sub(self.materialized_scalar_bytes)
            ),
            max_total_comment_bytes: usize::MAX,
            max_merge_keys: usize::MAX,
            enforce_alias_anchor_ratio: false
        }
    }

    pub(crate) fn semantic_alias_limits(
        &self,
        raw: &serde_saphyr::budget::BudgetReport,
    ) -> serde_saphyr::options::AliasLimits {
        serde_saphyr::alias_limits! {
            // A replayed container contributes one semantic node but two
            // balanced start/end events. Two events per remaining node keeps
            // alias replay bounded without rejecting an exact node boundary.
            max_total_replayed_events: YamlLimits::parser_limit(
                self.limits
                    .nodes
                    .saturating_sub(self.nodes)
                    .saturating_mul(2)
            ),
            max_replay_stack_depth: YamlLimits::parser_limit(self.limits.depth),
            max_alias_expansions_per_anchor: YamlLimits::semantic_parser_limit(
                raw.aliases,
                self.limits.aliases.saturating_sub(self.aliases)
            )
        }
    }

    fn record_raw_report(
        &mut self,
        path: &CatalogPath,
        report: &serde_saphyr::budget::BudgetReport,
    ) -> Result<(), LimitExceeded> {
        let observed_depth = CatalogLimits::observed(report.max_depth);
        if observed_depth > self.limits.depth {
            return Err(LimitExceeded::new(
                LimitResource::YamlDepth,
                self.limits.depth,
                observed_depth,
                Some(path.clone()),
            ));
        }

        let documents = self
            .documents
            .saturating_add(CatalogLimits::observed(report.documents));
        if documents > self.limits.documents {
            return Err(LimitExceeded::new(
                LimitResource::YamlDocumentCount,
                self.limits.documents,
                documents,
                Some(path.clone()),
            ));
        }
        let nodes = self
            .nodes
            .saturating_add(CatalogLimits::observed(report.nodes));
        if nodes > self.limits.nodes {
            return Err(LimitExceeded::new(
                LimitResource::YamlNodeCount,
                self.limits.nodes,
                nodes,
                Some(path.clone()),
            ));
        }
        let aliases = self
            .aliases
            .saturating_add(CatalogLimits::observed(report.aliases));
        if aliases > self.limits.aliases {
            return Err(LimitExceeded::new(
                LimitResource::YamlAliasCount,
                self.limits.aliases,
                aliases,
                Some(path.clone()),
            ));
        }
        let materialized_scalar_bytes = self
            .materialized_scalar_bytes
            .saturating_add(CatalogLimits::observed(report.total_scalar_bytes));
        if materialized_scalar_bytes > self.limits.alias_expansion_bytes {
            return Err(LimitExceeded::new(
                LimitResource::YamlAliasExpansionBytes,
                self.limits.alias_expansion_bytes,
                materialized_scalar_bytes,
                Some(path.clone()),
            ));
        }

        self.documents = documents;
        self.nodes = nodes;
        self.aliases = aliases;
        self.materialized_scalar_bytes = materialized_scalar_bytes;
        Ok(())
    }

    pub(crate) fn record_replay_report(
        &mut self,
        path: &CatalogPath,
        raw: &serde_saphyr::budget::BudgetReport,
        semantic: &serde_saphyr::budget::BudgetReport,
    ) -> Result<(), LimitExceeded> {
        let nodes = self.nodes.saturating_add(CatalogLimits::observed(
            semantic.nodes.saturating_sub(raw.nodes),
        ));
        if nodes > self.limits.nodes {
            return Err(LimitExceeded::new(
                LimitResource::YamlNodeCount,
                self.limits.nodes,
                nodes,
                Some(path.clone()),
            ));
        }
        let materialized_scalar_bytes =
            self.materialized_scalar_bytes
                .saturating_add(CatalogLimits::observed(
                    semantic
                        .total_scalar_bytes
                        .saturating_sub(raw.total_scalar_bytes),
                ));
        if materialized_scalar_bytes > self.limits.alias_expansion_bytes {
            return Err(LimitExceeded::new(
                LimitResource::YamlAliasExpansionBytes,
                self.limits.alias_expansion_bytes,
                materialized_scalar_bytes,
                Some(path.clone()),
            ));
        }

        self.nodes = nodes;
        self.materialized_scalar_bytes = materialized_scalar_bytes;
        Ok(())
    }

    pub(crate) fn limit_for_semantic_parser_error(
        &self,
        path: &CatalogPath,
        raw: &serde_saphyr::budget::BudgetReport,
        error: &serde_saphyr::Error,
    ) -> Option<LimitExceeded> {
        match error.without_snippet() {
            serde_saphyr::Error::Budget { breach, .. } => {
                self.limit_for_semantic_breach(path, raw, breach)
            }
            serde_saphyr::Error::AliasReplayLimitExceeded {
                total_replayed_events,
                ..
            } => Some(LimitExceeded::new(
                LimitResource::YamlNodeCount,
                self.limits.nodes,
                self.observed_nodes_after_replay_events(*total_replayed_events),
                Some(path.clone()),
            )),
            serde_saphyr::Error::AliasExpansionLimitExceeded { expansions, .. } => {
                let prior_aliases = self
                    .aliases
                    .saturating_sub(CatalogLimits::observed(raw.aliases));
                Some(LimitExceeded::new(
                    LimitResource::YamlAliasCount,
                    self.limits.aliases,
                    prior_aliases.saturating_add(CatalogLimits::observed(*expansions)),
                    Some(path.clone()),
                ))
            }
            serde_saphyr::Error::AliasReplayStackDepthExceeded { depth, .. } => {
                Some(LimitExceeded::new(
                    LimitResource::YamlDepth,
                    self.limits.depth,
                    CatalogLimits::observed(*depth),
                    Some(path.clone()),
                ))
            }
            _ => None,
        }
    }

    fn observed_nodes_after_replay_events(&self, replayed_events: usize) -> u64 {
        let replayed_nodes = replayed_events.div_ceil(2);
        self.nodes
            .saturating_add(CatalogLimits::observed(replayed_nodes))
    }

    fn limit_for_raw_breach(
        &self,
        path: &CatalogPath,
        breach: &serde_saphyr::budget::BudgetBreach,
    ) -> Option<LimitExceeded> {
        use serde_saphyr::budget::BudgetBreach;

        let (resource, limit, accumulated, encountered) = match breach {
            BudgetBreach::Aliases { aliases } => (
                LimitResource::YamlAliasCount,
                self.limits.aliases,
                self.aliases,
                CatalogLimits::observed(*aliases),
            ),
            BudgetBreach::Depth { depth } => (
                LimitResource::YamlDepth,
                self.limits.depth,
                0,
                CatalogLimits::observed(*depth),
            ),
            BudgetBreach::Documents { documents } => (
                LimitResource::YamlDocumentCount,
                self.limits.documents,
                self.documents,
                CatalogLimits::observed(*documents),
            ),
            BudgetBreach::Nodes { nodes } => (
                LimitResource::YamlNodeCount,
                self.limits.nodes,
                self.nodes,
                CatalogLimits::observed(*nodes),
            ),
            BudgetBreach::ScalarBytes { total_scalar_bytes } => (
                LimitResource::YamlAliasExpansionBytes,
                self.limits.alias_expansion_bytes,
                self.materialized_scalar_bytes,
                CatalogLimits::observed(*total_scalar_bytes),
            ),
            _ => return None,
        };
        Some(LimitExceeded::new(
            resource,
            limit,
            accumulated.saturating_add(encountered),
            Some(path.clone()),
        ))
    }

    fn limit_for_semantic_breach(
        &self,
        path: &CatalogPath,
        raw: &serde_saphyr::budget::BudgetReport,
        breach: &serde_saphyr::budget::BudgetBreach,
    ) -> Option<LimitExceeded> {
        use serde_saphyr::budget::BudgetBreach;

        let (resource, limit, accumulated, encountered, already_recorded) = match breach {
            BudgetBreach::Aliases { aliases } => (
                LimitResource::YamlAliasCount,
                self.limits.aliases,
                self.aliases,
                *aliases,
                raw.aliases,
            ),
            BudgetBreach::Depth { depth } => {
                (LimitResource::YamlDepth, self.limits.depth, 0, *depth, 0)
            }
            BudgetBreach::Documents { documents } => (
                LimitResource::YamlDocumentCount,
                self.limits.documents,
                self.documents,
                *documents,
                raw.documents,
            ),
            BudgetBreach::Nodes { nodes } => (
                LimitResource::YamlNodeCount,
                self.limits.nodes,
                self.nodes,
                *nodes,
                raw.nodes,
            ),
            BudgetBreach::ScalarBytes { total_scalar_bytes } => (
                LimitResource::YamlAliasExpansionBytes,
                self.limits.alias_expansion_bytes,
                self.materialized_scalar_bytes,
                *total_scalar_bytes,
                raw.total_scalar_bytes,
            ),
            _ => return None,
        };
        Some(LimitExceeded::new(
            resource,
            limit,
            accumulated.saturating_add(CatalogLimits::observed(
                encountered.saturating_sub(already_recorded),
            )),
            Some(path.clone()),
        ))
    }
}

/// Rust source and extracted-item budgets.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RustExtractionLimits {
    /// Maximum participating Rust source files.
    pub source_files: u64,
    /// Maximum bytes in one participating Rust source file.
    pub per_file_bytes: u64,
    /// Maximum extracted Rust items.
    pub items: u64,
    /// Maximum grouped signatures.
    pub signatures: u64,
    /// Maximum bytes in one compiler-produced rustdoc JSON artifact.
    pub compiler_artifact_bytes: u64,
    /// Maximum rustdoc index, path-summary, and source-map nodes combined.
    pub compiler_nodes: u64,
}

impl Default for RustExtractionLimits {
    fn default() -> Self {
        Self {
            source_files: 10_000,
            per_file_bytes: 16 * 1024 * 1024,
            items: 1_000_000,
            signatures: 500_000,
            compiler_artifact_bytes: 256 * 1024 * 1024,
            compiler_nodes: 2_000_000,
        }
    }
}

impl RustExtractionLimits {
    pub(crate) fn validate_source_count(&self, count: usize) -> Result<(), LimitExceeded> {
        let observed = CatalogLimits::observed(count);
        if observed > self.source_files {
            return Err(LimitExceeded::new(
                LimitResource::RustSourceFileCount,
                self.source_files,
                observed,
                None,
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_source_file(
        &self,
        path: &CatalogPath,
        bytes: usize,
    ) -> Result<(), LimitExceeded> {
        let observed = CatalogLimits::observed(bytes);
        if observed > self.per_file_bytes {
            return Err(LimitExceeded::new(
                LimitResource::RustSourceFileBytes,
                self.per_file_bytes,
                observed,
                Some(path.clone()),
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_compiler_artifact_bytes(
        &self,
        bytes: usize,
    ) -> Result<(), LimitExceeded> {
        let observed = CatalogLimits::observed(bytes);
        if observed > self.compiler_artifact_bytes {
            return Err(LimitExceeded::new(
                LimitResource::RustCompilerArtifactBytes,
                self.compiler_artifact_bytes,
                observed,
                None,
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_compiler_nodes(&self, nodes: usize) -> Result<(), LimitExceeded> {
        let observed = CatalogLimits::observed(nodes);
        if observed > self.compiler_nodes {
            return Err(LimitExceeded::new(
                LimitResource::RustCompilerNodeCount,
                self.compiler_nodes,
                observed,
                None,
            ));
        }
        Ok(())
    }

    pub(crate) fn usage(&self) -> RustExtractionUsage<'_> {
        RustExtractionUsage {
            limits: self,
            items: 0,
            signatures: 0,
        }
    }
}

/// Incremental Rust extraction accounting for one complete public operation.
pub(crate) struct RustExtractionUsage<'limits> {
    limits: &'limits RustExtractionLimits,
    items: u64,
    signatures: u64,
}

impl RustExtractionUsage<'_> {
    pub(crate) fn record_item(&mut self, file: Option<&CatalogPath>) -> Result<(), LimitExceeded> {
        self.record_items(1, file)
    }

    pub(crate) fn record_items(
        &mut self,
        count: usize,
        file: Option<&CatalogPath>,
    ) -> Result<(), LimitExceeded> {
        let encountered = CatalogLimits::observed(count);
        let remaining = self.limits.items.saturating_sub(self.items);
        if encountered > remaining {
            self.items = self.limits.items.saturating_add(1);
            return Err(LimitExceeded::new(
                LimitResource::RustItemCount,
                self.limits.items,
                self.items,
                file.cloned(),
            ));
        }
        self.items = self.items.saturating_add(encountered);
        Ok(())
    }

    pub(crate) fn record_signatures(&mut self, count: usize) -> Result<(), LimitExceeded> {
        self.signatures = self
            .signatures
            .saturating_add(CatalogLimits::observed(count));
        if self.signatures > self.limits.signatures {
            return Err(LimitExceeded::new(
                LimitResource::SignatureCount,
                self.limits.signatures,
                self.signatures,
                None,
            ));
        }
        Ok(())
    }
}

/// Diagnostic collection and evidence budgets.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticLimits {
    /// Maximum correctness diagnostics.
    pub count: u64,
    /// Maximum serialized diagnostic bytes.
    pub serialized_bytes: u64,
}

impl Default for DiagnosticLimits {
    fn default() -> Self {
        Self {
            count: 10_000,
            serialized_bytes: 16 * 1024 * 1024,
        }
    }
}

impl DiagnosticLimits {
    pub(crate) fn validate_count(&self, count: usize) -> Result<(), LimitExceeded> {
        let observed = CatalogLimits::observed(count);
        if observed > self.count {
            return Err(LimitExceeded::new(
                LimitResource::DiagnosticCount,
                self.count,
                observed,
                None,
            ));
        }
        Ok(())
    }

    pub(crate) fn usage(&self) -> Result<DiagnosticUsage<'_>, SignatureContractKitError> {
        DiagnosticUsage::new(self)
    }

    pub(crate) fn evidence_usage(&self) -> DiagnosticEvidenceUsage<'_> {
        DiagnosticEvidenceUsage {
            limits: self,
            count: 0,
            retained_bytes: 2,
        }
    }
}

/// Incremental in-memory accounting before diagnostic evidence enters a response.
pub(crate) struct DiagnosticEvidenceUsage<'limits> {
    limits: &'limits DiagnosticLimits,
    count: u64,
    retained_bytes: u64,
}

impl DiagnosticEvidenceUsage<'_> {
    pub(crate) fn record_text(&mut self, text: &str) -> Result<(), LimitExceeded> {
        let next_count = self.count.saturating_add(1);
        if next_count > self.limits.count {
            return Err(LimitExceeded::new(
                LimitResource::DiagnosticCount,
                self.limits.count,
                next_count,
                None,
            ));
        }
        let separator_bytes = u64::from(self.count > 0);
        let next_bytes = self
            .retained_bytes
            .saturating_add(separator_bytes)
            .saturating_add(Self::serialized_text_bytes(text));
        if next_bytes > self.limits.serialized_bytes {
            return Err(LimitExceeded::new(
                LimitResource::DiagnosticBytes,
                self.limits.serialized_bytes,
                next_bytes,
                None,
            ));
        }
        self.count = next_count;
        self.retained_bytes = next_bytes;
        Ok(())
    }

    fn serialized_text_bytes(text: &str) -> u64 {
        text.as_bytes().iter().fold(2_u64, |bytes, byte| {
            let encoded = match byte {
                b'"' | b'\\' | b'\x08' | b'\x09' | b'\x0a' | b'\x0c' | b'\x0d' => 2,
                0x00..=0x1f => 6,
                _ => 1,
            };
            bytes.saturating_add(encoded)
        })
    }
}

/// Incremental accounting for one serialized diagnostic array.
pub(crate) struct DiagnosticUsage<'limits> {
    limits: &'limits DiagnosticLimits,
    count: u64,
    serialized_bytes: u64,
}

impl<'limits> DiagnosticUsage<'limits> {
    fn new(limits: &'limits DiagnosticLimits) -> Result<Self, SignatureContractKitError> {
        let serialized_bytes = 2;
        if serialized_bytes > limits.serialized_bytes {
            return Err(LimitExceeded::new(
                LimitResource::DiagnosticBytes,
                limits.serialized_bytes,
                limits.serialized_bytes.saturating_add(1),
                None,
            )
            .into());
        }
        Ok(Self {
            limits,
            count: 0,
            serialized_bytes,
        })
    }

    pub(crate) fn record<T>(&mut self, diagnostic: &T) -> Result<(), SignatureContractKitError>
    where
        T: Serialize + ?Sized,
    {
        let next_count = self.count.saturating_add(1);
        if next_count > self.limits.count {
            return Err(LimitExceeded::new(
                LimitResource::DiagnosticCount,
                self.limits.count,
                next_count,
                None,
            )
            .into());
        }

        let mut counter =
            DiagnosticByteCounter::new(self.limits.serialized_bytes, self.serialized_bytes);
        if self.count > 0 {
            std::io::Write::write_all(&mut counter, b",").map_err(|_| {
                LimitExceeded::new(
                    LimitResource::DiagnosticBytes,
                    self.limits.serialized_bytes,
                    counter.observed,
                    None,
                )
            })?;
        }
        let result = serde_json::to_writer(&mut counter, diagnostic);
        if counter.exceeded {
            return Err(LimitExceeded::new(
                LimitResource::DiagnosticBytes,
                self.limits.serialized_bytes,
                counter.observed,
                None,
            )
            .into());
        }
        result.map_err(|source| {
            SignatureContractKitError::conversion_failed(format!(
                "failed to encode diagnostics for resource accounting: {source}"
            ))
        })?;
        self.count = next_count;
        self.serialized_bytes = counter.observed;
        Ok(())
    }
}

struct DiagnosticByteCounter {
    limit: u64,
    observed: u64,
    exceeded: bool,
}

impl DiagnosticByteCounter {
    fn new(limit: u64, observed: u64) -> Self {
        Self {
            limit,
            observed,
            exceeded: false,
        }
    }
}

impl std::io::Write for DiagnosticByteCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let next = self
            .observed
            .saturating_add(CatalogLimits::observed(bytes.len()));
        if next > self.limit {
            self.observed = self.limit.saturating_add(1);
            self.exceeded = true;
            return Err(std::io::Error::other("diagnostic byte budget exceeded"));
        }
        self.observed = next;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Generated-output budgets.
///
/// Returned output and simultaneously retained scratch each default to
/// 512 MiB and are enforced independently.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OutputLimits {
    /// Maximum aggregate bytes returned by one operation.
    pub generated_bytes: u64,
    /// Maximum simultaneously retained generated or edit-text bytes.
    #[serde(default = "OutputLimits::default_scratch_bytes")]
    pub scratch_bytes: u64,
}

impl Default for OutputLimits {
    fn default() -> Self {
        Self {
            generated_bytes: Self::DEFAULT_BYTES,
            scratch_bytes: Self::default_scratch_bytes(),
        }
    }
}

impl OutputLimits {
    const DEFAULT_BYTES: u64 = 512 * 1024 * 1024;

    const fn default_scratch_bytes() -> u64 {
        Self::DEFAULT_BYTES
    }

    pub(crate) fn meter(&self, cancellation: &CancellationProbe) -> GeneratedOutputMeter<'_> {
        GeneratedOutputMeter::new(self, cancellation.clone())
    }
}

/// Incremental aggregate accounting for generated catalog bytes.
pub(crate) struct GeneratedOutputMeter<'limits> {
    limits: &'limits OutputLimits,
    generated_bytes: u64,
    retained_scratch_bytes: std::cell::Cell<u64>,
    cancellation: CancellationProbe,
}

impl<'limits> GeneratedOutputMeter<'limits> {
    pub(crate) fn new(limits: &'limits OutputLimits, cancellation: CancellationProbe) -> Self {
        Self {
            limits,
            generated_bytes: 0,
            retained_scratch_bytes: std::cell::Cell::new(0),
            cancellation,
        }
    }

    pub(crate) fn record(
        &mut self,
        file: &CatalogPath,
        bytes: usize,
    ) -> Result<(), SignatureContractKitError> {
        self.cancellation.checkpoint()?;
        self.generated_bytes = self
            .generated_bytes
            .saturating_add(CatalogLimits::observed(bytes));
        if self.generated_bytes > self.limits.generated_bytes {
            return Err(LimitExceeded::new(
                LimitResource::GeneratedOutputBytes,
                self.limits.generated_bytes,
                self.generated_bytes,
                Some(file.clone()),
            )
            .into());
        }
        Ok(())
    }

    pub(crate) fn serialize_yaml<T>(
        &mut self,
        file: &CatalogPath,
        value: &T,
    ) -> Result<Vec<u8>, SignatureContractKitError>
    where
        T: Serialize,
    {
        let mut buffer = self.returned_buffer(file);
        let result = serde_saphyr::to_io_writer(&mut buffer, value);
        self.commit_returned_buffer(buffer, result)
    }

    pub(crate) fn serialize_yaml_scratch<'meter, T>(
        &'meter self,
        file: &CatalogPath,
        value: &T,
    ) -> Result<ScratchText<'meter, 'limits>, SignatureContractKitError>
    where
        T: Serialize,
    {
        let mut writer = self.scratch_writer(file)?;
        let result = serde_saphyr::to_io_writer(&mut writer, value);
        writer.finish_text(result)
    }

    pub(crate) fn serialize_pretty_json<T>(
        &mut self,
        file: &CatalogPath,
        value: &T,
    ) -> Result<Vec<u8>, SignatureContractKitError>
    where
        T: Serialize + ?Sized,
    {
        let mut buffer = self.returned_buffer(file);
        let result = serde_json::to_writer_pretty(&mut buffer, value);
        self.commit_returned_buffer(buffer, result)
    }

    pub(crate) fn returned_buffer(&self, file: &CatalogPath) -> ReturnedOutputBuffer {
        ReturnedOutputBuffer::new(
            self.limits.generated_bytes,
            self.generated_bytes,
            file.clone(),
            self.cancellation.clone(),
        )
    }

    pub(crate) fn commit_returned_buffer<E>(
        &mut self,
        buffer: ReturnedOutputBuffer,
        result: Result<(), E>,
    ) -> Result<Vec<u8>, SignatureContractKitError>
    where
        E: std::fmt::Display,
    {
        self.cancellation.checkpoint()?;
        if buffer.starting_bytes != self.generated_bytes {
            return Err(SignatureContractKitError::conversion_failed(
                "stale generated output buffer cannot be committed after another output",
            ));
        }
        let file = buffer.file.clone();
        let bytes = buffer.finish(result)?;
        self.record(&file, bytes.len())?;
        Ok(bytes)
    }

    pub(crate) fn scratch_writer<'meter>(
        &'meter self,
        file: &CatalogPath,
    ) -> Result<ScratchWriter<'meter, 'limits>, SignatureContractKitError> {
        self.cancellation.checkpoint()?;
        Ok(ScratchWriter::new(self, file.clone()))
    }

    fn release_scratch(&self, released: u64) {
        let current = self.retained_scratch_bytes.get();
        debug_assert!(released <= current);
        self.retained_scratch_bytes
            .set(current.saturating_sub(released));
    }
}

/// A file-local writer that refuses the first byte beyond the aggregate output budget.
pub(crate) struct ReturnedOutputBuffer {
    limit: u64,
    starting_bytes: u64,
    file: CatalogPath,
    bytes: Vec<u8>,
    observed_at_least: Option<u64>,
    cancellation: CancellationProbe,
    canceled: bool,
}

impl ReturnedOutputBuffer {
    fn new(
        limit: u64,
        starting_bytes: u64,
        file: CatalogPath,
        cancellation: CancellationProbe,
    ) -> Self {
        Self {
            limit,
            starting_bytes,
            file,
            bytes: Vec::new(),
            observed_at_least: None,
            cancellation,
            canceled: false,
        }
    }

    fn finish<E>(self, result: Result<(), E>) -> Result<Vec<u8>, SignatureContractKitError>
    where
        E: std::fmt::Display,
    {
        self.cancellation.checkpoint()?;
        if self.canceled {
            return Err(SignatureContractKitError::operation_canceled());
        }
        if let Some(observed_at_least) = self.observed_at_least {
            return Err(LimitExceeded::new(
                LimitResource::GeneratedOutputBytes,
                self.limit,
                observed_at_least,
                Some(self.file),
            )
            .into());
        }
        result.map_err(|source| {
            SignatureContractKitError::write_failed(&self.file, source.to_string())
        })?;
        Ok(self.bytes)
    }

    pub(crate) fn checkpoint_format(&mut self) -> std::fmt::Result {
        if self.cancellation.checkpoint().is_err() {
            self.canceled = true;
            Err(std::fmt::Error)
        } else {
            Ok(())
        }
    }
}

impl std::io::Write for ReturnedOutputBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let observed = self
            .starting_bytes
            .saturating_add(CatalogLimits::observed(self.bytes.len()))
            .saturating_add(CatalogLimits::observed(bytes.len()));
        if observed > self.limit {
            self.observed_at_least = Some(self.limit.saturating_add(1));
            return Err(std::io::Error::other(
                "generated output byte budget exceeded",
            ));
        }

        const WRITE_CHUNK_BYTES: usize = 64 * 1024;
        for chunk in bytes.chunks(WRITE_CHUNK_BYTES) {
            if self.cancellation.checkpoint().is_err() {
                self.canceled = true;
                return Err(std::io::Error::other("generated output canceled"));
            }
            self.bytes.extend_from_slice(chunk);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl std::fmt::Write for ReturnedOutputBuffer {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        std::io::Write::write_all(self, value.as_bytes()).map_err(|_| std::fmt::Error)
    }
}

enum ScratchFailure {
    Cancelled,
    Limit { observed_at_least: u64 },
    Allocation(String),
}

pub(crate) struct ScratchWriter<'meter, 'limits> {
    meter: &'meter GeneratedOutputMeter<'limits>,
    file: CatalogPath,
    bytes: Vec<u8>,
    reserved: u64,
    failure: Option<ScratchFailure>,
}

impl<'meter, 'limits> ScratchWriter<'meter, 'limits> {
    fn new(meter: &'meter GeneratedOutputMeter<'limits>, file: CatalogPath) -> Self {
        Self {
            meter,
            file,
            bytes: Vec::new(),
            reserved: 0,
            failure: None,
        }
    }

    pub(crate) fn finish_text<E>(
        mut self,
        result: Result<(), E>,
    ) -> Result<ScratchText<'meter, 'limits>, SignatureContractKitError>
    where
        E: std::fmt::Display,
    {
        self.meter.cancellation.checkpoint()?;
        if let Some(failure) = self.failure.take() {
            return Err(match failure {
                ScratchFailure::Cancelled => SignatureContractKitError::operation_canceled(),
                ScratchFailure::Limit { observed_at_least } => LimitExceeded::new(
                    LimitResource::OutputScratchBytes,
                    self.meter.limits.scratch_bytes,
                    observed_at_least,
                    Some(self.file.clone()),
                )
                .into(),
                ScratchFailure::Allocation(source) => {
                    SignatureContractKitError::write_failed(&self.file, source)
                }
            });
        }
        result.map_err(|source| {
            SignatureContractKitError::write_failed(&self.file, source.to_string())
        })?;
        let text = String::from_utf8(std::mem::take(&mut self.bytes)).map_err(|source| {
            SignatureContractKitError::write_failed(&self.file, source.to_string())
        })?;
        let reserved = std::mem::take(&mut self.reserved);
        Ok(ScratchText {
            meter: self.meter,
            text,
            reserved,
        })
    }

    fn fail(&mut self, failure: ScratchFailure, message: &'static str) -> std::io::Error {
        if self.failure.is_none() {
            self.failure = Some(failure);
        }
        std::io::Error::other(message)
    }
}

impl std::io::Write for ScratchWriter<'_, '_> {
    fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
        if self.failure.is_some() {
            return Err(std::io::Error::other("scratch writer has failed"));
        }
        if self.meter.cancellation.checkpoint().is_err() {
            return Err(self.fail(ScratchFailure::Cancelled, "scratch generation canceled"));
        }
        if input.is_empty() {
            return Ok(0);
        }

        const WRITE_CHUNK_BYTES: usize = 64 * 1024;
        let length = input.len().min(WRITE_CHUNK_BYTES);
        let additional = CatalogLimits::observed(length);
        let next = self
            .meter
            .retained_scratch_bytes
            .get()
            .saturating_add(additional);
        if next > self.meter.limits.scratch_bytes {
            return Err(self.fail(
                ScratchFailure::Limit {
                    observed_at_least: self.meter.limits.scratch_bytes.saturating_add(1),
                },
                "output scratch byte budget exceeded",
            ));
        }
        if let Err(source) = self.bytes.try_reserve(length) {
            return Err(self.fail(
                ScratchFailure::Allocation(source.to_string()),
                "output scratch allocation failed",
            ));
        }
        self.bytes.extend_from_slice(&input[..length]);
        self.meter.retained_scratch_bytes.set(next);
        self.reserved = self.reserved.saturating_add(additional);
        Ok(length)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.failure.is_some() {
            return Err(std::io::Error::other("scratch writer has failed"));
        }
        if self.meter.cancellation.checkpoint().is_err() {
            return Err(self.fail(ScratchFailure::Cancelled, "scratch generation canceled"));
        }
        Ok(())
    }
}

impl std::fmt::Write for ScratchWriter<'_, '_> {
    fn write_str(&mut self, value: &str) -> std::fmt::Result {
        std::io::Write::write_all(self, value.as_bytes()).map_err(|_| std::fmt::Error)
    }
}

impl Drop for ScratchWriter<'_, '_> {
    fn drop(&mut self) {
        self.meter.release_scratch(self.reserved);
    }
}

pub(crate) struct ScratchText<'meter, 'limits> {
    meter: &'meter GeneratedOutputMeter<'limits>,
    text: String,
    reserved: u64,
}

impl ScratchText<'_, '_> {
    pub(crate) fn as_str(&self) -> &str {
        &self.text
    }
}

impl Drop for ScratchText<'_, '_> {
    fn drop(&mut self) {
        self.meter.release_scratch(self.reserved);
    }
}

/// Resource whose configured budget was exceeded.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum LimitResource {
    /// Catalog entry count.
    CatalogEntryCount,
    /// Aggregate catalog bytes.
    CatalogTotalBytes,
    /// Bytes in one catalog file.
    CatalogFileBytes,
    /// YAML document count.
    YamlDocumentCount,
    /// YAML nesting depth.
    YamlDepth,
    /// YAML semantic node count.
    YamlNodeCount,
    /// YAML alias count.
    YamlAliasCount,
    /// YAML alias expansion bytes.
    YamlAliasExpansionBytes,
    /// Participating Rust source-file count.
    RustSourceFileCount,
    /// Bytes in one participating Rust source file.
    RustSourceFileBytes,
    /// Bytes in one compiler-produced rustdoc JSON artifact.
    RustCompilerArtifactBytes,
    /// Rustdoc index, path-summary, and source-map node count.
    RustCompilerNodeCount,
    /// Extracted Rust item count.
    RustItemCount,
    /// Grouped signature count.
    SignatureCount,
    /// Correctness diagnostic count.
    DiagnosticCount,
    /// Serialized diagnostic bytes.
    DiagnosticBytes,
    /// Generated output bytes.
    GeneratedOutputBytes,
    /// Simultaneously retained generated or edit-text bytes.
    OutputScratchBytes,
}

/// Typed evidence for one exceeded resource budget.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LimitExceeded {
    /// Resource that exceeded its budget.
    pub resource: LimitResource,
    /// Configured maximum.
    pub limit: u64,
    /// Lower bound observed when processing stopped.
    pub observed_at_least: u64,
    /// Participating file, when the budget is file-specific.
    pub file: Option<CatalogPath>,
}

impl LimitExceeded {
    fn new(
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

impl std::fmt::Display for LimitExceeded {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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

#[cfg(test)]
mod tests {
    use super::{
        CatalogLimits, DiagnosticLimits, GeneratedOutputMeter, LimitResource, OutputLimits,
        RustExtractionLimits, YamlLimits,
    };
    use crate::files::{CatalogPath, FileCatalog};
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

        fn validate(&self, limits: &CatalogLimits) -> super::LimitExceeded {
            let error = limits
                .usage()
                .record(&self.catalog, &CancellationProbe::new())
                .expect_err("fixture must exceed a limit");
            error
                .limit_exceeded()
                .cloned()
                .expect("typed catalog limit")
        }
    }

    #[test]
    fn catalog_entry_limit_precedes_byte_scans() {
        let error = LimitFixture::with(&[("a.rs", 9), ("b.rs", 9)]).validate(&CatalogLimits {
            entry_count: 1,
            total_bytes: 1,
            per_file_bytes: 1,
        });

        assert_eq!(error.resource, LimitResource::CatalogEntryCount);
        assert_eq!(error.limit, 1);
        assert_eq!(error.observed_at_least, 2);
        assert_eq!(error.file, None);
    }

    #[test]
    fn catalog_per_file_limit_identifies_first_ordered_path() {
        let error = LimitFixture::with(&[("z.rs", 8), ("a.rs", 7)]).validate(&CatalogLimits {
            entry_count: 2,
            total_bytes: 20,
            per_file_bytes: 6,
        });

        assert_eq!(error.resource, LimitResource::CatalogFileBytes);
        assert_eq!(error.limit, 6);
        assert_eq!(error.observed_at_least, 7);
        assert_eq!(error.file.expect("file").as_str(), "a.rs");
    }

    #[test]
    fn yaml_depth_budget_reports_the_typed_resource_and_contract_path() {
        let path = CatalogPath::new("main.yml").expect("catalog path");
        let limits = YamlLimits {
            depth: 2,
            ..YamlLimits::default()
        };
        let error = limits
            .usage()
            .validate_source(&path, "value: [[[0]]]\n")
            .expect_err("nested YAML must exceed the configured depth");

        assert_eq!(error.resource, LimitResource::YamlDepth);
        assert_eq!(error.limit, 2);
        assert!(error.observed_at_least > 2);
        assert_eq!(error.file.as_ref(), Some(&path));
    }

    #[test]
    fn yaml_node_budget_reports_the_typed_resource() {
        let path = CatalogPath::new("main.yml").expect("catalog path");
        let limits = YamlLimits {
            nodes: 2,
            ..YamlLimits::default()
        };
        let error = limits
            .usage()
            .validate_source(&path, "value: [1, 2]\n")
            .expect_err("YAML nodes must use the configured budget");

        assert_eq!(error.resource, LimitResource::YamlNodeCount);
        assert_eq!(error.limit, 2);
        assert!(error.observed_at_least > 2);
    }

    #[test]
    fn yaml_alias_budget_reports_the_typed_resource() {
        let path = CatalogPath::new("main.yml").expect("catalog path");
        let limits = YamlLimits {
            aliases: 0,
            ..YamlLimits::default()
        };
        let error = limits
            .usage()
            .validate_source(&path, "value: &shared 1\ncopy: *shared\n")
            .expect_err("YAML aliases must use the configured budget");

        assert_eq!(error.resource, LimitResource::YamlAliasCount);
        assert_eq!(error.limit, 0);
        assert_eq!(error.observed_at_least, 1);
    }

    #[test]
    fn yaml_materialized_scalar_budget_maps_to_alias_expansion_bytes() {
        let path = CatalogPath::new("main.yml").expect("catalog path");
        let limits = YamlLimits {
            alias_expansion_bytes: 13,
            ..YamlLimits::default()
        };
        let error = limits
            .usage()
            .limit_for_raw_breach(
                &path,
                &serde_saphyr::budget::BudgetBreach::ScalarBytes {
                    total_scalar_bytes: 17,
                },
            )
            .expect("scalar materialization breaches are configured limits");

        assert_eq!(error.resource, LimitResource::YamlAliasExpansionBytes);
        assert_eq!(error.limit, 13);
        assert_eq!(error.observed_at_least, 17);
        assert_eq!(error.file.as_ref(), Some(&path));
    }

    #[test]
    fn catalog_total_limit_stops_at_the_first_exceedance() {
        let error = LimitFixture::with(&[("a.rs", 4), ("b.rs", 5)]).validate(&CatalogLimits {
            entry_count: 2,
            total_bytes: 7,
            per_file_bytes: 5,
        });

        assert_eq!(error.resource, LimitResource::CatalogTotalBytes);
        assert_eq!(error.limit, 7);
        assert_eq!(error.observed_at_least, 9);
        assert_eq!(error.file.expect("file").as_str(), "b.rs");
    }

    #[test]
    fn catalog_usage_accumulates_every_input_catalog_in_one_operation() {
        let limits = CatalogLimits {
            entry_count: 2,
            total_bytes: 7,
            per_file_bytes: 5,
        };
        let first = LimitFixture::with(&[("source.rs", 4)]);
        let second = LimitFixture::with(&[("main.yml", 5)]);
        let mut usage = limits.usage();

        let cancellation = CancellationProbe::new();
        usage
            .record(&first.catalog, &cancellation)
            .expect("first catalog fits");
        let error = usage
            .record(&second.catalog, &cancellation)
            .expect_err("the second catalog must cross the operation budget");
        let error = error.limit_exceeded().expect("typed catalog limit");

        assert_eq!(error.resource, LimitResource::CatalogTotalBytes);
        assert_eq!(error.limit, 7);
        assert_eq!(error.observed_at_least, 9);
        assert_eq!(
            error.file.as_ref().expect("crossing file").as_str(),
            "main.yml"
        );
    }

    #[test]
    fn canceled_catalog_accounting_stops_before_entry_scans() {
        let fixture = LimitFixture::with(&[("a.rs", 8), ("b.rs", 8)]);
        let limits = CatalogLimits::default();
        let cancellation = CancellationProbe::new();
        cancellation.cancel();

        let error = limits
            .usage()
            .record(&fixture.catalog, &cancellation)
            .expect_err("canceled catalog accounting must stop");

        assert!(error.is_operation_canceled());
    }

    #[test]
    fn yaml_usage_accumulates_stream_reports_but_keeps_depth_per_stream() {
        let limits = YamlLimits {
            documents: 2,
            depth: 3,
            nodes: 5,
            aliases: 2,
            alias_expansion_bytes: 12,
        };
        let first_path = CatalogPath::new("first.yml").expect("first path");
        let second_path = CatalogPath::new("second.yml").expect("second path");
        let mut usage = limits.usage();
        let first = serde_saphyr::budget::BudgetReport {
            documents: 1,
            nodes: 3,
            aliases: 1,
            max_depth: 3,
            total_scalar_bytes: 6,
            ..serde_saphyr::budget::BudgetReport::default()
        };
        let second = serde_saphyr::budget::BudgetReport {
            documents: 1,
            nodes: 3,
            aliases: 1,
            max_depth: 3,
            total_scalar_bytes: 6,
            ..serde_saphyr::budget::BudgetReport::default()
        };

        usage
            .record_raw_report(&first_path, &first)
            .expect("first stream fits");
        let error = usage
            .record_raw_report(&second_path, &second)
            .expect_err("aggregate nodes must cross on the second stream");

        assert_eq!(error.resource, LimitResource::YamlNodeCount);
        assert_eq!(error.limit, 5);
        assert_eq!(error.observed_at_least, 6);
        assert_eq!(error.file.as_ref(), Some(&second_path));
    }

    #[test]
    #[allow(deprecated)]
    fn yaml_semantic_budget_allows_recorded_raw_input_plus_only_operation_remainder() {
        let limits = YamlLimits {
            documents: 2,
            depth: 7,
            nodes: 8,
            aliases: 4,
            alias_expansion_bytes: 16,
        };
        let path = CatalogPath::new("first.yml").expect("path");
        let mut usage = limits.usage();
        usage
            .record_raw_report(
                &path,
                &serde_saphyr::budget::BudgetReport {
                    documents: 1,
                    nodes: 3,
                    aliases: 1,
                    max_depth: 7,
                    total_scalar_bytes: 5,
                    ..serde_saphyr::budget::BudgetReport::default()
                },
            )
            .expect("first stream fits");

        let raw = serde_saphyr::budget::BudgetReport {
            documents: 1,
            nodes: 3,
            aliases: 1,
            max_depth: 7,
            total_scalar_bytes: 5,
            ..serde_saphyr::budget::BudgetReport::default()
        };
        let semantic = usage
            .semantic_parser_budget(&raw)
            .expect("semantic parser budget");
        assert_eq!(semantic.max_documents, 2);
        assert_eq!(semantic.max_depth, 7);
        assert_eq!(semantic.max_nodes, 8);
        assert_eq!(semantic.max_aliases, 4);
        assert_eq!(semantic.max_total_scalar_bytes, 16);
        assert_eq!(
            usage.semantic_alias_limits(&raw).max_total_replayed_events,
            10
        );
    }

    #[test]
    fn two_container_alias_breach_reports_the_exact_proven_node_lower_bound() {
        let source = "first: &empty []\nsecond: *empty\nthird: *empty\n";
        let raw = serde_saphyr::budget::check_yaml_budget(
            source,
            serde_saphyr::Budget::default(),
            serde_saphyr::budget::EnforcingPolicy::AllContent,
        )
        .expect("raw YAML budget report");
        let raw_nodes = CatalogLimits::observed(raw.nodes);
        let limits = YamlLimits {
            nodes: raw_nodes.saturating_add(1),
            ..YamlLimits::default()
        };
        let path = CatalogPath::new("aliases.yml").expect("path");
        let mut usage = limits.usage();
        let recorded = usage
            .validate_source(&path, source)
            .expect("raw stream leaves one replay node")
            .expect("valid stream has a raw report");
        let options = serde_saphyr::options! {
            budget: usage.semantic_parser_budget(&recorded),
            alias_limits: usage.semantic_alias_limits(&recorded),
        };

        // The first empty-container alias consumes start/end events at the
        // exact boundary. The second alias's start is event three, proving a
        // second replay node without counting every event as a node.
        let error = serde_saphyr::from_multiple_with_options::<serde_json::Value>(source, options)
            .expect_err("the second container alias must cross the replay-event cap");
        let limit = usage
            .limit_for_semantic_parser_error(&path, &recorded, &error)
            .expect("replay-event breach maps to the node budget");

        assert_eq!(limit.resource, LimitResource::YamlNodeCount);
        assert_eq!(limit.limit, raw_nodes.saturating_add(1));
        assert_eq!(limit.observed_at_least, raw_nodes.saturating_add(2));
        assert_eq!(limit.file.as_ref(), Some(&path));
    }

    #[test]
    fn document_and_participating_source_limits_are_typed() {
        let document_limits = YamlLimits {
            documents: 1,
            ..YamlLimits::default()
        };
        let document = document_limits
            .usage()
            .record_documents(2, None)
            .expect_err("document limit");
        assert_eq!(document.resource, LimitResource::YamlDocumentCount);

        let source = RustExtractionLimits {
            source_files: 1,
            ..RustExtractionLimits::default()
        }
        .validate_source_count(2)
        .expect_err("source limit");
        assert_eq!(source.resource, LimitResource::RustSourceFileCount);
    }

    #[test]
    fn rust_file_item_and_signature_limits_are_typed_and_stop_on_crossing() {
        let path = CatalogPath::new("lib.rs").expect("catalog path");
        let limits = RustExtractionLimits {
            per_file_bytes: 3,
            items: 1,
            signatures: 1,
            ..RustExtractionLimits::default()
        };
        let file = limits
            .validate_source_file(&path, 4)
            .expect_err("source file bytes");
        assert_eq!(file.resource, LimitResource::RustSourceFileBytes);
        assert_eq!(file.file.as_ref(), Some(&path));

        let mut usage = limits.usage();
        usage.record_item(Some(&path)).expect("first item");
        let item = usage
            .record_item(Some(&path))
            .expect_err("second item must cross the limit");
        assert_eq!(item.resource, LimitResource::RustItemCount);
        assert_eq!(item.observed_at_least, 2);
        assert_eq!(item.file.as_ref(), Some(&path));

        usage.record_signatures(1).expect("first signature");
        let signature = usage
            .record_signatures(1)
            .expect_err("second signature must cross the limit");
        assert_eq!(signature.resource, LimitResource::SignatureCount);
        assert_eq!(signature.observed_at_least, 2);
    }

    #[test]
    fn rust_item_batch_accepts_the_exact_boundary_and_reports_first_crossing() {
        let path = CatalogPath::new("lib.rs").expect("catalog path");
        let limits = RustExtractionLimits {
            items: 5,
            ..RustExtractionLimits::default()
        };
        let mut usage = limits.usage();

        usage
            .record_items(5, Some(&path))
            .expect("the exact item boundary must be accepted");
        let error = usage
            .record_items(3, Some(&path))
            .expect_err("the next encountered item must stop accounting");

        assert_eq!(error.resource, LimitResource::RustItemCount);
        assert_eq!(error.limit, 5);
        assert_eq!(error.observed_at_least, 6);
        assert_eq!(error.file.as_ref(), Some(&path));
    }

    #[test]
    fn diagnostic_count_and_streamed_serialized_bytes_are_bounded() {
        let count_limits = DiagnosticLimits {
            count: 1,
            ..DiagnosticLimits::default()
        };
        let mut count_usage = count_limits.usage().expect("empty diagnostic array");
        count_usage.record(&"first").expect("first diagnostic");
        let count = count_usage.record(&"second").expect_err("diagnostic count");
        assert_eq!(
            count.limit_exceeded().expect("typed count limit").resource,
            LimitResource::DiagnosticCount
        );

        let byte_limits = DiagnosticLimits {
            count: 2,
            serialized_bytes: 11,
        };
        let mut byte_usage = byte_limits.usage().expect("empty diagnostic array");
        let error = byte_usage
            .record(&"evidence")
            .expect_err("serialized diagnostics must be streamed through the byte budget");
        let limit = error.limit_exceeded().expect("typed limit evidence");
        assert_eq!(limit.resource, LimitResource::DiagnosticBytes);
        assert_eq!(limit.limit, 11);
        assert_eq!(limit.observed_at_least, 12);
    }

    #[test]
    fn diagnostic_usage_accounts_one_shared_json_array_across_error_and_warning_batches() {
        let baseline_limits = DiagnosticLimits::default();
        let mut baseline = baseline_limits.usage().expect("baseline usage");
        baseline.record(&"error").expect("error diagnostic");
        baseline.record(&"warning").expect("warning diagnostic");

        let exact_bytes = serde_json::to_vec(&vec!["error", "warning"])
            .expect("fixture diagnostics")
            .len();
        let exact_limits = DiagnosticLimits {
            count: 2,
            serialized_bytes: u64::try_from(exact_bytes).expect("fixture size"),
        };
        let mut exact = exact_limits.usage().expect("exact usage");
        exact.record(&"error").expect("first batch");
        exact.record(&"warning").expect("second batch");

        let crossing_limits = DiagnosticLimits {
            count: 2,
            serialized_bytes: u64::try_from(exact_bytes - 1).expect("fixture size"),
        };
        let mut crossing = crossing_limits.usage().expect("crossing usage");
        crossing.record(&"error").expect("first batch");
        let error = crossing
            .record(&"warning")
            .expect_err("the shared second batch must cross the aggregate byte budget");
        let limit = error.limit_exceeded().expect("typed limit evidence");
        assert_eq!(limit.resource, LimitResource::DiagnosticBytes);
        assert_eq!(
            limit.limit,
            u64::try_from(exact_bytes - 1).expect("fixture size")
        );
        assert_eq!(
            limit.observed_at_least,
            u64::try_from(exact_bytes).expect("fixture size")
        );
    }

    #[test]
    fn diagnostic_evidence_accounts_exact_json_escaping_and_array_separators() {
        let evidence = ["quote\"", "line\ncontrol\u{0001}"];
        let exact_bytes = serde_json::to_vec(&evidence)
            .expect("fixture evidence")
            .len();
        let exact_limits = DiagnosticLimits {
            count: 2,
            serialized_bytes: u64::try_from(exact_bytes).expect("fixture size"),
        };
        let mut exact = exact_limits.evidence_usage();
        exact.record_text(evidence[0]).expect("first evidence");
        exact
            .record_text(evidence[1])
            .expect("exact evidence boundary");

        let crossing_limits = DiagnosticLimits {
            count: 2,
            serialized_bytes: u64::try_from(exact_bytes - 1).expect("fixture size"),
        };
        let mut crossing = crossing_limits.evidence_usage();
        crossing
            .record_text(evidence[0])
            .expect("first evidence fits");
        let error = crossing
            .record_text(evidence[1])
            .expect_err("escaped evidence must cross the serialized byte budget");

        assert_eq!(error.resource, LimitResource::DiagnosticBytes);
        assert_eq!(
            error.limit,
            u64::try_from(exact_bytes - 1).expect("fixture size")
        );
        assert_eq!(
            error.observed_at_least,
            u64::try_from(exact_bytes).expect("fixture size")
        );
    }

    #[test]
    fn generated_output_meter_stops_at_the_first_file_that_crosses_the_budget() {
        let limits = OutputLimits {
            generated_bytes: 5,
            ..OutputLimits::default()
        };
        let mut meter = GeneratedOutputMeter::new(&limits, CancellationProbe::new());
        meter
            .record(&CatalogPath::new("a.yml").expect("path"), 3)
            .expect("first output");
        let path = CatalogPath::new("b.yml").expect("path");
        let error = meter
            .record(&path, 3)
            .expect_err("second output crosses aggregate budget");
        let error = error.limit_exceeded().expect("typed output limit");

        assert_eq!(error.resource, LimitResource::GeneratedOutputBytes);
        assert_eq!(error.limit, 5);
        assert_eq!(error.observed_at_least, 6);
        assert_eq!(error.file.as_ref(), Some(&path));
    }

    #[test]
    fn generated_output_buffer_accepts_the_exact_boundary_before_allocating_more() {
        use std::io::Write as _;

        let limits = OutputLimits {
            generated_bytes: 5,
            ..OutputLimits::default()
        };
        let mut meter = GeneratedOutputMeter::new(&limits, CancellationProbe::new());
        let path = CatalogPath::new("main.yml").expect("path");
        let mut exact = meter.returned_buffer(&path);
        exact.write_all(b"12345").expect("exact boundary");
        let bytes = meter
            .commit_returned_buffer(exact, Ok::<(), std::io::Error>(()))
            .expect("exact output");
        assert_eq!(bytes, b"12345");

        let mut crossing = meter.returned_buffer(&path);
        let write_error = crossing
            .write_all(b"x")
            .expect_err("the next byte must be rejected before allocation");
        let error = meter
            .commit_returned_buffer(crossing, Err(write_error))
            .expect_err("crossing output budget");
        let limit = error.limit_exceeded().expect("typed output limit");
        assert_eq!(limit.resource, LimitResource::GeneratedOutputBytes);
        assert_eq!(limit.limit, 5);
        assert_eq!(limit.observed_at_least, 6);
        assert_eq!(limit.file.as_ref(), Some(&path));
    }

    #[test]
    fn maintained_serializers_stream_through_the_generated_output_budget() {
        let path = CatalogPath::new("report.yml").expect("path");
        let baseline_limits = OutputLimits::default();
        let mut baseline = GeneratedOutputMeter::new(&baseline_limits, CancellationProbe::new());
        let yaml = baseline
            .serialize_yaml(&path, &vec!["evidence"])
            .expect("baseline YAML");

        let exact_limits = OutputLimits {
            generated_bytes: u64::try_from(yaml.len()).expect("fixture size"),
            ..OutputLimits::default()
        };
        let mut exact = GeneratedOutputMeter::new(&exact_limits, CancellationProbe::new());
        assert_eq!(
            exact
                .serialize_yaml(&path, &vec!["evidence"])
                .expect("exact YAML boundary"),
            yaml
        );

        let mut json_baseline =
            GeneratedOutputMeter::new(&baseline_limits, CancellationProbe::new());
        let json = json_baseline
            .serialize_pretty_json(&path, &vec!["evidence"])
            .expect("baseline JSON");
        let crossing_limits = OutputLimits {
            generated_bytes: u64::try_from(json.len() - 1).expect("fixture size"),
            ..OutputLimits::default()
        };
        let mut crossing = GeneratedOutputMeter::new(&crossing_limits, CancellationProbe::new());
        let error = crossing
            .serialize_pretty_json(&path, &vec!["evidence"])
            .expect_err("JSON output must use the same bounded writer");
        let limit = error.limit_exceeded().expect("typed output limit");
        assert_eq!(limit.resource, LimitResource::GeneratedOutputBytes);
        assert_eq!(limit.file.as_ref(), Some(&path));
    }

    #[test]
    fn generated_output_meter_rejects_a_stale_uncommitted_buffer() {
        use std::io::Write as _;

        let limits = OutputLimits {
            generated_bytes: 6,
            ..OutputLimits::default()
        };
        let mut meter = GeneratedOutputMeter::new(&limits, CancellationProbe::new());
        let path = CatalogPath::new("main.yml").expect("path");
        let mut first = meter.returned_buffer(&path);
        let mut stale = meter.returned_buffer(&path);
        first.write_all(b"123").expect("first buffer");
        stale.write_all(b"456").expect("stale buffer local budget");
        meter
            .commit_returned_buffer(first, Ok::<(), std::io::Error>(()))
            .expect("first commit");

        let error = meter
            .commit_returned_buffer(stale, Ok::<(), std::io::Error>(()))
            .expect_err("a stale starting offset must not be committed");
        assert!(error.to_string().contains("stale generated output buffer"));
    }

    #[test]
    fn generated_output_writer_preserves_typed_cancellation() {
        use std::io::Write as _;

        let limits = OutputLimits::default();
        let cancellation = CancellationProbe::new();
        cancellation.cancel();
        let mut meter = GeneratedOutputMeter::new(&limits, cancellation);
        let path = CatalogPath::new("report.yml").expect("path");

        let error = meter
            .serialize_yaml(&path, &vec!["large"; 1024])
            .expect_err("canceled serialization must stop");

        assert!(error.is_operation_canceled());

        let cancellation = CancellationProbe::new();
        let mut meter = GeneratedOutputMeter::new(&limits, cancellation.clone());
        let mut buffer = meter.returned_buffer(&path);
        buffer.write_all(b"complete bytes").expect("initial write");
        cancellation.cancel();
        let error = meter
            .commit_returned_buffer(buffer, Ok::<(), std::io::Error>(()))
            .expect_err("cancellation before commit must win");
        assert!(error.is_operation_canceled());
    }

    #[test]
    fn scratch_texts_share_one_live_budget_and_release_on_drop() {
        use std::io::Write as _;

        let limits = OutputLimits {
            scratch_bytes: 5,
            ..OutputLimits::default()
        };
        let meter = GeneratedOutputMeter::new(&limits, CancellationProbe::new());
        let path = CatalogPath::new("main.yml").expect("path");

        let mut first_writer = meter.scratch_writer(&path).expect("first writer");
        first_writer.write_all(b"12").expect("first scratch text");
        let first = first_writer.finish_text(Ok::<(), std::io::Error>(()));
        let first = first.expect("first retained text");

        let mut second_writer = meter.scratch_writer(&path).expect("second writer");
        second_writer
            .write_all(b"345")
            .expect("combined exact boundary");
        let second = second_writer
            .finish_text(Ok::<(), std::io::Error>(()))
            .expect("second retained text");

        let mut crossing = meter.scratch_writer(&path).expect("crossing writer");
        crossing
            .write_all(b"x")
            .expect_err("the first byte beyond the live budget must fail");
        let error = crossing
            .finish_text(Ok::<(), std::io::Error>(()))
            .err()
            .expect("crossing scratch text");
        let error = error.limit_exceeded().expect("typed scratch limit");
        assert_eq!(error.resource, LimitResource::OutputScratchBytes);
        assert_eq!(error.limit, 5);
        assert_eq!(error.observed_at_least, 6);
        assert_eq!(error.file.as_ref(), Some(&path));

        drop(first);
        let mut replacement = meter.scratch_writer(&path).expect("replacement writer");
        replacement
            .write_all(b"67")
            .expect("dropping the first text releases exactly two bytes");
        let replacement = replacement
            .finish_text(Ok::<(), std::io::Error>(()))
            .expect("replacement text");

        assert_eq!(second.as_str(), "345");
        assert_eq!(replacement.as_str(), "67");
    }

    #[test]
    fn scratch_failures_release_partial_reservations_and_do_not_charge_output() {
        use std::io::Write as _;

        let limits = OutputLimits {
            generated_bytes: 3,
            scratch_bytes: 3,
        };
        let mut meter = GeneratedOutputMeter::new(&limits, CancellationProbe::new());
        let path = CatalogPath::new("main.yml").expect("path");

        let mut failed = meter.scratch_writer(&path).expect("scratch writer");
        failed.write_all(b"12").expect("partial scratch bytes");
        failed
            .finish_text(Err(std::io::Error::other("serializer failed")))
            .err()
            .expect("serializer failure");
        assert_eq!(meter.retained_scratch_bytes.get(), 0);

        let mut invalid_utf8 = meter.scratch_writer(&path).expect("UTF-8 writer");
        invalid_utf8
            .write_all(&[0xff])
            .expect("invalid UTF-8 remains valid scratch bytes until completion");
        invalid_utf8
            .finish_text(Ok::<(), std::io::Error>(()))
            .err()
            .expect("scratch text must validate UTF-8");
        assert_eq!(meter.retained_scratch_bytes.get(), 0);

        let mut complete = meter.scratch_writer(&path).expect("replacement writer");
        complete
            .write_all(b"123")
            .expect("the released scratch budget is reusable");
        let complete = complete
            .finish_text(Ok::<(), std::io::Error>(()))
            .expect("complete scratch text");
        assert_eq!(complete.as_str(), "123");
        drop(complete);

        let mut returned = meter.returned_buffer(&path);
        returned
            .write_all(b"abc")
            .expect("scratch use does not consume returned output");
        assert_eq!(
            meter
                .commit_returned_buffer(returned, Ok::<(), std::io::Error>(()))
                .expect("returned output"),
            b"abc"
        );
    }

    #[test]
    fn scratch_writer_preserves_cancellation_and_supports_zero() {
        use std::io::Write as _;

        let path = CatalogPath::new("main.yml").expect("path");
        let zero = OutputLimits {
            scratch_bytes: 0,
            ..OutputLimits::default()
        };
        let meter = GeneratedOutputMeter::new(&zero, CancellationProbe::new());
        let mut writer = meter.scratch_writer(&path).expect("zero-limit writer");
        writer
            .write_all(b"x")
            .expect_err("zero scratch rejects the first byte");
        let error = writer
            .finish_text(Ok::<(), std::io::Error>(()))
            .err()
            .expect("zero scratch limit");
        assert_eq!(
            error.limit_exceeded().expect("typed limit").resource,
            LimitResource::OutputScratchBytes
        );

        let cancellation = CancellationProbe::new();
        cancellation.cancel();
        let default_limits = OutputLimits::default();
        let meter = GeneratedOutputMeter::new(&default_limits, cancellation);
        let error = meter
            .scratch_writer(&path)
            .err()
            .expect("pre-cancelled construction must fail");
        assert!(error.is_operation_canceled());

        let cancellation = CancellationProbe::new();
        let default_limits = OutputLimits::default();
        let meter = GeneratedOutputMeter::new(&default_limits, cancellation.clone());
        let mut writer = meter.scratch_writer(&path).expect("scratch writer");
        writer.write_all(b"retained").expect("initial write");
        cancellation.cancel();
        let error = writer
            .finish_text(Ok::<(), std::io::Error>(()))
            .err()
            .expect("mid-write cancellation must win");
        assert!(error.is_operation_canceled());
        assert_eq!(meter.retained_scratch_bytes.get(), 0);
    }

    #[test]
    fn limit_display_uses_typed_file_evidence_without_cached_text() {
        let error = LimitFixture::with(&[("lib.rs", 2)]).validate(&CatalogLimits {
            entry_count: 1,
            total_bytes: 2,
            per_file_bytes: 1,
        });

        assert_eq!(
            error.to_string(),
            "resource limit CatalogFileBytes exceeded: limit 1, observed at least 2 in lib.rs"
        );
    }
}
