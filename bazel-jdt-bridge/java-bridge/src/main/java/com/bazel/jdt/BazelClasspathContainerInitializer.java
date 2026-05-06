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
            String wildcardLabel = "//" + project.getProject().getName() + ":*";
            LOG.warn("No persisted target labels for project '" + project.getProject().getName()
                + "' - using wildcard fallback '" + wildcardLabel + "'. Re-import to fix.");
            BazelClasspathManager.setClasspathContainer(project.getProject(), wildcardLabel);
        } else {
            for (String label : targetLabels) {
                BazelClasspathManager.setClasspathContainer(project.getProject(), label);
            }
        }
    }

    private void recoverFromCache(IJavaProject project, BazelBridge bridge) {
        List<String> targetLabels = TargetProjectMapping.readTargets(project.getProject());
        if (targetLabels.isEmpty()) {
            LOG.info("No persisted targets for " + project.getProject().getName()
                + " — skipping container initialization until import runs");
            return;
        }
        boolean anyRecovered = false;
        for (String label : targetLabels) {
            String[] cached = TargetProjectMapping.readCachedClasspath(project.getProject(), label);
            if (cached != null && cached.length > 0) {
                try {
                    BazelClasspathContainer container = new BazelClasspathContainer(
                        cached, Collections.emptyList(),
                        bridge.getDependencyResolutionMode());
                    JavaCore.setClasspathContainer(
                        BazelClasspathContainer.CONTAINER_PATH,
                        new IJavaProject[]{project},
                        new IClasspathContainer[]{container},
                        null
                    );
                    anyRecovered = true;
                    LOG.info("Recovered classpath from cache for " + project.getProject().getName()
                        + " / " + label + " (" + cached.length + " entries)");
                } catch (Exception e) {
                    LOG.warn("Failed to apply cached classpath for " + label + ": " + e.getMessage());
                }
            } else {
                LOG.info("No cached classpath for " + project.getProject().getName()
                    + " / " + label + " — will resolve after import");
            }
        }
        if (!anyRecovered) {
            LOG.info("No cached classpath for " + project.getProject().getName()
                + " — skipping until import provides entries");
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
