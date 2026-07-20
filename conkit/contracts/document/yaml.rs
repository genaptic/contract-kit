//! Operation-wide YAML stream analysis and resource accounting.

use std::path::Path;

use serde_saphyr::granit_parser::{Event, Parser, ScalarStyle};

use crate::context::ApplicationCancellation;
use crate::error::CliError;

pub(super) struct ContractYamlStream {
    document_count: usize,
    document_start_lines: Vec<u64>,
    raw_report: serde_saphyr::budget::BudgetReport,
}

/// CLI-owned YAML limits applied across every physical contract file in one
/// command-side validation operation.
#[derive(Clone, Copy, Debug)]
enum ContractYamlResource {
    Documents,
    Depth,
    Nodes,
    Aliases,
    ScalarBytes,
}

impl ContractYamlResource {
    const RAW_ACCOUNTING_ORDER: [Self; 5] = [
        Self::Depth,
        Self::Documents,
        Self::Nodes,
        Self::Aliases,
        Self::ScalarBytes,
    ];
    const REPLAY_ACCOUNTING_ORDER: [Self; 2] = [Self::Nodes, Self::ScalarBytes];

    fn label(self) -> &'static str {
        match self {
            Self::Documents => "document count",
            Self::Depth => "depth",
            Self::Nodes => "node count",
            Self::Aliases => "alias count",
            Self::ScalarBytes => "materialized scalar byte count",
        }
    }

    fn from_budget_breach(breach: &serde_saphyr::budget::BudgetBreach) -> Option<(Self, usize)> {
        match breach {
            serde_saphyr::budget::BudgetBreach::Documents { documents } => {
                Some((Self::Documents, *documents))
            }
            serde_saphyr::budget::BudgetBreach::Depth { depth } => Some((Self::Depth, *depth)),
            serde_saphyr::budget::BudgetBreach::Nodes { nodes } => Some((Self::Nodes, *nodes)),
            serde_saphyr::budget::BudgetBreach::Aliases { aliases } => {
                Some((Self::Aliases, *aliases))
            }
            serde_saphyr::budget::BudgetBreach::ScalarBytes { total_scalar_bytes } => {
                Some((Self::ScalarBytes, *total_scalar_bytes))
            }
            _ => None,
        }
    }

    fn observe(self, current: usize, amount: usize) -> usize {
        match self {
            Self::Depth => current.max(amount),
            Self::Documents | Self::Nodes | Self::Aliases | Self::ScalarBytes => {
                current.saturating_add(amount)
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(in super::super) struct ContractYamlCounters {
    pub(in super::super) documents: usize,
    pub(in super::super) depth: usize,
    pub(in super::super) nodes: usize,
    pub(in super::super) aliases: usize,
    pub(in super::super) scalar_bytes: usize,
}

impl ContractYamlCounters {
    fn from_raw_report(report: &serde_saphyr::budget::BudgetReport) -> Self {
        Self {
            documents: report.documents,
            depth: report.max_depth,
            nodes: report.nodes,
            aliases: report.aliases,
            scalar_bytes: report.total_scalar_bytes,
        }
    }

    fn replay_delta(
        raw: &serde_saphyr::budget::BudgetReport,
        semantic: &serde_saphyr::budget::BudgetReport,
    ) -> Self {
        Self {
            nodes: semantic.nodes.saturating_sub(raw.nodes),
            scalar_bytes: semantic
                .total_scalar_bytes
                .saturating_sub(raw.total_scalar_bytes),
            ..Self::default()
        }
    }

    fn value(self, resource: ContractYamlResource) -> usize {
        match resource {
            ContractYamlResource::Documents => self.documents,
            ContractYamlResource::Depth => self.depth,
            ContractYamlResource::Nodes => self.nodes,
            ContractYamlResource::Aliases => self.aliases,
            ContractYamlResource::ScalarBytes => self.scalar_bytes,
        }
    }

    fn charge(
        &mut self,
        resource: ContractYamlResource,
        amount: usize,
        limits: &Self,
    ) -> Result<(), ContractYamlBreach> {
        let limit = limits.value(resource);
        let current = match resource {
            ContractYamlResource::Documents => &mut self.documents,
            ContractYamlResource::Depth => &mut self.depth,
            ContractYamlResource::Nodes => &mut self.nodes,
            ContractYamlResource::Aliases => &mut self.aliases,
            ContractYamlResource::ScalarBytes => &mut self.scalar_bytes,
        };
        let observed_at_least = resource.observe(*current, amount);
        if observed_at_least > limit {
            return Err(ContractYamlBreach {
                resource,
                limit,
                observed_at_least,
            });
        }
        *current = observed_at_least;
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub(in super::super) struct ContractYamlLimits {
    pub(in super::super) ceiling: ContractYamlCounters,
}

impl Default for ContractYamlLimits {
    fn default() -> Self {
        Self {
            ceiling: ContractYamlCounters {
                documents: 1_024,
                depth: 128,
                nodes: 1_000_000,
                aliases: 10_000,
                scalar_bytes: 64 * 1024 * 1024,
            },
        }
    }
}

struct ContractYamlBreach {
    resource: ContractYamlResource,
    limit: usize,
    observed_at_least: usize,
}

impl ContractYamlBreach {
    fn into_error(self, path: &Path) -> CliError {
        CliError::ContractLayout {
            path: path.to_path_buf(),
            message: format!(
                "operation-wide YAML {} limit exceeded: limit {}, observed at least {}",
                self.resource.label(),
                self.limit,
                self.observed_at_least
            ),
        }
    }
}

/// Incremental CLI header-parser accounting for one complete operation.
pub(in super::super) struct ContractYamlUsage<'cancellation> {
    limits: ContractYamlLimits,
    cancellation: &'cancellation ApplicationCancellation,
    pub(super) used: ContractYamlCounters,
}

impl<'cancellation> ContractYamlUsage<'cancellation> {
    pub(in super::super) fn new(cancellation: &'cancellation ApplicationCancellation) -> Self {
        Self::with_limits(cancellation, ContractYamlLimits::default())
    }

    pub(in super::super) fn with_limits(
        cancellation: &'cancellation ApplicationCancellation,
        limits: ContractYamlLimits,
    ) -> Self {
        Self {
            limits,
            cancellation,
            used: ContractYamlCounters::default(),
        }
    }

    fn inspect_source(
        &mut self,
        document_path: &Path,
        source: &str,
    ) -> Result<Option<serde_saphyr::budget::BudgetReport>, CliError> {
        self.cancellation.checkpoint()?;
        let Some(budget) = self.raw_parser_budget() else {
            return Err(CliError::ContractLayout {
                path: document_path.to_path_buf(),
                message: "failed to construct the maintained YAML parser budget".to_owned(),
            });
        };
        let report = serde_saphyr::budget::check_yaml_budget(
            source,
            budget,
            serde_saphyr::budget::EnforcingPolicy::AllContent,
        );
        self.cancellation.checkpoint()?;
        let Ok(report) = report else {
            // The physical event scan immediately below owns syntax errors and
            // can retain their zero-based YAML document index.
            return Ok(None);
        };

        if let Some(breach) = &report.breached {
            return Err(self.raw_breach(document_path, breach));
        }
        self.record(
            document_path,
            ContractYamlCounters::from_raw_report(&report),
            &ContractYamlResource::RAW_ACCOUNTING_ORDER,
        )?;
        Ok(Some(report))
    }

    fn raw_parser_budget(&self) -> Option<serde_saphyr::Budget> {
        serde_saphyr::budget! {
            max_reader_input_bytes: None,
            max_events: usize::MAX,
            max_aliases: self
                .limits
                .ceiling
                .aliases
                .saturating_sub(self.used.aliases),
            max_anchors: usize::MAX,
            max_depth: self.limits.ceiling.depth,
            max_inclusion_depth: 0,
            max_documents: self
                .limits
                .ceiling
                .documents
                .saturating_sub(self.used.documents),
            max_nodes: self
                .limits
                .ceiling
                .nodes
                .saturating_sub(self.used.nodes),
            max_total_scalar_bytes: self
                .limits
                .ceiling
                .scalar_bytes
                .saturating_sub(self.used.scalar_bytes),
            max_total_comment_bytes: usize::MAX,
            max_merge_keys: usize::MAX,
            enforce_alias_anchor_ratio: false
        }
    }

    pub(super) fn semantic_parser_budget(
        &self,
        raw: &serde_saphyr::budget::BudgetReport,
    ) -> Option<serde_saphyr::Budget> {
        serde_saphyr::budget! {
            max_reader_input_bytes: None,
            max_events: usize::MAX,
            max_aliases: raw.aliases.saturating_add(
                self.limits
                    .ceiling
                    .aliases
                    .saturating_sub(self.used.aliases)
            ),
            max_anchors: usize::MAX,
            max_depth: self.limits.ceiling.depth,
            max_inclusion_depth: 0,
            max_documents: raw.documents.saturating_add(
                self.limits
                    .ceiling
                    .documents
                    .saturating_sub(self.used.documents)
            ),
            max_nodes: raw.nodes.saturating_add(
                self.limits
                    .ceiling
                    .nodes
                    .saturating_sub(self.used.nodes)
            ),
            max_total_scalar_bytes: raw.total_scalar_bytes.saturating_add(
                self.limits
                    .ceiling
                    .scalar_bytes
                    .saturating_sub(self.used.scalar_bytes)
            ),
            max_total_comment_bytes: usize::MAX,
            max_merge_keys: usize::MAX,
            enforce_alias_anchor_ratio: false
        }
    }

    pub(super) fn semantic_alias_limits(
        &self,
        raw: &serde_saphyr::budget::BudgetReport,
    ) -> serde_saphyr::options::AliasLimits {
        serde_saphyr::alias_limits! {
            // Replayed container nodes have distinct start and end events.
            // The semantic budget below remains the exact node authority.
            max_total_replayed_events: self
                .limits
                .ceiling
                .nodes
                .saturating_sub(self.used.nodes)
                .saturating_mul(2),
            max_replay_stack_depth: self.limits.ceiling.depth,
            max_alias_expansions_per_anchor: raw.aliases.saturating_add(
                self.limits
                    .ceiling
                    .aliases
                    .saturating_sub(self.used.aliases)
            )
        }
    }

    fn record(
        &mut self,
        document_path: &Path,
        incoming: ContractYamlCounters,
        resources: &[ContractYamlResource],
    ) -> Result<(), CliError> {
        let mut next = self.used;
        for resource in resources {
            next.charge(*resource, incoming.value(*resource), &self.limits.ceiling)
                .map_err(|breach| breach.into_error(document_path))?;
        }
        self.used = next;
        Ok(())
    }

    pub(super) fn record_replay_report(
        &mut self,
        document_path: &Path,
        raw: &serde_saphyr::budget::BudgetReport,
        semantic: &serde_saphyr::budget::BudgetReport,
    ) -> Result<(), CliError> {
        self.record(
            document_path,
            ContractYamlCounters::replay_delta(raw, semantic),
            &ContractYamlResource::REPLAY_ACCOUNTING_ORDER,
        )
    }

    pub(super) fn semantic_limit_error(
        &self,
        document_path: &Path,
        raw: &serde_saphyr::budget::BudgetReport,
        error: &serde_saphyr::Error,
    ) -> Option<CliError> {
        let prior = ContractYamlCounters {
            documents: self.used.documents.saturating_sub(raw.documents),
            depth: 0,
            nodes: self.used.nodes.saturating_sub(raw.nodes),
            aliases: self.used.aliases.saturating_sub(raw.aliases),
            scalar_bytes: self
                .used
                .scalar_bytes
                .saturating_sub(raw.total_scalar_bytes),
        };

        match error {
            serde_saphyr::Error::Budget { breach, .. } => {
                ContractYamlResource::from_budget_breach(breach).map(|(resource, amount)| {
                    self.observed_error(document_path, prior, resource, amount)
                })
            }
            serde_saphyr::Error::AliasReplayLimitExceeded {
                total_replayed_events,
                ..
            } => {
                let replayed_nodes_at_least =
                    (*total_replayed_events / 2).saturating_add(*total_replayed_events % 2);
                Some(self.observed_error(
                    document_path,
                    self.used,
                    ContractYamlResource::Nodes,
                    replayed_nodes_at_least,
                ))
            }
            serde_saphyr::Error::AliasExpansionLimitExceeded { expansions, .. } => {
                Some(self.observed_error(
                    document_path,
                    prior,
                    ContractYamlResource::Aliases,
                    *expansions,
                ))
            }
            serde_saphyr::Error::AliasReplayStackDepthExceeded { depth, .. } => {
                Some(self.observed_error(
                    document_path,
                    ContractYamlCounters::default(),
                    ContractYamlResource::Depth,
                    *depth,
                ))
            }
            _ => None,
        }
    }

    fn raw_breach(
        &self,
        document_path: &Path,
        breach: &serde_saphyr::budget::BudgetBreach,
    ) -> CliError {
        ContractYamlResource::from_budget_breach(breach).map_or_else(
            || CliError::ContractLayout {
                path: document_path.to_path_buf(),
                message: format!("YAML parser budget exceeded: {breach:?}"),
            },
            |(resource, amount)| self.observed_error(document_path, self.used, resource, amount),
        )
    }

    fn observed_error(
        &self,
        document_path: &Path,
        base: ContractYamlCounters,
        resource: ContractYamlResource,
        amount: usize,
    ) -> CliError {
        ContractYamlBreach {
            resource,
            limit: self.limits.ceiling.value(resource),
            observed_at_least: resource.observe(base.value(resource), amount),
        }
        .into_error(document_path)
    }

    pub(in super::super) fn cancellation(&self) -> &ApplicationCancellation {
        self.cancellation
    }
}
impl ContractYamlStream {
    pub(super) fn inspect(
        document_path: &Path,
        source: &str,
        usage: &mut ContractYamlUsage<'_>,
    ) -> Result<Self, CliError> {
        let path = document_path.to_path_buf();
        let raw_report = usage.inspect_source(document_path, source)?;

        let mut document_count = 0;
        let mut document_start_lines = Vec::new();
        let mut document_has_data = None;

        for next in Parser::new_from_str(source) {
            usage.cancellation().checkpoint()?;
            let (event, span) = next.map_err(|source_error| CliError::ContractLayout {
                path: path.clone(),
                message: format!("YAML document index {document_count} is invalid: {source_error}"),
            })?;

            match event {
                Event::DocumentStart(..) => {
                    if document_has_data.replace(false).is_some() {
                        return Err(CliError::ContractLayout {
                            path,
                            message:
                                "YAML parser started a document before ending the previous document"
                                    .to_owned(),
                        });
                    }
                    document_start_lines.push(u64::try_from(span.start.line()).unwrap_or(u64::MAX));
                }
                Event::Alias(_) | Event::SequenceStart(..) | Event::MappingStart(..) => {
                    if let Some(has_data) = document_has_data.as_mut() {
                        *has_data = true;
                    }
                }
                Event::Scalar(value, style, _, tag) => {
                    if let Some(has_data) = document_has_data.as_mut()
                        && !Self::is_null_scalar(value.as_ref(), style, tag.as_deref())
                    {
                        *has_data = true;
                    }
                }
                Event::DocumentEnd => {
                    let has_data = document_has_data.take().unwrap_or(false);
                    if !has_data {
                        return Err(CliError::ContractLayout {
                            path,
                            message: format!(
                                "YAML document index {document_count} must not be empty or null"
                            ),
                        });
                    }
                    document_count += 1;
                }
                Event::Nothing
                | Event::StreamStart
                | Event::StreamEnd
                | Event::Comment(..)
                | Event::SequenceEnd
                | Event::MappingEnd => {}
            }
        }

        if document_has_data.is_some() {
            return Err(CliError::ContractLayout {
                path,
                message: "YAML parser did not terminate the final document".to_owned(),
            });
        }
        if document_count == 0 {
            return Err(CliError::ContractLayout {
                path,
                message: "no contract documents were found in the YAML stream".to_owned(),
            });
        }
        let raw_report = raw_report.ok_or_else(|| CliError::ContractLayout {
            path: document_path.to_path_buf(),
            message: "YAML resource preflight did not return a report for a valid stream"
                .to_owned(),
        })?;

        Ok(Self {
            document_count,
            document_start_lines,
            raw_report,
        })
    }

    pub(super) fn document_count(&self) -> usize {
        self.document_count
    }

    pub(super) fn raw_report(&self) -> &serde_saphyr::budget::BudgetReport {
        &self.raw_report
    }

    pub(super) fn document_index_for_error(&self, error: &serde_saphyr::Error) -> usize {
        let Some(line) = error.location().map(|location| location.line()) else {
            return 0;
        };
        self.document_start_lines
            .partition_point(|start| *start <= line)
            .saturating_sub(1)
    }

    fn is_null_scalar(
        value: &str,
        style: ScalarStyle,
        tag: Option<&serde_saphyr::granit_parser::Tag>,
    ) -> bool {
        if style != ScalarStyle::Plain {
            return false;
        }

        if let Some(tag) = tag {
            return tag.core_suffix() == Some("null");
        }

        value.is_empty() || value == "~" || value.eq_ignore_ascii_case("null")
    }
}

#[cfg(test)]
mod tests {
    use conkit_signature::CatalogPath;

    use super::super::tests::DocumentFixture;
    use super::super::{ContractDocument, ContractDocumentPath};
    use super::{ContractYamlCounters, ContractYamlLimits, ContractYamlUsage};
    use crate::context::ApplicationCancellation;

    #[test]
    fn enforces_the_semantic_parser_default_depth_budget() {
        let fixture = DocumentFixture::new();
        let nested_depth = ContractYamlLimits::default()
            .ceiling
            .depth
            .saturating_add(1);
        let nested = format!("{}0{}", "[".repeat(nested_depth), "]".repeat(nested_depth));
        let document = fixture
            .document(&["lib.rs"], &[("primary", "lib.rs")])
            .replace("sketches: []", &format!("sketches: {nested}"));
        let error = fixture
            .parse(document.as_bytes())
            .expect_err("deep YAML must breach the maintained parser budget");
        let rendered = error.to_string().to_lowercase();

        assert!(
            rendered.contains("depth") || rendered.contains("too complex"),
            "{error}"
        );
        fixture.close();
    }

    #[test]
    fn rejects_empty_document_streams_and_accepts_explicit_markers_around_v2() {
        let fixture = DocumentFixture::new();
        let error = fixture
            .parse(b"")
            .expect_err("empty document stream must be rejected");
        assert!(
            error.to_string().contains("no contract documents"),
            "{error}"
        );

        for bytes in [b"---\n...\n".as_slice(), b"null\n".as_slice()] {
            let error = fixture
                .parse(bytes)
                .expect_err("empty physical document must be rejected");
            assert!(error.to_string().contains("document index 0"), "{error}");
            assert!(error.to_string().contains("empty or null"), "{error}");
        }

        let document = fixture.document(&["lib.rs"], &[("primary", "lib.rs")]);
        fixture
            .parse(format!("---\n{document}...\n").as_bytes())
            .expect("explicit markers around one v2 document");
        fixture.close();
    }

    #[test]
    fn rejects_empty_or_null_documents_at_their_physical_stream_index() {
        let fixture = DocumentFixture::new();
        let valid = fixture.document(&["lib.rs"], &[("primary", "lib.rs")]);

        for (stream, index) in [
            (format!("---\n...\n---\n{valid}"), 0),
            (format!("{valid}---\nnull\n---\n{valid}"), 1),
            (format!("{valid}---\n!!null null\n"), 1),
            (format!("{valid}---\n"), 1),
        ] {
            let error = fixture
                .parse(stream.as_bytes())
                .expect_err("empty physical document must fail");
            let rendered = error.to_string();

            assert!(
                rendered.contains(&format!("document index {index}")),
                "expected physical document index {index} in {rendered}"
            );
            assert!(rendered.contains("empty or null"), "{rendered}");
        }

        let legacy_after_empty = format!(
            "{valid}---\nnull\n---\ncontract_version: 1\nroot: ../src\nfiles: []\nsignatures: []\nsketches: []\n"
        );
        let error = fixture
            .parse(legacy_after_empty.as_bytes())
            .expect_err("empty middle document must fail before the later legacy document");
        assert!(error.to_string().contains("document index 1"), "{error}");

        fixture.close();
    }

    #[test]
    fn operation_yaml_usage_spans_files_and_charges_alias_replay_at_exact_boundaries() {
        let fixture = DocumentFixture::new();
        let document = fixture
            .document(&["lib.rs"], &[("primary", "lib.rs")])
            .replace("  - lib.rs\n", "  - &source_file lib.rs\n")
            .replace("      root: lib.rs\n", "      root: *source_file\n");
        let first = ContractDocumentPath::try_from(
            CatalogPath::new("first.yml").expect("first document path"),
        )
        .expect("checked first path");
        let second = ContractDocumentPath::try_from(
            CatalogPath::new("second.yml").expect("second document path"),
        )
        .expect("checked second path");

        let measurement_cancellation = ApplicationCancellation::new();
        let mut measured = ContractYamlUsage::new(&measurement_cancellation);
        ContractDocument::validate_bytes(first.clone(), document.as_bytes(), &mut measured)
            .expect("one aliased document should establish its semantic usage");
        let raw = serde_saphyr::budget::check_yaml_budget(
            &document,
            serde_saphyr::Budget::default(),
            serde_saphyr::budget::EnforcingPolicy::AllContent,
        )
        .expect("the aliased fixture should have a raw parser report");
        assert!(measured.used.nodes > raw.nodes);
        assert!(measured.used.scalar_bytes > raw.total_scalar_bytes);

        for (limits, expected_resource) in [
            (
                ContractYamlLimits {
                    ceiling: ContractYamlCounters {
                        documents: 2,
                        nodes: measured.used.nodes.saturating_mul(2).saturating_sub(1),
                        ..ContractYamlLimits::default().ceiling
                    },
                },
                "node count",
            ),
            (
                ContractYamlLimits {
                    ceiling: ContractYamlCounters {
                        documents: 2,
                        scalar_bytes: measured
                            .used
                            .scalar_bytes
                            .saturating_mul(2)
                            .saturating_sub(1),
                        ..ContractYamlLimits::default().ceiling
                    },
                },
                "materialized scalar byte count",
            ),
        ] {
            let cancellation = ApplicationCancellation::new();
            let mut usage = ContractYamlUsage::with_limits(&cancellation, limits);
            ContractDocument::validate_bytes(first.clone(), document.as_bytes(), &mut usage)
                .expect("the first file remains within the operation budget");
            let error =
                ContractDocument::validate_bytes(second.clone(), document.as_bytes(), &mut usage)
                    .expect_err("the second file must exceed the cumulative budget");
            assert!(error.to_string().contains(expected_resource), "{error}");
            assert!(error.to_string().contains("second.yml"), "{error}");
        }

        let exact_cancellation = ApplicationCancellation::new();
        let mut exact = ContractYamlUsage::with_limits(
            &exact_cancellation,
            ContractYamlLimits {
                ceiling: ContractYamlCounters {
                    documents: 2,
                    nodes: measured.used.nodes.saturating_mul(2),
                    scalar_bytes: measured.used.scalar_bytes.saturating_mul(2),
                    ..ContractYamlLimits::default().ceiling
                },
            },
        );
        ContractDocument::validate_bytes(first, document.as_bytes(), &mut exact)
            .expect("first exact-boundary input");
        ContractDocument::validate_bytes(second, document.as_bytes(), &mut exact)
            .expect("the aggregate exact boundary must pass");

        let container_alias = fixture
            .document(&["lib.rs"], &[("primary", "lib.rs")])
            .replace(
                "signatures: []\nsketches: []",
                "signatures: &payload []\nsketches: *payload",
            );
        let raw_container = serde_saphyr::budget::check_yaml_budget(
            &container_alias,
            serde_saphyr::Budget::default(),
            serde_saphyr::budget::EnforcingPolicy::AllContent,
        )
        .expect("the container-alias fixture should have a raw report");
        let container_cancellation = ApplicationCancellation::new();
        let mut container_exact = ContractYamlUsage::with_limits(
            &container_cancellation,
            ContractYamlLimits {
                ceiling: ContractYamlCounters {
                    documents: raw_container.documents,
                    nodes: raw_container.nodes.saturating_add(1),
                    aliases: raw_container.aliases,
                    scalar_bytes: raw_container.total_scalar_bytes,
                    ..ContractYamlLimits::default().ceiling
                },
            },
        );
        ContractDocument::validate_bytes(
            ContractDocumentPath::try_from(
                CatalogPath::new("container.yml").expect("container document path"),
            )
            .expect("checked container path"),
            container_alias.as_bytes(),
            &mut container_exact,
        )
        .expect("one replayed container node uses two events but exactly one node");

        let two_container_aliases = fixture
            .document(&["lib.rs"], &[("primary", "lib.rs")])
            .replace(
                "signatures: []\nsketches: []",
                "signatures: &payload []\nsketches: [*payload, *payload]",
            );
        let raw_two = serde_saphyr::budget::check_yaml_budget(
            &two_container_aliases,
            serde_saphyr::Budget::default(),
            serde_saphyr::budget::EnforcingPolicy::AllContent,
        )
        .expect("the two-container-alias fixture should have a raw report");
        let breach_cancellation = ApplicationCancellation::new();
        let mut container_breach = ContractYamlUsage::with_limits(
            &breach_cancellation,
            ContractYamlLimits {
                ceiling: ContractYamlCounters {
                    documents: raw_two.documents,
                    nodes: raw_two.nodes.saturating_add(1),
                    aliases: raw_two.aliases,
                    scalar_bytes: raw_two.total_scalar_bytes,
                    ..ContractYamlLimits::default().ceiling
                },
            },
        );
        let error = ContractDocument::validate_bytes(
            ContractDocumentPath::try_from(
                CatalogPath::new("two-containers.yml").expect("two-container document path"),
            )
            .expect("checked two-container path"),
            two_container_aliases.as_bytes(),
            &mut container_breach,
        )
        .expect_err("the second replayed container must exceed one remaining node");
        assert!(
            error.to_string().contains(&format!(
                "limit {}, observed at least {}",
                raw_two.nodes.saturating_add(1),
                raw_two.nodes.saturating_add(2),
            )),
            "{error}"
        );
        fixture.close();
    }

    #[test]
    fn operation_yaml_usage_counts_documents_across_files() {
        let fixture = DocumentFixture::new();
        let document = fixture.document(&["lib.rs"], &[("primary", "lib.rs")]);
        let first = ContractDocumentPath::try_from(
            CatalogPath::new("first.yml").expect("first document path"),
        )
        .expect("checked first path");
        let second = ContractDocumentPath::try_from(
            CatalogPath::new("second.yml").expect("second document path"),
        )
        .expect("checked second path");
        let cancellation = ApplicationCancellation::new();
        let mut usage = ContractYamlUsage::with_limits(
            &cancellation,
            ContractYamlLimits {
                ceiling: ContractYamlCounters {
                    documents: 1,
                    ..ContractYamlLimits::default().ceiling
                },
            },
        );
        ContractDocument::validate_bytes(first, document.as_bytes(), &mut usage)
            .expect("first document");
        let error = ContractDocument::validate_bytes(second, document.as_bytes(), &mut usage)
            .expect_err("the second physical file must exceed the shared document budget");
        assert!(error.to_string().contains("document count"), "{error}");
        fixture.close();
    }
}
