use serde::de::{self, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fmt;

/// An ordered in-memory catalog of logical paths to byte contents.
///
/// `FileCatalog` is the crate boundary for source files, combined contract
/// files, refreshed contract files, and report bytes. Its ordered storage makes
/// [`FileCatalog::iter`], [`FileCatalog::into_entries`], and serde serialization
/// emit entries in ascending [`CatalogPath`] order. [`FileCatalog::insert`]
/// rejects duplicate paths instead of overwriting bytes.
///
/// Serde encodings represent catalog paths as scalar string map keys.
/// Deserialization validates every key through [`CatalogPath::new`], so encoded
/// input cannot bypass the logical-path rules, and duplicate encoded keys are
/// rejected instead of overwriting an earlier entry.
///
/// # Examples
///
/// ```
/// use conkit_sketch::{CatalogPath, FileCatalog};
///
/// let mut catalog = FileCatalog::new();
/// let lib = CatalogPath::new("src/lib.rs")?;
/// let contract = CatalogPath::new("contracts/main.yml")?;
///
/// catalog.insert(lib.clone(), b"pub fn answer() -> u8 { 42 }\n".to_vec())?;
/// catalog.insert(
///     contract.clone(),
///     b"contract_version: 2\nroot: ../src\nfiles: []\nsignatures: []\nsketches: []\n"
///         .to_vec(),
/// )?;
///
/// assert_eq!(catalog.get(&lib), Some(&b"pub fn answer() -> u8 { 42 }\n"[..]));
/// assert_eq!(
///     catalog.iter().map(|(path, _)| path.as_str()).collect::<Vec<_>>(),
///     ["contracts/main.yml", "src/lib.rs"],
/// );
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
    /// use conkit_sketch::FileCatalog;
    ///
    /// let catalog = FileCatalog::new();
    /// assert!(catalog.is_empty());
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts one logical path and its bytes.
    ///
    /// # Errors
    ///
    /// Returns [`FileCatalogError::DuplicatePath`] when the path already
    /// exists in this catalog.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_sketch::{CatalogPath, FileCatalog, FileCatalogError};
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

    /// Returns the bytes for `path`, if present.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_sketch::{CatalogPath, FileCatalog};
    ///
    /// let path = CatalogPath::new("src/lib.rs")?;
    /// let mut catalog = FileCatalog::new();
    /// catalog.insert(path.clone(), b"contents".to_vec())?;
    ///
    /// assert_eq!(catalog.get(&path), Some(&b"contents"[..]));
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
    /// use conkit_sketch::{CatalogPath, FileCatalog};
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
    /// use conkit_sketch::{CatalogPath, FileCatalog};
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
    /// use conkit_sketch::{CatalogPath, FileCatalog};
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
    /// use conkit_sketch::{CatalogPath, FileCatalog};
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
/// of the host operating system path format. Validation rejects every `:`
/// character, not only operating-system drive prefixes. Serde represents a
/// path as a scalar string and applies the same validation when deserializing.
///
/// # Examples
///
/// ```
/// use conkit_sketch::{CatalogPath, FileCatalogError};
///
/// let path = CatalogPath::new("contracts/rust/main.yaml")?;
/// assert_eq!(path.as_str(), "contracts/rust/main.yaml");
/// assert_eq!(path.to_string(), "contracts/rust/main.yaml");
///
/// assert!(matches!(
///     CatalogPath::new("../main.yml"),
///     Err(FileCatalogError::InvalidPath { .. })
/// ));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct CatalogPath {
    value: String,
}

impl CatalogPath {
    /// Validates and creates a logical catalog path.
    ///
    /// # Errors
    ///
    /// Returns [`FileCatalogError::InvalidPath`] when the value is empty, starts
    /// with `/`, contains `\`, any `:` character, or a NUL byte, or contains an
    /// empty, `.`, or `..` slash-delimited component.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_sketch::{CatalogPath, FileCatalogError};
    ///
    /// assert_eq!(CatalogPath::new("src/lib.rs")?.as_str(), "src/lib.rs");
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
    /// use conkit_sketch::CatalogPath;
    ///
    /// let path = CatalogPath::new("reports/output.yml")?;
    /// assert_eq!(path.as_str(), "reports/output.yml");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn as_str(&self) -> &str {
        &self.value
    }

    /// Returns `true` when the suffix after the final `.` matches `extension`.
    ///
    /// The comparison is ASCII case-insensitive. Pass `extension` without a
    /// leading `.`.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_sketch::CatalogPath;
    ///
    /// let path = CatalogPath::new("contracts/main.yml")?;
    /// assert!(path.has_extension("yml"));
    /// assert!(!path.has_extension("yaml"));
    /// assert!(path.has_extension("YML"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn has_extension(&self, extension: &str) -> bool {
        self.value
            .rsplit_once('.')
            .is_some_and(|(_, actual)| actual.eq_ignore_ascii_case(extension))
    }

    /// Returns a copy of this logical path with its suffix after the final `.`
    /// replaced.
    ///
    /// This method currently performs only that logical string transformation:
    /// when the path contains no `.`, it appends `.` and `extension`. It does
    /// not apply host-filesystem extension rules, infer compound extensions, or
    /// normalize case. The returned path is validated before being returned.
    ///
    /// # Errors
    ///
    /// Returns [`FileCatalogError::InvalidPath`] if the resulting path is not a
    /// valid logical catalog path.
    ///
    /// # Examples
    ///
    /// ```
    /// use conkit_sketch::CatalogPath;
    ///
    /// let path = CatalogPath::new("src/generated/lib.rs")?;
    /// let yaml = path.with_extension("yaml")?;
    /// assert_eq!(yaml.as_str(), "src/generated/lib.yaml");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn with_extension(&self, extension: &str) -> Result<Self, FileCatalogError> {
        let stem = self
            .value
            .rsplit_once('.')
            .map_or(self.value.as_str(), |(stem, _)| stem);

        Self::new(format!("{stem}.{extension}"))
    }
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

/// Errors returned by [`FileCatalog`] mutation and [`CatalogPath`] validation.
///
/// These errors reject invalid catalog-boundary input. Valid sketch checks that
/// do not match source code are represented by
/// [`SketchDiagnostic`](crate::SketchDiagnostic) values instead.
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
            "contracts/rust/main.yaml",
            "reports/output.yml",
            "nested/path with spaces/file.txt",
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
        let path = CatalogPath::new("contracts/main.yml").expect("path");

        assert!(path.has_extension("yml"));
        assert!(
            CatalogPath::new("contracts/main.YML")
                .expect("path")
                .has_extension("yml")
        );
        assert!(!path.has_extension("yaml"));
    }

    #[test]
    fn replaces_extension_without_platform_path_conversion() {
        let path = CatalogPath::new("src/generated/lib.rs").expect("path");

        let actual = path.with_extension("yaml").expect("extension");

        assert_eq!(actual.as_str(), "src/generated/lib.yaml");
    }

    #[test]
    fn appends_extension_when_none_exists() {
        let path = CatalogPath::new("contracts/main").expect("path");

        let actual = path.with_extension("yml").expect("extension");

        assert_eq!(actual.as_str(), "contracts/main.yml");
    }

    #[test]
    fn catalog_path_serializes_as_scalar_string() {
        let path = CatalogPath::new("src/lib.rs").expect("path");

        let json = serde_json::to_string(&path).expect("serialize path");

        assert_eq!(json, r#""src/lib.rs""#);
    }

    #[test]
    fn catalog_path_deserialization_validates_scalar_string() {
        let path =
            serde_json::from_str::<CatalogPath>(r#""src/lib.rs""#).expect("deserialize valid path");

        assert_eq!(path.as_str(), "src/lib.rs");
        assert!(serde_json::from_str::<CatalogPath>(r#""../bad""#).is_err());
        assert!(serde_json::from_str::<CatalogPath>(r#"{"value":"src/lib.rs"}"#).is_err());
    }

    #[test]
    fn nonempty_file_catalog_json_round_trip_preserves_order() {
        let mut catalog = FileCatalog::new();
        catalog
            .insert(CatalogPath::new("src/z.rs").expect("path"), b"z".to_vec())
            .expect("insert z");
        catalog
            .insert(CatalogPath::new("src/a.rs").expect("path"), b"a".to_vec())
            .expect("insert a");

        let json = serde_json::to_string(&catalog).expect("serialize catalog");
        let round_tripped =
            serde_json::from_str::<FileCatalog>(&json).expect("deserialize catalog");

        assert_eq!(json, r#"{"files":{"src/a.rs":[97],"src/z.rs":[122]}}"#);
        assert_eq!(round_tripped, catalog);
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

        assert!(
            error
                .to_string()
                .contains("duplicate catalog path: src/lib.rs")
        );
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
