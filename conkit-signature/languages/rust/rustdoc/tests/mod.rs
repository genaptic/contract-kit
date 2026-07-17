mod artifact;
mod declarations;
mod modules;
mod provenance;
mod types;

use std::collections::{BTreeSet, HashMap};

use super::artifact::{
    CompilerSourcePath, CompilerSourceProvenance, RUST_COMPILER_ARTIFACT_SCHEMA_VERSION,
    RustCompilerArtifact, RustCompilerArtifactFailure, RustCompilerCrate, RustCompilerExtraction,
};
use super::index::{CompilerInventory, RustdocIndex};
use super::provenance::{CompilerSourceFileIndex, CompilerSourceIndex};
use crate::api::{
    CheckMode, CheckRequest, ContractScope, GenerateDocument, GenerateRequest, GenerateTarget,
    ReportRequest, ResolveSketchesRequest, RustCrateKind, RustCrateRoot, RustExtractionInput,
    SignatureContractKit,
};
use crate::files::{CatalogPath, FileCatalog};
use crate::languages::rust::types::associated_item::RustAssociatedItem;
use crate::languages::rust::types::attributes::{RustAttribute, RustReprHint};
use crate::languages::rust::types::declaration::RustDeclaration;
use crate::languages::rust::types::impl_type::RustImplementedTrait;
use crate::languages::rust::types::primitive_types::Visibility;
use crate::limits::RustExtractionLimits;
use crate::work::CancellationProbe;

struct CompilerArtifactFixture;

impl CompilerArtifactFixture {
    fn source() -> &'static [u8] {
        b"make_answer!();\n"
    }

    fn supported_surface_source() -> &'static [u8] {
        br#"#[must_use = "consume the value"]
pub fn generic_value<T>() -> T {
    panic!()
}

#[repr(C)]
pub struct Record<T> {
    pub value: T,
}

#[non_exhaustive]
pub enum State {
    Ready,
    Code(u8),
}

pub union Bits {
    pub byte: u8,
}

pub trait Service<T> {
    type Output;
    const READY: bool;
    fn ready(&self) -> bool;
}

use crate as sample;

impl<T> Record<T> {
    pub fn defaulted() -> Self {
        panic!()
    }
    pub const ZERO: u8 = 0;
}

impl<T> sample::Service<T> for Record<T> {
    type Output = u8;
    const READY: bool = true;

    fn ready(&self) -> bool {
        Self::READY
    }
}
"#
    }

    fn rustdoc_document() -> rustdoc_types::Crate {
        let root_id = rustdoc_types::Id(0);
        let function_id = rustdoc_types::Id(1);
        let root = rustdoc_types::Item {
            id: root_id,
            crate_id: 0,
            name: Some("sample".to_owned()),
            span: None,
            visibility: rustdoc_types::Visibility::Public,
            docs: None,
            links: HashMap::new(),
            attrs: Vec::new(),
            deprecation: None,
            stability: None,
            const_stability: None,
            inner: rustdoc_types::ItemEnum::Module(rustdoc_types::Module {
                is_crate: true,
                items: vec![function_id],
                is_stripped: false,
            }),
        };
        let function = rustdoc_types::Item {
            id: function_id,
            crate_id: 0,
            name: Some("answer".to_owned()),
            span: None,
            visibility: rustdoc_types::Visibility::Public,
            docs: None,
            links: HashMap::new(),
            attrs: Vec::new(),
            deprecation: None,
            stability: None,
            const_stability: None,
            inner: rustdoc_types::ItemEnum::Function(rustdoc_types::Function {
                sig: rustdoc_types::FunctionSignature {
                    inputs: Vec::new(),
                    output: Some(rustdoc_types::Type::Primitive("u8".to_owned())),
                    is_c_variadic: false,
                },
                generics: rustdoc_types::Generics {
                    params: Vec::new(),
                    where_predicates: Vec::new(),
                },
                header: rustdoc_types::FunctionHeader {
                    is_const: false,
                    is_unsafe: false,
                    is_async: false,
                    abi: rustdoc_types::Abi::Rust,
                },
                has_body: true,
                default_unstable: None,
            }),
        };
        let mut index = HashMap::new();
        index.insert(root_id, root);
        index.insert(function_id, function);
        let mut paths = HashMap::new();
        paths.insert(
            function_id,
            rustdoc_types::ItemSummary {
                crate_id: 0,
                path: vec!["sample".to_owned(), "answer".to_owned()],
                kind: rustdoc_types::ItemKind::Function,
            },
        );
        rustdoc_types::Crate {
            root: root_id,
            crate_version: Some("0.1.0".to_owned()),
            includes_private: false,
            index,
            paths,
            external_crates: HashMap::new(),
            target: rustdoc_types::Target {
                triple: "x86_64-unknown-linux-gnu".to_owned(),
                target_features: Vec::new(),
            },
            format_version: rustdoc_types::FORMAT_VERSION,
        }
    }

    fn supported_surface_document() -> rustdoc_types::Crate {
        let mut document = Self::rustdoc_document();
        let empty_generics = rustdoc_types::Generics {
            params: Vec::new(),
            where_predicates: Vec::new(),
        };
        let type_parameter = rustdoc_types::GenericParamDef {
            name: "T".to_owned(),
            kind: rustdoc_types::GenericParamDefKind::Type {
                bounds: Vec::new(),
                default: None,
                is_synthetic: false,
            },
        };
        let generic_metadata = rustdoc_types::Generics {
            params: vec![type_parameter.clone()],
            where_predicates: Vec::new(),
        };
        let primitive = |name: &str| rustdoc_types::Type::Primitive(name.to_owned());
        let generic_arguments =
            |value: rustdoc_types::Type| rustdoc_types::GenericArgs::AngleBracketed {
                args: vec![rustdoc_types::GenericArg::Type(value)],
                constraints: Vec::new(),
            };
        let function = |inputs: Vec<(String, rustdoc_types::Type)>,
                        output: Option<rustdoc_types::Type>,
                        generics: rustdoc_types::Generics,
                        has_body: bool| {
            rustdoc_types::ItemEnum::Function(rustdoc_types::Function {
                sig: rustdoc_types::FunctionSignature {
                    inputs,
                    output,
                    is_c_variadic: false,
                },
                generics,
                header: rustdoc_types::FunctionHeader {
                    is_const: false,
                    is_unsafe: false,
                    is_async: false,
                    abi: rustdoc_types::Abi::Rust,
                },
                has_body,
                default_unstable: None,
            })
        };

        let function_id = rustdoc_types::Id(1);
        let record_id = rustdoc_types::Id(2);
        let record_field_id = rustdoc_types::Id(3);
        let state_id = rustdoc_types::Id(4);
        let ready_variant_id = rustdoc_types::Id(5);
        let code_variant_id = rustdoc_types::Id(6);
        let code_field_id = rustdoc_types::Id(7);
        let bits_id = rustdoc_types::Id(8);
        let bits_field_id = rustdoc_types::Id(9);
        let service_id = rustdoc_types::Id(10);
        let inherent_impl_id = rustdoc_types::Id(11);
        let defaulted_id = rustdoc_types::Id(12);
        let zero_id = rustdoc_types::Id(13);
        let trait_impl_id = rustdoc_types::Id(15);
        let impl_output_id = rustdoc_types::Id(16);
        let impl_ready_constant_id = rustdoc_types::Id(17);
        let impl_ready_method_id = rustdoc_types::Id(18);
        let trait_output_id = rustdoc_types::Id(20);
        let trait_ready_constant_id = rustdoc_types::Id(21);
        let trait_ready_method_id = rustdoc_types::Id(22);

        let mut generic_function = Self::public_item(
            function_id.0,
            Some("generic_value"),
            function(
                Vec::new(),
                Some(rustdoc_types::Type::Generic("T".to_owned())),
                generic_metadata.clone(),
                true,
            ),
        );
        generic_function.attrs = vec![rustdoc_types::Attribute::MustUse {
            reason: Some("consume the value".to_owned()),
        }];

        let mut record = Self::public_item(
            record_id.0,
            Some("Record"),
            rustdoc_types::ItemEnum::Struct(rustdoc_types::Struct {
                kind: rustdoc_types::StructKind::Plain {
                    fields: vec![record_field_id],
                    has_stripped_fields: false,
                },
                generics: generic_metadata.clone(),
                impls: vec![inherent_impl_id, trait_impl_id],
            }),
        );
        record.attrs = vec![rustdoc_types::Attribute::Repr(
            rustdoc_types::AttributeRepr {
                kind: rustdoc_types::ReprKind::C,
                align: None,
                packed: None,
                int: None,
            },
        )];
        let record_field = Self::public_item(
            record_field_id.0,
            Some("value"),
            rustdoc_types::ItemEnum::StructField(rustdoc_types::Type::Generic("T".to_owned())),
        );

        let mut state = Self::public_item(
            state_id.0,
            Some("State"),
            rustdoc_types::ItemEnum::Enum(rustdoc_types::Enum {
                generics: empty_generics.clone(),
                has_stripped_variants: false,
                variants: vec![ready_variant_id, code_variant_id],
                impls: Vec::new(),
            }),
        );
        state.attrs = vec![rustdoc_types::Attribute::NonExhaustive];
        let ready_variant = Self::public_item(
            ready_variant_id.0,
            Some("Ready"),
            rustdoc_types::ItemEnum::Variant(rustdoc_types::Variant {
                kind: rustdoc_types::VariantKind::Plain,
                discriminant: None,
            }),
        );
        let code_variant = Self::public_item(
            code_variant_id.0,
            Some("Code"),
            rustdoc_types::ItemEnum::Variant(rustdoc_types::Variant {
                kind: rustdoc_types::VariantKind::Tuple(vec![Some(code_field_id)]),
                discriminant: None,
            }),
        );
        let mut code_field = Self::public_item(
            code_field_id.0,
            None,
            rustdoc_types::ItemEnum::StructField(primitive("u8")),
        );
        code_field.visibility = rustdoc_types::Visibility::Default;

        let bits = Self::public_item(
            bits_id.0,
            Some("Bits"),
            rustdoc_types::ItemEnum::Union(rustdoc_types::Union {
                generics: empty_generics.clone(),
                has_stripped_fields: false,
                fields: vec![bits_field_id],
                impls: Vec::new(),
            }),
        );
        let bits_field = Self::public_item(
            bits_field_id.0,
            Some("byte"),
            rustdoc_types::ItemEnum::StructField(primitive("u8")),
        );

        let service = Self::public_item(
            service_id.0,
            Some("Service"),
            rustdoc_types::ItemEnum::Trait(rustdoc_types::Trait {
                is_auto: false,
                is_unsafe: false,
                is_dyn_compatible: true,
                items: vec![
                    trait_output_id,
                    trait_ready_constant_id,
                    trait_ready_method_id,
                ],
                generics: generic_metadata.clone(),
                bounds: Vec::new(),
                implementations: vec![trait_impl_id],
            }),
        );
        let mut trait_output = Self::public_item(
            trait_output_id.0,
            Some("Output"),
            rustdoc_types::ItemEnum::AssocType {
                generics: empty_generics.clone(),
                bounds: Vec::new(),
                type_: None,
                default_unstable: None,
            },
        );
        trait_output.visibility = rustdoc_types::Visibility::Default;
        let mut trait_ready_constant = Self::public_item(
            trait_ready_constant_id.0,
            Some("READY"),
            rustdoc_types::ItemEnum::AssocConst {
                type_: primitive("bool"),
                value: None,
                default_unstable: None,
            },
        );
        trait_ready_constant.visibility = rustdoc_types::Visibility::Default;
        let mut trait_ready_method = Self::public_item(
            trait_ready_method_id.0,
            Some("ready"),
            function(
                vec![(
                    "&self".to_owned(),
                    rustdoc_types::Type::BorrowedRef {
                        lifetime: None,
                        is_mutable: false,
                        type_: Box::new(rustdoc_types::Type::Generic("Self".to_owned())),
                    },
                )],
                Some(primitive("bool")),
                empty_generics.clone(),
                false,
            ),
        );
        trait_ready_method.visibility = rustdoc_types::Visibility::Default;

        let record_generic = Self::resolved_type(
            record_id.0,
            "Record",
            Some(generic_arguments(rustdoc_types::Type::Generic(
                "T".to_owned(),
            ))),
        );
        let inherent_impl = Self::public_item(
            inherent_impl_id.0,
            None,
            rustdoc_types::ItemEnum::Impl(rustdoc_types::Impl {
                is_unsafe: false,
                generics: generic_metadata.clone(),
                provided_trait_methods: Vec::new(),
                trait_: None,
                for_: record_generic,
                items: vec![defaulted_id, zero_id],
                is_negative: false,
                is_synthetic: false,
                blanket_impl: None,
            }),
        );
        let defaulted = Self::public_item(
            defaulted_id.0,
            Some("defaulted"),
            function(
                Vec::new(),
                Some(rustdoc_types::Type::Generic("Self".to_owned())),
                empty_generics.clone(),
                true,
            ),
        );
        let zero = Self::public_item(
            zero_id.0,
            Some("ZERO"),
            rustdoc_types::ItemEnum::AssocConst {
                type_: primitive("u8"),
                value: Some("0".to_owned()),
                default_unstable: None,
            },
        );

        let trait_record = Self::resolved_type(
            record_id.0,
            "Record",
            Some(generic_arguments(rustdoc_types::Type::Generic(
                "T".to_owned(),
            ))),
        );
        let service_path = rustdoc_types::Path {
            path: "Service".to_owned(),
            id: service_id,
            args: Some(Box::new(generic_arguments(rustdoc_types::Type::Generic(
                "T".to_owned(),
            )))),
        };
        let trait_impl = Self::public_item(
            trait_impl_id.0,
            None,
            rustdoc_types::ItemEnum::Impl(rustdoc_types::Impl {
                is_unsafe: false,
                generics: generic_metadata,
                provided_trait_methods: Vec::new(),
                trait_: Some(service_path),
                for_: trait_record,
                items: vec![impl_output_id, impl_ready_constant_id, impl_ready_method_id],
                is_negative: false,
                is_synthetic: false,
                blanket_impl: None,
            }),
        );
        let mut impl_output = Self::public_item(
            impl_output_id.0,
            Some("Output"),
            rustdoc_types::ItemEnum::AssocType {
                generics: empty_generics.clone(),
                bounds: Vec::new(),
                type_: Some(primitive("u8")),
                default_unstable: None,
            },
        );
        impl_output.visibility = rustdoc_types::Visibility::Default;
        let mut impl_ready_constant = Self::public_item(
            impl_ready_constant_id.0,
            Some("READY"),
            rustdoc_types::ItemEnum::AssocConst {
                type_: primitive("bool"),
                value: Some("true".to_owned()),
                default_unstable: None,
            },
        );
        impl_ready_constant.visibility = rustdoc_types::Visibility::Default;
        let mut impl_ready_method = Self::public_item(
            impl_ready_method_id.0,
            Some("ready"),
            function(
                vec![(
                    "&self".to_owned(),
                    rustdoc_types::Type::BorrowedRef {
                        lifetime: None,
                        is_mutable: false,
                        type_: Box::new(rustdoc_types::Type::Generic("Self".to_owned())),
                    },
                )],
                Some(primitive("bool")),
                empty_generics,
                true,
            ),
        );
        impl_ready_method.visibility = rustdoc_types::Visibility::Default;

        Self::set_root_items(
            &mut document,
            vec![function_id, record_id, state_id, bits_id, service_id],
        );
        document.index.extend([
            (function_id, generic_function),
            (record_id, record),
            (record_field_id, record_field),
            (state_id, state),
            (ready_variant_id, ready_variant),
            (code_variant_id, code_variant),
            (code_field_id, code_field),
            (bits_id, bits),
            (bits_field_id, bits_field),
            (service_id, service),
            (inherent_impl_id, inherent_impl),
            (defaulted_id, defaulted),
            (zero_id, zero),
            (trait_impl_id, trait_impl),
            (impl_output_id, impl_output),
            (impl_ready_constant_id, impl_ready_constant),
            (impl_ready_method_id, impl_ready_method),
            (trait_output_id, trait_output),
            (trait_ready_constant_id, trait_ready_constant),
            (trait_ready_method_id, trait_ready_method),
        ]);
        Self::add_path(
            &mut document,
            record_id.0,
            "Record",
            rustdoc_types::ItemKind::Struct,
        );
        Self::add_path(
            &mut document,
            state_id.0,
            "State",
            rustdoc_types::ItemKind::Enum,
        );
        Self::add_path(
            &mut document,
            bits_id.0,
            "Bits",
            rustdoc_types::ItemKind::Union,
        );
        Self::add_path(
            &mut document,
            service_id.0,
            "Service",
            rustdoc_types::ItemKind::Trait,
        );
        document
    }

    fn artifact() -> RustCompilerArtifact {
        Self::artifact_for(Self::rustdoc_document())
    }

    fn artifact_for(document: rustdoc_types::Crate) -> RustCompilerArtifact {
        let mut source_paths = document
            .index
            .values()
            .filter(|item| item.crate_id == 0)
            .map(|item| CompilerSourcePath {
                rustdoc_item_id: item.id.0,
                provenance: CompilerSourceProvenance::CompilerGenerated {
                    crate_root: CatalogPath::new("lib.rs").expect("fixture crate root"),
                },
            })
            .collect::<Vec<_>>();
        source_paths.sort_by_key(|mapping| mapping.rustdoc_item_id);
        RustCompilerArtifact {
            schema_version: RUST_COMPILER_ARTIFACT_SCHEMA_VERSION,
            extractor_version: "conkit-host/1".to_owned(),
            compiler_version: "rustc 1.97.0".to_owned(),
            rustdoc_format_version: rustdoc_types::FORMAT_VERSION,
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            features: vec!["zeta".to_owned(), "alpha".to_owned(), "alpha".to_owned()],
            cfg_values: vec!["unix".to_owned(), "target_pointer_width=64".to_owned()],
            crates: vec![RustCompilerCrate {
                id: "sample".to_owned(),
                package: "sample".to_owned(),
                target: "sample".to_owned(),
                root: CatalogPath::new("lib.rs").expect("fixture root"),
                root_item_id: 0,
                kind: RustCrateKind::Library,
            }],
            rustdoc_json: serde_json::to_vec(&document).expect("fixture rustdoc JSON"),
            source_paths,
        }
    }

    fn exact_artifact() -> RustCompilerArtifact {
        let mut document = Self::rustdoc_document();
        document
            .index
            .get_mut(&rustdoc_types::Id(1))
            .expect("fixture function")
            .span = Some(rustdoc_types::Span {
            filename: "lib.rs".into(),
            begin: (1, 1),
            end: (1, 15),
        });
        let mut artifact = Self::artifact_for(document);
        Self::set_exact_range(
            &mut artifact,
            1,
            0,
            u64::try_from(Self::source().len() - 1).expect("fixture source length"),
        );
        artifact
    }

    fn set_exact_range(
        artifact: &mut RustCompilerArtifact,
        item_id: u32,
        byte_start: u64,
        byte_end: u64,
    ) {
        Self::set_provenance(
            artifact,
            item_id,
            CompilerSourceProvenance::Exact {
                file: CatalogPath::new("lib.rs").expect("fixture source path"),
                byte_start,
                byte_end,
            },
        );
    }

    fn set_provenance(
        artifact: &mut RustCompilerArtifact,
        item_id: u32,
        provenance: CompilerSourceProvenance,
    ) {
        artifact
            .source_paths
            .iter_mut()
            .find(|mapping| mapping.rustdoc_item_id == item_id)
            .expect("fixture source mapping")
            .provenance = provenance;
    }

    fn sources() -> FileCatalog {
        Self::sources_from(Self::source())
    }

    fn sources_from(source: &[u8]) -> FileCatalog {
        let mut files = FileCatalog::new();
        files
            .insert(
                CatalogPath::new("lib.rs").expect("fixture path"),
                source.to_vec(),
            )
            .expect("insert fixture source");
        files
    }

    fn signature_section(document: &[u8]) -> &str {
        let document = std::str::from_utf8(document).expect("contract UTF-8");
        let start = document.find("signatures:\n").expect("signature section");
        let end = document[start..]
            .find("\nsketches:")
            .map(|offset| start + offset)
            .unwrap_or(document.len());
        &document[start..end]
    }

    fn allowed_files() -> BTreeSet<CatalogPath> {
        BTreeSet::from([CatalogPath::new("lib.rs").expect("fixture path")])
    }

    fn public_item(
        id: u32,
        name: Option<&str>,
        inner: rustdoc_types::ItemEnum,
    ) -> rustdoc_types::Item {
        rustdoc_types::Item {
            id: rustdoc_types::Id(id),
            crate_id: 0,
            name: name.map(ToOwned::to_owned),
            span: None,
            visibility: rustdoc_types::Visibility::Public,
            docs: None,
            links: HashMap::new(),
            attrs: Vec::new(),
            deprecation: None,
            stability: None,
            const_stability: None,
            inner,
        }
    }

    fn module_item(id: u32, name: &str, items: Vec<rustdoc_types::Id>) -> rustdoc_types::Item {
        Self::public_item(
            id,
            Some(name),
            rustdoc_types::ItemEnum::Module(rustdoc_types::Module {
                is_crate: false,
                items,
                is_stripped: false,
            }),
        )
    }

    fn module_cycle(node_count: u32) -> rustdoc_types::Crate {
        let mut document = Self::rustdoc_document();
        let ids = (10..10 + node_count)
            .map(rustdoc_types::Id)
            .collect::<Vec<_>>();
        for (index, id) in ids.iter().copied().enumerate() {
            let next = ids[(index + 1) % ids.len()];
            document.index.insert(
                id,
                Self::module_item(id.0, &format!("module_{}", id.0), vec![next]),
            );
        }
        Self::set_root_items(&mut document, vec![ids[0]]);
        document
    }

    fn type_alias_item(
        id: u32,
        name: &str,
        params: Vec<rustdoc_types::GenericParamDef>,
        target: rustdoc_types::Type,
    ) -> rustdoc_types::Item {
        Self::public_item(
            id,
            Some(name),
            rustdoc_types::ItemEnum::TypeAlias(rustdoc_types::TypeAlias {
                type_: target,
                generics: rustdoc_types::Generics {
                    params,
                    where_predicates: Vec::new(),
                },
            }),
        )
    }

    fn resolved_type(
        id: u32,
        path: &str,
        args: Option<rustdoc_types::GenericArgs>,
    ) -> rustdoc_types::Type {
        rustdoc_types::Type::ResolvedPath(rustdoc_types::Path {
            path: path.to_owned(),
            id: rustdoc_types::Id(id),
            args: args.map(Box::new),
        })
    }

    fn set_root_items(document: &mut rustdoc_types::Crate, items: Vec<rustdoc_types::Id>) {
        *Self::root_items(document) = items;
    }

    fn root_items(document: &mut rustdoc_types::Crate) -> &mut Vec<rustdoc_types::Id> {
        let rustdoc_types::ItemEnum::Module(root) = &mut document
            .index
            .get_mut(&rustdoc_types::Id(0))
            .expect("fixture root")
            .inner
        else {
            panic!("fixture root must be a module");
        };
        &mut root.items
    }

    fn set_function_output(document: &mut rustdoc_types::Crate, output: rustdoc_types::Type) {
        let rustdoc_types::ItemEnum::Function(function) = &mut document
            .index
            .get_mut(&rustdoc_types::Id(1))
            .expect("fixture function")
            .inner
        else {
            panic!("fixture item must remain a function");
        };
        function.sig.output = Some(output);
    }

    fn add_path(
        document: &mut rustdoc_types::Crate,
        id: u32,
        name: &str,
        kind: rustdoc_types::ItemKind,
    ) {
        document.paths.insert(
            rustdoc_types::Id(id),
            rustdoc_types::ItemSummary {
                crate_id: 0,
                path: vec!["sample".to_owned(), name.to_owned()],
                kind,
            },
        );
    }

    fn extract(
        document: rustdoc_types::Crate,
    ) -> Result<RustCompilerExtraction, crate::error::SignatureContractKitError> {
        Self::extract_artifact(Self::artifact_for(document))
    }

    fn extract_artifact(
        artifact: RustCompilerArtifact,
    ) -> Result<RustCompilerExtraction, crate::error::SignatureContractKitError> {
        Self::extract_artifact_from(artifact, Self::source())
    }

    fn exact_module_artifact(
        source: &[u8],
        mut module: rustdoc_types::Item,
        span: rustdoc_types::Span,
    ) -> RustCompilerArtifact {
        let module_id = module.id;
        let module_name = module.name.clone().expect("fixture module name");
        module.span = Some(span);
        let mut document = Self::rustdoc_document();
        document.index.insert(module_id, module);
        Self::add_path(
            &mut document,
            module_id.0,
            &module_name,
            rustdoc_types::ItemKind::Module,
        );
        Self::set_root_items(&mut document, vec![module_id]);

        let mut artifact = Self::artifact_for(document);
        let byte_end = source.strip_suffix(b"\n").map_or(source.len(), <[u8]>::len);
        Self::set_exact_range(
            &mut artifact,
            module_id.0,
            0,
            u64::try_from(byte_end).expect("module source length"),
        );
        artifact
    }

    fn extract_artifact_from(
        artifact: RustCompilerArtifact,
        source: &[u8],
    ) -> Result<RustCompilerExtraction, crate::error::SignatureContractKitError> {
        let limits = RustExtractionLimits::default();
        let mut usage = limits.usage();
        artifact.extract(
            Self::sources_from(source),
            &Self::allowed_files(),
            &limits,
            &mut usage,
            &CancellationProbe::new(),
        )
    }

    fn extraction_error(artifact: RustCompilerArtifact) -> crate::error::SignatureContractKitError {
        Self::extraction_error_with_limits(artifact, RustExtractionLimits::default())
    }

    fn extraction_error_with_limits(
        artifact: RustCompilerArtifact,
        limits: RustExtractionLimits,
    ) -> crate::error::SignatureContractKitError {
        let mut usage = limits.usage();
        Self::error(
            artifact.extract(
                Self::sources(),
                &Self::allowed_files(),
                &limits,
                &mut usage,
                &CancellationProbe::new(),
            ),
            "compiler artifact unexpectedly extracted",
        )
    }

    fn document_error(document: rustdoc_types::Crate) -> crate::error::SignatureContractKitError {
        Self::extraction_error(Self::artifact_for(document))
    }

    fn error<T>(
        result: Result<T, crate::error::SignatureContractKitError>,
        expectation: &str,
    ) -> crate::error::SignatureContractKitError {
        result.err().expect(expectation)
    }

    fn linked_contracts() -> FileCatalog {
        let contract = format!(
            r#"contract_version: 2
root: ../src
files: [lib.rs]
extraction:
  mode: rust_compiler_v1
  profile: rust_api_v1
  crates: [{{ id: sample, root: lib.rs, kind: library }}]
  compiler:
    artifact_schema_version: 1
    extractor_version: conkit-host/1
    compiler_version: rustc 1.97.0
    rustdoc_format_version: {}
    target_triple: x86_64-unknown-linux-gnu
    features: [alpha, zeta]
    cfg_values: [target_pointer_width=64, unix]
    package: sample
    target: sample
    macro_expansion: true
    name_resolution: true
signatures:
  - answer:
      file: lib.rs
      signature_type: function
      name: answer
      visibility: public
      return_type: u8
      sketch: answer_example
sketches:
  - answer_example:
      file: lib.rs
      signature: answer
      signature_type: function
      matching: {{ normalization: exact_lines_v1, occurrence: exactly_one }}
      code: old
"#,
            rustdoc_types::FORMAT_VERSION,
        );
        let mut contracts = FileCatalog::new();
        contracts
            .insert(
                CatalogPath::new("main.yml").expect("contract path"),
                contract.into_bytes(),
            )
            .expect("compiler contract");
        contracts
    }
}
