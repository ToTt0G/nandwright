// The chip simulator panel: an interactive view of one chip, next to its
// source. It reloads as the student types, so the loop is edit, look, edit.

import * as path from 'node:path';
import * as vscode from 'vscode';
import { Runtime } from './runtime/client';
import { ChipState, EngineError, TruthTable, isError } from './runtime/protocol';
import { panelHtml } from './panelHtml';
import type { FromPanel, ToPanel } from './webview/chip';

/** Typing pauses this long before the panel reloads the chip. */
const RELOAD_DELAY_MS = 250;

class ChipPanel {
    private session: number | undefined;
    private state: ChipState | null = null;
    private table: TruthTable | null = null;
    private problem: string | null = null;
    private reloadTimer: NodeJS.Timeout | undefined;
    private disposed = false;
    private readonly disposables: vscode.Disposable[] = [];
    /** Requests are handled one at a time, in order. */
    private queue: Promise<void> = Promise.resolve();

    constructor(
        readonly uri: vscode.Uri,
        readonly panel: vscode.WebviewPanel,
        private readonly runtime: Runtime,
        private readonly onDispose: () => void,
    ) {
        panel.webview.onDidReceiveMessage((message: FromPanel) => this.enqueue(() => this.handle(message)), null, this.disposables);
        panel.onDidDispose(() => this.dispose(), null, this.disposables);
        const dir = path.dirname(uri.fsPath);
        const inFolder = (document: vscode.TextDocument) =>
            document.uri.scheme === 'file' &&
            document.fileName.toLowerCase().endsWith('.hdl') &&
            path.dirname(document.uri.fsPath) === dir;
        this.disposables.push(
            vscode.workspace.onDidChangeTextDocument((event) => {
                if (inFolder(event.document) && event.contentChanges.length > 0) {
                    this.scheduleReload();
                }
            }),
            vscode.workspace.onDidSaveTextDocument((document) => {
                if (inFolder(document)) {
                    this.scheduleReload();
                }
            }),
        );
    }

    private enqueue(work: () => Promise<void>): void {
        this.queue = this.queue.then(() => (this.disposed ? undefined : work())).catch((error) => {
            this.problem = error instanceof Error ? error.message : String(error);
            this.post();
        });
    }

    private scheduleReload(): void {
        clearTimeout(this.reloadTimer);
        this.reloadTimer = setTimeout(() => this.enqueue(() => this.reload()), RELOAD_DELAY_MS);
    }

    private post(): void {
        if (this.disposed) {
            return;
        }
        const message: ToPanel = { type: 'state', state: this.state, table: this.table, problem: this.problem };
        void this.panel.webview.postMessage(message);
    }

    /** Applies an engine reply: a new state, or a problem to show. */
    private async accept(reply: ChipState | EngineError): Promise<boolean> {
        if (isError(reply)) {
            this.problem = reply.error.message;
            return false;
        }
        this.problem = null;
        this.state = reply;
        await this.refreshTable();
        return true;
    }

    private async refreshTable(): Promise<void> {
        this.table = null;
        if (this.session === undefined || !this.state || this.state.clocked) {
            return;
        }
        const reply = await this.runtime.tryCall<TruthTable>({ op: 'chip.table', session: this.session });
        if (!isError(reply)) {
            this.table = reply;
        }
    }

    /** (Re)loads the chip from the editor's current text. */
    async reload(): Promise<void> {
        await this.runtime.syncUnsaved();
        if (this.session === undefined) {
            const reply = await this.runtime.tryCall<ChipState>({ op: 'chip.open', path: this.uri.fsPath });
            if (!isError(reply)) {
                this.session = reply.session;
            }
            await this.accept(reply);
        } else {
            const reply = await this.runtime.tryCall<ChipState>({ op: 'chip.reset', session: this.session });
            if (isError(reply) && /session has ended/.test(reply.error.message)) {
                // The simulator restarted: open the chip again.
                this.session = undefined;
                return this.reload();
            }
            await this.accept(reply);
        }
        this.post();
    }

    private async handle(message: FromPanel): Promise<void> {
        if (message.type === 'ready') {
            if (this.state || this.problem) {
                this.post();
            } else {
                await this.reload();
            }
            return;
        }
        if (this.session === undefined) {
            await this.reload();
            if (this.session === undefined) {
                return;
            }
        }
        const session = this.session;
        switch (message.type) {
            case 'set': {
                const reply = await this.runtime.tryCall<ChipState>({
                    op: 'chip.set',
                    session,
                    pin: message.pin,
                    value: message.value,
                    eval: !this.state?.clocked,
                });
                if (isError(reply)) {
                    if (!this.disposed) {
                        const rejected: ToPanel = { type: 'rejected', pin: message.pin, message: reply.error.message };
                        void this.panel.webview.postMessage(rejected);
                    }
                    return;
                }
                this.state = reply;
                break;
            }
            case 'setAll': {
                let last: ChipState | undefined;
                for (const [pin, value] of Object.entries(message.values)) {
                    const reply = await this.runtime.tryCall<ChipState>({ op: 'chip.set', session, pin, value, eval: true });
                    if (!isError(reply)) {
                        last = reply;
                    }
                }
                if (last) {
                    this.state = last;
                }
                break;
            }
            case 'action': {
                if (message.action === 'reset') {
                    await this.reload();
                    return;
                }
                const reply = await this.runtime.tryCall<ChipState>({ op: `chip.${message.action}`, session });
                if (isError(reply)) {
                    this.problem = reply.error.message;
                } else {
                    this.problem = null;
                    this.state = reply;
                }
                break;
            }
        }
        this.post();
    }

    /** The state last shown, for tests. */
    snapshot(): { state: ChipState | null; table: TruthTable | null; problem: string | null } {
        return { state: this.state, table: this.table, problem: this.problem };
    }

    /** Handles a message as if the panel had sent it, for tests. */
    async simulate(message: FromPanel): Promise<void> {
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
        this.disposed = true;
        clearTimeout(this.reloadTimer);
        if (this.session !== undefined) {
            this.runtime.close(this.session);
            this.session = undefined;
        }
        this.disposables.forEach((d) => d.dispose());
        this.onDispose();
    }
}

export class ChipPanels implements vscode.Disposable {
    private readonly panels = new Map<string, ChipPanel>();

    constructor(
        private readonly extensionUri: vscode.Uri,
        private readonly runtime: Runtime,
    ) {}

    /** Opens (or reveals) the simulator for a chip file. */
    async open(uri: vscode.Uri): Promise<ChipPanel> {
        const key = uri.toString();
        const existing = this.panels.get(key);
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
        const chip = new ChipPanel(uri, panel, this.runtime, () => this.panels.delete(key));
        this.panels.set(key, chip);
        await chip.reload();
        return chip;
    }

    get(uri: vscode.Uri): ChipPanel | undefined {
        return this.panels.get(uri.toString());
    }

    dispose(): void {
        for (const panel of [...this.panels.values()]) {
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
.bus-cell { flex: 1 1 auto; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.bus { width: 100%; max-width: 260px; padding: 3px 6px; color: var(--vscode-input-foreground); background: var(--vscode-input-background); border: 1px solid var(--vscode-input-border, var(--vscode-contrastBorder, transparent)); border-radius: 2px; }
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
