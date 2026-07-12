use crate::languages::rust::types::callable_type::RustFunctionAbi;
use serde::Serialize;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) enum Visibility {
    Public,
    PublicCrate,
    Restricted(String),
    #[default]
    Private,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum RustType {
    Bool,
    Char,
    Str,
    String,
    Never,
    SignedInteger(SignedIntegerType),
    UnsignedInteger(UnsignedIntegerType),
    Float(FloatType),
    Unit,
    Tuple(Vec<RustType>),
    Array(RustArrayType),
    Slice(Box<RustType>),
    Reference(RustReferenceType),
    RawPointer(RustRawPointerType),
    FunctionPointer(RustFunctionPointerType),
    TraitObject(RustTraitObjectType),
    ImplTrait(RustImplTraitType),
    TypePath(RustTypePath),
    GenericParameter(String),
    SelfType,
    Inferred,
    Parenthesized(Box<RustType>),
    MacroInvocation(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum SignedIntegerType {
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum UnsignedIntegerType {
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum FloatType {
    F32,
    F64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustArrayType {
    element_type: Box<RustType>,
    length: String,
}

impl RustArrayType {
    pub(crate) fn new(element_type: RustType, length: String) -> Self {
        Self {
            element_type: Box::new(element_type),
            length,
        }
    }

    pub(crate) fn element_type(&self) -> &RustType {
        &self.element_type
    }

    pub(crate) fn length(&self) -> &str {
        &self.length
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustReferenceType {
    lifetime: Option<String>,
    mutable: bool,
    referenced_type: Box<RustType>,
}

impl RustReferenceType {
    pub(crate) fn new(lifetime: Option<String>, mutable: bool, referenced_type: RustType) -> Self {
        Self {
            lifetime,
            mutable,
            referenced_type: Box::new(referenced_type),
        }
    }

    pub(crate) fn lifetime(&self) -> Option<&str> {
        self.lifetime.as_deref()
    }

    pub(crate) fn mutable(&self) -> bool {
        self.mutable
    }

    pub(crate) fn referenced_type(&self) -> &RustType {
        &self.referenced_type
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustRawPointerType {
    mutable: bool,
    pointee_type: Box<RustType>,
}

impl RustRawPointerType {
    pub(crate) fn new(mutable: bool, pointee_type: RustType) -> Self {
        Self {
            mutable,
            pointee_type: Box::new(pointee_type),
        }
    }

    pub(crate) fn mutable(&self) -> bool {
        self.mutable
    }

    pub(crate) fn pointee_type(&self) -> &RustType {
        &self.pointee_type
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustFunctionPointerType {
    lifetimes: Vec<String>,
    is_unsafe: bool,
    abi: RustFunctionAbi,
    parameters: Vec<RustFunctionPointerParameter>,
    variadic: Option<RustFunctionPointerVariadic>,
    return_type: Option<Box<RustType>>,
}

impl RustFunctionPointerType {
    pub(crate) fn from_parts(
        parameters: Vec<RustFunctionPointerParameter>,
        return_type: Option<RustType>,
        is_unsafe: bool,
    ) -> Self {
        Self {
            lifetimes: Vec::new(),
            is_unsafe,
            abi: RustFunctionAbi::Rust,
            parameters,
            variadic: None,
            return_type: return_type.map(Box::new),
        }
    }

    pub(crate) fn with_lifetimes(mut self, lifetimes: Vec<String>) -> Self {
        self.lifetimes = lifetimes;
        self
    }

    pub(crate) fn with_abi(mut self, abi: RustFunctionAbi) -> Self {
        self.abi = abi;
        self
    }

    pub(crate) fn with_variadic(mut self, variadic: Option<RustFunctionPointerVariadic>) -> Self {
        self.variadic = variadic;
        self
    }

    pub(crate) fn lifetimes(&self) -> &[String] {
        &self.lifetimes
    }

    pub(crate) fn is_unsafe(&self) -> bool {
        self.is_unsafe
    }

    pub(crate) fn abi(&self) -> &RustFunctionAbi {
        &self.abi
    }

    pub(crate) fn parameters(&self) -> &[RustFunctionPointerParameter] {
        &self.parameters
    }

    pub(crate) fn variadic(&self) -> Option<&RustFunctionPointerVariadic> {
        self.variadic.as_ref()
    }

    pub(crate) fn return_type(&self) -> Option<&RustType> {
        self.return_type.as_deref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustFunctionPointerParameter {
    attributes: Vec<String>,
    parameter_type: RustType,
}

impl RustFunctionPointerParameter {
    pub(crate) fn new(attributes: Vec<String>, parameter_type: RustType) -> Self {
        Self {
            attributes,
            parameter_type,
        }
    }

    pub(crate) fn attributes(&self) -> &[String] {
        &self.attributes
    }

    pub(crate) fn parameter_type(&self) -> &RustType {
        &self.parameter_type
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustFunctionPointerVariadic {
    name: Option<String>,
    attributes: Vec<String>,
}

impl RustFunctionPointerVariadic {
    pub(crate) fn new(name: Option<String>, attributes: Vec<String>) -> Self {
        Self { name, attributes }
    }

    pub(crate) fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub(crate) fn attributes(&self) -> &[String] {
        &self.attributes
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustTraitObjectType {
    bounds: Vec<String>,
}

impl RustTraitObjectType {
    pub(crate) fn new(bounds: Vec<String>) -> Self {
        Self { bounds }
    }

    pub(crate) fn bounds(&self) -> &[String] {
        &self.bounds
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustImplTraitType {
    bounds: Vec<String>,
}

impl RustImplTraitType {
    pub(crate) fn new(bounds: Vec<String>) -> Self {
        Self { bounds }
    }

    pub(crate) fn bounds(&self) -> &[String] {
        &self.bounds
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustTypePath {
    segments: Vec<String>,
}

impl RustTypePath {
    pub(crate) fn new(segments: Vec<String>) -> Self {
        Self { segments }
    }

    pub(crate) fn segments(&self) -> &[String] {
        &self.segments
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustFunctionParameter {
    name: Option<String>,
    parameter_type: RustType,
}

impl RustFunctionParameter {
    pub(crate) fn new(name: Option<String>, parameter_type: RustType) -> Self {
        Self {
            name,
            parameter_type,
        }
    }

    pub(crate) fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub(crate) fn parameter_type(&self) -> &RustType {
        &self.parameter_type
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustGenericMetadata {
    parameters: Vec<RustGenericParameter>,
    where_predicates: Vec<String>,
}

impl RustGenericMetadata {
    pub(crate) fn new(parameters: Vec<RustGenericParameter>) -> Self {
        Self {
            parameters,
            where_predicates: Vec::new(),
        }
    }

    pub(crate) fn with_where_predicates(mut self, where_predicates: Vec<String>) -> Self {
        self.where_predicates = where_predicates;
        self
    }

    pub(crate) fn parameters(&self) -> &[RustGenericParameter] {
        &self.parameters
    }

    pub(crate) fn where_predicates(&self) -> &[String] {
        &self.where_predicates
    }
}

impl From<Vec<RustGenericParameter>> for RustGenericMetadata {
    fn from(parameters: Vec<RustGenericParameter>) -> Self {
        Self::new(parameters)
    }
}

impl Default for RustGenericMetadata {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum RustGenericParameter {
    Type {
        name: String,
        bounds: Vec<String>,
        default: Option<String>,
    },
    Lifetime {
        name: String,
        bounds: Vec<String>,
    },
    Const {
        name: String,
        parameter_type: String,
        default: Option<String>,
    },
}

impl RustGenericParameter {
    pub(crate) fn type_parameter(
        name: String,
        bounds: Vec<String>,
        default: Option<String>,
    ) -> Self {
        Self::Type {
            name,
            bounds,
            default,
        }
    }

    pub(crate) fn lifetime_parameter(name: String, bounds: Vec<String>) -> Self {
        Self::Lifetime { name, bounds }
    }

    pub(crate) fn const_parameter(
        name: String,
        parameter_type: String,
        default: Option<String>,
    ) -> Self {
        Self::Const {
            name,
            parameter_type,
            default,
        }
    }
}
