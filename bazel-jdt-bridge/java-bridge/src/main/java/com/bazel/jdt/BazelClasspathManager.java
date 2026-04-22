package com.bazel.jdt;

import org.eclipse.core.resources.IProject;
import org.eclipse.core.runtime.CoreException;
import org.eclipse.core.runtime.IPath;
import org.eclipse.core.runtime.Path;
import org.eclipse.jdt.core.ClasspathContainerInitializer;
import org.eclipse.jdt.core.IClasspathContainer;
import org.eclipse.jdt.core.JavaCore;

import java.util.ArrayList;
import java.util.List;

public class BazelClasspathManager {

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
            // Silently ignore classpath errors — JDT.LS will retry
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
                    continue;
                }
                for (String targetLabel : targets) {
                    setClasspathContainer(project, targetLabel);
                }
            }
        } catch (Exception e) {
            // Silently ignore refresh errors
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
            // Silently ignore refresh errors
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
            
            // For each changed BUILD file, compute the corresponding target label
            // using the project's location as the workspace root context
            for (String filePath : changedFiles) {
                // Extract package-relative path and convert to Bazel label
                // e.g., /workspace/foo/bar/BUILD -> //foo/bar:target
                String projectName = project.getName();
                if (filePath.contains(projectName)) {
                    // Add a label placeholder — actual resolution happens in Rust
                    labels.add("//" + projectName + ":*" );
                }
            }
        } catch (CoreException e) {
            // Project nature check failed
        }
        return labels;
    }
}
