use crate::SignatureContractKitError;
use crate::languages::rust::types::callable_type::RustFunctionAbi;
use crate::languages::rust::types::primitive_types::{
    FloatType, RustArrayType, RustFunctionPointerParameter, RustFunctionPointerType,
    RustFunctionPointerVariadic, RustImplTraitType, RustRawPointerType, RustReferenceType,
    RustTraitObjectType, RustType, RustTypePath, SignedIntegerType, UnsignedIntegerType,
};
use quote::ToTokens;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default)]
pub(crate) struct RustTypeConverter {
    generic_parameters: BTreeSet<String>,
}

impl RustTypeConverter {
    pub(crate) fn with_generic_parameters(generic_parameters: Vec<String>) -> Self {
        Self {
            generic_parameters: generic_parameters.into_iter().collect(),
        }
    }

    pub(crate) fn convert_type_text(
        &self,
        value: &str,
    ) -> Result<RustType, SignatureContractKitError> {
        let rust_type = syn::parse_str::<syn::Type>(value)
            .map_err(|source| SignatureContractKitError::parse_failed(value, source.to_string()))?;

        Ok(self.convert_type(rust_type))
    }

    pub(crate) fn convert_type(&self, rust_type: syn::Type) -> RustType {
        match rust_type {
            syn::Type::Path(path) => self.convert_type_path(path),
            syn::Type::Reference(reference) => self.convert_reference(reference),
            syn::Type::Slice(slice) => self.convert_slice(slice),
            syn::Type::Array(array) => self.convert_array(array),
            syn::Type::Tuple(tuple) => self.convert_tuple(tuple),
            syn::Type::Never(_) => RustType::Never,
            syn::Type::Ptr(pointer) => self.convert_raw_pointer(pointer),
            syn::Type::BareFn(function_pointer) => self.convert_function_pointer(function_pointer),
            syn::Type::TraitObject(trait_object) => self.convert_trait_object(trait_object),
            syn::Type::ImplTrait(impl_trait) => self.convert_impl_trait(impl_trait),
            syn::Type::Infer(_) => RustType::Inferred,
            syn::Type::Paren(parenthesized) => {
                RustType::Parenthesized(Box::new(self.convert_type(*parenthesized.elem)))
            }
            syn::Type::Group(group) => self.convert_type(*group.elem),
            syn::Type::Macro(type_macro) => RustType::MacroInvocation(self.tokens(&type_macro)),
            other => RustType::MacroInvocation(self.tokens(&other)),
        }
    }

    fn convert_type_path(&self, path: syn::TypePath) -> RustType {
        if path.qself.is_some() {
            return RustType::TypePath(RustTypePath::new(vec![self.tokens(&path)]));
        }

        let mut segments = path.path.segments.iter();
        let Some(first) = segments.next() else {
            return RustType::TypePath(RustTypePath::new(Vec::new()));
        };

        if segments.next().is_none() && matches!(first.arguments, syn::PathArguments::None) {
            return self.convert_plain_path_segment(&first.ident.to_string());
        }

        RustType::TypePath(RustTypePath::new(
            path.path
                .segments
                .iter()
                .map(|segment| self.tokens(segment))
                .collect(),
        ))
    }

    fn convert_plain_path_segment(&self, ident: &str) -> RustType {
        match ident {
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
            value if self.generic_parameters.contains(value) => {
                RustType::GenericParameter(value.to_owned())
            }
            value => RustType::TypePath(RustTypePath::new(vec![value.to_owned()])),
        }
    }

    fn convert_reference(&self, reference: syn::TypeReference) -> RustType {
        RustType::Reference(RustReferenceType::new(
            reference.lifetime.map(|lifetime| lifetime.to_string()),
            reference.mutability.is_some(),
            self.convert_type(*reference.elem),
        ))
    }

    fn convert_slice(&self, slice: syn::TypeSlice) -> RustType {
        RustType::Slice(Box::new(self.convert_type(*slice.elem)))
    }

    fn convert_array(&self, array: syn::TypeArray) -> RustType {
        RustType::Array(RustArrayType::new(
            self.convert_type(*array.elem),
            self.tokens(&array.len),
        ))
    }

    fn convert_tuple(&self, tuple: syn::TypeTuple) -> RustType {
        if tuple.elems.is_empty() {
            return RustType::Unit;
        }

        RustType::Tuple(
            tuple
                .elems
                .into_iter()
                .map(|element| self.convert_type(element))
                .collect(),
        )
    }

    fn convert_raw_pointer(&self, pointer: syn::TypePtr) -> RustType {
        RustType::RawPointer(RustRawPointerType::new(
            pointer.mutability.is_some(),
            self.convert_type(*pointer.elem),
        ))
    }

    fn convert_function_pointer(&self, function_pointer: syn::TypeBareFn) -> RustType {
        let lifetimes = function_pointer
            .lifetimes
            .map(|lifetimes| {
                lifetimes
                    .lifetimes
                    .into_iter()
                    .map(|lifetime| self.tokens(&lifetime))
                    .collect()
            })
            .unwrap_or_default();
        let parameters = function_pointer
            .inputs
            .into_iter()
            .map(|argument| {
                RustFunctionPointerParameter::new(
                    argument
                        .attrs
                        .iter()
                        .map(|attribute| self.tokens(attribute))
                        .collect(),
                    self.convert_type(argument.ty),
                )
            })
            .collect();
        let return_type = match function_pointer.output {
            syn::ReturnType::Default => None,
            syn::ReturnType::Type(_, return_type) => Some(self.convert_type(*return_type)),
        };

        RustType::FunctionPointer(
            RustFunctionPointerType::from_parts(
                parameters,
                return_type,
                function_pointer.unsafety.is_some(),
            )
            .with_lifetimes(lifetimes)
            .with_abi(self.convert_function_pointer_abi(function_pointer.abi.as_ref()))
            .with_variadic(
                self.convert_function_pointer_variadic(function_pointer.variadic.as_ref()),
            ),
        )
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
    ) -> Option<RustFunctionPointerVariadic> {
        variadic.map(|variadic| {
            RustFunctionPointerVariadic::new(
                variadic.name.as_ref().map(|(name, _)| name.to_string()),
                variadic
                    .attrs
                    .iter()
                    .map(|attribute| self.tokens(attribute))
                    .collect(),
            )
        })
    }

    fn convert_trait_object(&self, trait_object: syn::TypeTraitObject) -> RustType {
        RustType::TraitObject(RustTraitObjectType::new(
            trait_object
                .bounds
                .into_iter()
                .map(|bound| self.tokens(&bound))
                .collect(),
        ))
    }

    fn convert_impl_trait(&self, impl_trait: syn::TypeImplTrait) -> RustType {
        RustType::ImplTrait(RustImplTraitType::new(
            impl_trait
                .bounds
                .into_iter()
                .map(|bound| self.tokens(&bound))
                .collect(),
        ))
    }

    fn tokens(&self, value: &impl ToTokens) -> String {
        value.to_token_stream().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::RustTypeConverter;
    use crate::languages::rust::types::callable_type::RustFunctionAbi;
    use crate::languages::rust::types::primitive_types::{
        FloatType, RustFunctionPointerParameter, RustFunctionPointerType,
        RustFunctionPointerVariadic, RustReferenceType, RustType, SignedIntegerType,
        UnsignedIntegerType,
    };

    #[test]
    fn converts_primitive_scalars() {
        let converter = RustTypeConverter::default();

        assert_eq!(
            converter.convert_type(syn::parse_str("bool").expect("type")),
            RustType::Bool
        );
        assert_eq!(
            converter.convert_type(syn::parse_str("i64").expect("type")),
            RustType::SignedInteger(SignedIntegerType::I64)
        );
        assert_eq!(
            converter.convert_type(syn::parse_str("usize").expect("type")),
            RustType::UnsignedInteger(UnsignedIntegerType::Usize)
        );
        assert_eq!(
            converter.convert_type(syn::parse_str("f32").expect("type")),
            RustType::Float(FloatType::F32)
        );
    }

    #[test]
    fn converts_compound_types() {
        let converter = RustTypeConverter::default();

        assert!(matches!(
            converter.convert_type(syn::parse_str("&'a mut Vec<T>").expect("type")),
            RustType::Reference(_)
        ));
        assert!(matches!(
            converter.convert_type(syn::parse_str("[u8; 32]").expect("type")),
            RustType::Array(_)
        ));
        assert!(matches!(
            converter.convert_type(syn::parse_str("(String, usize)").expect("type")),
            RustType::Tuple(_)
        ));
    }

    #[test]
    fn converts_pointer_trait_and_impl_types() {
        let converter = RustTypeConverter::default();

        assert!(matches!(
            converter.convert_type(syn::parse_str("*const u8").expect("type")),
            RustType::RawPointer(_)
        ));
        assert!(matches!(
            converter.convert_type(syn::parse_str("fn(u8) -> bool").expect("type")),
            RustType::FunctionPointer(_)
        ));
        assert!(matches!(
            converter.convert_type(syn::parse_str("dyn Send + Sync").expect("type")),
            RustType::TraitObject(_)
        ));
        assert!(matches!(
            converter.convert_type(syn::parse_str("impl Iterator<Item = u8>").expect("type")),
            RustType::ImplTrait(_)
        ));
    }

    #[test]
    fn function_pointer_abi_changes_converted_type() {
        let converter = RustTypeConverter::default();

        let rust = converter.convert_type(syn::parse_str("fn(u8) -> bool").expect("type"));
        let c =
            converter.convert_type(syn::parse_str(r#"extern "C" fn(u8) -> bool"#).expect("type"));

        assert_ne!(rust, c);
    }

    #[test]
    fn function_pointer_variadic_changes_converted_type() {
        let converter = RustTypeConverter::default();

        let fixed = converter
            .convert_type(syn::parse_str(r#"unsafe extern "C" fn(*const u8)"#).expect("type"));
        let variadic = converter
            .convert_type(syn::parse_str(r#"unsafe extern "C" fn(*const u8, ...)"#).expect("type"));

        assert_ne!(fixed, variadic);
    }

    #[test]
    fn function_pointer_lifetimes_change_converted_type() {
        let converter = RustTypeConverter::default();

        let bare = converter.convert_type(syn::parse_str("fn(&u8)").expect("type"));
        let higher_ranked =
            converter.convert_type(syn::parse_str("for<'a> fn(&'a u8)").expect("type"));

        assert_ne!(bare, higher_ranked);
    }

    #[test]
    fn converts_function_pointer_lifetimes_exactly() {
        let converter = RustTypeConverter::default();

        let actual = converter.convert_type(syn::parse_str("for<'a> fn(&'a u8)").expect("type"));
        let expected = RustType::FunctionPointer(
            RustFunctionPointerType::from_parts(
                vec![RustFunctionPointerParameter::new(
                    Vec::new(),
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
        let converter = RustTypeConverter::default();

        let plain = converter.convert_type(syn::parse_str("fn(u8)").expect("type"));
        let attributed = converter.convert_type(
            syn::parse_str(r#"fn(#[cfg(target_pointer_width = "64")] u8)"#).expect("type"),
        );

        assert_ne!(plain, attributed);
    }

    #[test]
    fn converts_function_pointer_argument_attributes_exactly() {
        let converter = RustTypeConverter::default();

        let actual = converter.convert_type(
            syn::parse_str(r#"fn(#[cfg(target_pointer_width = "64")] u8)"#).expect("type"),
        );
        let expected = RustType::FunctionPointer(RustFunctionPointerType::from_parts(
            vec![RustFunctionPointerParameter::new(
                vec![r#"# [cfg (target_pointer_width = "64")]"#.to_owned()],
                RustType::UnsignedInteger(UnsignedIntegerType::U8),
            )],
            None,
            false,
        ));

        assert_eq!(actual, expected);
    }

    #[test]
    fn converts_function_pointer_abi_and_variadic_exactly() {
        let converter = RustTypeConverter::default();

        let actual = converter.convert_type(
            syn::parse_str(r#"unsafe extern "C" fn(*const u8, args: ...)"#).expect("type"),
        );
        let expected = RustType::FunctionPointer(
            RustFunctionPointerType::from_parts(
                vec![RustFunctionPointerParameter::new(
                    Vec::new(),
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
                Some("args".to_owned()),
                Vec::new(),
            ))),
        );

        assert_eq!(actual, expected);
    }

    #[test]
    fn function_pointer_argument_names_do_not_change_converted_type() {
        let converter = RustTypeConverter::default();

        let left = converter.convert_type(syn::parse_str("fn(left: u8)").expect("type"));
        let right = converter.convert_type(syn::parse_str("fn(right: u8)").expect("type"));

        assert_eq!(left, right);
    }

    #[test]
    fn uses_declared_generic_context() {
        let converter = RustTypeConverter::with_generic_parameters(vec!["Item".to_owned()]);

        assert_eq!(
            converter.convert_type(syn::parse_str("Item").expect("type")),
            RustType::GenericParameter("Item".to_owned())
        );
        assert_eq!(
            converter.convert_type(syn::parse_str("Output").expect("type")),
            RustType::TypePath(
                crate::languages::rust::types::primitive_types::RustTypePath::new(vec![
                    "Output".to_owned()
                ])
            )
        );
    }
}
