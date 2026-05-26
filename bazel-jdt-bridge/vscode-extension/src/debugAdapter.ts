import * as vscode from 'vscode';

export class BazelDebugConfigurationProvider implements vscode.DebugConfigurationProvider {
    async resolveDebugConfiguration(
        _folder: vscode.WorkspaceFolder | undefined,
        config: vscode.DebugConfiguration,
        _token?: vscode.CancellationToken
    ): Promise<vscode.DebugConfiguration | undefined> {
        const projectName = config.projectName as string | undefined;
        if (projectName) {
            try {
                await vscode.commands.executeCommand(
                    'java.execute.workspaceCommand',
                    'bazel-jdt.setActiveDebugProject',
                    projectName
                );
            } catch {
                // Best-effort: filter not set, debug may be slower but still works
            }
        }
        return config;
    }

    async resolveDebugConfigurationWithSubstitutedVariables(
        _folder: vscode.WorkspaceFolder | undefined,
        config: vscode.DebugConfiguration,
        token?: vscode.CancellationToken
    ): Promise<vscode.DebugConfiguration | undefined> {
        const projectName = config.projectName as string | undefined;
        if (!projectName) {
            return config;
        }

        if (token?.isCancellationRequested) {
            return undefined;
        }

        try {
            await vscode.window.withProgress(
                {
                    location: vscode.ProgressLocation.Notification,
                    title: `Bazel: building ${projectName}...`,
                    cancellable: true
                },
                (_progress, progressToken) => {
                    const buildPromise = vscode.commands.executeCommand(
                        'java.execute.workspaceCommand',
                        'bazel-jdt.buildTarget',
                        projectName
                    );
                    return new Promise<void>((resolve, reject) => {
                        progressToken.onCancellationRequested(() =>
                            reject(new Error('Build cancelled by user')));
                        token?.onCancellationRequested(() =>
                            reject(new Error('Debug launch cancelled')));
                        buildPromise.then(() => resolve(), reject);
                    });
                }
            );
        } catch (err) {
            const msg = err instanceof Error ? err.message : String(err);
            const action = await vscode.window.showWarningMessage(
                `Bazel build failed: ${msg}`,
                'Debug Anyway', 'Cancel'
            );
            if (action !== 'Debug Anyway') {
                return undefined;
            }
        }

        // The test runner pre-resolves classPaths before this handler runs,
        // so newly-built JARs (e.g. the test's own output) are missing.
        // Re-resolve from the updated container and merge new entries.
        // We merge rather than replace to keep test-runner JARs (RemoteTestRunner etc.).
        if (config.mainClass && Array.isArray(config.classPaths)) {
            try {
                const resolved = await vscode.commands.executeCommand<[string[], string[]]>(
                    'java.execute.workspaceCommand',
                    'vscode.java.resolveClasspath', config.mainClass, config.projectName);
                if (resolved && Array.isArray(resolved[1])) {
                    const existing = new Set(config.classPaths as string[]);
                    for (const entry of resolved[1]) {
                        if (!existing.has(entry)) {
                            (config.classPaths as string[]).push(entry);
                        }
                    }
                }
            } catch {
                // Best-effort: fall back to the pre-resolved classpath
            }
        }

        return config;
    }
}
