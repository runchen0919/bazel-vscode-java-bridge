package com.bazel.jdt;

import static org.junit.Assert.*;

import java.io.File;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;

public class BazelExternalRepoResolverTest {

    private Path tempDir;

    @Before
    public void setUp() throws IOException {
        tempDir = Files.createTempDirectory("bazel-resolver-test");
        BazelExternalRepoResolver.resetCaches();
    }

    @After
    public void tearDown() {
        BazelExternalRepoResolver.resetCaches();
        deleteRecursive(tempDir.toFile());
    }

    @Test
    public void extractRepoNameFromExternalPath() {
        assertEquals("junit_junit",
            BazelExternalRepoResolver.extractRepoName(
                "/private/var/tmp/_bazel/execroot/ws/external/junit_junit/jar/_ijar/downloaded-ijar.jar"));
    }

    @Test
    public void extractRepoNameFromBazelOutPath() {
        assertEquals("maven",
            BazelExternalRepoResolver.extractRepoName(
                "/workspace/bazel-out/k8-fastbuild/bin/external/maven/com/junit/junit/4.12/_ijar/downloaded-ijar.jar"));
    }

    @Test
    public void extractRepoNameReturnsNullForNonExternalPath() {
        assertNull(BazelExternalRepoResolver.extractRepoName(
            "/workspace/bazel-out/k8-fastbuild/bin/3rdparty/libjunit.jar"));
    }

    @Test
    public void extractRepoNameReturnsNullForEmptyAfterExternal() {
        assertNull(BazelExternalRepoResolver.extractRepoName(
            "/workspace/external/"));
    }

    @Test
    public void findBzlmodRepoDirMatchesTildePattern() throws IOException {
        String outputBase = tempDir.toString();
        File bzlmodDir = new File(outputBase, "external/rules_jvm_external~~maven~maven");
        assertTrue(bzlmodDir.mkdirs());

        File result = BazelExternalRepoResolver.findBzlmodRepoDir(outputBase, "maven");
        assertNotNull(result);
        assertEquals(bzlmodDir.getAbsolutePath(), result.getAbsolutePath());
    }

    @Test
    public void findBzlmodRepoDirReturnsNullWhenNoMatch() throws IOException {
        String outputBase = tempDir.toString();
        File externalDir = new File(outputBase, "external");
        assertTrue(externalDir.mkdirs());

        assertNull(BazelExternalRepoResolver.findBzlmodRepoDir(outputBase, "maven"));
    }

    @Test
    public void resolveFallbackJarFindsJarInExternalRepo() throws IOException {
        String outputBase = tempDir.toString();
        File repoDir = new File(outputBase, "external/junit_junit/jar");
        assertTrue(repoDir.mkdirs());
        File jar = new File(repoDir, "downloaded.jar");
        assertTrue(jar.createNewFile());

        String wsPath = tempDir.resolve("workspace").toString();
        BazelExternalRepoResolver.setOutputBaseForTest(wsPath, outputBase);

        String missingPath = outputBase + "/execroot/ws/external/junit_junit/jar/_ijar/downloaded-ijar.jar";
        String result = BazelExternalRepoResolver.resolveFallbackJar(missingPath, wsPath);

        assertNotNull("Should resolve fallback JAR", result);
        assertEquals(jar.getAbsolutePath(), result);
    }

    @Test
    public void resolveFallbackJarSkipsSourcesJar() throws IOException {
        String outputBase = tempDir.toString();
        File repoDir = new File(outputBase, "external/guava/jar");
        assertTrue(repoDir.mkdirs());
        assertTrue(new File(repoDir, "guava-31.1-sources.jar").createNewFile());
        File classesJar = new File(repoDir, "guava-31.1.jar");
        assertTrue(classesJar.createNewFile());

        String wsPath = tempDir.resolve("workspace").toString();
        BazelExternalRepoResolver.setOutputBaseForTest(wsPath, outputBase);

        String missingPath = outputBase + "/execroot/ws/external/guava/jar/_ijar/downloaded-ijar.jar";
        String result = BazelExternalRepoResolver.resolveFallbackJar(missingPath, wsPath);

        assertNotNull(result);
        assertEquals(classesJar.getAbsolutePath(), result);
    }

    @Test
    public void resolveFallbackJarReturnsNullWhenNoJarExists() throws IOException {
        String outputBase = tempDir.toString();
        File repoDir = new File(outputBase, "external/missing_repo");
        assertTrue(repoDir.mkdirs());

        String wsPath = tempDir.resolve("workspace").toString();
        BazelExternalRepoResolver.setOutputBaseForTest(wsPath, outputBase);

        String missingPath = outputBase + "/execroot/ws/external/missing_repo/jar/foo.jar";
        String result = BazelExternalRepoResolver.resolveFallbackJar(missingPath, wsPath);

        assertNull(result);
    }

    @Test
    public void resolveFallbackJarResolvesLibJarViaBuildOutput() throws IOException {
        String outputBase = tempDir.toString();
        File repoDir = new File(outputBase, "external/junit_junit/jar");
        assertTrue(repoDir.mkdirs());
        File jar = new File(repoDir, "junit-4.13.2.jar");
        assertTrue(jar.createNewFile());

        String wsPath = tempDir.resolve("workspace").toString();
        BazelExternalRepoResolver.setOutputBaseForTest(wsPath, outputBase);

        String result = BazelExternalRepoResolver.resolveFallbackJar(
            "/workspace/bazel-out/darwin_arm64-fastbuild/bin/3rdparty/libjunit.jar", wsPath);
        assertNotNull("Should resolve lib<name>.jar via build-output fallback", result);
        assertEquals(jar.getAbsolutePath(), result);
    }

    @Test
    public void resolveFallbackJarReturnsNullForNonExternalNonLibPath() {
        String wsPath = tempDir.resolve("workspace").toString();
        BazelExternalRepoResolver.setOutputBaseForTest(wsPath, tempDir.toString());

        String result = BazelExternalRepoResolver.resolveFallbackJar(
            "/workspace/bazel-out/bin/3rdparty/junit.jar", wsPath);
        assertNull(result);
    }

    // --- extractArtifactNameFromLibJar tests ---

    @Test
    public void extractArtifactNameFromValidLibJar() {
        assertEquals("junit",
            BazelExternalRepoResolver.extractArtifactNameFromLibJar(
                "/workspace/bazel-out/bin/3rdparty/libjunit.jar"));
    }

    @Test
    public void extractArtifactNameFromLibJarWithUnderscore() {
        assertEquals("hamcrest_core",
            BazelExternalRepoResolver.extractArtifactNameFromLibJar(
                "bazel-out/bin/3rdparty/libhamcrest_core.jar"));
    }

    @Test
    public void extractArtifactNameReturnsNullForNonLibPrefix() {
        assertNull(BazelExternalRepoResolver.extractArtifactNameFromLibJar(
            "bazel-out/bin/3rdparty/junit.jar"));
    }

    @Test
    public void extractArtifactNameReturnsNullForNonJarExtension() {
        assertNull(BazelExternalRepoResolver.extractArtifactNameFromLibJar(
            "bazel-out/bin/3rdparty/libfoo.txt"));
    }

    @Test
    public void extractArtifactNameReturnsNullForEmptyName() {
        assertNull(BazelExternalRepoResolver.extractArtifactNameFromLibJar(
            "bazel-out/bin/3rdparty/lib.jar"));
    }

    @Test
    public void extractArtifactNameFromFileNameOnly() {
        assertEquals("guava",
            BazelExternalRepoResolver.extractArtifactNameFromLibJar("libguava.jar"));
    }

    // --- findCandidateRepoDir tests ---

    @Test
    public void findCandidateRepoDirExactDoubleMatch() throws IOException {
        String outputBase = tempDir.toString();
        File exact = new File(outputBase, "external/junit_junit");
        assertTrue(exact.mkdirs());

        File result = BazelExternalRepoResolver.findCandidateRepoDir(outputBase, "junit");
        assertNotNull(result);
        assertEquals(exact.getAbsolutePath(), result.getAbsolutePath());
    }

    @Test
    public void findCandidateRepoDirExactSingleMatch() throws IOException {
        String outputBase = tempDir.toString();
        File exact = new File(outputBase, "external/guava");
        assertTrue(exact.mkdirs());

        File result = BazelExternalRepoResolver.findCandidateRepoDir(outputBase, "guava");
        assertNotNull(result);
        assertEquals(exact.getAbsolutePath(), result.getAbsolutePath());
    }

    @Test
    public void findCandidateRepoDirPrefersDoubleOverSingle() throws IOException {
        String outputBase = tempDir.toString();
        File single = new File(outputBase, "external/junit");
        assertTrue(single.mkdirs());
        File double_ = new File(outputBase, "external/junit_junit");
        assertTrue(double_.mkdirs());

        File result = BazelExternalRepoResolver.findCandidateRepoDir(outputBase, "junit");
        assertNotNull(result);
        assertEquals(double_.getAbsolutePath(), result.getAbsolutePath());
    }

    @Test
    public void findCandidateRepoDirPrefixMatch() throws IOException {
        String outputBase = tempDir.toString();
        File prefixed = new File(outputBase, "external/guava_guava_jre");
        assertTrue(prefixed.mkdirs());

        File result = BazelExternalRepoResolver.findCandidateRepoDir(outputBase, "guava");
        assertNotNull(result);
        assertEquals(prefixed.getAbsolutePath(), result.getAbsolutePath());
    }

    @Test
    public void findCandidateRepoDirBzlmodMatch() throws IOException {
        String outputBase = tempDir.toString();
        File bzlmod = new File(outputBase, "external/rules_jvm_external~~maven~junit");
        assertTrue(bzlmod.mkdirs());

        File result = BazelExternalRepoResolver.findCandidateRepoDir(outputBase, "junit");
        assertNotNull(result);
        assertEquals(bzlmod.getAbsolutePath(), result.getAbsolutePath());
    }

    @Test
    public void findCandidateRepoDirNoMatch() throws IOException {
        String outputBase = tempDir.toString();
        File externalDir = new File(outputBase, "external");
        assertTrue(externalDir.mkdirs());

        assertNull(BazelExternalRepoResolver.findCandidateRepoDir(outputBase, "nonexistent"));
    }

    // --- resolveBuildOutputJar end-to-end tests ---

    @Test
    public void resolveBuildOutputJarEndToEnd() throws IOException {
        String outputBase = tempDir.toString();
        File repoDir = new File(outputBase, "external/hamcrest_core/jar");
        assertTrue(repoDir.mkdirs());
        File jar = new File(repoDir, "hamcrest-core-1.3.jar");
        assertTrue(jar.createNewFile());

        String result = BazelExternalRepoResolver.resolveBuildOutputJar(
            "bazel-out/k8-fastbuild/bin/3rdparty/libhamcrest_core.jar", outputBase);
        assertNotNull(result);
        assertEquals(jar.getAbsolutePath(), result);
    }

    @Test
    public void resolveBuildOutputJarReturnsNullForNonLibJar() {
        String result = BazelExternalRepoResolver.resolveBuildOutputJar(
            "bazel-out/bin/3rdparty/junit.jar", tempDir.toString());
        assertNull(result);
    }

    @Test
    public void resolveBuildOutputJarReturnsNullWhenNoRepoDir() throws IOException {
        String outputBase = tempDir.toString();
        File externalDir = new File(outputBase, "external");
        assertTrue(externalDir.mkdirs());

        String result = BazelExternalRepoResolver.resolveBuildOutputJar(
            "bazel-out/bin/3rdparty/libunknown.jar", outputBase);
        assertNull(result);
    }

    private static void deleteRecursive(File file) {
        if (file.isDirectory()) {
            File[] children = file.listFiles();
            if (children != null) {
                for (File child : children) {
                    deleteRecursive(child);
                }
            }
        }
        file.delete();
    }
}
