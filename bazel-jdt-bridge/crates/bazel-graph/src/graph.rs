use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use std::collections::{HashMap, HashSet, VecDeque};

/// A classpath JAR paired with its resolved source attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedJar {
    pub classpath_path: String,
    pub source_path: Option<String>,
}

/// Dependency graph of Bazel targets
pub struct DependencyGraph {
    graph: DiGraph<String, ()>,
    label_to_index: HashMap<String, NodeIndex>,
    /// JARs associated with each target, each carrying its resolved source attachment
    target_jars: HashMap<String, Vec<ResolvedJar>>,
    /// Targets that have `testonly = True` in their Bazel rule definition.
    /// These targets can only be depended on by test targets.
    testonly_targets: HashSet<String>,
    /// Maps apparent external repo labels to canonical bzlmod labels.
    /// e.g. `@maven//:guava` → `@@rules_jvm_external~maven~maven//:guava`
    pub(crate) label_aliases: HashMap<String, String>,
}

/// Error for graph operations
#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("Circular dependency detected: {path}")]
    CircularDependency { path: String },

    #[error("Target not found: {label}")]
    TargetNotFound { label: String },
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            label_to_index: HashMap::new(),
            target_jars: HashMap::new(),
            testonly_targets: HashSet::new(),
            label_aliases: HashMap::new(),
        }
    }

    /// Add a target node to the graph
    pub fn add_target(&mut self, label: &str) {
        if !self.label_to_index.contains_key(label) {
            if let Some(canonical) = self.label_aliases.get(label) {
                if self.label_to_index.contains_key(canonical) {
                    return;
                }
            }
            let idx = self.graph.add_node(label.to_string());
            self.label_to_index.insert(label.to_string(), idx);
        }
    }

    /// Add a directed dependency edge
    pub fn add_dep(&mut self, from: &str, to: &str) {
        self.add_target(from);
        self.add_target(to);

        let from_idx = self.label_to_index[from];
        let to_idx = self.label_to_index[to];

        // Avoid duplicate edges
        if !self.graph.contains_edge(from_idx, to_idx) {
            self.graph.add_edge(from_idx, to_idx, ());
        }
    }

    /// Associate resolved JARs (with source attachments) with a target
    pub fn set_target_jars(&mut self, label: &str, jars: Vec<ResolvedJar>) {
        self.add_target(label);
        self.target_jars.insert(label.to_string(), jars);
    }

    /// Get all transitive dependencies via BFS
    pub fn transitive_deps(&self, label: &str) -> Result<Vec<String>, GraphError> {
        let start = self
            .label_to_index
            .get(label)
            .ok_or_else(|| GraphError::TargetNotFound {
                label: label.to_string(),
            })?;

        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut result = Vec::new();
        let mut path = vec![label.to_string()];

        queue.push_back(*start);

        while let Some(node) = queue.pop_front() {
            if visited.contains(&node) {
                continue;
            }
            visited.insert(node);

            let node_label = &self.graph[node];
            if node_label != label {
                result.push(node_label.clone());
            }

            for neighbor in self.graph.neighbors(node) {
                // Circular dependency detection - check BEFORE visited to catch cycles back to start
                // This detects cycles where a dependency eventually leads back to the starting target
                if neighbor == *start {
                    path.push(self.graph[neighbor].clone());
                    return Err(GraphError::CircularDependency {
                        path: path.join(" -> "),
                    });
                }
                if visited.contains(&neighbor) {
                    continue;
                }
                queue.push_back(neighbor);
            }
        }

        Ok(result)
    }

    /// Check if a target exists in the graph
    pub fn has_target(&self, label: &str) -> bool {
        self.label_to_index.contains_key(label)
            || self
                .label_aliases
                .get(label)
                .map(|c| self.label_to_index.contains_key(c))
                .unwrap_or(false)
    }

    /// Get JARs for a target, resolving through the alias map if needed
    pub fn get_target_jars(&self, label: &str) -> Option<&Vec<ResolvedJar>> {
        if let Some(jars) = self.target_jars.get(label) {
            return Some(jars);
        }
        if let Some(canonical) = self.label_aliases.get(label) {
            return self.target_jars.get(canonical);
        }
        None
    }

    /// Check if a target has `testonly = True`.
    pub fn is_testonly(&self, label: &str) -> bool {
        self.testonly_targets.contains(label)
    }

    /// Get all target labels
    pub fn all_targets(&self) -> Vec<String> {
        self.label_to_index.keys().cloned().collect()
    }

    /// Clear the graph
    pub fn clear(&mut self) {
        self.graph = DiGraph::new();
        self.label_to_index.clear();
        self.target_jars.clear();
        self.testonly_targets.clear();
        self.label_aliases.clear();
    }

    /// Populate graph from Bazel aspect output (primary data source).
    /// Iterates aspect results, creating nodes, edges, and JAR associations.
    pub fn populate_from_aspects(
        &mut self,
        results: &[bazel_aspect::TargetIdeInfo],
        workspace_root: &std::path::Path,
    ) {
        log::debug!(
            "[bazel-jdt] populate_from_aspects called with workspace_root='{}'",
            workspace_root.display()
        );
        for info in results {
            let label = &info.label;
            self.add_target(label);

            if let Some(apparent) = bazel_aspect::canonical_to_apparent_label(label) {
                self.label_aliases
                    .entry(apparent)
                    .or_insert_with(|| label.clone());
            }

            if info.kind == "java_test" {
                self.testonly_targets.insert(label.clone());
            }

            if let Some(ref java_info) = info.java_info {
                let mut jars: Vec<String> = java_info
                    .jars
                    .iter()
                    .filter_map(|j| normalize_artifact_path(&j.jar, workspace_root))
                    .collect();

                let compile_jars: Vec<String> = java_info
                    .compile_jars
                    .iter()
                    .filter_map(|j| normalize_artifact_path(j, workspace_root))
                    .collect();

                let jars_all_derived_internal = !jars.is_empty()
                    && java_info
                        .jars
                        .iter()
                        .all(|j| !j.jar.is_source && !j.jar.is_external);
                if jars.is_empty() || (jars_all_derived_internal && !compile_jars.is_empty()) {
                    jars = compile_jars;
                }

                if !jars.is_empty() {
                    let resolved = build_resolved_jars(java_info, &jars, workspace_root, label);
                    self.set_target_jars(label, resolved);
                }

                let pkg = package_of(label);
                for dep in &info.deps {
                    let resolved = normalize_dep_label(dep, pkg);
                    self.add_dep(label, &resolved);
                }
                for dep in &info.runtime_deps {
                    let resolved = normalize_dep_label(dep, pkg);
                    self.add_dep(label, &resolved);
                }
                for exp in &info.exports {
                    let resolved = normalize_dep_label(exp, pkg);
                    self.add_dep(label, &resolved);
                }
            } else {
                let pkg = package_of(label);
                for dep in &info.deps {
                    let resolved = normalize_dep_label(dep, pkg);
                    self.add_dep(label, &resolved);
                }
            }
        }
    }

    /// Get all target labels belonging to a specific Bazel package.
    /// Matches labels of the form `//package/path:name`.
    pub fn targets_in_package(&self, package_label: &str) -> Vec<String> {
        let prefix = format!("{}:", package_label);
        self.label_to_index
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect()
    }

    /// Find all targets that directly depend on the given target.
    /// Uses reverse edge traversal (Direction::Incoming).
    pub fn direct_dependers(&self, label: &str) -> Vec<String> {
        let idx = match self.label_to_index.get(label) {
            Some(&i) => i,
            None => return Vec::new(),
        };
        self.graph
            .neighbors_directed(idx, Direction::Incoming)
            .map(|n| self.graph[n].clone())
            .collect()
    }

    /// Find all targets that transitively depend on the given target (reverse BFS).
    /// Returns labels of all ancestors reachable via incoming edges, excluding
    /// external labels (`@`-prefixed) and Bazel-internal labels.
    pub fn reverse_transitive_deps(&self, label: &str) -> Vec<String> {
        let start = match self.label_to_index.get(label) {
            Some(&i) => i,
            None => return Vec::new(),
        };

        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut result = Vec::new();

        queue.push_back(start);

        while let Some(node) = queue.pop_front() {
            if visited.contains(&node) {
                continue;
            }
            visited.insert(node);

            for neighbor in self.graph.neighbors_directed(node, Direction::Incoming) {
                if !visited.contains(&neighbor) {
                    let dep_label = &self.graph[neighbor];
                    if dep_label != label {
                        result.push(dep_label.clone());
                    }
                    queue.push_back(neighbor);
                }
            }
        }

        result
    }

    /// Surgically update graph from a newly parsed BUILD file.
    ///
    /// Compared to `populate_from_parsed` which only adds, this method:
    /// - Removes targets that no longer exist in the BUILD file
    /// - Updates dependency edges for modified targets (preserving JAR data)
    /// - Adds new targets with their dependency edges
    ///
    /// Returns `(added_labels, removed_labels, modified_labels)`.
    pub fn update_from_parsed(
        &mut self,
        new_parsed: &bazel_parser::model::ParsedBuildFile,
        workspace_root: &std::path::Path,
    ) -> (Vec<String>, Vec<String>, Vec<String>) {
        let package_label = compute_package_label_from_build_path(&new_parsed.path, workspace_root);

        let existing_labels: HashSet<String> = self
            .targets_in_package(&package_label)
            .into_iter()
            .collect();

        let new_labels: HashMap<String, &bazel_parser::model::JavaRule> = new_parsed
            .rules
            .iter()
            .map(|r| (format!("{}:{}", package_label, r.name), r))
            .collect();
        let new_label_set: HashSet<String> = new_labels.keys().cloned().collect();

        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut modified = Vec::new();

        for label in &existing_labels {
            if !new_label_set.contains(label) {
                if let Some(idx) = self.label_to_index.remove(label) {
                    self.graph.remove_node(idx);
                }
                self.target_jars.remove(label);
                self.testonly_targets.remove(label);
                removed.push(label.clone());
            }
        }

        for (label, rule) in &new_labels {
            if existing_labels.contains(label) {
                if let Some(&idx) = self.label_to_index.get(label) {
                    let edge_ids: Vec<_> = self.graph.edges(idx).map(|e| e.id()).collect();
                    for eid in edge_ids {
                        self.graph.remove_edge(eid);
                    }
                }

                for dep in &rule.deps {
                    let resolved = normalize_dep_label(dep, &package_label);
                    self.add_dep(label, &resolved);
                }
                for dep in &rule.runtime_deps {
                    let resolved = normalize_dep_label(dep, &package_label);
                    self.add_dep(label, &resolved);
                }
                for exp in &rule.exports {
                    let resolved = normalize_dep_label(exp, &package_label);
                    self.add_dep(label, &resolved);
                }

                if rule.test_only {
                    self.testonly_targets.insert(label.clone());
                } else {
                    self.testonly_targets.remove(label);
                }

                modified.push(label.clone());
            }
        }

        for (label, rule) in &new_labels {
            if !existing_labels.contains(label) {
                self.add_target(label);
                for dep in &rule.deps {
                    let resolved = normalize_dep_label(dep, &package_label);
                    self.add_dep(label, &resolved);
                }
                for dep in &rule.runtime_deps {
                    let resolved = normalize_dep_label(dep, &package_label);
                    self.add_dep(label, &resolved);
                }
                for exp in &rule.exports {
                    let resolved = normalize_dep_label(exp, &package_label);
                    self.add_dep(label, &resolved);
                }
                if rule.test_only {
                    self.testonly_targets.insert(label.clone());
                }
                added.push(label.clone());
            }
        }

        (added, removed, modified)
    }

    /// Populate graph from a parsed BUILD file.
    /// Computes the Bazel package label from the file path (relative to workspace root)
    /// and creates target nodes and dependency edges from Java rules.
    pub fn populate_from_parsed(
        &mut self,
        file: &bazel_parser::model::ParsedBuildFile,
        workspace_root: &std::path::Path,
    ) {
        let package_label = compute_package_label_from_build_path(&file.path, workspace_root);
        for rule in &file.rules {
            let target_label = format!("{}:{}", package_label, rule.name);
            self.add_target(&target_label);
            if rule.test_only {
                self.testonly_targets.insert(target_label.clone());
            }
            for dep in &rule.deps {
                let resolved = normalize_dep_label(dep, &package_label);
                self.add_dep(&target_label, &resolved);
            }
            for dep in &rule.runtime_deps {
                let resolved = normalize_dep_label(dep, &package_label);
                self.add_dep(&target_label, &resolved);
            }
            for exp in &rule.exports {
                let resolved = normalize_dep_label(exp, &package_label);
                self.add_dep(&target_label, &resolved);
            }
        }
    }

    /// Populate graph from multiple parsed BUILD files.
    pub fn populate_from_parsed_batch(
        &mut self,
        files: &[bazel_parser::model::ParsedBuildFile],
        workspace_root: &std::path::Path,
    ) {
        for file in files {
            self.populate_from_parsed(file, workspace_root);
        }
    }

    /// Returns the number of targets in the graph.
    pub fn target_count(&self) -> usize {
        self.label_to_index.len()
    }

    /// Get deduplicated package paths for all workspace-internal transitive dependencies.
    /// Returns package paths (e.g., "utils", "service") not full labels.
    pub fn transitive_dependency_packages(
        &self,
        target_label: &str,
    ) -> Result<Vec<String>, GraphError> {
        let deps = self.transitive_deps(target_label)?;
        let mut packages: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for dep in &deps {
            let dep = crate::normalize_label(dep);
            if !dep.starts_with('@') && !crate::is_bazel_internal_label(&dep) {
                let pkg = package_path(&dep);
                if seen.insert(pkg.clone()) {
                    packages.push(pkg);
                }
            }
        }
        Ok(packages)
    }

    /// Get workspace-internal transitive dependency packages with their actual target labels.
    /// Returns entries formatted as "package_path|//pkg:target1,//pkg:target2".
    /// Each entry represents one unique package with all its dependency targets.
    pub fn transitive_dependency_targets(
        &self,
        target_labels: &[&str],
    ) -> Result<Vec<String>, GraphError> {
        use std::collections::HashMap;
        let mut pkg_targets: HashMap<String, Vec<String>> = HashMap::new();
        for label in target_labels {
            let deps = self.transitive_deps(label)?;
            for dep in &deps {
                let dep = crate::normalize_label(dep);
                if !dep.starts_with('@') && !crate::is_bazel_internal_label(&dep) {
                    let pkg = package_path(&dep);
                    pkg_targets.entry(pkg).or_default().push(dep.clone());
                }
            }
        }
        let mut result: Vec<String> = Vec::new();
        for (pkg, targets) in &pkg_targets {
            result.push(format!("{}|{}", pkg, targets.join(",")));
        }
        Ok(result)
    }
}

/// Extract the package label from a fully-qualified target label.
/// E.g., `//foo/bar:baz` → `//foo/bar`, `//foo/bar` → `//foo/bar`.
fn package_of(label: &str) -> &str {
    match label.rfind(':') {
        Some(i) if label.starts_with("//") => &label[..i],
        _ => label,
    }
}

fn package_path(label: &str) -> String {
    let pkg = package_of(label);
    pkg.strip_prefix("//").unwrap_or(pkg).to_string()
}

/// Normalize a dep label relative to a package label.
///
/// Resolves relative deps to fully-qualified form:
/// - `":target"` → `"//package:target"`
/// - `"target"` (bare name) → `"//package:target"`
/// - `"//already/qualified:target"` → unchanged
/// - `"@external//pkg:target"` → unchanged
pub fn normalize_dep_label(dep: &str, package_label: &str) -> String {
    if dep.starts_with("//") || dep.starts_with('@') {
        return normalize_label(dep);
    }
    if let Some(target) = dep.strip_prefix(':') {
        format!("{}:{}", package_label, target)
    } else {
        format!("{}:{}", package_label, dep)
    }
}

/// Normalize a Bazel label to canonical form.
///
/// Converts package-only labels to include the implicit target name:
/// - `"//foo/bar"` → `"//foo/bar:bar"`
/// - `"//foo/bar:baz"` → unchanged
/// - `"@maven//:guava"` → unchanged
pub fn normalize_label(label: &str) -> String {
    if !label.starts_with("//") {
        return label.to_string();
    }
    if label[2..].contains(':') {
        return label.to_string();
    }
    let last_component = label.rsplit('/').next().unwrap_or("");
    if last_component.is_empty() {
        return label.to_string();
    }
    format!("{}:{}", label, last_component)
}

/// Compute Bazel package label from a BUILD file path relative to workspace root.
/// e.g., `/workspace/foo/bar/BUILD` with root `/workspace` → `//foo/bar`
fn compute_package_label_from_build_path(
    path: &std::path::Path,
    workspace_root: &std::path::Path,
) -> String {
    if let Ok(relative) = path.parent().unwrap_or(path).strip_prefix(workspace_root) {
        let rel_str = relative.to_string_lossy().replace('\\', "/");
        if rel_str.is_empty() {
            "//".to_string()
        } else {
            format!("//{}", rel_str)
        }
    } else {
        let parent = path.parent().unwrap_or(path);
        let path_str = parent.to_string_lossy().replace('\\', "/");
        if path_str.is_empty() {
            "//".to_string()
        } else {
            format!("//{}", path_str)
        }
    }
}

fn parent_key(path: &str) -> String {
    std::path::Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn normalize_artifact_path(
    artifact: &bazel_aspect::ArtifactLocation,
    workspace_root: &std::path::Path,
) -> Option<String> {
    let path = artifact.best_path()?;
    Some(if artifact.is_source {
        resolve_external_path(&path, workspace_root).unwrap_or(path)
    } else {
        path
    })
}

/// Build `Vec<ResolvedJar>` using a 6-strategy source resolution chain.
fn build_resolved_jars(
    java_info: &bazel_aspect::JavaIdeInfo,
    classpath_jars: &[String],
    workspace_root: &std::path::Path,
    label: &str,
) -> Vec<ResolvedJar> {
    let mut class_to_source: HashMap<String, String> = HashMap::new();
    let mut interface_to_source: HashMap<String, String> = HashMap::new();
    let mut all_source_jars: HashSet<String> = HashSet::new();

    for jar_info in &java_info.jars {
        if let Some(src_loc) = jar_info.source_jar.as_ref() {
            if let Some(src_path) = src_loc.best_path() {
                let resolved_src = if src_loc.is_source {
                    resolve_external_path(&src_path, workspace_root).unwrap_or(src_path)
                } else {
                    absolutize_source_path(&src_path, workspace_root)
                };
                all_source_jars.insert(resolved_src.clone());

                if let Some(key) = normalize_artifact_path(&jar_info.jar, workspace_root) {
                    class_to_source.insert(key, resolved_src.clone());
                }
                if let Some(iface_loc) = jar_info.interface_jar.as_ref() {
                    if let Some(key) = normalize_artifact_path(iface_loc, workspace_root) {
                        interface_to_source.insert(key, resolved_src.clone());
                    }
                }
            }
        }
    }

    let source_by_dir: HashMap<String, String> = java_info
        .source_jars
        .iter()
        .filter_map(|s| {
            let p = s.best_path()?;
            let resolved = if s.is_source {
                resolve_external_path(&p, workspace_root).unwrap_or(p)
            } else {
                absolutize_source_path(&p, workspace_root)
            };
            all_source_jars.insert(resolved.clone());
            Some((parent_key(&resolved), resolved))
        })
        .collect();

    let single_source = if all_source_jars.len() == 1 && classpath_jars.len() == 1 {
        all_source_jars.into_iter().next()
    } else {
        None
    };

    let ws_str = workspace_root.to_str().unwrap_or("");

    let mut stubs_discarded: u32 = 0;
    let mut maven_cache_hits: u32 = 0;
    let mut with_source: u32 = 0;

    let result = classpath_jars
        .iter()
        .map(|jar_path| {
            let mut strategy = "";
            let source_path = class_to_source
                .get(jar_path)
                .cloned()
                .and_then(|p| validate_source_jar(&p))
                .map(|p| {
                    strategy = "class_to_source";
                    p
                })
                .or_else(|| {
                    interface_to_source
                        .get(jar_path)
                        .cloned()
                        .and_then(|p| validate_source_jar(&p))
                        .map(|p| {
                            strategy = "interface_to_source";
                            p
                        })
                })
                .or_else(|| {
                    source_by_dir
                        .get(&parent_key(jar_path))
                        .cloned()
                        .and_then(|p| validate_source_jar(&p))
                        .map(|p| {
                            strategy = "source_by_dir";
                            p
                        })
                })
                .or_else(|| {
                    single_source
                        .clone()
                        .and_then(|p| validate_source_jar(&p))
                        .map(|p| {
                            strategy = "single_source";
                            p
                        })
                })
                .or_else(|| {
                    if !label.starts_with('@') {
                        infer_source_attachment(label, Some(ws_str)).map(|p| {
                            strategy = "infer_source";
                            p
                        })
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    probe_source_jar_by_convention(jar_path)
                        .and_then(|p| validate_source_jar(&p))
                        .map(|p| {
                            strategy = "convention_probe";
                            p
                        })
                })
                .or_else(|| {
                    let (g, a, v) = extract_maven_coordinates(jar_path)?;
                    let result = probe_maven_local_cache(&g, &a, &v);
                    if result.is_some() {
                        strategy = "maven_cache";
                        maven_cache_hits += 1;
                    }
                    result
                });

            if let Some(ref src) = source_path {
                log::info!(
                    "[bazel-jdt] {} -> source via {}: {}",
                    jar_path,
                    strategy,
                    src
                );
            }

            if source_path.is_none()
                && class_to_source.contains_key(jar_path)
                && validate_source_jar(class_to_source.get(jar_path).unwrap()).is_none()
            {
                stubs_discarded += 1;
            }

            if source_path.is_some() {
                with_source += 1;
            }

            ResolvedJar {
                classpath_path: jar_path.clone(),
                source_path,
            }
        })
        .collect();

    log::info!(
        "[bazel-jdt] Source resolution for '{}': {}/{} with source, {} stubs discarded, {} maven cache hits",
        label,
        with_source,
        classpath_jars.len(),
        stubs_discarded,
        maven_cache_hits
    );

    result
}

fn validate_source_jar(path: &str) -> Option<String> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() < 1024 => None,
        Ok(_) => Some(path.to_string()),
        Err(_) => None,
    }
}

fn extract_maven_coordinates(jar_path: &str) -> Option<(String, String, String)> {
    let maven_idx = jar_path
        .rfind("/maven2/")
        .map(|i| i + 8)
        .or_else(|| jar_path.rfind("/maven/").map(|i| i + 7))?;
    let after_maven = &jar_path[maven_idx..];

    let parts: Vec<&str> = after_maven.split('/').collect();
    if parts.len() < 4 {
        return None;
    }

    let filename = *parts.last()?;
    if !filename.ends_with(".jar") {
        return None;
    }

    let version = parts[parts.len() - 2];
    let artifact_id = parts[parts.len() - 3];
    let group_parts = &parts[..parts.len() - 3];
    if group_parts.is_empty() {
        return None;
    }
    let group_id = group_parts.join(".");

    Some((group_id, artifact_id.to_string(), version.to_string()))
}

fn probe_maven_local_cache(group_id: &str, artifact_id: &str, version: &str) -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let group_path = group_id.replace('.', "/");
    let candidate = std::path::PathBuf::from(&home)
        .join(".m2/repository")
        .join(&group_path)
        .join(artifact_id)
        .join(version)
        .join(format!("{}-{}-sources.jar", artifact_id, version));
    if candidate.exists() {
        Some(candidate.to_string_lossy().into_owned())
    } else {
        None
    }
}

fn absolutize_source_path(path: &str, workspace_root: &std::path::Path) -> String {
    if path.starts_with('/') {
        return path.to_string();
    }
    if path.starts_with("bazel-out/") {
        return workspace_root.join(path).to_string_lossy().into_owned();
    }
    path.to_string()
}

fn probe_source_jar_by_convention(jar_path: &str) -> Option<String> {
    let path = std::path::Path::new(jar_path);
    let stem = path.file_stem()?.to_str()?;
    let parent = path.parent()?;
    let candidate = parent.join(format!("{}-sources.jar", stem));
    if candidate.exists() {
        return Some(candidate.to_string_lossy().into_owned());
    }
    None
}

fn pkg_contains_java_content(dir: &std::path::Path) -> bool {
    dir.read_dir()
        .ok()
        .map(|mut entries| {
            entries.any(|e| {
                e.map(|e| {
                    let name = e.file_name();
                    let name_str = name.to_string_lossy();
                    name_str.ends_with(".java")
                })
                .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

pub(crate) fn infer_source_attachment(
    dep_label: &str,
    workspace_root: Option<&str>,
) -> Option<String> {
    let ws_root = workspace_root?;
    let label = dep_label.strip_prefix("//")?;
    let package_path = label.split(':').next().unwrap_or(label);
    if package_path.is_empty() {
        return None;
    }

    let source_root_markers = ["src/main/java", "src/test/java", "src/java", "java"];
    let pkg = std::path::Path::new(ws_root).join(package_path);

    for marker in &source_root_markers {
        let candidate = pkg.join(marker);
        if candidate.is_dir() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }

    if pkg.is_dir() && pkg_contains_java_content(&pkg) {
        return Some(pkg.to_string_lossy().into_owned());
    }

    let substring_markers = [
        "src/main/java/",
        "src/test/java/",
        "src/java/",
        "javatests/",
        "java/",
    ];
    for marker in &substring_markers {
        if let Some(idx) = package_path.find(marker) {
            let root = &package_path[..idx + marker.len() - 1];
            return Some(format!("{}/{}", ws_root, root));
        }
    }

    None
}

impl Default for DependencyGraph {
    fn default() -> Self {
        Self::new()
    }
}

fn resolve_external_path(path: &str, workspace_root: &std::path::Path) -> Option<String> {
    let relative = if path.starts_with("external/") {
        path.to_string()
    } else if path.contains("/external/") && path.starts_with(workspace_root.to_str()?) {
        let idx = path.find("/external/")?;
        path[idx + 1..].to_string()
    } else {
        return None;
    };

    let bazel_out = workspace_root.join("bazel-out");
    match std::fs::canonicalize(&bazel_out) {
        Ok(resolved) => {
            if let Some(execroot) = resolved.parent() {
                let candidate = execroot.join(&relative);
                if candidate.exists() {
                    let result = candidate.to_string_lossy().into_owned();
                    log::debug!(
                        "[bazel-jdt] resolve_external_path: '{}' -> '{}' (execroot)",
                        path,
                        result
                    );
                    return Some(result);
                }
                if let Some(output_base) = execroot.parent().and_then(|p| p.parent()) {
                    let candidate = output_base.join(&relative);
                    if candidate.exists() {
                        let result = candidate.to_string_lossy().into_owned();
                        log::debug!(
                            "[bazel-jdt] resolve_external_path: '{}' -> '{}' (output_base)",
                            path,
                            result
                        );
                        return Some(result);
                    }
                }
                let result = execroot.join(&relative).to_string_lossy().into_owned();
                log::debug!(
                    "[bazel-jdt] resolve_external_path: '{}' -> '{}' (execroot, file not yet found)",
                    path, result
                );
                Some(result)
            } else {
                log::debug!(
                    "[bazel-jdt] resolve_external_path: no parent for bazel-out '{}'",
                    resolved.display()
                );
                None
            }
        }
        Err(e) => {
            log::debug!(
                "[bazel-jdt] resolve_external_path: canonicalize('{}') failed: {}",
                bazel_out.display(),
                e
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bazel_aspect::{ArtifactLocation, JarInfo, JavaIdeInfo, TargetIdeInfo};
    use bazel_parser::model::{JavaRule, LoadStatement, ParsedBuildFile, RuleType};
    use std::path::{Path, PathBuf};

    fn make_target(label: &str, deps: Vec<&str>, jar_paths: Vec<&str>) -> TargetIdeInfo {
        let jars: Vec<JarInfo> = jar_paths
            .iter()
            .map(|p| JarInfo {
                jar: ArtifactLocation {
                    absolute_path: Some(p.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            })
            .collect();

        TargetIdeInfo {
            label: label.to_string(),
            kind: "java_library".to_string(),
            build_file: None,
            java_info: if jars.is_empty() && deps.is_empty() {
                None
            } else {
                Some(JavaIdeInfo {
                    jars,
                    ..Default::default()
                })
            },
            deps: deps.iter().map(|s| s.to_string()).collect(),
            runtime_deps: Vec::new(),
            exports: Vec::new(),
        }
    }

    fn make_target_with_compile_jars(
        label: &str,
        deps: Vec<&str>,
        compile_jar_paths: Vec<&str>,
    ) -> TargetIdeInfo {
        let compile_jars: Vec<ArtifactLocation> = compile_jar_paths
            .iter()
            .map(|p| ArtifactLocation {
                absolute_path: Some(p.to_string()),
                ..Default::default()
            })
            .collect();

        TargetIdeInfo {
            label: label.to_string(),
            kind: "java_import".to_string(),
            build_file: None,
            java_info: Some(JavaIdeInfo {
                jars: vec![],
                compile_jars,
                ..Default::default()
            }),
            deps: deps.iter().map(|s| s.to_string()).collect(),
            runtime_deps: Vec::new(),
            exports: Vec::new(),
        }
    }

    #[test]
    fn test_populate_from_aspects_basic() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//foo:lib", vec!["//bar:util"], vec!["/path/lib.jar"]),
            make_target("//bar:util", vec![], vec!["/path/util.jar"]),
            make_target("//baz:app", vec!["//foo:lib"], vec![]),
        ];

        graph.populate_from_aspects(&results, Path::new("/workspace"));
    }

    #[test]
    fn test_populate_from_aspects_empty() {
        let mut graph = DependencyGraph::new();
        graph.populate_from_aspects(&[], Path::new("/workspace"));
        assert_eq!(graph.target_count(), 0);
    }

    #[test]
    fn test_populate_from_aspects_duplicate_target() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//foo:lib", vec![], vec!["/first.jar"]),
            make_target("//foo:lib", vec![], vec!["/second.jar"]),
        ];

        graph.populate_from_aspects(&results, Path::new("/workspace"));

        assert_eq!(graph.target_count(), 1);
        let jars = graph.get_target_jars("//foo:lib").unwrap();
        assert_eq!(jars[0].classpath_path, "/second.jar");
    }

    #[test]
    fn test_populate_from_parsed_basic() {
        let mut graph = DependencyGraph::new();
        let workspace_root = PathBuf::from("/workspace");
        let parsed = ParsedBuildFile {
            path: PathBuf::from("/workspace/foo/bar/BUILD"),
            content_hash: String::new(),
            rules: vec![JavaRule {
                rule_type: RuleType::JavaLibrary,
                name: "mylib".to_string(),
                srcs: vec!["MyLib.java".to_string()],
                deps: vec!["//common:utils".to_string()],
                runtime_deps: vec!["//runtime:dep".to_string()],
                resources: vec![],
                plugins: vec![],
                exports: vec![],
                test_only: false,
                visibility: vec![],
            }],
            loads: vec![LoadStatement {
                path: "@rules_java//java:defs.bzl".to_string(),
                symbols: vec!["java_library".to_string()],
            }],
        };

        graph.populate_from_parsed(&parsed, &workspace_root);

        assert_eq!(graph.target_count(), 3);
        assert!(graph.has_target("//foo/bar:mylib"));
        assert!(graph.has_target("//common:utils"));
        assert!(graph.has_target("//runtime:dep"));

        let deps = graph.transitive_deps("//foo/bar:mylib").unwrap();
        assert!(deps.contains(&"//common:utils".to_string()));
    }

    #[test]
    fn test_populate_from_parsed_no_java_rules() {
        let mut graph = DependencyGraph::new();
        let parsed = ParsedBuildFile {
            path: PathBuf::from("/workspace/cc/BUILD"),
            content_hash: String::new(),
            rules: vec![],
            loads: vec![],
        };

        graph.populate_from_parsed(&parsed, Path::new("/workspace"));
        assert_eq!(graph.target_count(), 0);
    }

    #[test]
    fn test_populate_from_parsed_batch() {
        let mut graph = DependencyGraph::new();
        let files = vec![
            ParsedBuildFile {
                path: PathBuf::from("/workspace/a/BUILD"),
                content_hash: String::new(),
                rules: vec![JavaRule {
                    rule_type: RuleType::JavaLibrary,
                    name: "lib_a".to_string(),
                    srcs: vec![],
                    deps: vec![],
                    runtime_deps: vec![],
                    resources: vec![],
                    plugins: vec![],
                    exports: vec![],
                    test_only: false,
                    visibility: vec![],
                }],
                loads: vec![],
            },
            ParsedBuildFile {
                path: PathBuf::from("/workspace/b/BUILD"),
                content_hash: String::new(),
                rules: vec![JavaRule {
                    rule_type: RuleType::JavaLibrary,
                    name: "lib_b".to_string(),
                    srcs: vec![],
                    deps: vec!["//a:lib_a".to_string()],
                    runtime_deps: vec![],
                    resources: vec![],
                    plugins: vec![],
                    exports: vec![],
                    test_only: false,
                    visibility: vec![],
                }],
                loads: vec![],
            },
        ];

        graph.populate_from_parsed_batch(&files, Path::new("/workspace"));

        assert_eq!(graph.target_count(), 2);
        assert!(graph.has_target("//a:lib_a"));
        assert!(graph.has_target("//b:lib_b"));
    }

    #[test]
    fn test_clear_and_repopulate() {
        let mut graph = DependencyGraph::new();
        graph.add_target("//old:target");
        assert_eq!(graph.target_count(), 1);

        graph.clear();
        assert_eq!(graph.target_count(), 0);

        let results = vec![make_target("//new:target", vec![], vec![])];
        graph.populate_from_aspects(&results, Path::new("/workspace"));
        assert_eq!(graph.target_count(), 1);
        assert!(!graph.has_target("//old:target"));
        assert!(graph.has_target("//new:target"));
    }

    #[test]
    fn test_testonly_from_aspect_java_test() {
        let mut graph = DependencyGraph::new();
        let mut test_target = make_target("//foo:my_test", vec!["//foo:lib"], vec![]);
        test_target.kind = "java_test".to_string();
        let results = vec![
            test_target,
            make_target("//foo:lib", vec![], vec!["/lib.jar"]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));
        assert!(graph.is_testonly("//foo:my_test"));
        assert!(!graph.is_testonly("//foo:lib"));
    }

    #[test]
    fn test_testonly_from_parsed_build_file() {
        let mut graph = DependencyGraph::new();
        let workspace_root = PathBuf::from("/workspace");
        let parsed = ParsedBuildFile {
            path: PathBuf::from("/workspace/pkg/BUILD"),
            content_hash: String::new(),
            rules: vec![
                JavaRule {
                    rule_type: RuleType::JavaLibrary,
                    name: "lib".to_string(),
                    srcs: vec![],
                    deps: vec![],
                    runtime_deps: vec![],
                    resources: vec![],
                    plugins: vec![],
                    exports: vec![],
                    test_only: true,
                    visibility: vec![],
                },
                JavaRule {
                    rule_type: RuleType::JavaLibrary,
                    name: "public_lib".to_string(),
                    srcs: vec![],
                    deps: vec![],
                    runtime_deps: vec![],
                    resources: vec![],
                    plugins: vec![],
                    exports: vec![],
                    test_only: false,
                    visibility: vec![],
                },
            ],
            loads: vec![],
        };
        graph.populate_from_parsed(&parsed, &workspace_root);
        assert!(graph.is_testonly("//pkg:lib"));
        assert!(!graph.is_testonly("//pkg:public_lib"));
    }

    #[test]
    fn test_clear_resets_testonly() {
        let mut graph = DependencyGraph::new();
        let mut test_target = make_target("//foo:test", vec![], vec![]);
        test_target.kind = "java_test".to_string();
        graph.populate_from_aspects(&[test_target], Path::new("/workspace"));
        assert!(graph.is_testonly("//foo:test"));
        graph.clear();
        assert!(!graph.is_testonly("//foo:test"));
    }

    #[test]
    fn test_populate_from_aspects_compile_jars_fallback() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//app:app", vec!["@maven//:guava"], vec!["/app.jar"]),
            make_target_with_compile_jars("@maven//:guava", vec![], vec!["/guava.jar"]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let jars = graph.get_target_jars("@maven//:guava");
        assert!(
            jars.is_some(),
            "Expected JAR data for @maven//:guava from compile_jars fallback"
        );
        let jar_list = jars.unwrap();
        assert_eq!(jar_list.len(), 1);
        assert_eq!(jar_list[0].classpath_path, "/guava.jar");
    }

    #[test]
    fn test_populate_from_aspects_source_jars_kept_over_compile_jars() {
        let mut graph = DependencyGraph::new();

        let target = TargetIdeInfo {
            label: "//lib:mylib".to_string(),
            kind: "java_library".to_string(),
            build_file: None,
            java_info: Some(JavaIdeInfo {
                jars: vec![JarInfo {
                    jar: ArtifactLocation {
                        is_source: true,
                        absolute_path: Some("/output.jar".to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                }],
                compile_jars: vec![ArtifactLocation {
                    absolute_path: Some("/compile.jar".to_string()),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            deps: vec![],
            runtime_deps: Vec::new(),
            exports: Vec::new(),
        };

        graph.populate_from_aspects(&[target], Path::new("/workspace"));

        let jars = graph.get_target_jars("//lib:mylib").unwrap();
        assert_eq!(jars.len(), 1);
        assert_eq!(
            jars[0].classpath_path, "/output.jar",
            "Source jars should be kept over compile_jars"
        );
    }

    #[test]
    fn test_jars_kept_when_compile_jars_empty() {
        let mut graph = DependencyGraph::new();

        let target = TargetIdeInfo {
            label: "//lib:plain".to_string(),
            kind: "java_library".to_string(),
            build_file: None,
            java_info: Some(JavaIdeInfo {
                jars: vec![JarInfo {
                    jar: ArtifactLocation {
                        is_source: false,
                        is_external: false,
                        absolute_path: Some("/libplain.jar".to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                }],
                compile_jars: vec![],
                ..Default::default()
            }),
            deps: vec![],
            runtime_deps: Vec::new(),
            exports: Vec::new(),
        };

        graph.populate_from_aspects(&[target], Path::new("/workspace"));

        let jars = graph.get_target_jars("//lib:plain").unwrap();
        assert_eq!(jars.len(), 1);
        assert_eq!(
            jars[0].classpath_path, "/libplain.jar",
            "jars should be kept when compile_jars is empty"
        );
    }

    #[test]
    fn test_populate_from_aspects_extracts_source_jars() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let output_jar_path = workspace.join("output.jar");
        let output_src_path = workspace.join("output-sources.jar");
        let extra_jar_path = workspace.join("extra.jar");
        std::fs::write(&output_jar_path, [0u8; 2048]).unwrap();
        std::fs::write(&output_src_path, [0u8; 2048]).unwrap();
        std::fs::write(&extra_jar_path, [0u8; 2048]).unwrap();

        let mut graph = DependencyGraph::new();
        let target = TargetIdeInfo {
            label: "//lib:utils".to_string(),
            kind: "java_library".to_string(),
            build_file: None,
            java_info: Some(JavaIdeInfo {
                jars: vec![
                    JarInfo {
                        jar: ArtifactLocation {
                            absolute_path: Some(output_jar_path.to_string_lossy().into_owned()),
                            ..Default::default()
                        },
                        source_jar: Some(ArtifactLocation {
                            absolute_path: Some(output_src_path.to_string_lossy().into_owned()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    JarInfo {
                        jar: ArtifactLocation {
                            absolute_path: Some(extra_jar_path.to_string_lossy().into_owned()),
                            ..Default::default()
                        },
                        source_jar: None,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }),
            deps: vec![],
            runtime_deps: Vec::new(),
            exports: Vec::new(),
        };

        graph.populate_from_aspects(&[target], &workspace);

        let jars = graph.get_target_jars("//lib:utils").unwrap();
        let output_jar = jars
            .iter()
            .find(|j| j.classpath_path == output_jar_path.to_string_lossy().as_ref())
            .expect("Expected output.jar");
        assert_eq!(
            output_jar.source_path.as_deref(),
            Some(output_src_path.to_str().unwrap()),
            "Expected source JAR mapping for output.jar"
        );
        let extra_jar = jars
            .iter()
            .find(|j| j.classpath_path == extra_jar_path.to_string_lossy().as_ref())
            .expect("Expected extra.jar");
        assert_eq!(
            extra_jar.source_path, None,
            "Expected no source JAR for extra.jar (no source_jar in JarInfo)"
        );
    }

    #[test]
    fn test_compile_jars_fallback_matches_source_jars_by_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let jar_dir = workspace.join("external/maven/guava/33.4.0-jre");
        std::fs::create_dir_all(&jar_dir).unwrap();
        let bin_file = jar_dir.join("processed_guava-33.4.0-jre.jar");
        let src_file = jar_dir.join("guava-33.4.0-jre-sources.jar");
        std::fs::write(&bin_file, [0u8; 2048]).unwrap();
        std::fs::write(&src_file, [0u8; 2048]).unwrap();

        let bin_path = bin_file.to_string_lossy().into_owned();
        let src_path = src_file.to_string_lossy().into_owned();

        let mut graph = DependencyGraph::new();
        let target = TargetIdeInfo {
            label: "@maven//:guava".to_string(),
            kind: "java_import".to_string(),
            build_file: None,
            java_info: Some(JavaIdeInfo {
                jars: vec![],
                compile_jars: vec![ArtifactLocation {
                    absolute_path: Some(bin_path.clone()),
                    ..Default::default()
                }],
                source_jars: vec![ArtifactLocation {
                    absolute_path: Some(src_path.clone()),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            deps: vec![],
            runtime_deps: Vec::new(),
            exports: Vec::new(),
        };

        graph.populate_from_aspects(&[target], &workspace);

        let jars = graph.get_target_jars("@maven//:guava").unwrap();
        let jar = jars
            .iter()
            .find(|j| j.classpath_path == bin_path)
            .expect("Expected compile_jar entry");
        assert_eq!(
            jar.source_path,
            Some(src_path),
            "Expected source JAR matched by parent directory for java_import compile_jars fallback"
        );
    }

    #[test]
    fn test_resolved_jar_source_via_alias() {
        let mut graph = DependencyGraph::new();
        let canonical = "@@rules_jvm_external~maven~maven//:guava";
        let apparent = "@maven//:guava";

        graph.add_target(canonical);
        graph.set_target_jars(
            canonical,
            vec![ResolvedJar {
                classpath_path: "/guava.jar".to_string(),
                source_path: Some("/guava-sources.jar".to_string()),
            }],
        );
        graph
            .label_aliases
            .insert(apparent.to_string(), canonical.to_string());

        let jars = graph.get_target_jars(apparent).unwrap();
        assert_eq!(
            jars[0].source_path,
            Some("/guava-sources.jar".to_string()),
            "Expected source JAR via alias resolution"
        );
        let jars2 = graph.get_target_jars(canonical).unwrap();
        assert_eq!(
            jars2[0].source_path,
            Some("/guava-sources.jar".to_string()),
            "Expected source JAR via direct canonical label"
        );
    }

    #[test]
    fn test_targets_in_package() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//foo:lib", vec![], vec!["/foo.jar"]),
            make_target("//foo:utils", vec![], vec!["/utils.jar"]),
            make_target("//bar:app", vec!["//foo:lib"], vec!["/app.jar"]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let foo_targets = graph.targets_in_package("//foo");
        assert_eq!(foo_targets.len(), 2);
        assert!(foo_targets.contains(&"//foo:lib".to_string()));
        assert!(foo_targets.contains(&"//foo:utils".to_string()));

        let bar_targets = graph.targets_in_package("//bar");
        assert_eq!(bar_targets.len(), 1);
        assert_eq!(bar_targets[0], "//bar:app");

        let empty = graph.targets_in_package("//nonexistent");
        assert!(empty.is_empty());
    }

    #[test]
    fn test_direct_dependers() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//app:main", vec!["//lib:core", "//lib:utils"], vec![]),
            make_target("//test:e2e", vec!["//lib:core"], vec![]),
            make_target("//lib:core", vec![], vec!["/core.jar"]),
            make_target("//lib:utils", vec![], vec!["/utils.jar"]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let core_dependers = graph.direct_dependers("//lib:core");
        assert_eq!(core_dependers.len(), 2);
        assert!(core_dependers.contains(&"//app:main".to_string()));
        assert!(core_dependers.contains(&"//test:e2e".to_string()));

        let utils_dependers = graph.direct_dependers("//lib:utils");
        assert_eq!(utils_dependers.len(), 1);
        assert_eq!(utils_dependers[0], "//app:main");

        let main_dependers = graph.direct_dependers("//app:main");
        assert!(main_dependers.is_empty());

        let unknown = graph.direct_dependers("//nonexistent:target");
        assert!(unknown.is_empty());
    }

    #[test]
    fn test_reverse_transitive_deps_simple() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//d:app", vec!["//b:svc"], vec![]),
            make_target("//b:svc", vec!["//a:lib"], vec![]),
            make_target("//a:lib", vec![], vec!["/lib.jar"]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let reverse = graph.reverse_transitive_deps("//a:lib");
        assert_eq!(reverse.len(), 2);
        assert!(reverse.contains(&"//b:svc".to_string()));
        assert!(reverse.contains(&"//d:app".to_string()));
    }

    #[test]
    fn test_reverse_transitive_deps_chain() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//d:app", vec!["//c:svc"], vec![]),
            make_target("//c:svc", vec!["//b:util"], vec![]),
            make_target("//b:util", vec!["//a:base"], vec![]),
            make_target("//a:base", vec![], vec!["/base.jar"]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let reverse = graph.reverse_transitive_deps("//a:base");
        assert_eq!(reverse.len(), 3);
        assert!(reverse.contains(&"//b:util".to_string()));
        assert!(reverse.contains(&"//c:svc".to_string()));
        assert!(reverse.contains(&"//d:app".to_string()));
    }

    #[test]
    fn test_update_from_parsed_deps_only_preserves_jars() {
        let mut graph = DependencyGraph::new();
        let workspace_root = PathBuf::from("/workspace");

        let aspect_results = vec![
            make_target("//foo:lib", vec!["//bar:util"], vec!["/foo.jar"]),
            make_target("//bar:util", vec![], vec!["/util.jar"]),
        ];
        graph.populate_from_aspects(&aspect_results, &workspace_root);

        assert!(graph.get_target_jars("//foo:lib").is_some());

        let new_parsed = ParsedBuildFile {
            path: PathBuf::from("/workspace/foo/BUILD"),
            content_hash: String::new(),
            rules: vec![JavaRule {
                rule_type: RuleType::JavaLibrary,
                name: "lib".to_string(),
                srcs: vec!["Lib.java".to_string()],
                deps: vec!["//baz:new".to_string()],
                runtime_deps: vec![],
                resources: vec![],
                plugins: vec![],
                exports: vec![],
                test_only: false,
                visibility: vec![],
            }],
            loads: vec![],
        };

        let (added, removed, modified) = graph.update_from_parsed(&new_parsed, &workspace_root);

        assert!(added.is_empty());
        assert!(removed.is_empty());
        assert_eq!(modified.len(), 1);
        assert_eq!(modified[0], "//foo:lib");

        assert!(
            graph.get_target_jars("//foo:lib").is_some(),
            "JAR data should be preserved after deps-only change"
        );
        assert_eq!(
            graph.get_target_jars("//foo:lib").unwrap()[0].classpath_path,
            "/foo.jar"
        );

        assert!(graph.has_target("//baz:new"));
    }

    #[test]
    fn test_update_from_parsed_target_removed() {
        let mut graph = DependencyGraph::new();
        let workspace_root = PathBuf::from("/workspace");

        let parsed = ParsedBuildFile {
            path: PathBuf::from("/workspace/foo/BUILD"),
            content_hash: String::new(),
            rules: vec![
                JavaRule {
                    rule_type: RuleType::JavaLibrary,
                    name: "lib".to_string(),
                    srcs: vec![],
                    deps: vec![],
                    runtime_deps: vec![],
                    resources: vec![],
                    plugins: vec![],
                    exports: vec![],
                    test_only: false,
                    visibility: vec![],
                },
                JavaRule {
                    rule_type: RuleType::JavaLibrary,
                    name: "old".to_string(),
                    srcs: vec![],
                    deps: vec![],
                    runtime_deps: vec![],
                    resources: vec![],
                    plugins: vec![],
                    exports: vec![],
                    test_only: false,
                    visibility: vec![],
                },
            ],
            loads: vec![],
        };
        graph.populate_from_parsed(&parsed, &workspace_root);
        assert_eq!(graph.target_count(), 2);

        graph.set_target_jars(
            "//foo:old",
            vec![ResolvedJar {
                classpath_path: "/old.jar".to_string(),
                source_path: None,
            }],
        );

        let new_parsed = ParsedBuildFile {
            path: PathBuf::from("/workspace/foo/BUILD"),
            content_hash: String::new(),
            rules: vec![JavaRule {
                rule_type: RuleType::JavaLibrary,
                name: "lib".to_string(),
                srcs: vec![],
                deps: vec![],
                runtime_deps: vec![],
                resources: vec![],
                plugins: vec![],
                exports: vec![],
                test_only: false,
                visibility: vec![],
            }],
            loads: vec![],
        };

        let (added, removed, modified) = graph.update_from_parsed(&new_parsed, &workspace_root);

        assert!(added.is_empty());
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0], "//foo:old");
        assert!(modified.contains(&"//foo:lib".to_string()));

        assert!(!graph.has_target("//foo:old"));
        assert!(graph.get_target_jars("//foo:old").is_none());
        assert!(graph.has_target("//foo:lib"));
    }

    #[test]
    fn test_update_from_parsed_target_added() {
        let mut graph = DependencyGraph::new();
        let workspace_root = PathBuf::from("/workspace");

        let parsed = ParsedBuildFile {
            path: PathBuf::from("/workspace/foo/BUILD"),
            content_hash: String::new(),
            rules: vec![JavaRule {
                rule_type: RuleType::JavaLibrary,
                name: "lib".to_string(),
                srcs: vec![],
                deps: vec![],
                runtime_deps: vec![],
                resources: vec![],
                plugins: vec![],
                exports: vec![],
                test_only: false,
                visibility: vec![],
            }],
            loads: vec![],
        };
        graph.populate_from_parsed(&parsed, &workspace_root);
        assert_eq!(graph.target_count(), 1);

        let new_parsed = ParsedBuildFile {
            path: PathBuf::from("/workspace/foo/BUILD"),
            content_hash: String::new(),
            rules: vec![
                JavaRule {
                    rule_type: RuleType::JavaLibrary,
                    name: "lib".to_string(),
                    srcs: vec![],
                    deps: vec![],
                    runtime_deps: vec![],
                    resources: vec![],
                    plugins: vec![],
                    exports: vec![],
                    test_only: false,
                    visibility: vec![],
                },
                JavaRule {
                    rule_type: RuleType::JavaLibrary,
                    name: "new_lib".to_string(),
                    srcs: vec![],
                    deps: vec!["//bar:util".to_string()],
                    runtime_deps: vec![],
                    resources: vec![],
                    plugins: vec![],
                    exports: vec![],
                    test_only: false,
                    visibility: vec![],
                },
            ],
            loads: vec![],
        };

        let (added, removed, modified) = graph.update_from_parsed(&new_parsed, &workspace_root);

        assert_eq!(added.len(), 1);
        assert_eq!(added[0], "//foo:new_lib");
        assert!(removed.is_empty());
        assert!(modified.contains(&"//foo:lib".to_string()));

        assert!(graph.has_target("//foo:new_lib"));
        assert!(graph.has_target("//bar:util"));
    }

    #[test]
    fn test_normalize_dep_label_bare_name() {
        assert_eq!(
            normalize_dep_label("helper", "//src/java/com/example"),
            "//src/java/com/example:helper"
        );
    }

    #[test]
    fn test_normalize_dep_label_colon_prefix() {
        assert_eq!(
            normalize_dep_label(":utils", "//src/java/com/example"),
            "//src/java/com/example:utils"
        );
    }

    #[test]
    fn test_normalize_dep_label_already_qualified() {
        assert_eq!(
            normalize_dep_label("//other/package:lib", "//src/java/com/example"),
            "//other/package:lib"
        );
    }

    #[test]
    fn test_normalize_dep_label_external() {
        assert_eq!(
            normalize_dep_label("@maven//:guava", "//src/java/com/example"),
            "@maven//:guava"
        );
    }

    #[test]
    fn test_normalize_dep_label_package_only() {
        assert_eq!(
            normalize_dep_label("//src/java/com/urbancompass/monitoring", "//irrelevant"),
            "//src/java/com/urbancompass/monitoring:monitoring"
        );
    }

    #[test]
    fn test_normalize_label_package_only() {
        assert_eq!(
            normalize_label("//src/java/com/urbancompass/monitoring"),
            "//src/java/com/urbancompass/monitoring:monitoring"
        );
    }

    #[test]
    fn test_normalize_label_already_canonical() {
        assert_eq!(normalize_label("//foo/bar:baz"), "//foo/bar:baz");
    }

    #[test]
    fn test_normalize_label_external() {
        assert_eq!(normalize_label("@maven//:guava"), "@maven//:guava");
    }

    #[test]
    fn test_normalize_label_external_canonical() {
        assert_eq!(
            normalize_label("@@rules_jvm_external~maven~maven//:guava"),
            "@@rules_jvm_external~maven~maven//:guava"
        );
    }

    #[test]
    fn test_normalize_label_root_package() {
        assert_eq!(normalize_label("//"), "//");
    }

    #[test]
    fn test_normalize_label_bare_and_relative() {
        assert_eq!(normalize_label("target"), "target");
        assert_eq!(normalize_label(":target"), ":target");
    }

    #[test]
    fn test_package_of() {
        assert_eq!(package_of("//foo/bar:baz"), "//foo/bar");
        assert_eq!(package_of("//foo/bar"), "//foo/bar");
        assert_eq!(package_of("//:root_target"), "//");
    }

    #[test]
    fn test_package_path_extraction() {
        assert_eq!(package_path("//utils:string_utils"), "utils");
        assert_eq!(package_path("//service:user_service"), "service");
        assert_eq!(package_path("//foo/bar:baz"), "foo/bar");
        assert_eq!(package_path("//:root_target"), "");
    }

    #[test]
    fn test_transitive_dependency_packages_basic() {
        let mut graph = DependencyGraph::new();
        graph.add_target("//app:app");
        graph.add_target("//utils:string_utils");
        graph.add_target("//service:user_service");
        graph.add_dep("//app:app", "//utils:string_utils");
        graph.add_dep("//app:app", "//service:user_service");

        let packages = graph.transitive_dependency_packages("//app:app").unwrap();
        assert_eq!(packages, vec!["service", "utils"]);
    }

    #[test]
    fn test_transitive_dependency_packages_filters_external() {
        let mut graph = DependencyGraph::new();
        graph.add_target("//app:app");
        graph.add_target("//utils:string_utils");
        graph.add_target("@maven//:guava");
        graph.add_dep("//app:app", "//utils:string_utils");
        graph.add_dep("//app:app", "@maven//:guava");

        let packages = graph.transitive_dependency_packages("//app:app").unwrap();
        assert_eq!(packages, vec!["utils"]);
    }

    #[test]
    fn test_transitive_dependency_packages_no_internal() {
        let mut graph = DependencyGraph::new();
        graph.add_target("//app:app");
        graph.add_target("@maven//:guava");
        graph.add_dep("//app:app", "@maven//:guava");

        let packages = graph.transitive_dependency_packages("//app:app").unwrap();
        assert!(packages.is_empty());
    }

    #[test]
    fn test_transitive_dependency_packages_diamond_dedup() {
        let mut graph = DependencyGraph::new();
        graph.add_target("//app:app");
        graph.add_target("//utils:string_utils");
        graph.add_target("//service:user_service");
        graph.add_target("//common:util");
        graph.add_dep("//app:app", "//utils:string_utils");
        graph.add_dep("//app:app", "//service:user_service");
        graph.add_dep("//utils:string_utils", "//common:util");
        graph.add_dep("//service:user_service", "//common:util");

        let packages = graph.transitive_dependency_packages("//app:app").unwrap();
        let common_count = packages.iter().filter(|p| **p == "common").count();
        assert_eq!(
            common_count, 1,
            "common should appear exactly once, got: {:?}",
            packages
        );
        assert!(packages.contains(&"common".to_string()));
        assert!(packages.contains(&"utils".to_string()));
        assert!(packages.contains(&"service".to_string()));
    }

    #[test]
    fn test_populate_resolves_external_binary_jar_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        // Simulate execroot at <tmp>/execroot with bazel-out symlinked
        let execroot = tmp.path().join("execroot");
        std::fs::create_dir_all(&execroot).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        // Create bazel-out inside execroot, then symlink from workspace
        let real_bazel_out = execroot.join("bazel-out");
        std::fs::create_dir_all(&real_bazel_out).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_bazel_out, workspace.join("bazel-out")).unwrap();
        // Create the external JAR at the execroot location
        let jar_dir = execroot.join("external/org_example/jar");
        std::fs::create_dir_all(&jar_dir).unwrap();
        std::fs::File::create(jar_dir.join("downloaded.jar")).unwrap();

        // Aspect provides a stale path pointing to workspace/external/...
        let stale_path = format!(
            "{}/external/org_example/jar/downloaded.jar",
            workspace.display()
        );
        let mut graph = DependencyGraph::new();
        let results = vec![TargetIdeInfo {
            label: "//app:app".to_string(),
            kind: "java_library".to_string(),
            build_file: None,
            java_info: Some(JavaIdeInfo {
                jars: vec![JarInfo {
                    jar: ArtifactLocation {
                        absolute_path: Some(stale_path),
                        is_source: true,
                        ..Default::default()
                    },
                    ..Default::default()
                }],
                ..Default::default()
            }),
            deps: vec![],
            runtime_deps: Vec::new(),
            exports: Vec::new(),
        }];
        graph.populate_from_aspects(&results, &workspace);

        let jars = graph.get_target_jars("//app:app").unwrap();
        assert_eq!(jars.len(), 1);
        assert!(
            jars[0]
                .classpath_path
                .contains("/execroot/external/org_example/jar/downloaded.jar"),
            "Expected JAR resolved to execroot, got: {}",
            jars[0].classpath_path
        );
    }

    #[test]
    fn test_populate_does_not_modify_non_external_jar_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        // No bazel-out symlink — resolve_external_path will return None

        let jar_path = "/some/bazel-out/k8-fastbuild/bin/lib/libfoo.jar".to_string();
        let mut graph = DependencyGraph::new();
        let results = vec![TargetIdeInfo {
            label: "//lib:foo".to_string(),
            kind: "java_library".to_string(),
            build_file: None,
            java_info: Some(JavaIdeInfo {
                jars: vec![JarInfo {
                    jar: ArtifactLocation {
                        absolute_path: Some(jar_path.clone()),
                        ..Default::default()
                    },
                    ..Default::default()
                }],
                ..Default::default()
            }),
            deps: vec![],
            runtime_deps: Vec::new(),
            exports: Vec::new(),
        }];
        graph.populate_from_aspects(&results, &workspace);

        let jars = graph.get_target_jars("//lib:foo").unwrap();
        assert_eq!(jars.len(), 1);
        assert_eq!(
            jars[0].classpath_path, jar_path,
            "Non-external JAR path should remain unchanged"
        );
    }

    #[test]
    fn test_populate_preserves_jar_path_when_bazel_out_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        // No bazel-out symlink — canonicalize will fail, resolve_external_path returns None

        let stale_path = format!(
            "{}/external/org_example/jar/downloaded.jar",
            workspace.display()
        );
        let mut graph = DependencyGraph::new();
        let results = vec![TargetIdeInfo {
            label: "//app:app".to_string(),
            kind: "java_library".to_string(),
            build_file: None,
            java_info: Some(JavaIdeInfo {
                jars: vec![JarInfo {
                    jar: ArtifactLocation {
                        absolute_path: Some(stale_path.clone()),
                        is_source: true,
                        ..Default::default()
                    },
                    ..Default::default()
                }],
                ..Default::default()
            }),
            deps: vec![],
            runtime_deps: Vec::new(),
            exports: Vec::new(),
        }];
        graph.populate_from_aspects(&results, &workspace);

        let jars = graph.get_target_jars("//app:app").unwrap();
        assert_eq!(jars.len(), 1);
        assert_eq!(
            jars[0].classpath_path, stale_path,
            "Original path should be preserved when resolve_external_path returns None"
        );
    }

    #[test]
    fn test_derived_artifact_jar_preserves_bazel_out_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        // Create bazel-out symlink so resolve_external_path would work if called
        let execroot = tmp.path().join("execroot");
        std::fs::create_dir_all(&execroot).unwrap();
        let real_bazel_out = execroot.join("bazel-out");
        std::fs::create_dir_all(&real_bazel_out).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_bazel_out, workspace.join("bazel-out")).unwrap();

        // Derived artifact path with bazel-out/<config>/bin/ prefix
        let derived_path = format!(
            "{}/bazel-out/k8-fastbuild/bin/external/maven/v1/https/repo/com/example/artifact-1.0.jar",
            workspace.display()
        );
        let mut graph = DependencyGraph::new();
        let results = vec![TargetIdeInfo {
            label: "//app:app".to_string(),
            kind: "java_library".to_string(),
            build_file: None,
            java_info: Some(JavaIdeInfo {
                jars: vec![JarInfo {
                    jar: ArtifactLocation {
                        absolute_path: Some(derived_path.clone()),
                        is_source: false,
                        ..Default::default()
                    },
                    ..Default::default()
                }],
                ..Default::default()
            }),
            deps: vec![],
            runtime_deps: Vec::new(),
            exports: Vec::new(),
        }];
        graph.populate_from_aspects(&results, &workspace);

        let jars = graph.get_target_jars("//app:app").unwrap();
        assert_eq!(jars.len(), 1);
        assert_eq!(
            jars[0].classpath_path, derived_path,
            "Derived artifact path should be preserved without resolve_external_path mangling"
        );
        assert!(
            jars[0]
                .classpath_path
                .contains("bazel-out/k8-fastbuild/bin/external/maven"),
            "bazel-out/<config>/bin/ prefix must be retained, got: {}",
            jars[0].classpath_path
        );
    }

    #[test]
    fn test_derived_source_attachment_not_resolved() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        let execroot = tmp.path().join("execroot");
        std::fs::create_dir_all(&execroot).unwrap();
        let real_bazel_out = execroot.join("bazel-out");
        std::fs::create_dir_all(&real_bazel_out).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_bazel_out, workspace.join("bazel-out")).unwrap();

        let bin_path = format!(
            "{}/bazel-out/k8-fastbuild/bin/external/maven/v1/artifact-1.0.jar",
            workspace.display()
        );
        let src_path = format!(
            "{}/bazel-out/k8-fastbuild/bin/external/maven/v1/artifact-1.0-sources.jar",
            workspace.display()
        );
        let jar_dir = std::path::Path::new(&bin_path).parent().unwrap();
        std::fs::create_dir_all(jar_dir).unwrap();
        std::fs::write(&bin_path, [0u8; 2048]).unwrap();
        std::fs::write(&src_path, [0u8; 2048]).unwrap();
        let mut graph = DependencyGraph::new();
        let results = vec![TargetIdeInfo {
            label: "//app:app".to_string(),
            kind: "java_library".to_string(),
            build_file: None,
            java_info: Some(JavaIdeInfo {
                jars: vec![JarInfo {
                    jar: ArtifactLocation {
                        absolute_path: Some(bin_path.clone()),
                        is_source: false,
                        ..Default::default()
                    },
                    source_jar: Some(ArtifactLocation {
                        absolute_path: Some(src_path.clone()),
                        is_source: false,
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            deps: vec![],
            runtime_deps: Vec::new(),
            exports: Vec::new(),
        }];
        graph.populate_from_aspects(&results, &workspace);

        let jars = graph.get_target_jars("//app:app").unwrap();
        let jar = jars
            .iter()
            .find(|j| j.classpath_path == bin_path)
            .expect("Expected bin jar");
        let resolved_src = jar.source_path.as_ref().unwrap();
        assert_eq!(
            resolved_src, &src_path,
            "Derived source attachment should not be resolved via resolve_external_path"
        );
        assert!(
            resolved_src.contains("bazel-out/k8-fastbuild/bin/external/maven"),
            "bazel-out prefix must be retained in source attachment, got: {}",
            resolved_src
        );
    }

    // --- absolutize_source_path tests ---

    #[test]
    fn test_absolutize_relative_bazel_out_path() {
        let ws = std::path::Path::new("/home/user/workspace");
        let result = absolutize_source_path("bazel-out/k8-fastbuild/bin/lib-src.jar", ws);
        assert_eq!(
            result,
            "/home/user/workspace/bazel-out/k8-fastbuild/bin/lib-src.jar"
        );
    }

    #[test]
    fn test_absolutize_already_absolute_path() {
        let ws = std::path::Path::new("/home/user/workspace");
        let result = absolutize_source_path("/absolute/path/to/src.jar", ws);
        assert_eq!(result, "/absolute/path/to/src.jar");
    }

    #[test]
    fn test_absolutize_non_bazel_out_relative_path() {
        let ws = std::path::Path::new("/home/user/workspace");
        let result = absolutize_source_path("external/maven/src.jar", ws);
        assert_eq!(result, "external/maven/src.jar");
    }

    #[test]
    fn test_populate_absolutizes_derived_source_jar() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let jar_dir = workspace.join("bazel-out/k8-fastbuild/bin/lib");
        std::fs::create_dir_all(&jar_dir).unwrap();
        std::fs::write(jar_dir.join("liblib.jar"), [0u8; 2048]).unwrap();
        std::fs::write(jar_dir.join("liblib-src.jar"), [0u8; 2048]).unwrap();

        let mut graph = DependencyGraph::new();
        let results = vec![TargetIdeInfo {
            label: "//lib:lib".to_string(),
            kind: "java_library".to_string(),
            build_file: None,
            java_info: Some(JavaIdeInfo {
                jars: vec![JarInfo {
                    jar: ArtifactLocation {
                        relative_path: Some("lib/liblib.jar".to_string()),
                        root_path: Some("bazel-out/k8-fastbuild/bin".to_string()),
                        is_source: false,
                        ..Default::default()
                    },
                    source_jar: Some(ArtifactLocation {
                        relative_path: Some("lib/liblib-src.jar".to_string()),
                        root_path: Some("bazel-out/k8-fastbuild/bin".to_string()),
                        is_source: false,
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            deps: vec![],
            runtime_deps: Vec::new(),
            exports: Vec::new(),
        }];
        graph.populate_from_aspects(&results, &workspace);

        let jars = graph.get_target_jars("//lib:lib").unwrap();
        let jar = &jars[0];
        let src = jar.source_path.as_ref().unwrap();
        assert!(
            src.starts_with(workspace.to_str().unwrap()),
            "Derived source path should be absolutized with workspace_root, got: {}",
            src
        );
        assert!(
            src.ends_with("lib/liblib-src.jar"),
            "Source path should end with the original relative path, got: {}",
            src
        );
    }

    // --- stub JAR detection tests ---

    #[test]
    fn test_stub_source_jar_discarded() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let stub_jar = workspace.join("stub-sources.jar");
        std::fs::write(&stub_jar, [0u8; 283]).unwrap();

        let classpath_jar = workspace.join("lib.jar");
        std::fs::write(&classpath_jar, [0u8; 5000]).unwrap();

        let java_info = JavaIdeInfo {
            jars: vec![JarInfo {
                jar: ArtifactLocation {
                    absolute_path: Some(classpath_jar.to_string_lossy().into_owned()),
                    is_source: false,
                    ..Default::default()
                },
                source_jar: Some(ArtifactLocation {
                    absolute_path: Some(stub_jar.to_string_lossy().into_owned()),
                    is_source: false,
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        };

        let classpath_jars = vec![classpath_jar.to_string_lossy().into_owned()];
        let resolved = build_resolved_jars(&java_info, &classpath_jars, &workspace, "//lib:lib");

        assert_eq!(resolved.len(), 1);
        assert!(
            resolved[0].source_path.is_none(),
            "Stub source JAR (< 1KB) should be discarded, got: {:?}",
            resolved[0].source_path
        );
    }

    #[test]
    fn test_real_source_jar_preserved() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let source_jar = workspace.join("real-sources.jar");
        std::fs::write(&source_jar, [0u8; 5000]).unwrap();

        let classpath_jar = workspace.join("lib.jar");
        std::fs::write(&classpath_jar, [0u8; 5000]).unwrap();

        let java_info = JavaIdeInfo {
            jars: vec![JarInfo {
                jar: ArtifactLocation {
                    absolute_path: Some(classpath_jar.to_string_lossy().into_owned()),
                    is_source: false,
                    ..Default::default()
                },
                source_jar: Some(ArtifactLocation {
                    absolute_path: Some(source_jar.to_string_lossy().into_owned()),
                    is_source: false,
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        };

        let classpath_jars = vec![classpath_jar.to_string_lossy().into_owned()];
        let resolved = build_resolved_jars(&java_info, &classpath_jars, &workspace, "//lib:lib");

        assert_eq!(resolved.len(), 1);
        assert_eq!(
            resolved[0].source_path.as_deref(),
            Some(source_jar.to_str().unwrap()),
            "Real source JAR (>= 1KB) should be preserved"
        );
    }

    #[test]
    fn test_nonexistent_source_jar_filtered() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let classpath_jar = workspace.join("lib.jar");
        std::fs::write(&classpath_jar, [0u8; 2048]).unwrap();
        let phantom_source = "/nonexistent/path/to/sources.jar".to_string();

        let java_info = JavaIdeInfo {
            jars: vec![JarInfo {
                jar: ArtifactLocation {
                    absolute_path: Some(classpath_jar.to_string_lossy().into_owned()),
                    is_source: false,
                    ..Default::default()
                },
                source_jar: Some(ArtifactLocation {
                    absolute_path: Some(phantom_source),
                    is_source: false,
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        };

        let classpath_jars = vec![classpath_jar.to_string_lossy().into_owned()];
        let resolved = build_resolved_jars(&java_info, &classpath_jars, &workspace, "//lib:lib");

        assert_eq!(resolved.len(), 1);
        assert!(
            resolved[0].source_path.is_none(),
            "Non-existent source path should be filtered so chain can try later strategies"
        );
    }

    // --- validate_source_jar tests ---

    #[test]
    fn test_validate_source_jar_stub() {
        let tmp = tempfile::tempdir().unwrap();
        let stub = tmp.path().join("stub.jar");
        std::fs::write(&stub, [0u8; 283]).unwrap();
        assert_eq!(validate_source_jar(stub.to_str().unwrap()), None);
    }

    #[test]
    fn test_validate_source_jar_real() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real.jar");
        std::fs::write(&real, [0u8; 5000]).unwrap();
        assert_eq!(
            validate_source_jar(real.to_str().unwrap()),
            Some(real.to_str().unwrap().to_string())
        );
    }

    #[test]
    fn test_validate_source_jar_nonexistent() {
        let result = validate_source_jar("/nonexistent/path/sources.jar");
        assert_eq!(result, None);
    }

    #[test]
    fn test_stub_bypassed_maven_cache_used() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let stub_jar = workspace.join("stub-sources.jar");
        std::fs::write(&stub_jar, [0u8; 283]).unwrap();

        let m2_path = tmp
            .path()
            .join(".m2/repository/com/google/guava/guava/28.2-jre");
        std::fs::create_dir_all(&m2_path).unwrap();
        let maven_source = m2_path.join("guava-28.2-jre-sources.jar");
        std::fs::write(&maven_source, [0u8; 5000]).unwrap();

        let classpath_jar_path = workspace
            .join("external/maven/v1/https/repo1.maven.org/maven2/com/google/guava/guava/28.2-jre/guava-28.2-jre.jar");
        std::fs::create_dir_all(classpath_jar_path.parent().unwrap()).unwrap();
        std::fs::write(&classpath_jar_path, [0u8; 5000]).unwrap();

        let java_info = JavaIdeInfo {
            jars: vec![JarInfo {
                jar: ArtifactLocation {
                    absolute_path: Some(classpath_jar_path.to_string_lossy().into_owned()),
                    is_source: false,
                    ..Default::default()
                },
                source_jar: Some(ArtifactLocation {
                    absolute_path: Some(stub_jar.to_string_lossy().into_owned()),
                    is_source: false,
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        };

        std::env::set_var("HOME", tmp.path());
        let classpath_jars = vec![classpath_jar_path.to_string_lossy().into_owned()];
        let resolved =
            build_resolved_jars(&java_info, &classpath_jars, &workspace, "@maven//:guava");

        assert_eq!(resolved.len(), 1);
        assert_eq!(
            resolved[0].source_path.as_deref(),
            Some(maven_source.to_str().unwrap()),
            "Should fall through stub to Maven cache source"
        );
    }

    #[test]
    fn test_stub_bypassed_no_fallback_gives_none() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let stub_jar = workspace.join("stub-sources.jar");
        std::fs::write(&stub_jar, [0u8; 283]).unwrap();

        let classpath_jar = workspace.join("lib.jar");
        std::fs::write(&classpath_jar, [0u8; 5000]).unwrap();

        let java_info = JavaIdeInfo {
            jars: vec![JarInfo {
                jar: ArtifactLocation {
                    absolute_path: Some(classpath_jar.to_string_lossy().into_owned()),
                    is_source: false,
                    ..Default::default()
                },
                source_jar: Some(ArtifactLocation {
                    absolute_path: Some(stub_jar.to_string_lossy().into_owned()),
                    is_source: false,
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        };

        std::env::set_var("HOME", tmp.path());
        let classpath_jars = vec![classpath_jar.to_string_lossy().into_owned()];
        let resolved = build_resolved_jars(&java_info, &classpath_jars, &workspace, "//lib:lib");

        assert_eq!(resolved.len(), 1);
        assert!(
            resolved[0].source_path.is_none(),
            "Stub with no fallback should give None, got: {:?}",
            resolved[0].source_path
        );
    }

    #[test]
    fn test_convention_stub_bypassed_to_maven_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let maven_jar_dir = workspace
            .join("external/maven/v1/https/repo1.maven.org/maven2/com/google/guava/guava/28.2-jre");
        std::fs::create_dir_all(&maven_jar_dir).unwrap();
        let classpath_jar = maven_jar_dir.join("guava-28.2-jre.jar");
        std::fs::write(&classpath_jar, [0u8; 5000]).unwrap();
        let convention_stub = maven_jar_dir.join("guava-28.2-jre-sources.jar");
        std::fs::write(&convention_stub, [0u8; 283]).unwrap();

        let m2_path = tmp
            .path()
            .join(".m2/repository/com/google/guava/guava/28.2-jre");
        std::fs::create_dir_all(&m2_path).unwrap();
        let maven_source = m2_path.join("guava-28.2-jre-sources.jar");
        std::fs::write(&maven_source, [0u8; 5000]).unwrap();

        let java_info = JavaIdeInfo {
            jars: vec![],
            ..Default::default()
        };

        std::env::set_var("HOME", tmp.path());
        let classpath_jars = vec![classpath_jar.to_string_lossy().into_owned()];
        let resolved =
            build_resolved_jars(&java_info, &classpath_jars, &workspace, "@maven//:guava");

        assert_eq!(resolved.len(), 1);
        assert_eq!(
            resolved[0].source_path.as_deref(),
            Some(maven_source.to_str().unwrap()),
            "Convention stub should be filtered, falling through to Maven cache"
        );
    }

    // --- Maven coordinate extraction tests ---

    #[test]
    fn test_extract_maven_coordinates_standard() {
        let path = "external/maven/v1/https/repo1.maven.org/maven2/com/google/guava/guava/28.2-jre/guava-28.2-jre.jar";
        let result = extract_maven_coordinates(path);
        assert_eq!(
            result,
            Some((
                "com.google.guava".to_string(),
                "guava".to_string(),
                "28.2-jre".to_string()
            ))
        );
    }

    #[test]
    fn test_extract_maven_coordinates_custom_host() {
        let path = "external/maven/v1/https/artifacts.company.net/repository/maven/org/slf4j/slf4j-api/1.7.36/slf4j-api-1.7.36.jar";
        let result = extract_maven_coordinates(path);
        assert_eq!(
            result,
            Some((
                "org.slf4j".to_string(),
                "slf4j-api".to_string(),
                "1.7.36".to_string()
            ))
        );
    }

    #[test]
    fn test_extract_maven_coordinates_non_maven_path() {
        let path = "bazel-out/k8-fastbuild/bin/lib/liblib.jar";
        assert_eq!(extract_maven_coordinates(path), None);
    }

    #[test]
    fn test_probe_maven_local_cache_found() {
        let tmp = tempfile::tempdir().unwrap();
        let m2_path = tmp
            .path()
            .join(".m2/repository/com/google/guava/guava/28.2-jre");
        std::fs::create_dir_all(&m2_path).unwrap();
        let sources_jar = m2_path.join("guava-28.2-jre-sources.jar");
        std::fs::write(&sources_jar, [0u8; 5000]).unwrap();

        std::env::set_var("HOME", tmp.path());
        let result = probe_maven_local_cache("com.google.guava", "guava", "28.2-jre");
        assert_eq!(result, Some(sources_jar.to_string_lossy().into_owned()));
    }

    #[test]
    fn test_probe_maven_local_cache_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", tmp.path());
        let result = probe_maven_local_cache("com.nonexistent", "artifact", "1.0");
        assert_eq!(result, None);
    }
}
