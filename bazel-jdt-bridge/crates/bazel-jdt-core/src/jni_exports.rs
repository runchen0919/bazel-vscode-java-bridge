use crate::state::{BazelJdtState, SyncState};
use crate::watcher::BuildFileWatcher;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use bazel_graph::infer_target_kind;
use jni::objects::{JClass, JObject, JObjectArray, JString};
use jni::sys::{jint, jlong, jobjectArray, jsize};
use jni::JNIEnv;

static REGISTRY: OnceLock<Mutex<HashMap<u64, Box<BazelJdtState>>>> = OnceLock::new();
static NEXT_KEY: AtomicU64 = AtomicU64::new(1);

const CONFIG_CHANGED_SENTINEL: &str = "__CONFIG_CHANGED__";

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
    // Initialize stderr logger (controlled via RUST_LOG env var, default=warn).
    // try_init is idempotent — safe if called multiple times.
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default()
            .default_filter_or("warn,bazel_jdt_core=info,bazel_query=info"),
    )
    .try_init();

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

    {
        let mut reg = registry().lock().unwrap_or_else(|e| e.into_inner());
        reg.insert(key, Box::new(state));
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

fn make_watcher_callback(
    registry_key: u64,
) -> Box<dyn Fn(Vec<std::path::PathBuf>) + Send + 'static> {
    Box::new(move |paths| {
        log::info!("Watched files changed: {:?}", paths);
        let reg = registry().lock().unwrap_or_else(|e| e.into_inner());
        let state = match reg.get(&registry_key) {
            Some(s) => s,
            None => return,
        };
        if state.is_shutdown() {
            return;
        }

        let mut changes_to_record: Vec<(String, String)> = Vec::new();
        let mut packages_to_add: Vec<String> = Vec::new();
        let mut config_changed = false;

        for path in &paths {
            if crate::watcher::is_bazelproject_file(path) {
                config_changed = true;
                continue;
            }

            let path_str = path.to_string_lossy();
            if let Ok(current_hash) = crate::change_detector::compute_file_hash(path) {
                let needs_update = match state.cache.get_build_hash(&path_str) {
                    Ok(Some(cached_hash)) => cached_hash != current_hash,
                    Ok(None) => true,
                    Err(_) => true,
                };
                if needs_update {
                    let package_label = crate::change_detector::compute_build_file_package_label(
                        path,
                        &state.workspace_root,
                    );
                    packages_to_add.push(package_label);
                    changes_to_record.push((path_str.into_owned(), current_hash));
                }
            }
        }

        let mut pending = state
            .pending_changes
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        if config_changed && !pending.contains(&CONFIG_CHANGED_SENTINEL.to_string()) {
            pending.push(CONFIG_CHANGED_SENTINEL.to_string());
        }

        for package_label in &packages_to_add {
            if !pending.contains(package_label) {
                pending.push(package_label.clone());
            }
        }
        drop(pending);

        for (path_str, hash) in changes_to_record {
            let _ = state.cache.put_build_hash(&path_str, &hash);
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_bazel_jdt_BazelBridge_nativeUpdateWatchPaths(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    watch_paths: JObjectArray,
) {
    let state = match get_state(&mut env, handle) {
        Some(s) => s,
        None => return,
    };

    // Stop existing watcher if any
    {
        let mut watcher_opt = state.watcher.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut watcher) = watcher_opt.take() {
            watcher.stop();
        }
    }

    let paths = match parse_java_string_array(&mut env, &watch_paths) {
        Some(p) => p,
        None => {
            log::info!("No watch paths provided, watcher stopped");
            return;
        }
    };

    if paths.is_empty() {
        log::info!("Empty watch paths, watcher stopped");
        return;
    }

    let watch_path_bufs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    let workspace_root = state.workspace_root.clone();
    let key = handle as u64;

    let callback = make_watcher_callback(key);

    match BuildFileWatcher::start(workspace_root, watch_path_bufs, callback) {
        Ok(watcher) => {
            *state.watcher.lock().unwrap_or_else(|e| e.into_inner()) = Some(watcher);
        }
        Err(e) => {
            log::warn!("Failed to start file watcher: {}", e);
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_com_bazel_jdt_BazelBridge_nativeQueryTargets(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    scope_patterns: JObjectArray,
) -> jobjectArray {
    let state = match get_state(&mut env, handle) {
        Some(s) => s,
        None => return std::ptr::null_mut(),
    };
    state.set_sync_state(SyncState::Syncing);

    let scope = parse_java_string_array(&mut env, &scope_patterns);
    let scope_ref: Option<&[String]> = scope.as_deref();

    log::info!(
        "nativeQueryTargets: bazel query, workspace={:?}",
        state.workspace_root
    );
    let targets = match state.invoker.discover_java_targets_sync(scope_ref, None) {
        Ok(t) => t,
        Err(e) => {
            let msg = format!(
                "Failed to discover targets: {}. Try running 'bazel query //...:*' in the workspace to verify Java targets exist.",
                e
            );
            log::error!("nativeQueryTargets error: {}", msg);
            state.set_sync_state(SyncState::Error);
            let _ = env.throw_new("java/lang/RuntimeException", msg);
            return std::ptr::null_mut();
        }
    };

    log::info!("nativeQueryTargets: found {} targets", targets.len());
    match create_string_array(&mut env, &targets) {
        Ok(arr) => arr,
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "system" fn Java_com_bazel_jdt_BazelBridge_nativePopulateGraph(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    let state = match get_state(&mut env, handle) {
        Some(s) => s,
        None => return,
    };

    log::info!("nativePopulateGraph: parsing BUILD files");
    match state.populate_graph_from_build_files() {
        Ok(count) => log::info!("Populated dependency graph from {} BUILD files", count),
        Err(e) => log::warn!("Failed to populate graph from BUILD files: {}", e),
    }
}

#[no_mangle]
pub extern "system" fn Java_com_bazel_jdt_BazelBridge_nativeRunAspectBuild(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    targets: JObjectArray,
    build_flags: JObjectArray,
) -> jobjectArray {
    let state = match get_state(&mut env, handle) {
        Some(s) => s,
        None => return std::ptr::null_mut(),
    };

    let target_vec = match parse_java_string_array(&mut env, &targets) {
        Some(t) => t,
        None => {
            state.set_sync_state(SyncState::Idle);
            return match create_string_array(&mut env, &[]) {
                Ok(arr) => arr,
                Err(_) => std::ptr::null_mut(),
            };
        }
    };
    let build_flags_vec = parse_java_string_array(&mut env, &build_flags);
    let build_flags_ref: Option<&[String]> = build_flags_vec.as_deref();

    log::info!(
        "nativeRunAspectBuild: starting batch aspect build for {} targets",
        target_vec.len()
    );
    match state
        .invoker
        .resolve_full_classpath_sync(&target_vec, build_flags_ref)
    {
        Ok(aspect_results) => {
            log::info!(
                "Populating dependency graph from {} aspect results...",
                aspect_results.len()
            );
            let mut graph = state.graph.lock().unwrap_or_else(|e| e.into_inner());
            graph.populate_from_aspects(&aspect_results, &state.workspace_root);
            let total = aspect_results.len() as i32;
            let with_jars = aspect_results
                .iter()
                .filter(|r| r.java_info.is_some())
                .count() as i32;
            state
                .aspect_total_files
                .store(total, std::sync::atomic::Ordering::SeqCst);
            state
                .aspect_files_with_jars
                .store(with_jars, std::sync::atomic::Ordering::SeqCst);
            log::info!(
                "Batch aspect build complete: {} output files, {} with JARs",
                total,
                with_jars
            );
        }
        Err(e) => {
            log::warn!(
                "Batch aspect build failed: {}. Per-target resolution will be used.",
                e
            );
        }
    }

    state.set_sync_state(SyncState::Idle);
    match create_string_array(&mut env, &target_vec) {
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
    let label = bazel_graph::normalize_label(&label);

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
        match bazel_graph::ComputedClasspath::compute_for(
            &graph,
            &label,
            target_kind,
            Some(state.workspace_root.to_str().unwrap_or("")),
        ) {
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

    match run_full_resolution(state, &label, build_flags_ref) {
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
pub extern "system" fn Java_com_bazel_jdt_BazelBridge_nativeComputeClasspathMerged(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    labels: JObjectArray,
) -> jobjectArray {
    let state = match get_state(&mut env, handle) {
        Some(s) => s,
        None => return std::ptr::null_mut(),
    };

    let len = match env.get_array_length(&labels) {
        Ok(l) => l,
        Err(_) => {
            let _ = env.throw_new("java/lang/IllegalArgumentException", "Invalid labels array");
            return std::ptr::null_mut();
        }
    };

    if len == 0 {
        return match create_string_array(&mut env, &[]) {
            Ok(arr) => arr,
            Err(_) => std::ptr::null_mut(),
        };
    }

    let mut label_strings = Vec::with_capacity(len as usize);
    for i in 0..len {
        let s = env
            .get_object_array_element(&labels, i)
            .ok()
            .and_then(|obj| {
                let jstr = JString::from(obj);
                env.get_string(&jstr).ok().map(String::from)
            });
        if let Some(s) = s {
            label_strings.push(bazel_graph::normalize_label(&s));
        }
    }

    let label_refs: Vec<&str> = label_strings.iter().map(|s| s.as_str()).collect();

    let graph = state.graph.lock().unwrap_or_else(|e| e.into_inner());
    match bazel_graph::ComputedClasspath::compute_for_targets(
        &graph,
        &label_refs,
        Some(state.workspace_root.to_str().unwrap_or("")),
    ) {
        Ok(computed) => {
            let entries = computed.to_pipe_delimited_entries();
            log::debug!(
                "[bazel-jdt] nativeComputeClasspathMerged {} targets -> {} entries",
                label_strings.len(),
                entries.len()
            );
            drop(graph);
            match create_string_array(&mut env, &entries) {
                Ok(arr) => arr,
                Err(_) => std::ptr::null_mut(),
            }
        }
        Err(e) => {
            drop(graph);
            let _ = env.throw_new(
                "java/lang/RuntimeException",
                format!("Merged classpath computation failed: {}", e),
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

    let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
    let entries = match graph.transitive_dependency_targets(&label_refs) {
        Ok(e) => e,
        Err(_) => {
            return match create_string_array(&mut env, &[]) {
                Ok(arr) => arr,
                Err(_) => std::ptr::null_mut(),
            }
        }
    };

    log::info!(
        "Transitive workspace dep packages for {} targets: {} packages",
        labels.len(),
        entries.len()
    );

    match create_string_array(&mut env, &entries) {
        Ok(arr) => arr,
        Err(_) => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "system" fn Java_com_bazel_jdt_BazelBridge_nativeSyncIncremental(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    changed_file_paths: JObjectArray,
) -> jobjectArray {
    let state = match get_state(&mut env, handle) {
        Some(s) => s,
        None => return std::ptr::null_mut(),
    };

    let changed_files = match parse_java_string_array(&mut env, &changed_file_paths) {
        Some(paths) => paths.into_iter().map(PathBuf::from).collect::<Vec<_>>(),
        None => {
            return match create_string_array(&mut env, &[]) {
                Ok(arr) => arr,
                Err(_) => std::ptr::null_mut(),
            }
        }
    };

    state.set_sync_state(SyncState::Syncing);

    match state.sync_incremental(&changed_files) {
        Ok(affected_labels) => {
            state.set_sync_state(SyncState::Idle);
            match create_string_array(&mut env, &affected_labels) {
                Ok(arr) => arr,
                Err(_) => std::ptr::null_mut(),
            }
        }
        Err(e) => {
            state.set_sync_state(SyncState::Error);
            let _ = env.throw_new(
                "java/lang/RuntimeException",
                format!("Incremental sync failed: {}", e),
            );
            std::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_com_bazel_jdt_BazelBridge_nativeGetAspectBuildStats(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jni::sys::jstring {
    let state = match get_state(&mut env, handle) {
        Some(s) => s,
        None => return std::ptr::null_mut(),
    };
    let total = state
        .aspect_total_files
        .load(std::sync::atomic::Ordering::SeqCst);
    let with_jars = state
        .aspect_files_with_jars
        .load(std::sync::atomic::Ordering::SeqCst);
    let stats = format!("{}|{}", total, with_jars);
    match env.new_string(&stats) {
        Ok(s) => s.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

fn run_full_resolution(
    state: &BazelJdtState,
    target_label: &str,
    build_flags: Option<&[String]>,
) -> Result<bazel_graph::ComputedClasspath, String> {
    let targets = vec![target_label.to_string()];

    log::info!("run_full_resolution for '{}'", target_label);
    let aspect_results = state
        .invoker
        .resolve_full_classpath_sync(&targets, build_flags)
        .map_err(|e| {
            let err_str = format!("{}", e);
            let is_aspect_not_found = (err_str.contains("repository")
                && err_str.contains("not found"))
                || (err_str.contains("package") && err_str.contains("not found"));
            if is_aspect_not_found {
                format!(
                    "Bazel aspect build failed: the IDE aspect files are missing. \
                     Try running 'Bazel: Import Project' to re-extract them. Details: {}",
                    err_str
                )
            } else {
                format!("Bazel aspect build failed: {}", err_str)
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
        Some(state.workspace_root.to_str().unwrap_or("")),
    )
    .map_err(|e| format!("Graph computation failed: {}", e))
}

