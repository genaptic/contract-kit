use super::{CatalogLimits, LimitCharge, LimitExceeded, LimitResource};
use crate::files::CatalogPath;
use serde::{Deserialize, Serialize};
use serde_saphyr::granit_parser::{Event, Tag};

/// YAML stream and materialization budgets.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct YamlLimits {
    /// Maximum semantic documents parsed across one complete operation.
    pub documents: u64,
    /// Maximum YAML nesting depth in one physical stream.
    pub depth: u64,
    /// Maximum YAML semantic nodes across one complete operation.
    pub nodes: u64,
    /// Maximum YAML aliases across one complete operation.
    pub aliases: u64,
    /// Maximum materialized scalar bytes across one operation, including alias replay.
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
    pub(crate) fn budget(&self) -> YamlBudget<'_> {
        YamlBudget {
            limits: self,
            documents: 0,
            nodes: 0,
            aliases: 0,
            materialized_scalar_bytes: 0,
        }
    }

    fn semantic_parser_limit(raw: usize, remaining: u64) -> usize {
        raw.saturating_add(CatalogLimits::parser_limit(remaining))
    }
}

pub(crate) struct YamlBudget<'limits> {
    limits: &'limits YamlLimits,
    documents: u64,
    nodes: u64,
    aliases: u64,
    materialized_scalar_bytes: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RawYamlReport {
    documents: usize,
    nodes: usize,
    aliases: usize,
    max_depth: usize,
    total_scalar_bytes: usize,
}

pub(crate) struct RawYamlMeter<'budget, 'limits> {
    budget: &'budget mut YamlBudget<'limits>,
    path: &'budget CatalogPath,
    report: RawYamlReport,
    depth: usize,
}

impl RawYamlMeter<'_, '_> {
    pub(crate) fn observe(&mut self, event: &Event<'_>) -> Result<(), LimitExceeded> {
        match event {
            Event::DocumentStart(..) => self.charge(LimitResource::YamlDocumentCount, 1)?,
            Event::Scalar(value, _, _, tag) => {
                self.charge(LimitResource::YamlNodeCount, 1)?;
                self.charge(
                    LimitResource::YamlAliasExpansionBytes,
                    value.len().saturating_add(Self::tag_bytes(tag.as_deref())),
                )?;
            }
            Event::MappingStart(_, _, tag) | Event::SequenceStart(_, _, tag) => {
                self.charge(LimitResource::YamlNodeCount, 1)?;
                self.depth = self.depth.saturating_add(1);
                self.charge(
                    LimitResource::YamlDepth,
                    self.depth.saturating_sub(self.report.max_depth),
                )?;
                self.charge(
                    LimitResource::YamlAliasExpansionBytes,
                    Self::tag_bytes(tag.as_deref()),
                )?;
            }
            Event::MappingEnd | Event::SequenceEnd => {
                self.depth = self.depth.saturating_sub(1);
            }
            Event::Alias(_) => self.charge(LimitResource::YamlAliasCount, 1)?,
            Event::Nothing
            | Event::StreamStart
            | Event::StreamEnd
            | Event::DocumentEnd
            | Event::Comment(..) => {}
        }
        Ok(())
    }

    pub(crate) fn finish(self) -> Result<RawYamlReport, LimitExceeded> {
        let Self {
            budget,
            report,
            depth,
            ..
        } = self;
        debug_assert_eq!(depth, 0, "a completed parser stream must be balanced");
        let documents = CatalogLimits::observed(report.documents);
        let nodes = CatalogLimits::observed(report.nodes);
        let aliases = CatalogLimits::observed(report.aliases);
        let scalar_bytes = CatalogLimits::observed(report.total_scalar_bytes);
        budget.documents = budget.documents.saturating_add(documents);
        budget.nodes = budget.nodes.saturating_add(nodes);
        budget.aliases = budget.aliases.saturating_add(aliases);
        budget.materialized_scalar_bytes = budget
            .materialized_scalar_bytes
            .saturating_add(scalar_bytes);
        Ok(report)
    }

    fn charge(&mut self, resource: LimitResource, amount: usize) -> Result<(), LimitExceeded> {
        let path = self.path;
        let (encountered, limit, accumulated) = match resource {
            LimitResource::YamlDocumentCount => (
                &mut self.report.documents,
                self.budget.limits.documents,
                self.budget.documents,
            ),
            LimitResource::YamlDepth => (&mut self.report.max_depth, self.budget.limits.depth, 0),
            LimitResource::YamlNodeCount => (
                &mut self.report.nodes,
                self.budget.limits.nodes,
                self.budget.nodes,
            ),
            LimitResource::YamlAliasCount => (
                &mut self.report.aliases,
                self.budget.limits.aliases,
                self.budget.aliases,
            ),
            LimitResource::YamlAliasExpansionBytes => (
                &mut self.report.total_scalar_bytes,
                self.budget.limits.alias_expansion_bytes,
                self.budget.materialized_scalar_bytes,
            ),
            _ => unreachable!("raw YAML meters only charge YAML resources"),
        };
        *encountered = encountered.saturating_add(amount);
        LimitCharge::new(resource, limit, accumulated).charge(*encountered, path)?;
        Ok(())
    }

    fn tag_bytes(tag: Option<&Tag>) -> usize {
        tag.map_or(0, |tag| {
            let (handle, suffix) = tag.parts();
            handle.len().saturating_add(suffix.len())
        })
    }
}

impl<'limits> YamlBudget<'limits> {
    pub(crate) fn raw_meter<'budget>(
        &'budget mut self,
        path: &'budget CatalogPath,
    ) -> RawYamlMeter<'budget, 'limits> {
        RawYamlMeter {
            budget: self,
            path,
            report: RawYamlReport::default(),
            depth: 0,
        }
    }

    pub(crate) fn semantic_parser_budget(
        &self,
        raw: &RawYamlReport,
    ) -> Option<serde_saphyr::Budget> {
        serde_saphyr::budget! {
            max_reader_input_bytes: None,
            max_events: usize::MAX,
            max_aliases: YamlLimits::semantic_parser_limit(
                raw.aliases,
                self.limits.aliases.saturating_sub(self.aliases)
            ),
            max_anchors: usize::MAX,
            max_depth: CatalogLimits::parser_limit(self.limits.depth),
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
        raw: &RawYamlReport,
    ) -> serde_saphyr::options::AliasLimits {
        serde_saphyr::alias_limits! {
            // A replayed container contributes one semantic node but two
            // balanced start/end events. Two events per remaining node keeps
            // alias replay bounded without rejecting an exact node boundary.
            max_total_replayed_events: CatalogLimits::parser_limit(
                self.limits
                    .nodes
                    .saturating_sub(self.nodes)
                    .saturating_mul(2)
            ),
            max_replay_stack_depth: CatalogLimits::parser_limit(self.limits.depth),
            max_alias_expansions_per_anchor: YamlLimits::semantic_parser_limit(
                raw.aliases,
                self.limits.aliases.saturating_sub(self.aliases)
            )
        }
    }

    pub(crate) fn record_replay_report(
        &mut self,
        path: &CatalogPath,
        raw: &RawYamlReport,
        semantic: &serde_saphyr::budget::BudgetReport,
    ) -> Result<(), LimitExceeded> {
        let nodes = LimitCharge::new(LimitResource::YamlNodeCount, self.limits.nodes, self.nodes)
            .charge(semantic.nodes.saturating_sub(raw.nodes), path)?;
        let materialized_scalar_bytes = LimitCharge::new(
            LimitResource::YamlAliasExpansionBytes,
            self.limits.alias_expansion_bytes,
            self.materialized_scalar_bytes,
        )
        .charge(
            semantic
                .total_scalar_bytes
                .saturating_sub(raw.total_scalar_bytes),
            path,
        )?;

        self.nodes = nodes;
        self.materialized_scalar_bytes = materialized_scalar_bytes;
        Ok(())
    }

    pub(crate) fn limit_for_semantic_parser_error(
        &self,
        path: &CatalogPath,
        raw: &RawYamlReport,
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

    fn limit_for_semantic_breach(
        &self,
        path: &CatalogPath,
        raw: &RawYamlReport,
        breach: &serde_saphyr::budget::BudgetBreach,
    ) -> Option<LimitExceeded> {
        use serde_saphyr::budget::BudgetBreach;

        let (charge, encountered, already_recorded) = match breach {
            BudgetBreach::Aliases { aliases } => (
                LimitCharge::new(
                    LimitResource::YamlAliasCount,
                    self.limits.aliases,
                    self.aliases,
                ),
                *aliases,
                raw.aliases,
            ),
            BudgetBreach::Depth { depth } => (
                LimitCharge::new(LimitResource::YamlDepth, self.limits.depth, 0),
                *depth,
                0,
            ),
            BudgetBreach::Documents { documents } => (
                LimitCharge::new(
                    LimitResource::YamlDocumentCount,
                    self.limits.documents,
                    self.documents,
                ),
                *documents,
                raw.documents,
            ),
            BudgetBreach::Nodes { nodes } => (
                LimitCharge::new(LimitResource::YamlNodeCount, self.limits.nodes, self.nodes),
                *nodes,
                raw.nodes,
            ),
            BudgetBreach::ScalarBytes { total_scalar_bytes } => (
                LimitCharge::new(
                    LimitResource::YamlAliasExpansionBytes,
                    self.limits.alias_expansion_bytes,
                    self.materialized_scalar_bytes,
                ),
                *total_scalar_bytes,
                raw.total_scalar_bytes,
            ),
            _ => return None,
        };
        Some(charge.breach(encountered, already_recorded, path))
    }
}

#[cfg(test)]
mod tests {
    use super::{RawYamlReport, YamlBudget, YamlLimits};
    use crate::files::CatalogPath;
    use crate::limits::{CatalogLimits, LimitResource};
    use serde_saphyr::granit_parser::Parser;

    struct RawYamlFixture<'source> {
        path: CatalogPath,
        source: &'source str,
    }

    impl<'source> RawYamlFixture<'source> {
        fn new(path: &str, source: &'source str) -> Self {
            Self {
                path: CatalogPath::new(path).expect("YAML fixture path"),
                source,
            }
        }

        fn scan(&self, limits: &YamlLimits) -> Result<RawYamlReport, super::LimitExceeded> {
            self.scan_budget(&mut limits.budget())
        }

        fn scan_budget(
            &self,
            budget: &mut YamlBudget<'_>,
        ) -> Result<RawYamlReport, super::LimitExceeded> {
            let mut meter = budget.raw_meter(&self.path);
            for next in Parser::new_from_str(self.source) {
                let (event, _) = next.expect("raw YAML fixture must be syntactically valid");
                meter.observe(&event)?;
            }
            meter.finish()
        }

        fn oracle(&self) -> serde_saphyr::budget::BudgetReport {
            serde_saphyr::budget::check_yaml_budget(
                self.source,
                serde_saphyr::Budget::default(),
                serde_saphyr::budget::EnforcingPolicy::AllContent,
            )
            .expect("raw YAML oracle must parse")
        }
    }

    #[test]
    fn raw_yaml_meter_matches_the_parser_oracle_at_every_exact_boundary() {
        let fixture = RawYamlFixture::new(
            "main.yml",
            "%TAG !e! tag:example.com,2026:\n---\nroot: !e!map {nested: [one, \"λ\"]}\nanchor: &shared value\nalias: *shared\n---\ntagged: !scalar text\nsequence: !sequence [one]\n# ignored comment\n",
        );
        let oracle = fixture.oracle();
        let exact = YamlLimits {
            documents: CatalogLimits::observed(oracle.documents),
            depth: CatalogLimits::observed(oracle.max_depth),
            nodes: CatalogLimits::observed(oracle.nodes),
            aliases: CatalogLimits::observed(oracle.aliases),
            alias_expansion_bytes: CatalogLimits::observed(oracle.total_scalar_bytes),
        };
        let report = fixture.scan(&exact).expect("every exact boundary must fit");
        assert_eq!(report.documents, oracle.documents);
        assert_eq!(report.nodes, oracle.nodes);
        assert_eq!(report.aliases, oracle.aliases);
        assert_eq!(report.max_depth, oracle.max_depth);
        assert_eq!(report.total_scalar_bytes, oracle.total_scalar_bytes);

        let mut one_less = Vec::new();
        for resource in [
            LimitResource::YamlDocumentCount,
            LimitResource::YamlDepth,
            LimitResource::YamlNodeCount,
            LimitResource::YamlAliasCount,
            LimitResource::YamlAliasExpansionBytes,
        ] {
            let mut limits = exact.clone();
            match resource {
                LimitResource::YamlDocumentCount => limits.documents -= 1,
                LimitResource::YamlDepth => limits.depth -= 1,
                LimitResource::YamlNodeCount => limits.nodes -= 1,
                LimitResource::YamlAliasCount => limits.aliases -= 1,
                LimitResource::YamlAliasExpansionBytes => limits.alias_expansion_bytes -= 1,
                _ => unreachable!("closed raw YAML resource list"),
            }
            one_less.push((limits, resource));
        }

        for (limits, expected) in one_less {
            let error = fixture
                .scan(&limits)
                .expect_err("one less than an observed raw resource must fail");
            assert_eq!(error.resource, expected);
            assert_eq!(error.file.as_ref(), Some(&fixture.path));
            assert!(error.observed_at_least > error.limit);
        }
    }

    #[test]
    fn yaml_node_alias_and_scalar_budgets_accumulate_across_physical_files() {
        let first = CatalogPath::new("first.yml").expect("first path");
        let second = CatalogPath::new("second.yml").expect("second path");

        for (limits, source, expected) in [
            (
                YamlLimits {
                    nodes: 3,
                    ..YamlLimits::default()
                },
                "value: one\n",
                LimitResource::YamlNodeCount,
            ),
            (
                YamlLimits {
                    aliases: 1,
                    ..YamlLimits::default()
                },
                "value: &shared one\ncopy: *shared\n",
                LimitResource::YamlAliasCount,
            ),
            (
                YamlLimits {
                    alias_expansion_bytes: 8,
                    ..YamlLimits::default()
                },
                "key: val\n",
                LimitResource::YamlAliasExpansionBytes,
            ),
        ] {
            let mut budget = limits.budget();
            let _ = RawYamlFixture {
                path: first.clone(),
                source,
            }
            .scan_budget(&mut budget)
            .expect("one physical file remains within the aggregate budget");
            let error = RawYamlFixture {
                path: second.clone(),
                source,
            }
            .scan_budget(&mut budget)
            .expect_err("the second physical file must cross the aggregate budget");

            assert_eq!(error.resource, expected);
            assert_eq!(error.file.as_ref(), Some(&second));
        }

        let document_limits = YamlLimits {
            documents: 1,
            ..YamlLimits::default()
        };
        let mut document_budget = document_limits.budget();
        let _ = RawYamlFixture {
            path: first.clone(),
            source: "value: one\n",
        }
        .scan_budget(&mut document_budget)
        .expect("one physical document remains within the aggregate budget");
        let document_error = RawYamlFixture {
            path: second.clone(),
            source: "value: two\n",
        }
        .scan_budget(&mut document_budget)
        .expect_err("the second physical document must cross the aggregate budget");
        assert_eq!(document_error.resource, LimitResource::YamlDocumentCount);
        assert_eq!(document_error.file.as_ref(), Some(&second));
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
        let mut budget = limits.budget();
        let raw = RawYamlFixture::new("first.yml", "a: b\n")
            .scan_budget(&mut budget)
            .expect("raw stream fits");

        let semantic = budget
            .semantic_parser_budget(&raw)
            .expect("semantic parser budget");
        assert_eq!(semantic.max_documents, 2);
        assert_eq!(semantic.max_depth, 7);
        assert_eq!(semantic.max_nodes, 8);
        assert_eq!(semantic.max_aliases, 4);
        assert_eq!(semantic.max_total_scalar_bytes, 16);
        assert_eq!(
            budget.semantic_alias_limits(&raw).max_total_replayed_events,
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
        let mut budget = limits.budget();
        let recorded = RawYamlFixture {
            path: path.clone(),
            source,
        }
        .scan_budget(&mut budget)
        .expect("raw stream leaves one replay node");
        let options = serde_saphyr::options! {
            budget: budget.semantic_parser_budget(&recorded),
            alias_limits: budget.semantic_alias_limits(&recorded),
        };

        // The first empty-container alias consumes start/end events at the
        // exact boundary. The second alias's start is event three, proving a
        // second replay node without counting every event as a node.
        let error = serde_saphyr::from_multiple_with_options::<serde_json::Value>(source, options)
            .expect_err("the second container alias must cross the replay-event cap");
        let limit = budget
            .limit_for_semantic_parser_error(&path, &recorded, &error)
            .expect("replay-event breach maps to the node budget");

        assert_eq!(limit.resource, LimitResource::YamlNodeCount);
        assert_eq!(limit.limit, raw_nodes.saturating_add(1));
        assert_eq!(limit.observed_at_least, raw_nodes.saturating_add(2));
        assert_eq!(limit.file.as_ref(), Some(&path));
    }
}
