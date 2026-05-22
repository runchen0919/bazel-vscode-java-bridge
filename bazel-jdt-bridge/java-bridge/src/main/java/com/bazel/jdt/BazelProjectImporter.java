package com.bazel.jdt;

import java.io.File;
import java.util.Hashtable;

import org.eclipse.core.resources.IProject;
import org.eclipse.core.resources.IWorkspaceRoot;
import org.eclipse.core.resources.IWorkspaceRunnable;
import org.eclipse.core.resources.ResourcesPlugin;
import org.eclipse.core.runtime.CoreException;
import org.eclipse.core.runtime.ILog;
import org.eclipse.core.runtime.IProgressMonitor;
import org.eclipse.core.runtime.IStatus;
import org.eclipse.core.runtime.Platform;
import org.eclipse.core.runtime.Status;
import org.eclipse.jdt.core.JavaCore;
import org.eclipse.jdt.internal.compiler.impl.CompilerOptions;
import org.eclipse.jdt.launching.AbstractVMInstall;
import org.eclipse.jdt.launching.IVMInstall;
import org.eclipse.jdt.launching.JavaRuntime;
import org.eclipse.jdt.ls.core.internal.AbstractProjectImporter;
import org.eclipse.jdt.ls.core.internal.JobHelpers;

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

        if (tryFastReload(monitor)) {
            return;
        }

        String workspacePath = rootFolder.getAbsolutePath();
        String cacheDir = BazelCommandHandler.DEFAULT_CACHE_DIR;

        String[] scopePatterns = null;
        BazelProjectView projectView = BazelProjectView.parse(rootFolder);

        final String bazelPath;
        if (projectView != null && !projectView.getBazelBinary().isEmpty()) {
            bazelPath = projectView.getBazelBinary();
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Using custom bazel binary from .bazelproject: " + bazelPath));
        } else {
            bazelPath = "bazel";
        }

        bridge.initialize(workspacePath, bazelPath, cacheDir);
        LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
            "Importing Bazel workspace: " + workspacePath));

        ensureBazelProjectsGitignore(workspacePath);

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

        if (projectView != null) {
            bridge.setProjectView(projectView);
        }

        LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
            "Starting target discovery. This may take several minutes for large workspaces..."));
        long totalStart = System.currentTimeMillis();

        // Phase 1/3: bazel query
        String[] targets;
        try {
            long phaseStart = System.currentTimeMillis();
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Phase 1/3: running bazel query..."));
            targets = bridge.queryTargets(scopePatterns);
            long phaseElapsed = (System.currentTimeMillis() - phaseStart) / 1000;
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Phase 1/3: bazel query complete — "
                + (targets != null ? targets.length : 0) + " targets found (" + phaseElapsed + "s)"));
        } catch (Exception e) {
            throw new CoreException(
                new Status(IStatus.ERROR, "com.bazel.jdt",
                    "Failed during bazel query: " + e.getMessage(), e)
            );
        }

        if (targets == null || targets.length == 0) {
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "No targets found, skipping remaining phases"));
            return;
        }

        // Phase 2/3: BUILD file parsing + graph population
        try {
            long phaseStart = System.currentTimeMillis();
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Phase 2/3: parsing BUILD files..."));
            bridge.populateGraph();
            long phaseElapsed = (System.currentTimeMillis() - phaseStart) / 1000;
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Phase 2/3: BUILD parsing complete — graph populated (" + phaseElapsed + "s)"));
        } catch (Exception e) {
            throw new CoreException(
                new Status(IStatus.ERROR, "com.bazel.jdt",
                    "Failed during BUILD parsing: " + e.getMessage(), e)
            );
        }

        // Phase 3/3: aspect build
        final String[] finalTargets;
        try {
            long phaseStart = System.currentTimeMillis();
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Phase 3/3: running aspect build for " + targets.length + " targets..."));
            finalTargets = bridge.runAspectBuild(targets, bridge.getBuildFlags());
            long phaseElapsed = (System.currentTimeMillis() - phaseStart) / 1000;

            String aspectStats = bridge.getAspectBuildStats();
            String statsDetail = "";
            if (aspectStats != null) {
                String[] parts = aspectStats.split("\\|");
                if (parts.length == 2) {
                    statsDetail = " (" + parts[0] + " output files, " + parts[1] + " with JARs)";
                }
            }
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Phase 3/3: aspect build complete — "
                + (finalTargets != null ? finalTargets.length : 0) + " targets" + statsDetail
                + " (" + phaseElapsed + "s)"));
        } catch (Exception e) {
            throw new CoreException(
                new Status(IStatus.ERROR, "com.bazel.jdt",
                    "Failed during aspect build: " + e.getMessage(), e)
            );
        }

        long totalElapsed = (System.currentTimeMillis() - totalStart) / 1000;
        LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
            "Target discovery complete in " + totalElapsed + "s"));

        if (finalTargets == null || finalTargets.length == 0) return;

        // Phase 1: Create all projects (deferred classpath resolution)
        BazelClasspathContainerInitializer.setImportInProgress(true);
        try {
            ResourcesPlugin.getWorkspace().run(new IWorkspaceRunnable() {
                @Override
                public void run(IProgressMonitor pm) throws CoreException {
                    IWorkspaceRoot workspaceRoot = ResourcesPlugin.getWorkspace().getRoot();
                    boolean firstProject = true;

                    for (String targetLabel : finalTargets) {
                        try {
                            String packagePath = extractPackageName(targetLabel);
                            IProject project = BazelProjectCreator.createProjectForPackage(
                                workspacePath, packagePath, targetLabel, pm, true);

                            if (firstProject && project != null) {
                                if (TargetProjectMapping.readWorkspaceConfig(project) == null) {
                                    TargetProjectMapping.storeWorkspaceConfig(project, workspacePath, bazelPath, cacheDir);
                                }
                                TargetProjectMapping.storeWorkspaceConfigFile(workspacePath, bazelPath, cacheDir);
                                firstProject = false;
                            }
                        } catch (Exception e) {
                            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                                "Failed to import target: " + targetLabel, e));
                        }
                    }

                    String loadingMode = bridge.getDependencySourceLoadingMode();
                    String[] depEntries = bridge.getTransitiveWorkspaceDeps(finalTargets);
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
                                    workspacePath, packagePath, firstLabel, pm, true);
                                LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                                    "Auto-created project for dependency package: " + packagePath));
                            } catch (Exception e) {
                                LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                                    "Failed to auto-create project for dependency: " + entry, e));
                            }
                        }
                    }
                }
            }, monitor);

            preSetJavaCoreOptions();
            BazelClasspathManager.batchSetClasspathContainers(false);
        } finally {
            BazelClasspathContainerInitializer.setImportInProgress(false);
        }

        // Wait for JDT indexer — containers are already set, so indexer runs once with real data
        LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
            "All projects created and classpath containers set. Waiting for JDT indexes to be ready..."));
        JobHelpers.waitUntilIndexesReady();
        LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
            "JDT indexes ready. Import complete."));

    }

    private boolean tryFastReload(IProgressMonitor monitor) throws CoreException {
        String[] config = TargetProjectMapping.readWorkspaceConfigFile();
        if (config == null) {
            return false;
        }
        java.util.List<String[]> index = TargetProjectMapping.readProjectIndex();
        if (index.isEmpty()) {
            return false;
        }

        String workspacePath = config[0];
        String bazelPath = config[1];
        String cacheDir = config[2];

        BazelProjectView projectView = rootFolder != null
            ? BazelProjectView.parse(rootFolder) : null;
        if (projectView != null && !projectView.getBazelBinary().isEmpty()) {
            bazelPath = projectView.getBazelBinary();
        }

        long startTime = System.currentTimeMillis();
        LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
            "Fast reload: found " + index.size() + " cached projects, skipping Bazel discovery"));

        BazelBridge bridge = BazelBridge.getInstance();
        bridge.initialize(workspacePath, bazelPath, cacheDir);

        if (projectView != null) {
            bridge.setProjectView(projectView);
        }

        ensureBazelProjectsGitignore(workspacePath);

        if (projectView != null && !projectView.getDirectories().isEmpty()) {
            String[] watchDirs = projectView.getDirectories().toArray(new String[0]);
            bridge.updateWatchPaths(watchDirs);
        }

        final String wsPath = workspacePath;
        final String bzPath = bazelPath;
        final String cDir = cacheDir;
        BazelClasspathContainerInitializer.setImportInProgress(true);
        try {
            ResourcesPlugin.getWorkspace().run(new IWorkspaceRunnable() {
                @Override
                public void run(IProgressMonitor pm) throws CoreException {
                    boolean firstProject = true;
                    for (String[] entry : index) {
                        String projectName = entry[0];
                        String targetLabel = entry[1];
                        String packagePath = entry[2];
                        try {
                            IProject project = BazelProjectCreator.createProjectForPackage(
                                wsPath, packagePath, targetLabel, pm, true);
                            if (firstProject && project != null) {
                                if (TargetProjectMapping.readWorkspaceConfig(project) == null) {
                                    TargetProjectMapping.storeWorkspaceConfig(project, wsPath, bzPath, cDir);
                                }
                                firstProject = false;
                            }
                        } catch (Exception e) {
                            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                                "Fast reload: failed to recreate project " + projectName, e));
                        }
                    }
                }
            }, monitor);

            preSetJavaCoreOptions();
            BazelClasspathManager.batchSetClasspathContainers(true);
        } finally {
            BazelClasspathContainerInitializer.setImportInProgress(false);
        }

        JobHelpers.waitUntilIndexesReady();

        long elapsed = System.currentTimeMillis() - startTime;
        LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
            "Fast reload complete: " + index.size() + " projects restored in " + elapsed + "ms"));

        return true;
    }

    private static void ensureBazelProjectsGitignore(String workspacePath) {
        File gitignore = new File(workspacePath, ".gitignore");
        String entry = ".bazel-projects/";
        try {
            if (gitignore.exists()) {
                String content = new String(java.nio.file.Files.readAllBytes(gitignore.toPath()),
                    java.nio.charset.StandardCharsets.UTF_8);
                for (String line : content.split("\n")) {
                    if (line.trim().equals(entry)) {
                        return;
                    }
                }
                String separator = content.endsWith("\n") ? "" : "\n";
                java.nio.file.Files.write(gitignore.toPath(),
                    (separator + entry + "\n").getBytes(java.nio.charset.StandardCharsets.UTF_8),
                    java.nio.file.StandardOpenOption.APPEND);
            } else {
                java.nio.file.Files.write(gitignore.toPath(),
                    (entry + "\n").getBytes(java.nio.charset.StandardCharsets.UTF_8));
            }
            LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                "Added .bazel-projects/ to .gitignore"));
        } catch (Exception e) {
            LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                "Failed to update .gitignore: " + e.getMessage()));
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

    private void preSetJavaCoreOptions() {
        try {
            Hashtable<String, String> defaultOptions = JavaCore.getDefaultOptions();
            IVMInstall defaultVM = JavaRuntime.getDefaultVMInstall();
            if (defaultVM instanceof AbstractVMInstall jvm) {
                long jdkLevel = CompilerOptions.versionToJdkLevel(jvm.getJavaVersion());
                String compliance = CompilerOptions.versionFromJdkLevel(jdkLevel);
                JavaCore.setComplianceOptions(compliance, defaultOptions);
            } else {
                JavaCore.setComplianceOptions(JavaCore.VERSION_11, defaultOptions);
            }
            JavaCore.setOptions(defaultOptions);
        } catch (Exception e) {
            LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                "Failed to pre-set JavaCore options: " + e.getMessage()));
        }
    }

    private String extractPackageName(String targetLabel) {
        return LabelUtils.extractPackageName(targetLabel);
    }
}
