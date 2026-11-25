// The circuit diagram panel: a chip drawn as a live schematic, next to its
// source. It shares its simulation with the chip simulator panel of the same
// file, links parts and wires to their part statements, and follows the
// editor and its edits like the simulator does.

import * as path from 'node:path';
import * as vscode from 'vscode';
import { followsEditor } from './followEditor';
import { Build, CLOCK_OPS, ChipHub, ChipSessions, HubError, HubEvent, HubMember, HubRequest, isTransport } from './chipSessions';
import { sourceHash } from './sourceHash';
import { panelHtml } from './panelHtml';
import { pathKey, samePath } from './paths';
import { CIRCUIT_STYLE } from './webview/circuitStyle';
import { valuesFit } from './webview/circuitLayout';
import type { DiagramCommand, Diagram, FromCircuit, Settings, Span, ToCircuit, Unbound, Values, ValuesCause } from './webview/circuitTypes';
import type { Radix } from './values';

/** Typing pauses this long before the panel reloads the chip (as the simulator). */
const RELOAD_DELAY_MS = 250;

/** Where the radix chosen in the toolbar is kept, per workspace. */
const RADIX_KEY = 'nandwright.circuit.radix';

/** Messages kept for tests. */
const LOG_SIZE = 50;

const chipName = (uri: vscode.Uri) => path.basename(uri.fsPath, path.extname(uri.fsPath));

/**
 * The editor range of a span. Its offsets are bytes of the UTF-8 text the
 * chip was built from; the editor counts UTF-16 units. They are used only
 * when `exact` (the text has the diagram's `sourceHash`): after an edit, in
 * the editor or on disk, even one on the same line, they point elsewhere,
 * and the span's lines are the best guess until the chip reloads.
 */
export function spanRange(document: vscode.TextDocument, span: Span, exact: boolean): vscode.Range {
    if (exact) {
        const bytes = Buffer.from(document.getText(), 'utf8');
        const at = (offset: number) => document.positionAt(bytes.subarray(0, Math.min(offset, bytes.length)).toString('utf8').length);
        const start = at(span.start);
        const end = at(span.end);
        if (start.line === span.line && end.line === span.endLine) {
            return new vscode.Range(start, end);
        }
    }
    const last = Math.max(0, Math.min(span.endLine, document.lineCount - 1));
    return new vscode.Range(Math.min(span.line, last), 0, last, document.lineAt(last).text.length);
}

/**
 * Whether `diagram`'s spans are byte offsets into `text`: it is the exact
 * text the chip was built from (same `sourceHash`). Otherwise only the
 * span's lines are a fair guess.
 */
export function diagramSpansMatch(diagram: Diagram | null, text: string): boolean {
    const hash = diagram?.sourceHash;
    return typeof hash === 'string' && sourceHash(text) === hash;
}

const BOUND = new Set<FromCircuit['type']>(['set', 'drill', 'reveal']);

const isCommand = (message: FromCircuit): message is DiagramCommand => BOUND.has(message.type);

export class CircuitPanel implements HubMember {
    /** The chip the panel was opened for (the root of the drill path). */
    private chip: vscode.Uri;
    /** The shared session of the chip: every runtime call goes through it. */
    private hub: ChipHub;
    /** The level wanted. Kept when the session goes away; shortened only when the build says it no longer exists. */
    private path: number[] = [];
    private diagram: Diagram | null = null;
    /** The session the diagram is of (with its revision, the build it shows). */
    private diagramSession: number | undefined;
    /** Names the chip file currently targeted; increments on `showChip` so old clock/reset clicks are dropped. */
    private targetToken = 1;
    /** Names the diagram sent to the webview; inputs, drills and reveals about an older one are dropped. */
    private token = 0;
    /** The token of the diagram the webview has drawn. */
    private drawnToken = 0;
    private values: Values | null = null;
    /** Why the chip does not build (from the hub: only a build clears it). */
    private buildError: string | null = null;
    /** Set while the simulator has stopped for good: shown with Retry. */
    private halted: string | null = null;
    /** The last failure of an op this panel asked for, until its next success. */
    private failure: string | null = null;
    /** The problem and Retry state the webview shows. */
    private shownProblem: string | null = null;
    private shownRetry = false;
    /** The diagram and values asked for last: only their replies are used. */
    private diagramRequest: HubRequest | undefined;
    private valuesRequest: HubRequest | undefined;
    /** Set during a synchronous `busy` rejection so an in-flight `diagramRequest` is preserved. */
    private lastBusy = false;
    /** Why the values asked for are wanted (the latest change before they come). */
    private valuesCause: ValuesCause = 'external';
    private ready = false;
    private disposed = false;
    private reloadTimer: NodeJS.Timeout | undefined;
    private readonly disposables: vscode.Disposable[] = [];
    /** Work that does not touch the runtime (the editor, files), one at a time. */
    private queue: Promise<void> = Promise.resolve();
    /** Messages sent to the webview, newest last, for tests. */
    readonly posted: ToCircuit[] = [];

    constructor(
        uri: vscode.Uri,
        readonly panel: vscode.WebviewPanel,
        private readonly sessions: ChipSessions,
        private readonly host: CircuitPanels,
        private readonly onDispose: () => void,
    ) {
        this.chip = uri;
        this.hub = sessions.join(uri.fsPath, this);
        panel.webview.onDidReceiveMessage((message: FromCircuit) => void this.receive(message), null, this.disposables);
        panel.onDidDispose(() => this.dispose(), null, this.disposables);
        // Any chip of the folder can be a part of this one.
        const inFolder = (document: vscode.TextDocument) =>
            document.uri.scheme === 'file' &&
            document.fileName.toLowerCase().endsWith('.hdl') &&
            pathKey(path.dirname(document.uri.fsPath)) === pathKey(path.dirname(this.chip.fsPath));
        this.disposables.push(
            vscode.workspace.onDidChangeTextDocument((event) => {
                if (inFolder(event.document) && event.contentChanges.length > 0) {
                    this.scheduleReload();
                }
            }),
            vscode.workspace.onDidSaveTextDocument((document) => inFolder(document) && this.scheduleReload()),
        );
    }

    /** The chip file shown now (the top level). */
    get uri(): vscode.Uri {
        return this.chip;
    }

    /** The chip file shown (switching is immediate). */
    get target(): vscode.Uri {
        return this.chip;
    }

    private enqueue(work: () => Promise<unknown>): Promise<void> {
        this.queue = this.queue
            .then(async () => {
                if (!this.disposed) {
                    await work();
                }
            })
            .catch((error) => {
                this.failure = error instanceof Error ? error.message : String(error);
                this.syncProblem();
            });
        return this.queue;
    }

    private post(message: ToCircuit): void {
        if (this.disposed) {
            return;
        }
        this.posted.push(message);
        if (this.posted.length > LOG_SIZE) {
            this.posted.splice(0, this.posted.length - LOG_SIZE);
        }
        if (this.ready) {
            void this.panel.webview.postMessage(message);
        }
    }

    /** The problem to show: the chip's own and the simulator halt, or the last op's failure. */
    private get problem(): string | null {
        if (this.buildError !== null && this.halted !== null) {
            return `${this.buildError}\n\n${this.halted}`;
        }
        return this.buildError ?? this.halted ?? this.failure;
    }

    /** Shows the problem if it changed (a diagram drawn again clears it in the webview). */
    private syncProblem(): void {
        const problem = this.problem;
        const retry = this.retryable();
        if (problem === this.shownProblem && retry === this.shownRetry) {
            return;
        }
        this.shownProblem = problem;
        this.shownRetry = retry;
        if (problem !== null) {
            this.post({ type: 'error', message: problem, ...(retry ? { retry: true } : {}) });
        } else if (this.diagram) {
            this.postDiagram();
        }
    }

    private retryable(): boolean {
        return this.halted !== null;
    }

    /** Replaces the diagram shown; inputs, drills and reveals sent about the old one no longer apply. */
    private setDiagram(diagram: Diagram | null, session: number | undefined): void {
        this.diagram = diagram;
        this.diagramSession = diagram ? session : undefined;
        this.values = diagram?.values ?? null;
        this.token++;
    }

    private postDiagram(): void {
        if (this.diagram) {
            // The values may have moved on since the diagram came.
            const diagram = { ...this.diagram, values: this.values ?? this.diagram.values };
            this.post({ type: 'diagram', diagram, settings: this.host.settings(), token: this.token, target: this.targetToken });
            // The webview clears its problem on a new diagram: show it again if it stands.
            this.shownProblem = null;
            this.shownRetry = false;
            const problem = this.problem;
            if (problem !== null) {
                const retry = this.retryable();
                this.shownProblem = problem;
                this.shownRetry = retry;
                this.post({ type: 'error', message: problem, ...(retry ? { retry: true } : {}) });
            }
        } else if (this.problem !== null) {
            const retry = this.retryable();
            this.shownProblem = this.problem;
            this.shownRetry = retry;
            this.post({ type: 'error', message: this.problem, ...(retry ? { retry: true } : {}) });
        }
    }

    /** Loads the chip: asks the hub for the diagram of the level shown (opening the chip if no panel has). */
    load(): Promise<void> {
        void this.requestDiagram();
        return this.settled();
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
        this.path = [];
        this.setDiagram(null, undefined);
        this.diagramRequest = undefined;
        this.valuesRequest = undefined;
        this.buildError = null;
        this.halted = null;
        this.failure = null;
        this.shownProblem = null;
        this.shownRetry = false;
        this.panel.title = `${chipName(uri)} · circuit`;
        void this.requestDiagram();
        return this.settled();
    }

    private scheduleReload(): void {
        clearTimeout(this.reloadTimer);
        this.reloadTimer = setTimeout(() => void this.hub.request(this, { op: 'reload' }), RELOAD_DELAY_MS);
    }

    /**
     * Asks for the diagram of `path`; only the reply to the latest request is
     * used. Explicit drill/breadcrumb clicks pass `expect` so a rebuild cannot
     * open a different part at the same index.
     */
    private requestDiagram(path: number[] = this.path, expect?: Build): Promise<unknown> {
        const previous = this.diagramRequest;
        const request: HubRequest = expect ? { op: 'diagram', path: [...path], expect } : { op: 'diagram', path: [...path] };
        this.diagramRequest = request;
        this.lastBusy = false;
        const done = this.hub.request(this, request);
        if (this.lastBusy) {
            this.diagramRequest = previous;
        }
        return done;
    }

    /** Asks for the values of the level shown, unless a request is already queued (it runs after the change). */
    private requestValues(cause: ValuesCause): void {
        if (!this.diagram) {
            if (!this.diagramRequest) {
                void this.requestDiagram();
            }
            return;
        }
        this.valuesCause = cause;
        if (this.valuesRequest) {
            return;
        }
        const request: HubRequest = { op: 'values', path: [...this.diagram.path] };
        this.valuesRequest = request;
        void this.hub.request(this, request);
    }

    /**
     * An op on the shared session ran (asked by this panel or the simulator
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
                if (event.outcome.ok) {
                    if (own) {
                        this.failure = null;
                    }
                    // A new build: its diagram, at the level wanted (if it still exists).
                    void this.requestDiagram();
                } else if (own && event.outcome.error.code === 'busy') {
                    this.post({ type: 'note', message: event.outcome.error.message });
                }
                break;
            case 'set':
            case 'setAll':
            case 'eval':
            case 'tick':
            case 'tock':
            case 'step':
                if (event.outcome.ok) {
                    if (own) {
                        this.failure = null;
                    }
                    this.requestValues(own ? (CLOCK_OPS.has(event.op) ? 'clock' : 'input') : 'external');
                } else if (own) {
                    this.failed(event.op, event.outcome.error);
                }
                break;
            case 'diagram': {
                const explicit = event.request?.op === 'diagram' && event.request.expect !== undefined;
                if (!event.outcome.ok && own && event.outcome.error.code === 'busy') {
                    this.lastBusy = true;
                    this.post({ type: 'note', message: event.outcome.error.message });
                    break;
                }
                if (!event.outcome.ok && own && explicit && event.outcome.error.code === 'stale-revision') {
                    // The chip was rebuilt before this drill click ran: stay at
                    // the current level, refresh it, and ask the student to click again.
                    if (event.request === this.diagramRequest) {
                        this.diagramRequest = undefined;
                    }
                    this.post({ type: 'note', message: 'The chip was rebuilt; click again.' });
                    if (!this.diagramRequest) {
                        void this.requestDiagram();
                    }
                    break;
                }
                if (event.request !== this.diagramRequest) {
                    break;
                }
                this.diagramRequest = undefined;
                if (event.outcome.ok) {
                    this.failure = null;
                    this.path = [...event.outcome.value.path];
                    this.setDiagram(event.outcome.value, event.session);
                    this.valuesRequest = undefined;
                    this.shownProblem = null;
                    this.shownRetry = false;
                    this.postDiagram();
                    return;
                }
                this.diagramFailed(event.path, event.outcome.error, explicit);
                break;
            }
            case 'values':
                if (event.request !== this.valuesRequest) {
                    break;
                }
                this.valuesRequest = undefined;
                if (event.outcome.ok) {
                    this.takeValues(event.outcome.value, event.session);
                } else if (event.outcome.error.code === 'invalid-path' || event.outcome.error.code === 'stale-revision') {
                    // The level is not in this build (any more): find the deepest that is.
                    void this.requestDiagram();
                }
                break;
            default:
                // `state`, `table` (the simulator's), `lost` (the hub opens the chip
                // again), `halted` (shown below).
                break;
        }
        // Any op can show that the session is at another build than the one
        // drawn (the engine says so in its reply): draw that build.
        // (Not after a diagram or values reply: those decide for themselves, and
        // asking again after a failed one would ask forever.)
        if (
            event.op !== 'diagram' &&
            event.op !== 'values' &&
            event.outcome.ok &&
            this.diagram &&
            !this.diagramRequest &&
            event.session !== undefined &&
            (event.session !== this.diagramSession || event.revision !== this.diagram.revision)
        ) {
            void this.requestDiagram();
        }
        this.syncProblem();
    }

    /** The diagram of `path` could not be had. */
    private diagramFailed(path: number[], error: HubError, explicit: boolean): void {
        if (isTransport(error)) {
            // A session that went away while an in-flight drill ran keeps that
            // drill path: the hub opens the chip again, and its `open` brings it back.
            if (explicit) {
                this.path = [...path];
            }
            return;
        }
        if (error.code === 'invalid-path' && path.length > 0) {
            if (!explicit || !this.diagram) {
                // Automatic refresh: this build has no composite part there, so
                // walk up one level at a time until a level exists.
                this.path = path.slice(0, -1);
                void this.requestDiagram();
            }
        } else if (error.code === 'stale-revision') {
            void this.requestDiagram();
        } else if (error.code !== 'build-failed' && error.code !== 'halted' && error.code !== 'busy') {
            this.failure = error.message;
        }
    }

    private takeValues(values: Values, session: number | undefined): void {
        const diagram = this.diagram;
        if (!diagram) {
            return;
        }
        if (session === this.diagramSession && valuesFit(diagram, values)) {
            this.values = values;
            this.post({ type: 'values', values, cause: this.valuesCause });
            return;
        }
        if (session !== this.diagramSession || values.revision !== diagram.revision) {
            // The chip was built again: these values number their nets for
            // another diagram. Draw the current one instead.
            void this.requestDiagram();
        }
    }

    /** An op this panel asked for failed. */
    private failed(op: string, error: HubError): void {
        if (isTransport(error) || error.code === 'build-failed' || error.code === 'halted') {
            return;
        }
        if (error.code === 'busy') {
            this.post({ type: 'note', message: error.message });
            return;
        }
        if (error.code === 'stale-revision') {
            // The chip was built again before the click got to it (a Reset or
            // an edit in the other panel): nothing was set.
            this.post({ type: 'note', message: 'The chip was rebuilt; click again.' });
            void this.requestDiagram();
            return;
        }
        if (op === 'set' || op === 'setAll') {
            void vscode.window.showWarningMessage(error.message);
            return;
        }
        this.failure = error.message;
    }

    /** The build the diagram shows. */
    private shownBuild(): Build | undefined {
        const revision = this.diagram?.revision;
        return this.diagramSession !== undefined && revision !== undefined ? { session: this.diagramSession, revision } : undefined;
    }

    /** Whether this panel has been disposed. */
    get isDisposed(): boolean {
        return this.disposed;
    }

    /**
     * A message from the webview. Runtime ops go to the hub at once, so they
     * run in the order clicked (in this panel and the simulator panel). An
     * input, drill or reveal is about the diagram it was clicked on, and is
     * dropped if that diagram has been replaced; clock buttons carry the
     * file-target token so they survive rebuilds of the same chip but are
     * dropped after the panel navigated to another file. The promise is for
     * tests.
     */
    private receive(message: FromCircuit): Promise<unknown> {
        if (this.disposed) {
            return Promise.resolve();
        }
        if (isCommand(message) && message.diagram !== this.token) {
            // Sent about a diagram that has been replaced since (another
            // chip, a reload, a drill): its pins, parts and nets may be others now.
            return Promise.resolve();
        }
        if (message.type === 'clock' && message.target !== this.targetToken) {
            // Sent before the panel switched to another chip file.
            return Promise.resolve();
        }
        switch (message.type) {
            case 'drawn':
                this.drawnToken = message.diagram;
                return Promise.resolve();
            case 'ready':
                this.ready = true;
                if (this.diagram || this.problem !== null) {
                    this.postDiagram();
                    return Promise.resolve();
                }
                return this.requestDiagram();
            case 'set': {
                if (!this.diagram || this.diagram.path.length > 0) {
                    return Promise.resolve();
                }
                const expect = this.shownBuild();
                if (!expect) {
                    return Promise.resolve();
                }
                return this.hub.request(this, { op: 'set', pin: message.pin, value: message.value, eval: !this.values?.clocked, expect });
            }
            case 'clock':
                return this.hub.request(this, { op: message.action });
            case 'retry':
                return this.sessions.retry(this);
            case 'drill': {
                const expect = this.shownBuild();
                if (!expect) {
                    return Promise.resolve();
                }
                return this.requestDiagram(message.path, expect);
            }
            case 'reveal': {
                const token = message.diagram;
                return this.enqueue(async () => {
                    if (token === this.token) {
                        await this.host.reveal(this, message.part, message.net, token);
                    }
                });
            }
            case 'export':
                return this.enqueue(() => this.export(message.format, message.data));
            case 'settings':
                return this.enqueue(() => this.host.updateSettings(message));
            case 'error':
                this.host.log(message.message);
                if (message.notify) {
                    void vscode.window.showWarningMessage(message.message);
                }
                return Promise.resolve();
        }
    }

    private async export(format: 'svg' | 'png', data: string): Promise<void> {
        const name = this.diagram?.chip ?? chipName(this.chip);
        const folder = path.dirname(this.diagram?.file ?? this.chip.fsPath);
        const target = await vscode.window.showSaveDialog({
            defaultUri: vscode.Uri.file(path.join(folder, format === 'svg' ? `${name}.svg` : `${name}@2x.png`)),
            filters: format === 'svg' ? { 'SVG image': ['svg'] } : { 'PNG image': ['png'] },
            title: `Export the ${name} diagram`,
        });
        if (!target) {
            return;
        }
        await this.host.writeExport(target, format === 'svg' ? Buffer.from(data, 'utf8') : Buffer.from(data, 'base64'));
    }

    /** Sends the settings again (after a change of the setting or the radix). */
    sendSettings(settings: Settings): void {
        this.post({ type: 'settings', settings });
    }

    /** Highlights a part or net of the level shown, from the editor cursor. */
    highlight(part: number | undefined, net: number | undefined): void {
        this.post({ type: 'highlight', ...(part !== undefined ? { part } : {}), ...(net !== undefined ? { net } : {}) });
    }

    /** What the panel shows now, for tests and the editor links. `drawn` is whether the webview has drawn the current diagram. */
    snapshot(): { path: number[]; diagram: Diagram | null; values: Values | null; problem: string | null; retry: boolean; token: number; target: number; drawn: boolean } {
        return {
            path: [...this.path],
            diagram: this.diagram,
            values: this.values,
            problem: this.problem,
            retry: this.retryable(),
            token: this.token,
            target: this.targetToken,
            drawn: this.diagram !== null && this.drawnToken === this.token,
        };
    }

    /**
     * Whether the diagram's spans are offsets into `text`: it is the exact
     * text the chip was built from (same `sourceHash`). Otherwise only the
     * span's lines are a fair guess.
     */
    spansMatch(text: string): boolean {
        return diagramSpansMatch(this.diagram, text);
    }

    /**
     * Handles a message as if the webview had sent it, for tests, and waits
     * for everything it led to. A command without a `diagram` or `target`
     * token is about the diagram/chip shown now.
     */
    async simulate(message: Unbound<FromCircuit>): Promise<void> {
        let filled: FromCircuit = message as FromCircuit;
        if (isCommand(filled) && (message as { diagram?: number }).diagram === undefined) {
            filled = { ...filled, diagram: this.token } as FromCircuit;
        } else if (filled.type === 'clock' && (message as { target?: number }).target === undefined) {
            filled = { ...filled, target: this.targetToken };
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
        this.sessions.leave(this.chip.fsPath, this);
        this.disposables.forEach((d) => d.dispose());
        this.onDispose();
    }
}

export class CircuitPanels implements vscode.Disposable {
    private readonly panels = new Set<CircuitPanel>();
    private readonly subscriptions: vscode.Disposable[] = [];
    private readonly decoration = vscode.window.createTextEditorDecorationType({
        backgroundColor: new vscode.ThemeColor('editor.wordHighlightStrongBackground'),
        borderColor: new vscode.ThemeColor('editor.wordHighlightStrongBorder'),
        borderStyle: 'solid',
        borderWidth: '1px',
        isWholeLine: false,
    });
    /** The editor and selection a reveal just made, so its own selection event is not read as the student's. */
    private revealed: { editor: vscode.TextEditor; selection: vscode.Selection } | undefined;
    private readonly revealing = new Set<vscode.Uri>();
    private decorated: vscode.TextEditor | undefined;

    constructor(
        private readonly extensionUri: vscode.Uri,
        private readonly sessions: ChipSessions,
        private readonly state: vscode.Memento,
        private readonly output: vscode.OutputChannel,
    ) {
        this.subscriptions.push(
            this.decoration,
            vscode.window.onDidChangeActiveTextEditor((editor) => {
                const uri = editor?.document.uri;
                if (!uri || uri.scheme !== 'file' || !/\.hdl$/i.test(uri.fsPath) || !followsEditor()) {
                    return;
                }
                // Opening a statement from the diagram keeps its drill path.
                if ([...this.revealing].some((file) => samePath(file.fsPath, uri.fsPath))) {
                    return;
                }
                const panel = this.followed();
                if (panel && !this.showing(uri)) {
                    this.clearDecoration();
                    void panel.showChip(uri);
                }
            }),
            vscode.window.onDidChangeTextEditorSelection((event) => this.selectionChanged(event)),
            vscode.workspace.onDidChangeConfiguration((event) => {
                if (event.affectsConfiguration('nandwright.circuit.animate') || event.affectsConfiguration('workbench.reduceMotion')) {
                    this.broadcast();
                }
            }),
        );
    }

    private followed(): CircuitPanel | undefined {
        return [...this.panels].pop();
    }

    private showing(uri: vscode.Uri): CircuitPanel | undefined {
        return [...this.panels].find((panel) => samePath(panel.target.fsPath, uri.fsPath));
    }

    /** Opens (or reveals) the circuit diagram of a chip file. */
    async open(uri: vscode.Uri): Promise<CircuitPanel> {
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
        const name = chipName(uri);
        const panel = vscode.window.createWebviewPanel(
            'nandwright.circuit',
            `${name} · circuit`,
            { viewColumn: vscode.ViewColumn.Beside, preserveFocus: true },
            {
                enableScripts: true,
                retainContextWhenHidden: true,
                localResourceRoots: [vscode.Uri.joinPath(this.extensionUri, 'dist', 'webview')],
            },
        );
        panel.webview.html = panelHtml(panel.webview, this.extensionUri, 'circuit.js', `${name} circuit`, CIRCUIT_STYLE);
        const circuit: CircuitPanel = new CircuitPanel(uri, panel, this.sessions, this, () => this.panels.delete(circuit));
        this.panels.add(circuit);
        await circuit.load();
        return circuit;
    }

    /** The panel showing this chip now. */
    get(uri: vscode.Uri): CircuitPanel | undefined {
        return [...this.panels].find((panel) => samePath(panel.uri.fsPath, uri.fsPath));
    }

    settings(): Settings {
        const animate = vscode.workspace.getConfiguration('nandwright.circuit').get<boolean>('animate', true);
        const radix = this.state.get<Radix>(RADIX_KEY, 'dec');
        const settings: Settings = { animate, radix };
        if (vscode.workspace.getConfiguration('workbench').get<string>('reduceMotion') === 'on') {
            settings.reducedMotion = true;
        }
        return settings;
    }

    private broadcast(): void {
        const settings = this.settings();
        for (const panel of this.panels) {
            panel.sendSettings(settings);
        }
    }

    /** A toolbar change: the radix is kept for the workspace, the animation is the setting. */
    async updateSettings(change: { animate?: boolean; radix?: Radix }): Promise<void> {
        if (change.radix) {
            await this.state.update(RADIX_KEY, change.radix);
        }
        if (change.animate !== undefined) {
            const config = vscode.workspace.getConfiguration('nandwright.circuit');
            if (config.get<boolean>('animate', true) !== change.animate) {
                await config.update('animate', change.animate, vscode.ConfigurationTarget.Global);
            }
        }
        this.broadcast();
    }

    log(message: string): void {
        this.output.appendLine(message);
    }

    async writeExport(target: vscode.Uri, data: Uint8Array): Promise<void> {
        await vscode.workspace.fs.writeFile(target, data);
        const inWorkspace = !!vscode.workspace.getWorkspaceFolder(target);
        void vscode.window
            .showInformationMessage(`Saved ${path.basename(target.fsPath)}.`, 'Open', inWorkspace ? 'Reveal in Explorer' : 'Reveal in File Manager')
            .then(async (choice) => {
                if (choice === 'Open') {
                    await vscode.commands.executeCommand('vscode.open', target, { viewColumn: vscode.ViewColumn.Beside });
                } else if (choice === 'Reveal in Explorer') {
                    await vscode.commands.executeCommand('revealInExplorer', target);
                } else if (choice) {
                    await vscode.commands.executeCommand('revealFileInOS', target);
                }
            });
    }

    private clearDecoration(): void {
        this.decorated?.setDecorations(this.decoration, []);
        this.decorated = undefined;
    }

    /**
     * A click in the diagram: a part selects its statement in the file of
     * the chip shown (another file when drilled in); a net marks every part
     * statement that uses it and selects the first.
     */
    async reveal(panel: CircuitPanel, part: number | undefined, net: number | undefined, token = panel.snapshot().token): Promise<void> {
        const stillCurrent = () => !panel.isDisposed && panel.snapshot().token === token;
        if (!stillCurrent()) {
            return;
        }
        this.clearDecoration();
        const diagram = panel.snapshot().diagram;
        if (!diagram?.file) {
            return;
        }
        let spans: Span[] = [];
        if (part !== undefined) {
            const span = diagram.parts[part]?.span;
            spans = span ? [span] : [];
        } else if (net !== undefined) {
            spans = diagram.parts.filter((p) => p.span && p.conns.some((c) => c.net === net)).map((p) => p.span!);
        }
        const document = await vscode.workspace.openTextDocument(vscode.Uri.file(diagram.file));
        if (!stillCurrent()) {
            return;
        }
        const here = panel.panel.viewColumn;
        const shown = vscode.window.visibleTextEditors.find((e) => e.document === document);
        const other = vscode.window.tabGroups.all.find((group) => group.viewColumn !== here)?.viewColumn;
        const source = vscode.Uri.file(diagram.file);
        this.revealing.add(source);
        let editor: vscode.TextEditor;
        try {
            editor = await vscode.window.showTextDocument(document, {
                viewColumn: shown?.viewColumn ?? other ?? vscode.ViewColumn.Beside,
                preserveFocus: true,
                preview: false,
            });
        } finally {
            this.revealing.delete(source);
        }
        if (!stillCurrent()) {
            return;
        }
        // Validate against the captured diagram's sourceHash, never a newer diagram's.
        const exact = diagramSpansMatch(diagram, editor.document.getText());
        const ranges = spans.map((span) => spanRange(editor.document, span, exact));
        if (ranges.length === 0) {
            return;
        }
        const selection = new vscode.Selection(ranges[0].start, ranges[0].end);
        this.revealed = { editor, selection };
        editor.selection = selection;
        editor.revealRange(ranges[0], vscode.TextEditorRevealType.InCenterIfOutsideViewport);
        if (net !== undefined) {
            editor.setDecorations(this.decoration, ranges);
            this.decorated = editor;
        }
    }

    /** The editor cursor in the file of a chip shown: highlight its part or net in the diagram. */
    private selectionChanged(event: vscode.TextEditorSelectionChangeEvent): void {
        const revealed = this.revealed;
        if (revealed && revealed.editor === event.textEditor && event.selections.length === 1 && event.selections[0].isEqual(revealed.selection)) {
            this.revealed = undefined;
            return;
        }
        this.revealed = undefined;
        this.clearDecoration();
        const document = event.textEditor.document;
        if (document.uri.scheme !== 'file') {
            return;
        }
        for (const panel of this.panels) {
            const diagram = panel.snapshot().diagram;
            if (!diagram?.file || !samePath(diagram.file, document.uri.fsPath)) {
                continue;
            }
            const { part, net } = cursorTarget(document, event.selections[0].active, diagram, diagramSpansMatch(diagram, document.getText()));
            panel.highlight(part, net);
        }
    }

    dispose(): void {
        this.subscriptions.forEach((s) => s.dispose());
        for (const panel of [...this.panels]) {
            panel.panel.dispose();
        }
    }
}

/**
 * What the cursor is on: the part whose statement contains it, and the net
 * named by the identifier under it (not a pin name before `=`, not the chip
 * name before `(`).
 */
export function cursorTarget(document: vscode.TextDocument, position: vscode.Position, diagram: Diagram, exact = true): { part?: number; net?: number } {
    const result: { part?: number; net?: number } = {};
    const cursor = Buffer.byteLength(document.getText(new vscode.Range(new vscode.Position(0, 0), position)), 'utf8');
    // Offsets only in the text the chip was built from; otherwise the span's lines.
    const part = exact
        ? diagram.parts.findIndex((p) => p.span && p.span.start <= cursor && cursor <= p.span.end)
        : diagram.parts.findIndex((p) => p.span && p.span.line <= position.line && position.line <= p.span.endLine);
    if (part >= 0) {
        result.part = part;
    }
    const word = document.getWordRangeAtPosition(position, /[A-Za-z_][A-Za-z0-9_]*/);
    if (word) {
        const name = document.getText(word);
        const after = document.lineAt(word.end.line).text.slice(word.end.character);
        const isPin = /^\s*(\[[^\]]*\]\s*)?=(?!=)/.test(after);
        const isChip = /^\s*\(/.test(after);
        const net = diagram.nets.findIndex((n) => n.name === name);
        if (net >= 0 && !isPin && !isChip) {
            result.net = net;
        }
    }
    return result;
}
