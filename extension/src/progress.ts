// Project progress in the status bar: how many of a folder's tests pass,
// and a list of what is passing, failing, not run or not started.

import * as fs from 'node:fs';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { TestExplorer, textOf, withoutComments } from './testing';
import { hasNoAsmCode, primaryScriptTarget } from './testLinks';
import { ignoresCase } from './paths';

export type ProgressState = 'failing' | 'notRun' | 'notStarted' | 'passing' | 'demonstration';

const ORDER: ProgressState[] = ['failing', 'notRun', 'notStarted', 'passing', 'demonstration'];

const ICONS: Record<ProgressState, string> = {
    failing: '$(error)',
    notRun: '$(circle-outline)',
    notStarted: '$(dash)',
    passing: '$(pass)',
    demonstration: '$(eye)',
};

/** One script of a folder, as the list shows it. */
export interface ProgressEntry {
    script: vscode.Uri;
    state: ProgressState;
    label: string;
    description: string;
    /** What picking it opens: the file the script loads, or the script. */
    target: vscode.Uri;
}

export interface FolderProgress {
    folder: string;
    passed: number;
    failed: number;
    notRun: number;
    total: number;
    running: boolean;
    entries: ProgressEntry[];
}

/** Whether a chip's `PARTS:` section is still empty: the chip as the course hands it out. */
export function hasNoParts(hdl: string): boolean {
    return /\bPARTS\s*:\s*\}\s*$/.test(withoutComments(hdl).trimEnd());
}

export class Progress implements vscode.Disposable {
    private readonly item = vscode.window.createStatusBarItem('nandwright.progress', vscode.StatusBarAlignment.Left, 10);
    /** Hand In: zips the project of the active file. */
    private readonly handIn = vscode.window.createStatusBarItem('nandwright.handIn', vscode.StatusBarAlignment.Left, 9);
    /** The folder Hand In zips. */
    private handInFolder: string | undefined;
    /** Folders that had a failing test this session, and those already offered a zip. */
    private readonly hadFailing = new Set<string>();
    private readonly offeredZip = new Set<string>();
    private readonly disposables: vscode.Disposable[] = [];
    /** The folder shown: the last one with tests whose file was active. */
    private folder: string | undefined;
    private timer: NodeJS.Timeout | undefined;
    private updating: Promise<void> = Promise.resolve();

    constructor(private readonly tests: TestExplorer) {
        this.item.name = 'Nand2Tetris Project Progress';
        this.item.command = 'nandwright.showProgress';
        this.handIn.name = 'Nand2Tetris Hand In';
        this.handIn.text = '$(file-zip) Hand In';
        this.handIn.tooltip = "Zip this project's files for handing in";
        this.disposables.push(
            this.item,
            this.handIn,
            vscode.window.onDidChangeActiveTextEditor(() => this.schedule()),
            tests.onDidChangeStatus(() => this.schedule()),
            vscode.commands.registerCommand('nandwright.showProgress', () => this.show()),
        );
        this.schedule();
    }

    /** The item's text and tooltip as shown now, for tests. */
    get shown(): { visible: boolean; text: string; tooltip: string; folder: string | undefined; handIn: string | undefined } {
        return {
            visible: this.visible,
            text: this.item.text,
            tooltip: String(this.item.tooltip ?? ''),
            folder: this.folder,
            handIn: this.handInShown ? this.handInFolder : undefined,
        };
    }

    private handInShown = false;

    /** Shows Hand In for a folder with Nand2Tetris files, for the active file's folder. */
    private async refreshHandIn(uri: vscode.Uri | undefined): Promise<void> {
        if (!uri) {
            return;
        }
        const folder = uri.scheme === 'file' ? path.dirname(uri.fsPath) : undefined;
        if (folder && (await hasNand2TetrisFiles(folder))) {
            this.handInFolder = folder;
            this.handIn.command = { title: 'Hand In', command: 'nandwright.zipProject', arguments: [vscode.Uri.file(folder)] };
            this.handInShown = true;
            this.handIn.show();
        } else {
            this.handInShown = false;
            this.handIn.hide();
        }
    }

    /** Once per folder and session: when its last failing test passes, offer to zip it. */
    private offerZip(progress: FolderProgress): void {
        const key = progress.folder;
        if (progress.failed > 0) {
            this.hadFailing.add(key);
            return;
        }
        if (progress.running || progress.total === 0 || progress.passed !== progress.total || !this.hadFailing.has(key) || this.offeredZip.has(key)) {
            return;
        }
        this.offeredZip.add(key);
        void vscode.window
            .showInformationMessage(`All ${progress.total} tests in ${path.basename(key)} pass.`, 'Zip for Hand-In')
            .then((answer) => answer && vscode.commands.executeCommand('nandwright.zipProject', vscode.Uri.file(key)));
    }

    private visible = false;

    private schedule(): void {
        clearTimeout(this.timer);
        this.timer = setTimeout(() => void this.update(), 100);
    }

    /** Brings the item up to date, after any update already under way. */
    update(): Promise<void> {
        this.updating = this.updating.then(() => this.refresh()).catch(() => undefined);
        return this.updating;
    }

    private async refresh(): Promise<void> {
        const uri = vscode.window.activeTextEditor?.document.uri;
        await this.refreshHandIn(uri);
        if (uri) {
            // A panel leaves no active editor and keeps the last folder; any
            // file outside a folder of tests hides the item.
            const folder = uri.scheme === 'file' ? path.dirname(uri.fsPath) : undefined;
            this.folder = folder && (await scriptsIn(folder)).length > 0 ? folder : undefined;
        }
        if (!this.folder) {
            this.visible = false;
            this.item.hide();
            return;
        }
        const progress = await this.of(this.folder);
        this.offerZip(progress);
        const name = path.basename(progress.folder);
        this.item.text = `${progress.running ? '$(sync~spin)' : '$(beaker)'} ${progress.passed} of ${progress.total} pass`;
        this.item.tooltip = `Nand2Tetris tests in ${name}: ${progress.passed} passing, ${progress.failed} failing, ${progress.notRun} not run. Click for the list.`;
        this.visible = true;
        this.item.show();
    }

    /** The scripts of a folder and how each stands. */
    async of(folder: string): Promise<FolderProgress> {
        const entries: ProgressEntry[] = [];
        let passed = 0;
        let failed = 0;
        let total = 0;
        let running = false;
        for (const script of await scriptsIn(folder)) {
            const stem = path.basename(script.fsPath, path.extname(script.fsPath));
            const text = (await textOf(script.fsPath)) ?? '';
            const loaded = await loadedFile(script, text);
            const status = this.tests.statusOf(script);
            running ||= status.status === 'running';
            let state: ProgressState;
            let description: string;
            if (await this.tests.runsForever(script)) {
                state = 'demonstration';
                description = 'a demonstration to watch, not a test';
            } else {
                total++;
                const result = status.status === 'current' ? status.result : undefined;
                const passes = result?.kind === 'passed' || result?.kind === 'unchecked';
                const fails = result?.kind === 'failed' || result?.kind === 'error' || result?.kind === 'limit';
                passed += passes ? 1 : 0;
                failed += fails ? 1 : 0;
                const loadedText = loaded && /\.(hdl|asm)$/i.test(loaded.fsPath) ? await textOf(loaded.fsPath) : null;
                if (loaded && /\.hdl$/i.test(loaded.fsPath) && loadedText !== null && hasNoParts(loadedText)) {
                    state = 'notStarted';
                    description = 'not started: the chip has no parts yet';
                } else if (loaded && /\.asm$/i.test(loaded.fsPath) && loadedText !== null && hasNoAsmCode(loadedText)) {
                    state = 'notStarted';
                    description = 'not started: the program has no instructions yet';
                } else if (fails && result) {
                    state = 'failing';
                    description =
                        result.kind === 'failed'
                            ? `${result.differingRows ?? result.failureCount} of ${result.rows ?? Math.max(0, result.comparedLines - 1)} rows differ`
                            : `error: ${result.message ?? ''}`;
                } else if (passes) {
                    state = 'passing';
                    description = 'passes';
                } else {
                    state = 'notRun';
                    description = 'not run since the last change';
                }
            }
            entries.push({ script, state, label: `${ICONS[state]} ${stem}`, description, target: loaded ?? script });
        }
        entries.sort((a, b) => ORDER.indexOf(a.state) - ORDER.indexOf(b.state));
        return { folder, passed, failed, notRun: total - passed - failed, total, running, entries };
    }

    /** Runs every test of a folder, as "run all" in the Testing view does. */
    async runAll(folder: string): Promise<void> {
        const progress = await this.of(folder);
        const scripts = progress.entries.filter((entry) => entry.state !== 'demonstration').map((entry) => entry.script);
        await this.tests.runScripts(scripts, { quiet: false });
    }

    /** Opens what an entry stands for. */
    async open(entry: ProgressEntry): Promise<void> {
        await vscode.window.showTextDocument(entry.target, { preview: false });
    }

    private async show(): Promise<void> {
        const folder =
            this.folder ??
            (vscode.window.activeTextEditor?.document.uri.scheme === 'file'
                ? path.dirname(vscode.window.activeTextEditor.document.uri.fsPath)
                : undefined);
        if (!folder) {
            return;
        }
        const progress = await this.of(folder);
        type Item = vscode.QuickPickItem & { entry?: ProgressEntry; runAll?: boolean; zip?: boolean };
        const items: Item[] = [
            { label: `$(run-all) Run all ${progress.total} tests in this folder`, runAll: true },
            { label: '$(file-zip) Zip this project for hand-in\u2026', zip: true },
            ...progress.entries.map((entry) => ({ label: entry.label, description: entry.description, entry })),
        ];
        const picked = await vscode.window.showQuickPick(items, {
            title: `${path.basename(folder)}: ${progress.passed} of ${progress.total} tests pass`,
        });
        if (picked?.runAll) {
            await this.runAll(folder);
        } else if (picked?.zip) {
            await vscode.commands.executeCommand('nandwright.zipProject', vscode.Uri.file(folder));
        } else if (picked?.entry) {
            await this.open(picked.entry);
        }
    }

    dispose(): void {
        clearTimeout(this.timer);
        this.disposables.forEach((d) => d.dispose());
    }
}

/** Whether a folder holds files of the Nand2Tetris tools directly. */
async function hasNand2TetrisFiles(folder: string): Promise<boolean> {
    try {
        return (await fs.promises.readdir(folder)).some((name) => /\.(hdl|tst|asm|hack|vm|jack)$/i.test(name));
    } catch {
        return false;
    }
}

/** The `.tst` scripts directly in a folder, by name. */
async function scriptsIn(folder: string): Promise<vscode.Uri[]> {
    try {
        const names = await fs.promises.readdir(folder);
        return names
            .filter((name) => /\.tst$/i.test(name))
            .sort((a, b) => a.localeCompare(b))
            .map((name) => vscode.Uri.file(path.join(folder, name)));
    } catch {
        return [];
    }
}

/** The primary file a script tests, if it is in the script's folder. */
async function loadedFile(script: vscode.Uri, text: string): Promise<vscode.Uri | undefined> {
    const dir = path.dirname(script.fsPath);
    let entries: string[];
    try {
        entries = await fs.promises.readdir(dir);
    } catch {
        return undefined;
    }
    const name = primaryScriptTarget(text, entries, ignoresCase(script.fsPath));
    if (!name) {
        return undefined;
    }
    const file = path.join(dir, name);
    try {
        await fs.promises.stat(file);
        return vscode.Uri.file(file);
    } catch {
        return undefined;
    }
}
