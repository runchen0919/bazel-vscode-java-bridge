use bazel_cache::BazelCache;
use bazel_graph::DependencyGraph;
use bazel_parser::BuildFileParser;
use bazel_query::BazelInvoker;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Mutex;
use tokio::runtime::Runtime;

use crate::watcher::BuildFileWatcher;

/// Synchronization state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncState {
    Idle = 0,
    Syncing = 1,
    Error = 2,
}

/// Central state for the Bazel JDT bridge
pub struct BazelJdtState {
    pub cache: BazelCache,
    pub graph: DependencyGraph,
    pub parser: BuildFileParser,
    pub invoker: BazelInvoker,
    pub runtime: Runtime,
    pub workspace_root: PathBuf,
    pub sync_state: AtomicI32,
    pub watcher: Mutex<Option<BuildFileWatcher>>,
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
            graph,
            parser,
            invoker,
            runtime,
            workspace_root,
            sync_state: AtomicI32::new(SyncState::Idle as i32),
            watcher: Mutex::new(None),
        })
    }

    pub fn get_sync_state(&self) -> SyncState {
        match self.sync_state.load(Ordering::SeqCst) {
            0 => SyncState::Idle,
            1 => SyncState::Syncing,
            _ => SyncState::Error,
        }
    }

    pub fn set_sync_state(&self, state: SyncState) {
        self.sync_state.store(state as i32, Ordering::SeqCst);
    }
}
