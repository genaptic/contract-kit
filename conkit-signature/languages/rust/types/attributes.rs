use crate::error::SignatureContractKitError;
use crate::languages::rust::parser::source_graph::RustModulePath;
use crate::languages::rust::types::syntax_text::RustSyntaxText;
use crate::work::CancellationProbe;
use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};
use serde::Serialize;
use syn::parse::Parser as _;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) struct RustAttributes {
    values: Vec<RustAttribute>,
}

impl RustAttributes {
    pub(crate) fn new(
        values: Vec<RustAttribute>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let attributes = Self { values };
        attributes.validate(cancellation)?;
        Ok(attributes)
    }

    pub(crate) fn from_syn(
        attributes: &[syn::Attribute],
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let mut converted = Self::default();

        for attribute in attributes {
            cancellation.checkpoint()?;
            converted.append_from_syn(attribute, cancellation)?;
        }

        Ok(converted)
    }

    pub(crate) fn values(&self) -> &[RustAttribute] {
        &self.values
    }

    pub(crate) fn requires_capability_warning(&self) -> bool {
        self.values
            .iter()
            .any(RustAttribute::requires_capability_warning)
    }

    pub(crate) fn primary_capability(&self) -> Option<RustAttributeCapability> {
        self.values.iter().find_map(RustAttribute::capability)
    }

    pub(crate) fn source_syntax(&self) -> String {
        let mut rendered = String::new();
        for (index, value) in self.values.iter().enumerate() {
            if index > 0 {
                rendered.push(' ');
            }
            rendered.push_str("#[");
            rendered.push_str(&value.meta_syntax());
            rendered.push(']');
        }
        rendered
    }

    fn append_derive(
        &mut self,
        attribute: &syn::Attribute,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        cancellation.checkpoint()?;
        let paths = attribute
            .parse_args_with(
                syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
            )
            .map_err(|error| {
                SignatureContractKitError::conversion_failed(format!(
                    "invalid derive attribute: {error}"
                ))
            })?;
        let mut converted_paths = Vec::with_capacity(paths.len());
        for path in paths {
            cancellation.checkpoint()?;
            converted_paths.push(RustPath::from_syn(path)?);
        }

        if converted_paths.is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "derive attribute requires at least one path",
            ));
        }

        self.values.push(RustAttribute::Derive(converted_paths));
        Ok(())
    }

    pub(crate) fn append_from_syn(
        &mut self,
        attribute: &syn::Attribute,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        cancellation.checkpoint()?;
        let mut converted = Self::default();
        converted.append_attribute(attribute, cancellation)?;
        converted.validate(cancellation)?;
        self.values.extend(converted.values);
        Ok(())
    }

    pub(in crate::languages::rust) fn from_meta(
        meta: syn::Meta,
        cancellation: &CancellationProbe,
    ) -> Result<Option<RustAttribute>, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let attribute = RustAttributeSyntax::attribute_from_meta(meta);
        let mut converted = Self::default();
        converted.append_attribute(&attribute, cancellation)?;
        converted.validate(cancellation)?;

        let mut values = converted.values.into_iter();
        let value = values.next();
        if values.next().is_some() {
            return Err(SignatureContractKitError::conversion_failed(
                "one Rust attribute produced multiple semantic attributes",
            ));
        }

        Ok(value)
    }

    pub(in crate::languages::rust) fn from_meta_sources(
        sources: Vec<String>,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let mut values = Vec::with_capacity(sources.len());
        for source in sources {
            cancellation.checkpoint()?;
            let meta = syn::parse_str::<syn::Meta>(&source).map_err(|error| {
                SignatureContractKitError::conversion_failed(format!(
                    "invalid compiler-provided Rust attribute {source:?}: {error}"
                ))
            })?;
            if let Some(value) = Self::from_meta(meta, cancellation)? {
                values.push(value);
            }
        }
        Self::new(values, cancellation)
    }

    fn append_attribute(
        &mut self,
        attribute: &syn::Attribute,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        if attribute.path().is_ident("derive") {
            return self.append_derive(attribute, cancellation);
        }

        if let Some(value) = RustAttribute::from_syn(attribute, cancellation)? {
            self.values.push(value);
        }

        Ok(())
    }

    fn validate(&self, cancellation: &CancellationProbe) -> Result<(), SignatureContractKitError> {
        for value in &self.values {
            cancellation.checkpoint()?;
            value.validate(cancellation)?;
        }

        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum RustAttributeCapability {
    DeriveExpansion,
    ConditionalCompilation,
    UnresolvedAttribute,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum RustAttribute {
    Derive(Vec<RustPath>),
    Repr(RustRepr),
    NonExhaustive,
    Conditional(RustConditional),
    Deprecated(RustDeprecation),
    MustUse(Option<String>),
    DocHidden,
    Export(RustExportAttribute),
    Linkage(RustLinkageAttribute),
    Unresolved(RustRawAttribute),
}

impl RustAttribute {
    fn from_syn(
        attribute: &syn::Attribute,
        cancellation: &CancellationProbe,
    ) -> Result<Option<Self>, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let path = RustPath::from_syn(attribute.path().clone())?;
        let name = path.as_str();

        match name {
            "repr" => RustRepr::from_syn(attribute, cancellation).map(|repr| repr.map(Self::Repr)),
            "non_exhaustive" => {
                RustAttributeSyntax::require_path(attribute, name)?;
                Ok(Some(Self::NonExhaustive))
            }
            "cfg" => Ok(Some(Self::Conditional(RustConditional::cfg(
                attribute,
                cancellation,
            )?))),
            "cfg_attr" => RustConditional::cfg_attr(attribute, cancellation)
                .map(|conditional| conditional.map(Self::Conditional)),
            "deprecated" => Ok(Some(Self::Deprecated(RustDeprecation::from_syn(
                attribute,
            )?))),
            "must_use" => Ok(Some(Self::MustUse(RustAttributeSyntax::optional_string(
                attribute, name,
            )?))),
            "doc" => RustAttributeSyntax::doc_hidden(attribute),
            "allow" | "warn" | "deny" | "forbid" | "expect" => Ok(None),
            "unsafe" => RustAttributeSyntax::unsafe_attribute(attribute, cancellation),
            "no_mangle" => {
                RustAttributeSyntax::require_path(attribute, name)?;
                Ok(Some(Self::Export(RustExportAttribute::NoMangle)))
            }
            "export_name" => Ok(Some(Self::Export(RustExportAttribute::Name(
                RustAttributeSyntax::required_string(attribute, name)?,
            )))),
            "link_section" => Ok(Some(Self::Linkage(RustLinkageAttribute::Section(
                RustAttributeSyntax::required_string(attribute, name)?,
            )))),
            "link_name" => Ok(Some(Self::Linkage(RustLinkageAttribute::Name(
                RustAttributeSyntax::required_string(attribute, name)?,
            )))),
            "link" => Ok(Some(Self::Linkage(RustLinkageAttribute::Library(
                RustNativeLibrary::from_syn(attribute)?,
            )))),
            _ => Ok(Some(Self::Unresolved(RustRawAttribute::from_syn(
                attribute,
            )?))),
        }
    }

    fn validate(&self, cancellation: &CancellationProbe) -> Result<(), SignatureContractKitError> {
        cancellation.checkpoint()?;
        match self {
            Self::Derive(paths) if paths.is_empty() => {
                Err(SignatureContractKitError::conversion_failed(
                    "derive attribute requires at least one path",
                ))
            }
            Self::Repr(repr) => repr.validate(),
            Self::Conditional(conditional) => conditional.validate(cancellation),
            Self::Linkage(RustLinkageAttribute::Library(library)) => library.validate(),
            Self::Unresolved(raw) => raw.validate(),
            Self::Derive(_)
            | Self::NonExhaustive
            | Self::Deprecated(_)
            | Self::MustUse(_)
            | Self::DocHidden
            | Self::Export(_)
            | Self::Linkage(RustLinkageAttribute::Name(_))
            | Self::Linkage(RustLinkageAttribute::Section(_)) => Ok(()),
        }
    }

    fn requires_capability_warning(&self) -> bool {
        self.capability().is_some()
    }

    fn capability(&self) -> Option<RustAttributeCapability> {
        match self {
            Self::Derive(_) => Some(RustAttributeCapability::DeriveExpansion),
            Self::Conditional(_) => Some(RustAttributeCapability::ConditionalCompilation),
            Self::Unresolved(_) => Some(RustAttributeCapability::UnresolvedAttribute),
            Self::Repr(_)
            | Self::NonExhaustive
            | Self::Deprecated(_)
            | Self::MustUse(_)
            | Self::DocHidden
            | Self::Export(_)
            | Self::Linkage(_) => None,
        }
    }

    fn meta_syntax(&self) -> String {
        match self {
            Self::Derive(paths) => format!(
                "derive({})",
                paths
                    .iter()
                    .map(RustPath::source_syntax)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Repr(repr) => format!(
                "repr({})",
                repr.hints()
                    .iter()
                    .map(RustReprHint::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::NonExhaustive => "non_exhaustive".to_owned(),
            Self::Conditional(conditional) => conditional.meta_syntax(),
            Self::Deprecated(deprecation) => deprecation.meta_syntax(),
            Self::MustUse(None) => "must_use".to_owned(),
            Self::MustUse(Some(message)) => format!(
                "must_use = {}",
                RustAttributeSyntax::string_literal(message)
            ),
            Self::DocHidden => "doc(hidden)".to_owned(),
            Self::Export(RustExportAttribute::NoMangle) => "no_mangle".to_owned(),
            Self::Export(RustExportAttribute::Name(name)) => format!(
                "export_name = {}",
                RustAttributeSyntax::string_literal(name)
            ),
            Self::Linkage(RustLinkageAttribute::Name(name)) => {
                format!("link_name = {}", RustAttributeSyntax::string_literal(name))
            }
            Self::Linkage(RustLinkageAttribute::Section(section)) => format!(
                "link_section = {}",
                RustAttributeSyntax::string_literal(section)
            ),
            Self::Linkage(RustLinkageAttribute::Library(library)) => library.meta_syntax(),
            Self::Unresolved(raw) => raw.meta_syntax(),
        }
    }
}

struct RustAttributeSyntax;

impl RustAttributeSyntax {
    fn tokens(value: &impl quote::ToTokens) -> String {
        Self::render_stream(quote::ToTokens::to_token_stream(value))
    }

    fn string_literal(value: &str) -> String {
        Self::tokens(&syn::LitStr::new(value, proc_macro2::Span::call_site()))
    }

    fn render_stream(stream: TokenStream) -> String {
        let tokens = stream.into_iter().collect::<Vec<_>>();
        let mut rendered = String::new();
        let mut index = 0;

        while index < tokens.len() {
            match &tokens[index] {
                TokenTree::Group(group) => {
                    Self::append_group(&mut rendered, group);
                    index += 1;
                }
                TokenTree::Ident(identifier) => {
                    Self::append_word(&mut rendered, &identifier.to_string());
                    index += 1;
                }
                TokenTree::Literal(literal) => {
                    Self::append_word(&mut rendered, &literal.to_string());
                    index += 1;
                }
                TokenTree::Punct(_) => {
                    let mut punctuation = String::new();
                    while let Some(TokenTree::Punct(value)) = tokens.get(index) {
                        punctuation.push(value.as_char());
                        index += 1;
                        if value.spacing() != Spacing::Joint {
                            break;
                        }
                    }
                    Self::append_punctuation(&mut rendered, &punctuation);
                }
            }
        }

        Self::trim_trailing_space(&mut rendered);
        rendered
    }

    fn append_group(rendered: &mut String, group: &proc_macro2::Group) {
        let contents = Self::render_stream(group.stream());
        match group.delimiter() {
            Delimiter::Parenthesis => {
                rendered.push('(');
                rendered.push_str(&contents);
                rendered.push(')');
            }
            Delimiter::Brace => {
                rendered.push('{');
                rendered.push_str(&contents);
                rendered.push('}');
            }
            Delimiter::Bracket => {
                rendered.push('[');
                rendered.push_str(&contents);
                rendered.push(']');
            }
            Delimiter::None => Self::append_fragment(rendered, &contents),
        }
    }

    fn append_fragment(rendered: &mut String, fragment: &str) {
        let separated = rendered
            .chars()
            .next_back()
            .is_some_and(Self::is_word_boundary)
            && fragment.chars().next().is_some_and(Self::is_word_boundary);
        if separated {
            rendered.push(' ');
        }
        rendered.push_str(fragment);
    }

    fn append_word(rendered: &mut String, word: &str) {
        let separated = rendered
            .chars()
            .next_back()
            .is_some_and(Self::is_word_boundary);
        if separated {
            rendered.push(' ');
        }
        rendered.push_str(word);
    }

    fn is_word_boundary(character: char) -> bool {
        character.is_alphanumeric() || matches!(character, '_' | ')' | ']' | '}' | '"' | '\'')
    }

    fn append_punctuation(rendered: &mut String, punctuation: &str) {
        match punctuation {
            "," | ";" => {
                Self::trim_trailing_space(rendered);
                rendered.push_str(punctuation);
                rendered.push(' ');
            }
            ":" => {
                Self::trim_trailing_space(rendered);
                rendered.push_str(": ");
            }
            "::" | "." | ".." | "..=" | "'" | "#" | "$" | "!" | "?" => {
                Self::trim_trailing_space(rendered);
                rendered.push_str(punctuation);
            }
            _ => {
                Self::trim_trailing_space(rendered);
                if !rendered.is_empty() {
                    rendered.push(' ');
                }
                rendered.push_str(punctuation);
                rendered.push(' ');
            }
        }
    }

    fn trim_trailing_space(rendered: &mut String) {
        while rendered.ends_with(' ') {
            rendered.pop();
        }
    }

    fn attribute_from_meta(meta: syn::Meta) -> syn::Attribute {
        syn::parse_quote!(#[#meta])
    }

    fn require_path(
        attribute: &syn::Attribute,
        name: &str,
    ) -> Result<(), SignatureContractKitError> {
        if matches!(&attribute.meta, syn::Meta::Path(_)) {
            Ok(())
        } else {
            Err(SignatureContractKitError::conversion_failed(format!(
                "attribute {name} does not accept arguments"
            )))
        }
    }

    fn required_string(
        attribute: &syn::Attribute,
        name: &str,
    ) -> Result<String, SignatureContractKitError> {
        Self::optional_string(attribute, name)?.ok_or_else(|| {
            SignatureContractKitError::conversion_failed(format!(
                "attribute {name} requires a string value"
            ))
        })
    }

    fn optional_string(
        attribute: &syn::Attribute,
        name: &str,
    ) -> Result<Option<String>, SignatureContractKitError> {
        match &attribute.meta {
            syn::Meta::Path(_) => Ok(None),
            syn::Meta::NameValue(value) => Self::string_expression(&value.value)
                .map(Some)
                .ok_or_else(|| {
                    SignatureContractKitError::conversion_failed(format!(
                        "attribute {name} requires a string literal"
                    ))
                }),
            syn::Meta::List(_) => Err(SignatureContractKitError::conversion_failed(format!(
                "attribute {name} has unsupported list syntax"
            ))),
        }
    }

    fn string_expression(expression: &syn::Expr) -> Option<String> {
        let syn::Expr::Lit(value) = expression else {
            return None;
        };
        let syn::Lit::Str(value) = &value.lit else {
            return None;
        };
        Some(value.value())
    }

    fn doc_hidden(
        attribute: &syn::Attribute,
    ) -> Result<Option<RustAttribute>, SignatureContractKitError> {
        match &attribute.meta {
            syn::Meta::List(list) if Self::tokens(&list.tokens) == "hidden" => {
                Ok(Some(RustAttribute::DocHidden))
            }
            syn::Meta::List(_) | syn::Meta::NameValue(_) | syn::Meta::Path(_) => Ok(None),
        }
    }

    fn unsafe_attribute(
        attribute: &syn::Attribute,
        cancellation: &CancellationProbe,
    ) -> Result<Option<RustAttribute>, SignatureContractKitError> {
        let syn::Meta::List(list) = &attribute.meta else {
            return Err(SignatureContractKitError::conversion_failed(
                "attribute unsafe requires one nested attribute",
            ));
        };
        let meta = syn::parse2::<syn::Meta>(list.tokens.clone()).map_err(|error| {
            SignatureContractKitError::conversion_failed(format!(
                "invalid unsafe attribute: {error}"
            ))
        })?;
        match RustAttributes::from_meta(meta, cancellation)? {
            Some(value @ RustAttribute::Export(RustExportAttribute::NoMangle))
            | Some(value @ RustAttribute::Export(RustExportAttribute::Name(_)))
            | Some(value @ RustAttribute::Linkage(RustLinkageAttribute::Section(_))) => {
                Ok(Some(value))
            }
            Some(_) | None => Ok(Some(RustAttribute::Unresolved(RustRawAttribute::from_syn(
                attribute,
            )?))),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustPath {
    value: String,
}

impl RustPath {
    pub(crate) fn new(value: String) -> Result<Self, SignatureContractKitError> {
        if value.trim().is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "Rust attribute path cannot be empty",
            ));
        }

        if value
            .split("::")
            .any(|segment| segment.trim().starts_with("r#"))
        {
            return Err(SignatureContractKitError::conversion_failed(format!(
                "Rust attribute path {value:?} must omit raw-identifier prefixes"
            )));
        }
        let source_path = value
            .split("::")
            .map(|segment| RustModulePath::source_ident(segment.trim()))
            .collect::<Vec<_>>()
            .join("::");
        let path = syn::parse_str::<syn::Path>(&source_path).map_err(|error| {
            SignatureContractKitError::conversion_failed(format!(
                "invalid Rust attribute path {value:?}: {error}"
            ))
        })?;
        Self::from_syn(path)
    }

    fn from_syn(path: syn::Path) -> Result<Self, SignatureContractKitError> {
        if path.segments.is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "Rust attribute path cannot be empty",
            ));
        }

        if let Some(segment) = path
            .segments
            .iter()
            .find(|segment| !matches!(&segment.arguments, syn::PathArguments::None))
        {
            return Err(SignatureContractKitError::conversion_failed(format!(
                "Rust attribute path segment {} cannot have generic arguments",
                segment.ident
            )));
        }

        let mut value = String::new();
        if path.leading_colon.is_some() {
            value.push_str("::");
        }
        for (index, segment) in path.segments.iter().enumerate() {
            if index > 0 {
                value.push_str("::");
            }
            value.push_str(&RustModulePath::semantic_ident(&segment.ident));
        }

        Ok(Self { value })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.value
    }

    fn source_syntax(&self) -> String {
        self.value
            .split("::")
            .map(RustModulePath::source_ident)
            .collect::<Vec<_>>()
            .join("::")
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustRepr {
    hints: Vec<RustReprHint>,
}

impl RustRepr {
    pub(crate) fn new(hints: Vec<RustReprHint>) -> Result<Self, SignatureContractKitError> {
        let repr = Self {
            hints: hints.into_iter().map(RustReprHint::canonical).collect(),
        };
        repr.validate()?;
        Ok(repr)
    }

    fn from_syn(
        attribute: &syn::Attribute,
        cancellation: &CancellationProbe,
    ) -> Result<Option<Self>, SignatureContractKitError> {
        let values = attribute
            .parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            )
            .map_err(|error| {
                SignatureContractKitError::conversion_failed(format!(
                    "invalid repr attribute: {error}"
                ))
            })?;
        if values.is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "repr attribute requires at least one hint",
            ));
        }
        let mut hints = Vec::with_capacity(values.len());
        for value in values {
            cancellation.checkpoint()?;
            if matches!(&value, syn::Meta::Path(path) if path.is_ident("Rust")) {
                continue;
            }
            hints.push(RustReprHint::from_syn(value)?);
        }

        if hints.is_empty() {
            Ok(None)
        } else {
            Self::new(hints).map(Some)
        }
    }

    fn validate(&self) -> Result<(), SignatureContractKitError> {
        if self.hints.is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "repr attribute requires at least one hint",
            ));
        }

        Ok(())
    }

    pub(crate) fn hints(&self) -> &[RustReprHint] {
        &self.hints
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum RustReprHint {
    C,
    Transparent,
    Simd,
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
    Align(u32),
    Packed(Option<u32>),
}

impl RustReprHint {
    fn canonical(self) -> Self {
        match self {
            Self::Packed(Some(1)) => Self::Packed(None),
            value => value,
        }
    }

    fn from_syn(meta: syn::Meta) -> Result<Self, SignatureContractKitError> {
        match meta {
            syn::Meta::Path(path) if path.is_ident("C") => Ok(Self::C),
            syn::Meta::Path(path) if path.is_ident("transparent") => Ok(Self::Transparent),
            syn::Meta::Path(path) if path.is_ident("simd") => Ok(Self::Simd),
            syn::Meta::Path(path) if path.is_ident("i8") => Ok(Self::I8),
            syn::Meta::Path(path) if path.is_ident("i16") => Ok(Self::I16),
            syn::Meta::Path(path) if path.is_ident("i32") => Ok(Self::I32),
            syn::Meta::Path(path) if path.is_ident("i64") => Ok(Self::I64),
            syn::Meta::Path(path) if path.is_ident("i128") => Ok(Self::I128),
            syn::Meta::Path(path) if path.is_ident("isize") => Ok(Self::Isize),
            syn::Meta::Path(path) if path.is_ident("u8") => Ok(Self::U8),
            syn::Meta::Path(path) if path.is_ident("u16") => Ok(Self::U16),
            syn::Meta::Path(path) if path.is_ident("u32") => Ok(Self::U32),
            syn::Meta::Path(path) if path.is_ident("u64") => Ok(Self::U64),
            syn::Meta::Path(path) if path.is_ident("u128") => Ok(Self::U128),
            syn::Meta::Path(path) if path.is_ident("usize") => Ok(Self::Usize),
            syn::Meta::Path(path) if path.is_ident("packed") => Ok(Self::Packed(None)),
            syn::Meta::List(list) if list.path.is_ident("align") => {
                Ok(Self::Align(Self::integer_argument(&list, "align")?))
            }
            syn::Meta::List(list) if list.path.is_ident("packed") => {
                Ok(Self::Packed(Some(Self::integer_argument(&list, "packed")?)))
            }
            unsupported => Err(SignatureContractKitError::conversion_failed(format!(
                "unsupported repr hint {}",
                RustAttributeSyntax::tokens(&unsupported)
            ))),
        }
    }

    fn integer_argument(
        list: &syn::MetaList,
        name: &str,
    ) -> Result<u32, SignatureContractKitError> {
        let literal = syn::parse2::<syn::LitInt>(list.tokens.clone()).map_err(|error| {
            SignatureContractKitError::conversion_failed(format!(
                "invalid repr {name} argument: {error}"
            ))
        })?;
        literal.base10_parse::<u32>().map_err(|error| {
            SignatureContractKitError::conversion_failed(format!(
                "invalid repr {name} argument: {error}"
            ))
        })
    }

    pub(crate) fn as_str(&self) -> String {
        match self {
            Self::C => "C".to_owned(),
            Self::Transparent => "transparent".to_owned(),
            Self::Simd => "simd".to_owned(),
            Self::I8 => "i8".to_owned(),
            Self::I16 => "i16".to_owned(),
            Self::I32 => "i32".to_owned(),
            Self::I64 => "i64".to_owned(),
            Self::I128 => "i128".to_owned(),
            Self::Isize => "isize".to_owned(),
            Self::U8 => "u8".to_owned(),
            Self::U16 => "u16".to_owned(),
            Self::U32 => "u32".to_owned(),
            Self::U64 => "u64".to_owned(),
            Self::U128 => "u128".to_owned(),
            Self::Usize => "usize".to_owned(),
            Self::Align(value) => format!("align({value})"),
            Self::Packed(None) => "packed".to_owned(),
            Self::Packed(Some(value)) => format!("packed({value})"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustCfgPredicate {
    value: String,
}

impl RustCfgPredicate {
    pub(crate) fn new(
        value: String,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let stream = value.parse::<TokenStream>().map_err(|error| {
            SignatureContractKitError::conversion_failed(format!(
                "invalid Rust cfg predicate {value:?}: {error}"
            ))
        })?;
        Self::from_tokens(stream, cancellation)
    }

    fn from_tokens(
        stream: TokenStream,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        cancellation.checkpoint()?;
        if stream.is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "Rust cfg predicate cannot be empty",
            ));
        }

        let mut pending = vec![stream.clone()];
        while let Some(predicate) = pending.pop() {
            cancellation.checkpoint()?;
            Self::validate_one(predicate, &mut pending, cancellation)?;
        }

        Ok(Self {
            value: RustAttributeSyntax::render_stream(stream),
        })
    }

    fn validate_one(
        predicate: TokenStream,
        pending: &mut Vec<TokenStream>,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        let tokens = predicate.into_iter().collect::<Vec<_>>();
        for _ in &tokens {
            cancellation.checkpoint()?;
        }
        match tokens.as_slice() {
            [TokenTree::Ident(identifier)] => Self::validate_identifier(identifier, true),
            [
                TokenTree::Ident(identifier),
                TokenTree::Punct(operator),
                TokenTree::Literal(value),
            ] if operator.as_char() == '=' => {
                Self::validate_identifier(identifier, false)?;
                syn::parse_str::<syn::LitStr>(&value.to_string())
                    .map(|_| ())
                    .map_err(|_| {
                        SignatureContractKitError::conversion_failed(
                            "Rust cfg option values must be string literals",
                        )
                    })
            }
            [TokenTree::Ident(operator), TokenTree::Group(group)]
                if group.delimiter() == Delimiter::Parenthesis =>
            {
                Self::validate_list(operator.to_string(), group.stream(), pending, cancellation)
            }
            _ => Err(SignatureContractKitError::conversion_failed(
                "Rust cfg predicate must be an option, string-valued option, all, any, or not",
            )),
        }
    }

    fn validate_identifier(
        identifier: &proc_macro2::Ident,
        allow_boolean: bool,
    ) -> Result<(), SignatureContractKitError> {
        let value = identifier.to_string();
        if allow_boolean && matches!(value.as_str(), "true" | "false") {
            return Ok(());
        }
        syn::parse_str::<syn::Ident>(&value)
            .map(|_| ())
            .map_err(|_| {
                SignatureContractKitError::conversion_failed(format!(
                    "Rust cfg option {value:?} must be a non-keyword identifier"
                ))
            })
    }

    fn validate_list(
        operator: String,
        contents: TokenStream,
        pending: &mut Vec<TokenStream>,
        cancellation: &CancellationProbe,
    ) -> Result<(), SignatureContractKitError> {
        if operator == "not" {
            if contents.is_empty()
                || contents
                    .clone()
                    .into_iter()
                    .any(|token| matches!(token, TokenTree::Punct(value) if value.as_char() == ','))
            {
                return Err(SignatureContractKitError::conversion_failed(
                    "Rust cfg not requires exactly one predicate without a trailing comma",
                ));
            }
            pending.push(contents);
            return Ok(());
        }
        if !matches!(operator.as_str(), "all" | "any") {
            return Err(SignatureContractKitError::conversion_failed(format!(
                "unsupported Rust cfg predicate operator {operator}"
            )));
        }

        let mut current = TokenStream::new();
        let mut predicates = Vec::new();
        for token in contents {
            cancellation.checkpoint()?;
            if matches!(&token, TokenTree::Punct(value) if value.as_char() == ',') {
                if current.is_empty() {
                    return Err(SignatureContractKitError::conversion_failed(
                        "Rust cfg predicate list contains an empty predicate",
                    ));
                }
                predicates.push(std::mem::take(&mut current));
            } else {
                current.extend(std::iter::once(token));
            }
        }
        if !current.is_empty() {
            predicates.push(current);
        }
        pending.extend(predicates.into_iter().rev());
        Ok(())
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.value
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum RustConditional {
    Cfg(RustCfgPredicate),
    CfgAttr {
        predicate: RustCfgPredicate,
        attributes: Vec<RustAttribute>,
    },
}

impl RustConditional {
    fn cfg(
        attribute: &syn::Attribute,
        cancellation: &CancellationProbe,
    ) -> Result<Self, SignatureContractKitError> {
        let syn::Meta::List(list) = &attribute.meta else {
            return Err(SignatureContractKitError::conversion_failed(
                "cfg attribute requires a predicate",
            ));
        };
        if list.tokens.is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "cfg attribute requires a predicate",
            ));
        }
        Ok(Self::Cfg(RustCfgPredicate::from_tokens(
            list.tokens.clone(),
            cancellation,
        )?))
    }

    fn cfg_attr(
        attribute: &syn::Attribute,
        cancellation: &CancellationProbe,
    ) -> Result<Option<Self>, SignatureContractKitError> {
        cancellation.checkpoint()?;
        let syn::Meta::List(list) = &attribute.meta else {
            return Err(SignatureContractKitError::conversion_failed(
                "cfg_attr attribute requires a predicate and comma",
            ));
        };
        let mut predicate_tokens = TokenStream::new();
        let mut attribute_tokens = TokenStream::new();
        let mut found_comma = false;
        for token in list.tokens.clone() {
            cancellation.checkpoint()?;
            if !found_comma && matches!(&token, TokenTree::Punct(value) if value.as_char() == ',') {
                found_comma = true;
            } else if found_comma {
                attribute_tokens.extend(std::iter::once(token));
            } else {
                predicate_tokens.extend(std::iter::once(token));
            }
        }
        if !found_comma {
            return Err(SignatureContractKitError::conversion_failed(
                "cfg_attr attribute requires a predicate followed by a comma",
            ));
        }
        let predicate = RustCfgPredicate::from_tokens(predicate_tokens, cancellation)?;
        if attribute_tokens.is_empty() {
            return Ok(None);
        }
        let nested = syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated
            .parse2(attribute_tokens)
            .map_err(|error| {
                SignatureContractKitError::conversion_failed(format!(
                    "invalid cfg_attr nested attributes: {error}"
                ))
            })?;
        let mut attributes = Vec::new();
        for meta in nested {
            cancellation.checkpoint()?;
            if let Some(attribute) = RustAttributes::from_meta(meta, cancellation)? {
                attributes.push(attribute);
            }
        }
        if attributes.is_empty() {
            return Ok(None);
        }

        let conditional = Self::CfgAttr {
            predicate,
            attributes,
        };
        conditional.validate(cancellation)?;
        Ok(Some(conditional))
    }

    fn validate(&self, cancellation: &CancellationProbe) -> Result<(), SignatureContractKitError> {
        cancellation.checkpoint()?;
        match self {
            Self::Cfg(_) => Ok(()),
            Self::CfgAttr { attributes, .. } if attributes.is_empty() => {
                Err(SignatureContractKitError::conversion_failed(
                    "cfg_attr attribute requires at least one nested semantic attribute",
                ))
            }
            Self::CfgAttr { attributes, .. } => {
                for attribute in attributes {
                    cancellation.checkpoint()?;
                    attribute.validate(cancellation)?;
                }
                Ok(())
            }
        }
    }

    fn meta_syntax(&self) -> String {
        match self {
            Self::Cfg(predicate) => format!("cfg({})", predicate.as_str()),
            Self::CfgAttr {
                predicate,
                attributes,
            } => {
                let mut rendered = format!("cfg_attr({}", predicate.as_str());
                for attribute in attributes {
                    rendered.push_str(", ");
                    rendered.push_str(&attribute.meta_syntax());
                }
                rendered.push(')');
                rendered
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustTokenSyntax {
    value: String,
}

impl RustTokenSyntax {
    pub(crate) fn new(value: String) -> Result<Self, SignatureContractKitError> {
        if value.trim().is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "Rust attribute token syntax cannot be empty",
            ));
        }

        let stream = value.parse::<TokenStream>().map_err(|error| {
            SignatureContractKitError::conversion_failed(format!(
                "invalid Rust attribute token syntax {value:?}: {error}"
            ))
        })?;
        if stream.is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "Rust attribute token syntax cannot be empty",
            ));
        }

        Ok(Self {
            value: RustAttributeSyntax::render_stream(stream),
        })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.value
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub(crate) struct RustDeprecation {
    since: Option<String>,
    note: Option<String>,
}

impl RustDeprecation {
    pub(crate) fn new(since: Option<String>, note: Option<String>) -> Self {
        Self { since, note }
    }

    fn from_syn(attribute: &syn::Attribute) -> Result<Self, SignatureContractKitError> {
        match &attribute.meta {
            syn::Meta::Path(_) => Ok(Self::default()),
            syn::Meta::List(_) => {
                let values = attribute
                    .parse_args_with(
                        syn::punctuated::Punctuated::<syn::MetaNameValue, syn::Token![,]>::parse_terminated,
                    )
                    .map_err(|error| {
                        SignatureContractKitError::conversion_failed(format!(
                            "invalid deprecated attribute: {error}"
                        ))
                    })?;
                let mut deprecation = Self::default();
                for value in values {
                    let Some(name) = value.path.get_ident().map(ToString::to_string) else {
                        return Err(SignatureContractKitError::conversion_failed(
                            "deprecated attribute keys must be identifiers",
                        ));
                    };
                    let text =
                        RustAttributeSyntax::string_expression(&value.value).ok_or_else(|| {
                            SignatureContractKitError::conversion_failed(format!(
                                "deprecated {name} requires a string literal"
                            ))
                        })?;
                    match name.as_str() {
                        "since" if deprecation.since.is_none() => deprecation.since = Some(text),
                        "note" if deprecation.note.is_none() => deprecation.note = Some(text),
                        "since" | "note" => {
                            return Err(SignatureContractKitError::conversion_failed(format!(
                                "duplicate deprecated {name} value"
                            )));
                        }
                        _ => {
                            return Err(SignatureContractKitError::conversion_failed(format!(
                                "unsupported deprecated field {name}"
                            )));
                        }
                    }
                }
                Ok(deprecation)
            }
            syn::Meta::NameValue(_) => Err(SignatureContractKitError::conversion_failed(
                "deprecated attribute does not accept name-value syntax",
            )),
        }
    }

    pub(crate) fn since(&self) -> Option<&str> {
        self.since.as_deref()
    }

    pub(crate) fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    fn meta_syntax(&self) -> String {
        let mut fields = Vec::new();
        if let Some(since) = &self.since {
            fields.push(format!(
                "since = {}",
                RustAttributeSyntax::string_literal(since)
            ));
        }
        if let Some(note) = &self.note {
            fields.push(format!(
                "note = {}",
                RustAttributeSyntax::string_literal(note)
            ));
        }

        if fields.is_empty() {
            "deprecated".to_owned()
        } else {
            format!("deprecated({})", fields.join(", "))
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum RustExportAttribute {
    NoMangle,
    Name(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum RustLinkageAttribute {
    Name(String),
    Section(String),
    Library(RustNativeLibrary),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustNativeLibrary {
    name: String,
    kind: Option<String>,
    modifiers: Option<String>,
}

impl RustNativeLibrary {
    pub(crate) fn new(
        name: String,
        kind: Option<String>,
        modifiers: Option<String>,
    ) -> Result<Self, SignatureContractKitError> {
        let library = Self {
            name,
            kind,
            modifiers,
        };
        library.validate()?;
        Ok(library)
    }

    fn from_syn(attribute: &syn::Attribute) -> Result<Self, SignatureContractKitError> {
        let values = attribute
            .parse_args_with(
                syn::punctuated::Punctuated::<syn::MetaNameValue, syn::Token![,]>::parse_terminated,
            )
            .map_err(|error| {
                SignatureContractKitError::conversion_failed(format!(
                    "invalid link attribute: {error}"
                ))
            })?;
        let mut library_name = None;
        let mut kind = None;
        let mut modifiers = None;

        for value in values {
            let Some(field_name) = value.path.get_ident().map(ToString::to_string) else {
                return Err(SignatureContractKitError::conversion_failed(
                    "link attribute keys must be identifiers",
                ));
            };
            let text = RustAttributeSyntax::string_expression(&value.value).ok_or_else(|| {
                SignatureContractKitError::conversion_failed(format!(
                    "link {field_name} requires a string literal"
                ))
            })?;
            match field_name.as_str() {
                "name" if library_name.is_none() => library_name = Some(text),
                "kind" if kind.is_none() => kind = Some(text),
                "modifiers" if modifiers.is_none() => modifiers = Some(text),
                "name" | "kind" | "modifiers" => {
                    return Err(SignatureContractKitError::conversion_failed(format!(
                        "duplicate link {field_name} value"
                    )));
                }
                _ => {
                    return Err(SignatureContractKitError::conversion_failed(format!(
                        "unsupported link field {field_name}"
                    )));
                }
            }
        }

        let name = library_name.ok_or_else(|| {
            SignatureContractKitError::conversion_failed("link attribute requires a nonempty name")
        })?;
        Self::new(name, kind, modifiers)
    }

    fn validate(&self) -> Result<(), SignatureContractKitError> {
        if self.name.is_empty() {
            return Err(SignatureContractKitError::conversion_failed(
                "link attribute requires a nonempty name",
            ));
        }

        Ok(())
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn kind(&self) -> Option<&str> {
        self.kind.as_deref()
    }

    pub(crate) fn modifiers(&self) -> Option<&str> {
        self.modifiers.as_deref()
    }

    fn meta_syntax(&self) -> String {
        let mut fields = vec![format!(
            "name = {}",
            RustAttributeSyntax::string_literal(&self.name)
        )];
        if let Some(kind) = &self.kind {
            fields.push(format!(
                "kind = {}",
                RustAttributeSyntax::string_literal(kind)
            ));
        }
        if let Some(modifiers) = &self.modifiers {
            fields.push(format!(
                "modifiers = {}",
                RustAttributeSyntax::string_literal(modifiers)
            ));
        }
        format!("link({})", fields.join(", "))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) enum RustRawAttributeArguments {
    Path,
    List(RustTokenSyntax),
    NameValue(RustSyntaxText),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RustRawAttribute {
    path: RustPath,
    arguments: RustRawAttributeArguments,
}

impl RustRawAttribute {
    pub(crate) fn new(
        path: RustPath,
        arguments: RustRawAttributeArguments,
    ) -> Result<Self, SignatureContractKitError> {
        let raw = Self { path, arguments };
        raw.validate()?;
        Ok(raw)
    }

    fn from_syn(attribute: &syn::Attribute) -> Result<Self, SignatureContractKitError> {
        let arguments = match &attribute.meta {
            syn::Meta::Path(_) => RustRawAttributeArguments::Path,
            syn::Meta::List(list) => RustRawAttributeArguments::List(RustTokenSyntax::new(
                RustAttributeSyntax::tokens(&list.tokens),
            )?),
            syn::Meta::NameValue(value) => {
                RustRawAttributeArguments::NameValue(RustSyntaxText::from_expression(&value.value))
            }
        };
        Self::new(RustPath::from_syn(attribute.path().clone())?, arguments)
    }

    fn validate(&self) -> Result<(), SignatureContractKitError> {
        match &self.arguments {
            RustRawAttributeArguments::Path => Ok(()),
            RustRawAttributeArguments::List(arguments) => {
                RustTokenSyntax::new(arguments.value.clone()).map(|_| ())
            }
            RustRawAttributeArguments::NameValue(_) => Ok(()),
        }
    }

    pub(crate) fn path(&self) -> &str {
        self.path.as_str()
    }

    pub(crate) fn arguments(&self) -> &RustRawAttributeArguments {
        &self.arguments
    }

    fn meta_syntax(&self) -> String {
        let path = self.path.source_syntax();
        match &self.arguments {
            RustRawAttributeArguments::Path => path,
            RustRawAttributeArguments::List(arguments) => {
                format!("{path}({})", arguments.as_str())
            }
            RustRawAttributeArguments::NameValue(value) => {
                format!("{path} = {}", value.as_str())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RustAttribute, RustAttributes, RustCfgPredicate, RustConditional, RustExportAttribute,
        RustLinkageAttribute, RustNativeLibrary, RustPath, RustRawAttribute,
        RustRawAttributeArguments, RustRepr, RustReprHint, RustTokenSyntax,
    };
    use crate::languages::rust::types::syntax_text::RustSyntaxText;

    struct AttributeFixture {
        attributes: Vec<syn::Attribute>,
    }

    impl AttributeFixture {
        fn on_struct(source: proc_macro2::TokenStream) -> Self {
            let item: syn::ItemStruct = syn::parse2(quote::quote! {
                #source
                struct Probe;
            })
            .expect("attribute fixture");
            Self {
                attributes: item.attrs,
            }
        }

        fn parse(&self) -> RustAttributes {
            RustAttributes::from_syn(&self.attributes, &crate::work::CancellationProbe::new())
                .expect("semantic attributes")
        }

        fn try_parse(&self) -> Result<RustAttributes, crate::error::SignatureContractKitError> {
            RustAttributes::from_syn(&self.attributes, &crate::work::CancellationProbe::new())
        }
    }

    #[test]
    fn repr_forms_remain_typed_and_in_source_order() {
        let fixture = AttributeFixture::on_struct(quote::quote! {
            #[repr(C, transparent, u8, align(16), packed, packed(2))]
        });
        let attributes = fixture.parse();
        let RustAttribute::Repr(repr) = &attributes.values()[0] else {
            panic!("repr must not degrade into raw attribute text");
        };
        let hints = repr
            .hints()
            .iter()
            .map(RustReprHint::as_str)
            .collect::<Vec<_>>();

        assert_eq!(
            hints,
            ["C", "transparent", "u8", "align(16)", "packed", "packed(2)"]
        );
        assert!(!attributes.requires_capability_warning());
    }

    #[test]
    fn alignment_one_packing_has_one_canonical_representation() {
        let implicit = AttributeFixture::on_struct(quote::quote! {
            #[repr(packed)]
        })
        .parse();
        let explicit = AttributeFixture::on_struct(quote::quote! {
            #[repr(packed(1))]
        })
        .parse();

        assert_eq!(implicit, explicit);
        let [RustAttribute::Repr(repr)] = explicit.values() else {
            panic!("explicit alignment-one packing must remain a repr attribute");
        };
        assert_eq!(
            repr.hints()
                .iter()
                .map(RustReprHint::as_str)
                .collect::<Vec<_>>(),
            ["packed"]
        );

        let direct = RustRepr::new(vec![RustReprHint::Packed(Some(1))])
            .expect("direct canonical representation");
        assert_eq!(direct.hints(), [RustReprHint::Packed(None)]);
    }

    #[test]
    fn packing_greater_than_one_remains_explicit() {
        let attributes = AttributeFixture::on_struct(quote::quote! {
            #[repr(C, packed(2))]
        })
        .parse();
        let [RustAttribute::Repr(repr)] = attributes.values() else {
            panic!("packing modifier must remain a repr attribute");
        };

        assert_eq!(
            repr.hints()
                .iter()
                .map(RustReprHint::as_str)
                .collect::<Vec<_>>(),
            ["C", "packed(2)"]
        );
    }

    #[test]
    fn explicit_rust_repr_is_the_implicit_default_but_retains_modifiers() {
        let default = AttributeFixture::on_struct(quote::quote! {
            #[repr(Rust)]
        })
        .parse();
        assert!(default.values().is_empty());

        let modified = AttributeFixture::on_struct(quote::quote! {
            #[repr(Rust, align(16), packed(2))]
        })
        .parse();
        let [RustAttribute::Repr(repr)] = modified.values() else {
            panic!("repr modifiers must remain semantic after dropping the default Rust hint");
        };
        assert_eq!(
            repr.hints()
                .iter()
                .map(RustReprHint::as_str)
                .collect::<Vec<_>>(),
            ["align(16)", "packed(2)"]
        );
    }

    #[test]
    fn non_exhaustive_and_api_metadata_are_typed() {
        let fixture = AttributeFixture::on_struct(quote::quote! {
            #[non_exhaustive]
            #[deprecated(since = "2.0.0", note = "use Replacement")]
            #[must_use = "dropping this value loses work"]
            #[doc(hidden)]
        });
        let attributes = fixture.parse();

        assert!(matches!(
            attributes.values()[0],
            RustAttribute::NonExhaustive
        ));
        let RustAttribute::Deprecated(deprecation) = &attributes.values()[1] else {
            panic!("deprecated must be structured");
        };
        assert_eq!(deprecation.since(), Some("2.0.0"));
        assert_eq!(deprecation.note(), Some("use Replacement"));
        let RustAttribute::MustUse(message) = &attributes.values()[2] else {
            panic!("must_use must be structured");
        };
        assert_eq!(message.as_deref(), Some("dropping this value loses work"));
        assert!(matches!(attributes.values()[3], RustAttribute::DocHidden));
    }

    #[test]
    fn cfg_and_cfg_attr_preserve_structure_and_require_a_capability_warning() {
        let fixture = AttributeFixture::on_struct(quote::quote! {
            #[cfg(all(unix, feature = "fast"))]
            #[cfg_attr(target_os = "windows", repr(C), must_use = "consume")]
        });
        let attributes = fixture.parse();

        let RustAttribute::Conditional(RustConditional::Cfg(condition)) = &attributes.values()[0]
        else {
            panic!("cfg must be structured");
        };
        assert_eq!(condition.as_str(), "all(unix, feature = \"fast\")");
        let RustAttribute::Conditional(RustConditional::CfgAttr {
            predicate,
            attributes: nested,
        }) = &attributes.values()[1]
        else {
            panic!("cfg_attr must retain its predicate and nested attributes");
        };
        assert_eq!(predicate.as_str(), "target_os = \"windows\"");
        assert!(matches!(nested[0], RustAttribute::Repr(_)));
        assert!(matches!(nested[1], RustAttribute::MustUse(_)));
        assert!(attributes.requires_capability_warning());
    }

    #[test]
    fn cfg_predicates_accept_only_the_rust_reference_grammar() {
        let valid = [
            quote::quote!(#[cfg(unix)]),
            quote::quote!(#[cfg(feature = "fast")]),
            quote::quote!(#[cfg(target_feature = r#"sse4.2"#)]),
            quote::quote!(#[cfg(true)]),
            quote::quote!(#[cfg(false)]),
            quote::quote!(#[cfg(all())]),
            quote::quote!(#[cfg(any())]),
            quote::quote!(#[cfg(all(unix, feature = "fast",))]),
            quote::quote!(#[cfg(not(windows))]),
            quote::quote!(#[cfg(any(all(unix, feature = "fast"), not(windows)))]),
        ];
        for source in valid {
            AttributeFixture::on_struct(source)
                .try_parse()
                .expect("valid cfg predicate");
        }

        let invalid = [
            quote::quote!(#[cfg(crate::unix)]),
            quote::quote!(#[cfg(crate)]),
            quote::quote!(#[cfg(fn)]),
            quote::quote!(#[cfg(true = "value")]),
            quote::quote!(#[cfg(feature = 1)]),
            quote::quote!(#[cfg(feature = true)]),
            quote::quote!(#[cfg(target_os("linux"))]),
            quote::quote!(#[cfg(unix, windows)]),
            quote::quote!(#[cfg(not())]),
            quote::quote!(#[cfg(not(unix, windows))]),
            quote::quote!(#[cfg(not(unix,))]),
            quote::quote!(#[cfg(all(,))]),
            quote::quote!(#[cfg(any(unix,, windows))]),
            quote::quote!(#[cfg({ unix })]),
            quote::quote!(#[cfg([unix])]),
        ];
        for source in invalid {
            AttributeFixture::on_struct(source)
                .try_parse()
                .expect_err("balanced non-cfg syntax must fail closed");
        }
    }

    #[test]
    fn cfg_attr_accepts_an_empty_tail_and_attribute_trailing_comma() {
        let no_op = AttributeFixture::on_struct(quote::quote! {
            #[cfg_attr(unix,)]
        })
        .parse();
        assert!(no_op.values().is_empty());

        let retained = AttributeFixture::on_struct(quote::quote! {
            #[cfg_attr(true, repr(C), must_use,)]
        })
        .parse();
        let [
            RustAttribute::Conditional(RustConditional::CfgAttr {
                predicate,
                attributes,
            }),
        ] = retained.values()
        else {
            panic!("cfg_attr with nested semantic attributes must remain structured");
        };
        assert_eq!(predicate.as_str(), "true");
        assert_eq!(attributes.len(), 2);

        for source in [
            quote::quote!(#[cfg_attr(crate::unix,)]),
            quote::quote!(#[cfg_attr(feature = 1, must_use)]),
        ] {
            AttributeFixture::on_struct(source)
                .try_parse()
                .expect_err("cfg_attr must validate its predicate before nested semantics");
        }
    }

    #[test]
    fn export_and_linkage_attributes_are_not_lost() {
        let fixture = AttributeFixture::on_struct(quote::quote! {
            #[unsafe(no_mangle)]
            #[unsafe(export_name = "contract_symbol")]
            #[unsafe(link_section = ".contract")]
            #[link_name = "native_symbol"]
            #[link(name = "contract_native", kind = "static", modifiers = "+bundle")]
        });
        let attributes = fixture.parse();

        assert!(matches!(
            attributes.values()[0],
            RustAttribute::Export(RustExportAttribute::NoMangle)
        ));
        assert!(matches!(
            &attributes.values()[1],
            RustAttribute::Export(RustExportAttribute::Name(name))
                if name == "contract_symbol"
        ));
        assert!(matches!(
            &attributes.values()[2],
            RustAttribute::Linkage(RustLinkageAttribute::Section(section))
                if section == ".contract"
        ));
        assert!(matches!(
            &attributes.values()[3],
            RustAttribute::Linkage(RustLinkageAttribute::Name(name))
                if name == "native_symbol"
        ));
        let RustAttribute::Linkage(RustLinkageAttribute::Library(library)) =
            &attributes.values()[4]
        else {
            panic!("native library linkage must be structured");
        };
        assert_eq!(library.name(), "contract_native");
        assert_eq!(library.kind(), Some("static"));
        assert_eq!(library.modifiers(), Some("+bundle"));
    }

    #[test]
    fn unknown_semantic_attributes_are_retained_and_warned() {
        let fixture = AttributeFixture::on_struct(quote::quote! {
            #[contract_runtime(mode = "stable", nested(flag))]
        });
        let attributes = fixture.parse();
        let RustAttribute::Unresolved(raw) = &attributes.values()[0] else {
            panic!("unknown semantic attribute must not be dropped");
        };

        assert_eq!(raw.path(), "contract_runtime");
        assert_eq!(
            raw.arguments(),
            &RustRawAttributeArguments::List(
                RustTokenSyntax::new("mode = \"stable\", nested(flag)".to_owned())
                    .expect("canonical raw arguments")
            )
        );
        assert!(attributes.requires_capability_warning());
    }

    #[test]
    fn ordinary_docs_and_lints_do_not_enter_api_semantics() {
        let fixture = AttributeFixture::on_struct(quote::quote! {
            #[doc = "ordinary prose"]
            #[allow(dead_code)]
            #[warn(clippy::pedantic)]
            #[expect(unused_variables, reason = "fixture")]
            #[cfg_attr(unix, allow(dead_code), doc = "conditional prose")]
        });
        let attributes = fixture.parse();

        assert!(attributes.values().is_empty());
        assert!(!attributes.requires_capability_warning());
    }

    #[test]
    fn derive_paths_preserve_attribute_path_and_duplicate_order() {
        let fixture = AttributeFixture::on_struct(quote::quote! {
            #[derive(Clone, serde::Serialize, Clone,)]
            #[derive(core::fmt::Debug)]
        });
        let attributes = fixture.parse();
        let paths = attributes
            .values()
            .iter()
            .map(|attribute| {
                let RustAttribute::Derive(paths) = attribute else {
                    panic!("derive attributes must remain distinct");
                };
                paths.iter().map(|path| path.as_str()).collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        assert_eq!(
            paths,
            [
                vec!["Clone", "serde::Serialize", "Clone"],
                vec!["core::fmt::Debug"],
            ]
        );
    }

    #[test]
    fn direct_derive_requires_a_syntax_mode_capability_warning() {
        let attributes = AttributeFixture::on_struct(quote::quote! {
            #[derive(Clone, serde::Serialize)]
        })
        .parse();

        assert!(
            attributes.requires_capability_warning(),
            "rust_syntax_v2 retains derive syntax but does not expand derive macros"
        );
    }

    #[test]
    fn attribute_paths_store_keyword_semantics_and_render_valid_raw_syntax() {
        let attribute: syn::Attribute = syn::parse_quote!(#[derive(r#type)]);
        let attributes =
            RustAttributes::from_syn(&[attribute], &crate::work::CancellationProbe::new())
                .expect("raw source attribute path");
        let RustAttribute::Derive(paths) = &attributes.values()[0] else {
            panic!("derive attribute");
        };

        assert_eq!(paths[0].as_str(), "type");
        assert_eq!(attributes.source_syntax(), "#[derive(r#type)]");
        assert_eq!(
            RustPath::new("type".to_owned())
                .expect("canonical keyword path")
                .as_str(),
            "type"
        );
        assert!(RustPath::new("r#type".to_owned()).is_err());
    }

    #[test]
    fn yaml_constructors_validate_and_canonicalize_semantic_values() {
        let path = RustPath::new(" serde :: Serialize ".to_owned())
            .expect("spaced path remains valid Rust syntax");
        assert_eq!(path.as_str(), "serde::Serialize");

        let predicate = RustCfgPredicate::new(
            "all ( unix , feature=\"fast\" )".to_owned(),
            &crate::work::CancellationProbe::new(),
        )
        .expect("valid cfg tokens");
        assert_eq!(predicate.as_str(), "all(unix, feature = \"fast\")");

        let raw = RustRawAttribute::new(
            RustPath::new("contract_runtime".to_owned()).expect("raw path"),
            RustRawAttributeArguments::List(
                RustTokenSyntax::new("mode=\"stable\",nested ( flag )".to_owned())
                    .expect("raw list arguments"),
            ),
        )
        .expect("valid raw attribute");
        assert_eq!(
            raw.arguments(),
            &RustRawAttributeArguments::List(
                RustTokenSyntax::new("mode = \"stable\", nested(flag)".to_owned())
                    .expect("canonical raw list arguments")
            )
        );

        assert!(RustPath::new("serde::Serialize<T>".to_owned()).is_err());
        assert!(RustTokenSyntax::new("all(".to_owned()).is_err());
        assert!(RustRepr::new(Vec::new()).is_err());
        assert!(RustNativeLibrary::new(String::new(), None, None).is_err());
        assert!(
            RustAttributes::new(
                vec![RustAttribute::Derive(Vec::new())],
                &crate::work::CancellationProbe::new(),
            )
            .is_err()
        );

        let empty_cfg_attr = RustAttribute::Conditional(RustConditional::CfgAttr {
            predicate: RustCfgPredicate::new(
                "unix".to_owned(),
                &crate::work::CancellationProbe::new(),
            )
            .expect("predicate"),
            attributes: Vec::new(),
        });
        assert!(
            RustAttributes::new(vec![empty_cfg_attr], &crate::work::CancellationProbe::new(),)
                .is_err()
        );

        let invalid_nested_cfg_attr = RustAttribute::Conditional(RustConditional::CfgAttr {
            predicate: RustCfgPredicate::new(
                "unix".to_owned(),
                &crate::work::CancellationProbe::new(),
            )
            .expect("predicate"),
            attributes: vec![RustAttribute::Derive(Vec::new())],
        });
        assert!(
            RustAttributes::new(
                vec![invalid_nested_cfg_attr],
                &crate::work::CancellationProbe::new(),
            )
            .is_err()
        );
    }

    #[test]
    fn raw_attribute_source_syntax_preserves_path_list_and_name_value_forms() {
        let path = RustPath::new("contract_runtime".to_owned()).expect("raw path");
        let attributes = RustAttributes::new(
            vec![
                RustAttribute::Unresolved(
                    RustRawAttribute::new(path.clone(), RustRawAttributeArguments::Path)
                        .expect("path attribute"),
                ),
                RustAttribute::Unresolved(
                    RustRawAttribute::new(
                        path.clone(),
                        RustRawAttributeArguments::List(
                            RustTokenSyntax::new("mode = \"stable\"".to_owned())
                                .expect("list arguments"),
                        ),
                    )
                    .expect("list attribute"),
                ),
                RustAttribute::Unresolved(
                    RustRawAttribute::new(
                        path,
                        RustRawAttributeArguments::NameValue(
                            RustSyntaxText::parse_expression("\"stable\"")
                                .expect("name-value expression"),
                        ),
                    )
                    .expect("name-value attribute"),
                ),
            ],
            &crate::work::CancellationProbe::new(),
        )
        .expect("semantic raw attributes");

        assert_eq!(
            attributes.source_syntax(),
            "#[contract_runtime] #[contract_runtime(mode = \"stable\")] #[contract_runtime = \"stable\"]"
        );
    }

    #[test]
    fn raw_name_value_arguments_use_the_validated_expression_owner() {
        let error = RustSyntaxText::parse_expression("let")
            .expect_err("a token stream that is not an expression must fail closed");

        assert!(
            error
                .to_string()
                .contains("invalid Rust syntax text for expression"),
            "{error}"
        );
    }

    #[test]
    fn cfg_attr_nested_values_use_the_complete_attribute_dispatcher() {
        let fixture = AttributeFixture::on_struct(quote::quote! {
            #[cfg_attr(
                unix,
                derive(Clone, serde::Serialize),
                cfg(feature = "fast"),
                unsafe(no_mangle),
                link(name = "contract_native"),
                contract_runtime(flag)
            )]
        });
        let attributes = fixture.parse();
        let RustAttribute::Conditional(RustConditional::CfgAttr {
            attributes: nested, ..
        }) = &attributes.values()[0]
        else {
            panic!("cfg_attr must remain structured");
        };

        assert!(matches!(nested[0], RustAttribute::Derive(_)));
        assert!(matches!(nested[1], RustAttribute::Conditional(_)));
        assert!(matches!(
            nested[2],
            RustAttribute::Export(RustExportAttribute::NoMangle)
        ));
        assert!(matches!(
            nested[3],
            RustAttribute::Linkage(RustLinkageAttribute::Library(_))
        ));
        assert!(matches!(nested[4], RustAttribute::Unresolved(_)));
        assert!(attributes.requires_capability_warning());
    }

    #[test]
    fn malformed_known_attributes_fail_closed() {
        let invalid_attributes = [
            quote::quote!(#[repr()]),
            quote::quote!(#[non_exhaustive(value)]),
            quote::quote!(#[cfg()]),
            quote::quote!(#[cfg_attr(unix)]),
            quote::quote!(#[deprecated(unknown = "value")]),
            quote::quote!(#[must_use("value")]),
            quote::quote!(#[export_name = 1]),
            quote::quote!(#[link(kind = "static")]),
            quote::quote!(#[unsafe()]),
        ];

        for source in invalid_attributes {
            let error = AttributeFixture::on_struct(source)
                .try_parse()
                .expect_err("malformed known attribute must fail");
            assert!(!error.to_string().is_empty());
        }
    }

    #[test]
    fn malformed_derive_is_atomic_and_exposes_no_partial_paths() {
        let valid = AttributeFixture::on_struct(quote::quote! {
            #[derive(Clone)]
        });
        let malformed = AttributeFixture::on_struct(quote::quote! {
            #[derive(serde::Serialize, , Debug)]
        });
        let mut attributes = RustAttributes::default();
        attributes
            .append_derive(&valid.attributes[0], &crate::work::CancellationProbe::new())
            .expect("valid derive");
        let before = attributes.clone();

        let error = attributes
            .append_derive(
                &malformed.attributes[0],
                &crate::work::CancellationProbe::new(),
            )
            .expect_err("malformed derive must fail");

        assert_eq!(attributes, before, "partial derive paths escaped");
        assert!(error.to_string().contains("derive"), "{error}");
    }

    #[test]
    fn canceled_nested_cfg_attribute_conversion_stops_before_commit() {
        let fixture = AttributeFixture::on_struct(quote::quote! {
            #[cfg_attr(unix, derive(Clone, Copy), cfg(feature = "one"), cfg(feature = "two"))]
        });
        let cancellation = crate::work::CancellationProbe::new();
        cancellation.cancel();

        let error = RustAttributes::from_syn(&fixture.attributes, &cancellation)
            .expect_err("canceled attribute conversion must stop");

        assert!(error.is_operation_canceled());
    }

    #[test]
    fn pre_canceled_cfg_predicate_stops_before_parsing_large_token_text() {
        let cancellation = crate::work::CancellationProbe::new();
        cancellation.cancel();
        let source = format!("all({}", "unix,".repeat(16 * 1024));

        let error = RustCfgPredicate::new(source, &cancellation)
            .expect_err("pre-cancellation must precede cfg token parsing");

        assert!(error.is_operation_canceled());
    }
}
