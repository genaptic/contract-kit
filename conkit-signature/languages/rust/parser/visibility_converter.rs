use crate::languages::rust::types::primitive_types::Visibility;

#[derive(Clone, Debug, Default)]
pub(crate) struct RustVisibilityConverter;

impl RustVisibilityConverter {
    pub(crate) fn convert_visibility(&self, visibility: syn::Visibility) -> Visibility {
        match visibility {
            syn::Visibility::Public(_) => Visibility::Public,
            syn::Visibility::Restricted(restricted) if restricted.path.is_ident("crate") => {
                Visibility::PublicCrate
            }
            syn::Visibility::Restricted(restricted) => {
                let path = restricted.path;
                Visibility::Restricted(quote::quote!(#path).to_string())
            }
            syn::Visibility::Inherited => Visibility::Private,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RustVisibilityConverter;
    use crate::languages::rust::types::primitive_types::Visibility;

    #[test]
    fn converts_public_and_private_visibility() {
        let converter = RustVisibilityConverter;

        assert_eq!(
            converter.convert_visibility(syn::parse_str("pub").expect("visibility")),
            Visibility::Public
        );
        assert_eq!(
            converter.convert_visibility(syn::Visibility::Inherited),
            Visibility::Private
        );
    }

    #[test]
    fn converts_crate_and_restricted_visibility() {
        let converter = RustVisibilityConverter;

        assert_eq!(
            converter.convert_visibility(syn::parse_str("pub(crate)").expect("visibility")),
            Visibility::PublicCrate
        );
        assert_eq!(
            converter.convert_visibility(syn::parse_str("pub(super)").expect("visibility")),
            Visibility::Restricted("super".to_owned())
        );
    }
}
