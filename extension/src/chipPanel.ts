// The chip simulator panel: an interactive view of one chip, next to its
// source. It reloads as the student types, so the loop is edit, look, edit.

import * as fs from 'node:fs';
import * as path from 'node:path';
import * as vscode from 'vscode';
import { Runtime } from './runtime/client';
import { ChipState, EngineError, Expected, TruthTable, isError } from './runtime/protocol';
import { panelHtml } from './panelHtml';
import { pathKey, samePath } from './paths';
import { parseCompare } from './values';
import type { FromPanel, ToPanel } from './webview/chip';

/** Typing pauses this long before the panel reloads the chip. */
const RELOAD_DELAY_MS = 250;

/** Rows of the compare file shown; the rest are counted, not listed. */
const EXPECTED_ROWS = 256;

/** The text of a file as the student sees it: an open editor's, saved or not, else the disk's. */
async function textOf(file: string): Promise<string | null> {
    const open = vscode.workspace.textDocuments.find((document) => document.uri.scheme === 'file' && samePath(document.uri.fsPath, file));
    if (open) {
        return open.getText();
    }
    try {
        return await fs.promises.readFile(file, 'utf8');
    } catch {
        return null;
    }
}

/**
 * The compare file of a chip: `<Chip>.cmp` beside it, or else the one named
 * by the first `compare-to` of `<Chip>.tst`. Null when there is none.
 */
async function findExpected(chip: vscode.Uri): Promise<Expected | null> {
    const dir = path.dirname(chip.fsPath);
    const stem = path.basename(chip.fsPath, path.extname(chip.fsPath));
    let file = path.join(dir, `${stem}.cmp`);
    let text = await textOf(file);
    if (text === null) {
        const script = await textOf(path.join(dir, `${stem}.tst`));
        // Comments may mention compare-to; only a command counts.
        const named = script?.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/.*$/gm, '').match(/compare-to\s+([^\s,;]+)/)?.[1];
        if (!named) {
            return null;
        }
        file = path.resolve(dir, named);
        text = await textOf(file);
    }
    const parsed = text === null ? null : parseCompare(text, EXPECTED_ROWS);
    return parsed ? { path: file, name: path.basename(file), ...parsed } : null;
}

class ChipPanel {
    private session: number | undefined;
    private state: ChipState | null = null;
    private table: TruthTable | null = null;
    private problem: string | null = null;
    private reloadTimer: NodeJS.Timeout | undefined;
    private expectedTimer: NodeJS.Timeout | undefined;
    private expected: Expected | null = null;
    private watcher: vscode.FileSystemWatcher | undefined;
    private disposed = false;
    /** The chip shown. It changes when the panel follows the editor. */
    private chip: vscode.Uri;
    /** The chip the panel was last asked to show, until the queue gets to it. */
    private wanted: vscode.Uri | undefined;
    private readonly disposables: vscode.Disposable[] = [];
    /** Requests are handled one at a time, in order. */
    private queue: Promise<void> = Promise.resolve();

    constructor(
        uri: vscode.Uri,
        readonly panel: vscode.WebviewPanel,
        private readonly runtime: Runtime,
        private readonly onDispose: () => void,
    ) {
        this.chip = uri;
        panel.webview.onDidReceiveMessage((message: FromPanel) => this.enqueue(() => this.handle(message)), null, this.disposables);
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

    /** The chip file shown, or the one the panel is about to switch to. */
    get target(): vscode.Uri {
        return this.wanted ?? this.chip;
    }

    /**
     * Asks the panel to show another chip. Requests made while one is
     * loading are folded into the last: each queued job takes whatever is
     * wanted when its turn comes, and finds nothing if an earlier job took it.
     */
    showChip(uri: vscode.Uri): Promise<void> {
        this.wanted = uri;
        this.enqueue(() => this.switchChip());
        return this.queue;
    }

    private async switchChip(): Promise<void> {
        const next = this.wanted;
        this.wanted = undefined;
        if (!next || samePath(next.fsPath, this.chip.fsPath)) {
            return;
        }
        clearTimeout(this.reloadTimer);
        if (this.session !== undefined) {
            this.runtime.close(this.session);
            this.session = undefined;
        }
        this.chip = next;
        this.state = null;
        this.table = null;
        this.problem = null;
        this.expected = null;
        this.watchFolder();
        this.panel.title = `${path.basename(next.fsPath, path.extname(next.fsPath))} · simulator`;
        this.post();
        await this.reload();
    }

    private enqueue(work: () => Promise<void>): void {
        this.queue = this.queue.then(() => (this.disposed ? undefined : work())).catch((error) => {
            this.problem = error instanceof Error ? error.message : String(error);
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
        const found = await findExpected(this.chip);
        if (!this.disposed) {
            this.expected = found;
        }
    }

    private scheduleReload(): void {
        clearTimeout(this.reloadTimer);
        this.reloadTimer = setTimeout(() => this.enqueue(() => this.reload()), RELOAD_DELAY_MS);
    }

    private post(): void {
        if (this.disposed) {
            return;
        }
        const message: ToPanel = { type: 'state', state: this.state, table: this.table, problem: this.problem, expected: this.expected };
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

    /**
     * Loads the chip, after anything already asked of the panel. Everything
     * that opens or reloads goes through the queue: two loads at once would
     * open two sessions and lose track of one.
     */
    load(): Promise<void> {
        this.enqueue(() => this.reload());
        return this.queue;
    }

    /** (Re)loads the chip from the editor's current text. */
    private async reload(): Promise<void> {
        await this.runtime.syncUnsaved();
        if (this.session === undefined) {
            const reply = await this.runtime.tryCall<ChipState>({ op: 'chip.open', path: this.chip.fsPath });
            if (this.disposed) {
                // The panel was closed while the chip was loading: the
                // session that has just been made has no owner.
                if (!isError(reply) && reply.session !== undefined) {
                    this.runtime.close(reply.session);
                }
                return;
            }
            if (!isError(reply)) {
                this.session = reply.session;
            }
            await this.accept(reply);
        } else {
            const reply = await this.runtime.tryCall<ChipState>({ op: 'chip.reset', session: this.session });
            if (this.disposed) {
                return;
            }
            if (isError(reply) && /session has ended/.test(reply.error.message)) {
                // The simulator restarted: open the chip again.
                this.session = undefined;
                return this.reload();
            }
            await this.accept(reply);
        }
        await this.readExpected();
        this.post();
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

    private async handle(message: FromPanel): Promise<void> {
        if (message.type === 'ready') {
            if (this.state || this.problem) {
                this.post();
            } else {
                await this.reload();
            }
            return;
        }
        if (message.type === 'openExpected') {
            await this.openExpected();
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
    snapshot(): { state: ChipState | null; table: TruthTable | null; problem: string | null; expected: Expected | null } {
        return { state: this.state, table: this.table, problem: this.problem, expected: this.expected };
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
        this.wanted = undefined;
        clearTimeout(this.reloadTimer);
        clearTimeout(this.expectedTimer);
        this.watcher?.dispose();
        if (this.session !== undefined) {
            this.runtime.close(this.session);
            this.session = undefined;
        }
        this.disposables.forEach((d) => d.dispose());
        this.onDispose();
    }
}

/** Whether the simulator follows the chip being edited (read when it matters). */
function followsEditor(): boolean {
    return vscode.workspace.getConfiguration('nandwright.simulator').get<boolean>('followEditor', true);
}

export class ChipPanels implements vscode.Disposable {
    // A set, not a map keyed by file: a panel can change chip.
    private readonly panels = new Set<ChipPanel>();
    private readonly subscription: vscode.Disposable;

    constructor(
        private readonly extensionUri: vscode.Uri,
        private readonly runtime: Runtime,
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
        const chip: ChipPanel = new ChipPanel(uri, panel, this.runtime, () => this.panels.delete(chip));
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
.bus-cell { flex: 1 1 auto; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.bus { width: 100%; max-width: 260px; padding: 3px 6px; color: var(--vscode-input-foreground); background: var(--vscode-input-background); border: 1px solid var(--vscode-input-border, var(--vscode-contrastBorder, transparent)); border-radius: 2px; }
.expected td, .expected th { font-family: var(--vscode-editor-font-family); }
.expected td.differs { color: var(--vscode-errorForeground); font-weight: 600; }
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
