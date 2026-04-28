use bazel_cache::BazelCache;
use bazel_graph::DependencyGraph;
use bazel_parser::BuildFileParser;
use bazel_query::BazelInvoker;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::Mutex;
use tokio::runtime::Runtime;

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
    pub sync_state: AtomicI32,
    pub watcher: Mutex<Option<BuildFileWatcher>>,
    pub generation: AtomicU32,
    pub shutdown_flag: AtomicBool,
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
        let invoker = BazelInvoker::new(bazel_path, &workspace_root);
        let runtime = Runtime::new()?;

        Ok(Self {
            cache,
            graph: Mutex::new(graph),
            parser,
            invoker,
            runtime,
            workspace_root,
            sync_state: AtomicI32::new(SyncState::Idle as i32),
            watcher: Mutex::new(None),
            generation: AtomicU32::new(0),
            shutdown_flag: AtomicBool::new(false),
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

    pub fn current_generation(&self) -> u32 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn next_generation(&self) -> u32 {
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    pub fn set_sync_state(&self, state: SyncState) {
        self.sync_state.store(state as i32, Ordering::SeqCst);
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
        graph.populate_from_parsed_batch(&parsed);
        Ok(count)
    }
}
