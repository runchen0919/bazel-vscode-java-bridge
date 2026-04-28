# Bazel JDT Bridge — 全链路调试指南

本文档覆盖 Bazel JDT Bridge 项目从 TypeScript → Java (OSGi/JDT.LS) → Rust (JNI/cdylib) → Bazel CLI 四层架构的完整调试方法。

---

## 目录

1. [架构概览与调试入口](#1-架构概览与调试入口)
2. [环境准备](#2-环境准备)
3. [Rust 层调试](#3-rust-层调试)
4. [Java 层调试](#4-java-层调试)
5. [TypeScript / VS Code 扩展调试](#5-typescript--vs-code-扩展调试)
6. [JNI 跨语言调试（全链路）](#6-jni-跨语言调试全链路)
7. [推荐的 VS Code 调试配置](#7-推荐的-vs-code-调试配置)
8. [日志系统配置](#8-日志系统配置)
9. [JVM 崩溃与 Panic 调试](#9-jvm-崩溃与-panic-调试)
10. [已知问题与调试陷阱](#10-已知问题与调试陷阱)
11. [常用调试场景](#11-常用调试场景)
12. [附录：文件速查表](#12-附录文件速查表)

---

## 1. 架构概览与调试入口

```
┌──────────────────────────────────────────────────────────────────┐
│                     VS Code Extension (TypeScript)                │
│                  extension.ts / commands.ts / statusBar.ts        │
│   调试入口: F5 启动 Extension Development Host                    │
└────────────────────────┬─────────────────────────────────────────┘
                         │ vscode.commands.executeCommand
                         │ ('java.execute.workspaceCommand')
┌────────────────────────▼─────────────────────────────────────────┐
│                     Java OSGi Bundle (JDT.LS 集成)                │
│   BazelCommandHandler → BazelBridge → BazelClasspathManager      │
│   调试入口: Attach to JDT.LS Java Process (port 5005)            │
└────────────────────────┬─────────────────────────────────────────┘
                         │ JNI (6 native methods, jlong handle)
┌────────────────────────▼─────────────────────────────────────────┐
│                     Rust Core Engine (cdylib)                     │
│   jni_exports.rs → state.rs → {parser, query, graph, cache}     │
│   调试入口: LLDB/GDB attach to java process, 断点 cdylib        │
└────────────────────────┬─────────────────────────────────────────┘
                         │ tokio async subprocess
┌────────────────────────▼─────────────────────────────────────────┐
│                     Bazel CLI (子进程)                             │
│   调试入口: 手动运行 bazel query / bazel build 命令验证           │
└──────────────────────────────────────────────────────────────────┘
```

### 四层对应的调试工具

| 层 | 语言 | 调试器 | 主要断点位置 |
|----|------|--------|-------------|
| VS Code Extension | TypeScript | Chrome DevTools / VS Code JS Debug | `extension.ts`, `commands.ts`, `statusBar.ts` |
| Java OSGi Bundle | Java 17 | JDWP (Java Debug Wire Protocol) | `BazelBridge.java`, `BazelClasspathManager.java`, `BazelProjectImporter.java` |
| Rust Core Engine | Rust | LLDB / GDB | `jni_exports.rs`, `state.rs`, `classpath.rs` |
| Bazel CLI | Shell | 日志 / 手动执行 | 终端直接运行 `bazel query` |

### JNI 边界接口（6 个函数）

| # | Rust 函数 | Java 声明 | 用途 |
|---|-----------|-----------|------|
| 1 | `nativeInitialize` | `private native long nativeInitialize(String, String, String)` | 创建 `BazelJdtState`，启动文件监听，加载缓存 |
| 2 | `nativeShutdown` | `private native void nativeShutdown(long)` | 释放状态，停止监听 |
| 3 | `nativeDiscoverTargets` | `private native String[] nativeDiscoverTargets(long)` | 执行 `bazel query` 获取 Java target |
| 4 | `nativeComputeClasspath` | `private native String[] nativeComputeClasspath(long, String)` | 缓存优先 → 图 BFS → 全量 aspect 解析 |
| 5 | `nativeGetSyncState` | `private native int nativeGetSyncState(long)` | 返回同步状态 (0=Idle, 1=Syncing, 2=Error) |
| 6 | `nativeCleanCache` | `private native void nativeCleanCache(long)` | 清空 redb 缓存 |

---

## 2. 环境准备

### 2.1 前置依赖

```bash
rustc --version    # >= 1.75
java -version      # JDK 17
mvn -version       # >= 3.8
node --version     # >= 18
npm --version      # >= 9
bazel --version    # 任意稳定版
```

### 2.2 VS Code 必装扩展

| 扩展 | ID | 用途 |
|------|-----|------|
| Red Hat Java Language Support | `redhat.java` | JDT.LS 运行时（本项目的依赖） |
| CodeLLDB | `vadimcn.vscode-lldb` | Rust 原生调试 |
| rust-analyzer | `rust-lang.rust-analyzer` | Rust 语言服务 |
| Extension Development Host | 内置 | VS Code 扩展调试 |

### 2.3 Debug 构建配置

**重要：** 项目所有构建脚本默认使用 `--release`（无调试符号）。调试时必须手动构建 debug 版本。

```bash
# 构建 Debug 版本的 Rust 原生库（包含完整调试符号）
cd bazel-jdt-bridge
cargo build -p bazel-jdt-core
# 产物: target/debug/libbazel_jdt_core.so (Linux)
#       target/debug/libbazel_jdt_core.dylib (macOS)
#       target/debug/bazel_jdt_core.dll (Windows)
```

> **注意**: 当前 `Cargo.toml` 没有自定义 `[profile.*]` 段，全部使用 Cargo 默认值。
> Debug profile 默认包含完整调试信息 (`debug = true`, `opt-level = 0`)。

### 2.4 Release 保留调试符号（可选）

如果需要在 release 构建中也保留调试符号，在 `Cargo.toml` 中添加：

```toml
# bazel-jdt-bridge/Cargo.toml
[profile.release]
debug = 2       # 保留完整调试符号
```

---

## 3. Rust 层调试

### 3.1 单元测试

项目有 15 个内联单元测试，分布在 4 个 crate 中：

```bash
cd bazel-jdt-bridge

# 运行所有 Rust 测试
cargo test --workspace

# 运行特定 crate 的测试
cargo test -p bazel-aspect    # 6 tests: text_proto 解析
cargo test -p bazel-query     # 3 tests: 输出解析
cargo test -p bazel-jdt-core  # 6 tests: 文件监听 + 变更检测

# 查看测试输出 (println!)
cargo test --workspace -- --nocapture

# 运行特定测试
cargo test -p bazel-aspect test_simple_target
```

### 3.2 在 VS Code 中调试 Rust 测试

使用 `rust-analyzer` 扩展，在测试函数上方会出现 "Run | Debug" 点击按钮：

```rust
// crates/bazel-aspect/src/text_proto.rs
#[cfg(test)]
mod tests {
    #[test]
    fn test_simple_target() {  // ← 点击这里出现 "Run | Debug"
        // ...
    }
}
```

或使用 CodeLLDB 配置（见第 7 节的 `launch.json`）。

### 3.3 Clippy & Format 检查

```bash
# 格式检查 (CI 也会运行)
cargo fmt --all -- --check

# Clippy lint (CI 中 warnings 是 fatal)
cargo clippy --workspace --all-targets -- -D warnings
```

### 3.4 Rust 日志输出

项目使用了 `log` crate（16 处调用），但 **`env_logger` 从未被初始化**，所有日志默认为空操作。要启用日志，需要：

1. 在 `nativeInitialize` 函数开头添加初始化代码：

```rust
// crates/bazel-jdt-core/src/jni_exports.rs
// 在 nativeInitialize 函数体开头添加:
let _ = env_logger::Builder::from_env("RUST_LOG")
    .format_timestamp_millis(true)
    .try_init();
```

2. 确保 `crates/bazel-jdt-core/Cargo.toml` 包含依赖：

```toml
[dependencies]
env_logger = { workspace = true }
```

3. 设置环境变量启动 JDT.LS：

```bash
RUST_LOG=bazel_jdt_core=debug code /path/to/workspace
# 或者更细粒度:
RUST_LOG=bazel_jdt_core::jni_exports=trace,bazel_jdt_core::watcher=debug code /path/to/workspace
```

### 3.5 Rust Backtrace

```bash
# 启用完整 backtrace
RUST_BACKTRACE=full code /path/to/workspace

# 仅在 panic 时显示 backtrace
RUST_BACKTRACE=1 cargo test --workspace
```

### 3.6 直接调试 cdylib（不经过 Java）

有时需要单独测试 Rust 逻辑，不启动 JNI 环境。可以编写测试：

```bash
# 测试特定 crate 的纯逻辑（不需要 JNI）
cargo test -p bazel-parser   # BUILD 文件解析
cargo test -p bazel-graph    # 依赖图计算
cargo test -p bazel-cache    # 缓存读写

# 注意：bazel-jdt-core 的 JNI 函数不能直接单元测试
# 需要通过 JNI 桥接层调用（见第 6 节）
```

---

## 4. Java 层调试

### 4.1 构建

```bash
cd bazel-jdt-bridge/java-bridge

# 编译（不需要先运行测试）
mvn compile

# 打包 OSGi Bundle（跳过测试）
mvn clean package -DskipTests

# 带测试的构建（需要先构建 Rust 原生库）
cd ..
cargo build -p bazel-jdt-core          # Debug 版本
cd java-bridge
mvn test -Djava.library.path=../target/debug

# 或使用 Release 版本
cargo build -p bazel-jdt-core --release
mvn test -Djava.library.path=../target/release
```

### 4.2 远程调试 JDT.LS Java 进程

这是调试 Java 层最核心的方法。通过 JDWP 协议 attach 到 JDT.LS 进程：

#### 步骤 1：配置 JDT.LS 的 JVM 参数

在 VS Code 的 `settings.json` 中添加：

```jsonc
// .vscode/settings.json 或全局 settings.json
{
  // 让 JDT.LS 以 debug 模式启动
  "java.jdt.ls.vmargs": "-agentlib:jdwp=transport=dt_socket,server=y,suspend=n,address=*:5005"
}
```

- `suspend=n`: 不暂停等待调试器连接，JDT.LS 正常启动
- `suspend=y`: 暂停等待调试器连接（调试初始化流程时使用）

#### 步骤 2：在 VS Code 中 Attach

1. 打开 VS Code 的 "Run and Debug" 面板
2. 选择 "Attach to Java Process" 或使用配置（见第 7 节）
3. 连接到 `localhost:5005`
4. 在 Java 源码中设置断点

#### 步骤 3：触发断点

打开一个包含 `WORKSPACE` 文件的 Bazel 项目，JDT.LS 会自动激活 Bazel 扩展：

```
扩展激活 → extension.ts:activate()
  → java.execute.workspaceCommand('bazel-jdt.importProject')
  → BazelCommandHandler.handleImportProject()      ← 断点位置
    → BazelBridge.initialize()                      ← 断点位置
    → BazelBridge.discoverTargets()                 ← 断点位置
      → nativeDiscoverTargets(handle)               ← JNI 边界
    → BazelClasspathManager.refreshClasspath()      ← 断点位置
```

### 4.3 关键 Java 断点位置

| 文件 | 行号区域 | 用途 |
|------|---------|------|
| `BazelBridge.java:21` | `nativeInitialize()` 调用 | 验证 JNI 参数传递 |
| `BazelBridge.java:33` | `nativeDiscoverTargets(handle)` | 验证 JNI handle 有效 |
| `BazelCommandHandler.java:15-23` | switch 路由 | 验证命令分发 |
| `BazelProjectImporter.java:32` | `bridge.initialize()` | 项目导入入口 |
| `BazelProjectImporter.java:36` | `bridge.discoverTargets()` | target 发现 |
| `BazelClasspathManager.java` | `setClasspathContainer()` / `refreshClasspath()` | classpath 容器操作 |
| `NativeLoader.java:27` | `getResourceAsStream()` | 原生库加载 |

### 4.4 Java 原生库加载路径调试

当原生库加载失败时，在 `NativeLoader.java:23-41` 设置断点，检查：

1. `detectPlatform()` 返回的平台字符串
2. `resourcePath` 是否正确（`/native/<platform>/<lib>`）
3. `getResourceAsStream()` 是否返回 null（资源不存在）
4. 如果回退到 `System.loadLibrary()`，检查 `java.library.path`

```java
// NativeLoader.java 关键调试点
String platform = detectPlatform();                    // 如 "linux-x86_64"
String resourcePath = "/native/" + platform + "/" + libFileName;  // 完整路径
InputStream is = NativeLoader.class.getResourceAsStream(resourcePath);  // 是否为 null?
```

> **已知 Bug**: `NativeLoader.detectOs()` 返回 `"macos"`，但 `bnd.bnd` 和 `build-native.sh` 使用 `"darwin"`。
> 这意味着 macOS 上从 JAR 资源加载会失败（找不到 `/native/macos-x86_64/...`），会回退到 `System.loadLibrary()`。

### 4.5 检查 OSGi Bundle

```bash
# 查看 JAR 内的 MANIFEST.MF（验证 Bundle-NativeCode 声明）
cd bazel-jdt-bridge/java-bridge
jar xf target/bazel-jdt-bridge-0.1.0.jar META-INF/MANIFEST.MF
cat META-INF/MANIFEST.MF

# 查看原生库是否正确打包在 JAR 中
jar tf target/bazel-jdt-bridge-0.1.0.jar | grep native/
# 预期输出:
#   native/linux-x86_64/libbazel_jdt_core.so
#   native/darwin-x86_64/libbazel_jdt_core.dylib
#   native/windows-x86_64/bazel_jdt_core.dll
#   ...
```

---

## 5. TypeScript / VS Code 扩展调试

### 5.1 Extension Development Host 调试

这是调试 TypeScript 层的标准方式。

#### 前置步骤

```bash
cd bazel-jdt-bridge/vscode-extension
npm install
npm run build    # 或 npm run watch（自动重编译）
```

#### 调试步骤

1. 在 VS Code 中打开 `bazel-jdt-bridge/vscode-extension/` 目录
2. 按 `F5` 启动 Extension Development Host（新窗口）
3. 在新窗口中打开一个包含 `WORKSPACE` 文件的 Bazel 项目
4. 扩展自动激活，断点命中

> **注意**: 项目当前没有 `.vscode/launch.json`，需要创建。见第 7 节的推荐配置。

### 5.2 TypeScript 源码映射

`tsconfig.json` 中 `"sourceMap": true` 已启用，但 `esbuild` 命令没有 `--sourcemap` 参数。调试时看到的会是打包后的代码。

修复方法：在 `package.json` 的 `scripts.build` 中添加 `--sourcemap`：

```jsonc
// package.json scripts
{
  "build": "esbuild src/extension.ts --bundle --outfile=dist/extension.js --external:vscode --format=cjs --platform=node --target=node18 --sourcemap",
  "watch": "esbuild src/extension.ts --bundle --outfile=dist/extension.js --external:vscode --format=cjs --platform=node --target=node18 --watch --sourcemap"
}
```

### 5.3 关键 TypeScript 断点位置

| 文件 | 行号 | 用途 |
|------|------|------|
| `extension.ts:22` | `executeCommand('java.execute.workspaceCommand', ...)` | 验证命令调用参数 |
| `extension.ts:30` | `catch (error)` | 捕获 import 失败 |
| `commands.ts` | 命令注册和执行 | 验证命令分发 |
| `statusBar.ts` | 2 秒轮询 | 验证同步状态 |
| `config.ts` | `getConfig()` | 验证配置读取 |

### 5.4 Extension Output 面板

VS Code 的 "Output" 面板可以选择 "Bazel JDT Bridge" 查看扩展输出（如果有的话）。当前扩展没有任何 `console.log` 输出。

### 5.5 Developer Tools

在 Extension Development Host 窗口中按 `Ctrl+Shift+I` (Windows/Linux) 或 `Cmd+Option+I` (macOS) 打开 Developer Tools，查看 Console 面板。

---

## 6. JNI 跨语言调试（全链路）

### 6.1 概念：同时调试 Java + Rust

JNI 调试的核心是同时 attach 两个调试器：
- **Java 调试器**（JDWP）调试 Java 侧
- **原生调试器**（LLDB/GDB）调试 Rust 侧

两个调试器 attach 到**同一个 Java 进程**。

### 6.2 全链路调试步骤

#### 步骤 1：构建 Debug 版本

```bash
cd bazel-jdt-bridge

# 构建 Rust 原生库（Debug，包含调试符号）
cargo build -p bazel-jdt-core
# 产物: target/debug/libbazel_jdt_core.so

# 构建 Java OSGi Bundle
cd java-bridge
mvn clean package -DskipTests

# 将 debug 版本的原生库复制到 JAR 资源目录
# （替换 release 版本）
cp ../target/debug/libbazel_jdt_core.so src/main/resources/native/linux-x86_64/
# macOS:
# cp ../target/debug/libbazel_jdt_core.dylib src/main/resources/native/darwin-x86_64/

# 重新打包 JAR（包含 debug 原生库）
mvn clean package -DskipTests

# 构建扩展
cd ../vscode-extension
npm install && npm run build

# 组装到 server 目录
mkdir -p server
cp ../java-bridge/target/bazel-jdt-bridge-0.1.0.jar server/com.bazel.jdt.jar
```

#### 步骤 2：配置 JDT.LS 启动参数

```jsonc
// VS Code settings.json
{
  "java.jdt.ls.vmargs": "-agentlib:jdwp=transport=dt_socket,server=y,suspend=y,address=*:5005"
}
```

> `suspend=y` 会暂停 JDT.LS 启动，等你连接调试器后才继续。这样可以捕获初始化阶段。

#### 步骤 3：启动 VS Code

打开一个包含 `WORKSPACE` 文件的 Bazel 项目。JDT.LS 会启动但暂停（因为 `suspend=y`）。

#### 步骤 4：Attach Java 调试器

1. 在 VS Code 的 "Run and Debug" 面板选择 "Attach to Remote JDT.LS"
2. 连接到 `localhost:5005`
3. 在 `BazelBridge.java` 中设置断点

#### 步骤 5：Attach 原生调试器（LLDB）

找到 JDT.LS 的 Java 进程 PID：

```bash
# Linux
ps aux | grep 'java.*jdt'

# macOS
ps aux | grep 'java.*jdt'
```

使用 LLDB attach：

```bash
# Linux (也可以用 GDB)
lldb -p <PID>

# 在 LLDB 中设置断点
(lldb) breakpoint set --name Java_com_bazel_jdt_BazelBridge_nativeInitialize
(lldb) breakpoint set --name Java_com_bazel_jdt_BazelBridge_nativeComputeClasspath
(lldb) continue
```

或者使用 GDB：

```bash
gdb -p <PID>
(gdb) break Java_com_bazel_jdt_BazelBridge_nativeInitialize
(gdb) break Java_com_bazel_jdt_BazelBridge_nativeComputeClasspath
(gdb) continue
```

#### 步骤 6：同时调试

现在两个调试器都 attach 到同一个进程：

- **Java 断点** 在 `BazelBridge.java:21` (`nativeInitialize` 调用前)
- **Rust 断点** 在 `jni_exports.rs:25` (`Java_com_bazel_jdt_BazelBridge_nativeInitialize` 入口)

从 Java 侧 step into `nativeInitialize()` → 自动跳转到 Rust 侧断点。

### 6.3 JNI 调试工具

#### JVM `-Xcheck:jni` 标志

启用 JNI 参数检查，帮助发现 JNI 调用中的类型不匹配等问题：

```jsonc
// VS Code settings.json
{
  "java.jdt.ls.vmargs": "-agentlib:jdwp=transport=dt_socket,server=y,suspend=n,address=*:5005 -Xcheck:jni -verbose:jni"
}
```

`-Xcheck:jni` 检测：参数类型不匹配、错误线程使用、无效 JNI 引用、critical 区域违规。
`-verbose:jni` 输出：原生库加载路径、native 方法绑定、JNI 调用统计。

#### 使用 `rust-gdb` / `rust-lldb`

Rust 提供了定制的 GDB/LLDB 包装器，能更好地格式化 Rust 类型：

```bash
# Linux
rust-gdb -p <PID>

# macOS
rust-lldb -p <PID>
```

#### 动态库加载时的断点技巧

JVM 通过 `System.loadLibrary()` 动态加载 cdylib。如果你在 LLDB 中设置的 Rust 断点显示 "unresolved"（因为库还没加载），使用以下技巧：

```lldb
# 先在 dlopen 设断点，等库加载完成后再设 Rust 断点
(lldb) breakpoint set --name dlopen
(lldb) continue
# 命中后检查加载的路径
(lldb) frame variable path
(lldb) continue
# 现在设置 Rust 断点（符号已加载）
(lldb) breakpoint set --name Java_com_bazel_jdt_BazelBridge_nativeInitialize
```

### 6.4 JNI Handle 安全性

当前 JNI handle 实现存在 use-after-free 风险。调试时注意：

```java
// BazelBridge.java
private long handle = -1;  // -1 = 未初始化

// nativeShutdown 后 handle 被设为 -1
// 但如果其他线程正在使用旧的 handle 值，
// Rust 侧会解引用已释放的指针 → Undefined Behavior
```

调试建议：
1. 在 `nativeShutdown` 的 Rust 实现中添加日志：
   ```rust
   log::warn!("Shutting down, handle={:p}", handle as *mut BazelJdtState);
   ```
2. 在所有 JNI 函数入口添加日志：
   ```rust
   log::debug!("nativeComputeClasspath called with handle={:p}", handle as *const BazelJdtState);
   ```

---

## 7. 推荐的 VS Code 调试配置

### 7.1 `.vscode/launch.json`

创建文件 `bazel-jdt-bridge/.vscode/launch.json`：

```jsonc
{
  "version": "0.2.0",
  "configurations": [
    // ========================================
    // 1. VS Code Extension 开发调试
    // ========================================
    {
      "name": "Debug Extension",
      "type": "extensionHost",
      "request": "launch",
      "args": [
        "--extensionDevelopmentPath=${workspaceFolder}/vscode-extension",
        // 打开一个 Bazel 工作空间作为测试项目
        "--extensionTestsPath=/path/to/your/bazel/workspace"
      ],
      "outFiles": ["${workspaceFolder}/vscode-extension/dist/**/*.js"],
      "sourceMaps": true,
      "preLaunchTask": "npm: build"
    },

    // ========================================
    // 2. Attach 到 JDT.LS Java 进程
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
    // 3. Attach 到 Java 进程的原生层 (LLDB)
    //    用于同时调试 Rust cdylib
    // ========================================
    {
      "name": "Attach to Native (LLDB)",
      "type": "lldb",
      "request": "attach",
      "pid": "${command:pickProcess}",
      "sourceLanguages": ["rust"]
    },

    // ========================================
    // 4. Rust 单元测试
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
    // 5. Maven Test (带 Debug)
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

  // 复合启动配置：同时启动 Extension + Attach Java
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

创建文件 `bazel-jdt-bridge/.vscode/tasks.json`：

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

### 7.3 `.vscode/settings.json`（调试用）

```jsonc
{
  // JDT.LS 调试端口
  "java.jdt.ls.vmargs": "-agentlib:jdwp=transport=dt_socket,server=y,suspend=n,address=*:5005 -Xcheck:jni",

  // Rust 分析器设置
  "rust-analyzer.cargo.features": "all",
  "rust-analyzer.checkOnSave.command": "clippy",

  // Bazel 扩展配置（调试时）
  "bazel-jdt.bazelPath": "bazel",
  "bazel-jdt.syncOnSave": false,
  "bazel-jdt.cacheDir": "/tmp/bazel-jdt-debug-cache"
}
```

---

## 8. 日志系统配置

### 8.1 当前状态

| 层 | 日志框架 | 状态 |
|----|---------|------|
| Rust | `log` crate (16 calls) | **未初始化** — 所有日志静默 |
| Java | 无 | **零日志** — 12 个 catch 块全部静默 |
| TypeScript | 无 | 仅有 `showInformationMessage` / `showErrorMessage` |

### 8.2 启用 Rust 日志

```bash
# 方法 1: 环境变量（需要在代码中初始化 env_logger）
RUST_LOG=debug code /path/to/workspace

# 方法 2: 按模块过滤
RUST_LOG=bazel_jdt_core=trace,bazel_graph=debug,bazel_cache=info code /path/to/workspace

# 方法 3: 仅错误日志
RUST_LOG=error code /path/to/workspace
```

可用的日志级别：

| 级别 | 当前使用数 | 用途 |
|------|-----------|------|
| `trace` | 0 | — |
| `debug` | 1 | 文件监听事件 (watcher.rs) |
| `info` | 5 | 缓存加载、target 发现、文件变更 |
| `warn` | 9 | 缓存加载失败、文件监听启动失败、反序列化错误 |
| `error` | 0 | — |

### 8.3 启用 Java 日志（建议添加）

当前 Java 层没有任何日志基础设施。建议添加 `java.util.logging`（零依赖）：

```java
// 在 BazelBridge.java 中添加
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

### 8.4 JDT.LS 自身的日志

JDT.LS 的日志通常在以下位置：

```bash
# Linux
~/.cache/jdtls/

# macOS
~/Library/Caches/jdtls/

# Windows
%LOCALAPPDATA%\jdtls\
```

查看方式：VS Code Output 面板 → 选择 "Language Support for Java"。

---

## 9. JVM 崩溃与 Panic 调试

### 9.1 Rust Panic 跨越 JNI 边界

**核心风险**: Rust panic 跨越 JNI 边界会导致 **JVM 进程直接 abort**（不是 Java Exception，是进程崩溃）。

```
Rust panic → unwind → 穿越 JNI 边界 → Undefined Behavior → JVM abort (SIGABRT)
```

`jni-rs` crate 的 `JNIEnv` 方法内部使用了 `std::panic::catch_unwind`，但你自己的 JNI 导出函数体**没有**被包裹。

**解决方案**: 在每个 JNI 导出函数中包裹 `catch_unwind`：

```rust
// jni_exports.rs 中推荐的模式
#[no_mangle]
pub extern "system" fn Java_com_bazel_jdt_BazelBridge_nativeComputeClasspath(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    target_label: JString,
) -> jobjectArray {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // 实际逻辑在这里
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

### 9.2 JVM 崩溃 (SIGSEGV/SIGABRT) 调试

当 JVM 在 Rust 代码中崩溃时：

#### 步骤 1：找到 hs_err_pid 日志

JVM 崩溃时会生成 `hs_err_pid<PID>.log` 文件（通常在工作目录或 `/tmp/`）。

```bash
# 查找崩溃日志
find /tmp -name 'hs_err_pid*.log' -mmin -60 2>/dev/null
find ~ -name 'hs_err_pid*.log' -mmin -60 2>/dev/null
```

日志中的关键信息：
- `siginfo:` — 信号类型（SIGSEGV, SIGBUS 等）
- `C/C++ frames:` — 原生调用栈（包含 Rust 函数名，如果有调试符号）
- `Java frames:` — Java 调用栈

#### 步骤 2：启用核心转储

```bash
# Linux: 启用核心转储
ulimit -c unlimited

# macOS
ulimit -c unlimited

# 使用核心转储分析崩溃
rust-lldb -c /path/to/core -- /path/to/java
(lldb) thread backtrace all
```

#### 步骤 3：使用 `-Xcheck:jni` + `-verbose:jni`

```jsonc
// VS Code settings.json — 完整调试 JVM 参数
{
  "java.jdt.ls.vmargs": "-agentlib:jdwp=transport=dt_socket,server=y,suspend=n,address=*:5005 -Xcheck:jni -verbose:jni"
}
```

`-Xcheck:jni` 检测的内容：
- JNI 函数参数类型验证
- 错误线程使用（从非 Java 线程调用 JNI）
- 无效 JNI 引用
- JNI critical 区域违规

`-verbose:jni` 输出的内容：
- 原生库加载路径
- native 方法绑定
- JNI 调用统计

#### 步骤 4：使用 Valgrind (Linux)

对于难以复现的内存问题：

```bash
valgrind \
  --smc-check=all \           # 必须：JVM JIT 自修改代码检测
  --trace-children=yes \      # 跟踪子进程
  --show-leak-kinds=all \
  --track-origins=yes \
  --leak-check=full \
  code /path/to/workspace
```

> **注意**: `--smc-check=all` 对 JVM 调试是**必须的**，因为 JVM 在运行时生成代码（JIT），Valgrind 必须检测自修改代码。性能会很慢，仅用于调试。

### 9.3 动态加载 cdylib 的 LLDB 调试技巧

JVM 通过 `System.loadLibrary()` 动态加载 cdylib，LLDB attach 时可能还没有加载符号。

**技巧: 断在 `dlopen` 上**

```lldb
# 1. Attach 到 JVM 进程
(lldb) attach <PID>

# 2. 在 dlopen 上设断点（捕获库加载时刻）
(lldb) breakpoint set --name dlopen
(lldb) continue

# 3. 当断点命中，检查加载的库路径
(lldb) frame variable path
# 或 (lldb) x/s $rdi   (Linux x86_64)

# 4. 继续执行让库加载完成
(lldb) continue

# 5. 现在设置 Rust 断点（符号已加载）
(lldb) breakpoint set --name Java_com_bazel_jdt_BazelBridge_nativeInitialize
(lldb) breakpoint set --file jni_exports.rs --line 42
```

### 9.4 JDT.LS Eclipse 调试标志

JDT.LS 基于 Eclipse 平台，支持额外的调试标志：

```jsonc
// VS Code settings.json
{
  "java.jdt.ls.vmargs": "...");
}

// 或在手动启动 JDT.LS 时添加：
// -Dorg.eclipse.jdt.core/debug=true
// -Dorg.eclipse.jdt.core/debug/builder=true
// -Dorg.eclipse.jdt.core/debug/compiler=true
// -Dorg.eclipse.jdt.core/debug/indexmanager=true
// -Dlog.protocol=true          // 记录所有 LSP 消息
// -Dlog.level=ALL              // 最大日志详细度
// -consoleLog                  // OSGi 控制台输出到 stdout
// -debug                       // Eclipse 调试模式
```

---

## 10. 已知问题与调试陷阱

### 9.1 macOS 原生库加载失败

**问题**: `NativeLoader.detectOs()` 返回 `"macos"`，但 `bnd.bnd` 和 `build-native.sh` 使用的平台目录名是 `"darwin"`。

```
NativeLoader 查找: /native/macos-x86_64/libbazel_jdt_core.dylib  ← 找不到
JAR 中实际路径:    /native/darwin-x86_64/libbazel_jdt_core.dylib
```

**结果**: macOS 上从 JAR 资源加载永远失败，总是回退到 `System.loadLibrary()`。

**调试方法**:
1. 在 `NativeLoader.java:27` 设置断点
2. 检查 `resourcePath` 值
3. 检查 `getResourceAsStream()` 是否返回 null

**临时解决**: 设置 `java.library.path` 指向 debug 构建目录。

### 9.2 静默异常吞噬

Java 侧有 **4 个完全空的 catch 块**，异常被静默吞噬：

| 文件 | 行号 | 影响 |
|------|------|------|
| `BazelClasspathManager.java` | ~28 | classpath 设置失败无日志 |
| `BazelClasspathManager.java` | ~58 | classpath 刷新失败无日志 |
| `BazelClasspathManager.java` | ~79 | 文件变更触发刷新失败无日志 |
| `BazelProjectImporter.java` | ~59 | 单个 target 项目创建失败被忽略 |

**调试方法**: 在这些 catch 块中添加 `e.printStackTrace()` 或断点。

### 9.3 JNI Handle Use-After-Free

```java
// BazelBridge.java
public synchronized void shutdown() {
    if (handle != -1) {
        nativeShutdown(handle);  // Rust 侧 Box::from_raw 释放内存
        handle = -1;
    }
}
```

虽然 Java 方法是 `synchronized`，但 Rust 侧的指针解引用没有保护：

```rust
// jni_exports.rs:135
let state = unsafe { &*(handle as *const BazelJdtState) };
// 如果另一个线程刚调用了 nativeShutdown，这个指针已经悬空
```

**调试方法**: 在 `nativeShutdown` 中添加延迟日志，确认没有并发调用。

### 9.4 `bazel-jdt.getSyncState` 命令未注册

`statusBar.ts` 每 2 秒轮询 `bazel-jdt.getSyncState` 命令，但 `BazelCommandHandler.java` 的 switch 中没有这个 case，会落入 `default: return null`。

**结果**: 状态栏轮询总是抛异常（被 catch 捕获），显示 "Bazel ✓" 而不是真实状态。

### 9.5 `syncOnSave` 配置是死代码

`package.json` 声明了 `bazel-jdt.syncOnSave` 配置，`config.ts` 读取了它，但 **没有任何地方使用这个值**。没有 `onDidSaveTextDocument` 监听器。

### 9.6 `BazelClasspathContainerInitializer` 类缺失

`plugin.xml:14` 引用了 `com.bazel.jdt.BazelClasspathContainerInitializer`，但源码中不存在这个类。运行时 OSGi 可能无法正确初始化 classpath 容器。

### 9.7 Release 构建流水线问题

| 问题 | 影响 |
|------|------|
| `build-native.sh` 使用 `windows-gnu`，`release.yml` 使用 `windows-msvc` | Windows 产物 ABI 不兼容 |
| `release.yml` 的 artifact 下载后未复制到 `native/` 资源目录 | VSIX 打包时可能缺少原生库 |
| `package-extension.sh` 使用 `|| true` 吞噬 vsce 错误 | 打包失败但脚本返回 0 |

---

## 11. 常用调试场景

### 场景 1：代码补全不工作

```
排查链路 (从外到内):
1. VS Code Output → "Language Support for Java" → 有没有报错?
2. 命令面板 → "Bazel: Import Project" → 是否成功?
3. Java 断点: BazelClasspathManager.setClasspathContainer() → classpath 是否为空?
4. Java 断点: BazelBridge.computeClasspath() → JNI 返回了什么?
5. Rust 日志: nativeComputeClasspath → 走了哪条路径 (cache/graph/aspect)?
6. 终端: bazel query '//...:*' → Bazel 能发现 Java targets 吗?
```

### 场景 2：Bazel target 发现失败

```bash
# 手动验证 Bazel 是否正常
cd /path/to/bazel/workspace
bazel query '//...:*' --output=label

# 只看 Java 相关
bazel query 'kind(java_.*, //...:*)' --output=label

# 测试 aspect
bazel build //path/to:target --aspects=@intellij_aspect//:intellij_info.bzl%intellij_info_aspect
```

### 场景 3：缓存导致 classpath 过期

```
1. 命令面板 → "Bazel: Clean Cache" (清空 redb 缓存)
2. 重新 "Bazel: Import Project"
3. 或者手动删除缓存目录: rm -rf ~/.cache/bazel-jdt/
```

### 场景 4：原生库加载失败（UnsatisfiedLinkError）

```
1. 确认平台: NativeLoader.detectPlatform() 返回什么?
2. 确认路径: JAR 中 /native/<platform>/ 目录存在吗?
3. 确认文件: 原生库文件大小 > 0?
4. Linux 额外检查:
   ldd libbazel_jdt_core.so  # 是否有缺失的动态链接依赖?
5. 临时解决: 通过 -Djava.library.path 指向 debug 构建目录
```

### 场景 5：文件变更不触发同步

```
1. Rust 日志: watcher.rs 是否输出 "Build files changed"?
2. Java 断点: BazelBuildSupport.fileChanged() 是否被调用?
3. 确认 notify crate 的 inotify watch 限制:
   cat /proc/sys/fs/inotify/max_user_watches
   # 如果太小: sudo sysctl fs.inotify.max_user_watches=524288
```

### 场景 6：调试 Rust 的 Bazel CLI 调用

```rust
// 在 bazel-query/src/command.rs 中添加日志
log::debug!("Executing bazel command: {:?}", command);
log::debug!("Bazel output: {}", output);
```

或手动模拟 Rust 的 Bazel 调用：

```bash
# nativeDiscoverTargets 等效于:
bazel query 'kind(java_.*, //...:*)' --output=label

# nativeComputeClasspath (aspect 路径) 等效于:
bazel build //path/to:target \
  --aspects=@intellij_aspect//:intellij_info.bzl%intellij_info_aspect \
  --output_groups=intellij-info-compile,intellij-info-resolve
```

---

## 12. 附录：文件速查表

### Rust 核心文件

| 文件 | 用途 | 测试数 |
|------|------|--------|
| `crates/bazel-jdt-core/src/jni_exports.rs` | JNI 6 个导出函数 | 0 |
| `crates/bazel-jdt-core/src/state.rs` | 全局状态管理 | 0 |
| `crates/bazel-jdt-core/src/watcher.rs` | 文件变更监听 | 2 |
| `crates/bazel-jdt-core/src/change_detector.rs` | 增量变更检测 | 4 |
| `crates/bazel-parser/src/` | Starlark/BUILD 解析 | 0 |
| `crates/bazel-aspect/src/text_proto.rs` | text_proto 解析 | 6 |
| `crates/bazel-query/src/output.rs` | Bazel 输出解析 | 3 |
| `crates/bazel-graph/src/classpath.rs` | 依赖图 + classpath | 0 |
| `crates/bazel-cache/src/redb_store.rs` | redb KV 缓存 | 0 |

### Java 核心文件

| 文件 | 用途 |
|------|------|
| `java-bridge/src/main/java/com/bazel/jdt/BazelBridge.java` | JNI 单例门面 |
| `java-bridge/src/main/java/com/bazel/jdt/NativeLoader.java` | 原生库跨平台加载 |
| `java-bridge/src/main/java/com/bazel/jdt/BazelCommandHandler.java` | VS Code 命令路由 |
| `java-bridge/src/main/java/com/bazel/jdt/BazelProjectImporter.java` | JDT.LS 项目导入 |
| `java-bridge/src/main/java/com/bazel/jdt/BazelClasspathManager.java` | classpath 容器管理 |
| `java-bridge/src/main/java/com/bazel/jdt/BazelClasspathContainer.java` | IClasspathContainer 实现 |
| `java-bridge/src/main/java/com/bazel/jdt/BazelBuildSupport.java` | BUILD 文件变更监听 |
| `java-bridge/src/main/resources/plugin.xml` | OSGi 扩展点声明 |
| `java-bridge/bnd.bnd` | OSGi Bundle 元数据 |

### TypeScript 核心文件

| 文件 | 用途 |
|------|------|
| `vscode-extension/src/extension.ts` | 扩展入口 |
| `vscode-extension/src/commands.ts` | 3 个命令注册 |
| `vscode-extension/src/statusBar.ts` | 状态栏轮询 |
| `vscode-extension/src/config.ts` | 配置读取 |
| `vscode-extension/package.json` | 扩展清单 |

### 构建与 CI

| 文件 | 用途 |
|------|------|
| `Cargo.toml` | Rust workspace 配置 |
| `rust-toolchain.toml` | Rust 工具链 (stable) |
| `java-bridge/pom.xml` | Maven 构建 |
| `.github/workflows/ci.yml` | CI 流水线 |
| `.github/workflows/release.yml` | 发布流水线 |
| `scripts/build-native.sh` | 跨平台编译 |
| `scripts/package-extension.sh` | VSIX 打包 |

---

## 快速参考卡

```
# === 构建命令 ===
cargo build -p bazel-jdt-core                    # Debug 构建 (有符号)
cargo build -p bazel-jdt-core --release          # Release 构建 (无符号)
mvn clean package -DskipTests                    # Java 打包
cd vscode-extension && npm run build             # TS 构建

# === 测试命令 ===
cargo test --workspace                           # Rust 全部测试
cargo test -- --nocapture                        # 显示 println!
mvn test -Djava.library.path=../target/debug     # Java 测试 (Debug)
mvn test -Djava.library.path=../target/release   # Java 测试 (Release)

# === 调试命令 ===
RUST_LOG=debug RUST_BACKTRACE=full code .        # Rust 日志 + backtrace
java -agentlib:jdwp=... -Xcheck:jni -verbose:jni # Java 远程调试 + JNI 检查

# === 验证命令 ===
cargo fmt --all -- --check                       # 格式检查
cargo clippy --workspace --all-targets -- -D warnings  # Lint
jar tf target/*.jar | grep native/               # 检查原生库打包

# === 崩溃分析 ===
find /tmp -name 'hs_err_pid*.log' -mmin -60      # 查找 JVM 崩溃日志
ulimit -c unlimited                              # 启用核心转储
rust-lldb -c /path/to/core -- /path/to/java      # 分析 core dump

# === 环境变量速查 ===
RUST_BACKTRACE=full                              # Rust 完整 backtrace
RUST_LOG=bazel_jdt_core=debug                    # Rust 模块级日志
JAVA_OPTS="-Xcheck:jni -verbose:jni"             # JVM JNI 调试标志
```
