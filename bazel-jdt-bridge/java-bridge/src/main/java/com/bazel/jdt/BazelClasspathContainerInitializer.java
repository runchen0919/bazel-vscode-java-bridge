package com.bazel.jdt;

import java.util.Collections;
import java.util.List;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;

import org.eclipse.core.resources.IProject;
import org.eclipse.core.runtime.CoreException;
import org.eclipse.core.runtime.ILog;
import org.eclipse.core.runtime.IPath;
import org.eclipse.core.runtime.IStatus;
import org.eclipse.core.runtime.Platform;
import org.eclipse.core.runtime.Status;
import org.eclipse.jdt.core.ClasspathContainerInitializer;
import org.eclipse.jdt.core.IClasspathContainer;
import org.eclipse.jdt.core.IJavaProject;
import org.eclipse.jdt.core.JavaCore;

public class BazelClasspathContainerInitializer extends ClasspathContainerInitializer {

    private static final ILog LOG = Platform.getLog(BazelClasspathContainerInitializer.class);
    private static final Set<String> INITIALIZING = ConcurrentHashMap.newKeySet();

    @Override
    public void initialize(IPath containerPath, IJavaProject project) throws CoreException {
        if (!BazelClasspathContainer.CONTAINER_PATH.equals(containerPath)) {
            return;
        }
        String projectName = project.getProject().getName();
        if (!INITIALIZING.add(projectName)) {
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Skipping recursive initialize for project " + projectName));
            return;
        }
        try {
            doInitialize(project);
        } finally {
            INITIALIZING.remove(projectName);
        }
    }

    private void doInitialize(IJavaProject project) throws CoreException {
        BazelBridge bridge = BazelBridge.getInstance();
        if (!bridge.isInitialized()) {
            recoverFromCache(project, bridge);
            return;
        }
        List<String> targetLabels = TargetProjectMapping.readTargets(project.getProject());
        if (targetLabels.isEmpty()) {
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "No persisted target labels for project '" + project.getProject().getName()
                + "' - setting empty container (importer will configure)"));
            JavaCore.setClasspathContainer(
                BazelClasspathContainer.CONTAINER_PATH,
                new IJavaProject[]{project},
                new IClasspathContainer[]{BazelClasspathContainer.EMPTY},
                null
            );
            return;
        } else {
            BazelClasspathManager.setMergedClasspathContainer(project.getProject());
        }
    }

    private void recoverFromCache(IJavaProject project, BazelBridge bridge) {
        List<String> targetLabels = TargetProjectMapping.readTargets(project.getProject());
        if (targetLabels.isEmpty()) {
            LOG.info("No persisted targets for " + project.getProject().getName()
                + " — skipping container initialization until import runs");
            return;
        }
        java.util.ArrayList<String> allEntries = new java.util.ArrayList<>();
        for (String label : targetLabels) {
            String[] cached = TargetProjectMapping.readCachedClasspath(project.getProject(), label);
            if (cached != null) {
                Collections.addAll(allEntries, cached);
            }
        }
        if (allEntries.isEmpty()) {
            LOG.info("No cached classpath for " + project.getProject().getName()
                + " — skipping until import provides entries");
            return;
        }
        try {
            BazelClasspathContainer container = new BazelClasspathContainer(
                allEntries.toArray(new String[0]), Collections.emptyList(),
                bridge.getDependencyResolutionMode(),
                project.getProject().getName());
            JavaCore.setClasspathContainer(
                BazelClasspathContainer.CONTAINER_PATH,
                new IJavaProject[]{project},
                new IClasspathContainer[]{container},
                null
            );
        } catch (Exception e) {
            LOG.warn("Failed to apply cached classpath for " + project.getProject().getName()
                + ": " + e.getMessage());
        }
    }

    @Override
    public boolean canUpdateClasspathContainer(IPath containerPath, IJavaProject project) {
        return true;
    }

    @Override
    public String getDescription(IPath containerPath, IJavaProject project) {
        return "Bazel Dependencies";
    }
}
