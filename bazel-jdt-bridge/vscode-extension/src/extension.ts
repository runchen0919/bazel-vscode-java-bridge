import * as vscode from 'vscode';
import { registerCommands } from './commands';
import { createStatusBar } from './statusBar';

export async function activate(context: vscode.ExtensionContext) {
    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;

    if (!workspaceRoot) {
        return;
    }

    const workspaceFiles = await vscode.workspace.findFiles('{WORKSPACE,WORKSPACE.bazel}');
    if (workspaceFiles.length === 0) {
        return;
    }

    // Do NOT call bazel-jdt.importProject here.
    // BazelProjectImporter is auto-triggered by JDT.LS during workspace initialization
    // (via org.eclipse.jdt.ls.core.importers extension point in plugin.xml).
    // Calling it again from here causes a race: both paths call bridge.initialize(),
    // and the second call shuts down the state created by the first, producing
    // "Stale handle: state has been re-initialized" on the first path's discoverTargets().
    //
    // Do NOT add an onDidSaveTextDocument handler for BUILD files here.
    // BUILD file changes are detected by Java-side BazelBuildSupport.fileChanged()
    // via JDT.LS's IBuildSupport extension point, which is more reliable and covers
    // non-editor file changes too.

    const statusBarItem = createStatusBar(context);
    registerCommands(context);

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
