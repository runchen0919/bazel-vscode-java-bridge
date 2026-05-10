use std::fs;
use std::path::Path;
use std::process::Command;

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
const ASPECT_CC_INFO: &str = include_str!("aspects/cc_info.bzl");
const ASPECT_CODE_GENERATOR_INFO: &str = include_str!("aspects/code_generator_info.bzl");
const ASPECT_PYTHON_INFO: &str = include_str!("aspects/python_info.bzl");

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
        ("cc_info.bzl", ASPECT_CC_INFO),
        ("code_generator_info.bzl", ASPECT_CODE_GENERATOR_INFO),
        ("python_info.bzl", ASPECT_PYTHON_INFO),
    ]
}

/// Parse the major version number from `bazel --version` output.
/// Returns the major version, or `None` if parsing fails.
pub fn parse_bazel_major_version(version_output: &str) -> Option<u32> {
    let version_str = version_output.trim();
    let version_part = version_str.strip_prefix("bazel ").unwrap_or(version_str);
    version_part.split('.').next()?.parse::<u32>().ok()
}

/// Detect the major version of the Bazel binary.
/// Returns the major version number, defaulting to 8 if detection fails.
pub fn detect_bazel_major_version(bazel_path: &str) -> u32 {
    match Command::new(bazel_path).arg("--version").output() {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            match parse_bazel_major_version(&stdout) {
                Some(v) => v,
                None => {
                    log::warn!(
                        "Could not parse Bazel version from '{}', defaulting to 8",
                        stdout.trim()
                    );
                    8
                }
            }
        }
        Ok(output) => {
            log::warn!(
                "bazel --version exited with {}, defaulting to version 8",
                output.status
            );
            8
        }
        Err(e) => {
            log::warn!(
                "Failed to run '{} --version': {}, defaulting to version 8",
                bazel_path,
                e
            );
            8
        }
    }
}

/// Strip `toolchains_aspects` lines for Bazel < 9 (attribute unsupported before Bazel 9).
/// Safe to use naive line filtering because the content is embedded at compile time
/// via `include_str!` — we control the exact format.
fn adapt_bundled_bzl_for_version(content: &str, bazel_major: u32) -> String {
    if bazel_major >= 9 {
        return content.to_string();
    }
    content
        .lines()
        .filter(|line| !line.contains("toolchains_aspects"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Compute a SHA-256 hash of all embedded aspect file contents,
/// incorporating the Bazel major version so that version changes trigger re-extraction.
pub fn version_hash(bazel_major: u32) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bazel_major.to_le_bytes());
    for (name, content) in aspect_files() {
        hasher.update(name.as_bytes());
        hasher.update(content.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// Extract aspect files to the workspace if the embedded version differs from
/// what's already on disk. Returns the workspace-relative Bazel aspect label.
pub fn extract_if_needed(workspace_root: &Path, bazel_path: &str) -> Result<String, AspectError> {
    let bazel_major = detect_bazel_major_version(bazel_path);
    let aspect_dir = workspace_root.join(ASPECT_DIR_NAME);
    let version_path = aspect_dir.join(VERSION_FILE);
    let current_hash = version_hash(bazel_major);

    let needs_extraction = !matches!(
        fs::read_to_string(&version_path),
        Ok(stored_hash) if stored_hash == current_hash
    );

    if needs_extraction {
        fs::create_dir_all(&aspect_dir)?;

        for (name, content) in aspect_files() {
            let final_content = if name == "intellij_info_bundled.bzl" {
                adapt_bundled_bzl_for_version(content, bazel_major)
            } else {
                content.to_string()
            };
            fs::write(aspect_dir.join(name), final_content)?;
        }

        fs::write(&version_path, &current_hash)?;

        log::info!(
            "Extracted aspect files to {} (version: {}, bazel: {})",
            aspect_dir.display(),
            &current_hash[..8],
            bazel_major
        );
    }

    let label = format!(
        "//{}:intellij_info_bundled.bzl%intellij_info_aspect",
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
        let h1 = version_hash(8);
        let h2 = version_hash(8);
        assert_eq!(h1, h2, "version hash must be deterministic");
        assert!(!h1.is_empty());
        assert_eq!(h1.len(), 64, "SHA-256 hex should be 64 chars");
    }

    #[test]
    fn test_version_hash_differs_by_bazel_version() {
        let h8 = version_hash(8);
        let h9 = version_hash(9);
        assert_ne!(h8, h9, "version hash must differ between Bazel 8 and 9");
    }

    #[test]
    fn test_extract_creates_files() {
        let tmp = tempfile::tempdir().unwrap();
        let label = extract_if_needed(tmp.path(), "bazel").unwrap();

        assert!(label.contains("intellij_info_bundled.bzl%intellij_info_aspect"));
        assert!(label.starts_with("//.bazel-jdt/aspects:intellij_info_bundled.bzl"));

        let aspect_dir = tmp.path().join(ASPECT_DIR_NAME);
        assert!(aspect_dir.join("BUILD").exists());
        assert!(aspect_dir.join("intellij_info_bundled.bzl").exists());
        assert!(aspect_dir.join("intellij_info_impl_bundled.bzl").exists());
        assert!(aspect_dir.join("artifacts.bzl").exists());
        assert!(aspect_dir.join("make_variables.bzl").exists());
        assert!(aspect_dir.join("cc_info.bzl").exists());
        assert!(aspect_dir.join("code_generator_info.bzl").exists());
        assert!(aspect_dir.join("python_info.bzl").exists());
        assert!(aspect_dir.join(".version").exists());

        let build_content = fs::read_to_string(aspect_dir.join("BUILD")).unwrap();
        assert!(build_content.is_empty());
    }

    #[test]
    fn test_extract_skips_when_version_matches() {
        let tmp = tempfile::tempdir().unwrap();
        extract_if_needed(tmp.path(), "bazel").unwrap();

        let version_path = tmp.path().join(ASPECT_DIR_NAME).join(VERSION_FILE);
        std::thread::sleep(std::time::Duration::from_millis(10));

        extract_if_needed(tmp.path(), "bazel").unwrap();

        let stored = fs::read_to_string(&version_path).unwrap();
        assert_eq!(stored, version_hash(detect_bazel_major_version("bazel")));
    }

    #[test]
    fn test_extract_updates_when_version_differs() {
        let tmp = tempfile::tempdir().unwrap();
        let aspect_dir = tmp.path().join(ASPECT_DIR_NAME);

        extract_if_needed(tmp.path(), "bazel").unwrap();

        let version_path = aspect_dir.join(VERSION_FILE);
        fs::write(&version_path, "invalid-hash").unwrap();

        extract_if_needed(tmp.path(), "bazel").unwrap();

        let stored = fs::read_to_string(&version_path).unwrap();
        assert_eq!(stored, version_hash(detect_bazel_major_version("bazel")));
    }

    #[test]
    fn test_parse_bazel_major_version_standard() {
        assert_eq!(parse_bazel_major_version("bazel 7.4.1"), Some(7));
        assert_eq!(parse_bazel_major_version("bazel 8.0.0"), Some(8));
        assert_eq!(parse_bazel_major_version("bazel 9.1.0"), Some(9));
    }

    #[test]
    fn test_parse_bazel_major_version_with_whitespace() {
        assert_eq!(parse_bazel_major_version("bazel 8.0.0\n"), Some(8));
        assert_eq!(parse_bazel_major_version("  bazel 9.0.0  "), Some(9));
    }

    #[test]
    fn test_parse_bazel_major_version_invalid() {
        assert_eq!(parse_bazel_major_version(""), None);
        assert_eq!(parse_bazel_major_version("not-bazel"), None);
        assert_eq!(parse_bazel_major_version("bazel "), None);
    }

    #[test]
    fn test_detect_bazel_major_version_invalid_binary() {
        assert_eq!(detect_bazel_major_version("/nonexistent/bazel"), 8);
    }

    #[test]
    fn test_adapt_bundled_bzl_bazel8_strips_toolchains_aspects() {
        let content = "line1\n    toolchains_aspects = TOOLCHAIN_TYPE_DEPS,\nline3\n";
        let result = adapt_bundled_bzl_for_version(content, 8);
        assert!(!result.contains("toolchains_aspects"));
        assert!(result.contains("line1"));
        assert!(result.contains("line3"));
    }

    #[test]
    fn test_adapt_bundled_bzl_bazel9_keeps_toolchains_aspects() {
        let content = "line1\n    toolchains_aspects = TOOLCHAIN_TYPE_DEPS,\nline3\n";
        let result = adapt_bundled_bzl_for_version(content, 9);
        assert!(result.contains("toolchains_aspects"));
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
