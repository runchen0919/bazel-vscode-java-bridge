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
import java.util.List;

public class BazelClasspathManager {
    private static final ILog LOG = Platform.getLog(BazelClasspathManager.class);

    public static void setClasspathContainer(IProject project, String targetLabel) {
        try {
            BazelBridge bridge = BazelBridge.getInstance();
            String[] rawEntries = bridge.computeClasspath(targetLabel);
            BazelClasspathContainer container = new BazelClasspathContainer(rawEntries);
            JavaCore.setClasspathContainer(
                BazelClasspathContainer.CONTAINER_PATH,
                new org.eclipse.jdt.core.IJavaProject[]{JavaCore.create(project)},
                new IClasspathContainer[]{container},
                null
            );
        } catch (Exception e) {
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                "Failed to set classpath container for " + targetLabel, e));
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

            BazelBridge bridge = BazelBridge.getInstance();
            String[] targets = bridge.discoverTargets();
            if (targets == null) return;

            for (IProject project : projects) {
                if (!project.isOpen()) continue;
                try {
                    if (!project.hasNature("org.eclipse.jdt.core.javanature")) continue;
                } catch (CoreException e) {
                    LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                        "Nature check failed for project " + project.getName(), e));
                    continue;
                }
                for (String targetLabel : targets) {
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
    private static List<String> extractTargetLabels(IProject project, List<String> changedFiles) {
        List<String> labels = new ArrayList<>();
        try {
            if (!project.isOpen() || !project.hasNature("com.bazel.jdt.bazelNature")) {
                return labels;
            }
        } catch (CoreException e) {
            LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                "Nature check failed for project " + project.getName(), e));
            return labels;
        }

        BazelBridge bridge = BazelBridge.getInstance();
        String[] pendingLabels = bridge.getPendingChanges();
        for (String label : pendingLabels) {
            if (!labels.contains(label)) {
                labels.add(label);
            }
        }

        if (labels.isEmpty()) {
            for (String filePath : changedFiles) {
                String projectName = project.getName();
                if (filePath.contains(projectName)) {
                    labels.add("//" + projectName + ":*");
                }
            }
        }
        return labels;
    }
}
