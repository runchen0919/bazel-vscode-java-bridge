package com.bazel.jdt;

import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;

import org.eclipse.core.runtime.ILog;
import org.eclipse.core.runtime.IStatus;
import org.eclipse.core.runtime.Platform;
import org.eclipse.core.runtime.Status;

/**
 * Cross-platform native library loader for bazel_jdt_core.
 * Supports: linux-x86_64, linux-aarch64, macos-x86_64, macos-aarch64, windows-x86_64
 */
public final class NativeLoader {
    private NativeLoader() {}

    private static final ILog LOG = Platform.getLog(NativeLoader.class);
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
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                "Failed to load native library: " + e.getMessage(), e));
            throw new RuntimeException("Failed to load native library: " + e.getMessage(), e);
        } catch (UnsatisfiedLinkError e) {
            LOG.log(new Status(IStatus.ERROR, "com.bazel.jdt",
                "Native library not found for platform '" + platform + "': " + e.getMessage(), e));
            throw new RuntimeException("Native library not found for platform '" + platform + "'", e);
        }
    }

    static String detectPlatform() {
        return PlatformDetector.detectPlatform();
    }

    static String getLibraryFileName(String platform) {
        return PlatformDetector.getLibraryFileName(platform);
    }
}
