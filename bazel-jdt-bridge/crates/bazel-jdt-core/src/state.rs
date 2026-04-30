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

        let aspect_label = match crate::aspect::extract_if_needed(&workspace_root) {
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
}
