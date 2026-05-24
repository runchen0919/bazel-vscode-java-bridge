package com.bazel.jdt;

import static org.junit.Assert.*;

import org.junit.Test;

public class BazelSourceLookupFixTest {

    @Test
    public void testNullInput() {
        assertNull(BazelSourceLookupFix.deduplicateContainers(null));
    }

    @Test
    public void testIsJdkContainerJrtFs() {
        assertTrue(BazelSourceLookupFix.isJdkContainer(null,
            "/usr/lib/jvm/java-21-openjdk-amd64/lib/jrt-fs.jar"));
    }

    @Test
    public void testIsJdkContainerRtJar() {
        assertTrue(BazelSourceLookupFix.isJdkContainer(null,
            "/usr/lib/jvm/java-8-openjdk-amd64/jre/lib/rt.jar"));
    }

    @Test
    public void testIsJdkContainerJreLib() {
        assertTrue(BazelSourceLookupFix.isJdkContainer(null,
            "/usr/lib/jvm/java-8-openjdk-amd64/jre/lib/charsets.jar"));
    }

    @Test
    public void testIsJdkContainerMacOsClasses() {
        assertTrue(BazelSourceLookupFix.isJdkContainer(null,
            "/System/Library/Frameworks/Classes/classes.jar"));
    }

    @Test
    public void testIsNotJdkContainerGuava() {
        assertFalse(BazelSourceLookupFix.isJdkContainer(null,
            "/home/user/.cache/bazel/external/maven/processed_guava-33.4.0-jre.jar"));
    }

    @Test
    public void testIsNotJdkContainerJUnit() {
        assertFalse(BazelSourceLookupFix.isJdkContainer(null,
            "/home/user/.m2/repository/junit/junit/4.13.2/junit-4.13.2.jar"));
    }

    @Test
    public void testIsNotJdkContainerProjectJar() {
        assertFalse(BazelSourceLookupFix.isJdkContainer(null,
            "/home/user/workspace/bazel-out/bin/utils/string_utils.jar"));
    }

    @Test
    public void testResolveSourceFileURINullFqnReturnsOriginal() {
        String original = "jdt://contents/something";
        assertEquals(original,
            BazelSourceLookupFix.resolveSourceFileURI(null, "path", original));
    }

    @Test
    public void testResolveSourceFileURIEmptyFqnReturnsOriginal() {
        String original = "jdt://contents/something";
        assertEquals(original,
            BazelSourceLookupFix.resolveSourceFileURI("", "path", original));
    }

    @Test
    public void testResolveSourceFileURINoWorkspaceFallsBackToOriginal() {
        String original = "jdt://contents/test.jar/pkg/Foo.class?=project/...";
        String result = BazelSourceLookupFix.resolveSourceFileURI(
            "com.example.Nonexistent", "com/example/Nonexistent.java", original);
        assertEquals(original, result);
    }
}
