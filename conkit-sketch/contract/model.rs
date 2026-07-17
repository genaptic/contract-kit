use crate::api::SketchSeed;
use crate::error::SketchContractKitError;
use crate::files::CatalogPath;
use crate::id::SketchId;
use crate::limits::MatchingLimits;
use crate::normalize::NormalizedSnippet;
use crate::work::CancellationProbe;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Versioned normalization applied before matching one sketch against source bytes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SketchNormalization {
    /// Normalize line endings while preserving all other line bytes exactly.
    ExactLinesV1,
}

impl SketchNormalization {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::ExactLinesV1 => "exact_lines_v1",
        }
    }
}

/// Required number of occurrences for one sketch match.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SketchOccurrence {
    /// Accept the first matching occurrence, including when duplicates exist.
    AtLeastOne,
    /// Require exactly one occurrence and reject both absence and duplicates.
    ExactlyOne,
}

impl SketchOccurrence {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::AtLeastOne => "at_least_one",
            Self::ExactlyOne => "exactly_one",
        }
    }
}

/// Explicit, versioned matching semantics stored with every sketch contract.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct SketchMatchPolicy {
    normalization: SketchNormalization,
    occurrence: SketchOccurrence,
}

impl SketchMatchPolicy {
    /// Creates an explicit matching policy.
    pub const fn new(normalization: SketchNormalization, occurrence: SketchOccurrence) -> Self {
        Self {
            normalization,
            occurrence,
        }
    }

    /// Returns the versioned normalization policy.
    pub const fn normalization(&self) -> SketchNormalization {
        self.normalization
    }

    /// Returns the required occurrence policy.
    pub const fn occurrence(&self) -> SketchOccurrence {
        self.occurrence
    }
}

#[derive(Debug)]
pub(crate) struct SketchContracts {
    entries: Vec<SketchContract>,
    contract_document_count: usize,
}

impl SketchContracts {
    pub(super) fn from_resolved(
        entries: Vec<SketchContract>,
        contract_document_count: usize,
    ) -> Self {
        Self {
            entries,
            contract_document_count,
        }
    }

    pub(crate) fn entries(&self) -> &[SketchContract] {
        &self.entries
    }

    pub(crate) fn get(&self, id: &SketchId) -> Option<&SketchContract> {
        self.entries
            .binary_search_by(|contract| contract.id().cmp(id))
            .map(|index| &self.entries[index])
            .ok()
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn contract_document_count(&self) -> usize {
        self.contract_document_count
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct ContractDocumentLocator {
    contract_file: CatalogPath,
    document_index: usize,
}

impl ContractDocumentLocator {
    pub(super) fn new(contract_file: CatalogPath, document_index: usize) -> Self {
        Self {
            contract_file,
            document_index,
        }
    }
}

impl fmt::Display for ContractDocumentLocator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} document {}",
            self.contract_file, self.document_index
        )
    }
}

#[derive(Debug)]
pub(crate) struct SketchContract {
    id: SketchId,
    locator: ContractDocumentLocator,
    file: CatalogPath,
    linked_signature: SignatureLabel,
    signature_type: SignatureType,
    policy: SketchMatchPolicy,
    snippet: SketchSnippet,
}

impl SketchContract {
    pub(super) fn from_resolved(
        id: SketchId,
        locator: ContractDocumentLocator,
        file: CatalogPath,
        linked_signature: SignatureLabel,
        signature_type: SignatureType,
        policy: SketchMatchPolicy,
        snippet: SketchSnippet,
    ) -> Self {
        Self {
            id,
            locator,
            file,
            linked_signature,
            signature_type,
            policy,
            snippet,
        }
    }

    pub(crate) fn id(&self) -> &SketchId {
        &self.id
    }

    pub(crate) fn contract_file(&self) -> &CatalogPath {
        &self.locator.contract_file
    }

    pub(crate) fn file(&self) -> &CatalogPath {
        &self.file
    }

    pub(super) fn linked_signature(&self) -> &SignatureLabel {
        &self.linked_signature
    }

    pub(crate) fn document_index(&self) -> usize {
        self.locator.document_index
    }

    pub(crate) fn signature_type(&self) -> &SignatureType {
        &self.signature_type
    }

    pub(crate) fn normalization(&self) -> SketchNormalization {
        self.policy.normalization()
    }

    pub(crate) fn occurrence(&self) -> SketchOccurrence {
        self.policy.occurrence()
    }

    pub(super) fn matching_policy(&self) -> SketchMatchPolicy {
        self.policy
    }

    pub(crate) fn snippet(&self) -> &SketchSnippet {
        &self.snippet
    }

    pub(crate) fn validate_seed(
        &self,
        seed: &SketchSeed,
        id: &SketchId,
    ) -> Result<(), SketchContractKitError> {
        if seed.signature_type.trim().is_empty() {
            return Err(SketchContractKitError::conversion_failed(format!(
                "sketch refresh seed {} signature_type must not be empty",
                id.as_str()
            )));
        }
        if &seed.contract_file != self.contract_file() {
            return Err(SketchContractKitError::conversion_failed(format!(
                "sketch refresh seed {} targets contract document {}, expected {}",
                id.as_str(),
                seed.contract_file,
                self.contract_file()
            )));
        }
        if seed.document_index != self.document_index() {
            return Err(SketchContractKitError::conversion_failed(format!(
                "sketch refresh seed {} targets document index {}, expected {}",
                id.as_str(),
                seed.document_index,
                self.document_index()
            )));
        }
        if &seed.file != self.file() {
            return Err(SketchContractKitError::conversion_failed(format!(
                "sketch refresh seed {} targets source file {}, expected {}",
                id.as_str(),
                seed.file,
                self.file()
            )));
        }
        if seed.signature_type.as_str() != self.signature_type().as_str() {
            return Err(SketchContractKitError::conversion_failed(format!(
                "sketch refresh seed {} has signature_type {}, expected {}",
                id.as_str(),
                seed.signature_type,
                self.signature_type().as_str()
            )));
        }

        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct SketchSnippet {
    normalized: NormalizedSnippet,
}

impl SketchSnippet {
    pub(super) fn new(
        code: &str,
        normalization: SketchNormalization,
        locator: &ContractDocumentLocator,
        sketch_id: &str,
        limits: &MatchingLimits,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SketchContractKitError> {
        let normalized = normalization.normalize_snippet(
            code.as_bytes(),
            limits,
            &locator.contract_file,
            cancellation,
        )?;
        if normalized.is_empty() {
            return Err(SketchContractKitError::parse_failed(
                locator,
                format!("sketch {sketch_id} code must not be empty"),
            ));
        }

        Ok(Self { normalized })
    }

    pub(crate) fn normalized(&self) -> &NormalizedSnippet {
        &self.normalized
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct SignatureLabel {
    value: String,
}

impl SignatureLabel {
    pub(super) fn new(
        value: impl Into<String>,
        location: impl ToString,
    ) -> Result<Self, SketchContractKitError> {
        let value = value.into().trim().to_owned();
        if value.is_empty() {
            return Err(SketchContractKitError::parse_failed(
                location,
                "signature label must not be empty",
            ));
        }

        Ok(Self { value })
    }

    pub(super) fn as_str(&self) -> &str {
        &self.value
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SignatureType {
    value: String,
}

impl SignatureType {
    pub(super) fn from_contract(
        value: impl Into<String>,
        location: impl ToString,
        subject: &str,
    ) -> Result<Self, SketchContractKitError> {
        let value = value.into().trim().to_owned();
        if value.is_empty() {
            return Err(SketchContractKitError::parse_failed(
                location,
                format!("{subject} signature_type must not be empty"),
            ));
        }

        Ok(Self { value })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.value
    }
}

#[cfg(test)]
mod tests {
    use super::{ContractDocumentLocator, SketchNormalization, SketchSnippet};
    use crate::contract::tests::{SketchContracts, TestCatalog};
    use crate::files::CatalogPath;
    use crate::id::SketchId;
    use crate::limits::MatchingLimits;
    use crate::work::CancellationProbe;

    #[test]
    fn sketch_snippet_retains_exact_horizontal_bytes() {
        let catalog_name = CatalogPath::new("main.yml").expect("catalog path");
        let locator = ContractDocumentLocator::new(catalog_name.clone(), 0);
        let snippet = SketchSnippet::new(
            "  let   value = 42;  ",
            SketchNormalization::ExactLinesV1,
            &locator,
            "answer",
            &MatchingLimits::default(),
            &CancellationProbe::new(),
        )
        .expect("valid sketch snippet");

        let matching = MatchingLimits::default();
        let expected = SketchNormalization::ExactLinesV1
            .normalize_snippet(
                b"  let   value = 42;  ",
                &matching,
                &catalog_name,
                &CancellationProbe::new(),
            )
            .expect("expected snippet");
        let changed = SketchNormalization::ExactLinesV1
            .normalize_snippet(
                b"let value = 42;",
                &matching,
                &catalog_name,
                &CancellationProbe::new(),
            )
            .expect("changed snippet");

        assert_eq!(snippet.normalized(), &expected);
        assert_ne!(snippet.normalized(), &changed);
    }

    #[test]
    fn canonical_sketch_lookup_covers_sorted_boundaries_and_misses() {
        let contracts = SketchContracts::from_catalog(
            TestCatalog::new()
                .with_file(
                    "lookup.yml",
                    "contract_version: 2\nroot: ../src\nfiles: [c.rs, a.rs, b.rs]\nextraction: { mode: rust_syntax_v2, profile: rust_api_v1, crates: [{ id: example, root: a.rs, kind: library }] }\nsignatures:\n  - charlie:\n      file: c.rs\n      signature_type: function\n      sketch: charlie_body\n  - alpha:\n      file: a.rs\n      signature_type: function\n      sketch: alpha_body\n  - bravo:\n      file: b.rs\n      signature_type: function\n      sketch: bravo_body\nsketches:\n  - charlie_body:\n      file: c.rs\n      signature: charlie\n      signature_type: function\n      matching: { normalization: exact_lines_v1, occurrence: at_least_one }\n      code: charlie\n  - alpha_body:\n      file: a.rs\n      signature: alpha\n      signature_type: function\n      matching: { normalization: exact_lines_v1, occurrence: at_least_one }\n      code: alpha\n  - bravo_body:\n      file: b.rs\n      signature: bravo\n      signature_type: function\n      matching: { normalization: exact_lines_v1, occurrence: at_least_one }\n      code: bravo\n",
                )
                .into_catalog(),
        )
        .expect("lookup contracts");

        for expected in ["alpha_body", "bravo_body", "charlie_body"] {
            let id = SketchId::new(expected.to_owned(), usize::MAX).expect("lookup id");
            assert_eq!(
                contracts.get(&id).map(|contract| contract.id().as_str()),
                Some(expected)
            );
        }
        let missing = SketchId::new("missing_body".to_owned(), usize::MAX).expect("missing id");
        assert!(contracts.get(&missing).is_none());
        let empty = SketchContracts::from_catalog(TestCatalog::new().into_catalog())
            .expect("empty contracts");
        assert!(empty.get(&missing).is_none());
    }
}
