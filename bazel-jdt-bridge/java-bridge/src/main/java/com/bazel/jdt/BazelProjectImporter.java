package com.bazel.jdt;

import java.io.File;
import java.util.ArrayList;
import java.util.List;

import org.eclipse.core.resources.IProject;
import org.eclipse.core.resources.IWorkspaceRoot;
import org.eclipse.core.resources.ResourcesPlugin;
import org.eclipse.core.runtime.CoreException;
import org.eclipse.core.runtime.ILog;
import org.eclipse.core.runtime.IProgressMonitor;
import org.eclipse.core.runtime.IStatus;
import org.eclipse.core.runtime.Platform;
import org.eclipse.core.runtime.Status;
import org.eclipse.jdt.ls.core.internal.AbstractProjectImporter;

public class BazelProjectImporter extends AbstractProjectImporter {
    private static final ILog LOG = Platform.getLog(BazelProjectImporter.class);

    @Override
    public boolean applies(IProgressMonitor monitor) {
        if (rootFolder == null) return false;
        return new File(rootFolder, "WORKSPACE").exists()
                || new File(rootFolder, "WORKSPACE.bazel").exists();
    }

    @Override
    public void importToWorkspace(IProgressMonitor monitor) throws CoreException {
        String workspacePath = rootFolder.getAbsolutePath();
        String bazelPath = "bazel";
        String cacheDir = BazelCommandHandler.DEFAULT_CACHE_DIR;

        BazelBridge bridge = BazelBridge.getInstance();
        bridge.initialize(workspacePath, bazelPath, cacheDir);
        LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
            "Importing Bazel workspace: " + workspacePath));

        String[] targets;
        try {
            targets = bridge.discoverTargets();
        } catch (Exception e) {
            throw new CoreException(
                new Status(IStatus.ERROR, "com.bazel.jdt",
                    "Failed to discover Bazel targets: " + e.getMessage(), e)
            );
        }

        if (targets == null || targets.length == 0) return;

        IWorkspaceRoot workspaceRoot = ResourcesPlugin.getWorkspace().getRoot();

        for (String targetLabel : targets) {
            try {
                String packageName = extractPackageName(targetLabel);
                IProject project = workspaceRoot.getProject(packageName);
                if (!project.exists()) {
                    project.create(monitor);
                }
                if (!project.isOpen()) {
                    project.open(monitor);
                }
                org.eclipse.core.resources.IProjectDescription desc =
                    project.getDescription();
                String[] natureIds = desc.getNatureIds();
                String[] newNatureIds = new String[natureIds.length + 1];
                System.arraycopy(natureIds, 0, newNatureIds, 0, natureIds.length);
                newNatureIds[natureIds.length] = BazelNature.NATURE_ID;
                desc.setNatureIds(newNatureIds);
                project.setDescription(desc, monitor);
                BazelClasspathManager.setClasspathContainer(project, targetLabel);
            } catch (Exception e) {
                LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                    "Failed to import target: " + targetLabel, e));
            }
        }
    }

    @Override
    public void reset() {
        // No-op: BazelBridge.initialize() in importToWorkspace() handles native handle
        // lifecycle. Calling shutdown() here would permanently kill the executor, making
        // subsequent discoverTargets() calls fail with RejectedExecutionException.
    }

    private String extractPackageName(String targetLabel) {
        int colonIndex = targetLabel.lastIndexOf(':');
        if (colonIndex > 2) {
            return targetLabel.substring(2, colonIndex);
        }
        return targetLabel.substring(2);
    }
}
