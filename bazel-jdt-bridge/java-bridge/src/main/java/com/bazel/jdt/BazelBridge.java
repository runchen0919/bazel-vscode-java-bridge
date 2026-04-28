package com.bazel.jdt;

import java.util.concurrent.locks.ReentrantReadWriteLock;

public final class BazelBridge {
    private static final BazelBridge INSTANCE = new BazelBridge();
    private long handle = -1;
    private final ReentrantReadWriteLock rwLock = new ReentrantReadWriteLock();

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
            if (handle != -1) {
                nativeShutdown(handle);
                handle = -1;
            }
            handle = nativeInitialize(workspacePath, bazelPath, cacheDir);
        } finally {
            rwLock.writeLock().unlock();
        }
    }

    public void shutdown() {
        rwLock.writeLock().lock();
        try {
            if (handle != -1) {
                nativeShutdown(handle);
                handle = -1;
            }
        } finally {
            rwLock.writeLock().unlock();
        }
    }

    public String[] discoverTargets() {
        long h = snapshotHandle();
        return nativeDiscoverTargets(h);
    }

    public String[] computeClasspath(String targetLabel) {
        long h = snapshotHandle();
        return nativeComputeClasspath(h, targetLabel);
    }

    private static final int SYNC_STATE_DEAD = 3;

    public int getSyncState() {
        long h = snapshotHandleNullable();
        if (h == -1) return SYNC_STATE_DEAD;
        return nativeGetSyncState(h);
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
    private native String[] nativeDiscoverTargets(long handle);
    private native String[] nativeComputeClasspath(long handle, String targetLabel);
    private native int nativeGetSyncState(long handle);
    private native void nativeCleanCache(long handle);
    private native String[] nativeGetPendingChanges(long handle);
}
