mod inventory_builder;
mod item_converter;
mod signature_id;
mod type_converter;
mod visibility_converter;
mod yaml;

use crate::api::{
    ContractScope, GenerateResponse, GenerateTarget, ResolveSketchesRequest,
    ResolveSketchesResponse,
};
use crate::error::SignatureContractKitError;
use crate::files::{CatalogPath, FileCatalog};
use crate::inventory::{SignatureDigest, SignatureEntry, SignatureId, SignatureInventory};
use crate::languages::rust::parser::inventory_builder::RustInventoryBuilder;
use crate::languages::rust::parser::signature_id::{
    RustImplementationId, RustItemId, RustItemKind,
};
use crate::languages::rust::source::RustSourceFile;
use crate::languages::rust::types::enum_type::{EnumCanonical, EnumType};
use crate::languages::rust::types::function_type::{FunctionCanonical, FunctionType};
use crate::languages::rust::types::impl_type::{ImplementationCanonical, ImplementationType};
use crate::languages::rust::types::macro_type::{MacroCanonical, MacroType};
use crate::languages::rust::types::primitive_types::{
    RustGenericMetadata, RustGenericParameter, Visibility,
};
use crate::languages::rust::types::static_type::{StaticCanonical, StaticType};
use crate::languages::rust::types::struct_type::{StructCanonical, StructType};
use crate::languages::rust::types::trait_type::{TraitCanonical, TraitType};
use crate::languages::rust::types::type_alias_type::{TypeAliasCanonical, TypeAliasType};
use crate::languages::rust::types::union_type::{UnionCanonical, UnionType};
use crate::work::AsyncWorkPool;
use rayon::prelude::*;
use serde::Serialize;
use std::collections::BTreeSet;

pub(crate) struct SignatureParser {
    inner: SignatureParserInner,
}

enum SignatureParserInner {
    Rust(RustParser),
}

impl Default for SignatureParser {
    fn default() -> Self {
        Self {
            inner: SignatureParserInner::Rust(RustParser),
        }
    }
}

struct RustSourceFiles {
    files: Vec<RustSourceFile>,
}

impl RustSourceFiles {
    fn from_catalog(catalog: FileCatalog) -> Self {
        let files = catalog
            .into_entries()
            .filter(|(path, _)| path.has_extension("rs"))
            .map(|(path, bytes)| RustSourceFile::new(path, bytes))
            .collect();

        Self { files }
    }

    fn retain_allowlist(
        mut self,
        allowlist: &BTreeSet<CatalogPath>,
    ) -> Result<Self, SignatureContractKitError> {
        let mut found = BTreeSet::new();
        self.files.retain(|file| {
            if !allowlist.contains(file.path()) {
                return false;
            }

            found.insert(file.path().clone());
            true
        });

        if let Some(missing) = allowlist.iter().find(|path| !found.contains(*path)) {
            return Err(SignatureContractKitError::parse_failed(
                missing,
                "listed source file is missing from source catalog",
            ));
        }

        Ok(self)
    }

    fn parse_all(self) -> Result<SignatureInventory, SignatureContractKitError> {
        self.parse_files()?.into_inventory()
    }

    fn parse_all_for_yaml(self) -> Result<RustParsedFiles, SignatureContractKitError> {
        self.parse_files()
    }

    fn parse_files(self) -> Result<RustParsedFiles, SignatureContractKitError> {
        let mut files = self
            .files
            .into_par_iter()
            .map(RustSourceFile::parse_inventory)
            .collect::<Result<Vec<_>, _>>()?;

        files.sort_by(|left, right| left.path().cmp(right.path()));
        RustParsedFiles { files }.normalize_implementations()
    }
}

impl RustSourceFile {
    fn parse_inventory(self) -> Result<RustParsedInventory, SignatureContractKitError> {
        let (path, bytes) = self.into_parts();
        let source = std::str::from_utf8(&bytes)
            .map_err(|source| SignatureContractKitError::parse_failed(&path, source.to_string()))?;
        let syntax = syn::parse_file(source)
            .map_err(|source| SignatureContractKitError::parse_failed(&path, source.to_string()))?;

        RustInventoryBuilder::new(path).collect_file(syntax)
    }
}

struct RustParsedFiles {
    files: Vec<RustParsedInventory>,
}

impl RustParsedFiles {
    fn files(&self) -> &[RustParsedInventory] {
        &self.files
    }

    fn normalize_implementations(mut self) -> Result<Self, SignatureContractKitError> {
        let owners = RustOwnerIndex::from_parsed_files(&self);
        let mut implementation_blocks = Vec::new();
        for file in &mut self.files {
            let mut normalized = Vec::with_capacity(file.entries.len());
            for entry in std::mem::take(&mut file.entries) {
                match entry.signature() {
                    RustSignature::Implementation(implementation) => {
                        let owner = owners.resolve(entry.id(), implementation)?;
                        implementation_blocks.push(entry.into_owner_context(owner));
                    }
                    _ => normalized.push(entry),
                }
            }
            file.entries = normalized;
        }
        self.merge_implementation_blocks(implementation_blocks)?;
        Ok(self)
    }

    fn merge_implementation_blocks(
        &mut self,
        blocks: Vec<RustParsedEntry>,
    ) -> Result<(), SignatureContractKitError> {
        let mut grouped = std::collections::BTreeMap::<
            (CatalogPath, Vec<String>, String, Vec<u8>),
            RustParsedEntry,
        >::new();

        for entry in blocks {
            let descriptor = entry.implementation_descriptor_bytes()?;
            let key = (
                entry.id().file().clone(),
                entry.id().module_path().to_vec(),
                entry.id().name().to_owned(),
                descriptor,
            );
            match grouped.entry(key) {
                std::collections::btree_map::Entry::Occupied(mut group) => {
                    group.get_mut().merge_implementation(entry)?;
                }
                std::collections::btree_map::Entry::Vacant(group) => {
                    group.insert(entry);
                }
            }
        }

        let mut merged = Vec::with_capacity(grouped.len());
        for ((file, module_path, base_name, descriptor), mut entry) in grouped {
            entry.sort_implementation_methods()?;
            merged.push((file, module_path, base_name, descriptor, entry));
        }

        let mut collisions = std::collections::BTreeMap::new();
        for (file, module_path, base_name, _, _) in &merged {
            *collisions
                .entry((file.clone(), module_path.clone(), base_name.clone()))
                .or_insert(0_usize) += 1;
        }

        for (file, module_path, base_name, descriptor, mut entry) in merged {
            if collisions
                .get(&(file.clone(), module_path.clone(), base_name.clone()))
                .copied()
                .unwrap_or_default()
                > 1
            {
                entry.disambiguate_implementation(&descriptor);
            }
            let Some(owner_file) = self
                .files
                .iter_mut()
                .find(|candidate| candidate.path == file)
            else {
                return Err(SignatureContractKitError::conversion_failed(format!(
                    "normalized implementation owner file {file} is missing"
                )));
            };
            owner_file.entries.push(entry);
        }

        for file in &mut self.files {
            file.entries.sort_by(|left, right| left.id.cmp(&right.id));
        }
        Ok(())
    }

    fn into_inventory(self) -> Result<SignatureInventory, SignatureContractKitError> {
        let owner_groups = self
            .files
            .iter()
            .flat_map(RustParsedInventory::entries)
            .filter(|entry| {
                matches!(
                    entry.signature(),
                    RustSignature::Struct(_)
                        | RustSignature::Enum(_)
                        | RustSignature::Union(_)
                        | RustSignature::TypeAlias(_)
                )
            })
            .map(|entry| {
                (
                    (
                        entry.id().file().clone(),
                        entry.id().module_path().to_vec(),
                        entry.id().name().to_owned(),
                    ),
                    SignatureId::new(entry.id().render()),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut inventory = SignatureInventory::default();

        for file in self.files {
            for entry in file.entries {
                let group_id = match entry.signature() {
                    RustSignature::Implementation(value) => owner_groups
                        .get(&(
                            entry.id().file().clone(),
                            entry.id().module_path().to_vec(),
                            value.owner_type().to_owned(),
                        ))
                        .cloned()
                        .ok_or_else(|| {
                            SignatureContractKitError::conversion_failed(format!(
                                "normalized implementation owner {} is missing",
                                value.owner_type()
                            ))
                        })?,
                    _ => SignatureId::new(entry.id().render()),
                };
                inventory.insert(entry.into_signature_entry(group_id)?)?;
            }
        }

        Ok(inventory)
    }
}

struct RustParsedInventory {
    path: CatalogPath,
    entries: Vec<RustParsedEntry>,
}

impl RustParsedInventory {
    fn new(path: CatalogPath, entries: Vec<RustParsedEntry>) -> Self {
        Self { path, entries }
    }

    fn path(&self) -> &CatalogPath {
        &self.path
    }

    fn entries(&self) -> &[RustParsedEntry] {
        &self.entries
    }
}

struct RustParsedEntry {
    id: RustItemId,
    signature: RustSignature,
}

impl RustParsedEntry {
    fn new(id: RustItemId, signature: RustSignature) -> Self {
        Self { id, signature }
    }

    fn id(&self) -> &RustItemId {
        &self.id
    }

    fn signature(&self) -> &RustSignature {
        &self.signature
    }

    fn into_owner_context(self, owner: &RustOwnerCandidate) -> Self {
        let RustSignature::Implementation(implementation) = self.signature else {
            return self;
        };
        let implementation = implementation.into_owner_context(
            owner.id.name().to_owned(),
            owner.id.file().clone(),
            owner.id.module_path().to_vec(),
            owner.visibility.clone(),
        );
        let implementation_id = match implementation.implemented_trait() {
            crate::languages::rust::types::impl_type::RustImplementedTrait::Inherent => {
                RustImplementationId::inherent(owner.id.name().to_owned())
            }
            crate::languages::rust::types::impl_type::RustImplementedTrait::Trait {
                name,
                polarity,
            } => RustImplementationId::trait_impl(
                owner.id.name().to_owned(),
                name.clone(),
                *polarity,
            ),
        };

        Self {
            id: RustItemId::new(
                owner.id.file().clone(),
                owner.id.module_path().to_vec(),
                RustItemKind::Implementation,
                implementation_id.render(),
            ),
            signature: RustSignature::Implementation(implementation),
        }
    }

    fn implementation_descriptor_bytes(&self) -> Result<Vec<u8>, SignatureContractKitError> {
        let RustSignature::Implementation(implementation) = &self.signature else {
            return Err(SignatureContractKitError::conversion_failed(
                "only implementation entries have implementation descriptors",
            ));
        };
        implementation
            .descriptor_bytes()
            .map_err(|source| SignatureContractKitError::conversion_failed(source.to_string()))
    }

    fn merge_implementation(&mut self, incoming: Self) -> Result<(), SignatureContractKitError> {
        let RustSignature::Implementation(current) = &mut self.signature else {
            return Err(SignatureContractKitError::conversion_failed(
                "implementation group contains a non-implementation entry",
            ));
        };
        let RustSignature::Implementation(incoming) = incoming.signature else {
            return Err(SignatureContractKitError::conversion_failed(
                "implementation group contains a non-implementation entry",
            ));
        };
        current.append_methods(incoming);
        Ok(())
    }

    fn sort_implementation_methods(&mut self) -> Result<(), SignatureContractKitError> {
        let RustSignature::Implementation(implementation) = &mut self.signature else {
            return Err(SignatureContractKitError::conversion_failed(
                "implementation group contains a non-implementation entry",
            ));
        };
        implementation
            .sort_methods()
            .map_err(|source| SignatureContractKitError::conversion_failed(source.to_string()))
    }

    fn disambiguate_implementation(&mut self, descriptor: &[u8]) {
        let digest = SignatureDigest::from_canonical_bytes(descriptor);
        self.id = RustItemId::new(
            self.id.file().clone(),
            self.id.module_path().to_vec(),
            RustItemKind::Implementation,
            format!("{}#{}", self.id.name(), digest.as_str()),
        );
    }

    fn into_signature_entry(
        self,
        group_id: SignatureId,
    ) -> Result<SignatureEntry, SignatureContractKitError> {
        let id = self.id.into_signature_id();
        let canonical_bytes = self.signature.canonical_bytes()?;

        Ok(SignatureEntry::from_grouped_canonical_bytes(
            id,
            group_id,
            &canonical_bytes,
        ))
    }
}

struct RustOwnerCandidate {
    id: RustItemId,
    visibility: Visibility,
    crate_path: Vec<String>,
}

impl RustOwnerCandidate {
    fn from_entry(entry: &RustParsedEntry) -> Option<Self> {
        let base = match entry.signature() {
            RustSignature::Struct(value) => value.base(),
            RustSignature::Enum(value) => value.base(),
            RustSignature::Union(value) => value.base(),
            RustSignature::TypeAlias(value) => value.base(),
            _ => return None,
        };
        let mut crate_path = RustOwnerIndex::effective_module(entry.id());
        crate_path.push(entry.id().name().to_owned());
        Some(Self {
            id: entry.id().clone(),
            visibility: base.visibility().clone(),
            crate_path,
        })
    }
}

struct RustOwnerIndex {
    candidates: Vec<RustOwnerCandidate>,
}

impl RustOwnerIndex {
    fn from_parsed_files(files: &RustParsedFiles) -> Self {
        let mut candidates = files
            .files
            .iter()
            .flat_map(RustParsedInventory::entries)
            .filter_map(RustOwnerCandidate::from_entry)
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.id.cmp(&right.id));
        Self { candidates }
    }

    fn resolve<'a>(
        &'a self,
        implementation_id: &RustItemId,
        implementation: &ImplementationType,
    ) -> Result<&'a RustOwnerCandidate, SignatureContractKitError> {
        let reference =
            RustOwnerReference::new(implementation.owner_type(), implementation.generics())?;
        let segments = reference.segments.as_deref().unwrap_or_default();
        let matches = match segments.last() {
            Some(_) if segments.len() > 1 => {
                let targets = self.qualified_targets(implementation_id, segments)?;
                self.candidates
                    .iter()
                    .filter(|candidate| targets.contains(&candidate.crate_path))
                    .collect::<Vec<_>>()
            }
            Some(name) => self
                .candidates
                .iter()
                .filter(|candidate| candidate.id.name() == name)
                .collect(),
            None => Vec::new(),
        };

        match matches.as_slice() {
            [] => Err(SignatureContractKitError::conversion_failed(format!(
                "cannot resolve implementation owner {} to a source-declared struct, enum, union, or type alias",
                implementation.owner_type()
            ))),
            [owner] => {
                reference.ensure_supported_local_application(implementation.owner_type())?;
                Ok(*owner)
            }
            _ => Err(SignatureContractKitError::conversion_failed(format!(
                "implementation owner {} is ambiguous across source declarations",
                implementation.owner_type()
            ))),
        }
    }

    fn qualified_targets(
        &self,
        implementation_id: &RustItemId,
        segments: &[String],
    ) -> Result<Vec<Vec<String>>, SignatureContractKitError> {
        let mut module = Self::effective_module(implementation_id);
        match segments.first().map(String::as_str) {
            Some("crate") => Ok(vec![segments[1..].to_vec()]),
            Some("self") => {
                module.extend_from_slice(&segments[1..]);
                Ok(vec![module])
            }
            Some("super") => {
                let mut index = 0;
                while segments.get(index).map(String::as_str) == Some("super") {
                    if module.pop().is_none() {
                        return Err(SignatureContractKitError::conversion_failed(format!(
                            "implementation owner {} traverses above the source root",
                            segments.join("::")
                        )));
                    }
                    index += 1;
                }
                module.extend_from_slice(&segments[index..]);
                Ok(vec![module])
            }
            Some(_) => {
                let mut relative = module;
                relative.extend_from_slice(segments);
                Ok(vec![relative, segments.to_vec()])
            }
            None => Ok(Vec::new()),
        }
    }

    fn effective_module(id: &RustItemId) -> Vec<String> {
        let mut module = id
            .file()
            .as_str()
            .split('/')
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if let Some(file) = module.pop() {
            let stem = file.strip_suffix(".rs").unwrap_or(&file);
            if !matches!(stem, "lib" | "main" | "mod") {
                module.push(stem.to_owned());
            }
        }
        module.extend(id.module_path().iter().cloned());
        module
    }
}

struct RustOwnerReference {
    segments: Option<Vec<String>>,
    application: RustOwnerApplication,
}

impl RustOwnerReference {
    fn new(value: &str, generics: &RustGenericMetadata) -> Result<Self, SignatureContractKitError> {
        let owner = syn::parse_str::<syn::Type>(value).map_err(|source| {
            SignatureContractKitError::conversion_failed(format!(
                "failed to parse implementation owner {value}: {source}"
            ))
        })?;
        let (segments, application) = match owner {
            syn::Type::Path(value)
                if value.qself.is_none() && value.path.leading_colon.is_none() =>
            {
                let application = RustOwnerApplication::from_path(&value.path, generics);
                let segments = value
                    .path
                    .segments
                    .into_iter()
                    .map(|segment| segment.ident.to_string())
                    .collect();
                (Some(segments), application)
            }
            _ => (None, RustOwnerApplication::Unsupported),
        };
        Ok(Self {
            segments,
            application,
        })
    }

    fn ensure_supported_local_application(
        &self,
        owner_type: &str,
    ) -> Result<(), SignatureContractKitError> {
        if matches!(
            self.application,
            RustOwnerApplication::Bare | RustOwnerApplication::Identity
        ) {
            return Ok(());
        }
        Err(SignatureContractKitError::conversion_failed(format!(
            "unsupported local implementation owner application {owner_type}; use a bare local owner or apply its declared generic parameters unchanged and in order"
        )))
    }
}

enum RustOwnerApplication {
    Bare,
    Identity,
    Unsupported,
}

impl RustOwnerApplication {
    fn from_path(path: &syn::Path, generics: &RustGenericMetadata) -> Self {
        let Some(last) = path.segments.last() else {
            return Self::Unsupported;
        };
        if path
            .segments
            .iter()
            .take(path.segments.len().saturating_sub(1))
            .any(|segment| !matches!(segment.arguments, syn::PathArguments::None))
        {
            return Self::Unsupported;
        }

        match &last.arguments {
            syn::PathArguments::None => Self::Bare,
            syn::PathArguments::AngleBracketed(arguments)
                if arguments.args.len() == generics.parameters().len()
                    && generics.parameters().iter().zip(&arguments.args).all(
                        |(parameter, argument)| Self::argument_matches(parameter, argument),
                    ) =>
            {
                Self::Identity
            }
            syn::PathArguments::AngleBracketed(_) | syn::PathArguments::Parenthesized(_) => {
                Self::Unsupported
            }
        }
    }

    fn argument_matches(parameter: &RustGenericParameter, argument: &syn::GenericArgument) -> bool {
        match (parameter, argument) {
            (RustGenericParameter::Type { name, .. }, syn::GenericArgument::Type(value)) => {
                Self::type_path_name(value).as_deref() == Some(name)
            }
            (
                RustGenericParameter::Lifetime { name, .. },
                syn::GenericArgument::Lifetime(value),
            ) => value.to_string() == *name,
            (RustGenericParameter::Const { name, .. }, syn::GenericArgument::Const(value)) => {
                Self::const_path_name(value).as_deref() == Some(name)
            }
            (RustGenericParameter::Const { name, .. }, syn::GenericArgument::Type(value)) => {
                Self::type_path_name(value).as_deref() == Some(name)
            }
            _ => false,
        }
    }

    fn type_path_name(value: &syn::Type) -> Option<String> {
        let syn::Type::Path(value) = value else {
            return None;
        };
        if value.qself.is_some() {
            return None;
        }
        Self::path_name(&value.path)
    }

    fn const_path_name(value: &syn::Expr) -> Option<String> {
        match value {
            syn::Expr::Path(value) if value.qself.is_none() => Self::path_name(&value.path),
            syn::Expr::Block(value)
                if value.attrs.is_empty()
                    && matches!(value.block.stmts.as_slice(), [syn::Stmt::Expr(_, None)]) =>
            {
                let [syn::Stmt::Expr(value, None)] = value.block.stmts.as_slice() else {
                    return None;
                };
                Self::const_path_name(value)
            }
            _ => None,
        }
    }

    fn path_name(path: &syn::Path) -> Option<String> {
        if path.leading_colon.is_some() || path.segments.len() != 1 {
            return None;
        }
        let segment = path.segments.first()?;
        matches!(segment.arguments, syn::PathArguments::None).then(|| segment.ident.to_string())
    }
}

enum RustSignature {
    Function(FunctionType),
    Struct(StructType),
    Enum(EnumType),
    Trait(TraitType),
    Implementation(ImplementationType),
    Union(UnionType),
    Static(StaticType),
    Macro(MacroType),
    TypeAlias(TypeAliasType),
}

impl RustSignature {
    fn canonical_bytes(&self) -> Result<Vec<u8>, SignatureContractKitError> {
        serde_json::to_vec(&self.canonical_form())
            .map_err(|source| SignatureContractKitError::conversion_failed(source.to_string()))
    }

    fn canonical_form(&self) -> RustSignatureCanonicalForm {
        match self {
            Self::Function(value) => RustSignatureCanonicalForm::Function(value.canonical_form()),
            Self::Struct(value) => RustSignatureCanonicalForm::Struct(value.canonical_form()),
            Self::Enum(value) => RustSignatureCanonicalForm::Enum(value.canonical_form()),
            Self::Trait(value) => RustSignatureCanonicalForm::Trait(value.canonical_form()),
            Self::Implementation(value) => {
                RustSignatureCanonicalForm::Implementation(value.canonical_form())
            }
            Self::Union(value) => RustSignatureCanonicalForm::Union(value.canonical_form()),
            Self::Static(value) => RustSignatureCanonicalForm::Static(value.canonical_form()),
            Self::Macro(value) => RustSignatureCanonicalForm::Macro(value.canonical_form()),
            Self::TypeAlias(value) => RustSignatureCanonicalForm::TypeAlias(value.canonical_form()),
        }
    }
}

#[derive(Serialize)]
enum RustSignatureCanonicalForm {
    Function(FunctionCanonical),
    Struct(StructCanonical),
    Enum(EnumCanonical),
    Trait(TraitCanonical),
    Implementation(ImplementationCanonical),
    Union(UnionCanonical),
    Static(StaticCanonical),
    Macro(MacroCanonical),
    TypeAlias(TypeAliasCanonical),
}

pub(crate) trait SignatureParserBackend {
    async fn parse_check_inventories(
        &self,
        source_files: FileCatalog,
        contract_files: FileCatalog,
        work: AsyncWorkPool,
    ) -> Result<(SignatureInventory, SignatureInventory), SignatureContractKitError>;

    async fn parse_contract_inventory(
        &self,
        contract_files: FileCatalog,
        work: AsyncWorkPool,
    ) -> Result<SignatureInventory, SignatureContractKitError>;

    async fn generate_contract_files(
        &self,
        source_files: FileCatalog,
        target: GenerateTarget,
        scope: ContractScope,
        work: AsyncWorkPool,
    ) -> Result<GenerateResponse, SignatureContractKitError>;

    async fn resolve_sketches(
        &self,
        request: ResolveSketchesRequest,
        work: AsyncWorkPool,
    ) -> Result<ResolveSketchesResponse, SignatureContractKitError>;
}

impl SignatureParserBackend for SignatureParser {
    async fn parse_check_inventories(
        &self,
        source_files: FileCatalog,
        contract_files: FileCatalog,
        work: AsyncWorkPool,
    ) -> Result<(SignatureInventory, SignatureInventory), SignatureContractKitError> {
        match &self.inner {
            SignatureParserInner::Rust(inner) => {
                inner
                    .parse_check_inventories(source_files, contract_files, work)
                    .await
            }
        }
    }

    async fn parse_contract_inventory(
        &self,
        contract_files: FileCatalog,
        work: AsyncWorkPool,
    ) -> Result<SignatureInventory, SignatureContractKitError> {
        match &self.inner {
            SignatureParserInner::Rust(inner) => {
                inner.parse_contract_inventory(contract_files, work).await
            }
        }
    }

    async fn generate_contract_files(
        &self,
        source_files: FileCatalog,
        target: GenerateTarget,
        scope: ContractScope,
        work: AsyncWorkPool,
    ) -> Result<GenerateResponse, SignatureContractKitError> {
        match &self.inner {
            SignatureParserInner::Rust(inner) => {
                inner
                    .generate_contract_files(source_files, target, scope, work)
                    .await
            }
        }
    }

    async fn resolve_sketches(
        &self,
        request: ResolveSketchesRequest,
        work: AsyncWorkPool,
    ) -> Result<ResolveSketchesResponse, SignatureContractKitError> {
        match &self.inner {
            SignatureParserInner::Rust(inner) => inner.resolve_sketches(request, work).await,
        }
    }
}

struct RustParser;

impl SignatureParserBackend for RustParser {
    async fn parse_check_inventories(
        &self,
        source_files: FileCatalog,
        contract_files: FileCatalog,
        work: AsyncWorkPool,
    ) -> Result<(SignatureInventory, SignatureInventory), SignatureContractKitError> {
        work.submit(move || {
            let contracts = yaml::RustContractDocuments::parse(contract_files)?;
            let allowlist = contracts.source_allowlist();
            let source = RustSourceFiles::from_catalog(source_files)
                .retain_allowlist(&allowlist)?
                .parse_all()?;
            let contract = contracts.into_inventory()?;
            Ok((source, contract))
        })
        .into_result()
        .await
        .map_err(|source| SignatureContractKitError::worker_failed(source.to_string()))?
    }

    async fn parse_contract_inventory(
        &self,
        contract_files: FileCatalog,
        work: AsyncWorkPool,
    ) -> Result<SignatureInventory, SignatureContractKitError> {
        work.submit(move || yaml::RustContractDocuments::parse(contract_files)?.into_inventory())
            .into_result()
            .await
            .map_err(|source| SignatureContractKitError::worker_failed(source.to_string()))?
    }

    async fn generate_contract_files(
        &self,
        source_files: FileCatalog,
        target: GenerateTarget,
        scope: ContractScope,
        work: AsyncWorkPool,
    ) -> Result<GenerateResponse, SignatureContractKitError> {
        work.submit(move || {
            let parsed = RustSourceFiles::from_catalog(source_files).parse_all_for_yaml()?;

            yaml::RustYamlRenderer::new(parsed, target, scope).render()
        })
        .into_result()
        .await
        .map_err(|source| SignatureContractKitError::worker_failed(source.to_string()))?
    }

    async fn resolve_sketches(
        &self,
        request: ResolveSketchesRequest,
        work: AsyncWorkPool,
    ) -> Result<ResolveSketchesResponse, SignatureContractKitError> {
        work.submit(move || yaml::RustSketchResolver::new(request).resolve())
            .into_result()
            .await
            .map_err(|source| SignatureContractKitError::worker_failed(source.to_string()))?
    }
}
