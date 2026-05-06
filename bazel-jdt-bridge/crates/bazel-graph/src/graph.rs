use petgraph::graph::{DiGraph, NodeIndex};
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
                    .filter_map(|j| j.jar.best_path())
                    .collect();

                if jars.is_empty() {
                    jars = java_info
                        .compile_jars
                        .iter()
                        .filter_map(|j| j.best_path())
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
                        let resolved_src =
                            resolve_external_path(&src_path, workspace_root).unwrap_or(src_path);
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
                            s.best_path().map(|p| {
                                let resolved =
                                    resolve_external_path(&p, workspace_root).unwrap_or(p);
                                (parent_key(&resolved), resolved)
                            })
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

                for dep in &info.deps {
                    self.add_dep(label, dep);
                }
                for dep in &info.runtime_deps {
                    self.add_dep(label, dep);
                }
                for exp in &info.exports {
                    self.add_dep(label, exp);
                }
            } else {
                for dep in &info.deps {
                    self.add_dep(label, dep);
                }
            }
        }
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
                self.add_dep(&target_label, dep);
            }
            for dep in &rule.runtime_deps {
                self.add_dep(&target_label, dep);
            }
            for exp in &rule.exports {
                self.add_dep(&target_label, exp);
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
}
