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

public class JdkSourceLookupPatcher implements WeavingHook, Opcodes {

    private static final Logger LOG = Logger.getLogger(JdkSourceLookupPatcher.class.getName());

    private static final String TARGET_BUNDLE = "com.microsoft.java.debug.plugin";
    private static final String TARGET_CLASS =
        "com/microsoft/java/debug/plugin/internal/JdtSourceLookUpProvider";

    private static final String TARGET_METHOD = "getSourceFileURI";
    private static final String TARGET_DESC =
        "(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;";

    private static final String FIX_INTERNAL = "com/bazel/jdt/BazelSourceLookupFix";
    private static final String FIX_METHOD = "resolveSourceFileURI";
    private static final String FIX_DESC =
        "(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;";

    @Override
    public void weave(WovenClass wovenClass) {
        if (!TARGET_BUNDLE.equals(wovenClass.getBundleWiring().getBundle().getSymbolicName())) {
            return;
        }

        String className = wovenClass.getClassName().replace('.', '/');
        if (!TARGET_CLASS.equals(className)) {
            return;
        }

        byte[] original = wovenClass.getBytes();
        try {
            byte[] patched = patchGetSourceFileURI(original);
            if (patched != null) {
                wovenClass.setBytes(patched);
                wovenClass.getDynamicImports().add("com.bazel.jdt");
                LOG.info("Patched JdtSourceLookUpProvider.getSourceFileURI: injected JDK source lookup fallback");
            } else {
                LOG.warning("JdtSourceLookUpProvider found but getSourceFileURI not matched - skipping patch");
            }
        } catch (Exception e) {
            LOG.log(Level.WARNING,
                "Failed to patch JdtSourceLookUpProvider, leaving class unmodified", e);
        }
    }

    byte[] patchGetSourceFileURI(byte[] classBytes) {
        ClassReader reader = new ClassReader(classBytes);
        ClassWriter writer = new SafeClassWriter(reader, ClassWriter.COMPUTE_FRAMES | ClassWriter.COMPUTE_MAXS);
        FallbackInjector[] injectorHolder = {null};

        ClassVisitor visitor = new ClassVisitor(ASM9, writer) {
            @Override
            public MethodVisitor visitMethod(int access, String name, String descriptor,
                                              String signature, String[] exceptions) {
                MethodVisitor mv = super.visitMethod(access, name, descriptor, signature, exceptions);
                if (TARGET_METHOD.equals(name) && TARGET_DESC.equals(descriptor)) {
                    FallbackInjector injector = new FallbackInjector(mv);
                    injectorHolder[0] = injector;
                    return injector;
                }
                return mv;
            }
        };

        reader.accept(visitor, 0);
        if (injectorHolder[0] != null && injectorHolder[0].getInjectionCount() > 0) {
            LOG.info("Patched JdtSourceLookUpProvider.getSourceFileURI: injected fallback at "
                + injectorHolder[0].getInjectionCount() + " ARETURN points");
            return writer.toByteArray();
        }
        return null;
    }

    static class FallbackInjector extends MethodVisitor {
        private int injectionCount = 0;

        FallbackInjector(MethodVisitor mv) {
            super(ASM9, mv);
        }

        @Override
        public void visitInsn(int opcode) {
            if (opcode == ARETURN) {
                // Stack: [original return value]
                // Slot 0=this, 1=fqn, 2=sourcePath, 3=free for temp
                super.visitVarInsn(ASTORE, 3);
                super.visitVarInsn(ALOAD, 1);
                super.visitVarInsn(ALOAD, 2);
                super.visitVarInsn(ALOAD, 3);
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
