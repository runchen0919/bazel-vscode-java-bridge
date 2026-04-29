package com.bazel.jdt;

import org.eclipse.core.resources.IProject;
import org.eclipse.core.resources.IProjectNature;
import org.eclipse.core.runtime.CoreException;

/**
 * Eclipse project nature for Bazel-managed Java projects.
 * Marks a project as managed by the Bazel build system so that
 * {@link BazelBuildSupport} and {@link BazelClasspathManager} can identify it.
 */
public class BazelNature implements IProjectNature {

    /** Fully qualified nature ID as registered in plugin.xml. */
    public static final String NATURE_ID = "com.bazel.jdt.bazelNature";

    private IProject project;

    @Override
    public void setProject(IProject project) {
        this.project = project;
    }

    @Override
    public IProject getProject() {
        return project;
    }

    @Override
    public void configure() throws CoreException {
        // No-op: classpath setup is handled by BazelClasspathManager
    }

    @Override
    public void deconfigure() throws CoreException {
        // No-op: cleanup handled by bundle stop()
    }
}
