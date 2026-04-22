# Bazel JDT Bridge

## 项目简介

Bazel JDT Bridge 是一个 VS Code 扩展，为 Bazel 工作空间中的 Java 开发提供完整的 IDE 支持。

Bazel 是一个高性能构建系统，但它在 Java IDE 集成方面存在明显短板。开发者打开一个 Bazel 工作空间后，面对的是没有代码补全、没有跳转定义、没有依赖提示的"裸"编辑器。Bazel JDT Bridge 填补了这个空白，它将 Bazel 的构建信息桥接到 Eclipse JDT Language Server，让 VS Code 能像对待 Maven/Gradle 项目一样处理 Bazel Java 项目。

**核心功能：**

- **代码补全**：基于完整的 classpath 信息，提供准确的类名、方法、字段补全
- **代码导航**：支持跳转到定义 (Go to Definition)、查找引用 (Find References) 等操作
- **依赖解析**：通过 Bazel CLI 和 BUILD 文件解析，构建完整的 Java 依赖图
- **实时同步**：监听 BUILD 文件变更，自动触发增量同步，保持 classpath 与工作空间一致
- **智能缓存**：基于 redb 的持久化 KV 存储，区分快速路径和慢速路径，减少不必要的 Bazel 调用

**项目目录结构：**

```
spec-kit-project/
├── bazel-jdt-bridge/         # 主应用 (Rust + Java + TypeScript)
│   ├── crates/               # 6 个 Rust workspace crates
│   │   ├── bazel-parser/     # Starlark/BUILD 文件解析
│   │   ├── bazel-aspect/     # Bazel aspect text_proto 解析
│   │   ├── bazel-query/      # Bazel CLI 异步查询
│   │   ├── bazel-graph/      # 依赖图 + classpath 计算
│   │   ├── bazel-cache/      # redb 持久化 KV 缓存
│   │   └── bazel-jdt-core/   # JNI 桥接 (cdylib)
│   ├── java-bridge/          # Eclipse JDT.LS OSGi Bundle (Maven, Java 17)
│   ├── vscode-extension/     # VS Code 扩展 UI (TypeScript, esbuild)
│   └── scripts/              # 跨平台构建和打包脚本
├── .claude/commands/         # SpecKit AI 辅助开发命令
├── .opencode/                # OpenCode AI 配置
└── openspec/                 # Spec 驱动开发配置
```

## 架构设计

### 四层架构

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

**TypeScript Shell** 是最上层，负责 VS Code 集成。它注册 3 个命令 (import/sync/cleanCache)，管理状态栏轮询，读取用户配置。这一层不包含任何业务逻辑，所有请求通过 `java.execute.workspaceCommand` 转发给 Java 层。

**Java OSGi Bundle** 是中间桥梁，对接 Eclipse JDT.LS 的扩展点体系。它管理 JNI 生命周期，在 JDT 的 `IClasspathContainer` 模型和 Rust 的管道分隔格式之间做翻译。共 7 个 Java 类，采用 OSGi 单例模式运行。

**Rust Core Engine** 是整个项目的核心，承载全部业务逻辑：BUILD 文件解析、Bazel CLI 调用、依赖图构建、classpath 计算、持久化缓存、文件变更监听。由 6 个 crate 组成。

**Bazel CLI** 是最底层，作为构建目标和产物路径的 source of truth。

### 端到端数据流

1. VS Code 打开工作空间，JDT.LS 检测到 `WORKSPACE` 文件，加载 OSGi bundle
2. `BazelProjectImporter` 触发导入流程，调用 JNI `nativeInitialize()` 创建 `BazelJdtState`
3. `nativeDiscoverTargets()` 执行 `bazel query` 获取所有 Java target 标签，返回 `String[]`
4. 对每个 target 调用 `nativeComputeClasspath()`，走以下解析链路：
   - **快速路径**：检查 redb 缓存。命中则直接返回，不调用 Bazel
   - **中速路径**：缓存未命中时，通过 BUILD 文件解析 + 依赖图 BFS (petgraph) 计算 classpath
   - **慢速路径**：图信息不足时，执行 `bazel build --aspects` 触发 IntelliJ aspects 做完整解析，然后缓存结果
5. Java 侧将管道分隔的 classpath 条目解析为 JDT 的 `IClasspathEntry[]`，JDT.LS 据此提供代码补全和导航

Classpath 数据格式 (Rust 到 Java)：

```
TYPE|path|sourceAttachmentPath|isTest|isExported|accessRules
```

其中 TYPE 取值为 `LIB`、`PROJ` 或 `SRC`。

### Rust Crate 依赖关系

```
bazel-jdt-core (cdylib, JNI 入口)
├── bazel-parser (Starlark 解析, starlark_syntax)
├── bazel-aspect (text_proto 解析)
├── bazel-query (异步 Bazel CLI, tokio)
│   └── bazel-aspect
├── bazel-graph (依赖图 + classpath, petgraph)
│   └── bazel-aspect
└── bazel-cache (redb 持久化存储)
```

各 crate 职责：

| Crate | 职责 | 关键依赖 |
|-------|------|----------|
| `bazel-parser` | 解析 Starlark 语法和 BUILD 文件，提取 Java 规则 | `starlark_syntax` |
| `bazel-aspect` | 解析 Bazel aspect 输出的 text_proto 格式 | `serde`, `serde_json` |
| `bazel-query` | 异步调用 `bazel query` 命令，解析输出 | `tokio` |
| `bazel-graph` | 构建 petgraph 依赖图，执行 BFS 计算 classpath | `petgraph`, `bazel-aspect` |
| `bazel-cache` | redb 持久化 KV 存储，管理缓存读写和失效 | `redb`, `sha2` |
| `bazel-jdt-core` | JNI FFI 边界、全局状态、文件监听、变更检测 | 以上全部 + `jni`, `notify` |

### 缓存架构

缓存基于 redb（Rust ACID KV 数据库），维护两张表：

- **classpath 表**：以 target label 为 key，序列化的 classpath JSON 为 value
- **build_hash 表**：以 BUILD 文件路径为 key，SHA-256 哈希为 value

缓存失效策略是按 target 粒度进行的。当文件监听器检测到 BUILD 文件变更时，比较哈希值判断哪些 target 受影响，只重新计算受影响的 classpath。用户也可以通过 `Bazel: Clean Cache` 命令手动清空全部缓存。

## 环境配置

### 前置依赖

| 工具 | 最低版本 | 用途 |
|------|----------|------|
| Rust (cargo) | 1.75+ | 构建原生引擎 |
| Java JDK | 17 | 编译 OSGi Bundle |
| Maven | 3.8+ | Java 构建管理 |
| Node.js | 18+ | 构建 VS Code 扩展 |
| npm | 9+ | JS 依赖管理 |

验证当前环境：

```bash
rustc --version    # 需要 >= 1.75
java -version      # 需要 JDK 17
mvn -version       # 需要 >= 3.8
node --version     # 需要 >= 18
npm --version      # 需要 >= 9
```

### 跨平台编译依赖（可选）

如果需要为非当前宿主平台编译原生库，需要安装以下工具：

```bash
# 安装 Zig 工具链和 cargo-zigbuild
pip install ziglang cargo-zigbuild
```

如果只构建当前平台，用标准 `cargo` 即可，不需要这些额外依赖。

## 构建与打包

### 本地开发构建

按以下顺序执行，构建当前平台版本：

```bash
# 1. 构建 Rust 原生库
cd bazel-jdt-bridge
cargo build -p bazel-jdt-core --release

# 2. 构建 Java OSGi Bundle
cd java-bridge
mvn clean package -DskipTests

# 3. 构建 VS Code 扩展
cd ../vscode-extension
npm install
npm run build
```

构建顺序很重要：Rust 原生库必须在 Java 测试之前构建完成，因为 Java 测试通过 JNI 加载 `.so`/`.dylib`/`.dll`。

### 测试

```bash
# Rust 单元测试 (15 个内联测试)
cd bazel-jdt-bridge
cargo test --workspace

# Java 测试 (需要先构建 Rust 原生库)
cd java-bridge
mvn test -Djava.library.path=../target/release

# Rust 代码检查
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

Rust 测试采用内联 `#[cfg(test)] mod tests` 方式，不使用独立的测试文件或 dev-dependencies。

### 跨平台发布构建

```bash
cd bazel-jdt-bridge

# 跨平台编译 5 个目标平台的原生库
./scripts/build-native.sh

# 打包为 VSIX (包含所有平台原生库)
./scripts/package-extension.sh
```

支持的目标平台：

| 目标平台 | 产物 |
|----------|------|
| `x86_64-unknown-linux-gnu` | `libbazel_jdt_core.so` |
| `aarch64-unknown-linux-gnu` | `libbazel_jdt_core.so` |
| `x86_64-apple-darwin` | `libbazel_jdt_core.dylib` |
| `aarch64-apple-darwin` | `libbazel_jdt_core.dylib` |
| `x86_64-pc-windows-gnu` | `bazel_jdt_core.dll` |

### 构建产物链

```
Rust (cdylib)  →  Java (OSGi JAR)  →  TypeScript (esbuild bundle)  →  VSIX
.so/.dylib/.dll    com.bazel.jdt.jar    dist/extension.js              bazel-jdt-bridge-0.1.0.vsix
```

原生库通过 OSGi 的 `Bundle-NativeCode` 声明打包进 JAR，按 `native/<platform>/` 目录结构组织。`package-extension.sh` 脚本负责将 JAR 放入 VS Code 扩展的 `server/` 目录，然后用 `@vscode/vsce` 打包成 VSIX。

### 安装扩展

```bash
code --install-extension build/bazel-jdt-bridge-0.1.0.vsix
```

安装后，扩展会在打开包含 `WORKSPACE` 或 `WORKSPACE.bazel` 文件的 Java 项目时自动激活。激活后可通过命令面板执行以下命令：

- `Bazel: Import Project`：导入 Bazel 工作空间，构建完整的 classpath
- `Bazel: Sync Project`：增量同步，更新已变更的依赖信息
- `Bazel: Clean Cache`：清空缓存，强制下次全量重新计算

扩展提供了 3 个配置项（在 VS Code 设置中搜索 "Bazel JDT Bridge"）：

- `bazel-jdt.bazelPath`：Bazel 可执行文件路径，默认 `bazel`
- `bazel-jdt.syncOnSave`：保存 BUILD 文件时自动同步，默认开启
- `bazel-jdt.cacheDir`：缓存目录，默认为空（使用系统临时目录）
