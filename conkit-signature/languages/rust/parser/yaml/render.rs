use super::document::RustContractCatalogName;
use super::input::{RustYamlDocument, RustYamlSketch};
use super::type_text::RustTypeTextRenderer;
use crate::api::{ContractScope, GenerateResponse, GenerateTarget};
use crate::error::SignatureContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::languages::rust::parser::signature_id::{RustItemId, RustItemKind};
use crate::languages::rust::parser::{RustParsedEntry, RustParsedFiles, RustSignature};
use crate::languages::rust::types::base_type::BaseType;
use crate::languages::rust::types::callable_type::{
    RustCallableSignature, RustFunctionAbi, RustMethod, RustVariadicParameter,
};
use crate::languages::rust::types::enum_type::{EnumType, EnumVariant, EnumVariantField};
use crate::languages::rust::types::function_type::FunctionType;
use crate::languages::rust::types::impl_type::{
    ImplementationType, RustImplPolarity, RustImplementedTrait,
};
use crate::languages::rust::types::macro_type::MacroType;
use crate::languages::rust::types::primitive_types::{
    RustFunctionParameter, RustGenericMetadata, RustGenericParameter, RustType, Visibility,
};
use crate::languages::rust::types::static_type::StaticType;
use crate::languages::rust::types::struct_type::{StructField, StructType};
use crate::languages::rust::types::trait_type::TraitType;
use crate::languages::rust::types::type_alias_type::TypeAliasType;
use crate::languages::rust::types::union_type::UnionType;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::languages::rust::parser) struct RustYamlRenderer {
    parsed: RustParsedFiles,
    target: GenerateTarget,
    scope: ContractScope,
}

impl RustYamlRenderer {
    pub(in crate::languages::rust::parser) fn new(
        parsed: RustParsedFiles,
        target: GenerateTarget,
        scope: ContractScope,
    ) -> Self {
        Self {
            parsed,
            target,
            scope,
        }
    }

    pub(in crate::languages::rust::parser) fn render(
        self,
    ) -> Result<GenerateResponse, SignatureContractKitError> {
        let Self {
            parsed,
            target,
            scope,
        } = self;
        let groups = RustYamlSourceGroups::from_parsed_files(&parsed)?;
        let documents = match target {
            GenerateTarget::New(layout) => {
                vec![RustYamlGeneratedDocument::from_new(
                    layout, &parsed, &groups,
                )?]
            }
            GenerateTarget::Existing(files) => {
                RustYamlGeneratedDocument::from_existing(files, &parsed, &groups, scope)?
            }
        };
        Self::validate_documents(&documents)?;
        let signature_count = documents
            .iter()
            .map(|document| document.signatures.len())
            .sum();
        let sketch_count = documents
            .iter()
            .map(|document| document.sketches.len())
            .sum();
        let mut contract_files = FileCatalog::new();
        for document in documents {
            let (catalog_name, bytes) = document.render()?;
            contract_files.insert(catalog_name, bytes)?;
        }
        Ok(GenerateResponse {
            contract_files,
            signature_count,
            sketch_count,
        })
    }

    fn validate_documents(
        documents: &[RustYamlGeneratedDocument],
    ) -> Result<(), SignatureContractKitError> {
        let mut files = BTreeMap::<&str, &CatalogPath>::new();
        let mut labels = BTreeMap::<&str, &CatalogPath>::new();
        let mut sketches = BTreeMap::<&str, &CatalogPath>::new();
        for document in documents {
            for file in &document.files {
                if let Some(previous) = files.insert(file, &document.catalog_name) {
                    return Err(SignatureContractKitError::write_failed(
                        &document.catalog_name,
                        format!("source file {file} is also owned by {previous}"),
                    ));
                }
            }
            for signature in &document.signatures {
                for label in signature.keys() {
                    if let Some(previous) = labels.insert(label, &document.catalog_name) {
                        return Err(SignatureContractKitError::write_failed(
                            &document.catalog_name,
                            format!("signature label {label} is also used by {previous}"),
                        ));
                    }
                }
            }
            for sketch in &document.sketches {
                if let Some(previous) = sketches.insert(&sketch.id, &document.catalog_name) {
                    return Err(SignatureContractKitError::write_failed(
                        &document.catalog_name,
                        format!("sketch id {} is also used by {previous}", sketch.id),
                    ));
                }
            }
        }
        Ok(())
    }
}

struct RustYamlSourceGroup<'a> {
    primary: &'a RustParsedEntry,
    implementations: Vec<&'a RustParsedEntry>,
}

impl RustYamlSourceGroup<'_> {
    fn structural_key(&self) -> &RustItemId {
        self.primary.id()
    }

    fn file(&self) -> &CatalogPath {
        self.primary.id().file()
    }
}

struct RustYamlSourceGroups<'a> {
    groups: Vec<RustYamlSourceGroup<'a>>,
}

impl<'a> RustYamlSourceGroups<'a> {
    fn from_parsed_files(parsed: &'a RustParsedFiles) -> Result<Self, SignatureContractKitError> {
        let mut primary = Vec::new();
        let mut implementations = Vec::new();
        for file in parsed.files() {
            for entry in file.entries() {
                match entry.signature() {
                    RustSignature::Implementation(_) => implementations.push(entry),
                    _ => primary.push(RustYamlSourceGroup {
                        primary: entry,
                        implementations: Vec::new(),
                    }),
                }
            }
        }

        for implementation in implementations {
            let RustSignature::Implementation(value) = implementation.signature() else {
                return Err(SignatureContractKitError::conversion_failed(
                    "implementation group contains a non-implementation signature",
                ));
            };
            let candidate = primary.iter_mut().find(|group| {
                group.primary.id().file() == implementation.id().file()
                    && group.primary.id().module_path() == implementation.id().module_path()
                    && group.primary.id().name() == value.owner_type()
                    && matches!(
                        group.primary.signature(),
                        RustSignature::Struct(_)
                            | RustSignature::Enum(_)
                            | RustSignature::Union(_)
                            | RustSignature::TypeAlias(_)
                    )
            });
            let Some(group) = candidate else {
                return Err(SignatureContractKitError::conversion_failed(format!(
                    "cannot fold implementation {} into an owning type",
                    implementation.id().name()
                )));
            };
            group.implementations.push(implementation);
        }
        primary.sort_by(|left, right| left.structural_key().cmp(right.structural_key()));
        Ok(Self { groups: primary })
    }

    fn for_files(&self, files: &[CatalogPath]) -> Vec<&RustYamlSourceGroup<'a>> {
        self.groups
            .iter()
            .filter(|group| files.contains(group.file()))
            .collect()
    }
}

#[derive(Serialize)]
struct RustYamlGeneratedDocument {
    #[serde(skip)]
    catalog_name: CatalogPath,
    root: String,
    files: Vec<String>,
    signatures: Vec<BTreeMap<String, RustYamlShorthandSignatureOutput>>,
    sketches: Vec<RustYamlSketch>,
}

impl RustYamlGeneratedDocument {
    fn from_new(
        layout: crate::api::GenerateDocument,
        parsed: &RustParsedFiles,
        groups: &RustYamlSourceGroups<'_>,
    ) -> Result<Self, SignatureContractKitError> {
        Self::validate_layout(&layout.contract_file, &layout.files, parsed)?;
        let selected = groups.for_files(&layout.files);
        let labels = RustYamlLabelPlanner::new().plan(&selected)?;
        let signatures = selected
            .iter()
            .map(|group| {
                let label = labels[group.structural_key()].clone();
                let output = RustYamlShorthandSignatureOutput::from_group(group, None, &label)?;
                Ok(BTreeMap::from([(label, output)]))
            })
            .collect::<Result<Vec<_>, SignatureContractKitError>>()?;
        Ok(Self {
            catalog_name: layout.contract_file,
            root: layout.root,
            files: layout
                .files
                .into_iter()
                .map(|file| file.as_str().to_owned())
                .collect(),
            signatures,
            sketches: Vec::new(),
        })
    }

    fn from_existing(
        catalog: FileCatalog,
        parsed: &RustParsedFiles,
        groups: &RustYamlSourceGroups<'_>,
        scope: ContractScope,
    ) -> Result<Vec<Self>, SignatureContractKitError> {
        let existing_documents = catalog
            .into_entries()
            .filter(|(name, _)| RustContractCatalogName::new(name).is_root_yaml())
            .map(|(catalog_name, bytes)| {
                RustYamlDocument::parse(&catalog_name, &bytes)
                    .map(|document| (catalog_name, document))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let all_groups = groups.groups.iter().collect::<Vec<_>>();
        let labels = RustYamlLabelPlanner::from_existing(&existing_documents)?.plan(&all_groups)?;
        let mut documents = Vec::new();
        for (catalog_name, existing) in existing_documents {
            Self::validate_layout(&catalog_name, &existing.files, parsed)?;
            let selected = groups.for_files(&existing.files);
            let existing_by_key = existing
                .signatures
                .iter()
                .filter_map(|signature| {
                    signature.entries.first().map(|entry| {
                        (
                            entry.id().clone(),
                            (signature.label.clone(), signature.sketch.clone()),
                        )
                    })
                })
                .collect::<BTreeMap<_, _>>();
            let selected_keys = selected
                .iter()
                .map(|group| group.structural_key())
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            let mut removed_sketches = std::collections::BTreeSet::new();
            for signature in &existing.signatures {
                let Some(entry) = signature.entries.first() else {
                    continue;
                };
                if !selected_keys.contains(entry.id())
                    && let Some(sketch) = &signature.sketch
                {
                    if scope == ContractScope::Signatures {
                        return Err(SignatureContractKitError::conversion_failed(format!(
                            "removing signature {} would orphan preserved sketch {sketch}",
                            signature.label
                        )));
                    }
                    removed_sketches.insert(sketch.clone());
                }
            }
            let signatures = selected
                .iter()
                .map(|group| {
                    let key = group.structural_key();
                    let (label, sketch) = existing_by_key
                        .get(key)
                        .cloned()
                        .unwrap_or_else(|| (labels[key].clone(), None));
                    let output =
                        RustYamlShorthandSignatureOutput::from_group(group, sketch, &label)?;
                    Ok(BTreeMap::from([(label, output)]))
                })
                .collect::<Result<Vec<_>, SignatureContractKitError>>()?;
            documents.push(Self {
                catalog_name,
                root: existing.root,
                files: existing
                    .files
                    .into_iter()
                    .map(|file| file.as_str().to_owned())
                    .collect(),
                signatures,
                sketches: existing
                    .sketches
                    .into_iter()
                    .filter(|sketch| !removed_sketches.contains(&sketch.id))
                    .collect(),
            });
        }
        Ok(documents)
    }

    fn validate_layout(
        catalog_name: &CatalogPath,
        files: &[CatalogPath],
        parsed: &RustParsedFiles,
    ) -> Result<(), SignatureContractKitError> {
        if !RustContractCatalogName::new(catalog_name).is_root_yaml() {
            return Err(SignatureContractKitError::write_failed(
                catalog_name,
                "combined contract file must be a direct root-level .yml or .yaml document",
            ));
        }
        let available = parsed
            .files()
            .iter()
            .map(|file| file.path())
            .collect::<std::collections::BTreeSet<_>>();
        let mut seen = std::collections::BTreeSet::new();
        for file in files {
            if !seen.insert(file) {
                return Err(SignatureContractKitError::write_failed(
                    catalog_name,
                    format!("duplicate listed source file {file}"),
                ));
            }
            if !available.contains(file) {
                return Err(SignatureContractKitError::write_failed(
                    catalog_name,
                    format!("listed source file {file} is missing from source catalog"),
                ));
            }
        }
        Ok(())
    }

    fn render(self) -> Result<(CatalogPath, Vec<u8>), SignatureContractKitError> {
        let catalog_name = self.catalog_name.clone();
        let rendered = serde_yaml::to_string(&self).map_err(|source| {
            SignatureContractKitError::write_failed(&catalog_name, source.to_string())
        })?;
        let mut normalized = String::with_capacity(rendered.len());
        for line in rendered.split_inclusive('\n') {
            if line.starts_with("- ") && line.ends_with(": null\n") {
                normalized.push_str(line.trim_end_matches(" null\n"));
                normalized.push('\n');
            } else {
                normalized.push_str(line);
            }
        }
        Ok((catalog_name, normalized.into_bytes()))
    }
}

struct RustYamlLabelPlanner {
    retained: BTreeMap<RustItemId, String>,
    reserved: BTreeSet<String>,
}

impl RustYamlLabelPlanner {
    fn new() -> Self {
        Self {
            retained: BTreeMap::new(),
            reserved: BTreeSet::new(),
        }
    }

    fn from_existing(
        documents: &[(CatalogPath, RustYamlDocument)],
    ) -> Result<Self, SignatureContractKitError> {
        let mut planner = Self::new();

        for (catalog_name, document) in documents {
            for signature in &document.signatures {
                planner.reserved.insert(signature.label.clone());

                let Some(primary) = signature.entries.first() else {
                    continue;
                };
                let structural_key = primary.id().clone();
                if let Some(previous) = planner
                    .retained
                    .insert(structural_key.clone(), signature.label.clone())
                    && previous != signature.label
                {
                    return Err(SignatureContractKitError::write_failed(
                        catalog_name,
                        format!(
                            "structural signature {} uses both {previous} and {}",
                            structural_key.render(),
                            signature.label
                        ),
                    ));
                }
            }
        }

        Ok(planner)
    }

    fn plan(
        &self,
        groups: &[&RustYamlSourceGroup<'_>],
    ) -> Result<BTreeMap<RustItemId, String>, SignatureContractKitError> {
        let mut used = self.reserved.clone();
        let mut labels = BTreeMap::new();
        for group in groups {
            let structural_key = group.structural_key();
            if let Some(retained) = self.retained.get(structural_key) {
                labels.insert(structural_key.clone(), retained.clone());
                continue;
            }

            let base = Self::base(group);
            let qualified = self.qualified(group, &base);
            let label = self.allocate(base, qualified, &mut used)?;
            labels.insert(structural_key.clone(), label);
        }
        Ok(labels)
    }

    fn qualified(&self, group: &RustYamlSourceGroup<'_>, base: &str) -> String {
        let prefix = format!(
            "{}_{}_{}",
            group.primary.id().file().as_str(),
            group.primary.id().module_path().join("_"),
            base
        );
        RustYamlSanitizedLabel::new(&prefix).into_string()
    }

    fn allocate(
        &self,
        base: String,
        qualified: String,
        used: &mut BTreeSet<String>,
    ) -> Result<String, SignatureContractKitError> {
        if used.insert(base.clone()) {
            return Ok(base);
        }
        if used.insert(qualified.clone()) {
            return Ok(qualified);
        }

        let mut ordinal = 2_usize;
        loop {
            let candidate = format!("{qualified}_{ordinal}");
            if used.insert(candidate.clone()) {
                return Ok(candidate);
            }
            ordinal = ordinal.checked_add(1).ok_or_else(|| {
                SignatureContractKitError::conversion_failed(
                    "generated signature label space is exhausted",
                )
            })?;
        }
    }

    fn base(group: &RustYamlSourceGroup<'_>) -> String {
        let name = RustYamlSanitizedLabel::new(group.primary.id().name()).into_string();
        match group.primary.id().kind() {
            RustItemKind::Function if group.primary.id().name() == "main" => "main".to_owned(),
            RustItemKind::Function => format!("{name}_function"),
            RustItemKind::Struct => format!("{name}_struct"),
            RustItemKind::Enum => format!("{name}_enum"),
            RustItemKind::Trait => format!("{name}_trait"),
            RustItemKind::Union => format!("{name}_union"),
            RustItemKind::Static => format!("{name}_static"),
            RustItemKind::Macro => format!("{name}_macro"),
            RustItemKind::TypeAlias => format!("{name}_type_alias"),
            RustItemKind::Implementation => name,
        }
    }
}

struct RustYamlSanitizedLabel {
    value: String,
}

impl RustYamlSanitizedLabel {
    fn new(value: &str) -> Self {
        let mut output = String::new();
        for character in value.chars() {
            if character.is_ascii_alphanumeric() {
                output.push(character.to_ascii_lowercase());
            } else if !output.ends_with('_') {
                output.push('_');
            }
        }
        Self {
            value: output.trim_matches('_').to_owned(),
        }
    }

    fn into_string(self) -> String {
        self.value
    }
}

#[derive(Serialize)]
struct RustYamlShorthandSignatureOutput {
    file: String,
    #[serde(rename = "signature_type")]
    signature_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    module_path: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    visibility: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    derives: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generics: Option<Vec<String>>,
    #[serde(rename = "where", skip_serializing_if = "Option::is_none")]
    where_predicates: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fields: Option<RustYamlShorthandFieldsOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    variants: Option<Vec<RustYamlShorthandVariantOutput>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    methods: Option<Vec<RustYamlShorthandMethodOutput>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    implementations: Option<Vec<RustYamlShorthandImplementationOutput>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    supertraits: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    qualifiers: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    abi: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    variadic: Option<RustYamlShorthandVariadicOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<Vec<BTreeMap<String, String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    return_type: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    type_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mutable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sketch: Option<String>,
}

impl RustYamlShorthandSignatureOutput {
    fn from_group(
        group: &RustYamlSourceGroup<'_>,
        sketch: Option<String>,
        label: &str,
    ) -> Result<Self, SignatureContractKitError> {
        let mut output = Self::from_entry(group.primary)?;
        if label != group.primary.id().name() {
            output.name = Some(group.primary.id().name().to_owned());
        }
        let mut methods = output.methods.take().unwrap_or_default();
        let mut implementations = Vec::new();
        for entry in &group.implementations {
            let RustSignature::Implementation(value) = entry.signature() else {
                return Err(SignatureContractKitError::conversion_failed(
                    "implementation group contains a non-implementation signature",
                ));
            };
            methods.extend(
                value.methods().iter().map(|method| {
                    RustYamlShorthandMethodOutput::from_implementation(method, value)
                }),
            );
            if value.methods().is_empty() {
                implementations.push(RustYamlShorthandImplementationOutput::from_implementation(
                    value,
                ));
            }
        }
        output.methods = (!methods.is_empty()).then_some(methods);
        output.implementations = (!implementations.is_empty()).then_some(implementations);
        output.sketch = sketch;
        Ok(output)
    }

    fn from_entry(entry: &RustParsedEntry) -> Result<Self, SignatureContractKitError> {
        Ok(match entry.signature() {
            RustSignature::Function(function) => Self::function(entry.id(), function),
            RustSignature::Struct(value) => Self::structure(entry.id(), value),
            RustSignature::Enum(value) => Self::enumeration(entry.id(), value),
            RustSignature::Trait(value) => Self::trait_signature(entry.id(), value),
            RustSignature::Union(value) => Self::union(entry.id(), value),
            RustSignature::Static(value) => Self::static_signature(entry.id(), value),
            RustSignature::Macro(value) => Self::macro_signature(entry.id(), value),
            RustSignature::TypeAlias(value) => Self::type_alias(entry.id(), value),
            RustSignature::Implementation(_) => {
                return Err(SignatureContractKitError::conversion_failed(
                    "implementation entries cannot be top-level contract signatures",
                ));
            }
        })
    }

    fn function(id: &RustItemId, function: &FunctionType) -> Self {
        let signature_type = if function.base().name() == "main" {
            "main_method"
        } else {
            "function"
        };
        let mut output = Self::base(id, signature_type);
        output.apply_metadata(function.base());
        if function.base().name() == "main" {
            output.visibility = None;
        }
        output.apply_callable(function.signature());
        output
    }

    fn structure(id: &RustItemId, value: &StructType) -> Self {
        let mut output = Self::base(id, "struct");
        output.apply_metadata(value.base());
        output.apply_generics(value.generics());
        output.fields = RustYamlShorthandFieldsOutput::from_fields(value.fields());
        output.methods = RustYamlShorthandMethodOutput::from_trait_methods(value.methods());
        output
    }

    fn enumeration(id: &RustItemId, value: &EnumType) -> Self {
        let mut output = Self::base(id, "enum");
        output.apply_metadata(value.base());
        output.apply_generics(value.generics());
        output.variants = Some(
            value
                .variants()
                .iter()
                .map(RustYamlShorthandVariantOutput::from_variant)
                .collect(),
        )
        .filter(|variants: &Vec<_>| !variants.is_empty());
        output.methods = RustYamlShorthandMethodOutput::from_trait_methods(value.methods());
        output
    }

    fn trait_signature(id: &RustItemId, value: &TraitType) -> Self {
        let mut output = Self::base(id, "trait");
        output.apply_metadata(value.base());
        output.apply_generics(value.generics());
        output.supertraits =
            Some(value.supertraits().to_vec()).filter(|supertraits| !supertraits.is_empty());
        output.methods = RustYamlShorthandMethodOutput::from_trait_methods(value.methods());
        output
    }

    fn union(id: &RustItemId, value: &UnionType) -> Self {
        let mut output = Self::base(id, "union");
        output.apply_metadata(value.base());
        output.apply_generics(value.generics());
        output.fields = RustYamlShorthandFieldsOutput::from_fields(value.fields());
        output
    }

    fn static_signature(id: &RustItemId, value: &StaticType) -> Self {
        let mut output = Self::base(id, "static");
        output.apply_metadata(value.base());
        output.type_text = Some(Self::type_text(value.static_type()));
        output.mutable = value.mutable().then_some(true);
        output
    }

    fn macro_signature(id: &RustItemId, value: &MacroType) -> Self {
        let mut output = Self::base(id, "macro");
        output.apply_metadata(value.base());
        output.tokens = (!value.tokens().is_empty()).then(|| value.tokens().to_owned());
        output
    }

    fn type_alias(id: &RustItemId, value: &TypeAliasType) -> Self {
        let mut output = Self::base(id, "type_alias");
        output.apply_metadata(value.base());
        output.apply_generics(value.generics());
        output.target_type = Some(Self::type_text(value.target_type()));
        output
    }

    fn base(id: &RustItemId, signature_type: &str) -> Self {
        Self {
            file: id.file().as_str().to_owned(),
            signature_type: signature_type.to_owned(),
            name: None,
            module_path: id.module_path().to_vec(),
            visibility: None,
            derives: Vec::new(),
            generics: None,
            where_predicates: None,
            fields: None,
            variants: None,
            methods: None,
            implementations: None,
            supertraits: None,
            qualifiers: Vec::new(),
            abi: None,
            variadic: None,
            parameters: None,
            return_type: None,
            type_text: None,
            target_type: None,
            mutable: None,
            tokens: None,
            sketch: None,
        }
    }

    fn apply_metadata(&mut self, metadata: &BaseType) {
        self.visibility = Some(Self::visibility(metadata.visibility()));
        self.derives = metadata.derives().to_vec();
    }

    fn apply_generics(&mut self, metadata: &RustGenericMetadata) {
        self.generics = Self::generic_parameters(metadata);
        self.where_predicates = Self::where_predicates(metadata);
    }

    fn apply_callable(&mut self, callable: &RustCallableSignature) {
        self.apply_generics(callable.generics());
        self.qualifiers = Self::callable_qualifiers(callable);
        self.abi = Self::abi(callable.abi());
        self.variadic = callable
            .variadic()
            .map(RustYamlShorthandVariadicOutput::from_variadic);
        self.parameters = Self::parameters(callable.parameters());
        self.return_type = callable.return_type().map(Self::type_text);
    }

    fn visibility(visibility: &Visibility) -> String {
        match visibility {
            Visibility::Public => "public".to_owned(),
            Visibility::PublicCrate => "public(crate)".to_owned(),
            Visibility::Restricted(value) => value.clone(),
            Visibility::Private => "private".to_owned(),
        }
    }

    fn generic_parameters(metadata: &RustGenericMetadata) -> Option<Vec<String>> {
        Some(
            metadata
                .parameters()
                .iter()
                .map(Self::generic_parameter)
                .collect(),
        )
        .filter(|parameters: &Vec<_>| !parameters.is_empty())
    }

    fn where_predicates(metadata: &RustGenericMetadata) -> Option<Vec<String>> {
        Some(metadata.where_predicates().to_vec())
            .filter(|where_predicates| !where_predicates.is_empty())
    }

    fn generic_parameter(parameter: &RustGenericParameter) -> String {
        match parameter {
            RustGenericParameter::Type {
                name,
                bounds,
                default,
            } => Self::bounded_parameter(name.clone(), bounds, default.as_deref()),
            RustGenericParameter::Lifetime { name, bounds } => {
                Self::bounded_parameter(name.clone(), bounds, None)
            }
            RustGenericParameter::Const {
                name,
                parameter_type,
                default,
            } => Self::bounded_parameter(
                format!("const {name}: {parameter_type}"),
                &[],
                default.as_deref(),
            ),
        }
    }

    fn bounded_parameter(mut value: String, bounds: &[String], default: Option<&str>) -> String {
        if !bounds.is_empty() {
            value.push_str(": ");
            value.push_str(&bounds.join(" + "));
        }

        if let Some(default) = default {
            value.push_str(" = ");
            value.push_str(default);
        }

        value
    }

    fn callable_qualifiers(callable: &RustCallableSignature) -> Vec<&'static str> {
        let mut qualifiers = Vec::new();
        if callable.is_const() {
            qualifiers.push("const");
        }
        if callable.is_async() {
            qualifiers.push("async");
        }
        if callable.is_unsafe() {
            qualifiers.push("unsafe");
        }
        qualifiers
    }

    fn abi(abi: &RustFunctionAbi) -> Option<String> {
        match abi {
            RustFunctionAbi::Rust => None,
            RustFunctionAbi::Extern { name } => {
                Some(name.clone().unwrap_or_else(|| "extern".to_owned()))
            }
        }
    }

    fn parameters(parameters: &[RustFunctionParameter]) -> Option<Vec<BTreeMap<String, String>>> {
        Some(
            parameters
                .iter()
                .map(|parameter| {
                    BTreeMap::from([(
                        parameter.name().unwrap_or("_").to_owned(),
                        Self::type_text(parameter.parameter_type()),
                    )])
                })
                .collect(),
        )
        .filter(|parameters: &Vec<_>| !parameters.is_empty())
    }

    fn type_text(value: &RustType) -> String {
        RustTypeTextRenderer.render_type(value)
    }
}

enum RustYamlShorthandFieldsOutput {
    Named(Vec<RustYamlShorthandNamedFieldOutput>),
    Unnamed(Vec<RustYamlShorthandFieldValueOutput>),
}

struct RustYamlShorthandNamedFieldOutput {
    name: String,
    value: RustYamlShorthandFieldValueOutput,
}

impl Serialize for RustYamlShorthandFieldsOutput {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Named(fields) => {
                use serde::ser::SerializeMap;
                let mut mapping = serializer.serialize_map(Some(fields.len()))?;
                for field in fields {
                    mapping.serialize_entry(&field.name, &field.value)?;
                }
                mapping.end()
            }
            Self::Unnamed(fields) => fields.serialize(serializer),
        }
    }
}

impl RustYamlShorthandFieldsOutput {
    fn from_fields(fields: &[StructField]) -> Option<Self> {
        if fields.is_empty() {
            return None;
        }
        let named = fields
            .iter()
            .map(|field| {
                field.name().map(|name| RustYamlShorthandNamedFieldOutput {
                    name: name.to_owned(),
                    value: RustYamlShorthandFieldValueOutput::from_field(field),
                })
            })
            .collect::<Option<Vec<_>>>();
        if let Some(fields) = named {
            return Some(Self::Named(fields));
        }
        Some(Self::Unnamed(
            fields
                .iter()
                .map(RustYamlShorthandFieldValueOutput::from_field)
                .collect(),
        ))
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum RustYamlShorthandFieldValueOutput {
    Type(String),
    Details(RustYamlShorthandFieldDetailsOutput),
}

impl RustYamlShorthandFieldValueOutput {
    fn from_field(field: &StructField) -> Self {
        if matches!(field.visibility(), Visibility::Private) {
            return Self::Type(RustYamlShorthandSignatureOutput::type_text(
                field.field_type(),
            ));
        }

        Self::Details(RustYamlShorthandFieldDetailsOutput {
            field_type: RustYamlShorthandSignatureOutput::type_text(field.field_type()),
            visibility: RustYamlShorthandSignatureOutput::visibility(field.visibility()),
        })
    }
}

#[derive(Serialize)]
struct RustYamlShorthandFieldDetailsOutput {
    #[serde(rename = "type")]
    field_type: String,
    visibility: String,
}

#[derive(Serialize)]
#[serde(untagged)]
enum RustYamlShorthandVariadicOutput {
    Present(bool),
    Details(RustYamlShorthandVariadicDetailsOutput),
}

impl RustYamlShorthandVariadicOutput {
    fn from_variadic(value: &RustVariadicParameter) -> Self {
        if value.pattern().is_none() && value.attributes().is_empty() {
            return Self::Present(true);
        }

        Self::Details(RustYamlShorthandVariadicDetailsOutput {
            pattern: value.pattern().map(str::to_owned),
            attributes: value.attributes().to_vec(),
        })
    }
}

#[derive(Serialize)]
struct RustYamlShorthandVariadicDetailsOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pattern: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    attributes: Vec<String>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum RustYamlShorthandVariantOutput {
    Unit(String),
    Tuple(BTreeMap<String, Vec<String>>),
    Details(BTreeMap<String, RustYamlShorthandVariantDetailsOutput>),
}

impl RustYamlShorthandVariantOutput {
    fn from_variant(variant: &EnumVariant) -> Self {
        let fields = RustYamlShorthandVariantFieldsOutput::from_fields(variant.fields());
        let discriminant = variant.discriminant().map(str::to_owned);

        if fields.is_none() && discriminant.is_none() {
            return Self::Unit(variant.name().to_owned());
        }

        if let Some(RustYamlShorthandVariantFieldsOutput::Tuple(fields)) = fields
            && discriminant.is_none()
        {
            return Self::Tuple(BTreeMap::from([(variant.name().to_owned(), fields)]));
        }

        Self::Details(BTreeMap::from([(
            variant.name().to_owned(),
            RustYamlShorthandVariantDetailsOutput {
                fields: RustYamlShorthandVariantFieldsOutput::from_fields(variant.fields()),
                discriminant,
            },
        )]))
    }
}

#[derive(Serialize)]
struct RustYamlShorthandVariantDetailsOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    fields: Option<RustYamlShorthandVariantFieldsOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    discriminant: Option<String>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum RustYamlShorthandVariantFieldsOutput {
    Tuple(Vec<String>),
    Named(Vec<BTreeMap<String, String>>),
}

impl RustYamlShorthandVariantFieldsOutput {
    fn from_fields(fields: &[EnumVariantField]) -> Option<Self> {
        if fields.is_empty() {
            return None;
        }

        if fields.iter().all(|field| field.name().is_none()) {
            return Some(Self::Tuple(
                fields
                    .iter()
                    .map(|field| RustYamlShorthandSignatureOutput::type_text(field.field_type()))
                    .collect(),
            ));
        }

        Some(Self::Named(
            fields
                .iter()
                .filter_map(|field| {
                    field.name().map(|name| {
                        BTreeMap::from([(
                            name.to_owned(),
                            RustYamlShorthandSignatureOutput::type_text(field.field_type()),
                        )])
                    })
                })
                .collect(),
        ))
    }
}

#[derive(Serialize)]
struct RustYamlShorthandMethodOutput {
    #[serde(rename = "signature_type")]
    signature_type: &'static str,
    name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    derives: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    receiver: Option<String>,
    visibility: String,
    #[serde(rename = "trait", skip_serializing_if = "Option::is_none")]
    implemented_trait: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    impl_qualifiers: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    impl_generics: Option<Vec<String>>,
    #[serde(rename = "impl_where", skip_serializing_if = "Option::is_none")]
    impl_where_predicates: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    qualifiers: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    abi: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    variadic: Option<RustYamlShorthandVariadicOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generics: Option<Vec<String>>,
    #[serde(rename = "where", skip_serializing_if = "Option::is_none")]
    where_predicates: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<Vec<BTreeMap<String, String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    return_type: Option<String>,
}

impl RustYamlShorthandMethodOutput {
    fn from_trait_methods(methods: &[RustMethod]) -> Option<Vec<Self>> {
        Some(
            methods
                .iter()
                .map(|method| Self::from_method(method, None))
                .collect(),
        )
        .filter(|methods: &Vec<_>| !methods.is_empty())
    }

    fn from_implementation(method: &RustMethod, implementation: &ImplementationType) -> Self {
        Self::from_method(method, Some(implementation))
    }

    fn from_method(method: &RustMethod, implementation: Option<&ImplementationType>) -> Self {
        let function = method.function();
        let callable = function.signature();

        Self {
            signature_type: "method",
            name: function.base().name().to_owned(),
            derives: function.base().derives().to_vec(),
            receiver: method.receiver().map(Self::receiver),
            visibility: RustYamlShorthandSignatureOutput::visibility(method.visibility()),
            implemented_trait: implementation
                .and_then(RustYamlShorthandImplementationOutput::implemented_trait),
            impl_qualifiers: implementation
                .map(RustYamlShorthandImplementationOutput::qualifiers)
                .unwrap_or_default(),
            impl_generics: implementation.and_then(|value| {
                RustYamlShorthandSignatureOutput::generic_parameters(value.generics())
            }),
            impl_where_predicates: implementation.and_then(|value| {
                RustYamlShorthandSignatureOutput::where_predicates(value.generics())
            }),
            qualifiers: RustYamlShorthandSignatureOutput::callable_qualifiers(callable),
            abi: RustYamlShorthandSignatureOutput::abi(callable.abi()),
            variadic: callable
                .variadic()
                .map(RustYamlShorthandVariadicOutput::from_variadic),
            generics: RustYamlShorthandSignatureOutput::generic_parameters(callable.generics()),
            where_predicates: RustYamlShorthandSignatureOutput::where_predicates(
                callable.generics(),
            ),
            parameters: RustYamlShorthandSignatureOutput::parameters(callable.parameters()),
            return_type: callable
                .return_type()
                .map(RustYamlShorthandSignatureOutput::type_text),
        }
    }

    fn receiver(value: &str) -> String {
        match value {
            "& self" => "ref".to_owned(),
            "& mut self" => "mut".to_owned(),
            value => value.to_owned(),
        }
    }
}

#[derive(Serialize)]
struct RustYamlShorthandImplementationOutput {
    #[serde(rename = "trait", skip_serializing_if = "Option::is_none")]
    implemented_trait: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    impl_qualifiers: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generics: Option<Vec<String>>,
    #[serde(rename = "where", skip_serializing_if = "Option::is_none")]
    where_predicates: Option<Vec<String>>,
}

impl RustYamlShorthandImplementationOutput {
    fn from_implementation(value: &ImplementationType) -> Self {
        Self {
            implemented_trait: Self::implemented_trait(value),
            impl_qualifiers: Self::qualifiers(value),
            generics: RustYamlShorthandSignatureOutput::generic_parameters(value.generics()),
            where_predicates: RustYamlShorthandSignatureOutput::where_predicates(value.generics()),
        }
    }

    fn implemented_trait(value: &ImplementationType) -> Option<String> {
        match value.implemented_trait() {
            RustImplementedTrait::Inherent => None,
            RustImplementedTrait::Trait { name, polarity } => Some(match polarity {
                RustImplPolarity::Positive => name.clone(),
                RustImplPolarity::Negative => format!("!{name}"),
            }),
        }
    }

    fn qualifiers(value: &ImplementationType) -> Vec<&'static str> {
        let mut qualifiers = Vec::new();
        if value.is_unsafe() {
            qualifiers.push("unsafe");
        }
        if value.is_default() {
            qualifiers.push("default");
        }
        qualifiers
    }
}
