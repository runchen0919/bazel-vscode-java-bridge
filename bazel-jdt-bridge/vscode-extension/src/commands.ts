import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import { getConfig } from './config';
import { runImportWizard } from './importWizard';
import { parseBazelprojectFile, addDirectoryToBazelproject } from './bazelproject';

export function registerImportCommand(context: vscode.ExtensionContext) {
    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath || '';

    context.subscriptions.push(
        vscode.commands.registerCommand('bazel-jdt.importProject', async () => {
            try {
                const config = getConfig();
                await vscode.window.withProgress(
                    { location: vscode.ProgressLocation.Window, title: 'Importing Bazel project...' },
                    async (progress) => {
                        progress.report({ message: 'Setting up import...' });

                        const wizardResult = await runImportWizard(workspaceRoot);
                        const scopePatterns = wizardResult?.patterns || [];
                        let buildFlags: string[] = [];
                        let bazelPath = config.bazelPath;
                        if (wizardResult?.bazelprojectPath) {
                            const viewConfig = parseBazelprojectFile(wizardResult.bazelprojectPath);
                            if (viewConfig) {
                                buildFlags = viewConfig.buildFlags;
                                if (viewConfig.bazelBinary) {
                                    bazelPath = viewConfig.bazelBinary;
                                }
                            }
                        }

                        progress.report({ message: 'Discovering Java targets...' });
                        await vscode.commands.executeCommand('java.execute.workspaceCommand',
                            'bazel-jdt.importProject', workspaceRoot, bazelPath, config.cacheDir,
                            scopePatterns, buildFlags, config.dependencyResolution, config.dependencySourceLoading,
                            config.syncMode);
                    }
                );
                vscode.window.showInformationMessage('Bazel project imported successfully');
            } catch (error) {
                vscode.window.showErrorMessage(`Bazel import failed: ${error}`);
            }
        })
    );
}

export function registerAddDirectoryCommand(context: vscode.ExtensionContext, workspaceRoot: string) {
    context.subscriptions.push(
        vscode.commands.registerCommand('bazel-jdt.addDirectoryToProject', async (uri: vscode.Uri) => {
            if (!uri) return;

            const dirPath = uri.fsPath;
            const hasBuild = fs.existsSync(path.join(dirPath, 'BUILD')) ||
                             fs.existsSync(path.join(dirPath, 'BUILD.bazel'));
            if (!hasBuild) {
                vscode.window.showWarningMessage('No BUILD file found in selected directory.');
                return;
            }

            const relativeDir = path.relative(workspaceRoot, dirPath);
            const bazelprojectPath = path.join(workspaceRoot, '.bazelproject');
            const result = addDirectoryToBazelproject(bazelprojectPath, relativeDir);

            if (result === 'already-exists') {
                vscode.window.showInformationMessage(`'${relativeDir}' is already in the Bazel project scope.`);
            } else if (result === 'added') {
                vscode.window.showInformationMessage(`Added '${relativeDir}' to Bazel project scope.`);
            } else {
                vscode.window.showInformationMessage(`Created .bazelproject with '${relativeDir}'.`);
            }
        })
    );
}

export function registerRuntimeCommands(context: vscode.ExtensionContext) {
    context.subscriptions.push(
        vscode.commands.registerCommand('bazel-jdt.syncProject', async () => {
            try {
                const config = getConfig();
                await vscode.commands.executeCommand('java.execute.workspaceCommand',
                    'bazel-jdt.syncProject', config.dependencyResolution, config.dependencySourceLoading);
            } catch (error) {
                vscode.window.showErrorMessage(`Bazel sync failed: ${error}`);
            }
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('bazel-jdt.cleanCache', async () => {
            const confirm = await vscode.window.showWarningMessage(
                'Clear Bazel cache? This will trigger a full re-sync.',
                { modal: true },
                'Clear Cache'
            );
            if (confirm === 'Clear Cache') {
                try {
                    await vscode.commands.executeCommand('java.execute.workspaceCommand', 'bazel-jdt.cleanCache');
                    vscode.window.showInformationMessage('Bazel cache cleared');
                } catch (error) {
                    vscode.window.showErrorMessage(`Failed to clear cache: ${error}`);
                }
            }
        })
    );

    context.subscriptions.push(
        vscode.commands.registerCommand('bazel-jdt.createProjectForPackage', async (packagePath: string, targetLabel: string) => {
            const config = getConfig();
            const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath || '';
            await vscode.commands.executeCommand('java.execute.workspaceCommand',
                'bazel-jdt.createProjectForPackage', workspaceRoot, config.bazelPath,
                config.cacheDir, packagePath, targetLabel);
            vscode.window.showInformationMessage(`Created project for ${packagePath}`);
        })
    );
}
