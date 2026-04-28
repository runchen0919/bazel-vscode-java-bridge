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
        rwLock.writeLock().lock();
        try {
            checkHandle();
            return nativeDiscoverTargets(handle);
        } finally {
            rwLock.writeLock().unlock();
        }
    }

    public String[] computeClasspath(String targetLabel) {
        rwLock.writeLock().lock();
        try {
            checkHandle();
            return nativeComputeClasspath(handle, targetLabel);
        } finally {
            rwLock.writeLock().unlock();
        }
    }

    private static final int SYNC_STATE_DEAD = 3;

    public int getSyncState() {
        rwLock.readLock().lock();
        try {
            if (handle == -1) return SYNC_STATE_DEAD;
            return nativeGetSyncState(handle);
        } finally {
            rwLock.readLock().unlock();
        }
    }

    public void cleanCache() {
        rwLock.writeLock().lock();
        try {
            checkHandle();
            nativeCleanCache(handle);
        } finally {
            rwLock.writeLock().unlock();
        }
    }

    public String[] getPendingChanges() {
        rwLock.readLock().lock();
        try {
            if (handle == -1) return new String[0];
            return nativeGetPendingChanges(handle);
        } finally {
            rwLock.readLock().unlock();
        }
    }

    private void checkHandle() {
        if (handle == -1) {
            throw new IllegalStateException("BazelBridge not initialized");
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
