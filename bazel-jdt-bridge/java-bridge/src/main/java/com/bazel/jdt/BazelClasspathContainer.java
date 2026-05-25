package com.bazel.jdt;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;

import org.eclipse.core.resources.ResourcesPlugin;
import org.eclipse.core.runtime.ILog;
import org.eclipse.core.runtime.IPath;
import org.eclipse.core.runtime.IStatus;
import org.eclipse.core.runtime.Path;
import org.eclipse.core.runtime.Platform;
import org.eclipse.core.runtime.Status;
import org.eclipse.jdt.core.IAccessRule;
import org.eclipse.jdt.core.IClasspathAttribute;
import org.eclipse.jdt.core.IClasspathContainer;
import org.eclipse.jdt.core.IClasspathEntry;
import org.eclipse.jdt.core.JavaCore;

public class BazelClasspathContainer implements IClasspathContainer {
    private static final ILog LOG = Platform.getLog(BazelClasspathContainer.class);
    private static final Set<String> WARNED_MISSING_PATHS = ConcurrentHashMap.newKeySet();
    private static final java.util.concurrent.ConcurrentHashMap<String, Boolean> FILE_EXISTS_CACHE = new java.util.concurrent.ConcurrentHashMap<>();
    public static final IPath CONTAINER_PATH = Path.fromPortableString("com.bazel.jdt.BAZEL_CONTAINER");
    private static final String DESCRIPTION = "Bazel Dependencies";

    public static final BazelClasspathContainer EMPTY = new BazelClasspathContainer((String[]) null);

    private final IClasspathEntry[] entries;
    private final List<String> testSourcePatterns;
    private final String resolutionMode;
    private final String ownerProjectName;

    public BazelClasspathContainer(String[] rawEntries) {
        this(rawEntries, Collections.emptyList(), "transitive", null);
    }

    public BazelClasspathContainer(String[] rawEntries, List<String> testSourcePatterns) {
        this(rawEntries, testSourcePatterns, "transitive", null);
    }

    public BazelClasspathContainer(String[] rawEntries, List<String> testSourcePatterns, String resolutionMode) {
        this(rawEntries, testSourcePatterns, resolutionMode, null);
    }

    public BazelClasspathContainer(String[] rawEntries, List<String> testSourcePatterns,
            String resolutionMode, String ownerProjectName) {
        this.ownerProjectName = ownerProjectName;
        this.resolutionMode = resolutionMode != null ? resolutionMode : "transitive";
        this.testSourcePatterns = testSourcePatterns != null ? testSourcePatterns : Collections.emptyList();
        List<IClasspathEntry> parsed = new ArrayList<>();
        if (rawEntries == null) {
            this.entries = parsed.toArray(new IClasspathEntry[0]);
            return;
        }
        for (String raw : rawEntries) {
            IClasspathEntry entry = parseEntry(raw);
            if (entry != null) {
                parsed.add(entry);
            }
        }
        this.entries = parsed.toArray(new IClasspathEntry[0]);
    }

    private IClasspathEntry parseEntry(String raw) {
        String[] parts = raw.split("\\|", -1);
        if (parts.length < 2) return null;
        String type = parts[0];
        String path = parts[1];
        String sourcePath = parts.length > 2 && !parts[2].isEmpty() ? parts[2] : null;
        boolean isTest = parts.length > 3 && Boolean.parseBoolean(parts[3]);
        boolean isExported = parts.length > 4 && Boolean.parseBoolean(parts[4]);
        String accessRulesStr = parts.length > 5 ? parts[5] : "";
        switch (type) {
            case "LIB":
                IPath jarPath = Path.fromPortableString(path);
                if (!fileExists(path, jarPath)) {
                    String workspacePath = BazelBridge.getInstance().getWorkspacePath();
                    if (workspacePath != null) {
                        String fallback = BazelExternalRepoResolver.resolveFallbackJar(
                            path, workspacePath);
                        if (fallback != null) {
                            jarPath = Path.fromPortableString(fallback);
                            path = fallback;
                        }
                    }
                    if (!fileExists(path, jarPath)) {
                        if (WARNED_MISSING_PATHS.add(path)) {
                            LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                                "Skipping non-existent JAR: " + path));
                        }
                        return null;
                    }
                }
                IPath srcPath = sourcePath != null ? Path.fromPortableString(sourcePath) : null;
                if (srcPath != null && !fileExists(sourcePath, srcPath)) {
                    if (WARNED_MISSING_PATHS.add(sourcePath)) {
                        LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                            "Source attachment path does not exist, ignoring: " + sourcePath));
                    }
                    srcPath = null;
                }
                IAccessRule[] accessRules = parseAccessRules(accessRulesStr);
                boolean matchesTestPattern = false;
                if (srcPath != null) {
                    String srcPathStr = srcPath.toPortableString();
                    for (String pattern : testSourcePatterns) {
                        String prefix = pattern.replace("/**", "").replace("**", "");
                        if (!prefix.isEmpty() && srcPathStr.contains(prefix)) {
                            matchesTestPattern = true;
                            break;
                        }
                    }
                }
                boolean effectiveIsTest = isTest || matchesTestPattern;
                if (effectiveIsTest) {
                    List<IClasspathAttribute> attrs = new ArrayList<>();
                    attrs.add(JavaCore.newClasspathAttribute(IClasspathAttribute.TEST, "true"));
                    return JavaCore.newLibraryEntry(jarPath, srcPath, null,
                        accessRules,
                        attrs.toArray(new IClasspathAttribute[0]),
                        isExported);
                }
                return JavaCore.newLibraryEntry(jarPath, srcPath, null,
                    accessRules,
                    new org.eclipse.jdt.core.IClasspathAttribute[0],
                    isExported);
            case "PROJ":
                if (path.startsWith("@@")) {
                    return null;
                }
                String projectName = LabelUtils.toProjectName(extractPackageName(path));
                if (ownerProjectName != null && projectName.equals(ownerProjectName)) {
                    return null;
                }
                // In source-view mode, skip project references entirely —
                // dependencies are only available via source attachment on LIB entries
                String loadingMode = BazelBridge.getInstance().getDependencySourceLoadingMode();
                if ("source-view".equals(loadingMode)) {
                    return null;
                }
                if ("optional".equals(resolutionMode)) {
                    IClasspathAttribute[] optionalAttrs = new IClasspathAttribute[]{
                        JavaCore.newClasspathAttribute(IClasspathAttribute.OPTIONAL, "true")
                    };
                    return JavaCore.newProjectEntry(
                        Path.fromPortableString("/" + projectName),
                        new IAccessRule[0],
                        true,
                        optionalAttrs,
                        false);
                }
                if (!ResourcesPlugin.getWorkspace().getRoot().getProject(projectName).exists()) {
                    return null;
                }
                return JavaCore.newProjectEntry(Path.fromPortableString("/" + projectName));
            case "SRC":
                if (isTest) {
                    IClasspathAttribute[] testAttrs = new IClasspathAttribute[]{
                        JavaCore.newClasspathAttribute(IClasspathAttribute.TEST, "true")
                    };
                    return JavaCore.newSourceEntry(Path.fromPortableString(path),
                        null, null, null, testAttrs);
                }
                return JavaCore.newSourceEntry(Path.fromPortableString(path));
            default:
                return null;
        }
    }

    private IAccessRule[] parseAccessRules(String rulesStr) {
        if (rulesStr == null || rulesStr.isEmpty()) {
            return new IAccessRule[0];
        }
        List<IAccessRule> rules = new ArrayList<>();
        for (String rule : rulesStr.split(":")) {
            String trimmed = rule.trim();
            if (trimmed.isEmpty()) continue;
            if (trimmed.startsWith("+")) {
                rules.add(JavaCore.newAccessRule(
                    Path.fromPortableString(trimmed.substring(1) + "/**"),
                    IAccessRule.K_ACCESSIBLE));
            } else if (trimmed.startsWith("-")) {
                rules.add(JavaCore.newAccessRule(
                    Path.fromPortableString(trimmed.substring(1) + "/**"),
                    IAccessRule.K_NON_ACCESSIBLE));
            }
        }
        return rules.toArray(new IAccessRule[0]);
    }

    private static String extractPackageName(String targetLabel) {
        return LabelUtils.extractPackageName(targetLabel);
    }

    private static boolean fileExists(String pathKey, IPath ipath) {
        return FILE_EXISTS_CACHE.computeIfAbsent(pathKey, k -> ipath.toFile().exists());
    }

    static void resetWarnings() {
        WARNED_MISSING_PATHS.clear();
        FILE_EXISTS_CACHE.clear();
    }

    @Override
    public IClasspathEntry[] getClasspathEntries() {
        return entries;
    }

    @Override
    public String getDescription() {
        return DESCRIPTION;
    }

    @Override
    public int getKind() {
        return K_APPLICATION;
    }

    @Override
    public IPath getPath() {
        return CONTAINER_PATH;
    }
}
