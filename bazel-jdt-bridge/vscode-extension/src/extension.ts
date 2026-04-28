import * as vscode from 'vscode';
import * as path from 'path';
import { registerCommands } from './commands';
import { createStatusBar } from './statusBar';
import { getConfig } from './config';

export async function activate(context: vscode.ExtensionContext) {
    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;

    if (!workspaceRoot) {
        return;
    }

    const workspaceFiles = await vscode.workspace.findFiles('{WORKSPACE,WORKSPACE.bazel}');
    if (workspaceFiles.length === 0) {
        return;
    }

    const config = getConfig();

    // Do NOT call bazel-jdt.importProject here.
    // BazelProjectImporter is auto-triggered by JDT.LS during workspace initialization
    // (via org.eclipse.jdt.ls.core.importers extension point in plugin.xml).
    // Calling it again from here causes a race: both paths call bridge.initialize(),
    // and the second call shuts down the state created by the first, producing
    // "Stale handle: state has been re-initialized" on the first path's discoverTargets().

    const statusBarItem = createStatusBar(context);
    registerCommands(context);

    if (config.syncOnSave) {
        let syncTimer: ReturnType<typeof setTimeout> | undefined;
        context.subscriptions.push(new vscode.Disposable(() => clearTimeout(syncTimer)));
        context.subscriptions.push(
            vscode.workspace.onDidSaveTextDocument(doc => {
                const fileName = path.basename(doc.uri.fsPath);
                if (fileName === 'BUILD' || fileName === 'BUILD.bazel') {
                    clearTimeout(syncTimer);
                    syncTimer = setTimeout(() => {
                        vscode.commands.executeCommand('bazel-jdt.syncProject');
                    }, 1000);
                }
            })
        );
    }

    context.subscriptions.push(statusBarItem);
}

export async function deactivate() {
    try {
        await vscode.commands.executeCommand(
            'java.execute.workspaceCommand', 'bazel-jdt.shutdown'
        );
    } catch {
        // LSP connection may already be disposed during deactivation — safe to ignore
    }
}
