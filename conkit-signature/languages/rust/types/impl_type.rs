use crate::error::SignatureContractKitError;
use crate::languages::rust::parser::signature_id::RustItemId;
use crate::languages::rust::types::associated_item::RustAssociatedItem;
use crate::languages::rust::types::attributes::RustAttributes;
use crate::languages::rust::types::base_type::RustImplementationContext;
use crate::languages::rust::types::declaration::RustItemKind;
use crate::languages::rust::types::primitive_types::{
    RustGenericMetadata, RustSyntaxCapabilityProbe,
};
use crate::work::CancellationProbe;
use quote::ToTokens as _;
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RustImplementationOwner {
    id: RustItemId,
    spelling: String,
}

impl RustImplementationOwner {
    pub(crate) fn new(id: RustItemId, spelling: String) -> Result<Self, SignatureContractKitError> {
        if !Self::supports(&id) {
            return Err(SignatureContractKitError::conversion_failed(format!(
                "Rust {} declaration {} cannot own an implementation",
                id.kind(),
                id.render()
            )));
        }
        if spelling.is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "implementation owner spelling cannot be empty",
            ));
        }
        if spelling.trim() != spelling {
            return Err(SignatureContractKitError::conversion_failed(
                "implementation owner spelling cannot have surrounding whitespace",
            ));
        }
        if spelling.chars().any(char::is_control) {
            return Err(SignatureContractKitError::conversion_failed(
                "implementation owner spelling cannot contain control characters",
            ));
        }
        let owner = syn::parse_str::<syn::Type>(&spelling).map_err(|source| {
            SignatureContractKitError::conversion_failed(format!(
                "invalid implementation owner spelling {spelling:?}: {source}"
            ))
        })?;
        if !matches!(owner, syn::Type::Path(value) if value.qself.is_none()) {
            return Err(SignatureContractKitError::conversion_failed(format!(
                "implementation owner spelling {spelling:?} must be an ordinary type path without qualified-self syntax"
            )));
        }

        Ok(Self { id, spelling })
    }

    pub(crate) fn supports(id: &RustItemId) -> bool {
        matches!(
            id.kind(),
            RustItemKind::Enum
                | RustItemKind::Struct
                | RustItemKind::TypeAlias
                | RustItemKind::Union
        )
    }

    pub(crate) fn id(&self) -> &RustItemId {
        &self.id
    }
}

impl Serialize for RustImplementationOwner {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.id.render())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ImplementationType {
    #[serde(rename = "owner_id")]
    owner: RustImplementationOwner,
    implemented_trait: RustImplementedTrait,
    is_default: bool,
    is_unsafe: bool,
    generics: RustGenericMetadata,
    attributes: RustAttributes,
    items: Vec<RustAssociatedItem>,
}

impl ImplementationType {
    pub(crate) fn new(owner: RustImplementationOwner) -> Self {
        Self {
            owner,
            implemented_trait: RustImplementedTrait::Inherent,
            is_default: false,
            is_unsafe: false,
            generics: RustGenericMetadata::default(),
            attributes: RustAttributes::default(),
            items: Vec::new(),
        }
    }

    pub(crate) fn with_implemented_trait(
        mut self,
        implemented_trait: RustImplementedTrait,
    ) -> Self {
        self.implemented_trait = implemented_trait;
        self
    }

    pub(crate) fn with_qualifiers(mut self, is_default: bool, is_unsafe: bool) -> Self {
        self.is_default = is_default;
        self.is_unsafe = is_unsafe;
        self
    }

    pub(crate) fn with_generic_metadata(mut self, generics: RustGenericMetadata) -> Self {
        self.generics = generics;
        self
    }

    pub(crate) fn with_attributes(mut self, attributes: RustAttributes) -> Self {
        self.attributes = attributes;
        self
    }

    pub(crate) fn with_items(mut self, items: Vec<RustAssociatedItem>) -> Self {
        self.items = items;
        self
    }

    pub(crate) fn sort_associated_items(
        &mut self,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        cancellation.checkpoint()?;
        let mut keys = Vec::with_capacity(self.items.len());
        for item in &self.items {
            cancellation.checkpoint()?;
            keys.push(item.canonical_bytes()?);
        }
        cancellation.checkpoint()?;
        let items = std::mem::take(&mut self.items);
        let mut keyed = keys.into_iter().zip(items).collect::<Vec<_>>();
        keyed.sort_by(|left, right| left.0.cmp(&right.0));
        cancellation.checkpoint()?;
        let mut sorted = Vec::with_capacity(keyed.len());
        for (_, item) in keyed {
            cancellation.checkpoint()?;
            sorted.push(item);
        }
        cancellation.checkpoint()?;
        self.items = sorted;
        Ok(())
    }

    pub(crate) fn normalize_for_owner(
        &mut self,
        context: &RustImplementationContext,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        let trait_owned = matches!(self.implemented_trait, RustImplementedTrait::Trait { .. });
        for item in &mut self.items {
            cancellation.checkpoint()?;
            item.normalize_implementation_member(context, trait_owned);
        }
        Ok(())
    }

    pub(crate) fn append_same_descriptor(
        &mut self,
        other: Self,
    ) -> Result<(), SignatureContractKitError> {
        let current_descriptor = self.descriptor_bytes().map_err(|error| {
            SignatureContractKitError::conversion_failed(format!(
                "failed to encode existing Rust implementation descriptor: {error}"
            ))
        })?;
        let incoming_descriptor = other.descriptor_bytes().map_err(|error| {
            SignatureContractKitError::conversion_failed(format!(
                "failed to encode incoming Rust implementation descriptor: {error}"
            ))
        })?;
        if current_descriptor != incoming_descriptor {
            return Err(SignatureContractKitError::conversion_failed(
                "cannot merge Rust implementation blocks with different descriptors",
            ));
        }

        self.items.extend(other.items);
        Ok(())
    }

    pub(crate) fn owner(&self) -> &RustImplementationOwner {
        &self.owner
    }

    pub(crate) fn implemented_trait(&self) -> &RustImplementedTrait {
        &self.implemented_trait
    }

    pub(crate) fn is_default(&self) -> bool {
        self.is_default
    }

    pub(crate) fn is_unsafe(&self) -> bool {
        self.is_unsafe
    }

    pub(crate) fn generics(&self) -> &RustGenericMetadata {
        &self.generics
    }

    pub(crate) fn attributes(&self) -> &RustAttributes {
        &self.attributes
    }

    pub(crate) fn items(&self) -> &[RustAssociatedItem] {
        &self.items
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.implemented_trait.requires_capability_warning()
            || self.generics.requires_capability_warning()
            || self.attributes.requires_capability_warning()
            || self
                .items
                .iter()
                .any(RustAssociatedItem::requires_capability_warning)
    }

    pub(crate) fn descriptor_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(&(
            &self.owner,
            &self.implemented_trait,
            self.is_default,
            self.is_unsafe,
            &self.generics,
            &self.attributes,
        ))
    }
}

/// Canonical syntax-mode path naming a trait implemented by a Rust `impl`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct RustTraitPath(String);

impl RustTraitPath {
    pub(crate) fn new(value: String) -> Result<Self, SignatureContractKitError> {
        if value.is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "implementation trait path cannot be empty",
            ));
        }
        if value.trim() != value {
            return Err(SignatureContractKitError::conversion_failed(
                "implementation trait path cannot have surrounding whitespace",
            ));
        }
        if value.chars().any(char::is_control) {
            return Err(SignatureContractKitError::conversion_failed(
                "implementation trait path cannot contain control characters",
            ));
        }

        let parsed = syn::parse_str::<syn::Path>(&value).map_err(|error| {
            SignatureContractKitError::conversion_failed(format!(
                "invalid implementation trait path {value:?}: {error}"
            ))
        })?;
        let canonical = parsed.to_token_stream().to_string();
        if canonical.is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "implementation trait path cannot be empty",
            ));
        }

        Ok(Self(canonical))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    fn requires_capability_warning(&self) -> bool {
        RustSyntaxCapabilityProbe::path_contains_macro(&self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum RustImplementedTrait {
    Inherent,
    Trait {
        path: RustTraitPath,
        polarity: RustImplPolarity,
    },
}

impl RustImplementedTrait {
    pub(crate) fn for_trait(
        path: String,
        polarity: RustImplPolarity,
    ) -> Result<Self, SignatureContractKitError> {
        Ok(Self::Trait {
            path: RustTraitPath::new(path)?,
            polarity,
        })
    }

    fn requires_capability_warning(&self) -> bool {
        match self {
            Self::Inherent => false,
            Self::Trait { path, .. } => path.requires_capability_warning(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub(crate) enum RustImplPolarity {
    Positive,
    Negative,
}

#[cfg(test)]
mod tests {
    use super::{
        ImplementationType, RustImplPolarity, RustImplementationOwner, RustImplementedTrait,
        RustTraitPath,
    };
    use crate::languages::rust::parser::signature_id::RustItemId;
    use crate::languages::rust::parser::source_graph::{RustCrateId, RustModuleId, RustModulePath};
    use crate::languages::rust::types::associated_item::{
        RustAssociatedConstant, RustAssociatedItem, RustAssociatedType,
    };
    use crate::languages::rust::types::attributes::RustAttributes;
    use crate::languages::rust::types::base_type::{BaseType, RustImplementationContext};
    use crate::languages::rust::types::declaration::RustItemKind;
    use crate::languages::rust::types::primitive_types::{
        RustGenericMetadata, RustType, Visibility,
    };
    use crate::languages::rust::types::syntax_text::RustSyntaxText;

    struct ImplementationFixture {
        owner_id: RustItemId,
    }

    impl ImplementationFixture {
        fn new() -> Self {
            let module_id = RustModuleId::new(
                RustCrateId::new("sample", &crate::work::CancellationProbe::new())
                    .expect("fixture crate id"),
                RustModulePath::new(vec!["model".to_owned()]).expect("fixture module path"),
            );
            Self {
                owner_id: RustItemId::new(module_id, RustItemKind::Struct, "Widget"),
            }
        }

        fn owner(&self, spelling: &str) -> RustImplementationOwner {
            RustImplementationOwner::new(self.owner_id.clone(), spelling.to_owned())
                .expect("valid fixture owner")
        }

        fn implementation(&self, spelling: &str) -> ImplementationType {
            ImplementationType::new(self.owner(spelling))
        }

        fn constant(name: &str) -> RustAssociatedItem {
            RustAssociatedItem::Constant(
                RustAssociatedConstant::new(
                    name.to_owned(),
                    Visibility::Private,
                    RustType::Bool,
                    Some(RustSyntaxText::parse_expression("true").expect("constant value")),
                    false,
                    RustAttributes::default(),
                )
                .expect("valid fixture associated constant"),
            )
        }

        fn associated_type(name: &str, visibility: Visibility) -> RustAssociatedItem {
            RustAssociatedItem::Type(
                RustAssociatedType::new(
                    name.to_owned(),
                    visibility,
                    RustGenericMetadata::default(),
                    Vec::new(),
                    Some(RustType::Bool),
                    false,
                    RustAttributes::default(),
                )
                .expect("valid fixture associated type"),
            )
        }

        fn context(&self, visibility: Visibility) -> RustImplementationContext {
            RustImplementationContext::new(
                &self.owner_id,
                &BaseType::new(
                    "Widget".to_owned(),
                    visibility,
                    crate::files::CatalogPath::new("models.rs").expect("owner file"),
                    self.owner_id.module_id().clone(),
                    RustAttributes::default(),
                ),
            )
            .expect("valid owner context")
        }
    }

    #[test]
    fn trait_paths_reject_empty_surrounding_whitespace_and_malformed_syntax() {
        for value in [
            "",
            " crate::Service",
            "crate::Service ",
            "crate::",
            "Send + Sync",
            "!Send",
            "crate::Service\n",
            "crate::Ser\u{7}vice",
        ] {
            let error = RustTraitPath::new(value.to_owned())
                .expect_err("invalid implementation trait paths must fail");
            assert!(
                error.to_string().contains("implementation trait path"),
                "unexpected error for {value:?}: {error}"
            );
        }
    }

    #[test]
    fn trait_paths_canonicalize_qualified_and_generic_syntax() {
        let compact = RustTraitPath::new(
            "::transport::Service<Request, Response = Result<u8, Error>>".to_owned(),
        )
        .expect("qualified generic trait path");
        let spaced = RustTraitPath::new(
            ":: transport :: Service < Request , Response = Result < u8 , Error > >".to_owned(),
        )
        .expect("equivalent spaced trait path");

        assert_eq!(compact, spaced);
        assert_eq!(
            compact.as_str(),
            ":: transport :: Service < Request , Response = Result < u8 , Error > >"
        );
    }

    #[test]
    fn implemented_traits_preserve_negative_polarity_and_canonical_bytes() {
        let fixture = ImplementationFixture::new();
        let positive = RustImplementedTrait::for_trait(
            "crate::Service<T>".to_owned(),
            RustImplPolarity::Positive,
        )
        .expect("positive trait implementation");
        let negative = RustImplementedTrait::for_trait(
            "crate :: Service < T >".to_owned(),
            RustImplPolarity::Negative,
        )
        .expect("negative trait implementation");

        let RustImplementedTrait::Trait { path, polarity } = &negative else {
            panic!("negative trait implementation must remain a trait implementation");
        };
        assert_eq!(path.as_str(), "crate :: Service < T >");
        assert_eq!(*polarity, RustImplPolarity::Negative);

        let positive_bytes = fixture
            .implementation("Widget")
            .with_implemented_trait(positive)
            .descriptor_bytes()
            .expect("positive descriptor");
        let negative_bytes = fixture
            .implementation("Widget")
            .with_implemented_trait(negative)
            .descriptor_bytes()
            .expect("negative descriptor");
        assert_ne!(positive_bytes, negative_bytes);

        let equivalent_negative_bytes = fixture
            .implementation("Widget")
            .with_implemented_trait(
                RustImplementedTrait::for_trait(
                    "crate::Service<T>".to_owned(),
                    RustImplPolarity::Negative,
                )
                .expect("equivalent negative trait implementation"),
            )
            .descriptor_bytes()
            .expect("equivalent negative descriptor");
        assert_eq!(negative_bytes, equivalent_negative_bytes);
    }

    #[test]
    fn owner_spelling_is_rendering_metadata_not_descriptor_semantics() {
        let fixture = ImplementationFixture::new();
        let bare = fixture
            .implementation("Widget")
            .descriptor_bytes()
            .expect("bare descriptor");
        let qualified = fixture
            .implementation("crate::model::Widget")
            .descriptor_bytes()
            .expect("qualified descriptor");

        assert_eq!(bare, qualified);
        assert_eq!(fixture.owner("Widget").id(), &fixture.owner_id);
        assert_eq!(
            fixture.owner("crate::model::Widget").spelling,
            "crate::model::Widget"
        );
    }

    #[test]
    fn implementation_serialization_preserves_the_v2_descriptor_shape() {
        let fixture = ImplementationFixture::new();

        assert_eq!(
            serde_json::to_string(&fixture.implementation("Widget"))
                .expect("canonical implementation"),
            r#"{"owner_id":"rust:v2:crate:6:sample:modules:1:5:model:kind:6:struct:name:6:Widget:occurrence:1:1","implemented_trait":"Inherent","is_default":false,"is_unsafe":false,"generics":{"parameters":[],"where_predicates":[]},"attributes":{"values":[]},"items":[]}"#,
        );
    }

    #[test]
    fn distinct_canonical_owner_ids_change_descriptor_semantics() {
        let fixture = ImplementationFixture::new();
        let other_owner = RustImplementationOwner::new(
            RustItemId::new(
                RustModuleId::new(
                    RustCrateId::new("sample", &crate::work::CancellationProbe::new())
                        .expect("fixture crate id"),
                    RustModulePath::new(vec!["other".to_owned()]).expect("fixture module path"),
                ),
                RustItemKind::Struct,
                "Widget",
            ),
            "Widget".to_owned(),
        )
        .expect("valid distinct owner");

        assert_ne!(
            fixture
                .implementation("Widget")
                .descriptor_bytes()
                .expect("first descriptor"),
            ImplementationType::new(other_owner)
                .descriptor_bytes()
                .expect("second descriptor")
        );
    }

    #[test]
    fn implementation_owners_reject_non_type_declarations_and_invalid_spelling() {
        let fixture = ImplementationFixture::new();
        let module_id = fixture.owner_id.module_id().clone();

        for spelling in ["", " Widget", "Widget ", "Widget\n", "Widget\u{7}"] {
            let error = RustImplementationOwner::new(fixture.owner_id.clone(), spelling.to_owned())
                .expect_err("invalid owner spelling must fail");
            assert!(error.to_string().contains("implementation owner"));
        }

        let error = RustImplementationOwner::new(
            RustItemId::new(module_id, RustItemKind::Function, "Widget"),
            "Widget".to_owned(),
        )
        .expect_err("functions cannot own implementations");
        assert!(error.to_string().contains("cannot own"));
    }

    #[test]
    fn same_descriptor_blocks_append_and_finalize_in_canonical_associated_item_order() {
        let fixture = ImplementationFixture::new();
        let mut merged = fixture
            .implementation("Widget")
            .with_items(vec![ImplementationFixture::constant("zeta")]);
        merged
            .append_same_descriptor(
                fixture
                    .implementation("crate::model::Widget")
                    .with_items(vec![ImplementationFixture::constant("alpha")]),
            )
            .expect("same canonical owner descriptor appends");
        merged
            .sort_associated_items(&crate::work::CancellationProbe::new())
            .expect("merged block canonical order");

        let mut single_block = fixture.implementation("Widget").with_items(vec![
            ImplementationFixture::constant("alpha"),
            ImplementationFixture::constant("zeta"),
        ]);
        single_block
            .sort_associated_items(&crate::work::CancellationProbe::new())
            .expect("single block canonical order");

        assert_eq!(merged.items(), single_block.items());
        assert_eq!(merged.owner().spelling, "Widget");
    }

    #[test]
    fn implementation_finalization_observes_pre_cancellation_before_sorting() {
        let fixture = ImplementationFixture::new();
        let mut implementation = fixture.implementation("Widget").with_items(vec![
            ImplementationFixture::constant("zeta"),
            ImplementationFixture::constant("alpha"),
        ]);
        let cancellation = crate::work::CancellationProbe::new();
        cancellation.cancel();

        let error = implementation
            .sort_associated_items(&cancellation)
            .expect_err("canceled implementation finalization must stop");

        assert!(error.is_operation_canceled());
    }

    #[test]
    fn different_descriptors_fail_before_mutating_the_existing_block() {
        let fixture = ImplementationFixture::new();
        let original_items = vec![ImplementationFixture::constant("original")];
        let mut current = fixture
            .implementation("Widget")
            .with_items(original_items.clone());
        let incoming = fixture
            .implementation("Widget")
            .with_implemented_trait(
                RustImplementedTrait::for_trait(
                    "crate::Service".to_owned(),
                    RustImplPolarity::Positive,
                )
                .expect("fixture trait implementation"),
            )
            .with_items(vec![ImplementationFixture::constant("incoming")]);

        let error = current
            .append_same_descriptor(incoming)
            .expect_err("different descriptors cannot merge");

        assert!(error.to_string().contains("different descriptors"));
        assert_eq!(current.items(), original_items);
    }

    #[test]
    fn owner_context_applies_trait_visibility_and_preserves_inherent_restrictions() {
        let fixture = ImplementationFixture::new();
        let cancellation = crate::work::CancellationProbe::new();
        let mut trait_implementation = fixture
            .implementation("Widget")
            .with_implemented_trait(
                RustImplementedTrait::for_trait(
                    "crate::Service".to_owned(),
                    RustImplPolarity::Positive,
                )
                .expect("fixture trait implementation"),
            )
            .with_items(vec![
                ImplementationFixture::constant("ENABLED"),
                ImplementationFixture::associated_type("Output", Visibility::Private),
            ]);
        trait_implementation
            .normalize_for_owner(&fixture.context(Visibility::Public), &cancellation)
            .expect("trait items normalize to owner visibility");

        for item in trait_implementation.items() {
            let visibility = match item {
                RustAssociatedItem::Constant(constant) => constant.visibility(),
                RustAssociatedItem::Type(associated_type) => associated_type.visibility(),
                RustAssociatedItem::Method(_) => panic!("fixture has no method"),
            };
            assert_eq!(visibility, &Visibility::Public);
        }

        let restricted_module = RustModuleId::new(
            fixture.owner_id.module_id().crate_id().clone(),
            RustModulePath::new(vec!["impls".to_owned()]).expect("impl module"),
        );
        let mut inherent = fixture.implementation("Widget").with_items(vec![
            ImplementationFixture::associated_type(
                "Output",
                Visibility::Module(restricted_module.clone()),
            ),
        ]);
        inherent
            .normalize_for_owner(&fixture.context(Visibility::Public), &cancellation)
            .expect("inherent item normalization");
        let RustAssociatedItem::Type(associated_type) = &inherent.items()[0] else {
            panic!("fixture associated type");
        };
        assert_eq!(
            associated_type.visibility(),
            &Visibility::Module(restricted_module)
        );
    }

    #[test]
    fn implemented_trait_generics_and_impl_attributes_require_warnings() {
        let fixture = ImplementationFixture::new();
        let attribute: syn::Attribute = syn::parse_quote!(#[derive(Clone)]);
        let attributes =
            RustAttributes::from_syn(&[attribute], &crate::work::CancellationProbe::new())
                .expect("retained derive attribute");
        let generic_trait = fixture.implementation("Widget").with_implemented_trait(
            RustImplementedTrait::for_trait(
                "crate::Service<contract_type!()>".to_owned(),
                RustImplPolarity::Positive,
            )
            .expect("macro-bearing implemented trait"),
        );
        let attributed_impl = fixture.implementation("Widget").with_attributes(attributes);

        assert!(generic_trait.requires_capability_warning());
        assert!(attributed_impl.requires_capability_warning());
    }
}
