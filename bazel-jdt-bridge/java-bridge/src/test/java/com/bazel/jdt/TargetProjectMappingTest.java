package com.bazel.jdt;

import static org.junit.Assert.*;

import org.junit.Test;

public class TargetProjectMappingTest {

    @Test
    public void workspaceConfigKeysAreDistinct() {
        assertNotEquals(TargetProjectMapping.KEY_WORKSPACE_PATH, TargetProjectMapping.KEY_BAZEL_PATH);
        assertNotEquals(TargetProjectMapping.KEY_WORKSPACE_PATH, TargetProjectMapping.KEY_CACHE_DIR);
        assertNotEquals(TargetProjectMapping.KEY_BAZEL_PATH, TargetProjectMapping.KEY_CACHE_DIR);
    }

    @Test
    public void configKeysDifferFromTargetKey() {
        assertNotEquals(TargetProjectMapping.KEY, TargetProjectMapping.KEY_WORKSPACE_PATH);
        assertNotEquals(TargetProjectMapping.KEY, TargetProjectMapping.KEY_BAZEL_PATH);
        assertNotEquals(TargetProjectMapping.KEY, TargetProjectMapping.KEY_CACHE_DIR);
    }

    @Test
    public void qualifierIsConsistent() {
        assertEquals("com.bazel.jdt", TargetProjectMapping.QUALIFIER);
    }
}
