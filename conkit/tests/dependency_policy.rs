use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use cargo_metadata::{Metadata, MetadataCommand};
use toml_edit::{DocumentMut, Item, Table, Value};

#[test]
fn conkit_manifest_uses_one_cross_platform_binary_name() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let document = fs::read_to_string(&manifest)
        .expect("conkit manifest should be readable")
        .parse::<DocumentMut>()
        .expect("conkit manifest should be valid TOML");
    let manifest = ConkitManifest::new(&document);

    assert_eq!(manifest.binary_names(), ["conkit"]);
    assert!(!manifest.has_feature("windows-bin-name"));
}

#[test]
fn workspace_uses_canonical_conkit_package_and_dependency_names() {
    let metadata = MetadataCommand::new()
        .no_deps()
        .exec()
        .expect("cargo metadata should describe the workspace");
    let package_names = metadata
        .packages
        .iter()
        .filter(|package| metadata.workspace_members.contains(&package.id))
        .map(|package| package.name.as_str())
        .collect::<BTreeSet<_>>();

    assert_eq!(
        package_names,
        BTreeSet::from(["conkit", "conkit-signature", "conkit-sketch"])
    );

    let conkit = metadata
        .packages
        .iter()
        .find(|package| package.name == "conkit")
        .expect("conkit package should exist");
    for dependency_name in ["conkit-signature", "conkit-sketch"] {
        let dependency = conkit
            .dependencies
            .iter()
            .find(|dependency| dependency.name == dependency_name)
            .unwrap_or_else(|| panic!("missing canonical dependency {dependency_name}"));
        assert_eq!(
            dependency.rename, None,
            "{dependency_name} must not be aliased"
        );
    }
}

#[test]
fn compiler_private_dependencies_are_not_declared() {
    let metadata = MetadataCommand::new()
        .no_deps()
        .exec()
        .expect("cargo metadata should describe the workspace");
    let workspace_root = metadata.workspace_root.as_std_path();
    let manifests = workspace_manifests(&metadata);

    let banned_fragment = ["rust", "c"].concat();
    let mut violations = Vec::new();

    for manifest in manifests {
        collect_manifest_violations(&manifest, workspace_root, &banned_fragment, &mut violations);
    }

    assert!(
        violations.is_empty(),
        "compiler-private dependency names are not allowed in production manifests:\n{}",
        violations.join("\n")
    );
}

#[test]
fn production_rust_sources_do_not_define_test_cfg_shims() {
    let metadata = MetadataCommand::new()
        .no_deps()
        .exec()
        .expect("cargo metadata should describe the workspace");
    let workspace_root = metadata.workspace_root.as_std_path();
    let mut violations = Vec::new();

    for package in &metadata.packages {
        if !metadata
            .workspace_members
            .iter()
            .any(|member| member == &package.id)
        {
            continue;
        }

        let manifest = package.manifest_path.as_std_path();
        let package_root = manifest
            .parent()
            .unwrap_or_else(|| panic!("{} should have a parent", manifest.display()));
        collect_test_cfg_shim_violations(package_root, workspace_root, &mut violations);
    }

    assert!(
        violations.is_empty(),
        "production Rust sources may only use #[cfg(test)] to gate local test modules:\n{}",
        violations.join("\n")
    );
}

fn workspace_manifests(metadata: &Metadata) -> BTreeSet<PathBuf> {
    let mut manifests = BTreeSet::new();
    manifests.insert(metadata.workspace_root.as_std_path().join("Cargo.toml"));

    for package in &metadata.packages {
        if metadata
            .workspace_members
            .iter()
            .any(|member| member == &package.id)
        {
            manifests.insert(package.manifest_path.as_std_path().to_path_buf());
        }
    }

    manifests
}

fn collect_test_cfg_shim_violations(
    directory: &Path,
    workspace_root: &Path,
    violations: &mut Vec<String>,
) {
    if has_path_component(directory, "tests") {
        return;
    }

    for entry in fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", directory.display()))
    {
        let entry = entry.expect("directory entry should be readable");
        let path = entry.path();

        if path.is_dir() {
            collect_test_cfg_shim_violations(&path, workspace_root, violations);
            continue;
        }

        if path.extension().and_then(std::ffi::OsStr::to_str) != Some("rs") {
            continue;
        }

        collect_file_test_cfg_shim_violations(&path, workspace_root, violations);
    }
}

fn collect_file_test_cfg_shim_violations(
    path: &Path,
    workspace_root: &Path,
    violations: &mut Vec<String>,
) {
    let contents = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    let lines = contents.lines().collect::<Vec<_>>();

    for (index, line) in lines.iter().enumerate() {
        let attribute = CfgAttribute::new(line);
        if !attribute.mentions_test() {
            continue;
        }

        if attribute.is_plain_test_cfg()
            && next_significant_line(&lines, index + 1).is_some_and(is_test_module_declaration)
        {
            continue;
        }

        violations.push(format!(
            "{}:{}",
            display_path(path, workspace_root),
            index + 1
        ));
    }
}

struct CfgAttribute<'a> {
    line: &'a str,
}

impl<'a> CfgAttribute<'a> {
    fn new(line: &'a str) -> Self {
        Self { line }
    }

    fn mentions_test(&self) -> bool {
        let normalized = self.normalized();
        let is_cfg = normalized.starts_with("#[cfg(") || normalized.starts_with("#[cfg_attr(");

        is_cfg
            && normalized
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .any(|token| token == "test")
    }

    fn is_plain_test_cfg(&self) -> bool {
        self.normalized() == "#[cfg(test)]"
    }

    fn normalized(&self) -> String {
        self.line
            .trim()
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect()
    }
}

fn next_significant_line<'a>(lines: &'a [&str], start: usize) -> Option<&'a str> {
    lines
        .iter()
        .skip(start)
        .copied()
        .find(|line| !line.trim().is_empty())
}

fn is_test_module_declaration(line: &str) -> bool {
    line.trim_start().starts_with("mod tests")
}

fn has_path_component(path: &Path, expected: &str) -> bool {
    path.components()
        .any(|component| component.as_os_str() == expected)
}

fn collect_manifest_violations(
    manifest: &Path,
    workspace_root: &Path,
    banned_fragment: &str,
    violations: &mut Vec<String>,
) {
    let contents = fs::read_to_string(manifest)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", manifest.display()));
    let document = contents
        .parse::<DocumentMut>()
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", manifest.display()));

    collect_table_violations(
        document.as_table(),
        &mut Vec::new(),
        manifest,
        workspace_root,
        banned_fragment,
        violations,
    );
}

fn collect_table_violations(
    table: &Table,
    table_path: &mut Vec<String>,
    manifest: &Path,
    workspace_root: &Path,
    banned_fragment: &str,
    violations: &mut Vec<String>,
) {
    if is_production_dependency_table(table_path) {
        collect_dependency_table_violations(
            table,
            table_path,
            manifest,
            workspace_root,
            banned_fragment,
            violations,
        );
    }

    for (key, item) in table.iter() {
        if let Some(child_table) = item.as_table() {
            table_path.push(key.to_owned());
            collect_table_violations(
                child_table,
                table_path,
                manifest,
                workspace_root,
                banned_fragment,
                violations,
            );
            table_path.pop();
        }
    }
}

fn collect_dependency_table_violations(
    table: &Table,
    table_path: &[String],
    manifest: &Path,
    workspace_root: &Path,
    banned_fragment: &str,
    violations: &mut Vec<String>,
) {
    for (dependency_key, item) in table.iter() {
        push_violation_if_banned(
            dependency_key,
            table_path,
            manifest,
            workspace_root,
            banned_fragment,
            violations,
        );

        if let Some(package_name) = package_rename(item) {
            push_violation_if_banned(
                package_name,
                table_path,
                manifest,
                workspace_root,
                banned_fragment,
                violations,
            );
        }
    }
}

fn push_violation_if_banned(
    dependency_name: &str,
    table_path: &[String],
    manifest: &Path,
    workspace_root: &Path,
    banned_fragment: &str,
    violations: &mut Vec<String>,
) {
    if normalize_dependency_name(dependency_name).contains(banned_fragment) {
        violations.push(format!(
            "{} [{}]: {}",
            display_path(manifest, workspace_root),
            table_path.join("."),
            dependency_name
        ));
    }
}

fn is_production_dependency_table(table_path: &[String]) -> bool {
    if table_path
        .iter()
        .any(|segment| segment == "dev-dependencies")
    {
        return false;
    }

    match table_path {
        [section] => is_production_dependency_section(section),
        [workspace, section] => {
            workspace == "workspace" && is_production_dependency_section(section)
        }
        [target, _cfg, section] => target == "target" && is_production_dependency_section(section),
        _ => false,
    }
}

fn is_production_dependency_section(section: &str) -> bool {
    matches!(section, "dependencies" | "build-dependencies")
}

fn package_rename(item: &Item) -> Option<&str> {
    if let Some(table) = item.as_table() {
        return table
            .get("package")
            .and_then(Item::as_value)
            .and_then(Value::as_str);
    }

    item.as_value()
        .and_then(Value::as_inline_table)
        .and_then(|table| table.get("package"))
        .and_then(Value::as_str)
}

fn normalize_dependency_name(name: &str) -> String {
    name.replace(['-', '_'], "").to_ascii_lowercase()
}

fn display_path(path: &Path, workspace_root: &Path) -> String {
    path.strip_prefix(workspace_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

struct ConkitManifest<'a> {
    document: &'a DocumentMut,
}

impl<'a> ConkitManifest<'a> {
    fn new(document: &'a DocumentMut) -> Self {
        Self { document }
    }

    fn binary_names(&self) -> Vec<String> {
        self.document
            .get("bin")
            .and_then(Item::as_array_of_tables)
            .map(|bins| {
                bins.iter()
                    .filter_map(|bin| {
                        bin.get("name")
                            .and_then(Item::as_value)
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn has_feature(&self, feature: &str) -> bool {
        self.document
            .get("features")
            .and_then(Item::as_table)
            .is_some_and(|features| features.contains_key(feature))
    }
}
