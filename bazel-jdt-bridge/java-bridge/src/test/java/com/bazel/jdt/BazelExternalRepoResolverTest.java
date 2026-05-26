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
    public void resolveFallbackJarReturnsNullForNonExternalPath() {
        String wsPath = tempDir.resolve("workspace").toString();
        BazelExternalRepoResolver.setOutputBaseForTest(wsPath, tempDir.toString());

        String result = BazelExternalRepoResolver.resolveFallbackJar(
            "/workspace/bazel-out/bin/3rdparty/libjunit.jar", wsPath);
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
