use crate::state::{BazelJdtState, SyncState};
use crate::watcher::BuildFileWatcher;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use bazel_graph::TargetKind;
use jni::objects::{JClass, JObject, JObjectArray, JString};
use jni::sys::{jint, jlong, jobjectArray, jsize};
use jni::JNIEnv;

static REGISTRY: OnceLock<Mutex<HashMap<u64, Box<BazelJdtState>>>> = OnceLock::new();
static NEXT_KEY: AtomicU64 = AtomicU64::new(1);

fn registry() -> &'static Mutex<HashMap<u64, Box<BazelJdtState>>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_key() -> u64 {
    NEXT_KEY.fetch_add(1, Ordering::Relaxed)
}

fn get_state(env: &mut JNIEnv, handle: jlong) -> Option<&'static BazelJdtState> {
    if handle <= 0 {
        let _ = env.throw_new("java/lang/IllegalStateException", "Not initialized");
        return None;
    }
    let key = handle as u64;
    let reg = registry().lock().unwrap_or_else(|e| e.into_inner());
    match reg.get(&key) {
        Some(state) => {
            if state.is_shutdown() {
                let _ = env.throw_new(
                    "java/lang/IllegalStateException",
                    "Bridge has been shut down",
                );
                return None;
            }
            // SAFETY: The Box lives in the registry until nativeShutdown
            // removes it. We return a &'static ref that is valid as long as
            // the entry remains in the registry. JNI functions never hold
            // this reference across calls, so this is safe.
            let ptr: *const BazelJdtState = &**state;
            Some(unsafe { &*ptr })
        }
        None => {
            let _ = env.throw_new(
                "java/lang/IllegalStateException",
                "Invalid or expired handle",
            );
            None
        }
    }
}

fn create_string_array(
    env: &mut JNIEnv,
    strings: &[String],
) -> Result<jobjectArray, jni::errors::Error> {
    let array =
        env.new_object_array(strings.len() as jsize, "java/lang/String", JObject::null())?;
    for (i, s) in strings.iter().enumerate() {
        let java_str = env.new_string(s)?;
        env.set_object_array_element(&array, i as jsize, java_str)?;
    }
    Ok(array.into_raw())
}

fn parse_java_string_array(env: &mut JNIEnv, array: &JObjectArray) -> Option<Vec<String>> {
    let len = match env.get_array_length(array) {
        Ok(l) => l,
        Err(_) => return None,
    };
    if len == 0 {
        return None;
    }
    let mut result = Vec::with_capacity(len as usize);
    for i in 0..len {
        let s = env.get_object_array_element(array, i).ok().and_then(|obj| {
            let jstr = JString::from(obj);
            env.get_string(&jstr).ok().map(String::from)
        });
        if let Some(s) = s {
            result.push(s);
        } else {
            log::warn!(
                "Null or invalid string at index {} in scope_patterns array, skipping",
                i
            );
        }
    }
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

#[no_mangle]
pub extern "system" fn Java_com_bazel_jdt_BazelBridge_nativeInitialize(
    mut env: JNIEnv,
    _class: JClass,
    workspace_path: JString,
    bazel_path: JString,
    cache_dir: JString,
) -> jlong {
    let workspace: String = match env.get_string(&workspace_path) {
        Ok(s) => s.into(),
        Err(_) => {
            let _ = env.throw_new(
                "java/lang/IllegalArgumentException",
                "Invalid workspace path",
            );
            return -1;
        }
    };

    let bazel: String = match env.get_string(&bazel_path) {
        Ok(s) => s.into(),
        Err(_) => {
            let _ = env.throw_new("java/lang/IllegalArgumentException", "Invalid bazel path");
            return -1;
        }
    };

    let cache: String = match env.get_string(&cache_dir) {
        Ok(s) => s.into(),
        Err(_) => {
            let _ = env.throw_new("java/lang/IllegalArgumentException", "Invalid cache dir");
            return -1;
        }
    };

    let state = match BazelJdtState::new(
        std::path::PathBuf::from(&workspace),
        &bazel,
        std::path::Path::new(&cache),
    ) {
        Ok(s) => s,
        Err(e) => {
            let _ = env.throw_new(
                "java/lang/RuntimeException",
                format!("Initialization failed: {}", e),
            );
            return -1;
        }
    };

    match state.cache.load_all_classpaths() {
        Ok(cached) => {
            if !cached.is_empty() {
                log::info!("Loaded {} cached classpath entries", cached.len());
            }
        }
        Err(e) => {
            log::warn!("Failed to load cached classpaths: {}", e);
        }
    }

    let key = next_key();
    let workspace_root = state.workspace_root.clone();

    {
        let mut reg = registry().lock().unwrap_or_else(|e| e.into_inner());
        reg.insert(key, Box::new(state));
    }

    let watcher_cb: Box<dyn Fn(Vec<std::path::PathBuf>) + Send + 'static> = {
        let cb_key = key;
        Box::new(move |paths| {
            log::info!("Build files changed: {:?}", paths);
            let reg = registry().lock().unwrap_or_else(|e| e.into_inner());
            let state = match reg.get(&cb_key) {
                Some(s) => s,
                None => return,
            };
            if state.is_shutdown() {
                return;
            }

            let mut changes_to_record: Vec<(String, String)> = Vec::new();
            let mut packages_to_add: Vec<String> = Vec::new();

            for path in &paths {
                let path_str = path.to_string_lossy();
                if let Ok(current_hash) = crate::change_detector::compute_file_hash(path) {
                    let needs_update = match state.cache.get_build_hash(&path_str) {
                        Ok(Some(cached_hash)) => cached_hash != current_hash,
                        Ok(None) => true,
                        Err(_) => true,
                    };
                    if needs_update {
                        let package_label =
                            crate::change_detector::compute_build_file_package_label(
                                path,
                                &state.workspace_root,
                            );
                        packages_to_add.push(package_label);
                        changes_to_record.push((path_str.into_owned(), current_hash));
                    }
                }
            }

            if !packages_to_add.is_empty() {
                let mut pending = state
                    .pending_changes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                for package_label in &packages_to_add {
                    if !pending.contains(package_label) {
                        pending.push(package_label.clone());
                    }
                }
                drop(pending);

                for (path_str, hash) in changes_to_record {
                    let _ = state.cache.put_build_hash(&path_str, &hash);
                }
            }
        })
    };

    {
        let mut reg = registry().lock().unwrap_or_else(|e| e.into_inner());
        let state = reg.get_mut(&key).expect("key just inserted");
        match BuildFileWatcher::start(workspace_root, watcher_cb) {
            Ok(watcher) => {
                *state.watcher.lock().unwrap_or_else(|e| e.into_inner()) = Some(watcher);
            }
            Err(e) => {
                log::warn!("Failed to start file watcher: {}", e);
            }
        }
    }

    key as jlong
}

#[no_mangle]
pub extern "system" fn Java_com_bazel_jdt_BazelBridge_nativeShutdown(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle <= 0 {
        return;
    }
    let key = handle as u64;

    let mut state_box = {
        let mut reg = registry().lock().unwrap_or_else(|e| e.into_inner());
        match reg.remove(&key) {
            Some(b) => b,
            None => return,
        }
    };

    let state = &mut *state_box;
    state.signal_shutdown();
    state.set_sync_state(SyncState::Dead);

    let join_handle = {
        let mut watcher_opt = state.watcher.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut watcher) = watcher_opt.take() {
            watcher.stop_nonblocking()
        } else {
            None
        }
    };

    if let Some(jh) = join_handle {
        *state
            .watcher_join_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(jh);
    }

    let jh = state
        .watcher_join_handle
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    if let Some(join_handle) = jh {
        let _ = join_handle.join();
    }
}

#[no_mangle]
pub extern "system" fn Java_com_bazel_jdt_BazelBridge_nativeDiscoverTargets(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    scope_patterns: JObjectArray,
    build_flags: JObjectArray,
) -> jobjectArray {
    let state = match get_state(&mut env, handle) {
        Some(s) => s,
        None => return std::ptr::null_mut(),
    };
    state.set_sync_state(SyncState::Syncing);

    let scope = parse_java_string_array(&mut env, &scope_patterns);
    let scope_ref: Option<&[String]> = scope.as_deref();
    let build_flags_vec = parse_java_string_array(&mut env, &build_flags);
    let build_flags_ref: Option<&[String]> = build_flags_vec.as_deref();

    let mut shutdown_rx = state.shutdown_signal();
    let targets = match state.runtime.block_on(async {
        tokio::select! {
            result = tokio::time::timeout(state.query_timeout, state.invoker.discover_java_targets(scope_ref, build_flags_ref)) => {
                match result {
                    Ok(Ok(t)) => Ok(t),
                    Ok(Err(e)) => Err(format!(
                        "Failed to discover targets: {}. Try running 'bazel query //...:*' in the workspace to verify Java targets exist.",
                        e
                    )),
                    Err(_) => Err(format!(
                        "Bazel query timed out after {}s. Try running 'bazel query //...:*' manually to check performance.",
                        state.query_timeout.as_secs()
                    )),
                }
            }
            _ = shutdown_rx.changed() => {
                Err("Operation cancelled: shutdown requested".to_string())
            }
        }
    }) {
        Ok(t) => t,
        Err(e) => {
            state.set_sync_state(SyncState::Error);
            let _ = env.throw_new("java/lang/RuntimeException", e);
            return std::ptr::null_mut();
        }
    };

    if targets.is_empty() {
        log::info!("No Java targets found in workspace - setting state to Idle");
        state.set_sync_state(SyncState::Idle);
        return match create_string_array(&mut env, &[]) {
            Ok(arr) => arr,
            Err(_) => std::ptr::null_mut(),
        };
    }

    match state.populate_graph_from_build_files() {
        Ok(count) => log::info!("Populated dependency graph from {} BUILD files", count),
        Err(e) => log::warn!("Failed to populate graph from BUILD files: {}", e),
    }

    // Batch aspect build: populate JAR data for ALL targets in a single Bazel invocation.
    // This ensures subsequent nativeComputeClasspath() calls hit the fast path (graph data).
    // If this fails, individual targets will fall through to per-target aspect builds.
    {
        let aspect_targets = targets.clone();
        let mut batch_rx = state.shutdown_signal();
        let batch_result = state.runtime.block_on(async {
            tokio::select! {
                result = tokio::time::timeout(
                    state.aspect_timeout,
                    state.invoker.resolve_full_classpath_with_flags(&aspect_targets, build_flags_ref),
                ) => {
                    match result {
                        Ok(Ok(results)) => Ok(results),
                        Ok(Err(e)) => {
                            log::warn!("Batch aspect build failed: {}. Per-target resolution will be used.", e);
                            Err(())
                        }
                        Err(_) => {
                            log::warn!(
                                "Batch aspect build timed out after {}s. Per-target resolution will be used.",
                                state.aspect_timeout.as_secs()
                            );
                            Err(())
                        }
                    }
                }
                _ = batch_rx.changed() => {
                    log::info!("Batch aspect build cancelled: shutdown requested");
                    Err(())
                }
            }
        });
        if let Ok(aspect_results) = batch_result {
            let mut graph = state.graph.lock().unwrap_or_else(|e| e.into_inner());
            graph.populate_from_aspects(&aspect_results, &state.workspace_root);
            log::info!(
                "Populated graph with batch aspect data: {} targets, {} with JARs",
                aspect_results.len(),
                aspect_results
                    .iter()
                    .filter(|r| r.java_info.is_some())
                    .count()
            );
        }
    }

    state.set_sync_state(SyncState::Idle);
    match create_string_array(&mut env, &targets) {
        Ok(arr) => arr,
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "system" fn Java_com_bazel_jdt_BazelBridge_nativeComputeClasspath(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    target_label: JString,
    build_flags: JObjectArray,
) -> jobjectArray {
    let state = match get_state(&mut env, handle) {
        Some(s) => s,
        None => return std::ptr::null_mut(),
    };

    let label: String = match env.get_string(&target_label) {
        Ok(s) => s.into(),
        Err(_) => {
            let _ = env.throw_new("java/lang/IllegalArgumentException", "Invalid target label");
            return std::ptr::null_mut();
        }
    };

    let build_flags_vec = parse_java_string_array(&mut env, &build_flags);
    let build_flags_ref: Option<&[String]> = build_flags_vec.as_deref();

    state.set_sync_state(SyncState::Syncing);

    if let Ok(Some(cached_json)) = state.cache.get_classpath(&label) {
        match serde_json::from_str::<bazel_graph::ComputedClasspath>(&cached_json) {
            Ok(computed) => {
                let entries = computed.to_pipe_delimited_entries();
                state.set_sync_state(SyncState::Idle);
                return match create_string_array(&mut env, &entries) {
                    Ok(arr) => arr,
                    Err(_) => std::ptr::null_mut(),
                };
            }
            Err(e) => {
                log::warn!(
                    "Failed to deserialize cached classpath for {}: {}",
                    label,
                    e
                );
            }
        }
    }

    let target_kind = infer_target_kind(&label);

    let graph = state.graph.lock().unwrap_or_else(|e| e.into_inner());
    let has_aspect_data = graph.get_target_jars(&label).is_some();
    drop(graph);

    if has_aspect_data {
        let graph = state.graph.lock().unwrap_or_else(|e| e.into_inner());
        match bazel_graph::ComputedClasspath::compute_for(&graph, &label, target_kind) {
            Ok(computed) => {
                let entries = computed.to_pipe_delimited_entries();
                log::debug!(
                    "[bazel-jdt] nativeComputeClasspath '{}' -> {} entries (graph path)",
                    label,
                    entries.len()
                );
                for entry in &entries {
                    if entry.contains("-sources") || entry.contains("source") {
                        log::trace!("[bazel-jdt]   SOURCE entry: {}", entry);
                    }
                }
                if let Ok(json) = serde_json::to_string(&computed) {
                    let _ = state.cache.put_classpath(&label, &json);
                }
                state.set_sync_state(SyncState::Idle);
                return match create_string_array(&mut env, &entries) {
                    Ok(arr) => arr,
                    Err(_) => std::ptr::null_mut(),
                };
            }
            Err(e) => {
                log::warn!("Graph compute_for failed for {}: {}", label, e);
            }
        }
    }

    match run_full_resolution(state, &label, state.shutdown_signal(), build_flags_ref) {
        Ok(resolved) => {
            let entries = resolved.to_pipe_delimited_entries();
            log::debug!(
                "[bazel-jdt] nativeComputeClasspath '{}' -> {} entries (slow path)",
                label,
                entries.len()
            );
            for entry in &entries {
                if entry.contains("-sources") || entry.contains("source") {
                    log::trace!("[bazel-jdt]   SOURCE entry: {}", entry);
                }
            }
            if let Ok(json) = serde_json::to_string(&resolved) {
                let _ = state.cache.put_classpath(&label, &json);
            }
            state.set_sync_state(SyncState::Idle);
            match create_string_array(&mut env, &entries) {
                Ok(arr) => arr,
                Err(_) => std::ptr::null_mut(),
            }
        }
        Err(resolution_err) => {
            state.set_sync_state(SyncState::Error);
            let _ = env.throw_new(
                "java/lang/RuntimeException",
                format!(
                    "Classpath resolution failed for '{}': {}. \
                     Try running 'bazel-jdt.cleanCache' then reimporting.",
                    label, resolution_err
                ),
            );
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_com_bazel_jdt_BazelBridge_nativeGetSyncState(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jint {
    let state = match get_state(&mut env, handle) {
        Some(s) => s,
        None => return SyncState::Dead as jint,
    };
    state.get_sync_state() as jint
}

#[no_mangle]
pub extern "system" fn Java_com_bazel_jdt_BazelBridge_nativeCleanCache(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    let state = match get_state(&mut env, handle) {
        Some(s) => s,
        None => return,
    };
    if let Err(e) = state.cache.clear() {
        state.set_sync_state(SyncState::Error);
        let _ = env.throw_new(
            "java/lang/RuntimeException",
            format!("Failed to clear cache: {}", e),
        );
    }
}

#[no_mangle]
pub extern "system" fn Java_com_bazel_jdt_BazelBridge_nativeGetPendingChanges(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jobjectArray {
    let state = match get_state(&mut env, handle) {
        Some(s) => s,
        None => return std::ptr::null_mut(),
    };
    let labels: Vec<String> = {
        let mut pending = state
            .pending_changes
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *pending)
    };
    match create_string_array(&mut env, &labels) {
        Ok(arr) => arr,
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "system" fn Java_com_bazel_jdt_BazelBridge_nativeGetTransitiveWorkspaceDeps(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    target_labels: JObjectArray,
) -> jobjectArray {
    let state = match get_state(&mut env, handle) {
        Some(s) => s,
        None => return std::ptr::null_mut(),
    };

    let labels = match parse_java_string_array(&mut env, &target_labels) {
        Some(l) => l,
        None => {
            return match create_string_array(&mut env, &[]) {
                Ok(arr) => arr,
                Err(_) => std::ptr::null_mut(),
            }
        }
    };

    let graph = state.graph.lock().unwrap_or_else(|e| e.into_inner());
    let mut workspace_deps: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for label in &labels {
        if let Ok(deps) = graph.transitive_deps(label) {
            for dep in deps {
                if !dep.starts_with('@')
                    && !bazel_graph::is_bazel_internal_label(&dep)
                    && seen.insert(dep.clone())
                {
                    workspace_deps.push(dep);
                }
            }
        }
    }

    log::info!(
        "Transitive workspace deps for {} targets: {} labels",
        labels.len(),
        workspace_deps.len()
    );

    match create_string_array(&mut env, &workspace_deps) {
        Ok(arr) => arr,
        Err(_) => std::ptr::null_mut(),
    }
}

fn infer_target_kind(label: &str) -> TargetKind {
    let rule_name = label.rsplit(':').next().unwrap_or(label);
    if rule_name.contains("_test") || rule_name.ends_with("Test") {
        TargetKind::JavaTest
    } else if rule_name.contains("_binary") || rule_name.ends_with("Binary") || rule_name == "main"
    {
        TargetKind::JavaBinary
    } else if rule_name.contains("_import") || rule_name.ends_with("Import") {
        TargetKind::JavaImport
    } else {
        TargetKind::JavaLibrary
    }
}

fn run_full_resolution(
    state: &BazelJdtState,
    target_label: &str,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    build_flags: Option<&[String]>,
) -> Result<bazel_graph::ComputedClasspath, String> {
    let targets = vec![target_label.to_string()];

    let aspect_results = state.runtime.block_on(async {
        tokio::select! {
            result = tokio::time::timeout(
                state.aspect_timeout,
                state.invoker.resolve_full_classpath_with_flags(&targets, build_flags),
            ) => {
                result
                .map_err(|_| {
                    format!(
                        "Bazel aspect build timed out after {}s for target '{}'",
                        state.aspect_timeout.as_secs(),
                        target_label
                    )
                })?
                .map_err(|e| {
                    let err_str = format!("{}", e);
                    let is_aspect_not_found = (err_str.contains("repository")
                        && err_str.contains("not found"))
                        || (err_str.contains("package")
                            && err_str.contains("not found"));
                    if is_aspect_not_found {
                        format!(
                            "Bazel aspect build failed: the IDE aspect files are missing. \
                             Try running 'Bazel: Import Project' to re-extract them. Details: {}",
                            err_str
                        )
                    } else {
                        format!("Bazel aspect build failed: {}", err_str)
                    }
                })
            }
            _ = shutdown_rx.changed() => {
                Err(format!(
                    "Operation cancelled during aspect build for '{}'",
                    target_label
                ))
            }
        }
    })?;

    if aspect_results.is_empty() {
        return Err(format!(
            "No aspect output produced for target '{}'. \
             Verify the target exists and has Java rules.",
            target_label
        ));
    }

    {
        let mut graph = state.graph.lock().unwrap_or_else(|e| e.into_inner());
        graph.populate_from_aspects(&aspect_results, &state.workspace_root);
        log::info!(
            "Populated graph with {} aspect results for slow-path resolution",
            aspect_results.len()
        );
    }

    let graph = state.graph.lock().unwrap_or_else(|e| e.into_inner());
    bazel_graph::ComputedClasspath::compute_for(
        &graph,
        target_label,
        infer_target_kind(target_label),
    )
    .map_err(|e| format!("Graph computation failed: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_target_kind_library() {
        assert_eq!(infer_target_kind("//foo:utils"), TargetKind::JavaLibrary);
        assert_eq!(infer_target_kind("//app:app_lib"), TargetKind::JavaLibrary);
        assert_eq!(infer_target_kind("//pkg:Greeter"), TargetKind::JavaLibrary);
    }

    #[test]
    fn test_infer_target_kind_test() {
        assert_eq!(infer_target_kind("//foo:my_test"), TargetKind::JavaTest);
        assert_eq!(infer_target_kind("//foo:GreeterTest"), TargetKind::JavaTest);
        assert_eq!(
            infer_target_kind("//foo:integration_test"),
            TargetKind::JavaTest
        );
    }

    #[test]
    fn test_infer_target_kind_binary() {
        assert_eq!(infer_target_kind("//foo:my_binary"), TargetKind::JavaBinary);
        assert_eq!(infer_target_kind("//foo:AppBinary"), TargetKind::JavaBinary);
        assert_eq!(infer_target_kind("//foo:main"), TargetKind::JavaBinary);
    }

    #[test]
    fn test_infer_target_kind_import() {
        assert_eq!(infer_target_kind("//foo:my_import"), TargetKind::JavaImport);
        assert_eq!(
            infer_target_kind("//foo:MavenImport"),
            TargetKind::JavaImport
        );
    }

    #[test]
    fn test_infer_target_kind_external() {
        assert_eq!(infer_target_kind("@maven//:guava"), TargetKind::JavaLibrary);
        assert_eq!(
            infer_target_kind("@@rules_jvm_external~maven~maven//:com_google_guava_guava"),
            TargetKind::JavaLibrary
        );
    }
}
