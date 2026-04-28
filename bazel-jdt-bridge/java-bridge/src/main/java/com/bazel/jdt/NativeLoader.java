package com.bazel.jdt;

import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;

/**
 * Cross-platform native library loader for bazel_jdt_core.
 * Supports: linux-x86_64, linux-aarch64, macos-x86_64, macos-aarch64, windows-x86_64
 */
public final class NativeLoader {
    private NativeLoader() {}

    private static final String LIB_NAME = "bazel_jdt_core";

    /**
     * Loads the native library for the current platform.
     * First attempts to load from bundled JAR resources, falls back to system library.
     */
    public static void load() {
        String platform = detectPlatform();
        String libFileName = getLibraryFileName(platform);
        String resourcePath = "/native/" + platform + "/" + libFileName;

        try (InputStream is = NativeLoader.class.getResourceAsStream(resourcePath)) {
            if (is == null) {
                System.loadLibrary(LIB_NAME);
                return;
            }
            Path tempDir = Files.createTempDirectory("bazel-jdt-native");
            tempDir.toFile().deleteOnExit();
            Path tempLib = tempDir.resolve(libFileName);
            Files.copy(is, tempLib, StandardCopyOption.REPLACE_EXISTING);
            tempLib.toFile().deleteOnExit();
            System.load(tempLib.toString());
        } catch (IOException e) {
            throw new RuntimeException("Failed to load native library: " + e.getMessage(), e);
        }
    }

    static String detectPlatform() {
        String os = System.getProperty("os.name").toLowerCase();
        String arch = System.getProperty("os.arch").toLowerCase();
        return detectOs(os) + "-" + detectArch(arch);
    }

    private static String detectOs(String os) {
        if (os.contains("linux")) return "linux";
        if (os.contains("mac") || os.contains("darwin")) return "darwin";
        if (os.contains("win")) return "windows";
        throw new UnsupportedOperationException("Unsupported OS: " + os);
    }

    private static String detectArch(String arch) {
        // x86_64 is reported as x86_64 (Linux/macOS), amd64 (Windows), or x64 (some JVMs)
        if (arch.contains("x86_64") || arch.contains("amd64") || arch.contains("x64")) {
            return "x86_64";
        }
        // aarch64 is reported as aarch64 (Linux), arm64 (macOS), or arm (older systems)
        if (arch.contains("aarch64") || arch.contains("arm64") || arch.contains("arm")) {
            return "aarch64";
        }
        throw new UnsupportedOperationException("Unsupported architecture: " + arch);
    }

    static String getLibraryFileName(String platform) {
        if (platform.startsWith("linux")) return "lib" + LIB_NAME + ".so";
        if (platform.startsWith("darwin")) return "lib" + LIB_NAME + ".dylib";
        if (platform.startsWith("windows")) return LIB_NAME + ".dll";
        throw new UnsupportedOperationException("Unknown platform: " + platform);
    }
}
