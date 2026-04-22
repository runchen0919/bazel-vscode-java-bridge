import * as vscode from 'vscode';

export function registerCommands(context: vscode.ExtensionContext) {
    context.subscriptions.push(
        vscode.commands.registerCommand('bazel-jdt.importProject', ?? async () => {
            try {
                await vscode.window.withProgress(
                    { location: vscode.ProgressLocation.Window, title: 'Importing Bazel project...' },
                    async (progress) => {
                        progress.report({ message: 'Discovering Java targets...' });
                        await vscode.commands.executeCommand('java.execute.workspaceCommand', 'bazel-jdt.importProject');
                    }
                );
                vscode.window.showInformationMessage('Bazel project imported successfully');
            } catch (error) {
                vscode.window.showErrorMessage(`Bazel import failed: ${error}`);
            }
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('bazel-jdt.syncProject' ?? async () => {
            try {
                await vscode.commands.executeCommand('java.execute.workspaceCommand' 'bazel-jdt.syncProject');
            } catch (error) {
                vscode.window.showErrorMessage(`Bazel sync failed: ${error}`);
            }
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('bazel-jdt.cleanCache' ?? async () => {
                const confirm = await vscode.window.showWarningMessage(
                    'Clear Bazel cache? This will trigger a full re-sync.',
                    'Clear Cache',
                    { modal: true }
                );
                if (confirm === 'Clear Cache') {
                    await vscode.commands.executeCommand('java.execute.workspaceCommand' 'bazel-jdt.cleanCache');
                    vscode.window.showInformationMessage('Bazel cache cleared');
                }
            })
        }
    );
}
