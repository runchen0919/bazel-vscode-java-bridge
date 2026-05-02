# Bazel JDT Bridge — 项目导入完整生命周期分析

> 分析版本：2026-05-01 | 分支：001-bazel-java-resolver

---

## 1. 系统架构总览

### 1.1 四层架构

```
┌─────────────────────────────────────────────────────────┐
│                   VS Code Extension                      │
│              (TypeScript / esbuild)                      │
│   extension.ts · commands.ts · statusBar.ts · config.ts │
├─────────────────────────────────────────────────────────┤
│                  Eclipse JDT.LS                          │
│              (Java / OSGi Runtime)                       │
│  提供: ProjectImporter · BuildSupport · Classpath API    │
├─────────────────────────────────────────────────────────┤
│               Bazel JDT Bridge (Java)                    │
│            (OSGi Bundle / Maven / Java 17)               │
│  13个类: Bridge · Importer · ClasspathManager · ...      │
│  plugin.xml 注册 5 个扩展点                              │
├─────────────────────────────────────────────────────────┤
│               Bazel JDT Core (Rust)                      │
│          (cdylib / JNI / Cargo Workspace)                │
│  6个crate: parser · aspect · query · graph · cache · core│
│  7个FFI函数 · redb持久缓存 · notify文件监控              │
└─────────────────────────────────────────────────────────┘
         │                    │                    │
    VS Code API        JDT.LS Extension      JNI FFI
    workspaceCommand   Points (plugin.xml)   (long handle)
```

### 1.2 组件依赖关系

```mermaid
graph TB
    subgraph "VS Code Extension (TypeScript)"
        EXT[extension.ts<br/>激活入口]
        CMD[commands.ts<br/>命令注册]
        SB[statusBar.ts<br/>状态轮询]
        CFG[config.ts<br/>配置读取]
    end

    subgraph "JDT.LS Runtime"
        JDT[Eclipse JDT.LS<br/>语言服务器]
    end

    subgraph "Bazel Bridge Bundle (Java OSGi)"
        ACT[BazelActivator<br/>Bundle生命周期]
        IMP[BazelProjectImporter<br/>项目导入入口]
        CPM[BazelClasspathManager<br/>Classpath管理]
        CPC[BazelClasspathContainer<br/>IClasspathContainer]
        CPI[BazelClasspathContainerInitializer<br/>容器初始化器]
        BS[BazelBuildSupport<br/>构建文件监控]
        CH[BazelCommandHandler<br/>命令路由]
        BB[BazelBridge<br/>JNI单例桥接]
        NL[NativeLoader<br/>原生库加载]
        PD[PlatformDetector<br/>平台检测]
        NAT[BazelNature<br/>项目Nature]
        TPM[TargetProjectMapping<br/>目标-项目映射]
        LU[LabelUtils<br/>标签解析]
    end

    subgraph "Rust Core (6 Crates)"
        JNI[jni_exports.rs<br/>7个FFI函数]
        ST[state.rs<br/>BazelJdtState]
        WT[watcher.rs<br/>文件监控]
        CD[change_detector.rs<br/>变更检测]
        ASP[aspect.rs<br/>Aspect提取]
        PARSER[bazel-parser<br/>Starlark解析]
        ASPECT[bazel-aspect<br/>TextProto解析]
        QUERY[bazel-query<br/>Bazel CLI调用]
        GRAPH[bazel-graph<br/>依赖图+Classpath]
        CACHE[bazel-cache<br/>redb KV存储]
    end

    EXT --> CMD
    EXT --> SB
    EXT --> CFG
    CMD -->|java.execute.workspaceCommand| JDT
    SB -->|getSyncState| JDT

    JDT -->|extension point| IMP
    JDT -->|extension point| BS
    JDT -->|extension point| CPI
    JDT -->|delegateCommand| CH

    IMP --> BB
    IMP --> CPM
    IMP --> TPM
    IMP --> NAT
    IMP --> LU

    CPM --> BB
    CPM --> CPC
    CPM --> TPM

    CPI --> CPM
    CPI --> BB
    CPI --> TPM

    BS --> CPM

    CH --> BB
    CH --> CPM

    BB --> NL
    NL --> PD
    BB -->|JNI| JNI

    JNI --> ST
    ST --> PARSER
    ST --> ASPECT
    ST --> QUERY
    ST --> GRAPH
    ST --> CACHE
    ST --> WT
    ST --> CD
    ST --> ASP

    GRAPH --> PARSER
    GRAPH --> ASPECT
    QUERY --> ASPECT

    style BB fill:#ffd8a8,stroke:#e67700
    style JNI fill:#ffc9c9,stroke:#e03131
    style GRAPH fill:#b2f2bb,stroke:#2f9e44
    style CACHE fill:#c3fae8,stroke:#099268
```

---

## 2. 完整生命周期时序图

### 2.1 项目导入主流程

```mermaid
sequenceDiagram
    participant User as 用户
    participant VSCode as VS Code
    participant RHJava as Red Hat Java<br/>(JDT.LS Host)
    participant JDTLS as JDT.LS
    participant OSGi as OSGi Runtime
    participant Activator as BazelActivator
    participant Importer as BazelProjectImporter
    participant Bridge as BazelBridge
    participant NativeLoader as NativeLoader
    participant JNI as jni_exports.rs
    participant State as BazelJdtState
    participant Bazel as Bazel CLI

    User->>VSCode: 打开含 WORKSPACE 的目录
    VSCode->>RHJava: activationEvent: workspaceContains:WORKSPACE
    RHJava->>JDTLS: 启动语言服务器
    RHJava->>OSGi: 加载 javaExtensions: com.bazel.jdt.jar

    rect rgb(230, 240, 255)
        Note over OSGi,Activator: Phase 0: Bundle 激活
        OSGi->>Activator: start(bundleContext)
        Activator->>Activator: 注册 IResourceChangeListener<br/>(清理幽灵项目)
    end

    rect rgb(230, 255, 230)
        Note over JDTLS,NativeLoader: Phase 0.5: 原生库加载 (static initializer)
        JDTLS->>Importer: applies(monitor)
        Importer->>Importer: 检查 WORKSPACE/WORKSPACE.bazel 存在
        Importer-->>JDTLS: true (claim workspace)
    end

    rect rgb(255, 245, 230)
        Note over JDTLS,Importer: Phase 1: 项目导入
        JDTLS->>Importer: importToWorkspace(monitor)
        Importer->>Bridge: getInstance()
        Bridge->>NativeLoader: load()
        NativeLoader->>NativeLoader: PlatformDetector.detectPlatform()
        NativeLoader->>NativeLoader: 从JAR提取.so/.dylib/.dll到临时目录
        NativeLoader->>NativeLoader: System.load(tempPath)

        Importer->>Bridge: isInitialized()
        Bridge-->>Importer: false

        Importer->>Bridge: initialize(workspacePath, "bazel", cacheDir)
        Bridge->>Bridge: rwLock.writeLock()
        Bridge->>JNI: nativeInitialize(ws, bazel, cache) → jlong handle
        JNI->>State: BazelJdtState::new()
        State->>State: BazelCache::open(cacheDir)
        State->>State: DependencyGraph::new()
        State->>State: extract_if_needed() → 提取7个.bzl aspect文件
        State->>State: BazelInvoker::new()
        State->>State: watch::channel(false) ← shutdown信号
        State->>State: BuildFileWatcher::start() ← 文件监控线程
        JNI-->>Bridge: handle = 42
        Bridge->>Bridge: this.handle = 42
        Bridge->>Bridge: rwLock.writeLock().unlock()
    end

    rect rgb(240, 230, 255)
        Note over Importer,Bazel: Phase 2: 目标发现
        Importer->>Bridge: discoverTargets()
        Bridge->>Bridge: snapshotHandle() → h=42 (readLock)
        Bridge->>JNI: nativeDiscoverTargets(42)
        JNI->>State: set_sync_state(Syncing)
        JNI->>State: invoker.discover_java_targets() [async, 120s]
        State->>Bazel: bazel query --output=label<br/>kind(java_library, //...:*) union ...

        Bazel-->>State: //app:lib\n//app:main\n//lib:utils\n...
        State->>State: populate_graph_from_build_files()
        State->>State: change_detector::collect_build_files()
        loop 每个 BUILD 文件
            State->>State: parser.parse_file() → ParsedBuildFile
            State->>State: 提取 java_library/binary/test/import 规则
        end
        State->>State: graph.populate_from_parsed_batch()

        State->>Bazel: bazel build --aspects=//.bazel-jdt/aspects:...<br/>--output_groups=intellij-info-java ...
        Bazel-->>State: .intellij-info.txt 文件路径列表
        loop 每个 .intellij-info.txt
            State->>State: TextProtoParser::parse_target_ide_info()
            State->>State: 提取: label, kind, jars, deps, exports
        end
        State->>State: graph.populate_from_aspects()
        State->>State: set_sync_state(Idle)
        JNI-->>Bridge: String[] {"//app:lib", "//app:main", "//lib:utils"}
    end

    rect rgb(255, 230, 230)
        Note over Importer,State: Phase 3: 项目创建 + Classpath 设置
        loop 每个 targetLabel
            Importer->>Importer: extractPackageName(label)<br/>//app:lib → "app"
            Importer->>Importer: workspaceRoot.getProject("app")
            alt 项目不存在
                Importer->>Importer: project.create() + project.open()
            end
            Importer->>Importer: 设置 natures: javanature + bazelNature
            Importer->>Importer: TargetProjectMapping.appendTargets()
            Importer->>Importer: 配置 source entries

            Importer->>Bridge: computeClasspath("//app:lib")
            Bridge->>JNI: nativeComputeClasspath(42, "//app:lib")

            alt Tier 1: 缓存命中
                JNI->>State: cache.get_classpath("//app:lib")
                State-->>JNI: ComputedClasspath JSON
            else Tier 2: 图计算
                JNI->>State: graph.transitive_deps("//app:lib") [BFS]
                JNI->>State: ComputedClasspath::compute_for()
            else Tier 3: 完整Aspect构建
                JNI->>Bazel: bazel build --aspects=... //app:lib
                Bazel-->>JNI: .intellij-info.txt
                JNI->>State: graph.populate_from_aspects()
                JNI->>State: ComputedClasspath::compute_for()
            end

            JNI-->>Bridge: String[] {"LIB|/path/a.jar|...|false|false|", ...}
            Bridge-->>Importer: String[] entries

            Importer->>Importer: new BazelClasspathContainer(entries)
            Importer->>Importer: TargetProjectMapping.storeCachedClasspath()
            Importer->>Importer: JavaCore.setClasspathContainer()
            Importer->>Importer: javaProject.setRawClasspath()
        end
    end

    rect rgb(230, 255, 255)
        Note over VSCode,Bridge: Phase 4: VS Code 扩展激活
        VSCode->>VSCode: activate(context)
        VSCode->>VSCode: createStatusBar() ← 状态轮询
        VSCode->>VSCode: registerCommands() ← 3个命令

        loop 每 2-10 秒轮询
            VSCode->>Bridge: getSyncState()
            Bridge->>JNI: nativeGetSyncState(42)
            JNI-->>Bridge: 0 (Idle)
            Bridge-->>VSCode: 0
            VSCode->>VSCode: 状态栏: "Bazel ✓" (绿色)
        end
    end
```

### 2.2 增量同步流程

```mermaid
sequenceDiagram
    participant User as 用户
    participant FS as 文件系统
    participant Watcher as BuildFileWatcher<br/>(Rust 线程)
    participant JDTLS as JDT.LS
    participant BS as BazelBuildSupport
    participant CPM as BazelClasspathManager
    participant Bridge as BazelBridge
    participant JNI as jni_exports.rs
    participant Cache as BazelCache<br/>(redb)

    Note over User,Cache: 路径A: 文件变更自动触发

    User->>FS: 修改 BUILD 文件
    FS->>Watcher: inotify/FSEvents 通知 (500ms 去抖)
    Watcher->>Watcher: compute_file_hash(path) → SHA-256
    Watcher->>Cache: get_build_hash(path)
    Cache-->>Watcher: old_hash
    Watcher->>Watcher: 新旧 hash 比较
    alt hash 未变
        Watcher->>Watcher: 跳过 (false positive)
    else hash 改变
        Watcher->>Cache: put_build_hash(path, new_hash)
        Watcher->>Watcher: pending_changes.push("//app:*")
    end

    Note over JDTLS,Cache: 路径B: JDT.LS BuildSupport 触发

    JDTLS->>BS: fileChanged(resource, CHANGE_TYPE)
    BS->>BS: isBuildFile(resource) → true
    BS->>CPM: refreshClasspathForFiles([filePath])
    CPM->>Bridge: getPendingChanges()
    Bridge->>JNI: nativeGetPendingChanges(42)
    JNI->>JNI: drain pending_changes → ["//app:*"]
    JNI-->>Bridge: String[] pending
    Bridge-->>CPM: ["//app:*"]

    CPM->>CPM: 匹配 affected projects
    loop 每个匹配的项目
        CPM->>CPM: extractTargetLabels(project)
        CPM->>Bridge: computeClasspath("//app:lib")
        Bridge->>JNI: nativeComputeClasspath(42, "//app:lib")
        Note over JNI,Cache: 3-Tier 解析<br/>Tier 2 优先 (graph 已有数据)
        JNI-->>Bridge: String[] pipe-delimited entries
        CPM->>CPM: JavaCore.setClasspathContainer()
    end

    Note over User,Cache: 路径C: 手动同步命令

    User->>JDTLS: Command Palette → "Bazel: Sync Project"
    JDTLS->>CPM: refreshClasspath()
    loop 所有 Java 项目
        CPM->>CPM: read target labels
        CPM->>Bridge: computeClasspath(label)
    end
```

### 2.3 关闭流程

```mermaid
sequenceDiagram
    participant VSCode as VS Code
    participant JDTLS as JDT.LS
    participant CH as BazelCommandHandler
    participant Bridge as BazelBridge
    participant JNI as jni_exports.rs
    participant State as BazelJdtState
    participant Watcher as BuildFileWatcher

    VSCode->>VSCode: deactivate()
    VSCode->>JDTLS: java.execute.workspaceCommand("bazel-jdt.shutdown")
    JDTLS->>CH: executeCommand("bazel-jdt.shutdown")
    CH->>Bridge: shutdown()
    Bridge->>Bridge: jniExecutor.shutdownNow()
    Bridge->>Bridge: awaitTermination(5, SECONDS)
    Bridge->>JNI: nativeShutdown(42)
    JNI->>JNI: registry.remove(42) → take Box<BazelJdtState>
    JNI->>State: signal_shutdown() → shutdown_tx.send(true)
    JNI->>State: set_sync_state(Dead)
    JNI->>Watcher: stop_nonblocking()
    Watcher-->>JNI: JoinHandle
    JNI->>JNI: join_handle.join()
    JNI->>JNI: Box<BazelJdtState> dropped (析构所有字段)
    Note over Bridge,State: handle = -1, executor terminated
```

---

## 3. 状态机

### 3.1 系统状态转换

```mermaid
stateDiagram-v2
    [*] --> BundleLoaded : OSGi 加载 com.bazel.jdt.jar

    state "Bundle 加载阶段" as Phase0 {
        BundleLoaded --> NativeLoaded : NativeLoader.load<br/>提取 .so 到临时目录
        NativeLoaded --> Uninitialized : BazelBridge INSTANCE 创建<br/>handle = -1
    }

    state "未初始化" as Uninitialized

    Uninitialized --> Initializing : nativeInitialize<br/>创建 BazelJdtState

    state "初始化中" as Initializing {
        [*] --> OpenCache : 打开 redb 缓存
        OpenCache --> ExtractAspects : 提取7个.bzl文件
        ExtractAspects --> CreateInvoker : 创建 BazelInvoker
        CreateInvoker --> StartWatcher : 启动文件监控线程
        StartWatcher --> [*] : handle 有效
    }

    Initializing --> Idle : 初始化成功<br/>handle = ptr

    state "活跃状态" as Active {
        state "空闲" as Idle
        state "同步中" as Syncing
        state "错误" as Error

        Idle --> Syncing : discoverTargets<br/>或 computeClasspath
        Syncing --> Idle : 操作成功
        Syncing --> Error : 超时/Bazel错误
        Error --> Syncing : 重试操作
        Idle --> Idle : getPendingChanges<br/>文件变更入队
    }

    Active --> ShuttingDown : nativeShutdown

    state "关闭中" as ShuttingDown {
        [*] --> StopExecutor : shutdownNow
        StopExecutor --> SignalShutdown : shutdown_tx.send true
        SignalShutdown --> StopWatcher : watcher.stop
        StopWatcher --> DropState : Box drop
        DropState --> [*]
    }

    ShuttingDown --> Dead : handle = -1

    state "已终止" as Dead

    Dead --> Initializing : 重新初始化<br/>先 shutdown 再 initialize
```

### 3.2 Classpath 3-Tier 解析策略

```mermaid
stateDiagram-v2
    state "Classpath 请求" as Request
    state "Tier 1: 缓存" as Tier1
    state "Tier 2: 图计算" as Tier2
    state "Tier 3: Bazel构建" as Tier3

    [*] --> Request : nativeComputeClasspath
    Request --> Tier1 : 查询 redb 缓存

    Tier1 --> Hit : cache.get_classpath
    Hit --> [*] : 返回缓存结果 最快

    Tier1 --> Miss : 缓存未命中
    Miss --> Tier2 : graph.get_target_jars

    Tier2 --> HasAspectData : 图中有 Aspect 数据
    HasAspectData --> Compute : compute_for BFS传递依赖
    Compute --> CacheAndReturn : 写入 redb 缓存
    CacheAndReturn --> [*] : 返回计算结果

    Tier2 --> NoAspectData : 图中无 Aspect 数据
    NoAspectData --> Tier3 : run_full_resolution

    Tier3 --> AspectBuild : bazel build --aspects
    AspectBuild --> ParseTextProto : TextProtoParser
    ParseTextProto --> PopulateGraph : populate_from_aspects
    PopulateGraph --> Compute2 : compute_for
    Compute2 --> CacheAndReturn2 : 写入 redb 缓存
    CacheAndReturn2 --> [*] : 返回构建结果 最慢
```

---

## 4. 数据流

### 4.1 管道分隔格式 (Rust → Java)

```
格式: TYPE|path|sourceAttachmentPath|isTest|isExported|accessRules

TYPE:
  LIB  → JavaCore.newLibraryEntry()     外部JAR
  PROJ → JavaCore.newProjectEntry()     工作区内部目标
  SRC  → JavaCore.newSourceEntry()      源码目录

示例:
  LIB|/home/user/.cache/bazel/.../guava.jar||false|false|+com.google.**:-internal.**
  PROJ|//app:lib||false|false|
  SRC|/workspace/app/src/main/java||false|false|
```

### 4.2 TextProto 格式 (Bazel Aspect → Rust)

```
Bazel Aspect 输出 .intellij-info.txt (TextProto格式):

label: "//app:lib"
kind: "java_library_"
java_info {
  jars { jar { relative_path: "app/lib.jar" } }
  jars { jar { relative_path: "app/lib-src.jar" } source_jar { relative_path: "app/lib-sources.jar" } }
  javac_options { option: "--release" option: "17" }
  generated_class_jar { relative_path: "app/gen.jar" }
}
deps { label: "//lib:utils" }
runtime_deps { label: "//runtime:driver" }
exports { label: "//api:public" }
```

### 4.3 持久化存储

```
Eclipse Persistent Properties (per IProject):
  com.bazel.jdt / targetLabels     → "//app:lib,//app:main"
  com.bazel.jdt / workspacePath    → "/home/user/workspace"
  com.bazel.jdt / bazelPath        → "bazel"
  com.bazel.jdt / cacheDir         → "/home/user/.cache/bazel-jdt"
  com.bazel.jdt / classpath.<label> → pipe-delimited entries JSON

redb Tables (per workspace):
  classpath  → target_label → ComputedClasspath JSON
  build_hash → build_file_path → SHA-256 hex
```

### 4.4 完整数据流图

```mermaid
flowchart LR
    subgraph "Bazel Workspace"
        BF[BUILD 文件<br/>Starlark语法]
        WS[WORKSPACE]
        JF[.jar 文件<br/>maven_jar/rules]
    end

    subgraph "Rust: bazel-parser"
        PARSER[BuildFileParser<br/>starlark_syntax AST]
        PBF[ParsedBuildFile<br/>JavaRule列表]
    end

    subgraph "Rust: bazel-query"
        BAZEL[BazelInvoker<br/>CLI调用]
        LABELS[target labels<br/>//pkg:name]
    end

    subgraph "Rust: bazel-aspect"
        TP[TextProtoParser<br/>递归下降解析]
        TII[TargetIdeInfo<br/>jars/deps/exports]
    end

    subgraph "Rust: bazel-graph"
        DG[DependencyGraph<br/>petgraph DiGraph]
        CC[ComputedClasspath<br/>管道分隔序列化]
    end

    subgraph "Rust: bazel-cache"
        RDB[redb KV Store<br/>classpath + build_hash]
    end

    subgraph "Java: OSGi Bundle"
        BB[BazelBridge<br/>JNI单例]
        CPC["BazelClasspathContainer<br/>IClasspathEntry array"]
        TPM[TargetProjectMapping<br/>Eclipse Properties]
    end

    BF -->|文件读取| PARSER
    PARSER -->|AST解析| PBF
    PBF -->|populate_from_parsed| DG

    BAZEL -->|"bazel query"| LABELS
    BAZEL -->|"bazel build --aspects"| TP
    TP -->|解析 text_proto| TII
    TII -->|populate_from_aspects| DG
    LABELS -->|返回Java| BB

    DG -->|transitive_deps| CC
    CC -->|缓存| RDB
    RDB -->|缓存命中| CC
    CC -->|"String array pipe-delimited"| BB

    BB -->|parseEntry| CPC
    BB -->|store| TPM
    CPC -->|JDT API| JDT[JDT.LS<br/>ClasspathManager]
```

---

## 5. 关键类与方法详解

### 5.1 Java 层核心类

| 类 | 职责 | 关键方法 | 扩展点 |
|----|------|---------|--------|
| `BazelBridge` | JNI单例桥接，管理handle和executor | `initialize()`, `discoverTargets()`, `computeClasspath()`, `shutdown()` | — |
| `BazelProjectImporter` | 项目导入入口，创建Eclipse项目 | `applies()`, `importToWorkspace()`, `configureClasspath()` | `org.eclipse.jdt.ls.core.importers` |
| `BazelClasspathManager` | 静态工具，设置/刷新Classpath容器 | `setClasspathContainer()`, `refreshClasspath()`, `refreshClasspathForFiles()` | — |
| `BazelClasspathContainer` | JDT容器实现，解析管道格式 | `getClasspathEntries()`, `getDescription()`, `parseEntry()` | — |
| `BazelClasspathContainerInitializer` | JDT容器延迟初始化 | `initialize()`, `doInitialize()`, `recoverFromCache()` | `org.eclipse.jdt.core.classpathContainerInitializer` |
| `BazelBuildSupport` | BUILD文件变更检测 | `fileChanged()`, `isBuildFile()` | `org.eclipse.jdt.ls.core.buildSupport` |
| `BazelCommandHandler` | VS Code命令路由 | `executeCommand()`, 5个handle方法 | `org.eclipse.jdt.ls.core.delegateCommandHandler` |
| `BazelActivator` | OSGi Bundle生命周期 | `start()`, `stop()` | `Bundle-Activator` |
| `NativeLoader` | 原生库提取加载 | `load()` | — |
| `PlatformDetector` | OS/架构检测 | `detectPlatform()` | — |
| `BazelNature` | 项目Nature标记 | `setNatures()`, `configure()` | `org.eclipse.core.resources.natures` |
| `TargetProjectMapping` | 持久化属性存储 | `appendTargets()`, `readTargets()`, `storeCachedClasspath()` | — |
| `LabelUtils` | 标签解析工具 | `extractPackageName()` | — |

### 5.2 Rust FFI 函数表

| # | FFI 函数 | Java 签名 | 功能 | 超时 |
|---|---------|-----------|------|------|
| 1 | `nativeInitialize` | `long nativeInitialize(String ws, String bazel, String cache)` | 创建全局状态，提取aspects，启动文件监控 | — |
| 2 | `nativeShutdown` | `void nativeShutdown(long handle)` | 信号关闭，停止监控，释放状态 | 5s |
| 3 | `nativeDiscoverTargets` | `String[] nativeDiscoverTargets(long handle)` | bazel query + BUILD解析 + 批量aspect构建 | 330s |
| 4 | `nativeComputeClasspath` | `String[] nativeComputeClasspath(long handle, String target)` | 3-Tier解析：缓存→图→aspect构建 | 330s |
| 5 | `nativeGetSyncState` | `int nativeGetSyncState(long handle)` | 返回同步状态枚举值 | — |
| 6 | `nativeCleanCache` | `void nativeCleanCache(long handle)` | 清空redb所有表 | — |
| 7 | `nativeGetPendingChanges` | `String[] nativeGetPendingChanges(long handle)` | 排空文件变更队列 | — |

### 5.3 Rust Crate 依赖图

```mermaid
graph TD
    CORE["bazel-jdt-core<br/>(cdylib + lib)<br/>JNI FFI + 状态管理"]
    PARSER["bazel-parser<br/>Starlark BUILD 解析"]
    ASPECT["bazel-aspect<br/>TextProto 解析<br/>(叶子节点)"]
    QUERY["bazel-query<br/>Bazel CLI 异步调用"]
    GRAPH["bazel-graph<br/>依赖图 + Classpath"]
    CACHE["bazel-cache<br/>redb KV 持久存储"]

    CORE --> PARSER
    CORE --> ASPECT
    CORE --> QUERY
    CORE --> GRAPH
    CORE --> CACHE

    QUERY --> ASPECT
    GRAPH --> ASPECT
    GRAPH --> PARSER

    style CORE fill:#ffc9c9,stroke:#e03131
    style ASPECT fill:#c3fae8,stroke:#099268
    style GRAPH fill:#b2f2bb,stroke:#2f9e44
```

---

## 6. 构建打包流水线

```mermaid
flowchart TD
    subgraph "Step 1: Rust 编译 (5平台交叉编译)"
        RUST["cargo build -p bazel-jdt-core --release"]
        RUST --> SO_LINUX["libbazel_jdt_core.so<br/>(linux-x86_64)"]
        RUST --> SO_LINUX_ARM["libbazel_jdt_core.so<br/>(linux-aarch64)"]
        RUST --> DYLIB_MAC["libbazel_jdt_core.dylib<br/>(darwin-x86_64)"]
        RUST --> DYLIB_MAC_ARM["libbazel_jdt_core.dylib<br/>(darwin-aarch64)"]
        RUST --> DLL_WIN["libbazel_jdt_core.dll<br/>(windows-x86_64)"]
    end

    subgraph "Step 2: 原生库嵌入 Maven Resources"
        SO_LINUX --> RES1["java-bridge/src/main/resources/native/linux-x86_64/"]
        SO_LINUX_ARM --> RES2["native/linux-aarch64/"]
        DYLIB_MAC --> RES3["native/darwin-x86_64/"]
        DYLIB_MAC_ARM --> RES4["native/darwin-aarch64/"]
        DLL_WIN --> RES5["native/windows-x86_64/"]
    end

    subgraph "Step 3: Java OSGi Bundle 构建"
        MVN["mvn clean package"]
        RES1 & RES2 & RES3 & RES4 & RES5 --> MVN
        MVN --> JAR["bazel-jdt-bridge-0.1.0.jar<br/>(含 Bundle-NativeCode)"]
    end

    subgraph "Step 4: TypeScript 编译"
        NPM["npm run build (esbuild)"]
        NPM --> DIST["dist/extension.js"]
    end

    subgraph "Step 5: VSIX 组装"
        PKG["package-extension.sh"]
        JAR --> PKG
        DIST --> PKG
        PKG --> VSIX["bazel-jdt-bridge-0.1.0.vsix<br/>(VS Code 插件包)"]
    end

    subgraph "VSIX 内部结构"
        VSIX --> VSIX_STRUCT["├── server/<br/>│   └── com.bazel.jdt.jar<br/>│       ├── BazelBridge.class<br/>│       ├── plugin.xml<br/>│       └── native/<br/>│           ├── linux-x86_64/*.so<br/>│           ├── darwin-x86_64/*.dylib<br/>│           └── ...<br/>├── dist/<br/>│   └── extension.js<br/>└── package.json"]
    end

    style JAR fill:#b2f2bb,stroke:#2f9e44
    style VSIX fill:#a5d8ff,stroke:#1971c2
```

---

## 7. 设计分析

### 7.1 架构亮点

| 设计决策 | 分析 |
|---------|------|
| **Handle-based State** | Java 持有 `jlong` 键，Rust 持有 `Box<BazelJdtState>` 在全局 HashMap。解耦了两层内存管理，但无 generation/lifetime 校验 — shutdown 后调用是 UB。 |
| **3-Tier Classpath 解析** | 缓存 → 图计算 → Bazel构建。大多数请求命中 Tier 2（图已有 Aspect 数据），只有新目标或缓存失效才走 Tier 3。分层设计有效减少 Bazel 调用。 |
| **Single-Thread JNI Executor** | 所有 JNI 调用序列化到单线程 `jniExecutor`，避免并发 JNI 调用。`ReentrantReadWriteLock` 保护 handle 读写。 |
| **Bundled Aspects** | 7个 `.bzl` 文件通过 `include_str!()` 嵌入 Rust 二进制，首次运行提取到 `.bazel-jdt/aspects/`，版本用 SHA-256 跟踪。自包含，无需额外安装。 |
| **双路径触发** | 自动触发（JDT.LS importer）+ 手动触发（VS Code 命令）。幂等守卫防止双重初始化。 |
| **持久化恢复** | `BazelClasspathContainerInitializer.recoverFromCache()` 从 Eclipse persistent properties 恢复 classpath，无需重跑 Bazel。IDE 重启快速恢复。 |

### 7.2 已知风险与反模式

| 风险 | 严重性 | 位置 | 说明 |
|------|--------|------|------|
| **JNI Use-After-Free** | 高 | `BazelBridge.snapshotHandle()` | shutdown 后 handle=-1，但并发 executor 中的任务可能仍在使用旧 handle。无 generation counter 或 guard。 |
| **空 catch 块** | 中 | `BazelClasspathManager` (3处), `BazelProjectImporter` (1处) | 异常被静默吞掉，可能掩盖关键错误。 |
| **filter_by_visibility 空实现** | 中 | `classpath.rs::filter_by_visibility()` | 函数体为空，所有目标都通过可见性过滤。Bazel visibility 规则未生效。 |
| **NativeLoader 手动提取** | 低 | `NativeLoader.java` | `Bundle-NativeCode` 声明在 bnd.bnd 但实际不用 OSGi 原生加载机制。声明与实现不一致。 |
| **幂等守卫不对等** | 低 | `BazelProjectImporter` vs `BazelCommandHandler` | importer 有 `isInitialized()` 守卫跳过重复导入；command handler 的 `handleImportProject` 没有，可以强制重新初始化。设计意图但可能混淆。 |
| **syncOnSave 死代码** | 低 | `config.ts` | 配置项声明但未使用。BUILD 文件监控完全由 Java 层 `BazelBuildSupport` 处理。 |
| **打包验证宽松** | 低 | `package-extension.sh` | 原生库缺失只 WARNING 不 ERROR (`|| true`)，可能产出不含原生库的 VSIX。 |

### 7.3 线程模型

```
┌──────────────────────────────────────────────────┐
│ VS Code Main Thread                              │
│   extension.ts activate/deactivate               │
│   statusBar poll loop (setInterval)              │
│   command handlers → java.execute.workspaceCommand│
└──────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────┐
│ JDT.LS Thread Pool                               │
│   BazelProjectImporter.importToWorkspace()        │
│   BazelBuildSupport.fileChanged()                 │
│   BazelCommandHandler.executeCommand()             │
│   BazelClasspathContainerInitializer.initialize() │
└──────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────┐
│ bazel-jdt-native Thread (Java single-thread)     │
│   所有 JNI 调用序列化执行                          │
│   nativeInitialize / nativeDiscoverTargets / ...  │
│   ReentrantReadWriteLock 保护 handle              │
└──────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────┐
│ bazel-jdt-build-watcher Thread (Rust OS thread)  │
│   notify debouncer (500ms)                        │
│   SHA-256 hash 比较                               │
│   pending_changes 队列                            │
└──────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────┐
│ Tokio Runtime (Rust async)                       │
│   BazelInvoker: bazel query/build 子进程          │
│   shutdown watch channel 监听                     │
└──────────────────────────────────────────────────┘
```

---

## 8. 命令路由表

| VS Code 命令 | TS Handler | Java Handler | JNI 调用 | 功能 |
|-------------|-----------|-------------|---------|------|
| `bazel-jdt.importProject` | 进度窗口 "Discovering Java targets..." | `handleImportProject()` → initialize + discoverTargets + refreshClasspath | nativeInitialize + nativeDiscoverTargets + N×nativeComputeClasspath | 完整重新导入 |
| `bazel-jdt.syncProject` | 无UI | `handleSyncProject()` → refreshClasspath | N×nativeComputeClasspath | 增量同步 |
| `bazel-jdt.cleanCache` | 确认对话框 | `handleCleanCache()` | nativeCleanCache | 清空redb缓存 |
| `bazel-jdt.getSyncState` | 状态栏自动调用 | 直接调用 | nativeGetSyncState | 查询状态 |
| `bazel-jdt.shutdown` | deactivate() 自动调用 | `handleShutdown()` | nativeShutdown | 关闭清理 |

---

## 9. plugin.xml 扩展点注册

```xml
<!-- 项目导入器 (order=200, 优先级较低) -->
<extension point="org.eclipse.jdt.ls.core.importers">
    <importer class="com.bazel.jdt.BazelProjectImporter" order="200"/>
</extension>

<!-- 构建支持 (BUILD文件变更检测) -->
<extension point="org.eclipse.jdt.ls.core.buildSupport">
    <buildSupport class="com.bazel.jdt.BazelBuildSupport" order="200"/>
</extension>

<!-- Classpath容器初始化器 -->
<extension point="org.eclipse.jdt.core.classpathContainerInitializer">
    <classpathContainerInitializer
        id="com.bazel.jdt.BAZEL_CONTAINER"
        class="com.bazel.jdt.BazelClasspathContainerInitializer"/>
</extension>

<!-- VS Code 命令处理器 -->
<extension point="org.eclipse.jdt.ls.core.delegateCommandHandler">
    <delegateCommandHandler id="bazel-jdt">
        <command id="bazel-jdt.importProject"/>
        <command id="bazel-jdt.syncProject"/>
        <command id="bazel-jdt.cleanCache"/>
        <command id="bazel-jdt.getSyncState"/>
        <command id="bazel-jdt.shutdown"/>
    </delegateCommandHandler>
</extension>

<!-- 项目 Nature -->
<extension point="org.eclipse.core.resources.natures">
    <runtime>
        <run class="com.bazel.jdt.BazelNature"/>
    </runtime>
</extension>
```
