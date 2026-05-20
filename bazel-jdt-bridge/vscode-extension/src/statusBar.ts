import * as vscode from 'vscode';

const IDLE_INTERVAL_MS = 10_000;
const SYNCING_INTERVAL_MS = 2_000;
const ERROR_INTERVAL_MS = 15_000;
const MAX_CONSECUTIVE_FAILURES = 3;

export function createStatusBar(context: vscode.ExtensionContext): vscode.StatusBarItem {
    const statusBarItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
    statusBarItem.text = 'Bazel ✓';
    statusBarItem.show();
    statusBarItem.command = 'bazel-jdt.syncProject';

    let stopped = false;
    let consecutiveFailures = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;

    const poll = async () => {
        if (stopped) return;

        try {
            const state = await vscode.commands.executeCommand('java.execute.workspaceCommand', 'bazel-jdt.getSyncState');
            consecutiveFailures = 0;

            if (typeof state === 'number') {
                if (state === 0) {
                    statusBarItem.text = '$(sync~spin) Indexing...';
                    statusBarItem.backgroundColor = undefined;
                    try {
                        await vscode.commands.executeCommand('java.execute.workspaceCommand', 'bazel-jdt.waitForIndexesReady');
                    } catch {
                        // Command not available or failed, proceed to ready state
                    }
                    if (stopped) return;
                    statusBarItem.text = 'Bazel ✓';
                    statusBarItem.backgroundColor = undefined;
                    timer = setTimeout(poll, IDLE_INTERVAL_MS);
                } else if (state === 1) {
                    statusBarItem.text = 'Bazel ⟳ Syncing...';
                    statusBarItem.backgroundColor = new vscode.ThemeColor('statusBarItem.warningBackground');
                    timer = setTimeout(poll, SYNCING_INTERVAL_MS);
                } else {
                    statusBarItem.text = 'Bazel ✗';
                    statusBarItem.backgroundColor = new vscode.ThemeColor('statusBarItem.errorBackground');
                    timer = setTimeout(poll, ERROR_INTERVAL_MS);
                }
            } else {
                timer = setTimeout(poll, IDLE_INTERVAL_MS);
            }
        } catch {
            consecutiveFailures++;
            if (consecutiveFailures >= MAX_CONSECUTIVE_FAILURES) {
                statusBarItem.text = '$(sync~spin) Bazel';
                statusBarItem.backgroundColor = undefined;
                return;
            }
            statusBarItem.text = '$(sync~spin) Bazel';
            statusBarItem.backgroundColor = undefined;
            timer = setTimeout(poll, ERROR_INTERVAL_MS);
        }
    };

    timer = setTimeout(poll, IDLE_INTERVAL_MS);

    context.subscriptions.push(new vscode.Disposable(() => {
        stopped = true;
        if (timer !== undefined) {
            clearTimeout(timer);
        }
    }));

    return statusBarItem;
}
