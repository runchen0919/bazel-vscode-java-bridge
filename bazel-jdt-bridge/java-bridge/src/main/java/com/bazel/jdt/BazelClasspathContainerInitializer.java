package com.bazel.jdt;

import java.util.List;

import org.eclipse.core.runtime.CoreException;
import org.eclipse.core.runtime.ILog;
import org.eclipse.core.runtime.IPath;
import org.eclipse.core.runtime.Platform;
import org.eclipse.jdt.core.ClasspathContainerInitializer;
import org.eclipse.jdt.core.IClasspathContainer;
import org.eclipse.jdt.core.IJavaProject;
import org.eclipse.jdt.core.JavaCore;

public class BazelClasspathContainerInitializer extends ClasspathContainerInitializer {

    private static final ILog LOG = Platform.getLog(BazelClasspathContainerInitializer.class);

    @Override
    public void initialize(IPath containerPath, IJavaProject project) throws CoreException {
        if (!BazelClasspathContainer.CONTAINER_PATH.equals(containerPath)) {
            return;
        }
        if (!BazelBridge.getInstance().isInitialized()) {
            LOG.log(new org.eclipse.core.runtime.Status(
                org.eclipse.core.runtime.IStatus.INFO, "com.bazel.jdt",
                "Bridge not initialized, setting empty container for "
                    + project.getProject().getName()
                    + " — will refresh after initialization"));
            try {
                JavaCore.setClasspathContainer(
                    BazelClasspathContainer.CONTAINER_PATH,
                    new IJavaProject[]{project},
                    new IClasspathContainer[]{BazelClasspathContainer.EMPTY},
                    null
                );
            } catch (Exception e) {
                LOG.log(new org.eclipse.core.runtime.Status(
                    org.eclipse.core.runtime.IStatus.WARNING, "com.bazel.jdt",
                    "Failed to set empty container for " + project.getProject().getName(), e));
            }
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

    @Override
    public boolean canUpdateClasspathContainer(IPath containerPath, IJavaProject project) {
        return true;
    }

    @Override
    public String getDescription(IPath containerPath, IJavaProject project) {
        return "Bazel Dependencies";
    }
}
