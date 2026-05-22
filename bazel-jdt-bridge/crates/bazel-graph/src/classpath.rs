use crate::graph::{infer_source_attachment, DependencyGraph, GraphError};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Type of Bazel Java target for classpath computation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TargetKind {
    JavaLibrary,
    JavaBinary,
    JavaTest,
    JavaImport,
    #[default]
    Unknown,
}

/// Type of classpath entry
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ClasspathEntryType {
    Library,
    Project,
    Source,
}

/// Visibility level for a classpath entry
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Visibility {
    #[default]
    Public,
    Private,
    // Package-private visibility with allowed packages
    PackagePrivate(Vec<String>),
}

/// A single entry in a computed classpath
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClasspathEntry {
    pub entry_type: ClasspathEntryType,
    pub path: String,
    pub source_attachment_path: Option<String>,
    pub is_test: bool,
    pub is_exported: bool,
    pub access_rules: Vec<AccessRule>,
    /// Visibility level for this entry (used for Bazel visibility enforcement)
    #[serde(default)]
    pub visibility: Visibility,
}

/// Access rule for classpath visibility
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessRule {
    pub pattern: String,
    pub is_accessible: bool,
}

/// Detected duplicate JAR in classpath
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JarConflict {
    pub jar_path: String,
    pub occurrences: usize,
}

/// Computed classpath for a target
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputedClasspath {
    pub target_label: String,
    pub entries: Vec<ClasspathEntry>,
    pub source_roots: Vec<String>,
    pub generated_source_dirs: Vec<String>,
    pub annotation_processors: Vec<String>,
    pub output_jars: Vec<String>,
}

impl ComputedClasspath {
    pub fn compute_for(
        graph: &DependencyGraph,
        target_label: &str,
        target_kind: TargetKind,
        workspace_root: Option<&str>,
    ) -> Result<Self, GraphError> {
        let is_test = target_kind == TargetKind::JavaTest;

        match target_kind {
            TargetKind::JavaImport => Self::compute_for_import(graph, target_label),
            TargetKind::JavaLibrary
            | TargetKind::JavaBinary
            | TargetKind::JavaTest
            | TargetKind::Unknown => {
                Self::compute_for_library(graph, target_label, is_test, workspace_root, &target_kind)
            }
        }
    }

    /// Compute a merged classpath for multiple targets, deduplicating entries by
    /// `(entry_type, path)` and resolving conflicts across targets.
    pub fn compute_for_targets(
        graph: &DependencyGraph,
        labels: &[&str],
        workspace_root: Option<&str>,
    ) -> Result<Self, GraphError> {
        if labels.is_empty() {
            return Ok(ComputedClasspath {
                target_label: String::new(),
                entries: Vec::new(),
                source_roots: Vec::new(),
                generated_source_dirs: Vec::new(),
                annotation_processors: Vec::new(),
                output_jars: Vec::new(),
            });
        }

        if labels.len() == 1 {
            let kind = graph.get_target_kind(labels[0]);
            return Self::compute_for(graph, labels[0], kind, workspace_root);
        }

        let mut merged: IndexMap<(ClasspathEntryType, String), ClasspathEntry> = IndexMap::new();
        let mut all_output_jars = Vec::new();

        for &label in labels {
            let kind = graph.get_target_kind(label);
            let cp = match Self::compute_for(graph, label, kind, workspace_root) {
                Ok(cp) => cp,
                Err(e) => {
                    log::warn!("Skipping target '{}' during merge: {}", label, e);
                    continue;
                }
            };

            all_output_jars.extend(cp.output_jars);

            for entry in cp.entries {
                let key = (entry.entry_type.clone(), entry.path.clone());
                merged
                    .entry(key)
                    .and_modify(|existing| {
                        if existing.source_attachment_path.is_none()
                            && entry.source_attachment_path.is_some()
                        {
                            existing.source_attachment_path = entry.source_attachment_path.clone();
                        }
                        if existing.is_test && !entry.is_test {
                            existing.is_test = false;
                        }
                        if !existing.is_exported && entry.is_exported {
                            existing.is_exported = true;
                        }
                        for rule in &entry.access_rules {
                            if !existing
                                .access_rules
                                .iter()
                                .any(|r| r.pattern == rule.pattern)
                            {
                                existing.access_rules.push(rule.clone());
                            }
                        }
                    })
                    .or_insert(entry);
            }
        }

        all_output_jars.sort();
        all_output_jars.dedup();

        Ok(ComputedClasspath {
            target_label: labels.join("+"),
            entries: merged.into_values().collect(),
            source_roots: Vec::new(),
            generated_source_dirs: Vec::new(),
            annotation_processors: Vec::new(),
            output_jars: all_output_jars,
        })
    }

    fn compute_for_library(
        graph: &DependencyGraph,
        target_label: &str,
        is_test_context: bool,
        workspace_root: Option<&str>,
        target_kind: &TargetKind,
    ) -> Result<Self, GraphError> {
        let deps = graph.transitive_deps(target_label)?;

        let mut entries = Vec::new();
        let mut seen_jars: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        for dep_label in &deps {
            let dep_is_testonly = is_test_context && graph.is_testonly(dep_label);

            if is_bazel_internal_label(dep_label) {
                continue;
            }

            let is_workspace_internal = !dep_label.starts_with('@');

            if is_workspace_internal {
                entries.push(ClasspathEntry {
                    entry_type: ClasspathEntryType::Project,
                    path: dep_label.clone(),
                    source_attachment_path: None,
                    is_test: dep_is_testonly,
                    is_exported: false,
                    access_rules: Vec::new(),
                    visibility: Visibility::default(),
                });
            }

            if let Some(jars) = graph.get_target_jars(dep_label) {
                for jar in jars {
                    let resolve_source = |jar: &crate::graph::ResolvedJar| {
                        jar.source_path
                            .as_ref()
                            .filter(|p| std::path::Path::new(p).exists())
                            .cloned()
                            .or_else(|| {
                                if is_workspace_internal {
                                    infer_source_attachment(dep_label, workspace_root)
                                } else {
                                    None
                                }
                            })
                    };

                    if let Some(&existing_idx) = seen_jars.get(&jar.classpath_path) {
                        if entries[existing_idx].source_attachment_path.is_none() {
                            let source = resolve_source(jar);
                            if source.is_some() {
                                entries[existing_idx].source_attachment_path = source;
                            }
                        }
                    } else {
                        let source = resolve_source(jar);
                        seen_jars.insert(jar.classpath_path.clone(), entries.len());
                        entries.push(ClasspathEntry {
                            entry_type: ClasspathEntryType::Library,
                            path: jar.classpath_path.clone(),
                            source_attachment_path: source,
                            is_test: dep_is_testonly,
                            is_exported: false,
                            access_rules: Vec::new(),
                            visibility: Visibility::default(),
                        });
                    }
                }
            }
        }

        let mut output_jars: Vec<String> = graph
            .get_target_jars(target_label)
            .map(|jars| jars.iter().map(|j| j.classpath_path.clone()).collect())
            .unwrap_or_default();

        if *target_kind == TargetKind::JavaBinary {
            for dep_label in &deps {
                if is_bazel_internal_label(dep_label) {
                    continue;
                }
                if let Some(dep_jars) = graph.get_target_jars(dep_label) {
                    for jar in dep_jars {
                        if !output_jars.contains(&jar.classpath_path) {
                            output_jars.push(jar.classpath_path.clone());
                        }
                    }
                }
            }

            if output_jars.len() > 1 {
                output_jars.retain(|jar_path| {
                    match std::fs::metadata(jar_path) {
                        Ok(meta) => meta.len() >= 1024,
                        Err(_) => true,
                    }
                });
            }
        }

        Ok(ComputedClasspath {
            target_label: target_label.to_string(),
            entries,
            source_roots: Vec::new(),
            generated_source_dirs: Vec::new(),
            annotation_processors: Vec::new(),
            output_jars,
        })
    }

    fn compute_for_import(graph: &DependencyGraph, target_label: &str) -> Result<Self, GraphError> {
        if !graph.has_target(target_label) {
            return Err(GraphError::TargetNotFound {
                label: target_label.to_string(),
            });
        }

        let mut entries = Vec::new();

        if let Some(jars) = graph.get_target_jars(target_label) {
            for jar in jars {
                let source = jar
                    .source_path
                    .as_ref()
                    .filter(|p| std::path::Path::new(p).exists())
                    .cloned();
                entries.push(ClasspathEntry {
                    entry_type: ClasspathEntryType::Library,
                    path: jar.classpath_path.clone(),
                    source_attachment_path: source,
                    is_test: false,
                    is_exported: false,
                    access_rules: Vec::new(),
                    visibility: Visibility::default(),
                });
            }
        }

        let output_jars = graph
            .get_target_jars(target_label)
            .map(|jars| jars.iter().map(|j| j.classpath_path.clone()).collect())
            .unwrap_or_default();

        Ok(ComputedClasspath {
            target_label: target_label.to_string(),
            entries,
            source_roots: Vec::new(),
            generated_source_dirs: Vec::new(),
            annotation_processors: Vec::new(),
            output_jars,
        })
    }

    pub fn filter_by_visibility(&mut self, _requesting_package: &str) {
        // TODO: Implement proper Bazel visibility filtering using access_rules.
        // Currently retains all entries — visibility is enforced at the Bazel level
        // during aspect resolution, so classpath entries are already correctly scoped.
    }

    pub fn detect_duplicate_jars(&self) -> Vec<JarConflict> {
        let mut seen = std::collections::HashMap::new();
        for entry in &self.entries {
            if entry.entry_type == ClasspathEntryType::Library {
                *seen.entry(entry.path.clone()).or_insert(0usize) += 1;
            }
        }
        seen.into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(path, count)| {
                log::warn!("Duplicate JAR in classpath: {} ({}x)", path, count);
                JarConflict {
                    jar_path: path,
                    occurrences: count,
                }
            })
            .collect()
    }

    /// Convert to pipe-delimited string array for JNI.
    /// Output JARs (the target's own compiled JARs) are emitted first as LIB
    /// entries so they appear on the runtime classpath, filling the gap left by
    /// the empty Eclipse output location when JavaBuilder is disabled.
    pub fn to_pipe_delimited_entries(&self) -> Vec<String> {
        let mut result = Vec::with_capacity(self.output_jars.len() + self.entries.len());

        let entry_paths: std::collections::HashSet<&str> =
            self.entries.iter().map(|e| e.path.as_str()).collect();
        for jar_path in &self.output_jars {
            if !entry_paths.contains(jar_path.as_str()) {
                result.push(format!("LIB|{}||false|false|", jar_path));
            }
        }

        for entry in &self.entries {
            let type_str = match entry.entry_type {
                ClasspathEntryType::Library => "LIB",
                ClasspathEntryType::Project => "PROJ",
                ClasspathEntryType::Source => "SRC",
            };
            let source = entry.source_attachment_path.as_deref().unwrap_or("");
            let access = if entry.access_rules.is_empty() {
                "".to_string()
            } else {
                entry
                    .access_rules
                    .iter()
                    .map(|r| {
                        if r.is_accessible {
                            format!("+{}", r.pattern)
                        } else {
                            format!("-{}", r.pattern)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(":")
            };
            result.push(format!(
                "{}|{}|{}|{}|{}|{}",
                type_str, entry.path, source, entry.is_test, entry.is_exported, access
            ));
        }

        result
    }
}

/// Returns true for Bazel-internal toolchain/platform targets that should never
/// appear on a Java classpath. In Bazel 6+, canonical repo labels use "@@" prefix.
/// External dependencies like Maven artifacts (e.g. `@@maven+...//:guava`) must NOT
/// be filtered — only Bazel's own infrastructure targets.
pub fn is_bazel_internal_label(label: &str) -> bool {
    label.starts_with("@@bazel_tools//")
        || label.starts_with("@@local_config_")
        || label.starts_with("@@platforms//")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{infer_source_attachment, ResolvedJar};
    use bazel_aspect::{ArtifactLocation, JarInfo, JavaIdeInfo, TargetIdeInfo};
    use std::path::Path;

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
    fn test_toolchain_targets_filtered_from_proj_entries() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target(
                "//app:app",
                vec!["@rules_java//java:toolchain", "@@rules_cc++ext//:compiler"],
                vec!["/app.jar"],
            ),
            make_target("@rules_java//java:toolchain", vec![], vec![]),
            make_target("@@rules_cc++ext//:compiler", vec![], vec![]),
        ];

        graph.populate_from_aspects(&results, Path::new("/workspace"));
        let cp = ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None)
            .unwrap();

        let proj_entries: Vec<&ClasspathEntry> = cp
            .entries
            .iter()
            .filter(|e| e.entry_type == ClasspathEntryType::Project)
            .collect();

        for entry in &proj_entries {
            assert!(
                !entry.path.starts_with("@@"),
                "Expected no @@ entries, got: {}",
                entry.path
            );
        }
    }

    #[test]
    fn test_regular_proj_entries_preserved() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//app:app", vec!["//lib:utils"], vec!["/app.jar"]),
            make_target("//lib:utils", vec![], vec![]),
        ];

        graph.populate_from_aspects(&results, Path::new("/workspace"));
        let cp = ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None)
            .unwrap();

        let proj_paths: Vec<&str> = cp
            .entries
            .iter()
            .filter(|e| e.entry_type == ClasspathEntryType::Project)
            .map(|e| e.path.as_str())
            .collect();

        assert!(
            proj_paths.contains(&"//lib:utils"),
            "Expected //lib:utils PROJ entry, got: {:?}",
            proj_paths
        );
    }

    #[test]
    fn test_mixed_deps_filters_only_at_at() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target(
                "//app:app",
                vec!["//lib:utils", "@@toolchain//:tc", "//lib:api"],
                vec!["/app.jar"],
            ),
            make_target("//lib:utils", vec![], vec![]),
            make_target("@@toolchain//:tc", vec![], vec![]),
            make_target("//lib:api", vec![], vec![]),
        ];

        graph.populate_from_aspects(&results, Path::new("/workspace"));
        let cp = ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None)
            .unwrap();

        let proj_paths: Vec<&str> = cp
            .entries
            .iter()
            .filter(|e| e.entry_type == ClasspathEntryType::Project)
            .map(|e| e.path.as_str())
            .collect();

        assert_eq!(proj_paths.len(), 2);
        assert!(proj_paths.contains(&"//lib:utils"));
        assert!(proj_paths.contains(&"//lib:api"));
        assert!(!proj_paths.iter().any(|p| p.starts_with("@@")));
    }

    #[test]
    fn test_regular_lib_dep_of_test_target_is_not_test() {
        let mut graph = DependencyGraph::new();
        let mut test_target = make_target("//app:app_test", vec!["//lib:greeter_lib"], vec![]);
        test_target.kind = "java_test".to_string();
        let results = vec![
            test_target,
            make_target("//lib:greeter_lib", vec![], vec!["/greeter.jar"]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app_test", TargetKind::JavaTest, None)
                .unwrap();

        let greeter_entry = cp
            .entries
            .iter()
            .find(|e| e.path == "/greeter.jar")
            .unwrap();
        assert!(
            !greeter_entry.is_test,
            "Regular library dep should NOT have is_test=true"
        );
    }

    #[test]
    fn test_testonly_dep_of_test_target_is_test() {
        let mut graph = DependencyGraph::new();
        let mut test_target = make_target("//app:app_test", vec!["//lib:test_helpers"], vec![]);
        test_target.kind = "java_test".to_string();
        let mut test_helpers = make_target("//lib:test_helpers", vec![], vec!["/helpers.jar"]);
        test_helpers.kind = "java_test".to_string();
        let results = vec![test_target, test_helpers];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app_test", TargetKind::JavaTest, None)
                .unwrap();

        let helpers_entry = cp
            .entries
            .iter()
            .find(|e| e.path == "/helpers.jar")
            .unwrap();
        assert!(
            helpers_entry.is_test,
            "Testonly dep should have is_test=true"
        );
    }

    #[test]
    fn test_library_target_all_deps_not_test() {
        let mut graph = DependencyGraph::new();
        let mut test_helpers = make_target("//lib:test_helpers", vec![], vec!["/helpers.jar"]);
        test_helpers.kind = "java_test".to_string();
        let results = vec![
            make_target("//app:app", vec!["//lib:test_helpers"], vec!["/app.jar"]),
            test_helpers,
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp = ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None)
            .unwrap();

        for entry in &cp.entries {
            assert!(
                !entry.is_test,
                "Library target deps should all have is_test=false, got is_test=true for {}",
                entry.path
            );
        }
    }

    #[test]
    fn test_internal_dep_with_jars_produces_proj_and_lib() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//app:app", vec!["//lib:utils"], vec!["/app.jar"]),
            make_target("//lib:utils", vec![], vec!["/utils.jar"]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp = ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None)
            .unwrap();

        let proj_idx = cp
            .entries
            .iter()
            .position(|e| e.entry_type == ClasspathEntryType::Project && e.path == "//lib:utils");
        let lib_idx = cp
            .entries
            .iter()
            .position(|e| e.entry_type == ClasspathEntryType::Library && e.path == "/utils.jar");

        assert!(proj_idx.is_some(), "Expected PROJ entry for //lib:utils");
        assert!(lib_idx.is_some(), "Expected LIB entry for /utils.jar");
        assert!(
            proj_idx.unwrap() < lib_idx.unwrap(),
            "PROJ entry should appear before LIB entry for same dependency"
        );
    }

    #[test]
    fn test_internal_dep_without_jars_produces_only_proj() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//app:app", vec!["//lib:api"], vec!["/app.jar"]),
            make_target("//lib:api", vec![], vec![]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp = ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None)
            .unwrap();

        let proj_count = cp
            .entries
            .iter()
            .filter(|e| e.entry_type == ClasspathEntryType::Project && e.path == "//lib:api")
            .count();
        let lib_count = cp
            .entries
            .iter()
            .filter(|e| e.entry_type == ClasspathEntryType::Library && e.path.contains("api"))
            .count();

        assert_eq!(
            proj_count, 1,
            "Expected exactly one PROJ entry for //lib:api"
        );
        assert_eq!(
            lib_count, 0,
            "Expected no LIB entries for //lib:api (no JAR data)"
        );
    }

    #[test]
    fn test_external_dep_produces_only_lib() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//app:app", vec!["@maven//:guava"], vec!["/app.jar"]),
            make_target("@maven//:guava", vec![], vec!["/guava.jar"]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp = ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None)
            .unwrap();

        let proj_count = cp
            .entries
            .iter()
            .filter(|e| e.entry_type == ClasspathEntryType::Project)
            .count();
        let lib_count = cp
            .entries
            .iter()
            .filter(|e| e.entry_type == ClasspathEntryType::Library && e.path == "/guava.jar")
            .count();

        assert_eq!(
            proj_count, 0,
            "Expected no PROJ entries for external @maven dependency"
        );
        assert_eq!(lib_count, 1, "Expected exactly one LIB entry for guava.jar");
    }

    #[test]
    fn test_at_at_prefixed_dep_produces_no_entries() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target(
                "//app:app",
                vec!["@@bazel_tools//tools/jdk:toolchain", "@@platforms//cpu:cpu"],
                vec!["/app.jar"],
            ),
            make_target("@@bazel_tools//tools/jdk:toolchain", vec![], vec![]),
            make_target("@@platforms//cpu:cpu", vec![], vec![]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp = ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None)
            .unwrap();

        let internal_entries: Vec<&ClasspathEntry> = cp
            .entries
            .iter()
            .filter(|e| e.path.contains("bazel_tools") || e.path.contains("platforms"))
            .collect();

        assert!(
            internal_entries.is_empty(),
            "Expected no entries for Bazel-internal @@ targets, got: {:?}",
            internal_entries
        );
    }

    #[test]
    fn test_canonical_external_dep_produces_lib_entry() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target(
                "//app:app",
                vec!["@@maven//:com_google_guava_guava"],
                vec!["/app.jar"],
            ),
            make_target(
                "@@maven//:com_google_guava_guava",
                vec![],
                vec!["/guava-33.4.0-jre.jar"],
            ),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp = ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None)
            .unwrap();

        let proj_count = cp
            .entries
            .iter()
            .filter(|e| e.entry_type == ClasspathEntryType::Project)
            .count();
        let guava_lib = cp.entries.iter().find(|e| {
            e.entry_type == ClasspathEntryType::Library && e.path == "/guava-33.4.0-jre.jar"
        });

        assert_eq!(
            proj_count, 0,
            "Expected no PROJ entries for external @@maven dep"
        );
        assert!(
            guava_lib.is_some(),
            "Expected LIB entry for guava JAR, got entries: {:?}",
            cp.entries
        );
    }

    #[test]
    fn test_canonical_mixed_deps_ordering() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target(
                "//app:app",
                vec!["//utils:string_utils", "@@maven//:guava", "//service:api"],
                vec!["/app.jar"],
            ),
            make_target("//utils:string_utils", vec![], vec!["/utils.jar"]),
            make_target("@@maven//:guava", vec![], vec!["/guava.jar"]),
            make_target("//service:api", vec![], vec![]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp = ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None)
            .unwrap();

        let utils_proj_idx = cp.entries.iter().position(|e| {
            e.entry_type == ClasspathEntryType::Project && e.path == "//utils:string_utils"
        });
        let utils_lib_idx = cp
            .entries
            .iter()
            .position(|e| e.entry_type == ClasspathEntryType::Library && e.path == "/utils.jar");
        let guava_lib_idx = cp
            .entries
            .iter()
            .position(|e| e.entry_type == ClasspathEntryType::Library && e.path == "/guava.jar");
        let api_proj_idx = cp
            .entries
            .iter()
            .position(|e| e.entry_type == ClasspathEntryType::Project && e.path == "//service:api");
        let guava_proj_idx = cp.entries.iter().position(|e| {
            e.entry_type == ClasspathEntryType::Project && e.path == "@@maven//:guava"
        });

        assert!(
            utils_proj_idx.is_some(),
            "Expected PROJ for //utils:string_utils"
        );
        assert!(utils_lib_idx.is_some(), "Expected LIB for /utils.jar");
        assert!(
            guava_lib_idx.is_some(),
            "Expected LIB for /guava.jar from @@maven dep"
        );
        assert!(api_proj_idx.is_some(), "Expected PROJ for //service:api");
        assert!(
            guava_proj_idx.is_none(),
            "Expected no PROJ for external @@maven//:guava"
        );

        assert!(
            utils_proj_idx.unwrap() < utils_lib_idx.unwrap(),
            "PROJ for utils should precede its LIB"
        );
    }

    #[test]
    fn test_canonical_external_dep_no_jars_produces_nothing() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//app:app", vec!["@@maven//:some_target"], vec!["/app.jar"]),
            make_target("@@maven//:some_target", vec![], vec![]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp = ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None)
            .unwrap();

        let maven_entries: Vec<&ClasspathEntry> = cp
            .entries
            .iter()
            .filter(|e| e.path.contains("maven"))
            .collect();

        assert!(
            maven_entries.is_empty(),
            "Expected no entries for @@maven target with no JAR data, got: {:?}",
            maven_entries
        );
    }

    #[test]
    fn test_mixed_deps_correct_ordering() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target(
                "//app:app",
                vec!["//lib:utils", "@maven//:guava", "//lib:api"],
                vec!["/app.jar"],
            ),
            make_target("//lib:utils", vec![], vec!["/utils.jar"]),
            make_target("@maven//:guava", vec![], vec!["/guava.jar"]),
            make_target("//lib:api", vec![], vec![]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp = ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None)
            .unwrap();

        let utils_proj_idx = cp
            .entries
            .iter()
            .position(|e| e.entry_type == ClasspathEntryType::Project && e.path == "//lib:utils");
        let utils_lib_idx = cp
            .entries
            .iter()
            .position(|e| e.entry_type == ClasspathEntryType::Library && e.path == "/utils.jar");
        let guava_lib_idx = cp
            .entries
            .iter()
            .position(|e| e.entry_type == ClasspathEntryType::Library && e.path == "/guava.jar");
        let api_proj_idx = cp
            .entries
            .iter()
            .position(|e| e.entry_type == ClasspathEntryType::Project && e.path == "//lib:api");
        let guava_proj_idx = cp.entries.iter().position(|e| {
            e.entry_type == ClasspathEntryType::Project && e.path == "@maven//:guava"
        });

        assert!(utils_proj_idx.is_some(), "Expected PROJ for //lib:utils");
        assert!(utils_lib_idx.is_some(), "Expected LIB for /utils.jar");
        assert!(guava_lib_idx.is_some(), "Expected LIB for /guava.jar");
        assert!(api_proj_idx.is_some(), "Expected PROJ for //lib:api");
        assert!(
            guava_proj_idx.is_none(),
            "Expected no PROJ for external @maven//:guava"
        );

        assert!(
            utils_proj_idx.unwrap() < utils_lib_idx.unwrap(),
            "PROJ for utils should precede its LIB"
        );
    }

    #[test]
    fn test_rules_jvm_external_not_filtered() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target(
                "//app:app",
                vec!["@@rules_jvm_external~maven~maven//:com_google_guava_guava"],
                vec!["/app.jar"],
            ),
            make_target_with_jar_path(
                "@@rules_jvm_external~maven~maven//:com_google_guava_guava",
                vec![],
                "/guava.jar",
            ),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp = ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None)
            .unwrap();

        let guava_entries: Vec<&ClasspathEntry> = cp
            .entries
            .iter()
            .filter(|e| e.path.contains("guava"))
            .collect();
        assert!(
            !guava_entries.is_empty(),
            "Expected LIB entry for @@rules_jvm_external maven dep, got: {:?}",
            cp.entries
        );
    }

    #[test]
    fn test_label_alias_registered_on_canonical_target() {
        let mut graph = DependencyGraph::new();
        let results = vec![make_target_with_jar_path(
            "@@rules_jvm_external~maven~maven//:com_google_guava_guava",
            vec![],
            "/guava.jar",
        )];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let jars = graph.get_target_jars("@maven//:com_google_guava_guava");
        assert!(
            jars.is_some(),
            "Expected JAR data for @maven//:guava via alias"
        );
        assert_eq!(jars.unwrap().len(), 1);
    }

    #[test]
    fn test_get_target_jars_resolves_alias() {
        let mut graph = DependencyGraph::new();
        graph.add_target("@@rules_jvm_external~maven~maven//:guava");
        graph.set_target_jars(
            "@@rules_jvm_external~maven~maven//:guava",
            vec![ResolvedJar {
                classpath_path: "/guava.jar".to_string(),
                source_path: None,
            }],
        );
        graph.label_aliases.insert(
            "@maven//:guava".to_string(),
            "@@rules_jvm_external~maven~maven//:guava".to_string(),
        );

        let jars = graph.get_target_jars("@maven//:guava");
        assert!(jars.is_some());
        assert_eq!(jars.unwrap()[0].classpath_path, "/guava.jar");
    }

    #[test]
    fn test_transitive_deps_via_alias() {
        let mut graph = DependencyGraph::new();

        graph.add_target("//utils:string_utils");
        graph.add_target("@maven//:com_google_guava_guava");
        graph.add_dep("//utils:string_utils", "@maven//:com_google_guava_guava");

        graph.add_target("@@rules_jvm_external~maven~maven//:com_google_guava_guava");
        graph.set_target_jars(
            "@@rules_jvm_external~maven~maven//:com_google_guava_guava",
            vec![ResolvedJar {
                classpath_path: "/guava-33.4.0-jre.jar".to_string(),
                source_path: None,
            }],
        );
        graph.label_aliases.insert(
            "@maven//:com_google_guava_guava".to_string(),
            "@@rules_jvm_external~maven~maven//:com_google_guava_guava".to_string(),
        );

        let cp = ComputedClasspath::compute_for(
            &graph,
            "//utils:string_utils",
            TargetKind::JavaLibrary,
            None,
        )
        .unwrap();

        let guava_lib = cp
            .entries
            .iter()
            .find(|e| e.entry_type == ClasspathEntryType::Library && e.path.contains("guava"));
        assert!(
            guava_lib.is_some(),
            "Expected Guava LIB entry via alias resolution, got: {:?}",
            cp.entries
        );
    }

    #[test]
    fn test_clear_resets_label_aliases() {
        let mut graph = DependencyGraph::new();
        graph.label_aliases.insert(
            "@maven//:guava".to_string(),
            "@@rules_jvm_external~maven~maven//:guava".to_string(),
        );
        assert!(!graph.label_aliases.is_empty());

        graph.clear();
        assert!(graph.label_aliases.is_empty());
    }

    fn make_target_with_jar_path(label: &str, deps: Vec<&str>, jar_path: &str) -> TargetIdeInfo {
        TargetIdeInfo {
            label: label.to_string(),
            kind: "java_import".to_string(),
            build_file: None,
            java_info: Some(JavaIdeInfo {
                jars: vec![JarInfo {
                    jar: ArtifactLocation {
                        absolute_path: Some(jar_path.to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                }],
                ..Default::default()
            }),
            deps: deps.iter().map(|s| s.to_string()).collect(),
            runtime_deps: Vec::new(),
            exports: Vec::new(),
        }
    }

    // --- compute_for_targets tests ---

    #[test]
    fn test_merge_overlapping_deps() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target(
                "//pkg:A",
                vec!["@maven//:guava", "@maven//:mongo"],
                vec!["/a.jar"],
            ),
            make_target("@maven//:guava", vec![], vec!["/guava.jar"]),
            make_target("@maven//:mongo", vec![], vec!["/mongo.jar"]),
            make_target("//pkg:B", vec!["@maven//:guava"], vec!["/b.jar"]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp =
            ComputedClasspath::compute_for_targets(&graph, &["//pkg:A", "//pkg:B"], None).unwrap();

        let paths: Vec<&str> = cp
            .entries
            .iter()
            .filter(|e| e.entry_type == ClasspathEntryType::Library)
            .map(|e| e.path.as_str())
            .collect();
        assert!(paths.contains(&"/guava.jar"), "Expected guava.jar");
        assert!(paths.contains(&"/mongo.jar"), "Expected mongo.jar");
        let guava_count = paths.iter().filter(|p| **p == "/guava.jar").count();
        assert_eq!(guava_count, 1, "guava.jar should appear exactly once");
    }

    #[test]
    fn test_merge_source_attachment_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let guava_jar = tmp.path().join("guava.jar");
        let src_jar = tmp.path().join("guava-sources.jar");
        std::fs::write(&guava_jar, [0u8; 2048]).unwrap();
        std::fs::write(&src_jar, [0u8; 2048]).unwrap();

        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//pkg:A", vec!["@maven//:guava"], vec!["/a.jar"]),
            make_target_with_source_jar(
                "@maven//:guava",
                vec![],
                guava_jar.to_str().unwrap(),
                src_jar.to_str().unwrap(),
            ),
            make_target("//pkg:B", vec!["@maven//:guava"], vec!["/b.jar"]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp =
            ComputedClasspath::compute_for_targets(&graph, &["//pkg:A", "//pkg:B"], None).unwrap();

        let guava = cp
            .entries
            .iter()
            .find(|e| e.path == guava_jar.to_str().unwrap())
            .expect("Expected guava entry");
        assert!(
            guava.source_attachment_path.is_some(),
            "Merged entry should retain source attachment"
        );
    }

    #[test]
    fn test_merge_is_test_conflict() {
        let mut graph = DependencyGraph::new();
        let mut test_target = make_target("//pkg:A_test", vec!["//lib:helpers"], vec![]);
        test_target.kind = "java_test".to_string();
        let mut helpers = make_target("//lib:helpers", vec![], vec!["/helpers.jar"]);
        helpers.kind = "java_test".to_string();
        let results = vec![
            test_target,
            helpers,
            make_target("//pkg:B", vec!["//lib:helpers"], vec!["/b.jar"]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp = ComputedClasspath::compute_for_targets(&graph, &["//pkg:A_test", "//pkg:B"], None)
            .unwrap();

        let helpers_entry = cp
            .entries
            .iter()
            .find(|e| e.path == "/helpers.jar")
            .expect("Expected helpers.jar");
        assert!(
            !helpers_entry.is_test,
            "Merged is_test should be false when any target uses it as non-test"
        );
    }

    #[test]
    fn test_merge_proj_before_lib_ordering() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//pkg:A", vec!["//lib:utils"], vec!["/a.jar"]),
            make_target("//lib:utils", vec![], vec!["/utils.jar"]),
            make_target("//pkg:B", vec!["//lib:utils"], vec!["/b.jar"]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp =
            ComputedClasspath::compute_for_targets(&graph, &["//pkg:A", "//pkg:B"], None).unwrap();

        let proj_idx = cp
            .entries
            .iter()
            .position(|e| e.entry_type == ClasspathEntryType::Project && e.path == "//lib:utils");
        let lib_idx = cp
            .entries
            .iter()
            .position(|e| e.entry_type == ClasspathEntryType::Library && e.path == "/utils.jar");

        assert!(proj_idx.is_some(), "Expected PROJ entry for //lib:utils");
        assert!(lib_idx.is_some(), "Expected LIB entry for /utils.jar");
        assert!(
            proj_idx.unwrap() < lib_idx.unwrap(),
            "PROJ should appear before LIB in merged result"
        );
    }

    #[test]
    fn test_merge_single_target_passthrough() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//pkg:A", vec!["@maven//:guava"], vec!["/a.jar"]),
            make_target("@maven//:guava", vec![], vec!["/guava.jar"]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let single =
            ComputedClasspath::compute_for(&graph, "//pkg:A", TargetKind::JavaLibrary, None)
                .unwrap();
        let merged = ComputedClasspath::compute_for_targets(&graph, &["//pkg:A"], None).unwrap();

        assert_eq!(single.entries.len(), merged.entries.len());
        for (s, m) in single.entries.iter().zip(merged.entries.iter()) {
            assert_eq!(s.entry_type, m.entry_type);
            assert_eq!(s.path, m.path);
        }
    }

    #[test]
    fn test_merge_empty_labels() {
        let graph = DependencyGraph::new();
        let cp = ComputedClasspath::compute_for_targets(&graph, &[], None).unwrap();
        assert!(cp.entries.is_empty());
    }

    fn make_target_with_source_jar(
        label: &str,
        deps: Vec<&str>,
        jar_path: &str,
        source_jar_path: &str,
    ) -> TargetIdeInfo {
        TargetIdeInfo {
            label: label.to_string(),
            kind: "java_library".to_string(),
            build_file: None,
            java_info: Some(JavaIdeInfo {
                jars: vec![JarInfo {
                    jar: ArtifactLocation {
                        absolute_path: Some(jar_path.to_string()),
                        ..Default::default()
                    },
                    source_jar: Some(ArtifactLocation {
                        absolute_path: Some(source_jar_path.to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            deps: deps.iter().map(|s| s.to_string()).collect(),
            runtime_deps: Vec::new(),
            exports: Vec::new(),
        }
    }

    #[test]
    fn test_classpath_entry_includes_source_attachment() {
        let tmp = tempfile::tempdir().unwrap();
        let guava_jar = tmp.path().join("guava.jar");
        let src_jar = tmp.path().join("guava-sources.jar");
        std::fs::write(&guava_jar, [0u8; 2048]).unwrap();
        std::fs::write(&src_jar, [0u8; 2048]).unwrap();

        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//app:app", vec!["@maven//:guava"], vec!["/app.jar"]),
            make_target_with_source_jar(
                "@maven//:guava",
                vec![],
                guava_jar.to_str().unwrap(),
                src_jar.to_str().unwrap(),
            ),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp = ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None)
            .unwrap();

        let guava_entry = cp
            .entries
            .iter()
            .find(|e| e.path == guava_jar.to_str().unwrap())
            .expect("Expected guava.jar entry");
        assert_eq!(
            guava_entry.source_attachment_path,
            Some(src_jar.to_str().unwrap().to_string()),
            "Expected source attachment for guava.jar when source JAR exists on disk"
        );
    }

    #[test]
    fn test_classpath_entry_no_source_when_not_available() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//app:app", vec!["@maven//:guava"], vec!["/app.jar"]),
            make_target_with_jar_path("@maven//:guava", vec![], "/guava.jar"),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp = ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None)
            .unwrap();

        let guava_entry = cp
            .entries
            .iter()
            .find(|e| e.path == "/guava.jar")
            .expect("Expected guava.jar entry");
        assert!(
            guava_entry.source_attachment_path.is_none(),
            "Expected no source attachment when JarInfo has no source_jar"
        );
    }

    #[test]
    fn test_pipe_delimited_includes_source_path() {
        let entry = ClasspathEntry {
            entry_type: ClasspathEntryType::Library,
            path: "/guava.jar".to_string(),
            source_attachment_path: Some("/guava-sources.jar".to_string()),
            is_test: false,
            is_exported: false,
            access_rules: Vec::new(),
            visibility: Visibility::default(),
        };
        let cp = ComputedClasspath {
            target_label: "//app:app".to_string(),
            entries: vec![entry],
            source_roots: Vec::new(),
            generated_source_dirs: Vec::new(),
            annotation_processors: Vec::new(),
            output_jars: Vec::new(),
        };
        let lines = cp.to_pipe_delimited_entries();
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0], "LIB|/guava.jar|/guava-sources.jar|false|false|",
            "Expected source path in pipe-delimited field 2"
        );
    }

    #[test]
    fn test_workspace_internal_label_filtering() {
        assert!(!is_bazel_internal_label("//utils:string_utils"));
        assert!(!is_bazel_internal_label("//service:user_service"));
        assert!(!is_bazel_internal_label("@maven//:guava"));
        assert!(!is_bazel_internal_label("@@maven//:guava"));
        assert!(is_bazel_internal_label(
            "@@bazel_tools//tools/jdk:toolchain"
        ));
        assert!(is_bazel_internal_label("@@local_config_cc//:compiler"));
        assert!(is_bazel_internal_label("@@platforms//cpu:cpu"));
    }

    #[test]
    fn test_infer_source_attachment_standard_maven_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_string_lossy().into_owned();
        std::fs::create_dir_all(tmp.path().join("utils/src/main/java")).unwrap();
        let result = infer_source_attachment("//utils:string_utils", Some(&ws));
        assert_eq!(result, Some(format!("{}/utils/src/main/java", ws)));
    }

    #[test]
    fn test_infer_source_attachment_test_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_string_lossy().into_owned();
        std::fs::create_dir_all(tmp.path().join("foo/src/test/java")).unwrap();
        let result = infer_source_attachment("//foo:foo_test", Some(&ws));
        assert_eq!(result, Some(format!("{}/foo/src/test/java", ws)));
    }

    #[test]
    fn test_infer_source_attachment_flat_java_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_string_lossy().into_owned();
        std::fs::create_dir_all(tmp.path().join("bar/java")).unwrap();
        let result = infer_source_attachment("//bar:bar_lib", Some(&ws));
        assert_eq!(result, Some(format!("{}/bar/java", ws)));
    }

    #[test]
    fn test_infer_source_attachment_no_matching_source_root() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_string_lossy().into_owned();
        std::fs::create_dir_all(tmp.path().join("baz/data")).unwrap();
        let result = infer_source_attachment("//baz:baz_lib", Some(&ws));
        assert_eq!(result, None);
    }

    #[test]
    fn test_infer_source_attachment_no_workspace_root() {
        let result = infer_source_attachment("//utils:string_utils", None);
        assert_eq!(result, None);
    }

    #[test]
    fn test_infer_source_attachment_substring_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_string_lossy().into_owned();
        // No filesystem dirs created — substring fallback kicks in
        let result = infer_source_attachment("//some/src/main/java/com/example:lib", Some(&ws));
        assert_eq!(result, Some(format!("{}/some/src/main/java", ws)));
    }

    #[test]
    fn test_infer_source_attachment_probe_order_main_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_string_lossy().into_owned();
        std::fs::create_dir_all(tmp.path().join("pkg/src/main/java")).unwrap();
        std::fs::create_dir_all(tmp.path().join("pkg/src/test/java")).unwrap();
        let result = infer_source_attachment("//pkg:pkg_lib", Some(&ws));
        assert_eq!(result, Some(format!("{}/pkg/src/main/java", ws)));
    }

    #[test]
    fn test_source_jar_exists_on_disk_used_directly() {
        let tmp = tempfile::tempdir().unwrap();
        let binary_jar = tmp.path().join("lib.jar");
        let source_jar = tmp.path().join("lib-sources.jar");
        std::fs::write(&binary_jar, [0u8; 2048]).unwrap();
        std::fs::write(&source_jar, [0u8; 2048]).unwrap();

        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//app:app", vec!["@maven//:guava"], vec!["/app.jar"]),
            make_target_with_source_jar(
                "@maven//:guava",
                vec![],
                binary_jar.to_str().unwrap(),
                source_jar.to_str().unwrap(),
            ),
        ];
        graph.populate_from_aspects(&results, Path::new(tmp.path()));

        let cp = ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None)
            .unwrap();

        let entry = cp
            .entries
            .iter()
            .find(|e| e.path == binary_jar.to_str().unwrap())
            .expect("Expected lib.jar entry");
        assert_eq!(
            entry.source_attachment_path,
            Some(source_jar.to_str().unwrap().to_string()),
            "Source JAR should be used directly when it exists on disk"
        );
    }

    #[test]
    fn test_phantom_source_jar_external_dep_falls_to_none() {
        let tmp = tempfile::tempdir().unwrap();
        let binary_jar = tmp.path().join("lib.jar");
        std::fs::File::create(&binary_jar).unwrap();
        // source JAR deliberately not created — aspect data references a phantom file

        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//app:app", vec!["@maven//:guava"], vec!["/app.jar"]),
            make_target_with_source_jar(
                "@maven//:guava",
                vec![],
                binary_jar.to_str().unwrap(),
                "/nonexistent/lib-sources.jar",
            ),
        ];
        graph.populate_from_aspects(&results, Path::new(tmp.path()));

        let cp = ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None)
            .unwrap();

        let entry = cp
            .entries
            .iter()
            .find(|e| e.path == binary_jar.to_str().unwrap())
            .expect("Expected lib.jar entry");
        assert!(
            entry.source_attachment_path.is_none(),
            "Phantom source JAR for external dep should produce None, got: {:?}",
            entry.source_attachment_path
        );
    }

    #[test]
    fn test_phantom_source_jar_workspace_internal_falls_to_infer() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_string_lossy().into_owned();
        let binary_jar = tmp.path().join("utils/lib.jar");
        std::fs::create_dir_all(tmp.path().join("utils")).unwrap();
        std::fs::File::create(&binary_jar).unwrap();
        std::fs::create_dir_all(tmp.path().join("utils/src/main/java")).unwrap();
        // source JAR deliberately not created — aspect data references a phantom file

        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target_with_jar_path("//app:app", vec!["//utils:string_utils"], "/app.jar"),
            make_target_with_source_jar(
                "//utils:string_utils",
                vec![],
                binary_jar.to_str().unwrap(),
                "/nonexistent/lib-sources.jar", // phantom path
            ),
        ];
        graph.populate_from_aspects(&results, Path::new(tmp.path()));

        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, Some(&ws))
                .unwrap();

        let entry = cp
            .entries
            .iter()
            .find(|e| e.path == binary_jar.to_str().unwrap())
            .expect("Expected lib.jar entry");
        assert_eq!(
            entry.source_attachment_path,
            Some(format!("{}/utils/src/main/java", ws)),
            "Phantom source JAR for workspace-internal dep should fall back to infer_source_attachment"
        );
    }

    #[test]
    fn test_duplicate_jar_merges_source_from_later_target() {
        let tmp = tempfile::tempdir().unwrap();
        let guava_jar = tmp.path().join("guava.jar");
        let src_jar = tmp.path().join("guava-sources.jar");
        std::fs::write(&guava_jar, [0u8; 2048]).unwrap();
        std::fs::write(&src_jar, [0u8; 2048]).unwrap();

        let guava_path = guava_jar.to_str().unwrap();
        let src_path = src_jar.to_str().unwrap();

        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//app:app", vec!["//3rdparty:guava"], vec!["/app.jar"]),
            make_target_with_jar_path("//3rdparty:guava", vec!["@maven//:guava"], guava_path),
            make_target_with_source_jar("@maven//:guava", vec![], guava_path, src_path),
        ];
        graph.populate_from_aspects(&results, Path::new(tmp.path()));

        let ws = tmp.path().to_string_lossy().into_owned();
        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, Some(&ws))
                .unwrap();

        let jar_entries: Vec<_> = cp.entries.iter().filter(|e| e.path == guava_path).collect();
        assert_eq!(
            jar_entries.len(),
            1,
            "Duplicate JAR should appear only once"
        );
        assert_eq!(
            jar_entries[0].source_attachment_path,
            Some(src_path.to_string()),
            "Source from later @maven// target should be merged into entry from //3rdparty: wrapper"
        );
    }

    #[test]
    fn test_duplicate_jar_preserves_first_valid_source() {
        let tmp = tempfile::tempdir().unwrap();
        let guava_jar = tmp.path().join("guava.jar");
        let src1_jar = tmp.path().join("src1-sources.jar");
        let src2_jar = tmp.path().join("src2-sources.jar");
        std::fs::write(&guava_jar, [0u8; 2048]).unwrap();
        std::fs::write(&src1_jar, [0u8; 2048]).unwrap();
        std::fs::write(&src2_jar, [0u8; 2048]).unwrap();

        let guava_path = guava_jar.to_str().unwrap();
        let src1_path = src1_jar.to_str().unwrap();
        let src2_path = src2_jar.to_str().unwrap();

        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//app:app", vec!["@repo_a//:guava"], vec!["/app.jar"]),
            make_target_with_source_jar(
                "@repo_a//:guava",
                vec!["@repo_b//:guava"],
                guava_path,
                src1_path,
            ),
            make_target_with_source_jar("@repo_b//:guava", vec![], guava_path, src2_path),
        ];
        graph.populate_from_aspects(&results, Path::new(tmp.path()));

        let ws = tmp.path().to_string_lossy().into_owned();
        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, Some(&ws))
                .unwrap();

        let jar_entries: Vec<_> = cp.entries.iter().filter(|e| e.path == guava_path).collect();
        assert_eq!(
            jar_entries.len(),
            1,
            "Duplicate JAR should appear only once"
        );
        assert_eq!(
            jar_entries[0].source_attachment_path,
            Some(src1_path.to_string()),
            "First valid source should be preserved, not overwritten by later target"
        );
    }

    #[test]
    fn test_importer_named_target_gets_transitive_deps() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target(
                "//funds/csv:funds_csv_importer",
                vec!["@maven//:guava", "//lib:utils"],
                vec!["/importer.jar"],
            ),
            make_target("@maven//:guava", vec![], vec!["/guava.jar"]),
            make_target("//lib:utils", vec![], vec!["/utils.jar"]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp = ComputedClasspath::compute_for_targets(
            &graph,
            &["//funds/csv:funds_csv_importer"],
            None,
        )
        .unwrap();

        let lib_paths: Vec<&str> = cp
            .entries
            .iter()
            .filter(|e| e.entry_type == ClasspathEntryType::Library)
            .map(|e| e.path.as_str())
            .collect();

        assert!(
            lib_paths.contains(&"/guava.jar"),
            "Expected transitive dep guava.jar for 'importer' target, got: {:?}",
            lib_paths
        );
        assert!(
            lib_paths.contains(&"/utils.jar"),
            "Expected transitive dep utils.jar for 'importer' target, got: {:?}",
            lib_paths
        );
    }

    // --- output_jars serialization tests ---

    #[test]
    fn test_output_jars_included_in_pipe_delimited() {
        let cp = ComputedClasspath {
            target_label: "//app:app".to_string(),
            entries: vec![],
            source_roots: Vec::new(),
            generated_source_dirs: Vec::new(),
            annotation_processors: Vec::new(),
            output_jars: vec!["/bazel-bin/app/app.jar".to_string()],
        };
        let lines = cp.to_pipe_delimited_entries();
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0], "LIB|/bazel-bin/app/app.jar||false|false|",
            "output_jar should be serialized as a LIB entry"
        );
    }

    #[test]
    fn test_output_jars_before_dependency_entries() {
        let cp = ComputedClasspath {
            target_label: "//app:app".to_string(),
            entries: vec![ClasspathEntry {
                entry_type: ClasspathEntryType::Library,
                path: "/guava.jar".to_string(),
                source_attachment_path: None,
                is_test: false,
                is_exported: false,
                access_rules: Vec::new(),
                visibility: Visibility::default(),
            }],
            source_roots: Vec::new(),
            generated_source_dirs: Vec::new(),
            annotation_processors: Vec::new(),
            output_jars: vec!["/bazel-bin/app/app.jar".to_string()],
        };
        let lines = cp.to_pipe_delimited_entries();
        assert_eq!(lines.len(), 2);
        assert!(
            lines[0].contains("/bazel-bin/app/app.jar"),
            "output_jar should appear BEFORE dependency entries, got: {:?}",
            lines
        );
        assert!(
            lines[1].contains("/guava.jar"),
            "dependency entry should appear AFTER output_jar, got: {:?}",
            lines
        );
    }

    #[test]
    fn test_empty_output_jars_no_extra_entries() {
        let dep_entry = ClasspathEntry {
            entry_type: ClasspathEntryType::Library,
            path: "/guava.jar".to_string(),
            source_attachment_path: None,
            is_test: false,
            is_exported: false,
            access_rules: Vec::new(),
            visibility: Visibility::default(),
        };
        let cp = ComputedClasspath {
            target_label: "//app:app".to_string(),
            entries: vec![dep_entry],
            source_roots: Vec::new(),
            generated_source_dirs: Vec::new(),
            annotation_processors: Vec::new(),
            output_jars: Vec::new(),
        };
        let lines = cp.to_pipe_delimited_entries();
        assert_eq!(
            lines.len(),
            1,
            "Empty output_jars should not add extra entries"
        );
    }

    #[test]
    fn test_merged_targets_output_jars_in_pipe_delimited() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//pkg:A", vec![], vec!["/a.jar"]),
            make_target("//pkg:B", vec![], vec!["/b.jar"]),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp =
            ComputedClasspath::compute_for_targets(&graph, &["//pkg:A", "//pkg:B"], None).unwrap();
        let lines = cp.to_pipe_delimited_entries();

        let output_jar_lines: Vec<&String> = lines
            .iter()
            .filter(|l| l.contains("/a.jar") || l.contains("/b.jar"))
            .collect();
        assert!(
            output_jar_lines.len() >= 2,
            "Expected output_jars from both targets in pipe-delimited output, got: {:?}",
            lines
        );
    }

    #[test]
    fn test_java_import_output_jars_serialized() {
        let mut graph = DependencyGraph::new();
        let results = vec![make_target_with_jar_path(
            "@maven//:guava",
            vec![],
            "/guava.jar",
        )];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp =
            ComputedClasspath::compute_for(&graph, "@maven//:guava", TargetKind::JavaImport, None)
                .unwrap();

        assert!(
            !cp.output_jars.is_empty(),
            "java_import should have output_jars"
        );
        let lines = cp.to_pipe_delimited_entries();
        let has_guava = lines.iter().any(|l| l.contains("/guava.jar"));
        assert!(
            has_guava,
            "java_import output_jars should appear in pipe-delimited output, got: {:?}",
            lines
        );
    }

    #[test]
    fn test_output_jars_deduplicated_against_entries() {
        let shared_path = "/workspace/bazel-bin/lib/libfoo.jar";
        let cp = ComputedClasspath {
            target_label: "//lib:foo".to_string(),
            entries: vec![ClasspathEntry {
                entry_type: ClasspathEntryType::Library,
                path: shared_path.to_string(),
                source_attachment_path: Some("/workspace/lib/src".to_string()),
                is_test: false,
                is_exported: true,
                access_rules: vec![],
                visibility: Visibility::Public,
            }],
            output_jars: vec![shared_path.to_string()],
            source_roots: vec![],
            generated_source_dirs: vec![],
            annotation_processors: vec![],
        };

        let lines = cp.to_pipe_delimited_entries();
        let count = lines.iter().filter(|l| l.contains(shared_path)).count();
        assert_eq!(
            count, 1,
            "JAR present in both output_jars and entries should appear only once, got: {:?}",
            lines
        );
        assert!(
            lines[0].starts_with("LIB|"),
            "the single entry should be the dependency LIB entry (with source attachment)"
        );
        assert!(
            lines[0].contains("/workspace/lib/src"),
            "should preserve source attachment from the dependency entry"
        );
    }

    fn make_target_with_kind(
        label: &str,
        kind: &str,
        deps: Vec<&str>,
        runtime_deps: Vec<&str>,
        jar_paths: Vec<&str>,
    ) -> TargetIdeInfo {
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
            kind: kind.to_string(),
            build_file: None,
            java_info: Some(JavaIdeInfo {
                jars,
                ..Default::default()
            }),
            deps: deps.iter().map(|s| s.to_string()).collect(),
            runtime_deps: runtime_deps.iter().map(|s| s.to_string()).collect(),
            exports: Vec::new(),
        }
    }

    #[test]
    fn test_java_binary_no_srcs_includes_runtime_deps_jars() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target_with_kind(
                "//app:app",
                "java_binary",
                vec![],
                vec!["//lib:lib"],
                vec!["/workspace/bazel-bin/app/app.jar"],
            ),
            make_target_with_kind(
                "//lib:lib",
                "java_library",
                vec![],
                vec![],
                vec!["/workspace/bazel-bin/lib/liblib.jar"],
            ),
        ];

        graph.populate_from_aspects(&results, Path::new("/workspace"));
        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaBinary, None)
                .unwrap();

        assert!(
            cp.output_jars
                .contains(&"/workspace/bazel-bin/lib/liblib.jar".to_string()),
            "java_binary output_jars should contain runtime_deps jar, got: {:?}",
            cp.output_jars
        );
    }

    #[test]
    fn test_java_binary_with_srcs_includes_dep_jars() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target_with_kind(
                "//app:app",
                "java_binary",
                vec!["//lib:lib"],
                vec![],
                vec!["/workspace/bazel-bin/app/app.jar"],
            ),
            make_target_with_kind(
                "//lib:lib",
                "java_library",
                vec![],
                vec![],
                vec!["/workspace/bazel-bin/lib/liblib.jar"],
            ),
        ];

        graph.populate_from_aspects(&results, Path::new("/workspace"));
        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaBinary, None)
                .unwrap();

        assert!(
            cp.output_jars
                .contains(&"/workspace/bazel-bin/app/app.jar".to_string()),
            "should contain target's own jar, got: {:?}",
            cp.output_jars
        );
        assert!(
            cp.output_jars
                .contains(&"/workspace/bazel-bin/lib/liblib.jar".to_string()),
            "should contain dep jar, got: {:?}",
            cp.output_jars
        );
    }

    #[test]
    fn test_java_binary_dep_jar_deduplicated_in_serialization() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target_with_kind(
                "//app:app",
                "java_binary",
                vec!["//lib:lib"],
                vec![],
                vec!["/workspace/bazel-bin/app/app.jar"],
            ),
            make_target_with_kind(
                "//lib:lib",
                "java_library",
                vec![],
                vec![],
                vec!["/workspace/bazel-bin/lib/liblib.jar"],
            ),
        ];

        graph.populate_from_aspects(&results, Path::new("/workspace"));
        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaBinary, None)
                .unwrap();

        let lines = cp.to_pipe_delimited_entries();
        let lib_count = lines
            .iter()
            .filter(|l| l.contains("/workspace/bazel-bin/lib/liblib.jar"))
            .count();
        assert_eq!(
            lib_count, 1,
            "dep jar should appear exactly once in serialized output, got: {:?}",
            lines
        );
    }

    #[test]
    fn test_java_library_output_jars_unchanged() {
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target_with_kind(
                "//lib:lib",
                "java_library",
                vec!["//lib:dep"],
                vec![],
                vec!["/workspace/bazel-bin/lib/liblib.jar"],
            ),
            make_target_with_kind(
                "//lib:dep",
                "java_library",
                vec![],
                vec![],
                vec!["/workspace/bazel-bin/lib/libdep.jar"],
            ),
        ];

        graph.populate_from_aspects(&results, Path::new("/workspace"));
        let cp =
            ComputedClasspath::compute_for(&graph, "//lib:lib", TargetKind::JavaLibrary, None)
                .unwrap();

        assert_eq!(
            cp.output_jars,
            vec!["/workspace/bazel-bin/lib/liblib.jar"],
            "java_library output_jars should only contain target's own jars"
        );
        assert!(
            !cp.output_jars
                .contains(&"/workspace/bazel-bin/lib/libdep.jar".to_string()),
            "java_library output_jars should NOT contain dep jars"
        );
    }
}
