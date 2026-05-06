import * as vscode from 'vscode';
import * as assert from 'assert';

suite('Dependency Resolution', () => {
    test('dependencyResolution config has correct defaults and enum values', async function () {
        this.timeout(30_000);
        const config = vscode.workspace.getConfiguration('bazel-jdt');
        const mode = config.get<string>('dependencyResolution');
        assert.ok(
            mode === 'transitive' || mode === 'optional',
            `dependencyResolution should be 'transitive' or 'optional', got '${mode}'`
        );
    });

    test('dependencyResolution can be switched to optional mode', async function () {
        this.timeout(30_000);
        const config = vscode.workspace.getConfiguration('bazel-jdt');
        const original = config.get<string>('dependencyResolution');

        try {
            await config.update('dependencyResolution', 'optional', vscode.ConfigurationTarget.Workspace);
            const updated = config.get<string>('dependencyResolution');
            assert.strictEqual(updated, 'optional', 'Should be switched to optional');
        } finally {
            await config.update('dependencyResolution', original, vscode.ConfigurationTarget.Workspace);
        }
    });

    test('dependencyResolution can be switched to transitive mode', async function () {
        this.timeout(30_000);
        const config = vscode.workspace.getConfiguration('bazel-jdt');
        const original = config.get<string>('dependencyResolution');

        try {
            await config.update('dependencyResolution', 'transitive', vscode.ConfigurationTarget.Workspace);
            const updated = config.get<string>('dependencyResolution');
            assert.strictEqual(updated, 'transitive', 'Should be switched to transitive');
        } finally {
            await config.update('dependencyResolution', original, vscode.ConfigurationTarget.Workspace);
        }
    });

    test('config change persists across reads', async function () {
        this.timeout(30_000);
        const config = vscode.workspace.getConfiguration('bazel-jdt');
        const original = config.get<string>('dependencyResolution');

        try {
            await config.update('dependencyResolution', 'optional', vscode.ConfigurationTarget.Workspace);

            const config2 = vscode.workspace.getConfiguration('bazel-jdt');
            const read1 = config2.get<string>('dependencyResolution');
            assert.strictEqual(read1, 'optional');

            await config.update('dependencyResolution', 'transitive', vscode.ConfigurationTarget.Workspace);
            const read2 = vscode.workspace.getConfiguration('bazel-jdt').get<string>('dependencyResolution');
            assert.strictEqual(read2, 'transitive');
        } finally {
            await config.update('dependencyResolution', original, vscode.ConfigurationTarget.Workspace);
        }
    });
});
