package com.bazel.jdt;

import org.eclipse.core.resources.IProject;
import org.eclipse.core.runtime.CoreException;
import org.eclipse.core.runtime.ILog;
import org.eclipse.core.runtime.IStatus;
import org.eclipse.core.runtime.Platform;
import org.eclipse.core.runtime.Status;
import org.eclipse.jdt.core.IClasspathContainer;
import org.eclipse.jdt.core.JavaCore;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

public class BazelClasspathManager {
    private static final ILog LOG = Platform.getLog(BazelClasspathManager.class);
    private static final String CONFIG_CHANGED_SENTINEL = "__CONFIG_CHANGED__";
    private static final int BATCH_SIZE = 50;

    public static void setMergedClasspathContainer(IProject project) {
        try {
            BazelBridge bridge = BazelBridge.getInstance();
            if (!bridge.isInitialized()) {
                LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                    "Bridge not initialized, using empty container for " + project.getName()));
                JavaCore.setClasspathContainer(
                    BazelClasspathContainer.CONTAINER_PATH,
                    new org.eclipse.jdt.core.IJavaProject[]{JavaCore.create(project)},
                    new IClasspathContainer[]{BazelClasspathContainer.EMPTY},
                    null
                );
                return;
            }
            List<String> targetLabels = TargetProjectMapping.readTargets(project);
            if (targetLabels.isEmpty()) {
                LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                    "No target labels for project " + project.getName()));
                return;
            }
            String[] labels = targetLabels.toArray(new String[0]);
            String[] rawEntries = bridge.computeClasspathMerged(labels);

            String[] cachedEntries = TargetProjectMapping.readCachedClasspath(project, targetLabels.get(0));
            if (cachedEntries != null && Arrays.equals(rawEntries, cachedEntries)) {
                LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                    "Classpath unchanged for project " + project.getName() + ", skipping container update"));
                return;
            }

            BazelClasspathContainer container = new BazelClasspathContainer(
                rawEntries, getTestSourcePatterns(project),
                bridge.getDependencyResolutionMode(), project.getName());
            TargetProjectMapping.storeCachedClasspath(project, targetLabels.get(0), rawEntries);
            JavaCore.setClasspathContainer(
                BazelClasspathContainer.CONTAINER_PATH,
                new org.eclipse.jdt.core.IJavaProject[]{JavaCore.create(project)},
                new IClasspathContainer[]{container},
                null
            );
        } catch (Exception e) {
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                "FAILED setMergedClasspathContainer for project " + project.getName() + ": " + e.getMessage(), e));
        }
    }

    static void setClasspathContainer(IProject project, String targetLabel) {
        try {
            BazelBridge bridge = BazelBridge.getInstance();
            if (!bridge.isInitialized()) {
                LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                    "Bridge not initialized, using empty container for " + targetLabel));
                JavaCore.setClasspathContainer(
                    BazelClasspathContainer.CONTAINER_PATH,
                    new org.eclipse.jdt.core.IJavaProject[]{JavaCore.create(project)},
                    new IClasspathContainer[]{BazelClasspathContainer.EMPTY},
                    null
                );
                return;
            }
            String[] rawEntries = bridge.computeClasspath(targetLabel);

            if (rawEntries == null || rawEntries.length == 0) {
                try {
                    IClasspathContainer existing = JavaCore.getClasspathContainer(
                        BazelClasspathContainer.CONTAINER_PATH, JavaCore.create(project));
                    if (existing != null && existing.getClasspathEntries().length > 0) {
                        LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                            "Skipping empty classpath for '" + targetLabel
                            + "' - project " + project.getName()
                            + " already has " + existing.getClasspathEntries().length + " entries"));
                        return;
                    }
                } catch (Exception ex) {
                    LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                        "Could not check existing container: " + ex.getMessage()));
                }
            }

            BazelClasspathContainer container = new BazelClasspathContainer(
                rawEntries, getTestSourcePatterns(project),
                bridge.getDependencyResolutionMode(), project.getName());
            TargetProjectMapping.storeCachedClasspath(project, targetLabel, rawEntries);
            JavaCore.setClasspathContainer(
                BazelClasspathContainer.CONTAINER_PATH,
                new org.eclipse.jdt.core.IJavaProject[]{JavaCore.create(project)},
                new IClasspathContainer[]{container},
                null
            );
        } catch (Exception e) {
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                "FAILED setClasspathContainer for " + targetLabel + " in project " + project.getName() + ": " + e.getMessage(), e));
        }
    }

    /**
     * Refresh classpath for all open Bazel projects.
     * Called by BazelCommandHandler for import/sync commands.
     */
    public static void refreshClasspath() {
        BazelClasspathContainer.resetWarnings();
        try {
            org.eclipse.core.resources.IWorkspace workspace =
                org.eclipse.core.resources.ResourcesPlugin.getWorkspace();
            IProject[] projects = workspace.getRoot().getProjects();

            List<IProject> eligible = new ArrayList<>();
            for (IProject project : projects) {
                if (!project.isOpen()) continue;
                try {
                    if (!project.hasNature("org.eclipse.jdt.core.javanature")) continue;
                } catch (CoreException e) {
                    LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                        "Nature check failed for project " + project.getName(), e));
                    continue;
                }
                eligible.add(project);
            }

            int total = eligible.size();
            int totalBatches = (total + BATCH_SIZE - 1) / BATCH_SIZE;
            long startTime = System.currentTimeMillis();

            for (int i = 0; i < total; i += BATCH_SIZE) {
                int batchNum = (i / BATCH_SIZE) + 1;
                int end = Math.min(i + BATCH_SIZE, total);
                LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                    "Resolving classpath batch " + batchNum + "/" + totalBatches
                    + " (projects " + (i + 1) + "-" + end + " of " + total + ")"));

                for (int j = i; j < end; j++) {
                    setMergedClasspathContainer(eligible.get(j));
                }

                if (end < total) {
                    Thread.yield();
                }
            }

            long elapsed = System.currentTimeMillis() - startTime;
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Classpath resolution complete: " + total + " projects in " + elapsed + "ms"));
        } catch (Exception e) {
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                "Failed to refresh classpath", e));
        }
    }

    /**
     * Refresh classpath for projects affected by changed BUILD files.
     * Called by BazelBuildSupport when file changes are detected.
     */
    public static void refreshClasspathForFiles(List<String> changedFiles) {
        try {
            org.eclipse.core.resources.IWorkspace workspace = 
                org.eclipse.core.resources.ResourcesPlugin.getWorkspace();
            IProject[] projects = workspace.getRoot().getProjects();
            
            for (IProject project : projects) {
                List<String> targetLabels = extractTargetLabels(project, changedFiles);
                if (!targetLabels.isEmpty()) {
                    setMergedClasspathContainer(project);
                }
            }
        } catch (Exception e) {
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                "Failed to refresh classpath for changed files", e));
        }
    }

    /**
     * Refresh classpath for specific target labels.
     * Used by incremental sync to update only affected targets.
     */
    public static void refreshClasspathForTargets(List<String> targetLabels) {
        if (targetLabels == null || targetLabels.isEmpty()) return;
        try {
            org.eclipse.core.resources.IWorkspace workspace =
                org.eclipse.core.resources.ResourcesPlugin.getWorkspace();
            IProject[] projects = workspace.getRoot().getProjects();

            // Build target → project mapping
            Map<String, IProject> targetToProject = new HashMap<>();
            for (IProject project : projects) {
                if (!project.isOpen()) continue;
                List<String> stored = TargetProjectMapping.readTargets(project);
                for (String label : stored) {
                    targetToProject.put(label, project);
                }
            }

            // Collect affected projects and refresh each once with merged classpath
            Set<IProject> affectedProjects = new LinkedHashSet<>();
            for (String targetLabel : targetLabels) {
                IProject project = targetToProject.get(targetLabel);
                if (project != null && project.isOpen()) {
                    affectedProjects.add(project);
                }
            }
            for (IProject project : affectedProjects) {
                setMergedClasspathContainer(project);
            }
        } catch (Exception e) {
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                "Failed to refresh classpath for targets", e));
        }
    }

    /**
     * Extract target labels from a project that are affected by the given changed files.
     */
    private static List<String> getTestSourcePatterns(IProject project) {
        try {
            org.eclipse.core.resources.IWorkspaceRoot wsRoot = project.getWorkspace().getRoot();
            if (wsRoot.getLocation() == null) {
                return Collections.emptyList();
            }
            java.io.File workspaceRoot = wsRoot.getLocation().toFile();
            BazelProjectView projectView = BazelProjectView.parse(workspaceRoot);
            if (projectView != null && !projectView.getTestSourcePatterns().isEmpty()) {
                return projectView.getTestSourcePatterns();
            }
        } catch (Exception e) {
            LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                "Failed to get test source patterns", e));
        }
        return Collections.emptyList();
    }

    private static void handleConfigChanged(IProject project, BazelBridge bridge) {
        try {
            org.eclipse.core.resources.IWorkspaceRoot wsRoot = project.getWorkspace().getRoot();
            if (wsRoot.getLocation() == null) {
                LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                    "Cannot handle config change: workspace location is null (remote workspace?)"));
                return;
            }
            java.io.File workspaceRoot = wsRoot.getLocation().toFile();
            BazelProjectView projectView = BazelProjectView.parse(workspaceRoot);
            if (projectView != null && !projectView.getDirectories().isEmpty()) {
                String[] watchDirs = projectView.getDirectories().toArray(new String[0]);
                bridge.updateWatchPaths(watchDirs);
                LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                    ".bazelproject changed, watcher updated to " + watchDirs.length + " directories"));
            }
        } catch (Exception e) {
            LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                "Failed to update watch paths after .bazelproject change: " + e.getMessage(), e));
        }
    }

    private static List<String> extractTargetLabels(IProject project, List<String> changedFiles) {
        Set<String> labels = new LinkedHashSet<>();
        try {
            if (!project.isOpen() || !project.hasNature(BazelNature.NATURE_ID)) {
                return new ArrayList<>(labels);
            }
        } catch (CoreException e) {
            LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                "Nature check failed for project " + project.getName(), e));
            return new ArrayList<>(labels);
        }

        BazelBridge bridge = BazelBridge.getInstance();
        String[] pendingLabels = bridge.getPendingChanges();
        String projectName = project.getName();

        for (String pending : pendingLabels) {
            if (CONFIG_CHANGED_SENTINEL.equals(pending)) {
                handleConfigChanged(project, bridge);
                continue;
            }
            String pendingPackage = pending.startsWith("//") ? pending.substring(2) : pending;
            if (projectName.equals(pendingPackage)) {
                List<String> stored = TargetProjectMapping.readTargets(project);
                labels.addAll(stored);
            }
        }

        if (labels.isEmpty()) {
            for (String filePath : changedFiles) {
                if (filePath.contains(projectName)) {
                    List<String> stored = TargetProjectMapping.readTargets(project);
                    labels.addAll(stored);
                    break;
                }
            }
        }
        return new ArrayList<>(labels);
    }
}
