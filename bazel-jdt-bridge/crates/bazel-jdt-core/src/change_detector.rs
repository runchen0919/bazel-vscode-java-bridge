//! Change detection logic for incremental BUILD file changes.
//!
//! This module compares parsed BUILD files before and after modifications
//! to identify affected Java targets that need re-resolution.

use bazel_parser::model::{JavaRule, ParsedBuildFile, RuleType};
use sha2::Digest;
use std::path::PathBuf;

/// Type of change detected for a rule
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeType {
    /// Rule was added in the new version
    Added,
    /// Rule was removed in the new version
    Removed,
    /// Rule exists in both but has field changes
    Modified,
    /// Rule exists in both with no changes
    Unchanged,
}

/// Types of field changes that can occur in a Java rule
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldChange {
    /// deps attribute changed
    DepsChanged,
    /// srcs attribute changed
    SrcsChanged,
    /// runtime_deps attribute changed
    RuntimeDepsChanged,
    /// resources attribute changed
    ResourcesChanged,
    /// plugins attribute changed
    PluginsChanged,
    /// exports attribute changed
    ExportsChanged,
    /// visibility attribute changed
    VisibilityChanged,
    /// testonly attribute changed
    TestOnlyChanged,
}

impl FieldChange {
    /// Returns true if this field change affects the classpath.
    ///
    /// Classpath-relevant changes require re-resolution of dependencies.
    pub fn is_classpath_relevant(&self) -> bool {
        match self {
            FieldChange::DepsChanged
            | FieldChange::SrcsChanged
            | FieldChange::RuntimeDepsChanged
            | FieldChange::ExportsChanged
            | FieldChange::PluginsChanged => true,
            FieldChange::VisibilityChanged
            | FieldChange::TestOnlyChanged
            | FieldChange::ResourcesChanged => false,
        }
    }
}

/// Represents the diff for a single rule between before/after states
#[derive(Debug, Clone)]
pub struct RuleDiff {
    /// Name of the rule
    pub rule_name: String,
    /// Type of the rule (java_library, java_test, etc.)
    pub rule_type: RuleType,
    /// What kind of change occurred
    pub change_type: ChangeType,
    /// Which fields changed (empty unless Modified)
    pub field_changes: Vec<FieldChange>,
}

/// Result of comparing two BUILD file versions
#[derive(Debug, Clone)]
pub struct ChangeResult {
    /// Path to the BUILD file
    pub file_path: PathBuf,
    /// Diffs for all rules that changed
    pub rule_diffs: Vec<RuleDiff>,
    /// Target labels that need re-resolution (format: "//package:name")
    pub affected_labels: Vec<String>,
    /// Whether any change affects the classpath
    pub is_classpath_relevant: bool,
}

/// Compares two parsed BUILD files and detects changes.
///
/// # Arguments
/// * `before` - The previous state of the BUILD file
/// * `after` - The current state of the BUILD file
///
/// # Returns
/// A `ChangeResult` containing all detected changes
pub fn detect_changes(before: &ParsedBuildFile, after: &ParsedBuildFile) -> ChangeResult {
    let mut rule_diffs = Vec::new();
    let mut affected_labels = Vec::new();
    let mut is_classpath_relevant = false;

    let before_rules: std::collections::HashMap<&str, &JavaRule> =
        before.rules.iter().map(|r| (r.name.as_str(), r)).collect();

    let after_rules: std::collections::HashMap<&str, &JavaRule> =
        after.rules.iter().map(|r| (r.name.as_str(), r)).collect();

    let package_label = compute_package_label_from_path(&after.path);

    for (name, after_rule) in &after_rules {
        if let Some(before_rule) = before_rules.get(name) {
            let field_changes = compute_field_changes(before_rule, after_rule);

            let change_type = if field_changes.is_empty() {
                ChangeType::Unchanged
            } else {
                ChangeType::Modified
            };

            let rule_classpath_relevant = !field_changes.is_empty()
                && field_changes.iter().any(|fc| fc.is_classpath_relevant());

            if rule_classpath_relevant {
                is_classpath_relevant = true;
                affected_labels.push(format!("{}:{}", package_label, name));
            }

            rule_diffs.push(RuleDiff {
                rule_name: name.to_string(),
                rule_type: after_rule.rule_type.clone(),
                change_type,
                field_changes,
            });
        } else {
            is_classpath_relevant = true;
            affected_labels.push(format!("{}:{}", package_label, name));

            rule_diffs.push(RuleDiff {
                rule_name: name.to_string(),
                rule_type: after_rule.rule_type.clone(),
                change_type: ChangeType::Added,
                field_changes: Vec::new(),
            });
        }
    }

    for (name, before_rule) in &before_rules {
        if !after_rules.contains_key(name) {
            is_classpath_relevant = true;
            affected_labels.push(format!("{}:{}", package_label, name));

            rule_diffs.push(RuleDiff {
                rule_name: name.to_string(),
                rule_type: before_rule.rule_type.clone(),
                change_type: ChangeType::Removed,
                field_changes: Vec::new(),
            });
        }
    }

    ChangeResult {
        file_path: after.path.clone(),
        rule_diffs,
        affected_labels,
        is_classpath_relevant,
    }
}

/// Computes the field-level changes between two versions of a rule.
fn compute_field_changes(before: &JavaRule, after: &JavaRule) -> Vec<FieldChange> {
    let mut changes = Vec::new();

    if before.deps != after.deps {
        changes.push(FieldChange::DepsChanged);
    }

    if before.srcs != after.srcs {
        changes.push(FieldChange::SrcsChanged);
    }

    if before.runtime_deps != after.runtime_deps {
        changes.push(FieldChange::RuntimeDepsChanged);
    }

    if before.resources != after.resources {
        changes.push(FieldChange::ResourcesChanged);
    }

    if before.plugins != after.plugins {
        changes.push(FieldChange::PluginsChanged);
    }

    if before.exports != after.exports {
        changes.push(FieldChange::ExportsChanged);
    }

    if before.visibility != after.visibility {
        changes.push(FieldChange::VisibilityChanged);
    }

    if before.test_only != after.test_only {
        changes.push(FieldChange::TestOnlyChanged);
    }

    changes
}

/// Detects which files were added or removed between two snapshots.
///
/// # Arguments
/// * `before_files` - List of file paths in the previous snapshot
/// * `after_files` - List of file paths in the current snapshot
///
/// # Returns
/// A tuple of (added_files, removed_files)
pub fn detect_added_removed_files(
    before_files: &[PathBuf],
    after_files: &[PathBuf],
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let before_set: std::collections::HashSet<_> = before_files.iter().collect();
    let after_set: std::collections::HashSet<_> = after_files.iter().collect();

    let added: Vec<PathBuf> = after_files
        .iter()
        .filter(|p| !before_set.contains(p))
        .cloned()
        .collect();

    let removed: Vec<PathBuf> = before_files
        .iter()
        .filter(|p| !after_set.contains(p))
        .cloned()
        .collect();

    (added, removed)
}

/// Computes the Bazel package label from a BUILD file path.
///
/// Given a BUILD file path like `/workspace/foo/bar/BUILD` and workspace root
/// `/workspace`, returns `//foo/bar`.
///
/// # Arguments
/// * `build_file_path` - Absolute path to the BUILD file
/// * `workspace_root` - Absolute path to the workspace root
///
/// # Returns
/// The package label in format `//package/path`
pub fn compute_build_file_package_label(
    build_file_path: &std::path::Path,
    workspace_root: &std::path::Path,
) -> String {
    if let Ok(relative) = build_file_path.strip_prefix(workspace_root) {
        if let Some(parent) = relative.parent() {
            let path_str = parent.to_string_lossy();
            if path_str.is_empty() {
                "//".to_string()
            } else {
                format!("//{}", path_str.replace('\\', "/"))
            }
        } else {
            "//".to_string()
        }
    } else {
        compute_package_label_from_path(build_file_path)
    }
}

/// Helper function to compute package label from a BUILD file path.
/// Uses the parent directory of the BUILD file.
fn compute_package_label_from_path(build_file_path: &std::path::Path) -> String {
    if let Some(parent) = build_file_path.parent() {
        let path_str = parent.to_string_lossy();
        if path_str.is_empty() {
            "//".to_string()
        } else {
            format!("//{}", path_str.replace('\\', "/"))
        }
    } else {
        "//".to_string()
    }
}

/// Computes a SHA-256 hash of a file's contents.
///
/// # Arguments
/// * `path` - Path to the file to hash
///
/// # Returns
/// The hex-encoded SHA-256 hash of the file contents
pub fn compute_file_hash(path: &std::path::Path) -> Result<String, std::io::Error> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = sha2::Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Result of scanning a workspace for BUILD file changes.
#[derive(Debug, Clone)]
pub struct WorkspaceChangeScan {
    /// BUILD files that have changed (hash differs from cache)
    pub changed_files: Vec<std::path::PathBuf>,
    /// BUILD files that are new (not in cache)
    pub new_files: Vec<std::path::PathBuf>,
    /// BUILD files that were deleted (in cache but not on disk)
    pub deleted_files: Vec<std::path::PathBuf>,
    /// Affected target labels (e.g., "//package:name")
    pub affected_labels: Vec<String>,
}

/// Scan workspace for BUILD file changes by comparing current hashes to cached hashes.
/// Returns a scan result identifying changed/new/deleted files and affected target labels.
///
/// # Arguments
/// * `workspace_root` - Root directory of the Bazel workspace
/// * `cache` - The cache to read/store BUILD file hashes
///
/// # Returns
/// A `WorkspaceChangeScan` with detected changes
pub fn scan_workspace_changes(
    workspace_root: &std::path::Path,
    cache: &bazel_cache::BazelCache,
) -> Result<WorkspaceChangeScan, Box<dyn std::error::Error>> {
    let current_build_files = collect_build_files(workspace_root)?;
    let mut changed = Vec::new();
    let mut new = Vec::new();
    let mut affected = Vec::new();
    let deleted = Vec::new();

    for build_file in &current_build_files {
        let path_str = build_file.to_string_lossy();
        let current_hash = compute_file_hash(build_file)?;

        match cache.get_build_hash(&path_str) {
            Ok(Some(cached_hash)) => {
                if cached_hash != current_hash {
                    changed.push(build_file.clone());
                    affected.push(compute_build_file_package_label(build_file, workspace_root));
                    let _ = cache.put_build_hash(&path_str, &current_hash);
                }
            }
            Ok(None) => {
                new.push(build_file.clone());
                affected.push(compute_build_file_package_label(build_file, workspace_root));
                let _ = cache.put_build_hash(&path_str, &current_hash);
            }
            Err(e) => {
                log::warn!("Failed to check hash for {}: {}", path_str, e);
            }
        }
    }

    Ok(WorkspaceChangeScan {
        changed_files: changed,
        new_files: new,
        deleted_files: deleted,
        affected_labels: affected,
    })
}

pub fn collect_build_files(
    root: &std::path::Path,
) -> Result<Vec<std::path::PathBuf>, std::io::Error> {
    let mut files = Vec::new();
    collect_build_files_recursive(root, &mut files)?;
    Ok(files)
}

fn collect_build_files_recursive(
    dir: &std::path::Path,
    files: &mut Vec<std::path::PathBuf>,
) -> Result<(), std::io::Error> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.')
                || name == "bazel-out"
                || name == "bazel-bin"
                || name == "bazel-testlogs"
            {
                continue;
            }
            collect_build_files_recursive(&path, files)?;
        } else if crate::watcher::is_build_file(&path) {
            files.push(path);
        }
    }
    Ok(())
}

pub fn detect_git_branch_changes(
    workspace_root: &std::path::Path,
    old_ref: &str,
    new_ref: &str,
) -> Result<Vec<std::path::PathBuf>, Box<dyn std::error::Error>> {
    let output = std::process::Command::new("git")
        .current_dir(workspace_root)
        .args(["diff", "--name-only", &format!("{}..{}", old_ref, new_ref)])
        .output()?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let changed: Vec<std::path::PathBuf> = stdout
        .lines()
        .map(|l| l.trim())
        .filter(|l| {
            let path = std::path::Path::new(l);
            path.file_name()
                .and_then(|n| n.to_str())
                .map(|n| matches!(n, "BUILD" | "BUILD.bazel" | "WORKSPACE" | "WORKSPACE.bazel"))
                .unwrap_or(false)
        })
        .map(|l| workspace_root.join(l))
        .collect();

    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_field_change_is_classpath_relevant() {
        assert!(FieldChange::DepsChanged.is_classpath_relevant());
        assert!(FieldChange::SrcsChanged.is_classpath_relevant());
        assert!(FieldChange::RuntimeDepsChanged.is_classpath_relevant());
        assert!(FieldChange::ExportsChanged.is_classpath_relevant());
        assert!(FieldChange::PluginsChanged.is_classpath_relevant());

        assert!(!FieldChange::VisibilityChanged.is_classpath_relevant());
        assert!(!FieldChange::TestOnlyChanged.is_classpath_relevant());
        assert!(!FieldChange::ResourcesChanged.is_classpath_relevant());
    }

    #[test]
    fn test_detect_added_removed_files() {
        let before = vec![
            PathBuf::from("/workspace/BUILD"),
            PathBuf::from("/workspace/foo/BUILD"),
        ];
        let after = vec![
            PathBuf::from("/workspace/BUILD"),
            PathBuf::from("/workspace/bar/BUILD"),
        ];

        let (added, removed) = detect_added_removed_files(&before, &after);

        assert_eq!(added, vec![PathBuf::from("/workspace/bar/BUILD")]);
        assert_eq!(removed, vec![PathBuf::from("/workspace/foo/BUILD")]);
    }

    #[test]
    fn test_compute_build_file_package_label() {
        let build_path = PathBuf::from("/workspace/foo/bar/BUILD");
        let workspace = PathBuf::from("/workspace");

        assert_eq!(
            compute_build_file_package_label(&build_path, &workspace),
            "//foo/bar"
        );
    }

    #[test]
    fn test_compute_build_file_package_label_root() {
        let build_path = PathBuf::from("/workspace/BUILD");
        let workspace = PathBuf::from("/workspace");

        assert_eq!(
            compute_build_file_package_label(&build_path, &workspace),
            "//"
        );
    }
}
