use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::Deserialize;
use walkdir::WalkDir;

use super::error::StepError;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TreeContents {
    Text,
    Bytes,
}

pub(super) struct TreeSnapshot {
    entries: BTreeMap<String, TreeEntry>,
}

struct TreeEntry {
    kind: TreeEntryKind,
    bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TreeEntryKind {
    Directory,
    File,
}

impl TreeSnapshot {
    pub(super) fn read(root: &Path) -> Result<Self, StepError> {
        let root_metadata = fs::symlink_metadata(root).map_err(|source| StepError::Inspect {
            path: root.to_path_buf(),
            source,
        })?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err(StepError::TreeRoot {
                path: root.to_path_buf(),
            });
        }

        let mut entries = BTreeMap::new();
        let walker = WalkDir::new(root).follow_links(false).sort_by_file_name();
        for entry in walker {
            let entry = entry.map_err(|error| StepError::Walk {
                path: root.to_path_buf(),
                message: error.to_string(),
            })?;
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("walked tree entry is below tree root");
            if relative.as_os_str().is_empty() {
                continue;
            }
            let Some(relative) = relative.to_str() else {
                return Err(StepError::InvalidPath {
                    value: relative.to_string_lossy().into_owned(),
                    message: "tree entry path is not valid UTF-8".to_owned(),
                });
            };
            let relative = relative.replace('\\', "/");
            let file_type = entry.file_type();
            let tree_entry = if file_type.is_dir() {
                TreeEntry {
                    kind: TreeEntryKind::Directory,
                    bytes: Vec::new(),
                }
            } else if file_type.is_file() {
                let bytes = fs::read(path).map_err(|source| StepError::Read {
                    path: path.to_path_buf(),
                    source,
                })?;
                TreeEntry {
                    kind: TreeEntryKind::File,
                    bytes,
                }
            } else {
                return Err(StepError::UnsupportedEntry {
                    path: path.to_path_buf(),
                });
            };
            entries.insert(relative, tree_entry);
        }
        Ok(Self { entries })
    }

    pub(super) fn assert_matches(
        &self,
        expected: &Self,
        contents: TreeContents,
    ) -> Result<(), StepError> {
        let mut paths = BTreeSet::new();
        paths.extend(self.entries.keys().cloned());
        paths.extend(expected.entries.keys().cloned());
        let mut mismatches = Vec::new();

        for path in paths {
            let actual_entry = self.entries.get(&path);
            let expected_entry = expected.entries.get(&path);
            match (actual_entry, expected_entry) {
                (None, Some(_)) => mismatches.push(format!("{path}: missing")),
                (Some(_), None) => mismatches.push(format!("{path}: unexpected")),
                (Some(actual), Some(expected)) if actual.kind != expected.kind => {
                    mismatches.push(format!(
                        "{path}: type mismatch (expected {:?}, got {:?})",
                        expected.kind, actual.kind
                    ));
                }
                (Some(actual), Some(expected)) if actual.kind == TreeEntryKind::File => {
                    match contents {
                        TreeContents::Bytes if actual.bytes != expected.bytes => {
                            mismatches.push(format!("{path}: content mismatch"));
                        }
                        TreeContents::Bytes => {}
                        TreeContents::Text => {
                            let actual_text = match std::str::from_utf8(&actual.bytes) {
                                Ok(text) => Some(text),
                                Err(_) => {
                                    mismatches
                                        .push(format!("{path}: actual text is not valid UTF-8"));
                                    None
                                }
                            };
                            let expected_text = match std::str::from_utf8(&expected.bytes) {
                                Ok(text) => Some(text),
                                Err(_) => {
                                    mismatches
                                        .push(format!("{path}: expected text is not valid UTF-8"));
                                    None
                                }
                            };
                            if let (Some(actual_text), Some(expected_text)) =
                                (actual_text, expected_text)
                            {
                                let normalized_expected = expected_text.replace("\r\n", "\n");
                                if actual_text != normalized_expected.as_str() {
                                    mismatches.push(format!("{path}: content mismatch"));
                                }
                            }
                        }
                    }
                }
                (Some(_), Some(_)) => {}
                (None, None) => unreachable!("path came from at least one tree"),
            }
        }

        if mismatches.is_empty() {
            Ok(())
        } else {
            Err(StepError::TreeMismatch {
                details: mismatches.join("\n"),
            })
        }
    }
}
