use crate::error::InventoryError;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct SignatureId {
    value: String,
}

impl SignatureId {
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.value
    }
}

impl fmt::Display for SignatureId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SignatureDigest {
    value: String,
}

impl SignatureDigest {
    pub(crate) fn from_canonical_bytes(bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        let mut value = String::with_capacity(64);

        for byte in digest {
            use std::fmt::Write;
            write!(&mut value, "{byte:02x}").expect("writing to String should not fail");
        }

        Self { value }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.value
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
            digest: SignatureDigest::from_canonical_bytes(canonical_bytes),
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct SignatureGroup {
    entry_ids: BTreeSet<SignatureId>,
    context: Vec<u8>,
}

impl SignatureGroup {
    fn insert(&mut self, id: SignatureId) {
        self.entry_ids.insert(id);
    }

    fn digest_from(&self, entries: &BTreeMap<SignatureId, SignatureEntry>) -> SignatureDigest {
        let canonical_entries = self
            .entry_ids
            .iter()
            .filter_map(|id| entries.get(id))
            .map(|entry| {
                (
                    entry.id().as_str().to_owned(),
                    entry.digest().as_str().to_owned(),
                )
            })
            .collect::<Vec<_>>();
        let bytes = serde_json::to_vec(&(canonical_entries, &self.context))
            .expect("neutral signature group should serialize");

        SignatureDigest::from_canonical_bytes(&bytes)
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
                    id: entry.id().clone(),
                    existing_group: existing.group_id().clone(),
                    incoming_group: entry.group_id().clone(),
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
        context: Vec<u8>,
    ) -> Result<(), InventoryError> {
        let group = self.groups.entry(group_id.clone()).or_default();
        if !group.context.is_empty() && group.context != context {
            return Err(InventoryError::DuplicateSignatureMismatch { id: group_id });
        }
        group.context = context;
        Ok(())
    }

    pub(crate) fn merge(&mut self, incoming: Self) -> Result<(), InventoryError> {
        let SignatureInventory { entries, groups } = incoming;
        for entry in entries.into_values() {
            self.insert(entry)?;
        }
        for (group_id, group) in groups {
            self.set_group_context(group_id, group.context)?;
        }

        Ok(())
    }

    pub(crate) fn merge_all(inventories: Vec<Self>) -> Result<Self, InventoryError> {
        let mut merged = Self::default();

        for inventory in inventories {
            merged.merge(inventory)?;
        }

        Ok(merged)
    }

    pub(crate) fn len(&self) -> usize {
        self.groups.len()
    }

    pub(crate) fn inventory_digest(&self) -> SignatureDigest {
        let entries = self
            .groups
            .keys()
            .map(|group_id| {
                (
                    group_id.as_str().to_owned(),
                    self.group_digest(group_id).as_str().to_owned(),
                )
            })
            .collect::<Vec<_>>();
        let bytes = serde_json::to_vec(&entries).expect("neutral inventory should serialize");

        SignatureDigest::from_canonical_bytes(&bytes)
    }

    pub(crate) fn compare_against(
        &self,
        expected: &Self,
    ) -> Result<InventoryComparison, InventoryError> {
        let mut diagnostics = BTreeMap::<SignatureId, InventoryDiagnostic>::new();

        for (id, expected_entry) in &expected.entries {
            match self.entries.get(id) {
                Some(actual_entry) if actual_entry.digest() == expected_entry.digest() => {}
                Some(_) => {
                    let group_id = expected_entry.group_id().clone();
                    diagnostics.insert(
                        group_id.clone(),
                        InventoryDiagnostic::Mismatched {
                            signature_id: group_id.clone(),
                            expected_digest: expected.group_digest(&group_id),
                            actual_digest: expected.groups.get(&group_id).map_or_else(
                                || SignatureDigest::from_canonical_bytes(&[]),
                                |group| group.digest_from(&self.entries),
                            ),
                        },
                    );
                }
                None => {
                    let group_id = expected_entry.group_id().clone();
                    diagnostics.insert(
                        group_id.clone(),
                        InventoryDiagnostic::Missing {
                            signature_id: group_id,
                        },
                    );
                }
            }
        }

        for id in self.entries.keys() {
            if !expected.entries.contains_key(id) {
                let entry = &self.entries[id];
                let signature_id = expected
                    .group_for_structural_peer(entry.group_id())
                    .unwrap_or_else(|| entry.group_id().clone());
                diagnostics
                    .entry(signature_id.clone())
                    .or_insert(InventoryDiagnostic::Extra { signature_id });
            }
        }

        Ok(InventoryComparison {
            source_signature_count: self.len(),
            contract_signature_count: expected.len(),
            inventory_digest: self.inventory_digest(),
            diagnostics: diagnostics.into_values().collect(),
        })
    }

    pub(crate) fn diff_against(&self, previous: &Self) -> Result<InventoryDiff, InventoryError> {
        let mut entries = Vec::new();

        for id in self.groups.keys() {
            let current_digest = self.group_digest(id);
            match previous.groups.get(id) {
                Some(_) if previous.group_digest(id) == current_digest => {}
                Some(_) => entries.push(InventoryDiffEntry::Changed {
                    signature_id: id.clone(),
                    current_digest,
                    previous_digest: previous.group_digest(id),
                }),
                None => entries.push(InventoryDiffEntry::Added {
                    signature_id: id.clone(),
                }),
            }
        }

        for id in previous.groups.keys() {
            if !self.groups.contains_key(id) {
                entries.push(InventoryDiffEntry::Removed {
                    signature_id: id.clone(),
                });
            }
        }

        Ok(InventoryDiff { entries })
    }

    fn group_digest(&self, group_id: &SignatureId) -> SignatureDigest {
        self.groups.get(group_id).map_or_else(
            || SignatureDigest::from_canonical_bytes(&[]),
            |group| group.digest_from(&self.entries),
        )
    }

    fn group_for_structural_peer(&self, structural_group: &SignatureId) -> Option<SignatureId> {
        self.entries
            .get(structural_group)
            .map(|entry| entry.group_id().clone())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InventoryComparison {
    source_signature_count: usize,
    contract_signature_count: usize,
    inventory_digest: SignatureDigest,
    diagnostics: Vec<InventoryDiagnostic>,
}

impl InventoryComparison {
    pub(crate) fn source_signature_count(&self) -> usize {
        self.source_signature_count
    }

    pub(crate) fn contract_signature_count(&self) -> usize {
        self.contract_signature_count
    }

    pub(crate) fn inventory_digest(&self) -> &SignatureDigest {
        &self.inventory_digest
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
    entries: Vec<InventoryDiffEntry>,
}

impl InventoryDiff {
    pub(crate) fn changed(&self) -> bool {
        !self.entries.is_empty()
    }

    pub(crate) fn entries(&self) -> &[InventoryDiffEntry] {
        &self.entries
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum InventoryDiffEntry {
    Added {
        signature_id: SignatureId,
    },
    Removed {
        signature_id: SignatureId,
    },
    Changed {
        signature_id: SignatureId,
        current_digest: SignatureDigest,
        previous_digest: SignatureDigest,
    },
}

#[cfg(test)]
mod tests {
    use super::{
        InventoryDiagnostic, InventoryDiffEntry, SignatureEntry, SignatureId, SignatureInventory,
    };

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

        let inventory = SignatureInventory::merge_all(vec![left, right]).expect("merge");
        let ids = inventory
            .entries
            .values()
            .map(|entry| entry.id().as_str().to_owned())
            .collect::<Vec<_>>();

        assert_eq!(ids, vec!["rust:function:a", "rust:function:b"]);
    }

    #[test]
    fn group_and_inventory_digests_are_independent_of_insertion_order() {
        let group_id = SignatureId::new("group");
        let mut forward = SignatureInventory::default();
        forward
            .insert(entry_in_group("rust:function:a", group_id.as_str(), b"a"))
            .expect("forward a");
        forward
            .insert(entry_in_group("rust:function:b", group_id.as_str(), b"b"))
            .expect("forward b");
        forward
            .set_group_context(group_id.clone(), b"context".to_vec())
            .expect("forward context");

        let mut reverse = SignatureInventory::default();
        reverse
            .insert(entry_in_group("rust:function:b", group_id.as_str(), b"b"))
            .expect("reverse b");
        reverse
            .insert(entry_in_group("rust:function:a", group_id.as_str(), b"a"))
            .expect("reverse a");
        reverse
            .set_group_context(group_id.clone(), b"context".to_vec())
            .expect("reverse context");

        assert_eq!(forward, reverse);
        assert_eq!(
            forward.group_digest(&group_id),
            reverse.group_digest(&group_id)
        );
        assert_eq!(forward.inventory_digest(), reverse.inventory_digest());
        assert_eq!(
            forward.group_digest(&group_id).as_str(),
            "279dbc2cb61ab6be3b323727a779274d278fdd23683dc660df7b0fade7bfff2e"
        );
        assert_eq!(
            forward.inventory_digest().as_str(),
            "18b16d813586b2c6630c0b01168ae163802cdd1db284ff2adb62abca67cb81d5"
        );
    }

    #[test]
    fn group_context_remains_part_of_compare_and_diff_semantics() {
        let group_id = SignatureId::new("shared_contract");
        let mut current = SignatureInventory::default();
        current
            .insert(entry_in_group("rust:function:a", group_id.as_str(), b"a"))
            .expect("current entry");
        current
            .set_group_context(group_id.clone(), b"current".to_vec())
            .expect("current context");

        let mut previous = SignatureInventory::default();
        previous
            .insert(entry_in_group("rust:function:a", group_id.as_str(), b"a"))
            .expect("previous entry");
        previous
            .set_group_context(group_id.clone(), b"previous".to_vec())
            .expect("previous context");

        assert_ne!(current.inventory_digest(), previous.inventory_digest());
        assert!(
            current
                .compare_against(&previous)
                .expect("compare")
                .diagnostics()
                .is_empty()
        );
        let current_digest = current.group_digest(&group_id);
        let previous_digest = previous.group_digest(&group_id);
        assert_eq!(
            current.diff_against(&previous).expect("diff").entries(),
            &[InventoryDiffEntry::Changed {
                signature_id: group_id,
                current_digest,
                previous_digest,
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
            .set_group_context(SignatureId::new("label"), b"contract".to_vec())
            .expect("expected context");

        let comparison = actual.compare_against(&expected).expect("comparison");
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
        assert_eq!(
            expected_digest.as_str(),
            "9efd4ab998c5903c85057a838531b0b55ef077e736209682772cd5d529d11f3e"
        );
        assert_eq!(
            actual_digest.as_str(),
            "0ff9c4b7b556befc7e1736a454faeff1650c4e7e1bcafa53945839901a3ece01"
        );
    }

    #[test]
    fn compare_reports_missing_extra_and_mismatched_entries() {
        let source = SignatureInventory::merge_all(vec![
            InventoryFixture::new("rust:function:changed", b"source").into_inventory(),
            InventoryFixture::new("rust:function:extra", b"extra").into_inventory(),
        ])
        .expect("source");
        let expected = SignatureInventory::merge_all(vec![
            InventoryFixture::new("rust:function:changed", b"expected").into_inventory(),
            InventoryFixture::new("rust:function:missing", b"missing").into_inventory(),
        ])
        .expect("expected");

        let comparison = source.compare_against(&expected).expect("compare");

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
        let current = SignatureInventory::merge_all(vec![
            InventoryFixture::new("rust:function:added", b"added").into_inventory(),
            InventoryFixture::new("rust:function:changed", b"current").into_inventory(),
        ])
        .expect("current");
        let previous = SignatureInventory::merge_all(vec![
            InventoryFixture::new("rust:function:removed", b"removed").into_inventory(),
            InventoryFixture::new("rust:function:changed", b"previous").into_inventory(),
        ])
        .expect("previous");

        let diff = current.diff_against(&previous).expect("diff");

        assert!(diff.changed());
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

    struct InventoryFixture {
        id: SignatureId,
        canonical_bytes: Vec<u8>,
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
