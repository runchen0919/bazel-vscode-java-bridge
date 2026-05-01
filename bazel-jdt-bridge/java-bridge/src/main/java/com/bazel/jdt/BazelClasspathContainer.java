package com.bazel.jdt;

import java.util.ArrayList;
import java.util.List;

import org.eclipse.core.runtime.IPath;
import org.eclipse.core.runtime.Path;
import org.eclipse.jdt.core.IClasspathContainer;
import org.eclipse.jdt.core.IClasspathEntry;
import org.eclipse.jdt.core.JavaCore;

public class BazelClasspathContainer implements IClasspathContainer {
    public static final IPath CONTAINER_PATH = Path.fromPortableString("com.bazel.jdt.BAZEL_CONTAINER");
    private static final String DESCRIPTION = "Bazel Dependencies";

    public static final BazelClasspathContainer EMPTY = new BazelClasspathContainer((String[]) null);

    private final IClasspathEntry[] entries;

    public BazelClasspathContainer(String[] rawEntries) {
        List<IClasspathEntry> parsed = new ArrayList<>();
        if (rawEntries == null) {
            this.entries = parsed.toArray(new IClasspathEntry[0]);
            return;
        }
        for (String raw : rawEntries) {
            IClasspathEntry entry = parseEntry(raw);
            if (entry != null) {
                parsed.add(entry);
            }
        }
        this.entries = parsed.toArray(new IClasspathEntry[0]);
    }

    private IClasspathEntry parseEntry(String raw) {
        String[] parts = raw.split("\\|", -1);
        if (parts.length < 2) return null;
        String type = parts[0];
        String path = parts[1];
        String sourcePath = parts.length > 2 && !parts[2].isEmpty() ? parts[2] : null;
        boolean isTest = parts.length > 3 && Boolean.parseBoolean(parts[3]);
        switch (type) {
            case "LIB":
                IPath jarPath = Path.fromPortableString(path);
                IPath srcPath = sourcePath != null ? Path.fromPortableString(sourcePath) : null;
                return JavaCore.newLibraryEntry(jarPath, srcPath, null);
            case "PROJ":
                if (path.startsWith("@@")) {
                    return null;
                }
                String projectName = extractPackageName(path);
                return JavaCore.newProjectEntry(Path.fromPortableString("/" + projectName));
            case "SRC":
                return JavaCore.newSourceEntry(Path.fromPortableString(path));
            default:
                return null;
        }
    }

    private static String extractPackageName(String targetLabel) {
        return LabelUtils.extractPackageName(targetLabel);
    }

    @Override
    public IClasspathEntry[] getClasspathEntries() {
        return entries;
    }

    @Override
    public String getDescription() {
        return DESCRIPTION;
    }

    @Override
    public int getKind() {
        return K_APPLICATION;
    }

    @Override
    public IPath getPath() {
        return CONTAINER_PATH;
    }
}
