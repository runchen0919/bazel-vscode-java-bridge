use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::{HashMap, HashSet, VecDeque};

/// Dependency graph of Bazel targets
pub struct DependencyGraph {
    graph: DiGraph<String, ()>,
    label_to_index: HashMap<String, NodeIndex>,
    /// JARs associated with each target
    target_jars: HashMap<String, Vec<String>>,
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
        }
    }

    /// Add a target node to the graph
    pub fn add_target(&mut self, label: &str) {
        if !self.label_to_index.contains_key(label) {
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
    }

    /// Get JARs for a target
    pub fn get_target_jars(&self, label: &str) -> Option<&Vec<String>> {
        self.target_jars.get(label)
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
    }

    /// Populate graph from Bazel aspect output (primary data source).
    /// Iterates aspect results, creating nodes, edges, and JAR associations.
    pub fn populate_from_aspects(&mut self, results: &[bazel_aspect::TargetIdeInfo]) {
        for info in results {
            let label = &info.label;
            self.add_target(label);

            if let Some(ref java_info) = info.java_info {
                let jars: Vec<String> = java_info
                    .jars
                    .iter()
                    .filter_map(|j| j.jar.best_path().map(|p| p.to_string()))
                    .collect();
                if !jars.is_empty() {
                    self.set_target_jars(label, jars);
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

impl Default for DependencyGraph {
    fn default() -> Self {
        Self::new()
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

    #[test]
    fn test_populate_from_aspects_basic() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//foo:lib", vec!["//bar:util"], vec!["/path/lib.jar"]),
            make_target("//bar:util", vec![], vec!["/path/util.jar"]),
            make_target("//baz:app", vec!["//foo:lib"], vec![]),
        ];

        graph.populate_from_aspects(&results);

        assert_eq!(graph.target_count(), 3);
        assert!(graph.has_target("//foo:lib"));
        assert!(graph.has_target("//bar:util"));
        assert!(graph.has_target("//baz:app"));

        let deps = graph.transitive_deps("//foo:lib").unwrap();
        assert!(deps.contains(&"//bar:util".to_string()));

        let jars = graph.get_target_jars("//foo:lib").unwrap();
        assert_eq!(jars.len(), 1);
        assert_eq!(jars[0], "/path/lib.jar");
    }

    #[test]
    fn test_populate_from_aspects_empty() {
        let mut graph = DependencyGraph::new();
        graph.populate_from_aspects(&[]);
        assert_eq!(graph.target_count(), 0);
    }

    #[test]
    fn test_populate_from_aspects_duplicate_target() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//foo:lib", vec![], vec!["/first.jar"]),
            make_target("//foo:lib", vec![], vec!["/second.jar"]),
        ];

        graph.populate_from_aspects(&results);

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
        graph.populate_from_aspects(&results);
        assert_eq!(graph.target_count(), 1);
        assert!(!graph.has_target("//old:target"));
        assert!(graph.has_target("//new:target"));
    }
}
