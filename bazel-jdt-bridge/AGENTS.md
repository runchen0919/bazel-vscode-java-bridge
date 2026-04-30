# BAZEL-JDT-BRIDGE

Bazel → Eclipse JDT Language Server bridge. Rust core with Java JNI binding and VS Code extension shell.

## OVERVIEW

Execution chain: VS Code opens Java file → JDT.LS loads OSGi bundle → Java `BazelBridge` loads Rust cdylib via JNI → Rust queries Bazel, parses BUILD files, resolves classpaths.

## STRUCTURE

```
bazel-jdt-bridge/
├── crates/
│   ├── bazel-parser/      # Starlark/BUILD file parser (starlark_syntax)
│   ├── bazel-aspect/      # Bazel aspect text_proto parser
│   ├── bazel-query/       # Async Bazel CLI query (tokio)
│   ├── bazel-graph/       # Dependency graph + classpath (petgraph)
│   ├── bazel-cache/       # Persistent KV cache (redb)
│   └── bazel-jdt-core/    # JNI cdylib + state + watcher + change detection
├── java-bridge/           # OSGi bundle for JDT.LS (Maven, Java 17, bnd)
├── vscode-extension/      # VS Code extension (TypeScript, esbuild)
├── scripts/               # build-native.sh, package-extension.sh
└── tests/                 # e2e/ and stress/ (empty placeholders)
```

## WHERE TO LOOK

| Task | File(s) |
|------|---------|
| Add new JNI function | `crates/bazel-jdt-core/src/jni_exports.rs` + `java-bridge/.../BazelBridge.java` |
| Parse new BUILD rule attribute | `crates/bazel-parser/src/model.rs` + `parser.rs` |
| Change classpath resolution | `crates/bazel-graph/src/classpath.rs` |
| Modify dependency graph | `crates/bazel-graph/src/graph.rs` |
| Parse aspect output | `crates/bazel-aspect/src/text_proto.rs` + `ide_info.rs` |
| Add Bazel CLI query | `crates/bazel-query/src/command.rs` |
| Persistent cache schema | `crates/bazel-cache/src/redb_store.rs` |
| File change detection | `crates/bazel-jdt-core/src/change_detector.rs` + `watcher.rs` |
| Java-side classpath container | `java-bridge/.../BazelClasspathManager.java` + `BazelClasspathContainer.java` |
| VS Code commands/status bar | `vscode-extension/src/commands.ts` + `statusBar.ts` |
| Build/package pipeline | `scripts/build-native.sh` + `scripts/package-extension.sh` |

## CRATE DEPENDENCY GRAPH

```
bazel-jdt-core ──→ bazel-parser
                ──→ bazel-aspect
                ──→ bazel-query
                ──→ bazel-graph ──→ bazel-parser
                ──→ bazel-cache
```

No other cross-crate dependencies. Each crate is independently testable except `bazel-jdt-core` (requires all).

## JNI FFI INTERFACE

6 exported functions in `jni_exports.rs`:

| Function | Purpose |
|----------|---------|
| `nativeInitialize` | Create `BazelJdtState`, start file watcher |
| `nativeShutdown` | Drop state, stop watcher |
| `nativeDiscoverTargets` | Query Bazel, parse BUILD files |
| `nativeComputeClasspath` | Resolve dependency graph → classpath entries |
| `nativeGetSyncState` | Return current sync status |
| `nativeCleanCache` | Clear redb cache |

All take `JNIEnv` + `jlong` handle (pointer to `BazelJdtState`). Handle is created by `nativeInitialize`, freed by `nativeShutdown`. **No use-after-free protection** — calling after shutdown is UB.

## CONVENTIONS

- All Rust crates use `pub mod` + `pub use` re-exports in `lib.rs` (no barrel index files)
- Tests are inline `#[cfg(test)] mod tests` — no separate test files, no dev-dependencies
- Java side is OSGi singleton (`plugin.xml` + `bnd.bnd`) — must be thread-safe
- VS Code extension activates on `onLanguage:java` + `workspaceContains:WORKSPACE`
- Native lib packaged in JAR at `native/<platform>/` via `bnd.bnd` `Bundle-NativeCode`

## ANTI-PATTERNS

- `BazelClasspathManager.java`: 3 silent `catch (Exception e)` blocks
- `BazelProjectImporter.java` line 69: completely empty `catch (Exception e) { }`
- `classpath.rs::filter_by_visibility()`: no-op placeholder method

## KNOWN ISSUES

- Release pipeline: cross-compiled native libs may not reach final VSIX (artifact download step missing)
- `build-native.sh` targets `x86_64-pc-windows-gnu` but `release.yml` uses `x86_64-pc-windows-msvc`
- `package-extension.sh` uses `|| true` after vsce packaging — silently swallows errors
- OSGi bundle may not load in some JDT.LS environments due to `Require-Bundle` version mismatch

## BUILD ORDER

```
1. cargo build -p bazel-jdt-core --release   # Rust → native .so/.dylib/.dll
2. mvn clean package -DskipTests              # Java → com.bazel.jdt.jar (OSGi bundle)
3. npm run build (in vscode-extension/)       # TS → dist/extension.js
4. scripts/package-extension.sh               # Assemble → .vsix
```

Java tests require native lib: `mvn test -Djava.library.path=../target/release`
