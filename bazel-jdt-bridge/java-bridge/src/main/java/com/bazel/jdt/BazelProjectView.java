package com.bazel.jdt;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileReader;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

import org.eclipse.core.runtime.ILog;
import org.eclipse.core.runtime.IStatus;
import org.eclipse.core.runtime.Platform;
import org.eclipse.core.runtime.Status;

/**
 * Parses IntelliJ-compatible .bazelproject files.
 * Supported sections: directories, derive_targets_from_directories, targets.
 */
public final class BazelProjectView {
    private static final ILog LOG = Platform.getLog(BazelProjectView.class);
    private static final String BAZELPROJECT_FILE = ".bazelproject";

    private final List<String> directories = new ArrayList<>();
    private final List<String> targets = new ArrayList<>();
    private final List<String> buildFlags = new ArrayList<>();
    private final List<String> syncFlags = new ArrayList<>();
    private final List<String> testSources = new ArrayList<>();
    private final List<String> excludeTarget = new ArrayList<>();
    private final List<String> imports = new ArrayList<>();
    private boolean deriveTargetsFromDirectories = true;
    private String bazelBinary = "";
    private String javaLanguageLevel = "";

    private BazelProjectView() {}

    public static BazelProjectView parse(File workspaceRoot) {
        File projectViewFile = new File(workspaceRoot, BAZELPROJECT_FILE);
        if (!projectViewFile.exists()) {
            return null;
        }

        BazelProjectView view = new BazelProjectView();
        String currentSection = null;

        try (BufferedReader reader = new BufferedReader(new FileReader(projectViewFile, StandardCharsets.UTF_8))) {
            String line;
            while ((line = reader.readLine()) != null) {
                String trimmed = line.trim();
                if (trimmed.isEmpty() || trimmed.startsWith("#")) {
                    continue;
                }

                if (trimmed.endsWith(":")) {
                    currentSection = trimmed.substring(0, trimmed.length() - 1).trim().toLowerCase();
                    continue;
                }

                int colonIdx = trimmed.indexOf(':');
                if (colonIdx > 0 && !trimmed.endsWith(":")) {
                    String key = trimmed.substring(0, colonIdx).trim().toLowerCase();
                    String value = trimmed.substring(colonIdx + 1).trim();
                    if ("derive_targets_from_directories".equals(key)) {
                        view.deriveTargetsFromDirectories = "true".equalsIgnoreCase(value);
                        currentSection = null;
                        continue;
                    }
                    if ("bazel_binary".equals(key)) {
                        view.bazelBinary = value;
                        currentSection = null;
                        continue;
                    }
                    if ("java_language_level".equals(key)) {
                        view.javaLanguageLevel = value;
                        currentSection = null;
                        continue;
                    }
                }

                if (currentSection == null) {
                    continue;
                }

                switch (currentSection) {
                    case "directories":
                        view.directories.add(trimmed);
                        break;
                    case "derive_targets_from_directories":
                        view.deriveTargetsFromDirectories = "true".equalsIgnoreCase(trimmed);
                        break;
                    case "targets":
                        view.targets.add(trimmed);
                        break;
                    case "build_flags":
                        view.buildFlags.add(trimmed);
                        break;
                    case "sync_flags":
                        view.syncFlags.add(trimmed);
                        break;
                    case "test_sources":
                        view.testSources.add(trimmed);
                        break;
                    case "exclude_target":
                        view.excludeTarget.add(trimmed);
                        break;
                    case "import":
                    case "try_import":
                        view.imports.add(trimmed);
                        break;
                    default:
                        break;
                }
            }
        } catch (IOException e) {
            LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                "Failed to parse .bazelproject: " + e.getMessage(), e));
        }

        return view;
    }

    public List<String> getScopePatterns() {
        List<String> patterns = new ArrayList<>();

        if (deriveTargetsFromDirectories) {
            for (String dir : directories) {
                if (dir.startsWith("-")) {
                    String path = dir.substring(1);
                    if (".".equals(path)) {
                        patterns.add("-//...:*");
                    } else {
                        patterns.add("-//" + path + "/...:*");
                    }
                } else {
                    if (".".equals(dir)) {
                        patterns.add("//...:*");
                    } else {
                        patterns.add("//" + dir + "/...:*");
                    }
                }
            }
        }

        patterns.addAll(targets);

        for (String target : excludeTarget) {
            if (target.startsWith("-")) {
                patterns.add(target);
            } else {
                patterns.add("-" + target);
            }
        }

        return patterns;
    }

    public List<String> getDirectories() {
        return Collections.unmodifiableList(directories);
    }

    public List<String> getTargets() {
        return Collections.unmodifiableList(targets);
    }

    public boolean isDeriveTargetsFromDirectories() {
        return deriveTargetsFromDirectories;
    }

    public boolean hasScope() {
        return !directories.isEmpty() || !targets.isEmpty() || !excludeTarget.isEmpty();
    }

    public List<String> getBuildFlags() {
        return Collections.unmodifiableList(buildFlags);
    }

    public List<String> getSyncFlags() {
        return Collections.unmodifiableList(syncFlags);
    }

    public List<String> getTestSourcePatterns() {
        return Collections.unmodifiableList(testSources);
    }

    public List<String> getExcludeTargets() {
        return Collections.unmodifiableList(excludeTarget);
    }

    public List<String> getImports() {
        return Collections.unmodifiableList(imports);
    }

    public String getBazelBinary() {
        return bazelBinary;
    }

    public String getJavaLanguageLevel() {
        return javaLanguageLevel;
    }
}
