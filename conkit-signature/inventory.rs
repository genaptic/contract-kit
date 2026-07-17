use crate::error::InventoryError;
use crate::error::SignatureContractKitError;
use crate::limits::DiagnosticLimits;
use crate::work::CancellationProbe;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct SignatureId {
    value: String,
    semantic_value: String,
}

impl SignatureId {
    pub(crate) fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        Self {
            semantic_value: value.clone(),
            value,
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.value
    }

    pub(crate) fn semantic_str(&self) -> &str {
        &self.semantic_value
    }

    pub(crate) fn scoped(document: Option<&str>, value: impl AsRef<str>) -> Self {
        let semantic_value = value.as_ref().to_owned();
        match document {
            Some(document) => Self {
                value: format!("{document}::{semantic_value}"),
                semantic_value,
            },
            None => Self::new(semantic_value),
        }
    }
}

impl fmt::Display for SignatureId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct SignatureDigest {
    bytes: [u8; 32],
}

impl SignatureDigest {
    pub(crate) const VERSION: u16 = 2;
    const ENTRY_DOMAIN: &'static [u8] = b"conkit-signature:v2:entry";
    const GROUP_DOMAIN: &'static [u8] = b"conkit-signature:v2:group";
    const INVENTORY_DOMAIN: &'static [u8] = b"conkit-signature:v2:inventory";

    fn for_entry(bytes: &[u8]) -> Self {
        DigestEncoder::new(Self::ENTRY_DOMAIN)
            .with_field(bytes)
            .finish()
    }

    fn hex_bytes(self) -> [u8; 64] {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut rendered = [0; 64];
        for (index, byte) in self.bytes.into_iter().enumerate() {
            rendered[index * 2] = HEX[usize::from(byte >> 4)];
            rendered[index * 2 + 1] = HEX[usize::from(byte & 0x0f)];
        }
        rendered
    }

    pub(crate) fn render(self) -> String {
        String::from_utf8(self.hex_bytes().to_vec()).expect("hex digest is valid UTF-8")
    }
}

struct DigestEncoder {
    hasher: Sha256,
}

impl DigestEncoder {
    fn new(domain: &[u8]) -> Self {
        let mut encoder = Self {
            hasher: Sha256::new(),
        };
        encoder.write_field(domain);
        encoder
    }

    fn with_field(mut self, value: &[u8]) -> Self {
        self.write_field(value);
        self
    }

    fn write_field(&mut self, value: &[u8]) {
        self.hasher.update((value.len() as u64).to_be_bytes());
        self.hasher.update(value);
    }

    fn write_optional_field(&mut self, value: Option<&[u8]>) {
        match value {
            Some(value) => {
                self.write_field(&[1]);
                self.write_field(value);
            }
            None => self.write_field(&[0]),
        }
    }

    fn finish(self) -> SignatureDigest {
        SignatureDigest {
            bytes: self.hasher.finalize().into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SignatureEntry {
    id: SignatureId,
    group_id: SignatureId,
    digest: SignatureDigest,
}

impl SignatureEntry {
    pub(crate) fn from_grouped_canonical_bytes(
        id: SignatureId,
        group_id: SignatureId,
        canonical_bytes: &[u8],
    ) -> Self {
        Self {
            id,
            group_id,
            digest: SignatureDigest::for_entry(canonical_bytes),
        }
    }

    pub(crate) fn id(&self) -> &SignatureId {
        &self.id
    }

    pub(crate) fn digest(&self) -> &SignatureDigest {
        &self.digest
    }

    pub(crate) fn group_id(&self) -> &SignatureId {
        &self.group_id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SignatureGroupContext {
    extraction: Option<Vec<u8>>,
    document_metadata: Option<Vec<u8>>,
}

impl SignatureGroupContext {
    pub(crate) fn new(extraction: Option<Vec<u8>>, document_metadata: Option<Vec<u8>>) -> Self {
        Self {
            extraction,
            document_metadata,
        }
    }

    fn extraction(&self) -> Option<&[u8]> {
        self.extraction.as_deref()
    }

    fn document_metadata(&self) -> Option<&[u8]> {
        self.document_metadata.as_deref()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct SignatureGroup {
    entry_ids: BTreeSet<SignatureId>,
    context: Option<SignatureGroupContext>,
}

impl SignatureGroup {
    fn insert(&mut self, id: SignatureId) {
        self.entry_ids.insert(id);
    }

    fn source_shape_digest(
        &self,
        entries: &BTreeMap<SignatureId, SignatureEntry>,
        cancellation: &CancellationProbe,
    ) -> Result<SignatureDigest, SignatureContractKitError> {
        let mut canonical_entries = Vec::with_capacity(self.entry_ids.len());
        for id in &self.entry_ids {
            cancellation.checkpoint()?;
            let entry = entries
                .get(id)
                .ok_or_else(|| InventoryError::MissingSignatureEntry { id: id.clone() })?;
            canonical_entries.push((entry.id().semantic_str(), *entry.digest()));
        }
        canonical_entries.sort();

        let mut encoder =
            DigestEncoder::new(SignatureDigest::GROUP_DOMAIN).with_field(b"source_shape");
        for (id, digest) in canonical_entries {
            cancellation.checkpoint()?;
            encoder.write_field(id.as_bytes());
            encoder.write_field(&digest.hex_bytes());
        }
        Ok(encoder.finish())
    }

    fn contract_digest(
        &self,
        entries: &BTreeMap<SignatureId, SignatureEntry>,
        cancellation: &CancellationProbe,
    ) -> Result<SignatureDigest, SignatureContractKitError> {
        let source_shape = self.source_shape_digest(entries, cancellation)?;
        let mut encoder = DigestEncoder::new(SignatureDigest::GROUP_DOMAIN).with_field(b"contract");
        encoder.write_field(&source_shape.hex_bytes());
        encoder.write_optional_field(
            self.context
                .as_ref()
                .and_then(SignatureGroupContext::extraction),
        );
        encoder.write_optional_field(
            self.context
                .as_ref()
                .and_then(SignatureGroupContext::document_metadata),
        );
        Ok(encoder.finish())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SignatureInventory {
    entries: BTreeMap<SignatureId, SignatureEntry>,
    groups: BTreeMap<SignatureId, SignatureGroup>,
}

impl SignatureInventory {
    pub(crate) fn insert(&mut self, entry: SignatureEntry) -> Result<(), InventoryError> {
        if let Some(existing) = self.entries.get(entry.id()) {
            if existing == &entry {
                return Ok(());
            }

            if existing.group_id() != entry.group_id() {
                return Err(InventoryError::DuplicateSignatureGroup {
                    id: Box::new(entry.id().clone()),
                    existing_group: Box::new(existing.group_id().clone()),
                    incoming_group: Box::new(entry.group_id().clone()),
                });
            }

            return Err(InventoryError::DuplicateSignatureMismatch {
                id: entry.id().clone(),
            });
        }

        self.groups
            .entry(entry.group_id().clone())
            .or_default()
            .insert(entry.id().clone());
        self.entries.insert(entry.id().clone(), entry);
        Ok(())
    }

    pub(crate) fn set_group_context(
        &mut self,
        group_id: SignatureId,
        context: SignatureGroupContext,
    ) -> Result<(), InventoryError> {
        let group = self.groups.entry(group_id.clone()).or_default();
        if group
            .context
            .as_ref()
            .is_some_and(|existing| existing != &context)
        {
            return Err(InventoryError::DuplicateSignatureMismatch { id: group_id });
        }
        group.context = Some(context);
        Ok(())
    }

    pub(crate) fn merge(
        &mut self,
        incoming: Self,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        let SignatureInventory { entries, groups } = incoming;
        for entry in entries.into_values() {
            cancellation.checkpoint()?;
            self.insert(entry)?;
        }
        for (group_id, group) in groups {
            cancellation.checkpoint()?;
            if let Some(context) = group.context {
                self.set_group_context(group_id, context)?;
            }
        }

        Ok(())
    }

    pub(crate) fn merge_all(
        inventories: Vec<Self>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let mut merged = Self::default();

        for inventory in inventories {
            cancellation.checkpoint()?;
            merged.merge(inventory, cancellation)?;
        }

        Ok(merged)
    }

    pub(crate) fn len(&self) -> usize {
        self.groups.len()
    }

    pub(crate) fn source_shape_digest(
        &self,
        cancellation: &CancellationProbe,
    ) -> Result<SignatureDigest, SignatureContractKitError> {
        let mut groups = Vec::with_capacity(self.groups.len());
        for group_id in self.groups.keys() {
            cancellation.checkpoint()?;
            groups.push(self.group_source_shape_digest(group_id, cancellation)?);
        }
        groups.sort();

        let mut encoder =
            DigestEncoder::new(SignatureDigest::INVENTORY_DOMAIN).with_field(b"source_shape");
        for digest in groups {
            cancellation.checkpoint()?;
            encoder.write_field(&digest.hex_bytes());
        }
        Ok(encoder.finish())
    }

    pub(crate) fn contract_digest(
        &self,
        cancellation: &CancellationProbe,
    ) -> Result<SignatureDigest, SignatureContractKitError> {
        let mut groups = Vec::with_capacity(self.groups.len());
        for group_id in self.groups.keys() {
            cancellation.checkpoint()?;
            let digest = self.group_contract_digest(group_id, cancellation)?;
            groups.push((group_id.semantic_str(), digest));
        }
        groups.sort();

        let mut encoder =
            DigestEncoder::new(SignatureDigest::INVENTORY_DOMAIN).with_field(b"contract");
        for (id, digest) in groups {
            cancellation.checkpoint()?;
            encoder.write_field(id.as_bytes());
            encoder.write_field(&digest.hex_bytes());
        }
        Ok(encoder.finish())
    }

    pub(crate) fn compare_against(
        &self,
        expected: &Self,
        limits: &DiagnosticLimits,
        cancellation: &CancellationProbe,
    ) -> Result<InventoryComparison, SignatureContractKitError> {
        let mut diagnostics = BTreeMap::<SignatureId, InventoryDiagnostic>::new();

        for (id, expected_entry) in &expected.entries {
            cancellation.checkpoint()?;
            let group_id = expected_entry.group_id().clone();
            if diagnostics.contains_key(&group_id) {
                continue;
            }
            match self.entries.get(id) {
                Some(actual_entry) if actual_entry.digest() == expected_entry.digest() => {}
                Some(actual_entry) => {
                    let expected_group = expected.groups.get(&group_id).ok_or_else(|| {
                        InventoryError::MissingSignatureGroup {
                            id: group_id.clone(),
                        }
                    })?;
                    let actual_group_id = actual_entry.group_id();
                    let actual_group = self.groups.get(actual_group_id).ok_or_else(|| {
                        InventoryError::MissingSignatureGroup {
                            id: actual_group_id.clone(),
                        }
                    })?;
                    diagnostics.insert(
                        group_id.clone(),
                        InventoryDiagnostic::Mismatched {
                            signature_id: group_id.clone(),
                            expected_digest: expected_group
                                .source_shape_digest(&expected.entries, cancellation)?,
                            actual_digest: actual_group
                                .source_shape_digest(&self.entries, cancellation)?,
                        },
                    );
                }
                None => {
                    diagnostics.insert(
                        group_id.clone(),
                        InventoryDiagnostic::Missing {
                            signature_id: group_id,
                        },
                    );
                }
            }
            limits.validate_count(diagnostics.len())?;
        }

        for id in self.entries.keys() {
            cancellation.checkpoint()?;
            if !expected.entries.contains_key(id) {
                let entry = &self.entries[id];
                let signature_id = expected
                    .group_for_structural_peer(entry.group_id())
                    .unwrap_or_else(|| entry.group_id().clone());
                diagnostics
                    .entry(signature_id.clone())
                    .or_insert(InventoryDiagnostic::Extra { signature_id });
            }
            limits.validate_count(diagnostics.len())?;
        }

        Ok(InventoryComparison {
            source_signature_count: self.len(),
            contract_signature_count: expected.len(),
            source_shape_digest: self.source_shape_digest(cancellation)?,
            diagnostics: diagnostics.into_values().collect(),
        })
    }

    pub(crate) fn diff_against(
        &self,
        previous: &Self,
        limits: &DiagnosticLimits,
        cancellation: &CancellationProbe,
    ) -> Result<InventoryDiff, SignatureContractKitError> {
        let mut entries = Vec::new();
        let current_groups = self.semantic_groups(cancellation)?;
        let previous_groups = previous.semantic_groups(cancellation)?;
        let previous_label_peers =
            previous_groups.label_peers_absent_from(&current_groups, cancellation)?;
        let current_label_peers =
            current_groups.label_peers_absent_from(&previous_groups, cancellation)?;
        let mut semantic_ids = BTreeSet::new();
        for &semantic_id in current_groups.groups.keys() {
            cancellation.checkpoint()?;
            semantic_ids.insert(semantic_id);
        }
        for &semantic_id in previous_groups.groups.keys() {
            cancellation.checkpoint()?;
            semantic_ids.insert(semantic_id);
        }

        for semantic_id in semantic_ids {
            cancellation.checkpoint()?;
            let current = current_groups
                .groups
                .get(semantic_id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let previous = previous_groups
                .groups
                .get(semantic_id)
                .map(Vec::as_slice)
                .unwrap_or_default();
            if current == previous {
                continue;
            }

            let mut unmatched_current = Vec::new();
            let mut unmatched_previous = Vec::new();
            let mut current_index = 0;
            let mut previous_index = 0;
            while current_index < current.len() && previous_index < previous.len() {
                cancellation.checkpoint()?;
                match current[current_index].cmp(&previous[previous_index]) {
                    std::cmp::Ordering::Less => {
                        unmatched_current.push(&current[current_index]);
                        current_index += 1;
                    }
                    std::cmp::Ordering::Equal => {
                        current_index += 1;
                        previous_index += 1;
                    }
                    std::cmp::Ordering::Greater => {
                        unmatched_previous.push(&previous[previous_index]);
                        previous_index += 1;
                    }
                }
            }
            for group in &current[current_index..] {
                cancellation.checkpoint()?;
                unmatched_current.push(group);
            }
            for group in &previous[previous_index..] {
                cancellation.checkpoint()?;
                unmatched_previous.push(group);
            }

            let shared = unmatched_current.len().min(unmatched_previous.len());
            for index in 0..shared {
                cancellation.checkpoint()?;
                let current = unmatched_current[index];
                let previous = unmatched_previous[index];
                entries.push(InventoryDiffEntry::Changed {
                    signature_id: SignatureId::new(semantic_id),
                    current_digest: current.contract,
                    previous_digest: previous.contract,
                    categories: current.changed_categories(previous),
                });
                limits.validate_count(entries.len())?;
            }
            for group in &unmatched_current[shared..] {
                cancellation.checkpoint()?;
                entries.push(InventoryDiffEntry::Added {
                    signature_id: SignatureId::new(semantic_id),
                    categories: group.unmatched_categories(
                        &current_groups,
                        &previous_groups,
                        &previous_label_peers,
                    ),
                });
                limits.validate_count(entries.len())?;
            }
            for group in &unmatched_previous[shared..] {
                cancellation.checkpoint()?;
                entries.push(InventoryDiffEntry::Removed {
                    signature_id: SignatureId::new(semantic_id),
                    categories: group.unmatched_categories(
                        &previous_groups,
                        &current_groups,
                        &current_label_peers,
                    ),
                });
                limits.validate_count(entries.len())?;
            }
        }

        Ok(InventoryDiff {
            contract_digest: self.contract_digest(cancellation)?,
            entries,
        })
    }

    fn semantic_groups<'inventory>(
        &'inventory self,
        cancellation: &CancellationProbe,
    ) -> Result<InventorySemanticIndex<'inventory>, SignatureContractKitError> {
        let mut groups = BTreeMap::<&str, Vec<InventoryGroupDigest<'_>>>::new();
        for (group_id, group) in &self.groups {
            cancellation.checkpoint()?;
            groups
                .entry(group_id.semantic_str())
                .or_default()
                .push(InventoryGroupDigest::new(
                    group,
                    &self.entries,
                    cancellation,
                )?);
        }
        for digests in groups.values_mut() {
            digests.sort();
        }
        InventorySemanticIndex::new(groups, cancellation)
    }

    fn group_source_shape_digest(
        &self,
        group_id: &SignatureId,
        cancellation: &CancellationProbe,
    ) -> Result<SignatureDigest, SignatureContractKitError> {
        self.groups
            .get(group_id)
            .ok_or_else(|| InventoryError::MissingSignatureGroup {
                id: group_id.clone(),
            })?
            .source_shape_digest(&self.entries, cancellation)
    }

    fn group_contract_digest(
        &self,
        group_id: &SignatureId,
        cancellation: &CancellationProbe,
    ) -> Result<SignatureDigest, SignatureContractKitError> {
        self.groups
            .get(group_id)
            .ok_or_else(|| InventoryError::MissingSignatureGroup {
                id: group_id.clone(),
            })?
            .contract_digest(&self.entries, cancellation)
    }

    fn group_for_structural_peer(&self, structural_group: &SignatureId) -> Option<SignatureId> {
        self.entries
            .get(structural_group)
            .map(|entry| entry.group_id().clone())
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct InventoryGroupDigest<'inventory> {
    source_shape: SignatureDigest,
    contract: SignatureDigest,
    extraction_context: Option<&'inventory [u8]>,
    document_metadata: Option<&'inventory [u8]>,
}

impl<'inventory> InventoryGroupDigest<'inventory> {
    fn new(
        group: &'inventory SignatureGroup,
        entries: &BTreeMap<SignatureId, SignatureEntry>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let context = group.context.as_ref();
        Ok(Self {
            source_shape: group.source_shape_digest(entries, cancellation)?,
            contract: group.contract_digest(entries, cancellation)?,
            extraction_context: context.and_then(SignatureGroupContext::extraction),
            document_metadata: context.and_then(SignatureGroupContext::document_metadata),
        })
    }

    fn changed_categories(
        &self,
        previous: &InventoryGroupDigest<'_>,
    ) -> BTreeSet<InventoryChangeCategory> {
        let mut categories = BTreeSet::new();
        if self.source_shape != previous.source_shape {
            categories.insert(InventoryChangeCategory::SourceSemantics);
        }
        if self.extraction_context != previous.extraction_context {
            categories.insert(InventoryChangeCategory::ExtractionContext);
        }
        if self.document_metadata != previous.document_metadata {
            categories.insert(InventoryChangeCategory::DocumentMetadata);
        }
        categories
    }

    fn unmatched_categories(
        &self,
        own: &InventorySemanticIndex<'_>,
        other: &InventorySemanticIndex<'_>,
        label_peers: &InventoryLabelPeers<'_, '_>,
    ) -> BTreeSet<InventoryChangeCategory> {
        let own_shape_count = own.source_shape_count(&self.source_shape);
        let other_shape_count = other.source_shape_count(&self.source_shape);
        if own_shape_count != other_shape_count {
            return [InventoryChangeCategory::SourceSemantics]
                .into_iter()
                .collect();
        }
        match label_peers.peer_for(self) {
            Some(peer) => {
                let mut categories = self.changed_categories(peer);
                categories.insert(InventoryChangeCategory::Labels);
                categories
            }
            None => [InventoryChangeCategory::SourceSemantics]
                .into_iter()
                .collect(),
        }
    }
}

#[derive(Default)]
struct InventoryLabelPeers<'index, 'inventory> {
    by_source_shape: BTreeMap<
        SignatureDigest,
        BTreeMap<SignatureDigest, &'index InventoryGroupDigest<'inventory>>,
    >,
}

impl<'index, 'inventory> InventoryLabelPeers<'index, 'inventory> {
    fn insert(&mut self, digest: &'index InventoryGroupDigest<'inventory>) {
        self.by_source_shape
            .entry(digest.source_shape)
            .or_default()
            .entry(digest.contract)
            .or_insert(digest);
    }

    fn peer_for(
        &self,
        digest: &InventoryGroupDigest<'_>,
    ) -> Option<&'index InventoryGroupDigest<'inventory>> {
        let peers = self.by_source_shape.get(&digest.source_shape)?;
        peers
            .get(&digest.contract)
            .copied()
            .or_else(|| peers.values().next().copied())
    }
}

struct InventorySemanticIndex<'inventory> {
    groups: BTreeMap<&'inventory str, Vec<InventoryGroupDigest<'inventory>>>,
    source_shape_counts: BTreeMap<SignatureDigest, usize>,
    labels_by_source_shape: BTreeMap<SignatureDigest, BTreeSet<&'inventory str>>,
}

impl<'inventory> InventorySemanticIndex<'inventory> {
    fn new(
        groups: BTreeMap<&'inventory str, Vec<InventoryGroupDigest<'inventory>>>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let mut source_shape_counts = BTreeMap::new();
        let mut labels_by_source_shape = BTreeMap::<SignatureDigest, BTreeSet<&str>>::new();
        for (semantic_id, digests) in &groups {
            cancellation.checkpoint()?;
            for digest in digests {
                cancellation.checkpoint()?;
                *source_shape_counts.entry(digest.source_shape).or_insert(0) += 1;
                labels_by_source_shape
                    .entry(digest.source_shape)
                    .or_default()
                    .insert(*semantic_id);
            }
        }
        Ok(Self {
            groups,
            source_shape_counts,
            labels_by_source_shape,
        })
    }

    fn source_shape_count(&self, source_shape: &SignatureDigest) -> usize {
        self.source_shape_counts
            .get(source_shape)
            .copied()
            .unwrap_or(0)
    }

    fn label_peers_absent_from<'index>(
        &'index self,
        other: &InventorySemanticIndex<'_>,
        cancellation: &CancellationProbe,
    ) -> Result<InventoryLabelPeers<'index, 'inventory>, SignatureContractKitError> {
        let mut peers = InventoryLabelPeers::default();
        for (label, digests) in &self.groups {
            cancellation.checkpoint()?;
            for digest in digests {
                cancellation.checkpoint()?;
                let other_labels = other.labels_by_source_shape.get(&digest.source_shape);
                if other_labels.is_some_and(|other| other.contains(*label)) {
                    continue;
                }
                peers.insert(digest);
            }
        }
        Ok(peers)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InventoryComparison {
    source_signature_count: usize,
    contract_signature_count: usize,
    source_shape_digest: SignatureDigest,
    diagnostics: Vec<InventoryDiagnostic>,
}

impl InventoryComparison {
    pub(crate) fn source_signature_count(&self) -> usize {
        self.source_signature_count
    }

    pub(crate) fn contract_signature_count(&self) -> usize {
        self.contract_signature_count
    }

    pub(crate) fn source_shape_digest(&self) -> &SignatureDigest {
        &self.source_shape_digest
    }

    pub(crate) fn diagnostics(&self) -> &[InventoryDiagnostic] {
        &self.diagnostics
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum InventoryDiagnostic {
    Missing {
        signature_id: SignatureId,
    },
    Extra {
        signature_id: SignatureId,
    },
    Mismatched {
        signature_id: SignatureId,
        expected_digest: SignatureDigest,
        actual_digest: SignatureDigest,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InventoryDiff {
    contract_digest: SignatureDigest,
    entries: Vec<InventoryDiffEntry>,
}

impl InventoryDiff {
    pub(crate) fn contract_digest(&self) -> &SignatureDigest {
        &self.contract_digest
    }

    pub(crate) fn entries(&self) -> &[InventoryDiffEntry] {
        &self.entries
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum InventoryDiffEntry {
    Added {
        signature_id: SignatureId,
        categories: BTreeSet<InventoryChangeCategory>,
    },
    Removed {
        signature_id: SignatureId,
        categories: BTreeSet<InventoryChangeCategory>,
    },
    Changed {
        signature_id: SignatureId,
        current_digest: SignatureDigest,
        previous_digest: SignatureDigest,
        categories: BTreeSet<InventoryChangeCategory>,
    },
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum InventoryChangeCategory {
    SourceSemantics,
    ExtractionContext,
    Labels,
    DocumentMetadata,
}

#[cfg(test)]
mod tests {
    use super::{
        InventoryChangeCategory, InventoryDiagnostic, InventoryDiffEntry, SignatureDigest,
        SignatureEntry, SignatureGroupContext, SignatureId, SignatureInventory,
    };
    use proptest::prelude::*;
    use std::collections::BTreeSet;

    fn cancellation() -> crate::work::CancellationProbe {
        crate::work::CancellationProbe::new()
    }

    #[test]
    fn v2_digest_domains_are_distinct_and_lowercase_sha256() {
        let entry = SignatureDigest::for_entry(b"same canonical payload");
        let group = super::DigestEncoder::new(SignatureDigest::GROUP_DOMAIN)
            .with_field(b"same canonical payload")
            .finish();
        let inventory = super::DigestEncoder::new(SignatureDigest::INVENTORY_DOMAIN)
            .with_field(b"same canonical payload")
            .finish();

        assert_ne!(entry, group);
        assert_ne!(entry, inventory);
        assert_ne!(group, inventory);
        assert_eq!(
            entry.render(),
            "32ed8807a96eaf8ff5a7197bce1242084ebd6c772376993d1ca9de01b2578e29"
        );
        assert_eq!(
            group.render(),
            "7b559f5cfed56194a6ed8b5c7a36d42f9886b5ea4d9d9a0e679aa2e94646e28f"
        );
        assert_eq!(
            inventory.render(),
            "e72733446b5daaf629006d5e7fbd0490038c3f056d4c7be58a679be82270cb46"
        );
        let entry_bytes: [u8; 64] = entry.hex_bytes();
        assert_eq!(std::mem::size_of::<SignatureDigest>(), 32);
        assert_eq!(
            &entry_bytes,
            b"32ed8807a96eaf8ff5a7197bce1242084ebd6c772376993d1ca9de01b2578e29"
        );
        for digest in [entry, group, inventory] {
            let rendered = digest.hex_bytes();
            assert_eq!(rendered.len(), 64);
            assert!(
                rendered
                    .into_iter()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                "digest must be deterministic lowercase hexadecimal: {digest:?}"
            );
        }
    }

    #[test]
    fn inventory_digest_stops_before_work_when_the_operation_is_canceled() {
        let mut inventory = SignatureInventory::default();
        inventory
            .insert(entry("rust:function:answer", b"answer"))
            .expect("fixture entry");
        let cancellation = crate::work::CancellationProbe::new();
        cancellation.cancel();

        let error = inventory
            .source_shape_digest(&cancellation)
            .expect_err("canceled digest construction must stop");

        assert!(error.to_string().contains("canceled"), "{error}");
    }

    #[test]
    fn absent_group_context_is_none_instead_of_an_empty_byte_sentinel() {
        let mut inventory = SignatureInventory::default();
        inventory
            .insert(entry("rust:function:answer", b"answer"))
            .expect("entry");

        let group = inventory
            .groups
            .get(&SignatureId::new("rust:function:answer"))
            .expect("group");

        assert_eq!(group.context, None);
    }

    #[test]
    fn v2_source_shape_and_contract_identity_have_stable_goldens() {
        let mut inventory = SignatureInventory::default();
        inventory
            .insert(entry("rust:function:a", b"a"))
            .expect("golden entry");

        assert_eq!(
            inventory
                .source_shape_digest(&cancellation())
                .expect("golden source-shape digest")
                .render(),
            "d57e58709d839d40a91ec3288da3f6325780b27e35cbd5d00ac0b411cb434dce"
        );
        assert_eq!(
            inventory
                .contract_digest(&cancellation())
                .expect("golden contract digest")
                .render(),
            "4b6db77adb11d8c2a6d2ce3aeaeb9d6d7e2c3abdefab1f05ac89ed52ae3edcfc"
        );
    }

    #[test]
    fn malformed_internal_topology_returns_a_typed_error_instead_of_panicking() {
        let mut inventory = SignatureInventory::default();
        inventory
            .groups
            .entry(SignatureId::new("group"))
            .or_default()
            .insert(SignatureId::new("missing-entry"));

        let error = inventory
            .source_shape_digest(&cancellation())
            .expect_err("missing topology entry must be fallible");

        assert_eq!(
            error.to_string(),
            "signature inventory entry is missing: missing-entry"
        );
    }

    #[test]
    fn source_shape_digest_excludes_contract_context() {
        let group_id = SignatureId::new("answer");
        let mut first = SignatureInventory::default();
        first
            .insert(entry_in_group(
                "rust:function:answer",
                "answer",
                b"same API",
            ))
            .expect("first entry");
        first
            .set_group_context(
                group_id.clone(),
                SignatureGroupContext::new(
                    Some(b"syntax extraction A".to_vec()),
                    Some(b"document metadata A".to_vec()),
                ),
            )
            .expect("first context");
        let (stored_label, stored_group) = first.groups.first_key_value().expect("first group");
        let digest_view =
            super::InventoryGroupDigest::new(stored_group, &first.entries, &cancellation())
                .expect("borrowed digest view");
        let borrowed_extraction = digest_view.extraction_context.expect("borrowed context");
        let context = stored_group.context.as_ref().expect("stored context");
        let stored_extraction = context.extraction().expect("stored extraction context");
        assert!(std::ptr::eq(borrowed_extraction, stored_extraction));
        let semantic_index = first
            .semantic_groups(&cancellation())
            .expect("borrowed semantic index");
        let borrowed_label = semantic_index
            .groups
            .keys()
            .next()
            .copied()
            .expect("semantic label");
        assert!(std::ptr::eq(borrowed_label, stored_label.semantic_str()));

        let mut second = SignatureInventory::default();
        second
            .insert(entry_in_group(
                "rust:function:answer",
                "answer",
                b"same API",
            ))
            .expect("second entry");
        second
            .set_group_context(
                group_id,
                SignatureGroupContext::new(
                    Some(b"syntax extraction B".to_vec()),
                    Some(b"document metadata B".to_vec()),
                ),
            )
            .expect("second context");

        assert_eq!(
            first
                .source_shape_digest(&cancellation())
                .expect("first source-shape digest"),
            second
                .source_shape_digest(&cancellation())
                .expect("second source-shape digest")
        );
        assert_ne!(
            first
                .contract_digest(&cancellation())
                .expect("first contract digest"),
            second
                .contract_digest(&cancellation())
                .expect("second contract digest")
        );
        assert!(
            first
                .compare_against(
                    &second,
                    &crate::limits::DiagnosticLimits::default(),
                    &crate::work::CancellationProbe::new(),
                )
                .expect("source-shape comparison")
                .diagnostics()
                .is_empty()
        );
    }

    #[test]
    fn contract_diff_classifies_each_context_facet() {
        let group_id = SignatureId::new("answer");
        let mut previous = SignatureInventory::default();
        previous
            .insert(entry_in_group(
                "rust:function:answer",
                "answer",
                b"same API",
            ))
            .expect("previous entry");
        previous
            .set_group_context(
                group_id.clone(),
                SignatureGroupContext::new(
                    Some(b"syntax extraction A".to_vec()),
                    Some(b"document metadata A".to_vec()),
                ),
            )
            .expect("previous context");

        let mut current = SignatureInventory::default();
        current
            .insert(entry_in_group(
                "rust:function:answer",
                "answer",
                b"same API",
            ))
            .expect("current entry");
        current
            .set_group_context(
                group_id,
                SignatureGroupContext::new(
                    Some(b"syntax extraction B".to_vec()),
                    Some(b"document metadata B".to_vec()),
                ),
            )
            .expect("current context");

        let diff = current
            .diff_against(
                &previous,
                &crate::limits::DiagnosticLimits::default(),
                &crate::work::CancellationProbe::new(),
            )
            .expect("contract diff");
        let [InventoryDiffEntry::Changed { categories, .. }] = diff.entries() else {
            panic!("expected one changed contract group: {:?}", diff.entries());
        };

        assert_eq!(
            categories,
            &[
                InventoryChangeCategory::ExtractionContext,
                InventoryChangeCategory::DocumentMetadata,
            ]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn label_only_change_is_reported_as_a_label_change() {
        let mut previous = SignatureInventory::default();
        previous
            .insert(entry_in_group(
                "rust:function:answer",
                "old_label",
                b"same API",
            ))
            .expect("previous entry");

        let mut current = SignatureInventory::default();
        current
            .insert(entry_in_group(
                "rust:function:answer",
                "new_label",
                b"same API",
            ))
            .expect("current entry");

        assert_eq!(
            current
                .source_shape_digest(&cancellation())
                .expect("current source shape"),
            previous
                .source_shape_digest(&cancellation())
                .expect("previous source shape")
        );
        assert_ne!(
            current
                .contract_digest(&cancellation())
                .expect("current contract identity"),
            previous
                .contract_digest(&cancellation())
                .expect("previous contract identity")
        );
        let diff = current
            .diff_against(
                &previous,
                &crate::limits::DiagnosticLimits::default(),
                &crate::work::CancellationProbe::new(),
            )
            .expect("contract diff");
        assert_eq!(diff.entries().len(), 2);
        assert!(diff.entries().iter().all(|entry| matches!(
            entry,
            InventoryDiffEntry::Added { categories, .. }
                | InventoryDiffEntry::Removed { categories, .. }
                | InventoryDiffEntry::Changed { categories, .. }
                if categories == &BTreeSet::from([InventoryChangeCategory::Labels])
        )));

        let cancellation = crate::work::CancellationProbe::new();
        cancellation.cancel();
        let error = current
            .diff_against(
                &previous,
                &crate::limits::DiagnosticLimits::default(),
                &cancellation,
            )
            .expect_err("canceled many-shape indexing must stop");
        assert!(error.is_operation_canceled());
    }

    #[test]
    fn many_shapes_under_one_renamed_label_use_the_composite_peer_index() {
        let mut previous = SignatureInventory::default();
        let mut current = SignatureInventory::default();
        const SHAPE_COUNT: usize = 256;

        for index in 0..SHAPE_COUNT {
            let scope = format!("document-{index}");
            let structural = format!("rust:function:item_{index}");
            let canonical = format!("API shape {index}");
            previous
                .insert(SignatureEntry::from_grouped_canonical_bytes(
                    SignatureId::scoped(Some(&scope), &structural),
                    SignatureId::scoped(Some(&scope), "previous_label"),
                    canonical.as_bytes(),
                ))
                .expect("previous shape");
            current
                .insert(SignatureEntry::from_grouped_canonical_bytes(
                    SignatureId::scoped(Some(&scope), &structural),
                    SignatureId::scoped(Some(&scope), "current_label"),
                    canonical.as_bytes(),
                ))
                .expect("current shape");
        }

        let diff = current
            .diff_against(
                &previous,
                &crate::limits::DiagnosticLimits::default(),
                &crate::work::CancellationProbe::new(),
            )
            .expect("many-shape label diff");

        assert_eq!(diff.entries().len(), SHAPE_COUNT * 2);
        assert!(diff.entries().iter().all(|entry| matches!(
            entry,
            InventoryDiffEntry::Added { categories, .. }
                | InventoryDiffEntry::Removed { categories, .. }
                | InventoryDiffEntry::Changed { categories, .. }
                if categories == &BTreeSet::from([InventoryChangeCategory::Labels])
        )));
    }

    #[test]
    fn adding_a_duplicate_shape_does_not_misreport_a_retained_label_as_renamed() {
        let mut previous = SignatureInventory::default();
        previous
            .insert(SignatureEntry::from_grouped_canonical_bytes(
                SignatureId::scoped(Some("first.yml::document:0"), "rust:function:answer"),
                SignatureId::scoped(Some("first.yml::document:0"), "retained_label"),
                b"same API",
            ))
            .expect("previous entry");
        let mut current = previous.clone();
        current
            .insert(SignatureEntry::from_grouped_canonical_bytes(
                SignatureId::scoped(Some("second.yml::document:0"), "rust:function:answer"),
                SignatureId::scoped(Some("second.yml::document:0"), "added_label"),
                b"same API",
            ))
            .expect("duplicate-shape entry");

        let diff = current
            .diff_against(
                &previous,
                &crate::limits::DiagnosticLimits::default(),
                &crate::work::CancellationProbe::new(),
            )
            .expect("contract diff");
        let [InventoryDiffEntry::Added { categories, .. }] = diff.entries() else {
            panic!("expected one added contract group: {:?}", diff.entries());
        };
        assert_eq!(
            categories,
            &[InventoryChangeCategory::SourceSemantics]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn duplicate_label_diff_preserves_an_exact_peer_before_pairing_residuals() {
        let mut previous = SignatureInventory::default();
        previous
            .insert(SignatureEntry::from_grouped_canonical_bytes(
                SignatureId::scoped(Some("first.yml::document:0"), "rust:function:answer"),
                SignatureId::scoped(Some("first.yml::document:0"), "shared_label"),
                b"retained API",
            ))
            .expect("retained previous entry");
        previous
            .insert(SignatureEntry::from_grouped_canonical_bytes(
                SignatureId::scoped(Some("second.yml::document:0"), "rust:function:answer"),
                SignatureId::scoped(Some("second.yml::document:0"), "shared_label"),
                b"previous API",
            ))
            .expect("changed previous entry");

        let mut current = SignatureInventory::default();
        current
            .insert(SignatureEntry::from_grouped_canonical_bytes(
                SignatureId::scoped(Some("first.yml::document:0"), "rust:function:answer"),
                SignatureId::scoped(Some("first.yml::document:0"), "shared_label"),
                b"retained API",
            ))
            .expect("retained current entry");
        current
            .insert(SignatureEntry::from_grouped_canonical_bytes(
                SignatureId::scoped(Some("third.yml::document:0"), "rust:function:answer"),
                SignatureId::scoped(Some("third.yml::document:0"), "shared_label"),
                b"current API",
            ))
            .expect("changed current entry");

        let diff = current
            .diff_against(
                &previous,
                &crate::limits::DiagnosticLimits::default(),
                &crate::work::CancellationProbe::new(),
            )
            .expect("contract diff");

        let [InventoryDiffEntry::Changed { categories, .. }] = diff.entries() else {
            panic!(
                "one exact peer must be retained before residual pairing: {:?}",
                diff.entries()
            );
        };
        assert_eq!(
            categories,
            &[InventoryChangeCategory::SourceSemantics]
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn relabeled_duplicate_shapes_pair_exact_contexts_before_reporting_context_changes() {
        let mut previous = SignatureInventory::default();
        for (document, label, extraction) in [
            ("old-a.yml::document:0", "old_a", b"context A".as_slice()),
            ("old-b.yml::document:0", "old_b", b"context B".as_slice()),
        ] {
            let group_id = SignatureId::scoped(Some(document), label);
            previous
                .insert(SignatureEntry::from_grouped_canonical_bytes(
                    SignatureId::scoped(Some(document), "rust:function:answer"),
                    group_id.clone(),
                    b"same API",
                ))
                .expect("previous duplicate shape");
            previous
                .set_group_context(
                    group_id,
                    SignatureGroupContext::new(Some(extraction.to_vec()), None),
                )
                .expect("previous context");
        }

        let mut current = SignatureInventory::default();
        for (document, label, extraction) in [
            ("new-a.yml::document:0", "new_a", b"context A".as_slice()),
            ("new-b.yml::document:0", "new_b", b"context B".as_slice()),
        ] {
            let group_id = SignatureId::scoped(Some(document), label);
            current
                .insert(SignatureEntry::from_grouped_canonical_bytes(
                    SignatureId::scoped(Some(document), "rust:function:answer"),
                    group_id.clone(),
                    b"same API",
                ))
                .expect("current duplicate shape");
            current
                .set_group_context(
                    group_id,
                    SignatureGroupContext::new(Some(extraction.to_vec()), None),
                )
                .expect("current context");
        }

        let diff = current
            .diff_against(
                &previous,
                &crate::limits::DiagnosticLimits::default(),
                &crate::work::CancellationProbe::new(),
            )
            .expect("duplicate-shape label diff");

        assert_eq!(diff.entries().len(), 4);
        assert!(diff.entries().iter().all(|entry| matches!(
            entry,
            InventoryDiffEntry::Added { categories, .. }
                | InventoryDiffEntry::Removed { categories, .. }
                | InventoryDiffEntry::Changed { categories, .. }
                if categories == &BTreeSet::from([InventoryChangeCategory::Labels])
        )));
    }

    #[test]
    fn duplicate_insert_with_same_digest_is_idempotent() {
        let entry = entry("rust:function:a", b"a");
        let mut inventory = SignatureInventory::default();

        inventory.insert(entry.clone()).expect("first insert");
        inventory.insert(entry).expect("duplicate insert");

        assert_eq!(inventory.len(), 1);
    }

    #[test]
    fn duplicate_insert_with_different_digest_fails() {
        let mut inventory = SignatureInventory::default();
        inventory
            .insert(entry("rust:function:a", b"a"))
            .expect("first insert");

        let error = inventory
            .insert(entry("rust:function:a", b"changed"))
            .expect_err("duplicate mismatch");

        assert_eq!(
            error.to_string(),
            "signature digest mismatch for duplicate id: rust:function:a"
        );
    }

    #[test]
    fn duplicate_insert_with_different_group_fails() {
        let mut inventory = SignatureInventory::default();
        inventory
            .insert(entry_in_group("rust:function:a", "first_label", b"a"))
            .expect("first insert");

        let error = inventory
            .insert(entry_in_group("rust:function:a", "second_label", b"a"))
            .expect_err("one structural signature cannot belong to two groups");

        assert_eq!(
            error.to_string(),
            "signature id rust:function:a is assigned to multiple groups: first_label and second_label"
        );
    }

    #[test]
    fn merge_all_is_deterministic() {
        let left = InventoryFixture::new("rust:function:b", b"b").into_inventory();
        let right = InventoryFixture::new("rust:function:a", b"a").into_inventory();

        let inventory =
            SignatureInventory::merge_all(vec![left, right], &cancellation()).expect("merge");
        let ids = inventory
            .entries
            .values()
            .map(|entry| entry.id().as_str().to_owned())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["rust:function:a", "rust:function:b"]);
    }

    #[test]
    fn group_and_contract_digests_are_independent_of_insertion_order() {
        let group_id = SignatureId::new("group");
        let mut forward = SignatureInventory::default();
        forward
            .insert(entry_in_group("rust:function:a", group_id.as_str(), b"a"))
            .expect("forward a");
        forward
            .insert(entry_in_group("rust:function:b", group_id.as_str(), b"b"))
            .expect("forward b");
        forward
            .set_group_context(
                group_id.clone(),
                SignatureGroupContext::new(Some(b"context".to_vec()), None),
            )
            .expect("forward context");

        let mut reverse = SignatureInventory::default();
        reverse
            .insert(entry_in_group("rust:function:b", group_id.as_str(), b"b"))
            .expect("reverse b");
        reverse
            .insert(entry_in_group("rust:function:a", group_id.as_str(), b"a"))
            .expect("reverse a");
        reverse
            .set_group_context(
                group_id.clone(),
                SignatureGroupContext::new(Some(b"context".to_vec()), None),
            )
            .expect("reverse context");

        assert_eq!(forward, reverse);
        assert_eq!(
            forward
                .group_contract_digest(&group_id, &cancellation())
                .expect("forward group digest"),
            reverse
                .group_contract_digest(&group_id, &cancellation())
                .expect("reverse group digest")
        );
        assert_eq!(
            forward
                .contract_digest(&cancellation())
                .expect("forward contract digest"),
            reverse
                .contract_digest(&cancellation())
                .expect("reverse contract digest")
        );
    }

    #[test]
    fn modeled_item_and_attribute_mutations_are_isolated_to_their_own_group() {
        let fixture = MutationIsolationFixture::new();
        let baseline = br#"{"item":"function","name":"target","return":"u8","attributes":[]}"#;
        for (case, mutated) in [
            (
                "function return type",
                br#"{"item":"function","name":"target","return":"u16","attributes":[]}"#.as_slice(),
            ),
            (
                "aggregate field",
                br#"{"item":"struct","name":"target","fields":[["value","u8"]],"attributes":[]}"#.as_slice(),
            ),
            (
                "trait associated item",
                br#"{"item":"trait","name":"target","items":[{"type":"Output","default":"u8"}],"attributes":[]}"#.as_slice(),
            ),
            (
                "representation attribute",
                br#"{"item":"function","name":"target","return":"u8","attributes":[{"repr":["C"]}]}"#.as_slice(),
            ),
            (
                "non-exhaustive attribute",
                br#"{"item":"function","name":"target","return":"u8","attributes":["non_exhaustive"]}"#.as_slice(),
            ),
            (
                "must-use attribute",
                br#"{"item":"function","name":"target","return":"u8","attributes":[{"must_use":"consume"}]}"#.as_slice(),
            ),
        ] {
            let previous = fixture.inventory(baseline);
            let current = fixture.inventory(mutated);
            let previous_target = previous
                .entries
                .get(&fixture.target_entry)
                .expect("previous target entry");
            let current_target = current
                .entries
                .get(&fixture.target_entry)
                .expect("current target entry");
            let previous_unrelated = previous
                .entries
                .get(&fixture.unrelated_entry)
                .expect("previous unrelated entry");
            let current_unrelated = current
                .entries
                .get(&fixture.unrelated_entry)
                .expect("current unrelated entry");

            assert_ne!(
                current_target.digest(),
                previous_target.digest(),
                "{case} must change its modeled entry digest",
            );
            assert_eq!(
                current_unrelated.digest(),
                previous_unrelated.digest(),
                "{case} must not change an unrelated entry digest",
            );
            assert_ne!(
                current
                    .group_source_shape_digest(&fixture.target_group, &cancellation())
                    .expect("current target group digest"),
                previous
                    .group_source_shape_digest(&fixture.target_group, &cancellation())
                    .expect("previous target group digest"),
                "{case} must change its owning group digest",
            );
            assert_eq!(
                current
                    .group_source_shape_digest(&fixture.unrelated_group, &cancellation())
                    .expect("current unrelated group digest"),
                previous
                    .group_source_shape_digest(&fixture.unrelated_group, &cancellation())
                    .expect("previous unrelated group digest"),
                "{case} must not contaminate an unrelated group digest",
            );

            let comparison = current
                .compare_against(
                    &previous,
                    &crate::limits::DiagnosticLimits::default(),
                    &cancellation(),
                )
                .expect("isolated mutation comparison");
            assert!(matches!(
                comparison.diagnostics(),
                [InventoryDiagnostic::Mismatched { signature_id, .. }]
                    if signature_id == &fixture.target_group
            ));
        }
    }

    #[test]
    fn group_context_is_excluded_from_check_and_included_in_contract_diff() {
        let group_id = SignatureId::new("shared_contract");
        let mut current = SignatureInventory::default();
        current
            .insert(entry_in_group("rust:function:a", group_id.as_str(), b"a"))
            .expect("current entry");
        current
            .set_group_context(
                group_id.clone(),
                SignatureGroupContext::new(Some(b"current".to_vec()), None),
            )
            .expect("current context");

        let mut previous = SignatureInventory::default();
        previous
            .insert(entry_in_group("rust:function:a", group_id.as_str(), b"a"))
            .expect("previous entry");
        previous
            .set_group_context(
                group_id.clone(),
                SignatureGroupContext::new(Some(b"previous".to_vec()), None),
            )
            .expect("previous context");

        assert_eq!(
            current
                .source_shape_digest(&cancellation())
                .expect("current source digest"),
            previous
                .source_shape_digest(&cancellation())
                .expect("previous source digest")
        );
        assert_ne!(
            current
                .contract_digest(&cancellation())
                .expect("current contract digest"),
            previous
                .contract_digest(&cancellation())
                .expect("previous contract digest")
        );
        assert!(
            current
                .compare_against(
                    &previous,
                    &crate::limits::DiagnosticLimits::default(),
                    &crate::work::CancellationProbe::new(),
                )
                .expect("compare")
                .diagnostics()
                .is_empty()
        );
        let current_digest = current
            .group_contract_digest(&group_id, &cancellation())
            .expect("current group digest");
        let previous_digest = previous
            .group_contract_digest(&group_id, &cancellation())
            .expect("previous group digest");
        assert_eq!(
            current
                .diff_against(
                    &previous,
                    &crate::limits::DiagnosticLimits::default(),
                    &crate::work::CancellationProbe::new(),
                )
                .expect("diff")
                .entries(),
            &[InventoryDiffEntry::Changed {
                signature_id: group_id,
                current_digest,
                previous_digest,
                categories: [InventoryChangeCategory::ExtractionContext]
                    .into_iter()
                    .collect(),
            }]
        );
    }

    #[test]
    fn mismatched_digest_uses_expected_group_topology() {
        let mut actual = SignatureInventory::default();
        actual
            .insert(entry_in_group(
                "rust:function:a",
                "rust:function:a",
                b"actual-a",
            ))
            .expect("actual a");
        actual
            .insert(entry_in_group(
                "rust:function:b",
                "rust:function:b",
                b"same-b",
            ))
            .expect("actual b");

        let mut expected = SignatureInventory::default();
        expected
            .insert(entry_in_group("rust:function:b", "label", b"same-b"))
            .expect("expected b");
        expected
            .insert(entry_in_group("rust:function:a", "label", b"expected-a"))
            .expect("expected a");
        expected
            .set_group_context(
                SignatureId::new("label"),
                SignatureGroupContext::new(Some(b"contract".to_vec()), None),
            )
            .expect("expected context");

        let comparison = actual
            .compare_against(
                &expected,
                &crate::limits::DiagnosticLimits::default(),
                &crate::work::CancellationProbe::new(),
            )
            .expect("comparison");
        let [
            InventoryDiagnostic::Mismatched {
                signature_id,
                expected_digest,
                actual_digest,
            },
        ] = comparison.diagnostics()
        else {
            panic!(
                "expected one mismatched diagnostic: {:?}",
                comparison.diagnostics()
            );
        };

        assert_eq!(signature_id.as_str(), "label");
        assert_ne!(expected_digest, actual_digest);
        assert_eq!(expected_digest.render().len(), 64);
        assert_eq!(actual_digest.render().len(), 64);
    }

    #[test]
    fn compare_reports_missing_extra_and_mismatched_entries() {
        let source = SignatureInventory::merge_all(
            vec![
                InventoryFixture::new("rust:function:changed", b"source").into_inventory(),
                InventoryFixture::new("rust:function:extra", b"extra").into_inventory(),
            ],
            &cancellation(),
        )
        .expect("source");
        let expected = SignatureInventory::merge_all(
            vec![
                InventoryFixture::new("rust:function:changed", b"expected").into_inventory(),
                InventoryFixture::new("rust:function:missing", b"missing").into_inventory(),
            ],
            &cancellation(),
        )
        .expect("expected");

        let comparison = source
            .compare_against(
                &expected,
                &crate::limits::DiagnosticLimits::default(),
                &crate::work::CancellationProbe::new(),
            )
            .expect("compare");

        assert_eq!(comparison.source_signature_count(), 2);
        assert_eq!(comparison.contract_signature_count(), 2);
        assert_eq!(comparison.diagnostics().len(), 3);
        assert!(
            comparison
                .diagnostics()
                .iter()
                .any(|diagnostic| matches!(diagnostic, InventoryDiagnostic::Missing { .. }))
        );
        assert!(
            comparison
                .diagnostics()
                .iter()
                .any(|diagnostic| matches!(diagnostic, InventoryDiagnostic::Extra { .. }))
        );
        assert!(
            comparison
                .diagnostics()
                .iter()
                .any(|diagnostic| { matches!(diagnostic, InventoryDiagnostic::Mismatched { .. }) })
        );
    }

    #[test]
    fn diff_reports_added_removed_and_changed_entries() {
        let current = SignatureInventory::merge_all(
            vec![
                InventoryFixture::new("rust:function:added", b"added").into_inventory(),
                InventoryFixture::new("rust:function:changed", b"current").into_inventory(),
            ],
            &cancellation(),
        )
        .expect("current");
        let previous = SignatureInventory::merge_all(
            vec![
                InventoryFixture::new("rust:function:removed", b"removed").into_inventory(),
                InventoryFixture::new("rust:function:changed", b"previous").into_inventory(),
            ],
            &cancellation(),
        )
        .expect("previous");

        let diff = current
            .diff_against(
                &previous,
                &crate::limits::DiagnosticLimits::default(),
                &crate::work::CancellationProbe::new(),
            )
            .expect("diff");

        assert!(!diff.entries().is_empty());
        assert_eq!(diff.entries().len(), 3);
        assert!(
            diff.entries()
                .iter()
                .any(|entry| matches!(entry, InventoryDiffEntry::Added { .. }))
        );
        assert!(
            diff.entries()
                .iter()
                .any(|entry| matches!(entry, InventoryDiffEntry::Removed { .. }))
        );
        assert!(
            diff.entries()
                .iter()
                .any(|entry| matches!(entry, InventoryDiffEntry::Changed { .. }))
        );
    }

    #[test]
    fn document_locator_scope_is_excluded_from_source_shape_and_contract_diff() {
        let mut current = SignatureInventory::default();
        let current_group = SignatureId::scoped(Some("current.yml::document:4"), "answer");
        let current_entry = SignatureId::scoped(
            Some("current.yml::document:4"),
            "rust:lib.rs:function:answer",
        );
        current
            .insert(SignatureEntry::from_grouped_canonical_bytes(
                current_entry,
                current_group.clone(),
                b"same API",
            ))
            .expect("current entry");
        current
            .set_group_context(
                current_group,
                SignatureGroupContext::new(
                    Some(b"same extraction".to_vec()),
                    Some(b"same metadata".to_vec()),
                ),
            )
            .expect("current context");

        let mut previous = SignatureInventory::default();
        let previous_group = SignatureId::scoped(Some("previous.yml::document:0"), "answer");
        let previous_entry = SignatureId::scoped(
            Some("previous.yml::document:0"),
            "rust:lib.rs:function:answer",
        );
        previous
            .insert(SignatureEntry::from_grouped_canonical_bytes(
                previous_entry,
                previous_group.clone(),
                b"same API",
            ))
            .expect("previous entry");
        previous
            .set_group_context(
                previous_group,
                SignatureGroupContext::new(
                    Some(b"same extraction".to_vec()),
                    Some(b"same metadata".to_vec()),
                ),
            )
            .expect("previous context");

        assert_eq!(
            current
                .source_shape_digest(&cancellation())
                .expect("current source digest"),
            previous
                .source_shape_digest(&cancellation())
                .expect("previous source digest")
        );
        assert_eq!(
            current
                .contract_digest(&cancellation())
                .expect("current contract digest"),
            previous
                .contract_digest(&cancellation())
                .expect("previous contract digest")
        );
        assert!(
            current
                .diff_against(
                    &previous,
                    &crate::limits::DiagnosticLimits::default(),
                    &crate::work::CancellationProbe::new(),
                )
                .expect("diff")
                .entries()
                .is_empty()
        );
    }

    proptest! {
        #[test]
        fn inventory_digests_are_independent_of_entry_insertion_order(
            values in prop::collection::btree_map(
                "[a-z]{1,24}",
                prop::collection::vec(any::<u8>(), 0..128),
                0..48,
            ),
        ) {
            let entries = values
                .into_iter()
                .map(|(name, bytes)| (format!("rust:function:{name}"), bytes))
                .collect::<Vec<_>>();
            let mut forward = SignatureInventory::default();
            let mut reverse = SignatureInventory::default();
            for (id, bytes) in &entries {
                forward
                    .insert(entry(id, bytes))
                    .expect("unique forward inventory entry");
            }
            for (id, bytes) in entries.iter().rev() {
                reverse
                    .insert(entry(id, bytes))
                    .expect("unique reverse inventory entry");
            }

            prop_assert_eq!(
                forward
                    .source_shape_digest(&cancellation())
                    .expect("forward source digest"),
                reverse
                    .source_shape_digest(&cancellation())
                    .expect("reverse source digest"),
            );
            prop_assert_eq!(
                forward
                    .contract_digest(&cancellation())
                    .expect("forward contract digest"),
                reverse
                    .contract_digest(&cancellation())
                    .expect("reverse contract digest"),
            );
        }
    }

    struct InventoryFixture {
        id: SignatureId,
        canonical_bytes: Vec<u8>,
    }

    struct MutationIsolationFixture {
        target_entry: SignatureId,
        target_group: SignatureId,
        unrelated_entry: SignatureId,
        unrelated_group: SignatureId,
    }

    impl MutationIsolationFixture {
        fn new() -> Self {
            Self {
                target_entry: SignatureId::new("rust:function:target"),
                target_group: SignatureId::new("target_group"),
                unrelated_entry: SignatureId::new("rust:struct:unrelated"),
                unrelated_group: SignatureId::new("unrelated_group"),
            }
        }

        fn inventory(&self, target_canonical_bytes: &[u8]) -> SignatureInventory {
            let mut inventory = SignatureInventory::default();
            inventory
                .insert(SignatureEntry::from_grouped_canonical_bytes(
                    self.target_entry.clone(),
                    self.target_group.clone(),
                    target_canonical_bytes,
                ))
                .expect("target fixture entry");
            inventory
                .insert(SignatureEntry::from_grouped_canonical_bytes(
                    self.unrelated_entry.clone(),
                    self.unrelated_group.clone(),
                    br#"{"item":"struct","name":"unrelated","fields":[]}"#,
                ))
                .expect("unrelated fixture entry");
            inventory
        }
    }

    impl InventoryFixture {
        fn new(id: &str, canonical_bytes: &[u8]) -> Self {
            Self {
                id: SignatureId::new(id),
                canonical_bytes: canonical_bytes.to_vec(),
            }
        }

        fn into_inventory(self) -> SignatureInventory {
            let mut inventory = SignatureInventory::default();
            inventory
                .insert(SignatureEntry::from_grouped_canonical_bytes(
                    self.id.clone(),
                    self.id,
                    &self.canonical_bytes,
                ))
                .expect("fixture insert");
            inventory
        }
    }

    fn entry(id: &str, canonical_bytes: &[u8]) -> SignatureEntry {
        let id = SignatureId::new(id);
        SignatureEntry::from_grouped_canonical_bytes(id.clone(), id, canonical_bytes)
    }

    fn entry_in_group(id: &str, group_id: &str, canonical_bytes: &[u8]) -> SignatureEntry {
        SignatureEntry::from_grouped_canonical_bytes(
            SignatureId::new(id),
            SignatureId::new(group_id),
            canonical_bytes,
        )
    }
}
