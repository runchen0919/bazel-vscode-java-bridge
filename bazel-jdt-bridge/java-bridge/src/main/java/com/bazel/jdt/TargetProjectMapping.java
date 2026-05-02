package com.bazel.jdt;

import org.eclipse.core.resources.IProject;
import org.eclipse.core.runtime.CoreException;
import org.eclipse.core.runtime.ILog;
import org.eclipse.core.runtime.Platform;
import org.eclipse.core.runtime.QualifiedName;
import org.osgi.framework.Bundle;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Set;

/**
 * Centralized utility for persisting the mapping between Eclipse projects
 * and their concrete Bazel target labels, plus file-based classpath caching.
 * <p>
 * Target labels and workspace config use Eclipse persistent properties.
 * Classpath cache uses the plugin state area to avoid the persistent property
 * size limit (~32KB).
 */
public final class TargetProjectMapping {

    private static final ILog LOG = Platform.getLog(TargetProjectMapping.class);

    static final String QUALIFIER = "com.bazel.jdt";
    static final String KEY = "targetLabels";
    static final String KEY_WORKSPACE_PATH = "workspacePath";
    static final String KEY_BAZEL_PATH = "bazelPath";
    static final String KEY_CACHE_DIR = "cacheDir";
    static final String KEY_CACHED_CLASSPATH = "cachedClasspath";

    private TargetProjectMapping() {}

    private static QualifiedName propertyName() {
        return new QualifiedName(QUALIFIER, KEY);
    }

    /**
     * Store target labels for a project, replacing any existing mapping.
     *
     * @param project      the Eclipse project
     * @param targetLabels list of concrete Bazel target labels (e.g. "//app:lib")
     */
    public static void storeTargets(IProject project, List<String> targetLabels) {
        try {
            String value = String.join(",", targetLabels);
            project.setPersistentProperty(propertyName(), value);
            LOG.info("Stored target labels for project '" + project.getName() + "': " + value);
        } catch (CoreException e) {
            LOG.error("Failed to store target labels for project '" + project.getName() + "'", e);
        }
    }

    /**
     * Read the persisted target labels for a project.
     *
     * @param project the Eclipse project
     * @return list of concrete Bazel target labels, or empty list if none persisted
     */
    public static List<String> readTargets(IProject project) {
        try {
            String value = project.getPersistentProperty(propertyName());
            if (value == null || value.isEmpty()) {
                return Collections.emptyList();
            }
            List<String> labels = new ArrayList<>();
            for (String label : value.split(",")) {
                String trimmed = label.trim();
                if (!trimmed.isEmpty()) {
                    labels.add(trimmed);
                }
            }
            return labels;
        } catch (CoreException e) {
            LOG.error("Failed to read target labels for project '" + project.getName() + "'", e);
            return Collections.emptyList();
        }
    }

    /**
     * Append target labels to a project's existing mapping, deduplicating.
     *
     * @param project   the Eclipse project
     * @param newLabels labels to add
     */
    public static void appendTargets(IProject project, List<String> newLabels) {
        List<String> existing = readTargets(project);
        Set<String> merged = new LinkedHashSet<>(existing);
        merged.addAll(newLabels);
        storeTargets(project, new ArrayList<>(merged));
    }

    /**
     * Remove the persisted target labels from a project.
     *
     * @param project the Eclipse project
     */
    public static void clearTargets(IProject project) {
        try {
            project.setPersistentProperty(propertyName(), null);
        } catch (CoreException e) {
            LOG.error("Failed to clear target labels for project '" + project.getName() + "'", e);
        }
    }

    public static void storeWorkspaceConfig(IProject project, String workspacePath,
            String bazelPath, String cacheDir) {
        try {
            project.setPersistentProperty(new QualifiedName(QUALIFIER, KEY_WORKSPACE_PATH), workspacePath);
            project.setPersistentProperty(new QualifiedName(QUALIFIER, KEY_BAZEL_PATH), bazelPath);
            project.setPersistentProperty(new QualifiedName(QUALIFIER, KEY_CACHE_DIR), cacheDir);
            LOG.info("Stored workspace config for project '" + project.getName() + "'");
        } catch (CoreException e) {
            LOG.error("Failed to store workspace config for '" + project.getName() + "'", e);
        }
    }

    public static String[] readWorkspaceConfig(IProject project) {
        try {
            String ws = project.getPersistentProperty(new QualifiedName(QUALIFIER, KEY_WORKSPACE_PATH));
            String bp = project.getPersistentProperty(new QualifiedName(QUALIFIER, KEY_BAZEL_PATH));
            String cd = project.getPersistentProperty(new QualifiedName(QUALIFIER, KEY_CACHE_DIR));
            if (ws == null) return null;
            return new String[]{ws, bp != null ? bp : "bazel", cd != null ? cd : BazelCommandHandler.DEFAULT_CACHE_DIR};
        } catch (CoreException e) {
            LOG.error("Failed to read workspace config for '" + project.getName() + "'", e);
            return null;
        }
    }

    private static String sanitizeLabel(String label) {
        return label
                .replace("//", "_")
                .replace(":", "_")
                .replace("@", "_")
                .replace("/", "_")
                .replace("~", "_")
                + ".cache";
    }

    private static Path getCacheDir() throws IOException {
        Bundle bundle = Platform.getBundle("com.bazel.jdt");
        Path stateDir = Platform.getStateLocation(bundle).toFile().toPath();
        Path cacheDir = stateDir.resolve("classpath-cache");
        Files.createDirectories(cacheDir);
        return cacheDir;
    }

    private static Path getCacheFile(String targetLabel) throws IOException {
        return getCacheDir().resolve(sanitizeLabel(targetLabel));
    }

    public static void storeCachedClasspath(IProject project, String targetLabel, String[] rawEntries) {
        try {
            Path cacheFile = getCacheFile(targetLabel);
            String value = rawEntries == null ? "" : String.join("\n", rawEntries);
            Files.writeString(cacheFile, value);
        } catch (IOException e) {
            LOG.error("Failed to cache classpath for '" + targetLabel + "' to file", e);
        }
    }

    public static String[] readCachedClasspath(IProject project, String targetLabel) {
        try {
            Path cacheFile = getCacheFile(targetLabel);
            if (Files.exists(cacheFile)) {
                String value = Files.readString(cacheFile);
                if (value.isEmpty()) return null;
                return value.split("\n");
            }
        } catch (IOException e) {
            LOG.error("Failed to read cached classpath file for '" + targetLabel + "'", e);
        }

        try {
            String key = KEY_CACHED_CLASSPATH + "." + targetLabel;
            String value = project.getPersistentProperty(new QualifiedName(QUALIFIER, key));
            if (value != null && !value.isEmpty()) {
                try {
                    Path cacheFile = getCacheFile(targetLabel);
                    Files.writeString(cacheFile, value);
                } catch (IOException e) {
                    LOG.error("Failed to migrate classpath cache for '" + targetLabel + "'", e);
                }
                project.setPersistentProperty(new QualifiedName(QUALIFIER, key), null);
                return value.split("\n");
            }
        } catch (CoreException e) {
            LOG.error("Failed to read/migrate old classpath cache for '" + targetLabel + "'", e);
        }
        return null;
    }

    public static void clearClasspathCache() {
        try {
            Path cacheDir = getCacheDir();
            try (var stream = Files.newDirectoryStream(cacheDir, "*.cache")) {
                for (Path file : stream) {
                    Files.deleteIfExists(file);
                }
            }
        } catch (IOException e) {
            LOG.error("Failed to clear classpath cache directory", e);
        }
    }
}
