// The computer panel: runs a program with its screen, keyboard, registers and
// memory, next to the source. Assembly and machine code run on the CPU
// emulator; VM code and Jack run on the VM emulator. The line about to
// execute is highlighted in the editor, and breakpoints set in the gutter
// pause the program.

import * as fs from 'node:fs';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { resultRows } from './chipRows';
import { inspectedAddress, memoryStartForAddress, rowMismatches } from './inspectRow';
import { followsEditor } from './followEditor';
import { panelHtml } from './panelHtml';
import { ignoresCase, pathKey, samePath } from './paths';
import { Runtime } from './runtime/client';
import {
    ChipTest,
    EngineError,
    Expected,
    ExpectedImage,
    InspectedRow,
    MachineState,
    ScriptChoice,
    TestEvent,
    TestOpened,
    TestPromoted,
    TestStatus,
    isError,
} from './runtime/protocol';
import { TestExplorer, rankScriptsForFile, scriptsForFile } from './testing';
import { debugScript } from './debug';
import { compareFilesForScript, rankExpectedImages } from './testLinks';
import { parseCompare, rawCells } from './values';
import type { ComputerCommand, FromComputer, ToComputer } from './webview/computer';

/** Memory cells shown at a time. */
const WINDOW = 16;

/** Typing or disk updates settle this long before re-reading expected files. */
const EXPECTED_DELAY_MS = 250;

/** Rows of the compare file shown; the rest are counted, not listed. */
const EXPECTED_ROWS = 256;

/** Reference screen images and preview text files larger than this are skipped. */
const MAX_IMAGE_BYTES = 512 * 1024;
const MAX_PREVIEW_TEXT_BYTES = 512 * 1024;

export const isProgram = (uri: vscode.Uri): boolean => /\.(asm|hack|vm|jack)$/i.test(uri.fsPath);
const machineKind = (uri: vscode.Uri) => /\.(vm|jack)$/i.test(uri.fsPath) ? 'vm' : 'cpu';
const panelTitle = (uri: vscode.Uri) => `${path.basename(uri.fsPath)} · ${machineKind(uri) === 'vm' ? 'VM emulator' : 'Hack computer'}`;

/**
 * Reads at most `maxBytes` from a regular file on disk, verifying `fstat`
 * (`isFile()`) and bounding actual bytes read so special files (FIFOs/devices)
 * or files growing concurrently cannot hang or exhaust memory.
 */
async function readBoundedBytes(filePath: string, maxBytes: number): Promise<Buffer | null> {
    let handle: fs.promises.FileHandle | undefined;
    try {
        // Check lstat/stat before opening so a FIFO never blocks on `open()`.
        const pre = await fs.promises.stat(filePath);
        if (!pre.isFile() || pre.size > maxBytes) {
            return null;
        }
        handle = await fs.promises.open(filePath, 'r');
        const st = await handle.stat();
        if (!st.isFile() || st.size > maxBytes) {
            return null;
        }
        const chunks: Buffer[] = [];
        let total = 0;
        const buf = Buffer.allocUnsafe(Math.min(64 * 1024, maxBytes + 1));
        for (;;) {
            const want = Math.min(buf.length, maxBytes + 1 - total);
            const { bytesRead } = await handle.read(buf, 0, want, null);
            if (bytesRead === 0) {
                break;
            }
            total += bytesRead;
            if (total > maxBytes) {
                return null;
            }
            chunks.push(Buffer.from(buf.subarray(0, bytesRead)));
        }
        return Buffer.concat(chunks, total);
    } catch {
        return null;
    } finally {
        await handle?.close().catch(() => {});
    }
}

async function readBoundedText(filePath: string, maxBytes = MAX_PREVIEW_TEXT_BYTES): Promise<string | null> {
    const open = vscode.workspace.textDocuments.find((doc) => doc.uri.scheme === 'file' && samePath(doc.uri.fsPath, filePath));
    if (open) {
        const text = open.getText();
        return Buffer.byteLength(text, 'utf8') <= maxBytes ? text : null;
    }
    const bytes = await readBoundedBytes(filePath, maxBytes);
    return bytes ? bytes.toString('utf8') : null;
}

async function findProgramExpected(
    program: vscode.Uri,
    tests: TestExplorer,
    selectedScriptPath: string | null = null,
): Promise<{ scripts: ScriptChoice[]; script: vscode.Uri | null; expected: Expected | null; expectedImage: ExpectedImage | null }> {
    const ranked = await rankScriptsForFile(await scriptsForFile(program), program, tests);
    const choices: ScriptChoice[] = ranked.map((uri) => ({ name: path.basename(uri.fsPath), path: uri.fsPath }));
    // The script the student picked goes first while it still tests this program.
    const picked = selectedScriptPath ? ranked.find((uri) => samePath(uri.fsPath, selectedScriptPath)) : undefined;
    const scripts = picked ? [picked, ...ranked.filter((uri) => uri !== picked)] : ranked;
    let script: vscode.Uri | null = null;
    let expected: Expected | null = null;
    if (picked) {
        script = picked;
        const text = await readBoundedText(picked.fsPath);
        const { comparePath } = text === null ? { comparePath: null } : compareFilesForScript(text, picked.fsPath);
        const compareText = comparePath ? await readBoundedText(comparePath) : null;
        const parsed = compareText === null ? null : parseCompare(compareText, EXPECTED_ROWS);
        if (parsed && comparePath) {
            expected = { path: comparePath, name: path.basename(comparePath), ...parsed };
        }
    }
    for (const candidate of picked ? [] : scripts) {
        const text = await readBoundedText(candidate.fsPath);
        if (text === null) {
            continue;
        }
        const { comparePath, explicitCompare } = compareFilesForScript(text, candidate.fsPath);
        const compareText = comparePath ? await readBoundedText(comparePath) : null;
        const parsed = compareText === null ? null : parseCompare(compareText, EXPECTED_ROWS);
        if (parsed && comparePath) {
            script = candidate;
            expected = { path: comparePath, name: path.basename(comparePath), ...parsed };
            break;
        }
        if (!script && !explicitCompare) {
            script = candidate;
        }
    }
    if (!script && scripts.length > 0) {
        script = scripts[0];
    }

    let expectedImage: ExpectedImage | null = null;
    const dir = path.dirname(program.fsPath);
    try {
        const entries = await fs.promises.readdir(dir);
        const preferredStems = [
            script ? path.basename(script.fsPath, path.extname(script.fsPath)) : '',
            path.basename(program.fsPath, path.extname(program.fsPath)),
        ].filter(Boolean);
        for (const imageName of rankExpectedImages(entries, preferredStems)) {
            const imagePath = path.join(dir, imageName);
            const bytes = await readBoundedBytes(imagePath, MAX_IMAGE_BYTES);
            if (!bytes) {
                continue;
            }
            const mime = /\.png$/i.test(imageName) ? 'image/png' : 'image/gif';
            expectedImage = {
                path: imagePath,
                name: imageName,
                dataUrl: `data:${mime};base64,${bytes.toString('base64')}`,
            };
            break;
        }
    } catch {
        // Unreadable folder: show no reference image.
    }

    return { scripts: choices, script, expected, expectedImage };
}

class ComputerPanel {
    private session: number | undefined;
    private state: MachineState | null = null;
    /** The last screen received: states carry the screen only when it changed. */
    private lastScreen: string | null = null;
    private problem: string | null = null;
    private expected: Expected | null = null;
    private expectedImage: ExpectedImage | null = null;
    private expectedVersion = 0;
    private sentExpectedVersion = -1;
    private testVersion = 0;
    private sentTestVersion = -1;
    private imageVersion = 0;
    private sentImageVersion = -1;
    private lastTestJson = '';
    private script: vscode.Uri | null = null;
    /** Every script that tests the program, best first. */
    private scripts: ScriptChoice[] = [];
    /** The script the student picked in the panel, kept while it still tests the program. */
    private selectedScriptPath: string | null = null;
    /** The compare row the machine was jumped to, until a reset. */
    private inspected: InspectedRow | null = null;
    private inspectCts: vscode.CancellationTokenSource | undefined;
    /** The file actually loaded by a promoted CPU session, for breakpoints. */
    private loadedFile: string | null = null;
    private expectedTimer: NodeJS.Timeout | undefined;
    private watcher: vscode.FileSystemWatcher | undefined;
    private speed = 0;
    private memoryStart = 0;
    /** A fresh machine can be reloaded until the student runs it or changes its state. */
    private fresh = false;
    private running: { stop(): void; done: Promise<unknown> } | undefined;
    private disposed = false;
    private readonly disposables: vscode.Disposable[] = [];
    private queue: Promise<void> = Promise.resolve();
    private target = 0;
    private readonly revealing = new Set<vscode.Uri>();
    private readonly shown = new Set<vscode.TextEditor>();

    constructor(
        private program: vscode.Uri,
        readonly panel: vscode.WebviewPanel,
        private readonly runtime: Runtime,
        private readonly tests: TestExplorer,
        private readonly highlight: vscode.TextEditorDecorationType,
        private readonly onDispose: () => void,
    ) {
        panel.webview.onDidReceiveMessage((message: FromComputer) => {
            if (message.type === 'ready' || message.program === this.target) {
                // Cancel an in-flight test replay immediately so reset, script
                // switches, or a new row click do not wait behind a long loop.
                if (message.type === 'reset' || message.type === 'selectScript' || message.type === 'inspectRow') {
                    this.cancelInspection();
                }
                this.enqueue(() => this.handle(message));
            }
        }, null, this.disposables);
        panel.onDidDispose(() => this.dispose(), null, this.disposables);
        const expectedFile = (document: vscode.TextDocument) =>
            document.uri.scheme === 'file' &&
            /\.(cmp|tst)$/i.test(document.fileName) &&
            pathKey(path.dirname(document.uri.fsPath)) === pathKey(path.dirname(this.program.fsPath));
        this.disposables.push(
            this.runtime.onDidStop(() => {
                if (this.disposed) {
                    return;
                }
                this.running?.stop();
                this.running = undefined;
                this.session = undefined;
                this.loadedFile = null;
                this.fresh = false;
                this.endInspection();
                this.state = null;
                this.lastScreen = null;
                this.problem = 'The simulator stopped and lost this session. Press Reset to load the program again.';
                this.post();
            }),
            this.tests.onDidChangeStatus((changedScript) => {
                if (changedScript && (!this.script || !samePath(changedScript.fsPath, this.script.fsPath))) {
                    return;
                }
                const nextJson = JSON.stringify(this.testState());
                if (nextJson === this.lastTestJson) {
                    return;
                }
                this.lastTestJson = nextJson;
                this.testVersion++;
                this.post();
            }),
            vscode.window.onDidChangeVisibleTextEditors(() => this.showLine()),
            vscode.workspace.onDidChangeTextDocument((event) => {
                if (expectedFile(event.document) && event.contentChanges.length > 0) {
                    this.scheduleExpected();
                }
            }),
            vscode.workspace.onDidSaveTextDocument((document) => {
                const uri = document.uri;
                if (expectedFile(document)) {
                    this.scheduleExpected();
                }
                if (uri.scheme === 'file' && /\.asm$/i.test(uri.fsPath) && samePath(uri.fsPath, this.program.fsPath)) {
                    this.enqueue(async () => {
                        if (this.fresh) {
                            await this.reload();
                        }
                    });
                }
            }),
        );
        this.watchFolder();
    }

    get uri(): vscode.Uri {
        return this.program;
    }

    private get kind(): 'cpu' | 'vm' {
        return this.state?.kind ?? machineKind(this.program);
    }

    private get programKind(): 'cpu' | 'vm' {
        return machineKind(this.program);
    }

    isRevealing(uri: vscode.Uri): boolean {
        return [...this.revealing].some((source) => samePath(source.fsPath, uri.fsPath));
    }

    /** Retargets the existing webview, invalidating old frames and queued actions. */
    showProgram(uri: vscode.Uri): Promise<void> {
        if (this.disposed || samePath(uri.fsPath, this.program.fsPath)) {
            return this.settled();
        }
        clearTimeout(this.expectedTimer);
        this.target++;
        this.program = uri;
        this.fresh = false;
        this.state = null;
        this.lastScreen = null;
        this.problem = null;
        this.expected = null;
        this.expectedImage = null;
        this.expectedVersion++;
        this.sentExpectedVersion = -1;
        this.testVersion++;
        this.sentTestVersion = -1;
        this.imageVersion++;
        this.sentImageVersion = -1;
        this.lastTestJson = '';
        this.script = null;
        this.scripts = [];
        this.selectedScriptPath = null;
        this.loadedFile = null;
        this.endInspection();
        this.running?.stop();
        this.watchFolder();
        this.panel.title = panelTitle(uri);
        this.post();
        return this.load();
    }

    private watchFolder(): void {
        this.watcher?.dispose();
        const watcher = vscode.workspace.createFileSystemWatcher(
            new vscode.RelativePattern(vscode.Uri.file(path.dirname(this.program.fsPath)), '*.{cmp,tst,gif,png,CMP,TST,GIF,PNG}'),
        );
        const changed = () => this.scheduleExpected();
        watcher.onDidChange(changed);
        watcher.onDidCreate(changed);
        watcher.onDidDelete(changed);
        this.watcher = watcher;
    }

    private scheduleExpected(): void {
        clearTimeout(this.expectedTimer);
        this.expectedTimer = setTimeout(
            () =>
                this.enqueue(async () => {
                    await this.readExpected();
                    this.post();
                }),
            EXPECTED_DELAY_MS,
        );
    }

    private async readExpected(): Promise<void> {
        const target = this.target;
        const found = await findProgramExpected(this.program, this.tests, this.selectedScriptPath);
        if (!this.isCurrent(target)) {
            return;
        }
        if (this.inspected && (!found.script || !samePath(found.script.fsPath, this.inspected.scriptPath))) {
            this.inspected = null;
        }
        this.scripts = found.scripts;
        const imageChanged =
            this.expectedImage?.path !== found.expectedImage?.path
            || this.expectedImage?.dataUrl !== found.expectedImage?.dataUrl;
        this.script = found.script;
        this.expected = found.expected;
        this.expectedImage = found.expectedImage;
        this.expectedVersion++;
        this.testVersion++;
        this.lastTestJson = '';
        if (imageChanged) {
            this.imageVersion++;
        }
    }

    private testState(): ChipTest | null {
        if (!this.script) {
            return null;
        }
        const { status, result } = this.tests.statusOf(this.script, this.program);
        const test: ChipTest = {
            script: path.basename(this.script.fsPath),
            scriptPath: this.script.fsPath,
            status,
        };
        if (!result) {
            return test;
        }
        test.kind = result.kind;
        test.comparedLines = result.comparedLines;
        test.failureCount = result.failureCount;
        if (result.inputs && result.inputs.length > 0) {
            const names = result.inputs.map((input) => path.basename(input.path));
            const anyBuilt = result.inputs.some((input) => input.builtFromSource);
            const allBuilt = result.inputs.every((input) => input.builtFromSource);
            if (result.inputs.length === 1) {
                test.sourceLabel = anyBuilt
                    ? `${/\.asm$/i.test(names[0]) ? 'assembled' : 'compiled'} ${names[0]}`
                    : names[0];
            } else if (allBuilt) {
                test.sourceLabel = `compiled ${names.length} files`;
            } else if (anyBuilt) {
                const built = result.inputs.filter((i) => i.builtFromSource).map((i) => path.basename(i.path));
                test.sourceLabel = `mixed (${built.join(', ')} in memory)`;
            } else {
                test.sourceLabel = `${names.length} VM files`;
            }
        }
        if (result.message !== undefined) {
            test.message = result.message;
        }
        if (result.rows !== undefined) {
            test.comparedRows = result.rows;
            test.differingRows = result.differingRows ?? 0;
        }
        if (result.kind === 'failed' && result.failures.length > 0) {
            const first = result.failures[0];
            test.firstFailureLine = first.line;
            if (first.line === 1) {
                test.firstFailureReason = 'header line 1 differs';
            } else if (this.expected && this.expected.more === 0 && first.line > Math.max(1, ...this.expected.lines)) {
                test.firstFailureReason = `extra output at line ${first.line} (past end of ${this.expected.name})`;
            } else {
                test.firstFailureReason = `line ${first.line} differs`;
            }
        }
        if (this.expected && (result.kind === 'passed' || result.kind === 'failed')) {
            Object.assign(test, resultRows(this.expected.lines, result));
        }
        return test;
    }

    private isCurrent(target: number): boolean {
        return !this.disposed && this.target === target;
    }

    private enqueue(work: () => Promise<void>): void {
        const target = this.target;
        this.queue = this.queue
            .then(() => (this.isCurrent(target) ? work() : undefined))
            .catch((error) => {
                if (this.isCurrent(target)) {
                    this.problem = error instanceof Error ? error.message : String(error);
                    this.post();
                }
            });
    }

    private get window(): [number, number] {
        return [this.memoryStart, WINDOW];
    }

    private post(force = false): void {
        if (this.disposed) {
            return;
        }
        const includeExpected = force || this.expectedVersion !== this.sentExpectedVersion;
        if (includeExpected) {
            this.sentExpectedVersion = this.expectedVersion;
        }
        const includeTest = force || this.testVersion !== this.sentTestVersion;
        const testPayload = includeTest ? this.testState() : undefined;
        if (includeTest) {
            this.sentTestVersion = this.testVersion;
            this.lastTestJson = JSON.stringify(testPayload);
        }
        const includeImage = force || this.imageVersion !== this.sentImageVersion;
        if (includeImage) {
            this.sentImageVersion = this.imageVersion;
        }
        const message: ToComputer = {
            type: 'state',
            target: this.target,
            state: this.state,
            running: this.state !== null && this.running !== undefined,
            problem: this.problem,
            title: path.basename(this.program.fsPath),
            ...(includeExpected ? { expected: this.expected, expectedVersion: this.expectedVersion } : {}),
            ...(includeImage ? { expectedImage: this.expectedImage } : {}),
            ...(includeTest ? { test: testPayload } : {}),
            scripts: this.scripts,
            inspected: this.inspected,
        };
        void this.panel.webview.postMessage(message);
        this.showLine();
    }

    /** Marks the line about to execute in the editor, when paused. Scrolls only when `scroll` is set. */
    private showLine(scroll = false): void {
        const paused = !this.running && !this.disposed;
        const line = paused ? (this.state?.line ?? null) : null;
        const file = this.state?.file ? pathKey(this.state.file) : undefined;
        for (const editor of vscode.window.visibleTextEditors) {
            if (editor.document.uri.scheme !== 'file') {
                continue;
            }
            const here = file !== undefined && pathKey(editor.document.uri.fsPath) === file;
            if (!here || line === null || line >= editor.document.lineCount) {
                if (this.shown.delete(editor)) {
                    editor.setDecorations(this.highlight, []);
                }
            } else {
                const range = editor.document.lineAt(line).range;
                editor.setDecorations(this.highlight, [range]);
                if (scroll) {
                    editor.revealRange(range, vscode.TextEditorRevealType.InCenterIfOutsideViewport);
                }
                this.shown.add(editor);
            }
        }
    }

    /** Opens the file the program stopped in, if it is not on screen, and scrolls to the stopped line. */
    private async reveal(): Promise<void> {
        const target = this.target;
        const file = this.state?.file;
        if (!file || this.running || this.disposed) {
            return;
        }
        const visible = vscode.window.visibleTextEditors.some(
            (editor) => editor.document.uri.scheme === 'file' && pathKey(editor.document.uri.fsPath) === pathKey(file),
        );
        if (!visible) {
            const document = await vscode.workspace.openTextDocument(vscode.Uri.file(file));
            if (!this.isCurrent(target)) {
                return;
            }
            const beside = vscode.window.visibleTextEditors.find((editor) => editor.viewColumn !== this.panel.viewColumn);
            const source = vscode.Uri.file(file);
            this.revealing.add(source);
            try {
                await vscode.window.showTextDocument(document, {
                    viewColumn: beside?.viewColumn ?? vscode.ViewColumn.One,
                    preserveFocus: true,
                    preview: true,
                });
            } finally {
                this.revealing.delete(source);
            }
        }
        if (this.isCurrent(target)) {
            this.showLine(true);
        }
    }

    /** The enabled breakpoints, by file, as zero-based lines. */
    private breakpoints(): { path: string; lines: number[] }[] {
        const byFile = new Map<string, number[]>();
        for (const breakpoint of vscode.debug.breakpoints) {
            if (breakpoint instanceof vscode.SourceBreakpoint && breakpoint.enabled && breakpoint.location.uri.scheme === 'file') {
                const file = breakpoint.location.uri.fsPath;
                byFile.set(file, [...(byFile.get(file) ?? []), breakpoint.location.range.start.line]);
            }
        }
        return [...byFile].map(([file, lines]) => ({ path: file, lines }));
    }

    /** Zero-based lines of the enabled breakpoints in the program's file. */
    private breakLines(): number[] {
        const here = pathKey(this.loadedFile ?? this.uri.fsPath);
        return this.breakpoints()
            .filter((entry) => pathKey(entry.path) === here)
            .flatMap((entry) => entry.lines);
    }

    private accept(reply: MachineState | EngineError): boolean {
        if (isError(reply)) {
            this.problem = reply.error.message;
            return false;
        }
        this.take(reply);
        this.problem = reply.status === 'error' ? reply.error : null;
        return true;
    }

    private take(state: MachineState): void {
        this.state = state;
        if (state.screen) {
            this.lastScreen = state.screen;
        }
    }

    /**
     * Loads the program, after anything already asked of the panel. Loading
     * goes through the queue: two loads at once would open two sessions and
     * lose track of one.
     */
    load(): Promise<void> {
        this.enqueue(() => this.reload());
        return this.queue;
    }

    /** Loads the program, or loads it again from the editor's current text. */
    private async reload(): Promise<void> {
        const target = this.target;
        this.endInspection();
        this.loadedFile = null;
        await this.stop();
        if (!this.isCurrent(target)) {
            return;
        }
        this.fresh = true;
        if (this.session !== undefined) {
            this.runtime.close(this.session);
            this.session = undefined;
        }
        await this.runtime.syncUnsaved();
        if (!this.isCurrent(target)) {
            return;
        }
        const [reply] = await Promise.all([
            this.runtime.tryCall<MachineState>({
                op: `${this.programKind}.open`,
                path: this.uri.fsPath,
                ignoreCase: ignoresCase(this.uri.fsPath),
                ram: this.window,
                screen: true,
            }),
            this.readExpected(),
        ]);
        if (!this.isCurrent(target)) {
            // The panel was closed or switched while this session was opening.
            if (!isError(reply) && reply.session !== undefined) {
                this.runtime.close(reply.session);
            }
            return;
        }
        if (this.accept(reply) && !isError(reply)) {
            this.session = reply.session;
        } else {
            this.state = null;
            this.lastScreen = null;
        }
        this.post();
    }

    private async stop(): Promise<void> {
        const running = this.running;
        if (running) {
            running.stop();
            await running.done;
        }
    }

    private start(): void {
        if (this.session === undefined || this.running || this.state?.status === 'error' || this.state?.status === 'halted') {
            return;
        }
        this.fresh = false;
        this.leaveSnapshot();
        const target = this.target;
        const run = this.runtime.runMachine(
            {
                kind: this.kind,
                session: this.session,
                speed: this.speed,
                breakLines: this.breakLines(),
                breakpoints: this.breakpoints(),
                ram: this.window,
            },
            (frame) => {
                if (this.isCurrent(target) && this.running === handle) {
                    this.take(frame);
                    this.post();
                }
            },
        );
        const handle = {
            stop: run.stop,
            done: run.done.then((final) => {
                if (this.running === handle) {
                    this.running = undefined;
                }
                if (this.isCurrent(target)) {
                    this.accept(final);
                    this.post();
                    void this.reveal().catch(() => {});
                }
            }),
        };
        this.running = handle;
        this.post();
    }

    private async call(request: Record<string, unknown>): Promise<void> {
        if (this.session === undefined) {
            return;
        }
        const target = this.target;
        const reply = await this.runtime.tryCall<MachineState>({
            screen: true,
            ...request,
            op: `${this.kind}.${request.op}`,
            session: this.session,
            ram: this.window,
            now: Math.floor(performance.now()),
        });
        if (!this.isCurrent(target)) {
            return;
        }
        if (isError(reply) && request.op === 'set') {
            const rejected: ToComputer = { type: 'rejected', message: reply.error.message, target };
            void this.panel.webview.postMessage(rejected);
            return;
        }
        if (request.op === 'step' || request.op === 'set') {
            this.fresh = false;
            this.leaveSnapshot();
        }
        this.accept(isError(reply) || request.op !== 'state' ? reply : this.refreshed(reply));
        this.post();
    }

    /**
     * A reply to a pure refresh (`state`) only reports what is in the machine:
     * the engine does not remember why a program is stopped, so it always
     * says `paused`. It must not change that. While a run is in progress the
     * run's own frames own the status, so only the memory window and screen
     * are taken; when nothing has executed since the state the panel already
     * has, the status and error text stay (a breakpoint stays a breakpoint).
     * `halted` and `error` replies are never downgraded, as they are taken as
     * they come.
     */
    private refreshed(reply: MachineState): MachineState {
        const before = this.state;
        if (!before) {
            return reply;
        }
        if (this.running) {
            return { ...before, ram: reply.ram, screen: reply.screen };
        }
        if (reply.status === 'paused' && before.steps === reply.steps && before.status !== 'running') {
            return { ...reply, status: before.status, error: before.error };
        }
        return reply;
    }

    private async openExpected(): Promise<void> {
        if (!this.expected) {
            return;
        }
        const here = this.panel.viewColumn;
        const other = vscode.window.tabGroups.all.find((group) => group.viewColumn !== here)?.viewColumn;
        await vscode.window.showTextDocument(vscode.Uri.file(this.expected.path), {
            viewColumn: other ?? vscode.ViewColumn.Beside,
            preserveFocus: true,
            preview: true,
        });
    }

    private async openExpectedImage(): Promise<void> {
        if (!this.expectedImage) {
            return;
        }
        const here = this.panel.viewColumn;
        const other = vscode.window.tabGroups.all.find((group) => group.viewColumn !== here)?.viewColumn;
        await vscode.commands.executeCommand(
            'vscode.open',
            vscode.Uri.file(this.expectedImage.path),
            { viewColumn: other ?? vscode.ViewColumn.Beside, preserveFocus: true, preview: true },
        );
    }

    /** Cancels an in-progress row inspection without clearing an active snapshot banner. */
    private cancelInspection(): void {
        this.inspectCts?.cancel();
        this.inspectCts?.dispose();
        this.inspectCts = undefined;
    }

    /** Forgets the inspected row and stops an inspection in progress. */
    private endInspection(): void {
        this.cancelInspection();
        this.inspected = null;
    }

    /** The machine moved on from the inspected snapshot: its mismatches no longer describe it. */
    private leaveSnapshot(): void {
        if (this.inspected && !this.inspected.stepped) {
            this.inspected = { ...this.inspected, stepped: true };
        }
    }

    private async selectScript(scriptPath: string): Promise<void> {
        const choice = this.scripts.find((script) => samePath(script.path, scriptPath));
        if (!choice || (this.script && samePath(this.script.fsPath, choice.path))) {
            return;
        }
        const target = this.target;
        this.selectedScriptPath = choice.path;
        // Stop any in-flight replay, but keep an already-active snapshot banner
        // visible until Reset or a new inspection replaces the live machine.
        this.cancelInspection();
        await this.readExpected();
        if (this.isCurrent(target)) {
            this.post();
        }
    }

    /**
     * Runs the test script up to compare line `line` (without writing its
     * `.out` file), then makes that test's machine the panel's machine, so
     * the student sees the registers, memory and screen at that row.
     */
    private async inspectRow(line: number, rowIndex: number, scriptPath?: string, expectedVersion?: number): Promise<void> {
        const script = this.script;
        const expected = this.expected;
        const capturedExpectedVersion = this.expectedVersion;
        if (
            !script
            || !expected
            || (scriptPath && !samePath(scriptPath, script.fsPath))
            || (expectedVersion !== undefined && expectedVersion !== capturedExpectedVersion)
            || rowIndex < 0
            || rowIndex >= expected.lines.length
            || expected.lines[rowIndex] !== line
        ) {
            return;
        }
        const target = this.target;
        const reject = (text: string) => {
            if (this.isCurrent(target)) {
                const rejected: ToComputer = { type: 'rejected', target, message: text };
                void this.panel.webview.postMessage(rejected);
            }
        };
        await this.stop();
        if (!this.isCurrent(target) || this.expectedVersion !== capturedExpectedVersion) {
            return;
        }
        this.cancelInspection();
        const cts = new vscode.CancellationTokenSource();
        this.inspectCts = cts;
        const stale = () =>
            !this.isCurrent(target)
            || this.expectedVersion !== capturedExpectedVersion
            || cts.token.isCancellationRequested;

        let openedSession: number | undefined;
        let promoted = false;
        try {
            const overlay = this.runtime.captureUnsaved();
            const openOptions = await this.tests.openOptionsFor(script, [this.program], overlay);
            if (stale()) {
                return;
            }
            const opened = await this.runtime.tryCall<TestOpened>({
                op: 'test.open',
                path: script.fsPath,
                overlay,
                ...openOptions,
                collectAll: true,
                writeOutput: false,
                stopAtLine: line,
            });
            if (isError(opened)) {
                if (!stale()) {
                    reject(`Could not run ${path.basename(script.fsPath)} to that row: ${opened.error.message}`);
                }
                return;
            }
            openedSession = opened.session;
            if (stale()) {
                return;
            }
            let lastOutput: string | undefined;
            const onEvents = (events: TestEvent[]) => {
                for (const event of events) {
                    if (event.type === 'output') {
                        lastOutput = event.line;
                    }
                }
            };
            let status: TestStatus;
            const failures: TestStatus['failures'] = [];
            try {
                for (;;) {
                    status = await this.runtime.runTest(openedSession, {}, onEvents, cts.token);
                    failures.push(...status.failures);
                    // A `!` pause or a breakpoint stops a run too: carry on to the row.
                    if (status.state === 'finished' || status.reason === 'targetLine') {
                        break;
                    }
                }
            } catch (error) {
                if (!stale()) {
                    reject(`Could not run ${path.basename(script.fsPath)} to that row: ${error instanceof Error ? error.message : String(error)}`);
                }
                return;
            }
            if (stale()) {
                return;
            }
            if (status.state !== 'suspended' || status.reason !== 'targetLine') {
                const outcome = status.outcome;
                const why =
                    outcome?.kind === 'error'
                        ? `it stopped with an error: ${outcome.message}`
                        : outcome?.kind === 'limit'
                          ? 'it ran into the command limit'
                          : outcome?.kind === 'cancelled'
                            ? 'it was cancelled'
                            : 'the script ended first';
                reject(`${path.basename(script.fsPath)} did not reach line ${line} of ${expected.name}: ${why}.`);
                return;
            }

            // The engine stopped right after writing the target compare line, so
            // the last output event of this run is always that exact row (even
            // when the compare file contains blank lines).
            const produced = lastOutput !== undefined ? rawCells(lastOutput) : this.testState()?.actual?.[rowIndex];
            const mismatches = rowMismatches(expected, produced, rowIndex);
            const differs = failures.some((f) => f.line === line) || mismatches.length > 0;
            const address = inspectedAddress(expected, mismatches);
            if (address !== null) {
                this.memoryStart = memoryStartForAddress(address, WINDOW);
            }

            const reply = await this.runtime.tryCall<TestPromoted>({
                op: 'test.promote',
                session: openedSession,
                path: this.program.fsPath,
                ignoreCase: ignoresCase(this.program.fsPath),
                ram: this.window,
                screen: 'force',
            });
            if (isError(reply)) {
                if (!stale()) {
                    reject(`Could not show the machine at that row: ${reply.error.message}`);
                }
                return;
            }
            // The test session became the machine's session: never close it as a test.
            promoted = true;
            if (stale()) {
                this.runtime.close(reply.session);
                return;
            }
            if (this.session !== undefined && this.session !== reply.session) {
                this.runtime.close(this.session);
            }
            this.session = reply.session;
            this.loadedFile = reply.loaded?.[0]?.path ?? reply.state.file ?? null;
            this.fresh = false;
            this.take(reply.state);
            this.problem = reply.state.status === 'error' ? reply.state.error : null;
            this.inspected = {
                script: path.basename(script.fsPath),
                scriptPath: script.fsPath,
                scriptLine: reply.scriptRange
                    ? reply.scriptRange.startLine + 1
                    : status.at
                      ? status.at.startLine + 1
                      : null,
                rowIndex,
                compareLine: line,
                stepped: false,
                differs,
                mismatches,
            };
            this.post();
            await this.reveal();
        } finally {
            if (openedSession !== undefined && !promoted) {
                this.runtime.close(openedSession);
            }
            if (this.inspectCts === cts) {
                this.inspectCts = undefined;
            }
            cts.dispose();
        }
    }

    /** Resolves a script path the panel is currently showing or inspecting. */
    private allowedScript(scriptPath: string): vscode.Uri | null {
        if (this.script && samePath(scriptPath, this.script.fsPath)) {
            return this.script;
        }
        if (this.inspected && samePath(scriptPath, this.inspected.scriptPath)) {
            return vscode.Uri.file(this.inspected.scriptPath);
        }
        return null;
    }

    /** Opens a script beside the panel with 1-based `line` selected. */
    private async openScriptAt(scriptPath: string, line: number): Promise<void> {
        const script = this.allowedScript(scriptPath);
        if (!script) {
            return;
        }
        const document = await vscode.workspace.openTextDocument(script);
        const zeroBased = Math.max(0, Math.min(document.lineCount - 1, line - 1));
        const here = this.panel.viewColumn;
        const other = vscode.window.tabGroups.all.find((group) => group.viewColumn !== here)?.viewColumn;
        const range = document.lineAt(zeroBased).range;
        await vscode.window.showTextDocument(document, {
            viewColumn: other ?? vscode.ViewColumn.Beside,
            preserveFocus: false,
            preview: true,
            selection: new vscode.Range(range.start, range.end),
        });
    }

    private async handle(message: ComputerCommand): Promise<void> {
        const target = this.target;
        switch (message.type) {
            case 'ready':
                this.memoryStart = message.memoryStart ?? this.memoryStart;
                this.speed = message.speed ?? this.speed;
                this.sentExpectedVersion = -1;
                this.sentTestVersion = -1;
                this.sentImageVersion = -1;
                if (this.session !== undefined) {
                    // A fresh page for a program already loaded: send it everything.
                    await this.call({ op: 'state', screen: 'force' });
                } else {
                    this.post(true);
                }
                return;
            case 'run':
                this.start();
                return;
            case 'pause':
                await this.stop();
                return;
            case 'step':
                await this.stop();
                if (this.isCurrent(target)) {
                    await this.call({ op: 'step', unit: message.unit ?? 'command' });
                }
                if (this.isCurrent(target)) {
                    await this.reveal();
                }
                return;
            case 'reset':
                await this.reload();
                if (this.isCurrent(target)) {
                    await this.reveal();
                }
                return;
            case 'speed': {
                this.speed = message.speed;
                if (this.running) {
                    await this.stop();
                    if (this.isCurrent(target)) {
                        this.start();
                    }
                }
                return;
            }
            case 'key':
                if (this.session !== undefined) {
                    if (message.code !== 0) {
                        this.fresh = false;
                    }
                    await this.runtime.tryCall({ op: 'key', session: this.session, code: message.code });
                }
                return;
            case 'memory': {
                this.memoryStart = message.start;
                if (this.running) {
                    await this.stop();
                    if (this.isCurrent(target)) {
                        this.start();
                    }
                } else {
                    await this.call({ op: 'state' });
                }
                return;
            }
            case 'set':
                await this.call({ op: 'set', target: message.target, address: message.address, value: message.value });
                return;
            case 'runTest':
                if (this.script && (!message.script || samePath(message.script, this.script.fsPath))) {
                    void this.tests.runScripts([this.script], { quiet: true, source: this.program });
                }
                return;
            case 'openExpected':
                await this.openExpected();
                return;
            case 'openExpectedImage':
                await this.openExpectedImage();
                return;
            case 'selectScript':
                await this.selectScript(message.script);
                return;
            case 'inspectRow':
                await this.inspectRow(message.line, message.rowIndex, message.script, message.expectedVersion);
                return;
            case 'openScriptAt':
                await this.openScriptAt(message.script, message.line);
                return;
            case 'debugScript': {
                const script = this.allowedScript(message.script);
                if (script) {
                    await debugScript(script, this.program);
                }
                return;
            }
            case 'compareOutput':
                if (this.script && (!message.script || samePath(message.script, this.script.fsPath))) {
                    await vscode.commands.executeCommand('nandwright.compareOutput', {
                        script: this.script,
                        forFile: this.program,
                    });
                }
                return;
        }
    }

    /** For tests: what the panel shows now. */
    snapshot(): {
        state: MachineState | null;
        running: boolean;
        problem: string | null;
        expected: Expected | null;
        expectedImage: ExpectedImage | null;
        test: ChipTest | null;
        scripts: ScriptChoice[];
        inspected: InspectedRow | null;
    } {
        const state = this.state ? { ...this.state, screen: this.lastScreen } : null;
        return {
            state,
            running: this.state !== null && this.running !== undefined,
            problem: this.problem,
            expected: this.expected,
            expectedImage: this.expectedImage,
            test: this.testState(),
            scripts: this.scripts,
            inspected: this.inspected,
        };
    }

    /** For tests: handles a message as if the panel had sent it. */
    async simulate(message: ComputerCommand): Promise<void> {
        this.enqueue(() => this.handle(message));
        await this.queue;
    }

    async settled(): Promise<void> {
        await this.queue;
    }

    dispose(): void {
        if (this.disposed) {
            return;
        }
        this.running?.stop();
        this.running = undefined;
        this.disposed = true;
        this.endInspection();
        clearTimeout(this.expectedTimer);
        this.watcher?.dispose();
        for (const editor of this.shown) {
            editor.setDecorations(this.highlight, []);
        }
        this.shown.clear();
        if (this.session !== undefined) {
            this.runtime.close(this.session);
            this.session = undefined;
        }
        this.disposables.forEach((d) => d.dispose());
        this.onDispose();
    }
}

export class ComputerPanels implements vscode.Disposable {
    private readonly panels = new Set<ComputerPanel>();
    private readonly subscription: vscode.Disposable;
    private readonly highlight = vscode.window.createTextEditorDecorationType({
        isWholeLine: true,
        backgroundColor: new vscode.ThemeColor('editor.stackFrameHighlightBackground'),
        overviewRulerColor: new vscode.ThemeColor('editorOverviewRuler.findMatchForeground'),
        overviewRulerLane: vscode.OverviewRulerLane.Full,
    });

    constructor(
        private readonly extensionUri: vscode.Uri,
        private readonly runtime: Runtime,
        private readonly tests: TestExplorer,
    ) {
        this.subscription = vscode.window.onDidChangeActiveTextEditor((editor) => {
            const uri = editor?.document.uri;
            if (!uri || uri.scheme !== 'file' || !isProgram(uri) || !followsEditor()) {
                return;
            }
            if (!this.get(uri) && ![...this.panels].some((panel) => panel.isRevealing(uri))) {
                void this.followed()?.showProgram(uri);
            }
        });
    }

    private followed(): ComputerPanel | undefined {
        return [...this.panels].pop();
    }

    /** Opens (or reveals) the computer for a program: `.asm`, `.hack`, `.vm` or `.jack`. */
    async open(uri: vscode.Uri): Promise<ComputerPanel> {
        let existing = this.get(uri);
        if (!existing && followsEditor()) {
            existing = this.followed();
            void existing?.showProgram(uri);
        }
        if (existing) {
            existing.panel.reveal(undefined, true);
            await existing.settled();
            return existing;
        }
        const name = path.basename(uri.fsPath);
        const panel = vscode.window.createWebviewPanel(
            'nandwright.computer',
            panelTitle(uri),
            { viewColumn: vscode.ViewColumn.Beside, preserveFocus: true },
            {
                enableScripts: true,
                // The program keeps running, and the screen keeps its picture,
                // while the student looks at another tab.
                retainContextWhenHidden: true,
                localResourceRoots: [vscode.Uri.joinPath(this.extensionUri, 'dist', 'webview')],
            },
        );
        panel.webview.html = panelHtml(panel.webview, this.extensionUri, 'computer.js', `${name} running`, COMPUTER_STYLE);
        const computer: ComputerPanel = new ComputerPanel(uri, panel, this.runtime, this.tests, this.highlight, () => this.panels.delete(computer));
        this.panels.add(computer);
        await computer.load();
        return computer;
    }

    get(uri: vscode.Uri): ComputerPanel | undefined {
        return [...this.panels].find((panel) => samePath(panel.uri.fsPath, uri.fsPath));
    }

    dispose(): void {
        this.subscription.dispose();
        for (const panel of this.panels) {
            panel.panel.dispose();
        }
        this.highlight.dispose();
    }
}

const COMPUTER_STYLE = `
.status { margin: 6px 0 10px; min-height: 1.45em; font-family: var(--vscode-editor-font-family); color: var(--vscode-descriptionForeground); }
.screen { display: block; width: 100%; max-width: 512px; line-height: 0; border: 1px solid var(--vscode-contrastBorder, var(--vscode-widget-border, var(--vscode-descriptionForeground))); background: #fff; cursor: pointer; }
.screen:focus { outline: 2px solid var(--vscode-focusBorder); outline-offset: 2px; }
.screen canvas { display: block; width: 100%; height: auto; image-rendering: pixelated; }
.keyboard { margin-top: 6px; }
.registers { display: grid; grid-template-columns: repeat(auto-fill, minmax(150px, 1fr)); gap: 0 18px; border-top: 1px solid var(--line); }
.register { display: flex; align-items: center; gap: 10px; min-height: 32px; border-bottom: 1px solid var(--line); }
.register-name { flex: 0 0 26px; font-family: var(--vscode-editor-font-family); font-weight: 600; }
.cell { width: 100%; min-width: 0; padding: 2px 6px; font-family: var(--vscode-editor-font-family); font-variant-numeric: tabular-nums; color: var(--vscode-input-foreground); background: transparent; border: 1px solid transparent; border-radius: 2px; }
.cell:hover:not(:disabled) { border-color: var(--vscode-input-border, var(--vscode-contrastBorder, var(--vscode-descriptionForeground))); }
.cell:focus { background: var(--vscode-input-background); border-color: var(--vscode-focusBorder); outline: none; }
.memory-tools { margin: 2px 0 6px; }
.address-label { color: var(--vscode-descriptionForeground); }
.address { width: 72px; padding: 3px 6px; font-family: var(--vscode-editor-font-family); color: var(--vscode-input-foreground); background: var(--vscode-input-background); border: 1px solid var(--vscode-input-border, var(--vscode-contrastBorder, transparent)); border-radius: 2px; }
.link { padding: 2px 6px; border: 0; border-radius: 2px; background: transparent; color: var(--vscode-textLink-foreground); }
.link:hover { text-decoration: underline; }
.memory { width: 100%; border-collapse: collapse; font-family: var(--vscode-editor-font-family); font-variant-numeric: tabular-nums; }
.memory th, .memory td { padding: 1px 4px; border-bottom: 1px solid var(--line); text-align: left; font-weight: normal; }
.memory thead th { font-family: var(--vscode-font-family); font-size: 0.85em; color: var(--vscode-descriptionForeground); }
.memory tbody th { width: 42%; white-space: nowrap; color: var(--vscode-descriptionForeground); }
.memory .symbol { margin-left: 10px; color: var(--vscode-foreground); }
.memory tr.pointed { background: var(--vscode-list-inactiveSelectionBackground); }
.frames { margin: 0; padding: 0; list-style: none; border-top: 1px solid var(--line); font-family: var(--vscode-editor-font-family); }
.frames li { display: flex; gap: 12px; padding: 3px 4px; border-bottom: 1px solid var(--line); }
.frames li:first-child { font-weight: 600; }
.frames .where { margin-left: auto; color: var(--vscode-descriptionForeground); font-weight: normal; }
.stack { display: flex; flex-wrap: wrap; gap: 4px; margin: 0; padding: 0; list-style: none; font-family: var(--vscode-editor-font-family); font-variant-numeric: tabular-nums; }
.stack li { min-width: 44px; padding: 1px 8px; text-align: right; border: 1px solid var(--line); border-radius: 2px; }
.stack li:first-child { border-color: var(--vscode-focusBorder); }
.rejected { margin: 0 0 4px; color: var(--vscode-errorForeground); }
.rejected:empty { display: none; }
.test-run { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 10px; margin: 2px 0 8px; }
.test-run .summary { flex: 1 1 16em; min-width: 0; overflow-wrap: anywhere; }
.test-run .summary.fails { color: var(--vscode-errorForeground); }
.expected-scroll { max-height: 280px; overflow: auto; border-top: 1px solid var(--line); }
.expected { border-collapse: collapse; margin-top: 0; font-family: var(--vscode-editor-font-family); font-variant-numeric: tabular-nums; }
.expected th, .expected td { padding: 3px 10px; text-align: center; border-bottom: 1px solid var(--line); white-space: nowrap; }
.expected thead th { position: sticky; top: 0; background: var(--vscode-editor-background); font-weight: 600; color: var(--vscode-descriptionForeground); }
.expected td.differs { color: var(--vscode-errorForeground); font-weight: 600; }
.expected .got { display: block; font-size: 0.85em; font-weight: normal; color: var(--vscode-descriptionForeground); }
.expected .mark { width: 2em; padding: 3px 4px; font-family: var(--vscode-font-family); }
.expected td.mark.match { color: var(--vscode-testing-iconPassed, var(--vscode-foreground)); }
.expected td.mark.differ { color: var(--vscode-testing-iconFailed, var(--vscode-errorForeground)); font-weight: 600; }
.expected td.mark.unreached { color: var(--vscode-descriptionForeground); }
.expected-screen { display: inline-block; max-width: 512px; line-height: 0; border: 1px solid var(--vscode-contrastBorder, var(--vscode-widget-border, var(--vscode-descriptionForeground))); background: #fff; }
.expected-screen img { display: block; max-width: 100%; height: auto; image-rendering: pixelated; }
.link-inline { padding: 0; border: 0; background: none; font: inherit; color: var(--vscode-textLink-foreground); text-decoration: underline; cursor: pointer; }
.link-inline:hover { color: var(--vscode-textLink-activeForeground); }
.script-picker { display: flex; align-items: center; gap: 8px; margin: 2px 0 6px; color: var(--vscode-descriptionForeground); }
.script-picker select { max-width: 100%; }
.expected tr.inspectable { cursor: pointer; }
.expected tr.inspectable:hover { background: var(--vscode-list-hoverBackground); }
.expected tr.inspectable:focus-within { background: var(--vscode-list-hoverBackground); }
.expected .mark-button { padding: 0 2px; border: 0; background: transparent; color: inherit; font: inherit; cursor: pointer; }
.expected tr.inspectable:hover .mark-button::after, .expected .mark-button:focus-visible::after { content: ' Inspect'; font-size: 0.8em; font-weight: normal; color: var(--vscode-textLink-foreground); }
.expected tr.inspected { background: var(--vscode-list-inactiveSelectionBackground); box-shadow: inset 3px 0 0 var(--vscode-focusBorder); }
.inspect-banner { margin: 0 0 10px; padding: 8px 10px; border: 1px solid var(--vscode-focusBorder); border-left-width: 3px; border-radius: 2px; background: var(--vscode-editorWidget-background, transparent); }
.inspect-banner .inspect-title { display: block; margin-bottom: 4px; }
.inspect-banner .hint { margin: 2px 0 6px; }
.inspect-banner .toolbar { margin: 6px 0 0; }
.mismatches { display: flex; flex-wrap: wrap; gap: 6px; margin: 4px 0; padding: 0; list-style: none; }
.mismatches .chip { display: inline-block; padding: 2px 8px; font-family: var(--vscode-editor-font-family); font-size: 0.95em; color: var(--vscode-errorForeground); background: transparent; border: 1px solid var(--vscode-errorForeground); border-radius: 10px; }
.mismatches button.chip { cursor: pointer; }
.mismatches button.chip:hover { text-decoration: underline; }
.register.differs .register-name, .register.differs .cell, .memory tr.differs .cell { color: var(--vscode-errorForeground); font-weight: 600; }
.register.differs { box-shadow: inset 3px 0 0 var(--vscode-errorForeground); }
.memory tr.differs { background: var(--vscode-inputValidation-errorBackground, transparent); }
.register .got, .memory .got { flex: 0 0 auto; font-size: 0.85em; white-space: nowrap; color: var(--vscode-descriptionForeground); }
.memory td .got { display: inline; margin-left: 6px; }
.memory tr.differs .cell { width: calc(100% - 10em); }
[hidden] { display: none !important; }
`;
