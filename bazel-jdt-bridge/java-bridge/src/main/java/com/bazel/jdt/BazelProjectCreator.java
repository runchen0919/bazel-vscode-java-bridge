package com.bazel.jdt;

import java.io.File;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

import org.eclipse.core.resources.FileInfoMatcherDescription;
import org.eclipse.core.resources.ICommand;
import org.eclipse.core.resources.IProject;
import org.eclipse.core.resources.IResource;
import org.eclipse.core.resources.IResourceFilterDescription;
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

            File bazelProjectDir = new File(workspacePath, ".bazel-projects/" + projectName);
            IPath expectedLocation = new Path(bazelProjectDir.getAbsolutePath());

            if (project.exists()) {
                IPath currentLocation = project.getDescription().getLocation();
                if (currentLocation != null && currentLocation.equals(expectedLocation)) {
                    List<String> existingLabels = TargetProjectMapping.readTargets(project);
                    if (!existingLabels.contains(targetLabel)) {
                        TargetProjectMapping.appendTargets(project, Collections.singletonList(targetLabel));
                    }
                    LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                        "Project '" + projectName + "' already at .bazel-projects/, skipping rebuild"));
                    return project;
                }
                LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                    "Migrating project '" + projectName + "' to .bazel-projects/ location"));
                project.delete(false, true, monitor);
            }

            if (!bazelProjectDir.exists() && !bazelProjectDir.mkdirs()) {
                LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                    "Failed to create .bazel-projects directory: " + bazelProjectDir.getAbsolutePath()));
                return null;
            }

            if (!project.exists()) {
                org.eclipse.core.resources.IProjectDescription projDesc =
                    project.getWorkspace().newProjectDescription(projectName);
                projDesc.setLocation(expectedLocation);
                LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                    "Creating project '" + projectName + "' at " + bazelProjectDir.getAbsolutePath()));
                project.create(projDesc, monitor);
            }
            if (!project.isOpen()) {
                project.open(monitor);
            }

            TargetProjectMapping.appendTargets(project, Collections.singletonList(targetLabel));

            preCreateResourceFilter(project);
            ensureNatures(project, monitor);
            String inferredSourceRoot = SourceRootUtils.inferSourceRoot(workspacePath, packagePath);
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

        removeJavaBuilder(project, monitor);
    }

    private static void preCreateResourceFilter(IProject project) {
        try {
            for (IResourceFilterDescription f : project.getFilters()) {
                FileInfoMatcherDescription matcher = f.getFileInfoMatcherDescription();
                if ("org.eclipse.core.resources.regexFilterMatcher".equals(matcher.getId())
                        && matcher.getArguments() instanceof String args
                        && args.contains("__CREATED_BY_JAVA_LANGUAGE_SERVER__")) {
                    return;
                }
            }
            int filterType = IResourceFilterDescription.EXCLUDE_ALL
                    | IResourceFilterDescription.INHERITABLE
                    | IResourceFilterDescription.FILES
                    | IResourceFilterDescription.FOLDERS;
            project.createFilter(filterType,
                    new FileInfoMatcherDescription("org.eclipse.core.resources.regexFilterMatcher",
                            "__CREATED_BY_JAVA_LANGUAGE_SERVER__"),
                    IResource.NONE, null);
        } catch (CoreException e) {
            LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                "Failed to pre-create resource filter: " + e.getMessage()));
        }
    }

    private static void removeJavaBuilder(IProject project, IProgressMonitor monitor) throws CoreException {
        org.eclipse.core.resources.IProjectDescription desc = project.getDescription();
        ICommand[] buildSpec = desc.getBuildSpec();
        List<ICommand> filtered = new ArrayList<>();
        for (ICommand cmd : buildSpec) {
            if (!"org.eclipse.jdt.core.javabuilder".equals(cmd.getBuilderName())) {
                filtered.add(cmd);
            }
        }
        if (filtered.size() < buildSpec.length) {
            desc.setBuildSpec(filtered.toArray(new ICommand[0]));
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
                String linkedName = SourceRootUtils.linkedFolderName(srcRoot);
                org.eclipse.core.resources.IFolder linkedFolder = project.getFolder(linkedName);
                if (!linkedFolder.exists()) {
                    linkedFolder.createLink(new Path(srcDir.getAbsolutePath()), 0, monitor);
                }
                IPath sourcePath = new Path("/" + project.getName() + "/" + linkedName);
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
                        + ", falling back to linked package folder: " + e.getMessage()));
                    configureLinkedPackageFolder(project, workspacePath, packageName, entries, monitor);
                }
            } else {
                configureLinkedPackageFolder(project, workspacePath, packageName, entries, monitor);
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

    private static void configureLinkedPackageFolder(IProject project, String workspacePath,
            String packageName, List<IClasspathEntry> entries,
            IProgressMonitor monitor) throws CoreException {
        String linkedName = "_pkg";
        org.eclipse.core.resources.IFolder linkedFolder = project.getFolder(linkedName);
        if (!linkedFolder.exists()) {
            File packageDir = new File(workspacePath, packageName);
            linkedFolder.createLink(new Path(packageDir.getAbsolutePath()), 0, monitor);
        }
        entries.add(JavaCore.newSourceEntry(new Path("/" + project.getName() + "/" + linkedName)));
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
