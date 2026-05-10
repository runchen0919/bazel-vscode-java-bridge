package com.bazel.jdt;

import static org.junit.Assert.*;

import org.eclipse.jdt.core.IClasspathContainer;
import org.junit.Test;

public class BazelClasspathContainerTest {

    @Test
    public void nullInputProducesEmptyEntries() {
        BazelClasspathContainer c = new BazelClasspathContainer(null);
        assertEquals(0, c.getClasspathEntries().length);
    }

    @Test
    public void emptyArrayProducesEmptyEntries() {
        BazelClasspathContainer c = new BazelClasspathContainer(new String[0]);
        assertEquals(0, c.getClasspathEntries().length);
    }

    @Test
    public void getDescriptionReturnsBazelDependencies() {
        BazelClasspathContainer c = new BazelClasspathContainer(new String[0]);
        assertEquals("Bazel Dependencies", c.getDescription());
    }

    @Test
    public void getKindReturnsKApplication() {
        BazelClasspathContainer c = new BazelClasspathContainer(new String[0]);
        assertEquals(IClasspathContainer.K_APPLICATION, c.getKind());
    }

    @Test
    public void getPathReturnsContainerPath() {
        BazelClasspathContainer c = new BazelClasspathContainer(new String[0]);
        assertSame(BazelClasspathContainer.CONTAINER_PATH, c.getPath());
    }
}
