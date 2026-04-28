package com.bazel.jdt;

public final class BazelBridge {
    private static final BazelBridge INSTANCE = new BazelBridge();
    private long handle = -1;

    static {
        NativeLoader.load();
    }

    private BazelBridge() {}

    public static BazelBridge getInstance() {
        return INSTANCE;
    }

    public synchronized void initialize(String workspacePath, String bazelPath, String cacheDir) {
        if (handle != -1) {
            shutdown();
        }
        handle = nativeInitialize(workspacePath, bazelPath, cacheDir);
    }

    public synchronized void shutdown() {
        if (handle != -1) {
            nativeShutdown(handle);
            handle = -1;
        }
    }

    public synchronized String[] discoverTargets() {
        checkHandle();
        return nativeDiscoverTargets(handle);
    }

    public synchronized String[] computeClasspath(String targetLabel) {
        checkHandle();
        return nativeComputeClasspath(handle, targetLabel);
    }

    public synchronized int getSyncState() {
        if (handle == -1) return 0;
        return nativeGetSyncState(handle);
    }

    public synchronized void cleanCache() {
        checkHandle();
        nativeCleanCache(handle);
    }

    public synchronized String[] getPendingChanges() {
        if (handle == -1) return new String[0];
        return nativeGetPendingChanges(handle);
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
