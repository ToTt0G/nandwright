// The chip simulator panel: an interactive view of one chip, next to its
// source. It reloads as the student types, so the loop is edit, look, edit.

import * as path from 'node:path';
import * as vscode from 'vscode';
import { Runtime } from './runtime/client';
import { ChipState, EngineError, TruthTable, isError } from './runtime/protocol';
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
        panel.webview.html = this.html(panel.webview, name);
        const chip = new ChipPanel(uri, panel, this.runtime, () => this.panels.delete(key));
        this.panels.set(key, chip);
        await chip.reload();
        return chip;
    }

    get(uri: vscode.Uri): ChipPanel | undefined {
        return this.panels.get(uri.toString());
    }

    private html(webview: vscode.Webview, name: string): string {
        const script = webview.asWebviewUri(vscode.Uri.joinPath(this.extensionUri, 'dist', 'webview', 'chip.js'));
        const nonce = Array.from({ length: 32 }, () => Math.floor(Math.random() * 36).toString(36)).join('');
        const csp = [
            "default-src 'none'",
            `style-src 'nonce-${nonce}'`,
            `script-src 'nonce-${nonce}'`,
        ].join('; ');
        return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta http-equiv="Content-Security-Policy" content="${csp}">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>${name} simulator</title>
<style nonce="${nonce}">${STYLE}</style>
</head>
<body>
<main id="root"><p class="hint">Loading the chip…</p></main>
<script nonce="${nonce}" src="${script}"></script>
</body>
</html>`;
    }

    dispose(): void {
        for (const panel of [...this.panels.values()]) {
            panel.panel.dispose();
        }
    }
}

// Everything is drawn with VS Code's own theme variables, so the panel is
// right in light, dark and high-contrast themes without knowing which is on.
const STYLE = `
:root { --gap: 12px; --line: var(--vscode-widget-border, var(--vscode-contrastBorder, var(--vscode-panel-border, transparent))); }
* { box-sizing: border-box; }
body { margin: 0; padding: 0 20px 28px; color: var(--vscode-foreground); font-family: var(--vscode-font-family); font-size: var(--vscode-font-size); line-height: 1.45; }
main { max-width: 640px; }
header { display: flex; align-items: baseline; flex-wrap: wrap; gap: 10px; padding: 18px 0 10px; }
h1 { margin: 0; font-size: 1.5em; font-weight: 600; font-family: var(--vscode-editor-font-family); letter-spacing: -0.01em; }
h2 { margin: 22px 0 6px; font-size: 0.8em; font-weight: 600; text-transform: uppercase; letter-spacing: 0.06em; color: var(--vscode-descriptionForeground); }
.badge { padding: 1px 8px; border-radius: 10px; font-size: 0.85em; color: var(--vscode-badge-foreground); background: var(--vscode-badge-background); border: 1px solid var(--vscode-contrastBorder, transparent); }
.time { margin-left: auto; font-family: var(--vscode-editor-font-family); color: var(--vscode-descriptionForeground); font-variant-numeric: tabular-nums; }
.hint { margin: 2px 0 8px; color: var(--vscode-descriptionForeground); }
.toolbar { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; padding-bottom: 4px; }
.spacer { flex: 1 1 auto; }
button { font: inherit; color: inherit; cursor: pointer; }
button:focus-visible, input:focus-visible, tr:focus-visible { outline: 1px solid var(--vscode-focusBorder); outline-offset: 1px; }
.primary, .secondary { padding: 4px 12px; border-radius: 2px; border: 1px solid var(--vscode-button-border, var(--vscode-contrastBorder, transparent)); }
.primary { color: var(--vscode-button-foreground); background: var(--vscode-button-background); }
.primary:hover { background: var(--vscode-button-hoverBackground); }
.secondary { color: var(--vscode-button-secondaryForeground); background: var(--vscode-button-secondaryBackground); }
.secondary:hover { background: var(--vscode-button-secondaryHoverBackground); }
.segmented { display: inline-flex; border: 1px solid var(--line); border-radius: 3px; overflow: hidden; }
.segmented button { padding: 3px 10px; border: 0; background: transparent; color: var(--vscode-descriptionForeground); }
.segmented button + button { border-left: 1px solid var(--line); }
.segmented button:hover { color: var(--vscode-foreground); background: var(--vscode-toolbar-hoverBackground); }
.segmented .selected { color: var(--vscode-list-activeSelectionForeground, var(--vscode-foreground)); background: var(--vscode-list-activeSelectionBackground); }
.pins { border-top: 1px solid var(--line); }
.pin { display: flex; align-items: center; min-height: 32px; gap: var(--gap); padding: 3px 4px; border-bottom: 1px solid var(--line); }
.pin-name { flex: 0 0 34%; min-width: 0; overflow-wrap: anywhere; font-family: var(--vscode-editor-font-family); }
.pin-width { margin-left: 2px; color: var(--vscode-descriptionForeground); }
.value, .bus, .bit, .truth { font-family: var(--vscode-editor-font-family); font-variant-numeric: tabular-nums; }
.value { white-space: pre-wrap; overflow-wrap: anywhere; }
.bit { display: inline-flex; align-items: center; justify-content: center; min-width: 30px; height: 24px; padding: 0 8px; border-radius: 12px; border: 1px solid var(--vscode-input-border, var(--vscode-contrastBorder, var(--vscode-descriptionForeground))); background: transparent; color: var(--vscode-foreground); }
.bit.on { color: var(--vscode-button-foreground); background: var(--vscode-button-background); border-color: var(--vscode-button-border, var(--vscode-contrastBorder, var(--vscode-button-background))); font-weight: 600; }
button.bit:hover { border-color: var(--vscode-focusBorder); }
.bit.readonly { border-style: dashed; cursor: default; }
.bit.readonly.on { border-style: solid; }
.bus-cell { flex: 1 1 auto; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.bus { width: 100%; max-width: 260px; padding: 3px 6px; color: var(--vscode-input-foreground); background: var(--vscode-input-background); border: 1px solid var(--vscode-input-border, var(--vscode-contrastBorder, transparent)); border-radius: 2px; }
.rejected { color: var(--vscode-errorForeground); font-size: 0.9em; }
.rejected:empty { display: none; }
.problem { margin-top: 16px; padding: 10px 12px; border: 1px solid var(--vscode-inputValidation-errorBorder, var(--vscode-errorForeground)); background: var(--vscode-inputValidation-errorBackground, transparent); border-radius: 3px; }
.problem pre { margin: 6px 0 0; white-space: pre-wrap; overflow-wrap: anywhere; font-family: var(--vscode-editor-font-family); font-size: var(--vscode-editor-font-size); }
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
