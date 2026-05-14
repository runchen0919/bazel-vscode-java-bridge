use bazel_cache::BazelCache;
use bazel_graph::DependencyGraph;
use bazel_parser::BuildFileParser;
use bazel_query::BazelInvoker;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::watch;

use crate::watcher::BuildFileWatcher;

/// Synchronization state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncState {
    Idle = 0,
    Syncing = 1,
    Error = 2,
    Dead = 3,
}

/// Central state for the Bazel JDT bridge
pub struct BazelJdtState {
    pub cache: BazelCache,
    pub graph: Mutex<DependencyGraph>,
    pub parser: BuildFileParser,
    pub invoker: BazelInvoker,
    pub runtime: Runtime,
    pub workspace_root: PathBuf,
    pub aspect_label: String,
    pub sync_state: AtomicI32,
    pub aspect_total_files: AtomicI32,
    pub aspect_files_with_jars: AtomicI32,
    pub watcher: Mutex<Option<BuildFileWatcher>>,
    pub watcher_join_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
    pub shutdown_flag: AtomicBool,
    pub pending_changes: Mutex<Vec<String>>,
    /// Timeout for `bazel query` operations (default: 120s)
    pub query_timeout: Duration,
    /// Timeout for `bazel build --aspects` operations (default: 300s)
    pub aspect_timeout: Duration,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
}

impl BazelJdtState {
    pub fn new(
        workspace_root: PathBuf,
        bazel_path: &str,
        cache_dir: &std::path::Path,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let cache = BazelCache::open(cache_dir)?;
        let graph = DependencyGraph::new();
        let parser = BuildFileParser::new();

        let aspect_label = match crate::aspect::extract_if_needed(&workspace_root, bazel_path) {
            Ok(label) => label,
            Err(e) => {
                log::warn!(
                    "Failed to extract aspect files: {}. Falling back to @intellij_aspect.",
                    e
                );
                "@intellij_aspect//:intellij_info.bzl%intellij_info_java".to_string()
            }
        };

        if let Some(warning) = crate::aspect::check_bazelignore(&workspace_root) {
            log::warn!("{}", warning);
        }

        let invoker = BazelInvoker::new(bazel_path, &workspace_root, &aspect_label);
        let runtime = Runtime::new()?;
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        Ok(Self {
            cache,
            graph: Mutex::new(graph),
            parser,
            invoker,
            runtime,
            workspace_root,
            aspect_label,
            sync_state: AtomicI32::new(SyncState::Idle as i32),
            aspect_total_files: AtomicI32::new(0),
            aspect_files_with_jars: AtomicI32::new(0),
            watcher: Mutex::new(None),
            watcher_join_handle: Mutex::new(None),
            shutdown_flag: AtomicBool::new(false),
            pending_changes: Mutex::new(Vec::new()),
            query_timeout: Duration::from_secs(120),
            aspect_timeout: Duration::from_secs(300),
            shutdown_tx,
            shutdown_rx,
        })
    }

    pub fn get_sync_state(&self) -> SyncState {
        match self.sync_state.load(Ordering::SeqCst) {
            0 => SyncState::Idle,
            1 => SyncState::Syncing,
            2 => SyncState::Error,
            3 => SyncState::Dead,
            _ => SyncState::Error,
        }
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown_flag.load(Ordering::Acquire)
    }

    pub fn set_sync_state(&self, state: SyncState) {
        self.sync_state.store(state as i32, Ordering::SeqCst);
    }

    pub fn shutdown_signal(&self) -> watch::Receiver<bool> {
        self.shutdown_rx.clone()
    }

    pub fn signal_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    /// Parse all BUILD files in workspace and populate dependency graph.
    /// Returns the number of BUILD files parsed.
    pub fn populate_graph_from_build_files(&self) -> Result<usize, Box<dyn std::error::Error>> {
        let build_files = crate::change_detector::collect_build_files(&self.workspace_root)?;
        let mut parsed = Vec::new();
        for bf in &build_files {
            match self.parser.parse_file(bf) {
                Ok(pf) => parsed.push(pf),
                Err(e) => log::warn!("Failed to parse {}: {}", bf.display(), e),
            }
        }
        let count = parsed.len();
        let mut graph = self.graph.lock().unwrap_or_else(|e| e.into_inner());
        graph.populate_from_parsed_batch(&parsed, &self.workspace_root);
        Ok(count)
    }

    /// Incrementally sync changed BUILD files into the dependency graph.
    ///
    /// For each changed file: parse → surgical graph update → cascade invalidation
    /// → conditional aspect rebuild. Returns all invalidated target labels
    /// (directly affected plus reverse transitive dependers).
    pub fn sync_incremental(
        &self,
        changed_files: &[PathBuf],
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let mut all_invalidated: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for file_path in changed_files {
            let new_parsed = match self.parser.parse_file(file_path) {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("Failed to parse {}: {}", file_path.display(), e);
                    continue;
                }
            };

            let package_label = crate::change_detector::compute_build_file_package_label(
                file_path,
                &self.workspace_root,
            );

            if let Ok(hash) = crate::change_detector::compute_file_hash(file_path) {
                let path_str = file_path.to_string_lossy();
                let _ = self.cache.put_build_hash(&path_str, &hash);
            }

            let old_parsed = self.parse_cached_build_file(&package_label);

            let (added, removed, modified) = {
                let mut graph = self.graph.lock().unwrap_or_else(|e| e.into_inner());
                graph.update_from_parsed(&new_parsed, &self.workspace_root)
            };

            let directly_affected: Vec<String> = added
                .iter()
                .chain(removed.iter())
                .chain(modified.iter())
                .cloned()
                .collect();

            let cascaded: Vec<String> = {
                let graph = self.graph.lock().unwrap_or_else(|e| e.into_inner());
                let mut cascade_labels = Vec::new();
                for label in &directly_affected {
                    let deps = graph.reverse_transitive_deps(label);
                    cascade_labels.extend(deps);
                }
                cascade_labels
            };

            for label in directly_affected.iter().chain(cascaded.iter()) {
                all_invalidated.insert(label.clone());
            }

            let needs_aspect = self.should_run_aspect(&old_parsed, &new_parsed, &added);

            if needs_aspect {
                let labels_to_build: Vec<String> =
                    added.iter().chain(modified.iter()).cloned().collect();

                if !labels_to_build.is_empty() {
                    match self.run_aspect_build(&labels_to_build) {
                        Ok(results) => {
                            let mut graph = self.graph.lock().unwrap_or_else(|e| e.into_inner());
                            graph.populate_from_aspects(&results, &self.workspace_root);
                            log::info!(
                                "Aspect build for {} targets produced {} results",
                                labels_to_build.len(),
                                results.len()
                            );
                        }
                        Err(e) => {
                            log::warn!(
                                "Aspect build failed for {}: {}. Cache invalidated, will use slow path on next access.",
                                package_label,
                                e
                            );
                        }
                    }
                }
            }

            let to_invalidate: Vec<String> = all_invalidated.iter().cloned().collect();
            if !to_invalidate.is_empty() {
                if let Err(e) = self.cache.invalidate_targets(&to_invalidate) {
                    log::warn!("Failed to invalidate cache: {}", e);
                }
            }
        }

        let mut result: Vec<String> = all_invalidated.into_iter().collect();
        result.sort();
        Ok(result)
    }

    /// Parse the cached version of a BUILD file by reading existing graph data.
    /// Returns a synthetic ParsedBuildFile if the graph has targets for this package.
    fn parse_cached_build_file(
        &self,
        package_label: &str,
    ) -> Option<bazel_parser::model::ParsedBuildFile> {
        let graph = self.graph.lock().unwrap_or_else(|e| e.into_inner());
        let labels = graph.targets_in_package(package_label);
        if labels.is_empty() {
            return None;
        }

        let mut rules = Vec::new();
        for label in &labels {
            let name = label.rsplit(':').next().unwrap_or(label).to_string();
            rules.push(bazel_parser::model::JavaRule {
                rule_type: bazel_parser::model::RuleType::JavaLibrary,
                name,
                srcs: vec![],
                deps: vec![],
                runtime_deps: vec![],
                resources: vec![],
                plugins: vec![],
                exports: vec![],
                test_only: false,
                visibility: vec![],
            });
        }

        Some(bazel_parser::model::ParsedBuildFile {
            path: self.workspace_root.join(
                package_label
                    .trim_start_matches("//")
                    .replace('/', std::path::MAIN_SEPARATOR.to_string().as_str()),
            ),
            content_hash: String::new(),
            rules,
            loads: vec![],
        })
    }

    /// Determine whether an aspect build is needed.
    /// Aspect builds are expensive, so we only run them when srcs changed or targets are new.
    fn should_run_aspect(
        &self,
        old_parsed: &Option<bazel_parser::model::ParsedBuildFile>,
        new_parsed: &bazel_parser::model::ParsedBuildFile,
        added: &[String],
    ) -> bool {
        if !added.is_empty() {
            return true;
        }

        if let Some(old) = old_parsed {
            let change_result = crate::change_detector::detect_changes(old, new_parsed);
            return change_result.is_classpath_relevant;
        }

        true
    }

    /// Run aspect build for the given targets (sync path via system()).
    fn run_aspect_build(
        &self,
        targets: &[String],
    ) -> Result<Vec<bazel_aspect::TargetIdeInfo>, String> {
        self.invoker
            .resolve_full_classpath_sync(targets, None, false)
            .map_err(|e| format!("Aspect build failed: {}", e))
    }
}
