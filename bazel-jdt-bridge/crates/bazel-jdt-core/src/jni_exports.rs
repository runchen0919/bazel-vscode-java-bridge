use crate::state::{BazelJdtState, SyncState};
use crate::watcher::BuildFileWatcher;
use std::sync::atomic::Ordering;

use bazel_graph::TargetKind;
use jni::objects::{JClass, JObject, JString};
use jni::sys::{jint, jlong, jobjectArray, jsize};
use jni::JNIEnv;

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

fn encode_handle(generation: u32, ptr: *mut BazelJdtState) -> jlong {
    (((generation as u64) << 32) | (ptr as u64)) as jlong
}

fn decode_handle(handle: jlong) -> (u32, *mut BazelJdtState) {
    let gen = (handle >> 32) as u32;
    let ptr = (handle & 0xFFFFFFFF) as *mut BazelJdtState;
    (gen, ptr)
}

fn get_valid_state(env: &mut JNIEnv, handle: jlong) -> Option<&'static BazelJdtState> {
    if handle == -1 {
        let _ = env.throw_new("java/lang/IllegalStateException", "Not initialized");
        return None;
    }
    let (gen, ptr) = decode_handle(handle);
    if ptr.is_null() {
        let _ = env.throw_new("java/lang/IllegalStateException", "Invalid handle");
        return None;
    }
    let state = unsafe { &*ptr };
    if state.is_shutdown() {
        let _ = env.throw_new(
            "java/lang/IllegalStateException",
            "Native library has been shut down",
        );
        return None;
    }
    if state.current_generation() != gen {
        let _ = env.throw_new(
            "java/lang/IllegalStateException",
            "Stale handle: state has been re-initialized",
        );
        return None;
    }
    Some(state)
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

    let workspace_root = state.workspace_root.clone();
    let state_ptr: usize = &state as *const BazelJdtState as usize;
    match BuildFileWatcher::start(
        workspace_root,
        Box::new(move |paths| {
            log::info!("Build files changed: {:?}", paths);
            let state = unsafe { &*(state_ptr as *const BazelJdtState) };
            if state.is_shutdown() {
                return;
            }
            for path in &paths {
                let path_str = path.to_string_lossy();
                if let Ok(Some(cached_hash)) = state.cache.get_build_hash(&path_str) {
                    if let Ok(current_hash) = crate::change_detector::compute_file_hash(path) {
                        if cached_hash != current_hash {
                            let package_label =
                                crate::change_detector::compute_build_file_package_label(
                                    path,
                                    &state.workspace_root,
                                );
                            let mut pending = state
                                .pending_changes
                                .lock()
                                .unwrap_or_else(|e| e.into_inner());
                            if !pending.contains(&package_label) {
                                pending.push(package_label);
                            }
                            let _ = state.cache.put_build_hash(&path_str, &current_hash);
                        }
                    }
                } else {
                    if let Ok(current_hash) = crate::change_detector::compute_file_hash(path) {
                        let package_label =
                            crate::change_detector::compute_build_file_package_label(
                                path,
                                &state.workspace_root,
                            );
                        let mut pending = state
                            .pending_changes
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        if !pending.contains(&package_label) {
                            pending.push(package_label);
                        }
                        let _ = state.cache.put_build_hash(&path_str, &current_hash);
                    }
                }
            }
        }),
    ) {
        Ok(watcher) => {
            *state.watcher.lock().unwrap_or_else(|e| e.into_inner()) = Some(watcher);
        }
        Err(e) => {
            log::warn!("Failed to start file watcher: {}", e);
        }
    }

    let gen = state.next_generation();
    let ptr = Box::into_raw(Box::new(state));
    encode_handle(gen, ptr)
}

#[no_mangle]
pub extern "system" fn Java_com_bazel_jdt_BazelBridge_nativeShutdown(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle == -1 {
        return;
    }
    let (_, ptr) = decode_handle(handle);
    if ptr.is_null() {
        return;
    }
    unsafe {
        let state = &*ptr;
        if state
            .shutdown_flag
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        state.set_sync_state(SyncState::Dead);
        if let Some(mut watcher) = state
            .watcher
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            watcher.stop();
        };
        let state = Box::from_raw(ptr);
        drop(state);
    }
}

#[no_mangle]
pub extern "system" fn Java_com_bazel_jdt_BazelBridge_nativeDiscoverTargets(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jobjectArray {
    let state = match get_valid_state(&mut env, handle) {
        Some(s) => s,
        None => return std::ptr::null_mut(),
    };
    state.set_sync_state(SyncState::Syncing);

    let targets = match state
        .runtime
        .block_on(state.invoker.discover_java_targets())
    {
        Ok(t) => t,
        Err(e) => {
            state.set_sync_state(SyncState::Error);
            let _ = env.throw_new(
                "java/lang/RuntimeException",
                format!(
                    "Failed to discover targets: {}. \
                     Try running 'bazel query //...:*' in the workspace to verify Java targets exist.",
                    e
                ),
            );
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
) -> jobjectArray {
    let state = match get_valid_state(&mut env, handle) {
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
    match bazel_graph::ComputedClasspath::compute_for(&graph, &label, target_kind) {
        Ok(computed) => {
            let entries = computed.to_pipe_delimited_entries();
            drop(graph);
            if let Ok(json) = serde_json::to_string(&computed) {
                let _ = state.cache.put_classpath(&label, &json);
            }
            state.set_sync_state(SyncState::Idle);
            match create_string_array(&mut env, &entries) {
                Ok(arr) => arr,
                Err(_) => std::ptr::null_mut(),
            }
        }
        Err(bazel_graph::GraphError::TargetNotFound { .. }) => {
            drop(graph);
            match run_full_resolution(state, &label) {
                Ok(computed) => {
                    let entries = computed.to_pipe_delimited_entries();
                    if let Ok(json) = serde_json::to_string(&computed) {
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
        Err(e) => {
            state.set_sync_state(SyncState::Error);
            let _ = env.throw_new(
                "java/lang/RuntimeException",
                format!(
                    "Failed to compute classpath for '{}': {}. \
                     Check that the target exists and its dependencies are valid.",
                    label, e
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
    let state = match get_valid_state(&mut env, handle) {
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
    let state = match get_valid_state(&mut env, handle) {
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
    let state = match get_valid_state(&mut env, handle) {
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
) -> Result<bazel_graph::ComputedClasspath, String> {
    let targets = vec![target_label.to_string()];

    let aspect_results = state
        .runtime
        .block_on(state.invoker.resolve_full_classpath(&targets))
        .map_err(|e| format!("Bazel aspect build failed: {}", e))?;

    if aspect_results.is_empty() {
        return Err(format!(
            "No aspect output produced for target '{}'. \
             Verify the target exists and has Java rules.",
            target_label
        ));
    }

    {
        let mut graph = state.graph.lock().unwrap_or_else(|e| e.into_inner());
        graph.populate_from_aspects(&aspect_results);
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
