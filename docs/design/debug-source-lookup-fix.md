# Debug Source Lookup Fix — Architecture Analysis

**Date**: 2026-05-23
**Branch**: 001-bazel-java-resolver
**Status**: Implementation Complete (3 layers: label-normalization, graph-lifecycle, runtime-classpath)

## Problem Statement

When debugging a Bazel Java project in VS Code:
- **F3 navigation works**: Clicking `userService.getUserName()` correctly opens the `.java` source file
- **Debug fails**: Setting a breakpoint on `getUserName()` stops execution correctly, but opens the **decompiled `.class` file** instead of the `.java` source

## Root Cause: Three-Layer Architecture Defect

### Layer 1: State Lifecycle — Empty Graph After Fast Reload

**Root Cause**: `BazelProjectImporter.tryFastReload()` calls `bridge.initialize()` which creates a new `BazelJdtState` with an **empty** `DependencyGraph`, but never calls `populateGraph()` or `runAspectBuild()`.

**Impact**: At debug time, `setMergedClasspathContainer()` → `computeClasspathMerged()` → `transitive_deps()` fails with `Target not found: //app:application` because the graph is empty.

**Three caches with independent lifecycles:**

| Cache | Location | Persistence | Populated By |
|-------|----------|-------------|-------------|
| redb KV | `~/.cache/bazel-jdt/` | Survives restart | `nativeComputeClasspath()` on cache miss |
| In-memory graph | `BazelJdtState.graph` | **Dies with process** | `populate_from_parsed_batch()`, `populate_from_aspects()` |
| Java file cache | `.bazel-projects/` per project | Survives restart | `batchSetClasspathContainers(fromCache=false)` |

**The gap**: Fast reload uses Java file cache (survives) but graph (dies) is never repopulated.

**Key files:**
- `BazelProjectImporter.java:244-325` — `tryFastReload()` skips `populateGraph()` + `runAspectBuild()`
- `BazelClasspathManager.java:30-76` — `setMergedClasspathContainer()` fails on empty graph
- `BazelCommandHandler.java:173-219` — `handleBuildTarget()` triggers the failing path
- `state.rs:48-98` — `BazelJdtState::new()` creates empty graph
- `graph.rs:122-128` — `transitive_deps()` crashes on empty graph

### Layer 2: Label Normalization — No Single Source of Truth

**Root Cause**: 4 different label transformation functions with 6 inconsistencies. 4 graph lookup methods bypass alias resolution.

**Label entry points produce different formats:**

| Entry Point | Output Format | Normalization |
|-------------|--------------|---------------|
| `bazel query --output=label` | `//app:application` or `@//app:application` (Bazel 7+) | trim only |
| BUILD file parsing | `//app:application` | derived from file path |
| Aspect output (BZL) | `//app:application` (strips `@//` and `@@//`) | BZL strips `@` |
| JNI from Java | whatever `TargetProjectMapping` stores | `normalize_label()` only adds implicit target |

**Functions:**
- `normalize_label()` (graph.rs:604-616): adds implicit target name, ignores `@` prefix
- `normalize_dep_label()` (graph.rs:587-596): resolves relative deps, delegates to `normalize_label()`
- `canonical_to_apparent_label()` (ide_info.rs:100-113): `@@module~ext~repo` → `@repo` only
- Two `compute_package_label_from_*` functions with different behavior

**4 lookup methods that bypass alias resolution:**

| Method | Location | Risk |
|--------|----------|------|
| `transitive_deps()` | graph.rs:123 | **Crashes** — `TargetNotFound` |
| `direct_dependers()` | graph.rs:328 | Silently returns `[]` |
| `reverse_transitive_deps()` | graph.rs:342 | Silently returns `[]` |
| `get_target_kind()` | graph.rs:204 | Silently returns `Unknown` |

**Contrast with correct methods:** `has_target()` (line 168) and `get_target_jars()` (line 178) DO check aliases.

**Additional issue:** `add_dep()` (line 106) uses direct indexing `self.label_to_index[from]` — potential panic.

### Layer 3: Runtime Classpath Resolution — CPE_PROJECT Gap

**Root Cause**: `BazelRuntimeClasspathEntryResolver.buildEntries()` only processes `CPE_LIBRARY` entries, skipping `CPE_PROJECT` (workspace-internal deps) entirely.

**The resolver at line 80:**
```java
if (cpEntry.getEntryKind() != IClasspathEntry.CPE_LIBRARY) {
    continue;  // SKIPS CPE_PROJECT and CPE_SOURCE!
}
```

**What the Rust side emits for workspace-internal deps:**
- `PROJ|//service:user_service|...` → Java creates `CPE_PROJECT` pointing to workspace project
- `LIB|/path/to/service.jar|/source/path|...` → Java creates `CPE_LIBRARY` with source attachment

**What happens during debug:**
1. Resolver skips `CPE_PROJECT` entries → workspace-internal deps invisible to debug source lookup
2. `CPE_LIBRARY` source attachment may or may not be correct (depends on `infer_source_attachment()`)
3. Debugger falls back to decompiled `.class` file

**Missing in `plugin.xml`:**
- No `sourcePathComputer` extension
- No `sourceContainerResolvers` extension
- No `sourceLookupParticipants` extension

## Causal Chain (Full Picture)

```
VS Code restart
  → tryFastReload() → bridge.initialize() → empty graph
  → batchSetClasspathContainers(fromCache=true) → works from cache
  → User starts debug session
  → handleBuildTarget(["app"])
    → buildTargets(["//app:application"]) → bazel build → SUCCESS
    → clearCacheForProject("app")
    → setMergedClasspathContainer(project, false)
      → computeClasspathMerged(["//app:application"])
        → transitive_deps("//app:application")
          → label_to_index.get("//app:application") → None (empty graph!)
          → ERROR: Target not found: //app:application
      → catch block → container NOT updated → stale import-time container
    → BazelRuntimeClasspathEntryResolver.buildEntries()
      → CPE_LIBRARY entries → processed with source attachment
      → CPE_PROJECT entries → SKIPPED
    → Debug source lookup can't find workspace-internal .java sources
    → Falls back to decompiled .class file
```

## Design Decisions

### Decision 1: Label Normalization — "Normalize at Ingestion"

**Choice**: Single `canonicalize_label()` function applied at every entry point. Graph stores only apparent form.

**Rationale**: More efficient than normalizing at lookup time. Eliminates alias bypass problems. Graph-internal code never needs to worry about label format.

**Canonical form:**
- `//pkg:target` for workspace-internal targets
- `@repo//pkg:target` for external repo targets (apparent form)
- `@@canonical//pkg:target` converted to `@repo//pkg:target` at ingestion

### Decision 2: Graph Lifecycle — "Always Parse BUILD Files on Init"

**Choice**: `nativeInitialize()` always calls `populate_graph_from_build_files()`. Aspect build runs async.

**Rationale**: BUILD file parsing is pure file I/O (< 1 second), no Bazel invocation. Provides immediate graph structure for dependency queries. Aspect data (JARs, sources) populated async.

**Why not persist graph to redb:**
- petgraph `NodeIndex` not stable across serialization
- BUILD file parsing already fast enough
- Avoids schema migration complexity

### Decision 3: Runtime Classpath — "Handle All Entry Types"

**Choice**: Resolver processes `CPE_PROJECT` using `JavaRuntime.newProjectRuntimeClasspathEntry()`.

**Rationale**: Standard Eclipse JDT pattern. Project runtime entries automatically include source folders in debug source lookup.

## Implementation Order

```
Layer 2 (Label Normalization) — Infrastructure, all layers depend on it
  ↓
Layer 1 (Graph Lifecycle) — State management, depends on correct labels
  ↓
Layer 3 (Runtime Classpath) — Top layer, depends on correct container
```

### Layer 2 Implementation Scope

1. New `canonicalize_label()` function in `graph.rs`
2. Replace all `normalize_label()` calls with `canonicalize_label()` at entry points
3. Add `resolve_index()` helper with alias fallback
4. Fix `add_dep()` to use safe lookup
5. Unify `compute_package_label_from_*` functions
6. Add unit tests for all label formats

### Layer 1 Implementation Scope

1. Call `populate_graph_from_build_files()` in `nativeInitialize()`
2. Update `tryFastReload()` to call `populateGraph()`
3. Add `graph_populated: AtomicBool` to `BazelJdtState`
4. Graceful degradation when aspect data not yet available

### Layer 3 Implementation Scope

1. Handle `CPE_PROJECT` in `BazelRuntimeClasspathEntryResolver.buildEntries()`
2. Handle `CPE_SOURCE` entries
3. Test with workspace-internal dependency breakpoints

## Success Criteria

| Criteria | How to Verify |
|----------|--------------|
| `cargo test --workspace` passes | All existing + new tests green |
| `cargo clippy --workspace` clean | No warnings |
| Fast reload + debug works | Breakpoint in workspace-internal dep opens `.java` not `.class` |
| Label format resilient to Bazel 7+ bzlmod | `@//` and `@@//` labels correctly normalized |
| No regression in F3 navigation | Go-to-definition still works |
