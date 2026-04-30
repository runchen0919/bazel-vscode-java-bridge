package com.bazel.jdt;

import static org.junit.Assert.*;

import org.junit.Test;

public class BazelProjectImporterTest {

    @Test
    public void extractPackageNameSimple() {
        assertEquals("app", LabelUtils.extractPackageName("//app:app"));
    }

    @Test
    public void extractPackageNameNested() {
        assertEquals("foo/bar", LabelUtils.extractPackageName("//foo/bar:baz"));
    }

    @Test
    public void extractPackageNameNoColon() {
        assertEquals("single", LabelUtils.extractPackageName("//single"));
    }

    @Test
    public void extractPackageNameRootPackage() {
        assertEquals(":root", LabelUtils.extractPackageName("//:root"));
    }

    @Test
    public void extractPackageNameMultipleColons() {
        assertEquals("a:b", LabelUtils.extractPackageName("//a:b:c"));
    }

    @Test
    public void bazelNatureIdConstant() {
        assertEquals("com.bazel.jdt.bazelNature", BazelNature.NATURE_ID);
    }
}
