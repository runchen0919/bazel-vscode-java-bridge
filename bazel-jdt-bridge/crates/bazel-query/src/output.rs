/// Parse bazel query label output (one label per line)
pub fn parse_label_output(output: &str) -> Vec<String> {
    output
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && l.starts_with("//"))
        .collect()
}

/// Parse aspect output file locations from stderr.
/// Format: bazel-out/<config>/bin/<package>/<target>-<hash>.intellij-info.txt
/// Deduplicates paths since multiple targets share transitive deps.
pub fn parse_aspect_output_locations(output: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    output
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| l.ends_with(".intellij-info.txt"))
        .filter(|l| seen.insert(l.clone()))
        .collect()
}

/// Discover aspect output files by scanning bazel-bin/ on the filesystem.
///
/// This is the primary discovery method — it finds ALL `.intellij-info.txt` files
/// produced by the aspect build, not just the subset printed to stderr by
/// `--show_result`. Returns paths relative to `workspace_root`.
pub fn discover_aspect_outputs(workspace_root: &std::path::Path) -> Vec<String> {
    let bazel_bin = workspace_root.join("bazel-bin");
    if !bazel_bin.exists() {
        log::warn!(
            "bazel-bin directory not found at {:?}, cannot scan for aspect outputs",
            bazel_bin
        );
        return Vec::new();
    }

    let mut results = Vec::new();
    walk_for_intellij_info(&bazel_bin, workspace_root, &mut results);
    results
}

fn walk_for_intellij_info(
    dir: &std::path::Path,
    workspace_root: &std::path::Path,
    results: &mut Vec<String>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_for_intellij_info(&path, workspace_root, results);
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.ends_with(".intellij-info.txt") {
                if let Ok(rel) = path.strip_prefix(workspace_root) {
                    results.push(rel.to_string_lossy().into_owned());
                }
            }
        }
    }
}

/// Extract a dedup key from an aspect output path by stripping the
/// `bazel-bin/` or `bazel-out/<config>/bin/` prefix, leaving the
/// package-relative path (e.g. `src/java/com/example/lib-abc.intellij-info.txt`).
fn aspect_dedup_key(path: &str) -> &str {
    if let Some(rest) = path.strip_prefix("bazel-bin/") {
        return rest;
    }
    // bazel-out/<config>/bin/<rest>
    if path.starts_with("bazel-out/") {
        if let Some(idx) = path.find("/bin/") {
            return &path[idx + 5..];
        }
    }
    path
}

/// Merge stderr-parsed and filesystem-discovered aspect output paths,
/// deduplicating by package-relative path. Returns (merged, stderr_count, fs_new_count).
pub fn merge_aspect_outputs(
    stderr_paths: Vec<String>,
    fs_paths: Vec<String>,
) -> (Vec<String>, usize, usize) {
    let stderr_count = stderr_paths.len();
    let mut seen = std::collections::HashSet::new();
    let mut merged = Vec::new();

    for path in stderr_paths {
        let key = aspect_dedup_key(&path).to_string();
        if seen.insert(key) {
            merged.push(path);
        }
    }

    let mut fs_new = 0usize;
    for path in fs_paths {
        let key = aspect_dedup_key(&path).to_string();
        if seen.insert(key) {
            merged.push(path);
            fs_new += 1;
        }
    }

    (merged, stderr_count, fs_new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_label_output() {
        let output = "//java/com/example:lib\n//java/com/example:lib_test\n";
        let labels = parse_label_output(output);
        assert_eq!(
            labels,
            vec!["//java/com/example:lib", "//java/com/example:lib_test"]
        );
    }

    #[test]
    fn test_parse_label_output_empty() {
        let labels = parse_label_output("");
        assert!(labels.is_empty());
    }

    #[test]
    fn test_parse_aspect_output_locations() {
        let output = "bazel-out/k8-fastbuild/bin/java/com/example/lib-abc123.intellij-info.txt\n";
        let locations = parse_aspect_output_locations(output);
        assert_eq!(locations.len(), 1);
        assert!(locations[0].ends_with(".intellij-info.txt"));
    }

    #[test]
    fn test_discover_aspect_outputs_finds_nested_files() {
        let tmp = tempfile::tempdir().unwrap();
        let bazel_bin = tmp.path().join("bazel-bin");
        let nested = bazel_bin.join("java/com/example");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("lib-abc.intellij-info.txt"), "").unwrap();
        std::fs::write(nested.join("lib2-def.intellij-info.txt"), "").unwrap();
        std::fs::write(nested.join("other.txt"), "").unwrap();

        let results = discover_aspect_outputs(tmp.path());
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.ends_with(".intellij-info.txt")));
    }

    #[test]
    fn test_discover_aspect_outputs_missing_bazel_bin() {
        let tmp = tempfile::tempdir().unwrap();
        let results = discover_aspect_outputs(tmp.path());
        assert!(results.is_empty());
    }

    #[test]
    fn test_aspect_dedup_key_bazel_bin() {
        assert_eq!(
            aspect_dedup_key("bazel-bin/src/java/com/example/lib-abc.intellij-info.txt"),
            "src/java/com/example/lib-abc.intellij-info.txt"
        );
    }

    #[test]
    fn test_aspect_dedup_key_bazel_out() {
        assert_eq!(
            aspect_dedup_key("bazel-out/darwin_arm64-fastbuild/bin/src/java/com/example/lib-abc.intellij-info.txt"),
            "src/java/com/example/lib-abc.intellij-info.txt"
        );
    }

    #[test]
    fn test_merge_aspect_outputs_dedup() {
        let stderr = vec![
            "bazel-out/darwin_arm64-fastbuild/bin/src/java/lib-abc.intellij-info.txt".to_string(),
        ];
        let fs = vec![
            "bazel-bin/src/java/lib-abc.intellij-info.txt".to_string(),
            "bazel-bin/3rdparty/grpc-123.intellij-info.txt".to_string(),
        ];
        let (merged, stderr_count, fs_new) = merge_aspect_outputs(stderr, fs);
        assert_eq!(stderr_count, 1);
        assert_eq!(fs_new, 1);
        assert_eq!(merged.len(), 2);
        assert!(merged[0].starts_with("bazel-out/"));
        assert!(merged[1].contains("3rdparty"));
    }
}
