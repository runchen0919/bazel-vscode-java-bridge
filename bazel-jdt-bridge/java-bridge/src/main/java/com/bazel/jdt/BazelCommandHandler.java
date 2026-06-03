package com.bazel.jdt;

import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

import org.eclipse.core.resources.IProject;
import org.eclipse.core.resources.IWorkspace;
import org.eclipse.core.resources.ResourcesPlugin;
import org.eclipse.core.runtime.CoreException;
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
            case "bazel-jdt.setActiveDebugProject":
                return handleSetActiveDebugProject(arguments);
            case "bazel-jdt.clearActiveDebugProject":
                return handleClearActiveDebugProject();
            case "bazel-jdt.partialSync":
                return handlePartialSync(arguments);
            default:
                return null;
        }
    }

    private Object handleClearActiveDebugProject() {
        BazelRuntimeClasspathEntryResolver.clearActiveDebugProject();
        return null;
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

            BazelProjectView projectView = BazelProjectView.parse(new java.io.File(workspacePath));
            if (projectView != null) {
                bridge.setProjectView(projectView);
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

            String[] targets = bridge.discoverTargets(scopePatterns, bridge.getBuildFlags());

            java.util.List<String> newTargetLabels = createProjectsForNewTargets(workspacePath, targets, bridge);

            if (!newTargetLabels.isEmpty()) {
                BazelClasspathManager.refreshClasspathForTargets(newTargetLabels);
            } else {
                BazelClasspathManager.refreshClasspath();
            }
            return null;
        } catch (Exception e) {
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt", "Bazel import failed", e));
            throw new RuntimeException("Bazel import failed: " + e.getMessage(), e);
        }
    }

    private List<String> createProjectsForNewTargets(String workspacePath, String[] targets, BazelBridge bridge) {
        Set<String> existingTargetLabels = getExistingTargetLabels();
        Set<String> newTargets = findNewTargets(targets, existingTargetLabels);

        LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
            "Discovered " + newTargets.size() + " new targets (existing: " + existingTargetLabels.size() + ")"));

        if (!newTargets.isEmpty()) {
            createProjectsForTargets(workspacePath, newTargets, bridge);
        }
        return new ArrayList<>(newTargets);
    }

    private Set<String> getExistingTargetLabels() {
        Set<String> existingTargetLabels = new HashSet<>();
        IWorkspace workspace = ResourcesPlugin.getWorkspace();
        for (IProject project : workspace.getRoot().getProjects()) {
            if (!project.isOpen()) continue;
            try {
                if (!project.hasNature(BazelNature.NATURE_ID)) continue;
            } catch (CoreException e) {
                continue;
            }
            List<String> labels = TargetProjectMapping.readTargets(project);
            existingTargetLabels.addAll(labels);
        }
        LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
            "Found " + existingTargetLabels.size() + " existing targets in " + workspace.getRoot().getProjects().length + " projects"));
        return existingTargetLabels;
    }

    private Set<String> findNewTargets(String[] targets, Set<String> existingTargetLabels) {
        Set<String> newTargets = new HashSet<>();
        if (targets != null) {
            for (String target : targets) {
                if (!existingTargetLabels.contains(target)) {
                    newTargets.add(target);
                }
            }
        }
        return newTargets;
    }

    private void createProjectsForTargets(String workspacePath, Set<String> newTargets, BazelBridge bridge) {
        LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
            "Creating projects for " + newTargets.size() + " new targets: " + newTargets));
        for (String targetLabel : newTargets) {
            try {
                String packagePath = LabelUtils.extractPackageName(targetLabel);
                boolean isTestTarget = bridge.isTestTarget(targetLabel);
                IProject project =
                    BazelProjectCreator.createProjectForPackage(
                        workspacePath, packagePath, targetLabel, null, true, isTestTarget);
                if (project != null) {
                    LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                        "Created project for target: " + targetLabel));
                }
            } catch (Exception e) {
                LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                    "Failed to create project for target: " + targetLabel, e));
            }
        }
    }

    private Object handlePartialSync(List<Object> arguments) {
        try {
            if (arguments.isEmpty() || !(arguments.get(0) instanceof String)) {
                throw new IllegalArgumentException("Scope pattern required");
            }
            String scopePattern = (String) arguments.get(0);

            BazelBridge bridge = BazelBridge.getInstance();
            if (!bridge.isInitialized()) {
                throw new IllegalStateException(
                    "Bazel project not imported yet. Import the project first.");
            }

            String syncMode = arguments.size() > 1 && arguments.get(1) instanceof String
                ? (String) arguments.get(1) : bridge.getSyncMode();
            bridge.setSyncMode(syncMode);

            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Partial sync: querying targets for " + scopePattern));
            String[] targets = bridge.queryTargets(new String[]{scopePattern});
            if (targets == null || targets.length == 0) {
                LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                    "Partial sync: no targets found for " + scopePattern));
                Map<String, Object> result = new HashMap<>();
                result.put("refreshed", 0);
                result.put("newTargets", new ArrayList<String>());
                return result;
            }

            String[] rdeps = bridge.getReverseDepsInProjects(targets);
            int rdepCount = rdeps != null ? rdeps.length : 0;
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Partial sync: " + targets.length + " scope targets, " + rdepCount + " rdep targets"));

            Set<String> mergedSet = new java.util.LinkedHashSet<>();
            for (String t : targets) mergedSet.add(t);
            if (rdeps != null) {
                for (String r : rdeps) mergedSet.add(r);
            }
            String[] mergedTargets = mergedSet.toArray(new String[0]);

            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Partial sync: running aspect build for " + mergedTargets.length + " targets (scope + rdeps)"));
            bridge.runAspectBuild(mergedTargets, bridge.getBuildFlags());

            Set<String> existingTargetLabels = getExistingTargetLabels();
            List<String> existingTargets = new ArrayList<>();
            List<String> newTargets = new ArrayList<>();
            for (String label : mergedTargets) {
                if (!label.startsWith("//")) {
                    LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                        "Partial sync: skipping invalid label: " + label));
                    continue;
                }
                if (existingTargetLabels.contains(label)) {
                    existingTargets.add(label);
                } else {
                    newTargets.add(label);
                }
            }

            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Partial sync: " + existingTargets.size() + " existing, "
                + newTargets.size() + " new targets"));

            if (!existingTargets.isEmpty()) {
                BazelClasspathManager.refreshClasspathForTargets(existingTargets, true);
            }

            Map<String, Object> result = new HashMap<>();
            result.put("refreshed", existingTargets.size());
            result.put("newTargets", newTargets);
            result.put("rdeps", rdepCount);
            return result;
        } catch (Exception e) {
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                "Partial sync failed", e));
            throw new RuntimeException("Partial sync failed: " + e.getMessage(), e);
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

            boolean buildSuccess = bridge.buildTargets(
                targets.toArray(new String[0]), bridge.getBuildFlags());
            if (!buildSuccess) {
                String msg = "Bazel build failed for targets: " + targets
                    + " (project: " + projectName + ")";
                LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt", msg));
                throw new RuntimeException(msg);
            }

            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Pre-debug build complete for " + projectName));

            BazelClasspathContainer.resetWarnings();
            BazelClasspathManager.setMergedClasspathContainer(project, true);
            BazelRuntimeClasspathEntryResolver.clearCacheForProject(projectName);

            return null;
        } catch (Exception e) {
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                "Pre-debug build failed", e));
            throw new RuntimeException("Pre-debug build failed: " + e.getMessage(), e);
        }
    }

    private Object handleSetActiveDebugProject(List<Object> arguments) {
        if (!arguments.isEmpty() && arguments.get(0) instanceof String) {
            BazelRuntimeClasspathEntryResolver.setActiveDebugProject((String) arguments.get(0));
        }
        return null;
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
