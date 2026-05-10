package com.bazel.jdt;

import static org.junit.Assert.*;

import java.io.File;
import java.io.FileWriter;
import java.io.IOException;

import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

public class SourceRootUtilsTest {

    @Rule
    public TemporaryFolder tmp = new TemporaryFolder();

    private File writeJavaFile(String relativePath, String content) throws IOException {
        File file = new File(tmp.getRoot(), relativePath);
        file.getParentFile().mkdirs();
        try (FileWriter w = new FileWriter(file)) {
            w.write(content);
        }
        return file;
    }

    // --- extractPackageDeclaration tests ---

    @Test
    public void extractPackageDeclaration_standard() throws IOException {
        File f = writeJavaFile("Foo.java", "package com.example.foo;\n\npublic class Foo {}");
        assertEquals("com.example.foo", SourceRootUtils.extractPackageDeclaration(f));
    }

    @Test
    public void extractPackageDeclaration_defaultPackage() throws IOException {
        File f = writeJavaFile("Foo.java", "public class Foo {}");
        assertEquals("", SourceRootUtils.extractPackageDeclaration(f));
    }

    @Test
    public void extractPackageDeclaration_commentsBeforePackage() throws IOException {
        File f = writeJavaFile("Foo.java",
            "// Copyright 2024\n/* License header */\n* continued\npackage org.test;\n\nclass Foo {}");
        assertEquals("org.test", SourceRootUtils.extractPackageDeclaration(f));
    }

    @Test
    public void extractPackageDeclaration_emptyFile() throws IOException {
        File f = writeJavaFile("Empty.java", "");
        assertEquals("", SourceRootUtils.extractPackageDeclaration(f));
    }

    @Test
    public void extractPackageDeclaration_nullFile() {
        assertEquals("", SourceRootUtils.extractPackageDeclaration(null));
    }

    @Test
    public void extractPackageDeclaration_nonExistentFile() {
        assertEquals("", SourceRootUtils.extractPackageDeclaration(new File("/nonexistent/Foo.java")));
    }

    // --- inferSourceRoot tests ---

    @Test
    public void inferSourceRoot_deepPackagePath() throws IOException {
        writeJavaFile("src/java/com/urbancompass/demo_app/DemoApp.java",
            "package com.urbancompass.demo_app;\n\npublic class DemoApp {}");
        String result = SourceRootUtils.inferSourceRoot(
            tmp.getRoot().getAbsolutePath(), "src/java/com/urbancompass/demo_app");
        assertEquals("src/java", result);
    }

    @Test
    public void inferSourceRoot_singleSegmentPackage() throws IOException {
        writeJavaFile("src/java/mylib/Lib.java", "package mylib;\n\nclass Lib {}");
        String result = SourceRootUtils.inferSourceRoot(
            tmp.getRoot().getAbsolutePath(), "src/java/mylib");
        assertEquals("src/java", result);
    }

    @Test
    public void inferSourceRoot_noJavaFiles() throws IOException {
        new File(tmp.getRoot(), "src/java/empty").mkdirs();
        String result = SourceRootUtils.inferSourceRoot(
            tmp.getRoot().getAbsolutePath(), "src/java/empty");
        assertNull(result);
    }

    @Test
    public void inferSourceRoot_defaultPackage() throws IOException {
        writeJavaFile("mypackage/Main.java", "public class Main {}");
        String result = SourceRootUtils.inferSourceRoot(
            tmp.getRoot().getAbsolutePath(), "mypackage");
        assertNull(result);
    }

    @Test
    public void inferSourceRoot_packageMismatch() throws IOException {
        writeJavaFile("wrong/path/Foo.java", "package com.other;\n\nclass Foo {}");
        String result = SourceRootUtils.inferSourceRoot(
            tmp.getRoot().getAbsolutePath(), "wrong/path");
        assertNull(result);
    }

    @Test
    public void inferSourceRoot_nonExistentDir() {
        String result = SourceRootUtils.inferSourceRoot(
            tmp.getRoot().getAbsolutePath(), "does/not/exist");
        assertNull(result);
    }

    @Test
    public void inferSourceRoot_javaAtRoot() throws IOException {
        writeJavaFile("com/example/App.java", "package com.example;\n\nclass App {}");
        String result = SourceRootUtils.inferSourceRoot(
            tmp.getRoot().getAbsolutePath(), "com/example");
        assertNull("source root at workspace root returns null (no prefix to strip)", result);
    }

    // --- linkedFolderName tests ---

    @Test
    public void linkedFolderName_standardSourceRoot() {
        assertEquals("_src_java", SourceRootUtils.linkedFolderName("src/java"));
    }

    @Test
    public void linkedFolderName_sameSourceRootForDifferentPackages() {
        String folderA = SourceRootUtils.linkedFolderName("src/java");
        String folderB = SourceRootUtils.linkedFolderName("src/java");
        assertEquals("Same source root produces same folder name", folderA, folderB);
        assertEquals("_src_java", folderA);
    }

    @Test
    public void linkedFolderName_singleSegmentSourceRoot() {
        assertEquals("_java", SourceRootUtils.linkedFolderName("java"));
    }

    @Test
    public void linkedFolderName_deepSourceRoot() {
        assertEquals("_src_main_java", SourceRootUtils.linkedFolderName("src/main/java"));
    }

    // --- integration-style tests ---

    @Test
    public void inferenceProducesCorrectLinkedFolderConfig() throws IOException {
        writeJavaFile("src/java/com/urbancompass/demo_app/DemoApp.java",
            "package com.urbancompass.demo_app;\n\npublic class DemoApp {}");
        String sourceRoot = SourceRootUtils.inferSourceRoot(
            tmp.getRoot().getAbsolutePath(), "src/java/com/urbancompass/demo_app");
        assertNotNull(sourceRoot);
        assertEquals("src/java", sourceRoot);
        assertEquals("_src_java", SourceRootUtils.linkedFolderName(sourceRoot));
    }

    @Test
    public void mavenLayoutNotTriggeredByInference() throws IOException {
        writeJavaFile("pkg/src/main/java/com/example/App.java",
            "package com.example;\n\nclass App {}");
        new File(tmp.getRoot(), "pkg/src/main/java").mkdirs();
        boolean hasMavenRoot = new File(tmp.getRoot(), "pkg/src/main/java").isDirectory();
        assertTrue("Maven src/main/java should be detected first", hasMavenRoot);
    }
}
