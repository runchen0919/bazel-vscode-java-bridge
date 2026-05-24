package com.bazel.jdt;

import java.util.logging.Level;
import java.util.logging.Logger;

import org.objectweb.asm.ClassReader;
import org.objectweb.asm.ClassVisitor;
import org.objectweb.asm.ClassWriter;
import org.objectweb.asm.MethodVisitor;
import org.objectweb.asm.Opcodes;
import org.osgi.framework.hooks.weaving.WeavingHook;
import org.osgi.framework.hooks.weaving.WovenClass;

public class JavaDebugSourceLookupPatcher implements WeavingHook, Opcodes {

    private static final Logger LOG = Logger.getLogger(JavaDebugSourceLookupPatcher.class.getName());

    private static final String TARGET_BUNDLE = "com.microsoft.java.debug.plugin";
    private static final String JDTUTILS_INTERNAL = "com/microsoft/java/debug/plugin/internal/JdtUtils";

    private static final String TARGET_METHOD = "getSourceContainers";
    private static final String TARGET_DESC =
        "(Lorg/eclipse/jdt/core/IJavaProject;Ljava/util/Set;)[Lorg/eclipse/debug/core/sourcelookup/ISourceContainer;";
    private static final String FIX_INTERNAL = "com/bazel/jdt/BazelSourceLookupFix";
    private static final String FIX_METHOD = "deduplicateContainers";
    private static final String FIX_DESC =
        "([Lorg/eclipse/debug/core/sourcelookup/ISourceContainer;)[Lorg/eclipse/debug/core/sourcelookup/ISourceContainer;";

    @Override
    public void weave(WovenClass wovenClass) {
        if (!TARGET_BUNDLE.equals(wovenClass.getBundleWiring().getBundle().getSymbolicName())) {
            return;
        }

        if (!JDTUTILS_INTERNAL.equals(wovenClass.getClassName().replace('.', '/'))) {
            if (!JDTUTILS_INTERNAL.equals(wovenClass.getClassName())) {
                return;
            }
        }

        byte[] original = wovenClass.getBytes();
        try {
            byte[] patched = patchGetSourceContainers(original);
            if (patched != null) {
                wovenClass.setBytes(patched);
                wovenClass.getDynamicImports().add("com.bazel.jdt");
                LOG.info("Patched JdtUtils.getSourceContainers: injected source container deduplication");
            } else {
                LOG.warning("JdtUtils found but getSourceContainers(IJavaProject,Set) not matched - skipping patch");
            }
        } catch (Exception e) {
            LOG.log(Level.WARNING,
                "Failed to patch JdtUtils, leaving class unmodified", e);
        }
    }

    byte[] patchGetSourceContainers(byte[] classBytes) {
        ClassReader reader = new ClassReader(classBytes);
        ClassWriter writer = new SafeClassWriter(reader, ClassWriter.COMPUTE_FRAMES | ClassWriter.COMPUTE_MAXS);
        DeduplicateInjector[] injectorHolder = {null};

        ClassVisitor visitor = new ClassVisitor(ASM9, writer) {
            @Override
            public MethodVisitor visitMethod(int access, String name, String descriptor,
                                             String signature, String[] exceptions) {
                MethodVisitor mv = super.visitMethod(access, name, descriptor, signature, exceptions);
                if (TARGET_METHOD.equals(name) && TARGET_DESC.equals(descriptor)) {
                    DeduplicateInjector injector = new DeduplicateInjector(mv);
                    injectorHolder[0] = injector;
                    return injector;
                }
                return mv;
            }
        };

        reader.accept(visitor, 0);
        if (injectorHolder[0] != null && injectorHolder[0].getInjectionCount() > 0) {
            LOG.info("Patched JdtUtils.getSourceContainers: injected deduplication at "
                + injectorHolder[0].getInjectionCount() + " ARETURN points");
            return writer.toByteArray();
        }
        return null;
    }

    static class DeduplicateInjector extends MethodVisitor {
        private int injectionCount = 0;

        DeduplicateInjector(MethodVisitor mv) {
            super(ASM9, mv);
        }

        @Override
        public void visitInsn(int opcode) {
            if (opcode == ARETURN) {
                super.visitMethodInsn(
                    INVOKESTATIC,
                    FIX_INTERNAL,
                    FIX_METHOD,
                    FIX_DESC,
                    false
                );
                injectionCount++;
            }
            super.visitInsn(opcode);
        }

        int getInjectionCount() {
            return injectionCount;
        }
    }

    private static class SafeClassWriter extends ClassWriter {
        SafeClassWriter(ClassReader classReader, int flags) {
            super(classReader, flags);
        }

        @Override
        protected String getCommonSuperClass(String type1, String type2) {
            try {
                return super.getCommonSuperClass(type1, type2);
            } catch (Exception e) {
                return "java/lang/Object";
            }
        }
    }
}
