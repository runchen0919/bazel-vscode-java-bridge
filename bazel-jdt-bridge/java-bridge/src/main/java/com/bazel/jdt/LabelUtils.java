package com.bazel.jdt;

public final class LabelUtils {

    private static final java.util.logging.Logger LOG =
        java.util.logging.Logger.getLogger(LabelUtils.class.getName());

    private LabelUtils() {}

    public static String extractPackageName(String targetLabel) {
        if (targetLabel == null || !targetLabel.startsWith("//")) {
            LOG.warning("Invalid Bazel label (missing '//' prefix): " + targetLabel);
            return "";
        }
        int colonIndex = targetLabel.lastIndexOf(':');
        if (colonIndex >= 2) {
            return targetLabel.substring(2, colonIndex);
        }
        return targetLabel.substring(2);
    }

    public static String toProjectName(String packagePath) {
        return packagePath.replace('/', '.');
    }

    public static String fromProjectName(String projectName) {
        return projectName.replace('.', '/');
    }

}
