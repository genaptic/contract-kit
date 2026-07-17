use crate::SignatureContractKitError;
use crate::languages::rust::parser::source_graph::RustModulePath;
use crate::languages::rust::types::attributes::RustAttributes;
use crate::languages::rust::types::callable_type::RustFunctionAbi;
use crate::languages::rust::types::primitive_types::{
    FloatType, RustArrayType, RustFunctionPointerParameter, RustFunctionPointerType,
    RustFunctionPointerVariadic, RustImplTraitType, RustRawPointerType, RustReferenceType,
    RustTraitObjectType, RustType, RustTypePath, SignedIntegerType, UnsignedIntegerType,
};
use crate::languages::rust::types::syntax_text::RustSyntaxText;
use crate::work::CancellationProbe;
use quote::ToTokens;
use std::collections::BTreeSet;

#[derive(Debug)]
pub(crate) struct RustTypeConverter {
    generic_parameters: BTreeSet<String>,
    cancellation: CancellationProbe,
}

impl RustTypeConverter {
    pub(crate) fn new(cancellation: &CancellationProbe) -> Self {
        Self {
            generic_parameters: BTreeSet::new(),
            cancellation: cancellation.clone(),
        }
    }

    pub(crate) fn with_generic_parameters(
        generic_parameters: Vec<String>,
        cancellation: &CancellationProbe,
    ) -> Self {
        Self {
            generic_parameters: generic_parameters.into_iter().collect(),
            cancellation: cancellation.clone(),
        }
    }

    pub(crate) fn convert_type_text(
        &self,
        value: &str,
    ) -> Result<RustType, SignatureContractKitError> {
        let rust_type = syn::parse_str::<syn::Type>(value)
            .map_err(|source| SignatureContractKitError::parse_failed(value, source.to_string()))?;

        self.convert_type(rust_type)
    }

    pub(crate) fn convert_type(
        &self,
        rust_type: syn::Type,
    ) -> Result<RustType, SignatureContractKitError> {
        self.cancellation.checkpoint()?;
        match rust_type {
            syn::Type::Path(path) => Ok(self.convert_type_path(path)),
            syn::Type::Reference(reference) => self.convert_reference(reference),
            syn::Type::Slice(slice) => self.convert_slice(slice),
            syn::Type::Array(array) => self.convert_array(array),
            syn::Type::Tuple(tuple) => self.convert_tuple(tuple),
            syn::Type::Never(_) => Ok(RustType::Never),
            syn::Type::Ptr(pointer) => self.convert_raw_pointer(pointer),
            syn::Type::BareFn(function_pointer) => self.convert_function_pointer(function_pointer),
            syn::Type::TraitObject(trait_object) => self.convert_trait_object(trait_object),
            syn::Type::ImplTrait(impl_trait) => self.convert_impl_trait(impl_trait),
            syn::Type::Infer(_) => Ok(RustType::Inferred),
            syn::Type::Paren(parenthesized) => self
                .convert_type(*parenthesized.elem)
                .map(|converted| RustType::Parenthesized(Box::new(converted))),
            syn::Type::Group(group) => self.convert_type(*group.elem),
            syn::Type::Macro(type_macro) => Ok(RustType::MacroInvocation(self.tokens(&type_macro))),
            syn::Type::Verbatim(tokens) => Err(self.unsupported_type("verbatim type", &tokens)),
            unsupported => Err(self.unsupported_type("future type", &unsupported)),
        }
    }

    fn convert_type_path(&self, path: syn::TypePath) -> RustType {
        if path.qself.is_some() {
            return RustType::TypePath(RustTypePath::from_syn(&path));
        }

        let mut segments = path.path.segments.iter();
        let Some(first) = segments.next() else {
            return RustType::TypePath(RustTypePath::new(Vec::new()));
        };

        if segments.next().is_none() && matches!(first.arguments, syn::PathArguments::None) {
            return self.convert_plain_path_segment(first);
        }

        RustType::TypePath(RustTypePath::from_syn(&path))
    }

    fn convert_plain_path_segment(&self, segment: &syn::PathSegment) -> RustType {
        let ident = RustModulePath::semantic_ident(&segment.ident);

        if self.generic_parameters.contains(&ident) {
            return RustType::GenericParameter(ident);
        }

        match ident.as_str() {
            "bool" => RustType::Bool,
            "char" => RustType::Char,
            "str" => RustType::Str,
            "String" => RustType::String,
            "i8" => RustType::SignedInteger(SignedIntegerType::I8),
            "i16" => RustType::SignedInteger(SignedIntegerType::I16),
            "i32" => RustType::SignedInteger(SignedIntegerType::I32),
            "i64" => RustType::SignedInteger(SignedIntegerType::I64),
            "i128" => RustType::SignedInteger(SignedIntegerType::I128),
            "isize" => RustType::SignedInteger(SignedIntegerType::Isize),
            "u8" => RustType::UnsignedInteger(UnsignedIntegerType::U8),
            "u16" => RustType::UnsignedInteger(UnsignedIntegerType::U16),
            "u32" => RustType::UnsignedInteger(UnsignedIntegerType::U32),
            "u64" => RustType::UnsignedInteger(UnsignedIntegerType::U64),
            "u128" => RustType::UnsignedInteger(UnsignedIntegerType::U128),
            "usize" => RustType::UnsignedInteger(UnsignedIntegerType::Usize),
            "f32" => RustType::Float(FloatType::F32),
            "f64" => RustType::Float(FloatType::F64),
            "Self" => RustType::SelfType,
            _ => RustType::TypePath(RustTypePath::from_path_segment(segment)),
        }
    }

    fn convert_reference(
        &self,
        reference: syn::TypeReference,
    ) -> Result<RustType, SignatureContractKitError> {
        Ok(RustType::Reference(RustReferenceType::new(
            reference.lifetime.map(|lifetime| lifetime.to_string()),
            reference.mutability.is_some(),
            self.convert_type(*reference.elem)?,
        )))
    }

    fn convert_slice(&self, slice: syn::TypeSlice) -> Result<RustType, SignatureContractKitError> {
        Ok(RustType::Slice(Box::new(self.convert_type(*slice.elem)?)))
    }

    fn convert_array(&self, array: syn::TypeArray) -> Result<RustType, SignatureContractKitError> {
        Ok(RustType::Array(RustArrayType::new(
            self.convert_type(*array.elem)?,
            self.tokens(&array.len),
        )))
    }

    fn convert_tuple(&self, tuple: syn::TypeTuple) -> Result<RustType, SignatureContractKitError> {
        if tuple.elems.is_empty() {
            return Ok(RustType::Unit);
        }

        let mut elements = Vec::with_capacity(tuple.elems.len());
        for element in tuple.elems {
            self.cancellation.checkpoint()?;
            elements.push(self.convert_type(element)?);
        }
        Ok(RustType::Tuple(elements))
    }

    fn convert_raw_pointer(
        &self,
        pointer: syn::TypePtr,
    ) -> Result<RustType, SignatureContractKitError> {
        Ok(RustType::RawPointer(RustRawPointerType::new(
            pointer.mutability.is_some(),
            self.convert_type(*pointer.elem)?,
        )))
    }

    fn convert_function_pointer(
        &self,
        function_pointer: syn::TypeBareFn,
    ) -> Result<RustType, SignatureContractKitError> {
        let mut lifetimes = Vec::new();
        if let Some(bound_lifetimes) = function_pointer.lifetimes {
            lifetimes.reserve(bound_lifetimes.lifetimes.len());
            for lifetime in bound_lifetimes.lifetimes {
                self.cancellation.checkpoint()?;
                lifetimes.push(self.tokens(&lifetime));
            }
        }
        let mut parameters = Vec::with_capacity(function_pointer.inputs.len());
        for argument in function_pointer.inputs {
            self.cancellation.checkpoint()?;
            parameters.push(RustFunctionPointerParameter::new(
                RustAttributes::from_syn(&argument.attrs, &self.cancellation)?,
                self.convert_type(argument.ty)?,
            ));
        }
        let return_type = match function_pointer.output {
            syn::ReturnType::Default => None,
            syn::ReturnType::Type(_, return_type) => Some(self.convert_type(*return_type)?),
        };

        Ok(RustType::FunctionPointer(
            RustFunctionPointerType::from_parts(
                parameters,
                return_type,
                function_pointer.unsafety.is_some(),
            )
            .with_lifetimes(lifetimes)
            .with_abi(self.convert_function_pointer_abi(function_pointer.abi.as_ref()))
            .with_variadic(
                self.convert_function_pointer_variadic(function_pointer.variadic.as_ref())?,
            ),
        ))
    }

    fn convert_function_pointer_abi(&self, abi: Option<&syn::Abi>) -> RustFunctionAbi {
        match abi {
            None => RustFunctionAbi::Rust,
            Some(abi) => RustFunctionAbi::Extern {
                name: abi.name.as_ref().map(syn::LitStr::value),
            },
        }
    }

    fn convert_function_pointer_variadic(
        &self,
        variadic: Option<&syn::BareVariadic>,
    ) -> Result<Option<RustFunctionPointerVariadic>, SignatureContractKitError> {
        variadic
            .map(|variadic| {
                Ok(RustFunctionPointerVariadic::new(
                    variadic
                        .name
                        .as_ref()
                        .map(|(name, _)| RustSyntaxText::from_identifier_pattern(name)),
                    RustAttributes::from_syn(&variadic.attrs, &self.cancellation)?,
                ))
            })
            .transpose()
    }

    fn convert_trait_object(
        &self,
        trait_object: syn::TypeTraitObject,
    ) -> Result<RustType, SignatureContractKitError> {
        let mut bounds = Vec::with_capacity(trait_object.bounds.len());
        for bound in trait_object.bounds {
            self.cancellation.checkpoint()?;
            bounds.push(self.tokens(&bound));
        }
        Ok(RustType::TraitObject(RustTraitObjectType::new(bounds)))
    }

    fn convert_impl_trait(
        &self,
        impl_trait: syn::TypeImplTrait,
    ) -> Result<RustType, SignatureContractKitError> {
        let mut bounds = Vec::with_capacity(impl_trait.bounds.len());
        for bound in impl_trait.bounds {
            self.cancellation.checkpoint()?;
            bounds.push(self.tokens(&bound));
        }
        Ok(RustType::ImplTrait(RustImplTraitType::new(bounds)))
    }

    fn tokens(&self, value: &impl ToTokens) -> String {
        value.to_token_stream().to_string()
    }

    fn unsupported_type(
        &self,
        syntax_kind: &str,
        value: &impl ToTokens,
    ) -> SignatureContractKitError {
        SignatureContractKitError::conversion_failed(format!(
            "unsupported Rust syntax {syntax_kind}: {}",
            self.tokens(value)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::RustTypeConverter;
    use crate::languages::rust::types::attributes::RustAttributes;
    use crate::languages::rust::types::callable_type::RustFunctionAbi;
    use crate::languages::rust::types::primitive_types::{
        FloatType, RustFunctionPointerParameter, RustFunctionPointerType,
        RustFunctionPointerVariadic, RustReferenceType, RustType, SignedIntegerType,
        UnsignedIntegerType,
    };
    use crate::languages::rust::types::syntax_text::RustSyntaxText;

    struct TypeFixture {
        converter: RustTypeConverter,
    }

    impl Default for TypeFixture {
        fn default() -> Self {
            let cancellation = crate::work::CancellationProbe::new();
            Self {
                converter: RustTypeConverter::new(&cancellation),
            }
        }
    }

    impl TypeFixture {
        fn with_generic_parameters(parameters: &[&str]) -> Self {
            let cancellation = crate::work::CancellationProbe::new();
            Self {
                converter: RustTypeConverter::with_generic_parameters(
                    parameters
                        .iter()
                        .map(|parameter| (*parameter).to_owned())
                        .collect(),
                    &cancellation,
                ),
            }
        }

        fn convert(&self, source: &str) -> RustType {
            let rust_type = syn::parse_str(source).expect("Rust type fixture");
            self.converter
                .convert_type(rust_type)
                .expect("converted Rust type")
        }

        fn try_convert(&self, source: &str) -> Result<RustType, crate::SignatureContractKitError> {
            let rust_type = syn::parse_str(source).expect("Rust type fixture");
            self.converter.convert_type(rust_type)
        }
    }

    #[test]
    fn converts_primitive_scalars() {
        let fixture = TypeFixture::default();

        assert_eq!(fixture.convert("bool"), RustType::Bool);
        assert_eq!(
            fixture.convert("i64"),
            RustType::SignedInteger(SignedIntegerType::I64)
        );
        assert_eq!(
            fixture.convert("usize"),
            RustType::UnsignedInteger(UnsignedIntegerType::Usize)
        );
        assert_eq!(fixture.convert("f32"), RustType::Float(FloatType::F32));
    }

    #[test]
    fn converts_compound_types() {
        let fixture = TypeFixture::default();

        assert!(matches!(
            fixture.convert("&'a mut Vec<T>"),
            RustType::Reference(_)
        ));
        assert!(matches!(fixture.convert("[u8; 32]"), RustType::Array(_)));
        assert!(matches!(
            fixture.convert("(String, usize)"),
            RustType::Tuple(_)
        ));
    }

    #[test]
    fn converts_pointer_trait_and_impl_types() {
        let fixture = TypeFixture::default();

        assert!(matches!(
            fixture.convert("*const u8"),
            RustType::RawPointer(_)
        ));
        assert!(matches!(
            fixture.convert("fn(u8) -> bool"),
            RustType::FunctionPointer(_)
        ));
        assert!(matches!(
            fixture.convert("dyn Send + Sync"),
            RustType::TraitObject(_)
        ));
        assert!(matches!(
            fixture.convert("impl Iterator<Item = u8>"),
            RustType::ImplTrait(_)
        ));
    }

    #[test]
    fn genuine_type_macro_is_retained_and_requires_a_capability_warning() {
        let converted = TypeFixture::default().convert("project_type!(r#type)");

        assert_eq!(
            converted,
            RustType::MacroInvocation("project_type ! (r#type)".to_owned())
        );
        assert!(converted.requires_capability_warning());
    }

    #[test]
    fn verbatim_type_fails_closed_with_its_syntax_kind_and_tokens() {
        let error = TypeFixture::default()
            .converter
            .convert_type(syn::Type::Verbatim(quote::quote!(verbatim_type<r#type>)))
            .expect_err("verbatim type must fail closed");
        let message = error.to_string();

        assert!(message.contains("verbatim type"), "{message}");
        assert!(message.contains("verbatim_type < r#type >"), "{message}");
    }

    #[test]
    fn future_type_proxy_fails_closed_with_distinct_kind_and_tokens() {
        let error = TypeFixture::default()
            .converter
            .unsupported_type("future type", &quote::quote!(future_type<r#type>));
        let message = error.to_string();

        assert!(message.contains("future type"), "{message}");
        assert!(message.contains("future_type < r#type >"), "{message}");
        assert!(!message.contains("verbatim type"), "{message}");
    }

    #[test]
    fn function_pointer_abi_changes_converted_type() {
        let fixture = TypeFixture::default();

        let rust = fixture.convert("fn(u8) -> bool");
        let c = fixture.convert(r#"extern "C" fn(u8) -> bool"#);

        assert_ne!(rust, c);
    }

    #[test]
    fn function_pointer_variadic_changes_converted_type() {
        let fixture = TypeFixture::default();

        let fixed = fixture.convert(r#"unsafe extern "C" fn(*const u8)"#);
        let variadic = fixture.convert(r#"unsafe extern "C" fn(*const u8, ...)"#);

        assert_ne!(fixed, variadic);
    }

    #[test]
    fn function_pointer_lifetimes_change_converted_type() {
        let fixture = TypeFixture::default();

        let bare = fixture.convert("fn(&u8)");
        let higher_ranked = fixture.convert("for<'a> fn(&'a u8)");

        assert_ne!(bare, higher_ranked);
    }

    #[test]
    fn converts_function_pointer_lifetimes_exactly() {
        let fixture = TypeFixture::default();

        let actual = fixture.convert("for<'a> fn(&'a u8)");
        let expected = RustType::FunctionPointer(
            RustFunctionPointerType::from_parts(
                vec![RustFunctionPointerParameter::new(
                    RustAttributes::default(),
                    RustType::Reference(RustReferenceType::new(
                        Some("'a".to_owned()),
                        false,
                        RustType::UnsignedInteger(UnsignedIntegerType::U8),
                    )),
                )],
                None,
                false,
            )
            .with_lifetimes(vec!["'a".to_owned()]),
        );

        assert_eq!(actual, expected);
    }

    #[test]
    fn function_pointer_argument_attributes_change_converted_type() {
        let fixture = TypeFixture::default();

        let plain = fixture.convert("fn(u8)");
        let attributed = fixture.convert(r#"fn(#[cfg(target_pointer_width = "64")] u8)"#);

        assert_ne!(plain, attributed);
    }

    #[test]
    fn converts_function_pointer_argument_attributes_exactly() {
        let fixture = TypeFixture::default();

        let actual = fixture.convert(r#"fn(#[cfg(target_pointer_width = "64")] u8)"#);
        let expected_attributes = RustAttributes::from_syn(
            &[syn::parse_quote!(
                #[cfg(target_pointer_width = "64")]
            )],
            &crate::work::CancellationProbe::new(),
        )
        .expect("expected semantic attributes");
        let expected = RustType::FunctionPointer(RustFunctionPointerType::from_parts(
            vec![RustFunctionPointerParameter::new(
                expected_attributes,
                RustType::UnsignedInteger(UnsignedIntegerType::U8),
            )],
            None,
            false,
        ));

        assert_eq!(actual, expected);
    }

    #[test]
    fn converts_function_pointer_abi_and_variadic_exactly() {
        let fixture = TypeFixture::default();

        let actual = fixture.convert(r#"unsafe extern "C" fn(*const u8, args: ...)"#);
        let expected = RustType::FunctionPointer(
            RustFunctionPointerType::from_parts(
                vec![RustFunctionPointerParameter::new(
                    RustAttributes::default(),
                    RustType::RawPointer(
                        crate::languages::rust::types::primitive_types::RustRawPointerType::new(
                            false,
                            RustType::UnsignedInteger(UnsignedIntegerType::U8),
                        ),
                    ),
                )],
                None,
                true,
            )
            .with_abi(RustFunctionAbi::Extern {
                name: Some("C".to_owned()),
            })
            .with_variadic(Some(RustFunctionPointerVariadic::new(
                Some(RustSyntaxText::parse_pattern("args").expect("variadic pattern")),
                RustAttributes::default(),
            ))),
        );

        assert_eq!(actual, expected);
    }

    #[test]
    fn function_pointer_argument_names_do_not_change_converted_type() {
        let fixture = TypeFixture::default();

        let left = fixture.convert("fn(left: u8)");
        let right = fixture.convert("fn(right: u8)");

        assert_eq!(left, right);
    }

    #[test]
    fn malformed_function_pointer_parameter_attribute_fails_closed() {
        let error = TypeFixture::default()
            .try_convert("fn(#[repr()] u8)")
            .expect_err("empty repr must fail");

        assert!(error.to_string().contains("repr"));
    }

    #[test]
    fn function_pointer_variadic_attributes_use_the_semantic_attribute_model() {
        let actual = TypeFixture::default()
            .convert(r#"unsafe extern "C" fn(*const u8, #[cfg(unix)] args: ...)"#);
        let RustType::FunctionPointer(pointer) = actual else {
            panic!("expected a function pointer");
        };
        let variadic = pointer.variadic().expect("variadic metadata");

        assert!(variadic.attributes().requires_capability_warning());
    }

    #[test]
    fn uses_declared_generic_context() {
        let fixture = TypeFixture::with_generic_parameters(&["Item"]);

        assert_eq!(
            fixture.convert("Item"),
            RustType::GenericParameter("Item".to_owned())
        );
        assert_eq!(
            fixture.convert("Output"),
            RustType::TypePath(
                crate::languages::rust::types::primitive_types::RustTypePath::new(vec![
                    "Output".to_owned()
                ])
            )
        );
    }

    #[test]
    fn raw_generic_use_matches_its_unraw_semantic_declaration() {
        let fixture = TypeFixture::with_generic_parameters(&["type"]);

        assert_eq!(
            fixture.convert("r#type"),
            RustType::GenericParameter("type".to_owned())
        );
    }

    #[test]
    fn raw_named_type_path_keeps_source_syntax_out_of_semantic_identity() {
        let rust_type = TypeFixture::default().convert("r#type");
        let RustType::TypePath(path) = &rust_type else {
            panic!("raw keyword must remain a named type path");
        };

        assert_eq!(path.source_syntax(), "r#type");
        assert!(
            !serde_json::to_string(&rust_type)
                .expect("canonical raw named type")
                .contains("r#"),
            "raw source spelling must not enter semantic identity"
        );
        assert!(
            !rust_type.requires_capability_warning(),
            "valid raw identifier syntax must not create a capability warning"
        );
    }

    #[test]
    fn qualified_raw_type_paths_preserve_parseable_source_and_canonical_semantics() {
        let raw = TypeFixture::default().convert("::r#model::r#Widget<r#Item, nested::r#Output>");
        let ordinary = TypeFixture::default().convert("::model::Widget<Item, nested::Output>");
        let RustType::TypePath(path) = &raw else {
            panic!("qualified raw path must remain a named type path");
        };

        assert_eq!(raw, ordinary, "raw prefixes are not semantic identity");
        assert_eq!(
            serde_json::to_vec(&raw).expect("raw canonical path"),
            serde_json::to_vec(&ordinary).expect("ordinary canonical path")
        );
        syn::parse_str::<syn::Type>(path.source_syntax())
            .expect("retained raw path must remain valid Rust source");
        assert!(path.source_syntax().starts_with("::r#model"));
        assert!(path.source_syntax().contains("r#Widget"));
        assert!(path.source_syntax().contains("r#Item"));
        assert!(path.source_syntax().contains("r#Output"));
        assert!(!raw.requires_capability_warning());
    }

    #[test]
    fn qualified_self_raw_type_path_preserves_source_without_polluting_semantics() {
        let projected =
            TypeFixture::default().convert("<r#model::r#Type as r#contract::r#Trait>::r#Output");
        let RustType::TypePath(path) = &projected else {
            panic!("qualified-self raw path must remain a named type path");
        };

        syn::parse_str::<syn::Type>(path.source_syntax())
            .expect("qualified-self raw path must remain valid Rust source");
        assert!(path.source_syntax().contains("r#model"));
        assert!(path.source_syntax().contains("r#contract"));
        assert!(path.source_syntax().contains("r#Output"));
        assert!(
            !serde_json::to_string(&projected)
                .expect("qualified-self canonical path")
                .contains("r#")
        );
        assert!(!projected.requires_capability_warning());
    }

    #[test]
    fn canceled_nested_type_conversion_stops_before_recursive_work() {
        let cancellation = crate::work::CancellationProbe::new();
        cancellation.cancel();
        let converter = RustTypeConverter::new(&cancellation);
        let rust_type = syn::parse_str::<syn::Type>(
            "((((u8, u16), (u32, u64)), ((i8, i16), (i32, i64))), usize)",
        )
        .expect("nested type");

        let error = converter
            .convert_type(rust_type)
            .expect_err("canceled type conversion must stop");

        assert!(error.is_operation_canceled());
    }
}
