package com.bazel.jdt;

import static org.junit.Assert.*;

import org.junit.Test;

public class LabelUtilsTest {

    @Test
    public void extractPackageName_validLabel() {
        assertEquals("src/java/com/example", LabelUtils.extractPackageName("//src/java/com/example:lib"));
    }

    @Test
    public void extractPackageName_noTarget() {
        assertEquals("src/java/com/example", LabelUtils.extractPackageName("//src/java/com/example"));
    }

    @Test
    public void extractPackageName_rootPackage() {
        assertEquals("", LabelUtils.extractPackageName("//:root_target"));
    }

    @Test
    public void extractPackageName_bareLabel_returnsEmpty() {
        assertEquals("", LabelUtils.extractPackageName("demo_app"));
    }

    @Test
    public void extractPackageName_colonPrefix_returnsEmpty() {
        assertEquals("", LabelUtils.extractPackageName(":target"));
    }

    @Test
    public void extractPackageName_null_returnsEmpty() {
        assertEquals("", LabelUtils.extractPackageName(null));
    }

    @Test
    public void toProjectName_slashesToDots() {
        assertEquals("src.java.com.urbancompass.demo_app",
            LabelUtils.toProjectName("src/java/com/urbancompass/demo_app"));
    }

    @Test
    public void fromProjectName_dotsToSlashes() {
        assertEquals("src/java/com/urbancompass/demo_app",
            LabelUtils.fromProjectName("src.java.com.urbancompass.demo_app"));
    }

    @Test
    public void fromProjectName_singleSegment() {
        assertEquals("3rdparty", LabelUtils.fromProjectName("3rdparty"));
    }

    @Test
    public void roundTrip_toProjectName_fromProjectName() {
        String original = "src/java/com/urbancompass/demo_app";
        assertEquals(original, LabelUtils.fromProjectName(LabelUtils.toProjectName(original)));
    }

}
