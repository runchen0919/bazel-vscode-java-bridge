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

    /// Discover all Java targets in the workspace
    pub async fn discover_java_targets(&self) -> Result<Vec<String>, BazelError> {
        let output = Command::new(&self.bazel_path)
            .current_dir(&self.workspace_root)
            .args([
                "query",
                "--output=label",
                "kind(java_library, //...:*) union kind(java_binary, //...:*) union kind(java_test, //...:*) union kind(java_import, //...:*)",
            ])
            .output()
            .await?;

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

    /// Build targets with IntelliJ aspects to get dependency info
    pub async fn build_with_aspects(
        &self,
        targets: &[String],
        aspect_file: &str,
    ) -> Result<String, BazelError> {
        let targets_arg = targets.join(" ");

        let output = Command::new(&self.bazel_path)
            .current_dir(&self.workspace_root)
            .args([
                "build",
                &format!("--aspects={}", aspect_file),
                "--output_groups=intellij-info-generic",
                &targets_arg,
            ])
            .output()
            .await?;

        let stdout = String::from_utf8(output.stdout)?;
        let stderr = String::from_utf8(output.stderr)?;

        if !output.status.success() {
            return Err(BazelError::CommandFailed {
                message: format!("bazel build with aspects failed: {}", stderr),
            });
        }

        Ok(stdout)
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
        // If no targets, return empty
        if targets.is_empty() {
            return Ok(Vec::new());
        }

        // Step 1: Build with IntelliJ aspects
        let aspect_output = self.build_with_aspects(targets, &self.aspect_label).await?;

        // Step 2: Parse aspect output to get .intellij-info.txt file locations
        let info_files = crate::output::parse_aspect_output_locations(&aspect_output);

        // Step 3: Read and parse each info file
        let mut results = Vec::new();
        for info_path in &info_files {
            let normalized_path = normalize_path_separators(info_path);
            match tokio::fs::read_to_string(&normalized_path).await {
                Ok(content) => {
                    let target_info = bazel_aspect::text_proto::parse_text_proto_quiet(&content);
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
