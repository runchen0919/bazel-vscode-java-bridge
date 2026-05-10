use notify::RecursiveMode;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum WatcherError {
    #[error("Watcher error: {0}")]
    NotifyError(#[from] notify::Error),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

pub type ChangeCallback = Box<dyn Fn(Vec<PathBuf>) + Send + 'static>;

pub struct BuildFileWatcher {
    debouncer: Option<
        notify_debouncer_full::Debouncer<
            notify::RecommendedWatcher,
            notify_debouncer_full::RecommendedCache,
        >,
    >,
    running: Arc<AtomicBool>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl BuildFileWatcher {
    pub fn start(
        workspace_root: PathBuf,
        watch_paths: Vec<PathBuf>,
        callback: ChangeCallback,
    ) -> Result<Self, WatcherError> {
        let (tx, rx) = std::sync::mpsc::channel::<notify_debouncer_full::DebounceEventResult>();

        let mut debouncer =
            notify_debouncer_full::new_debouncer(Duration::from_millis(500), None, tx)?;

        // Watch workspace root non-recursively for WORKSPACE and .bazelproject changes
        if let Err(e) = debouncer.watch(&workspace_root, RecursiveMode::NonRecursive) {
            if cfg!(target_os = "linux") {
                log::warn!(
                    "File watcher failed (may need to increase fs.inotify.max_user_watches): {}",
                    e
                );
            }
            return Err(WatcherError::NotifyError(e));
        }

        // Watch each user-selected directory recursively for BUILD file changes
        for rel_path in &watch_paths {
            let abs_path = workspace_root.join(rel_path);
            if !abs_path.is_dir() {
                log::warn!(
                    "Watch path does not exist, skipping: {}",
                    abs_path.display()
                );
                continue;
            }
            if let Err(e) = debouncer.watch(&abs_path, RecursiveMode::Recursive) {
                log::warn!("Failed to watch directory {}: {}", abs_path.display(), e);
            }
        }

        log::info!(
            "File watcher started: {} user directories",
            watch_paths.len()
        );

        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let handle = std::thread::Builder::new()
            .name("bazel-jdt-build-watcher".to_string())
            .spawn(move || {
                while running_clone.load(Ordering::Acquire) {
                    match rx.recv_timeout(Duration::from_millis(200)) {
                        Ok(result) => {
                            let paths = filter_build_file_events(result);
                            if !paths.is_empty() {
                                log::info!(
                                    "Build files changed: {:?}",
                                    paths.iter().map(|p| p.display()).collect::<Vec<_>>()
                                );
                                callback(paths);
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .map_err(WatcherError::IoError)?;

        Ok(Self {
            debouncer: Some(debouncer),
            running,
            thread_handle: Some(handle),
        })
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
        self.debouncer.take();
    }

    pub fn stop_nonblocking(&mut self) -> Option<std::thread::JoinHandle<()>> {
        self.running.store(false, Ordering::Release);
        self.debouncer.take();
        self.thread_handle.take()
    }
}

fn filter_build_file_events(result: notify_debouncer_full::DebounceEventResult) -> Vec<PathBuf> {
    let events = match result {
        Ok(events) => events,
        Err(errors) => {
            for err in &errors {
                log::warn!("File watcher error: {:?}", err);
            }
            return Vec::new();
        }
    };

    let mut paths = Vec::new();
    let mut seen = HashSet::new();

    for event in events {
        for path in &event.event.paths {
            if is_watched_file(path) && seen.insert(path.clone()) {
                log::debug!("Detected change in watched file: {}", path.display());
                paths.push(path.clone());
            }
        }
    }

    paths
}

pub fn is_build_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            matches!(
                name,
                "BUILD" | "BUILD.bazel" | "WORKSPACE" | "WORKSPACE.bazel"
            )
        })
        .unwrap_or(false)
}

pub fn is_watched_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            matches!(
                name,
                "BUILD" | "BUILD.bazel" | "WORKSPACE" | "WORKSPACE.bazel" | ".bazelproject"
            )
        })
        .unwrap_or(false)
}

pub fn is_bazelproject_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| name == ".bazelproject")
        .unwrap_or(false)
}

impl Drop for BuildFileWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_watched_file() {
        assert!(is_watched_file(Path::new("BUILD")));
        assert!(is_watched_file(Path::new("BUILD.bazel")));
        assert!(is_watched_file(Path::new("WORKSPACE")));
        assert!(is_watched_file(Path::new("WORKSPACE.bazel")));
        assert!(is_watched_file(Path::new("/some/path/BUILD")));
        assert!(is_watched_file(Path::new("/some/path/BUILD.bazel")));
        assert!(is_watched_file(Path::new(".bazelproject")));
        assert!(is_watched_file(Path::new("/some/path/.bazelproject")));
    }

    #[test]
    fn test_is_not_watched_file() {
        assert!(!is_watched_file(Path::new("build")));
        assert!(!is_watched_file(Path::new("workspace")));
        assert!(!is_watched_file(Path::new("README.md")));
        assert!(!is_watched_file(Path::new("Cargo.toml")));
        assert!(!is_watched_file(Path::new("src/main.rs")));
        assert!(!is_watched_file(Path::new("")));
    }

    #[test]
    fn test_is_bazelproject_file() {
        assert!(is_bazelproject_file(Path::new(".bazelproject")));
        assert!(is_bazelproject_file(Path::new("/workspace/.bazelproject")));
        assert!(!is_bazelproject_file(Path::new("BUILD")));
        assert!(!is_bazelproject_file(Path::new("WORKSPACE")));
        assert!(!is_bazelproject_file(Path::new("")));
    }
}
