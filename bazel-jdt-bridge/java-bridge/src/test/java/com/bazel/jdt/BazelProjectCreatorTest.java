package com.bazel.jdt;

import static org.junit.Assert.*;

import org.eclipse.jdt.core.IClasspathAttribute;
import org.eclipse.jdt.core.IClasspathEntry;
import org.eclipse.core.runtime.Path;
import org.junit.Test;

public class BazelProjectCreatorTest {

    @Test
    public void newSourceEntryWithTestProjectHasTestAttribute() {
        IClasspathEntry entry = BazelProjectCreator.newSourceEntry(
            new Path("/myproject/src"), true);
        assertEquals(IClasspathEntry.CPE_SOURCE, entry.getEntryKind());
        boolean hasTest = false;
        for (IClasspathAttribute attr : entry.getExtraAttributes()) {
            if (IClasspathAttribute.TEST.equals(attr.getName())
                    && "true".equals(attr.getValue())) {
                hasTest = true;
            }
        }
        assertTrue("Source entry for test project should have TEST attribute", hasTest);
    }

    @Test
    public void newSourceEntryWithNonTestProjectHasNoTestAttribute() {
        IClasspathEntry entry = BazelProjectCreator.newSourceEntry(
            new Path("/myproject/src"), false);
        assertEquals(IClasspathEntry.CPE_SOURCE, entry.getEntryKind());
        for (IClasspathAttribute attr : entry.getExtraAttributes()) {
            assertNotEquals("Source entry for non-test project should not have TEST attribute",
                IClasspathAttribute.TEST, attr.getName());
        }
    }
}
