// Test scripts in VS Code's Testing view.
//
// Every `.tst` file in the workspace is a test. Running one executes the
// script in the engine and reports what the official tools would: pass, or
// the first compare line that differs. On top of that a failure shows every
// failing row and the value of every pin at that step.

import * as path from 'node:path';
import * as vscode from 'vscode';
import { Runtime, RuntimeError } from './runtime/client';
import {
    Pin,
    Range,
    TestEvent,
    TestFailure,
    TestInspection,
    TestOpened,
    TestOutcome,
    TestStatus,
} from './runtime/protocol';
import { describePin, keyName } from './values';

/** A script is stopped after this many commands. The longest official test
 * runs a few million; a script still going after this is stuck in a loop. */
const MAX_COMMANDS = 200_000_000;

/** Failing rows reported individually; the diff still shows all of them. */
const MAX_FAILURE_MESSAGES = 25;

const EXCLUDE = '**/{node_modules,.git,target,dist}/**';

export function toRange(range: Range): vscode.Range {
    return new vscode.Range(range.startLine, range.startCharacter, range.endLine, range.endCharacter);
}

/** The result of one script, as the rest of the extension sees it. */
export interface ScriptResult {
    uri: vscode.Uri;
    outcome: TestOutcome | { kind: 'error'; message: string; range: null };
    failures: TestFailure[];
    keysPressed: number[];
}

export class TestExplorer implements vscode.Disposable {
    readonly controller: vscode.TestController;
    private readonly disposables: vscode.Disposable[] = [];
    private readonly resultEmitter = new vscode.EventEmitter<ScriptResult>();
    /** Fires after each script finishes. Used by commands and by tests. */
    readonly onDidFinishScript = this.resultEmitter.event;
    private ready: Promise<void> | undefined;

    constructor(
        private readonly runtime: Runtime,
        private readonly startDebugging: (uri: vscode.Uri) => Thenable<boolean>,
    ) {
        this.controller = vscode.tests.createTestController('nandwright', 'Nand2Tetris');
        this.controller.resolveHandler = async (item) => {
            if (!item) {
                await this.discover();
            }
        };
        this.controller.refreshHandler = () => this.rediscover();
        this.controller.createRunProfile(
            'Run',
            vscode.TestRunProfileKind.Run,
            async (request, token) => {
                await this.run(request, token);
            },
            true,
        );
        this.controller.createRunProfile(
            'Step through the script',
            vscode.TestRunProfileKind.Debug,
            (request) => this.debug(request),
            true,
        );

        const watcher = vscode.workspace.createFileSystemWatcher('**/*.tst');
        watcher.onDidCreate((uri) => this.add(uri));
        watcher.onDidDelete((uri) => this.remove(uri));
        this.disposables.push(
            watcher,
            this.resultEmitter,
            vscode.workspace.onDidChangeWorkspaceFolders(() => this.rediscover()),
            vscode.workspace.onDidOpenTextDocument((document) => {
                if (document.uri.scheme === 'file' && document.fileName.toLowerCase().endsWith('.tst')) {
                    this.add(document.uri);
                }
            }),
        );
    }

    /** Finds every script in the workspace. Safe to call repeatedly. */
    discover(): Promise<void> {
        this.ready ??= (async () => {
            const files = await vscode.workspace.findFiles('**/*.tst', EXCLUDE);
            for (const uri of files) {
                this.add(uri);
            }
        })();
        return this.ready;
    }

    private async rediscover(): Promise<void> {
        this.controller.items.replace([]);
        this.ready = undefined;
        await this.discover();
    }

    /** The test item of a script, creating it (and its folders) if needed. */
    add(uri: vscode.Uri): vscode.TestItem {
        const folder = vscode.workspace.getWorkspaceFolder(uri);
        let collection = this.controller.items;
        if (folder) {
            const segments = path.relative(folder.uri.fsPath, path.dirname(uri.fsPath)).split(path.sep).filter(Boolean);
            const roots = vscode.workspace.workspaceFolders ?? [];
            let current = folder.uri;
            if (roots.length > 1) {
                collection = this.group(collection, folder.uri, folder.name);
            }
            for (const segment of segments) {
                current = vscode.Uri.joinPath(current, segment);
                collection = this.group(collection, current, segment);
            }
        }
        const id = uri.toString();
        let item = collection.get(id);
        if (!item) {
            item = this.controller.createTestItem(id, path.basename(uri.fsPath), uri);
            item.range = new vscode.Range(0, 0, 0, 0);
            collection.add(item);
        }
        return item;
    }

    private group(collection: vscode.TestItemCollection, uri: vscode.Uri, label: string): vscode.TestItemCollection {
        const id = `${uri.toString()}/`;
        let item = collection.get(id);
        if (!item) {
            item = this.controller.createTestItem(id, label, uri);
            collection.add(item);
        }
        return item.children;
    }

    private remove(uri: vscode.Uri): void {
        const id = uri.toString();
        const visit = (collection: vscode.TestItemCollection): boolean => {
            if (collection.get(id)) {
                collection.delete(id);
                return true;
            }
            let found = false;
            collection.forEach((item) => {
                if (!found && visit(item.children)) {
                    found = true;
                    if (item.children.size === 0 && item.id.endsWith('/')) {
                        collection.delete(item.id);
                    }
                }
            });
            return found;
        };
        visit(this.controller.items);
    }

    /** The scripts a request selects, in a stable order. */
    private scripts(request: vscode.TestRunRequest): vscode.TestItem[] {
        const excluded = new Set((request.exclude ?? []).map((item) => item.id));
        const scripts: vscode.TestItem[] = [];
        const visit = (item: vscode.TestItem) => {
            if (excluded.has(item.id)) {
                return;
            }
            if (item.id.endsWith('/')) {
                item.children.forEach(visit);
            } else {
                scripts.push(item);
            }
        };
        if (request.include) {
            request.include.forEach(visit);
        } else {
            this.controller.items.forEach(visit);
        }
        return scripts.sort((a, b) => a.id.localeCompare(b.id));
    }

    /** Runs scripts and shows the results in the Testing view. */
    async runScripts(uris: vscode.Uri[], token?: vscode.CancellationToken): Promise<ScriptResult[]> {
        const items = uris.map((uri) => this.add(uri));
        return this.run(new vscode.TestRunRequest(items), token ?? new vscode.CancellationTokenSource().token);
    }

    private async debug(request: vscode.TestRunRequest): Promise<void> {
        const [first] = this.scripts(request);
        if (first?.uri) {
            await this.startDebugging(first.uri);
        }
    }

    private async run(request: vscode.TestRunRequest, token: vscode.CancellationToken): Promise<ScriptResult[]> {
        const run = this.controller.createTestRun(request);
        const results: ScriptResult[] = [];
        const scripts = this.scripts(request);
        scripts.forEach((item) => run.enqueued(item));
        try {
            await this.runtime.syncUnsaved();
            for (const item of scripts) {
                if (token.isCancellationRequested) {
                    run.skipped(item);
                    continue;
                }
                const result = await this.runOne(run, item, token);
                results.push(result);
                this.resultEmitter.fire(result);
            }
        } catch (error) {
            const message = error instanceof Error ? error.message : String(error);
            run.appendOutput(`${message}\r\n`);
            void vscode.window.showErrorMessage(`Nand2Tetris: ${message}`);
        } finally {
            run.end();
        }
        return results;
    }

    private async runOne(
        run: vscode.TestRun,
        item: vscode.TestItem,
        token: vscode.CancellationToken,
    ): Promise<ScriptResult> {
        const uri = item.uri!;
        const name = path.basename(uri.fsPath);
        const say = (text: string) => run.appendOutput(`${text.replace(/\r?\n/g, '\r\n')}\r\n`, undefined, item);
        const started = Date.now();
        run.started(item);

        const errored = (message: string, range: Range | null): ScriptResult => {
            const testMessage = new vscode.TestMessage(message);
            if (range) {
                testMessage.location = new vscode.Location(uri, toRange(range));
            }
            run.errored(item, testMessage, Date.now() - started);
            say(`✗ ${name}: ${message}`);
            return { uri, outcome: { kind: 'error', message, range: null }, failures: [], keysPressed: [] };
        };

        let session: number | undefined;
        try {
            const opened = await this.runtime.call<TestOpened>({
                op: 'test.open',
                path: uri.fsPath,
                crlf: process.platform === 'win32',
                collectAll: true,
                autoKeyboard: vscode.workspace.getConfiguration('nandwright').get('test.pressKeys', true),
                maxCommands: MAX_COMMANDS,
            });
            session = opened.session;
            if (opened.runsForever) {
                const reason =
                    'This script repeats forever by design (it has a `repeat` without a count), so it has no result to report. It is meant to be watched: use "Step Through the Script" to run it.';
                run.skipped(item);
                say(`- ${name}: skipped. ${reason}`);
                return { uri, outcome: { kind: 'skipped', reason }, failures: [], keysPressed: [] };
            }
            const lines: string[] = [];
            const failures: TestFailure[] = [];
            const keysPressed: number[] = [];
            const notes: string[] = [];
            const onEvents = (events: TestEvent[]) => {
                for (const event of events) {
                    if (event.type === 'echo') {
                        say(`  ${name} says: ${event.text}`);
                    } else if (event.type === 'note') {
                        notes.push(event.text);
                        say(`  Note: ${event.text}`);
                    } else if (event.type === 'key') {
                        keysPressed.push(event.code);
                        say(`  Pressed ${keyName(event.code)} for the script, which was waiting for it.`);
                    }
                }
            };
            let status: TestStatus;
            for (;;) {
                status = await this.runtime.runTest(session, {}, onEvents, token);
                for (const event of status.events) {
                    if (event.type === 'output') {
                        lines.push(event.line);
                    }
                }
                failures.push(...status.failures);
                // A `!` pause or a script breakpoint: carry on, as "run" means run.
                if (status.state === 'finished') {
                    break;
                }
            }
            const outcome = status.outcome!;
            const duration = Date.now() - started;
            switch (outcome.kind) {
                case 'passed': {
                    run.passed(item, duration);
                    say(`✓ ${name}: comparison ended successfully (${outcome.comparedLines} lines compared)`);
                    if (outcome.unreachedLines > 0) {
                        say(
                            `  Note: the compare file has ${outcome.unreachedLines} more line(s) that the script never produced. The official tools pass this too.`,
                        );
                    }
                    break;
                }
                case 'unchecked':
                    run.passed(item, duration);
                    say(`✓ ${name}: the script ran to its end. It has no compare-to, so nothing was checked.`);
                    break;
                case 'failed': {
                    const inspection = await this.runtime.call<TestInspection>({ op: 'test.inspect', session });
                    run.failed(item, failureMessages(uri, failures, inspection, notes), duration);
                    say(`✗ ${name}: comparison failure at line ${outcome.line}`);
                    say(`    expected: ${outcome.expected ?? '(the compare file ended here)'}`);
                    say(`    actual:   ${outcome.actual ?? '(the script produced no more lines)'}`);
                    if (failures.length > 1) {
                        say(`    ${failures.length} lines differ in all.`);
                    }
                    say(comparisonTable(inspection.expected ?? [], lines));
                    break;
                }
                case 'error':
                    return errored(outcome.message, outcome.range);
                case 'limit':
                    return errored(
                        `The script was still running after ${outcome.commands.toLocaleString('en-US')} commands, so it was stopped.` +
                            (outcome.echo ? ` Its last message was: "${outcome.echo}".` : '') +
                            ' A script that loops forever or waits for input is meant to be used interactively.',
                        null,
                    );
                case 'cancelled':
                    run.skipped(item);
                    say(`- ${name}: cancelled`);
                    break;
                case 'skipped':
                    run.skipped(item);
                    break;
            }
            return { uri, outcome, failures, keysPressed };
        } catch (error) {
            if (error instanceof RuntimeError) {
                return errored(error.message, null);
            }
            throw error;
        } finally {
            if (session !== undefined) {
                this.runtime.close(session);
            }
        }
    }

    dispose(): void {
        this.controller.dispose();
        this.disposables.forEach((d) => d.dispose());
    }
}

function groupPins(pins: Pin[]): string {
    const groups: [string, Pin['role'][]][] = [
        ['in', ['input']],
        ['out', ['output']],
        ['internal', ['internal']],
        ['registers', ['register']],
        ['memory', ['memory']],
    ];
    return groups
        .map(([label, roles]) => {
            const members = pins.filter((pin) => roles.includes(pin.role));
            return members.length ? `${label}: ${members.map(describePin).join('  ')}` : '';
        })
        .filter(Boolean)
        .join('\n');
}

/**
 * The compare file and the produced output side by side, with a mark on each
 * line that differs: the whole picture, for the Test Results panel.
 */
export function comparisonTable(expected: string[], produced: string[]): string {
    const width = Math.max(8, ...expected.map((line) => line.length));
    const rows = [`      ${'expected'.padEnd(width)}   actual`];
    for (let i = 0; i < Math.max(expected.length, produced.length); i++) {
        const want = expected[i];
        const got = produced[i];
        if (got === undefined) {
            // The script stopped producing output here (or never got this far).
            break;
        }
        const same = want !== undefined && want.length === got.length && [...want].every((c, k) => c === '*' || c === got[k]);
        rows.push(`${String(i + 1).padStart(3)} ${same ? ' ' : '≠'} ${(want ?? '').padEnd(width)}   ${got}`);
    }
    return rows.join('\n');
}

/** The names of the columns in which two rows of an output table differ. */
export function differingColumns(header: string | undefined, expected: string | null, actual: string | null): string[] {
    if (!header || expected === null || actual === null) {
        return [];
    }
    const cells = (row: string) => row.split('|').slice(1, -1);
    const [names, want, got] = [cells(header), cells(expected), cells(actual)];
    if (want.length !== got.length || want.length !== names.length) {
        return [];
    }
    return names.filter((_, i) => want[i] !== got[i]).map((name) => name.trim());
}

/**
 * One message per failing row, at the script line that produced it, with the
 * pin values at that step.
 */
function failureMessages(
    uri: vscode.Uri,
    failures: TestFailure[],
    inspection: TestInspection,
    notes: string[],
): vscode.TestMessage[] {
    const compareName = inspection.comparePath ? path.basename(inspection.comparePath) : 'the compare file';
    const header = inspection.expected?.[0];
    const messages: vscode.TestMessage[] = [];
    for (const failure of failures.slice(0, MAX_FAILURE_MESSAGES)) {
        const columns = differingColumns(header, failure.expected, failure.actual);
        const where = columns.length > 0 ? `: ${columns.join(', ')} ${columns.length === 1 ? 'differs' : 'differ'}` : ' differs';
        const text = [
            ...notes.map((note) => `Note: ${note}\n`),
            `Line ${failure.line} of ${compareName}${where}`,
            '',
            `expected  ${failure.expected ?? '(the compare file ended here)'}`,
            `actual    ${failure.actual ?? '(nothing)'}`,
            '',
            'Values at this step',
            groupPins(failure.state),
        ].join('\n');
        const message = new vscode.TestMessage(text);
        message.location = new vscode.Location(uri, toRange(failure.range));
        messages.push(message);
    }
    return messages;
}
