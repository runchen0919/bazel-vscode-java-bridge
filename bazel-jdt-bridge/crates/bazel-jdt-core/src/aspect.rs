use std::fs;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum AspectError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("UTF-8 error: {0}")]
    Utf8Error(#[from] std::string::FromUtf8Error),
}

const ASPECT_DIR_NAME: &str = ".bazel-jdt/aspects";
const VERSION_FILE: &str = ".version";

const ASPECT_BUILD: &str = "";
const ASPECT_INTELLIJ_INFO_BUNDLED: &str = include_str!("aspects/intellij_info_bundled.bzl");
const ASPECT_INTELLIJ_INFO_IMPL_BUNDLED: &str =
    include_str!("aspects/intellij_info_impl_bundled.bzl");
const ASPECT_ARTIFACTS: &str = include_str!("aspects/artifacts.bzl");
const ASPECT_MAKE_VARIABLES: &str = include_str!("aspects/make_variables.bzl");

fn aspect_files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("BUILD", ASPECT_BUILD),
        ("intellij_info_bundled.bzl", ASPECT_INTELLIJ_INFO_BUNDLED),
        (
            "intellij_info_impl_bundled.bzl",
            ASPECT_INTELLIJ_INFO_IMPL_BUNDLED,
        ),
        ("artifacts.bzl", ASPECT_ARTIFACTS),
        ("make_variables.bzl", ASPECT_MAKE_VARIABLES),
    ]
}

/// Compute a SHA-256 hash of all embedded aspect file contents.
/// Used to detect when the bundled aspect version has changed.
pub fn version_hash() -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for (name, content) in aspect_files() {
        hasher.update(name.as_bytes());
        hasher.update(content.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// Extract aspect files to the workspace if the embedded version differs from
/// what's already on disk. Returns the workspace-relative Bazel aspect label.
pub fn extract_if_needed(workspace_root: &Path) -> Result<String, AspectError> {
    let aspect_dir = workspace_root.join(ASPECT_DIR_NAME);
    let version_path = aspect_dir.join(VERSION_FILE);
    let current_hash = version_hash();

    let needs_extraction = !matches!(
        fs::read_to_string(&version_path),
        Ok(stored_hash) if stored_hash == current_hash
    );

    if needs_extraction {
        fs::create_dir_all(&aspect_dir)?;

        for (name, content) in aspect_files() {
            fs::write(aspect_dir.join(name), content)?;
        }

        fs::write(&version_path, &current_hash)?;

        log::info!(
            "Extracted aspect files to {} (version: {})",
            aspect_dir.display(),
            &current_hash[..8]
        );
    }

    let label = format!(
        "//{}/intellij_info_bundled.bzl%intellij_info_aspect",
        ASPECT_DIR_NAME
    );
    Ok(label)
}

/// Check if `.bazelignore` contains patterns that would exclude the aspect directory.
/// Returns `Some(warning_message)` if a conflict is detected.
pub fn check_bazelignore(workspace_root: &Path) -> Option<String> {
    let bazelignore_path = workspace_root.join(".bazelignore");
    let contents = fs::read_to_string(&bazelignore_path).ok()?;

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == ".bazel-jdt" || line == ".bazel-jdt/" || line == ".bazel-jdt/**" || line == "/" {
            return Some(format!(
                "Aspect directory '{}' is covered by .bazelignore \
                 (pattern: '{}') — aspects may not be found by Bazel. \
                 Please remove this pattern from .bazelignore.",
                ASPECT_DIR_NAME, line
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_version_hash_deterministic() {
        let h1 = version_hash();
        let h2 = version_hash();
        assert_eq!(h1, h2, "version hash must be deterministic");
        assert!(!h1.is_empty());
        assert_eq!(h1.len(), 64, "SHA-256 hex should be 64 chars");
    }

    #[test]
    fn test_extract_creates_files() {
        let tmp = tempfile::tempdir().unwrap();
        let label = extract_if_needed(tmp.path()).unwrap();

        assert!(label.contains("intellij_info_bundled.bzl%intellij_info_aspect"));
        assert!(label.starts_with("//.bazel-jdt/aspects"));

        let aspect_dir = tmp.path().join(ASPECT_DIR_NAME);
        assert!(aspect_dir.join("BUILD").exists());
        assert!(aspect_dir.join("intellij_info_bundled.bzl").exists());
        assert!(aspect_dir.join("intellij_info_impl_bundled.bzl").exists());
        assert!(aspect_dir.join("artifacts.bzl").exists());
        assert!(aspect_dir.join("make_variables.bzl").exists());
        assert!(aspect_dir.join(".version").exists());

        let build_content = fs::read_to_string(aspect_dir.join("BUILD")).unwrap();
        assert!(build_content.is_empty());
    }

    #[test]
    fn test_extract_skips_when_version_matches() {
        let tmp = tempfile::tempdir().unwrap();
        extract_if_needed(tmp.path()).unwrap();

        let version_path = tmp.path().join(ASPECT_DIR_NAME).join(VERSION_FILE);
        std::thread::sleep(std::time::Duration::from_millis(10));

        extract_if_needed(tmp.path()).unwrap();

        let stored = fs::read_to_string(&version_path).unwrap();
        assert_eq!(stored, version_hash());
    }

    #[test]
    fn test_extract_updates_when_version_differs() {
        let tmp = tempfile::tempdir().unwrap();
        let aspect_dir = tmp.path().join(ASPECT_DIR_NAME);

        extract_if_needed(tmp.path()).unwrap();

        let version_path = aspect_dir.join(VERSION_FILE);
        fs::write(&version_path, "invalid-hash").unwrap();

        extract_if_needed(tmp.path()).unwrap();

        let stored = fs::read_to_string(&version_path).unwrap();
        assert_eq!(stored, version_hash());
    }

    #[test]
    fn test_bazelignore_detects_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join(".bazelignore"),
            "node_modules\n.bazel-jdt\n",
        )
        .unwrap();

        let result = check_bazelignore(tmp.path());
        assert!(result.is_some());
        assert!(result.unwrap().contains(".bazel-jdt"));
    }

    #[test]
    fn test_bazelignore_no_conflict_when_no_file() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(check_bazelignore(tmp.path()).is_none());
    }

    #[test]
    fn test_bazelignore_no_conflict_when_no_match() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(".bazelignore"), "node_modules\nbazel-bin\n").unwrap();
        assert!(check_bazelignore(tmp.path()).is_none());
    }
}
