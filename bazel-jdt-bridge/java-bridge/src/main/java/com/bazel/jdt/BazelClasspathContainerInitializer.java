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
    private static volatile boolean IMPORT_IN_PROGRESS = false;

    public static void setImportInProgress(boolean inProgress) {
        IMPORT_IN_PROGRESS = inProgress;
    }

    @Override
    public void initialize(IPath containerPath, IJavaProject project) throws CoreException {
        if (!BazelClasspathContainer.CONTAINER_PATH.equals(containerPath)) {
            return;
        }
        String projectName = project.getProject().getName();

        if (IMPORT_IN_PROGRESS) {
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Skipping container resolve during import for project " + projectName));
            JavaCore.setClasspathContainer(
                BazelClasspathContainer.CONTAINER_PATH,
                new IJavaProject[]{project},
                new IClasspathContainer[]{BazelClasspathContainer.EMPTY},
                null
            );
            return;
        }

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
        if (tryRecoverFromCache(project, bridge)) {
            return;
        }
        if (bridge.isInitialized()) {
            List<String> targetLabels = TargetProjectMapping.readTargets(project.getProject());
            if (!targetLabels.isEmpty()) {
                BazelClasspathManager.setMergedClasspathContainer(project.getProject());
                return;
            }
        }
        LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
            "No persisted target labels for project '" + project.getProject().getName()
            + "' - setting empty container (importer will configure)"));
        JavaCore.setClasspathContainer(
            BazelClasspathContainer.CONTAINER_PATH,
            new IJavaProject[]{project},
            new IClasspathContainer[]{BazelClasspathContainer.EMPTY},
            null
        );
    }

    private boolean tryRecoverFromCache(IJavaProject project, BazelBridge bridge) {
        List<String> targetLabels = TargetProjectMapping.readTargets(project.getProject());
        if (targetLabels.isEmpty()) {
            return false;
        }
        java.util.ArrayList<String> allEntries = new java.util.ArrayList<>();
        for (String label : targetLabels) {
            String[] cached = TargetProjectMapping.readCachedClasspath(project.getProject(), label);
            if (cached != null) {
                Collections.addAll(allEntries, cached);
            }
        }
        if (allEntries.isEmpty()) {
            return false;
        }
        try {
            BazelClasspathContainer container = new BazelClasspathContainer(
                allEntries.toArray(new String[0]), Collections.emptyList(),
                bridge.getDependencyResolutionMode(),
                project.getProject().getName());
            if (container.getClasspathEntries().length == 0) {
                LOG.info("All cached classpath entries for " + project.getProject().getName()
                    + " reference stale artifacts — skipping cache recovery");
                return false;
            }
            JavaCore.setClasspathContainer(
                BazelClasspathContainer.CONTAINER_PATH,
                new IJavaProject[]{project},
                new IClasspathContainer[]{container},
                null
            );
            LOG.info("Recovered classpath from file cache for " + project.getProject().getName()
                + " (" + container.getClasspathEntries().length + " entries)");
            return true;
        } catch (Exception e) {
            LOG.warn("Failed to apply cached classpath for " + project.getProject().getName()
                + ": " + e.getMessage());
            return false;
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
