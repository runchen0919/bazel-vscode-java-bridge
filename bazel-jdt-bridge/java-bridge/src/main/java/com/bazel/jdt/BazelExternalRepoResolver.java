package com.bazel.jdt;

import java.io.File;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.concurrent.ConcurrentHashMap;
import java.util.logging.Logger;

public final class BazelExternalRepoResolver {
    private static final Logger LOG = Logger.getLogger(BazelExternalRepoResolver.class.getName());
    private static final ConcurrentHashMap<String, String> OUTPUT_BASE_CACHE = new ConcurrentHashMap<>();
    private static final ConcurrentHashMap<String, String> JAR_FALLBACK_CACHE = new ConcurrentHashMap<>();
    private static final int MAX_JAR_SEARCH_DEPTH = 3;

    private BazelExternalRepoResolver() {}

    static String resolveOutputBase(String workspacePath) {
        return OUTPUT_BASE_CACHE.computeIfAbsent(workspacePath, ws -> {
            try {
                Path bazelOut = new File(ws, "bazel-out").toPath();
                if (Files.exists(bazelOut)) {
                    Path resolved = bazelOut.toRealPath();
                    Path execroot = resolved.getParent();
                    if (execroot != null) {
                        Path execrootParent = execroot.getParent();
                        if (execrootParent != null) {
                            Path outputBase = execrootParent.getParent();
                            if (outputBase != null
                                    && Files.isDirectory(outputBase.resolve("external"))) {
                                String result = outputBase.toString();
                                LOG.info("Resolved output_base from bazel-out symlink: " + result);
                                return result;
                            }
                        }
                    }
                }
            } catch (IOException e) {
                LOG.warning("Failed to resolve bazel-out symlink: " + e.getMessage());
            }

            try {
                ProcessBuilder pb = new ProcessBuilder("bazel", "info", "output_base");
                pb.directory(new File(ws));
                pb.redirectErrorStream(true);
                Process proc = pb.start();
                String output = new String(proc.getInputStream().readAllBytes()).trim();
                int exitCode = proc.waitFor();
                if (exitCode == 0 && !output.isEmpty() && new File(output).isDirectory()) {
                    LOG.info("Resolved output_base from bazel info: " + output);
                    return output;
                }
            } catch (Exception e) {
                LOG.warning("Failed to run bazel info output_base: " + e.getMessage());
            }

            return null;
        });
    }

    static String resolveFallbackJar(String missingPath, String workspacePath) {
        return JAR_FALLBACK_CACHE.computeIfAbsent(missingPath, path -> {
            String outputBase = resolveOutputBase(workspacePath);
            if (outputBase == null) return null;

            String repoName = extractRepoName(path);
            if (repoName != null) {
                File repoDir = new File(outputBase, "external/" + repoName);
                if (!repoDir.isDirectory()) {
                    repoDir = findBzlmodRepoDir(outputBase, repoName);
                }
                if (repoDir != null && repoDir.isDirectory()) {
                    String found = findJarInDirectory(repoDir, MAX_JAR_SEARCH_DEPTH);
                    if (found != null) {
                        LOG.info("Fallback JAR resolved: " + path + " -> " + found);
                        return found;
                    }
                }
                return null;
            }

            return resolveBuildOutputJar(path, outputBase);
        });
    }

    static String resolveBuildOutputJar(String missingPath, String outputBase) {
        String artifactName = extractArtifactNameFromLibJar(missingPath);
        if (artifactName == null) return null;

        File repoDir = findCandidateRepoDir(outputBase, artifactName);
        if (repoDir == null) return null;

        String found = findJarInDirectory(repoDir, MAX_JAR_SEARCH_DEPTH);
        if (found != null) {
            LOG.info("Build-output JAR resolved: " + missingPath + " -> " + found);
        }
        return found;
    }

    static String extractArtifactNameFromLibJar(String path) {
        String fileName = path;
        int lastSlash = path.lastIndexOf('/');
        if (lastSlash >= 0) {
            fileName = path.substring(lastSlash + 1);
        }
        if (!fileName.startsWith("lib") || !fileName.endsWith(".jar")) return null;
        String name = fileName.substring(3, fileName.length() - 4);
        if (name.isEmpty()) return null;
        return name;
    }

    static File findCandidateRepoDir(String outputBase, String artifactName) {
        File externalRoot = new File(outputBase, "external");
        if (!externalRoot.isDirectory()) return null;

        File exact1 = new File(externalRoot, artifactName + "_" + artifactName);
        if (exact1.isDirectory()) return exact1;

        File exact2 = new File(externalRoot, artifactName);
        if (exact2.isDirectory()) return exact2;

        File[] prefixMatches = externalRoot.listFiles((dir, name) ->
            name.startsWith(artifactName + "_"));
        if (prefixMatches != null && prefixMatches.length > 0) {
            for (File candidate : prefixMatches) {
                if (candidate.isDirectory()) return candidate;
            }
        }

        return findBzlmodRepoDir(outputBase, artifactName);
    }

    static String extractRepoName(String path) {
        int idx = path.indexOf("/external/");
        if (idx < 0) return null;
        String afterExternal = path.substring(idx + "/external/".length());
        int slash = afterExternal.indexOf('/');
        if (slash <= 0) return null;
        return afterExternal.substring(0, slash);
    }

    static File findBzlmodRepoDir(String outputBase, String repoName) {
        File externalRoot = new File(outputBase, "external");
        if (!externalRoot.isDirectory()) return null;
        File[] candidates = externalRoot.listFiles((dir, name) ->
            name.contains("~~") && name.endsWith("~" + repoName));
        if (candidates != null && candidates.length > 0) {
            return candidates[0];
        }
        return null;
    }

    private static String findJarInDirectory(File dir, int maxDepth) {
        if (maxDepth <= 0 || !dir.isDirectory()) return null;
        File[] files = dir.listFiles();
        if (files == null) return null;

        for (File f : files) {
            if (f.isFile() && f.getName().endsWith(".jar")
                    && !f.getName().endsWith("-sources.jar")) {
                return f.getAbsolutePath();
            }
        }
        for (File f : files) {
            if (f.isDirectory()) {
                String found = findJarInDirectory(f, maxDepth - 1);
                if (found != null) return found;
            }
        }
        return null;
    }

    static void setOutputBaseForTest(String workspacePath, String outputBase) {
        OUTPUT_BASE_CACHE.put(workspacePath, outputBase);
    }

    static void resetCaches() {
        OUTPUT_BASE_CACHE.clear();
        JAR_FALLBACK_CACHE.clear();
    }
}
