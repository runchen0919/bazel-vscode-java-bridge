package com.bazel.jdt;

import java.util.List;

import org.eclipse.core.resources.IProject;
import org.eclipse.core.resources.ResourcesPlugin;
import org.eclipse.core.runtime.ILog;
import org.eclipse.core.runtime.IProgressMonitor;
import org.eclipse.core.runtime.IStatus;
import org.eclipse.core.runtime.Platform;
import org.eclipse.core.runtime.Status;
import org.eclipse.jdt.ls.core.internal.IDelegateCommandHandler;
import org.eclipse.jdt.ls.core.internal.JobHelpers;

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
            case "bazel-jdt.getDependencyPackages":
                return handleGetDependencyPackages(arguments);
            case "bazel-jdt.createProjectForPackage":
                return handleCreateProjectForPackage(arguments, monitor);
            case "bazel-jdt.waitForIndexesReady":
                return handleWaitForIndexesReady();
            case "bazel-jdt.buildTarget":
                return handleBuildTarget(arguments);
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

            if (arguments.size() > 5 && arguments.get(5) instanceof String) {
                String mode = (String) arguments.get(5);
                bridge.setDependencyResolutionMode(mode);
                LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                    "Dependency resolution mode set to: " + mode));
            }

            if (arguments.size() > 6 && arguments.get(6) instanceof String) {
                String loadingMode = (String) arguments.get(6);
                bridge.setDependencySourceLoadingMode(loadingMode);
                LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                    "Dependency source loading mode set to: " + loadingMode));
            }

            if (arguments.size() > 7 && arguments.get(7) instanceof String) {
                String syncMode = (String) arguments.get(7);
                bridge.setSyncMode(syncMode);
                LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                    "Sync mode set to: " + syncMode));
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

    private Object handleGetDependencyPackages(List<Object> arguments) {
        try {
            BazelBridge bridge = BazelBridge.getInstance();
            String[] cached = bridge.getCachedDependencyPackages();
            if (cached != null && cached.length > 0) {
                return cached;
            }
            String[] scopePatterns = null;
            if (!arguments.isEmpty() && arguments.get(0) instanceof List) {
                @SuppressWarnings("unchecked")
                List<String> patterns = (List<String>) arguments.get(0);
                if (!patterns.isEmpty()) {
                    scopePatterns = patterns.toArray(new String[0]);
                }
            }
            String[] targets = bridge.discoverTargets(scopePatterns);
            String[] depPackages = bridge.getTransitiveWorkspaceDeps(targets);
            bridge.setCachedDependencyPackages(depPackages);
            return depPackages != null ? depPackages : new String[0];
        } catch (Exception e) {
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                "Failed to get dependency packages", e));
            return new String[0];
        }
    }

    private Object handleWaitForIndexesReady() {
        try {
            JobHelpers.waitUntilIndexesReady();
            return true;
        } catch (Exception e) {
            LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                "waitForIndexesReady failed: " + e.getMessage()));
            return false;
        }
    }

    private Object handleBuildTarget(List<Object> arguments) {
        try {
            if (arguments.isEmpty() || !(arguments.get(0) instanceof String)) {
                throw new IllegalArgumentException("Project name required");
            }
            String projectName = (String) arguments.get(0);

            IProject project = ResourcesPlugin.getWorkspace().getRoot().getProject(projectName);
            if (!project.exists()) {
                throw new IllegalArgumentException("Project not found: " + projectName);
            }

            List<String> targets = TargetProjectMapping.readTargets(project);
            if (targets.isEmpty()) {
                throw new IllegalStateException("No Bazel targets for project: " + projectName);
            }

            BazelBridge bridge = BazelBridge.getInstance();
            if (!bridge.isInitialized()) {
                throw new IllegalStateException(
                    "Bazel project not imported yet. Import the project first.");
            }
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Pre-debug build for " + projectName + ": " + targets));

            bridge.buildTargets(targets.toArray(new String[0]), null);

            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Pre-debug build complete for " + projectName));
            return null;
        } catch (Exception e) {
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                "Pre-debug build failed", e));
            throw new RuntimeException("Pre-debug build failed: " + e.getMessage(), e);
        }
    }

    private Object handleCreateProjectForPackage(List<Object> arguments, IProgressMonitor monitor) {
        try {
            if (arguments.isEmpty() || !(arguments.get(0) instanceof String)) {
                return null;
            }
            String packagePath = (String) arguments.get(0);
            String workspacePath = TargetProjectMapping.readWorkspacePath();
            if (workspacePath == null || workspacePath.isEmpty()) {
                LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                    "Cannot create project: workspace path not available"));
                return null;
            }
            String targetName = packagePath.contains("/")
                ? packagePath.substring(packagePath.lastIndexOf('/') + 1)
                : packagePath;
            String targetLabel = "//" + packagePath + ":" + targetName;
            IProject project = BazelProjectCreator.createProjectForPackage(
                workspacePath, packagePath, targetLabel, monitor);
            if (project != null) {
                BazelClasspathManager.refreshClasspath();
            }
            return project != null ? project.getName() : null;
        } catch (Exception e) {
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                "Failed to create project for package: " + arguments, e));
            return null;
        }
    }
}
