use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use cargo_metadata::{Metadata, MetadataCommand, Package};
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use toml_edit::{DocumentMut, Item, Table, Value};

#[test]
fn conkit_manifest_uses_one_cross_platform_binary_name() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let document = fs::read_to_string(&manifest)
        .expect("conkit manifest should be readable")
        .parse::<DocumentMut>()
        .expect("conkit manifest should be valid TOML");
    let binaries = document
        .get("bin")
        .and_then(Item::as_array_of_tables)
        .into_iter()
        .flatten()
        .filter_map(|bin| bin.get("name")?.as_value()?.as_str())
        .collect::<Vec<_>>();
    assert_eq!(binaries, ["conkit"]);
    assert!(
        !document
            .get("features")
            .and_then(Item::as_table)
            .is_some_and(|features| features.contains_key("windows-bin-name"))
    );
}

#[test]
fn workspace_uses_canonical_conkit_package_and_dependency_names() {
    let policy = WorkspacePolicy::load();
    policy.assert_three_crate_dependency_topology();
    policy.assert_cli_dependency_ownership();
    policy.assert_production_rust_syntax();
    policy.assert_closed_command_dispatch();
}

#[test]
fn rust_extraction_backends_use_closed_receiver_dispatch() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("conkit should be a workspace member");
    let backend_root = workspace_root.join("conkit-signature/languages/rust/parser/backend.rs");
    let syntax_backend =
        workspace_root.join("conkit-signature/languages/rust/parser/backend/syntax.rs");
    let compiler_backend =
        workspace_root.join("conkit-signature/languages/rust/parser/backend/compiler.rs");
    RustBackendTopology::parse(
        &fs::read_to_string(&backend_root).expect("Rust backend dispatcher source"),
        &fs::read_to_string(&syntax_backend).expect("syntax backend source"),
        &fs::read_to_string(&compiler_backend).expect("compiler backend source"),
    )
    .assert_valid();
}

#[test]
fn every_workspace_package_declares_the_supported_rust_version() {
    WorkspacePolicy::load().assert_supported_rust_version();
}

#[test]
fn process_termination_handler_is_cli_owned_and_feature_bounded() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("conkit should be a workspace member");
    let manifests = WorkspaceManifests::load(workspace_root);
    manifests.assert_workspace_dependency("ctrlc", "3.5.2", &["termination"]);

    WorkspacePolicy::load().assert_direct_dependency_owners("ctrlc", &["conkit"]);
}

#[test]
fn compiler_private_dependencies_are_not_declared() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("conkit should be a workspace member");
    WorkspaceManifests::load(root).assert_production_dependency_fragment_absent(
        &["rust", "c"].concat(),
        "compiler-private dependency names are not allowed in production manifests",
    );
}

#[test]
fn maintained_yaml_stack_is_narrowly_scoped() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("conkit should be a workspace member");
    let manifests = WorkspaceManifests::load(workspace_root);

    manifests.assert_workspace_dependency("serde-saphyr", "0.0.29", &["deserialize", "serialize"]);
    manifests.assert_workspace_dependency("yaml-edit", "0.2.3", &[]);

    manifests.assert_package_yaml_dependencies("conkit", true, false);
    manifests.assert_package_yaml_dependencies("conkit-signature", true, true);
    manifests.assert_package_yaml_dependencies("conkit-sketch", true, true);
    manifests.assert_production_dependency_fragment_absent(
        "serdeyaml",
        "deprecated serde_yaml dependencies remain",
    );
}

#[test]
fn fuzz_workspace_and_hardening_dependencies_remain_isolated() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("conkit should be a workspace member");
    let manifests = WorkspaceManifests::load(workspace_root);

    manifests.assert_fuzz_workspace_isolated();
    manifests.assert_hardening_dependency_scope();
}

#[test]
fn maintained_yaml_parser_enforces_the_accepted_budget_contract() {
    for (name, yaml, budget, matches) in [
        (
            "events",
            "value\n",
            serde_saphyr::budget! { max_events: 0 },
            (|breach: &serde_saphyr::budget::BudgetBreach| {
                matches!(breach, serde_saphyr::budget::BudgetBreach::Events { .. })
            }) as fn(&serde_saphyr::budget::BudgetBreach) -> bool,
        ),
        (
            "nodes",
            "[value]\n",
            serde_saphyr::budget! { max_nodes: 1 },
            |breach| matches!(breach, serde_saphyr::budget::BudgetBreach::Nodes { .. }),
        ),
        (
            "depth",
            "[[]]\n",
            serde_saphyr::budget! { max_depth: 1 },
            |breach| matches!(breach, serde_saphyr::budget::BudgetBreach::Depth { .. }),
        ),
        (
            "aliases",
            "value: &value original\ncopy: *value\n",
            serde_saphyr::budget! { max_aliases: 0 },
            |breach| matches!(breach, serde_saphyr::budget::BudgetBreach::Aliases { .. }),
        ),
        (
            "scalar bytes",
            "word\n",
            serde_saphyr::budget! { max_total_scalar_bytes: 1 },
            |breach| {
                matches!(
                    breach,
                    serde_saphyr::budget::BudgetBreach::ScalarBytes { .. }
                )
            },
        ),
        (
            "comment bytes",
            "# comment\nvalue\n",
            serde_saphyr::budget! { max_total_comment_bytes: 1 },
            |breach| {
                matches!(
                    breach,
                    serde_saphyr::budget::BudgetBreach::CommentBytes { .. }
                )
            },
        ),
        (
            "documents",
            "---\none\n---\ntwo\n",
            serde_saphyr::budget! { max_documents: 1 },
            |breach| matches!(breach, serde_saphyr::budget::BudgetBreach::Documents { .. }),
        ),
    ] {
        let report = serde_saphyr::budget::check_yaml_budget(
            yaml,
            budget.expect("serde-saphyr's budget macro should construct a budget"),
            serde_saphyr::budget::EnforcingPolicy::AllContent,
        )
        .unwrap_or_else(|error| panic!("{name} YAML should scan: {error}"));
        let breach = report
            .breached
            .unwrap_or_else(|| panic!("{name} YAML should exceed its budget"));
        assert!(
            matches(&breach),
            "{name} YAML breached the wrong budget: {breach:?}"
        );
    }

    let options = serde_saphyr::options! {
        alias_limits: serde_saphyr::alias_limits! { max_total_replayed_events: 10 },
    };
    let yaml = "defs: &value [1, 2, 3, 4]\nlist: [*value, *value]\n";
    let error = serde_saphyr::from_str_with_options::<serde_json::Value>(yaml, options)
        .expect_err("semantic alias expansion should exceed the replay budget");
    match error.without_snippet() {
        serde_saphyr::Error::AliasError { msg, locations } => {
            assert_eq!(
                msg,
                "alias replay limit exceeded: total_replayed_events=11 > 10 at line 1, column 24"
            );
            assert_eq!(
                (
                    locations.reference_location.line(),
                    locations.reference_location.column(),
                    locations.reference_location.span().byte_offset(),
                    locations.reference_location.span().byte_len(),
                    locations.defined_location.line(),
                    locations.defined_location.column(),
                    locations.defined_location.span().byte_offset(),
                    locations.defined_location.span().byte_len(),
                ),
                (2, 16, Some(41), Some(6), 1, 14, Some(13), Some(1))
            );
        }
        other => panic!("semantic alias replay failed with the wrong error: {other}"),
    }
}

struct WorkspacePolicy {
    metadata: Metadata,
}

impl WorkspacePolicy {
    fn load() -> Self {
        let metadata = MetadataCommand::new()
            .no_deps()
            .exec()
            .expect("cargo metadata should describe the workspace");
        Self { metadata }
    }

    fn packages(&self) -> Vec<&Package> {
        self.metadata.workspace_packages()
    }

    fn assert_three_crate_dependency_topology(&self) {
        let packages = self.packages();
        let names = packages
            .iter()
            .map(|package| package.name.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            names,
            BTreeSet::from(["conkit", "conkit-signature", "conkit-sketch"])
        );

        let mut edges = BTreeSet::new();
        for package in &packages {
            for dependency in &package.dependencies {
                if names.contains(dependency.name.as_str()) {
                    assert!(
                        dependency.rename.is_none(),
                        "workspace crates must not be aliased"
                    );
                    edges.insert((package.name.as_str(), dependency.name.as_str()));
                }
            }
        }
        assert_eq!(
            edges,
            BTreeSet::from([("conkit", "conkit-signature"), ("conkit", "conkit-sketch"),])
        );
    }

    fn assert_cli_dependency_ownership(&self) {
        self.assert_direct_dependency_owners("ctrlc", &["conkit"]);
        self.assert_direct_dependency_owners("flate2", &["conkit"]);
        self.assert_direct_dependency_owners("rustdoc-types", &["conkit-signature"]);
        let cli_only = BTreeSet::from([
            "assert_cmd",
            "assert_fs",
            "async-trait",
            "atomic-write-file",
            "cap-fs-ext",
            "cap-std",
            "cargo_metadata",
            "clap",
            "command-group",
            "ctrlc",
            "flate2",
            "fs-err",
            "same-file",
            "tempfile",
            "tokio",
            "walkdir",
        ]);
        for package in self
            .packages()
            .into_iter()
            .filter(|package| package.name != "conkit")
        {
            let forbidden = package
                .dependencies
                .iter()
                .map(|dependency| dependency.name.as_str())
                .filter(|dependency| cli_only.contains(*dependency))
                .collect::<BTreeSet<_>>();
            assert!(forbidden.is_empty(), "{}: {forbidden:?}", package.name);
        }
    }

    fn assert_direct_dependency_owners(&self, dependency: &str, expected: &[&str]) {
        let owners = self
            .packages()
            .into_iter()
            .filter(|package| {
                package
                    .dependencies
                    .iter()
                    .any(|item| item.name == dependency)
            })
            .map(|package| package.name.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(owners, expected.iter().copied().collect());
    }

    fn assert_supported_rust_version(&self) {
        let supported = cargo_metadata::semver::Version::new(1, 97, 0);
        for package in self.packages() {
            assert_eq!(
                package.rust_version.as_ref(),
                Some(&supported),
                "{}",
                package.name
            );
        }
    }

    fn assert_production_rust_syntax(&self) {
        let mut violations = Vec::new();
        for package in self.packages() {
            let package_root = package
                .manifest_path
                .as_std_path()
                .parent()
                .expect("package root");
            for entry in walkdir::WalkDir::new(package_root) {
                let entry = entry.expect("workspace source entry");
                let relative = entry
                    .path()
                    .strip_prefix(package_root)
                    .expect("package-relative path");
                if !entry.file_type().is_file()
                    || entry.path().extension().and_then(std::ffi::OsStr::to_str) != Some("rs")
                    || relative.file_name().and_then(std::ffi::OsStr::to_str) == Some("tests.rs")
                    || relative.components().any(|part| {
                        matches!(
                            part.as_os_str().to_str(),
                            Some("tests" | "benches" | "examples")
                        )
                    })
                {
                    continue;
                }
                let source = fs::read_to_string(entry.path()).expect("production Rust source");
                match SourcePolicy::analyze(package.name.as_str(), entry.path(), &source) {
                    Ok(policy) => violations.extend(policy.violations),
                    Err(error) => violations.push(format!("{}: {error}", entry.path().display())),
                }
            }
        }
        assert!(violations.is_empty(), "{}", violations.join("\n"));
    }

    fn assert_closed_command_dispatch(&self) {
        let root = self.metadata.workspace_root.as_std_path();
        let args = fs::read_to_string(root.join("conkit/args.rs")).expect("args source");
        let command = fs::read_to_string(root.join("conkit/command.rs")).expect("command source");
        let policy = DispatchPolicy::analyze(&args, &command).expect("command syntax");
        assert!(
            policy.violations.is_empty(),
            "{}",
            policy.violations.join("\n")
        );
    }
}

struct SourcePolicy {
    package: String,
    path: PathBuf,
    use_path: Vec<String>,
    violations: Vec<String>,
}

impl SourcePolicy {
    fn analyze(package: &str, path: &Path, source: &str) -> syn::Result<Self> {
        let file = syn::parse_file(source)?;
        let mut policy = Self {
            package: package.to_owned(),
            path: path.to_path_buf(),
            use_path: Vec::new(),
            violations: Vec::new(),
        };
        policy.visit_file(&file);
        Ok(policy)
    }

    fn names(&self, path: &syn::Path) -> Vec<String> {
        path.segments
            .iter()
            .map(|part| part.ident.unraw().to_string())
            .collect()
    }

    fn inspect<T: Spanned>(&mut self, node: &T, names: &[String]) {
        let first = names.first().map(String::as_str);
        let second = names.get(1).map(String::as_str);
        let third = names.get(2).map(String::as_str);
        let domain = self.package != "conkit";
        let public_api = self.path.file_name().and_then(std::ffi::OsStr::to_str) == Some("api.rs");
        let raw_signature_options = self.package == "conkit"
            && self.path.file_name().and_then(std::ffi::OsStr::to_str) != Some("args.rs")
            && matches!(
                names.last().map(String::as_str),
                Some("SignatureOptions" | "SignatureExtractorArg")
            );
        let forbidden = raw_signature_options
            || domain
                && (matches!((first, second), (Some("std"), Some("fs" | "process")))
                    || matches!(
                        (first, second, third),
                        (Some("std"), Some("path"), Some("PathBuf"))
                    )
                    || (public_api && matches!((first, second), (Some("std"), Some("path"))))
                    || matches!(first, Some("async_trait" | "clap" | "fs_err" | "tokio"))
                    || matches!(
                        (self.package.as_str(), first),
                        ("conkit-signature", Some("conkit" | "conkit_sketch"))
                            | ("conkit-sketch", Some("conkit" | "conkit_signature"))
                    ));
        if forbidden {
            let at = node.span().start();
            self.violations.push(format!(
                "{}:{}:{}: forbidden `{}`",
                self.path.display(),
                at.line,
                at.column + 1,
                names.join("::")
            ));
        }
    }

    fn meta_has_test(&self, meta: &syn::Meta) -> syn::Result<bool> {
        match meta {
            syn::Meta::Path(path) => Ok(self.names(path).iter().any(|name| name == "test")),
            syn::Meta::NameValue(value) => {
                Ok(self.names(&value.path).iter().any(|name| name == "test"))
            }
            syn::Meta::List(list) => {
                let items = list.parse_args_with(
                    syn::punctuated::Punctuated::<syn::Meta, syn::token::Comma>::parse_terminated,
                )?;
                for item in &items {
                    if self.meta_has_test(item)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }

    fn has_test_cfg(&self, attribute: &syn::Attribute) -> bool {
        matches!(self.names(attribute.path()).as_slice(), [name] if name == "cfg" || name == "cfg_attr")
            && self.meta_has_test(&attribute.meta).unwrap_or(true)
    }

    fn local_tests(&self, module: &syn::ItemMod) -> bool {
        module.ident.unraw() == "tests"
            && matches!(module.vis, syn::Visibility::Inherited)
            && module.attrs.len() == 1
            && matches!(self.names(module.attrs[0].path()).as_slice(), [name] if name == "cfg")
            && module.attrs[0]
                .parse_args::<syn::Path>()
                .is_ok_and(|path| matches!(self.names(&path).as_slice(), [name] if name == "test"))
    }
}

impl<'ast> Visit<'ast> for SourcePolicy {
    fn visit_attribute(&mut self, attribute: &'ast syn::Attribute) {
        if self.has_test_cfg(attribute) {
            let at = attribute.span().start();
            self.violations.push(format!(
                "{}:{}:{}: test cfg outside local mod tests",
                self.path.display(),
                at.line,
                at.column + 1
            ));
        }
        visit::visit_attribute(self, attribute);
    }

    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        if !self.local_tests(module) {
            visit::visit_item_mod(self, module);
        }
    }

    fn visit_item_extern_crate(&mut self, item: &'ast syn::ItemExternCrate) {
        self.inspect(item, &[item.ident.unraw().to_string()]);
        visit::visit_item_extern_crate(self, item);
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        let names = self.names(path);
        self.inspect(path, &names);
        visit::visit_path(self, path);
    }

    fn visit_use_tree(&mut self, tree: &'ast syn::UseTree) {
        match tree {
            syn::UseTree::Path(path) => {
                self.use_path.push(path.ident.unraw().to_string());
                self.visit_use_tree(&path.tree);
                self.use_path.pop();
            }
            syn::UseTree::Name(name) => {
                let mut path = self.use_path.clone();
                path.push(name.ident.unraw().to_string());
                self.inspect(tree, &path);
            }
            syn::UseTree::Rename(rename) => {
                let mut path = self.use_path.clone();
                path.push(rename.ident.unraw().to_string());
                self.inspect(tree, &path);
            }
            syn::UseTree::Group(group) => group
                .items
                .iter()
                .for_each(|item| self.visit_use_tree(item)),
            syn::UseTree::Glob(_) => {
                let path = self.use_path.clone();
                self.inspect(tree, &path);
            }
        }
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let (syn::Expr::Path(function), Some(syn::Expr::Lit(argument))) =
            (call.func.as_ref(), call.args.first())
            && matches!(self.names(&function.path).as_slice(), [.., command, new]
                if command == "Command" && new == "new")
            && matches!(&argument.lit, syn::Lit::Str(value) if value.value() == "rustc")
        {
            self.violations.push(format!(
                "{}: direct Command::new(\"rustc\")",
                self.path.display()
            ));
        }
        visit::visit_expr_call(self, call);
    }
}

struct DispatchPolicy {
    expected: BTreeSet<String>,
    seen: BTreeSet<String>,
    matches: usize,
    binding: Option<String>,
    delegations: usize,
    violations: Vec<String>,
}

impl DispatchPolicy {
    fn analyze(args: &str, command: &str) -> syn::Result<Self> {
        let args = syn::parse_file(args)?;
        let command = syn::parse_file(command)?;
        let pairs = [
            ("Check", "CheckCommand"),
            ("Generate", "GenerateCommand"),
            ("Archive", "ArchiveCommand"),
            ("Diff", "DiffCommand"),
        ];
        let expected = pairs
            .iter()
            .map(|(variant, _)| (*variant).to_owned())
            .collect();
        let mut policy = Self {
            expected,
            seen: BTreeSet::new(),
            matches: 0,
            binding: None,
            delegations: 0,
            violations: Vec::new(),
        };

        let actual = args.items.iter().find_map(|item| match item {
            syn::Item::Enum(item) if item.ident == "Command" => Some(
                item.variants
                    .iter()
                    .filter_map(|variant| {
                        let syn::Fields::Unnamed(fields) = &variant.fields else {
                            return None;
                        };
                        if fields.unnamed.len() != 1 {
                            return None;
                        }
                        let syn::Type::Path(payload) = &fields.unnamed.first()?.ty else {
                            return None;
                        };
                        Some((
                            variant.ident.to_string(),
                            payload.path.segments.last()?.ident.to_string(),
                        ))
                    })
                    .collect::<BTreeSet<_>>(),
            ),
            _ => None,
        });
        assert_eq!(
            actual,
            Some(
                pairs
                    .into_iter()
                    .map(|(a, b)| (a.to_owned(), b.to_owned()))
                    .collect()
            )
        );

        let native_trait = command.items.iter().any(|item| matches!(item,
            syn::Item::Trait(item) if item.ident == "AppCommand"
                && !item.attrs.iter().any(|attr| attr.path().segments.last().is_some_and(|part| part.ident == "async_trait"))
                && item.items.iter().any(|member| matches!(member, syn::TraitItem::Fn(method)
                    if Self::execute_signature(&method.sig)
                        && !method.attrs.iter().any(|attr| attr.path().segments.last().is_some_and(|part| part.ident == "async_trait"))))
        ));
        assert!(native_trait, "native AppCommand trait");
        let method = command.items.iter().find_map(|item| match item {
            syn::Item::Impl(item) if item.trait_.as_ref().is_some_and(|(path, _)| path.segments.last().is_some_and(|part| part.ident == "AppCommand"))
                && matches!(item.self_ty.as_ref(), syn::Type::Path(path) if path.path.segments.last().is_some_and(|part| part.ident == "Command"))
                && !item.attrs.iter().any(|attr| attr.path().segments.last().is_some_and(|part| part.ident == "async_trait")) =>
                item.items.iter().find_map(|member| match member {
                    syn::ImplItem::Fn(method) if Self::execute_signature(&method.sig)
                        && !method.attrs.iter().any(|attr| attr.path().segments.last().is_some_and(|part| part.ident == "async_trait")) => Some(method),
                    _ => None,
                }),
            _ => None,
        }).expect("native AppCommand for Command");
        policy.visit_block(&method.block);
        if policy.matches != 1 || policy.seen != policy.expected {
            policy
                .violations
                .push("dispatch must exhaustively match Command once".to_owned());
        }
        Ok(policy)
    }

    fn execute_signature(signature: &syn::Signature) -> bool {
        let mut inputs = signature.inputs.iter();
        let receiver = matches!(inputs.next(), Some(syn::FnArg::Receiver(receiver))
            if receiver.mutability.is_none()
                && matches!(&receiver.kind, syn::ReceiverKind::Reference(_, _, mutability)
                    if mutability.is_none()));
        let context = matches!(inputs.next(), Some(syn::FnArg::Typed(context))
            if matches!(context.pat.as_ref(), syn::Pat::Ident(name)
                    if name.ident == "context"
                        && name.by_ref.is_none()
                        && name.mutability.is_none()
                        && name.subpat.is_none())
                && matches!(context.ty.as_ref(), syn::Type::Reference(reference)
                    if reference.mutability.is_none()
                        && matches!(reference.elem.as_ref(), syn::Type::Path(path)
                            if path.path.segments.last().is_some_and(|part| part.ident == "CommandContext"))));
        signature.ident == "execute"
            && signature.asyncness.is_some()
            && receiver
            && context
            && inputs.next().is_none()
    }

    fn arm(&mut self, arm: &syn::Arm) {
        let (pattern, guard) = match &arm.pat {
            syn::Pat::Guard(guard) => (guard.pat.as_ref(), Some(guard.guard.as_ref())),
            pattern => (pattern, None),
        };
        let syn::Pat::TupleStruct(pattern) = pattern else {
            self.violations
                .push("wildcard/catch-all dispatch arm".to_owned());
            if let Some(guard) = guard {
                self.visit_expr(guard);
            }
            self.visit_expr(&arm.body);
            return;
        };
        let Some(variant) = pattern
            .path
            .segments
            .last()
            .map(|part| part.ident.to_string())
        else {
            return;
        };
        let explicit = pattern.path.leading_colon.is_none()
            && pattern.path.segments.len() == 2
            && pattern
                .path
                .segments
                .first()
                .is_some_and(|part| part.ident == "Self")
            && pattern.elems.len() == 1;
        let Some(syn::Pat::Ident(binding)) = pattern.elems.first() else {
            self.violations
                .push(format!("{variant} must use receiver-style execute"));
            self.visit_expr(&arm.body);
            return;
        };
        self.binding = Some(binding.ident.to_string());
        self.delegations = 0;
        if let Some(guard) = guard {
            self.visit_expr(guard);
        }
        self.visit_expr(&arm.body);
        if !explicit
            || binding.by_ref.is_some()
            || binding.mutability.is_some()
            || binding.subpat.is_some()
            || guard.is_some()
            || self.delegations != 1
        {
            self.violations
                .push(format!("{variant} must use receiver-style execute"));
        }
        self.binding = None;
        self.seen.insert(variant);
    }
}

impl<'ast> Visit<'ast> for DispatchPolicy {
    fn visit_expr_match(&mut self, expression: &'ast syn::ExprMatch) {
        if matches!(expression.expr.as_ref(), syn::Expr::Path(path) if path.path.is_ident("self")) {
            self.matches += 1;
            expression.arms.iter().for_each(|arm| self.arm(arm));
        } else {
            visit::visit_expr_match(self, expression);
        }
    }

    fn visit_expr_await(&mut self, awaited: &'ast syn::ExprAwait) {
        if let syn::Expr::MethodCall(call) = awaited.base.as_ref() {
            let receiver = self.binding.as_deref().is_some_and(|binding| {
                matches!(call.receiver.as_ref(), syn::Expr::Path(path) if path.path.is_ident(binding))
            });
            let context = call.args.len() == 1
                && matches!(call.args.first(), Some(syn::Expr::Path(path)) if path.path.is_ident("context"));
            if receiver && call.method == "execute" && context {
                self.delegations += 1;
            }
        }
        visit::visit_expr_await(self, awaited);
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if matches!(call.func.as_ref(), syn::Expr::Path(path) if path.path.segments.last().is_some_and(|part| part.ident == "execute"))
        {
            self.violations.push("UFCS payload dispatch".to_owned());
        }
        visit::visit_expr_call(self, call);
    }

    fn visit_macro(&mut self, command_macro: &'ast syn::Macro) {
        self.violations.push("macro payload dispatch".to_owned());
        visit::visit_macro(self, command_macro);
    }
}

struct RustBackendTopology([syn::File; 3]);

impl RustBackendTopology {
    fn parse(root: &str, syntax: &str, compiler: &str) -> Self {
        let parse = |source| syn::parse_file(source).expect("backend syntax");
        Self([parse(root), parse(syntax), parse(compiler)])
    }

    fn assert_valid(&self) {
        let mut traits = self
            .0
            .iter()
            .flat_map(|file| &file.items)
            .filter_map(|item| match item {
                syn::Item::Trait(item) if item.ident == "RustExtractionBackend" => Some(item),
                _ => None,
            });
        let backend_trait = traits.next().expect("RustExtractionBackend trait");
        assert!(traits.next().is_none());
        let methods = backend_trait
            .items
            .iter()
            .filter_map(|item| match item {
                syn::TraitItem::Fn(method) => Some(method),
                _ => None,
            })
            .collect::<Vec<_>>();
        Self::assert_operations(methods.iter().map(|method| method.sig.ident.to_string()));
        for method in methods {
            assert!(
                method.default.is_none()
                    && method.sig.asyncness.is_none()
                    && matches!(method.sig.inputs.first(), Some(syn::FnArg::Receiver(receiver))
                        if matches!(&receiver.kind,
                            syn::ReceiverKind::Value | syn::ReceiverKind::Reference(..)))
            );
        }

        let backend = self.0[0]
            .items
            .iter()
            .find_map(|item| match item {
                syn::Item::Enum(item) if item.ident == "RustBackend" => Some(item),
                _ => None,
            })
            .expect("RustBackend enum");
        let variants = backend
            .variants
            .iter()
            .map(|variant| {
                let syn::Fields::Unnamed(fields) = &variant.fields else {
                    panic!("backend variant must carry data");
                };
                assert_eq!(fields.unnamed.len(), 1);
                (
                    variant.ident.to_string(),
                    Self::type_name(&fields.unnamed[0].ty),
                )
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            variants,
            BTreeSet::from([
                ("Compiler".to_owned(), "CompilerBackend".to_owned()),
                ("Syntax".to_owned(), "SyntaxBackend".to_owned()),
            ])
        );

        Self::assert_impl(&self.0[0], "RustBackend", true);
        Self::assert_impl(&self.0[1], "SyntaxBackend", false);
        Self::assert_impl(&self.0[2], "CompilerBackend", false);
    }

    fn assert_impl(file: &syn::File, owner: &str, dispatch: bool) {
        let mut implementations = file.items.iter().filter_map(|item| match item {
            syn::Item::Impl(item)
                if item.trait_.as_ref().is_some_and(|(path, _)| {
                    path.segments
                        .last()
                        .is_some_and(|part| part.ident == "RustExtractionBackend")
                }) && Self::type_name(&item.self_ty) == owner =>
            {
                Some(item)
            }
            _ => None,
        });
        let implementation = implementations.next().expect("backend implementation");
        assert!(implementations.next().is_none());
        let methods = implementation
            .items
            .iter()
            .filter_map(|item| match item {
                syn::ImplItem::Fn(method) => Some(method),
                _ => None,
            })
            .collect::<Vec<_>>();
        Self::assert_operations(methods.iter().map(|method| method.sig.ident.to_string()));
        if dispatch {
            methods.into_iter().for_each(Self::assert_forwarding);
        }
    }

    fn assert_forwarding(method: &syn::ImplItemFn) {
        let [syn::Stmt::Expr(syn::Expr::Match(dispatch), _)] = method.block.stmts.as_slice() else {
            panic!("backend method must contain one match");
        };
        assert!(
            matches!(dispatch.expr.as_ref(), syn::Expr::Path(path) if path.path.is_ident("self"))
        );
        let mut variants = BTreeSet::new();
        for arm in &dispatch.arms {
            let syn::Pat::TupleStruct(pattern) = &arm.pat else {
                panic!("catch-all backend dispatch");
            };
            assert!(
                pattern.path.segments.len() == 2
                    && pattern.path.segments[0].ident == "Self"
                    && pattern.elems.len() == 1
            );
            let syn::Pat::Ident(binding) = &pattern.elems[0] else {
                panic!("backend arm must bind its payload");
            };
            let call = match arm.body.as_ref() {
                syn::Expr::MethodCall(call) => call,
                syn::Expr::Block(block) => match block.block.stmts.as_slice() {
                    [syn::Stmt::Expr(syn::Expr::MethodCall(call), _)] => call,
                    _ => panic!("backend arm must directly call its receiver"),
                },
                _ => panic!("backend arm must directly call its receiver"),
            };
            assert!(
                call.method == method.sig.ident
                    && matches!(call.receiver.as_ref(), syn::Expr::Path(path)
                    if path.path.is_ident(&binding.ident))
            );
            variants.insert(pattern.path.segments[1].ident.to_string());
        }
        assert_eq!(dispatch.arms.len(), 2);
        assert_eq!(
            variants,
            BTreeSet::from(["Compiler".to_owned(), "Syntax".to_owned()])
        );
    }

    fn assert_operations(names: impl IntoIterator<Item = String>) {
        assert_eq!(
            names.into_iter().collect::<BTreeSet<_>>(),
            BTreeSet::from(["check", "generate", "resolve_sketches"].map(str::to_owned))
        );
    }

    fn type_name(ty: &syn::Type) -> String {
        let syn::Type::Path(path) = ty else {
            panic!("backend must be a concrete type");
        };
        let segment = path.path.segments.last().expect("backend type");
        segment.ident.to_string()
    }
}

#[test]
fn syntax_policy_handles_cfg_nesting_raw_identifiers_and_noncode_text() {
    let allowed = r#"const TEXT: &str = "std::process #[cfg(test)]"; // std::fs
        #[cfg(test)] mod tests { use std::fs; #[cfg(test)] fn helper() {} }"#;
    assert!(
        SourcePolicy::analyze("conkit-sketch", Path::new("allowed.rs"), allowed)
            .expect("allowed syntax")
            .violations
            .is_empty()
    );

    let rejected = r#"mod nested { #[allow(dead_code)] #[cfg(any(unix, test))] mod tests {} }
        #[cfg(test)] #[allow(dead_code)] fn stacked() {}
        #[cfg_attr(test, allow(dead_code))] struct Hidden;
        use r#std::process::Command;"#;
    assert_eq!(
        SourcePolicy::analyze("conkit-sketch", Path::new("rejected.rs"), rejected)
            .expect("rejected syntax")
            .violations
            .len(),
        4
    );

    assert!(
        SourcePolicy::analyze(
            "conkit-signature",
            Path::new("private.rs"),
            "use std::path::Path;",
        )
        .expect("private path syntax")
        .violations
        .is_empty()
    );
    assert_eq!(
        SourcePolicy::analyze(
            "conkit-signature",
            Path::new("api.rs"),
            "use std::path::Path;",
        )
        .expect("public path syntax")
        .violations
        .len(),
        1
    );
}

#[test]
fn syntax_policy_reports_malformed_files_and_direct_rustc() {
    assert!(SourcePolicy::analyze("conkit", Path::new("bad.rs"), "fn bad(").is_err());
    let policy = SourcePolicy::analyze(
        "conkit",
        Path::new("rustc.rs"),
        r#"fn run() { Command::new("rustc"); }"#,
    )
    .expect("process syntax");
    assert!(
        policy
            .violations
            .iter()
            .any(|item| item.contains("Command::new"))
    );
}

#[test]
fn dispatch_policy_rejects_escape_hatches() {
    let args = "enum Command { Check(CheckCommand), Generate(GenerateCommand), \
        Archive(ArchiveCommand), Diff(DiffCommand) }";
    let command = r#"
        trait AppCommand { async fn execute(&self, context: &CommandContext); }
        impl AppCommand for Command {
            async fn execute(&self, context: &CommandContext) {
                match self {
                    Self::Check(command) => AppCommand::execute(command, context).await,
                    Self::Generate(_) => dispatch!(),
                    _ => (),
                }
            }
        }"#;
    let policy = DispatchPolicy::analyze(args, command).expect("dispatch syntax");
    for expected in ["UFCS", "wildcard/catch-all", "macro"] {
        assert!(policy.violations.iter().any(|item| item.contains(expected)));
    }
}

struct WorkspaceManifests {
    workspace: DocumentMut,
    packages: Vec<(String, DocumentMut)>,
    fuzz: DocumentMut,
}

impl WorkspaceManifests {
    fn load(root: &Path) -> Self {
        let workspace = Self::read(root.join("Cargo.toml"));
        let packages = ["conkit", "conkit-signature", "conkit-sketch"]
            .into_iter()
            .map(|name| {
                (
                    name.to_owned(),
                    Self::read(root.join(name).join("Cargo.toml")),
                )
            })
            .collect();
        let fuzz = Self::read(root.join("fuzz").join("Cargo.toml"));

        Self {
            workspace,
            packages,
            fuzz,
        }
    }

    fn read(path: PathBuf) -> DocumentMut {
        fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
            .parse::<DocumentMut>()
            .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
    }

    fn assert_production_dependency_fragment_absent(&self, fragment: &str, message: &str) {
        let mut violations = Vec::new();
        self.collect_dependency_violations(
            "Cargo.toml",
            self.workspace.as_table(),
            &mut Vec::new(),
            fragment,
            &mut violations,
        );
        for (package, document) in &self.packages {
            self.collect_dependency_violations(
                &format!("{package}/Cargo.toml"),
                document.as_table(),
                &mut Vec::new(),
                fragment,
                &mut violations,
            );
        }
        assert!(
            violations.is_empty(),
            "{message}:\n{}",
            violations.join("\n")
        );
    }

    fn collect_dependency_violations(
        &self,
        manifest: &str,
        table: &Table,
        sections: &mut Vec<String>,
        fragment: &str,
        violations: &mut Vec<String>,
    ) {
        let dependency_section =
            |section: &str| matches!(section, "dependencies" | "build-dependencies");
        let production = matches!(sections.as_slice(), [section] if dependency_section(section))
            || matches!(sections.as_slice(), [workspace, section]
                if workspace == "workspace" && dependency_section(section))
            || matches!(sections.as_slice(), [target, _, section]
                if target == "target" && dependency_section(section));
        if production {
            for (name, item) in table {
                let renamed = item
                    .as_table()
                    .and_then(|value| value.get("package"))
                    .and_then(Item::as_value)
                    .and_then(Value::as_str)
                    .or_else(|| {
                        item.as_value()
                            .and_then(Value::as_inline_table)
                            .and_then(|value| value.get("package"))
                            .and_then(Value::as_str)
                    });
                for name in std::iter::once(name).chain(renamed) {
                    if name
                        .replace(['-', '_'], "")
                        .to_ascii_lowercase()
                        .contains(fragment)
                    {
                        violations.push(format!("{manifest} [{}]: {name}", sections.join(".")));
                    }
                }
            }
        }
        for (name, item) in table {
            if let Some(child) = item.as_table() {
                sections.push(name.to_owned());
                self.collect_dependency_violations(manifest, child, sections, fragment, violations);
                sections.pop();
            }
        }
    }

    fn assert_workspace_dependency(&self, name: &str, version: &str, features: &[&str]) {
        let dependency = self
            .workspace
            .get("workspace")
            .and_then(Item::as_table)
            .and_then(|workspace| workspace.get("dependencies"))
            .and_then(Item::as_table)
            .and_then(|dependencies| dependencies.get(name))
            .unwrap_or_else(|| panic!("workspace dependency {name} should be declared"));
        let dependency = dependency
            .as_value()
            .and_then(Value::as_inline_table)
            .unwrap_or_else(|| panic!("workspace dependency {name} should use an inline table"));

        assert_eq!(
            dependency.get("version").and_then(Value::as_str),
            Some(version),
            "workspace dependency {name} should pin the accepted compatibility line"
        );
        assert_eq!(
            dependency.get("default-features").and_then(Value::as_bool),
            Some(false),
            "workspace dependency {name} must not enable optional integrations"
        );

        let actual_features = dependency
            .get("features")
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(Value::as_str).collect::<Vec<_>>())
            .unwrap_or_default();
        assert_eq!(
            actual_features, features,
            "unexpected {name} feature surface"
        );
    }

    fn assert_package_yaml_dependencies(&self, package: &str, semantic: bool, lossless: bool) {
        let document = self
            .packages
            .iter()
            .find_map(|(name, document)| (name == package).then_some(document))
            .unwrap_or_else(|| panic!("missing package manifest {package}"));
        let dependencies = document
            .get("dependencies")
            .and_then(Item::as_table)
            .unwrap_or_else(|| panic!("{package} should declare dependencies"));

        assert_eq!(
            dependencies.contains_key("serde-saphyr"),
            semantic,
            "unexpected semantic YAML dependency scope for {package}"
        );
        assert_eq!(
            dependencies.contains_key("yaml-edit"),
            lossless,
            "unexpected lossless YAML dependency scope for {package}"
        );
    }

    fn assert_fuzz_workspace_isolated(&self) {
        let workspace = self
            .workspace
            .get("workspace")
            .and_then(Item::as_table)
            .expect("root workspace table");
        let members = workspace
            .get("members")
            .and_then(Item::as_value)
            .and_then(Value::as_array)
            .expect("root workspace members")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        let excluded = workspace
            .get("exclude")
            .and_then(Item::as_value)
            .and_then(Value::as_array)
            .expect("root workspace exclusions")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        let fuzz_members = self
            .fuzz
            .get("workspace")
            .and_then(Item::as_table)
            .and_then(|workspace| workspace.get("members"))
            .and_then(Item::as_value)
            .and_then(Value::as_array)
            .expect("fuzz workspace members")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();

        assert_eq!(members, ["conkit", "conkit-signature", "conkit-sketch"]);
        assert_eq!(excluded, ["fuzz"]);
        assert_eq!(fuzz_members, ["."]);
    }

    fn assert_hardening_dependency_scope(&self) {
        for (package, document) in &self.packages {
            let production = Self::dependency_names(document, "dependencies");
            for forbidden in ["criterion", "proptest", "libfuzzer-sys"] {
                assert!(
                    !production.contains(forbidden),
                    "{forbidden} must not be a production dependency of {package}"
                );
            }
        }

        let signature = self
            .packages
            .iter()
            .find_map(|(name, document)| (name == "conkit-signature").then_some(document))
            .expect("signature manifest");
        let sketch = self
            .packages
            .iter()
            .find_map(|(name, document)| (name == "conkit-sketch").then_some(document))
            .expect("sketch manifest");
        for (package, document) in [("conkit-signature", signature), ("conkit-sketch", sketch)] {
            let development = Self::dependency_names(document, "dev-dependencies");
            assert!(
                development.contains("criterion"),
                "{package} benchmark scope"
            );
            assert!(
                development.contains("proptest"),
                "{package} property-test scope"
            );
        }

        let fuzz = Self::dependency_names(&self.fuzz, "dependencies");
        assert!(fuzz.contains("libfuzzer-sys"));
        assert!(!fuzz.contains("criterion"));
        assert!(!fuzz.contains("proptest"));
    }

    fn dependency_names<'document>(
        document: &'document DocumentMut,
        section: &str,
    ) -> BTreeSet<&'document str> {
        document
            .get(section)
            .and_then(Item::as_table)
            .map(|dependencies| dependencies.iter().map(|(name, _)| name).collect())
            .unwrap_or_default()
    }
}
