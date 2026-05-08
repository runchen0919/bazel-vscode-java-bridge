use crate::graph::{DependencyGraph, GraphError};
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
                Self::compute_for_library(graph, target_label, is_test, workspace_root)
            }
        }
    }

    fn compute_for_library(
        graph: &DependencyGraph,
        target_label: &str,
        is_test_context: bool,
        workspace_root: Option<&str>,
    ) -> Result<Self, GraphError> {
        let deps = graph.transitive_deps(target_label)?;

        let mut entries = Vec::new();
        let mut seen_jars = std::collections::HashSet::new();

        for dep_label in &deps {
            let dep_is_testonly = is_test_context && graph.is_testonly(dep_label);

            // Bazel 6+ uses "@@" canonical labels for external repos. Only skip
            // toolchain/platform targets — Maven deps etc. must pass through.
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
                    if seen_jars.insert(jar.clone()) {
                        let source_path = graph.get_target_source_jar(dep_label, jar);
                        let effective_source = if source_path.is_some() {
                            source_path
                        } else if is_workspace_internal {
                            infer_source_attachment(dep_label, workspace_root)
                        } else {
                            None
                        };
                        entries.push(ClasspathEntry {
                            entry_type: ClasspathEntryType::Library,
                            path: jar.clone(),
                            source_attachment_path: effective_source,
                            is_test: dep_is_testonly,
                            is_exported: false,
                            access_rules: Vec::new(),
                            visibility: Visibility::default(),
                        });
                    }
                }
            }
        }

        let output_jars = graph
            .get_target_jars(target_label)
            .cloned()
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

    fn compute_for_import(graph: &DependencyGraph, target_label: &str) -> Result<Self, GraphError> {
        if !graph.has_target(target_label) {
            return Err(GraphError::TargetNotFound {
                label: target_label.to_string(),
            });
        }

        let mut entries = Vec::new();

        if let Some(jars) = graph.get_target_jars(target_label) {
            for jar in jars {
                let source_path = graph.get_target_source_jar(target_label, jar);
                entries.push(ClasspathEntry {
                    entry_type: ClasspathEntryType::Library,
                    path: jar.clone(),
                    source_attachment_path: source_path,
                    is_test: false,
                    is_exported: false,
                    access_rules: Vec::new(),
                    visibility: Visibility::default(),
                });
            }
        }

        let output_jars = graph
            .get_target_jars(target_label)
            .cloned()
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

    /// Convert to pipe-delimited string array for JNI
    pub fn to_pipe_delimited_entries(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|entry| {
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
                format!(
                    "{}|{}|{}|{}|{}|{}",
                    type_str, entry.path, source, entry.is_test, entry.is_exported, access
                )
            })
            .collect()
    }
}

fn infer_source_attachment(dep_label: &str, workspace_root: Option<&str>) -> Option<String> {
    let ws_root = workspace_root?;
    let label = dep_label.strip_prefix("//")?;
    let package_path = label.split(':').next().unwrap_or(label);
    if package_path.is_empty() {
        return None;
    }
    let source_root_markers = [
        "src/main/java/",
        "src/test/java/",
        "src/java/",
        "javatests/",
        "java/",
    ];
    for marker in &source_root_markers {
        if let Some(idx) = package_path.find(marker) {
            let root = &package_path[..idx + marker.len() - 1];
            return Some(format!("{}/{}", ws_root, root));
        }
    }
    None
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
        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None).unwrap();

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
        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None).unwrap();

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
        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None).unwrap();

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
            ComputedClasspath::compute_for(&graph, "//app:app_test", TargetKind::JavaTest, None).unwrap();

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
            ComputedClasspath::compute_for(&graph, "//app:app_test", TargetKind::JavaTest, None).unwrap();

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

        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None).unwrap();

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

        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None).unwrap();

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

        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None).unwrap();

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

        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None).unwrap();

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

        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None).unwrap();

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

        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None).unwrap();

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

        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None).unwrap();

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

        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None).unwrap();

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

        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None).unwrap();

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

        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None).unwrap();

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
            vec!["/guava.jar".to_string()],
        );
        graph.label_aliases.insert(
            "@maven//:guava".to_string(),
            "@@rules_jvm_external~maven~maven//:guava".to_string(),
        );

        let jars = graph.get_target_jars("@maven//:guava");
        assert!(jars.is_some());
        assert_eq!(jars.unwrap()[0], "/guava.jar");
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
            vec!["/guava-33.4.0-jre.jar".to_string()],
        );
        graph.label_aliases.insert(
            "@maven//:com_google_guava_guava".to_string(),
            "@@rules_jvm_external~maven~maven//:com_google_guava_guava".to_string(),
        );

        let cp =
            ComputedClasspath::compute_for(&graph, "//utils:string_utils", TargetKind::JavaLibrary, None)
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
        let mut graph = DependencyGraph::new();
        let results = vec![
            make_target("//app:app", vec!["@maven//:guava"], vec!["/app.jar"]),
            make_target_with_source_jar(
                "@maven//:guava",
                vec![],
                "/guava.jar",
                "/guava-sources.jar",
            ),
        ];
        graph.populate_from_aspects(&results, Path::new("/workspace"));

        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None).unwrap();

        let guava_entry = cp
            .entries
            .iter()
            .find(|e| e.path == "/guava.jar")
            .expect("Expected guava.jar entry");
        assert_eq!(
            guava_entry.source_attachment_path,
            Some("/guava-sources.jar".to_string()),
            "Expected source attachment for guava.jar"
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

        let cp =
            ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaLibrary, None).unwrap();

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
}
