package com.bazel.jdt;

import java.util.Arrays;
import java.util.List;

import org.eclipse.core.resources.IProject;
import org.eclipse.core.resources.IResource;
import org.eclipse.core.runtime.IProgressMonitor;
import org.eclipse.jdt.ls.core.internal.managers.IBuildSupport;
import org.eclipse.jdt.ls.core.internal.managers.ProjectsManager.CHANGE_TYPE;

public class BazelBuildSupport implements IBuildSupport {
    private static final List<String> WATCH_PATTERNS = Arrays.asList(
        "**/BUILD",
        "**/BUILD.bazel",
        "**/WORKSPACE",
        "**/WORKSPACE.bazel",
        "**/.bazelproject"
    );

    @Override
    public boolean applies(IProject project) {
        try {
            return project.hasNature("com.bazel.jdt.bazelNature");
        } catch (Exception e) {
            return false;
        }
    }

    @Override
    public boolean isBuildFile(IResource resource) {
        String name = resource.getName();
        return "BUILD".equals(name)
                || "BUILD.bazel".equals(name)
                || "WORKSPACE".equals(name)
                || "WORKSPACE.bazel".equals(name);
    }

    @Override
    public List<String> getWatchPatterns() {
        return WATCH_PATTERNS;
    }

    @Override
    public boolean fileChanged(IResource resource, CHANGE_TYPE changeType, IProgressMonitor monitor) {
        if (!isBuildFile(resource)) {
            return false;
        }
        String filePath = resource.getLocation().toOSString();
        BazelClasspathManager.refreshClasspathForFiles(Arrays.asList(filePath));
        return true;
    }
}
