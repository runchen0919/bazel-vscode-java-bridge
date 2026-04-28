package com.bazel.jdt;

import java.util.List;

import org.eclipse.core.runtime.ILog;
import org.eclipse.core.runtime.IStatus;
import org.eclipse.core.runtime.Platform;
import org.eclipse.core.runtime.Status;
import org.eclipse.core.runtime.IProgressMonitor;
import org.eclipse.jdt.ls.core.internal.IDelegateCommandHandler;

import com.bazel.jdt.BazelBridge;
import com.bazel.jdt.BazelClasspathManager;

public class BazelCommandHandler implements IDelegateCommandHandler {
    private static final ILog LOG = Platform.getLog(BazelCommandHandler.class);
    static final String DEFAULT_CACHE_DIR = System.getProperty("user.home", "") + "/.cache/bazel-jdt";

    @Override
    public Object executeCommand(String commandId, List<Object> arguments, IProgressMonitor monitor) {
        switch (commandId) {
            case "bazel-jdt.importProject":
                return handleImportProject(arguments);
            case "bazel-jdt.syncProject":
                return handleSyncProject();
            case "bazel-jdt.cleanCache":
                return handleCleanCache();
            case "bazel-jdt.getSyncState":
                return BazelBridge.getInstance().getSyncState();
            default:
                return null;
        }
    }

    private Object handleImportProject(List<Object> arguments) {
        try {
            BazelBridge bridge = BazelBridge.getInstance();
            String workspacePath = arguments.size() > 0 ? String.valueOf(arguments.get(0)) : "";
            String bazelPath = arguments.size() > 1 ? String.valueOf(arguments.get(1)) : "bazel";
            String cacheDir = arguments.size() > 2 ? String.valueOf(arguments.get(2)) : "";
            if (cacheDir.isEmpty()) {
                cacheDir = DEFAULT_CACHE_DIR;
            }
            bridge.initialize(workspacePath, bazelPath, cacheDir);
            String[] targets = bridge.discoverTargets();
            BazelClasspathManager.refreshClasspath();
            return null;
        } catch (Exception e) {
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt", "Bazel import failed", e));
            throw new RuntimeException("Bazel import failed: " + e.getMessage(), e);
        }
    }

    private Object handleSyncProject() {
        try {
            BazelClasspathManager.refreshClasspath();
            return null;
        } catch (Exception e) {
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt", "Bazel sync failed", e));
            throw new RuntimeException("Bazel sync failed: " + e.getMessage(), e);
        }
    }

    private Object handleCleanCache() {
        try {
            BazelBridge.getInstance().cleanCache();
            return null;
        } catch (Exception e) {
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt", "Bazel cache clean failed", e));
            throw new RuntimeException("Bazel cache clean failed: " + e.getMessage(), e);
        }
    }
}
