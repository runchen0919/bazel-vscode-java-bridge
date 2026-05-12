package com.bazel.jdt;

import java.util.Objects;
import java.util.concurrent.*;
import java.util.concurrent.locks.ReentrantReadWriteLock;

public final class BazelBridge {
    private static final BazelBridge INSTANCE = new BazelBridge();
    private static final long JNI_TIMEOUT_SECONDS = 330;
    private static final long DISCOVER_TIMEOUT_SECONDS = 1800;
    private long handle = -1;
    private final ReentrantReadWriteLock rwLock = new ReentrantReadWriteLock();
    private volatile ExecutorService jniExecutor = createExecutor();
    private String lastWorkspacePath;
    private String lastBazelPath;
    private String lastCacheDir;
    private volatile String dependencyResolutionMode = "transitive";
    private volatile String dependencySourceLoadingMode = "full-project";
    private volatile String[] cachedDependencyPackages = new String[0];

    private static ExecutorService createExecutor() {
        return Executors.newSingleThreadExecutor(r -> {
            Thread t = new Thread(r, "bazel-jdt-native");
            t.setDaemon(true);
            return t;
        });
    }

    static {
        NativeLoader.load();
    }

    private BazelBridge() {}

    public static BazelBridge getInstance() {
        return INSTANCE;
    }

    public void initialize(String workspacePath, String bazelPath, String cacheDir) {
        rwLock.writeLock().lock();
        try {
            if (handle != -1
                    && Objects.equals(workspacePath, lastWorkspacePath)
                    && Objects.equals(bazelPath, lastBazelPath)
                    && Objects.equals(cacheDir, lastCacheDir)) {
                return;
            }
            if (handle != -1) {
                nativeShutdown(handle);
                handle = -1;
            }
            if (jniExecutor.isShutdown() || jniExecutor.isTerminated()) {
                jniExecutor = createExecutor();
            }
            handle = nativeInitialize(workspacePath, bazelPath, cacheDir);
            lastWorkspacePath = workspacePath;
            lastBazelPath = bazelPath;
            lastCacheDir = cacheDir;
        } finally {
            rwLock.writeLock().unlock();
        }
    }

    public void shutdown() {
        rwLock.writeLock().lock();
        try {
            jniExecutor.shutdownNow();
            try {
                jniExecutor.awaitTermination(5, TimeUnit.SECONDS);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
            }
            if (handle != -1) {
                nativeShutdown(handle);
                handle = -1;
            }
            lastWorkspacePath = null;
            lastBazelPath = null;
            lastCacheDir = null;
        } finally {
            rwLock.writeLock().unlock();
        }
    }

    public String[] discoverTargets(String[] scopePatterns) {
        return discoverTargets(scopePatterns, null);
    }

    public String[] discoverTargets(String[] scopePatterns, String[] buildFlags) {
        String[] targets = queryTargets(scopePatterns);
        if (targets == null || targets.length == 0) return targets;
        populateGraph();
        return runAspectBuild(targets, buildFlags);
    }

    public String[] queryTargets(String[] scopePatterns) {
        long h = snapshotHandle();
        try {
            return jniExecutor.submit(() -> nativeQueryTargets(h, scopePatterns))
                .get(DISCOVER_TIMEOUT_SECONDS, TimeUnit.SECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new RuntimeException("Interrupted during queryTargets", e);
        } catch (ExecutionException e) {
            Throwable cause = e.getCause();
            if (cause instanceof RuntimeException) throw (RuntimeException) cause;
            throw new RuntimeException("queryTargets failed", cause);
        } catch (TimeoutException e) {
            throw new RuntimeException("queryTargets timed out", e);
        }
    }

    public void populateGraph() {
        long h = snapshotHandle();
        try {
            jniExecutor.submit(() -> { nativePopulateGraph(h); return null; })
                .get(DISCOVER_TIMEOUT_SECONDS, TimeUnit.SECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new RuntimeException("Interrupted during populateGraph", e);
        } catch (ExecutionException e) {
            Throwable cause = e.getCause();
            if (cause instanceof RuntimeException) throw (RuntimeException) cause;
            throw new RuntimeException("populateGraph failed", cause);
        } catch (TimeoutException e) {
            throw new RuntimeException("populateGraph timed out", e);
        }
    }

    public String[] runAspectBuild(String[] targets, String[] buildFlags) {
        long h = snapshotHandle();
        try {
            return jniExecutor.submit(() -> nativeRunAspectBuild(h, targets, buildFlags))
                .get(DISCOVER_TIMEOUT_SECONDS, TimeUnit.SECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new RuntimeException("Interrupted during runAspectBuild", e);
        } catch (ExecutionException e) {
            Throwable cause = e.getCause();
            if (cause instanceof RuntimeException) throw (RuntimeException) cause;
            throw new RuntimeException("runAspectBuild failed", cause);
        } catch (TimeoutException e) {
            throw new RuntimeException("runAspectBuild timed out", e);
        }
    }

    public String[] computeClasspath(String targetLabel) {
        long h = snapshotHandle();
        try {
            return jniExecutor.submit(() -> nativeComputeClasspath(h, targetLabel, null))
                .get(JNI_TIMEOUT_SECONDS, TimeUnit.SECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new RuntimeException("Interrupted during computeClasspath", e);
        } catch (ExecutionException e) {
            Throwable cause = e.getCause();
            if (cause instanceof RuntimeException) throw (RuntimeException) cause;
            throw new RuntimeException("computeClasspath failed for " + targetLabel, cause);
        } catch (TimeoutException e) {
            throw new RuntimeException("computeClasspath timed out for " + targetLabel, e);
        }
    }

    public String[] computeClasspathMerged(String[] labels) {
        if (labels == null || labels.length == 0) {
            return new String[0];
        }
        long h = snapshotHandle();
        try {
            return jniExecutor.submit(() -> nativeComputeClasspathMerged(h, labels))
                .get(JNI_TIMEOUT_SECONDS, TimeUnit.SECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new RuntimeException("Interrupted during computeClasspathMerged", e);
        } catch (ExecutionException e) {
            Throwable cause = e.getCause();
            if (cause instanceof RuntimeException) throw (RuntimeException) cause;
            throw new RuntimeException("computeClasspathMerged failed", cause);
        } catch (TimeoutException e) {
            throw new RuntimeException("computeClasspathMerged timed out", e);
        }
    }

    private static final int SYNC_STATE_DEAD = 3;

    public int getSyncState() {
        // Safe to bypass executor: nativeGetSyncState performs a single atomic read
        // of the sync state field in BazelJdtState — no locks, no I/O, no blocking.
        long h = snapshotHandleNullable();
        if (h == -1) return SYNC_STATE_DEAD;
        return nativeGetSyncState(h);
    }

    public boolean isInitialized() {
        rwLock.readLock().lock();
        try {
            return handle != -1;
        } finally {
            rwLock.readLock().unlock();
        }
    }

    public void setDependencyResolutionMode(String mode) {
        this.dependencyResolutionMode = mode;
    }

    public String getDependencyResolutionMode() {
        return this.dependencyResolutionMode;
    }

    public void setDependencySourceLoadingMode(String mode) {
        this.dependencySourceLoadingMode = mode;
    }

    public String getDependencySourceLoadingMode() {
        return this.dependencySourceLoadingMode;
    }

    public void setCachedDependencyPackages(String[] packages) {
        this.cachedDependencyPackages = packages != null ? packages : new String[0];
    }

    public String[] getCachedDependencyPackages() {
        return this.cachedDependencyPackages;
    }

    public void cleanCache() {
        long h = snapshotHandle();
        try {
            jniExecutor.submit(() -> { nativeCleanCache(h); return null; })
                .get(JNI_TIMEOUT_SECONDS, TimeUnit.SECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new RuntimeException("Interrupted during cleanCache", e);
        } catch (ExecutionException e) {
            Throwable cause = e.getCause();
            if (cause instanceof RuntimeException) throw (RuntimeException) cause;
            throw new RuntimeException("cleanCache failed", cause);
        } catch (TimeoutException e) {
            throw new RuntimeException("cleanCache timed out", e);
        }
    }

    public String[] getTransitiveWorkspaceDeps(String[] targetLabels) {
        long h = snapshotHandle();
        try {
            return jniExecutor.submit(() -> nativeGetTransitiveWorkspaceDeps(h, targetLabels))
                .get(JNI_TIMEOUT_SECONDS, TimeUnit.SECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new RuntimeException("Interrupted during getTransitiveWorkspaceDeps", e);
        } catch (ExecutionException e) {
            Throwable cause = e.getCause();
            if (cause instanceof RuntimeException) throw (RuntimeException) cause;
            throw new RuntimeException("getTransitiveWorkspaceDeps failed", cause);
        } catch (TimeoutException e) {
            throw new RuntimeException("getTransitiveWorkspaceDeps timed out", e);
        }
    }

    public String[] syncIncremental(String[] changedFilePaths) {
        long h = snapshotHandle();
        try {
            return jniExecutor.submit(() -> nativeSyncIncremental(h, changedFilePaths))
                .get(JNI_TIMEOUT_SECONDS, TimeUnit.SECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new RuntimeException("Interrupted during syncIncremental", e);
        } catch (ExecutionException e) {
            Throwable cause = e.getCause();
            if (cause instanceof RuntimeException) throw (RuntimeException) cause;
            throw new RuntimeException("syncIncremental failed", cause);
        } catch (TimeoutException e) {
            throw new RuntimeException("syncIncremental timed out", e);
        }
    }

    public void updateWatchPaths(String[] watchPaths) {
        long h = snapshotHandle();
        try {
            jniExecutor.submit(() -> {
                nativeUpdateWatchPaths(h, watchPaths != null ? watchPaths : new String[0]);
                return null;
            }).get(JNI_TIMEOUT_SECONDS, TimeUnit.SECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new RuntimeException("Interrupted during updateWatchPaths", e);
        } catch (ExecutionException e) {
            Throwable cause = e.getCause();
            if (cause instanceof RuntimeException) throw (RuntimeException) cause;
            throw new RuntimeException("updateWatchPaths failed", cause);
        } catch (TimeoutException e) {
            throw new RuntimeException("updateWatchPaths timed out", e);
        }
    }

    public String[] getPendingChanges() {
        long h = snapshotHandleNullable();
        if (h == -1) return new String[0];
        return nativeGetPendingChanges(h);
    }

    private long snapshotHandle() {
        rwLock.readLock().lock();
        try {
            if (handle == -1) {
                throw new IllegalStateException("BazelBridge not initialized");
            }
            return handle;
        } finally {
            rwLock.readLock().unlock();
        }
    }

    private long snapshotHandleNullable() {
        rwLock.readLock().lock();
        try {
            return handle;
        } finally {
            rwLock.readLock().unlock();
        }
    }

    private native long nativeInitialize(String workspacePath, String bazelPath, String cacheDir);
    private native void nativeShutdown(long handle);
    private native void nativeUpdateWatchPaths(long handle, String[] watchPaths);
    private native String[] nativeQueryTargets(long handle, String[] scopePatterns);
    private native void nativePopulateGraph(long handle);
    private native String[] nativeRunAspectBuild(long handle, String[] targets, String[] buildFlags);
    private native String[] nativeComputeClasspath(long handle, String targetLabel, String[] buildFlags);
    private native String[] nativeComputeClasspathMerged(long handle, String[] labels);
    private native int nativeGetSyncState(long handle);
    private native void nativeCleanCache(long handle);
    private native String[] nativeGetPendingChanges(long handle);
    private native String[] nativeGetTransitiveWorkspaceDeps(long handle, String[] targetLabels);
    private native String[] nativeSyncIncremental(long handle, String[] changedFilePaths);
    private native String nativeGetAspectBuildStats(long handle);

    public String getAspectBuildStats() {
        long h = snapshotHandleNullable();
        if (h == -1) return null;
        return nativeGetAspectBuildStats(h);
    }
}
