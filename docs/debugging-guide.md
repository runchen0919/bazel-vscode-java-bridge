# Bazel JDT Bridge — Full-Stack Debugging Guide

This document covers the complete debugging methods for the Bazel JDT Bridge project across its four-layer architecture: TypeScript → Java (OSGi/JDT.LS) → Rust (JNI/cdylib) → Bazel CLI.

---

## Table of Contents

1. [Architecture Overview & Debug Entry Points](#1-architecture-overview--debug-entry-points)
2. [Environment Setup](#2-environment-setup)
3. [Rust Layer Debugging](#3-rust-layer-debugging)
4. [Java Layer Debugging](#4-java-layer-debugging)
5. [TypeScript / VS Code Extension Debugging](#5-typescript--vs-code-extension-debugging)
6. [JNI Cross-Language Debugging (Full Stack)](#6-jni-cross-language-debugging-full-stack)
7. [Recommended VS Code Debug Configurations](#7-recommended-vs-code-debug-configurations)
8. [Logging System Configuration](#8-logging-system-configuration)
9. [JVM Crash & Panic Debugging](#9-jvm-crash--panic-debugging)
10. [Known Issues & Debugging Pitfalls](#10-known-issues--debugging-pitfalls)
11. [Common Debugging Scenarios](#11-common-debugging-scenarios)
12. [Appendix: File Quick Reference](#12-appendix-file-quick-reference)

---

## 1. Architecture Overview & Debug Entry Points

```
┌──────────────────────────────────────────────────────────────────┐
│                     VS Code Extension (TypeScript)                │
│                  extension.ts / commands.ts / statusBar.ts        │
│   Debug entry: F5 to launch Extension Development Host           │
└────────────────────────┬─────────────────────────────────────────┘
                         │ vscode.commands.executeCommand
                         │ ('java.execute.workspaceCommand')
┌────────────────────────▼─────────────────────────────────────────┐
│                     Java OSGi Bundle (JDT.LS Integration)        │
│   BazelCommandHandler → BazelBridge → BazelClasspathManager      │
│   Debug entry: Attach to JDT.LS Java Process (port 5005)        │
└────────────────────────┬─────────────────────────────────────────┘
                         │ JNI (6 native methods, jlong handle)
┌────────────────────────▼─────────────────────────────────────────┐
│                     Rust Core Engine (cdylib)                     │
│   jni_exports.rs → state.rs → {parser, query, graph, cache}     │
│   Debug entry: LLDB/GDB attach to java process, break on cdylib │
└────────────────────────┬─────────────────────────────────────────┘
                         │ tokio async subprocess
┌────────────────────────▼─────────────────────────────────────────┐
│                     Bazel CLI (subprocess)                        │
│   Debug entry: manually run bazel query / bazel build commands   │
└──────────────────────────────────────────────────────────────────┘
```

### Debugging Tools by Layer

| Layer | Language | Debugger | Primary Breakpoint Locations |
|-------|----------|----------|------------------------------|
| VS Code Extension | TypeScript | Chrome DevTools / VS Code JS Debug | `extension.ts`, `commands.ts`, `statusBar.ts` |
| Java OSGi Bundle | Java 17 | JDWP (Java Debug Wire Protocol) | `BazelBridge.java`, `BazelClasspathManager.java`, `BazelProjectImporter.java` |
| Rust Core Engine | Rust | LLDB / GDB | `jni_exports.rs`, `state.rs`, `classpath.rs` |
| Bazel CLI | Shell | Logging / manual execution | Run `bazel query` directly in terminal |

### JNI Boundary Interface (6 Functions)

| # | Rust Function | Java Declaration | Purpose |
|---|---------------|-----------------|---------|
| 1 | `nativeInitialize` | `private native long nativeInitialize(String, String, String)` | Create `BazelJdtState`, start file watching, load cache |
| 2 | `nativeShutdown` | `private native void nativeShutdown(long)` | Release state, stop watching |
| 3 | `nativeDiscoverTargets` | `private native String[] nativeDiscoverTargets(long)` | Execute `bazel query` to get Java targets |
| 4 | `nativeComputeClasspath` | `private native String[] nativeComputeClasspath(long, String)` | Cache-first → graph BFS → full aspect resolution |
| 5 | `nativeGetSyncState` | `private native int nativeGetSyncState(long)` | Return sync state (0=Idle, 1=Syncing, 2=Error) |
| 6 | `nativeCleanCache` | `private native void nativeCleanCache(long)` | Clear the redb cache |

---

## 2. Environment Setup

### 2.1 Prerequisites

```bash
rustc --version    # >= 1.75
java -version      # JDK 17
mvn -version       # >= 3.8
node --version     # >= 18
npm --version      # >= 9
bazel --version    # any stable version
```

### 2.2 Required VS Code Extensions

| Extension | ID | Purpose |
|-----------|-----|---------|
| Red Hat Java Language Support | `redhat.java` | JDT.LS runtime (dependency for this project) |
| CodeLLDB | `vadimcn.vscode-lldb` | Rust native debugging |
| rust-analyzer | `rust-lang.rust-analyzer` | Rust language service |
| Extension Development Host | Built-in | VS Code extension debugging |

### 2.3 Debug Build Configuration

**Important:** All project build scripts default to `--release` (no debug symbols). You must manually build a debug version for debugging.

```bash
# Build debug version of Rust native library (with full debug symbols)
cd bazel-jdt-bridge
cargo build -p bazel-jdt-core
# Output: target/debug/libbazel_jdt_core.so (Linux)
#         target/debug/libbazel_jdt_core.dylib (macOS)
#         target/debug/bazel_jdt_core.dll (Windows)
```

> **Note:** The current `Cargo.toml` has no custom `[profile.*]` sections, using all Cargo defaults.
> The debug profile includes full debug info by default (`debug = true`, `opt-level = 0`).

### 2.4 Retaining Debug Symbols in Release (Optional)

If you need debug symbols in release builds, add to `Cargo.toml`:

```toml
# bazel-jdt-bridge/Cargo.toml
[profile.release]
debug = 2       # Retain full debug symbols
```

---

## 3. Rust Layer Debugging

### 3.1 Unit Tests

The project has 15 inline unit tests distributed across 4 crates:

```bash
cd bazel-jdt-bridge

# Run all Rust tests
cargo test --workspace

# Run tests for a specific crate
cargo test -p bazel-aspect    # 6 tests: text_proto parsing
cargo test -p bazel-query     # 3 tests: output parsing
cargo test -p bazel-jdt-core  # 6 tests: file watching + change detection

# View test output (println!)
cargo test --workspace -- --nocapture

# Run a specific test
cargo test -p bazel-aspect test_simple_target
```

### 3.2 Debugging Rust Tests in VS Code

With the `rust-analyzer` extension, "Run | Debug" click buttons appear above test functions:

```rust
// crates/bazel-aspect/src/text_proto.rs
#[cfg(test)]
mod tests {
    #[test]
    fn test_simple_target() {  // ← Click here for "Run | Debug"
        // ...
    }
}
```

Or use CodeLLDB configuration (see the `launch.json` in Section 7).

### 3.3 Clippy & Format Checks

```bash
# Format check (also runs in CI)
cargo fmt --all -- --check

# Clippy lint (warnings are fatal in CI)
cargo clippy --workspace --all-targets -- -D warnings
```

### 3.4 Rust Log Output

The project uses the `log` crate (16 call sites), but **`env_logger` is never initialized**, so all logging is a no-op by default. To enable logging:

1. Add initialization code at the beginning of the `nativeInitialize` function:

```rust
// crates/bazel-jdt-core/src/jni_exports.rs
// Add at the beginning of the nativeInitialize function body:
let _ = env_logger::Builder::from_env("RUST_LOG")
    .format_timestamp_millis(true)
    .try_init();
```

2. Ensure `crates/bazel-jdt-core/Cargo.toml` includes the dependency:

```toml
[dependencies]
env_logger = { workspace = true }
```

3. Set environment variables when launching JDT.LS:

```bash
RUST_LOG=bazel_jdt_core=debug code /path/to/workspace
# Or more fine-grained:
RUST_LOG=bazel_jdt_core::jni_exports=trace,bazel_jdt_core::watcher=debug code /path/to/workspace
```

### 3.5 Rust Backtrace

```bash
# Enable full backtrace
RUST_BACKTRACE=full code /path/to/workspace

# Show backtrace only on panic
RUST_BACKTRACE=1 cargo test --workspace
```

### 3.6 Debugging cdylib Directly (Without Java)

Sometimes you need to test Rust logic without launching the JNI environment. You can write tests:

```bash
# Test specific crate's pure logic (no JNI needed)
cargo test -p bazel-parser   # BUILD file parsing
cargo test -p bazel-graph    # Dependency graph computation
cargo test -p bazel-cache    # Cache read/write

# Note: bazel-jdt-core's JNI functions cannot be unit-tested directly
# They must be called through the JNI bridge layer (see Section 6)
```

---

## 4. Java Layer Debugging

### 4.1 Building

```bash
cd bazel-jdt-bridge/java-bridge

# Compile (no need to run tests first)
mvn compile

# Package OSGi Bundle (skip tests)
mvn clean package -DskipTests

# Build with tests (requires Rust native library to be built first)
cd ..
cargo build -p bazel-jdt-core          # Debug build
cd java-bridge
mvn test -Djava.library.path=../target/debug

# Or use Release build
cargo build -p bazel-jdt-core --release
mvn test -Djava.library.path=../target/release
```

### 4.2 Remote Debugging the JDT.LS Java Process

This is the core method for debugging the Java layer. Attach to the JDT.LS process via JDWP protocol:

#### Step 1: Configure JDT.LS JVM Arguments

Add to VS Code's `settings.json`:

```jsonc
// .vscode/settings.json or global settings.json
{
  // Launch JDT.LS in debug mode
  "java.jdt.ls.vmargs": "-agentlib:jdwp=transport=dt_socket,server=y,suspend=n,address=*:5005"
}
```

- `suspend=n`: Don't pause waiting for debugger; JDT.LS starts normally
- `suspend=y`: Pause and wait for debugger to connect (use when debugging initialization)

#### Step 2: Attach in VS Code

1. Open VS Code's "Run and Debug" panel
2. Select "Attach to Java Process" or use the configuration (see Section 7)
3. Connect to `localhost:5005`
4. Set breakpoints in Java source code

#### Step 3: Trigger Breakpoints

Open a Bazel project containing a `WORKSPACE` file. JDT.LS will automatically activate the Bazel extension:

```
Extension activation → extension.ts:activate()
  → java.execute.workspaceCommand('bazel-jdt.importProject')
  → BazelCommandHandler.handleImportProject()      ← Breakpoint
    → BazelBridge.initialize()                      ← Breakpoint
    → BazelBridge.discoverTargets()                 ← Breakpoint
      → nativeDiscoverTargets(handle)               ← JNI boundary
    → BazelClasspathManager.refreshClasspath()      ← Breakpoint
```

### 4.3 Key Java Breakpoint Locations

| File | Line Range | Purpose |
|------|-----------|---------|
| `BazelBridge.java:21` | `nativeInitialize()` call | Verify JNI argument passing |
| `BazelBridge.java:33` | `nativeDiscoverTargets(handle)` | Verify JNI handle is valid |
| `BazelCommandHandler.java:15-23` | switch routing | Verify command dispatch |
| `BazelProjectImporter.java:32` | `bridge.initialize()` | Project import entry point |
| `BazelProjectImporter.java:36` | `bridge.discoverTargets()` | Target discovery |
| `BazelClasspathManager.java` | `setClasspathContainer()` / `refreshClasspath()` | Classpath container operations |
| `NativeLoader.java:27` | `getResourceAsStream()` | Native library loading |

### 4.4 Debugging Java Native Library Loading Path

When native library loading fails, set breakpoints at `NativeLoader.java:23-41` and check:

1. The platform string returned by `detectPlatform()`
2. Whether `resourcePath` is correct (`/native/<platform>/<lib>`)
3. Whether `getResourceAsStream()` returns null (resource not found)
4. If it falls back to `System.loadLibrary()`, check `java.library.path`

```java
// NativeLoader.java key debug points
String platform = detectPlatform();                    // e.g., "linux-x86_64"
String resourcePath = "/native/" + platform + "/" + libFileName;  // Full path
InputStream is = NativeLoader.class.getResourceAsStream(resourcePath);  // Is it null?
```

> **Known Bug**: `NativeLoader.detectOs()` returns `"macos"`, but `bnd.bnd` and `build-native.sh` use `"darwin"`.
> This means macOS JAR resource loading will always fail (can't find `/native/macos-x86_64/...`), falling back to `System.loadLibrary()`.

### 4.5 Inspecting the OSGi Bundle

```bash
# View MANIFEST.MF inside JAR (verify Bundle-NativeCode declaration)
cd bazel-jdt-bridge/java-bridge
jar xf target/bazel-jdt-bridge-0.1.0.jar META-INF/MANIFEST.MF
cat META-INF/MANIFEST.MF

# Check if native libraries are correctly packaged in JAR
jar tf target/bazel-jdt-bridge-0.1.0.jar | grep native/
# Expected output:
#   native/linux-x86_64/libbazel_jdt_core.so
#   native/darwin-x86_64/libbazel_jdt_core.dylib
#   native/windows-x86_64/bazel_jdt_core.dll
#   ...
```

---

## 5. TypeScript / VS Code Extension Debugging

### 5.1 Extension Development Host Debugging

This is the standard method for debugging the TypeScript layer.

#### Prerequisites

```bash
cd bazel-jdt-bridge/vscode-extension
npm install
npm run build    # or npm run watch (auto-recompile)
```

#### Debug Steps

1. Open the `bazel-jdt-bridge/vscode-extension/` directory in VS Code
2. Press `F5` to launch Extension Development Host (new window)
3. In the new window, open a Bazel project containing a `WORKSPACE` file
4. The extension activates automatically and breakpoints are hit

> **Note:** The project currently has no `.vscode/launch.json`. You need to create one. See the recommended configuration in Section 7.

### 5.2 TypeScript Source Maps

`tsconfig.json` has `"sourceMap": true` enabled, but the `esbuild` command doesn't include the `--sourcemap` flag. During debugging you'll see bundled code.

Fix: Add `--sourcemap` to `package.json`'s `scripts.build`:

```jsonc
// package.json scripts
{
  "build": "esbuild src/extension.ts --bundle --outfile=dist/extension.js --external:vscode --format=cjs --platform=node --target=node18 --sourcemap",
  "watch": "esbuild src/extension.ts --bundle --outfile=dist/extension.js --external:vscode --format=cjs --platform=node --target=node18 --watch --sourcemap"
}
```

### 5.3 Key TypeScript Breakpoint Locations

| File | Line | Purpose |
|------|------|---------|
| `extension.ts:22` | `executeCommand('java.execute.workspaceCommand', ...)` | Verify command call arguments |
| `extension.ts:30` | `catch (error)` | Catch import failures |
| `commands.ts` | Command registration and execution | Verify command dispatch |
| `statusBar.ts` | 2-second polling | Verify sync status |
| `config.ts` | `getConfig()` | Verify configuration reading |

### 5.4 Extension Output Panel

VS Code's "Output" panel can show "Bazel JDT Bridge" extension output (if any). Currently the extension has no `console.log` output.

### 5.5 Developer Tools

In the Extension Development Host window, press `Ctrl+Shift+I` (Windows/Linux) or `Cmd+Option+I` (macOS) to open Developer Tools and view the Console panel.

---

## 6. JNI Cross-Language Debugging (Full Stack)

### 6.1 Concept: Debugging Java + Rust Simultaneously

The core of JNI debugging is attaching two debuggers simultaneously:
- **Java debugger** (JDWP) for the Java side
- **Native debugger** (LLDB/GDB) for the Rust side

Both debuggers attach to the **same Java process**.

### 6.2 Full-Stack Debugging Steps

#### Step 1: Build Debug Version

```bash
cd bazel-jdt-bridge

# Build Rust native library (Debug, with debug symbols)
cargo build -p bazel-jdt-core
# Output: target/debug/libbazel_jdt_core.so

# Build Java OSGi Bundle
cd java-bridge
mvn clean package -DskipTests

# Copy debug native library to JAR resource directory
# (replace the release version)
cp ../target/debug/libbazel_jdt_core.so src/main/resources/native/linux-x86_64/
# macOS:
# cp ../target/debug/libbazel_jdt_core.dylib src/main/resources/native/darwin-x86_64/

# Repackage JAR (with debug native library)
mvn clean package -DskipTests

# Build extension
cd ../vscode-extension
npm install && npm run build

# Assemble into server directory
mkdir -p server
cp ../java-bridge/target/bazel-jdt-bridge-0.1.0.jar server/com.bazel.jdt.jar
```

#### Step 2: Configure JDT.LS Launch Parameters

```jsonc
// VS Code settings.json
{
  "java.jdt.ls.vmargs": "-agentlib:jdwp=transport=dt_socket,server=y,suspend=y,address=*:5005"
}
```

> `suspend=y` pauses JDT.LS startup until you connect the debugger. This allows capturing the initialization phase.

#### Step 3: Launch VS Code

Open a Bazel project containing a `WORKSPACE` file. JDT.LS will start but pause (because of `suspend=y`).

#### Step 4: Attach Java Debugger

1. In VS Code's "Run and Debug" panel, select "Attach to Remote JDT.LS"
2. Connect to `localhost:5005`
3. Set breakpoints in `BazelBridge.java`

#### Step 5: Attach Native Debugger (LLDB)

Find the JDT.LS Java process PID:

```bash
# Linux
ps aux | grep 'java.*jdt'

# macOS
ps aux | grep 'java.*jdt'
```

Attach with LLDB:

```bash
# Linux (GDB also works)
lldb -p <PID>

# Set breakpoints in LLDB
(lldb) breakpoint set --name Java_com_bazel_jdt_BazelBridge_nativeInitialize
(lldb) breakpoint set --name Java_com_bazel_jdt_BazelBridge_nativeComputeClasspath
(lldb) continue
```

Or using GDB:

```bash
gdb -p <PID>
(gdb) break Java_com_bazel_jdt_BazelBridge_nativeInitialize
(gdb) break Java_com_bazel_jdt_BazelBridge_nativeComputeClasspath
(gdb) continue
```

#### Step 6: Debug Simultaneously

Now both debuggers are attached to the same process:

- **Java breakpoint** at `BazelBridge.java:21` (before `nativeInitialize` call)
- **Rust breakpoint** at `jni_exports.rs:25` (`Java_com_bazel_jdt_BazelBridge_nativeInitialize` entry)

Step into `nativeInitialize()` from the Java side → automatically jumps to the Rust breakpoint.

### 6.3 JNI Debugging Tools

#### JVM `-Xcheck:jni` Flag

Enable JNI argument checking to help detect type mismatches in JNI calls:

```jsonc
// VS Code settings.json
{
  "java.jdt.ls.vmargs": "-agentlib:jdwp=transport=dt_socket,server=y,suspend=n,address=*:5005 -Xcheck:jni -verbose:jni"
}
```

`-Xcheck:jni` detects: parameter type mismatches, incorrect thread usage, invalid JNI references, critical region violations.
`-verbose:jni` outputs: native library load paths, native method bindings, JNI call statistics.

#### Using `rust-gdb` / `rust-lldb`

Rust provides customized GDB/LLDB wrappers that format Rust types better:

```bash
# Linux
rust-gdb -p <PID>

# macOS
rust-lldb -p <PID>
```

#### Breakpoint Tips for Dynamic Library Loading

JVM dynamically loads cdylib via `System.loadLibrary()`. If Rust breakpoints you set in LLDB show "unresolved" (because the library isn't loaded yet), use this technique:

```lldb
# First set a breakpoint on dlopen, then set Rust breakpoints after the library is loaded
(lldb) breakpoint set --name dlopen
(lldb) continue
# After hitting, check the loaded path
(lldb) frame variable path
(lldb) continue
# Now set Rust breakpoints (symbols are loaded)
(lldb) breakpoint set --name Java_com_bazel_jdt_BazelBridge_nativeInitialize
```

### 6.4 JNI Handle Safety

The current JNI handle implementation has a use-after-free risk. Be aware during debugging:

```java
// BazelBridge.java
private long handle = -1;  // -1 = uninitialized

// After nativeShutdown, handle is set to -1
// But if another thread is using the old handle value,
// the Rust side will dereference a freed pointer → Undefined Behavior
```

Debugging recommendations:
1. Add logging in the Rust implementation of `nativeShutdown`:
   ```rust
   log::warn!("Shutting down, handle={:p}", handle as *mut BazelJdtState);
   ```
2. Add logging at the entry of all JNI functions:
   ```rust
   log::debug!("nativeComputeClasspath called with handle={:p}", handle as *const BazelJdtState);
   ```

---

## 7. Recommended VS Code Debug Configurations

### 7.1 `.vscode/launch.json`

Create file `bazel-jdt-bridge/.vscode/launch.json`:

```jsonc
{
  "version": "0.2.0",
  "configurations": [
    // ========================================
    // 1. VS Code Extension Development Debug
    // ========================================
    {
      "name": "Debug Extension",
      "type": "extensionHost",
      "request": "launch",
      "args": [
        "--extensionDevelopmentPath=${workspaceFolder}/vscode-extension",
        // Open a Bazel workspace as the test project
        "--extensionTestsPath=/path/to/your/bazel/workspace"
      ],
      "outFiles": ["${workspaceFolder}/vscode-extension/dist/**/*.js"],
      "sourceMaps": true,
      "preLaunchTask": "npm: build"
    },

    // ========================================
    // 2. Attach to JDT.LS Java Process
    // ========================================
    {
      "name": "Attach to JDT.LS (Java)",
      "type": "java",
      "request": "attach",
      "hostName": "localhost",
      "port": 5005,
      "projectName": "bazel-jdt-bridge"
    },

    // ========================================
    // 3. Attach to Native Layer of Java Process (LLDB)
    //    For simultaneous Rust cdylib debugging
    // ========================================
    {
      "name": "Attach to Native (LLDB)",
      "type": "lldb",
      "request": "attach",
      "pid": "${command:pickProcess}",
      "sourceLanguages": ["rust"]
    },

    // ========================================
    // 4. Rust Unit Tests
    // ========================================
    {
      "name": "Rust Tests: All",
      "type": "lldb",
      "request": "launch",
      "cargo": {
        "args": ["test", "--workspace", "--no-run"],
        "filter": {
          "name": "bazel_jdt_core",
          "kind": "test"
        }
      },
      "sourceLanguages": ["rust"]
    },
    {
      "name": "Rust Tests: bazel-parser",
      "type": "lldb",
      "request": "launch",
      "cargo": {
        "args": ["test", "-p", "bazel-parser", "--no-run"],
        "filter": { "name": "bazel_parser", "kind": "test" }
      }
    },
    {
      "name": "Rust Tests: bazel-graph",
      "type": "lldb",
      "request": "launch",
      "cargo": {
        "args": ["test", "-p", "bazel-graph", "--no-run"],
        "filter": { "name": "bazel_graph", "kind": "test" }
      }
    },
    {
      "name": "Rust Tests: bazel-aspect",
      "type": "lldb",
      "request": "launch",
      "cargo": {
        "args": ["test", "-p", "bazel-aspect", "--no-run"],
        "filter": { "name": "bazel_aspect", "kind": "test" }
      }
    },

    // ========================================
    // 5. Maven Test (with Debug)
    // ========================================
    {
      "name": "Maven Test (Debug)",
      "type": "java",
      "request": "launch",
      "mainClass": "org.apache.maven.surefire.booter.ForkedBooter",
      "projectName": "bazel-jdt-bridge",
      "vmArgs": [
        "-Djava.library.path=${workspaceFolder}/target/debug",
        "-agentlib:jdwp=transport=dt_socket,server=y,suspend=y,address=5006"
      ]
    }
  ],

  // Compound launch: Extension + Java attach simultaneously
  "compounds": [
    {
      "name": "Full Chain: Extension + Java",
      "configurations": ["Debug Extension", "Attach to JDT.LS (Java)"],
      "stopAll": true
    }
  ]
}
```

### 7.2 `.vscode/tasks.json`

Create file `bazel-jdt-bridge/.vscode/tasks.json`:

```jsonc
{
  "version": "2.0.0",
  "tasks": [
    {
      "label": "Build Rust Native Lib (Debug)",
      "type": "shell",
      "command": "cargo",
      "args": ["build", "-p", "bazel-jdt-core"],
      "group": "build",
      "problemMatcher": "$rustc",
      "presentation": {
        "reveal": "always",
        "panel": "dedicated"
      }
    },
    {
      "label": "Build Rust Native Lib (Release)",
      "type": "shell",
      "command": "cargo",
      "args": ["build", "-p", "bazel-jdt-core", "--release"],
      "group": "build",
      "problemMatcher": "$rustc"
    },
    {
      "label": "Build Java OSGi Bundle",
      "type": "shell",
      "command": "mvn",
      "args": ["clean", "package", "-DskipTests"],
      "options": { "cwd": "${workspaceFolder}/java-bridge" },
      "group": "build",
      "problemMatcher": "$javac"
    },
    {
      "label": "Build VS Code Extension",
      "type": "shell",
      "command": "npm",
      "args": ["run", "build"],
      "options": { "cwd": "${workspaceFolder}/vscode-extension" },
      "group": "build",
      "problemMatcher": "$tsc"
    },
    {
      "label": "Build All (Debug)",
      "dependsOn": [
        "Build Rust Native Lib (Debug)",
        "Build Java OSGi Bundle",
        "Build VS Code Extension"
      ],
      "dependsOrder": "sequence",
      "group": {
        "kind": "build",
        "isDefault": true
      }
    },
    {
      "label": "Watch Extension",
      "type": "shell",
      "command": "npm",
      "args": ["run", "watch"],
      "options": { "cwd": "${workspaceFolder}/vscode-extension" },
      "isBackground": true,
      "problemMatcher": "$tsc-watch"
    },
    {
      "label": "Rust Tests",
      "type": "shell",
      "command": "cargo",
      "args": ["test", "--workspace"],
      "group": "test",
      "problemMatcher": "$rustc"
    },
    {
      "label": "Clippy Check",
      "type": "shell",
      "command": "cargo",
      "args": ["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"],
      "group": "test",
      "problemMatcher": "$rustc"
    },
    {
      "label": "Java Tests (Debug Build)",
      "type": "shell",
      "command": "mvn",
      "args": ["test", "-Djava.library.path=../target/debug"],
      "options": { "cwd": "${workspaceFolder}/java-bridge" },
      "group": "test",
      "problemMatcher": "$javac"
    }
  ]
}
```

### 7.3 `.vscode/settings.json` (For Debugging)

```jsonc
{
  // JDT.LS debug port
  "java.jdt.ls.vmargs": "-agentlib:jdwp=transport=dt_socket,server=y,suspend=n,address=*:5005 -Xcheck:jni",

  // Rust analyzer settings
  "rust-analyzer.cargo.features": "all",
  "rust-analyzer.checkOnSave.command": "clippy",

  // Bazel extension configuration (for debugging)
  "bazel-jdt.bazelPath": "bazel",
  "bazel-jdt.syncOnSave": false,
  "bazel-jdt.cacheDir": "/tmp/bazel-jdt-debug-cache"
}
```

---

## 8. Logging System Configuration

### 8.1 Current Status

| Layer | Logging Framework | Status |
|-------|-------------------|--------|
| Rust | `log` crate (16 calls) | **Not initialized** — all logging is silent |
| Java | None | **Zero logging** — 12 catch blocks are all silent |
| TypeScript | None | Only `showInformationMessage` / `showErrorMessage` |

### 8.2 Enabling Rust Logging

```bash
# Method 1: Environment variable (requires env_logger initialization in code)
RUST_LOG=debug code /path/to/workspace

# Method 2: Filter by module
RUST_LOG=bazel_jdt_core=trace,bazel_graph=debug,bazel_cache=info code /path/to/workspace

# Method 3: Error-level only
RUST_LOG=error code /path/to/workspace
```

Available log levels:

| Level | Current Usage Count | Purpose |
|-------|-------------------|---------|
| `trace` | 0 | — |
| `debug` | 1 | File watch events (watcher.rs) |
| `info` | 5 | Cache loading, target discovery, file changes |
| `warn` | 9 | Cache load failures, file watch startup failures, deserialization errors |
| `error` | 0 | — |

### 8.3 Enabling Java Logging (Recommended Addition)

The Java layer currently has no logging infrastructure. Recommended: add `java.util.logging` (zero dependencies):

```java
// Add to BazelBridge.java
import java.util.logging.Logger;
import java.util.logging.Level;

public final class BazelBridge {
    private static final Logger LOG = Logger.getLogger(BazelBridge.class.getName());

    public synchronized void initialize(String workspacePath, String bazelPath, String cacheDir) {
        LOG.log(Level.INFO, "Initializing BazelBridge: workspace={0}, bazel={1}, cache={2}",
            new Object[]{workspacePath, bazelPath, cacheDir});
        // ...
    }
}
```

### 8.4 JDT.LS's Own Logs

JDT.LS logs are typically located at:

```bash
# Linux
~/.cache/jdtls/

# macOS
~/Library/Caches/jdtls/

# Windows
%LOCALAPPDATA%\jdtls\
```

How to view: VS Code Output panel → select "Language Support for Java".

---

## 9. JVM Crash & Panic Debugging

### 9.1 Rust Panic Crossing JNI Boundary

**Core Risk**: A Rust panic crossing the JNI boundary causes **JVM process abort** (not a Java Exception, but a process crash).

```
Rust panic → unwind → crosses JNI boundary → Undefined Behavior → JVM abort (SIGABRT)
```

The `jni-rs` crate's `JNIEnv` methods internally use `std::panic::catch_unwind`, but your own JNI export function bodies are **not** wrapped.

**Solution**: Wrap each JNI export function with `catch_unwind`:

```rust
// Recommended pattern in jni_exports.rs
#[no_mangle]
pub extern "system" fn Java_com_bazel_jdt_BazelBridge_nativeComputeClasspath(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    target_label: JString,
) -> jobjectArray {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Actual logic goes here
        do_compute_classpath(&mut env, handle, target_label)
    }));

    match result {
        Ok(val) => val,
        Err(_) => {
            let _ = env.throw_new(
                "java/lang/RuntimeException",
                "Rust panic in nativeComputeClasspath (check RUST_BACKTRACE=full)"
            );
            std::ptr::null_mut()
        }
    }
}
```

### 9.2 JVM Crash (SIGSEGV/SIGABRT) Debugging

When the JVM crashes in Rust code:

#### Step 1: Find the hs_err_pid Log

JVM generates `hs_err_pid<PID>.log` files on crash (usually in the working directory or `/tmp/`).

```bash
# Find crash logs
find /tmp -name 'hs_err_pid*.log' -mmin -60 2>/dev/null
find ~ -name 'hs_err_pid*.log' -mmin -60 2>/dev/null
```

Key information in the log:
- `siginfo:` — Signal type (SIGSEGV, SIGBUS, etc.)
- `C/C++ frames:` — Native call stack (includes Rust function names if debug symbols are present)
- `Java frames:` — Java call stack

#### Step 2: Enable Core Dumps

```bash
# Linux: Enable core dumps
ulimit -c unlimited

# macOS
ulimit -c unlimited

# Analyze crash with core dump
rust-lldb -c /path/to/core -- /path/to/java
(lldb) thread backtrace all
```

#### Step 3: Use `-Xcheck:jni` + `-verbose:jni`

```jsonc
// VS Code settings.json — Full debug JVM arguments
{
  "java.jdt.ls.vmargs": "-agentlib:jdwp=transport=dt_socket,server=y,suspend=n,address=*:5005 -Xcheck:jni -verbose:jni"
}
```

What `-Xcheck:jni` detects:
- JNI function parameter type validation
- Incorrect thread usage (calling JNI from non-Java threads)
- Invalid JNI references
- JNI critical region violations

What `-verbose:jni` outputs:
- Native library load paths
- Native method bindings
- JNI call statistics

#### Step 4: Using Valgrind (Linux)

For hard-to-reproduce memory issues:

```bash
valgrind \
  --smc-check=all \           # Required: JVM JIT self-modifying code detection
  --trace-children=yes \      # Trace child processes
  --show-leak-kinds=all \
  --track-origins=yes \
  --leak-check=full \
  code /path/to/workspace
```

> **Note**: `--smc-check=all` is **required** for JVM debugging because the JVM generates code at runtime (JIT), and Valgrind must detect self-modifying code. Performance will be very slow — use for debugging only.

### 9.3 LLDB Debugging Tips for Dynamically Loaded cdylib

JVM dynamically loads cdylib via `System.loadLibrary()`. When you attach with LLDB, symbols may not be loaded yet.

**Tip: Break on `dlopen`**

```lldb
# 1. Attach to JVM process
(lldb) attach <PID>

# 2. Set breakpoint on dlopen (capture library load moment)
(lldb) breakpoint set --name dlopen
(lldb) continue

# 3. When breakpoint hits, check the loaded library path
(lldb) frame variable path
# Or (lldb) x/s $rdi   (Linux x86_64)

# 4. Continue to let the library finish loading
(lldb) continue

# 5. Now set Rust breakpoints (symbols are loaded)
(lldb) breakpoint set --name Java_com_bazel_jdt_BazelBridge_nativeInitialize
(lldb) breakpoint set --file jni_exports.rs --line 42
```

### 9.4 JDT.LS Eclipse Debug Flags

JDT.LS is built on the Eclipse platform and supports additional debug flags:

```jsonc
// VS Code settings.json
{
  "java.jdt.ls.vmargs": "...");
}

// Or add when manually launching JDT.LS:
// -Dorg.eclipse.jdt.core/debug=true
// -Dorg.eclipse.jdt.core/debug/builder=true
// -Dorg.eclipse.jdt.core/debug/compiler=true
// -Dorg.eclipse.jdt.core/debug/indexmanager=true
// -Dlog.protocol=true          // Log all LSP messages
// -Dlog.level=ALL              // Maximum log verbosity
// -consoleLog                  // OSGi console output to stdout
// -debug                       // Eclipse debug mode
```

---

## 10. Known Issues & Debugging Pitfalls

### 10.1 macOS Native Library Loading Failure

**Problem**: `NativeLoader.detectOs()` returns `"macos"`, but `bnd.bnd` and `build-native.sh` use `"darwin"` as the platform directory name.

```
NativeLoader looks for: /native/macos-x86_64/libbazel_jdt_core.dylib  ← Not found
Actual path in JAR:     /native/darwin-x86_64/libbazel_jdt_core.dylib
```

**Result**: macOS JAR resource loading always fails and falls back to `System.loadLibrary()`.

**Debug Method**:
1. Set breakpoint at `NativeLoader.java:27`
2. Check the `resourcePath` value
3. Check if `getResourceAsStream()` returns null

**Temporary workaround**: Set `java.library.path` to point to the debug build directory.

### 10.2 Silent Exception Swallowing

The Java side has **4 completely empty catch blocks** where exceptions are silently swallowed:

| File | Line | Impact |
|------|------|--------|
| `BazelClasspathManager.java` | ~28 | Classpath setup failure has no logging |
| `BazelClasspathManager.java` | ~58 | Classpath refresh failure has no logging |
| `BazelClasspathManager.java` | ~79 | File change triggered refresh failure has no logging |
| `BazelProjectImporter.java` | ~59 | Individual target project creation failure is ignored |

**Debug Method**: Add `e.printStackTrace()` or set breakpoints in these catch blocks.

### 10.3 JNI Handle Use-After-Free

```java
// BazelBridge.java
public synchronized void shutdown() {
    if (handle != -1) {
        nativeShutdown(handle);  // Rust side Box::from_raw releases memory
        handle = -1;
    }
}
```

Although the Java method is `synchronized`, the Rust-side pointer dereference has no protection:

```rust
// jni_exports.rs:135
let state = unsafe { &*(handle as *const BazelJdtState) };
// If another thread just called nativeShutdown, this pointer is dangling
```

**Debug Method**: Add delayed logging in `nativeShutdown` to confirm no concurrent calls.

### 10.4 `bazel-jdt.getSyncState` Command Not Registered

`statusBar.ts` polls `bazel-jdt.getSyncState` every 2 seconds, but `BazelCommandHandler.java`'s switch has no case for it, falling through to `default: return null`.

**Result**: Status bar polling always throws an exception (caught by catch), displaying "Bazel ✓" instead of the real status.

### 10.5 `syncOnSave` Configuration is Dead Code

`package.json` declares the `bazel-jdt.syncOnSave` configuration, and `config.ts` reads it, but **nothing actually uses this value**. There is no `onDidSaveTextDocument` listener.

### 10.6 `BazelClasspathContainerInitializer` Class Missing

`plugin.xml:14` references `com.bazel.jdt.BazelClasspathContainerInitializer`, but this class doesn't exist in the source code. At runtime, OSGi may fail to properly initialize the classpath container.

### 10.7 Release Build Pipeline Issues

| Issue | Impact |
|-------|--------|
| `build-native.sh` uses `windows-gnu`, `release.yml` uses `windows-msvc` | Windows artifact ABI incompatibility |
| `release.yml` artifact download doesn't copy to `native/` resource directory | VSIX packaging may miss native libraries |
| `package-extension.sh` uses `|| true` swallowing vsce errors | Packaging fails but script returns 0 |

---

## 11. Common Debugging Scenarios

### Scenario 1: Code Completion Not Working

```
Troubleshooting chain (outside-in):
1. VS Code Output → "Language Support for Java" → Any errors?
2. Command Palette → "Bazel: Import Project" → Did it succeed?
3. Java breakpoint: BazelClasspathManager.setClasspathContainer() → Is classpath empty?
4. Java breakpoint: BazelBridge.computeClasspath() → What did JNI return?
5. Rust log: nativeComputeClasspath → Which path was taken (cache/graph/aspect)?
6. Terminal: bazel query '//...:*' → Can Bazel discover Java targets?
```

### Scenario 2: Bazel Target Discovery Failure

```bash
# Manually verify Bazel is working
cd /path/to/bazel/workspace
bazel query '//...:*' --output=label

# Java-related only
bazel query 'kind(java_.*, //...:*)' --output=label

# Test aspect
bazel build //path/to:target --aspects=@intellij_aspect//:intellij_info.bzl%intellij_info_aspect
```

### Scenario 3: Stale Classpath Due to Caching

```
1. Command Palette → "Bazel: Clean Cache" (clear redb cache)
2. Re-run "Bazel: Import Project"
3. Or manually delete cache directory: rm -rf ~/.cache/bazel-jdt/
```

### Scenario 4: Native Library Loading Failure (UnsatisfiedLinkError)

```
1. Confirm platform: What does NativeLoader.detectPlatform() return?
2. Confirm path: Does the /native/<platform>/ directory exist in the JAR?
3. Confirm file: Is the native library file size > 0?
4. Linux extra check:
   ldd libbazel_jdt_core.so  # Any missing dynamic link dependencies?
5. Temporary workaround: Point -Djava.library.path to debug build directory
```

### Scenario 5: File Changes Not Triggering Sync

```
1. Rust log: Does watcher.rs output "Build files changed"?
2. Java breakpoint: Is BazelBuildSupport.fileChanged() being called?
3. Check notify crate's inotify watch limit:
   cat /proc/sys/fs/inotify/max_user_watches
   # If too low: sudo sysctl fs.inotify.max_user_watches=524288
```

### Scenario 6: Debugging Rust's Bazel CLI Calls

```rust
// Add logging in bazel-query/src/command.rs
log::debug!("Executing bazel command: {:?}", command);
log::debug!("Bazel output: {}", output);
```

Or manually simulate Rust's Bazel calls:

```bash
# nativeDiscoverTargets equivalent:
bazel query 'kind(java_.*, //...:*)' --output=label

# nativeComputeClasspath (aspect path) equivalent:
bazel build //path/to:target \
  --aspects=@intellij_aspect//:intellij_info.bzl%intellij_info_aspect \
  --output_groups=intellij-info-compile,intellij-info-resolve
```

---

## 12. Appendix: File Quick Reference

### Rust Core Files

| File | Purpose | Tests |
|------|---------|-------|
| `crates/bazel-jdt-core/src/jni_exports.rs` | JNI 6 export functions | 0 |
| `crates/bazel-jdt-core/src/state.rs` | Global state management | 0 |
| `crates/bazel-jdt-core/src/watcher.rs` | File change monitoring | 2 |
| `crates/bazel-jdt-core/src/change_detector.rs` | Incremental change detection | 4 |
| `crates/bazel-parser/src/` | Starlark/BUILD parsing | 0 |
| `crates/bazel-aspect/src/text_proto.rs` | text_proto parsing | 6 |
| `crates/bazel-query/src/output.rs` | Bazel output parsing | 3 |
| `crates/bazel-graph/src/classpath.rs` | Dependency graph + classpath | 0 |
| `crates/bazel-cache/src/redb_store.rs` | redb KV cache | 0 |

### Java Core Files

| File | Purpose |
|------|---------|
| `java-bridge/src/main/java/com/bazel/jdt/BazelBridge.java` | JNI singleton facade |
| `java-bridge/src/main/java/com/bazel/jdt/NativeLoader.java` | Cross-platform native library loading |
| `java-bridge/src/main/java/com/bazel/jdt/BazelCommandHandler.java` | VS Code command routing |
| `java-bridge/src/main/java/com/bazel/jdt/BazelProjectImporter.java` | JDT.LS project import |
| `java-bridge/src/main/java/com/bazel/jdt/BazelClasspathManager.java` | Classpath container management |
| `java-bridge/src/main/java/com/bazel/jdt/BazelClasspathContainer.java` | IClasspathContainer implementation |
| `java-bridge/src/main/java/com/bazel/jdt/BazelBuildSupport.java` | BUILD file change monitoring |
| `java-bridge/src/main/resources/plugin.xml` | OSGi extension point declarations |
| `java-bridge/bnd.bnd` | OSGi Bundle metadata |

### TypeScript Core Files

| File | Purpose |
|------|---------|
| `vscode-extension/src/extension.ts` | Extension entry point |
| `vscode-extension/src/commands.ts` | 3 command registrations |
| `vscode-extension/src/statusBar.ts` | Status bar polling |
| `vscode-extension/src/config.ts` | Configuration reading |
| `vscode-extension/package.json` | Extension manifest |

### Build & CI

| File | Purpose |
|------|---------|
| `Cargo.toml` | Rust workspace configuration |
| `rust-toolchain.toml` | Rust toolchain (stable) |
| `java-bridge/pom.xml` | Maven build |
| `.github/workflows/ci.yml` | CI pipeline |
| `.github/workflows/release.yml` | Release pipeline |
| `scripts/build-native.sh` | Cross-platform compilation |
| `scripts/package-extension.sh` | VSIX packaging |

---

## Quick Reference Card

```
# === Build Commands ===
cargo build -p bazel-jdt-core                    # Debug build (with symbols)
cargo build -p bazel-jdt-core --release          # Release build (no symbols)
mvn clean package -DskipTests                    # Java packaging
cd vscode-extension && npm run build             # TS build

# === Test Commands ===
cargo test --workspace                           # All Rust tests
cargo test -- --nocapture                        # Show println!
mvn test -Djava.library.path=../target/debug     # Java tests (Debug)
mvn test -Djava.library.path=../target/release   # Java tests (Release)

# === Debug Commands ===
RUST_LOG=debug RUST_BACKTRACE=full code .        # Rust logging + backtrace
java -agentlib:jdwp=... -Xcheck:jni -verbose:jni # Java remote debug + JNI check

# === Verification Commands ===
cargo fmt --all -- --check                       # Format check
cargo clippy --workspace --all-targets -- -D warnings  # Lint
jar tf target/*.jar | grep native/               # Check native library packaging

# === Crash Analysis ===
find /tmp -name 'hs_err_pid*.log' -mmin -60      # Find JVM crash logs
ulimit -c unlimited                              # Enable core dumps
rust-lldb -c /path/to/core -- /path/to/java      # Analyze core dump

# === Environment Variable Quick Reference ===
RUST_BACKTRACE=full                              # Rust full backtrace
RUST_LOG=bazel_jdt_core=debug                    # Rust module-level logging
JAVA_OPTS="-Xcheck:jni -verbose:jni"             # JVM JNI debug flags
```
