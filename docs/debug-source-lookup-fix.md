# Debug Source Lookup Fix

## Problem

When debugging Java applications in Bazel workspaces via VS Code, source file lookup fails for library code (both JDK and third-party JARs like Guava), causing:

1. **"Unknown Source"** displayed in call stack instead of actual source file
2. **Breakpoint red dot missing** — debug stops at a file but breakpoint markers don't appear
3. **Two different files opened** — Ctrl+Click navigation and debug stack trace open different editor tabs for the same class

### Root Cause Chain

Three distinct bugs contribute, each layered on the next:

```
┌─────────────────────────────────────────────────────────────────────────┐
│  Bug 1: Source Container Duplication                                   │
│  ────────────────────────────────────                                  │
│  JDT.LS's JdtUtils.getSourceContainers() returns 64 containers        │
│  (50 are jrt-fs.jar duplicates from modular JDK). This causes         │
│  excessive iteration but is not the primary failure.                   │
│                                                                        │
│  Bug 2: PFRSC fails for modular JDK classes                           │
│  ────────────────────────────────────────                              │
│  PackageFragmentRootSourceContainer.findSourceElements() uses           │
│  IPackageFragmentRoot.getPackageFragment("java.lang") to locate        │
│  JDK sources. In modular JDK (9+), "java.lang" lives under the        │
│  "java.base" module, so fragment.exists() returns false. Result:      │
│  getSourceFileURI() returns null → "Unknown Source".                   │
│                                                                        │
│  Bug 3: URI mismatch between debug and editor                          │
│  ─────────────────────────────────────                                 │
│  java-debug's getFileURI(IClassFile) produces:                         │
│    jdt://contents/jar/pkg/Foo.class?handleId                           │
│  JDT.LS's JDTUtils.toUri(IClassFile) produces:                        │
│    jdt://contents/jar/pkg/Foo.java?handleId&element=Foo.class         │
│  Different path (.class vs .java) + missing &element suffix +         │
│  different URL encoding → VS Code treats them as different files.     │
│  This affects ALL binary dependencies (JDK + third-party).            │
└─────────────────────────────────────────────────────────────────────────┘
```

## Solution

Three fixes applied via OSGi WeavingHook bytecode injection:

### Fix 1: Source Container Deduplication

**File**: `BazelSourceLookupFix.deduplicateContainers()`
**Hook**: `JavaDebugSourceLookupPatcher` (weaves `JdtUtils.getSourceContainers`)

Deduplicates `PackageFragmentRootSourceContainer` instances by their `IPackageFragmentRoot` path. JDK containers (identified by `jrt-fs`, `rt.jar`, `jre/lib` paths) use a global key; project-scoped containers include the project name. Reduces 64 → 14 containers.

### Fix 2: JDK Modular Source Lookup Fallback

**File**: `BazelSourceLookupFix.resolveSourceFileURI()` (originally `resolveJdkSourceFileURI`)
**Hook**: `JdkSourceLookupPatcher` (weaves `JdtSourceLookUpProvider.getSourceFileURI`)

When the original `getSourceFileURI` returns null or a mismatched URI, resolves the type via `IJavaProject.findType(fqn)`, obtains the `IClassFile`, and generates a proper URI via `JDTUtils.toUri()`.

### Fix 3: URI Normalization (all binary classes)

**File**: `BazelSourceLookupFix.resolveSourceFileURI()`
**Hook**: `JdkSourceLookupPatcher` (same hook as Fix 2)

Instead of building URIs manually, **always delegates to `JDTUtils.toUri(IClassFile)`** via reflection. This ensures debug URIs are identical to editor navigation URIs for:
- JDK classes (java.lang.String, java.util.Locale, etc.)
- Third-party JAR classes (com.google.common.base.Joiner, org.junit.Assert, etc.)
- Workspace binary dependencies

FQN-based `ConcurrentHashMap` cache avoids redundant `findType()` + `toUri()` calls.

## Architecture

```
                    OSGi Bundle Loading
                          │
                    BazelActivator.start()
                          │
               ┌──────────┴──────────┐
               │                     │
    JavaDebugSourceLookup-   JdkSourceLookup-
    Patcher (WeavingHook)    Patcher (WeavingHook)
               │                     │
    Weaves JdtUtils            Weaves JdtSourceLookUpProvider
    .getSourceContainers       .getSourceFileURI
               │                     │
    Calls deduplicate-         Calls resolveSourceFileURI
    Containers()               (fqn, sourcePath, originalUri)
                                     │
                              ┌──────┴──────┐
                              │ Cache hit?  │
                              └──────┬──────┘
                                   No │
                              findType(fqn) → IClassFile
                                     │
                              JDTUtils.toUri(classFile)
                              (via reflection)
                                     │
                              Cache + return
```

## Files

| File | Role |
|------|------|
| `BazelSourceLookupFix.java` | Deduplication + URI normalization + JDTUtils bridge |
| `JavaDebugSourceLookupPatcher.java` | WeavingHook for `JdtUtils.getSourceContainers` dedup |
| `JdkSourceLookupPatcher.java` | WeavingHook for `JdtSourceLookUpProvider.getSourceFileURI` fallback |
| `BazelActivator.java` | Registers both WeavingHooks as OSGi services |
| `BazelSourceLookupFixTest.java` | Unit tests (11 tests) |

## Testing

```bash
cd bazel-jdt-bridge/java-bridge
mvn test -Dtest=BazelSourceLookupFixTest   # 11 tests, 0 failures
mvn clean package -DskipTests               # Build OSGi bundle
```

Manual verification:
1. Debug into `java.lang.String.charAt()` → source file opens with breakpoint red dot
2. Debug into `com.google.common.base.Joiner.on()` → same file as Ctrl+Click navigation
3. Debug into workspace module code → unaffected (file:// URIs)
