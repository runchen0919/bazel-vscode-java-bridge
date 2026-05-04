import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import { parseBazelprojectFile, resolveScopePatterns } from './bazelproject';
import { getConfig } from './config';

export interface ImportWizardResult {
    strategy: 'existing' | 'manual' | 'everything';
    patterns: string[];
    bazelprojectPath?: string;
}

interface BazelprojectOptions {
    directories: string[];
    targets: string[];
    deriveTargetsFromDirectories: boolean;
    testSources: string[];
    buildFlags: string[];
    syncFlags: string[];
    excludeTarget: string[];
    imports: string[];
    bazelBinary: string;
    javaLanguageLevel: string;
}

export async function runImportWizard(
    workspaceRoot: string
): Promise<ImportWizardResult | undefined> {
    const existingFile = findBazelprojectFile(workspaceRoot);

    if (existingFile) {
        const useExisting = await vscode.window.showQuickPick(
            [
                {
                    label: 'Use existing .bazelproject',
                    description: path.basename(existingFile),
                    detail: existingFile,
                },
                {
                    label: 'Choose directories manually',
                    description: 'Override with manual selection',
                },
                {
                    label: 'Import everything',
                    description: 'Import all Java targets (//...:*)',
                },
            ],
            { placeHolder: 'Found .bazelproject file. How to proceed?' }
        );

        if (!useExisting) {
            return undefined;
        }

        if (useExisting.label.startsWith('Use existing')) {
            const config = parseBazelprojectFile(existingFile);
            const patterns = config ? resolveScopePatterns(config) : [];
            return {
                strategy: 'existing',
                patterns,
                bazelprojectPath: existingFile,
            };
        }

        if (useExisting.label.startsWith('Import everything')) {
            return { strategy: 'everything', patterns: [] };
        }
    } else {
        const strategy = await vscode.window.showQuickPick(
            [
                {
                    label: '$(file-directory) Select directories to import',
                    description: 'Choose workspace directories',
                },
                {
                    label: '$(globe) Import everything',
                    description: 'Import all Java targets (//...:*)',
                },
            ],
            { placeHolder: 'No .bazelproject found. How to import?' }
        );

        if (!strategy) {
            return undefined;
        }

        if (strategy.label.includes('Import everything')) {
            return { strategy: 'everything', patterns: [] };
        }
    }

    return runDirectoryPicker(workspaceRoot);
}

async function runDirectoryPicker(
    workspaceRoot: string
): Promise<ImportWizardResult | undefined> {
    const config = getConfig();
    const dirs = collectDirectories(workspaceRoot, config.importScanDirs);

    if (dirs.length === 0) {
        vscode.window.showWarningMessage('No directories with BUILD files found in workspace.');
        return undefined;
    }

    const picks = await vscode.window.showQuickPick(
        dirs.map((d) => ({
            label: d.name,
            description: d.parentPath,
            detail: d.relative,
        })),
        {
            placeHolder: 'Select directories to import (multi-select)',
            canPickMany: true,
        }
    );

    if (!picks || picks.length === 0) {
        return undefined;
    }

    const selectedDirs = picks.map((p) => p.detail);
    const patterns = selectedDirs.map((d) => `//${d}/...:*`);

    const derivePick = await vscode.window.showQuickPick(
        [
            { label: 'Yes', description: 'Automatically derive targets from selected directories' },
            { label: 'No', description: 'Specify targets manually' },
        ],
        {
            placeHolder: 'Derive targets from directories?',
        }
    );

    if (!derivePick) {
        return undefined;
    }

    const deriveTargets = derivePick.label === 'Yes';

    const javaLevelPick = await vscode.window.showQuickPick(
        ['8', '11', '17', '21'].map((v) => ({
            label: v,
            description: v === config.javaLanguageLevel ? 'current default' : undefined,
        })),
        {
            placeHolder: `Java language level (default: ${config.javaLanguageLevel})`,
        }
    );

    const javaLanguageLevel = javaLevelPick?.label || config.javaLanguageLevel;

    const bazelBinary = await vscode.window.showInputBox({
        prompt: 'Path to Bazel binary',
        value: config.bazelPath,
        placeHolder: 'bazel',
    });

    if (bazelBinary === undefined) {
        return undefined;
    }

    const advancedPick = await vscode.window.showQuickPick(
        [
            {
                label: '$(gear) Configure advanced options',
                description: 'Targets, excludes, sync flags, imports, test sources',
            },
            {
                label: 'Continue with defaults',
                description: 'Use recommended settings',
            },
        ],
        { placeHolder: 'Configure advanced .bazelproject options?' }
    );

    if (!advancedPick) {
        return undefined;
    }

    let options: BazelprojectOptions;

    if (advancedPick.label.includes('Configure advanced')) {
        const advancedResult = await runAdvancedOptions(config, workspaceRoot, selectedDirs, deriveTargets, javaLanguageLevel, bazelBinary);
        if (!advancedResult) {
            return undefined;
        }
        options = advancedResult;
    } else {
        const autoTestSources = detectTestSourceDirs(workspaceRoot, selectedDirs);
        options = {
            directories: selectedDirs,
            targets: [],
            deriveTargetsFromDirectories: deriveTargets,
            testSources: [...autoTestSources, ...config.testSources],
            buildFlags: config.buildFlags,
            syncFlags: config.syncFlags,
            excludeTarget: config.excludeTargets,
            imports: [],
            bazelBinary,
            javaLanguageLevel,
        };
    }

    if (!deriveTargets) {
        patterns.length = 0;
        patterns.push(...options.targets);
    }
    for (const ex of options.excludeTarget) {
        const prefix = ex.startsWith('-') ? ex : `-${ex}`;
        patterns.push(prefix);
    }

    const saveChoice = await vscode.window.showQuickPick(
        [
            { label: 'Yes, save to .bazelproject', description: 'Create .bazelproject for future imports' },
            { label: 'No, import only this time', description: 'Skip saving .bazelproject' },
        ],
        { placeHolder: 'Save configuration to .bazelproject?' }
    );

    if (saveChoice && saveChoice.label.startsWith('Yes')) {
        const bazelprojectPath = path.join(workspaceRoot, '.bazelproject');
        const content = generateBazelprojectContent(options);
        vscode.commands.executeCommand('_bazel-jdt.setWizardActive', true);
        fs.writeFileSync(bazelprojectPath, content, 'utf-8');
        setTimeout(() => {
            vscode.commands.executeCommand('_bazel-jdt.setWizardActive', false).then(undefined, () => {});
        }, 2000);
        return { strategy: 'manual', patterns, bazelprojectPath };
    }

    return { strategy: 'manual', patterns };
}

async function runAdvancedOptions(
    config: ReturnType<typeof getConfig>,
    workspaceRoot: string,
    selectedDirs: string[],
    deriveTargets: boolean,
    javaLanguageLevel: string,
    bazelBinary: string
): Promise<BazelprojectOptions | undefined> {
    let targets: string[] = [];
    if (!deriveTargets) {
        const targetsInput = await vscode.window.showInputBox({
            prompt: 'Bazel targets to import (comma-separated)',
            placeHolder: '//path/to:target, //another:all',
            value: '',
        });
        if (targetsInput === undefined) {
            return undefined;
        }
        targets = parseCommaInput(targetsInput);
    }

    const autoTestSources = detectTestSourceDirs(workspaceRoot, selectedDirs);
    const testSourcesDefault = [...autoTestSources, ...config.testSources].join(', ');
    const testSourcesInput = await vscode.window.showInputBox({
        prompt: 'Test source glob patterns (comma-separated)',
        placeHolder: 'src/test/java/**, javatests/**',
        value: testSourcesDefault,
    });
    if (testSourcesInput === undefined) {
        return undefined;
    }
    const testSources = parseCommaInput(testSourcesInput);

    const excludeDefault = config.excludeTargets.join(', ');
    const excludeInput = await vscode.window.showInputBox({
        prompt: 'Bazel targets to exclude (comma-separated)',
        placeHolder: '//third_party:expensive_test',
        value: excludeDefault,
    });
    if (excludeInput === undefined) {
        return undefined;
    }
    const excludeTarget = parseCommaInput(excludeInput);

    const buildFlagsDefault = config.buildFlags.join(', ');
    const buildFlagsInput = await vscode.window.showInputBox({
        prompt: 'Bazel build flags (comma-separated)',
        placeHolder: '--config=dev, --java_language_version=17',
        value: buildFlagsDefault,
    });
    if (buildFlagsInput === undefined) {
        return undefined;
    }
    const buildFlags = parseCommaInput(buildFlagsInput);

    const syncFlagsDefault = config.syncFlags.join(', ');
    const syncFlagsInput = await vscode.window.showInputBox({
        prompt: 'Sync flags (comma-separated) — reserved for future use',
        placeHolder: '--keep_going',
        value: syncFlagsDefault,
    });
    if (syncFlagsInput === undefined) {
        return undefined;
    }
    const syncFlags = parseCommaInput(syncFlagsInput);

    const importsInput = await vscode.window.showInputBox({
        prompt: 'Import other .bazelproject files (comma-separated) — reserved for future use',
        placeHolder: 'path/to/other.bazelproject',
        value: '',
    });
    if (importsInput === undefined) {
        return undefined;
    }
    const imports = parseCommaInput(importsInput);

    return {
        directories: selectedDirs,
        targets,
        deriveTargetsFromDirectories: deriveTargets,
        testSources,
        buildFlags,
        syncFlags,
        excludeTarget,
        imports,
        bazelBinary,
        javaLanguageLevel,
    };
}

function parseCommaInput(input: string): string[] {
    return input
        .split(',')
        .map((s) => s.trim())
        .filter((s) => s.length > 0);
}

interface DirectoryEntry {
    name: string;
    parentPath: string;
    relative: string;
}

function hasBuildFile(dirPath: string): boolean {
    return fs.existsSync(path.join(dirPath, 'BUILD')) ||
           fs.existsSync(path.join(dirPath, 'BUILD.bazel'));
}

const SKIP_DIRS = new Set(['.', 'bazel-out', 'bazel-bin', 'bazel-testlogs', 'bazel-genfiles']);

function collectDirectories(workspaceRoot: string, scanDirs: string[]): DirectoryEntry[] {
    const results: DirectoryEntry[] = [];
    const seen = new Set<string>();

    function addEntry(relativePath: string) {
        if (seen.has(relativePath)) {
            return;
        }
        seen.add(relativePath);
        const parts = relativePath.split('/');
        const name = parts[parts.length - 1];
        const parentParts = parts.slice(0, -1);
        const parentPath = parentParts.length > 0 ? parentParts.join('/') + '/' : '';
        results.push({ name, parentPath, relative: relativePath });
    }

    try {
        const entries = fs.readdirSync(workspaceRoot, { withFileTypes: true });
        for (const entry of entries) {
            if (!entry.isDirectory() || entry.name.startsWith('.') || SKIP_DIRS.has(entry.name)) {
                continue;
            }
            const fullPath = path.join(workspaceRoot, entry.name);
            if (hasBuildFile(fullPath)) {
                addEntry(entry.name);
            }
        }
    } catch {
    }

    for (const scanDir of scanDirs) {
        const scanPath = path.join(workspaceRoot, scanDir);
        try {
            const entries = fs.readdirSync(scanPath, { withFileTypes: true });
            for (const entry of entries) {
                if (!entry.isDirectory() || entry.name.startsWith('.') || SKIP_DIRS.has(entry.name)) {
                    continue;
                }
                const childPath = path.join(scanPath, entry.name);
                if (hasBuildFile(childPath)) {
                    const relative = scanDir + '/' + entry.name;
                    addEntry(relative);
                }
            }
        } catch {
        }
    }

    results.sort((a, b) => a.relative.localeCompare(b.relative));
    return results;
}

function detectTestSourceDirs(workspaceRoot: string, directories: string[]): string[] {
    const testPatterns: string[] = [];
    const testSubdirs = ['src/test/java', 'test', 'javatests'];

    for (const dir of directories) {
        for (const sub of testSubdirs) {
            const fullPath = path.join(workspaceRoot, dir, sub);
            if (fs.existsSync(fullPath) && fs.statSync(fullPath).isDirectory()) {
                testPatterns.push(`${dir}/${sub}/**`);
            }
        }
    }

    return testPatterns;
}

/**
 * Generate complete .bazelproject content from all configured keys.
 * Section order: directories → derive_targets_from_directories → targets →
 *   test_sources → build_flags → sync_flags → exclude_target → imports →
 *   bazel_binary → java_language_level
 *
 * Empty sections are omitted. Reserved sections (sync_flags, imports) get
 * a "# Reserved for future use" comment.
 */
function generateBazelprojectContent(options: BazelprojectOptions): string {
    const lines: string[] = ['# Generated by Bazel JDT Bridge'];
    let firstSection = true;

    function pushSectionHeader(header: string) {
        if (!firstSection) {
            lines.push('');
        }
        lines.push(header);
        firstSection = false;
    }

    pushSectionHeader('directories:');
    for (const dir of options.directories) {
        lines.push(`  ${dir}`);
    }

    pushSectionHeader('derive_targets_from_directories:');
    lines.push(`  ${options.deriveTargetsFromDirectories ? 'True' : 'False'}`);

    if (options.targets.length > 0) {
        pushSectionHeader('targets:');
        for (const t of options.targets) {
            lines.push(`  ${t}`);
        }
    }

    if (options.testSources.length > 0) {
        pushSectionHeader('test_sources:');
        for (const ts of options.testSources) {
            lines.push(`  ${ts}`);
        }
    }

    if (options.buildFlags.length > 0) {
        pushSectionHeader('build_flags:');
        for (const flag of options.buildFlags) {
            lines.push(`  ${flag}`);
        }
    }

    if (options.syncFlags.length > 0) {
        if (!firstSection) {
            lines.push('');
        }
        lines.push('# Reserved for future use');
        lines.push('sync_flags:');
        firstSection = false;
        for (const flag of options.syncFlags) {
            lines.push(`  ${flag}`);
        }
    }

    if (options.excludeTarget.length > 0) {
        pushSectionHeader('exclude_target:');
        for (const t of options.excludeTarget) {
            lines.push(`  ${t}`);
        }
    }

    if (options.imports.length > 0) {
        if (!firstSection) {
            lines.push('');
        }
        lines.push('# Reserved for future use');
        lines.push('import:');
        firstSection = false;
        for (const imp of options.imports) {
            lines.push(`  ${imp}`);
        }
    }

    if (options.bazelBinary && options.bazelBinary !== 'bazel') {
        pushSectionHeader(`bazel_binary: ${options.bazelBinary}`);
    }

    if (options.javaLanguageLevel) {
        pushSectionHeader(`java_language_level: ${options.javaLanguageLevel}`);
    }

    return lines.join('\n') + '\n';
}

function findBazelprojectFile(workspaceRoot: string): string | undefined {
    const candidates = ['.bazelproject'];
    for (const name of candidates) {
        const fullPath = path.join(workspaceRoot, name);
        if (fs.existsSync(fullPath)) {
            return fullPath;
        }
    }
    return undefined;
}
