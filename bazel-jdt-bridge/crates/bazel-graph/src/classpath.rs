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
    ) -> Result<Self, GraphError> {
        let is_test = target_kind == TargetKind::JavaTest;

        match target_kind {
            TargetKind::JavaImport => Self::compute_for_import(graph, target_label),
            TargetKind::JavaLibrary
            | TargetKind::JavaBinary
            | TargetKind::JavaTest
            | TargetKind::Unknown => Self::compute_for_library(graph, target_label, is_test),
        }
    }

    fn compute_for_library(
        graph: &DependencyGraph,
        target_label: &str,
        is_test: bool,
    ) -> Result<Self, GraphError> {
        let deps = graph.transitive_deps(target_label)?;

        let mut entries = Vec::new();
        let mut seen_jars = std::collections::HashSet::new();

        for dep_label in &deps {
            if let Some(jars) = graph.get_target_jars(dep_label) {
                for jar in jars {
                    if seen_jars.insert(jar.clone()) {
                        entries.push(ClasspathEntry {
                            entry_type: ClasspathEntryType::Library,
                            path: jar.clone(),
                            source_attachment_path: None,
                            is_test,
                            is_exported: false,
                            access_rules: Vec::new(),
                            visibility: Visibility::default(),
                        });
                    }
                }
            }

            entries.push(ClasspathEntry {
                entry_type: ClasspathEntryType::Project,
                path: dep_label.clone(),
                source_attachment_path: None,
                is_test,
                is_exported: false,
                access_rules: Vec::new(),
                visibility: Visibility::default(),
            });
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
                entries.push(ClasspathEntry {
                    entry_type: ClasspathEntryType::Library,
                    path: jar.clone(),
                    source_attachment_path: None,
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
        // In Bazel, visibility is enforced at the target level, not JAR level.
        // For now, this is a no-op placeholder — actual visibility enforcement
        // requires Bazel query integration to check visibility rules.
        // The access_rules field already handles package-level visibility filtering
        // that JDT.LS enforces at the IDE level.
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
