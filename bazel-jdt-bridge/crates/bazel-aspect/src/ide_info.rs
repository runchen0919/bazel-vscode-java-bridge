use serde::{Deserialize, Serialize};

/// Represents a fully resolved Java target from IntelliJ aspect output
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TargetIdeInfo {
    pub label: String,
    pub kind: String,
    pub build_file: Option<ArtifactLocation>,
    pub java_info: Option<JavaIdeInfo>,
    pub deps: Vec<String>,
    pub runtime_deps: Vec<String>,
    pub exports: Vec<String>,
}

impl TargetIdeInfo {
    pub fn new(label: String, kind: String) -> Self {
        Self {
            label,
            kind,
            build_file: None,
            java_info: None,
            deps: Vec::new(),
            runtime_deps: Vec::new(),
            exports: Vec::new(),
        }
    }

    pub fn is_java_target(&self) -> bool {
        self.java_info.is_some()
    }
}

impl Default for TargetIdeInfo {
    fn default() -> Self {
        Self::new(String::new(), String::new())
    }
}

/// Java-specific information from aspect output
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JavaIdeInfo {
    pub sources: Vec<ArtifactLocation>,
    pub jars: Vec<JarInfo>,
    pub generated_jars: Vec<JarInfo>,
    pub compile_jars: Vec<ArtifactLocation>,
    pub runtime_jars: Vec<ArtifactLocation>,
    pub annotation_processors: Vec<String>,
    pub source_jars: Vec<ArtifactLocation>,
    pub javac_options: Option<JavacOptions>,
}

/// JAR artifact with metadata
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct JarInfo {
    pub jar: ArtifactLocation,
    pub source_jar: Option<ArtifactLocation>,
    pub interface_jar: Option<ArtifactLocation>,
}

/// File artifact location within Bazel workspace
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArtifactLocation {
    pub relative_path: Option<String>,
    pub absolute_path: Option<String>,
    pub is_source: bool,
    pub is_external: bool,
    pub root_execution_path_fragment: Option<String>,
}

impl ArtifactLocation {
    /// Get the best available path for this artifact
    pub fn best_path(&self) -> Option<&str> {
        self.absolute_path
            .as_deref()
            .or(self.relative_path.as_deref())
    }
}

/// Javac compiler options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JavacOptions {
    pub options: Vec<String>,
}
