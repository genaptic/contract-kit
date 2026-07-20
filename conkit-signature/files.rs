use serde::de::{self, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fmt;

/// An ordered in-memory catalog of logical paths to byte contents.
///
/// `FileCatalog` is the crate boundary for source files, contract files,
/// generated reports, and previous contract catalogs. It keeps entries sorted by
/// [`CatalogPath`] and rejects duplicate paths instead of overwriting bytes.
/// When serialized, validated catalog path strings are map keys in the same
/// deterministic order. Deserialization observes encoded entries one at a
/// time and rejects duplicate map keys through the same catalog invariant.
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
/// catalog.insert(lib.clone(), b"lib".to_vec())?;
/// catalog.insert(module.clone(), b"a".to_vec())?;
///
/// assert_eq!(catalog.get(&lib), Some(&b"lib"[..]));
/// assert_eq!(
///     catalog.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(),
///     ["src/a.rs", "src/lib.rs"],
/// );
///
/// let json = serde_json::to_string(&catalog)?;
/// assert_eq!(json, r#"{"files":{"src/a.rs":[97],"src/lib.rs":[108,105,98]}}"#);
/// assert_eq!(serde_json::from_str::<FileCatalog>(&json)?, catalog);
///
/// let entries = catalog
///     .into_entries()
///     .map(|(path, bytes)| (path.to_string(), bytes))
///     .collect::<Vec<_>>();
/// assert_eq!(entries.len(), 2);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct FileCatalog {
    files: BTreeMap<CatalogPath, Vec<u8>>,
}

impl<'de> Deserialize<'de> for FileCatalog {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("FileCatalog", &["files"], FileCatalogVisitor)
    }
}

struct FileCatalogVisitor;

impl<'de> Visitor<'de> for FileCatalogVisitor {
    type Value = FileCatalog;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a file catalog with one files map")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let files = sequence
            .next_element::<CatalogEntries>()?
            .ok_or_else(|| de::Error::invalid_length(0, &self))?;

        if sequence.next_element::<IgnoredAny>()?.is_some() {
            return Err(de::Error::invalid_length(2, &self));
        }

        Ok(files.into_catalog())
    }

    fn visit_map<A>(self, mut mapping: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut files = None;

        while let Some(field) = mapping.next_key::<String>()? {
            if field == "files" {
                if files.is_some() {
                    return Err(de::Error::duplicate_field("files"));
                }
                files = Some(mapping.next_value::<CatalogEntries>()?.into_catalog());
            } else {
                mapping.next_value::<IgnoredAny>()?;
            }
        }

        files.ok_or_else(|| de::Error::missing_field("files"))
    }
}

struct CatalogEntries {
    catalog: FileCatalog,
}

impl CatalogEntries {
    fn into_catalog(self) -> FileCatalog {
        self.catalog
    }
}

impl<'de> Deserialize<'de> for CatalogEntries {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(CatalogEntriesVisitor)
    }
}

struct CatalogEntriesVisitor;

impl<'de> Visitor<'de> for CatalogEntriesVisitor {
    type Value = CatalogEntries;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a map of unique logical catalog paths to bytes")
    }

    fn visit_map<A>(self, mut mapping: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut catalog = FileCatalog::new();

        while let Some(path) = mapping.next_key::<CatalogPath>()? {
            if catalog.files.contains_key(&path) {
                return Err(de::Error::custom(FileCatalogError::duplicate(&path)));
            }
            let contents = mapping.next_value::<Vec<u8>>()?;
            catalog.insert(path, contents).map_err(de::Error::custom)?;
        }

        Ok(CatalogEntries { catalog })
    }
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
/// of the host operating system path format. Serde encodes a catalog path as a
/// scalar string and validates that string during deserialization; object forms
/// are rejected.
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
/// let json = serde_json::to_string(&path)?;
/// assert_eq!(json, r#""contracts/rust/lib.yaml""#);
/// assert_eq!(serde_json::from_str::<CatalogPath>(&json)?, path);
/// assert!(serde_json::from_str::<CatalogPath>(
///     r#"{"value":"contracts/rust/lib.yaml"}"#,
/// )
/// .is_err());
///
/// assert!(matches!(
///     CatalogPath::new("../lib.rs"),
///     Err(FileCatalogError::InvalidPath { .. })
/// ));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct CatalogPath {
    value: String,
}

impl Serialize for CatalogPath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CatalogPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
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
    use proptest::prelude::*;

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
    fn catalog_path_deserialization_requires_a_valid_scalar() {
        let path = serde_json::from_str::<CatalogPath>(r#""src/lib.rs""#)
            .expect("valid scalar catalog path");
        assert_eq!(path.as_str(), "src/lib.rs");

        let error = serde_json::from_str::<CatalogPath>(r#""../escape.yaml""#)
            .expect_err("deserialization must preserve the catalog path invariant");

        assert!(error.to_string().contains("invalid catalog path"));

        serde_json::from_str::<CatalogPath>(r#"{"value":"src/lib.rs"}"#)
            .expect_err("the legacy object form must be rejected");
    }

    #[test]
    fn catalog_path_serializes_as_a_scalar() {
        let path = CatalogPath::new("src/lib.rs").expect("catalog path");

        assert_eq!(
            serde_json::to_string(&path).expect("serialize catalog path"),
            r#""src/lib.rs""#
        );
    }

    #[test]
    fn nonempty_catalog_json_round_trip_is_path_sorted() {
        let mut catalog = FileCatalog::new();
        catalog
            .insert(CatalogPath::new("src/z.rs").expect("z path"), b"z".to_vec())
            .expect("insert z");
        catalog
            .insert(CatalogPath::new("src/a.rs").expect("a path"), b"a".to_vec())
            .expect("insert a");

        let json = serde_json::to_string(&catalog).expect("serialize nonempty catalog");
        assert_eq!(json, r#"{"files":{"src/a.rs":[97],"src/z.rs":[122]}}"#);

        let reparsed = serde_json::from_str::<FileCatalog>(&json).expect("reparse catalog");
        assert_eq!(reparsed, catalog);
    }

    #[test]
    fn catalog_json_deserialization_rejects_duplicate_encoded_paths() {
        let error =
            serde_json::from_str::<FileCatalog>(r#"{"files":{"src/lib.rs":[1],"src/lib.rs":[2]}}"#)
                .expect_err("encoded duplicate path must not overwrite the first entry");

        assert!(
            error
                .to_string()
                .contains("duplicate catalog path: src/lib.rs")
        );
    }

    #[test]
    fn catalog_yaml_deserialization_rejects_duplicate_encoded_paths() {
        let options = serde_saphyr::options! {
            duplicate_keys: serde_saphyr::DuplicateKeyPolicy::LastWins,
        };
        let error = serde_saphyr::from_str_with_options::<FileCatalog>(
            "files:\n  src/lib.rs: [1]\n  src/lib.rs: [2]\n",
            options,
        )
        .expect_err("encoded duplicate path must not overwrite the first entry");
        let rendered = error.to_string();

        assert!(
            rendered.contains("duplicate catalog path: src/lib.rs"),
            "{rendered}"
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

    proptest! {
        #[test]
        fn generated_portable_catalog_paths_round_trip_as_validated_scalars(
            components in prop::collection::vec(
                prop::collection::vec(b'a'..=b'z', 1..16),
                1..8,
            ),
        ) {
            let value = components
                .into_iter()
                .map(|component| String::from_utf8(component).expect("ASCII component"))
                .collect::<Vec<_>>()
                .join("/");
            let path = CatalogPath::new(value.clone()).expect("generated valid path");
            let json = serde_json::to_string(&path).expect("serialize generated path");
            let decoded = serde_json::from_str::<CatalogPath>(&json)
                .expect("deserialize generated path");

            prop_assert_eq!(decoded.as_str(), value);
            prop_assert_eq!(decoded, path);
        }

        #[test]
        fn catalog_serialization_is_independent_of_insertion_order(
            names in prop::collection::btree_set(
                prop::collection::vec(b'a'..=b'z', 1..24),
                0..32,
            ),
        ) {
            let entries = names
                .into_iter()
                .map(|name| {
                    let name = String::from_utf8(name).expect("ASCII name");
                    let path = CatalogPath::new(format!("src/{name}.rs"))
                        .expect("generated source path");
                    (path, name.into_bytes())
                })
                .collect::<Vec<_>>();
            let mut forward = FileCatalog::new();
            let mut reverse = FileCatalog::new();
            for (path, bytes) in &entries {
                forward
                    .insert(path.clone(), bytes.clone())
                    .expect("unique generated forward path");
            }
            for (path, bytes) in entries.iter().rev() {
                reverse
                    .insert(path.clone(), bytes.clone())
                    .expect("unique generated reverse path");
            }

            prop_assert_eq!(
                serde_json::to_vec(&forward).expect("serialize forward catalog"),
                serde_json::to_vec(&reverse).expect("serialize reverse catalog"),
            );
        }
    }
}
