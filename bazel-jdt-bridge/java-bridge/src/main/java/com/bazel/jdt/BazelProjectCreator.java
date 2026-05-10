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

public final class BazelProjectCreator {
    private static final ILog LOG = Platform.getLog(BazelProjectCreator.class);
    private static final String JAVA_NATURE = "org.eclipse.jdt.core.javanature";
    private static final String[] STANDARD_SRC_ROOTS = {
        "src/main/java",
        "src/test/java",
    };

    private BazelProjectCreator() {}

    public static IProject createProjectForPackage(
            String workspacePath, String packagePath, String targetLabel,
            IProgressMonitor monitor) {
        return createProjectForPackage(workspacePath, packagePath, targetLabel, monitor, false);
    }

    public static IProject createProjectForPackage(
            String workspacePath, String packagePath, String targetLabel,
            IProgressMonitor monitor, boolean deferContainerResolution) {
        try {
            String projectName = LabelUtils.toProjectName(packagePath);
            IWorkspaceRoot workspaceRoot = ResourcesPlugin.getWorkspace().getRoot();
            IProject project = workspaceRoot.getProject(projectName);

            String inferredSourceRoot = SourceRootUtils.inferSourceRoot(workspacePath, packagePath);

            if (project.exists() && inferredSourceRoot != null
                    && project.getDescription().getLocation() != null) {
                LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                    "Recreating project '" + projectName
                    + "' — stale custom location conflicts with linked source folder"));
                project.delete(false, true, monitor);
            }

            if (!project.exists()) {
                org.eclipse.core.resources.IProjectDescription projDesc =
                    project.getWorkspace().newProjectDescription(projectName);
                if (inferredSourceRoot == null) {
                    File packageDir = new File(workspacePath, packagePath);
                    projDesc.setLocation(new Path(packageDir.getAbsolutePath()));
                    LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                        "Creating project '" + projectName + "' at " + packageDir.getAbsolutePath()));
                } else {
                    LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                        "Creating project '" + projectName + "' with default location (source root: " + inferredSourceRoot + ")"));
                }
                project.create(projDesc, monitor);
            }
            if (!project.isOpen()) {
                project.open(monitor);
            }

            TargetProjectMapping.appendTargets(project, Collections.singletonList(targetLabel));

            ensureNatures(project, monitor);
            configureClasspath(project, packagePath, workspacePath, targetLabel, inferredSourceRoot, monitor, deferContainerResolution);

            return project;
        } catch (Exception e) {
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                "Failed to create project for package: " + packagePath, e));
            return null;
        }
    }

    private static void ensureNatures(IProject project, IProgressMonitor monitor) throws CoreException {
        org.eclipse.core.resources.IProjectDescription desc = project.getDescription();
        String[] natureIds = desc.getNatureIds();
        boolean hasJavaNature = false;
        boolean hasBazelNature = false;
        for (String nature : natureIds) {
            if (JAVA_NATURE.equals(nature)) hasJavaNature = true;
            if (BazelNature.NATURE_ID.equals(nature)) hasBazelNature = true;
        }

        if (!hasJavaNature || !hasBazelNature) {
            int extra = (hasJavaNature ? 0 : 1) + (hasBazelNature ? 0 : 1);
            String[] newNatureIds = new String[natureIds.length + extra];
            System.arraycopy(natureIds, 0, newNatureIds, 0, natureIds.length);
            int idx = natureIds.length;
            if (!hasJavaNature) newNatureIds[idx++] = JAVA_NATURE;
            if (!hasBazelNature) newNatureIds[idx] = BazelNature.NATURE_ID;
            desc.setNatureIds(newNatureIds);
            project.setDescription(desc, monitor);
        }
    }

    private static void configureClasspath(IProject project, String packageName,
            String workspacePath, String targetLabel, String inferredSourceRoot,
            IProgressMonitor monitor, boolean deferContainerResolution) throws CoreException {
        IJavaProject javaProject = JavaCore.create(project);

        List<IClasspathEntry> sourceEntries = new ArrayList<>();

        for (String srcRoot : STANDARD_SRC_ROOTS) {
            java.io.File srcDir = new java.io.File(workspacePath, packageName + "/" + srcRoot);
            if (srcDir.isDirectory()) {
                IPath sourcePath = new Path("/" + project.getName() + "/" + srcRoot);
                sourceEntries.add(JavaCore.newSourceEntry(sourcePath));
            }
        }

        List<IClasspathEntry> entries = new ArrayList<>();
        if (sourceEntries.isEmpty()) {
            if (inferredSourceRoot != null) {
                try {
                    SourceRootUtils.configureLinkedSourceFolder(
                        project, workspacePath, inferredSourceRoot, packageName, entries, monitor);
                } catch (Exception e) {
                    LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                        "Failed to create linked source folder for " + packageName
                        + ", falling back to project root: " + e.getMessage()));
                    entries.add(JavaCore.newSourceEntry(new Path("/" + project.getName())));
                }
            } else {
                entries.add(JavaCore.newSourceEntry(new Path("/" + project.getName())));
            }
        } else {
            entries.addAll(sourceEntries);
        }

        entries.add(JavaCore.newContainerEntry(BazelClasspathContainer.CONTAINER_PATH));

        addJreContainerEntry(entries);

        if (!deferContainerResolution) {
            BazelClasspathManager.setClasspathContainer(project, targetLabel);
        }

        javaProject.setRawClasspath(entries.toArray(new IClasspathEntry[0]), monitor);
        javaProject.setOutputLocation(new Path("/" + project.getName() + "/bin"), monitor);
    }

    private static void addJreContainerEntry(List<IClasspathEntry> entries) {
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
}
