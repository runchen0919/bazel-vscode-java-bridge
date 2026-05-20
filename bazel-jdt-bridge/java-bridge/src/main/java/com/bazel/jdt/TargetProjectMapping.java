package com.bazel.jdt;

import org.eclipse.core.resources.IProject;
import org.eclipse.core.runtime.CoreException;
import org.eclipse.core.runtime.ILog;
import org.eclipse.core.runtime.Platform;
import org.eclipse.core.runtime.QualifiedName;
import org.osgi.framework.Bundle;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HexFormat;
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
    static final String KEY_AUTO_IMPORTED = "autoImported";

    private TargetProjectMapping() {}

    private static QualifiedName propertyName() {
        return new QualifiedName(QUALIFIER, KEY);
    }

    static Path getStateDir() throws IOException {
        Bundle bundle = Platform.getBundle("com.bazel.jdt");
        Path stateDir = Platform.getStateLocation(bundle).toFile().toPath();
        Files.createDirectories(stateDir);
        return stateDir;
    }

    private static Path getTargetLabelsDir() throws IOException {
        Path labelsDir = getStateDir().resolve("target-labels");
        Files.createDirectories(labelsDir);
        return labelsDir;
    }

    private static Path getTargetLabelsFile(String projectName) throws IOException {
        return getTargetLabelsDir().resolve(sanitizeLabel(projectName));
    }

    public static void storeTargets(IProject project, List<String> targetLabels) {
        try {
            Path labelsFile = getTargetLabelsFile(project.getName());
            String value = String.join("\n", targetLabels);
            Files.writeString(labelsFile, value);
            LOG.info("Stored target labels for project '" + project.getName() + "': " + targetLabels.size() + " labels");
            if (!targetLabels.isEmpty()) {
                String firstLabel = targetLabels.get(0);
                String packagePath = LabelUtils.extractPackageName(firstLabel);
                updateProjectIndex(project.getName(), firstLabel, packagePath);
            }
        } catch (IOException e) {
            LOG.error("Failed to store target labels for project '" + project.getName() + "'", e);
        }
    }

    public static List<String> readTargets(IProject project) {
        try {
            Path labelsFile = getTargetLabelsFile(project.getName());
            if (Files.exists(labelsFile)) {
                String value = Files.readString(labelsFile);
                if (value.isEmpty()) return Collections.emptyList();
                List<String> labels = new ArrayList<>();
                for (String label : value.split("\n")) {
                    String trimmed = label.trim();
                    if (!trimmed.isEmpty()) {
                        labels.add(trimmed);
                    }
                }
                return labels;
            }
        } catch (IOException e) {
            LOG.error("Failed to read target labels file for project '" + project.getName() + "'", e);
        }

        try {
            String value = project.getPersistentProperty(propertyName());
            if (value != null && !value.isEmpty()) {
                List<String> labels = new ArrayList<>();
                for (String label : value.split(",")) {
                    String trimmed = label.trim();
                    if (!trimmed.isEmpty()) {
                        labels.add(trimmed);
                    }
                }
                try {
                    Path labelsFile = getTargetLabelsFile(project.getName());
                    Files.writeString(labelsFile, String.join("\n", labels));
                } catch (IOException ex) {
                    LOG.error("Failed to migrate target labels to file for '" + project.getName() + "'", ex);
                }
                try {
                    project.setPersistentProperty(propertyName(), null);
                } catch (CoreException ex) {
                    LOG.error("Failed to clear migrated persistent property for '" + project.getName() + "'", ex);
                }
                return labels;
            }
        } catch (CoreException e) {
            LOG.error("Failed to read target labels property for project '" + project.getName() + "'", e);
        }
        return Collections.emptyList();
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
            Path labelsFile = getTargetLabelsFile(project.getName());
            Files.deleteIfExists(labelsFile);
        } catch (IOException e) {
            LOG.error("Failed to clear target labels file for project '" + project.getName() + "'", e);
        }
        removeFromProjectIndex(project.getName());
        try {
            project.setPersistentProperty(propertyName(), null);
        } catch (CoreException e) {
            LOG.error("Failed to clear target labels property for project '" + project.getName() + "'", e);
        }
    }

    private static final String WORKSPACE_CONFIG_FILE = "_workspace_config";

    public static void storeWorkspaceConfigFile(String workspacePath, String bazelPath, String cacheDir) {
        try {
            Path configFile = getStateDir().resolve(WORKSPACE_CONFIG_FILE);
            String content = "workspacePath=" + workspacePath + "\n"
                + "bazelPath=" + (bazelPath != null ? bazelPath : "bazel") + "\n"
                + "cacheDir=" + (cacheDir != null ? cacheDir : BazelCommandHandler.DEFAULT_CACHE_DIR) + "\n";
            Files.writeString(configFile, content);
        } catch (IOException e) {
            LOG.error("Failed to store workspace config file", e);
        }
    }

    public static String[] readWorkspaceConfigFile() {
        try {
            Path configFile = getStateDir().resolve(WORKSPACE_CONFIG_FILE);
            if (!Files.exists(configFile)) return null;
            String content = Files.readString(configFile);
            String ws = null, bp = null, cd = null;
            for (String line : content.split("\n")) {
                int eq = line.indexOf('=');
                if (eq < 0) continue;
                String key = line.substring(0, eq);
                String val = line.substring(eq + 1);
                switch (key) {
                    case "workspacePath": ws = val; break;
                    case "bazelPath": bp = val; break;
                    case "cacheDir": cd = val; break;
                }
            }
            if (ws == null || ws.isEmpty()) return null;
            return new String[]{ws, bp != null ? bp : "bazel", cd != null ? cd : BazelCommandHandler.DEFAULT_CACHE_DIR};
        } catch (IOException e) {
            LOG.error("Failed to read workspace config file", e);
            return null;
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

    /**
     * Read the workspace root path from any project that has it stored.
     * Used by on-demand project creation to find the workspace root.
     *
     * @return workspace path, or null if not available
     */
    public static String readWorkspacePath() {
        org.eclipse.core.resources.IProject[] projects =
            org.eclipse.core.resources.ResourcesPlugin.getWorkspace().getRoot().getProjects();
        for (IProject project : projects) {
            if (!project.exists()) continue;
            try {
                String value = project.getPersistentProperty(
                    new QualifiedName(QUALIFIER, KEY_WORKSPACE_PATH));
                if (value != null && !value.isEmpty()) {
                    return value;
                }
            } catch (CoreException e) {
                LOG.error("Failed to read workspace path from project '" + project.getName() + "'", e);
            }
        }
        return null;
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

    private static final String INDEX_FILE = "_index";

    public static void updateProjectIndex(String projectName, String targetLabel, String packagePath) {
        try {
            Path indexFile = getTargetLabelsDir().resolve(INDEX_FILE);
            List<String> lines = Files.exists(indexFile)
                ? new ArrayList<>(Files.readAllLines(indexFile)) : new ArrayList<>();
            String prefix = projectName + "|";
            String newLine = projectName + "|" + targetLabel + "|" + packagePath;
            boolean replaced = false;
            for (int i = 0; i < lines.size(); i++) {
                if (lines.get(i).startsWith(prefix)) {
                    lines.set(i, newLine);
                    replaced = true;
                    break;
                }
            }
            if (!replaced) {
                lines.add(newLine);
            }
            Files.writeString(indexFile, String.join("\n", lines) + "\n");
        } catch (IOException e) {
            LOG.error("Failed to update project index for '" + projectName + "'", e);
        }
    }

    public static void removeFromProjectIndex(String projectName) {
        try {
            Path indexFile = getTargetLabelsDir().resolve(INDEX_FILE);
            if (!Files.exists(indexFile)) return;
            List<String> lines = new ArrayList<>(Files.readAllLines(indexFile));
            String prefix = projectName + "|";
            lines.removeIf(line -> line.startsWith(prefix));
            Files.writeString(indexFile, String.join("\n", lines) + (lines.isEmpty() ? "" : "\n"));
        } catch (IOException e) {
            LOG.error("Failed to remove from project index for '" + projectName + "'", e);
        }
    }

    public static List<String[]> readProjectIndex() {
        try {
            Path indexFile = getTargetLabelsDir().resolve(INDEX_FILE);
            if (!Files.exists(indexFile)) return Collections.emptyList();
            List<String[]> result = new ArrayList<>();
            for (String line : Files.readAllLines(indexFile)) {
                String trimmed = line.trim();
                if (trimmed.isEmpty()) continue;
                String[] parts = trimmed.split("\\|", 3);
                if (parts.length == 3) {
                    result.add(parts);
                }
            }
            return result;
        } catch (IOException e) {
            LOG.error("Failed to read project index", e);
            return Collections.emptyList();
        }
    }

    private static String sanitizeLabel(String label) {
        try {
            MessageDigest digest = MessageDigest.getInstance("SHA-256");
            byte[] hash = digest.digest(label.getBytes(StandardCharsets.UTF_8));
            return HexFormat.of().formatHex(hash).substring(0, 16) + ".cache";
        } catch (NoSuchAlgorithmException e) {
            throw new RuntimeException("SHA-256 not available", e);
        }
    }

    private static Path getCacheDir() throws IOException {
        Path cacheDir = getStateDir().resolve("classpath-cache");
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

    public static void setAutoImported(IProject project, boolean autoImported) {
        try {
            project.setPersistentProperty(
                new QualifiedName(QUALIFIER, KEY_AUTO_IMPORTED),
                autoImported ? "true" : null);
        } catch (CoreException e) {
            LOG.error("Failed to set autoImported for '" + project.getName() + "'", e);
        }
    }

    public static boolean isAutoImported(IProject project) {
        try {
            String value = project.getPersistentProperty(
                new QualifiedName(QUALIFIER, KEY_AUTO_IMPORTED));
            return "true".equals(value);
        } catch (CoreException e) {
            LOG.error("Failed to read autoImported for '" + project.getName() + "'", e);
            return false;
        }
    }

    public static boolean hasWorkspaceConfig(org.eclipse.core.resources.IWorkspaceRoot workspaceRoot) {
        for (IProject project : workspaceRoot.getProjects()) {
            if (!project.isOpen()) continue;
            try {
                String ws = project.getPersistentProperty(
                    new QualifiedName(QUALIFIER, KEY_WORKSPACE_PATH));
                if (ws != null) return true;
            } catch (CoreException e) {
                // ignore
            }
        }
        return false;
    }
}
