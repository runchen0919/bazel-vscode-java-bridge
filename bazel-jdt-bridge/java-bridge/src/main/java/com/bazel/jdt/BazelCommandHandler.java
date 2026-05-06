package com.bazel.jdt;

import java.util.List;

import org.eclipse.core.runtime.ILog;
import org.eclipse.core.runtime.IProgressMonitor;
import org.eclipse.core.runtime.IStatus;
import org.eclipse.core.runtime.Platform;
import org.eclipse.core.runtime.Status;
import org.eclipse.jdt.ls.core.internal.IDelegateCommandHandler;

public class BazelCommandHandler implements IDelegateCommandHandler {
    private static final ILog LOG = Platform.getLog(BazelCommandHandler.class);
    static final String DEFAULT_CACHE_DIR = System.getProperty("user.home", "") + "/.cache/bazel-jdt";

    @Override
    public Object executeCommand(String commandId, List<Object> arguments, IProgressMonitor monitor) {
        switch (commandId) {
            case "bazel-jdt.importProject":
                return handleImportProject(arguments);
            case "bazel-jdt.syncProject":
                return handleSyncProject(arguments);
            case "bazel-jdt.cleanCache":
                return handleCleanCache();
            case "bazel-jdt.getSyncState":
                return BazelBridge.getInstance().getSyncState();
            case "bazel-jdt.shutdown":
                return handleShutdown();
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

            String[] scopePatterns = null;
            if (arguments.size() > 3 && arguments.get(3) instanceof List) {
                @SuppressWarnings("unchecked")
                List<String> patterns = (List<String>) arguments.get(3);
                if (!patterns.isEmpty()) {
                    scopePatterns = patterns.toArray(new String[0]);
                }
            }

            String[] buildFlags = null;
            if (arguments.size() > 4 && arguments.get(4) instanceof List) {
                @SuppressWarnings("unchecked")
                List<String> flags = (List<String>) arguments.get(4);
                if (!flags.isEmpty()) {
                    buildFlags = flags.toArray(new String[0]);
                }
            }

            // Accept resolution mode from TypeScript (argument index 5)
            if (arguments.size() > 5 && arguments.get(5) instanceof String) {
                String mode = (String) arguments.get(5);
                bridge.setDependencyResolutionMode(mode);
                LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                    "Dependency resolution mode set to: " + mode));
            }

            String[] targets = bridge.discoverTargets(scopePatterns, buildFlags);
            BazelClasspathManager.refreshClasspath();
            return null;
        } catch (Exception e) {
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt", "Bazel import failed", e));
            throw new RuntimeException("Bazel import failed: " + e.getMessage(), e);
        }
    }

    private Object handleSyncProject(List<Object> arguments) {
        try {
            if (!arguments.isEmpty() && arguments.get(0) instanceof String) {
                BazelBridge.getInstance().setDependencyResolutionMode((String) arguments.get(0));
            }
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
            TargetProjectMapping.clearClasspathCache();
            return null;
        } catch (Exception e) {
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt", "Bazel cache clean failed", e));
            throw new RuntimeException("Bazel cache clean failed: " + e.getMessage(), e);
        }
    }

    private Object handleShutdown() {
        try {
            BazelBridge.getInstance().shutdown();
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt", "Bazel bridge shut down via command"));
        } catch (Exception e) {
            LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt", "Bazel bridge shutdown failed", e));
        }
        return null;
    }
}
