// The chip simulator panel: an interactive view of one chip, next to its
// source. It reloads as the student types, so the loop is edit, look, edit.

import * as path from 'node:path';
import * as vscode from 'vscode';
import { followsEditor } from './followEditor';
import { BUILD_OPS, Build, CLOCK_OPS, ChipHub, ChipSessions, HubError, HubEvent, HubMember, HubRequest, isTransport } from './chipSessions';
import { ChipState, ChipTest, ClockStep, Expected, TruthTable } from './runtime/protocol';
import { panelHtml } from './panelHtml';
import { pathKey, samePath } from './paths';
import { resultRows } from './chipRows';
import { TestExplorer, commandFiles, textOf } from './testing';
import { parseCompare } from './values';
import type { FromPanel, ToPanel } from './webview/chip';

/** Typing pauses this long before the panel reloads the chip. */
const RELOAD_DELAY_MS = 250;

/** Clock steps kept in the history; older ones are dropped. */
const HISTORY_STEPS = 100;

/** Rows of the compare file shown; the rest are counted, not listed. */
const EXPECTED_ROWS = 256;

/** Messages kept for tests. */
const LOG_SIZE = 50;

/** A panel message without its build or file-target token (filled in by `simulate`). */
export type UnboundPanel<T> = T extends { build: Build; target: number }
    ? Omit<T, 'build' | 'target'> & { build?: Build; target?: number }
    : T extends { target: number }
      ? Omit<T, 'target'> & { target?: number }
      : T;

/**
 * The compare file of a chip: the one the first `compare-to` of `<Chip>.tst`
 * names (what the test compares with, so its results line up), or else
 * `<Chip>.cmp` beside it. Null when there is none.
 */
async function findExpected(chip: vscode.Uri): Promise<Expected | null> {
    const dir = path.dirname(chip.fsPath);
    const stem = path.basename(chip.fsPath, path.extname(chip.fsPath));
    const script = await textOf(path.join(dir, `${stem}.tst`));
    // Comments may mention compare-to; only a command counts.
    const named = script === null ? undefined : commandFiles(script, 'compare-to')[0];
    const file = named ? path.resolve(dir, named) : path.join(dir, `${stem}.cmp`);
    const text = await textOf(file);
    const parsed = text === null ? null : parseCompare(text, EXPECTED_ROWS);
    return parsed ? { path: file, name: path.basename(file), ...parsed } : null;
}

/** The test script of a chip: `<Chip>.tst` beside it, if there is one. */
async function findScript(chip: vscode.Uri): Promise<vscode.Uri | null> {
    const stem = path.basename(chip.fsPath, path.extname(chip.fsPath));
    const file = path.join(path.dirname(chip.fsPath), `${stem}.tst`);
    return (await textOf(file)) === null ? null : vscode.Uri.file(file);
}

export class ChipPanel implements HubMember {
    private state: ChipState | null = null;
    /** The session `state` is of: with its revision, the build an input is meant for. */
    private stateSession: number | undefined;
    private table: TruthTable | null = null;
    /** Why the chip does not build (from the hub: only a build clears it). */
    private buildError: string | null = null;
    /** Set while the simulator has stopped for good: shown with Retry. */
    private halted: string | null = null;
    /** The last failure of an op this panel asked for, until its next success. */
    private failure: string | null = null;
    private reloadTimer: NodeJS.Timeout | undefined;
    private expectedTimer: NodeJS.Timeout | undefined;
    private expected: Expected | null = null;
    private script: vscode.Uri | null = null;
    /** The pins after each clock step taken on the chip (in either panel), oldest first. */
    private history: ClockStep[] = [];
    /** Whether older steps were dropped from the history. */
    private historyCapped = false;
    private watcher: vscode.FileSystemWatcher | undefined;
    private disposed = false;
    /** The chip shown. It changes when the panel follows the editor. */
    private chip: vscode.Uri;
    /** Names the chip file currently targeted; increments on `showChip` so old clicks are dropped. */
    private targetToken = 1;
    /** The shared session of the chip shown: every runtime call goes through it. */
    private hub: ChipHub;
    /** The truth table asked for last: only its reply is used. */
    private tableRequest: HubRequest | undefined;
    private readonly disposables: vscode.Disposable[] = [];
    /** Work that does not touch the runtime (files, the editor), one at a time. */
    private queue: Promise<void> = Promise.resolve();
    /** Messages sent to the webview, newest last, for tests. */
    readonly posted: ToPanel[] = [];

    constructor(
        uri: vscode.Uri,
        readonly panel: vscode.WebviewPanel,
        private readonly sessions: ChipSessions,
        private readonly tests: TestExplorer,
        private readonly onDispose: () => void,
    ) {
        this.chip = uri;
        this.hub = this.sessions.join(uri.fsPath, this);
        this.disposables.push(this.tests.onDidChangeStatus(() => this.post()));
        panel.webview.onDidReceiveMessage((message: FromPanel) => this.receive(message), null, this.disposables);
        panel.onDidDispose(() => this.dispose(), null, this.disposables);
        // The folder is read each time: the chip shown can change.
        const inFolder = (document: vscode.TextDocument) =>
            document.uri.scheme === 'file' &&
            document.fileName.toLowerCase().endsWith('.hdl') &&
            pathKey(path.dirname(document.uri.fsPath)) === pathKey(path.dirname(this.chip.fsPath));
        // Compare files and scripts in the folder change what is expected, not the chip.
        const expectedFile = (document: vscode.TextDocument) =>
            document.uri.scheme === 'file' &&
            /\.(cmp|tst)$/i.test(document.fileName) &&
            pathKey(path.dirname(document.uri.fsPath)) === pathKey(path.dirname(this.chip.fsPath));
        this.disposables.push(
            vscode.workspace.onDidChangeTextDocument((event) => {
                if (inFolder(event.document) && event.contentChanges.length > 0) {
                    this.scheduleReload();
                } else if (expectedFile(event.document) && event.contentChanges.length > 0) {
                    this.scheduleExpected();
                }
            }),
            vscode.workspace.onDidSaveTextDocument((document) => {
                if (inFolder(document)) {
                    this.scheduleReload();
                } else if (expectedFile(document)) {
                    this.scheduleExpected();
                }
            }),
        );
        this.watchFolder();
    }

    /** The chip file shown now. */
    get uri(): vscode.Uri {
        return this.chip;
    }

    /** The chip file shown (switching is immediate). */
    get target(): vscode.Uri {
        return this.chip;
    }

    /** Shows another chip: leaves the old chip's hub and joins the new one's. */
    showChip(uri: vscode.Uri): Promise<void> {
        if (this.disposed || samePath(uri.fsPath, this.chip.fsPath)) {
            return this.settled();
        }
        clearTimeout(this.reloadTimer);
        this.sessions.leave(this.chip.fsPath, this);
        this.chip = uri;
        this.hub = this.sessions.join(uri.fsPath, this);
        this.targetToken++;
        this.state = null;
        this.stateSession = undefined;
        this.table = null;
        this.tableRequest = undefined;
        this.buildError = null;
        this.halted = null;
        this.failure = null;
        this.expected = null;
        this.script = null;
        this.clearHistory();
        this.watchFolder();
        this.panel.title = `${path.basename(uri.fsPath, path.extname(uri.fsPath))} · simulator`;
        this.post();
        void this.hub.request(this, { op: 'state' });
        this.enqueue(() => this.readExpected().then(() => this.post()));
        return this.settled();
    }

    private enqueue(work: () => Promise<unknown>): void {
        this.queue = this.queue
            .then(async () => {
                if (!this.disposed) {
                    await work();
                }
            })
            .catch((error) => {
                this.failure = error instanceof Error ? error.message : String(error);
                this.post();
            });
    }

    /** Watches the chip's folder for changes made outside the editor (a script saved by a tool, a file replaced). */
    private watchFolder(): void {
        this.watcher?.dispose();
        const watcher = vscode.workspace.createFileSystemWatcher(new vscode.RelativePattern(vscode.Uri.file(path.dirname(this.chip.fsPath)), '*.{cmp,tst,CMP,TST}'));
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
            RELOAD_DELAY_MS,
        );
    }

    private async readExpected(): Promise<void> {
        const chip = this.chip;
        const [found, script] = await Promise.all([findExpected(chip), findScript(chip)]);
        if (!this.disposed && chip === this.chip) {
            this.expected = found;
            this.script = script;
        }
    }

    private scheduleReload(): void {
        clearTimeout(this.reloadTimer);
        this.reloadTimer = setTimeout(() => void this.reload(), RELOAD_DELAY_MS);
    }

    /** The problem to show: the chip's own and the simulator halt, or the last op's failure. */
    private get problem(): string | null {
        if (this.buildError !== null && this.halted !== null) {
            return `${this.buildError}\n\n${this.halted}`;
        }
        return this.buildError ?? this.halted ?? this.failure;
    }

    private postMessage(message: ToPanel): void {
        if (this.disposed) {
            return;
        }
        this.posted.push(message);
        if (this.posted.length > LOG_SIZE) {
            this.posted.splice(0, this.posted.length - LOG_SIZE);
        }
        void this.panel.webview.postMessage(message);
    }

    private post(): void {
        if (this.disposed) {
            return;
        }
        const message: ToPanel = {
            type: 'state',
            state: this.state,
            build: this.shownBuild() ?? null,
            target: this.targetToken,
            table: this.table,
            problem: this.problem,
            expected: this.expected,
            test: this.testState(),
            history: this.history,
            historyCapped: this.historyCapped,
            ...(this.halted !== null ? { retry: true } : {}),
        };
        this.postMessage(message);
    }

    /** The last run of the chip's test script, row by row against the compare file shown. */
    private testState(): ChipTest | null {
        if (!this.script) {
            return null;
        }
        const { status, result } = this.tests.statusOf(this.script);
        const test: ChipTest = { script: path.basename(this.script.fsPath), status };
        if (!result) {
            return test;
        }
        test.kind = result.kind;
        test.comparedLines = result.comparedLines;
        test.failureCount = result.failureCount;
        if (result.message !== undefined) {
            test.message = result.message;
        }
        if (result.rows !== undefined) {
            test.comparedRows = result.rows;
            test.differingRows = result.differingRows ?? 0;
        }
        if (this.expected && (result.kind === 'passed' || result.kind === 'failed')) {
            Object.assign(test, resultRows(this.expected.lines, result));
        }
        return test;
    }

    /** Loads the chip (asks the hub for its state, opening it if no panel has). */
    load(): Promise<void> {
        void this.hub.request(this, { op: 'state' });
        this.enqueue(() => this.readExpected().then(() => this.post()));
        return this.settled();
    }

    private clearHistory(): void {
        this.history = [];
        this.historyCapped = false;
    }

    /**
     * Builds the chip again from the editor's text, after an edit. A circuit
     * panel on the same chip that reloads after the same edit shares it.
     */
    private reload(): Promise<unknown> {
        const done = this.hub.request(this, { op: 'reload' });
        this.enqueue(() => this.readExpected().then(() => this.post()));
        return done;
    }

    /**
     * An op on the shared session ran (asked by this panel or the circuit
     * panel): what the panel shows follows from these events alone, in the
     * order the engine ran them.
     */
    hubEvent(event: HubEvent): void {
        if (this.disposed || event.key !== pathKey(this.chip.fsPath)) {
            return;
        }
        this.buildError = event.buildError;
        this.halted = event.halted;
        const own = event.origin === this;
        switch (event.op) {
            case 'open':
            case 'reload':
            case 'reset':
            case 'set':
            case 'setAll':
            case 'eval':
            case 'tick':
            case 'tock':
            case 'step':
            case 'state': {
                const outcome = event.outcome;
                if (BUILD_OPS.has(event.op) && (outcome.ok || outcome.error.code === 'build-failed')) {
                    // A new start: the clock history goes.
                    this.clearHistory();
                }
                if (outcome.ok) {
                    const rebuilt = event.session !== this.stateSession || outcome.value.revision !== this.state?.revision || outcome.value.clocked !== this.state?.clocked;
                    this.state = outcome.value;
                    this.stateSession = event.session;
                    if (own) {
                        this.failure = null;
                    }
                    // The step's own state goes in the history, in the engine's order.
                    if (CLOCK_OPS.has(event.op) && outcome.value.clocked) {
                        this.record(outcome.value);
                    }
                    if (rebuilt || BUILD_OPS.has(event.op)) {
                        this.refreshTable();
                    }
                } else if (own) {
                    this.failed(event.op, event.request, outcome.error);
                }
                break;
            }
            case 'table':
                if (event.request === this.tableRequest && event.outcome.ok && event.session === this.stateSession && event.revision === this.state?.revision) {
                    this.table = event.outcome.value;
                }
                break;
            case 'lost':
                // The hub opens the chip again (or says it gave up).
                break;
            case 'halted':
                break;
            default:
                // Diagram ops of the circuit panel change nothing here.
                return;
        }
        this.post();
    }

    private failed(op: string, request: HubRequest | undefined, error: HubError): void {
        if (isTransport(error)) {
            // The session went away under the op: the hub opens the chip again.
            return;
        }
        if (error.code === 'build-failed' || error.code === 'halted') {
            // Shown from the hub's own state.
            return;
        }
        if (error.code === 'busy') {
            this.postMessage({ type: 'note', message: error.message });
            return;
        }
        if (op === 'set' && request?.op === 'set') {
            const message = error.code === 'stale-revision' ? 'The chip was built again since; set the input again.' : error.message;
            this.postMessage({ type: 'rejected', pin: request.pin, message });
            if (error.code === 'stale-revision') {
                void this.hub.request(this, { op: 'state' });
            }
            return;
        }
        if (error.code === 'stale-revision') {
            this.postMessage({ type: 'note', message: 'The chip was built again since; select the row again.' });
            void this.hub.request(this, { op: 'state' });
            return;
        }
        this.failure = error.message;
    }

    /** Asks for the truth table of the build shown (only a chip without a clock has one). */
    private refreshTable(): void {
        this.table = null;
        this.tableRequest = undefined;
        if (!this.state || this.state.clocked) {
            return;
        }
        const request: HubRequest = { op: 'table' };
        this.tableRequest = request;
        void this.hub.request(this, request);
    }

    private record(state: ChipState): void {
        this.history.push({
            time: state.time,
            pins: state.pins.filter((pin) => pin.role === 'input' || pin.role === 'output'),
        });
        if (this.history.length > HISTORY_STEPS) {
            this.history.splice(0, this.history.length - HISTORY_STEPS);
            this.historyCapped = true;
        }
    }

    /** Shows the compare file beside the panel, leaving focus where it is. */
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

    /** The build the inputs shown are of. */
    private shownBuild(): Build | undefined {
        const revision = this.state?.revision;
        return this.stateSession !== undefined && revision !== undefined ? { session: this.stateSession, revision } : undefined;
    }

    /**
     * A message from the webview. Runtime ops go to the hub at once, so
     * they run in the order clicked (in this panel and the circuit panel);
     * the rest waits its turn in the panel's own queue. The promise is for
     * tests: it settles when the op's event has been handled.
     */
    private receive(message: FromPanel): Promise<unknown> {
        if (this.disposed) {
            return Promise.resolve();
        }
        switch (message.type) {
            case 'ready':
                if (this.state || this.problem) {
                    this.post();
                    return Promise.resolve();
                }
                return this.load();
            case 'openExpected':
                this.enqueue(() => this.openExpected());
                return this.queue;
            case 'clearHistory':
                this.clearHistory();
                this.post();
                return Promise.resolve();
            case 'runTest':
                // The run is not waited for: the panel stays usable, and the
                // results event brings the outcome.
                if (this.script) {
                    void this.tests.runScripts([this.script], { quiet: true });
                }
                return Promise.resolve();
            case 'retry':
                return this.sessions.retry(this);
            case 'set': {
                if (message.target !== this.targetToken || !message.build) {
                    return Promise.resolve();
                }
                return this.hub.request(this, { op: 'set', pin: message.pin, value: message.value, eval: !this.state?.clocked, expect: message.build });
            }
            case 'setAll': {
                if (message.target !== this.targetToken || !message.build) {
                    return Promise.resolve();
                }
                return this.hub.request(this, { op: 'setAll', values: message.values, expect: message.build });
            }
            case 'action':
                if (message.target !== this.targetToken) {
                    return Promise.resolve();
                }
                if (message.action === 'reset') {
                    const done = this.hub.request(this, { op: 'reset' });
                    this.enqueue(() => this.readExpected().then(() => this.post()));
                    return done;
                }
                return this.hub.request(this, { op: message.action });
        }
    }

    /** The state last shown, for tests. */
    snapshot(): {
        state: ChipState | null;
        build: Build | null;
        target: number;
        table: TruthTable | null;
        problem: string | null;
        expected: Expected | null;
        test: ChipTest | null;
        history: ClockStep[];
        retry: boolean;
    } {
        return {
            state: this.state,
            build: this.shownBuild() ?? null,
            target: this.targetToken,
            table: this.table,
            problem: this.problem,
            expected: this.expected,
            test: this.testState(),
            history: [...this.history],
            retry: this.halted !== null,
        };
    }

    /** Handles a message as if the panel had sent it, for tests; resolves when everything it led to is done. */
    async simulate(message: UnboundPanel<FromPanel>): Promise<void> {
        let filled: FromPanel;
        if (message.type === 'set' || message.type === 'setAll') {
            const build = message.build ?? this.shownBuild();
            const target = message.target ?? this.targetToken;
            if (!build) {
                return;
            }
            filled = { ...message, build, target } as FromPanel;
        } else if (message.type === 'action') {
            filled = { ...message, target: message.target ?? this.targetToken };
        } else {
            filled = message as FromPanel;
        }
        await this.receive(filled);
        await this.settled();
    }

    /** Resolves when the panel's queue and its hub are both idle. */
    async settled(): Promise<void> {
        for (;;) {
            const queue = this.queue;
            const hub = this.hub;
            await queue;
            await hub.whenIdle();
            if (queue === this.queue && hub === this.hub && hub.status().pending === 0) {
                return;
            }
        }
    }

    dispose(): void {
        if (this.disposed) {
            return;
        }
        this.disposed = true;
        clearTimeout(this.reloadTimer);
        clearTimeout(this.expectedTimer);
        this.watcher?.dispose();
        this.sessions.leave(this.chip.fsPath, this);
        this.disposables.forEach((d) => d.dispose());
        this.onDispose();
    }
}

export class ChipPanels implements vscode.Disposable {
    // A set, not a map keyed by file: a panel can change chip.
    private readonly panels = new Set<ChipPanel>();
    private readonly subscription: vscode.Disposable;

    constructor(
        private readonly extensionUri: vscode.Uri,
        private readonly sessions: ChipSessions,
        private readonly tests: TestExplorer,
    ) {
        this.subscription = vscode.window.onDidChangeActiveTextEditor((editor) => {
            // Anything but a chip file (a script, a built-in chip, the panel
            // itself, which leaves no editor) leaves the panel as it is.
            const uri = editor?.document.uri;
            if (!uri || uri.scheme !== 'file' || !/\.hdl$/i.test(uri.fsPath) || !followsEditor()) {
                return;
            }
            const panel = this.followed();
            if (panel && !this.showing(uri)) {
                void panel.showChip(uri);
            }
        });
    }

    /** The panel that follows the editor: the newest, if the setting was turned off and on. */
    private followed(): ChipPanel | undefined {
        return [...this.panels].pop();
    }

    private showing(uri: vscode.Uri): ChipPanel | undefined {
        return [...this.panels].find((panel) => samePath(panel.target.fsPath, uri.fsPath));
    }

    /** Opens (or reveals) the simulator for a chip file. */
    async open(uri: vscode.Uri): Promise<ChipPanel> {
        let existing = this.showing(uri);
        if (!existing && followsEditor()) {
            existing = this.followed();
            void existing?.showChip(uri);
        }
        if (existing) {
            existing.panel.reveal(undefined, true);
            await existing.settled();
            return existing;
        }
        const name = path.basename(uri.fsPath, path.extname(uri.fsPath));
        const panel = vscode.window.createWebviewPanel(
            'nandwright.chip',
            `${name} · simulator`,
            { viewColumn: vscode.ViewColumn.Beside, preserveFocus: true },
            {
                enableScripts: true,
                localResourceRoots: [vscode.Uri.joinPath(this.extensionUri, 'dist', 'webview')],
            },
        );
        panel.webview.html = panelHtml(panel.webview, this.extensionUri, 'chip.js', `${name} simulator`, CHIP_STYLE);
        const chip: ChipPanel = new ChipPanel(uri, panel, this.sessions, this.tests, () => this.panels.delete(chip));
        this.panels.add(chip);
        await chip.load();
        return chip;
    }

    /** The panel showing this chip now. */
    get(uri: vscode.Uri): ChipPanel | undefined {
        return [...this.panels].find((panel) => samePath(panel.uri.fsPath, uri.fsPath));
    }

    dispose(): void {
        this.subscription.dispose();
        for (const panel of [...this.panels]) {
            panel.panel.dispose();
        }
    }
}

const CHIP_STYLE = `
.pins { border-top: 1px solid var(--line); }
.pin { display: flex; align-items: center; min-height: 32px; gap: var(--gap); padding: 3px 4px; border-bottom: 1px solid var(--line); }
.pin-name { flex: 0 0 34%; min-width: 0; overflow-wrap: anywhere; font-family: var(--vscode-editor-font-family); }
.pin-width { margin-left: 2px; color: var(--vscode-descriptionForeground); }
.bit { display: inline-flex; align-items: center; justify-content: center; min-width: 30px; height: 24px; padding: 0 8px; border-radius: 12px; border: 1px solid var(--vscode-input-border, var(--vscode-contrastBorder, var(--vscode-descriptionForeground))); background: transparent; color: var(--vscode-foreground); }
.bit.on { color: var(--vscode-button-foreground); background: var(--vscode-button-background); border-color: var(--vscode-button-border, var(--vscode-contrastBorder, var(--vscode-button-background))); font-weight: 600; }
button.bit:hover { border-color: var(--vscode-focusBorder); }
.bit.readonly { border-style: dashed; border-radius: 4px; cursor: default; }
.bit.readonly.on { border-style: solid; }
.bit-cell { flex: 1 1 auto; display: flex; flex-direction: column; align-items: flex-start; gap: 2px; min-width: 0; }
.bus-cell { flex: 1 1 auto; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.bus { width: 100%; max-width: 260px; padding: 3px 6px; color: var(--vscode-input-foreground); background: var(--vscode-input-background); border: 1px solid var(--vscode-input-border, var(--vscode-contrastBorder, transparent)); border-radius: 2px; }
.note { margin: 6px 0 2px; padding: 6px 10px; border-radius: 3px; border: 1px solid var(--vscode-inputValidation-infoBorder, var(--line)); background: var(--vscode-inputValidation-infoBackground, var(--vscode-editorWidget-background, transparent)); font-size: 0.92em; }
.note:empty { display: none; }
.expected td, .expected th { font-family: var(--vscode-editor-font-family); }
.expected td.differs { color: var(--vscode-errorForeground); font-weight: 600; }
.expected .mark { width: 2em; padding: 3px 4px; font-family: var(--vscode-font-family); }
.expected td.mark.match { color: var(--vscode-testing-iconPassed, var(--vscode-foreground)); }
.expected td.mark.differ { color: var(--vscode-testing-iconFailed, var(--vscode-errorForeground)); font-weight: 600; }
.expected td.mark.unreached { color: var(--vscode-descriptionForeground); }
.history-bar { display: flex; flex-wrap: wrap; align-items: baseline; gap: 4px 12px; }
.history-scroll { max-height: 240px; overflow: auto; border-top: 1px solid var(--line); }
.history-scroll .truth { margin-top: 0; }
.history-scroll thead th { position: sticky; top: 0; background: var(--vscode-editor-background); }
.test-run { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 10px; margin: 2px 0 8px; }
.test-run .summary { flex: 1 1 16em; min-width: 0; overflow-wrap: anywhere; }
.test-run .summary.fails { color: var(--vscode-errorForeground); }
.link { padding: 0; border: 0; background: none; font: inherit; color: var(--vscode-textLink-foreground); text-decoration: underline; cursor: pointer; }
.link:hover { color: var(--vscode-textLink-activeForeground); }
.link:focus-visible { outline: 1px solid var(--vscode-focusBorder); outline-offset: 1px; }
.truth { border-collapse: collapse; margin-top: 4px; }
.truth th, .truth td { padding: 3px 14px; text-align: center; border-bottom: 1px solid var(--line); }
.truth th { font-weight: 600; color: var(--vscode-descriptionForeground); }
.truth .first-out { border-left: 1px solid var(--vscode-descriptionForeground); }
.truth tbody tr { cursor: pointer; }
.truth tbody tr:hover { background: var(--vscode-list-hoverBackground); }
.truth tr.current { color: var(--vscode-list-activeSelectionForeground, var(--vscode-foreground)); background: var(--vscode-list-activeSelectionBackground); outline: 1px solid var(--vscode-contrastActiveBorder, transparent); outline-offset: -1px; }
@keyframes flash { from { background: var(--vscode-editor-findMatchHighlightBackground, transparent); } to { background: transparent; } }
.pin.changed { animation: flash 0.9s ease-out; }
@media (prefers-reduced-motion: reduce) { .pin.changed { animation: none; } }
`;
