# Bazel JDT Bridge

## Overview

Bazel JDT Bridge is a VS Code extension that provides full IDE support for Java development in Bazel workspaces.

Bazel is a high-performance build system, but it has significant shortcomings in Java IDE integration. When developers open a Bazel workspace, they face a "bare" editor with no code completion, no go-to-definition, and no dependency hints. Bazel JDT Bridge fills this gap by bridging Bazel's build information to the Eclipse JDT Language Server, allowing VS Code to handle Bazel Java projects just like Maven/Gradle projects.

**Core Features:**

- **Code Completion**: Accurate class name, method, and field completion based on full classpath information
- **Code Navigation**: Support for Go to Definition, Find References, and other navigation operations
- **Dependency Resolution**: Build a complete Java dependency graph through Bazel CLI and BUILD file parsing
- **Real-time Sync**: Monitor BUILD file changes, automatically trigger incremental sync, and keep classpath consistent with the workspace
- **Smart Caching**: Persistent KV storage based on redb, distinguishing between fast and slow paths to reduce unnecessary Bazel invocations

**Project Directory Structure:**

```
spec-kit-project/
├── bazel-jdt-bridge/         # Main application (Rust + Java + TypeScript)
│   ├── crates/               # 6 Rust workspace crates
│   │   ├── bazel-parser/     # Starlark/BUILD file parsing
│   │   ├── bazel-aspect/     # Bazel aspect text_proto parsing
│   │   ├── bazel-query/      # Bazel CLI async query
│   │   ├── bazel-graph/      # Dependency graph + classpath computation
│   │   ├── bazel-cache/      # redb persistent KV cache
│   │   └── bazel-jdt-core/   # JNI bridge (cdylib)
│   ├── java-bridge/          # Eclipse JDT.LS OSGi Bundle (Maven, Java 17)
│   ├── vscode-extension/     # VS Code extension UI (TypeScript, esbuild)
│   └── scripts/              # Cross-platform build and packaging scripts
├── .claude/commands/         # SpecKit AI-assisted development commands
├── .opencode/                # OpenCode AI configuration
└── openspec/                 # Spec-driven development configuration
```

## Architecture

### Four-Layer Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    VS Code Extension                      │
│                  (TypeScript Shell)                       │
│         commands.ts / statusBar.ts / config.ts            │
└──────────────────────┬──────────────────────────────────┘
                       │ vscode.commands.executeCommand
                       │ ('java.execute.workspaceCommand')
┌──────────────────────▼──────────────────────────────────┐
│                  Java OSGi Bundle                         │
│           (Eclipse JDT.LS Integration)                    │
│    BazelBridge / BazelProjectImporter /                    │
│    BazelClasspathManager / BazelCommandHandler             │
└──────────────────────┬──────────────────────────────────┘
                       │ JNI (6 native methods)
                       │ pipe-delimited classpath format
┌──────────────────────▼──────────────────────────────────┐
│                   Rust Core Engine                        │
│              (cdylib via JNI)                             │
│   bazel-parser / bazel-query / bazel-graph /              │
│   bazel-aspect / bazel-cache / bazel-jdt-core             │
└──────────────────────┬──────────────────────────────────┘
                       │ tokio async subprocess
                       │ bazel query / bazel build --aspects
┌──────────────────────▼──────────────────────────────────┐
│                    Bazel CLI                              │
│              (Build System)                               │
└─────────────────────────────────────────────────────────┘
```

**TypeScript Shell** is the top layer, responsible for VS Code integration. It registers 3 commands (import/sync/cleanCache), manages status bar polling, and reads user configuration. This layer contains no business logic — all requests are forwarded to the Java layer via `java.execute.workspaceCommand`.

**Java OSGi Bundle** is the middle bridge, interfacing with Eclipse JDT.LS's extension point system. It manages the JNI lifecycle and translates between JDT's `IClasspathContainer` model and Rust's pipe-delimited format. It consists of 7 Java classes running in OSGi singleton mode.

**Rust Core Engine** is the heart of the project, carrying all business logic: BUILD file parsing, Bazel CLI invocation, dependency graph construction, classpath computation, persistent caching, and file change monitoring. It is composed of 6 crates.

**Bazel CLI** is the bottom layer, serving as the source of truth for build targets and artifact paths.

### End-to-End Data Flow

1. VS Code opens a workspace, JDT.LS detects a `WORKSPACE` file, and loads the OSGi bundle
2. `BazelProjectImporter` triggers the import flow, calling JNI `nativeInitialize()` to create a `BazelJdtState`
3. `nativeDiscoverTargets()` executes `bazel query` to get all Java target labels, returning `String[]`
4. For each target, `nativeComputeClasspath()` is called, following this resolution chain:
   - **Fast Path**: Check the redb cache. Return immediately on hit without invoking Bazel
   - **Medium Path**: On cache miss, compute classpath via BUILD file parsing + dependency graph BFS (petgraph)
   - **Slow Path**: When graph data is insufficient, execute `bazel build --aspects` to trigger IntelliJ aspects for full resolution, then cache the result
5. The Java side parses pipe-delimited classpath entries into JDT's `IClasspathEntry[]`, which JDT.LS uses to provide code completion and navigation

Classpath data format (Rust to Java):

```
TYPE|path|sourceAttachmentPath|isTest|isExported|accessRules
```

Where TYPE is `LIB`, `PROJ`, or `SRC`.

### Rust Crate Dependencies

```
bazel-jdt-core (cdylib, JNI entry point)
├── bazel-parser (Starlark parsing, starlark_syntax)
├── bazel-aspect (text_proto parsing)
├── bazel-query (async Bazel CLI, tokio)
│   └── bazel-aspect
├── bazel-graph (dependency graph + classpath, petgraph)
│   └── bazel-aspect
└── bazel-cache (redb persistent storage)
```

Crate responsibilities:

| Crate | Responsibility | Key Dependencies |
|-------|---------------|------------------|
| `bazel-parser` | Parse Starlark syntax and BUILD files, extract Java rules | `starlark_syntax` |
| `bazel-aspect` | Parse Bazel aspect output in text_proto format | `serde`, `serde_json` |
| `bazel-query` | Asynchronously invoke `bazel query` commands and parse output | `tokio` |
| `bazel-graph` | Build petgraph dependency graph, compute classpath via BFS | `petgraph`, `bazel-aspect` |
| `bazel-cache` | Persistent KV storage with redb, manage cache reads/writes and invalidation | `redb`, `sha2` |
| `bazel-jdt-core` | JNI FFI boundary, global state, file watching, change detection | All of the above + `jni`, `notify` |

### Cache Architecture

The cache is built on redb (a Rust ACID KV database) and maintains two tables:

- **classpath table**: target label as key, serialized classpath JSON as value
- **build_hash table**: BUILD file path as key, SHA-256 hash as value

Cache invalidation operates at target granularity. When the file watcher detects a BUILD file change, it compares hashes to determine which targets are affected and only recomputes classpaths for those targets. Users can also manually clear all caches via the `Bazel: Clean Cache` command.

## Environment Setup

### Prerequisites

| Tool | Minimum Version | Purpose |
|------|----------------|---------|
| Rust (cargo) | 1.75+ | Build the native engine |
| Java JDK | 17 | Compile the OSGi Bundle |
| Maven | 3.8+ | Java build management |
| Node.js | 18+ | Build the VS Code extension |
| npm | 9+ | JS dependency management |

Verify your environment:

```bash
rustc --version    # Requires >= 1.75
java -version      # Requires JDK 17
mvn -version       # Requires >= 3.8
node --version     # Requires >= 18
npm --version      # Requires >= 9
```

### Cross-Platform Compilation Dependencies (Optional)

If you need to compile native libraries for platforms other than your host, install the following tools:

```bash
# Install Zig toolchain and cargo-zigbuild
pip install ziglang cargo-zigbuild
```

If you only need to build for your current platform, standard `cargo` is sufficient — no additional dependencies required.

## Build & Package

### Local Development Build

Execute in the following order to build for your current platform:

```bash
# 1. Build the Rust native library
cd bazel-jdt-bridge
cargo build -p bazel-jdt-core --release

# 2. Build the Java OSGi Bundle
cd java-bridge
mvn clean package -DskipTests

# 3. Build the VS Code extension
cd ../vscode-extension
npm install
npm run build
```

Build order matters: the Rust native library must be built before Java tests can run, because Java tests load `.so`/`.dylib`/`.dll` via JNI.

### Testing

```bash
# Rust unit tests + integration tests (38 tests)
cd bazel-jdt-bridge
cargo test --workspace

# Java tests (requires Rust native library to be built first)
cd java-bridge
mvn test -Djava.library.path=../target/release

# Rust linting
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

### E2E Tests

E2E tests run in a real VS Code Extension Development Host (EDH), validating the complete flow from extension activation to code completion.

**Prerequisites:** Rust native library + Java OSGi Bundle + TypeScript must all be built.

```bash
# 1. Complete the full build first
cd bazel-jdt-bridge
cargo build -p bazel-jdt-core --release
cd java-bridge && mvn clean package -DskipTests
cd ../vscode-extension && npm install && npm run build

# 2. Run E2E tests (defaults to simple-java-project)
cd bazel-jdt-bridge/vscode-extension
npm run e2e

# 3. Run full E2E tests (all 3 workspaces)
npm run e2e:full

# 4. Test a specific workspace
TEST_WORKSPACE=maven-deps-project npm run e2e
TEST_WORKSPACE=multi-module-project npm run e2e
```

**Test Matrix:**

| Workspace | What It Verifies |
|-----------|-----------------|
| `simple-java-project` | Extension activation, basic completion, Greeter class resolution |
| `maven-deps-project` | External dependency completion (Guava, JUnit) |
| `multi-module-project` | Transitive dependency exports, resources |

**Layered Testing Strategy:**

| Change Type | Command to Run | Estimated Time |
|------------|---------------|----------------|
| Rust changes | `cargo test --workspace` | ~5s |
| Java changes | `mvn test` | ~10s |
| TS changes | `npm run e2e` | ~2min |

### Cross-Platform Release Build

```bash
cd bazel-jdt-bridge

# Cross-compile native libraries for 5 target platforms
./scripts/build-native.sh

# Package as VSIX (includes all platform native libraries)
./scripts/package-extension.sh
```

Supported target platforms:

| Target Platform | Artifact |
|----------------|----------|
| `x86_64-unknown-linux-gnu` | `libbazel_jdt_core.so` |
| `aarch64-unknown-linux-gnu` | `libbazel_jdt_core.so` |
| `x86_64-apple-darwin` | `libbazel_jdt_core.dylib` |
| `aarch64-apple-darwin` | `libbazel_jdt_core.dylib` |
| `x86_64-pc-windows-gnu` | `bazel_jdt_core.dll` |

### Build Artifact Chain

```
Rust (cdylib)  →  Java (OSGi JAR)  →  TypeScript (esbuild bundle)  →  VSIX
.so/.dylib/.dll    com.bazel.jdt.jar    dist/extension.js              bazel-jdt-bridge-0.1.0.vsix
```

Native libraries are packaged into the JAR via OSGi's `Bundle-NativeCode` declaration, organized in a `native/<platform>/` directory structure. The `package-extension.sh` script places the JAR into the VS Code extension's `server/` directory, then packages it into a VSIX using `@vscode/vsce`.

### Installing the Extension

```bash
code --install-extension build/bazel-jdt-bridge-0.1.0.vsix
```

After installation, the extension automatically activates when opening a Java project containing a `WORKSPACE` or `WORKSPACE.bazel` file. Once activated, the following commands are available via the Command Palette:

- `Bazel: Import Project`: Import the Bazel workspace and build the full classpath
- `Bazel: Sync Project`: Incremental sync to update changed dependency information
- `Bazel: Clean Cache`: Clear the cache, forcing a full recomputation on the next request

The extension provides 3 configuration options (search for "Bazel JDT Bridge" in VS Code settings):

- `bazel-jdt.bazelPath`: Path to the Bazel executable, defaults to `bazel`
- `bazel-jdt.syncOnSave`: Automatically sync when saving BUILD files, enabled by default
- `bazel-jdt.cacheDir`: Cache directory, defaults to empty (uses system temp directory)
