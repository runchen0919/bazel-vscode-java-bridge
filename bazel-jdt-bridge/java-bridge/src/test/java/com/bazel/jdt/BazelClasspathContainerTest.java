package com.bazel.jdt;

import static org.junit.Assert.*;

import org.eclipse.jdt.core.IClasspathAttribute;
import org.eclipse.jdt.core.IClasspathContainer;
import org.eclipse.jdt.core.IClasspathEntry;
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

    @Test
    public void srcEntryWithIsTestTrueHasTestAttribute() {
        BazelClasspathContainer c = new BazelClasspathContainer(
            new String[]{"SRC|/myproject/src||true"});
        IClasspathEntry[] entries = c.getClasspathEntries();
        assertEquals(1, entries.length);
        IClasspathEntry entry = entries[0];
        assertEquals(IClasspathEntry.CPE_SOURCE, entry.getEntryKind());
        boolean hasTest = false;
        for (IClasspathAttribute attr : entry.getExtraAttributes()) {
            if (IClasspathAttribute.TEST.equals(attr.getName())
                    && "true".equals(attr.getValue())) {
                hasTest = true;
            }
        }
        assertTrue("SRC entry with isTest=true should have TEST attribute", hasTest);
    }

    @Test
    public void srcEntryWithIsTestFalseHasNoTestAttribute() {
        BazelClasspathContainer c = new BazelClasspathContainer(
            new String[]{"SRC|/myproject/src||false"});
        IClasspathEntry[] entries = c.getClasspathEntries();
        assertEquals(1, entries.length);
        for (IClasspathAttribute attr : entries[0].getExtraAttributes()) {
            assertNotEquals("SRC entry with isTest=false should not have TEST attribute",
                IClasspathAttribute.TEST, attr.getName());
        }
    }

    @Test
    public void srcEntryWithMissingIsTestFieldHasNoTestAttribute() {
        BazelClasspathContainer c = new BazelClasspathContainer(
            new String[]{"SRC|/myproject/src"});
        IClasspathEntry[] entries = c.getClasspathEntries();
        assertEquals(1, entries.length);
        for (IClasspathAttribute attr : entries[0].getExtraAttributes()) {
            assertNotEquals("SRC entry with no isTest field should not have TEST attribute",
                IClasspathAttribute.TEST, attr.getName());
        }
    }
}
