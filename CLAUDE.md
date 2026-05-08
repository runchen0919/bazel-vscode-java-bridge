# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build Commands

All paths are relative to `bazel-jdt-bridge/`. Build order matters: Rust native lib must be built before Java tests can run.

### Rust (from `bazel-jdt-bridge/`)

```bash
cargo build -p bazel-jdt-core --release    # Build JNI native lib (.so/.dylib/.dll)
cargo build --workspace                     # Build all 6 crates
cargo test --workspace                      # Run all unit tests
cargo test -p bazel-graph                   # Test a single crate
cargo fmt --all -- --check                  # Format check
cargo clippy --workspace --all-targets -- -D warnings  # Lint (warnings are fatal)
```

### Java (from `bazel-jdt-bridge/java-bridge/`)

```bash
mvn compile                                 # Build OSGi bundle
mvn test -Djava.library.path=../target/release  # Run tests (needs Rust lib built first)
mvn test -Dtest=BazelProjectImporterTest    # Run a single test class
```

### TypeScript (from `bazel-jdt-bridge/vscode-extension/`)

```bash
npm run build                               # esbuild bundle
npm run watch                               # Dev watch mode
```

### Full Build & Packaging (from `bazel-jdt-bridge/`)

```bash
./scripts/build-native.sh                   # Cross-compile native libs for 5 platforms
./scripts/package-extension.sh              # Full pipeline: Rust → Maven JAR → npm → VSIX
./scripts/build-for-debug.sh                # Debug build (local platform only)
```

### E2E Tests (from `bazel-jdt-bridge/vscode-extension/`)

```bash
npm run e2e                                 # Test against simple-java-project
npm run e2e:full                            # Test against all 3 example workspaces
TEST_WORKSPACE=maven-deps-project npm run e2e  # Test against a specific workspace
```

## Architecture

Four-layer polyglot bridge: TypeScript (VS Code extension shell) → Java (Eclipse JDT.LS OSGi bundle) → Rust (core engine via JNI cdylib) → Bazel CLI.

**Data flow:** VS Code commands invoke JDT.LS workspace commands → Java `BazelCommandHandler` routes to `BazelBridge` JNI methods → Rust `jni_exports.rs` functions operate on `BazelJdtState` (held in a global registry keyed by opaque `jlong` handles) → Bazel CLI queries and aspect builds produce dependency/classpath data.

**Classpath format** (Rust→Java, pipe-delimited): `TYPE|path|sourceAttachmentPath|isTest|isExported|accessRules` where TYPE is `LIB`, `PROJ`, or `SRC`.

### Rust Crates (`bazel-jdt-bridge/crates/`)

| Crate | Role |
|-------|------|
| `bazel-jdt-core` | JNI entry point (cdylib), state management, file watcher, change detection |
| `bazel-graph` | Dependency graph (petgraph) and classpath computation |
| `bazel-parser` | BUILD file parsing via starlark_syntax |
| `bazel-aspect` | Bazel IntelliJ aspect text proto output parsing |
| `bazel-query` | Bazel CLI invocation and output parsing |
| `bazel-cache` | Persistent KV cache via redb |

`bazel-jdt-core` depends on all other crates. `bazel-graph` depends on `bazel-parser` and `bazel-aspect`. No other cross-crate dependencies.

### Classpath Resolution Paths

1. **Fast** — cache hit from redb (instant)
2. **Medium** — local BUILD file parsing + graph BFS (no Bazel invocation)
3. **Slow** — `bazel build --aspects` for full artifact discovery, then cache

### Incremental Sync

File watcher (notify crate, 500ms debounce) monitors BUILD files → change detector compares SHA-256 hashes → `update_from_parsed()` surgically updates graph → `reverse_transitive_deps()` cascades invalidation → affected classpaths recomputed.

## Conventions

- **Rust**: Edition 2021, MSRV 1.75, resolver v2. Clippy warnings are fatal. Default rustfmt config.
- **Java**: JDK 17, Maven 3.8+, OSGi bundle (bnd-maven-plugin). Eclipse JDT.LS deps are provided-scope.
- **TypeScript**: ES2022, Node16 modules, strict mode, esbuild bundler.
- **CI**: Lives in `bazel-jdt-bridge/.github/workflows/` (not repo root). `ci.yml` runs fmt → clippy → cargo test → maven build/test. `release.yml` cross-compiles via cargo-zigbuild for 5 platforms.
- **Native lib loading**: `NativeLoader.java` extracts platform-specific lib from JAR at runtime. Platform detection via `PlatformDetector.java`.
- **Bazel CLI calls**: Use synchronous C `system()` instead of Rust `Command` to avoid EBADF errors from JVM fd exhaustion.

Behavioral guidelines to reduce common LLM coding mistakes. Merge with project-specific instructions as needed.

**Tradeoff:** These guidelines bias toward caution over speed. For trivial tasks, use judgment.

## 1. Think Before Coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

Before implementing:
- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them - don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

## 2. Simplicity First

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.

## 3. Surgical Changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:
- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it - don't delete it.

When your changes create orphans:
- Remove imports/variables/functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: Every changed line should trace directly to the user's request.

## 4. Goal-Driven Execution

**Define success criteria. Loop until verified.**

Transform tasks into verifiable goals:
- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:
```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

---

**These guidelines are working if:** fewer unnecessary changes in diffs, fewer rewrites due to overcomplication, and clarifying questions come before implementation rather than after mistakes.