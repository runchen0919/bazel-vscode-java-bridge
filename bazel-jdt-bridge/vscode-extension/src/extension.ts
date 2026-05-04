import * as vscode from 'vscode';
import * as path from 'path';
import { registerCommands } from './commands';
import { createStatusBar } from './statusBar';
import { getConfig } from './config';
import { parseBazelprojectFile, resolveScopePatterns } from './bazelproject';

export async function activate(context: vscode.ExtensionContext) {
    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;

    if (!workspaceRoot) {
        return;
    }

    const workspaceFiles = await vscode.workspace.findFiles('{WORKSPACE,WORKSPACE.bazel}');
    if (workspaceFiles.length === 0) {
        return;
    }

    const statusBarItem = createStatusBar(context);
    registerCommands(context);

    const bazelprojectPattern = new vscode.RelativePattern(workspaceRoot, '.bazelproject');
    const watcher = vscode.workspace.createFileSystemWatcher(bazelprojectPattern);
    let debounceTimer: ReturnType<typeof setTimeout> | undefined;
    let wizardActive = false;

    context.subscriptions.push(
        vscode.commands.registerCommand('_bazel-jdt.setWizardActive', (active: boolean) => {
            wizardActive = active;
            if (active) {
                setTimeout(() => { wizardActive = false; }, 5000);
            }
        })
    );

    const triggerReimport = () => {
        if (debounceTimer) {
            clearTimeout(debounceTimer);
        }
        debounceTimer = setTimeout(async () => {
            if (wizardActive) {
                return;
            }

            const config = getConfig();
            const viewConfig = parseBazelprojectFile(path.join(workspaceRoot, '.bazelproject'));
            const patterns = viewConfig ? resolveScopePatterns(viewConfig) : [];
            const buildFlags = viewConfig ? viewConfig.buildFlags : [];
            const bazelPath = viewConfig?.bazelBinary || config.bazelPath;

            try {
                await vscode.commands.executeCommand('java.execute.workspaceCommand',
                    'bazel-jdt.importProject', workspaceRoot, bazelPath, config.cacheDir,
                    patterns, buildFlags);
                vscode.window.showInformationMessage('Bazel project re-imported (scope changed)');
            } catch {
                // Silently ignore — re-import is best-effort
            }
        }, 1000);
    };

    context.subscriptions.push(
        watcher.onDidChange(triggerReimport),
        watcher.onDidCreate(triggerReimport),
        watcher.onDidDelete(triggerReimport),
        watcher,
        statusBarItem,
    );
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
