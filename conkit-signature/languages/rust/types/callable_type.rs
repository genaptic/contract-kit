use crate::files::CatalogPath;
use crate::languages::rust::types::function_type::FunctionType;
use crate::languages::rust::types::primitive_types::{
    RustFunctionParameter, RustGenericMetadata, RustType, Visibility,
};
use serde::Serialize;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) enum RustFunctionAbi {
    #[default]
    Rust,
    Extern {
        name: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustCallableSignature {
    is_const: bool,
    is_async: bool,
    is_unsafe: bool,
    abi: RustFunctionAbi,
    variadic: Option<RustVariadicParameter>,
    generics: RustGenericMetadata,
    parameters: Vec<RustFunctionParameter>,
    return_type: Option<RustType>,
}

impl RustCallableSignature {
    pub(crate) fn builder() -> RustCallableSignatureBuilder {
        RustCallableSignatureBuilder::default()
    }

    pub(crate) fn empty() -> Self {
        Self::builder().build()
    }

    pub(crate) fn is_const(&self) -> bool {
        self.is_const
    }

    pub(crate) fn is_async(&self) -> bool {
        self.is_async
    }

    pub(crate) fn is_unsafe(&self) -> bool {
        self.is_unsafe
    }

    pub(crate) fn abi(&self) -> &RustFunctionAbi {
        &self.abi
    }

    pub(crate) fn variadic(&self) -> Option<&RustVariadicParameter> {
        self.variadic.as_ref()
    }

    pub(crate) fn generics(&self) -> &RustGenericMetadata {
        &self.generics
    }

    pub(crate) fn parameters(&self) -> &[RustFunctionParameter] {
        &self.parameters
    }

    pub(crate) fn return_type(&self) -> Option<&RustType> {
        self.return_type.as_ref()
    }
}

impl Default for RustCallableSignature {
    fn default() -> Self {
        Self::empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RustCallableSignatureBuilder {
    is_const: bool,
    is_async: bool,
    is_unsafe: bool,
    abi: RustFunctionAbi,
    variadic: Option<RustVariadicParameter>,
    generics: RustGenericMetadata,
    parameters: Vec<RustFunctionParameter>,
    return_type: Option<RustType>,
}

impl RustCallableSignatureBuilder {
    pub(crate) fn with_const(mut self, is_const: bool) -> Self {
        self.is_const = is_const;
        self
    }

    pub(crate) fn with_async(mut self, is_async: bool) -> Self {
        self.is_async = is_async;
        self
    }

    pub(crate) fn with_unsafe(mut self, is_unsafe: bool) -> Self {
        self.is_unsafe = is_unsafe;
        self
    }

    pub(crate) fn with_abi(mut self, abi: RustFunctionAbi) -> Self {
        self.abi = abi;
        self
    }

    pub(crate) fn with_variadic(mut self, variadic: Option<RustVariadicParameter>) -> Self {
        self.variadic = variadic;
        self
    }

    pub(crate) fn with_generics(mut self, generics: RustGenericMetadata) -> Self {
        self.generics = generics;
        self
    }

    pub(crate) fn with_parameters(mut self, parameters: Vec<RustFunctionParameter>) -> Self {
        self.parameters = parameters;
        self
    }

    pub(crate) fn with_return_type(mut self, return_type: Option<RustType>) -> Self {
        self.return_type = return_type;
        self
    }

    pub(crate) fn build(self) -> RustCallableSignature {
        RustCallableSignature {
            is_const: self.is_const,
            is_async: self.is_async,
            is_unsafe: self.is_unsafe,
            abi: self.abi,
            variadic: self.variadic,
            generics: self.generics,
            parameters: self.parameters,
            return_type: self.return_type,
        }
    }
}

impl Default for RustCallableSignatureBuilder {
    fn default() -> Self {
        Self {
            is_const: false,
            is_async: false,
            is_unsafe: false,
            abi: RustFunctionAbi::Rust,
            variadic: None,
            generics: RustGenericMetadata::default(),
            parameters: Vec::new(),
            return_type: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustVariadicParameter {
    pattern: Option<String>,
    attributes: Vec<String>,
}

impl RustVariadicParameter {
    pub(crate) fn new(pattern: Option<String>, attributes: Vec<String>) -> Self {
        Self {
            pattern,
            attributes,
        }
    }

    pub(crate) fn pattern(&self) -> Option<&str> {
        self.pattern.as_deref()
    }

    pub(crate) fn attributes(&self) -> &[String] {
        &self.attributes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RustMethod {
    function: FunctionType,
    receiver: Option<String>,
    visibility: Visibility,
}

impl RustMethod {
    pub(crate) fn new(
        function: FunctionType,
        receiver: Option<String>,
        visibility: Visibility,
    ) -> Self {
        Self {
            function,
            receiver,
            visibility,
        }
    }

    pub(crate) fn function(&self) -> &FunctionType {
        &self.function
    }

    pub(crate) fn receiver(&self) -> Option<&str> {
        self.receiver.as_deref()
    }

    pub(crate) fn visibility(&self) -> &Visibility {
        &self.visibility
    }

    pub(crate) fn into_owner_context(
        mut self,
        file: CatalogPath,
        module_path: Vec<String>,
        visibility: Visibility,
    ) -> Self {
        self.function = self
            .function
            .into_method_context(file, module_path, visibility.clone());
        self.visibility = visibility;
        self
    }

    pub(super) fn canonical_form(&self) -> RustMethodCanonical {
        RustMethodCanonical {
            function: self.function.canonical_form(),
            receiver: self.receiver.clone(),
            visibility: self.visibility.clone(),
        }
    }
}

#[derive(Serialize)]
pub(super) struct RustMethodCanonical {
    function: crate::languages::rust::types::function_type::FunctionCanonical,
    receiver: Option<String>,
    visibility: Visibility,
}
