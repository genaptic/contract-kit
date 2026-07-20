use crate::error::SignatureContractKitError;
use crate::languages::rust::parser::source_graph::{RustModuleId, RustModulePath};
use crate::languages::rust::types::primitive_types::Visibility;

#[derive(Clone, Debug, Default)]
pub(crate) struct RustVisibilityConverter;

impl RustVisibilityConverter {
    pub(crate) fn convert_visibility(
        &self,
        visibility: syn::Visibility,
        current_module: &RustModuleId,
    ) -> Result<Visibility, SignatureContractKitError> {
        match visibility {
            syn::Visibility::Public(_) => Ok(Visibility::Public),
            syn::Visibility::Restricted(restricted) => {
                self.convert_restricted(restricted, current_module)
            }
            syn::Visibility::Inherited => Ok(Visibility::Private),
        }
    }

    fn convert_restricted(
        &self,
        restricted: syn::VisRestricted,
        current_module: &RustModuleId,
    ) -> Result<Visibility, SignatureContractKitError> {
        let requested = self.render_restricted(&restricted);
        let path_segments = RustModulePath::semantic_path_segments(&restricted.path);
        let target = self.resolve_path(*restricted.path, current_module, &requested)?;

        if matches!(path_segments.as_slice(), [segment] if segment == "crate") {
            return Ok(Visibility::Crate);
        }
        if target == *current_module {
            return Ok(Visibility::Private);
        }
        if target == current_module.crate_root() {
            return Ok(Visibility::Crate);
        }
        if target.is_strict_ancestor_of(current_module) {
            return Ok(Visibility::Module(target));
        }

        Err(SignatureContractKitError::invalid_restricted_visibility(
            current_module.clone(),
            requested,
            Some(target),
            "the resolved module is not an ancestor of the item module",
        ))
    }

    fn resolve_path(
        &self,
        path: syn::Path,
        current_module: &RustModuleId,
        requested: &str,
    ) -> Result<RustModuleId, SignatureContractKitError> {
        if path.leading_colon.is_some() {
            return Err(SignatureContractKitError::invalid_restricted_visibility(
                current_module.clone(),
                requested,
                None,
                "an absolute leading `::` is not valid in restricted visibility",
            ));
        }

        for segment in &path.segments {
            if !matches!(segment.arguments, syn::PathArguments::None) {
                return Err(SignatureContractKitError::invalid_restricted_visibility(
                    current_module.clone(),
                    requested,
                    None,
                    "restricted visibility paths cannot contain generic arguments",
                ));
            }
        }
        let segments = RustModulePath::semantic_path_segments(&path);

        let Some(first) = segments.first().map(String::as_str) else {
            return Err(SignatureContractKitError::invalid_restricted_visibility(
                current_module.clone(),
                requested,
                None,
                "restricted visibility path must not be empty",
            ));
        };

        let (mut target, mut consumed) = match first {
            "crate" => (current_module.crate_root(), 1),
            "self" => (current_module.clone(), 1),
            "super" => (current_module.clone(), 0),
            _ => {
                return Err(SignatureContractKitError::invalid_restricted_visibility(
                    current_module.clone(),
                    requested,
                    None,
                    "restricted visibility paths must begin with `crate`, `self`, or `super`",
                ));
            }
        };

        while segments
            .get(consumed)
            .is_some_and(|segment| segment == "super")
        {
            target = target.parent().ok_or_else(|| {
                SignatureContractKitError::invalid_restricted_visibility(
                    current_module.clone(),
                    requested,
                    None,
                    "the requested visibility traverses above the crate root",
                )
            })?;
            consumed += 1;
        }

        let mut module_path = target.module_path().segments().to_vec();
        let remaining = &segments[consumed..];
        for (index, segment) in remaining.iter().enumerate() {
            if segment == "self" && index + 1 == remaining.len() {
                continue;
            }
            if matches!(segment.as_str(), "crate" | "self" | "super") {
                return Err(SignatureContractKitError::invalid_restricted_visibility(
                    current_module.clone(),
                    requested,
                    None,
                    format!(
                        "path keyword {segment:?} is not valid at this position in restricted visibility"
                    ),
                ));
            }
            module_path.push(segment.clone());
        }
        target = RustModuleId::new(
            current_module.crate_id().clone(),
            RustModulePath::new(module_path)?,
        );
        Ok(target)
    }

    fn render_restricted(&self, restricted: &syn::VisRestricted) -> String {
        let path = &restricted.path;
        let prefix = if path.leading_colon.is_some() {
            "::"
        } else {
            ""
        };
        let path = format!(
            "{prefix}{}",
            path.segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>()
                .join("::")
        );
        if restricted.in_token.is_some() {
            format!("pub(in {path})")
        } else {
            format!("pub({path})")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RustVisibilityConverter;
    use crate::error::SignatureContractKitError;
    use crate::languages::rust::parser::source_graph::{RustCrateId, RustModuleId, RustModulePath};
    use crate::languages::rust::types::primitive_types::Visibility;

    struct VisibilityFixture {
        current_module: RustModuleId,
    }

    impl VisibilityFixture {
        fn new(crate_id: &str, module_path: &[&str]) -> Self {
            Self {
                current_module: Self::module_id(crate_id, module_path),
            }
        }

        fn module_id(crate_id: &str, module_path: &[&str]) -> RustModuleId {
            RustModuleId::new(
                RustCrateId::new(crate_id, &crate::work::CancellationProbe::new())
                    .expect("valid fixture crate id"),
                RustModulePath::new(
                    module_path
                        .iter()
                        .map(|segment| (*segment).to_owned())
                        .collect(),
                )
                .expect("valid fixture module path"),
            )
        }

        fn convert(&self, source: &str) -> Result<Visibility, SignatureContractKitError> {
            RustVisibilityConverter.convert_visibility(
                syn::parse_str(source).expect("valid visibility syntax"),
                &self.current_module,
            )
        }

        fn convert_inherited(&self) -> Result<Visibility, SignatureContractKitError> {
            RustVisibilityConverter
                .convert_visibility(syn::Visibility::Inherited, &self.current_module)
        }
    }

    #[test]
    fn public_and_private_self_spellings_are_semantically_canonical() {
        let fixture = VisibilityFixture::new("sample", &["outer", "inner"]);

        assert_eq!(
            fixture.convert("pub").expect("public visibility"),
            Visibility::Public
        );
        assert_eq!(
            fixture
                .convert_inherited()
                .expect("inherited private visibility"),
            Visibility::Private
        );
        assert_eq!(
            fixture.convert("pub(self)").expect("self visibility"),
            Visibility::Private
        );
        assert_eq!(
            fixture.convert("pub(in self)").expect("in-self visibility"),
            Visibility::Private
        );
    }

    #[test]
    fn crate_visibility_spellings_are_equivalent() {
        let fixture = VisibilityFixture::new("sample", &["outer", "inner"]);

        assert_eq!(
            fixture.convert("pub(crate)").expect("crate visibility"),
            Visibility::Crate
        );
        assert_eq!(
            fixture
                .convert("pub(in crate)")
                .expect("in-crate visibility"),
            Visibility::Crate
        );

        let crate_root = VisibilityFixture::new("sample", &[]);
        assert_eq!(
            crate_root
                .convert("pub(crate)")
                .expect("crate-root crate visibility"),
            Visibility::Crate
        );
        assert_eq!(
            crate_root
                .convert("pub(in crate)")
                .expect("crate-root in-crate visibility"),
            Visibility::Crate
        );
        assert_eq!(
            crate_root
                .convert("pub(self)")
                .expect("crate-root self visibility"),
            Visibility::Private
        );
    }

    #[test]
    fn parent_visibility_spellings_resolve_to_one_canonical_module() {
        let fixture = VisibilityFixture::new("sample", &["outer", "inner"]);
        let expected = Visibility::Module(VisibilityFixture::module_id("sample", &["outer"]));

        assert_eq!(
            fixture.convert("pub(super)").expect("super visibility"),
            expected
        );
        assert_eq!(
            fixture
                .convert("pub(in super)")
                .expect("in-super visibility"),
            expected
        );
        assert_eq!(
            fixture
                .convert("pub(in crate::outer)")
                .expect("absolute parent visibility"),
            expected
        );
    }

    #[test]
    fn multi_level_relative_and_absolute_ancestor_paths_are_equivalent() {
        let fixture = VisibilityFixture::new("sample", &["outer", "inner", "leaf"]);
        let expected = Visibility::Module(VisibilityFixture::module_id("sample", &["outer"]));

        assert_eq!(
            fixture
                .convert("pub(in super::super)")
                .expect("relative ancestor visibility"),
            expected
        );
        assert_eq!(
            fixture
                .convert("pub(in crate::outer)")
                .expect("absolute ancestor visibility"),
            expected
        );
    }

    #[test]
    fn initial_self_may_precede_repeated_super_segments() {
        let fixture = VisibilityFixture::new("sample", &["outer", "inner", "leaf"]);
        let expected = Visibility::Module(VisibilityFixture::module_id("sample", &["outer"]));

        assert_eq!(
            fixture
                .convert("pub(in self::super::super)")
                .expect("self-qualified repeated-super visibility"),
            expected
        );
        assert_eq!(
            fixture
                .convert("pub(in crate::outer)")
                .expect("crate-qualified ancestor visibility"),
            expected
        );
    }

    #[test]
    fn trailing_self_preserves_the_preceding_module_identity() {
        let fixture = VisibilityFixture::new("sample", &["outer", "inner"]);
        let expected = Visibility::Module(VisibilityFixture::module_id("sample", &["outer"]));

        assert_eq!(
            fixture
                .convert("pub(in crate::outer::self)")
                .expect("absolute trailing-self visibility"),
            expected
        );
        assert_eq!(
            fixture
                .convert("pub(in super::self)")
                .expect("relative trailing-self visibility"),
            expected
        );
    }

    #[test]
    fn identical_relative_spelling_resolves_against_each_items_module() {
        let left = VisibilityFixture::new("sample", &["left", "leaf"]);
        let right = VisibilityFixture::new("sample", &["right", "leaf"]);

        assert_eq!(
            left.convert("pub(super)").expect("left parent"),
            Visibility::Module(VisibilityFixture::module_id("sample", &["left"]))
        );
        assert_eq!(
            right.convert("pub(super)").expect("right parent"),
            Visibility::Module(VisibilityFixture::module_id("sample", &["right"]))
        );
        assert_ne!(
            left.convert("pub(super)").expect("left parent"),
            right.convert("pub(super)").expect("right parent")
        );
    }

    #[test]
    fn raw_and_plain_identifiers_resolve_to_one_semantic_ancestor() {
        let fixture = VisibilityFixture::new("sample", &["outer", "leaf"]);
        let expected = Visibility::Module(VisibilityFixture::module_id("sample", &["outer"]));

        assert_eq!(
            fixture
                .convert("pub(in crate::outer)")
                .expect("plain ancestor visibility"),
            expected
        );
        assert_eq!(
            fixture
                .convert("pub(in crate::r#outer)")
                .expect("raw ancestor visibility"),
            expected
        );

        let keyword = VisibilityFixture::new("sample", &["type", "leaf"]);
        assert_eq!(
            keyword
                .convert("pub(in crate::r#type)")
                .expect("raw keyword ancestor visibility"),
            Visibility::Module(VisibilityFixture::module_id("sample", &["type"],))
        );
    }

    #[test]
    fn identical_restricted_paths_remain_crate_scoped_in_canonical_bytes() {
        let alpha = VisibilityFixture::new("alpha", &["outer", "leaf"])
            .convert("pub(super)")
            .expect("alpha visibility");
        let beta = VisibilityFixture::new("beta", &["outer", "leaf"])
            .convert("pub(super)")
            .expect("beta visibility");

        assert_ne!(alpha, beta);
        assert_ne!(
            serde_json::to_vec(&alpha).expect("alpha canonical visibility"),
            serde_json::to_vec(&beta).expect("beta canonical visibility")
        );
    }

    #[test]
    fn non_ancestor_restriction_retains_requested_spelling_and_resolved_target() {
        let fixture = VisibilityFixture::new("sample", &["left", "leaf"]);
        let error = fixture
            .convert("pub(in crate::right)")
            .expect_err("sibling module is not an ancestor");
        let rendered = error.to_string();

        assert!(rendered.contains("sample::left::leaf"), "{rendered}");
        assert!(rendered.contains("pub(in crate::right)"), "{rendered}");
        assert!(
            rendered.contains("resolved target sample::right"),
            "{rendered}"
        );
        assert!(rendered.contains("ancestor"), "{rendered}");
    }

    #[test]
    fn super_from_crate_root_fails_instead_of_traversing_above_root() {
        let fixture = VisibilityFixture::new("sample", &[]);
        let error = fixture
            .convert("pub(super)")
            .expect_err("crate root has no parent module");
        let rendered = error.to_string();

        assert!(rendered.contains("sample"), "{rendered}");
        assert!(rendered.contains("pub(super)"), "{rendered}");
        assert!(
            rendered.contains("resolved target unavailable"),
            "{rendered}"
        );
        assert!(
            rendered.contains("above") || rendered.contains("parent"),
            "{rendered}"
        );
    }
}
