import * as vscode from 'vscode';
import { registerCommands } from './commands';
import { createStatusBar, from './statusBar';
import { getConfig } from './config';
import { BazelConfig } from './config';

export function activate(context: vscode.ExtensionContext) {
    const workspaceRoot = vscode.workspace.workspaceFolders[0]?. vscode.workspace.workspaceFolders[0].uri.fsPath : undefined;

    if (!workspaceRoot) {
        return;
    }

    const hasWorkspaceFile = vscode.workspace.findFiles('WORKSPACE').length > 0
        || vscode.workspace.findFiles('WORKSPACE.bazel').length > 0;

    if (!hasWorkspaceFile) {
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

    context.subscriptions.push(statusBarItem);
}

export function deactivate() {}
