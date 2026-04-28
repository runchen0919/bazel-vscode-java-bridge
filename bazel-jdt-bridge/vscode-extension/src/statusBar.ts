import * as vscode from 'vscode';

export function createStatusBar(context: vscode.ExtensionContext): vscode.StatusBarItem {
    const statusBarItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
    statusBarItem.text = 'Bazel ✓';
    statusBarItem.show();
    statusBarItem.command = 'bazel-jdt.syncProject';

    const pollInterval = setInterval(async () => {
        try {
            const state = await vscode.commands.executeCommand('java.execute.workspaceCommand', 'bazel-jdt.getSyncState');
            if (typeof state === 'number') {
                if (state === 0) {
                    statusBarItem.text = 'Bazel ✓';
                    statusBarItem.backgroundColor = undefined;
                } else if (state === 1) {
                    statusBarItem.text = 'Bazel ⟳ Syncing...';
                    statusBarItem.backgroundColor = new vscode.ThemeColor('statusBarItem.warningBackground');
                } else {
                    statusBarItem.text = 'Bazel ✗';
                    statusBarItem.backgroundColor = new vscode.ThemeColor('statusBarItem.errorBackground');
                }
            }
        } catch {
            statusBarItem.text = '$(sync~spin) Bazel';
            statusBarItem.backgroundColor = undefined;
        }
    }, 2000);

    context.subscriptions.push(new vscode.Disposable(() => clearInterval(pollInterval)));

    return statusBarItem;
}
