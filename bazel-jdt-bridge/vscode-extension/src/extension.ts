import * as vscode from 'vscode';
import * as path from 'path';
import { registerCommands } from './commands';
import { createStatusBar } from './statusBar';
import { getConfig } from './config';
import { BazelConfig } from './config';

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

    try {
        await vscode.commands.executeCommand(
            'java.execute.workspaceCommand',
            'bazel-jdt.importProject',
            workspaceRoot,
            config.bazelPath,
            config.cacheDir
        );
        vscode.window.showInformationMessage('Bazel project imported successfully');
    } catch (error) {
        vscode.window.showErrorMessage(`Bazel import failed: ${error}`);
    }

    const statusBarItem = createStatusBar(context);
    registerCommands(context);

    if (config.syncOnSave) {
        context.subscriptions.push(
            vscode.workspace.onDidSaveTextDocument(doc => {
                const fileName = path.basename(doc.uri.fsPath);
                if (fileName === 'BUILD' || fileName === 'BUILD.bazel') {
                    vscode.commands.executeCommand('bazel-jdt.syncProject');
                }
            })
        );
    }

    context.subscriptions.push(statusBarItem);
}

export function deactivate() {}
