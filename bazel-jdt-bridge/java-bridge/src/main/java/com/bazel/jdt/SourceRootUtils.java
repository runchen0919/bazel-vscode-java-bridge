package com.bazel.jdt;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileReader;
import java.io.IOException;
import java.util.List;
import java.util.logging.Logger;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

import org.eclipse.core.resources.IFolder;
import org.eclipse.core.resources.IProject;
import org.eclipse.core.resources.IResource;
import org.eclipse.core.runtime.CoreException;
import org.eclipse.core.runtime.IPath;
import org.eclipse.core.runtime.IProgressMonitor;
import org.eclipse.core.runtime.Path;
import org.eclipse.jdt.core.IClasspathEntry;


public final class SourceRootUtils {

    private static final Logger LOG = Logger.getLogger(SourceRootUtils.class.getName());
    private static final Pattern PACKAGE_PATTERN = Pattern.compile(
        "^\\s*package\\s+([a-zA-Z_][a-zA-Z0-9_.]*?)\\s*;");

    private static final int MAX_RECURSIVE_DEPTH = 5;

    private SourceRootUtils() {}

    public static String extractPackageDeclaration(File javaFile) {
        if (javaFile == null || !javaFile.isFile()) {
            return "";
        }
        try (BufferedReader reader = new BufferedReader(new FileReader(javaFile))) {
            String line;
            while ((line = reader.readLine()) != null) {
                line = line.trim();
                if (line.isEmpty() || line.startsWith("//") || line.startsWith("/*") || line.startsWith("*")) {
                    continue;
                }
                Matcher matcher = PACKAGE_PATTERN.matcher(line);
                if (matcher.find()) {
                    return matcher.group(1);
                }
                if (line.startsWith("import ") || line.startsWith("public ") ||
                    line.startsWith("class ") || line.startsWith("interface ") ||
                    line.startsWith("enum ") || line.startsWith("@")) {
                    break;
                }
            }
        } catch (IOException e) {
            LOG.warning("Failed to read package declaration from " + javaFile.getAbsolutePath()
                + ": " + e.getMessage());
        }
        return "";
    }

    public static String inferSourceRoot(String workspacePath, String packagePath) {
        File packageDir = new File(workspacePath, packagePath);
        if (!packageDir.isDirectory()) {
            return null;
        }

        File[] javaFiles = packageDir.listFiles((dir, name) -> name.endsWith(".java"));
        if (javaFiles == null || javaFiles.length == 0) {
            File found = findJavaFileRecursive(packageDir, MAX_RECURSIVE_DEPTH);
            if (found != null) {
                return inferSourceRootFromFile(workspacePath, packagePath, found);
            }
            return null;
        }

        java.util.Arrays.sort(javaFiles, java.util.Comparator.comparing(File::getName));
        String packageDecl = "";
        for (File jf : javaFiles) {
            packageDecl = extractPackageDeclaration(jf);
            if (!packageDecl.isEmpty()) break;
        }
        if (packageDecl.isEmpty()) {
            return null;
        }

        String packageDirPath = packagePath.replace('\\', '/');
        String declPath = packageDecl.replace('.', '/');

        if (packageDirPath.endsWith(declPath)) {
            String sourceRoot = packageDirPath.substring(0, packageDirPath.length() - declPath.length());
            if (sourceRoot.endsWith("/")) {
                sourceRoot = sourceRoot.substring(0, sourceRoot.length() - 1);
            }
            if (sourceRoot.isEmpty()) {
                return null;
            }
            File sourceRootDir = new File(workspacePath, sourceRoot);
            if (sourceRootDir.isDirectory()) {
                return sourceRoot;
            }
            LOG.warning("Inferred source root '" + sourceRoot + "' does not exist for package " + packagePath);
            return null;
        }

        LOG.warning("Package declaration '" + packageDecl + "' does not match directory structure for " + packagePath);
        return null;
    }

    private static File findJavaFileRecursive(File dir, int maxDepth) {
        if (maxDepth <= 0) {
            return null;
        }
        File[] children = dir.listFiles();
        if (children == null) {
            return null;
        }
        java.util.Arrays.sort(children, java.util.Comparator.comparing(File::getName));
        for (File child : children) {
            if (child.isFile() && child.getName().endsWith(".java")) {
                return child;
            }
        }
        for (File child : children) {
            if (child.isDirectory()) {
                File found = findJavaFileRecursive(child, maxDepth - 1);
                if (found != null) {
                    return found;
                }
            }
        }
        return null;
    }

    private static String inferSourceRootFromFile(String workspacePath, String packagePath, File javaFile) {
        String packageDecl = extractPackageDeclaration(javaFile);
        if (packageDecl.isEmpty()) {
            return null;
        }

        String declPath = packageDecl.replace('.', '/');

        String fileParent = javaFile.getParentFile().getAbsolutePath();
        String wsPrefix = workspacePath.endsWith(File.separator) ? workspacePath : workspacePath + File.separator;
        if (!fileParent.startsWith(wsPrefix)) {
            return null;
        }
        String relativeParent = fileParent.substring(wsPrefix.length()).replace('\\', '/');

        if (!relativeParent.endsWith(declPath)) {
            return null;
        }

        String sourceRoot = relativeParent.substring(0, relativeParent.length() - declPath.length());
        if (sourceRoot.endsWith("/")) {
            sourceRoot = sourceRoot.substring(0, sourceRoot.length() - 1);
        }
        if (sourceRoot.isEmpty()) {
            return null;
        }

        String packageDirPath = packagePath.replace('\\', '/');
        if (!packageDirPath.startsWith(sourceRoot + "/")) {
            return null;
        }

        File sourceRootDir = new File(workspacePath, sourceRoot);
        if (!sourceRootDir.isDirectory()) {
            LOG.warning("Inferred source root '" + sourceRoot + "' does not exist for package " + packagePath);
            return null;
        }

        return sourceRoot;
    }

    static String linkedFolderName(String sourceRoot) {
        return "_" + sourceRoot.replace('/', '_').replace('\\', '_');
    }

    public static void configureLinkedSourceFolder(IProject project, String workspacePath,
            String sourceRoot, String packagePath, List<IClasspathEntry> entries,
            IProgressMonitor monitor) throws CoreException {
        configureLinkedSourceFolder(project, workspacePath, sourceRoot, packagePath, entries,
            monitor, false);
    }

    public static void configureLinkedSourceFolder(IProject project, String workspacePath,
            String sourceRoot, String packagePath, List<IClasspathEntry> entries,
            IProgressMonitor monitor, boolean isTestProject) throws CoreException {
        String topFolderName = linkedFolderName(sourceRoot);
        String prefix = sourceRoot + "/";
        String declPath = packagePath.startsWith(prefix)
            ? packagePath.substring(prefix.length()) : "";

        if (declPath.isEmpty()) {
            IFolder linkedFolder = project.getFolder(topFolderName);
            if (!linkedFolder.exists()) {
                IPath targetPath = new Path(new File(workspacePath, sourceRoot).getAbsolutePath());
                linkedFolder.createLink(targetPath, 0, monitor);
            }
            linkedFolder.refreshLocal(IResource.DEPTH_INFINITE, monitor);
            IPath sourcePath = new Path("/" + project.getName() + "/" + topFolderName);
            entries.add(BazelProjectCreator.newSourceEntry(sourcePath, isTestProject));
            LOG.info("Configured linked source folder '" + topFolderName + "' → " + sourceRoot
                + " for project " + project.getName());
            return;
        }

        IFolder topFolder = project.getFolder(topFolderName);
        if (!topFolder.exists()) {
            topFolder.create(IResource.FORCE | IResource.DERIVED, true, monitor);
        }

        String[] segments = declPath.split("/");
        IFolder current = topFolder;
        for (int i = 0; i < segments.length - 1; i++) {
            current = current.getFolder(segments[i]);
            if (!current.exists()) {
                current.create(IResource.FORCE | IResource.DERIVED, true, monitor);
            }
        }

        IFolder leafFolder = current.getFolder(segments[segments.length - 1]);
        if (!leafFolder.exists()) {
            IPath targetPath = new Path(new File(workspacePath, packagePath).getAbsolutePath());
            leafFolder.createLink(targetPath, 0, monitor);
        }
        leafFolder.refreshLocal(IResource.DEPTH_INFINITE, monitor);

        IPath sourcePath = new Path("/" + project.getName() + "/" + topFolderName);
        entries.add(BazelProjectCreator.newSourceEntry(sourcePath, isTestProject));

        LOG.info("Configured linked source folder '" + topFolderName + "/" + declPath
            + "' → " + packagePath + " for project " + project.getName());
    }
}
