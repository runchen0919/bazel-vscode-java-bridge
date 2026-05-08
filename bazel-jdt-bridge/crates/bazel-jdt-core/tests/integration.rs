use bazel_cache::BazelCache;
use bazel_graph::{ComputedClasspath, DependencyGraph, TargetKind};
use bazel_parser::{BuildFileParser, RuleType};

#[test]
fn parse_java_library_rule() {
    let temp_dir = tempfile::tempdir().unwrap();
    let build_file = temp_dir.path().join("BUILD.bazel");
    std::fs::write(
        &build_file,
        r#"java_library(
    name = "greeter",
    srcs = ["src/main/java/com/example/greeter/Greeter.java"],
)"#,
    )
    .unwrap();

    let parser = BuildFileParser::new();
    let parsed = parser.parse_file(&build_file).unwrap();

    assert_eq!(parsed.rules.len(), 1);
    let rule = &parsed.rules[0];
    assert_eq!(rule.name, "greeter");
    assert!(matches!(rule.rule_type, RuleType::JavaLibrary));
    assert_eq!(
        rule.srcs,
        vec!["src/main/java/com/example/greeter/Greeter.java"]
    );
}

#[test]
fn parse_java_binary_with_deps() {
    let temp_dir = tempfile::tempdir().unwrap();
    let build_file = temp_dir.path().join("BUILD.bazel");
    std::fs::write(
        &build_file,
        r#"java_binary(
    name = "app",
    srcs = ["src/main/java/com/example/app/Main.java"],
    deps = ["//greeter:greeter"],
    main_class = "com.example.app.Main",
)"#,
    )
    .unwrap();

    let parser = BuildFileParser::new();
    let parsed = parser.parse_file(&build_file).unwrap();

    assert_eq!(parsed.rules.len(), 1);
    let rule = &parsed.rules[0];
    assert!(matches!(rule.rule_type, RuleType::JavaBinary));
    assert_eq!(rule.deps, vec!["//greeter:greeter"]);
}

#[test]
fn dependency_chain_classpath() {
    let mut graph = DependencyGraph::new();

    graph.add_target("//utils:utils");
    graph.add_target("//service:service");
    graph.add_target("//app:app");

    graph.add_dep("//app:app", "//service:service");
    graph.add_dep("//service:service", "//utils:utils");

    graph.set_target_jars("//utils:utils", vec!["utils.jar".to_string()]);
    graph.set_target_jars("//service:service", vec!["service.jar".to_string()]);
    graph.set_target_jars("//app:app", vec!["app.jar".to_string()]);

    let classpath =
        ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaBinary, None).unwrap();

    let paths: Vec<&str> = classpath.entries.iter().map(|e| e.path.as_str()).collect();

    assert!(
        paths.iter().any(|p| p.contains("utils")),
        "Classpath should include transitive dep utils.jar, got: {:?}",
        paths
    );
    assert!(
        paths.iter().any(|p| p.contains("service")),
        "Classpath should include direct dep service.jar, got: {:?}",
        paths
    );
    assert_eq!(classpath.target_label, "//app:app");
}

#[test]
fn populate_graph_from_parsed_build_files() {
    let mut graph = DependencyGraph::new();

    let lib_target = "//lib:lib";
    let app_target = "//app:app";

    graph.add_target(lib_target);
    graph.add_target(app_target);
    graph.add_dep(app_target, lib_target);

    assert!(graph.has_target(lib_target));
    assert!(graph.has_target(app_target));

    let deps = graph.transitive_deps(app_target).unwrap();
    assert!(
        deps.contains(&lib_target.to_string()),
        "app should transitively depend on lib, got deps: {:?}",
        deps
    );
}

#[test]
fn cache_store_and_retrieve() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cache = BazelCache::open(temp_dir.path()).unwrap();

    let label = "//app:app";
    let classpath_json = r#"{"entries":[{"type":"LIB","path":"app.jar"}]}"#;

    cache.put_classpath(label, classpath_json).unwrap();

    let retrieved = cache.get_classpath(label).unwrap();
    assert_eq!(retrieved, Some(classpath_json.to_string()));
}

#[test]
fn cache_miss_for_unknown_target() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cache = BazelCache::open(temp_dir.path()).unwrap();

    cache.put_classpath("//dummy:init", "{}").unwrap();
    cache
        .invalidate_targets(&["//dummy:init".to_string()])
        .unwrap();

    let result = cache.get_classpath("//nonexistent:target").unwrap();
    assert_eq!(result, None);
}

#[test]
fn cache_build_hash_store_and_retrieve() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cache = BazelCache::open(temp_dir.path()).unwrap();

    let build_path = "app/BUILD.bazel";
    let hash = "abc123def456";

    cache.put_build_hash(build_path, hash).unwrap();

    let retrieved = cache.get_build_hash(build_path).unwrap();
    assert_eq!(retrieved, Some(hash.to_string()));
}

#[test]
fn cache_invalidation() {
    let temp_dir = tempfile::tempdir().unwrap();
    let cache = BazelCache::open(temp_dir.path()).unwrap();

    let label = "//app:app";
    cache
        .put_classpath(label, r#"{"entries":[{"type":"LIB","path":"app.jar"}]}"#)
        .unwrap();

    cache.invalidate_targets(&[label.to_string()]).unwrap();

    let result = cache.get_classpath(label).unwrap();
    assert_eq!(result, None, "Invalidated target should return None");
}

#[test]
fn pipe_delimited_classpath_output() {
    let mut graph = DependencyGraph::new();
    graph.add_target("//lib:lib");
    graph.add_target("//app:app");
    graph.add_dep("//app:app", "//lib:lib");
    graph.set_target_jars("//lib:lib", vec!["lib.jar".to_string()]);
    graph.set_target_jars("//app:app", vec!["app.jar".to_string()]);

    let classpath =
        ComputedClasspath::compute_for(&graph, "//app:app", TargetKind::JavaBinary, None).unwrap();

    let entries = classpath.to_pipe_delimited_entries();
    assert!(
        !entries.is_empty(),
        "Should produce pipe-delimited entries for target with deps"
    );
    assert!(
        entries.iter().any(|e| e.contains("lib.jar")),
        "Entries should contain lib.jar, got: {:?}",
        entries
    );
}
