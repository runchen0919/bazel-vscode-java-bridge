import * as vscode from 'vscode';

export interface BazelConfig {
    bazelPath: string;
    syncOnSave: boolean;
    cacheDir: string;
    importScanDirs: string[];
    buildFlags: string[];
    javaLanguageLevel: string;
    syncFlags: string[];
    excludeTargets: string[];
    testSources: string[];
    deriveTargets: boolean;
    dependencyResolution: string;
    dependencySourceLoading: string;
}

export function getConfig(): BazelConfig {
    const config = vscode.workspace.getConfiguration('bazel-jdt');
    return {
        bazelPath: config.get<string>('bazelPath', 'bazel'),
        syncOnSave: config.get<boolean>('syncOnSave', true),
        cacheDir: config.get<string>('cacheDir', ''),
        importScanDirs: config.get<string[]>('importScanDirs', []),
        buildFlags: config.get<string[]>('buildFlags', []),
        javaLanguageLevel: config.get<string>('javaLanguageLevel', '17'),
        syncFlags: config.get<string[]>('syncFlags', []),
        excludeTargets: config.get<string[]>('excludeTargets', []),
        testSources: config.get<string[]>('testSources', []),
        deriveTargets: config.get<boolean>('deriveTargets', true),
        dependencyResolution: config.get<string>('dependencyResolution', 'transitive'),
        dependencySourceLoading: config.get<string>('dependencySourceLoading', 'full-project'),
    };
}
