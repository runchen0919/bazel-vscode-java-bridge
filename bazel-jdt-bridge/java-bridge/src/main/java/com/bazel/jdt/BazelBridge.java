package com.bazel.jdt;

import java.util.Objects;
import java.util.concurrent.*;
import java.util.concurrent.locks.ReentrantReadWriteLock;

public final class BazelBridge {
    private static final BazelBridge INSTANCE = new BazelBridge();
    private static final long JNI_TIMEOUT_SECONDS = 330;
    private long handle = -1;
    private final ReentrantReadWriteLock rwLock = new ReentrantReadWriteLock();
    private volatile ExecutorService jniExecutor = createExecutor();
    private String lastWorkspacePath;
    private String lastBazelPath;
    private String lastCacheDir;
    private volatile String dependencyResolutionMode = "transitive";

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

    public String[] discoverTargets() {
        return discoverTargets(null);
    }

    public String[] discoverTargets(String[] scopePatterns) {
        return discoverTargets(scopePatterns, null);
    }

    public String[] discoverTargets(String[] scopePatterns, String[] buildFlags) {
        long h = snapshotHandle();
        try {
            return jniExecutor.submit(() -> nativeDiscoverTargets(h, scopePatterns, buildFlags))
                .get(JNI_TIMEOUT_SECONDS, TimeUnit.SECONDS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new RuntimeException("Interrupted during discoverTargets", e);
        } catch (ExecutionException e) {
            Throwable cause = e.getCause();
            if (cause instanceof RuntimeException) throw (RuntimeException) cause;
            throw new RuntimeException("discoverTargets failed", cause);
        } catch (TimeoutException e) {
            throw new RuntimeException("discoverTargets timed out", e);
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

    private static final int SYNC_STATE_DEAD = 3;

    public int getSyncState() {
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

    public void cleanCache() {
        long h = snapshotHandle();
        nativeCleanCache(h);
    }

    public String[] getPendingChanges() {
        long h = snapshotHandleNullable();
        if (h == -1) return new String[0];
        return nativeGetPendingChanges(h);
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
    private native String[] nativeDiscoverTargets(long handle, String[] scopePatterns, String[] buildFlags);
    private native String[] nativeComputeClasspath(long handle, String targetLabel, String[] buildFlags);
    private native int nativeGetSyncState(long handle);
    private native void nativeCleanCache(long handle);
    private native String[] nativeGetPendingChanges(long handle);
    private native String[] nativeGetTransitiveWorkspaceDeps(long handle, String[] targetLabels);
}
