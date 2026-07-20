use crate::error::SignatureContractKitError;
use crate::languages::rust::parser::source_graph::RustModuleId;
use crate::languages::rust::types::declaration::{RustIdentifier, RustItemKind};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct RustItemId {
    module_id: RustModuleId,
    kind: RustItemKind,
    name: String,
    occurrence: usize,
}

impl RustItemId {
    pub(crate) fn new(
        module_id: RustModuleId,
        kind: RustItemKind,
        name: impl Into<String>,
    ) -> Self {
        Self {
            module_id,
            kind,
            name: name.into(),
            occurrence: 1,
        }
    }

    pub(crate) fn module_id(&self) -> &RustModuleId {
        &self.module_id
    }

    pub(crate) fn kind(&self) -> RustItemKind {
        self.kind
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    fn validate_name(&self) -> Result<(), SignatureContractKitError> {
        let role = match self.kind {
            RustItemKind::Constant => Some("constant name"),
            RustItemKind::Enum => Some("enum name"),
            RustItemKind::Function => Some("function name"),
            RustItemKind::Module => Some("module name"),
            RustItemKind::Static => Some("static name"),
            RustItemKind::Struct => Some("struct name"),
            RustItemKind::Trait => Some("trait name"),
            RustItemKind::TraitAlias => Some("trait alias name"),
            RustItemKind::TypeAlias => Some("type alias name"),
            RustItemKind::Union => Some("union name"),
            RustItemKind::ExternCrate if self.name != "self" => Some("extern crate name"),
            RustItemKind::Reexport if self.name != "_" => Some("re-export name"),
            RustItemKind::ExternCrate
            | RustItemKind::ForeignModule
            | RustItemKind::Implementation
            | RustItemKind::Macro
            | RustItemKind::Reexport => None,
        };
        if let Some(role) = role {
            RustIdentifier::new(self.name.clone(), role)?;
        }
        Ok(())
    }

    pub(crate) fn render(&self) -> String {
        let crate_id = self.module_id.crate_id().as_str();
        let module_path = self.module_id.module_path().segments();
        let kind = self.kind.as_str();
        let occurrence = self.occurrence.to_string();
        let mut rendered = String::from("rust:v2:crate:");

        rendered.push_str(&crate_id.len().to_string());
        rendered.push(':');
        rendered.push_str(crate_id);
        rendered.push_str(":modules:");
        rendered.push_str(&module_path.len().to_string());
        for segment in module_path {
            rendered.push(':');
            rendered.push_str(&segment.len().to_string());
            rendered.push(':');
            rendered.push_str(segment);
        }
        rendered.push_str(":kind:");
        rendered.push_str(&kind.len().to_string());
        rendered.push(':');
        rendered.push_str(kind);
        rendered.push_str(":name:");
        rendered.push_str(&self.name.len().to_string());
        rendered.push(':');
        rendered.push_str(&self.name);
        rendered.push_str(":occurrence:");
        rendered.push_str(&occurrence.len().to_string());
        rendered.push(':');
        rendered.push_str(&occurrence);

        rendered
    }

    pub(crate) fn diagnostic_path(&self) -> String {
        format!(
            "{}::{} ({}, occurrence {})",
            self.module_id, self.name, self.kind, self.occurrence
        )
    }
}

#[derive(Default)]
pub(crate) struct RustItemIdAllocator {
    occurrences: BTreeMap<RustItemId, usize>,
}

impl RustItemIdAllocator {
    pub(crate) fn allocate(
        &mut self,
        mut id: RustItemId,
    ) -> Result<RustItemId, SignatureContractKitError> {
        id.occurrence = 1;
        id.validate_name()?;
        let prior_occurrences = self.occurrences.get(&id).copied().unwrap_or_default();
        if prior_occurrences > 0 && !id.kind.is_structurally_repeatable() {
            return Err(SignatureContractKitError::conversion_failed(format!(
                "duplicate Rust {} identity: {}",
                id.kind,
                id.render()
            )));
        }

        let occurrence = prior_occurrences.checked_add(1).ok_or_else(|| {
            SignatureContractKitError::conversion_failed("Rust item occurrence count is exhausted")
        })?;
        self.occurrences.insert(id.clone(), occurrence);
        id.occurrence = occurrence;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{RustItemId, RustItemIdAllocator};
    use crate::files::CatalogPath;
    use crate::languages::rust::parser::source_graph::{RustCrateId, RustModuleId, RustModulePath};
    use crate::languages::rust::types::attributes::RustAttributes;
    use crate::languages::rust::types::base_type::BaseType;
    use crate::languages::rust::types::callable_type::RustCallableSignature;
    use crate::languages::rust::types::declaration::{
        ModuleDeclarationType, RustDeclaration, RustItemKind,
    };
    use crate::languages::rust::types::function_type::FunctionType;
    use crate::languages::rust::types::primitive_types::{
        RustFunctionParameter, RustType, Visibility,
    };
    use crate::languages::rust::types::syntax_text::RustSyntaxText;

    #[derive(Clone)]
    struct IdentityFixture {
        crate_id: &'static str,
        file: &'static str,
        module_path: &'static [&'static str],
        kind: RustItemKind,
        name: &'static str,
    }

    impl IdentityFixture {
        fn function(name: &'static str) -> Self {
            Self {
                crate_id: "example",
                file: "src/lib.rs",
                module_path: &[],
                kind: RustItemKind::Function,
                name,
            }
        }

        fn with_crate_id(mut self, crate_id: &'static str) -> Self {
            self.crate_id = crate_id;
            self
        }

        fn with_file(mut self, file: &'static str) -> Self {
            self.file = file;
            self
        }

        fn with_module_path(mut self, module_path: &'static [&'static str]) -> Self {
            self.module_path = module_path;
            self
        }

        fn with_kind(mut self, kind: RustItemKind) -> Self {
            self.kind = kind;
            self
        }

        fn module_id(&self) -> RustModuleId {
            let crate_id = RustCrateId::new(self.crate_id, &crate::work::CancellationProbe::new())
                .expect("fixture crate identity should be valid");
            let module_path = RustModulePath::new(
                self.module_path
                    .iter()
                    .map(|segment| (*segment).to_owned())
                    .collect(),
            )
            .expect("fixture module path should be valid");

            RustModuleId::new(crate_id, module_path)
        }

        fn base(&self, name: &str) -> BaseType {
            BaseType::new(
                name.to_owned(),
                Visibility::Private,
                CatalogPath::new(self.file).expect("fixture file should be valid"),
                self.module_id(),
                RustAttributes::default(),
            )
        }

        fn id(&self) -> RustItemId {
            RustItemId::new(self.module_id(), self.kind, self.name)
        }
    }

    #[test]
    fn identity_includes_crate_and_module_but_not_physical_file() {
        let base = IdentityFixture::function("answer");

        assert_ne!(base.id(), base.clone().with_crate_id("other").id());
        assert_ne!(base.id(), base.clone().with_module_path(&["nested"]).id());
        let original = base.id();
        let relocated = base.with_file("src/relocated.rs").id();
        assert_eq!(
            original, relocated,
            "physical paths are locator metadata, not Rust semantic identity"
        );
        assert_eq!(original.render(), relocated.render());
    }

    #[test]
    fn same_physical_file_in_two_logical_contexts_has_distinct_identity() {
        let base = IdentityFixture::function("Widget").with_kind(RustItemKind::Struct);

        let first = base
            .clone()
            .with_crate_id("first")
            .with_module_path(&["api"])
            .id();
        let second = base.with_crate_id("second").with_module_path(&["api"]).id();

        assert_ne!(first, second);
        assert_ne!(first.render(), second.render());
    }

    #[test]
    fn render_is_versioned_and_component_length_delimited() {
        let id = IdentityFixture::function("main")
            .with_crate_id("alpha")
            .with_module_path(&["framework", "api"])
            .id();

        assert_eq!(
            id.render(),
            "rust:v2:crate:5:alpha:modules:2:9:framework:3:api:kind:8:function:name:4:main:occurrence:1:1"
        );
    }

    #[test]
    fn diagnostic_path_is_readable_without_reusing_wire_identity_encoding() {
        let id = IdentityFixture::function("Widget")
            .with_crate_id("sample")
            .with_module_path(&["models", "type"])
            .with_kind(RustItemKind::Struct)
            .id();

        assert_eq!(
            id.diagnostic_path(),
            "sample::models::type::Widget (struct, occurrence 1)"
        );
        assert!(!id.diagnostic_path().contains("rust:v2:"));
    }

    #[test]
    fn ordinary_main_uses_the_function_family() {
        let fixture = IdentityFixture::function("main").with_module_path(&["framework"]);
        let declaration = RustDeclaration::Function(FunctionType::new(fixture.base("main")));
        let kind = declaration.kind();
        let id = fixture.with_kind(kind).id();

        assert_eq!(id.kind(), RustItemKind::Function);
        assert!(id.render().contains(":kind:8:function:name:4:main:"));
    }

    #[test]
    fn parameter_patterns_do_not_participate_in_structural_identity() {
        let fixture = IdentityFixture::function("submit");
        let first = RustDeclaration::Function(
            FunctionType::new(fixture.base("submit")).with_callable_signature(
                RustCallableSignature::builder()
                    .with_parameters(vec![RustFunctionParameter::new(
                        Some(RustSyntaxText::parse_pattern("request").expect("pattern")),
                        RustType::Bool,
                    )])
                    .build(),
            ),
        );
        let second = RustDeclaration::Function(
            FunctionType::new(fixture.base("submit")).with_callable_signature(
                RustCallableSignature::builder()
                    .with_parameters(vec![RustFunctionParameter::new(
                        Some(
                            RustSyntaxText::parse_pattern("(value, _)")
                                .expect("destructuring pattern"),
                        ),
                        RustType::Bool,
                    )])
                    .build(),
            ),
        );

        let first = fixture.clone().with_kind(first.kind()).id();
        let second = fixture.with_kind(second.kind()).id();

        assert_eq!(first, second);
        assert_eq!(first.render(), second.render());
    }

    #[test]
    fn module_declarations_have_unique_signature_identity() {
        let fixture = IdentityFixture::function("transport");
        let declaration = RustDeclaration::Module(
            ModuleDeclarationType::new(fixture.base("transport"), false, None)
                .expect("module declaration fixture"),
        );
        let kind = declaration.kind();
        let base = fixture.with_kind(kind).id();
        let mut allocator = RustItemIdAllocator::default();
        let first = allocator
            .allocate(base.clone())
            .expect("first module declaration");
        let error = allocator
            .allocate(base)
            .expect_err("duplicate module identity must fail closed");

        assert_eq!(kind, RustItemKind::Module);
        assert_eq!(first.occurrence, 1);
        assert!(first.render().contains(":kind:6:module:name:9:transport:"));
        assert!(error.to_string().contains("duplicate Rust module identity"));
    }

    #[test]
    fn allocator_rejects_noncanonical_ordinary_declaration_names() {
        let fixture = IdentityFixture::function("r#match");
        let mut allocator = RustItemIdAllocator::default();
        let error = allocator
            .allocate(fixture.id())
            .expect_err("raw-prefixed semantic identities must fail closed");

        assert!(
            error
                .to_string()
                .contains("invalid Rust identifier for function name"),
            "{error}"
        );
    }

    #[test]
    fn allocator_ordinals_only_repeatable_structural_families() {
        for kind in [
            RustItemKind::Implementation,
            RustItemKind::Macro,
            RustItemKind::ForeignModule,
        ] {
            let mut allocator = RustItemIdAllocator::default();
            let base = IdentityFixture::function("repeated").with_kind(kind).id();

            let first = allocator
                .allocate(base.clone())
                .expect("first repeatable identity");
            let second = allocator
                .allocate(base)
                .expect("second repeatable identity");

            assert_eq!(first.occurrence, 1);
            assert_eq!(second.occurrence, 2);
            assert!(first.render().ends_with(":occurrence:1:1"));
            assert!(second.render().ends_with(":occurrence:1:2"));
        }
    }

    #[test]
    fn allocator_rejects_duplicate_unique_declarations() {
        let mut allocator = RustItemIdAllocator::default();
        let id = IdentityFixture::function("answer").id();

        allocator
            .allocate(id.clone())
            .expect("first function identity");
        let error = allocator
            .allocate(id)
            .expect_err("duplicate function identity must fail closed");

        assert!(
            error
                .to_string()
                .contains("duplicate Rust function identity")
        );
    }

    #[test]
    fn rendered_identity_cannot_confuse_name_text_with_occurrence_suffix() {
        let literal_suffix = IdentityFixture::function("call#2")
            .with_kind(RustItemKind::Macro)
            .id();
        let repeated = IdentityFixture::function("call")
            .with_kind(RustItemKind::Macro)
            .id();
        let mut allocator = RustItemIdAllocator::default();
        allocator
            .allocate(repeated.clone())
            .expect("first macro identity");
        let repeated = allocator.allocate(repeated).expect("second macro identity");

        assert_ne!(literal_suffix.render(), repeated.render());
        assert!(
            literal_suffix
                .render()
                .contains(":name:6:call#2:occurrence:1:1")
        );
        assert!(repeated.render().contains(":name:4:call:occurrence:1:2"));
    }

    #[test]
    fn allocation_is_deterministic_across_distinct_input_order() {
        let fixtures = [
            IdentityFixture::function("one"),
            IdentityFixture::function("two").with_module_path(&["nested"]),
            IdentityFixture::function("three").with_crate_id("other"),
        ];

        let mut forward_allocator = RustItemIdAllocator::default();
        let forward = fixtures
            .iter()
            .map(|fixture| {
                forward_allocator
                    .allocate(fixture.id())
                    .expect("distinct forward identity")
                    .render()
            })
            .collect::<BTreeSet<_>>();

        let mut reverse_allocator = RustItemIdAllocator::default();
        let reverse = fixtures
            .iter()
            .rev()
            .map(|fixture| {
                reverse_allocator
                    .allocate(fixture.id())
                    .expect("distinct reverse identity")
                    .render()
            })
            .collect::<BTreeSet<_>>();

        assert_eq!(forward, reverse);
    }
}
