use std::path::{Path, PathBuf};
use tokio::process::Command;

use bazel_aspect::TargetIdeInfo;

/// Returns the platform-specific Bazel binary name.
/// On Windows, returns "bazel.exe"; on Unix, returns "bazel".
fn bazel_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "bazel.exe"
    } else {
        "bazel"
    }
}

/// Normalizes path separators to forward slashes for cross-platform compatibility.
#[cfg(target_os = "windows")]
fn normalize_path_separators(path: &str) -> String {
    path.replace('\\', "/")
}

#[cfg(not(target_os = "windows"))]
fn normalize_path_separators(path: &str) -> String {
    path.to_string()
}

/// Error type for Bazel command execution
#[derive(Debug, thiserror::Error)]
pub enum BazelError {
    #[error("Bazel command failed: {message}")]
    CommandFailed { message: String },

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("UTF-8 error: {0}")]
    Utf8Error(#[from] std::string::FromUtf8Error),
}

/// Async Bazel command invoker
pub struct BazelInvoker {
    bazel_path: String,
    workspace_root: PathBuf,
    aspect_label: String,
}

impl BazelInvoker {
    pub fn new(bazel_path: &str, workspace_root: &Path, aspect_label: &str) -> Self {
        Self {
            bazel_path: bazel_path.to_string(),
            workspace_root: workspace_root.to_path_buf(),
            aspect_label: aspect_label.to_string(),
        }
    }

    pub fn with_default_bazel(workspace_root: &Path, aspect_label: &str) -> Self {
        Self::new(bazel_binary_name(), workspace_root, aspect_label)
    }

    /// Discover Java targets. `None` or empty scope = all targets.
    /// Patterns starting with `-` are excluded via `except`.
    pub async fn discover_java_targets(
        &self,
        scope_patterns: Option<&[String]>,
        build_flags: Option<&[String]>,
    ) -> Result<Vec<String>, BazelError> {
        let query = build_java_target_query(scope_patterns);

        let mut cmd = Command::new(&self.bazel_path);
        cmd.current_dir(&self.workspace_root);
        cmd.arg("query");
        if let Some(flags) = build_flags {
            for flag in flags {
                cmd.arg(flag);
            }
        }
        cmd.args(["--output=label", &query]);
        let output = cmd.output().await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(BazelError::CommandFailed {
                message: format!("bazel query failed: {}", stderr),
            });
        }

        let stdout = String::from_utf8(output.stdout)?;
        let targets = stdout
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();

        Ok(targets)
    }

    /// Build targets with IntelliJ aspects to get dependency info.
    /// Bazel writes aspect output file paths to stderr, not stdout.
    pub async fn build_with_aspects(
        &self,
        targets: &[String],
        aspect_file: &str,
        build_flags: Option<&[String]>,
    ) -> Result<String, BazelError> {
        let targets_arg = targets.join(" ");

        let mut cmd = Command::new(&self.bazel_path);
        cmd.current_dir(&self.workspace_root);
        cmd.arg("build");
        if let Some(flags) = build_flags {
            for flag in flags {
                cmd.arg(flag);
            }
        }
        cmd.args([
            &format!("--aspects={}", aspect_file),
            "--output_groups=intellij-info-java,intellij-info-generic",
            "--show_result=100",
            &targets_arg,
        ]);
        let output = cmd.output().await?;

        let stderr = String::from_utf8(output.stderr)?;

        if !output.status.success() {
            return Err(BazelError::CommandFailed {
                message: format!("bazel build with aspects failed: {}", stderr),
            });
        }

        Ok(stderr)
    }

    /// Get the execution root path for this workspace (e.g., for locating bazel-out artifacts)
    pub fn get_execution_root(&self) -> PathBuf {
        self.workspace_root.join("bazel-out")
    }

    /// Resolve full classpath information for targets using IntelliJ aspects.
    /// This is the "Slow Path" — invokes Bazel build with aspects.
    pub async fn resolve_full_classpath(
        &self,
        targets: &[String],
    ) -> Result<Vec<TargetIdeInfo>, BazelError> {
        self.resolve_full_classpath_with_flags(targets, None).await
    }

    pub async fn resolve_full_classpath_with_flags(
        &self,
        targets: &[String],
        build_flags: Option<&[String]>,
    ) -> Result<Vec<TargetIdeInfo>, BazelError> {
        if targets.is_empty() {
            return Ok(Vec::new());
        }

        let aspect_output = self
            .build_with_aspects(targets, &self.aspect_label, build_flags)
            .await?;

        let info_files = crate::output::parse_aspect_output_locations(&aspect_output);

        let mut results = Vec::new();
        for info_path in &info_files {
            let normalized_path = normalize_path_separators(info_path);
            let absolute_path = self.workspace_root.join(&normalized_path);
            match tokio::fs::read_to_string(&absolute_path).await {
                Ok(content) => {
                    let mut target_info =
                        bazel_aspect::text_proto::parse_text_proto_quiet(&content);
                    resolve_artifact_paths(&mut target_info, &self.workspace_root);
                    results.push(target_info);
                }
                Err(e) => {
                    log::warn!(
                        "Failed to read aspect output file {}: {}",
                        normalized_path,
                        e
                    );
                    continue;
                }
            }
        }

        Ok(results)
    }
}

/// Build a bazel query string for discovering Java targets.
/// `None` or empty scope → query all targets under `//...:*`.
/// Patterns starting with `-` are treated as negative patterns using `except`.
pub fn build_java_target_query(scope_patterns: Option<&[String]>) -> String {
    const JAVA_KINDS: &[&str] = &["java_library", "java_binary", "java_test", "java_import"];
    const DEFAULT_SCOPE: &str = "//...:*";

    let patterns = match scope_patterns {
        Some(p) if !p.is_empty() => p,
        _ => {
            return JAVA_KINDS
                .iter()
                .map(|k| format!("kind({}, {})", k, DEFAULT_SCOPE))
                .collect::<Vec<_>>()
                .join(" union ");
        }
    };

    let (positive, negative): (Vec<_>, Vec<_>) = patterns.iter().partition(|p| !p.starts_with('-'));

    let scope = if positive.is_empty() {
        DEFAULT_SCOPE.to_string()
    } else {
        positive
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(" union ")
    };

    let mut query = JAVA_KINDS
        .iter()
        .map(|k| format!("kind({}, {})", k, &scope))
        .collect::<Vec<_>>()
        .join(" union ");

    if !negative.is_empty() {
        let neg_patterns: Vec<&str> = negative
            .iter()
            .map(|p| p.strip_prefix('-').unwrap_or(p))
            .collect();
        query.push_str(" except ");
        query.push_str(&neg_patterns.join(" union "));
    }

    query
}

fn resolve_artifact_paths(info: &mut bazel_aspect::TargetIdeInfo, workspace_root: &Path) {
    let resolve_loc = |loc: &mut bazel_aspect::ArtifactLocation| {
        if loc.absolute_path.is_none() {
            if let Some(combined) = loc.best_path() {
                let absolute = workspace_root.join(&combined);
                loc.absolute_path = Some(absolute.to_string_lossy().into_owned());
            }
        }
    };

    if let Some(ref mut java) = info.java_info {
        for jar in &mut java.jars {
            resolve_loc(&mut jar.jar);
            if let Some(ref mut src) = jar.source_jar {
                resolve_loc(src);
            }
            if let Some(ref mut iface) = jar.interface_jar {
                resolve_loc(iface);
            }
        }
        for loc in &mut java.sources {
            resolve_loc(loc);
        }
        for loc in &mut java.compile_jars {
            resolve_loc(loc);
        }
        for loc in &mut java.runtime_jars {
            resolve_loc(loc);
        }
        for loc in &mut java.source_jars {
            resolve_loc(loc);
        }
        for jar in &mut java.generated_jars {
            resolve_loc(&mut jar.jar);
            if let Some(ref mut src) = jar.source_jar {
                resolve_loc(src);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_query_no_scope() {
        let query = build_java_target_query(None);
        assert!(query.contains("kind(java_library, //...:*)"));
        assert!(query.contains("kind(java_binary, //...:*)"));
        assert!(query.contains("kind(java_test, //...:*)"));
        assert!(query.contains("kind(java_import, //...:*)"));
        assert!(!query.contains("except"));
    }

    #[test]
    fn test_build_query_empty_scope() {
        let empty: Vec<String> = vec![];
        let query = build_java_target_query(Some(&empty));
        assert!(query.contains("kind(java_library, //...:*)"));
        assert!(!query.contains("except"));
    }

    #[test]
    fn test_build_query_with_scope() {
        let patterns = vec!["//services/...:*".to_string()];
        let query = build_java_target_query(Some(&patterns));
        assert!(query.contains("kind(java_library, //services/...:*)"));
        assert!(query.contains("kind(java_binary, //services/...:*)"));
        assert!(!query.contains("//...:*"));
        assert!(!query.contains("except"));
    }

    #[test]
    fn test_build_query_with_multiple_scopes() {
        let patterns = vec![
            "//services/...:*".to_string(),
            "//libs/core/...:*".to_string(),
        ];
        let query = build_java_target_query(Some(&patterns));
        assert!(query.contains("//services/...:*"));
        assert!(query.contains("//libs/core/...:*"));
        assert!(!query.contains("except"));
    }

    #[test]
    fn test_build_query_with_negative_patterns() {
        let patterns = vec!["//...:*".to_string(), "-//experimental/...:*".to_string()];
        let query = build_java_target_query(Some(&patterns));
        assert!(query.contains("kind(java_library, //...:*)"));
        assert!(query.contains("except //experimental/...:*"));
        assert!(!query.contains("-//"));
    }

    #[test]
    fn test_build_query_multiple_negative_patterns() {
        let patterns = vec![
            "//...:*".to_string(),
            "-//experimental/...:*".to_string(),
            "-//third_party/...:*".to_string(),
        ];
        let query = build_java_target_query(Some(&patterns));
        assert!(query.contains("except"));
        assert!(query.contains("//experimental/...:*"));
        assert!(query.contains("//third_party/...:*"));
        assert!(!query.contains("-//"));
    }

    #[test]
    fn test_build_query_only_negative_uses_default_scope() {
        let patterns = vec!["-//experimental/...:*".to_string()];
        let query = build_java_target_query(Some(&patterns));
        assert!(query.contains("kind(java_library, //...:*)"));
        assert!(query.contains("except //experimental/...:*"));
    }
}
