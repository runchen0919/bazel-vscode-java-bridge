import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import { registerImportCommand, registerRuntimeCommands } from './commands';
import { BazelDebugConfigurationProvider } from './debugAdapter';
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

    const bazelprojectPath = path.join(workspaceRoot, '.bazelproject');
    const hasBazelproject = fs.existsSync(bazelprojectPath);

    if (hasBazelproject) {
        activateFull(context, workspaceRoot);
    } else {
        registerImportCommand(context);
        setupCreationOnlyWatcher(context, workspaceRoot);
    }
}

function activateFull(context: vscode.ExtensionContext, workspaceRoot: string) {
    const statusBarItem = createStatusBar(context);
    registerImportCommand(context);
    registerRuntimeCommands(context);
    context.subscriptions.push(
        vscode.debug.registerDebugConfigurationProvider(
            'java', new BazelDebugConfigurationProvider()
        )
    );

    context.subscriptions.push(
        vscode.debug.onDidTerminateDebugSession(async (session) => {
            if (session.type === 'java') {
                try {
                    await vscode.commands.executeCommand(
                        'java.execute.workspaceCommand',
                        'bazel-jdt.clearActiveDebugProject'
                    );
                } catch {
                    // LSP connection may be closed — safe to ignore
                }
            }
        })
    );

    let dependencyPackageCache: string[] = [];

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
                    patterns, buildFlags, config.dependencySourceLoading);

                if (config.dependencySourceLoading === 'on-demand') {
                    try {
                        const depPackages = await vscode.commands.executeCommand(
                            'java.execute.workspaceCommand', 'bazel-jdt.getDependencyPackages',
                            patterns) as string[];
                        if (depPackages) {
                            dependencyPackageCache = depPackages;
                        }
                    } catch {
                        // Non-critical — on-demand detection may not work but import succeeds
                    }
                }

                vscode.window.showInformationMessage('Bazel project re-imported (scope changed)');
            } catch {
                // Silently ignore — re-import is best-effort
            }
        }, 1000);
    };

    context.subscriptions.push(
        watcher.onDidChange(triggerReimport),
        watcher.onDidCreate(triggerReimport),
        watcher,
        statusBarItem,
    );

    // On-demand dependency source loading: monitor opened Java files
    context.subscriptions.push(
        vscode.workspace.onDidOpenTextDocument(async (doc) => {
            const config = getConfig();
            if (config.dependencySourceLoading !== 'on-demand') return;
            if (doc.languageId !== 'java') return;
            if (doc.uri.scheme !== 'file') return;

            const filePath = doc.uri.fsPath;
            if (!filePath.startsWith(workspaceRoot)) return;

            const relPath = filePath.substring(workspaceRoot.length + 1);
            const matchedPackage = dependencyPackageCache.find(pkg =>
                relPath.startsWith(pkg + '/') || relPath.startsWith(pkg + '\\')
            );
            if (!matchedPackage) return;

            const fileName = path.basename(filePath);
            const action = await vscode.window.showInformationMessage(
                `${fileName} is not in a project. Create a project for '${matchedPackage}'?`,
                'Create Project', 'Dismiss'
            );
            if (action === 'Create Project') {
                await vscode.commands.executeCommand('bazel-jdt.createProjectForPackage',
                    matchedPackage, '//' + matchedPackage + ':' + matchedPackage.split('/').pop());
            }
        })
    );
}

function setupCreationOnlyWatcher(context: vscode.ExtensionContext, workspaceRoot: string) {
    const pattern = new vscode.RelativePattern(workspaceRoot, '.bazelproject');
    const watcher = vscode.workspace.createFileSystemWatcher(pattern);

    context.subscriptions.push(
        watcher.onDidCreate(async () => {
            watcher.dispose();

            const choice = await vscode.window.showInformationMessage(
                'Bazel project config detected. Reload window to activate.',
                'Reload',
                'Dismiss'
            );

            if (choice === 'Reload') {
                vscode.commands.executeCommand('workbench.action.reloadWindow');
            }
        })
    );

    context.subscriptions.push(watcher);
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
