use crate::error::SignatureContractKitError;
use crate::files::CatalogPath;
use crate::languages::rust::parser::source_graph::{RustModuleId, RustModulePath};
use crate::languages::rust::types::attributes::{RustAttributeCapability, RustAttributes};
use crate::languages::rust::types::base_type::BaseType;
use crate::languages::rust::types::callable_type::RustFunctionAbi;
use crate::languages::rust::types::enum_type::EnumType;
use crate::languages::rust::types::function_type::FunctionType;
use crate::languages::rust::types::impl_type::ImplementationType;
use crate::languages::rust::types::macro_type::{MacroType, RustMacroTokens};
use crate::languages::rust::types::primitive_types::{RustGenericMetadata, RustType};
use crate::languages::rust::types::static_type::StaticType;
use crate::languages::rust::types::struct_type::StructType;
use crate::languages::rust::types::syntax_text::RustSyntaxText;
use crate::languages::rust::types::trait_type::TraitType;
use crate::languages::rust::types::type_alias_type::TypeAliasType;
use crate::languages::rust::types::union_type::UnionType;
use crate::work::CancellationProbe;
use serde::Serialize;
use std::fmt;

/// One validated Rust identifier stored by declarations that do not own a
/// `BaseType`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct RustIdentifier(String);

impl RustIdentifier {
    pub(crate) fn new(
        value: String,
        role: &'static str,
    ) -> Result<Self, SignatureContractKitError> {
        let canonical =
            RustModulePath::canonical_declaration_segment(value.clone()).map_err(|error| {
                SignatureContractKitError::conversion_failed(format!(
                    "invalid Rust identifier for {role} {value:?}: {error}"
                ))
            })?;

        Ok(Self(canonical))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    fn new_ascii(value: String, role: &'static str) -> Result<Self, SignatureContractKitError> {
        let identifier = Self::new(value, role)?;
        if !identifier.as_str().is_ascii() {
            return Err(SignatureContractKitError::conversion_failed(format!(
                "{role} must use an ASCII Rust identifier"
            )));
        }
        Ok(identifier)
    }
}

/// Complete closed declaration model shared by syntax and compiler extraction.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum RustDeclaration {
    Constant(ConstantType),
    Enumeration(EnumType),
    ExternCrate(ExternCrateType),
    Function(FunctionType),
    ForeignModule(ForeignModuleType),
    Implementation(ImplementationType),
    Macro(MacroType),
    Module(ModuleDeclarationType),
    Static(StaticType),
    Structure(StructType),
    Trait(TraitType),
    TraitAlias(TraitAliasType),
    TypeAlias(TypeAliasType),
    Union(UnionType),
    Reexport(ReexportType),
}

impl RustDeclaration {
    pub(crate) fn implementation_owner_base(&self) -> Option<&BaseType> {
        match self {
            Self::Enumeration(value) => Some(value.base()),
            Self::Structure(value) => Some(value.base()),
            Self::TypeAlias(value) => Some(value.base()),
            Self::Union(value) => Some(value.base()),
            Self::Constant(_)
            | Self::ExternCrate(_)
            | Self::Function(_)
            | Self::ForeignModule(_)
            | Self::Implementation(_)
            | Self::Macro(_)
            | Self::Module(_)
            | Self::Static(_)
            | Self::Trait(_)
            | Self::TraitAlias(_)
            | Self::Reexport(_) => None,
        }
    }

    pub(crate) fn item_count(&self) -> usize {
        1 + match self {
            Self::Trait(value) => value.items().len(),
            Self::Implementation(value) => value.items().len(),
            Self::ForeignModule(value) => value.items().len(),
            Self::Constant(_)
            | Self::Enumeration(_)
            | Self::ExternCrate(_)
            | Self::Function(_)
            | Self::Macro(_)
            | Self::Module(_)
            | Self::Reexport(_)
            | Self::Static(_)
            | Self::Structure(_)
            | Self::TraitAlias(_)
            | Self::TypeAlias(_)
            | Self::Union(_) => 0,
        }
    }

    pub(crate) fn kind(&self) -> RustItemKind {
        match self {
            Self::Constant(_) => RustItemKind::Constant,
            Self::Enumeration(_) => RustItemKind::Enum,
            Self::ExternCrate(_) => RustItemKind::ExternCrate,
            Self::Function(_) => RustItemKind::Function,
            Self::ForeignModule(_) => RustItemKind::ForeignModule,
            Self::Implementation(_) => RustItemKind::Implementation,
            Self::Macro(_) => RustItemKind::Macro,
            Self::Module(_) => RustItemKind::Module,
            Self::Static(_) => RustItemKind::Static,
            Self::Structure(_) => RustItemKind::Struct,
            Self::Trait(_) => RustItemKind::Trait,
            Self::TraitAlias(_) => RustItemKind::TraitAlias,
            Self::TypeAlias(_) => RustItemKind::TypeAlias,
            Self::Union(_) => RustItemKind::Union,
            Self::Reexport(_) => RustItemKind::Reexport,
        }
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        match self {
            Self::Constant(value) => value.requires_capability_warning(),
            Self::Enumeration(value) => value.requires_capability_warning(),
            Self::ExternCrate(value) => value.requires_capability_warning(),
            Self::Function(value) => value.requires_capability_warning(),
            Self::ForeignModule(value) => value.requires_capability_warning(),
            Self::Implementation(value) => value.requires_capability_warning(),
            Self::Macro(value) => value.requires_capability_warning(),
            Self::Module(value) => value.requires_capability_warning(),
            Self::Static(value) => value.requires_capability_warning(),
            Self::Structure(value) => value.requires_capability_warning(),
            Self::Trait(value) => value.requires_capability_warning(),
            Self::TraitAlias(value) => value.requires_capability_warning(),
            Self::TypeAlias(value) => value.requires_capability_warning(),
            Self::Union(value) => value.requires_capability_warning(),
            Self::Reexport(_) => true,
        }
    }

    pub(crate) fn capability_reason(&self) -> Option<RustDeclarationCapability> {
        if !self.requires_capability_warning() {
            return None;
        }
        if let Some(attribute) = self.attributes().primary_capability() {
            return Some(RustDeclarationCapability::Attribute(attribute));
        }
        Some(match self {
            Self::Macro(_) => RustDeclarationCapability::MacroExpansion,
            Self::Reexport(_) => RustDeclarationCapability::ReexportResolution,
            Self::Constant(_)
            | Self::Enumeration(_)
            | Self::ExternCrate(_)
            | Self::Function(_)
            | Self::ForeignModule(_)
            | Self::Implementation(_)
            | Self::Module(_)
            | Self::Static(_)
            | Self::Structure(_)
            | Self::Trait(_)
            | Self::TraitAlias(_)
            | Self::TypeAlias(_)
            | Self::Union(_) => RustDeclarationCapability::NestedSemantics,
        })
    }

    fn attributes(&self) -> &RustAttributes {
        match self {
            Self::Constant(value) => value.attributes(),
            Self::Enumeration(value) => value.base().attributes(),
            Self::ExternCrate(value) => value.attributes(),
            Self::Function(value) => value.base().attributes(),
            Self::ForeignModule(value) => value.attributes(),
            Self::Implementation(value) => value.attributes(),
            Self::Macro(value) => value.base().attributes(),
            Self::Module(value) => value.attributes(),
            Self::Static(value) => value.base().attributes(),
            Self::Structure(value) => value.base().attributes(),
            Self::Trait(value) => value.base().attributes(),
            Self::TraitAlias(value) => value.attributes(),
            Self::TypeAlias(value) => value.base().attributes(),
            Self::Union(value) => value.base().attributes(),
            Self::Reexport(value) => value.base().attributes(),
        }
    }

    pub(crate) fn canonical_bytes(&self) -> Result<Vec<u8>, SignatureContractKitError> {
        serde_json::to_vec(self)
            .map_err(|error| SignatureContractKitError::conversion_failed(error.to_string()))
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum RustDeclarationCapability {
    Attribute(RustAttributeCapability),
    MacroExpansion,
    ReexportResolution,
    NestedSemantics,
}

impl fmt::Display for RustDeclarationCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Attribute(RustAttributeCapability::DeriveExpansion) => {
                formatter.write_str("derive macro expansion")
            }
            Self::Attribute(RustAttributeCapability::ConditionalCompilation) => {
                formatter.write_str("cfg/cfg_attr conditional compilation")
            }
            Self::Attribute(RustAttributeCapability::UnresolvedAttribute) => {
                formatter.write_str("an unresolved semantic attribute")
            }
            Self::MacroExpansion => formatter.write_str("macro expansion"),
            Self::ReexportResolution => formatter.write_str("re-export resolution"),
            Self::NestedSemantics => formatter.write_str("nested type or item semantics"),
        }
    }
}

/// Stable semantic declaration kind used by identity and rendering.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RustItemKind {
    Constant,
    Enum,
    ExternCrate,
    Function,
    ForeignModule,
    #[serde(rename = "impl")]
    Implementation,
    Macro,
    Module,
    Static,
    Struct,
    Trait,
    TraitAlias,
    TypeAlias,
    Union,
    Reexport,
}

impl RustItemKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Constant => "constant",
            Self::Enum => "enum",
            Self::ExternCrate => "extern_crate",
            Self::Function => "function",
            Self::ForeignModule => "foreign_module",
            Self::Implementation => "impl",
            Self::Macro => "macro",
            Self::Module => "module",
            Self::Static => "static",
            Self::Struct => "struct",
            Self::Trait => "trait",
            Self::TraitAlias => "trait_alias",
            Self::TypeAlias => "type_alias",
            Self::Union => "union",
            Self::Reexport => "reexport",
        }
    }

    pub(crate) fn is_structurally_repeatable(self) -> bool {
        matches!(
            self,
            Self::ForeignModule | Self::Implementation | Self::Macro
        )
    }
}

impl fmt::Display for RustItemKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A top-level constant declaration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ConstantType {
    base: BaseType,
    constant_type: RustType,
    value: RustSyntaxText,
}

impl ConstantType {
    pub(crate) fn new(
        base: BaseType,
        constant_type: RustType,
        value: RustSyntaxText,
    ) -> Result<Self, SignatureContractKitError> {
        RustIdentifier::new(base.name().to_owned(), "constant name")?;
        Ok(Self {
            base,
            constant_type,
            value,
        })
    }

    pub(crate) fn base(&self) -> &BaseType {
        &self.base
    }

    pub(crate) fn constant_type(&self) -> &RustType {
        &self.constant_type
    }

    pub(crate) fn value(&self) -> &str {
        self.value.as_str()
    }

    pub(crate) fn attributes(&self) -> &RustAttributes {
        self.base.attributes()
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.attributes().requires_capability_warning()
            || self.constant_type.requires_capability_warning()
            || self.value.contains_macro()
    }
}

/// An `extern crate` declaration and optional local alias.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ExternCrateType {
    base: BaseType,
    alias: Option<RustImportAlias>,
}

impl ExternCrateType {
    pub(crate) fn new(
        base: BaseType,
        alias: Option<String>,
    ) -> Result<Self, SignatureContractKitError> {
        if base.name() != "self" {
            RustIdentifier::new_ascii(base.name().to_owned(), "extern crate name")?;
        }
        if base.name() == "self" && alias.is_none() {
            return Err(SignatureContractKitError::conversion_failed(
                "extern crate self requires an alias",
            ));
        }
        Ok(Self {
            base,
            alias: alias
                .map(|value| RustImportAlias::new(value, "extern crate alias"))
                .transpose()?,
        })
    }

    pub(crate) fn base(&self) -> &BaseType {
        &self.base
    }

    pub(crate) fn alias(&self) -> Option<&str> {
        self.alias.as_ref().map(RustImportAlias::as_str)
    }

    pub(crate) fn attributes(&self) -> &RustAttributes {
        self.base.attributes()
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.attributes().requires_capability_warning()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
enum RustImportAlias {
    Named(RustIdentifier),
    Anonymous,
}

impl RustImportAlias {
    fn new(value: String, role: &'static str) -> Result<Self, SignatureContractKitError> {
        if value == "_" {
            return Ok(Self::Anonymous);
        }
        RustIdentifier::new(value, role).map(Self::Named)
    }

    fn as_str(&self) -> &str {
        match self {
            Self::Named(identifier) => identifier.as_str(),
            Self::Anonymous => "_",
        }
    }
}

/// One foreign module declaration. Its items preserve source order.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ForeignModuleType {
    #[serde(skip)]
    file_path: CatalogPath,
    module_id: RustModuleId,
    abi: RustFunctionAbi,
    is_unsafe: bool,
    attributes: RustAttributes,
    items: Vec<RustForeignItem>,
}

impl ForeignModuleType {
    pub(crate) fn new(
        file_path: CatalogPath,
        module_id: RustModuleId,
        abi: RustFunctionAbi,
        is_unsafe: bool,
        attributes: RustAttributes,
        items: Vec<RustForeignItem>,
    ) -> Result<Self, SignatureContractKitError> {
        Self::validate_abi(&abi)?;
        for item in &items {
            item.validate_context(&file_path, &module_id)?;
        }

        Ok(Self {
            file_path,
            module_id,
            abi,
            is_unsafe,
            attributes,
            items,
        })
    }

    fn validate_abi(abi: &RustFunctionAbi) -> Result<(), SignatureContractKitError> {
        let RustFunctionAbi::Extern { name } = abi else {
            return Err(SignatureContractKitError::conversion_failed(
                "foreign module ABI must be extern",
            ));
        };
        let Some(name) = name else {
            return Ok(());
        };
        if name.is_empty() || name.trim() != name || name.chars().any(char::is_control) {
            return Err(SignatureContractKitError::conversion_failed(
                "foreign module ABI name must be nonempty canonical text",
            ));
        }
        let canonical = name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
            && name
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_alphabetic())
            && name
                .chars()
                .next_back()
                .is_some_and(|character| character.is_ascii_alphanumeric());
        if !canonical {
            return Err(SignatureContractKitError::conversion_failed(format!(
                "invalid canonical foreign module ABI name {name:?}"
            )));
        }
        Ok(())
    }

    pub(crate) fn abi(&self) -> &RustFunctionAbi {
        &self.abi
    }

    pub(crate) fn is_unsafe(&self) -> bool {
        self.is_unsafe
    }

    pub(crate) fn attributes(&self) -> &RustAttributes {
        &self.attributes
    }

    pub(crate) fn items(&self) -> &[RustForeignItem] {
        &self.items
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.attributes.requires_capability_warning()
            || self
                .items
                .iter()
                .any(RustForeignItem::requires_capability_warning)
    }
}

/// Closed family of declarations inside a foreign module.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum RustForeignItem {
    Function(RustForeignFunction),
    Static(RustForeignStatic),
    Type(RustForeignType),
    Macro(RustForeignMacro),
}

impl RustForeignItem {
    pub(crate) fn requires_capability_warning(&self) -> bool {
        match self {
            Self::Function(value) => value.requires_capability_warning(),
            Self::Static(value) => value.requires_capability_warning(),
            Self::Type(value) => value.requires_capability_warning(),
            Self::Macro(value) => value.requires_capability_warning(),
        }
    }

    fn validate_context(
        &self,
        file_path: &CatalogPath,
        module_id: &RustModuleId,
    ) -> Result<(), SignatureContractKitError> {
        let Some((item_file, item_module)) = self.context() else {
            return Ok(());
        };
        if item_file != file_path || item_module != module_id {
            return Err(SignatureContractKitError::conversion_failed(format!(
                "foreign item context {item_file}::{item_module:?} does not match its module {file_path}::{module_id:?}"
            )));
        }
        Ok(())
    }

    fn context(&self) -> Option<(&CatalogPath, &RustModuleId)> {
        match self {
            Self::Function(value) => Some((
                value.function().base().file_path(),
                value.function().base().module_id(),
            )),
            Self::Static(value) => Some((
                value.value().base().file_path(),
                value.value().base().module_id(),
            )),
            Self::Type(value) => Some((value.base().file_path(), value.base().module_id())),
            Self::Macro(_) => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustForeignFunction {
    function: FunctionType,
}

impl RustForeignFunction {
    pub(crate) fn new(function: FunctionType) -> Result<Self, SignatureContractKitError> {
        RustIdentifier::new_ascii(function.base().name().to_owned(), "foreign function name")?;
        let signature = function.signature();
        if signature.is_const() || signature.is_async() {
            return Err(SignatureContractKitError::conversion_failed(
                "foreign functions cannot be const or async",
            ));
        }
        if !matches!(signature.abi(), RustFunctionAbi::Rust) {
            return Err(SignatureContractKitError::conversion_failed(
                "foreign function ABI must be inherited from its foreign module",
            ));
        }
        Ok(Self { function })
    }

    pub(crate) fn function(&self) -> &FunctionType {
        &self.function
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.function.requires_capability_warning()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustForeignStatic {
    value: StaticType,
}

impl RustForeignStatic {
    pub(crate) fn new(value: StaticType) -> Result<Self, SignatureContractKitError> {
        RustIdentifier::new_ascii(value.base().name().to_owned(), "foreign static name")?;
        Ok(Self { value })
    }

    pub(crate) fn value(&self) -> &StaticType {
        &self.value
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.value.requires_capability_warning()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustForeignType {
    base: BaseType,
}

impl RustForeignType {
    pub(crate) fn new(base: BaseType) -> Result<Self, SignatureContractKitError> {
        RustIdentifier::new_ascii(base.name().to_owned(), "foreign type name")?;
        Ok(Self { base })
    }

    pub(crate) fn base(&self) -> &BaseType {
        &self.base
    }

    pub(crate) fn attributes(&self) -> &RustAttributes {
        self.base.attributes()
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.attributes().requires_capability_warning()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustForeignMacro {
    tokens: RustMacroTokens,
    attributes: RustAttributes,
}

impl RustForeignMacro {
    pub(crate) fn new(
        tokens: String,
        attributes: RustAttributes,
    ) -> Result<Self, SignatureContractKitError> {
        Ok(Self {
            tokens: RustMacroTokens::new(tokens)?,
            attributes,
        })
    }

    pub(crate) fn tokens(&self) -> &str {
        self.tokens.as_str()
    }

    pub(crate) fn attributes(&self) -> &RustAttributes {
        &self.attributes
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        true
    }
}

/// One module declaration retained as a signature independently of graph traversal.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ModuleDeclarationType {
    base: BaseType,
    is_inline: bool,
    path_override: Option<String>,
}

impl ModuleDeclarationType {
    pub(crate) fn new(
        base: BaseType,
        is_inline: bool,
        path_override: Option<String>,
    ) -> Result<Self, SignatureContractKitError> {
        if let Some(path) = &path_override {
            Self::validate_path_override(path)?;
        }
        if is_inline || path_override.is_some() {
            RustIdentifier::new(base.name().to_owned(), "module name")?;
        } else {
            RustIdentifier::new_ascii(base.name().to_owned(), "module name")?;
        }
        Ok(Self {
            base,
            is_inline,
            path_override,
        })
    }

    pub(crate) fn from_syn(
        base: BaseType,
        item: &syn::ItemMod,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        if item.unsafety.is_some() {
            return Err(SignatureContractKitError::conversion_failed(
                "unsafe module declarations are unsupported",
            ));
        }

        let mut path_override = None;
        for attribute in item
            .attrs
            .iter()
            .filter(|attribute| attribute.path().is_ident("path"))
        {
            cancellation.checkpoint()?;
            if path_override.is_some() {
                return Err(SignatureContractKitError::conversion_failed(
                    "duplicate #[path] attributes on module declaration",
                ));
            }
            let syn::Meta::NameValue(name_value) = &attribute.meta else {
                return Err(SignatureContractKitError::conversion_failed(
                    "module #[path] must be a string name-value attribute",
                ));
            };
            let syn::Expr::Lit(expression) = &name_value.value else {
                return Err(SignatureContractKitError::conversion_failed(
                    "module #[path] must contain a string literal",
                ));
            };
            let syn::Lit::Str(path) = &expression.lit else {
                return Err(SignatureContractKitError::conversion_failed(
                    "module #[path] must contain a string literal",
                ));
            };
            path_override = Some(path.value());
        }

        Self::new(base, item.content.is_some(), path_override)
    }

    fn validate_path_override(path: &str) -> Result<(), SignatureContractKitError> {
        if path.is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "module path override cannot be empty",
            ));
        }
        if path.trim() != path {
            return Err(SignatureContractKitError::conversion_failed(
                "module path override cannot have surrounding whitespace",
            ));
        }
        if path.chars().any(char::is_control) {
            return Err(SignatureContractKitError::conversion_failed(
                "module path override cannot contain control characters",
            ));
        }
        Ok(())
    }

    pub(crate) fn base(&self) -> &BaseType {
        &self.base
    }

    pub(crate) fn is_inline(&self) -> bool {
        self.is_inline
    }

    pub(crate) fn path_override(&self) -> Option<&str> {
        self.path_override.as_deref()
    }

    pub(crate) fn attributes(&self) -> &RustAttributes {
        self.base.attributes()
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.attributes().requires_capability_warning()
    }
}

/// A trait alias declaration with ordered bounds.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct TraitAliasType {
    base: BaseType,
    generics: RustGenericMetadata,
    supertraits: Vec<RustSyntaxText>,
}

impl TraitAliasType {
    pub(crate) fn new(
        base: BaseType,
        generics: RustGenericMetadata,
        supertraits: Vec<RustSyntaxText>,
    ) -> Result<Self, SignatureContractKitError> {
        RustIdentifier::new(base.name().to_owned(), "trait alias name")?;
        Ok(Self {
            base,
            generics,
            supertraits,
        })
    }

    pub(crate) fn base(&self) -> &BaseType {
        &self.base
    }

    pub(crate) fn generics(&self) -> &RustGenericMetadata {
        &self.generics
    }

    pub(crate) fn supertraits(&self) -> &[RustSyntaxText] {
        &self.supertraits
    }

    pub(crate) fn attributes(&self) -> &RustAttributes {
        self.base.attributes()
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.attributes().requires_capability_warning()
            || self.generics.requires_capability_warning()
            || self.supertraits.iter().any(RustSyntaxText::contains_macro)
    }
}

/// A simple explicit re-export. Group and glob imports remain fail-closed at
/// the syntax converter until they receive a modeled representation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ReexportType {
    base: BaseType,
    path: String,
    alias: Option<RustImportAlias>,
}

impl ReexportType {
    pub(crate) fn new(
        base: BaseType,
        path: String,
        alias: Option<String>,
    ) -> Result<Self, SignatureContractKitError> {
        let path = Self::validate_path(path)?;
        let alias = alias
            .map(|value| RustImportAlias::new(value, "re-export alias"))
            .transpose()?;
        let expected_name = Self::visible_name(&path, alias.as_ref().map(RustImportAlias::as_str))?;
        if base.name() != expected_name {
            return Err(SignatureContractKitError::conversion_failed(format!(
                "re-export name {:?} does not match its path or alias {expected_name:?}",
                base.name()
            )));
        }
        if base.name() != "_" {
            RustIdentifier::new(base.name().to_owned(), "re-export name")?;
        }
        Ok(Self { base, path, alias })
    }

    pub(crate) fn visible_name<'a>(
        path: &'a str,
        alias: Option<&'a str>,
    ) -> Result<&'a str, SignatureContractKitError> {
        if let Some(alias) = alias {
            return Ok(alias);
        }

        let mut segments = path.rsplit("::");
        let final_segment = segments.next().filter(|segment| !segment.is_empty());
        let visible_name = match final_segment {
            Some("self") => segments.next().filter(|segment| !segment.is_empty()),
            Some("crate" | "super") | None => None,
            Some(segment) => Some(segment),
        };
        visible_name.ok_or_else(|| {
            SignatureContractKitError::conversion_failed(
                "re-export path requires an alias to define its visible name",
            )
        })
    }

    fn validate_path(path: String) -> Result<String, SignatureContractKitError> {
        if path.is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "re-export path cannot be empty",
            ));
        }
        if path.trim() != path {
            return Err(SignatureContractKitError::conversion_failed(
                "re-export path cannot have surrounding whitespace",
            ));
        }
        if path.chars().any(char::is_control) {
            return Err(SignatureContractKitError::conversion_failed(
                "re-export path cannot contain control characters",
            ));
        }
        let source_path = path
            .split("::")
            .map(RustModulePath::source_ident)
            .collect::<Vec<_>>()
            .join("::");
        let parsed = syn::parse_str::<syn::Path>(&source_path).map_err(|error| {
            SignatureContractKitError::conversion_failed(format!(
                "invalid re-export path {path:?}: {error}"
            ))
        })?;
        if let Some(segment) = parsed
            .segments
            .iter()
            .find(|segment| !matches!(&segment.arguments, syn::PathArguments::None))
        {
            return Err(SignatureContractKitError::conversion_failed(format!(
                "re-export path segment {} cannot have generic arguments",
                segment.ident
            )));
        }

        let mut canonical = String::new();
        if parsed.leading_colon.is_some() {
            canonical.push_str("::");
        }
        for (index, segment) in parsed.segments.iter().enumerate() {
            if index > 0 {
                canonical.push_str("::");
            }
            canonical.push_str(&RustModulePath::semantic_ident(&segment.ident));
        }
        if canonical != path {
            return Err(SignatureContractKitError::conversion_failed(format!(
                "re-export path {path:?} must use canonical form {canonical:?}"
            )));
        }
        Ok(path)
    }

    pub(crate) fn base(&self) -> &BaseType {
        &self.base
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) fn alias(&self) -> Option<&str> {
        self.alias.as_ref().map(RustImportAlias::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ConstantType, ExternCrateType, ForeignModuleType, ModuleDeclarationType, ReexportType,
        RustDeclaration, RustForeignFunction, RustForeignItem, RustForeignMacro, RustForeignStatic,
        RustForeignType, RustIdentifier, RustItemKind, TraitAliasType,
    };
    use crate::files::CatalogPath;
    use crate::languages::rust::parser::signature_id::RustItemId;
    use crate::languages::rust::parser::source_graph::{RustCrateId, RustModuleId, RustModulePath};
    use crate::languages::rust::types::attributes::{
        RustAttribute, RustAttributes, RustPath, RustRawAttribute, RustRawAttributeArguments,
    };
    use crate::languages::rust::types::base_type::BaseType;
    use crate::languages::rust::types::callable_type::{RustCallableSignature, RustFunctionAbi};
    use crate::languages::rust::types::enum_type::{EnumType, EnumVariant, EnumVariantField};
    use crate::languages::rust::types::function_type::FunctionType;
    use crate::languages::rust::types::impl_type::{ImplementationType, RustImplementationOwner};
    use crate::languages::rust::types::macro_type::MacroType;
    use crate::languages::rust::types::primitive_types::{
        RustFunctionParameter, RustGenericMetadata, RustType, UnsignedIntegerType, Visibility,
    };
    use crate::languages::rust::types::static_type::StaticType;
    use crate::languages::rust::types::struct_type::{StructField, StructType};
    use crate::languages::rust::types::syntax_text::RustSyntaxText;
    use crate::languages::rust::types::trait_type::TraitType;
    use crate::languages::rust::types::type_alias_type::TypeAliasType;
    use crate::languages::rust::types::union_type::UnionType;

    struct DeclarationFixture {
        file: CatalogPath,
        module: RustModuleId,
    }

    impl DeclarationFixture {
        fn new() -> Self {
            Self {
                file: CatalogPath::new("lib.rs").expect("fixture path"),
                module: RustModuleId::new(
                    RustCrateId::new("fixture", &crate::work::CancellationProbe::new())
                        .expect("fixture crate"),
                    RustModulePath::new(Vec::new()).expect("crate root module"),
                ),
            }
        }

        fn base(&self, name: &str, attributes: RustAttributes) -> BaseType {
            BaseType::new(
                name.to_owned(),
                Visibility::Public,
                self.file.clone(),
                self.module.clone(),
                attributes,
            )
        }

        fn implementation_owner(&self) -> RustImplementationOwner {
            RustImplementationOwner::new(
                RustItemId::new(self.module.clone(), RustItemKind::Struct, "Handler"),
                "Handler".to_owned(),
            )
            .expect("fixture implementation owner")
        }

        fn warning_attributes(&self) -> RustAttributes {
            RustAttributes::new(
                vec![RustAttribute::Unresolved(
                    RustRawAttribute::new(
                        RustPath::new("contract_semantic".to_owned()).expect("attribute path"),
                        RustRawAttributeArguments::Path,
                    )
                    .expect("raw attribute"),
                )],
                &crate::work::CancellationProbe::new(),
            )
            .expect("warning attributes")
        }

        fn expression(&self, value: &str) -> RustSyntaxText {
            RustSyntaxText::parse_expression(value).expect("expression fixture")
        }

        fn bound(&self, value: &str) -> RustSyntaxText {
            RustSyntaxText::parse_type_bound(value).expect("bound fixture")
        }

        fn predicate(&self, value: &str) -> RustSyntaxText {
            RustSyntaxText::parse_where_predicate(value).expect("where fixture")
        }

        fn pattern(&self, value: &str) -> RustSyntaxText {
            RustSyntaxText::parse_pattern(value).expect("pattern fixture")
        }

        fn declarations(&self) -> Vec<RustDeclaration> {
            vec![
                RustDeclaration::Constant(
                    ConstantType::new(
                        self.base("LIMIT", RustAttributes::default()),
                        RustType::UnsignedInteger(UnsignedIntegerType::Usize),
                        self.expression("4"),
                    )
                    .expect("constant"),
                ),
                RustDeclaration::Enumeration(EnumType::new(
                    self.base("Choice", RustAttributes::default()),
                )),
                RustDeclaration::ExternCrate(
                    ExternCrateType::new(
                        self.base("core", RustAttributes::default()),
                        Some("rust_core".to_owned()),
                    )
                    .expect("extern crate"),
                ),
                RustDeclaration::Function(FunctionType::new(
                    self.base("execute", RustAttributes::default()),
                )),
                RustDeclaration::ForeignModule(
                    ForeignModuleType::new(
                        self.file.clone(),
                        self.module.clone(),
                        RustFunctionAbi::Extern {
                            name: Some("C".to_owned()),
                        },
                        true,
                        RustAttributes::default(),
                        Vec::new(),
                    )
                    .expect("foreign module"),
                ),
                RustDeclaration::Implementation(ImplementationType::new(
                    self.implementation_owner(),
                )),
                RustDeclaration::Macro(
                    MacroType::new(
                        BaseType::new(
                            "contract_item".to_owned(),
                            Visibility::Private,
                            self.file.clone(),
                            self.module.clone(),
                            RustAttributes::default(),
                        ),
                        "contract_item!()".to_owned(),
                    )
                    .expect("macro declaration"),
                ),
                RustDeclaration::Module(
                    ModuleDeclarationType::new(
                        self.base("transport", RustAttributes::default()),
                        false,
                        Some("platform/transport.rs".to_owned()),
                    )
                    .expect("module"),
                ),
                RustDeclaration::Static(StaticType::new(
                    self.base("GLOBAL", RustAttributes::default()),
                    false,
                    RustType::UnsignedInteger(UnsignedIntegerType::Usize),
                )),
                RustDeclaration::Structure(StructType::new(
                    self.base("Handler", RustAttributes::default()),
                )),
                RustDeclaration::Trait(TraitType::new(
                    self.base("Service", RustAttributes::default()),
                )),
                RustDeclaration::TraitAlias(
                    TraitAliasType::new(
                        self.base("ServiceAlias", RustAttributes::default()),
                        RustGenericMetadata::default(),
                        vec![self.bound("Send"), self.bound("Sync")],
                    )
                    .expect("trait alias"),
                ),
                RustDeclaration::TypeAlias(TypeAliasType::new(
                    self.base("Value", RustAttributes::default()),
                    RustGenericMetadata::default(),
                    RustType::UnsignedInteger(UnsignedIntegerType::Usize),
                )),
                RustDeclaration::Union(UnionType::new(
                    self.base("Number", RustAttributes::default()),
                )),
                RustDeclaration::Reexport(
                    ReexportType::new(
                        self.base("PublicHandler", RustAttributes::default()),
                        "crate::internal::Handler".to_owned(),
                        Some("PublicHandler".to_owned()),
                    )
                    .expect("re-export"),
                ),
            ]
        }
    }

    #[test]
    fn declaration_identifiers_store_unraw_semantic_names_once() {
        assert_eq!(
            RustIdentifier::new("type".to_owned(), "fixture name")
                .expect("canonical keyword identifier")
                .as_str(),
            "type"
        );

        for invalid in ["r#type", "_", "crate", "self", "Self", "super"] {
            let error = RustIdentifier::new(invalid.to_owned(), "fixture name")
                .expect_err("raw prefixes and path-control words are not canonical names");
            assert!(
                error.to_string().contains("fixture name"),
                "{invalid}: {error}"
            );
        }
    }

    #[test]
    fn reexport_paths_accept_keyword_semantics_but_reject_raw_source_spelling() {
        let fixture = DeclarationFixture::new();
        let reexport = ReexportType::new(
            fixture.base("match", RustAttributes::default()),
            "crate::type::match".to_owned(),
            None,
        )
        .expect("canonical semantic re-export path");

        assert_eq!(reexport.path(), "crate::type::match");

        let error = ReexportType::new(
            fixture.base("match", RustAttributes::default()),
            "crate::r#type::r#match".to_owned(),
            None,
        )
        .expect_err("raw prefixes belong only to Rust source rendering");
        assert!(error.to_string().contains("canonical"), "{error}");
    }

    #[test]
    fn every_declaration_family_has_one_explicit_kind_and_canonical_variant() {
        let fixture = DeclarationFixture::new();
        let declarations = fixture.declarations();
        let expected = [
            (RustItemKind::Constant, "constant", false),
            (RustItemKind::Enum, "enum", false),
            (RustItemKind::ExternCrate, "extern_crate", false),
            (RustItemKind::Function, "function", false),
            (RustItemKind::ForeignModule, "foreign_module", true),
            (RustItemKind::Implementation, "impl", true),
            (RustItemKind::Macro, "macro", true),
            (RustItemKind::Module, "module", false),
            (RustItemKind::Static, "static", false),
            (RustItemKind::Struct, "struct", false),
            (RustItemKind::Trait, "trait", false),
            (RustItemKind::TraitAlias, "trait_alias", false),
            (RustItemKind::TypeAlias, "type_alias", false),
            (RustItemKind::Union, "union", false),
            (RustItemKind::Reexport, "reexport", false),
        ];

        assert_eq!(
            declarations
                .iter()
                .map(RustDeclaration::kind)
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|(kind, _, _)| *kind)
                .collect::<Vec<_>>()
        );
        for (kind, wire_name, repeatable) in expected {
            assert_eq!(kind.as_str(), wire_name);
            assert_eq!(kind.to_string(), wire_name);
            assert_eq!(
                serde_json::to_string(&kind).expect("kind serialization"),
                format!("\"{wire_name}\"")
            );
            assert_eq!(kind.is_structurally_repeatable(), repeatable);
        }
        for declaration in declarations {
            assert!(
                !declaration
                    .canonical_bytes()
                    .expect("canonical bytes")
                    .is_empty(),
                "{:?} must own a canonical representation",
                declaration.kind()
            );
        }
    }

    #[test]
    fn declaration_serialization_preserves_the_v2_canonical_bytes() {
        let fixture = DeclarationFixture::new();
        let declaration = RustDeclaration::Function(FunctionType::new(
            fixture.base("execute", RustAttributes::default()),
        ));

        assert_eq!(
            String::from_utf8(
                declaration
                    .canonical_bytes()
                    .expect("canonical declaration bytes"),
            )
            .expect("canonical JSON is UTF-8"),
            r#"{"Function":{"base":{"name":"execute","visibility":"Public","module_id":{"crate_id":"fixture","module_path":[]},"attributes":{"values":[]}},"signature":{"is_const":false,"is_async":false,"is_unsafe":false,"abi":"Rust","variadic":null,"generics":{"parameters":[],"where_predicates":[]},"parameters":[],"return_type":null}}}"#,
        );
    }

    #[test]
    fn physical_source_paths_do_not_change_canonical_declaration_bytes() {
        let fixture = DeclarationFixture::new();
        let first = RustDeclaration::Function(FunctionType::new(BaseType::new(
            "execute".to_owned(),
            Visibility::Public,
            CatalogPath::new("src/lib.rs").expect("first physical path"),
            fixture.module.clone(),
            RustAttributes::default(),
        )));
        let second = RustDeclaration::Function(FunctionType::new(BaseType::new(
            "execute".to_owned(),
            Visibility::Public,
            CatalogPath::new("generated/lib.rs").expect("second physical path"),
            fixture.module.clone(),
            RustAttributes::default(),
        )));

        assert_ne!(first, second, "physical locator evidence remains available");
        assert_eq!(
            first.canonical_bytes().expect("first canonical bytes"),
            second.canonical_bytes().expect("second canonical bytes"),
            "physical source paths are not semantic identity",
        );

        let first_foreign = RustDeclaration::ForeignModule(
            ForeignModuleType::new(
                CatalogPath::new("src/ffi.rs").expect("first foreign path"),
                fixture.module.clone(),
                RustFunctionAbi::Extern { name: None },
                false,
                RustAttributes::default(),
                Vec::new(),
            )
            .expect("first foreign module"),
        );
        let second_foreign = RustDeclaration::ForeignModule(
            ForeignModuleType::new(
                CatalogPath::new("generated/ffi.rs").expect("second foreign path"),
                fixture.module,
                RustFunctionAbi::Extern { name: None },
                false,
                RustAttributes::default(),
                Vec::new(),
            )
            .expect("second foreign module"),
        );

        assert_ne!(
            first_foreign, second_foreign,
            "foreign module locator evidence remains available",
        );
        assert_eq!(
            first_foreign
                .canonical_bytes()
                .expect("first foreign canonical bytes"),
            second_foreign
                .canonical_bytes()
                .expect("second foreign canonical bytes"),
            "foreign module physical paths are not semantic identity",
        );
    }

    #[test]
    fn implementation_owner_spelling_does_not_change_canonical_declaration_bytes() {
        let fixture = DeclarationFixture::new();
        let owner_id = RustItemId::new(fixture.module.clone(), RustItemKind::Struct, "Handler");
        let bare = RustDeclaration::Implementation(ImplementationType::new(
            RustImplementationOwner::new(owner_id.clone(), "Handler".to_owned())
                .expect("bare owner"),
        ));
        let qualified = RustDeclaration::Implementation(ImplementationType::new(
            RustImplementationOwner::new(owner_id, "crate::Handler".to_owned())
                .expect("qualified owner"),
        ));

        assert_ne!(bare, qualified, "rendering spelling remains available");
        assert_eq!(
            bare.canonical_bytes().expect("bare canonical bytes"),
            qualified
                .canonical_bytes()
                .expect("qualified canonical bytes"),
            "owner rendering spelling is not semantic identity",
        );
    }

    #[test]
    fn new_declaration_owners_retain_attributes_and_semantic_fields() {
        let fixture = DeclarationFixture::new();
        let attributes = fixture.warning_attributes();
        let constant = ConstantType::new(
            fixture.base("LIMIT", attributes.clone()),
            RustType::UnsignedInteger(UnsignedIntegerType::Usize),
            fixture.expression("4"),
        )
        .expect("constant");
        let external = ExternCrateType::new(
            fixture.base("core", attributes.clone()),
            Some("rust_core".to_owned()),
        )
        .expect("extern crate");
        let module = ModuleDeclarationType::new(
            fixture.base("transport", attributes.clone()),
            false,
            Some("platform/transport.rs".to_owned()),
        )
        .expect("module");
        let alias = TraitAliasType::new(
            fixture.base("ServiceAlias", attributes.clone()),
            RustGenericMetadata::default(),
            vec![fixture.bound("Send"), fixture.bound("Sync")],
        )
        .expect("trait alias");
        let reexport = ReexportType::new(
            fixture.base("PublicHandler", attributes),
            "crate::internal::Handler".to_owned(),
            Some("PublicHandler".to_owned()),
        )
        .expect("re-export");

        assert_eq!(constant.value(), "4");
        assert!(constant.requires_capability_warning());
        assert_eq!(external.alias(), Some("rust_core"));
        assert!(external.requires_capability_warning());
        let anonymous_external = ExternCrateType::new(
            fixture.base("link_only", RustAttributes::default()),
            Some("_".to_owned()),
        )
        .expect("extern crate supports an anonymous underscore import");
        assert_eq!(anonymous_external.alias(), Some("_"));
        let current_crate = ExternCrateType::new(
            fixture.base("self", RustAttributes::default()),
            Some("fixture_crate".to_owned()),
        )
        .expect("extern crate self supports a required local alias");
        assert_eq!(current_crate.alias(), Some("fixture_crate"));
        let unicode_alias = ExternCrateType::new(
            fixture.base("core", RustAttributes::default()),
            Some("東京".to_owned()),
        )
        .expect("extern crate aliases may use Unicode identifiers");
        assert_eq!(unicode_alias.alias(), Some("東京"));
        assert!(!module.is_inline());
        assert_eq!(module.path_override(), Some("platform/transport.rs"));
        assert!(module.requires_capability_warning());
        assert_eq!(
            alias
                .supertraits()
                .iter()
                .map(RustSyntaxText::as_str)
                .collect::<Vec<_>>(),
            vec!["Send", "Sync"]
        );
        assert!(alias.requires_capability_warning());
        let empty_alias = TraitAliasType::new(
            fixture.base("EmptyAlias", RustAttributes::default()),
            RustGenericMetadata::default(),
            Vec::new(),
        )
        .expect("trait alias bounds may be empty");
        assert!(empty_alias.supertraits().is_empty());
        assert!(!empty_alias.requires_capability_warning());
        assert_eq!(reexport.path(), "crate::internal::Handler");
        assert_eq!(reexport.alias(), Some("PublicHandler"));
        assert!(RustDeclaration::Reexport(reexport).requires_capability_warning());
        let anonymous_reexport = ReexportType::new(
            fixture.base("_", RustAttributes::default()),
            "crate::internal::Handler".to_owned(),
            Some("_".to_owned()),
        )
        .expect("re-export supports an anonymous underscore import");
        assert_eq!(anonymous_reexport.alias(), Some("_"));
        let self_reexport = ReexportType::new(
            fixture.base("internal", RustAttributes::default()),
            "crate::internal::self".to_owned(),
            None,
        )
        .expect("trailing self binds the preceding path segment");
        assert_eq!(self_reexport.path(), "crate::internal::self");
    }

    #[test]
    fn module_shape_path_visibility_and_attributes_are_canonical_semantics() {
        let fixture = DeclarationFixture::new();
        let plain = RustDeclaration::Module(
            ModuleDeclarationType::new(
                fixture.base("transport", RustAttributes::default()),
                false,
                None,
            )
            .expect("plain module"),
        );
        let inline = RustDeclaration::Module(
            ModuleDeclarationType::new(
                fixture.base("transport", RustAttributes::default()),
                true,
                None,
            )
            .expect("inline module"),
        );
        let path_directed = RustDeclaration::Module(
            ModuleDeclarationType::new(
                fixture.base("transport", RustAttributes::default()),
                false,
                Some("platform/transport.rs".to_owned()),
            )
            .expect("path-directed module"),
        );
        let attributed = RustDeclaration::Module(
            ModuleDeclarationType::new(
                fixture.base("transport", fixture.warning_attributes()),
                false,
                None,
            )
            .expect("attributed module"),
        );
        let private = RustDeclaration::Module(
            ModuleDeclarationType::new(
                BaseType::new(
                    "transport".to_owned(),
                    Visibility::Private,
                    fixture.file.clone(),
                    fixture.module.clone(),
                    RustAttributes::default(),
                ),
                false,
                None,
            )
            .expect("private module"),
        );
        let plain = plain.canonical_bytes().expect("plain canonical bytes");

        for changed in [inline, path_directed, attributed, private] {
            assert_ne!(
                plain,
                changed.canonical_bytes().expect("changed canonical bytes"),
                "every modeled module semantic must affect canonical bytes"
            );
        }
    }

    #[test]
    fn foreign_items_retain_order_attributes_and_capability_facts() {
        let fixture = DeclarationFixture::new();
        let warning = fixture.warning_attributes();
        let function = RustForeignFunction::new(
            FunctionType::new(fixture.base("run", warning.clone()))
                .with_callable_signature(RustCallableSignature::empty()),
        )
        .expect("foreign function");
        let static_item = RustForeignStatic::new(StaticType::new(
            fixture.base("FLAG", warning.clone()),
            false,
            RustType::UnsignedInteger(UnsignedIntegerType::U8),
        ))
        .expect("foreign static");
        let foreign_type =
            RustForeignType::new(fixture.base("Opaque", warning.clone())).expect("foreign type");
        let macro_item = RustForeignMacro::new("contract_foreign!()".to_owned(), warning)
            .expect("foreign macro");
        let module = ForeignModuleType::new(
            fixture.file.clone(),
            fixture.module.clone(),
            RustFunctionAbi::Extern {
                name: Some("C".to_owned()),
            },
            true,
            RustAttributes::default(),
            vec![
                RustForeignItem::Function(function),
                RustForeignItem::Static(static_item),
                RustForeignItem::Type(foreign_type),
                RustForeignItem::Macro(macro_item),
            ],
        )
        .expect("foreign module");

        assert!(module.is_unsafe());
        assert!(matches!(module.items()[0], RustForeignItem::Function(_)));
        assert!(matches!(module.items()[1], RustForeignItem::Static(_)));
        assert!(matches!(module.items()[2], RustForeignItem::Type(_)));
        assert!(matches!(module.items()[3], RustForeignItem::Macro(_)));
        let RustForeignItem::Macro(macro_item) = &module.items()[3] else {
            panic!("expected foreign macro");
        };
        assert_eq!(macro_item.tokens(), "contract_foreign ! ()");
        assert!(
            module
                .items()
                .iter()
                .all(RustForeignItem::requires_capability_warning)
        );
        assert!(module.requires_capability_warning());
    }

    #[test]
    fn declaration_capability_warnings_recurse_through_types_and_generics() {
        let fixture = DeclarationFixture::new();
        let macro_type = RustType::MacroInvocation("contract_type!()".to_owned());
        let macro_generic = RustGenericMetadata::default()
            .with_where_predicates(vec![fixture.predicate("T: Trait<Item = contract_type!()>")]);
        let field = StructField::new(
            Some("value".to_owned()),
            Visibility::Public,
            macro_type.clone(),
            RustAttributes::default(),
        )
        .expect("field with retained macro type");
        let variant_field = EnumVariantField::new(
            Some("value".to_owned()),
            macro_type.clone(),
            RustAttributes::default(),
        )
        .expect("variant field with retained macro type");
        let variant = EnumVariant::new(
            "Value".to_owned(),
            vec![variant_field],
            None,
            RustAttributes::default(),
        )
        .expect("variant with retained macro type");
        let function = FunctionType::new(fixture.base("execute", RustAttributes::default()))
            .with_callable_signature(
                RustCallableSignature::builder()
                    .with_parameters(vec![RustFunctionParameter::new(
                        Some(fixture.pattern("value")),
                        macro_type.clone(),
                    )])
                    .with_return_type(Some(macro_type.clone()))
                    .build(),
            );
        let foreign_function = RustForeignFunction::new(
            FunctionType::new(fixture.base("foreign", RustAttributes::default()))
                .with_callable_signature(
                    RustCallableSignature::builder()
                        .with_return_type(Some(macro_type.clone()))
                        .build(),
                ),
        )
        .expect("foreign function with retained macro return type");
        let foreign_static = RustForeignStatic::new(StaticType::new(
            fixture.base("FOREIGN", RustAttributes::default()),
            false,
            macro_type.clone(),
        ))
        .expect("foreign static with retained macro type");

        let declarations = [
            RustDeclaration::Constant(
                ConstantType::new(
                    fixture.base("LIMIT", RustAttributes::default()),
                    macro_type.clone(),
                    fixture.expression("4"),
                )
                .expect("constant with retained macro type"),
            ),
            RustDeclaration::Enumeration(
                EnumType::new(fixture.base("Choice", RustAttributes::default()))
                    .with_variants(vec![variant]),
            ),
            RustDeclaration::Function(function),
            RustDeclaration::ForeignModule(
                ForeignModuleType::new(
                    fixture.file.clone(),
                    fixture.module.clone(),
                    RustFunctionAbi::Extern { name: None },
                    false,
                    RustAttributes::default(),
                    vec![
                        RustForeignItem::Function(foreign_function),
                        RustForeignItem::Static(foreign_static),
                    ],
                )
                .expect("foreign module with retained nested macro types"),
            ),
            RustDeclaration::Implementation(
                ImplementationType::new(fixture.implementation_owner())
                    .with_generic_metadata(macro_generic.clone()),
            ),
            RustDeclaration::Static(StaticType::new(
                fixture.base("GLOBAL", RustAttributes::default()),
                false,
                macro_type.clone(),
            )),
            RustDeclaration::Structure(
                StructType::new(fixture.base("Handler", RustAttributes::default()))
                    .with_fields(vec![field.clone()]),
            ),
            RustDeclaration::Trait(
                TraitType::new(fixture.base("Service", RustAttributes::default()))
                    .with_generic_metadata(macro_generic.clone()),
            ),
            RustDeclaration::TraitAlias(
                TraitAliasType::new(
                    fixture.base("ServiceAlias", RustAttributes::default()),
                    macro_generic.clone(),
                    vec![fixture.bound("Send")],
                )
                .expect("trait alias with retained macro generic"),
            ),
            RustDeclaration::TypeAlias(TypeAliasType::new(
                fixture.base("Value", RustAttributes::default()),
                RustGenericMetadata::default(),
                macro_type.clone(),
            )),
            RustDeclaration::Union(
                UnionType::new(fixture.base("Number", RustAttributes::default()))
                    .with_fields(vec![field]),
            ),
        ];

        for declaration in declarations {
            assert!(
                declaration.requires_capability_warning(),
                "every retained nested macro fact must produce a syntax-mode capability warning for {:?}",
                declaration.kind()
            );
        }
    }

    #[test]
    fn retained_constant_values_and_trait_alias_bounds_require_warnings() {
        let fixture = DeclarationFixture::new();
        let constant = RustDeclaration::Constant(
            ConstantType::new(
                fixture.base("LIMIT", RustAttributes::default()),
                RustType::UnsignedInteger(UnsignedIntegerType::Usize),
                fixture.expression("contract_value!()"),
            )
            .expect("macro-valued constant"),
        );
        let alias = RustDeclaration::TraitAlias(
            TraitAliasType::new(
                fixture.base("ServiceAlias", RustAttributes::default()),
                RustGenericMetadata::default(),
                vec![fixture.bound("Service<Item = contract_type!()>")],
            )
            .expect("macro-bearing trait alias bound"),
        );

        assert!(constant.requires_capability_warning());
        assert!(alias.requires_capability_warning());
    }

    #[test]
    fn constructors_reject_invalid_semantic_text_and_context() {
        let fixture = DeclarationFixture::new();
        let invalid_constant = RustSyntaxText::parse_expression("let value")
            .expect_err("constant value must be an expression");
        let invalid_module = ModuleDeclarationType::new(
            fixture.base("transport", RustAttributes::default()),
            false,
            Some(" transport.rs".to_owned()),
        )
        .expect_err("module path whitespace must fail");
        let invalid_alias = RustSyntaxText::parse_type_bound("Send +")
            .expect_err("malformed trait alias bound must fail");
        let missing_self_alias =
            ExternCrateType::new(fixture.base("self", RustAttributes::default()), None)
                .expect_err("extern crate self must be renamed");
        let unicode_foreign_name =
            RustForeignType::new(fixture.base("東京", RustAttributes::default()))
                .expect_err("external block item names are ASCII-only");
        let duplicate_foreign_abi = RustForeignFunction::new(
            FunctionType::new(fixture.base("run", RustAttributes::default()))
                .with_callable_signature(
                    RustCallableSignature::builder()
                        .with_abi(RustFunctionAbi::Extern {
                            name: Some("C".to_owned()),
                        })
                        .build(),
                ),
        )
        .expect_err("foreign function ABI belongs to the foreign module");
        let async_foreign_function = RustForeignFunction::new(
            FunctionType::new(fixture.base("poll", RustAttributes::default()))
                .with_callable_signature(RustCallableSignature::builder().with_async(true).build()),
        )
        .expect_err("foreign functions cannot be async");
        let unicode_file_module = ModuleDeclarationType::new(
            fixture.base("東京", RustAttributes::default()),
            false,
            None,
        )
        .expect_err("filesystem-loaded module names are ASCII-only");
        ModuleDeclarationType::new(
            fixture.base("東京", RustAttributes::default()),
            false,
            Some("transport.rs".to_owned()),
        )
        .expect("path-overridden modules may use Unicode identifiers");
        let invalid_reexport = ReexportType::new(
            fixture.base("PublicHandler", RustAttributes::default()),
            "crate :: internal :: Handler".to_owned(),
            None,
        )
        .expect_err("noncanonical re-export path must fail");
        let mismatched_reexport = ReexportType::new(
            fixture.base("PublicHandler", RustAttributes::default()),
            "crate::internal::Handler".to_owned(),
            None,
        )
        .expect_err("re-export name must reflect its path or alias");
        let other_module = RustModuleId::new(
            RustCrateId::new("other", &crate::work::CancellationProbe::new()).expect("other crate"),
            RustModulePath::new(Vec::new()).expect("other root module"),
        );
        let foreign_item = RustForeignItem::Type(
            RustForeignType::new(fixture.base("Opaque", RustAttributes::default()))
                .expect("foreign type"),
        );
        let invalid_context = ForeignModuleType::new(
            fixture.file,
            other_module,
            RustFunctionAbi::Extern { name: None },
            false,
            RustAttributes::default(),
            vec![foreign_item],
        )
        .expect_err("foreign item context mismatch must fail");

        assert!(invalid_constant.to_string().contains("expression"));
        assert!(invalid_module.to_string().contains("path override"));
        assert!(invalid_alias.to_string().contains("type parameter bound"));
        assert!(missing_self_alias.to_string().contains("requires an alias"));
        assert!(unicode_foreign_name.to_string().contains("ASCII"));
        assert!(duplicate_foreign_abi.to_string().contains("inherited"));
        assert!(
            async_foreign_function
                .to_string()
                .contains("const or async")
        );
        assert!(unicode_file_module.to_string().contains("ASCII"));
        assert!(invalid_reexport.to_string().contains("canonical form"));
        assert!(mismatched_reexport.to_string().contains("does not match"));
        assert!(invalid_context.to_string().contains("context"));
    }

    #[test]
    fn macros_and_reexports_report_syntax_mode_capability_limits() {
        let fixture = DeclarationFixture::new();
        let macro_declaration = RustDeclaration::Macro(
            MacroType::new(
                BaseType::new(
                    "contract_item".to_owned(),
                    Visibility::Private,
                    fixture.file.clone(),
                    fixture.module.clone(),
                    RustAttributes::default(),
                ),
                "contract_item!()".to_owned(),
            )
            .expect("macro declaration"),
        );
        let reexport = RustDeclaration::Reexport(
            ReexportType::new(
                fixture.base("GeneratedApi", RustAttributes::default()),
                "external::GeneratedApi".to_owned(),
                None,
            )
            .expect("unresolved re-export"),
        );

        assert!(macro_declaration.requires_capability_warning());
        assert!(reexport.requires_capability_warning());
    }
}
