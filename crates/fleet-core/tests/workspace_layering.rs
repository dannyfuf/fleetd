//! The crate graph and manifest conventions of `docs/ARCHITECTURE.md:52-53`, as a test.
//!
//! The direction (`core <- proto <- {term, client, cli} <- {daemon, app}`, `ui-kit` on gpui
//! alone, `lazygit` never on `core`/`proto`) and the "inherit every dependency" rule were
//! prose only, so the first violating edge or version literal would land green. This lives in
//! `fleet-core` because it is the leaf crate: asserting on Cargo metadata must not drag a
//! GPUI-heavy build in with it.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    process::Command,
};

/// Edges that must never exist, even transitively, through normal dependencies.
const FORBIDDEN: &[(&str, &str)] = &[
    ("fleet-lazygit", "fleet-core"),
    ("fleet-lazygit", "fleet-proto"),
    ("fleet-ui-kit", "fleet-core"),
    ("fleet-core", "fleet-proto"),
    ("fleet-daemon", "fleet-app"),
];

/// The dependency tables a version literal is forbidden in; `[package] version` is not one.
const DEPENDENCY_TABLES: &[&str] = &["dependencies", "dev-dependencies", "build-dependencies"];

#[test]
fn layering_rules_hold() {
    let graph = workspace_graph();
    let mut violations = Vec::new();
    for &(from, to) in FORBIDDEN {
        if let Some(path) = dependency_path(&graph, from, to) {
            violations.push(path.join(" -> "));
        }
    }
    assert_eq!(
        violations,
        Vec::<String>::new(),
        "forbidden dependency path (docs/ARCHITECTURE.md:52); extract the shared type downward \
         instead of adding the edge"
    );
}

#[test]
fn crate_manifests_inherit_lints_and_dependency_versions() {
    let mut violations = Vec::new();
    for manifest in crate_manifests() {
        let text = std::fs::read_to_string(&manifest)
            .unwrap_or_else(|error| panic!("read {}: {error}", manifest.display()));
        let name = manifest.display().to_string();
        if !inherits_workspace_lints(&text) {
            violations.push(format!("{name}: missing `[lints]` + `workspace = true`"));
        }
        for line in dependency_lines(&text) {
            if let Some(literal) = version_literal(&line) {
                violations.push(format!(
                    "{name}: `{literal}` pins a version outside the workspace"
                ));
            }
        }
    }
    assert_eq!(
        violations,
        Vec::<String>::new(),
        "every crate inherits `[workspace.lints]` and declares dependencies as \
         `foo.workspace = true`; add the version to `[workspace.dependencies]` instead"
    );
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap_or_else(|| panic!("workspace root above {}", env!("CARGO_MANIFEST_DIR")))
        .to_path_buf()
}

fn crate_manifests() -> Vec<PathBuf> {
    let crates = workspace_root().join("crates");
    let mut manifests: Vec<PathBuf> = std::fs::read_dir(&crates)
        .unwrap_or_else(|error| panic!("read {}: {error}", crates.display()))
        .map(|entry| entry.unwrap_or_else(|error| panic!("read {}: {error}", crates.display())))
        .map(|entry| entry.path().join("Cargo.toml"))
        .filter(|manifest| manifest.is_file())
        .collect();
    manifests.sort();
    assert!(!manifests.is_empty(), "no crate manifests under crates/");
    manifests
}

/// `name -> normal (non-dev, non-build) `fleet-*` dependencies`, from the workspace manifests.
fn workspace_graph() -> HashMap<String, Vec<String>> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--no-deps",
            "--offline",
            "--format-version",
            "1",
        ])
        .current_dir(workspace_root())
        .output()
        .unwrap_or_else(|error| panic!("run cargo metadata: {error}"));
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("parse cargo metadata: {error}"));
    let packages = metadata["packages"]
        .as_array()
        .unwrap_or_else(|| panic!("cargo metadata has no packages array"));
    packages
        .iter()
        .map(|package| {
            let name = package["name"]
                .as_str()
                .unwrap_or_else(|| panic!("package without a name"))
                .to_owned();
            let dependencies = package["dependencies"]
                .as_array()
                .unwrap_or_else(|| panic!("{name} has no dependencies array"))
                .iter()
                // Dev-dependencies are exempt: tests may cross boundaries.
                .filter(|dependency| dependency["kind"].is_null())
                .filter_map(|dependency| dependency["name"].as_str())
                .filter(|dependency| dependency.starts_with("fleet-"))
                .map(str::to_owned)
                .collect();
            (name, dependencies)
        })
        .collect()
}

/// The shortest `from -> … -> to` path through normal dependencies, if one exists.
fn dependency_path(
    graph: &HashMap<String, Vec<String>>,
    from: &str,
    to: &str,
) -> Option<Vec<String>> {
    let mut seen: HashSet<&str> = HashSet::from([from]);
    let mut queue = VecDeque::from([vec![from.to_owned()]]);
    while let Some(path) = queue.pop_front() {
        let last = path.last().unwrap_or_else(|| panic!("path is never empty"));
        for dependency in graph.get(last.as_str()).map_or(&[][..], Vec::as_slice) {
            if dependency == to {
                let mut found = path.clone();
                found.push(dependency.clone());
                return Some(found);
            }
            if seen.insert(dependency.as_str()) {
                let mut next = path.clone();
                next.push(dependency.clone());
                queue.push_back(next);
            }
        }
    }
    None
}

fn inherits_workspace_lints(manifest: &str) -> bool {
    let mut in_lints = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_lints = line == "[lints]";
            continue;
        }
        if in_lints && line == "workspace = true" {
            return true;
        }
    }
    false
}

/// The entry lines of every dependency table, including `[target.'…'.dependencies]`.
fn dependency_lines(manifest: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut in_dependencies = false;
    for line in manifest.lines() {
        let line = line.trim();
        if let Some(header) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            let table = header.rsplit('.').next().unwrap_or(header);
            in_dependencies = DEPENDENCY_TABLES.contains(&table);
            continue;
        }
        if in_dependencies && !line.is_empty() && !line.starts_with('#') {
            lines.push(line.to_owned());
        }
    }
    lines
}

/// A dependency entry that pins its own version rather than inheriting the workspace's.
fn version_literal(line: &str) -> Option<&str> {
    let (name, value) = line.split_once('=')?;
    if value.contains("version = \"") || value.trim_start().starts_with('"') {
        return Some(name.trim());
    }
    None
}
