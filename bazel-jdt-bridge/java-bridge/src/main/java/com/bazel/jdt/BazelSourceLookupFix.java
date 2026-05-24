package com.bazel.jdt;

import java.util.ArrayList;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.logging.Level;
import java.util.logging.Logger;

import org.eclipse.core.resources.IWorkspaceRoot;
import org.eclipse.core.resources.ResourcesPlugin;
import org.eclipse.debug.core.sourcelookup.ISourceContainer;
import org.eclipse.jdt.core.IClassFile;
import org.eclipse.jdt.core.IJavaProject;
import org.eclipse.jdt.core.IType;
import org.eclipse.jdt.core.JavaCore;
import org.eclipse.jdt.core.JavaModelException;
import org.eclipse.jdt.core.IPackageFragmentRoot;

public class BazelSourceLookupFix {

    private static final Logger LOG = Logger.getLogger(BazelSourceLookupFix.class.getName());

    private static final String PFRSC_CLASS =
        "org.eclipse.jdt.launching.sourcelookup.containers.PackageFragmentRootSourceContainer";

    private static final ConcurrentHashMap<String, String> URI_CACHE = new ConcurrentHashMap<>();

    private static volatile java.lang.reflect.Method jdtUtilsToUri;

    private BazelSourceLookupFix() {}

    // --- Source container deduplication ---

    public static ISourceContainer[] deduplicateContainers(ISourceContainer[] containers) {
        if (containers == null || containers.length <= 1) {
            return containers;
        }

        try {
            return doDeduplicate(containers);
        } catch (Exception e) {
            LOG.log(Level.WARNING,
                "Source container deduplication failed, returning original array", e);
            return containers;
        }
    }

    private static ISourceContainer[] doDeduplicate(ISourceContainer[] containers) throws Exception {
        Set<String> seenKeys = new LinkedHashSet<>();
        List<ISourceContainer> result = new ArrayList<>();
        int removed = 0;

        for (ISourceContainer container : containers) {
            if (isPfrsc(container)) {
                String key = buildDedupKey(container);
                if (key != null) {
                    if (seenKeys.contains(key)) {
                        removed++;
                        continue;
                    }
                    seenKeys.add(key);
                }
            }
            result.add(container);
        }

        if (removed > 0) {
            LOG.info(String.format(
                "Deduplicated source containers: removed %d duplicate(s), %d remaining",
                removed, result.size()));
        }

        return result.toArray(new ISourceContainer[0]);
    }

    // --- Source file URI normalization ---

    public static String resolveSourceFileURI(String fqn, String sourcePath, String originalUri) {
        if (fqn == null || fqn.isEmpty()) {
            return originalUri;
        }

        String cached = URI_CACHE.get(fqn);
        if (cached != null) {
            return cached;
        }

        try {
            IWorkspaceRoot workspaceRoot = ResourcesPlugin.getWorkspace().getRoot();
            for (IJavaProject project : JavaCore.create(workspaceRoot).getJavaProjects()) {
                if (project == null || !project.exists()) {
                    continue;
                }
                IType type = project.findType(fqn);
                if (type != null && type.isBinary()) {
                    IClassFile classFile = type.getClassFile();
                    if (classFile != null) {
                        String normalizedUri = invokeJdtUtilsToUri(classFile);
                        if (normalizedUri != null) {
                            URI_CACHE.put(fqn, normalizedUri);
                            return normalizedUri;
                        }
                    }
                }
            }
        } catch (Exception e) {
            LOG.log(Level.FINE, "Source URI normalization failed for '" + fqn + "'", e);
        }

        return originalUri;
    }

    private static String invokeJdtUtilsToUri(IClassFile classFile) {
        try {
            java.lang.reflect.Method method = jdtUtilsToUri;
            if (method == null) {
                Class<?> clazz = Class.forName("org.eclipse.jdt.ls.core.internal.JDTUtils");
                method = clazz.getMethod("toUri", IClassFile.class);
                jdtUtilsToUri = method;
            }
            return (String) method.invoke(null, classFile);
        } catch (Exception e) {
            LOG.log(Level.FINE, "JDTUtils.toUri unavailable", e);
            return null;
        }
    }

    // --- Internal helpers ---

    private static String buildDedupKey(ISourceContainer container) {
        try {
            Object root = container.getClass().getMethod("getPackageFragmentRoot").invoke(container);
            if (root instanceof IPackageFragmentRoot) {
                IPackageFragmentRoot pfr = (IPackageFragmentRoot) root;
                String path = pfr.getPath().toString();
                if (isJdkContainer(pfr, path)) {
                    return "JDK|" + path;
                }
                String project = pfr.getJavaProject() != null
                    ? pfr.getJavaProject().getElementName() : "";
                return project + "|" + path;
            }
        } catch (Exception e) {
            LOG.log(Level.FINE,
                "Could not extract path from " + container.getClass().getName(), e);
        }
        return null;
    }

    static boolean isJdkContainer(IPackageFragmentRoot pfr, String path) {
        return path.contains("jrt-fs")
            || path.contains("/rt.jar")
            || path.contains("/jre/lib/")
            || path.contains("/Classes/");
    }

    private static boolean isPfrsc(Object obj) {
        return obj != null && PFRSC_CLASS.equals(obj.getClass().getName());
    }
}
