# PROJECT KNOWLEDGE BASE

**Generated:** 2026-04-22
**Commit:** 6f0ca6a
**Branch:** 001-bazel-java-resolver

## OVERVIEW

Bazel JDT Bridge — VS Code extension for Java development in Bazel workspaces. Polyglot: Rust (core engine via JNI) → Java (Eclipse JDT.LS OSGi bundle) → TypeScript (VS Code extension shell). Uses SpecKit/OpenSpec for AI-assisted spec-driven development.

## STRUCTURE

```
spec-kit-project/
├── bazel-jdt-bridge/         # PRIMARY APPLICATION (Rust + Java + TS)
│   ├── crates/               # 6 Rust workspace crates
│   ├── java-bridge/          # Eclipse JDT.LS OSGi bundle (Maven, Java 17)
│   ├── vscode-extension/     # VS Code extension UI (esbuild, TypeScript)
│   └── scripts/              # Cross-platform build + packaging scripts
├── .claude/commands/         # 9 SpecKit slash commands (Claude Code)
├── .opencode/                # 4 OpenCode commands + 4 skills
├── openspec/                 # Spec-driven config (ephemeral, gitignored)
└── docs/                     # Empty placeholder
```

## WHERE TO LOOK

| Task | Location | Notes |
|------|----------|-------|
| Add Bazel BUILD file parsing logic | `bazel-jdt-bridge/crates/bazel-parser/` | Starlark parser using starlark_syntax |
| Modify classpath computation | `bazel-jdt-bridge/crates/bazel-graph/src/classpath.rs` | petgraph-based dependency resolution |
| Change JNI interface | `bazel-jdt-bridge/crates/bazel-jdt-core/src/jni_exports.rs` | 6 `#[no_mangle] extern "system"` FFI functions |
| Fix Eclipse/JDT integration | `bazel-jdt-bridge/java-bridge/src/main/java/com/bazel/jdt/` | 7 Java classes, OSGi singleton |
| Update VS Code extension UI | `bazel-jdt-bridge/vscode-extension/src/` | 4 TS files: extension, commands, config, statusBar |
| Modify CI/CD | `bazel-jdt-bridge/.github/workflows/` | ci.yml + release.yml |
| Cross-compile native libs | `bazel-jdt-bridge/scripts/build-native.sh` | cargo-zigbuild, 5 platforms |
| AI dev workflow tooling | `.claude/commands/` or `.opencode/` | SpecKit commands/skills |

## CODE MAP

### Rust Crates (Cargo workspace, 3,089 lines)

| Crate | Role | Key Exports |
|-------|------|-------------|
| `bazel-parser` | Starlark/BUILD file parsing | `BuildFileParser`, `ParseError`, `ParsedBuildFile`, `JavaRule` |
| `bazel-aspect` | Bazel aspect output parsing | `TextProtoParser`, `TargetIdeInfo`, `JavaIdeInfo`, `JarInfo` |
| `bazel-query` | Bazel CLI query execution (async) | `BazelInvoker`, `BazelError`, `parse_label_output` |
| `bazel-graph` | Dependency graph + classpath | `DependencyGraph`, `ComputedClasspath`, `ClasspathEntry` |
| `bazel-cache` | Persistent redb KV store | `BazelCache`, `CacheError` |
| `bazel-jdt-core` | JNI bridge (cdylib) | `jni_exports` (FFI boundary), `BazelJdtState`, `SyncState` |

### Dependency Flow
```
bazel-jdt-core → {bazel-parser, bazel-aspect, bazel-query, bazel-graph, bazel-cache}
bazel-graph → bazel-parser
(No other cross-crate deps)
```

### Java Classes (488 lines, JDT.LS integration)

| Class | Role |
|-------|------|
| `BazelBridge` | Singleton entry; loads native lib, exposes JNI methods |
| `NativeLoader` | Extracts platform-specific .so/.dylib/.dll from JAR |
| `BazelProjectImporter` | Extends `AbstractProjectImporter`; triggered on WORKSPACE detection |
| `BazelClasspathManager` | Manages JDT classpath container lifecycle |
| `BazelClasspathContainer` | Implements `IClasspathContainer` |
| `BazelBuildSupport` | Bazel build support handler |
| `BazelCommandHandler` | Command routing |

## CONVENTIONS

- **Rust**: Edition 2021, MSRV 1.75, resolver v2. Clippy `-D warnings` (fatal). No custom rustfmt/clippy config — all defaults.
- **Java**: Java 17, Maven, OSGi bundle (bnd-maven-plugin), JUnit 4. Eclipse JDT.LS provided-scope deps.
- **TypeScript**: ES2022, Node16 modules, strict mode, esbuild bundler (not tsc for bundling).
- **Tests**: Inline `#[cfg(test)] mod tests` in Rust (15 tests, no dev-deps). No Java/TS tests yet.
- **Cross-compilation**: cargo-zigbuild (Zig linker) for Linux/macOS, native cargo for Windows.

## ANTI-PATTERNS (THIS PROJECT)

- **Empty catch blocks in Java**: `BazelClasspathManager` (3 places) and `BazelProjectImporter` (1 place) silently swallow exceptions
- **Dead code bug**: `BazelClasspathContainer` constructor — `if (rawEntries != null)` early-returns empty, making parsing loop unreachable (likely should be `== null`)
- **No-op method**: `classpath.rs::filter_by_visibility()` body is empty placeholder
- **JNI use-after-free risk**: No lifetime/generation tracking on JNI handle — calling after `nativeShutdown` is UB

## UNIQUE STYLES

- Triple-language bridge: TS → Java (OSGi) → Rust (cdylib/JNI). Unusual 4-layer VS Code extension architecture.
- `openspec/` directory is entirely gitignored (`*`) — specs are ephemeral working artifacts
- `.opencode/` is also gitignored — internal AI agent state
- CI lives in `bazel-jdt-bridge/.github/` not repo root (subproject-scoped)
- Release uses `cargo-zigbuild` for cross-compilation instead of cross-rs

## COMMANDS

```bash
# Rust (from bazel-jdt-bridge/)
cargo fmt --all -- --check          # Format check
cargo clippy --workspace --all-targets -- -D warnings  # Lint (fatal)
cargo test --workspace              # Run 15 inline unit tests
cargo build -p bazel-jdt-core --release  # Build JNI native lib

# Java (from bazel-jdt-bridge/java-bridge/)
mvn compile                         # Build OSGi bundle
mvn test -Djava.library.path=../target/release  # Test with native lib

# TypeScript (from bazel-jdt-bridge/vscode-extension/)
npm run build                       # esbuild bundle
npm run watch                       # Dev watch mode

# Packaging (from bazel-jdt-bridge/)
./scripts/build-native.sh           # Cross-compile 5 platforms
./scripts/package-extension.sh      # Maven JAR + npm build + VSIX
```

## NOTES

- Build order matters: Rust native lib MUST be built before Java tests can run
- `bazel-jdt-core` is `cdylib` (not `lib`) — produces `.so`/`.dylib`/`.dll`, not `.rlib`
- `bnd.bnd` declares `Bundle-NativeCode` for 5 platforms in the OSGi bundle
- No root-level Makefile — orchestration is via CI workflows and shell scripts
- Release workflow potential bug: cross-compiled native libs may not reach final VSIX (artifact flow gap)
- Windows target mismatch: `build-native.sh` uses `gnu`, `release.yml` uses `msvc`
