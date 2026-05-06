package com.bazel.jdt;

public final class PlatformDetector {

    private PlatformDetector() {}

    public static String detectPlatform() {
        String os = System.getProperty("os.name").toLowerCase();
        String arch = System.getProperty("os.arch").toLowerCase();
        return detectOs(os) + "-" + detectArch(arch);
    }

    public static String getLibraryFileName(String platform) {
        if (platform.startsWith("linux")) return "libbazel_jdt_core.so";
        if (platform.startsWith("darwin")) return "libbazel_jdt_core.dylib";
        if (platform.startsWith("windows")) return "bazel_jdt_core.dll";
        throw new UnsupportedOperationException("Unknown platform: " + platform);
    }

    private static String detectOs(String os) {
        if (os.contains("linux")) return "linux";
        if (os.contains("mac") || os.contains("darwin")) return "darwin";
        if (os.contains("win")) return "windows";
        throw new UnsupportedOperationException("Unsupported OS: " + os);
    }

    private static String detectArch(String arch) {
        if (arch.contains("x86_64") || arch.contains("amd64") || arch.contains("x64")) {
            return "x86_64";
        }
        if (arch.contains("aarch64") || arch.equals("arm64")) {
            return "aarch64";
        }
        throw new UnsupportedOperationException("Unsupported architecture: " + arch);
    }
}
