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
import org.eclipse.jdt.core.JavaCore;

public final class SourceRootUtils {

    private static final Logger LOG = Logger.getLogger(SourceRootUtils.class.getName());
    private static final Pattern PACKAGE_PATTERN = Pattern.compile(
        "^\\s*package\\s+([a-zA-Z_][a-zA-Z0-9_.]*?)\\s*;");

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
            return null;
        }

        String packageDecl = extractPackageDeclaration(javaFiles[0]);
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

    static String linkedFolderName(String sourceRoot) {
        return "_" + sourceRoot.replace('/', '_').replace('\\', '_');
    }

    public static void configureLinkedSourceFolder(IProject project, String workspacePath,
            String sourceRoot, String packagePath, List<IClasspathEntry> entries,
            IProgressMonitor monitor) throws CoreException {
        String folderName = linkedFolderName(sourceRoot);
        IFolder linkedFolder = project.getFolder(folderName);

        if (!linkedFolder.exists()) {
            IPath targetPath = new Path(new File(workspacePath, sourceRoot).getAbsolutePath());
            linkedFolder.createLink(targetPath, 0, monitor);
        }

        linkedFolder.refreshLocal(IResource.DEPTH_INFINITE, monitor);

        String declPath = packagePath;
        String prefix = sourceRoot + "/";
        if (declPath.startsWith(prefix)) {
            declPath = declPath.substring(prefix.length());
        }

        IPath sourcePath = new Path("/" + project.getName() + "/" + folderName);
        IPath[] inclusionPatterns = new IPath[] { new Path(declPath + "/") };
        entries.add(JavaCore.newSourceEntry(sourcePath, inclusionPatterns, new IPath[0], null));

        LOG.info("Configured linked source folder '" + folderName + "' → " + sourceRoot
            + " with inclusion pattern '" + declPath + "/' for project " + project.getName());
    }
}
