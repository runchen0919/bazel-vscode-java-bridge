package com.bazel.jdt;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.ConcurrentHashMap;

import org.eclipse.core.runtime.CoreException;
import org.eclipse.core.runtime.ILog;
import org.eclipse.core.runtime.IStatus;
import org.eclipse.core.runtime.Platform;
import org.eclipse.core.runtime.Status;
import org.eclipse.debug.core.ILaunchConfiguration;
import org.eclipse.jdt.core.IClasspathContainer;
import org.eclipse.jdt.core.IClasspathEntry;
import org.eclipse.jdt.core.IJavaProject;
import org.eclipse.jdt.core.JavaCore;
import org.eclipse.jdt.launching.IRuntimeClasspathEntry;
import org.eclipse.jdt.launching.IRuntimeClasspathEntryResolver;
import org.eclipse.jdt.launching.IVMInstall;
import org.eclipse.jdt.launching.JavaRuntime;

public class BazelRuntimeClasspathEntryResolver implements IRuntimeClasspathEntryResolver {
    private static final ILog LOG = Platform.getLog(BazelRuntimeClasspathEntryResolver.class);
    private static final ConcurrentHashMap<String, IRuntimeClasspathEntry[]> CACHE = new ConcurrentHashMap<>();
    private static final IRuntimeClasspathEntry[] EMPTY = new IRuntimeClasspathEntry[0];
    private static volatile String activeDebugProject;

    @Override
    public IRuntimeClasspathEntry[] resolveRuntimeClasspathEntry(
            IRuntimeClasspathEntry entry, ILaunchConfiguration configuration) throws CoreException {
        IJavaProject project = entry.getJavaProject();
        if (project == null) {
            return EMPTY;
        }
        return resolve(project);
    }

    @Override
    public IRuntimeClasspathEntry[] resolveRuntimeClasspathEntry(
            IRuntimeClasspathEntry entry, IJavaProject project) throws CoreException {
        if (project == null) {
            return EMPTY;
        }
        return resolve(project);
    }

    @Override
    public IVMInstall resolveVMInstall(IClasspathEntry entry) throws CoreException {
        return null;
    }

    private IRuntimeClasspathEntry[] resolve(IJavaProject project) {
        String projectName = project.getElementName();

        String active = activeDebugProject;
        if (active != null && !active.equals(projectName)) {
            return EMPTY;
        }

        IRuntimeClasspathEntry[] cached = CACHE.get(projectName);
        if (cached != null) {
            return cached;
        }

        IRuntimeClasspathEntry[] resolved = buildEntries(project);
        CACHE.put(projectName, resolved);
        return resolved;
    }

    private IRuntimeClasspathEntry[] buildEntries(IJavaProject project) {
        try {
            IClasspathContainer container = JavaCore.getClasspathContainer(
                BazelClasspathContainer.CONTAINER_PATH, project);
            if (container == null) {
                return EMPTY;
            }

            List<IRuntimeClasspathEntry> result = new ArrayList<>();
            for (IClasspathEntry cpEntry : container.getClasspathEntries()) {
                if (cpEntry.getEntryKind() != IClasspathEntry.CPE_LIBRARY) {
                    continue;
                }
                IRuntimeClasspathEntry rte = JavaRuntime.newArchiveRuntimeClasspathEntry(cpEntry.getPath());
                if (cpEntry.getSourceAttachmentPath() != null) {
                    rte.setSourceAttachmentPath(cpEntry.getSourceAttachmentPath());
                }
                if (cpEntry.getSourceAttachmentRootPath() != null) {
                    rte.setSourceAttachmentRootPath(cpEntry.getSourceAttachmentRootPath());
                }
                result.add(rte);
            }

            return result.toArray(EMPTY);
        } catch (Exception e) {
            LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                "Failed to resolve runtime classpath for " + project.getElementName(), e));
            return EMPTY;
        }
    }

    public static void clearCache() {
        CACHE.clear();
    }

    public static void clearCacheForProject(String projectName) {
        CACHE.remove(projectName);
    }

    public static void setActiveDebugProject(String projectName) {
        activeDebugProject = projectName;
    }

    public static void clearActiveDebugProject() {
        activeDebugProject = null;
    }
}
