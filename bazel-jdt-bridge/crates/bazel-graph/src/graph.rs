use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use std::collections::{HashMap, HashSet, VecDeque};

/// Dependency graph of Bazel targets
pub struct DependencyGraph {
    graph: DiGraph<String, ()>,
    label_to_index: HashMap<String, NodeIndex>,
    /// JARs associated with each target
    target_jars: HashMap<String, Vec<String>>,
    /// Source JAR mappings per target: {target_label → {binary_jar_path → source_jar_path}}
    target_source_jars: HashMap<String, HashMap<String, String>>,
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
            target_source_jars: HashMap::new(),
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

    /// Associate JARs with a target
    pub fn set_target_jars(&mut self, label: &str, jars: Vec<String>) {
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
    pub fn get_target_jars(&self, label: &str) -> Option<&Vec<String>> {
        if let Some(jars) = self.target_jars.get(label) {
            return Some(jars);
        }
        if let Some(canonical) = self.label_aliases.get(label) {
            return self.target_jars.get(canonical);
        }
        None
    }

    /// Get the source JAR path for a specific binary JAR of a target.
    /// Resolves through label_aliases like get_target_jars().
    pub fn get_target_source_jar(&self, label: &str, binary_jar: &str) -> Option<String> {
        if let Some(sources) = self.target_source_jars.get(label) {
            if let Some(src) = sources.get(binary_jar) {
                return Some(src.clone());
            }
        }
        if let Some(canonical) = self.label_aliases.get(label) {
            if let Some(sources) = self.target_source_jars.get(canonical) {
                return sources.get(binary_jar).cloned();
            }
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
        self.target_source_jars.clear();
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
                    .filter_map(|j| {
                        let path = j.jar.best_path()?;
                        Some(if j.jar.is_source {
                            resolve_external_path(&path, workspace_root).unwrap_or(path)
                        } else {
                            path
                        })
                    })
                    .collect();

                if jars.is_empty() {
                    jars = java_info
                        .compile_jars
                        .iter()
                        .filter_map(|j| {
                            let path = j.best_path()?;
                            Some(if j.is_source {
                                resolve_external_path(&path, workspace_root).unwrap_or(path)
                            } else {
                                path
                            })
                        })
                        .collect();
                }

                if !jars.is_empty() {
                    self.set_target_jars(label, jars);
                }

                let mut source_map: HashMap<String, String> = HashMap::new();
                for jar_info in &java_info.jars {
                    if let (Some(bin_path), Some(src_path)) = (
                        jar_info.jar.best_path(),
                        jar_info.source_jar.as_ref().and_then(|s| s.best_path()),
                    ) {
                        let src_is_source =
                            jar_info.source_jar.as_ref().map_or(true, |s| s.is_source);
                        let resolved_src = if src_is_source {
                            resolve_external_path(&src_path, workspace_root).unwrap_or(src_path)
                        } else {
                            src_path
                        };
                        source_map.insert(bin_path, resolved_src);
                    }
                }

                // When jars is empty (compile_jars fallback for java_import targets),
                // match source_jars to compile_jars by parent directory (Maven coordinates).
                if source_map.is_empty() && !java_info.source_jars.is_empty() {
                    let source_by_dir: HashMap<String, String> = java_info
                        .source_jars
                        .iter()
                        .filter_map(|s| {
                            let p = s.best_path()?;
                            let resolved = if s.is_source {
                                resolve_external_path(&p, workspace_root).unwrap_or(p)
                            } else {
                                p
                            };
                            Some((parent_key(&resolved), resolved))
                        })
                        .collect();

                    for bin_jar in self.target_jars.get(label).into_iter().flatten() {
                        if let Some(src_path) = source_by_dir.get(&parent_key(bin_jar)) {
                            source_map.insert(bin_jar.clone(), src_path.clone());
                        }
                    }
                }

                if !source_map.is_empty() {
                    log::debug!(
                        "[bazel-jdt] source_map for '{}': {} entries",
                        label,
                        source_map.len()
                    );
                    for (bin, src) in &source_map {
                        log::trace!("[bazel-jdt]   {} -> {}", bin, src);
                    }
                    self.target_source_jars.insert(label.clone(), source_map);
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
                self.target_source_jars.remove(label);
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
    pub fn transitive_dependency_packages(&self, target_label: &str) -> Result<Vec<String>, GraphError> {
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
    pub fn transitive_dependency_targets(&self, target_labels: &[&str]) -> Result<Vec<String>, GraphError> {
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
        assert_eq!(jars[0], "/second.jar");
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
        assert_eq!(jar_list[0], "/guava.jar");
    }

    #[test]
    fn test_populate_from_aspects_jars_preferred_over_compile_jars() {
        let mut graph = DependencyGraph::new();

        let target = TargetIdeInfo {
            label: "//lib:mylib".to_string(),
            kind: "java_library".to_string(),
            build_file: None,
            java_info: Some(JavaIdeInfo {
                jars: vec![JarInfo {
                    jar: ArtifactLocation {
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
            jars[0], "/output.jar",
            "Expected `jars` field to take precedence over `compile_jars`"
        );
    }

    #[test]
    fn test_populate_from_aspects_extracts_source_jars() {
        let mut graph = DependencyGraph::new();
        let target = TargetIdeInfo {
            label: "//lib:utils".to_string(),
            kind: "java_library".to_string(),
            build_file: None,
            java_info: Some(JavaIdeInfo {
                jars: vec![
                    JarInfo {
                        jar: ArtifactLocation {
                            absolute_path: Some("/output.jar".to_string()),
                            ..Default::default()
                        },
                        source_jar: Some(ArtifactLocation {
                            absolute_path: Some("/output-sources.jar".to_string()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    JarInfo {
                        jar: ArtifactLocation {
                            absolute_path: Some("/extra.jar".to_string()),
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

        graph.populate_from_aspects(&[target], Path::new("/workspace"));

        assert_eq!(
            graph.get_target_source_jar("//lib:utils", "/output.jar"),
            Some("/output-sources.jar".to_string()),
            "Expected source JAR mapping for /output.jar"
        );
        assert_eq!(
            graph.get_target_source_jar("//lib:utils", "/extra.jar"),
            None,
            "Expected no source JAR for /extra.jar (no source_jar in JarInfo)"
        );
    }

    #[test]
    fn test_compile_jars_fallback_matches_source_jars_by_directory() {
        let mut graph = DependencyGraph::new();
        let target = TargetIdeInfo {
            label: "@maven//:guava".to_string(),
            kind: "java_import".to_string(),
            build_file: None,
            java_info: Some(JavaIdeInfo {
                jars: vec![],
                compile_jars: vec![ArtifactLocation {
                    absolute_path: Some(
                        "external/maven/guava/33.4.0-jre/processed_guava-33.4.0-jre.jar"
                            .to_string(),
                    ),
                    ..Default::default()
                }],
                source_jars: vec![ArtifactLocation {
                    absolute_path: Some(
                        "external/maven/guava/33.4.0-jre/guava-33.4.0-jre-sources.jar".to_string(),
                    ),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            deps: vec![],
            runtime_deps: Vec::new(),
            exports: Vec::new(),
        };

        graph.populate_from_aspects(&[target], Path::new("/workspace"));

        let bin_path = "external/maven/guava/33.4.0-jre/processed_guava-33.4.0-jre.jar";
        let src_path = "external/maven/guava/33.4.0-jre/guava-33.4.0-jre-sources.jar";
        assert_eq!(
            graph.get_target_source_jar("@maven//:guava", bin_path),
            Some(src_path.to_string()),
            "Expected source JAR matched by parent directory for java_import compile_jars fallback"
        );
    }

    #[test]
    fn test_get_target_source_jar_resolves_alias() {
        let mut graph = DependencyGraph::new();
        let canonical = "@@rules_jvm_external~maven~maven//:guava";
        let apparent = "@maven//:guava";

        let mut source_map = HashMap::new();
        source_map.insert("/guava.jar".to_string(), "/guava-sources.jar".to_string());
        graph
            .target_source_jars
            .insert(canonical.to_string(), source_map);
        graph
            .label_aliases
            .insert(apparent.to_string(), canonical.to_string());

        assert_eq!(
            graph.get_target_source_jar(apparent, "/guava.jar"),
            Some("/guava-sources.jar".to_string()),
            "Expected source JAR via alias resolution"
        );
        assert_eq!(
            graph.get_target_source_jar(canonical, "/guava.jar"),
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
        assert_eq!(graph.get_target_jars("//foo:lib").unwrap()[0], "/foo.jar");

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

        graph.set_target_jars("//foo:old", vec!["/old.jar".to_string()]);

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
        assert_eq!(common_count, 1, "common should appear exactly once, got: {:?}", packages);
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
        let resolved = &jars[0];
        assert!(
            resolved.contains("/execroot/external/org_example/jar/downloaded.jar"),
            "Expected JAR resolved to execroot, got: {}",
            resolved
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
            jars[0], jar_path,
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
            jars[0], stale_path,
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
            jars[0], derived_path,
            "Derived artifact path should be preserved without resolve_external_path mangling"
        );
        assert!(
            jars[0].contains("bazel-out/k8-fastbuild/bin/external/maven"),
            "bazel-out/<config>/bin/ prefix must be retained, got: {}",
            jars[0]
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

        let resolved_src = graph.get_target_source_jar("//app:app", &bin_path).unwrap();
        assert_eq!(
            resolved_src, src_path,
            "Derived source attachment should not be resolved via resolve_external_path"
        );
        assert!(
            resolved_src.contains("bazel-out/k8-fastbuild/bin/external/maven"),
            "bazel-out prefix must be retained in source attachment, got: {}",
            resolved_src
        );
    }
}
