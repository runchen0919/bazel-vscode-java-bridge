package com.bazel.jdt;

import java.io.File;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

import org.eclipse.core.resources.IProject;
import org.eclipse.core.resources.IWorkspaceRoot;
import org.eclipse.core.resources.ResourcesPlugin;
import org.eclipse.core.runtime.CoreException;
import org.eclipse.core.runtime.ILog;
import org.eclipse.core.runtime.IPath;
import org.eclipse.core.runtime.IProgressMonitor;
import org.eclipse.core.runtime.IStatus;
import org.eclipse.core.runtime.Path;
import org.eclipse.core.runtime.Platform;
import org.eclipse.core.runtime.Status;
import org.eclipse.jdt.core.IClasspathEntry;
import org.eclipse.jdt.core.IJavaProject;
import org.eclipse.jdt.core.JavaCore;
import org.eclipse.jdt.ls.core.internal.AbstractProjectImporter;

public class BazelProjectImporter extends AbstractProjectImporter {
    private static final ILog LOG = Platform.getLog(BazelProjectImporter.class);

    private static final String JAVA_NATURE = "org.eclipse.jdt.core.javanature";
    private static final String[] STANDARD_SRC_ROOTS = {
        "src/main/java",
        "src/test/java",
    };

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
        String bazelPath = "bazel";
        String cacheDir = BazelCommandHandler.DEFAULT_CACHE_DIR;

        bridge.initialize(workspacePath, bazelPath, cacheDir);
        LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
            "Importing Bazel workspace: " + workspacePath));

        String[] scopePatterns = null;
        BazelProjectView projectView = BazelProjectView.parse(rootFolder);
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
                String packageName = extractPackageName(targetLabel);
                IProject project = workspaceRoot.getProject(packageName);

                boolean projectExisted = project.exists() && project.isOpen();
                if (!project.exists()) {
                    File packageDir = new File(workspacePath, packageName);
                    org.eclipse.core.resources.IProjectDescription projDesc =
                        project.getWorkspace().newProjectDescription(packageName);
                    projDesc.setLocation(new Path(packageDir.getAbsolutePath()));
                    LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                        "Creating project '" + packageName + "' at " + packageDir.getAbsolutePath()));
                    project.create(projDesc, monitor);
                }
                if (!project.isOpen()) {
                    project.open(monitor);
                }

                org.eclipse.core.resources.IProjectDescription desc =
                    project.getDescription();
                String[] natureIds = desc.getNatureIds();
                boolean hasJavaNature = false;
                boolean hasBazelNature = false;
                for (String nature : natureIds) {
                    if (JAVA_NATURE.equals(nature)) {
                        hasJavaNature = true;
                    }
                    if (BazelNature.NATURE_ID.equals(nature)) {
                        hasBazelNature = true;
                    }
                }

                if (!hasJavaNature || !hasBazelNature) {
                    int extraNatures = (hasJavaNature ? 0 : 1) + (hasBazelNature ? 0 : 1);
                    String[] newNatureIds = new String[natureIds.length + extraNatures];
                    System.arraycopy(natureIds, 0, newNatureIds, 0, natureIds.length);
                    int idx = natureIds.length;
                    if (!hasJavaNature) {
                        newNatureIds[idx++] = JAVA_NATURE;
                    }
                    if (!hasBazelNature) {
                        newNatureIds[idx] = BazelNature.NATURE_ID;
                    }
                    desc.setNatureIds(newNatureIds);
                    project.setDescription(desc, monitor);
                }

                TargetProjectMapping.appendTargets(project, Collections.singletonList(targetLabel));

                if (firstProject) {
                    TargetProjectMapping.storeWorkspaceConfig(project, workspacePath, bazelPath, cacheDir);
                    firstProject = false;
                }

                if (!projectExisted) {
                    configureClasspath(project, packageName, workspacePath, targetLabel, monitor);
                } else {
                    BazelClasspathManager.setClasspathContainer(project, targetLabel);
                }
            } catch (Exception e) {
                LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                    "Failed to import target: " + targetLabel, e));
            }
        }
    }

    private void configureClasspath(IProject project, String packageName,
            String workspacePath, String targetLabel, IProgressMonitor monitor) throws CoreException {
        IJavaProject javaProject = JavaCore.create(project);

        List<IClasspathEntry> entries = new ArrayList<>();

        for (String srcRoot : STANDARD_SRC_ROOTS) {
            java.io.File srcDir = new java.io.File(workspacePath, packageName + "/" + srcRoot);
            if (srcDir.isDirectory()) {
                IPath sourcePath = new Path("/" + project.getName() + "/" + srcRoot);
                entries.add(JavaCore.newSourceEntry(sourcePath));
            }
        }

        entries.add(JavaCore.newContainerEntry(BazelClasspathContainer.CONTAINER_PATH));

        addJreContainerEntry(entries);

        // Pre-resolve the container before setting raw classpath to prevent JDT from
        // triggering async container resolution via ClasspathContainerInitializer.
        // setClasspathContainer is a global JDT registry operation — it works even
        // though the raw classpath doesn't reference the container yet.
        BazelClasspathManager.setClasspathContainer(project, targetLabel);

        javaProject.setRawClasspath(entries.toArray(new IClasspathEntry[0]), monitor);
        javaProject.setOutputLocation(new Path("/" + project.getName() + "/bin"), monitor);
    }

    private void addJreContainerEntry(List<IClasspathEntry> entries) {
        try {
            Class<?> javaRuntimeClass = Class.forName("org.eclipse.jdt.launching.JavaRuntime");
            java.lang.reflect.Method method = javaRuntimeClass.getMethod("getDefaultJREContainerEntry");
            Object jreEntry = method.invoke(null);
            if (jreEntry instanceof IClasspathEntry) {
                entries.add((IClasspathEntry) jreEntry);
                return;
            }
        } catch (ReflectiveOperationException e) {
            LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                "Using fallback JRE container: " + e.getMessage()));
        }
        entries.add(JavaCore.newContainerEntry(
            Path.fromPortableString("org.eclipse.jdt.launching.JRE_CONTAINER")));
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
