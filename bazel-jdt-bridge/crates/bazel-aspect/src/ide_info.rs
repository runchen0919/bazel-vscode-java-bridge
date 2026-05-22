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
    pub root_path: Option<String>,
}

impl ArtifactLocation {
    /// Get the best available path for this artifact.
    /// Combines root_path + relative_path when absolute_path is not set.
    pub fn best_path(&self) -> Option<String> {
        if let Some(ref abs) = self.absolute_path {
            return Some(abs.clone());
        }
        match (&self.root_path, &self.relative_path) {
            (Some(root), Some(rel)) => Some(format!(
                "{}/{}",
                root.trim_end_matches('/'),
                rel.trim_start_matches('/')
            )),
            (_, Some(rel)) => Some(rel.clone()),
            _ => None,
        }
    }
}

/// Javac compiler options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JavacOptions {
    pub options: Vec<String>,
}

/// Derive the apparent repo label from a bzlmod canonical label.
/// Canonical form: `@@<module>~<ext>~<repo_name>//<rest>`
/// Apparent form: `@<repo_name>//<rest>`
/// Returns `None` for labels that don't match the canonical bzlmod pattern.
pub fn canonical_to_apparent_label(label: &str) -> Option<String> {
    if !label.starts_with("@@") {
        return None;
    }
    let rest = &label[2..];
    let slash_pos = rest.find("//")?;
    let repo_part = &rest[..slash_pos];
    if !repo_part.contains('~') {
        return None;
    }
    let after_slash = &rest[slash_pos..];
    let apparent_repo = repo_part.rsplit('~').next()?;
    Some(format!("@{}{}", apparent_repo, after_slash))
}

#[cfg(test)]
mod tests {
    use super::canonical_to_apparent_label;

    #[test]
    fn test_canonical_maven_label_to_apparent() {
        assert_eq!(
            canonical_to_apparent_label(
                "@@rules_jvm_external~maven~maven//:com_google_guava_guava"
            ),
            Some("@maven//:com_google_guava_guava".to_string())
        );
    }

    #[test]
    fn test_canonical_multi_segment_repo() {
        assert_eq!(
            canonical_to_apparent_label("@@some_module~ext~my_repo~nested//pkg:target"),
            Some("@nested//pkg:target".to_string())
        );
    }

    #[test]
    fn test_single_at_label_returns_none() {
        assert_eq!(
            canonical_to_apparent_label("@maven//:com_google_guava_guava"),
            None
        );
    }

    #[test]
    fn test_workspace_internal_returns_none() {
        assert_eq!(canonical_to_apparent_label("//utils:string_utils"), None);
    }

    #[test]
    fn test_bazel_tools_returns_none() {
        assert_eq!(
            canonical_to_apparent_label("@@bazel_tools//tools/jdk:toolchain"),
            None
        );
    }

    #[test]
    fn test_platforms_returns_none() {
        assert_eq!(canonical_to_apparent_label("@@platforms//cpu:cpu"), None);
    }

    use super::ArtifactLocation;

    #[test]
    fn test_best_path_prefers_absolute_path() {
        let loc = ArtifactLocation {
            absolute_path: Some("/execroot/ws/external/maven/v1/lib.jar".to_string()),
            root_path: Some("external/maven/v1".to_string()),
            relative_path: Some("lib.jar".to_string()),
            ..Default::default()
        };
        assert_eq!(
            loc.best_path(),
            Some("/execroot/ws/external/maven/v1/lib.jar".to_string())
        );
    }

    #[test]
    fn test_best_path_falls_back_to_root_plus_relative() {
        let loc = ArtifactLocation {
            absolute_path: None,
            root_path: Some("bazel-out/k8-fastbuild/bin".to_string()),
            relative_path: Some("app/libapp.jar".to_string()),
            ..Default::default()
        };
        assert_eq!(
            loc.best_path(),
            Some("bazel-out/k8-fastbuild/bin/app/libapp.jar".to_string())
        );
    }

    #[test]
    fn test_best_path_relative_only() {
        let loc = ArtifactLocation {
            absolute_path: None,
            root_path: None,
            relative_path: Some("src/Main.java".to_string()),
            ..Default::default()
        };
        assert_eq!(loc.best_path(), Some("src/Main.java".to_string()));
    }
}
