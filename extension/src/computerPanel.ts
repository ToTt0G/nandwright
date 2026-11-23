// The computer panel: runs a program with its screen, keyboard, registers and
// memory, next to the source. Assembly and machine code run on the CPU
// emulator; VM code and Jack run on the VM emulator. The line about to
// execute is highlighted in the editor, and breakpoints set in the gutter
// pause the program.

import * as path from 'node:path';
import * as vscode from 'vscode';
import { panelHtml } from './panelHtml';
import { Runtime } from './runtime/client';
import { EngineError, MachineState, isError } from './runtime/protocol';
import type { FromComputer, ToComputer } from './webview/computer';

/** Memory cells shown at a time. */
const WINDOW = 16;

class ComputerPanel {
    private session: number | undefined;
    private state: MachineState | null = null;
    /** The last screen received: states carry the screen only when it changed. */
    private lastScreen: string | null = null;
    private problem: string | null = null;
    private speed = 0;
    private memoryStart = 0;
    private running: { stop(): void; done: Promise<unknown> } | undefined;
    private disposed = false;
    private readonly disposables: vscode.Disposable[] = [];
    private queue: Promise<void> = Promise.resolve();
    private readonly title: string;
    private readonly shown = new Set<vscode.TextEditor>();

    constructor(
        readonly uri: vscode.Uri,
        readonly kind: 'cpu' | 'vm',
        readonly panel: vscode.WebviewPanel,
        private readonly runtime: Runtime,
        private readonly highlight: vscode.TextEditorDecorationType,
        private readonly onDispose: () => void,
    ) {
        this.title = path.basename(uri.fsPath);
        panel.webview.onDidReceiveMessage((message: FromComputer) => this.enqueue(() => this.handle(message)), null, this.disposables);
        panel.onDidDispose(() => this.dispose(), null, this.disposables);
        this.disposables.push(vscode.window.onDidChangeVisibleTextEditors(() => this.showLine()));
    }

    private enqueue(work: () => Promise<void>): void {
        this.queue = this.queue
            .then(() => (this.disposed ? undefined : work()))
            .catch((error) => {
                this.problem = error instanceof Error ? error.message : String(error);
                this.post();
            });
    }

    private get window(): [number, number] {
        return [this.memoryStart, WINDOW];
    }

    private post(): void {
        if (this.disposed) {
            return;
        }
        const message: ToComputer = {
            type: 'state',
            state: this.state,
            running: this.running !== undefined,
            problem: this.problem,
            title: this.title,
        };
        void this.panel.webview.postMessage(message);
        this.showLine();
    }

    /** Marks the line about to execute in the editor, when paused. */
    private showLine(): void {
        const paused = !this.running && !this.disposed;
        const line = paused ? (this.state?.line ?? null) : null;
        const file = this.state?.file ? samePath(this.state.file) : undefined;
        for (const editor of vscode.window.visibleTextEditors) {
            if (editor.document.uri.scheme !== 'file') {
                continue;
            }
            const here = file !== undefined && samePath(editor.document.uri.fsPath) === file;
            if (!here || line === null || line >= editor.document.lineCount) {
                if (this.shown.delete(editor)) {
                    editor.setDecorations(this.highlight, []);
                }
            } else {
                const range = editor.document.lineAt(line).range;
                editor.setDecorations(this.highlight, [range]);
                editor.revealRange(range, vscode.TextEditorRevealType.InCenterIfOutsideViewport);
                this.shown.add(editor);
            }
        }
    }

    /** Opens the file the program stopped in, if it is not on screen. */
    private async reveal(): Promise<void> {
        const file = this.state?.file;
        if (!file || this.running || this.disposed) {
            return;
        }
        const visible = vscode.window.visibleTextEditors.some(
            (editor) => editor.document.uri.scheme === 'file' && samePath(editor.document.uri.fsPath) === samePath(file),
        );
        if (!visible) {
            const beside = vscode.window.visibleTextEditors.find((editor) => editor.viewColumn !== this.panel.viewColumn);
            await vscode.window.showTextDocument(vscode.Uri.file(file), {
                viewColumn: beside?.viewColumn ?? vscode.ViewColumn.One,
                preserveFocus: true,
                preview: true,
            });
            this.showLine();
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
        const here = samePath(this.uri.fsPath);
        return this.breakpoints()
            .filter((entry) => samePath(entry.path) === here)
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

    /** Loads the program, or loads it again from the editor's current text. */
    async load(): Promise<void> {
        await this.stop();
        await this.runtime.syncUnsaved();
        if (this.session !== undefined) {
            this.runtime.close(this.session);
            this.session = undefined;
        }
        const reply = await this.runtime.tryCall<MachineState>({
            op: `${this.kind}.open`,
            path: this.uri.fsPath,
            ram: this.window,
            screen: true,
        });
        if (this.accept(reply) && !isError(reply)) {
            this.session = reply.session;
        } else {
            this.state = null;
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
                if (this.running === handle) {
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
                this.accept(final);
                this.post();
                void this.reveal();
            }),
        };
        this.running = handle;
        this.post();
    }

    private async call(request: Record<string, unknown>): Promise<void> {
        if (this.session === undefined) {
            return;
        }
        const reply = await this.runtime.tryCall<MachineState>({
            screen: true,
            ...request,
            op: `${this.kind}.${request.op}`,
            session: this.session,
            ram: this.window,
            now: Math.floor(performance.now()),
        });
        if (isError(reply) && request.op === 'set') {
            if (!this.disposed) {
                const rejected: ToComputer = { type: 'rejected', message: reply.error.message };
                void this.panel.webview.postMessage(rejected);
            }
            return;
        }
        this.accept(reply);
        this.post();
    }

    private async handle(message: FromComputer): Promise<void> {
        switch (message.type) {
            case 'ready':
                if (this.state || this.problem) {
                    // A fresh page for a program already loaded: send it everything.
                    await this.call({ op: 'state', screen: 'force' });
                } else {
                    await this.load();
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
                await this.call({ op: 'step', unit: message.unit ?? 'command' });
                await this.reveal();
                return;
            case 'reset':
                await this.load();
                return;
            case 'speed': {
                this.speed = message.speed;
                if (this.running) {
                    await this.stop();
                    this.start();
                }
                return;
            }
            case 'key':
                if (this.session !== undefined) {
                    await this.runtime.tryCall({ op: 'key', session: this.session, code: message.code });
                }
                return;
            case 'memory': {
                this.memoryStart = message.start;
                if (this.running) {
                    await this.stop();
                    this.start();
                } else {
                    await this.call({ op: 'state' });
                }
                return;
            }
            case 'set':
                await this.call({ op: 'set', target: message.target, address: message.address, value: message.value });
                return;
        }
    }

    /** For tests: what the panel shows now. */
    snapshot(): { state: MachineState | null; running: boolean; problem: string | null } {
        const state = this.state ? { ...this.state, screen: this.lastScreen } : null;
        return { state, running: this.running !== undefined, problem: this.problem };
    }

    /** For tests: handles a message as if the panel had sent it. */
    async simulate(message: FromComputer): Promise<void> {
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

/** A path in a form that compares equal for the same file on Windows too. */
function samePath(file: string): string {
    return path.normalize(file).replace(/\\/g, '/').toLowerCase();
}

export class ComputerPanels implements vscode.Disposable {
    private readonly panels = new Map<string, ComputerPanel>();
    private readonly highlight = vscode.window.createTextEditorDecorationType({
        isWholeLine: true,
        backgroundColor: new vscode.ThemeColor('editor.stackFrameHighlightBackground'),
        overviewRulerColor: new vscode.ThemeColor('editorOverviewRuler.findMatchForeground'),
        overviewRulerLane: vscode.OverviewRulerLane.Full,
    });

    constructor(
        private readonly extensionUri: vscode.Uri,
        private readonly runtime: Runtime,
    ) {}

    /** Opens (or reveals) the computer for a program: `.asm`, `.hack`, `.vm` or `.jack`. */
    async open(uri: vscode.Uri): Promise<ComputerPanel> {
        const kind = /\.(vm|jack)$/i.test(uri.fsPath) ? 'vm' : 'cpu';
        const key = uri.toString();
        const existing = this.panels.get(key);
        if (existing) {
            existing.panel.reveal(undefined, true);
            await existing.settled();
            return existing;
        }
        const name = path.basename(uri.fsPath);
        const panel = vscode.window.createWebviewPanel(
            'nandwright.computer',
            `${name} · ${kind === 'vm' ? 'VM emulator' : 'Hack computer'}`,
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
        const computer = new ComputerPanel(uri, kind, panel, this.runtime, this.highlight, () => this.panels.delete(key));
        this.panels.set(key, computer);
        await computer.load();
        return computer;
    }

    dispose(): void {
        for (const panel of [...this.panels.values()]) {
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
[hidden] { display: none !important; }
`;
