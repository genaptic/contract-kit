use crate::api::{DiffEntry, DiffResponse, GenerateResponse, SketchSeed};
use crate::error::SketchContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::generate::SketchRefreshSeeds;
use crate::id::SketchId;
use crate::normalize::NormalizedSnippet;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub(crate) struct SketchContracts {
    entries: Vec<SketchContract>,
    contract_file_count: usize,
}

impl SketchContracts {
    pub(crate) fn from_catalog(catalog: FileCatalog) -> Result<Self, SketchContractKitError> {
        let documents = SketchContractDocuments::from_catalog(catalog)?;
        documents.contracts()
    }

    pub(crate) fn entries(&self) -> &[SketchContract] {
        &self.entries
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn contract_file_count(&self) -> usize {
        self.contract_file_count
    }

    pub(crate) fn diff_against(&self, previous: &Self) -> DiffResponse {
        let current_by_id = self
            .entries
            .iter()
            .map(|contract| (contract.id(), contract))
            .collect::<BTreeMap<_, _>>();
        let previous_by_id = previous
            .entries
            .iter()
            .map(|contract| (contract.id(), contract))
            .collect::<BTreeMap<_, _>>();
        let ids = current_by_id
            .keys()
            .chain(previous_by_id.keys())
            .copied()
            .collect::<BTreeSet<_>>();
        let mut entries = Vec::new();

        for id in ids {
            if let Some(current) = current_by_id.get(id) {
                match previous_by_id.get(id) {
                    Some(previous) if current.semantically_matches(previous) => {}
                    Some(_) => entries.push(DiffEntry::Changed {
                        sketch_id: id.as_str().to_owned(),
                    }),
                    None => entries.push(DiffEntry::Added {
                        sketch_id: id.as_str().to_owned(),
                    }),
                }
            } else {
                entries.push(DiffEntry::Removed {
                    sketch_id: id.as_str().to_owned(),
                });
            }
        }

        DiffResponse {
            changed: !entries.is_empty(),
            entries,
        }
    }
}

#[derive(Debug)]
pub(crate) struct SketchContract {
    id: SketchId,
    contract_file: CatalogPath,
    file: CatalogPath,
    linked_signature: SignatureLabel,
    signature_type: SignatureType,
    snippet: SketchSnippet,
}

impl SketchContract {
    fn from_link(sketch: PendingSketch, link: SignatureLink) -> Self {
        Self {
            id: sketch.id,
            contract_file: sketch.contract_file,
            file: link.file,
            linked_signature: link.label,
            signature_type: sketch.signature_type,
            snippet: sketch.snippet,
        }
    }

    pub(crate) fn id(&self) -> &SketchId {
        &self.id
    }

    pub(crate) fn contract_file(&self) -> &CatalogPath {
        &self.contract_file
    }

    pub(crate) fn file(&self) -> &CatalogPath {
        &self.file
    }

    pub(crate) fn signature_type(&self) -> &SignatureType {
        &self.signature_type
    }

    pub(crate) fn snippet(&self) -> &SketchSnippet {
        &self.snippet
    }

    pub(crate) fn semantically_matches(&self, other: &Self) -> bool {
        self.file == other.file
            && self.linked_signature == other.linked_signature
            && self.signature_type == other.signature_type
            && self.snippet.normalized() == other.snippet.normalized()
    }

    pub(crate) fn validate_seed(
        &self,
        seed: &SketchSeed,
        id: &SketchId,
    ) -> Result<(), SketchContractKitError> {
        let signature_type = seed.signature_type.trim();
        if signature_type.is_empty() {
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
        if &seed.file != self.file() {
            return Err(SketchContractKitError::conversion_failed(format!(
                "sketch refresh seed {} targets source file {}, expected {}",
                id.as_str(),
                seed.file,
                self.file()
            )));
        }
        if signature_type != self.signature_type().as_str() {
            return Err(SketchContractKitError::conversion_failed(format!(
                "sketch refresh seed {} has signature_type {}, expected {}",
                id.as_str(),
                signature_type,
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
    fn new(
        code: impl Into<String>,
        catalog_name: &CatalogPath,
        sketch_id: &str,
    ) -> Result<Self, SketchContractKitError> {
        let normalized = NormalizedSnippet::from_code(&code.into());
        if normalized.is_empty() {
            return Err(SketchContractKitError::parse_failed(
                catalog_name,
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
pub(crate) struct SignatureLabel {
    value: String,
}

impl SignatureLabel {
    fn new(
        value: impl Into<String>,
        catalog_name: &CatalogPath,
    ) -> Result<Self, SketchContractKitError> {
        let value = value.into().trim().to_owned();
        if value.is_empty() {
            return Err(SketchContractKitError::parse_failed(
                catalog_name,
                "signature label must not be empty",
            ));
        }

        Ok(Self { value })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.value
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SignatureType {
    value: String,
}

impl SignatureType {
    fn from_contract(
        value: impl Into<String>,
        catalog_name: &CatalogPath,
        subject: &str,
    ) -> Result<Self, SketchContractKitError> {
        let value = value.into().trim().to_owned();
        if value.is_empty() {
            return Err(SketchContractKitError::parse_failed(
                catalog_name,
                format!("{subject} signature_type must not be empty"),
            ));
        }

        Ok(Self { value })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.value
    }
}

pub(crate) struct SketchContractDocuments {
    documents: BTreeMap<CatalogPath, SketchContractDocument>,
    passthrough: FileCatalog,
}

impl SketchContractDocuments {
    pub(crate) fn from_catalog(catalog: FileCatalog) -> Result<Self, SketchContractKitError> {
        let mut documents = BTreeMap::new();
        let mut passthrough = FileCatalog::new();

        for (catalog_name, bytes) in catalog.into_entries() {
            if !catalog_name.as_str().contains('/')
                && (catalog_name.has_extension("yaml") || catalog_name.has_extension("yml"))
            {
                let document = SketchContractDocument::parse(catalog_name.clone(), bytes)?;
                documents.insert(catalog_name, document);
            } else {
                passthrough.insert(catalog_name, bytes)?;
            }
        }

        Ok(Self {
            documents,
            passthrough,
        })
    }

    pub(crate) fn contracts(&self) -> Result<SketchContracts, SketchContractKitError> {
        let mut declared_files = BTreeMap::<CatalogPath, CatalogPath>::new();
        let mut signature_labels = BTreeMap::<SignatureLabel, CatalogPath>::new();
        let mut sketches = BTreeMap::<SketchId, PendingSketch>::new();
        let mut links = BTreeMap::<SketchId, Vec<SignatureLink>>::new();

        for (catalog_name, document) in &self.documents {
            let input = document.input()?;
            input.validate_root(catalog_name)?;
            let files = ContractFiles::from_input(input.files, catalog_name)?;
            let signatures = input
                .signatures
                .into_iter()
                .map(|value| SignatureIndexEntry::from_yaml(value, catalog_name, &files))
                .collect::<Result<Vec<_>, _>>()?;
            let pending_sketches = input
                .sketches
                .into_iter()
                .map(|value| PendingSketch::from_yaml(value, catalog_name))
                .collect::<Result<Vec<_>, _>>()?;

            for file in files.into_paths() {
                if let Some(previous) = declared_files.insert(file.clone(), catalog_name.clone()) {
                    return Err(SketchContractKitError::parse_failed(
                        catalog_name,
                        format!(
                            "source file {file} is listed by more than one contract document (also {previous})"
                        ),
                    ));
                }
            }

            for signature in signatures {
                if let Some(previous) =
                    signature_labels.insert(signature.label.clone(), catalog_name.clone())
                {
                    return Err(SketchContractKitError::parse_failed(
                        catalog_name,
                        format!(
                            "duplicate signature label {} (also declared in {previous})",
                            signature.label.as_str()
                        ),
                    ));
                }

                if let Some(link) = signature.into_link(catalog_name.clone()) {
                    links.entry(link.sketch_id.clone()).or_default().push(link);
                }
            }

            for sketch in pending_sketches {
                if let Some(previous) = sketches.insert(sketch.id.clone(), sketch) {
                    return Err(SketchContractKitError::parse_failed(
                        catalog_name,
                        format!(
                            "duplicate sketch id {} (also declared in {})",
                            previous.id.as_str(),
                            previous.contract_file
                        ),
                    ));
                }
            }
        }

        let mut entries = Vec::with_capacity(sketches.len());
        for (sketch_id, sketch) in sketches {
            let Some(mut sketch_links) = links.remove(&sketch_id) else {
                return Err(SketchContractKitError::parse_failed(
                    sketch.contract_file,
                    format!(
                        "orphan sketch {} is not referenced by a signature",
                        sketch_id.as_str()
                    ),
                ));
            };

            if sketch_links.len() != 1 {
                return Err(SketchContractKitError::parse_failed(
                    sketch.contract_file,
                    format!(
                        "sketch {} is referenced by more than one signature",
                        sketch_id.as_str()
                    ),
                ));
            }

            let link = sketch_links.pop().ok_or_else(|| {
                SketchContractKitError::parse_failed(
                    sketch.contract_file.clone(),
                    format!("sketch {} has no signature link", sketch_id.as_str()),
                )
            })?;
            if link.contract_file != sketch.contract_file {
                return Err(SketchContractKitError::parse_failed(
                    link.contract_file,
                    format!(
                        "signature {} links to sketch {} in another contract document {}",
                        link.label.as_str(),
                        sketch_id.as_str(),
                        sketch.contract_file
                    ),
                ));
            }
            if link.signature_type != sketch.signature_type {
                return Err(SketchContractKitError::parse_failed(
                    sketch.contract_file.clone(),
                    format!(
                        "sketch {} signature_type {} does not match linked signature {} type {}",
                        sketch_id.as_str(),
                        sketch.signature_type.as_str(),
                        link.label.as_str(),
                        link.signature_type.as_str()
                    ),
                ));
            }

            entries.push(SketchContract::from_link(sketch, link));
        }

        if let Some((sketch_id, remaining_links)) = links.into_iter().next() {
            let Some(link) = remaining_links.into_iter().next() else {
                return Err(SketchContractKitError::conversion_failed(
                    "internal sketch link collection was empty",
                ));
            };
            return Err(SketchContractKitError::parse_failed(
                link.contract_file,
                format!(
                    "signature {} references missing sketch {}",
                    link.label.as_str(),
                    sketch_id.as_str()
                ),
            ));
        }

        Ok(SketchContracts {
            entries,
            contract_file_count: self.documents.len(),
        })
    }

    pub(crate) fn refresh(
        self,
        seeds: SketchRefreshSeeds,
    ) -> Result<GenerateResponse, SketchContractKitError> {
        let sketch_count = seeds.len();
        let mut updated_count = 0;
        let Self {
            documents,
            mut passthrough,
        } = self;

        for (path, mut document) in documents {
            let bytes = if seeds.contains_document(&path) {
                updated_count += document.refresh(&seeds)?;
                document.render()?
            } else {
                document.original_bytes
            };
            passthrough.insert(path, bytes)?;
        }

        if updated_count != sketch_count {
            return Err(SketchContractKitError::conversion_failed(format!(
                "refreshed {updated_count} sketches but expected {sketch_count}"
            )));
        }

        Ok(GenerateResponse {
            contract_files: passthrough,
            sketch_count,
        })
    }
}

struct SketchContractDocument {
    catalog_name: CatalogPath,
    original_bytes: Vec<u8>,
    value: serde_yaml::Value,
}

impl SketchContractDocument {
    fn parse(catalog_name: CatalogPath, bytes: Vec<u8>) -> Result<Self, SketchContractKitError> {
        let value = serde_yaml::from_slice(&bytes).map_err(|source| {
            SketchContractKitError::parse_failed(&catalog_name, source.to_string())
        })?;

        Ok(Self {
            catalog_name,
            original_bytes: bytes,
            value,
        })
    }

    fn input(&self) -> Result<SketchYamlDocumentInput, SketchContractKitError> {
        SketchYamlDocumentInput::deserialize(&self.value).map_err(|source| {
            SketchContractKitError::parse_failed(&self.catalog_name, source.to_string())
        })
    }

    fn refresh(&mut self, seeds: &SketchRefreshSeeds) -> Result<usize, SketchContractKitError> {
        let Some(document) = self.value.as_mapping_mut() else {
            return Err(SketchContractKitError::parse_failed(
                &self.catalog_name,
                "combined contract document must be a mapping",
            ));
        };
        let sketches_key = serde_yaml::Value::String("sketches".to_owned());
        let Some(sketches) = document
            .get_mut(&sketches_key)
            .and_then(serde_yaml::Value::as_sequence_mut)
        else {
            return Err(SketchContractKitError::parse_failed(
                &self.catalog_name,
                "combined contract sketches must be a list",
            ));
        };
        let mut updated_count = 0;

        for value in sketches {
            let Some(mapping) = value.as_mapping_mut() else {
                return Err(SketchContractKitError::parse_failed(
                    &self.catalog_name,
                    "sketch entries must be flattened mappings",
                ));
            };
            let id = mapping
                .iter()
                .find_map(|(key, value)| {
                    let key = key.as_str()?;
                    (key != "signature_type" && key != "code" && value.is_null())
                        .then_some(key.to_owned())
                })
                .ok_or_else(|| {
                    SketchContractKitError::parse_failed(
                        &self.catalog_name,
                        "sketch entry is missing its null-valued identifier",
                    )
                })?;
            let id = SketchId::from_contract(id, &self.catalog_name)?;
            if let Some(code) = seeds.code_for(&id) {
                mapping.insert(
                    serde_yaml::Value::String("code".to_owned()),
                    serde_yaml::Value::String(code.to_owned()),
                );
                updated_count += 1;
            }
        }

        Ok(updated_count)
    }

    fn render(self) -> Result<Vec<u8>, SketchContractKitError> {
        serde_yaml::to_string(&self.value)
            .map(String::into_bytes)
            .map_err(|source| {
                SketchContractKitError::write_failed(&self.catalog_name, source.to_string())
            })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SketchYamlDocumentInput {
    root: String,
    files: Vec<String>,
    signatures: Vec<serde_yaml::Value>,
    sketches: Vec<serde_yaml::Value>,
}

impl SketchYamlDocumentInput {
    fn validate_root(&self, catalog_name: &CatalogPath) -> Result<(), SketchContractKitError> {
        if self.root.trim().is_empty() {
            return Err(SketchContractKitError::parse_failed(
                catalog_name,
                "contract root must not be empty",
            ));
        }

        Ok(())
    }
}

struct ContractFiles {
    paths: BTreeSet<CatalogPath>,
}

impl ContractFiles {
    fn from_input(
        values: Vec<String>,
        catalog_name: &CatalogPath,
    ) -> Result<Self, SketchContractKitError> {
        let mut paths = BTreeSet::new();

        for value in values {
            let path = CatalogPath::new(value).map_err(|source| {
                SketchContractKitError::parse_failed(
                    catalog_name,
                    format!("contract files contains an invalid path: {source}"),
                )
            })?;
            if !paths.insert(path.clone()) {
                return Err(SketchContractKitError::parse_failed(
                    catalog_name,
                    format!("duplicate contract file {path}"),
                ));
            }
        }

        Ok(Self { paths })
    }

    fn contains(&self, path: &CatalogPath) -> bool {
        self.paths.contains(path)
    }

    fn into_paths(self) -> Vec<CatalogPath> {
        self.paths.into_iter().collect()
    }
}

struct SignatureIndexEntry {
    label: SignatureLabel,
    file: CatalogPath,
    signature_type: SignatureType,
    sketch_id: Option<SketchId>,
}

impl SignatureIndexEntry {
    fn from_yaml(
        value: serde_yaml::Value,
        catalog_name: &CatalogPath,
        files: &ContractFiles,
    ) -> Result<Self, SketchContractKitError> {
        let Some(mapping) = value.as_mapping() else {
            return Err(SketchContractKitError::parse_failed(
                catalog_name,
                "signature entries must be mappings with exactly one named signature",
            ));
        };
        if mapping.len() != 1 {
            return Err(SketchContractKitError::parse_failed(
                catalog_name,
                "signature entries must contain exactly one named signature",
            ));
        }
        let Some((label, body)) = mapping.iter().next() else {
            return Err(SketchContractKitError::parse_failed(
                catalog_name,
                "signature entry is empty",
            ));
        };
        let Some(label) = label.as_str() else {
            return Err(SketchContractKitError::parse_failed(
                catalog_name,
                "signature label must be a string",
            ));
        };
        let label = SignatureLabel::new(label, catalog_name)?;
        let Some(body) = body.as_mapping() else {
            return Err(SketchContractKitError::parse_failed(
                catalog_name,
                format!("signature {} body must be a mapping", label.as_str()),
            ));
        };

        let file_key = serde_yaml::Value::String("file".to_owned());
        let Some(file) = body.get(&file_key).and_then(serde_yaml::Value::as_str) else {
            return Err(SketchContractKitError::parse_failed(
                catalog_name,
                format!("signature {} is missing string field file", label.as_str()),
            ));
        };
        let file = CatalogPath::new(file).map_err(|source| {
            SketchContractKitError::parse_failed(
                catalog_name,
                format!("signature {} has invalid file: {source}", label.as_str()),
            )
        })?;
        if !files.contains(&file) {
            return Err(SketchContractKitError::parse_failed(
                catalog_name,
                format!(
                    "signature {} references unlisted file {file}",
                    label.as_str()
                ),
            ));
        }

        let signature_type_key = serde_yaml::Value::String("signature_type".to_owned());
        let Some(signature_type) = body
            .get(&signature_type_key)
            .and_then(serde_yaml::Value::as_str)
        else {
            return Err(SketchContractKitError::parse_failed(
                catalog_name,
                format!(
                    "signature {} is missing string field signature_type",
                    label.as_str()
                ),
            ));
        };
        let signature_type = SignatureType::from_contract(
            signature_type,
            catalog_name,
            &format!("signature {}", label.as_str()),
        )?;

        let sketch_key = serde_yaml::Value::String("sketch".to_owned());
        let sketch_id = match body.get(&sketch_key) {
            Some(value) => {
                let Some(value) = value.as_str() else {
                    return Err(SketchContractKitError::parse_failed(
                        catalog_name,
                        format!("signature {} sketch link must be a string", label.as_str()),
                    ));
                };
                Some(SketchId::from_contract(value, catalog_name)?)
            }
            None => None,
        };

        Ok(Self {
            label,
            file,
            signature_type,
            sketch_id,
        })
    }

    fn into_link(self, contract_file: CatalogPath) -> Option<SignatureLink> {
        self.sketch_id.map(|sketch_id| SignatureLink {
            label: self.label,
            contract_file,
            file: self.file,
            signature_type: self.signature_type,
            sketch_id,
        })
    }
}

struct SignatureLink {
    label: SignatureLabel,
    contract_file: CatalogPath,
    file: CatalogPath,
    signature_type: SignatureType,
    sketch_id: SketchId,
}

struct PendingSketch {
    id: SketchId,
    contract_file: CatalogPath,
    signature_type: SignatureType,
    snippet: SketchSnippet,
}

impl PendingSketch {
    fn from_yaml(
        value: serde_yaml::Value,
        catalog_name: &CatalogPath,
    ) -> Result<Self, SketchContractKitError> {
        let Some(mapping) = value.as_mapping() else {
            return Err(SketchContractKitError::parse_failed(
                catalog_name,
                "sketch entries must be flattened mappings",
            ));
        };
        if mapping.len() != 3 {
            return Err(SketchContractKitError::parse_failed(
                catalog_name,
                "sketch entries must contain one null identifier, signature_type, and code",
            ));
        }

        let mut id = None;
        let mut signature_type = None;
        let mut code = None;

        for (key, value) in mapping {
            let Some(key) = key.as_str() else {
                return Err(SketchContractKitError::parse_failed(
                    catalog_name,
                    "sketch entry keys must be strings",
                ));
            };
            match key {
                "signature_type" => {
                    let Some(value) = value.as_str() else {
                        return Err(SketchContractKitError::parse_failed(
                            catalog_name,
                            "sketch signature_type must be a string",
                        ));
                    };
                    signature_type = Some(value.to_owned());
                }
                "code" => {
                    let Some(value) = value.as_str() else {
                        return Err(SketchContractKitError::parse_failed(
                            catalog_name,
                            "sketch code must be a string",
                        ));
                    };
                    code = Some(value.to_owned());
                }
                candidate => {
                    if !matches!(value, serde_yaml::Value::Null) {
                        return Err(SketchContractKitError::parse_failed(
                            catalog_name,
                            format!(
                                "flattened sketch identifier {candidate} must have a null value"
                            ),
                        ));
                    }
                    if id.replace(candidate.to_owned()).is_some() {
                        return Err(SketchContractKitError::parse_failed(
                            catalog_name,
                            "sketch entry contains more than one identifier",
                        ));
                    }
                }
            }
        }

        let id = id.ok_or_else(|| {
            SketchContractKitError::parse_failed(
                catalog_name,
                "sketch entry is missing its null-valued identifier",
            )
        })?;
        let id = SketchId::from_contract(id, catalog_name)?;
        let signature_type = signature_type.ok_or_else(|| {
            SketchContractKitError::parse_failed(
                catalog_name,
                format!("sketch {} is missing signature_type", id.as_str()),
            )
        })?;
        let signature_type = SignatureType::from_contract(
            signature_type,
            catalog_name,
            &format!("sketch {}", id.as_str()),
        )?;
        let code = code.ok_or_else(|| {
            SketchContractKitError::parse_failed(
                catalog_name,
                format!("sketch {} is missing code", id.as_str()),
            )
        })?;
        let snippet = SketchSnippet::new(code, catalog_name, id.as_str())?;

        Ok(Self {
            id,
            contract_file: catalog_name.clone(),
            signature_type,
            snippet,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{SketchContracts, SketchSnippet};
    use crate::api::DiffEntry;
    use crate::files::{CatalogPath, FileCatalog};
    use crate::normalize::NormalizedSnippet;

    struct TestCatalog {
        catalog: FileCatalog,
    }

    impl TestCatalog {
        fn new() -> Self {
            Self {
                catalog: FileCatalog::new(),
            }
        }

        fn with_file(mut self, path: &str, yaml: &str) -> Self {
            self.catalog
                .insert(
                    CatalogPath::new(path).expect("test path"),
                    yaml.as_bytes().to_vec(),
                )
                .expect("insert test yaml");
            self
        }

        fn into_catalog(self) -> FileCatalog {
            self.catalog
        }
    }

    struct ContractYaml;

    impl ContractYaml {
        fn linked(label: &str, sketch_id: &str, signature_type: &str, code: &str) -> String {
            format!(
                "root: ../src\nfiles: [lib.rs]\nsignatures:\n  - {label}:\n      file: lib.rs\n      signature_type: {signature_type}\n      name: answer\n      sketch: {sketch_id}\nsketches:\n  - {sketch_id}:\n    signature_type: {signature_type}\n    code: '{code}'\n"
            )
        }
    }

    #[test]
    fn combined_signature_and_flattened_sketch_yaml_is_accepted() {
        let catalog = TestCatalog::new()
            .with_file(
                "main.YmL",
                r#"
root: ../src
files:
  - utils.rs
signatures:
  - parse_positive:
      file: utils.rs
      signature_type: function
      name: parse_positive
      visibility: public(crate)
      parameters:
        - input: "&str"
      sketch: parse_positive_body
sketches:
  - parse_positive_body:
    signature_type: function
    code: |
      fn parse_positive(input: &str) -> i32 {
          input.parse().unwrap()
      }
"#,
            )
            .into_catalog();

        let contracts = SketchContracts::from_catalog(catalog).expect("parse");
        let entry = contracts.entries().first().expect("sketch entry");

        assert_eq!(contracts.len(), 1);
        assert_eq!(contracts.contract_file_count(), 1);
        assert_eq!(entry.id().as_str(), "parse_positive_body");
        assert_eq!(entry.contract_file().as_str(), "main.YmL");
        assert_eq!(entry.file().as_str(), "utils.rs");
        assert_eq!(entry.linked_signature.as_str(), "parse_positive");
        assert_eq!(entry.signature_type().as_str(), "function");
        assert!(!entry.snippet().normalized().is_empty());
    }

    #[test]
    fn sketch_snippet_retains_its_normalized_value() {
        let catalog_name = CatalogPath::new("main.yml").expect("catalog path");
        let snippet = SketchSnippet::new("  let   value = 42;  ", &catalog_name, "answer")
            .expect("valid sketch snippet");

        assert_eq!(
            snippet.normalized(),
            &NormalizedSnippet::from_code("let value = 42;")
        );
    }

    #[test]
    fn combined_document_without_sketch_links_is_valid() {
        let catalog = TestCatalog::new()
            .with_file(
                "main.yml",
                "root: ../src\nfiles: [lib.rs]\nsignatures:\n  - answer:\n      file: lib.rs\n      signature_type: function\n      name: answer\nsketches: []\n",
            )
            .into_catalog();

        let contracts = SketchContracts::from_catalog(catalog).expect("parse");

        assert!(contracts.entries().is_empty());
        assert_eq!(contracts.contract_file_count(), 1);
    }

    #[test]
    fn nested_yaml_entries_are_not_contract_documents() {
        let catalog = TestCatalog::new()
            .with_file(
                "nested/invalid.yml",
                "version: 1\nlanguage: rust\nthis is not the combined format\n",
            )
            .with_file(
                "main.yml",
                "root: ../src\nfiles: [lib.rs]\nsignatures: []\nsketches: []\n",
            )
            .into_catalog();

        let contracts = SketchContracts::from_catalog(catalog).expect("parse root document");

        assert!(contracts.entries().is_empty());
        assert_eq!(contracts.contract_file_count(), 1);
    }

    #[test]
    fn later_versioned_reverse_link_dialect_is_rejected() {
        let catalog = TestCatalog::new()
            .with_file(
                "main.yml",
                "version: 1\nlanguage: rust\nsketches:\n- answer:\n    file: lib.rs\n    signature: answer\n    code: fn answer() {}\n",
            )
            .into_catalog();

        let error = SketchContracts::from_catalog(catalog).expect_err("later dialect");

        assert!(error.to_string().contains("unknown field `version`"));
    }

    #[test]
    fn reverse_link_body_is_rejected_even_in_combined_document() {
        let catalog = TestCatalog::new()
            .with_file(
                "main.yml",
                r#"
root: ../src
files: [lib.rs]
signatures:
  - answer:
      file: lib.rs
      signature_type: function
      sketch: answer_body
sketches:
  - answer_body:
      file: lib.rs
      signature: answer
      code: fn answer() {}
"#,
            )
            .into_catalog();

        let error = SketchContracts::from_catalog(catalog).expect_err("reverse body");

        assert!(
            error
                .to_string()
                .contains("one null identifier, signature_type, and code")
        );
    }

    #[test]
    fn orphan_sketch_is_rejected() {
        let catalog = TestCatalog::new()
            .with_file(
                "main.yml",
                r#"
root: ../src
files: [lib.rs]
signatures:
  - answer:
      file: lib.rs
      signature_type: function
sketches:
  - answer_body:
    signature_type: function
    code: fn answer() {}
"#,
            )
            .into_catalog();

        let error = SketchContracts::from_catalog(catalog).expect_err("orphan");

        assert!(error.to_string().contains("orphan sketch answer_body"));
    }

    #[test]
    fn missing_linked_sketch_is_rejected() {
        let catalog = TestCatalog::new()
            .with_file(
                "main.yml",
                r#"
root: ../src
files: [lib.rs]
signatures:
  - answer:
      file: lib.rs
      signature_type: function
      sketch: answer_body
sketches: []
"#,
            )
            .into_catalog();

        let error = SketchContracts::from_catalog(catalog).expect_err("missing sketch");

        assert!(
            error
                .to_string()
                .contains("references missing sketch answer_body")
        );
    }

    #[test]
    fn multiply_referenced_sketch_is_rejected() {
        let catalog = TestCatalog::new()
            .with_file(
                "main.yml",
                r#"
root: ../src
files: [lib.rs]
signatures:
  - first:
      file: lib.rs
      signature_type: function
      sketch: shared
  - second:
      file: lib.rs
      signature_type: function
      sketch: shared
sketches:
  - shared:
    signature_type: function
    code: fn shared() {}
"#,
            )
            .into_catalog();

        let error = SketchContracts::from_catalog(catalog).expect_err("ambiguous link");

        assert!(
            error
                .to_string()
                .contains("referenced by more than one signature")
        );
    }

    #[test]
    fn signature_type_mismatch_is_rejected() {
        let yaml = ContractYaml::linked("answer", "answer_body", "function", "fn answer() {}");
        let yaml = yaml.replace(
            "signature_type: function\n    code",
            "signature_type: method\n    code",
        );
        let catalog = TestCatalog::new()
            .with_file("main.yml", &yaml)
            .into_catalog();

        let error = SketchContracts::from_catalog(catalog).expect_err("kind mismatch");

        assert!(error.to_string().contains("signature_type method"));
        assert!(error.to_string().contains("type function"));
    }

    #[test]
    fn cross_document_link_is_rejected() {
        let linked_signature = r#"
root: ../src
files: [lib.rs]
signatures:
  - answer:
      file: lib.rs
      signature_type: function
      sketch: answer_body
sketches: []
"#;
        let sketch = r#"
root: ../src
files: [other.rs]
signatures: []
sketches:
  - answer_body:
    signature_type: function
    code: fn answer() {}
"#;
        let catalog = TestCatalog::new()
            .with_file("a.yml", linked_signature)
            .with_file("b.yaml", sketch)
            .into_catalog();

        let error = SketchContracts::from_catalog(catalog).expect_err("cross-document link");

        assert!(error.to_string().contains("another contract document"));
    }

    #[test]
    fn global_duplicate_sketch_ids_are_rejected() {
        let first = ContractYaml::linked("first", "shared", "function", "fn first() {}");
        let second = ContractYaml::linked("second", "shared", "function", "fn second() {}")
            .replace("files: [lib.rs]", "files: [other.rs]")
            .replace("file: lib.rs", "file: other.rs");
        let catalog = TestCatalog::new()
            .with_file("a.yml", &first)
            .with_file("b.yml", &second)
            .into_catalog();

        let error = SketchContracts::from_catalog(catalog).expect_err("duplicate sketch");

        assert!(error.to_string().contains("duplicate sketch id shared"));
    }

    #[test]
    fn global_duplicate_signature_labels_are_rejected() {
        let first = ContractYaml::linked("answer", "first_body", "function", "fn first() {}");
        let second = ContractYaml::linked("answer", "second_body", "function", "fn second() {}")
            .replace("files: [lib.rs]", "files: [other.rs]")
            .replace("file: lib.rs", "file: other.rs");
        let catalog = TestCatalog::new()
            .with_file("a.yml", &first)
            .with_file("b.yml", &second)
            .into_catalog();

        let error = SketchContracts::from_catalog(catalog).expect_err("duplicate signature");

        assert!(
            error
                .to_string()
                .contains("duplicate signature label answer")
        );
    }

    #[test]
    fn overlapping_document_file_lists_are_rejected() {
        let catalog = TestCatalog::new()
            .with_file(
                "a.yml",
                "root: ../src\nfiles: [lib.rs]\nsignatures: []\nsketches: []\n",
            )
            .with_file(
                "b.yml",
                "root: ../src\nfiles: [lib.rs]\nsignatures: []\nsketches: []\n",
            )
            .into_catalog();

        let error = SketchContracts::from_catalog(catalog).expect_err("overlap");

        assert!(
            error
                .to_string()
                .contains("listed by more than one contract document")
        );
    }

    #[test]
    fn signature_file_must_be_listed_by_its_document() {
        let catalog = TestCatalog::new()
            .with_file(
                "main.yml",
                "root: ../src\nfiles: []\nsignatures:\n  - answer:\n      file: lib.rs\n      signature_type: function\nsketches: []\n",
            )
            .into_catalog();

        let error = SketchContracts::from_catalog(catalog).expect_err("unlisted file");

        assert!(
            error
                .to_string()
                .contains("references unlisted file lib.rs")
        );
    }

    #[test]
    fn unknown_root_fields_are_rejected() {
        let catalog = TestCatalog::new()
            .with_file(
                "main.yml",
                "root: ../src\nfiles: []\nsignatures: []\nsketches: []\nextra: false\n",
            )
            .into_catalog();

        let error = SketchContracts::from_catalog(catalog).expect_err("unknown root field");

        assert!(error.to_string().contains("unknown field `extra`"));
    }

    #[test]
    fn empty_and_whitespace_contract_roots_are_rejected() {
        for root in ["''", "'   '"] {
            let yaml = format!("root: {root}\nfiles: []\nsignatures: []\nsketches: []\n");
            let catalog = TestCatalog::new()
                .with_file("main.yml", &yaml)
                .into_catalog();

            let error =
                SketchContracts::from_catalog(catalog).expect_err("empty contract root must fail");

            assert!(
                error
                    .to_string()
                    .contains("contract root must not be empty")
            );
        }
    }

    #[test]
    fn semantic_diff_reports_added_removed_and_changed_sketches() {
        let previous = SketchContracts::from_catalog(
            TestCatalog::new()
                .with_file(
                    "previous.yml",
                    &ContractYaml::linked("same", "same_body", "function", "let value = 1;"),
                )
                .into_catalog(),
        )
        .expect("previous");
        let current = SketchContracts::from_catalog(
            TestCatalog::new()
                .with_file(
                    "current.yml",
                    &ContractYaml::linked("same", "same_body", "function", "let value = 2;"),
                )
                .into_catalog(),
        )
        .expect("current");

        let diff = current.diff_against(&previous);

        assert!(diff.changed);
        assert_eq!(
            diff.entries,
            vec![DiffEntry::Changed {
                sketch_id: "same_body".to_owned(),
            }]
        );
    }

    #[test]
    fn semantic_diff_ignores_document_relocation_and_whitespace_only_code_changes() {
        let previous = SketchContracts::from_catalog(
            TestCatalog::new()
                .with_file(
                    "previous.yml",
                    &ContractYaml::linked("same", "same_body", "function", "let value = 1;"),
                )
                .into_catalog(),
        )
        .expect("previous");
        let current = SketchContracts::from_catalog(
            TestCatalog::new()
                .with_file(
                    "current.YAML",
                    &ContractYaml::linked("same", "same_body", "function", "  let   value = 1;  "),
                )
                .into_catalog(),
        )
        .expect("current");

        let diff = current.diff_against(&previous);

        assert!(!diff.changed);
        assert!(diff.entries.is_empty());
    }
}
