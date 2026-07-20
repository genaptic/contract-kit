use super::artifact::RustCompilerArtifactFailure;
use super::index::RustdocIndex;
use crate::error::SignatureContractKitError;
use crate::languages::rust::parser::signature_id::RustItemId;
use crate::languages::rust::parser::source_graph::RustModulePath;
use crate::languages::rust::parser::yaml::type_text::RustTypeTextRenderer;
use crate::languages::rust::types::attributes::RustAttributes;
use crate::languages::rust::types::callable_type::RustFunctionAbi;
use crate::languages::rust::types::declaration::RustItemKind;
use crate::languages::rust::types::primitive_types::{
    FloatType, RustArrayType, RustFunctionPointerParameter, RustFunctionPointerType,
    RustFunctionPointerVariadic, RustGenericMetadata, RustGenericParameter, RustImplTraitType,
    RustRawPointerType, RustReferenceType, RustTraitObjectType, RustType, RustTypePath,
    SignedIntegerType, UnsignedIntegerType,
};
use crate::languages::rust::types::syntax_text::RustSyntaxText;
use std::collections::BTreeMap;

pub(super) struct RustdocTypeLowerer<'index> {
    pub(super) index: &'index RustdocIndex,
}

#[derive(Default)]
struct RustdocAliasBindings {
    alias_id: Option<u32>,
    types: BTreeMap<String, RustType>,
    lifetimes: BTreeMap<String, String>,
    constants: BTreeMap<String, String>,
}

#[derive(Default)]
pub(super) struct RustdocTypeContext {
    bindings: Vec<RustdocAliasBindings>,
}

impl RustdocTypeLowerer<'_> {
    pub(super) fn function_abi(&self, abi: &rustdoc_types::Abi) -> RustFunctionAbi {
        let name = match abi {
            rustdoc_types::Abi::Rust => return RustFunctionAbi::Rust,
            rustdoc_types::Abi::C { unwind } => self.abi_name("C", *unwind),
            rustdoc_types::Abi::Cdecl { unwind } => self.abi_name("cdecl", *unwind),
            rustdoc_types::Abi::Stdcall { unwind } => self.abi_name("stdcall", *unwind),
            rustdoc_types::Abi::Fastcall { unwind } => self.abi_name("fastcall", *unwind),
            rustdoc_types::Abi::Aapcs { unwind } => self.abi_name("aapcs", *unwind),
            rustdoc_types::Abi::Win64 { unwind } => self.abi_name("win64", *unwind),
            rustdoc_types::Abi::SysV64 { unwind } => self.abi_name("sysv64", *unwind),
            rustdoc_types::Abi::System { unwind } => self.abi_name("system", *unwind),
            rustdoc_types::Abi::Other(name) => name.clone(),
        };
        RustFunctionAbi::Extern { name: Some(name) }
    }

    fn abi_name(&self, name: &str, unwind: bool) -> String {
        if unwind {
            format!("{name}-unwind")
        } else {
            name.to_owned()
        }
    }

    pub(super) fn convert_generics(
        &self,
        owner: rustdoc_types::Id,
        value: &rustdoc_types::Generics,
    ) -> Result<RustGenericMetadata, SignatureContractKitError> {
        let mut parameters = Vec::with_capacity(value.params.len());
        for parameter in &value.params {
            let converted = match &parameter.kind {
                rustdoc_types::GenericParamDefKind::Lifetime { outlives } => {
                    self.validate_lifetime(owner, &parameter.name)?;
                    for lifetime in outlives {
                        self.validate_lifetime(owner, lifetime)?;
                    }
                    RustGenericParameter::lifetime_parameter(
                        parameter.name.clone(),
                        outlives.clone(),
                    )
                }
                rustdoc_types::GenericParamDefKind::Type {
                    bounds,
                    default,
                    is_synthetic,
                } => {
                    if *is_synthetic {
                        return Err(RustCompilerArtifactFailure::unsupported_item(
                            owner.0,
                            "generic parameter",
                            format!(
                                "compiler-synthetic parameter {:?} cannot be projected as source-declared generic metadata",
                                parameter.name
                            ),
                        ));
                    }
                    self.validate_identifier(owner, &parameter.name, "generic type parameter")?;
                    let bounds = self
                        .convert_bounds(owner, bounds)?
                        .into_iter()
                        .map(|bound| bound.as_str().to_owned())
                        .collect();
                    let default = default
                        .as_ref()
                        .map(|default| {
                            self.convert_type(owner, default, &mut RustdocTypeContext::default())
                                .map(|value| RustTypeTextRenderer.render_type(&value))
                        })
                        .transpose()?;
                    RustGenericParameter::type_parameter(parameter.name.clone(), bounds, default)
                }
                rustdoc_types::GenericParamDefKind::Const { type_, default } => {
                    self.validate_identifier(owner, &parameter.name, "const generic parameter")?;
                    let parameter_type =
                        self.convert_type(owner, type_, &mut RustdocTypeContext::default())?;
                    let default = default
                        .as_deref()
                        .map(RustSyntaxText::parse_expression)
                        .transpose()?
                        .map(|value| value.as_str().to_owned());
                    RustGenericParameter::const_parameter(
                        parameter.name.clone(),
                        RustTypeTextRenderer.render_type(&parameter_type),
                        default,
                    )
                }
            };
            parameters.push(converted);
        }

        let where_predicates = value
            .where_predicates
            .iter()
            .map(|predicate| self.where_predicate(owner, predicate))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RustGenericMetadata::new(parameters).with_where_predicates(where_predicates))
    }

    fn where_predicate(
        &self,
        owner: rustdoc_types::Id,
        value: &rustdoc_types::WherePredicate,
    ) -> Result<RustSyntaxText, SignatureContractKitError> {
        let source = match value {
            rustdoc_types::WherePredicate::BoundPredicate {
                type_,
                bounds,
                generic_params,
            } => {
                if bounds.is_empty() {
                    return Err(RustCompilerArtifactFailure::unsupported_item(
                        owner.0,
                        "where predicate",
                        "bound predicate has no bounds",
                    ));
                }
                let mut context = RustdocTypeContext::default();
                context.enter_lifetime_binder(generic_params);
                let rendered = (|| {
                    let binder = self.generic_binder_source(owner, generic_params, &context)?;
                    let constrained = self.convert_type(owner, type_, &mut context)?;
                    Ok::<_, SignatureContractKitError>(format!(
                        "{binder}{}: {}",
                        RustTypeTextRenderer.render_type(&constrained),
                        self.convert_bounds_with_context(owner, bounds, &mut context)?
                            .iter()
                            .map(RustSyntaxText::as_str)
                            .collect::<Vec<_>>()
                            .join(" + ")
                    ))
                })();
                context.leave_lifetime_binder();
                rendered?
            }
            rustdoc_types::WherePredicate::LifetimePredicate { lifetime, outlives } => {
                self.validate_lifetime(owner, lifetime)?;
                if outlives.is_empty() {
                    return Err(RustCompilerArtifactFailure::unsupported_item(
                        owner.0,
                        "where predicate",
                        "lifetime predicate has no outlives bounds",
                    ));
                }
                for bound in outlives {
                    self.validate_lifetime(owner, bound)?;
                }
                format!("{lifetime}: {}", outlives.join(" + "))
            }
            rustdoc_types::WherePredicate::EqPredicate { .. } => {
                return Err(RustCompilerArtifactFailure::unsupported_item(
                    owner.0,
                    "where predicate",
                    "compiler equality predicates are not representable by stable Rust where-clause syntax",
                ));
            }
        };
        RustSyntaxText::parse_where_predicate(&source)
    }

    pub(super) fn convert_bounds(
        &self,
        owner: rustdoc_types::Id,
        values: &[rustdoc_types::GenericBound],
    ) -> Result<Vec<RustSyntaxText>, SignatureContractKitError> {
        self.convert_bounds_with_context(owner, values, &mut RustdocTypeContext::default())
    }

    fn convert_bounds_with_context(
        &self,
        owner: rustdoc_types::Id,
        values: &[rustdoc_types::GenericBound],
        context: &mut RustdocTypeContext,
    ) -> Result<Vec<RustSyntaxText>, SignatureContractKitError> {
        values
            .iter()
            .map(|bound| {
                let source = match bound {
                    rustdoc_types::GenericBound::TraitBound {
                        trait_,
                        generic_params,
                        modifier,
                    } => {
                        let modifier = match modifier {
                            rustdoc_types::TraitBoundModifier::None => "",
                            rustdoc_types::TraitBoundModifier::Maybe => "?",
                            rustdoc_types::TraitBoundModifier::MaybeConst => "~const ",
                        };
                        context.enter_lifetime_binder(generic_params);
                        let rendered = (|| {
                            let binder =
                                self.generic_binder_source(owner, generic_params, context)?;
                            let path =
                                self.resolved_path_source_with_context(owner, trait_, context)?;
                            Ok::<_, SignatureContractKitError>(format!("{binder}{modifier}{path}"))
                        })();
                        context.leave_lifetime_binder();
                        rendered?
                    }
                    rustdoc_types::GenericBound::Outlives(lifetime) => {
                        let lifetime = context
                            .lifetime_binding(lifetime)
                            .unwrap_or(lifetime)
                            .to_owned();
                        self.validate_lifetime(owner, &lifetime)?;
                        lifetime
                    }
                    rustdoc_types::GenericBound::Use(arguments) => {
                        let mut rendered = Vec::with_capacity(arguments.len());
                        for argument in arguments {
                            match argument {
                                rustdoc_types::PreciseCapturingArg::Lifetime(value) => {
                                    let value =
                                        context.lifetime_binding(value).unwrap_or(value).to_owned();
                                    self.validate_lifetime(owner, &value)?;
                                    rendered.push(value);
                                }
                                rustdoc_types::PreciseCapturingArg::Param(value) => {
                                    self.validate_identifier(
                                        owner,
                                        value,
                                        "precise capturing parameter",
                                    )?;
                                    rendered.push(RustModulePath::source_ident(value));
                                }
                            }
                        }
                        format!("use<{}>", rendered.join(", "))
                    }
                };
                self.canonical_bound(owner, source)
            })
            .collect()
    }

    fn canonical_bound(
        &self,
        owner: rustdoc_types::Id,
        source: String,
    ) -> Result<RustSyntaxText, SignatureContractKitError> {
        RustSyntaxText::parse_type_bound(&source).map_err(|error| {
            RustCompilerArtifactFailure::unsupported_item(
                owner.0,
                "generic bound",
                format!("compiler-canonical bound {source:?} is not valid Rust syntax: {error}"),
            )
        })
    }

    fn generic_binder_source(
        &self,
        owner: rustdoc_types::Id,
        parameters: &[rustdoc_types::GenericParamDef],
        context: &RustdocTypeContext,
    ) -> Result<String, SignatureContractKitError> {
        if parameters.is_empty() {
            return Ok(String::new());
        }
        let mut lifetimes = Vec::with_capacity(parameters.len());
        for parameter in parameters {
            let rustdoc_types::GenericParamDefKind::Lifetime { outlives } = &parameter.kind else {
                return Err(RustCompilerArtifactFailure::unsupported_item(
                    owner.0,
                    "higher-ranked binder",
                    "higher-ranked binders may contain only lifetime parameters",
                ));
            };
            self.validate_lifetime(owner, &parameter.name)?;
            let mut source = parameter.name.clone();
            if !outlives.is_empty() {
                let outlives = outlives
                    .iter()
                    .map(|lifetime| {
                        context
                            .lifetime_binding(lifetime)
                            .unwrap_or(lifetime)
                            .to_owned()
                    })
                    .collect::<Vec<_>>();
                for lifetime in &outlives {
                    self.validate_lifetime(owner, lifetime)?;
                }
                source.push_str(": ");
                source.push_str(&outlives.join(" + "));
            }
            lifetimes.push(source);
        }
        Ok(format!("for<{}> ", lifetimes.join(", ")))
    }

    fn validate_identifier(
        &self,
        owner: rustdoc_types::Id,
        value: &str,
        role: &'static str,
    ) -> Result<(), SignatureContractKitError> {
        syn::parse_str::<syn::Ident>(value)
            .map(|_| ())
            .map_err(|error| {
                RustCompilerArtifactFailure::unsupported_item(
                    owner.0,
                    role,
                    format!("invalid identifier {value:?}: {error}"),
                )
            })
    }

    fn validate_lifetime(
        &self,
        owner: rustdoc_types::Id,
        value: &str,
    ) -> Result<(), SignatureContractKitError> {
        syn::parse_str::<syn::Lifetime>(value)
            .map(|_| ())
            .map_err(|error| {
                RustCompilerArtifactFailure::unsupported_item(
                    owner.0,
                    "lifetime",
                    format!("invalid lifetime {value:?}: {error}"),
                )
            })
    }

    fn generic_args_source(
        &self,
        owner: rustdoc_types::Id,
        value: &rustdoc_types::GenericArgs,
        context: &mut RustdocTypeContext,
    ) -> Result<String, SignatureContractKitError> {
        match value {
            rustdoc_types::GenericArgs::AngleBracketed { args, constraints } => {
                let mut parts = Vec::with_capacity(args.len() + constraints.len());
                for argument in args {
                    parts.push(match argument {
                        rustdoc_types::GenericArg::Lifetime(lifetime) => {
                            let lifetime = context
                                .lifetime_binding(lifetime)
                                .unwrap_or(lifetime)
                                .to_owned();
                            self.validate_lifetime(owner, &lifetime)?;
                            lifetime
                        }
                        rustdoc_types::GenericArg::Type(value) => {
                            let value = self.convert_type(owner, value, context)?;
                            RustTypeTextRenderer.render_type(&value)
                        }
                        rustdoc_types::GenericArg::Const(value) => {
                            self.canonical_const_expression(owner, &value.expr, context)?
                        }
                        rustdoc_types::GenericArg::Infer => "_".to_owned(),
                    });
                }
                for constraint in constraints {
                    let name = RustModulePath::source_ident(&constraint.name);
                    let arguments = constraint
                        .args
                        .as_deref()
                        .map(|arguments| self.generic_args_source(owner, arguments, context))
                        .transpose()?
                        .unwrap_or_default();
                    let binding = match &constraint.binding {
                        rustdoc_types::AssocItemConstraintKind::Equality(term) => {
                            let value = match term {
                                rustdoc_types::Term::Type(value) => {
                                    let value = self.convert_type(owner, value, context)?;
                                    RustTypeTextRenderer.render_type(&value)
                                }
                                rustdoc_types::Term::Constant(value) => {
                                    self.canonical_const_expression(owner, &value.expr, context)?
                                }
                            };
                            format!(" = {value}")
                        }
                        rustdoc_types::AssocItemConstraintKind::Constraint(bounds) => {
                            format!(
                                ": {}",
                                self.convert_bounds_with_context(owner, bounds, context)?
                                    .iter()
                                    .map(RustSyntaxText::as_str)
                                    .collect::<Vec<_>>()
                                    .join(" + ")
                            )
                        }
                    };
                    parts.push(format!("{name}{arguments}{binding}"));
                }
                Ok(format!("<{}>", parts.join(", ")))
            }
            rustdoc_types::GenericArgs::Parenthesized { inputs, output } => {
                let inputs = inputs
                    .iter()
                    .map(|input| {
                        self.convert_type(owner, input, context)
                            .map(|value| RustTypeTextRenderer.render_type(&value))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let output = output
                    .as_ref()
                    .map(|output| {
                        self.convert_type(owner, output, context).map(|value| {
                            format!(" -> {}", RustTypeTextRenderer.render_type(&value))
                        })
                    })
                    .transpose()?
                    .unwrap_or_default();
                Ok(format!("({}){output}", inputs.join(", ")))
            }
            rustdoc_types::GenericArgs::ReturnTypeNotation => {
                Err(RustCompilerArtifactFailure::unsupported_type(
                    owner.0,
                    "return type notation",
                    "return-type notation is unstable and is not represented by rust_api_v1",
                ))
            }
        }
    }

    fn convert_function_pointer(
        &self,
        owner: rustdoc_types::Id,
        value: &rustdoc_types::FunctionPointer,
        context: &mut RustdocTypeContext,
    ) -> Result<RustType, SignatureContractKitError> {
        if value.header.is_const || value.header.is_async {
            return Err(RustCompilerArtifactFailure::unsupported_type(
                owner.0,
                "function pointer",
                "const or async function pointers are not valid stable Rust types",
            ));
        }
        let mut lifetimes = Vec::with_capacity(value.generic_params.len());
        for parameter in &value.generic_params {
            let rustdoc_types::GenericParamDefKind::Lifetime { outlives } = &parameter.kind else {
                return Err(RustCompilerArtifactFailure::unsupported_type(
                    owner.0,
                    "function pointer binder",
                    "higher-ranked function-pointer binders may contain only lifetimes",
                ));
            };
            if !outlives.is_empty() {
                return Err(RustCompilerArtifactFailure::unsupported_type(
                    owner.0,
                    "function pointer binder",
                    "bounded higher-ranked function-pointer lifetimes are not represented losslessly",
                ));
            }
            self.validate_lifetime(owner, &parameter.name)?;
            lifetimes.push(parameter.name.clone());
        }
        context.enter_lifetime_binder(&value.generic_params);
        let converted = (|| {
            let parameters = value
                .sig
                .inputs
                .iter()
                .map(|(_, parameter)| {
                    self.convert_type(owner, parameter, context)
                        .map(|parameter| {
                            RustFunctionPointerParameter::new(RustAttributes::default(), parameter)
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let return_type = value
                .sig
                .output
                .as_ref()
                .map(|output| self.convert_type(owner, output, context))
                .transpose()?;
            Ok::<_, SignatureContractKitError>((parameters, return_type))
        })();
        context.leave_lifetime_binder();
        let (parameters, return_type) = converted?;
        let variadic = value
            .sig
            .is_c_variadic
            .then(|| RustFunctionPointerVariadic::new(None, RustAttributes::default()));
        Ok(RustType::FunctionPointer(
            RustFunctionPointerType::from_parts(parameters, return_type, value.header.is_unsafe)
                .with_lifetimes(lifetimes)
                .with_abi(self.function_abi(&value.header.abi))
                .with_variadic(variadic),
        ))
    }

    fn resolved_path_source_with_context(
        &self,
        owner: rustdoc_types::Id,
        path: &rustdoc_types::Path,
        context: &mut RustdocTypeContext,
    ) -> Result<String, SignatureContractKitError> {
        let segments = self
            .index
            .document
            .paths
            .get(&path.id)
            .map(|summary| summary.path.clone())
            .unwrap_or_else(|| {
                path.path
                    .split("::")
                    .filter(|segment| !segment.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            });
        if segments.is_empty() {
            return Err(RustCompilerArtifactFailure::unsupported_type(
                owner.0,
                "resolved path",
                "resolved path has no canonical segments",
            ));
        }
        let base = RustTypePath::new(segments).source_syntax().to_owned();
        let arguments = path
            .args
            .as_deref()
            .map(|arguments| self.generic_args_source(owner, arguments, context))
            .transpose()?
            .unwrap_or_default();
        Ok(format!("{base}{arguments}"))
    }

    fn canonical_const_expression(
        &self,
        owner: rustdoc_types::Id,
        source: &str,
        context: &RustdocTypeContext,
    ) -> Result<String, SignatureContractKitError> {
        let tokens = source
            .parse::<proc_macro2::TokenStream>()
            .map_err(|error| {
                RustCompilerArtifactFailure::unsupported_type(
                    owner.0,
                    "const generic expression",
                    format!("invalid compiler expression {source:?}: {error}"),
                )
            })?;
        let substituted = context.substitute_const_tokens(tokens).map_err(|reason| {
            RustCompilerArtifactFailure::unsupported_type(
                owner.0,
                "const generic expression",
                reason,
            )
        })?;
        RustSyntaxText::parse_expression(&substituted.to_string())
            .map(|expression| expression.as_str().to_owned())
    }

    fn convert_alias_type(
        &self,
        owner: rustdoc_types::Id,
        path: &rustdoc_types::Path,
        alias: &rustdoc_types::TypeAlias,
        context: &mut RustdocTypeContext,
    ) -> Result<RustType, SignatureContractKitError> {
        if !context.enter_alias(path.id.0) {
            return Err(self.unsupported_resolved_path(
                owner,
                path,
                format!(
                    "recursive type alias cycle through rustdoc item {}",
                    path.id.0
                ),
            ));
        }
        let result = (|| {
            let arguments = match path.args.as_deref() {
                None => &[][..],
                Some(rustdoc_types::GenericArgs::AngleBracketed { args, constraints })
                    if constraints.is_empty() =>
                {
                    args.as_slice()
                }
                Some(rustdoc_types::GenericArgs::AngleBracketed { constraints, .. }) => {
                    return Err(self.unsupported_resolved_path(
                        owner,
                        path,
                        format!(
                            "type alias applications cannot be normalized with {} associated-item constraints",
                            constraints.len()
                        ),
                    ));
                }
                Some(rustdoc_types::GenericArgs::Parenthesized { .. }) => {
                    return Err(self.unsupported_resolved_path(
                        owner,
                        path,
                        "type alias application uses parenthesized generic arguments",
                    ));
                }
                Some(rustdoc_types::GenericArgs::ReturnTypeNotation) => {
                    return Err(self.unsupported_resolved_path(
                        owner,
                        path,
                        "type alias application uses unstable return-type notation",
                    ));
                }
            };
            if arguments.len() > alias.generics.params.len() {
                return Err(self.unsupported_resolved_path(
                    owner,
                    path,
                    format!(
                        "type alias expects at most {} generic arguments, found {}",
                        alias.generics.params.len(),
                        arguments.len()
                    ),
                ));
            }

            for (index, parameter) in alias.generics.params.iter().enumerate() {
                let argument = arguments.get(index);
                match (&parameter.kind, argument) {
                    (
                        rustdoc_types::GenericParamDefKind::Lifetime { .. },
                        Some(rustdoc_types::GenericArg::Lifetime(value)),
                    ) => {
                        let value = context.lifetime_binding(value).unwrap_or(value).to_owned();
                        self.validate_lifetime(owner, &value)?;
                        context.bind_lifetime(parameter.name.clone(), value);
                    }
                    (rustdoc_types::GenericParamDefKind::Lifetime { .. }, None) => {
                        return Err(self.unsupported_resolved_path(
                            owner,
                            path,
                            format!(
                                "missing lifetime argument for type alias parameter {}",
                                parameter.name
                            ),
                        ));
                    }
                    (
                        rustdoc_types::GenericParamDefKind::Type { .. },
                        Some(rustdoc_types::GenericArg::Type(value)),
                    ) => {
                        let value = self.convert_type(owner, value, context)?;
                        context.bind_type(parameter.name.clone(), value);
                    }
                    (
                        rustdoc_types::GenericParamDefKind::Type { .. },
                        Some(rustdoc_types::GenericArg::Infer),
                    ) => {
                        context.bind_type(parameter.name.clone(), RustType::Inferred);
                    }
                    (rustdoc_types::GenericParamDefKind::Type { default, .. }, None) => {
                        let default = default.as_ref().ok_or_else(|| {
                            self.unsupported_resolved_path(
                                owner,
                                path,
                                format!(
                                    "missing type argument for type alias parameter {}",
                                    parameter.name
                                ),
                            )
                        })?;
                        let value = self.convert_type(owner, default, context)?;
                        context.bind_type(parameter.name.clone(), value);
                    }
                    (
                        rustdoc_types::GenericParamDefKind::Const { .. },
                        Some(rustdoc_types::GenericArg::Const(value)),
                    ) => {
                        let value = self.canonical_const_expression(owner, &value.expr, context)?;
                        context.bind_constant(parameter.name.clone(), value);
                    }
                    (rustdoc_types::GenericParamDefKind::Const { default, .. }, None) => {
                        let default = default.as_deref().ok_or_else(|| {
                            self.unsupported_resolved_path(
                                owner,
                                path,
                                format!(
                                    "missing const argument for type alias parameter {}",
                                    parameter.name
                                ),
                            )
                        })?;
                        let value = self.canonical_const_expression(owner, default, context)?;
                        context.bind_constant(parameter.name.clone(), value);
                    }
                    (_, Some(argument)) => {
                        return Err(self.unsupported_resolved_path(
                            owner,
                            path,
                            format!(
                                "generic argument {argument:?} does not match type alias parameter {}",
                                parameter.name
                            ),
                        ));
                    }
                }
            }
            self.convert_type(owner, &alias.type_, context)
        })();
        context.leave_alias(path.id.0);
        result
    }

    pub(super) fn convert_type(
        &self,
        owner: rustdoc_types::Id,
        value: &rustdoc_types::Type,
        context: &mut RustdocTypeContext,
    ) -> Result<RustType, SignatureContractKitError> {
        match value {
            rustdoc_types::Type::Primitive(name) => self.primitive_type(owner, name),
            rustdoc_types::Type::Generic(name) if name == "Self" => Ok(RustType::SelfType),
            rustdoc_types::Type::Generic(name) => Ok(context
                .type_binding(name)
                .cloned()
                .unwrap_or_else(|| RustType::GenericParameter(name.clone()))),
            rustdoc_types::Type::ResolvedPath(path) => {
                if let Some(item) = self.index.document.index.get(&path.id)
                    && let rustdoc_types::ItemEnum::TypeAlias(alias) = &item.inner
                {
                    return self.convert_alias_type(owner, path, alias, context);
                }
                let segments = self
                    .index
                    .document
                    .paths
                    .get(&path.id)
                    .map(|summary| summary.path.clone())
                    .unwrap_or_else(|| {
                        path.path
                            .split("::")
                            .filter(|segment| !segment.is_empty())
                            .map(ToOwned::to_owned)
                            .collect()
                    });
                if matches!(
                    segments.as_slice(),
                    [crate_name, string, name]
                        if matches!(crate_name.as_str(), "alloc" | "std")
                            && string == "string"
                            && name == "String"
                ) && path.args.is_none()
                {
                    return Ok(RustType::String);
                }
                if segments.is_empty() {
                    return Err(self.unsupported_type(
                        owner,
                        value,
                        "resolved path has no canonical segments",
                    ));
                }
                let base = RustTypePath::new(segments).source_syntax().to_owned();
                let arguments = path
                    .args
                    .as_deref()
                    .map(|arguments| self.generic_args_source(owner, arguments, context))
                    .transpose()?
                    .unwrap_or_default();
                let source = format!("{base}{arguments}");
                let path = syn::parse_str::<syn::TypePath>(&source).map_err(|error| {
                    RustCompilerArtifactFailure::unsupported_type(
                        owner.0,
                        "resolved path",
                        format!(
                            "compiler-canonical path {source:?} is not valid Rust type syntax: {error}"
                        ),
                    )
                })?;
                Ok(RustType::TypePath(RustTypePath::from_syn(&path)))
            }
            rustdoc_types::Type::Tuple(elements) => elements
                .iter()
                .map(|element| self.convert_type(owner, element, context))
                .collect::<Result<Vec<_>, _>>()
                .map(|elements| {
                    if elements.is_empty() {
                        RustType::Unit
                    } else {
                        RustType::Tuple(elements)
                    }
                }),
            rustdoc_types::Type::Slice(element) => self
                .convert_type(owner, element, context)
                .map(|element| RustType::Slice(Box::new(element))),
            rustdoc_types::Type::Array { type_, len } => {
                let element = self.convert_type(owner, type_, context)?;
                let length = self.canonical_const_expression(owner, len, context)?;
                Ok(RustType::Array(RustArrayType::new(
                    element,
                    length,
                )))
            }
            rustdoc_types::Type::BorrowedRef {
                lifetime,
                is_mutable,
                type_,
            } => {
                let referenced = self.convert_type(owner, type_, context)?;
                let lifetime = lifetime
                    .as_ref()
                    .map(|lifetime| {
                        context
                            .lifetime_binding(lifetime)
                            .unwrap_or(lifetime)
                            .to_owned()
                    });
                if let Some(lifetime) = &lifetime {
                    self.validate_lifetime(owner, lifetime)?;
                }
                Ok(RustType::Reference(RustReferenceType::new(
                    lifetime,
                    *is_mutable,
                    referenced,
                )))
            }
            rustdoc_types::Type::RawPointer { is_mutable, type_ } => self
                .convert_type(owner, type_, context)
                .map(|pointee| {
                    RustType::RawPointer(RustRawPointerType::new(*is_mutable, pointee))
                }),
            rustdoc_types::Type::FunctionPointer(function) => {
                self.convert_function_pointer(owner, function, context)
            }
            rustdoc_types::Type::DynTrait(value) => {
                let mut bounds = Vec::with_capacity(
                    value.traits.len() + usize::from(value.lifetime.is_some()),
                );
                for poly_trait in &value.traits {
                    context.enter_lifetime_binder(&poly_trait.generic_params);
                    let rendered = (|| {
                        let binder = self.generic_binder_source(
                            owner,
                            &poly_trait.generic_params,
                            context,
                        )?;
                        let trait_path = self.resolved_path_source_with_context(
                            owner,
                            &poly_trait.trait_,
                            context,
                        )?;
                        Ok::<_, SignatureContractKitError>(
                            format!("{binder}{trait_path}"),
                        )
                    })();
                    context.leave_lifetime_binder();
                    bounds.push(
                        self.canonical_bound(owner, rendered?)?
                            .as_str()
                            .to_owned(),
                    );
                }
                if let Some(lifetime) = &value.lifetime {
                    let lifetime = context
                        .lifetime_binding(lifetime)
                        .unwrap_or(lifetime)
                        .to_owned();
                    bounds.push(
                        self.canonical_bound(owner, lifetime)?
                            .as_str()
                            .to_owned(),
                    );
                }
                Ok(RustType::TraitObject(RustTraitObjectType::new(bounds)))
            }
            rustdoc_types::Type::ImplTrait(bounds) => self
                .convert_bounds_with_context(owner, bounds, context)
                .map(|bounds| {
                    RustType::ImplTrait(RustImplTraitType::new(
                        bounds
                            .into_iter()
                            .map(|bound| bound.as_str().to_owned())
                            .collect(),
                    ))
                }),
            rustdoc_types::Type::QualifiedPath {
                name,
                args,
                self_type,
                trait_,
            } => {
                let self_type = self.convert_type(owner, self_type, context)?;
                let rendered_self = RustTypeTextRenderer.render_type(&self_type);
                let qualifier = trait_
                    .as_ref()
                    .map(|path| {
                        self.resolved_path_source_with_context(owner, path, context)
                    })
                    .transpose()?;
                let base = match qualifier {
                    Some(trait_path) => format!("<{rendered_self} as {trait_path}>::{name}"),
                    None => format!("{rendered_self}::{name}"),
                };
                let arguments = args
                    .as_deref()
                    .map(|arguments| self.generic_args_source(owner, arguments, context))
                    .transpose()?
                    .unwrap_or_default();
                let source = format!("{base}{arguments}");
                let path = syn::parse_str::<syn::TypePath>(&source).map_err(|error| {
                    RustCompilerArtifactFailure::unsupported_type(
                        owner.0,
                        "qualified path",
                        format!(
                            "compiler-canonical path {source:?} is not valid Rust type syntax: {error}"
                        ),
                    )
                })?;
                Ok(RustType::TypePath(RustTypePath::from_syn(&path)))
            }
            rustdoc_types::Type::Infer => Ok(RustType::Inferred),
            rustdoc_types::Type::Pat { .. } => Err(self.unsupported_type(
                owner,
                value,
                "unstable pattern types do not expose a supported canonical semantic representation",
            )),
        }
    }

    fn primitive_type(
        &self,
        owner: rustdoc_types::Id,
        name: &str,
    ) -> Result<RustType, SignatureContractKitError> {
        let value = match name {
            "bool" => RustType::Bool,
            "char" => RustType::Char,
            "str" => RustType::Str,
            "!" | "never" => RustType::Never,
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
            _ => {
                return Err(RustCompilerArtifactFailure::unsupported_type(
                    owner.0,
                    format!("primitive {name:?}"),
                    "unknown primitive name for the pinned rustdoc schema",
                ));
            }
        };
        Ok(value)
    }

    pub(super) fn owner_id(
        &self,
        implementation: rustdoc_types::Id,
        owner: &rustdoc_types::Type,
    ) -> Result<RustItemId, SignatureContractKitError> {
        let rustdoc_types::Type::ResolvedPath(path) = owner else {
            return Err(self.unsupported_type(
                implementation,
                owner,
                "implementation owner is not an ordinary resolved path",
            ));
        };
        let item = self.index.item(path.id)?;
        if item.crate_id != 0 {
            return Err(self.index.unsupported_type(
                implementation,
                owner,
                "implementations for external owner types are not yet grouped losslessly",
            ));
        }
        let kind = match &item.inner {
            rustdoc_types::ItemEnum::Struct(_) => RustItemKind::Struct,
            rustdoc_types::ItemEnum::Enum(_) => RustItemKind::Enum,
            rustdoc_types::ItemEnum::Union(_) => RustItemKind::Union,
            rustdoc_types::ItemEnum::TypeAlias(_) => RustItemKind::TypeAlias,
            _ => {
                return Err(RustCompilerArtifactFailure::invalid_item(
                    path.id.0,
                    "implementation owner is not a modeled nominal type",
                ));
            }
        };
        let summary = self.index.document.paths.get(&path.id).ok_or_else(|| {
            RustCompilerArtifactFailure::invalid_item(
                path.id.0,
                "implementation owner has no canonical path summary",
            )
        })?;
        let name = summary.path.last().cloned().ok_or_else(|| {
            RustCompilerArtifactFailure::invalid_item(
                path.id.0,
                "implementation owner path is empty",
            )
        })?;
        Ok(RustItemId::new(
            self.index.module_id_for_summary(summary)?,
            kind,
            name,
        ))
    }

    pub(super) fn type_source(
        &self,
        owner: rustdoc_types::Id,
        value: &rustdoc_types::Type,
    ) -> Result<String, SignatureContractKitError> {
        let converted = self.convert_type(owner, value, &mut RustdocTypeContext::default())?;
        let source = RustTypeTextRenderer.render_type(&converted);
        if matches!(syn::parse_str::<syn::Type>(&source), Ok(syn::Type::Path(_))) {
            Ok(source)
        } else {
            Err(self.index.unsupported_type(
                owner,
                value,
                "type cannot be rendered as an ordinary owner path",
            ))
        }
    }

    pub(super) fn resolved_path_source(
        &self,
        owner: rustdoc_types::Id,
        path: &rustdoc_types::Path,
    ) -> Result<String, SignatureContractKitError> {
        self.resolved_path_source_with_context(owner, path, &mut RustdocTypeContext::default())
    }

    fn unsupported_type(
        &self,
        owner: rustdoc_types::Id,
        value: &rustdoc_types::Type,
        reason: impl Into<String>,
    ) -> SignatureContractKitError {
        self.index.unsupported_type(owner, value, reason)
    }

    fn unsupported_resolved_path(
        &self,
        owner: rustdoc_types::Id,
        path: &rustdoc_types::Path,
        reason: impl Into<String>,
    ) -> SignatureContractKitError {
        RustCompilerArtifactFailure::unsupported_type(
            owner.0,
            format!("ResolvedPath({path:?})"),
            reason,
        )
    }
}

impl RustdocTypeContext {
    fn type_binding(&self, name: &str) -> Option<&RustType> {
        self.bindings
            .iter()
            .rev()
            .find_map(|bindings| bindings.types.get(name))
    }

    fn lifetime_binding(&self, name: &str) -> Option<&str> {
        self.bindings
            .iter()
            .rev()
            .find_map(|bindings| bindings.lifetimes.get(name).map(String::as_str))
    }

    fn constant_binding(&self, name: &str) -> Option<&str> {
        self.bindings
            .iter()
            .rev()
            .find_map(|bindings| bindings.constants.get(name).map(String::as_str))
    }

    fn enter_alias(&mut self, alias_id: u32) -> bool {
        if self
            .bindings
            .iter()
            .any(|bindings| bindings.alias_id == Some(alias_id))
        {
            return false;
        }
        self.bindings.push(RustdocAliasBindings {
            alias_id: Some(alias_id),
            ..RustdocAliasBindings::default()
        });
        true
    }

    fn leave_alias(&mut self, alias_id: u32) {
        let removed = self.bindings.pop().and_then(|bindings| bindings.alias_id);
        debug_assert_eq!(removed, Some(alias_id));
    }

    fn bind_type(&mut self, name: String, value: RustType) {
        if let Some(bindings) = self.bindings.last_mut() {
            bindings.types.insert(name, value);
        }
    }

    fn bind_lifetime(&mut self, name: String, value: String) {
        if let Some(bindings) = self.bindings.last_mut() {
            bindings.lifetimes.insert(name, value);
        }
    }

    fn bind_constant(&mut self, name: String, value: String) {
        if let Some(bindings) = self.bindings.last_mut() {
            bindings.constants.insert(name, value);
        }
    }

    fn enter_lifetime_binder(&mut self, parameters: &[rustdoc_types::GenericParamDef]) {
        let mut bindings = RustdocAliasBindings::default();
        for parameter in parameters {
            if matches!(
                parameter.kind,
                rustdoc_types::GenericParamDefKind::Lifetime { .. }
            ) {
                bindings
                    .lifetimes
                    .insert(parameter.name.clone(), parameter.name.clone());
            }
        }
        self.bindings.push(bindings);
    }

    fn leave_lifetime_binder(&mut self) {
        let removed = self.bindings.pop().map(|bindings| bindings.alias_id);
        debug_assert_eq!(removed, Some(None));
    }

    fn substitute_const_tokens(
        &self,
        tokens: proc_macro2::TokenStream,
    ) -> Result<proc_macro2::TokenStream, String> {
        let source = tokens.into_iter().collect::<Vec<_>>();
        let mut substituted = proc_macro2::TokenStream::new();
        for (index, token) in source.iter().enumerate() {
            let replacement = match token {
                proc_macro2::TokenTree::Ident(identifier)
                    if !Self::is_qualified_identifier(&source, index) =>
                {
                    self.constant_binding(&identifier.to_string())
                }
                _ => None,
            };
            if let Some(replacement) = replacement {
                let replacement = replacement
                    .parse::<proc_macro2::TokenStream>()
                    .map_err(|error| error.to_string())?;
                substituted.extend(replacement);
                continue;
            }
            match token {
                proc_macro2::TokenTree::Group(group) => {
                    let stream = self.substitute_const_tokens(group.stream())?;
                    let mut replacement = proc_macro2::Group::new(group.delimiter(), stream);
                    replacement.set_span(group.span());
                    substituted.extend([proc_macro2::TokenTree::Group(replacement)]);
                }
                other => substituted.extend([other.clone()]),
            }
        }
        Ok(substituted)
    }

    fn is_qualified_identifier(tokens: &[proc_macro2::TokenTree], index: usize) -> bool {
        let adjacent_is_path_separator = |token: Option<&proc_macro2::TokenTree>| {
            matches!(
                token,
                Some(proc_macro2::TokenTree::Punct(punctuation))
                    if matches!(punctuation.as_char(), ':' | '.')
            )
        };
        adjacent_is_path_separator(index.checked_sub(1).and_then(|index| tokens.get(index)))
            || adjacent_is_path_separator(tokens.get(index + 1))
    }
}
