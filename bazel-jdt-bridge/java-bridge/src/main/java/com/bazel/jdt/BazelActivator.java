package com.bazel.jdt;

import java.util.ArrayList;
import java.util.List;
import java.util.regex.Pattern;

import org.eclipse.core.resources.IProject;
import org.eclipse.core.resources.IResourceChangeEvent;
import org.eclipse.core.resources.IResourceChangeListener;
import org.eclipse.core.resources.IResourceDelta;
import org.eclipse.core.resources.IWorkspaceRoot;
import org.eclipse.core.resources.ResourcesPlugin;
import org.eclipse.core.resources.WorkspaceJob;
import org.eclipse.core.runtime.CoreException;
import org.eclipse.core.runtime.ILog;
import org.eclipse.core.runtime.IProgressMonitor;
import org.eclipse.core.runtime.IStatus;
import org.eclipse.core.runtime.Platform;
import org.eclipse.core.runtime.Status;
import org.osgi.framework.BundleActivator;
import org.osgi.framework.BundleContext;

public class BazelActivator implements BundleActivator {
    private static final ILog LOG = Platform.getLog(BazelActivator.class);
    private static final Pattern INVISIBLE_PROJECT_PATTERN =
        Pattern.compile(".+_[0-9a-f]{4,}$");

    private IResourceChangeListener invisibleProjectListener;

    @Override
    public void start(BundleContext context) throws Exception {
        LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
            "Bazel JDT Bridge bundle starting"));

        invisibleProjectListener = this::checkForInvisibleProjects;
        ResourcesPlugin.getWorkspace().addResourceChangeListener(
            invisibleProjectListener, IResourceChangeEvent.POST_CHANGE);
    }

    @Override
    public void stop(BundleContext context) throws Exception {
        if (invisibleProjectListener != null) {
            ResourcesPlugin.getWorkspace().removeResourceChangeListener(invisibleProjectListener);
            invisibleProjectListener = null;
        }
        BazelBridge.getInstance().shutdown();
        LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
            "Bazel JDT Bridge bundle stopped (classpath recovery uses disk cache on next restart)"));
    }

    private void checkForInvisibleProjects(IResourceChangeEvent event) {
        IResourceDelta delta = event.getDelta();
        if (delta == null) return;

        List<String> candidates = new ArrayList<>();
        try {
            delta.accept(d -> {
                if (d.getResource() instanceof IWorkspaceRoot) return true;
                if (d.getResource() instanceof IProject project
                        && d.getKind() == IResourceDelta.ADDED
                        && INVISIBLE_PROJECT_PATTERN.matcher(project.getName()).matches()) {
                    candidates.add(project.getName());
                }
                return false;
            });
        } catch (CoreException e) {
            LOG.log(new Status(IStatus.WARNING, "com.bazel.jdt",
                "Error scanning resource delta: " + e.getMessage()));
        }

        if (!candidates.isEmpty()) {
            scheduleInvisibleProjectCleanup(candidates);
        }
    }

    private void scheduleInvisibleProjectCleanup(List<String> candidates) {
        WorkspaceJob job = new WorkspaceJob("Remove JDT.LS invisible project") {
            @Override
            public IStatus runInWorkspace(IProgressMonitor monitor) throws CoreException {
                IWorkspaceRoot root = ResourcesPlugin.getWorkspace().getRoot();

                boolean hasBazelProjects = false;
                for (IProject p : root.getProjects()) {
                    if (p.isAccessible()) {
                        try {
                            if (p.hasNature(BazelNature.NATURE_ID)) {
                                hasBazelProjects = true;
                                break;
                            }
                        } catch (CoreException e) {
                            continue;
                        }
                    }
                }
                if (!hasBazelProjects) return Status.OK_STATUS;

                for (String name : candidates) {
                    IProject project = root.getProject(name);
                    if (!project.exists() || !project.isAccessible()) continue;
                    try {
                        if (project.hasNature(BazelNature.NATURE_ID)) continue;
                    } catch (CoreException e) {
                        continue;
                    }
                    LOG.log(new Status(IStatus.INFO, "com.bazel.jdt",
                        "Deleting invisible project created by JDT.LS: " + name));
                    project.delete(false, true, monitor);
                }
                return Status.OK_STATUS;
            }
        };
        job.setRule(ResourcesPlugin.getWorkspace().getRoot());
        job.schedule(3000);
    }
}
