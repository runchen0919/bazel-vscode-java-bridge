import * as vscode from 'vscode';

export class BazelDebugConfigurationProvider implements vscode.DebugConfigurationProvider {
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

        return config;
    }
}
