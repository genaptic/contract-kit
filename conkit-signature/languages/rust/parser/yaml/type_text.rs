use crate::error::SignatureContractKitError;
use crate::languages::rust::parser::type_converter::RustTypeConverter;
use crate::languages::rust::types::callable_type::RustFunctionAbi;
use crate::languages::rust::types::primitive_types::{
    FloatType, RustFunctionPointerParameter, RustFunctionPointerType, RustFunctionPointerVariadic,
    RustGenericMetadata, RustGenericParameter, RustType, SignedIntegerType, UnsignedIntegerType,
};

#[derive(Default)]
pub(super) struct RustYamlGenericContext {
    type_parameters: Vec<String>,
}

impl RustYamlGenericContext {
    pub(super) fn from_metadata(metadata: &RustGenericMetadata) -> Self {
        Self {
            type_parameters: metadata
                .parameters()
                .iter()
                .filter_map(|parameter| match parameter {
                    RustGenericParameter::Type { name, .. } => Some(name.clone()),
                    RustGenericParameter::Lifetime { .. } | RustGenericParameter::Const { .. } => {
                        None
                    }
                })
                .collect(),
        }
    }

    pub(super) fn with_metadata(&self, metadata: &RustGenericMetadata) -> Self {
        let mut type_parameters = self.type_parameters.clone();
        type_parameters.extend(metadata.parameters().iter().filter_map(
            |parameter| match parameter {
                RustGenericParameter::Type { name, .. } => Some(name.clone()),
                RustGenericParameter::Lifetime { .. } | RustGenericParameter::Const { .. } => None,
            },
        ));
        Self { type_parameters }
    }

    pub(super) fn type_parameters(&self) -> Vec<String> {
        self.type_parameters.clone()
    }
}

pub(super) struct RustYamlTypeText {
    value: String,
}

impl RustYamlTypeText {
    pub(super) fn from_text(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
        }
    }

    pub(super) fn parse(
        &self,
        context: &RustYamlGenericContext,
    ) -> Result<RustType, SignatureContractKitError> {
        RustTypeConverter::with_generic_parameters(context.type_parameters())
            .convert_type_text(&self.value)
    }
}

pub(super) struct RustTypeTextRenderer;

impl RustTypeTextRenderer {
    pub(super) fn render_type(&self, value: &RustType) -> String {
        match value {
            RustType::Bool => "bool".to_owned(),
            RustType::Char => "char".to_owned(),
            RustType::Str => "str".to_owned(),
            RustType::String => "String".to_owned(),
            RustType::Never => "!".to_owned(),
            RustType::SignedInteger(value) => self.render_signed_integer(*value).to_owned(),
            RustType::UnsignedInteger(value) => self.render_unsigned_integer(*value).to_owned(),
            RustType::Float(value) => self.render_float(*value).to_owned(),
            RustType::Unit => "()".to_owned(),
            RustType::Tuple(values) => self.render_tuple(values),
            RustType::Array(value) => {
                format!(
                    "[{}; {}]",
                    self.render_type(value.element_type()),
                    value.length()
                )
            }
            RustType::Slice(value) => format!("[{}]", self.render_type(value)),
            RustType::Reference(value) => self.render_reference(value),
            RustType::RawPointer(value) => self.render_raw_pointer(value),
            RustType::FunctionPointer(value) => self.render_function_pointer(value),
            RustType::TraitObject(value) => format!("dyn {}", value.bounds().join(" + ")),
            RustType::ImplTrait(value) => format!("impl {}", value.bounds().join(" + ")),
            RustType::TypePath(value) => value.segments().join("::"),
            RustType::GenericParameter(value) => value.clone(),
            RustType::SelfType => "Self".to_owned(),
            RustType::Inferred => "_".to_owned(),
            RustType::Parenthesized(value) => format!("({})", self.render_type(value)),
            RustType::MacroInvocation(value) => value.clone(),
        }
    }

    fn render_tuple(&self, values: &[RustType]) -> String {
        if values.len() == 1 {
            return format!("({},)", self.render_type(&values[0]));
        }

        format!(
            "({})",
            values
                .iter()
                .map(|value| self.render_type(value))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    fn render_reference(
        &self,
        value: &crate::languages::rust::types::primitive_types::RustReferenceType,
    ) -> String {
        let lifetime = value
            .lifetime()
            .map(|lifetime| format!("{lifetime} "))
            .unwrap_or_default();
        let mutability = if value.mutable() { "mut " } else { "" };

        format!(
            "&{lifetime}{mutability}{}",
            self.render_type(value.referenced_type())
        )
    }

    fn render_raw_pointer(
        &self,
        value: &crate::languages::rust::types::primitive_types::RustRawPointerType,
    ) -> String {
        let mutability = if value.mutable() { "mut" } else { "const" };
        format!("*{mutability} {}", self.render_type(value.pointee_type()))
    }

    fn render_function_pointer(&self, value: &RustFunctionPointerType) -> String {
        let lifetimes = if value.lifetimes().is_empty() {
            String::new()
        } else {
            format!("for<{}> ", value.lifetimes().join(", "))
        };
        let unsafety = if value.is_unsafe() { "unsafe " } else { "" };
        let abi = self.render_abi_prefix(value.abi());
        let mut parameters = value
            .parameters()
            .iter()
            .map(|parameter| self.render_function_pointer_parameter(parameter))
            .collect::<Vec<_>>();

        if let Some(variadic) = value.variadic() {
            parameters.push(self.render_function_pointer_variadic(variadic));
        }

        let return_type = value
            .return_type()
            .map(|return_type| format!(" -> {}", self.render_type(return_type)))
            .unwrap_or_default();

        format!(
            "{lifetimes}{unsafety}{abi}fn({}){return_type}",
            parameters.join(", ")
        )
    }

    fn render_function_pointer_parameter(&self, value: &RustFunctionPointerParameter) -> String {
        let attributes = if value.attributes().is_empty() {
            String::new()
        } else {
            format!("{} ", value.attributes().join(" "))
        };

        format!("{attributes}{}", self.render_type(value.parameter_type()))
    }

    fn render_function_pointer_variadic(&self, value: &RustFunctionPointerVariadic) -> String {
        let attributes = if value.attributes().is_empty() {
            String::new()
        } else {
            format!("{} ", value.attributes().join(" "))
        };

        match value.name() {
            Some(name) => format!("{attributes}{name}: ..."),
            None => format!("{attributes}..."),
        }
    }

    fn render_abi_prefix(&self, value: &RustFunctionAbi) -> String {
        match value {
            RustFunctionAbi::Rust => String::new(),
            RustFunctionAbi::Extern { name } => match name {
                Some(name) => format!("extern \"{name}\" "),
                None => "extern ".to_owned(),
            },
        }
    }

    fn render_signed_integer(&self, value: SignedIntegerType) -> &'static str {
        match value {
            SignedIntegerType::I8 => "i8",
            SignedIntegerType::I16 => "i16",
            SignedIntegerType::I32 => "i32",
            SignedIntegerType::I64 => "i64",
            SignedIntegerType::I128 => "i128",
            SignedIntegerType::Isize => "isize",
        }
    }

    fn render_unsigned_integer(&self, value: UnsignedIntegerType) -> &'static str {
        match value {
            UnsignedIntegerType::U8 => "u8",
            UnsignedIntegerType::U16 => "u16",
            UnsignedIntegerType::U32 => "u32",
            UnsignedIntegerType::U64 => "u64",
            UnsignedIntegerType::U128 => "u128",
            UnsignedIntegerType::Usize => "usize",
        }
    }

    fn render_float(&self, value: FloatType) -> &'static str {
        match value {
            FloatType::F32 => "f32",
            FloatType::F64 => "f64",
        }
    }
}
