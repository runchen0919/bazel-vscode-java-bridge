package com.bazel.jdt;

import org.eclipse.core.resources.IProject;
import org.eclipse.core.runtime.CoreException;
import org.eclipse.core.runtime.ILog;
import org.eclipse.core.runtime.IPath;
import org.eclipse.core.runtime.IStatus;
import org.eclipse.core.runtime.Path;
import org.eclipse.core.runtime.Platform;
import org.eclipse.core.runtime.Status;
import org.eclipse.jdt.core.ClasspathContainerInitializer;
import org.eclipse.jdt.core.IClasspathContainer;
import org.eclipse.jdt.core.JavaCore;

import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Set;

public class BazelClasspathManager {
    private static final ILog LOG = Platform.getLog(BazelClasspathManager.class);

    public static void setClasspathContainer(IProject project, String targetLabel) {
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
            StringBuilder sb = new StringBuilder();
            sb.append("Classpath for '").append(targetLabel).append("' (").append(rawEntries == null ? "null" : rawEntries.length).append(" entries):");
            if (rawEntries != null) {
                for (String e : rawEntries) {
                    sb.append("\n  ").append(e);
                }
            }
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt", sb.toString()));
            BazelClasspathContainer container = new BazelClasspathContainer(
                rawEntries, getTestSourcePatterns(project),
                bridge.getDependencyResolutionMode());
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
        try {
            org.eclipse.core.resources.IWorkspace workspace =
                org.eclipse.core.resources.ResourcesPlugin.getWorkspace();
            IProject[] projects = workspace.getRoot().getProjects();

            for (IProject project : projects) {
                if (!project.isOpen()) continue;
                try {
                    if (!project.hasNature("org.eclipse.jdt.core.javanature")) continue;
                } catch (CoreException e) {
                    LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                        "Nature check failed for project " + project.getName(), e));
                    continue;
                }
                List<String> targetLabels = TargetProjectMapping.readTargets(project);
                for (String targetLabel : targetLabels) {
                    setClasspathContainer(project, targetLabel);
                }
            }
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
                for (String targetLabel : targetLabels) {
                    setClasspathContainer(project, targetLabel);
                }
            }
        } catch (Exception e) {
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                "Failed to refresh classpath for changed files", e));
        }
    }

    /**
     * Extract target labels from a project that are affected by the given changed files.
     */
    private static List<String> getTestSourcePatterns(IProject project) {
        try {
            org.eclipse.core.resources.IWorkspaceRoot wsRoot = project.getWorkspace().getRoot();
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
