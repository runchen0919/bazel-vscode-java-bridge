package com.bazel.jdt;

import java.io.File;
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
        boolean hasWorkspace = new File(rootFolder, "WORKSPACE").exists()
                || new File(rootFolder, "WORKSPACE.bazel").exists();
        if (!hasWorkspace) return false;
        return new File(rootFolder, ".bazelproject").exists();
    }

    @Override
    public void importToWorkspace(IProgressMonitor monitor) throws CoreException {
        BazelBridge bridge = BazelBridge.getInstance();
        if (bridge.isInitialized()) {
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Bridge already initialized, skipping re-import"));
            return;
        }

        String workspacePath = rootFolder.getAbsolutePath();
        String cacheDir = BazelCommandHandler.DEFAULT_CACHE_DIR;

        String[] scopePatterns = null;
        BazelProjectView projectView = BazelProjectView.parse(rootFolder);

        String bazelPath = "bazel";
        if (projectView != null && !projectView.getBazelBinary().isEmpty()) {
            bazelPath = projectView.getBazelBinary();
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Using custom bazel binary from .bazelproject: " + bazelPath));
        }

        bridge.initialize(workspacePath, bazelPath, cacheDir);
        LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
            "Importing Bazel workspace: " + workspacePath));

        if (projectView != null && !projectView.getDirectories().isEmpty()) {
            String[] watchDirs = projectView.getDirectories().toArray(new String[0]);
            bridge.updateWatchPaths(watchDirs);
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "File watcher scoped to " + watchDirs.length + " directories from .bazelproject"));
        }

        if (projectView != null && projectView.hasScope()) {
            java.util.List<String> patterns = projectView.getScopePatterns();
            scopePatterns = patterns.toArray(new String[0]);
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Scoped import with " + patterns.size() + " patterns from .bazelproject"));
        }

        String[] buildFlags = null;
        if (projectView != null && !projectView.getBuildFlags().isEmpty()) {
            buildFlags = projectView.getBuildFlags().toArray(new String[0]);
        }

        String[] targets;
        try {
            targets = bridge.discoverTargets(scopePatterns, buildFlags);
        } catch (Exception e) {
            throw new CoreException(
                new Status(IStatus.ERROR, "com.bazel.jdt",
                    "Failed to discover Bazel targets: " + e.getMessage(), e)
            );
        }

        if (targets == null || targets.length == 0) return;

        IWorkspaceRoot workspaceRoot = ResourcesPlugin.getWorkspace().getRoot();
        boolean firstProject = true;

        for (String targetLabel : targets) {
            try {
                String packagePath = extractPackageName(targetLabel);
                IProject project = BazelProjectCreator.createProjectForPackage(
                    workspacePath, packagePath, targetLabel, monitor);

                if (firstProject && project != null) {
                    TargetProjectMapping.storeWorkspaceConfig(project, workspacePath, bazelPath, cacheDir);
                    firstProject = false;
                }
            } catch (Exception e) {
                LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                    "Failed to import target: " + targetLabel, e));
            }
        }

        String loadingMode = bridge.getDependencySourceLoadingMode();
        String[] depEntries = bridge.getTransitiveWorkspaceDeps(targets);
        bridge.setCachedDependencyPackages(depEntries);

        if ("full-project".equals(loadingMode) && depEntries != null && depEntries.length > 0) {
            for (String entry : depEntries) {
                try {
                    String[] parts = entry.split("\\|", 2);
                    String packagePath = parts[0];
                    String firstLabel = parts.length > 1 && !parts[1].isEmpty()
                        ? parts[1].split(",")[0]
                        : null;

                    String projName = LabelUtils.toProjectName(packagePath);
                    if (workspaceRoot.getProject(projName).exists()) {
                        continue;
                    }
                    if (firstLabel == null) {
                        LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                            "No target label for dependency package: " + packagePath));
                        continue;
                    }
                    BazelProjectCreator.createProjectForPackage(
                        workspacePath, packagePath, firstLabel, monitor);
                    LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                        "Auto-created project for dependency package: " + packagePath));
                } catch (Exception e) {
                    LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                        "Failed to auto-create project for dependency: " + entry, e));
                }
            }
        }

    }

    @Override
    public void reset() {
        // No-op: BazelBridge.initialize() in importToWorkspace() handles native handle
        // lifecycle. Calling shutdown() here would permanently kill the executor, making
        // subsequent discoverTargets() calls fail with RejectedExecutionException.
    }

    @Override
    public boolean isResolved(java.io.File rootFolder) {
        return true;
    }

    private String extractPackageName(String targetLabel) {
        return LabelUtils.extractPackageName(targetLabel);
    }
}
