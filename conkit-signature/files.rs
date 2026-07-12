use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fmt;

/// An ordered in-memory catalog of logical paths to byte contents.
///
/// `FileCatalog` is the crate boundary for source files, contract files,
/// generated reports, and previous contract catalogs. It keeps entries sorted by
/// [`CatalogPath`] and rejects duplicate paths instead of overwriting bytes.
///
/// # Examples
///
/// ```
/// use conkit_signature::{CatalogPath, FileCatalog};
///
/// let mut catalog = FileCatalog::new();
/// let lib = CatalogPath::new("src/lib.rs")?;
/// let module = CatalogPath::new("src/a.rs")?;
///
/// catalog.insert(lib.clone(), b"pub fn answer() -> u8 { 42 }\n".to_vec())?;
/// catalog.insert(module.clone(), b"pub fn helper() {}\n".to_vec())?;
///
/// assert_eq!(catalog.get(&lib), Some(&b"pub fn answer() -> u8 { 42 }\n"[..]));
/// assert_eq!(
///     catalog.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(),
///     ["src/a.rs", "src/lib.rs"],
/// );
///
/// let entries = catalog
///     .into_entries()
///     .map(|(path, bytes)| (path.to_string(), bytes))
///     .collect::<Vec<_>>();
/// assert_eq!(entries.len(), 2);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileCatalog {
    files: BTreeMap<CatalogPath, Vec<u8>>,
}

impl FileCatalog {
    /// Creates an empty catalog.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_signature::FileCatalog;
    ///
    /// let catalog = FileCatalog::new();
    /// assert!(catalog.is_empty());
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts one logical path and its bytes.
    ///
    /// Returns [`FileCatalogError::DuplicatePath`] when the path already
    /// exists in the catalog.
    ///
    /// # Errors
    ///
    /// Returns an error if `path` duplicates an existing catalog entry.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_signature::{CatalogPath, FileCatalog, FileCatalogError};
    ///
    /// let path = CatalogPath::new("src/lib.rs")?;
    /// let mut catalog = FileCatalog::new();
    ///
    /// catalog.insert(path.clone(), b"first".to_vec())?;
    /// let error = catalog.insert(path.clone(), b"second".to_vec()).unwrap_err();
    ///
    /// assert_eq!(error, FileCatalogError::DuplicatePath { path });
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn insert(&mut self, path: CatalogPath, contents: Vec<u8>) -> Result<(), FileCatalogError> {
        match self.files.entry(path) {
            Entry::Vacant(entry) => {
                entry.insert(contents);
                Ok(())
            }
            Entry::Occupied(entry) => Err(FileCatalogError::duplicate(entry.key())),
        }
    }

    /// Returns the bytes for `path`, if that logical path is present.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_signature::{CatalogPath, FileCatalog};
    ///
    /// let path = CatalogPath::new("src/lib.rs")?;
    /// let mut catalog = FileCatalog::new();
    /// catalog.insert(path.clone(), b"contents".to_vec())?;
    ///
    /// assert_eq!(catalog.get(&path), Some(&b"contents"[..]));
    /// assert_eq!(catalog.get(&CatalogPath::new("src/missing.rs")?), None);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn get(&self, path: &CatalogPath) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }

    /// Iterates catalog entries in deterministic logical path order.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_signature::{CatalogPath, FileCatalog};
    ///
    /// let mut catalog = FileCatalog::new();
    /// catalog.insert(CatalogPath::new("src/z.rs")?, Vec::new())?;
    /// catalog.insert(CatalogPath::new("src/a.rs")?, Vec::new())?;
    ///
    /// let paths = catalog
    ///     .iter()
    ///     .map(|(path, _)| path.as_str())
    ///     .collect::<Vec<_>>();
    ///
    /// assert_eq!(paths, ["src/a.rs", "src/z.rs"]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = (&CatalogPath, &[u8])> {
        self.files
            .iter()
            .map(|(path, bytes)| (path, bytes.as_slice()))
    }

    /// Consumes the catalog and returns owned path and byte entries.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_signature::{CatalogPath, FileCatalog};
    ///
    /// let mut catalog = FileCatalog::new();
    /// catalog.insert(CatalogPath::new("src/lib.rs")?, b"contents".to_vec())?;
    ///
    /// let entries = catalog.into_entries().collect::<Vec<_>>();
    /// assert_eq!(entries[0].0.as_str(), "src/lib.rs");
    /// assert_eq!(entries[0].1, b"contents");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn into_entries(self) -> impl Iterator<Item = (CatalogPath, Vec<u8>)> {
        self.files.into_iter()
    }

    /// Returns the number of catalog entries.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_signature::{CatalogPath, FileCatalog};
    ///
    /// let mut catalog = FileCatalog::new();
    /// assert_eq!(catalog.len(), 0);
    ///
    /// catalog.insert(CatalogPath::new("src/lib.rs")?, Vec::new())?;
    /// assert_eq!(catalog.len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Returns `true` when the catalog contains no entries.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_signature::{CatalogPath, FileCatalog};
    ///
    /// let mut catalog = FileCatalog::new();
    /// assert!(catalog.is_empty());
    ///
    /// catalog.insert(CatalogPath::new("src/lib.rs")?, Vec::new())?;
    /// assert!(!catalog.is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// A validated logical catalog path.
///
/// Catalog paths are UTF-8, relative, use `/` separators, and are independent
/// of the host operating system path format.
///
/// # Examples
///
/// ```
/// use conkit_signature::{CatalogPath, FileCatalogError};
///
/// let path = CatalogPath::new("contracts/rust/lib.yaml")?;
/// assert_eq!(path.as_str(), "contracts/rust/lib.yaml");
/// assert_eq!(path.to_string(), "contracts/rust/lib.yaml");
///
/// assert!(matches!(
///     CatalogPath::new("../lib.rs"),
///     Err(FileCatalogError::InvalidPath { .. })
/// ));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize)]
pub struct CatalogPath {
    value: String,
}

impl<'de> Deserialize<'de> for CatalogPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct CatalogPathDocument {
            value: String,
        }

        let document = CatalogPathDocument::deserialize(deserializer)?;
        Self::new(document.value).map_err(serde::de::Error::custom)
    }
}

impl CatalogPath {
    /// Validates and creates a logical catalog path.
    ///
    /// # Errors
    ///
    /// Returns [`FileCatalogError::InvalidPath`] when the value is empty,
    /// absolute, contains platform separators, contains `:` anywhere, contains
    /// NUL bytes, or contains `.`, `..`, or empty path components.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_signature::{CatalogPath, FileCatalogError};
    ///
    /// assert_eq!(CatalogPath::new("src/lib.rs")?.as_str(), "src/lib.rs");
    /// assert!(matches!(
    ///     CatalogPath::new("contracts/a:b.yml"),
    ///     Err(FileCatalogError::InvalidPath { .. })
    /// ));
    /// assert!(matches!(
    ///     CatalogPath::new("/src/lib.rs"),
    ///     Err(FileCatalogError::InvalidPath { .. })
    /// ));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(value: impl Into<String>) -> Result<Self, FileCatalogError> {
        let value = value.into();
        CatalogPathParts::new(&value).validate()?;
        Ok(Self { value })
    }

    /// Returns the validated logical path string.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_signature::CatalogPath;
    ///
    /// let path = CatalogPath::new("archives/contracts.gzip")?;
    /// assert_eq!(path.as_str(), "archives/contracts.gzip");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn as_str(&self) -> &str {
        &self.value
    }

    pub(crate) fn has_extension(&self, extension: &str) -> bool {
        self.value
            .rsplit_once('.')
            .is_some_and(|(_, actual)| actual.eq_ignore_ascii_case(extension))
    }
}

impl fmt::Display for CatalogPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

struct CatalogPathParts<'a> {
    value: &'a str,
}

impl<'a> CatalogPathParts<'a> {
    fn new(value: &'a str) -> Self {
        Self { value }
    }

    fn validate(&self) -> Result<(), FileCatalogError> {
        if self.value.is_empty()
            || self.value.starts_with('/')
            || self.value.contains('\\')
            || self.value.contains(':')
            || self.value.as_bytes().contains(&0)
        {
            return Err(FileCatalogError::invalid(self.value));
        }

        if self
            .value
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
        {
            return Err(FileCatalogError::invalid(self.value));
        }

        Ok(())
    }
}

/// Errors returned by [`FileCatalog`] and [`CatalogPath`].
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum FileCatalogError {
    /// The provided logical catalog path is invalid.
    #[error("invalid catalog path: {path}")]
    InvalidPath {
        /// The rejected logical path.
        path: String,
    },
    /// A catalog already contains an entry for the logical path.
    #[error("duplicate catalog path: {path}")]
    DuplicatePath {
        /// The duplicate logical path.
        path: CatalogPath,
    },
}

impl FileCatalogError {
    fn invalid(path: &str) -> Self {
        Self::InvalidPath {
            path: path.to_owned(),
        }
    }

    fn duplicate(path: &CatalogPath) -> Self {
        Self::DuplicatePath { path: path.clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::{CatalogPath, FileCatalog, FileCatalogError};

    #[test]
    fn catalog_path_accepts_valid_logical_paths() {
        for path in [
            "src/lib.rs",
            "contracts/rust/lib.yaml",
            "archive/contracts.gzip",
        ] {
            let actual = CatalogPath::new(path).expect("path should be valid");

            assert_eq!(actual.as_str(), path);
            assert_eq!(actual.to_string(), path);
        }
    }

    #[test]
    fn catalog_path_rejects_invalid_logical_paths() {
        for path in [
            "",
            "/src/lib.rs",
            "src\\lib.rs",
            "C:/src/lib.rs",
            "a\0b",
            "src//lib.rs",
            "./src/lib.rs",
            "src/./lib.rs",
            "src/../lib.rs",
        ] {
            assert!(
                matches!(
                    CatalogPath::new(path),
                    Err(FileCatalogError::InvalidPath { .. })
                ),
                "path should be invalid: {path:?}"
            );
        }
    }

    #[test]
    fn catalog_path_deserialization_rejects_invalid_object_value() {
        let error = serde_json::from_str::<CatalogPath>(r#"{"value":"../escape.yaml"}"#)
            .expect_err("deserialization must preserve the catalog path invariant");

        assert!(error.to_string().contains("invalid catalog path"));
    }

    #[test]
    fn catalog_path_serialization_preserves_object_shape() {
        let path = CatalogPath::new("src/lib.rs").expect("catalog path");

        assert_eq!(
            serde_json::to_string(&path).expect("serialize catalog path"),
            r#"{"value":"src/lib.rs"}"#
        );
    }

    #[test]
    fn duplicate_insert_returns_catalog_error() {
        let path = CatalogPath::new("src/lib.rs").expect("path");
        let mut catalog = FileCatalog::new();

        catalog
            .insert(path.clone(), b"first".to_vec())
            .expect("first insert");
        let error = catalog
            .insert(path.clone(), b"second".to_vec())
            .expect_err("duplicate should fail");

        assert_eq!(
            error,
            FileCatalogError::DuplicatePath { path: path.clone() }
        );
        assert_eq!(catalog.get(&path), Some(&b"first"[..]));
    }

    #[test]
    fn iterates_in_catalog_path_order() {
        let mut catalog = FileCatalog::new();
        catalog
            .insert(CatalogPath::new("src/z.rs").expect("path"), Vec::new())
            .expect("insert z");
        catalog
            .insert(CatalogPath::new("src/a.rs").expect("path"), Vec::new())
            .expect("insert a");

        let actual = catalog
            .iter()
            .map(|(path, _)| path.as_str().to_owned())
            .collect::<Vec<_>>();

        assert_eq!(actual, vec!["src/a.rs", "src/z.rs"]);
    }

    #[test]
    fn exposes_catalog_accessors_without_raw_map() {
        let path = CatalogPath::new("src/lib.rs").expect("path");
        let mut catalog = FileCatalog::new();

        assert!(catalog.is_empty());
        assert_eq!(catalog.len(), 0);

        catalog
            .insert(path.clone(), b"contents".to_vec())
            .expect("insert");

        assert!(!catalog.is_empty());
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog.get(&path), Some(&b"contents"[..]));

        let entries = catalog
            .into_entries()
            .map(|(path, bytes)| (path.to_string(), bytes))
            .collect::<Vec<_>>();

        assert_eq!(
            entries,
            vec![("src/lib.rs".to_owned(), b"contents".to_vec())]
        );
    }

    #[test]
    fn detects_extensions() {
        let path = CatalogPath::new("src/lib.rs").expect("path");

        assert!(path.has_extension("rs"));
        assert!(
            CatalogPath::new("src/lib.RS")
                .expect("path")
                .has_extension("rs")
        );
        assert!(!path.has_extension("yaml"));
    }
}
