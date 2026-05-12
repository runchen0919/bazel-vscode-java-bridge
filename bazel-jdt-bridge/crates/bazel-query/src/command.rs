use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

static SYNC_CMD_COUNTER: AtomicU64 = AtomicU64::new(0);

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

    /// Synchronous target discovery — no tokio runtime needed.
    /// Safe to call from JNI threads where tokio's reactor may conflict with the host process.
    pub fn discover_java_targets_sync(
        &self,
        scope_patterns: Option<&[String]>,
        build_flags: Option<&[String]>,
    ) -> Result<Vec<String>, BazelError> {
        let query = build_java_target_query(scope_patterns);

        let mut args = vec!["query".to_string()];
        if let Some(flags) = build_flags {
            args.extend(flags.iter().map(|s| s.to_string()));
        }
        args.push("--output=label".to_string());
        args.push(query);

        let output = run_bazel_command_sync(&self.bazel_path, &self.workspace_root, &args)?;

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

    /// Discover Java targets. `None` or empty scope = all targets.
    /// Patterns starting with `-` are excluded via `except`.
    pub async fn discover_java_targets(
        &self,
        scope_patterns: Option<&[String]>,
        build_flags: Option<&[String]>,
    ) -> Result<Vec<String>, BazelError> {
        let query = build_java_target_query(scope_patterns);

        let bazel_path = self.bazel_path.clone();
        let workspace_root = self.workspace_root.clone();
        let mut args = vec!["query".to_string()];
        if let Some(flags) = build_flags {
            args.extend(flags.iter().map(|s| s.to_string()));
        }
        args.push("--output=label".to_string());
        args.push(query);

        let output = run_bazel_command(bazel_path, workspace_root, args).await?;

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

    /// Synchronous aspect build — uses `system()` to avoid JVM fd issues.
    pub fn build_with_aspects_sync(
        &self,
        targets: &[String],
        aspect_file: &str,
        build_flags: Option<&[String]>,
    ) -> Result<String, BazelError> {
        let mut args = vec!["build".to_string()];
        if let Some(flags) = build_flags {
            args.extend(flags.iter().map(|s| s.to_string()));
        }
        args.push(format!("--aspects={}", aspect_file));
        args.push("--output_groups=intellij-info-java,intellij-info-generic".to_string());
        args.push("--show_result=2147483647".to_string());
        args.extend(targets.iter().cloned());

        let output = run_bazel_command_sync(&self.bazel_path, &self.workspace_root, &args)?;

        let stderr = String::from_utf8(output.stderr)?;

        if !output.status.success() {
            return Err(BazelError::CommandFailed {
                message: format!("bazel build with aspects failed: {}", stderr),
            });
        }

        Ok(stderr)
    }

    /// Synchronous full classpath resolution via aspect build.
    pub fn resolve_full_classpath_sync(
        &self,
        targets: &[String],
        build_flags: Option<&[String]>,
    ) -> Result<Vec<TargetIdeInfo>, BazelError> {
        if targets.is_empty() {
            return Ok(Vec::new());
        }

        let aspect_output =
            self.build_with_aspects_sync(targets, &self.aspect_label, build_flags)?;

        log::info!("Discovering aspect output files...");
        let mut info_files =
            crate::output::parse_aspect_output_locations(&aspect_output);

        if info_files.is_empty() {
            log::warn!(
                "Stderr parsing found 0 aspect outputs — \
                 falling back to filesystem scan of bazel-bin/. \
                 This may be slow in large workspaces."
            );
            info_files = crate::output::discover_aspect_outputs(&self.workspace_root);
            log::info!(
                "Filesystem scan discovered {} aspect output files",
                info_files.len()
            );
        } else {
            log::info!(
                "Found {} aspect output files via stderr parsing",
                info_files.len()
            );
        }

        let total = info_files.len();
        let log_interval = (total / 10).max(100);
        let mut results = Vec::new();
        for (i, info_path) in info_files.iter().enumerate() {
            let normalized_path = normalize_path_separators(info_path);
            let absolute_path = self.workspace_root.join(&normalized_path);
            match std::fs::read_to_string(&absolute_path) {
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
            if (i + 1) % log_interval == 0 {
                log::info!("Parsed {}/{} aspect files...", i + 1, total);
            }
        }
        log::info!("All {} aspect files parsed ({} read failures skipped)",
            results.len(), total - results.len());

        Ok(results)
    }

    /// Build targets with IntelliJ aspects to get dependency info.
    /// Bazel writes aspect output file paths to stderr, not stdout.
    pub async fn build_with_aspects(
        &self,
        targets: &[String],
        aspect_file: &str,
        build_flags: Option<&[String]>,
    ) -> Result<String, BazelError> {
        let bazel_path = self.bazel_path.clone();
        let workspace_root = self.workspace_root.clone();
        let mut args = vec!["build".to_string()];
        if let Some(flags) = build_flags {
            args.extend(flags.iter().map(|s| s.to_string()));
        }
        args.push(format!("--aspects={}", aspect_file));
        args.push("--output_groups=intellij-info-java,intellij-info-generic".to_string());
        args.push("--show_result=2147483647".to_string());
        args.extend(targets.iter().cloned());

        let output = run_bazel_command(bazel_path, workspace_root, args).await?;

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

/// Resolve a binary name to its absolute path by searching PATH.
fn resolve_binary(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.is_absolute() {
        return if path.exists() {
            Some(path.to_path_buf())
        } else {
            None
        };
    }
    let path_var = std::env::var("PATH").ok()?;
    let sep = if cfg!(target_os = "windows") {
        ';'
    } else {
        ':'
    };
    for dir in path_var.split(sep) {
        let candidate = Path::new(dir).join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Run a bazel command synchronously using C `system()` to bypass Rust's `Command` entirely.
///
/// The JVM process has 11,000+ open file descriptors. Rust's `Command` (both posix_spawn
/// and fork+exec paths) creates internal error pipes and manipulates fds, which triggers
/// EBADF in this environment. C `system()` does a bare `fork()+exec("/bin/sh")` without
/// any fd manipulation or error pipes, sidestepping the issue completely.
/// Output is captured via shell-level redirection to temp files.
fn run_bazel_command_sync(
    bazel_path: &str,
    workspace_root: &Path,
    args: &[String],
) -> Result<std::process::Output, BazelError> {
    let resolved = resolve_binary(bazel_path);
    let binary = match &resolved {
        Some(p) => p.as_path(),
        None => Path::new(bazel_path),
    };

    log::info!(
        "Spawning via system(): {:?} (resolved from {:?}), cwd={:?}",
        binary,
        bazel_path,
        workspace_root
    );

    let tmp_dir = std::env::temp_dir();
    let id = SYNC_CMD_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let stdout_path = tmp_dir.join(format!("bazel-jdt-stdout-{}-{}.tmp", pid, id));
    let stderr_path = tmp_dir.join(format!("bazel-jdt-stderr-{}-{}.tmp", pid, id));

    let shell_cmd = format!(
        "cd {} && {} {} > {} 2> {}",
        shell_escape(workspace_root),
        shell_escape(binary),
        args.iter()
            .map(|a| shell_escape_str(a))
            .collect::<Vec<_>>()
            .join(" "),
        shell_escape(&stdout_path),
        shell_escape(&stderr_path),
    );

    log::debug!("system() cmd: {}", shell_cmd);

    extern "C" {
        fn system(command: *const std::os::raw::c_char) -> std::os::raw::c_int;
    }

    let c_cmd = std::ffi::CString::new(shell_cmd).map_err(|e| BazelError::CommandFailed {
        message: format!("CString conversion error: {}", e),
    })?;

    let ret = unsafe { system(c_cmd.as_ptr()) };
    log::debug!("system() returned raw status: {}", ret);

    if ret == -1 {
        let err = std::io::Error::last_os_error();
        log::error!("system() fork failed: {}", err);
        let _ = std::fs::remove_file(&stdout_path);
        let _ = std::fs::remove_file(&stderr_path);
        return Err(BazelError::IoError(err));
    }

    let stdout = std::fs::read(&stdout_path).unwrap_or_default();
    let stderr = std::fs::read(&stderr_path).unwrap_or_default();
    let _ = std::fs::remove_file(&stdout_path);
    let _ = std::fs::remove_file(&stderr_path);

    #[cfg(unix)]
    let status = std::process::ExitStatus::from_raw(ret);
    #[cfg(not(unix))]
    let status = {
        // Fallback: can't construct ExitStatus on non-Unix
        return Err(BazelError::CommandFailed {
            message: "system() not supported on this platform".to_string(),
        });
    };

    log::info!(
        "system() completed, status={}, stdout_len={}, stderr_len={}",
        status,
        stdout.len(),
        stderr.len()
    );

    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

fn shell_escape(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "'\\''"))
}

fn shell_escape_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Run a bazel/bazelisk command synchronously on a blocking thread.
async fn run_bazel_command(
    bazel_path: String,
    workspace_root: PathBuf,
    args: Vec<String>,
) -> Result<std::process::Output, BazelError> {
    tokio::task::spawn_blocking(move || {
        let resolved = resolve_binary(&bazel_path);
        let binary = match &resolved {
            Some(p) => p.as_path(),
            None => Path::new(&bazel_path),
        };

        log::info!(
            "[bazel-jdt] Spawning: {:?} (resolved from {:?}), cwd={:?}, args={:?}",
            binary,
            bazel_path,
            workspace_root,
            args
        );

        // Try 1: direct spawn
        match Command::new(binary)
            .current_dir(&workspace_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .args(&args)
            .output()
        {
            Ok(output) => Ok(output),
            Err(e) => {
                log::error!(
                    "[bazel-jdt] Direct spawn failed: {} (os_error={:?}). \
                     binary={:?}, cwd={:?}, cwd_exists={}, PATH={:?}",
                    e,
                    e.raw_os_error(),
                    binary,
                    workspace_root,
                    workspace_root.exists(),
                    std::env::var("PATH").unwrap_or_default()
                );

                // Try 2: spawn via /bin/sh as fallback
                log::info!("[bazel-jdt] Trying /bin/sh fallback...");
                let shell_cmd = std::iter::once(shell_escape_str(&binary.to_string_lossy()))
                    .chain(args.iter().map(|a| shell_escape_str(a)))
                    .collect::<Vec<_>>()
                    .join(" ");

                match Command::new("/bin/sh")
                    .current_dir(&workspace_root)
                    .args(["-c", &shell_cmd])
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .output()
                {
                    Ok(output) => {
                        log::info!("[bazel-jdt] /bin/sh fallback succeeded");
                        Ok(output)
                    }
                    Err(e2) => {
                        log::error!(
                            "[bazel-jdt] /bin/sh fallback also failed: {} (os_error={:?})",
                            e2,
                            e2.raw_os_error()
                        );
                        Err(e)
                    }
                }
            }
        }
    })
    .await
    .map_err(|e| {
        BazelError::IoError(std::io::Error::other(format!(
            "blocking task failed: {}",
            e
        )))
    })?
    .map_err(BazelError::IoError)
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
