package com.bazel.jdt;

import static org.junit.Assert.*;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

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
        assertEquals("", LabelUtils.extractPackageName("//:root"));
    }

    @Test
    public void extractPackageNameMultipleColons() {
        assertEquals("a:b", LabelUtils.extractPackageName("//a:b:c"));
    }

    @Test
    public void toProjectNameDeepPath() {
        assertEquals("src.java.com.urbancompass.demo_app",
            LabelUtils.toProjectName("src/java/com/urbancompass/demo_app"));
    }

    @Test
    public void toProjectNameSingleSegment() {
        assertEquals("mylib", LabelUtils.toProjectName("mylib"));
    }

    @Test
    public void toProjectNameTwoSegments() {
        assertEquals("src.mylib", LabelUtils.toProjectName("src/mylib"));
    }

    @Test
    public void bazelNatureIdConstant() {
        assertEquals("com.bazel.jdt.bazelNature", BazelNature.NATURE_ID);
    }

    @Test
    public void groupTransitiveDepsByProjectName() {
        String[] deps = {
            "//3rdparty:guava",
            "//3rdparty:protobuf",
            "//3rdparty:jackson",
            "//lib/utils:utils",
            "//lib/auth:auth",
        };

        Map<String, List<String>> projectToLabels = new LinkedHashMap<>();
        for (String depLabel : deps) {
            String projName = LabelUtils.toProjectName(LabelUtils.extractPackageName(depLabel));
            projectToLabels.computeIfAbsent(projName, k -> new ArrayList<>()).add(depLabel);
        }

        assertEquals("3 unique projects", 3, projectToLabels.size());
        assertEquals("3rdparty has 3 targets", 3, projectToLabels.get("3rdparty").size());
        assertEquals("lib.utils has 1 target", 1, projectToLabels.get("lib.utils").size());
        assertEquals("lib.auth has 1 target", 1, projectToLabels.get("lib.auth").size());
    }

    @Test
    public void groupTransitiveDepsSingleTargetPerProject() {
        String[] deps = {
            "//app:app",
            "//lib:lib",
            "//tests:tests",
        };

        Map<String, List<String>> projectToLabels = new LinkedHashMap<>();
        for (String depLabel : deps) {
            String projName = LabelUtils.toProjectName(LabelUtils.extractPackageName(depLabel));
            projectToLabels.computeIfAbsent(projName, k -> new ArrayList<>()).add(depLabel);
        }

        assertEquals("3 unique projects, one target each", 3, projectToLabels.size());
        for (List<String> labels : projectToLabels.values()) {
            assertEquals(1, labels.size());
        }
    }

    @Test
    public void groupTransitiveDepsPreservesOrder() {
        String[] deps = {
            "//3rdparty:guava",
            "//lib:lib",
            "//3rdparty:protobuf",
        };

        Map<String, List<String>> projectToLabels = new LinkedHashMap<>();
        for (String depLabel : deps) {
            String projName = LabelUtils.toProjectName(LabelUtils.extractPackageName(depLabel));
            projectToLabels.computeIfAbsent(projName, k -> new ArrayList<>()).add(depLabel);
        }

        List<String> keys = new ArrayList<>(projectToLabels.keySet());
        assertEquals("3rdparty first (first seen)", "3rdparty", keys.get(0));
        assertEquals("lib second", "lib", keys.get(1));
        assertEquals("3rdparty labels batched", 2, projectToLabels.get("3rdparty").size());
        assertEquals("//3rdparty:guava", projectToLabels.get("3rdparty").get(0));
        assertEquals("//3rdparty:protobuf", projectToLabels.get("3rdparty").get(1));
    }
}
